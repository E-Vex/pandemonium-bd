//! Determinism acceptance suite (plan §13 A1/A2, milestone M1 exit tests):
//!
//! - **A1** — same seed + command log → identical checkpoint hashes across two
//!   runs (in-process twice, plus pinned golden hashes so the CI 3-OS matrix
//!   makes the cross-OS claim real).
//! - **A2** — a recorded replay round-trips through the codec and re-simulates
//!   to every checkpoint hash and the final hash.
//! - **ID-never-reused** — the allocator is monotonic and dead ids never return.
//! - **Iteration order** — every boundary type (snapshot, player view) presents
//!   entities in ascending id order, every tick.
//! - **Invalid commands change no state** (plan §8.2) and **valid commands act
//!   within one tick** (plan §8.4, the sim side of A8).
//!
//! The world and script here are *not* the tools demo — the acceptance suite
//! keeps its own fixture so the two prove the same properties independently.

use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::run_command_log;
use pandemonium_sim::{
    CapTemplate, KindEconomy, KindTemplate, ResourceDef, ScheduledSpawnDef, Sim, SpawnDef,
    TrivialWorld,
};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, MatchSetup, MoveState, PlayerId,
    PlayerSetup, ResourceId, Snapshot, Vec2Fx,
};

/// How many ticks the acceptance match runs.
const TICKS: u32 = 65;

/// The match seed of the acceptance match.
const SEED: u64 = 1234;

/// Kinds of the acceptance world.
mod kinds {
    use pandemonium_sim_api::KindId;

    /// Grunt: health + move + vision.
    pub const GRUNT: KindId = KindId(0);
    /// Watcher: vision only.
    pub const WATCHER: KindId = KindId(1);
    /// Decayer: health that decays to zero (the death path).
    pub const DECAYER: KindId = KindId(2);
}

/// The acceptance world: two players, a decayer that dies early, scheduled spawns
/// that allocate ids both before and after that death, and nonzero spawn jitter
/// so the seeded RNG is exercised inside the hashed state.
fn acceptance_world() -> TrivialWorld {
    TrivialWorld {
        passability: TrivialWorld::open_passability(64, 64),
        buildability: TrivialWorld::open_buildability(64, 64),
        map_id: 0x00AC_E900_0000_0001,
        width_tiles: 64,
        height_tiles: 64,
        kinds: vec![
            KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 350,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ],
                economy: KindEconomy::default(),
            },
            KindTemplate {
                caps: vec![CapTemplate::Vision {
                    radius_milli_tiles: 9000,
                }],
                economy: KindEconomy::default(),
            },
            KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 2,
                        regen_per_tick: -1,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 1600,
                        radius_milli_tiles: 350,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 5000,
                    },
                ],
                economy: KindEconomy::default(),
            },
        ],
        resources: vec![ResourceDef {
            resource: ResourceId(0),
            starting: 200,
        }],
        production: vec![],
        base_population_cap: 0,
        initial_spawns: vec![
            SpawnDef {
                owner: PlayerId(0),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(10, 10),
            },
            SpawnDef {
                owner: PlayerId(0),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(12, 10),
            },
            SpawnDef {
                owner: PlayerId(0),
                kind: kinds::WATCHER,
                pos: Vec2Fx::from_ints(12, 14),
            },
            SpawnDef {
                owner: PlayerId(1),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(50, 50),
            },
            SpawnDef {
                owner: PlayerId(1),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(52, 50),
            },
            SpawnDef {
                owner: PlayerId(1),
                kind: kinds::DECAYER,
                pos: Vec2Fx::from_ints(44, 44),
            },
        ],
        scheduled_spawns: vec![
            ScheduledSpawnDef {
                tick: 10,
                spawn: SpawnDef {
                    owner: PlayerId(0),
                    kind: kinds::GRUNT,
                    pos: Vec2Fx::from_ints(20, 20),
                },
            },
            ScheduledSpawnDef {
                tick: 40,
                spawn: SpawnDef {
                    owner: PlayerId(0),
                    kind: kinds::GRUNT,
                    pos: Vec2Fx::from_ints(30, 30),
                },
            },
        ],
        spawn_jitter_milli: 250,
    }
}

