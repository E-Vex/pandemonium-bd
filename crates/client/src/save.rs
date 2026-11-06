//! The mid-mission save (B-027, ADR-0201 §6/§9 Phase 4): the match-so-far
//! persisted in the user config folder *beside `settings.cfg`* — a single
//! slot, one file, and the file IS a replay record: the ADR's assessment
//! ("a save would persist `(mission id, seed, the fed command log, current
//! tick)` in the user folder") maps one-to-one onto `--record`'s format
//! (the ADR's consequences line: "the mid-mission save reuses the replay
//! record itself"), so the save is written by the same recorder
//! (`report::replay_record`) and the extension is the replay codec's own
//! `.pdrp`.
//!
//! Load discipline (the progress module's law, best effort, never an error
//! the loop sees): a missing file is no save; a file that fails to decode,
//! validate, resolve, or match the content the save ran on surfaces as one
//! line and the run continues without it — no row, no crash, the file left
//! alone (never rewritten to purge). The saved tick is the record's forced
//! final checkpoint ([`MissionSave::saved_tick`]); commands submitted but
//! never fed at save time are inherently absent — the record carries the
//! fed log, exactly the stream `Sim::step` saw.

use std::path::{Path, PathBuf};

use pandemonium_content::ContentTree;
use pandemonium_replay::ReplayFile;

use crate::config;

/// The save file's name inside the config directory (A-215: ADR-0201 §6
/// names the folder — "the user folder", the same per-OS resolution
/// `settings.cfg` owns — and the fields, not the file name; the save is a
/// replay record, so a self-describing name with the codec's extension).
const SAVE_FILE: &str = "mission_save.pdrp";

/// One loaded, resumable save: the decoded record plus the resolved tree
/// index of its mission (what the resume path re-enters) and the mission's
/// display name (the Continue row's value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionSave {
    /// The save's mission, as an index into the tree's mission list.
    pub mission_index: usize,
    /// The mission's display name (ASCII).
    pub mission_name: String,
    /// The save's record: mission id, seed, player setup, the fed command
    /// log, the checkpoint trail, the final hash at the saved tick.
    pub replay: ReplayFile,
}

impl MissionSave {
    /// The tick the match was saved at — the record's forced final
    /// checkpoint. The rehost resumes exactly here.
    pub fn saved_tick(&self) -> u32 {
        self.replay
            .checkpoints
            .last()
            .map(|cp| cp.tick)
            .unwrap_or(0)
    }
}

/// What loading found. `Unresolvable` carries the one-line reason the
/// startup surfaces once (never an error the loop sees).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    /// No file (or persistence disabled) — the default state, no row.
    NoSave,
    /// A file exists but does not resolve against the loaded content:
    /// undecodable, structurally invalid, not a mission record, a mission
    /// the tree no longer carries, or content that changed since the save.
    Unresolvable(String),
    /// A resumable save.
    Ready(MissionSave),
}

/// Resolves a decoded record against the tree (pure over `(tree, replay)`;
/// the disk half is [`load_from`]): the mission id must name a mission the
/// tree carries (membership, `tools replay-verify`'s third comparison, at
/// load time), and the record's content identity must be what that mission
/// bundles today — a save from other content is stale and is never
/// rehosted (feeding a recorded log into a different world is undefined
/// territory the ADR never authorized).
fn resolve(tree: &ContentTree, replay: &ReplayFile) -> Result<MissionSave, String> {
    let Some(mission_id) = replay.mission_id else {
        return Err("the record carries no mission id (not a mission save)".to_string());
    };
    let Some((mission_index, def)) = tree
        .missions
        .iter()
        .enumerate()
        .find(|(_, def)| tree.mission_id(&def.id) == mission_id)
    else {
        return Err(format!(
            "the saved mission (id {mission_id:#018x}) is not in the loaded content"
        ));
    };
    // The content gate: the rehost feeds the recorded commands into the
    // mission's bundled world, so the record's identity must be that
    // world's identity — anything else is a save from different content.
    let bundle = tree
        .bundle_for_mission(&def.id)
        .map_err(|error| format!("could not bundle the saved mission '{}': {error}", def.id))?;
    let world = bundle.world();
    if world.content_hash() != replay.content_hash || world.map_id != replay.map_id {
        return Err(format!(
            "the content changed since the save (record {:#018x}/{:#018x}, today {:#018x}/{:#018x})",
            replay.content_hash,
            replay.map_id,
            world.content_hash(),
            world.map_id
        ));
    }
    Ok(MissionSave {
        mission_index,
        mission_name: def.display_name.clone(),
        replay: replay.clone(),
    })
}

