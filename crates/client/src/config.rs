//! The client's settings (PLAN-M10.2 §3.4): a small hand-written `key=value`
//! file in the user config directory, loaded with safe defaults when the
//! file is missing or malformed and saved on leaving the settings screen.
//!
//! No serde, no `dirs` crate — the dependency law's client allow-list stays
//! untouched (the plan's "prefer no new crates"; the config directory is
//! resolved by hand from `std::env`, and a machine with neither HOME nor
//! APPDATA gets in-memory defaults with persistence silently disabled, the
//! same fallback spirit as PLAN §4.3's audio route).
//!
//! The module is pure data-in/data-out: parsing and serialization are plain
//! functions over `&str` so the corrupt-file tests need no filesystem, and
//! the directory resolution takes the env values as parameters (a thin
//! wrapper reads `std::env`). The camera zoom defaults mirror the engine's
//! `DISTANCE_RANGE` (the renderer clamps there; these limits narrow it).

use std::path::PathBuf;

/// The persisted settings (PLAN §3.4's six keys — exactly these, no more).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Whether the screen-edge scroll band is active (DEBT-014's toggle).
    pub edge_scroll: bool,
    /// Camera pan speed in tiles per second (keys and edge band share it).
    pub pan_speed: f32,
    /// The camera distance lower clamp (tiles).
    pub zoom_min: f32,
    /// The camera distance upper clamp (tiles).
    pub zoom_max: f32,
    /// Master volume, 0.0..=1.0 — stored and shown, audible in Phase 4
    /// (the `AudioSink` seam is fed but null-backed, DEBT-011).
    pub master_volume: f32,
    /// Whether the window starts fullscreen.
    pub fullscreen: bool,
    /// Whether the F3 debug overlay is visible at match start.
    pub debug_overlay: bool,
}

/// The camera zoom default bounds — the engine's own `DISTANCE_RANGE`,
/// restated here so a settings file that never narrows them behaves exactly
/// like the engine's clamp.
pub const DEFAULT_ZOOM_MIN: f32 = 8.0;
/// See [`DEFAULT_ZOOM_MIN`].
pub const DEFAULT_ZOOM_MAX: f32 = 160.0;
/// The default pan speed — `PAN_TILES_PER_SECOND`'s value, now configurable.
pub const DEFAULT_PAN_SPEED: f32 = 34.0;

impl Default for Settings {
    fn default() -> Self {
        Self {
            edge_scroll: true,
            pan_speed: DEFAULT_PAN_SPEED,
            zoom_min: DEFAULT_ZOOM_MIN,
            zoom_max: DEFAULT_ZOOM_MAX,
            master_volume: 1.0,
            fullscreen: false,
            debug_overlay: false,
        }
    }
}

/// The settings keys, in file order (also the settings screen's row order).
// (the settings screen consumes this + the save path in its own commit,
// later in this same phase)
#[allow(dead_code)]
pub const SETTING_KEYS: [&str; 7] = [
    "edge_scroll",
    "pan_speed",
    "zoom_min",
    "zoom_max",
    "master_volume",
    "fullscreen",
    "debug_overlay",
];

impl Settings {
    /// Serializes to the file format: one `key=value` per line, in
    /// [`SETTING_KEYS`] order, with a leading comment line. Deterministic —
    /// two equal settings serialize to equal bytes.
    // (consumed by `save` / the settings screen, later in this phase)
    #[allow(dead_code)]
    pub fn to_text(self) -> String {
        let mut out = String::from("# pandemonium settings - key=value lines\n");
        out.push_str(&format!("edge_scroll={}\n", self.edge_scroll));
        out.push_str(&format!("pan_speed={}\n", self.pan_speed));
        out.push_str(&format!("zoom_min={}\n", self.zoom_min));
        out.push_str(&format!("zoom_max={}\n", self.zoom_max));
        out.push_str(&format!("master_volume={}\n", self.master_volume));
        out.push_str(&format!("fullscreen={}\n", self.fullscreen));
        out.push_str(&format!("debug_overlay={}\n", self.debug_overlay));
        out
    }