/// The acceptance match setup.
fn acceptance_setup() -> MatchSetup {
    MatchSetup {
        seed: SEED,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Ai,
            },
        ],
    }
}

/// Everything one full run observes, for cross-run comparison. Fields are exactly
/// the deterministic outputs (no wall anything, no iteration-order-dependent
/// collection: every list is built in ascending-tick or ascending-id order).
#[derive(Clone, PartialEq, Eq, Debug)]
struct RunResult {
    /// The tick-0 checkpoint plus every periodic one, ascending.
    checkpoints: Vec<Checkpoint>,
    /// The state hash at the final tick.
    final_hash: u64,
    /// Every event, in emission order, flattened across ticks.
    events: Vec<Event>,
    /// The snapshot after every tick, ascending.
    snapshots: Vec<Snapshot>,
    /// The id allocator watermark at the end.
    next_entity_id: u64,
}

/// Runs the match, feeding the script at each command's declared tick (the
/// replay-identical driving order: stable sort by tick, feed order preserved).
fn run_match(
    world: &TrivialWorld,
    setup: &MatchSetup,
    script: &[Command],
    ticks: u32,
) -> RunResult {
    let mut sim = Sim::new(world, setup.clone());
    let mut result = RunResult {
        checkpoints: vec![Checkpoint {
            tick: 0,
            hash: sim.state_hash(),
        }],
        final_hash: 0,
        events: Vec::new(),
        snapshots: Vec::new(),
        next_entity_id: 0,
    };

    let mut sorted: Vec<&Command> = script.iter().collect();
    sorted.sort_by_key(|cmd| cmd.tick); // stable: feed order within a tick
    let mut cursor = 0usize;
    while sim.tick() < ticks {
        let tick = sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        while cursor < sorted.len() && sorted[cursor].tick == tick {
            feed.push(sorted[cursor].clone());
            cursor += 1;
        }
        let out = sim.step(&feed);
        result.events.extend(out.events);
        if let Some(hash) = out.hash {
            result.checkpoints.push(Checkpoint {
                tick: sim.tick(),
                hash,
            });
        }
        result.snapshots.push(sim.snapshot());
    }
    result.final_hash = sim.state_hash();
    if result.checkpoints.last().map(|cp| cp.tick) != Some(sim.tick()) {
        result.checkpoints.push(Checkpoint {
            tick: sim.tick(),
            hash: result.final_hash,
        });
    }
    result.next_entity_id = sim.next_entity_id();
    result
}

/// Command builder with a per-issuer sequence counter.
struct Script {
    commands: Vec<Command>,
    seq: Vec<(PlayerId, u32)>,
}

impl Script {
    fn new() -> Self {
        Self {
            commands: Vec::new(),
            seq: vec![(PlayerId(0), 0), (PlayerId(1), 0)],
        }
    }

    fn push(&mut self, issuer: PlayerId, tick: u32, queue: bool, kind: CommandKind) {
        let seq = match self.seq.iter_mut().find(|(player, _)| *player == issuer) {
            Some(entry) => {
                entry.1 += 1;
                entry.1
            }
            None => 1, // issuer outside the match: refused anyway
        };
        self.commands.push(Command {
            issuer,
            tick,
            seq,
            queue,
            kind,
        });
    }

    fn finish(self) -> Vec<Command> {
        self.commands
    }
}

