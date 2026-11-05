//! The campaign progress file (B-026, ADR-0201 §6/§9 Phase 3): per-user
//! state recording which campaign missions were **completed** or
//! **skipped** — one `key=value` file in the user config folder *beside*
//! `settings.cfg`, on the same per-OS directory resolution
//! ([`crate::config::config_dir`]) and the same best-effort write
//! discipline (`config.rs`'s save: create the directory when missing,
//! surface a one-line notice on failure, never crash the loop).
//!
//! ADR-0201 §6's format law, verbatim: "a small hand-rolled file in the
//! `key=value` spirit of the settings format — one line per mission
//! (`<mission-id>=completed|skipped`), per-key salvage, unknown ids
//! ignored." Per-key salvage means one malformed line costs nothing but
//! itself; unknown ids (a mission removed from the tree, a friend's file)
//! parse fine and simply never resolve against the campaign — the
//! resolution layer ignores them, the file is not rewritten to purge them.
//!
//! Like the settings module this is pure data-in/data-out: parsing and
//! serialization are plain functions over `&str`, the file paths are
//! parameters (`load_from`/`save_to`), and the `std::env` wrappers are
//! thin. The progress is per-user convenience, never content (ADR-0201
//! §7): it rides outside every hash, is never synced, and never
//! exchanges between peers. A `BTreeMap` keeps the serialized order
//! deterministic (sorted by mission id) — the ordered-map discipline the
//! content loaders already follow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config;

/// The progress file's name inside the config directory (A-210: ADR-0201
/// §6 names the folder — "beside `settings.cfg`" — and the format, not
/// the file name; this one is self-describing and matches the settings
/// suffix).
const PROGRESS_FILE: &str = "campaign_progress.cfg";

/// One mission's recorded fate (ADR-0201 §6: the two are distinct — the
/// end screen and the progress file distinguish them; a skip unlocks the
/// next mission exactly like a completion does).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionRecord {
    /// The mission was won (the end screen's VICTORY on a mission match).
    Completed,
    /// The mission was skipped (the Skip affordance — the campaign's
    /// skippability requirement; a skipped mission stays replayable, and
    /// a later victory upgrades the record to `Completed`).
    Skipped,
}

impl MissionRecord {
    /// The record's value in the file format (ASCII).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Skipped => "skipped",
        }
    }

    /// Parses a file value; anything else is malformed (per-key salvage:
    /// the line costs nothing).
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "completed" => Some(Self::Completed),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

/// The per-user campaign progress: mission id → record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CampaignProgress {
    /// The recorded missions, sorted by id (the serialization order).
    entries: BTreeMap<String, MissionRecord>,
}

impl CampaignProgress {
    /// The record for a mission id (`None` = not yet completed or skipped).
    pub fn record_for(&self, mission_id: &str) -> Option<MissionRecord> {
        self.entries.get(mission_id).copied()
    }

    /// Records a mission's fate (upsert). Returns whether the recorded
    /// state **changed** — an idempotent re-record (winning an already
    /// completed mission again) returns `false` so the caller can skip
    /// the disk write; a skip upgraded by a later victory returns `true`.
    pub fn record(&mut self, mission_id: &str, record: MissionRecord) -> bool {
        match self.entries.get(mission_id) {
            Some(current) if *current == record => false,
            _ => {
                self.entries.insert(mission_id.to_string(), record);
                true
            }
        }
    }

    /// Serializes to the file format: one `<mission-id>=<record>` per
    /// line in mission-id order (deterministic — two equal progress maps
    /// serialize to equal bytes), with a leading comment line.
    pub fn to_text(&self) -> String {
        let mut out = String::from("# pandemonium campaign progress - key=value lines\n");
        for (id, record) in &self.entries {
            out.push_str(&format!("{id}={}\n", record.as_str()));
        }
        out
    }

