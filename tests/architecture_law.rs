//! Architecture law tests (plan §4) and determinism source bans (plan §5).
//!
//! These tests pin the dependency graph and the source-level determinism rules from
//! day zero (milestone M0). They parse the workspace member manifests directly —
//! a hermetic, offline-safe equivalent of inspecting `cargo metadata` (the plan's
//! wording; see docs/ASSUMPTIONS.md A-008) — and scan the determinism-critical
//! crate sources line by line for forbidden constructs.
//!
//! Dev-dependencies are exempt from the direction law: they are compiled only for
//! tests and never ship. The forbidden-crate list applies to them regardless.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Every workspace member that must exist, by package name (plan §4 layout).
const EXPECTED_MEMBERS: &[&str] = &[
    "pandemonium-ai",
    "pandemonium-client",
    "pandemonium-content",
    "pandemonium-engine",
    "pandemonium-fx",
    "pandemonium-replay",
    "pandemonium-sim",
    "pandemonium-sim-api",
    "pandemonium-tests",
    "pandemonium-tools",
];

/// Crates forbidden anywhere in the workspace (plan §3.2 "Forbidden"). The `bevy`
/// entry additionally matches as a prefix (bevy_* helper crates).
const FORBIDDEN_CRATES: &[&str] = &[
    "bevy",
    "macroquad",
    "ggez",
    "fyrox",
    "godot",
    "hecs",
    "legion",
    "specs",
    "shipyard",
    "flecs",
    "rapier",
    "rapier2d",
    "rapier3d",
    "pathfinding",
    "rand",
    "rand_core",
    "getrandom",
];

/// Allowed internal dependencies per member — the dependency law (plan §4).
///
/// The plan's diagram is read as non-exhaustive (docs/ASSUMPTIONS.md A-003/A-004/
/// A-005): the engine's game loop owns stepping the sim (§11.1) and hosts AI
/// controllers on the tick boundary (§9.6); the client constructs matches and
/// mutates state only through step inputs (M3 exit criterion). `pandemonium-tests`
/// is the acceptance host and is exempt from the internal law.
fn allowed_internal() -> BTreeMap<&'static str, &'static [&'static str]> {
    let mut m = BTreeMap::new();
    m.insert("pandemonium-fx", &[][..]);
    m.insert("pandemonium-sim-api", &["pandemonium-fx"][..]);
    m.insert(
        "pandemonium-sim",
        &["pandemonium-fx", "pandemonium-sim-api"][..],
    );
    m.insert(
        "pandemonium-content",
        &["pandemonium-fx", "pandemonium-sim", "pandemonium-sim-api"][..],
    );
    m.insert(
        "pandemonium-ai",
        &["pandemonium-fx", "pandemonium-sim-api"][..],
    );
    m.insert(
        "pandemonium-replay",
        &["pandemonium-fx", "pandemonium-sim-api"][..],
    );
    m.insert(
        "pandemonium-engine",
        &[
            "pandemonium-fx",
            "pandemonium-sim",
            "pandemonium-sim-api",
            "pandemonium-content",
            "pandemonium-ai",
        ][..],
    );
    m.insert(
        "pandemonium-client",
        &[
            "pandemonium-fx",
            "pandemonium-sim",
            "pandemonium-sim-api",
            "pandemonium-content",
            "pandemonium-ai",
            "pandemonium-replay",
            "pandemonium-engine",
        ][..],
    );
    m.insert(
        "pandemonium-tools",
        &[
            "pandemonium-fx",
            "pandemonium-sim",
            "pandemonium-sim-api",
            "pandemonium-content",
            "pandemonium-ai",
            "pandemonium-replay",
            "pandemonium-engine",
        ][..],
    );
    m
}