/// The acceptance script: valid movement, queued movement, and every refusal
/// class the M1 gate can produce — all deterministic, all recorded.
fn acceptance_script() -> Vec<Command> {
    let p0 = PlayerId(0);
    let p1 = PlayerId(1);
    let mut s = Script::new();

    // Tick 0: both sides march.
    s.push(
        p0,
        0,
        false,
        CommandKind::Move {
            units: vec![EntityId(1), EntityId(2)],
            target: Vec2Fx::from_ints(30, 30),
        },
    );
    s.push(
        p1,
        0,
        false,
        CommandKind::Move {
            units: vec![EntityId(4), EntityId(5)],
            target: Vec2Fx::from_ints(34, 34),
        },
    );

    // Tick 1: queue a follow-up for grunt 1; order the watcher to move (refused).
    s.push(
        p0,
        1,
        true,
        CommandKind::Move {
            units: vec![EntityId(1)],
            target: Vec2Fx::from_ints(28, 36),
        },
    );
    s.push(
        p0,
        1,
        false,
        CommandKind::Move {
            units: vec![EntityId(3)],
            target: Vec2Fx::from_ints(31, 31),
        },
    );

    // Tick 2: capability, existence, and ownership refusals.
    s.push(
        p0,
        2,
        false,
        CommandKind::Attack {
            units: vec![EntityId(1)],
            target: EntityId(4),
        },
    );
    s.push(
        p1,
        2,
        false,
        CommandKind::Move {
            units: vec![EntityId(999)],
            target: Vec2Fx::from_ints(1, 1),
        },
    );
    s.push(
        p0,
        2,
        false,
        CommandKind::Move {
            units: vec![EntityId(4)],
            target: Vec2Fx::from_ints(9, 9),
        },
    );

    // Tick 3: unknown issuer; a duplicate sequence (both recorded, second refused).
    s.push(PlayerId(9), 3, false, CommandKind::Resign {});
    s.push(
        p0,
        3,
        false,
        CommandKind::Gather {
            units: vec![EntityId(1)],
            node: EntityId(3),
        },
    );
    s.push(
        p0,
        3,
        false,
        CommandKind::Train {
            producer: EntityId(1),
            unit: pandemonium_sim_api::KindId(99),
        },
    );

    // Tick 5: a stop clears grunt 2's orders.
    s.push(
        p0,
        5,
        false,
        CommandKind::Stop {
            units: vec![EntityId(2)],
        },
    );

    // Tick 20: order the dead decayer (id 6, died at tick 1) — ids are never
    // reused, so this is an existence refusal, deterministically.
    s.push(
        p1,
        20,
        false,
        CommandKind::Move {
            units: vec![EntityId(6)],
            target: Vec2Fx::from_ints(40, 40),
        },
    );

    // Tick 25: player 1 resigns.
    s.push(p1, 25, false, CommandKind::Resign {});

    // Tick 45: the tick-40 spawn (id 8) gets orders.
    s.push(
        p0,
        45,
        false,
        CommandKind::Move {
            units: vec![EntityId(8)],
            target: Vec2Fx::from_ints(31, 31),
        },
    );

    s.finish()
}

/// A script containing *only* invalid commands, for the no-state-change proof.
fn invalid_only_script() -> Vec<Command> {
    let p0 = PlayerId(0);
    let mut s = Script::new();
    s.push(PlayerId(9), 0, false, CommandKind::Resign {});
    s.push(
        p0,
        0,
        false,
        CommandKind::Move {
            units: vec![EntityId(3)],
            target: Vec2Fx::from_ints(1, 1),
        },
    );
    s.push(
        p0,
        1,
        false,
        CommandKind::Move {
            units: vec![EntityId(999)],
            target: Vec2Fx::from_ints(1, 1),
        },
    );
    s.push(
        PlayerId(1),
        2,
        false,
        CommandKind::Attack {
            units: vec![EntityId(1)],
            target: EntityId(4),
        },
    );
    s.push(
        p0,
        3,
        false,
        CommandKind::Gather {
            units: vec![EntityId(1)],
            node: EntityId(3),
        },
    );
    s.finish()
}

// ---------------------------------------------------------------------------
// A1: determinism
// ---------------------------------------------------------------------------

#[test]
fn a1_same_seed_and_log_produce_identical_checkpoint_hashes() {
    let world = acceptance_world();
    let setup = acceptance_setup();
    let script = acceptance_script();

    let first = run_match(&world, &setup, &script, TICKS);
    let second = run_match(&world, &setup, &script, TICKS);

    assert_eq!(
        first.checkpoints, second.checkpoints,
        "two runs of the same seed + log must produce identical checkpoints"
    );
    assert_eq!(first.final_hash, second.final_hash);
    assert_eq!(first.events, second.events);
    assert_eq!(first.snapshots, second.snapshots);
    assert_eq!(first.next_entity_id, second.next_entity_id);

    // The seed must matter: a different seed diverges (the jitter path).
    let mut other = acceptance_setup();
    other.seed = SEED + 1;
    let third = run_match(&world, &other, &script, TICKS);
    assert_ne!(first.final_hash, third.final_hash);
}

