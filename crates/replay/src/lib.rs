//! Replay recording and verification (plan §6.5): a match is seed + content hash +
//! ordered command log; checkpoints carry `(tick, state hash)` pairs.
//!
//! The replay file is `{ format_version, content_hash, map_id, seed, player_setup,
//! commands, checkpoints, final_hash }` — encoded and decoded through this crate's
//! own canonical little-endian byte format with a trailing checksum (no serde:
//! the sim-side canonical-encoding rule extends to replays, and the dependency
//! law keeps serde out anyway). The re-simulation itself is orchestrated by
//! `tools` and the acceptance tests, which depend on `sim` — this crate depends
//! only on `fx` and `sim_api` (plan §4).
//!
//! The recorded command log is *exactly* the stream fed to `Sim::step`, tick by
//! tick, including commands the gate rejected: replaying reproduces the same
//! rejections, so the re-simulation converges to the same hashes (A1/A2).

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
#![warn(missing_docs)]

mod bytes;

use pandemonium_fx::{fnv1a64, Fx};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, PlayerId, PlayerSetup, Tick, Vec2Fx,
};

use bytes::{ReadError, Reader, Writer};

/// The magic word that opens every replay file: "PDRP" little-endian.
const MAGIC: u32 = u32::from_le_bytes(*b"PDRP");

/// The format version of this codec. Bump (with a migration or a deliberate
/// break) whenever the encoded field set changes; decode refuses other versions.
pub const FORMAT_VERSION: u32 = 1;

/// One `(tick, state hash)` pair recorded along the match (plan §6.5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Checkpoint {
    /// The tick the hash was taken at.
    pub tick: Tick,
    /// The canonical state hash.
    pub hash: u64,
}

/// A whole recorded match (plan §6.5's field list, verbatim).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReplayFile {
    /// The codec version this file was written with.
    pub format_version: u32,
    /// The canonical content identity the match ran on.
    pub content_hash: u64,
    /// The map the match ran on.
    pub map_id: u64,
    /// The match seed.
    pub seed: u64,
    /// The player setup (controller kinds; plan §6.5).
    pub player_setup: Vec<PlayerSetup>,
    /// The exact command stream fed to `Sim::step`, in feed order.
    pub commands: Vec<Command>,
    /// Checkpoint hashes, strictly ascending by tick.
    pub checkpoints: Vec<Checkpoint>,
    /// The state hash at the final checkpoint's tick (equal to the last
    /// checkpoint's hash by construction; recorded explicitly per the plan).
    pub final_hash: u64,
}

impl ReplayFile {
    /// Encodes the replay into its canonical byte form (with trailing checksum).
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u32(MAGIC);
        w.write_u32(FORMAT_VERSION);
        w.write_u64(self.content_hash);
        w.write_u64(self.map_id);
        w.write_u64(self.seed);

        w.write_u32(self.player_setup.len() as u32);
        for setup in &self.player_setup {
            w.write_u8(setup.player.0);
            w.write_u8(controller_tag(setup.controller));
        }

        w.write_u32(self.commands.len() as u32);
        for cmd in &self.commands {
            encode_command(&mut w, cmd);
        }

        w.write_u32(self.checkpoints.len() as u32);
        for cp in &self.checkpoints {
            w.write_u32(cp.tick);
            w.write_u64(cp.hash);
        }
        w.write_u64(self.final_hash);

