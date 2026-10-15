//! `tools content-validate [PATH]` (plan §12): load and validate every content
//! file and map under a directory, strictly, and report the bundle identity.
//!
//! The command is the designer's front door: it prints a per-category summary,
//! per-map facts (dimensions, ore, symmetry, heightmap, starts), and the content
//! hash, and exits nonzero with the precise error on the first problem found.
//!
//! The optional `--expect-content-hash` / `--expect-map-id` flags (the audit's
//! AUD-005) turn the report into a golden-identity pin: when given, the loaded
//! tree must bundle at least one map whose identity is exactly the registered
//! pair, or the command fails with the found identities — the executable
//! validation behind CI's `content-identity` gate. The tool itself pins
//! nothing by default (community trees validate the same way — ADR-0200); the
//! registered values live in the workflow file, where an authorized move is a
//! visible one-line diff, never a silent bypass.

use std::path::Path;

use anyhow::{bail, Context};
use pandemonium_content::ContentTree;

/// Parses a golden-identity value from the CLI: an optional `0x` prefix,
/// then sixteen hex digits (case-insensitive). `None` is a caller-facing
/// parse failure (the CLI turns it into a precise error naming the input).
pub fn parse_identity_hex(input: &str) -> Option<u64> {
    let stripped = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"));
    u64::from_str_radix(stripped.unwrap_or(input), 16).ok()
}