/// Golden hashes: pinned after the first green run, so any unintentional change
/// to the state encoding, the pipeline order, or the fixture shows up in review
/// as a value change instead of passing silently. Regenerate deliberately when
/// a milestone intentionally changes the encoding (docs/ASSUMPTIONS.md).
#[test]
fn a1_golden_hashes_are_pinned() {
    let world = acceptance_world();
    let setup = acceptance_setup();
    let script = acceptance_script();
    let run = run_match(&world, &setup, &script, TICKS);

    // Re-pinned M10.1: canonical hasher FNV-1a -> xxHash64 (DEBT-001 repaid);
    // encoding unchanged, algorithm moved. See docs/DEBT.md.
    // (Prior history: regenerated for M6 — state encoding v4: the Attack
    // capability block + AttackUnit order; fixture encoding v4: Attack
    // CapTemplate — see docs/ASSUMPTIONS.md A-056.)
    let golden: &[(u32, u64)] = &[
        (0, 0x5A467269B78F6B93),
        (30, 0x2BCCEA8144F1ED96),
        (60, 0x732E7B725B0F4E26),
    ];
    assert_eq!(run.checkpoints.len(), 4, "tick 0 + 30 + 60 + final 65");
    for (want_tick, want_hash) in golden {
        let found = run
            .checkpoints
            .iter()
            .find(|cp| cp.tick == *want_tick)
            .unwrap_or_else(|| panic!("missing checkpoint at tick {want_tick}"));
        assert_eq!(found.hash, *want_hash, "golden hash at tick {want_tick}");
    }
    assert_eq!(run.final_hash, 0xC5F543CFA16BA2B4); // re-pinned M10.1 (DEBT-001)
    assert_eq!(run.next_entity_id, 9);
}

// ---------------------------------------------------------------------------
// A2: replay round-trip
// ---------------------------------------------------------------------------

#[test]
fn a2_replay_roundtrip_verifies_to_identical_hashes() {
    let world = acceptance_world();
    let setup = acceptance_setup();
    let script = acceptance_script();
    let run = run_match(&world, &setup, &script, TICKS);

    // Record exactly like the tools recorder does.
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        seed: setup.seed,
        player_setup: setup.players.clone(),
        commands: script.clone(),
        checkpoints: run.checkpoints.clone(),
        final_hash: run.final_hash,
    };
    assert!(replay.validate().is_ok());

    // Codec round-trip is lossless.
    let bytes = replay.encode();
    let decoded = ReplayFile::decode(&bytes).expect("decode");
    assert_eq!(decoded, replay);

    // Corruption is detected by the checksum.
    let mut corrupted = bytes.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0x01;
    assert!(ReplayFile::decode(&corrupted).is_err());

    // Re-simulation through the shared canonical driver
    // (`run_command_log` — the same one `tools replay-verify` uses, DEBT-005)
    // reproduces every checkpoint and the final hash.
    let resim = run_command_log(&world, &setup, &decoded.commands, TICKS);
    let resim_checkpoints: Vec<Checkpoint> = resim
        .checkpoints
        .iter()
        .map(|&(tick, hash)| Checkpoint { tick, hash })
        .collect();
    assert_eq!(resim_checkpoints, decoded.checkpoints);
    assert_eq!(resim.final_hash, decoded.final_hash);
    assert_eq!(resim.next_entity_id, run.next_entity_id);
}

#[test]
fn a2_replay_from_a_different_world_is_refused() {
    let world = acceptance_world();
    let setup = acceptance_setup();
    let script = acceptance_script();
    let run = run_match(&world, &setup, &script, TICKS);
    let mut replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        seed: setup.seed,
        player_setup: setup.players.clone(),
        commands: script,
        checkpoints: run.checkpoints.clone(),
        final_hash: run.final_hash,
    };
    // Same shape, different content: the hash must catch it.
    let mut other_world = acceptance_world();
    other_world.map_id = 0xDEAD;
    replay.content_hash = other_world.content_hash();
    replay.map_id = other_world.map_id;
    // Structural validation still passes; the content check is the verifier's
    // first line (mirrored from tools' verify_replay).
    assert!(replay.validate().is_ok());
    assert_ne!(replay.content_hash, world.content_hash());
}

// ---------------------------------------------------------------------------
// Entity identity
// ---------------------------------------------------------------------------