        let bytes = w.into_bytes();
        // Trailing integrity checksum over the whole payload.
        let mut out = bytes;
        out.extend_from_slice(&fnv1a64(&out).to_le_bytes());
        out
    }

    /// Decodes and checksum-verifies a replay from its canonical byte form.
    pub fn decode(bytes: &[u8]) -> Result<Self, ReplayError> {
        if bytes.len() < 12 {
            return Err(ReplayError::Truncated);
        }
        let (payload, checksum_bytes) = bytes.split_at(bytes.len() - 8);
        let mut arr = [0u8; 8];
        arr.copy_from_slice(checksum_bytes);
        let checksum = u64::from_le_bytes(arr);
        if fnv1a64(payload) != checksum {
            return Err(ReplayError::ChecksumMismatch);
        }
        let mut r = Reader::new(payload);

        if r.read_u32().map_err(|_| ReplayError::Truncated)? != MAGIC {
            return Err(ReplayError::NotAReplay);
        }
        let format_version = r.read_u32().map_err(|_| ReplayError::Truncated)?;
        if format_version != FORMAT_VERSION {
            return Err(ReplayError::UnsupportedVersion(format_version));
        }
        let content_hash = r.read_u64().map_err(|_| ReplayError::Truncated)?;
        let map_id = r.read_u64().map_err(|_| ReplayError::Truncated)?;
        let seed = r.read_u64().map_err(|_| ReplayError::Truncated)?;

        let player_count = r.read_u32().map_err(|_| ReplayError::Truncated)? as usize;
        let mut player_setup = Vec::with_capacity(player_count);
        for _ in 0..player_count {
            let player = PlayerId(r.read_u8().map_err(|_| ReplayError::Truncated)?);
            let controller = controller_from_tag(r.read_u8().map_err(|_| ReplayError::Truncated)?)?;
            player_setup.push(PlayerSetup { player, controller });
        }

        let command_count = r.read_u32().map_err(|_| ReplayError::Truncated)? as usize;
        let mut commands = Vec::with_capacity(command_count);
        for _ in 0..command_count {
            commands.push(decode_command(&mut r)?);
        }

        let checkpoint_count = r.read_u32().map_err(|_| ReplayError::Truncated)? as usize;
        let mut checkpoints = Vec::with_capacity(checkpoint_count);
        for _ in 0..checkpoint_count {
            let tick = r.read_u32().map_err(|_| ReplayError::Truncated)?;
            let hash = r.read_u64().map_err(|_| ReplayError::Truncated)?;
            checkpoints.push(Checkpoint { tick, hash });
        }
        let final_hash = r.read_u64().map_err(|_| ReplayError::Truncated)?;
        if !r.is_empty() {
            return Err(ReplayError::TrailingBytes);
        }

        let file = Self {
            format_version,
            content_hash,
            map_id,
            seed,
            player_setup,
            commands,
            checkpoints,
            final_hash,
        };
        file.validate()?;
        Ok(file)
    }

    /// Structural validation, independent of any re-simulation: the invariants a
    /// recorder must have produced. `tools replay-verify` runs this *before*
    /// spending ticks re-simulating.
    pub fn validate(&self) -> Result<(), ReplayError> {
        if self.format_version != FORMAT_VERSION {
            return Err(ReplayError::UnsupportedVersion(self.format_version));
        }
        if self.player_setup.is_empty() {
            return Err(ReplayError::Invalid("replay has no players".into()));
        }
        let mut players = self.player_setup.clone();
        players.sort_by_key(|p| p.player);
        if players.windows(2).any(|w| w[0].player == w[1].player) {
            return Err(ReplayError::Invalid("duplicate player slots".into()));
        }
        if players.iter().any(|p| p.player == PlayerId::NEUTRAL) {
            return Err(ReplayError::Invalid(
                "the neutral sentinel cannot control a slot".into(),
            ));
        }
        // Checkpoints: non-empty, strictly ascending, first at tick 0 (the
        // initial state), and the final hash matches the last checkpoint.
        if self.checkpoints.is_empty() {
            return Err(ReplayError::Invalid("replay has no checkpoints".into()));
        }
        if self.checkpoints[0].tick != 0 {
            return Err(ReplayError::Invalid(
                "first checkpoint must record the tick-0 state".into(),
            ));
        }
        if self.checkpoints.windows(2).any(|w| w[0].tick >= w[1].tick) {
            return Err(ReplayError::Invalid(
                "checkpoint ticks must strictly ascend".into(),
            ));
        }
        let last = self.checkpoints[self.checkpoints.len() - 1];
        if last.hash != self.final_hash {
            return Err(ReplayError::Invalid(
                "final hash does not match the last checkpoint".into(),
            ));
        }
        // Commands: chronologically ordered (weakly ascending ticks — many
        // commands can share a tick, in feed order) and never past the end.
        let end = last.tick;
        for window in self.commands.windows(2) {
            if window[0].tick > window[1].tick {
                return Err(ReplayError::Invalid(
                    "commands must be in chronological order".into(),
                ));
            }
        }
        if let Some(cmd) = self.commands.last() {
            if cmd.tick > end {
                return Err(ReplayError::Invalid(format!(
                    "command at tick {} is past the final checkpoint tick {end}",
                    cmd.tick
                )));
            }
        }
        Ok(())
    }
}

