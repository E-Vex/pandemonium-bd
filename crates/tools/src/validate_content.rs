//! `tools content-validate [PATH]` (plan §12): load and validate every content
//! file and map under a directory, strictly, and report the bundle identity.
//!
//! The command is the designer's front door: it prints a per-category summary,
//! per-map facts (dimensions, ore, symmetry, heightmap, starts), and the content
//! hash, and exits nonzero with the precise error on the first problem found.

use std::path::Path;

use anyhow::Context;
use pandemonium_content::ContentTree;

/// Runs the validation over `path`, printing the report. Returns an error when
/// anything is invalid (the caller exits nonzero).
pub fn validate_content(path: &Path) -> anyhow::Result<()> {
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
        validate_content(&repo_content()).expect("the repository content directory must validate");
    }

    #[test]
    fn a_missing_directory_is_a_precise_error() {
        let err = validate_content(std::path::Path::new("/nonexistent/content")).unwrap_err();
        // The anyhow chain (`{:#}`) surfaces the underlying ContentError.
        let chain = format!("{err:#}");
        assert!(chain.contains("missing content directory"), "got: {chain}");
    }
}