    /// Parses a file body. Per-key salvage: a line whose id or value is
    /// malformed costs that line nothing else; blank and `#`-comment
    /// lines are ignored; an id that appears twice keeps the last
    /// parseable value (the settings module's repeated-key law). Ids the
    /// campaign does not know are kept — they are ignored at resolution
    /// time, never at parse time (the module cannot know the tree, and a
    /// friend's file must not lose lines it does not recognize).
    pub fn parse(text: &str) -> Self {
        let mut entries = BTreeMap::new();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((id, value)) = line.split_once('=') else {
                continue; // not key=value — ignored
            };
            let (id, value) = (id.trim(), value.trim());
            if id.is_empty() {
                continue;
            }
            if let Some(record) = MissionRecord::parse(value) {
                entries.insert(id.to_string(), record);
            }
        }
        Self { entries }
    }

    /// The progress file's path from the platform's env values (pure —
    /// [`progress_path`] is the `std::env` wrapper): the config directory
    /// with [`PROGRESS_FILE`] appended, `None` when the machine has no
    /// config directory (persistence disabled, A-109's law).
    pub fn path_from(
        appdata: Option<PathBuf>,
        home: Option<PathBuf>,
        xdg_config_home: Option<PathBuf>,
    ) -> Option<PathBuf> {
        config::config_dir_from(appdata, home, xdg_config_home).map(|dir| dir.join(PROGRESS_FILE))
    }

    /// Loads from an explicit path (the testable seam — [`load`] is the
    /// `std::env` wrapper). A missing or unreadable file is the empty
    /// progress, never an error (the settings module's law: a permissions
    /// problem is the no-file case).
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Saves to an explicit path (best effort — the caller surfaces the
    /// `Err` as a one-line notice and the run continues on the in-memory
    /// values). Creates the parent directory when missing (a fresh
    /// install has no config dir yet).
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, self.to_text())
    }
}

/// The progress file's path on this machine (the `std::env` wrapper over
/// [`CampaignProgress::path_from`] — the settings module's `config_path`
/// pattern, env read once in the wrapper, resolution pure and tested).
/// `None` = persistence disabled.
pub fn progress_path() -> Option<PathBuf> {
    CampaignProgress::path_from(
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )
}