/// Typed decode/validate failures (plan §3.2 allows thiserror here).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    /// The byte stream ended before the structure did.
    #[error("replay data is truncated")]
    Truncated,
    /// The trailing checksum does not match the payload — the file is corrupt or
    /// was edited.
    #[error("replay checksum mismatch")]
    ChecksumMismatch,
    /// The magic word is absent — this is not a replay file.
    #[error("not a pandemonium replay file")]
    NotAReplay,
    /// The file's format version is not supported by this codec.
    #[error("unsupported replay format version {0}")]
    UnsupportedVersion(u32),
    /// An unknown controller-kind byte.
    #[error("bad controller kind tag {0}")]
    BadControllerTag(u8),
    /// A structural invariant failed; the string describes which one.
    #[error("invalid replay: {0}")]
    Invalid(String),
    /// Bytes remained after the declared structure was read.
    #[error("trailing bytes after replay structure")]
    TrailingBytes,
    /// A command field failed to decode.
    #[error("invalid command encoding: {0}")]
    BadCommand(&'static str),
}

/// Bridges the reader's static reason strings into the typed error so `?` works.
impl From<ReadError> for ReplayError {
    fn from(reason: ReadError) -> Self {
        ReplayError::BadCommand(reason)
    }
}

fn controller_tag(controller: ControllerKind) -> u8 {
    match controller {
        ControllerKind::Human => 0,
        ControllerKind::Ai => 1,
    }
}

fn controller_from_tag(tag: u8) -> Result<ControllerKind, ReplayError> {
    match tag {
        0 => Ok(ControllerKind::Human),
        1 => Ok(ControllerKind::Ai),
        other => Err(ReplayError::BadControllerTag(other)),
    }
}

/// Command-kind tags in the codec. Adding a `CommandKind` variant forces a new
/// tag here (the encoder and decoder matches are exhaustive).
const TAG_MOVE: u8 = 1;
const TAG_ATTACK: u8 = 2;
const TAG_ATTACK_MOVE: u8 = 3;
const TAG_STOP: u8 = 4;
const TAG_GATHER: u8 = 5;
const TAG_BUILD: u8 = 6;
const TAG_TRAIN: u8 = 7;
const TAG_CANCEL_QUEUE_ITEM: u8 = 8;
const TAG_SET_RALLY: u8 = 9;
const TAG_RESIGN: u8 = 10;

fn encode_command(w: &mut Writer, cmd: &Command) {
    w.write_u8(cmd.issuer.0);
    w.write_u32(cmd.tick);
    w.write_u32(cmd.seq);
    w.write_bool(cmd.queue);
    match &cmd.kind {
        CommandKind::Move { units, target } => {
            w.write_u8(TAG_MOVE);
            encode_units(w, units);
            encode_vec(w, *target);
        }
        CommandKind::Attack { units, target } => {
            w.write_u8(TAG_ATTACK);
            encode_units(w, units);
            w.write_u64(target.0);
        }
        CommandKind::AttackMove { units, target } => {
            w.write_u8(TAG_ATTACK_MOVE);
            encode_units(w, units);
            encode_vec(w, *target);
        }
        CommandKind::Stop { units } => {
            w.write_u8(TAG_STOP);
            encode_units(w, units);
        }
        CommandKind::Gather { units, node } => {
            w.write_u8(TAG_GATHER);
            encode_units(w, units);
            w.write_u64(node.0);
        }
        CommandKind::Build {
            worker,
            structure,
            at,
        } => {
            w.write_u8(TAG_BUILD);
            w.write_u64(worker.0);
            w.write_u32(structure.0);
            w.write_i32(at.x);
            w.write_i32(at.y);
        }
        CommandKind::Train { producer, unit } => {
            w.write_u8(TAG_TRAIN);
            w.write_u64(producer.0);
            w.write_u32(unit.0);
        }
        CommandKind::CancelQueueItem { producer, index } => {
            w.write_u8(TAG_CANCEL_QUEUE_ITEM);
            w.write_u64(producer.0);
            w.write_u16(*index);
        }
        CommandKind::SetRally { producer, target } => {
            w.write_u8(TAG_SET_RALLY);
            w.write_u64(producer.0);
            encode_vec(w, *target);
        }
        CommandKind::Resign {} => {
            w.write_u8(TAG_RESIGN);
        }
    }
}