/// Allowed external dependencies per member — plan §3.2's dependency table, encoded
/// in full from day zero so that milestone additions (wgpu in M3, clap in tools,
/// …) are not law changes: they were always allowed where the plan allows them.
fn allowed_external() -> BTreeMap<&'static str, &'static [&'static str]> {
    let mut m = BTreeMap::new();
    m.insert("pandemonium-fx", &[][..]);
    for c in [
        "pandemonium-sim-api",
        "pandemonium-sim",
        "pandemonium-ai",
        "pandemonium-replay",
        "pandemonium-engine",
    ] {
        m.insert(c, &["thiserror"][..]);
    }
    // glam joined the plan §3.2 table with ADR-0001 (3D presentation): float
    // math for the presentation layer only, never in the sim's tree.
    m.insert("pandemonium-engine", &["glam", "thiserror"][..]);
    m.insert("pandemonium-content", &["serde", "ron", "thiserror"][..]);
    m.insert(
        "pandemonium-client",
        &[
            "anyhow",
            "bytemuck",
            "cpal",
            "egui",
            "fontdue",
            "glam",
            "image",
            "pollster",
            "thiserror",
            "wgpu",
            "winit",
        ][..],
    );
    m.insert(
        "pandemonium-tools",
        &["anyhow", "clap", "image", "thiserror"][..],
    );
    // Test infrastructure only — never ships, never links into the game crates.
    m.insert("pandemonium-tests", &["proptest", "serde_json"][..]);
    m
}

#[derive(Default, Debug)]
struct Manifest {
    name: Option<String>,
    normal: Vec<String>,
    dev: Vec<String>,
    build: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum DepSection {
    Normal,
    Dev,
    Build,
    Other,
}

/// Minimal reader for the manifest style this workspace authors: section headers
/// plus `crate = …` dependency lines. Deliberately conservative — anything it
/// cannot understand simply isn't recorded, and the law checks below fail loudly
/// on surprises, so an exotic manifest style cannot slip a dependency past the test.
fn parse_manifest(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    let mut section = DepSection::Other;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            section = if line.starts_with("[dependencies.") {
                DepSection::Normal
            } else if line.starts_with("[dev-dependencies.") {
                DepSection::Dev
            } else if line.starts_with("[build-dependencies.") {
                DepSection::Build
            } else if line == "[dependencies]" {
                DepSection::Normal
            } else if line == "[dev-dependencies]" {
                DepSection::Dev
            } else if line == "[build-dependencies]" {
                DepSection::Build
            } else {
                DepSection::Other
            };
            continue;
        }
        match section {
            DepSection::Other => {
                if let Some(rest) = line.strip_prefix("name") {
                    let rest = rest.trim_start();
                    if let Some(value) = rest.strip_prefix('=') {
                        let value = value.trim().trim_matches('"');
                        if manifest.name.is_none() {
                            manifest.name = Some(value.to_string());
                        }
                    }
                }
            }
            DepSection::Normal | DepSection::Dev | DepSection::Build => {
                let key = line.split('=').next().unwrap_or("").trim();
                let key = key.trim_matches('"');
                if key.is_empty() {
                    continue;
                }
                match section {
                    DepSection::Normal => manifest.normal.push(key.to_string()),
                    DepSection::Dev => manifest.dev.push(key.to_string()),
                    DepSection::Build => manifest.build.push(key.to_string()),
                    DepSection::Other => unreachable!(),
                }
            }
        }
    }
    manifest
}

/// The workspace root — the parent of this package's directory (`<root>/tests`).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the tests package sits inside the workspace root")
        .to_path_buf()
}

/// Reads every member manifest under `crates/` plus the `tests` package.
fn discover_members(root: &Path) -> BTreeMap<String, Manifest> {
    let mut members = BTreeMap::new();
    let crates_dir = root.join("crates");
    let entries = fs::read_dir(&crates_dir).expect("crates/ directory exists");
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("reading crates/ entry failed: {e}"));
        let manifest_path = entry.path().join("Cargo.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("reading {} failed: {e}", manifest_path.display()));
        let manifest = parse_manifest(&text);
        let name = manifest
            .name
            .clone()
            .unwrap_or_else(|| panic!("{} has no package name", manifest_path.display()));
        members.insert(name, manifest);
    }
    let tests_manifest = root.join("tests").join("Cargo.toml");
    let text = fs::read_to_string(&tests_manifest)
        .unwrap_or_else(|e| panic!("reading {} failed: {e}", tests_manifest.display()));
    let manifest = parse_manifest(&text);
    members.insert(
        manifest.name.clone().expect("tests package has a name"),
        manifest,
    );
    members
}

fn is_forbidden(crate_name: &str) -> bool {
    FORBIDDEN_CRATES.contains(&crate_name) || crate_name.starts_with("bevy")
}