/// Loads the save from an explicit path (the testable seam — [`load`] is
/// the `std::env` wrapper). A missing or unreadable file is `NoSave` — the
/// same no-file law the settings and progress modules follow; a file that
/// exists but fails to decode is `Unresolvable` with the reason.
pub fn load_from(tree: &ContentTree, path: &Path) -> LoadOutcome {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return LoadOutcome::NoSave,
    };
    let replay = match ReplayFile::decode(&bytes) {
        Ok(replay) => replay,
        Err(error) => return LoadOutcome::Unresolvable(format!("decode failed: {error}")),
    };
    match resolve(tree, &replay) {
        Ok(save) => LoadOutcome::Ready(save),
        Err(reason) => LoadOutcome::Unresolvable(reason),
    }
}

/// Writes the save to an explicit path (best effort — the caller surfaces
/// the `Err` as a one-line notice and the run continues). Creates the
/// parent directory when missing (a fresh install has no config dir yet),
/// the progress module's `save_to` law.
pub fn save_to(path: &Path, replay: &ReplayFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, replay.encode())
}

/// The save file's path from the platform's env values (pure —
/// [`save_path`] is the `std::env` wrapper): the config directory with
/// [`SAVE_FILE`] appended, `None` when the machine has no config directory
/// (persistence disabled, A-109's law).
pub fn save_path_from(
    appdata: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
) -> Option<PathBuf> {
    config::config_dir_from(appdata, home, xdg_config_home).map(|dir| dir.join(SAVE_FILE))
}

/// The save file's path on this machine (the `std::env` wrapper over
/// [`save_path_from`]). `None` = persistence disabled.
pub fn save_path() -> Option<PathBuf> {
    save_path_from(
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )
}