fn decode_command(r: &mut Reader<'_>) -> Result<Command, ReplayError> {
    let issuer = PlayerId(r.read_u8()?);
    let tick = r.read_u32()?;
    let seq = r.read_u32()?;
    let queue = r.read_bool()?;
    let kind = match r.read_u8()? {
        TAG_MOVE => {
            let units = decode_units(r)?;
            let target = decode_vec(r)?;
            CommandKind::Move { units, target }
        }
        TAG_ATTACK => {
            let units = decode_units(r)?;
            let target = pandemonium_sim_api::EntityId(r.read_u64()?);
            CommandKind::Attack { units, target }
        }
        TAG_ATTACK_MOVE => {
            let units = decode_units(r)?;
            let target = decode_vec(r)?;
            CommandKind::AttackMove { units, target }
        }
        TAG_STOP => {
            let units = decode_units(r)?;
            CommandKind::Stop { units }
        }
        TAG_GATHER => {
            let units = decode_units(r)?;
            let node = pandemonium_sim_api::EntityId(r.read_u64()?);
            CommandKind::Gather { units, node }
        }
        TAG_BUILD => {
            let worker = pandemonium_sim_api::EntityId(r.read_u64()?);
            let structure = pandemonium_sim_api::KindId(r.read_u32()?);
            let x = r.read_i32()?;
            let y = r.read_i32()?;
            CommandKind::Build {
                worker,
                structure,
                at: pandemonium_sim_api::TilePos { x, y },
            }
        }
        TAG_TRAIN => {
            let producer = pandemonium_sim_api::EntityId(r.read_u64()?);
            let unit = pandemonium_sim_api::KindId(r.read_u32()?);
            CommandKind::Train { producer, unit }
        }
        TAG_CANCEL_QUEUE_ITEM => {
            let producer = pandemonium_sim_api::EntityId(r.read_u64()?);
            let index = r.read_u16()?;
            CommandKind::CancelQueueItem { producer, index }
        }
        TAG_SET_RALLY => {
            let producer = pandemonium_sim_api::EntityId(r.read_u64()?);
            let target = decode_vec(r)?;
            CommandKind::SetRally { producer, target }
        }
        TAG_RESIGN => CommandKind::Resign {},
        tag => {
            return Err(ReplayError::Invalid(format!(
                "unknown command kind tag {tag}"
            )));
        }
    };
    Ok(Command {
        issuer,
        tick,
        seq,
        queue,
        kind,
    })
}

fn encode_units(w: &mut Writer, units: &[pandemonium_sim_api::EntityId]) {
    w.write_u32(units.len() as u32);
    for id in units {
        w.write_u64(id.0);
    }
}

fn decode_units(r: &mut Reader<'_>) -> Result<Vec<pandemonium_sim_api::EntityId>, ReplayError> {
    let count = r.read_u32()? as usize;
    let mut units = Vec::with_capacity(count);
    for _ in 0..count {
        units.push(pandemonium_sim_api::EntityId(r.read_u64()?));
    }
    Ok(units)
}

fn encode_vec(w: &mut Writer, v: Vec2Fx) {
    w.write_i32(v.x.raw());
    w.write_i32(v.y.raw());
}