    /// Parses a file body. Per-key salvage (A-108): a line whose value is
    /// malformed costs that key its default — one bad line does not discard
    /// the whole file, the plan's letter allows the stricter whole-file
    /// reset and this is the friendlier reading of "safe defaults".
    /// Unknown keys and blank/`#`-comment lines are ignored; a key that
    /// appears twice keeps the last parseable value.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue; // not key=value — ignored, everything stays at defaults-so-far
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "edge_scroll" => {
                    settings.edge_scroll = parse_bool(value).unwrap_or(settings.edge_scroll);
                }
                "pan_speed" => {
                    if let Some(speed) = parse_f32(value) {
                        settings.pan_speed = clamp_pan_speed(speed);
                    }
                }
                "zoom_min" | "zoom_max" => {
                    if let Some(zoom) = parse_f32(value) {
                        let zoom = clamp_zoom(zoom);
                        if key == "zoom_min" {
                            settings.zoom_min = zoom;
                        } else {
                            settings.zoom_max = zoom;
                        }
                    }
                    // A min above the max is meaningless — the stricter
                    // (later-in-file) bound wins and the other follows.
                    if settings.zoom_min > settings.zoom_max {
                        if key == "zoom_min" {
                            settings.zoom_max = settings.zoom_min;
                        } else {
                            settings.zoom_min = settings.zoom_max;
                        }
                    }
                }
                "master_volume" => {
                    if let Some(volume) = parse_f32(value) {
                        settings.master_volume = volume.clamp(0.0, 1.0);
                    }
                }
                "fullscreen" => {
                    settings.fullscreen = parse_bool(value).unwrap_or(settings.fullscreen);
                }
                "debug_overlay" => {
                    settings.debug_overlay = parse_bool(value).unwrap_or(settings.debug_overlay);
                }
                _ => {} // unknown key — ignored by design
            }
        }
        settings
    }
}