#[test]
fn dependency_law_holds() {
    let root = workspace_root();

    // The root manifest must remain a virtual workspace (plan §4's tree).
    let root_manifest =
        fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml readable");
    assert!(
        root_manifest.contains("[workspace]"),
        "the root manifest must declare [workspace]"
    );

    let members = discover_members(&root);

    // 1. Exactly the expected member set — no crates silently added or dropped.
    let mut actual: Vec<&str> = members.keys().map(String::as_str).collect();
    actual.sort_unstable();
    let mut expected: Vec<&str> = EXPECTED_MEMBERS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        actual, expected,
        "workspace members drifted from plan §4's layout"
    );

    let internal = allowed_internal();
    let external = allowed_external();
    let mut violations: Vec<String> = Vec::new();

    for (name, manifest) in &members {
        // Normal + build dependencies are the shipping law; dev-dependencies are
        // test-only and exempt — except from the forbidden-crate list.
        let shipping = manifest.normal.iter().chain(manifest.build.iter());

        for dep in shipping {
            if dep.starts_with("pandemonium-") {
                if !members.contains_key(dep) {
                    violations.push(format!(
                        "{name} depends on {dep}, which is not a workspace member"
                    ));
                }
                if name != "pandemonium-tests" {
                    // The acceptance host may depend on anything to test it.
                    let allowed = internal.get(name.as_str()).copied().unwrap_or(&[]);
                    if !allowed.contains(&dep.as_str()) {
                        violations.push(format!(
                            "{name} -> {dep} violates the dependency law (plan §4)"
                        ));
                    }
                }
            } else {
                if is_forbidden(dep) {
                    violations.push(format!("{name} uses forbidden crate '{dep}' (plan §3.2)"));
                } else {
                    let allowed_ext = external.get(name.as_str()).copied().unwrap_or(&[]);
                    if !allowed_ext.contains(&dep.as_str()) {
                        violations.push(format!(
                            "{name} uses external crate '{dep}' outside plan §3.2's allow-list"
                        ));
                    }
                }
            }
        }

        for dep in &manifest.dev {
            if is_forbidden(dep) {
                violations.push(format!(
                    "{name} uses forbidden crate '{dep}' even as a dev-dependency (plan §3.2)"
                ));
            }
        }
    }

    // 2. The explicit inverse law, stated in plan §4's own words — belt and braces,
    //    so a future edit to the allow-lists above cannot silently legalize these.
    let sim = &members["pandemonium-sim"];
    for forbidden in [
        "pandemonium-engine",
        "pandemonium-client",
        "pandemonium-ai",
        "pandemonium-replay",
        "pandemonium-content",
    ] {
        if sim.normal.iter().any(|d| d == forbidden) {
            violations.push(format!(
                "sim must NOT depend on {forbidden} (plan §4, FD-6)"
            ));
        }
    }
    let ai = &members["pandemonium-ai"];
    if ai.normal.iter().any(|d| d == "pandemonium-sim") {
        violations
            .push("ai must NOT depend on sim — parity is structural (plan §4, FD-7)".to_string());
    }
    let fx = &members["pandemonium-fx"];
    if !fx.normal.is_empty() || !fx.build.is_empty() {
        violations.push(format!(
            "fx must depend on nothing at all; found {:?}",
            fx.normal
        ));
    }

    assert!(
        violations.is_empty(),
        "architecture law violations:\n  {}",
        violations.join("\n  ")
    );
}

/// Tokens banned from the determinism crates (plan §5): unordered-map types,
/// wall-clock sources, and floating-point type names. The engine and client are
/// NOT scanned — rendering interpolation and real-time timing legitimately live
/// there (plan §11.1) and never enter the simulation.
#[test]
fn determinism_source_bans_hold() {
    let root = workspace_root();
    let scanned_crates = ["fx", "sim", "sim_api", "ai", "content"];
    let banned_tokens = ["HashMap", "HashSet", "Instant", "SystemTime", "f32", "f64"];

    let mut violations: Vec<String> = Vec::new();
    for crate_name in scanned_crates {
        let dir = root.join("crates").join(crate_name);
        let mut sources: Vec<PathBuf> = Vec::new();
        collect_rust_sources(&dir, &mut sources);
        assert!(
            !sources.is_empty(),
            "expected Rust sources under crates/{crate_name}/"
        );
        for path in sources {
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading {} failed: {e}", path.display()));
            for (index, line) in text.lines().enumerate() {
                for token in banned_tokens {
                    if line.contains(token) {
                        violations.push(format!(
                            "{}:{} contains banned token '{token}': {}",
                            path.display(),
                            index + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "determinism source bans violated (plan §5):\n  {}",
        violations.join("\n  ")
    );
}

/// Recursively collects every `.rs` file under `dir`.
fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