fn decode_vec(r: &mut Reader<'_>) -> Result<Vec2Fx, ReplayError> {
    let x = Fx::from_raw(r.read_i32()?);
    let y = Fx::from_raw(r.read_i32()?);
    Ok(Vec2Fx::new(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_sim_api::{EntityId, KindId, TilePos};

    fn sample_replay() -> ReplayFile {
        ReplayFile {
            format_version: FORMAT_VERSION,
            content_hash: 0xABCD,
            map_id: 7,
            seed: 42,
            player_setup: vec![
                PlayerSetup {
                    player: PlayerId(0),
                    controller: ControllerKind::Human,
                },
                PlayerSetup {
                    player: PlayerId(1),
                    controller: ControllerKind::Ai,
                },
            ],
            commands: vec![
                Command::new(
                    PlayerId(0),
                    0,
                    1,
                    CommandKind::Move {
                        units: vec![EntityId(1), EntityId(2)],
                        target: Vec2Fx::from_ints(3, 4),
                    },
                ),
                Command {
                    issuer: PlayerId(1),
                    tick: 0,
                    seq: 2,
                    queue: true,
                    kind: CommandKind::Attack {
                        units: vec![EntityId(5)],
                        target: EntityId(1),
                    },
                },
                Command::new(
                    PlayerId(0),
                    5,
                    3,
                    CommandKind::Build {
                        worker: EntityId(1),
                        structure: KindId(3),
                        at: TilePos { x: 10, y: 12 },
                    },
                ),
                Command::new(PlayerId(1), 6, 4, CommandKind::Resign {}),
            ],
            checkpoints: vec![
                Checkpoint { tick: 0, hash: 111 },
                Checkpoint {
                    tick: 30,
                    hash: 222,
                },
                Checkpoint {
                    tick: 60,
                    hash: 333,
                },
            ],
            final_hash: 333,
        }
    }

    #[test]
    fn encode_decode_roundtrip_is_lossless() {
        let replay = sample_replay();
        let bytes = replay.encode();
        let decoded = ReplayFile::decode(&bytes).expect("decode must succeed");
        assert_eq!(decoded, replay);
    }

    #[test]
    fn every_command_kind_roundtrips() {
        let commands = vec![
            CommandKind::Move {
                units: vec![],
                target: Vec2Fx::ZERO,
            },
            CommandKind::Attack {
                units: vec![EntityId(9)],
                target: EntityId(8),
            },
            CommandKind::AttackMove {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(-3, 5),
            },
            CommandKind::Stop { units: vec![] },
            CommandKind::Gather {
                units: vec![EntityId(2)],
                node: EntityId(30),
            },
            CommandKind::Build {
                worker: EntityId(2),
                structure: KindId(1),
                at: TilePos { x: -1, y: 2 },
            },
            CommandKind::Train {
                producer: EntityId(4),
                unit: KindId(2),
            },
            CommandKind::CancelQueueItem {
                producer: EntityId(4),
                index: 3,
            },
            CommandKind::SetRally {
                producer: EntityId(4),
                target: Vec2Fx::from_ints(0, 1),
            },
            CommandKind::Resign {},
        ];
        for (i, kind) in commands.into_iter().enumerate() {
            let mut replay = sample_replay();
            replay.commands = vec![Command::new(PlayerId(0), 1, i as u32, kind)];
            replay.checkpoints = vec![
                Checkpoint { tick: 0, hash: 5 },
                Checkpoint { tick: 1, hash: 9 },
            ];
            replay.final_hash = 9;
            let decoded = ReplayFile::decode(&replay.encode()).expect("roundtrip");
            assert_eq!(decoded.commands, replay.commands);
        }
    }

    #[test]
    fn checksum_detects_corruption() {
        let mut bytes = sample_replay().encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert_eq!(
            ReplayFile::decode(&bytes),
            Err(ReplayError::ChecksumMismatch)
        );
    }

    #[test]
    fn truncation_and_magic_are_typed_errors() {
        let bytes = sample_replay().encode();
        // Cutting bytes corrupts the checksum first (the trailing 8 bytes no longer
        // cover the payload they were computed over).
        assert_eq!(
            ReplayFile::decode(&bytes[..bytes.len() - 3]),
            Err(ReplayError::ChecksumMismatch)
        );
        // Too short to even hold magic + version + checksum.
        assert_eq!(ReplayFile::decode(&[0u8; 8]), Err(ReplayError::Truncated));
        // Valid checksum, but not a replay.
        let mut not_replay = vec![0xFF; 16];
        not_replay.extend_from_slice(&fnv1a64(&not_replay).to_le_bytes());
        assert_eq!(
            ReplayFile::decode(&not_replay),
            Err(ReplayError::NotAReplay)
        );
    }

    #[test]
    fn structural_validation_catches_broken_replays() {
        // Final hash mismatch.
        let mut broken = sample_replay();
        broken.final_hash = 999;
        assert!(matches!(broken.validate(), Err(ReplayError::Invalid(_))));
        // Empty checkpoints.
        let mut empty = sample_replay();
        empty.checkpoints = vec![];
        assert!(matches!(empty.validate(), Err(ReplayError::Invalid(_))));
        // Non-ascending checkpoints.
        let mut order = sample_replay();
        order.checkpoints[1] = Checkpoint {
            tick: 60,
            hash: 222,
        };
        assert!(matches!(order.validate(), Err(ReplayError::Invalid(_))));
        // First checkpoint not at tick 0.
        let mut start = sample_replay();
        start.checkpoints[0] = Checkpoint { tick: 3, hash: 111 };
        assert!(matches!(start.validate(), Err(ReplayError::Invalid(_))));
        // Chronologically broken commands.
        let mut chrono = sample_replay();
        chrono.commands[3].tick = 0;
        assert!(matches!(chrono.validate(), Err(ReplayError::Invalid(_))));
        // Duplicate players.
        let mut dupes = sample_replay();
        dupes.player_setup[1].player = PlayerId(0);
        assert!(matches!(dupes.validate(), Err(ReplayError::Invalid(_))));
    }
}