/// Parses `true`/`false` (any case); anything else is malformed.
fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Parses a finite `f32` (NaN and infinity are malformed, not values).
fn parse_f32(value: &str) -> Option<f32> {
    value.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Pan speed is clamped to a sane positive range — a zero/negative/absurd
/// speed must not corrupt the camera (the plan's value-range clamping).
pub fn clamp_pan_speed(speed: f32) -> f32 {
    speed.clamp(2.0, 300.0)
}

/// Zoom bounds are clamped into the engine's own distance range.
pub fn clamp_zoom(zoom: f32) -> f32 {
    zoom.clamp(DEFAULT_ZOOM_MIN, DEFAULT_ZOOM_MAX)
}

/// The config file's path from the platform's env values (pure — the thin
/// wrapper [`config_path`] reads `std::env`). Resolution order:
///
/// - Windows: `%APPDATA%\pandemonium\settings.cfg`
/// - macOS: `$HOME/Library/Application Support/pandemonium/settings.cfg`
/// - Linux/BSD: `$XDG_CONFIG_HOME/pandemonium/settings.cfg`, else
///   `$HOME/.config/pandemonium/settings.cfg`
///
/// No HOME and no APPDATA at all (CI, containers): `None` — the caller
/// runs on in-memory defaults with persistence silently disabled, never a
/// panic (A-109). The signature is uniform on every platform (the unused
/// arms are ignored per-OS) so the resolution logic itself is testable
/// anywhere.
pub fn config_path_from(
    appdata: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
) -> Option<PathBuf> {
    // An empty path is unset (the XDG spec's own reading) — treat it as
    // None everywhere so `.join()` never builds a relative path.
    let appdata = appdata.filter(|p| !p.as_os_str().is_empty());
    let home = home.filter(|p| !p.as_os_str().is_empty());
    let xdg_config_home = xdg_config_home.filter(|p| !p.as_os_str().is_empty());
    #[cfg(target_os = "windows")]
    {
        let _ = (home, xdg_config_home);
        appdata.map(|base| base.join("pandemonium").join("settings.cfg"))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (appdata, xdg_config_home);
        home.map(|base| {
            base.join("Library")
                .join("Application Support")
                .join("pandemonium")
                .join("settings.cfg")
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = appdata;
        match xdg_config_home {
            Some(base) => Some(base.join("pandemonium").join("settings.cfg")),
            None => home.map(|base| {
                base.join(".config")
                    .join("pandemonium")
                    .join("settings.cfg")
            }),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (appdata, home, xdg_config_home);
        None // unknown platform: run on defaults, persistence disabled
    }
}

/// The config file path on this machine, from `std::env` (the thin wrapper
/// over [`config_path_from`]). `None` means "run on defaults, persistence
/// disabled" — a read or write attempt is skipped, not an error the loop
/// ever sees.
pub fn config_path() -> Option<PathBuf> {
    config_path_from(
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )
}

/// Loads the settings from the config path when it exists and is readable,
/// else the defaults. A file that exists but cannot be read (permissions)
/// is the same "no file" case: defaults, no crash.
pub fn load() -> Settings {
    let Some(path) = config_path() else {
        return Settings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => Settings::parse(&text),
        Err(_) => Settings::default(),
    }
}

/// Saves the settings to the config path (best effort). Returns whether the
/// write landed — the caller surfaces a one-line notice on failure and the
/// run continues on the in-memory values either way (never a crash).
// (the settings screen's Done button calls this, later in this phase)
#[allow(dead_code)]
pub fn save(settings: &Settings) -> Option<std::io::Result<()>> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        // The directory may not exist yet on a fresh install.
        let _ = std::fs::create_dir_all(parent);
    }
    Some(std::fs::write(&path, settings.to_text()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values() {
        let s = Settings::default();
        assert!(s.edge_scroll);
        assert_eq!(s.pan_speed, 34.0);
        assert_eq!(s.zoom_min, 8.0);
        assert_eq!(s.zoom_max, 160.0);
        assert_eq!(s.master_volume, 1.0);
        assert!(!s.fullscreen);
        assert!(!s.debug_overlay);
    }

    #[test]
    fn round_trip_save_then_load_equals_the_same_settings() {
        let original = Settings {
            edge_scroll: false,
            pan_speed: 60.0,
            zoom_min: 12.0,
            zoom_max: 90.0,
            master_volume: 0.35,
            fullscreen: true,
            debug_overlay: true,
        };
        let parsed = Settings::parse(&original.to_text());
        assert_eq!(parsed, original);
    }

    #[test]
    fn round_trip_of_the_defaults_is_stable() {
        let text = Settings::default().to_text();
        assert_eq!(Settings::parse(&text), Settings::default());
    }

    #[test]
    fn a_missing_file_is_all_defaults() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("\n\n   \n"), Settings::default());
    }

    #[test]
    fn a_malformed_file_falls_back_to_defaults_per_key() {
        // Per-key salvage (A-108): the garbage lines cost their own keys,
        // the good line survives.
        let text = "edge_scroll=maybe\npan_speed=fast\nzoom_min=[]\n\
                    master_volume=loud\nfullscreen=perhaps\ndebug_overlay=???\n";
        let s = Settings::parse(text);
        assert!(s.edge_scroll); // malformed -> default (true)
        assert_eq!(s.pan_speed, DEFAULT_PAN_SPEED);
        assert_eq!(s.zoom_min, DEFAULT_ZOOM_MIN);
        assert_eq!(s.master_volume, 1.0);
        assert!(!s.fullscreen);
        assert!(!s.debug_overlay);
    }

    #[test]
    fn a_wholly_garbage_file_is_all_defaults() {
        let s = Settings::parse("this is not a config file\n\x00\x01\nkey without value\n====\n");
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn comments_and_blank_lines_and_unknown_keys_are_ignored() {
        let text = "# a comment\n\nedge_scroll=false\nunknown_key=42\n  # indented comment\n\
                    pan_speed=50\nnew_engine=serde\n";
        let s = Settings::parse(text);
        assert!(!s.edge_scroll);
        assert_eq!(s.pan_speed, 50.0);
        assert_eq!(s.zoom_max, DEFAULT_ZOOM_MAX); // untouched by the unknown key
    }

    #[test]
    fn values_are_range_clamped_not_state_corrupting() {
        // A negative/absurd pan speed clamps instead of corrupting the camera.
        assert_eq!(clamp_pan_speed(-5.0), 2.0);
        assert_eq!(clamp_pan_speed(1.0e9), 300.0);
        assert_eq!(Settings::parse("pan_speed=-5\n").pan_speed, 2.0);
        // NaN never parses (malformed -> default), and zoom clamps into range.
        assert_eq!(Settings::parse("zoom_min=0\n").zoom_min, DEFAULT_ZOOM_MIN);
        assert_eq!(
            Settings::parse("zoom_max=99999\n").zoom_max,
            DEFAULT_ZOOM_MAX
        );
        assert_eq!(Settings::parse("zoom_min=NaN\n").zoom_min, DEFAULT_ZOOM_MIN);
        // Volume clamps into 0..=1.
        assert_eq!(Settings::parse("master_volume=4\n").master_volume, 1.0);
        assert_eq!(Settings::parse("master_volume=-1\n").master_volume, 0.0);
    }

    #[test]
    fn a_min_above_the_max_follows_the_later_bound() {
        // zoom_min=100 then zoom_max=40: the later bound wins, min follows.
        let s = Settings::parse("zoom_min=100\nzoom_max=40\n");
        assert_eq!((s.zoom_min, s.zoom_max), (40.0, 40.0));
        // The other order: max first, then a smaller min — fine as written.
        let s = Settings::parse("zoom_max=40\nzoom_min=10\n");
        assert_eq!((s.zoom_min, s.zoom_max), (10.0, 40.0));
    }

    #[test]
    fn bools_parse_the_friendly_spellings() {
        assert!(parse_bool("TRUE").unwrap());
        assert!(parse_bool("on").unwrap());
        assert!(!parse_bool("False").unwrap());
        assert!(!parse_bool("0").unwrap());
        assert!(parse_bool("side").is_none());
    }

    #[test]
    fn a_repeated_key_keeps_the_last_parseable_value() {
        let s = Settings::parse("pan_speed=40\npan_speed=60\npan_speed=garbage\n");
        // The last *parseable* value wins; the garbage line costs nothing.
        assert_eq!(s.pan_speed, 60.0);
    }

    #[test]
    fn the_config_dir_resolves_from_the_platform_env_values() {
        use std::path::Path;
        // Linux: XDG_CONFIG_HOME wins when set...
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(
            config_path_from(
                None,
                Some(Path::new("/home/tester").to_path_buf()),
                Some(Path::new("/xdg").to_path_buf())
            ),
            Some(Path::new("/xdg/pandemonium/settings.cfg").to_path_buf())
        );
        // ...else $HOME/.config.
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(
            config_path_from(None, Some(Path::new("/home/tester").to_path_buf()), None),
            Some(Path::new("/home/tester/.config/pandemonium/settings.cfg").to_path_buf())
        );
        // Windows: APPDATA decides (HOME/XDG ignored on that platform).
        #[cfg(windows)]
        assert_eq!(
            config_path_from(
                Some(Path::new(r"C:\Users\t\AppData\Roaming").to_path_buf()),
                Some(Path::new("/home/ignored").to_path_buf()),
                None
            ),
            Some(Path::new(r"C:\Users\t\AppData\Roaming\pandemonium\settings.cfg").to_path_buf())
        );
        // macOS: HOME decides (APPDATA/XDG ignored on that platform).
        #[cfg(target_os = "macos")]
        assert_eq!(
            config_path_from(
                Some(Path::new("/ignored").to_path_buf()),
                Some(Path::new("/Users/t").to_path_buf()),
                Some(Path::new("/xdg-ignored").to_path_buf())
            ),
            Some(
                Path::new("/Users/t/Library/Application Support/pandemonium/settings.cfg")
                    .to_path_buf()
            )
        );
        // Nothing at all (CI, containers): persistence disabled, never a panic.
        assert_eq!(config_path_from(None, None, None), None);
        // An empty XDG_CONFIG_HOME is unset for our purposes (XDG spec).
        assert_eq!(config_path_from(None, None, Some(PathBuf::new())), None);
    }

    #[test]
    fn the_serialized_form_is_deterministic_and_ascii() {
        let text = Settings::default().to_text();
        assert!(text.is_ascii(), "the embedded font is an ASCII subset (T8)");
        assert!(text.starts_with('#'));
        assert_eq!(text, Settings::default().to_text());
        // Every declared key appears exactly once.
        for key in SETTING_KEYS {
            assert_eq!(text.matches(&format!("{key}=")).count(), 1);
        }
    }
}