/// Runs the validation over `path`, printing the report. Returns an error when
/// anything is invalid (the caller exits nonzero). When `expect_content_hash`
/// and/or `expect_map_id` are given, the loaded tree must bundle a map whose
/// identity matches every given expectation (the registered-identity pin);
/// a mismatch is an error naming both sides.
pub fn validate_content(
    path: &Path,
    expect_content_hash: Option<u64>,
    expect_map_id: Option<u64>,
) -> anyhow::Result<()> {
    let display = path.display();
    let tree = ContentTree::load_dir(path).with_context(|| format!("validating {display}"))?;

    println!("pandemonium content-validate — {display}");
    println!(
        "  ruleset:     {} ({} resource{}: {})",
        tree.rules.id,
        tree.rules.resources.len(),
        if tree.rules.resources.len() == 1 {
            ""
        } else {
            "s"
        },
        tree.rules
            .resources
            .iter()
            .map(|resource| resource.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  entities:    {} ({})",
        tree.entities.len(),
        tree.entities
            .iter()
            .map(|entity| entity.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  factions:    {} ({})",
        tree.factions.len(),
        tree.factions
            .iter()
            .map(|faction| faction.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  maps:        {}", tree.maps.len());
    for map in &tree.maps {
        let bundle = tree
            .bundle(&map.id)
            .with_context(|| format!("bundling map '{}' for the report", map.id))?;
        let heightmap = if map.heightmap.is_some() {
            "yes (display-only, ADR-0001)"
        } else {
            "none (flat)"
        };
        let symmetry = if map.symmetric {
            "declared + verified"
        } else {
            "not declared"
        };
        let starts = map
            .starts
            .iter()
            .map(|start| start.player.to_string())
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "    {}: {}x{}, {} ore nodes, starts: players {starts}, symmetry: {symmetry}, \
             heightmap: {heightmap}",
            map.id,
            map.width,
            map.height,
            map.ore_nodes.len()
        );
        println!(
            "      content hash: {:#018x}, map id: {:#018x}",
            bundle.content_hash(),
            bundle.map_id()
        );
    }
    // AUD-005: the identity pin. The expectations name the registered
    // identity; the tree must still bundle a map that carries it. An
    // unauthorized edit that changes any bundle byte fails here (with the
    // found identities listed); an authorized move re-baselines the pinned
    // values in the same dedicated commit — never a silent pass.
    if expect_content_hash.is_some() || expect_map_id.is_some() {
        let found = &tree.maps;
        let matches = |hash: u64, map_id: u64| {
            expect_content_hash.is_none_or(|expected| expected == hash)
                && expect_map_id.is_none_or(|expected| expected == map_id)
        };
        let mut listed = Vec::new();
        let mut pinned = false;
        for map in found {
            let bundle = tree
                .bundle(&map.id)
                .with_context(|| format!("bundling map '{}' for the pin check", map.id))?;
            let (hash, map_id) = (bundle.content_hash(), bundle.map_id());
            if matches(hash, map_id) {
                pinned = true;
                println!(
                    "  identity pin: {} — content hash {hash:#018x}, map id {map_id:#018x} \
                     (matches the registered identity)",
                    map.id
                );
            }
            listed.push(format!(
                "{}: content hash {hash:#018x}, map id {map_id:#018x}",
                map.id
            ));
        }
        if !pinned {
            bail!(
                "content identity mismatch: no loaded map bundles the registered identity \
                 (expected content hash {}, map id {}); the tree bundles: {}",
                expect_content_hash
                    .map(|hash| format!("{hash:#018x}"))
                    .unwrap_or_else(|| "any".to_string()),
                expect_map_id
                    .map(|id| format!("{id:#018x}"))
                    .unwrap_or_else(|| "any".to_string()),
                listed.join("; ")
            );
        }
    }
    println!("content-validate: PASS");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_content() -> std::path::PathBuf {
        // crates/tools -> crates/ -> the workspace root.
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the tools crate sits two levels under the workspace root")
            .join("content")
    }

    #[test]
    fn the_alpha_content_directory_validates() {
        // The repo's own content tree is a loadable, valid bundle — this is the
        // M2 exit criterion in test form ("all Alpha content and the map load").
        validate_content(&repo_content(), None, None)
            .expect("the repository content directory must validate");
    }

    #[test]
    fn the_registered_identity_pin_passes_on_the_repo_content() {
        // AUD-005: with the registered values the pin passes — the executable
        // form of "the content golden never moves" (the same pair CI's
        // content-identity job pins and tests/content_pipeline.rs asserts).
        validate_content(
            &repo_content(),
            Some(0x9bc1_8c52_1107_b262),
            Some(0xd381_3640_1ab0_2ff1),
        )
        .expect("the registered identity must pin cleanly");
    }

    #[test]
    fn a_foreign_identity_pin_fails_with_both_sides_named() {
        // A wrong expectation is a precise error naming the expected and the
        // found identities — the diagnostic an unauthorized content edit
        // produces before anyone re-pins anything.
        let err = validate_content(&repo_content(), Some(0xdead_beef_dead_beef), None).unwrap_err();
        let chain = format!("{err:#}");
        assert!(chain.contains("identity mismatch"), "got: {chain}");
        assert!(chain.contains("0xdeadbeefdeadbeef"), "got: {chain}");
        assert!(chain.contains("0x9bc18c521107b262"), "got: {chain}");
        let err = validate_content(&repo_content(), None, Some(0x1)).unwrap_err();
        let chain = format!("{err:#}");
        assert!(chain.contains("identity mismatch"), "got: {chain}");
    }

    #[test]
    fn the_hex_identity_parser_accepts_prefixed_and_bare_hex() {
        // CI passes 0x-prefixed values; bare hex parses the same way.
        assert_eq!(
            parse_identity_hex("0x9bc18c521107b262"),
            Some(0x9bc1_8c52_1107_b262)
        );
        assert_eq!(
            parse_identity_hex("9bc18c521107b262"),
            Some(0x9bc1_8c52_1107_b262)
        );
        assert_eq!(
            parse_identity_hex("0xD38136401AB02FF1"),
            Some(0xd381_3640_1ab0_2ff1)
        );
        assert_eq!(parse_identity_hex("not-a-hash"), None);
    }

    #[test]
    fn a_missing_directory_is_a_precise_error() {
        let err =
            validate_content(std::path::Path::new("/nonexistent/content"), None, None).unwrap_err();
        // The anyhow chain (`{:#}`) surfaces the underlying ContentError.
        let chain = format!("{err:#}");
        assert!(chain.contains("missing content directory"), "got: {chain}");
    }
}