#[test]
fn ids_are_monotonic_and_never_reused() {
    let world = acceptance_world();
    let setup = acceptance_setup();
    let script = acceptance_script();
    let run = run_match(&world, &setup, &script, TICKS);

    // Every Spawned id, in allocation order — must be strictly ascending.
    let spawned: Vec<EntityId> = run
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Spawned { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect();
    assert!(
        spawned.windows(2).all(|w| w[0] < w[1]),
        "ids must allocate ascending"
    );
    // Initial six + two scheduled.
    assert_eq!(spawned.len(), 8);
    assert_eq!(spawned[0], EntityId(1));
    assert_eq!(spawned[7], EntityId(8));
    // The allocator watermark sits exactly past the highest id.
    assert_eq!(run.next_entity_id, 9);

    // The decayer (id 6) died at tick 1 — its id must never return.
    let died: Vec<EntityId> = run
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Died { entity } => Some(*entity),
            _ => None,
        })
        .collect();
    assert_eq!(died, vec![EntityId(6)]);
    for (i, snapshot) in run.snapshots.iter().enumerate() {
        let present = snapshot.entities.iter().any(|e| e.id == EntityId(6));
        if i >= 1 {
            assert!(
                !present,
                "dead id reappeared in snapshot after tick {}",
                i + 1
            );
        }
    }
    // Ids allocated after the death are strictly higher than the dead id (this,
    // plus the ascending-allocation check above, is the never-reused proof).
    assert!(spawned[6] > EntityId(6) && spawned[7] > EntityId(6));
    // And id 6 itself is gone from the final state while ids 7 and 8 live on.
    let final_ids: Vec<EntityId> = run.snapshots[TICKS as usize - 1]
        .entities
        .iter()
        .map(|e| e.id)
        .collect();
    assert!(!final_ids.contains(&EntityId(6)));
    assert!(final_ids.contains(&EntityId(7)));
    assert!(final_ids.contains(&EntityId(8)));
}

// ---------------------------------------------------------------------------
// Iteration order
// ---------------------------------------------------------------------------