/// Loads the progress from the machine's config directory when it exists
/// and is readable, else the empty progress.
pub fn load() -> CampaignProgress {
    match progress_path() {
        Some(path) => CampaignProgress::load_from(&path),
        None => CampaignProgress::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_progress_serializes_to_the_header_only() {
        let text = CampaignProgress::default().to_text();
        assert!(text.starts_with('#'));
        assert_eq!(text.lines().count(), 1);
        assert_eq!(CampaignProgress::parse(&text), CampaignProgress::default());
    }

    #[test]
    fn round_trip_save_then_load_equals_the_same_progress() {
        let mut progress = CampaignProgress::default();
        assert!(progress.record("crossroads_01", MissionRecord::Completed));
        assert!(progress.record("crossroads_02", MissionRecord::Skipped));
        assert!(progress.record("crossroads_01_challenge", MissionRecord::Completed));
        let parsed = CampaignProgress::parse(&progress.to_text());
        assert_eq!(parsed, progress);
        assert_eq!(
            parsed.record_for("crossroads_01"),
            Some(MissionRecord::Completed)
        );
        assert_eq!(
            parsed.record_for("crossroads_02"),
            Some(MissionRecord::Skipped)
        );
        assert_eq!(parsed.record_for("crossroads_03"), None);
    }

    #[test]
    fn the_serialized_form_is_deterministic_and_ascii() {
        let mut progress = CampaignProgress::default();
        progress.record("zebra_mission", MissionRecord::Skipped);
        progress.record("alpha_mission", MissionRecord::Completed);
        let text = progress.to_text();
        assert!(text.is_ascii(), "the embedded font is an ASCII subset (T8)");
        assert_eq!(text, progress.to_text());
        // BTreeMap order: the ids serialize sorted, not in record order.
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(
            lines,
            vec!["alpha_mission=completed", "zebra_mission=skipped"]
        );
    }

    #[test]
    fn a_missing_file_is_the_empty_progress() {
        assert_eq!(CampaignProgress::parse(""), CampaignProgress::default());
        assert_eq!(
            CampaignProgress::parse("\n\n   \n"),
            CampaignProgress::default()
        );
    }

    #[test]
    fn a_wholly_garbage_file_is_the_empty_progress() {
        let parsed = CampaignProgress::parse(
            "this is not a progress file\n\x00\x01\nkey without value\n====\n=completed\n",
        );
        assert_eq!(parsed, CampaignProgress::default());
    }

    #[test]
    fn a_malformed_value_costs_only_its_own_line() {
        // Per-key salvage: the bad values cost their lines, the good line
        // survives, and ids the parser does not know are simply data.
        let parsed = CampaignProgress::parse(
            "# a comment\ncrossroads_01=completed\ncrossroads_02=maybe\n\
             crossroads_03=\nfuture_mission=won\n\nanother=skipped extra\n",
        );
        assert_eq!(
            parsed.record_for("crossroads_01"),
            Some(MissionRecord::Completed)
        );
        assert_eq!(parsed.record_for("crossroads_02"), None);
        assert_eq!(parsed.record_for("crossroads_03"), None);
        assert_eq!(parsed.record_for("future_mission"), None);
        assert_eq!(
            parsed.record_for("another"),
            None,
            "a trailing token is malformed"
        );
    }

    #[test]
    fn a_repeated_id_keeps_the_last_parseable_value() {
        let parsed = CampaignProgress::parse(
            "crossroads_01=skipped\ncrossroads_01=completed\ncrossroads_01=garbage\n",
        );
        assert_eq!(
            parsed.record_for("crossroads_01"),
            Some(MissionRecord::Completed)
        );
    }

    #[test]
    fn unknown_ids_parse_and_survive_a_round_trip() {
        // A friend's file (or a removed mission) keeps its lines: the
        // module ignores unknown ids at resolution time, never at parse
        // time — a load-save cycle must not purge them.
        let text = "# progress\nold_mission=skipped\ncrossroads_01=completed\n";
        let parsed = CampaignProgress::parse(text);
        assert_eq!(
            parsed.record_for("old_mission"),
            Some(MissionRecord::Skipped)
        );
        let round_trip = CampaignProgress::parse(&parsed.to_text());
        assert_eq!(round_trip, parsed, "no line is lost to a load-save cycle");
    }

    #[test]
    fn recording_is_an_upsert_with_a_change_signal() {
        let mut progress = CampaignProgress::default();
        // A first record changes the state...
        assert!(progress.record("crossroads_01", MissionRecord::Skipped));
        // ...an identical re-record does not (the caller skips the write)...
        assert!(!progress.record("crossroads_01", MissionRecord::Skipped));
        // ...and a skip upgraded by a later victory does (the ADR's
        // "a skipped mission stays replayable" — winning it counts).
        assert!(progress.record("crossroads_01", MissionRecord::Completed));
        assert_eq!(
            progress.record_for("crossroads_01"),
            Some(MissionRecord::Completed)
        );
        // A completed mission re-skipped changes back.
        assert!(progress.record("crossroads_01", MissionRecord::Skipped));
    }

    #[test]
    fn the_progress_path_shares_the_config_directory_with_settings() {
        use std::path::Path;
        // The ADR-0201 §6 law: beside settings.cfg, the same resolution.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let env = (
                None,
                Some(Path::new("/home/tester").to_path_buf()),
                Some(Path::new("/xdg").to_path_buf()),
            );
            assert_eq!(
                CampaignProgress::path_from(env.0.clone(), env.1.clone(), env.2.clone()),
                Some(Path::new("/xdg/pandemonium/campaign_progress.cfg").to_path_buf())
            );
            assert_eq!(
                crate::config::config_path_from(env.0, env.1, env.2),
                Some(Path::new("/xdg/pandemonium/settings.cfg").to_path_buf())
            );
        }
        // No config directory at all: persistence disabled, never a panic.
        assert_eq!(CampaignProgress::path_from(None, None, None), None);
        // An empty XDG_CONFIG_HOME is unset (the XDG spec).
        assert_eq!(
            CampaignProgress::path_from(None, None, Some(PathBuf::new())),
            None
        );
    }

    #[test]
    fn load_from_and_save_to_round_trip_through_the_filesystem() {
        let dir = std::env::temp_dir();
        let path = dir.join("pandemonium-b026-progress-round-trip.cfg");
        let _ = std::fs::remove_file(&path);

        // A missing file loads empty...
        assert_eq!(
            CampaignProgress::load_from(&path),
            CampaignProgress::default()
        );
        // ...a save lands beside nothing (the parent exists)...
        let mut progress = CampaignProgress::default();
        progress.record("crossroads_01", MissionRecord::Completed);
        progress.record("crossroads_02", MissionRecord::Skipped);
        progress.save_to(&path).expect("the save lands");
        // ...and the load reads the same state back.
        assert_eq!(CampaignProgress::load_from(&path), progress);

        // Tidy: the file lives in the session temp dir only.
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_to_creates_a_missing_parent_directory() {
        let dir = std::env::temp_dir().join("pandemonium-b026-progress-nested");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("deeper").join("campaign_progress.cfg");
        let mut progress = CampaignProgress::default();
        progress.record("crossroads_01", MissionRecord::Completed);
        progress.save_to(&path).expect("the nested save lands");
        assert_eq!(CampaignProgress::load_from(&path), progress);

        // Tidy.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_path_fails_gracefully_on_load() {
        // A path that is a directory (or otherwise unreadable) is the
        // no-file case: the empty progress, never a panic.
        let dir = std::env::temp_dir();
        assert_eq!(
            CampaignProgress::load_from(&dir),
            CampaignProgress::default()
        );
    }
}