/// Loads the save from the machine's config directory when it exists and
/// is readable (`NoSave` otherwise, `Unresolvable` when it cannot load).
pub fn load(tree: &ContentTree) -> LoadOutcome {
    match save_path() {
        Some(path) => load_from(tree, &path),
        None => LoadOutcome::NoSave,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pandemonium_sim::run_command_log_with_mission;
    use pandemonium_sim_api::{
        Command, CommandKind, ControllerKind, EntityId, MatchSetup, PlayerId, PlayerSetup, Vec2Fx,
    };

    /// The repository's content directory (the wiring suites' helper).
    fn repo_content() -> PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content")).to_path_buf()
    }

    fn repo_tree() -> ContentTree {
        ContentTree::load_dir(&repo_content()).expect("the repository content loads")
    }

    /// The Player vs AI setup a mission match runs (the client's
    /// `MatchMode::PlayerVsAi.match_setup` shape, seed 7).
    fn player_vs_ai() -> MatchSetup {
        MatchSetup {
            seed: 7,
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

    /// A real-shaped save record for `crossroads_01`: the app's own
    /// recorder over a genuine 30-tick run (one fed command, the canonical
    /// driver's checkpoint trail) — exactly what the save path writes.
    fn crossroads_record(tree: &ContentTree) -> ReplayFile {
        let (_, def) = tree.mission_by_id("crossroads_01").expect("resolves");
        let bundle = tree
            .bundle_for_mission(&def.id)
            .expect("the mission's map bundles");
        let world = bundle.world();
        let setup = player_vs_ai();
        let commands = vec![Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(8, 8),
            },
        )];
        let run = run_command_log_with_mission(&world, &setup, &commands, 30, None);
        crate::report::replay_record(
            &world,
            &setup,
            &commands,
            &run.checkpoints,
            30,
            run.final_hash,
            Some(tree.mission_id(&def.id)),
        )
    }

    /// A fresh temp path for one test (never the machine's real save file).
    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pandemonium-b027-{name}"));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn round_trip_write_then_load_resolves_the_save() {
        let tree = repo_tree();
        let record = crossroads_record(&tree);
        let path = temp("round-trip.pdrp");
        save_to(&path, &record).expect("the write succeeds");
        match load_from(&tree, &path) {
            LoadOutcome::Ready(save) => {
                let (index, def) = tree.mission_by_id("crossroads_01").expect("resolves");
                assert_eq!(save.mission_index, index);
                assert_eq!(save.mission_name, def.display_name);
                assert_eq!(save.mission_name, "Crossroads");
                assert_eq!(save.saved_tick(), 30);
                assert_eq!(save.replay, record);
            }
            outcome => panic!("the save loads: {outcome:?}"),
        }
    }

    #[test]
    fn a_missing_file_is_no_save() {
        let tree = repo_tree();
        let path = temp("missing.pdrp");
        assert_eq!(load_from(&tree, &path), LoadOutcome::NoSave);
    }

    #[test]
    fn a_corrupt_file_is_unresolvable() {
        let tree = repo_tree();
        let path = temp("corrupt.pdrp");
        std::fs::write(&path, b"not a replay at all").expect("the write succeeds");
        match load_from(&tree, &path) {
            LoadOutcome::Unresolvable(reason) => {
                assert!(reason.contains("decode failed"), "{reason}");
            }
            outcome => panic!("the corrupt file is unresolvable: {outcome:?}"),
        }
    }

    #[test]
    fn a_mission_less_record_is_unresolvable() {
        let tree = repo_tree();
        let mut record = crossroads_record(&tree);
        record.mission_id = None;
        let path = temp("mission-less.pdrp");
        save_to(&path, &record).expect("the write succeeds");
        match load_from(&tree, &path) {
            LoadOutcome::Unresolvable(reason) => {
                assert!(reason.contains("no mission id"), "{reason}");
            }
            outcome => panic!("the mission-less record is unresolvable: {outcome:?}"),
        }
    }

    #[test]
    fn a_record_from_other_content_is_unresolvable() {
        // The membership gate and the content gate: an unknown mission id
        // never resolves; a known mission whose recorded identity is not
        // today's identity is a save from different content — stale, never
        // rehosted.
        let tree = repo_tree();
        // Unknown mission id.
        let mut unknown = crossroads_record(&tree);
        unknown.mission_id = Some(0xDEAD_BEEF_DEAD_BEEF);
        let path = temp("unknown-mission.pdrp");
        save_to(&path, &unknown).expect("the write succeeds");
        match load_from(&tree, &path) {
            LoadOutcome::Unresolvable(reason) => {
                assert!(reason.contains("not in the loaded content"), "{reason}");
            }
            outcome => panic!("the unknown mission is unresolvable: {outcome:?}"),
        }
        // Known mission, changed content.
        let mut stale = crossroads_record(&tree);
        stale.content_hash ^= 1;
        let path = temp("stale-content.pdrp");
        save_to(&path, &stale).expect("the write succeeds");
        match load_from(&tree, &path) {
            LoadOutcome::Unresolvable(reason) => {
                assert!(reason.contains("content changed"), "{reason}");
            }
            outcome => panic!("the stale record is unresolvable: {outcome:?}"),
        }
    }

    #[test]
    fn save_to_creates_the_parent_directory() {
        let tree = repo_tree();
        let record = crossroads_record(&tree);
        let dir = std::env::temp_dir().join("pandemonium-b027-fresh-dir");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("mission_save.pdrp");
        save_to(&path, &record).expect("the write succeeds");
        assert!(path.exists(), "the parent directory was created");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