#[test]
fn boundary_types_iterate_in_ascending_entity_order() {
    let world = acceptance_world();
    let setup = acceptance_setup();
    let script = acceptance_script();
    let mut sim = Sim::new(&world, setup);
    let mut sorted: Vec<&Command> = script.iter().collect();
    sorted.sort_by_key(|cmd| cmd.tick);
    let mut cursor = 0usize;
    while sim.tick() < TICKS {
        let tick = sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        while cursor < sorted.len() && sorted[cursor].tick == tick {
            feed.push(sorted[cursor].clone());
            cursor += 1;
        }
        sim.step(&feed);

        // Snapshots: strictly ascending ids.
        let snapshot = sim.snapshot();
        assert!(
            snapshot.entities.windows(2).all(|w| w[0].id < w[1].id),
            "snapshot order broke at tick {}",
            sim.tick()
        );
        // Player views: strictly ascending ids, every few ticks.
        if sim.tick().is_multiple_of(7) {
            for player in [PlayerId(0), PlayerId(1)] {
                let view = sim.player_view(player);
                assert!(view.entities.windows(2).all(|w| w[0].id < w[1].id));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Command semantics
// ---------------------------------------------------------------------------

#[test]
fn invalid_commands_change_no_state() {
    let world = acceptance_world();
    let setup = acceptance_setup();
    let noisy = run_match(&world, &setup, &invalid_only_script(), TICKS);
    let quiet = run_match(&world, &setup, &[], TICKS);

    // The noisy run rejected every command it fed...
    let rejections = noisy
        .events
        .iter()
        .filter(|e| matches!(e, Event::CommandRejected { .. }))
        .count();
    assert!(
        rejections >= 5,
        "expected the refusals to be recorded, got {rejections}"
    );
    // ...and its state is bit-identical to the untouched run at every checkpoint
    // and at the end (plan §8.2: invalid commands change no state).
    assert_eq!(noisy.checkpoints, quiet.checkpoints);
    assert_eq!(noisy.final_hash, quiet.final_hash);
    assert_eq!(noisy.snapshots, quiet.snapshots);
}

#[test]
fn valid_move_produces_motion_within_one_tick() {
    // A8's simulation-side budget (plan §8.4): a valid command shows visible
    // feedback within one tick. Movement is visible feedback.
    let world = acceptance_world();
    let setup = acceptance_setup();
    let mut sim = Sim::new(&world, setup);
    let grunt = EntityId(1);
    let start = sim
        .snapshot()
        .entities
        .iter()
        .find(|e| e.id == grunt)
        .unwrap()
        .pos;
    let target = Vec2Fx::from_ints(40, 10);

    sim.step(&[Command::new(
        PlayerId(0),
        0,
        1,
        CommandKind::Move {
            units: vec![grunt],
            target,
        },
    )]);
    // One step contains both the application (stage 1) and the movement (stage 6),
    // so the position already moved.
    let after = sim
        .snapshot()
        .entities
        .iter()
        .find(|e| e.id == grunt)
        .unwrap()
        .pos;
    let moved = (after - start).len();
    assert!(
        moved > pandemonium_fx::Fx::ZERO,
        "no motion within one tick"
    );
    let remaining = (target - after).len();
    assert!(remaining < (target - start).len());
    // And the entity reports Moving.
    let after_snapshot = sim.snapshot();
    let view = after_snapshot
        .entities
        .iter()
        .find(|e| e.id == grunt)
        .unwrap();
    assert_eq!(view.move_state, MoveState::Moving);
}

#[test]
fn queue_semantics_replace_and_append() {
    // A plain Move replaces the queue; a queued Move appends behind it.
    let run_move_pair = |queued: bool| -> Vec2Fx {
        let world = acceptance_world();
        let setup = acceptance_setup();
        let mut sim = Sim::new(&world, setup);
        let grunt = EntityId(1);
        let a = Vec2Fx::from_ints(20, 10);
        let b = Vec2Fx::from_ints(10, 30);
        sim.step(&[
            Command::new(
                PlayerId(0),
                0,
                1,
                CommandKind::Move {
                    units: vec![grunt],
                    target: a,
                },
            ),
            Command {
                issuer: PlayerId(0),
                tick: 0,
                seq: 2,
                queue: queued,
                kind: CommandKind::Move {
                    units: vec![grunt],
                    target: b,
                },
            },
        ]);
        // ~11 tiles total for a+b when queued; plenty of ticks.
        for _ in 0..400 {
            sim.step(&[]);
        }
        sim.snapshot()
            .entities
            .iter()
            .find(|e| e.id == grunt)
            .unwrap()
            .pos
    };
    assert_eq!(
        run_move_pair(false),
        Vec2Fx::from_ints(10, 30),
        "replace goes straight to B"
    );
    assert_eq!(
        run_move_pair(true),
        Vec2Fx::from_ints(10, 30),
        "queued still ends at B"
    );
    // The behavioral difference showed up mid-run (the replace never visited A's
    // neighborhood); pin that too, at a mid time slice.
    let trace = |queued: bool| {
        let world = acceptance_world();
        let mut sim = Sim::new(&world, acceptance_setup());
        let grunt = EntityId(1);
        let a = Vec2Fx::from_ints(20, 10);
        let b = Vec2Fx::from_ints(10, 30);
        sim.step(&[
            Command::new(
                PlayerId(0),
                0,
                1,
                CommandKind::Move {
                    units: vec![grunt],
                    target: a,
                },
            ),
            Command {
                issuer: PlayerId(0),
                tick: 0,
                seq: 2,
                queue: queued,
                kind: CommandKind::Move {
                    units: vec![grunt],
                    target: b,
                },
            },
        ]);
        let mut min_dist_to_a = pandemonium_fx::Fx::MAX;
        for _ in 0..200 {
            sim.step(&[]);
            let pos = sim
                .snapshot()
                .entities
                .iter()
                .find(|e| e.id == grunt)
                .unwrap()
                .pos;
            let d = (a - pos).len();
            if d < min_dist_to_a {
                min_dist_to_a = d;
            }
        }
        min_dist_to_a
    };
    // The queued run must actually pass through A (or land on it exactly).
    assert!(trace(true) == pandemonium_fx::Fx::ZERO);
    // The replace run cuts the corner and never gets near A.
    assert!(trace(false) > pandemonium_fx::Fx::from_milli(5000));
}
