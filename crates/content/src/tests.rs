//! Content crate tests (plan §14 M2): versioning, strict mode, precise
//! validators, hash stability, and the sim seam.
//!
//! The factory builds a minimal, fully valid content set in memory (rules, three
//! entities, one faction, one 16×16 map); every error test mutates exactly one
//! thing and asserts the precise rejection.

use std::path::PathBuf;

use crate::{ContentBundle, ContentError, ContentTree, SourceMap};

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

const RULES: &str = r#"
(
    schema_version: 1,
    id: "alpha",
    display_name: "Alpha Test Rules",
    resources: [ ( id: "ore", display_name: "Ore", starting: 200 ) ],
    base_population_cap: 0,
    victory: ( kind: "elimination", allow_resignation: true ),
)
"#;

const BASE: &str = r#"
(
    schema_version: 1,
    id: "base",
    display_name: "Base",
    capabilities: [
        Health(( max: 500 )),
        Vision(( radius_milli_tiles: 6000 )),
        Footprint(( w: 2, h: 2 )),
        Produce(()),
    ],
    cost: ( ore: 100 ),
    build_time_ms: 10000,
    population: 0,
    requires: [],
)
"#;

const WORKER: &str = r#"
(
    schema_version: 1,
    id: "worker",
    display_name: "Worker",
    capabilities: [
        Health(( max: 40 )),
        Move(( speed_milli_tiles_per_s: 2600, radius_milli_tiles: 300 )),
        Gather(( carry_amount: 10, gather_time_ms: 2000 )),
        Vision(( radius_milli_tiles: 7000 )),
    ],
    cost: ( ore: 50 ),
    build_time_ms: 12000,
    population: 1,
    requires: [ "base" ],
)
"#;

const ORE_NODE: &str = r#"
(
    schema_version: 1,
    id: "ore_node",
    display_name: "Ore Node",
    capabilities: [
        Resource(( resource: "ore", amount: 1500 )),
        Footprint(( w: 2, h: 2 )),
    ],
    cost: ( ore: 0 ),
    build_time_ms: 0,
    population: 0,
    requires: [],
)
"#;

const FACTION: &str = r#"
(
    schema_version: 1,
    id: "testers",
    display_name: "Testers",
    roster: [ "base", "worker" ],
    production: { "base": [ "worker" ] },
    starting_forces: [
        ( entity: "base", offset: ( x: 0, y: 0 ) ),
        ( entity: "worker", offset: ( x: -1, y: 3 ) ),
        ( entity: "worker", offset: ( x: 1, y: 3 ) ),
    ],
)
"#;

/// A 16x16 all-ground grid.
fn dot_grid() -> Vec<String> {
    vec!["................".to_string(); 16]
}

/// Sets one grid cell to rock.
fn rock(grid: &mut [String], x: usize, y: usize) {
    grid[y].replace_range(x..x + 1, "#");
}

/// The factory map text, version 2 (heightmap era), 16x16, two starts, two ore
/// nodes. `grid` replaces the default all-ground grid.
fn map_text(grid: &[String], symmetric: bool, heightmap: Option<&str>) -> String {
    let rows = grid
        .iter()
        .map(|row| format!("        \"{row}\","))
        .collect::<Vec<_>>()
        .join("\n");
    let heightmap_block = match heightmap {
        Some(rows) => format!("    heightmap: Some(( rows: {rows} )),"),
        None => String::new(),
    };
    format!(
        r##"
(
    schema_version: 2,
    id: "testing",
    display_name: "Testing Grounds",
    width: 16,
    height: 16,
    terrain: [
        ( code: ".", name: "ground", passable: true, buildable: true ),
        ( code: "#", name: "rock", passable: false, buildable: false ),
    ],
    grid: [
{rows}
    ],
    starts: [
        ( player: 0, anchor: ( x: 4, y: 4 ) ),
        ( player: 1, anchor: ( x: 10, y: 10 ) ),
    ],
    ore_nodes: [
        ( kind: "ore_node", tile: ( x: 1, y: 1 ) ),
        ( kind: "ore_node", tile: ( x: 13, y: 13 ) ),
    ],
    symmetric: {symmetric},
{heightmap_block}
)
"##
    )
}

/// The full valid factory source map.
fn factory() -> SourceMap {
    SourceMap::new()
        .rules("alpha.ron", RULES)
        .entity("base.ron", BASE)
        .entity("worker.ron", WORKER)
        .entity("ore_node.ron", ORE_NODE)
        .faction("testers.ron", FACTION)
        .map("testing.ron", &map_text(&dot_grid(), false, None))
}

/// A flat heightmap block matching the 16x16 grid (16 rows of 16 zeros).
fn flat_heightmap() -> String {
    let row = format!("[{}]", vec!["0"; 16].join(", "));
    format!("[{}]", vec![row; 16].join(", "))
}

// ---------------------------------------------------------------------------
// Authoring-unit conversion (plan §10.2)
// ---------------------------------------------------------------------------

#[test]
fn ms_to_ticks_rounds_up_and_never_rounds_away() {
    // (ms * 30 + 999) / 1000 — plan §10.2's formula.
    assert_eq!(crate::ms_to_ticks(0, 30), 0);
    assert_eq!(crate::ms_to_ticks(1, 30), 1); // rounds up to one tick
    assert_eq!(crate::ms_to_ticks(999, 30), 30); // 29.97 -> 30
    assert_eq!(crate::ms_to_ticks(1000, 30), 30); // exactly 30
    assert_eq!(crate::ms_to_ticks(1001, 30), 31); // 30.03 -> 31
    assert_eq!(crate::ms_to_ticks(2000, 30), 60); // the gather cycle
}

#[test]
fn durations_convert_to_ticks_at_load() {
    let bundle = ContentTree::load(&factory())
        .unwrap()
        .single_map_bundle()
        .unwrap();
    let worker = bundle.entity("worker").unwrap();
    assert_eq!(worker.build_time_ticks, 360); // 12000 ms -> 360 ticks
    match worker.capability("Gather") {
        Some(crate::CapabilityDef::Gather {
            gather_time_ticks, ..
        }) => assert_eq!(*gather_time_ticks, 60), // 2000 ms -> 60 ticks
        other => panic!("expected Gather, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Strict mode + versioning
// ---------------------------------------------------------------------------

#[test]
fn unknown_fields_are_errors_in_strict_mode() {
    let bad = RULES.replace("base_population_cap: 0", "base_pop_cap: 0");
    let err = ContentTree::load(&factory().rules("alpha.ron", &bad)).unwrap_err();
    assert!(matches!(err, ContentError::Parse { .. }), "got: {err:?}");
    assert!(
        err.to_string().contains("Unexpected field") && err.to_string().contains("base_pop_cap"),
        "message should name the unknown field: {err}"
    );
    assert!(
        err.to_string().contains("rules/alpha.ron"),
        "message should name the file: {err}"
    );
}

#[test]
fn newer_schema_versions_are_rejected_precisely() {
    let future_entity = WORKER.replace("schema_version: 1", "schema_version: 2");
    let err = ContentTree::load(&factory().entity("worker.ron", &future_entity)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::Version {
            found: 2,
            supported: 1,
            ..
        }
    ));
    let future_map =
        map_text(&dot_grid(), false, None).replace("schema_version: 2", "schema_version: 3");
    let err = ContentTree::load(&factory().map("testing.ron", &future_map)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::Version {
            found: 3,
            supported: 2,
            ..
        }
    ));
}

#[test]
fn version_zero_is_rejected() {
    let zero = WORKER.replace("schema_version: 1", "schema_version: 0");
    let err = ContentTree::load(&factory().entity("worker.ron", &zero)).unwrap_err();
    assert!(matches!(err, ContentError::Version { found: 0, .. }));
}

#[test]
fn map_v1_migrates_forward_with_no_heightmap() {
    // A version-1 map (the pre-ADR-0001 shape): identical fields, no heightmap.
    let v1 = map_text(&dot_grid(), false, None).replace("schema_version: 2", "schema_version: 1");
    let sources = factory().map("testing.ron", &v1);
    let tree = ContentTree::load(&sources).unwrap();
    let map = tree.map("testing").unwrap();
    assert!(map.heightmap.is_none(), "v1 migrates to a flat v2 map");

    // And the migration is invisible to identity: a v1 file and a v2 file with
    // the same content hash the same.
    let v2_flat = map_text(&dot_grid(), false, None);
    let sources = factory().map("testing.ron", &v2_flat);
    let tree_v2 = ContentTree::load(&sources).unwrap();
    let a = tree.single_map_bundle().unwrap();
    let b = tree_v2.single_map_bundle().unwrap();
    assert_eq!(a.content_hash(), b.content_hash());
}

#[test]
fn heightmap_participates_in_the_content_hash() {
    let flat = factory();
    let hilly = factory().map(
        "testing.ron",
        &map_text(&dot_grid(), false, Some(&flat_heightmap())),
    );
    let a = ContentTree::load(&flat)
        .unwrap()
        .single_map_bundle()
        .unwrap();
    let b = ContentTree::load(&hilly)
        .unwrap()
        .single_map_bundle()
        .unwrap();
    assert_ne!(a.content_hash(), b.content_hash());
}

// ---------------------------------------------------------------------------
// Ids and duplicates
// ---------------------------------------------------------------------------

#[test]
fn declared_id_must_match_the_file_name() {
    let renamed = WORKER.replace("id: \"worker\"", "id: \"peasant\"");
    let err = ContentTree::load(&factory().entity("worker.ron", &renamed)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::IdMismatch { ref declared, ref stem, .. } if declared == "peasant" && stem == "worker"
    ));
}

#[test]
fn duplicate_capability_is_rejected() {
    let doubled = WORKER.replace(
        "Gather(( carry_amount: 10, gather_time_ms: 2000 )),",
        "Gather(( carry_amount: 10, gather_time_ms: 2000 )),\n        Gather(( carry_amount: 5, gather_time_ms: 1000 )),",
    );
    let err = ContentTree::load(&factory().entity("worker.ron", &doubled)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::DuplicateCapability { ref entity, ref capability, .. } if entity == "worker" && capability == "Gather"
    ));
}

#[test]
fn duplicate_source_names_are_rejected_with_both_files() {
    // `SourceMap` has public fields, so a caller can hand the loader the same
    // file twice — the duplicate-id guard catches it. (File stems on disk are
    // unique, so the directory reader can never produce this.)
    let mut sources = factory();
    sources.entities.push(crate::Source::new("base.ron", BASE));
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(matches!(
        err,
        ContentError::DuplicateId { ref id, ref first, ref second, .. }
            if id == "base" && first == "entities/base.ron" && second == "entities/base.ron"
    ));
}

// ---------------------------------------------------------------------------
// Entity stat validation
// ---------------------------------------------------------------------------

#[test]
fn invalid_stats_are_rejected_with_field_and_reason() {
    let bad = WORKER.replace("max: 40", "max: 0");
    let err = ContentTree::load(&factory().entity("worker.ron", &bad)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::InvalidStat { ref entity, ref field, .. }
            if entity == "worker" && field == "Health.max"
    ));

    let bad = BASE.replace("radius_milli_tiles: 6000", "radius_milli_tiles: 0");
    let err = ContentTree::load(&factory().entity("base.ron", &bad)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::InvalidStat { ref field, .. } if field == "Vision.radius_milli_tiles"
    ));
}

#[test]
fn unknown_resource_references_are_rejected() {
    let bad = ORE_NODE.replace("resource: \"ore\"", "resource: \"gold\"");
    let err = ContentTree::load(&factory().entity("ore_node.ron", &bad)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::UnknownResourceRef { ref reference, .. } if reference == "gold"
    ));
}

#[test]
fn unknown_requires_references_are_rejected() {
    let bad = WORKER.replace("\"base\"", "\"castle\"");
    let err = ContentTree::load(&factory().entity("worker.ron", &bad)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::UnknownEntityRef { ref reference, .. } if reference == "castle"
    ));
}

// ---------------------------------------------------------------------------
// Faction validation
// ---------------------------------------------------------------------------

#[test]
fn production_requires_the_produce_capability() {
    let no_produce = BASE.replace("Produce(()),", "");
    let err = ContentTree::load(&factory().entity("base.ron", &no_produce)).unwrap_err();
    assert!(matches!(err, ContentError::InvalidFaction { .. }));
    assert!(
        err.to_string().contains("Produce"),
        "message should explain the missing capability: {err}"
    );
}

#[test]
fn starting_forces_may_not_be_resource_nodes() {
    let bad = FACTION
        .replace(
            "roster: [ \"base\", \"worker\" ]",
            "roster: [ \"base\", \"worker\", \"ore_node\" ]",
        )
        .replace(
            "( entity: \"worker\", offset: ( x: -1, y: 3 ) ),",
            "( entity: \"ore_node\", offset: ( x: -1, y: 3 ) ),",
        );
    let err = ContentTree::load(&factory().faction("testers.ron", &bad)).unwrap_err();
    assert!(matches!(err, ContentError::InvalidFaction { .. }));
    assert!(
        err.to_string().contains("resource node"),
        "message should explain that maps place nodes: {err}"
    );
}

#[test]
fn roster_references_must_exist() {
    let bad = FACTION.replace("\"base\", \"worker\"", "\"base\", \"tank\"");
    let err = ContentTree::load(&factory().faction("testers.ron", &bad)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::UnknownEntityRef { ref reference, .. } if reference == "tank"
    ));
}

// ---------------------------------------------------------------------------
// Map file validation
// ---------------------------------------------------------------------------

#[test]
fn grid_shape_and_codes_are_validated() {
    let short_row: Vec<String> = vec!["..............".to_string(); 16]; // 14 wide
    let err = ContentTree::load(&factory().map("testing.ron", &map_text(&short_row, false, None)))
        .unwrap_err();
    assert!(matches!(err, ContentError::InvalidMap { .. }));
    assert!(
        err.to_string().contains("row 0 has 14 characters"),
        "message should name the row and width: {err}"
    );
}

#[test]
fn unknown_grid_codes_are_rejected_with_position() {
    let mut grid = dot_grid();
    rock(&mut grid, 7, 7); // '#' without the rock legend entry
    let sources = factory().map("testing.ron", &map_without_rock_legend(&grid));
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(matches!(err, ContentError::InvalidMap { .. }));
    assert!(
        err.to_string().contains("row 7 column 7"),
        "message should name the cell: {err}"
    );
}

/// The factory map minus the rock legend entry (so '#' becomes unknown).
fn map_without_rock_legend(grid: &[String]) -> String {
    map_text(grid, false, None).replace(
        "        ( code: \"#\", name: \"rock\", passable: false, buildable: false ),\n",
        "",
    )
}

#[test]
fn starts_are_validated_for_bounds_and_duplicates() {
    let out_of_bounds =
        map_text(&dot_grid(), false, None).replace("( x: 10, y: 10 )", "( x: 15, y: 15 )"); // 2x2 base exceeds 16
    let err = ContentTree::load(&factory().map("testing.ron", &out_of_bounds)).unwrap_err();
    assert!(matches!(err, ContentError::IllegalStart { player: 1, .. }));
    assert!(
        err.to_string().contains("outside"),
        "message should say the footprint leaves the map: {err}"
    );

    let dup = map_text(&dot_grid(), false, None).replace("( player: 1,", "( player: 0,");
    let err = ContentTree::load(&factory().map("testing.ron", &dup)).unwrap_err();
    assert!(matches!(err, ContentError::InvalidMap { .. }));
    assert!(
        err.to_string().contains("more than one start"),
        "message should name the duplicated player: {err}"
    );
}

#[test]
fn ore_nodes_are_validated_for_kind_bounds_and_overlap() {
    let unknown_kind = map_text(&dot_grid(), false, None).replace(
        "( kind: \"ore_node\", tile: ( x: 13, y: 13 ) )",
        "( kind: \"gold_node\", tile: ( x: 13, y: 13 ) )",
    );
    let err = ContentTree::load(&factory().map("testing.ron", &unknown_kind)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::UnknownEntityRef { ref reference, .. } if reference == "gold_node"
    ));

    let overlapping =
        map_text(&dot_grid(), false, None).replace("( x: 13, y: 13 )", "( x: 15, y: 15 )"); // 2x2 exceeds bounds
    let err = ContentTree::load(&factory().map("testing.ron", &overlapping)).unwrap_err();
    assert!(matches!(err, ContentError::InvalidMap { .. }));
    assert!(
        err.to_string().contains("outside"),
        "message should say the node leaves the map: {err}"
    );
}

#[test]
fn heightmap_shape_is_validated() {
    // Row 0 carries only 15 entries.
    let short_row = format!("[{}]", vec!["0"; 15].join(", "));
    let bad_heightmap = format!("[{}]", vec![short_row; 16].join(", "));
    let text = map_text(&dot_grid(), false, Some(&bad_heightmap));
    let err = ContentTree::load(&factory().map("testing.ron", &text)).unwrap_err();
    assert!(matches!(err, ContentError::InvalidMap { .. }));
    assert!(
        err.to_string().contains("heightmap row 0 has 15 entries"),
        "message should name the row and width: {err}"
    );
}

// ---------------------------------------------------------------------------
// Placement validation (plan §10.5)
// ---------------------------------------------------------------------------

#[test]
fn structures_may_not_spawn_on_unbuildable_ground() {
    let mut grid = dot_grid();
    rock(&mut grid, 5, 5); // inside player 0's base footprint (4..5, 4..5)
    let text = map_text(&grid, false, None);
    let err = ContentTree::load(&factory().map("testing.ron", &text)).unwrap_err();
    assert!(matches!(err, ContentError::IllegalStart { player: 0, .. }));
    assert!(
        err.to_string().contains("not buildable"),
        "message should name the terrain problem: {err}"
    );
}

#[test]
fn units_may_not_spawn_inside_static_footprints() {
    let bad = FACTION.replace("( x: -1, y: 3 )", "( x: -3, y: -3 )"); // lands on the ore node at (1,1)
    let err = ContentTree::load(&factory().faction("testers.ron", &bad)).unwrap_err();
    assert!(matches!(err, ContentError::IllegalStart { player: 0, .. }));
    assert!(
        err.to_string().contains("inside"),
        "message should say the tile is occupied: {err}"
    );
}

#[test]
fn unreachable_ore_is_rejected_per_start() {
    // Wall the node at (1,1) in completely: ring x 0..=3, y 0..=3 minus node.
    let mut grid = dot_grid();
    for x in 0..=3 {
        for y in 0..=3 {
            let inside_node = (1..=2).contains(&x) && (1..=2).contains(&y);
            if !inside_node {
                rock(&mut grid, x, y);
            }
        }
    }
    let text = map_text(&grid, false, None);
    let err = ContentTree::load(&factory().map("testing.ron", &text)).unwrap_err();
    assert!(matches!(
        err,
        ContentError::OreUnreachable {
            x: 1,
            y: 1,
            player: 0,
            ..
        }
    ));
}

#[test]
fn declared_symmetry_is_checked_and_breaks_loudly() {
    // The factory nodes at (1,1) and (13,13) mirror exactly (a 2x2 at (x,y)
    // mirrors to (14-x, 14-y) on a 16x16 map), so symmetry passes as declared.
    let text = map_text(&dot_grid(), true, None);
    let tree = ContentTree::load(&factory().map("testing.ron", &text)).unwrap();
    assert!(tree.map("testing").unwrap().symmetric);

    // Moving one node off its mirror point breaks the declaration.
    let broken = map_text(&dot_grid(), true, None).replace("( x: 13, y: 13 )", "( x: 12, y: 12 )");
    let err = ContentTree::load(&factory().map("testing.ron", &broken)).unwrap_err();
    assert!(matches!(err, ContentError::SymmetryBroken { .. }));
    assert!(
        err.to_string().contains("ore node"),
        "message should say which part is asymmetric: {err}"
    );
}

#[test]
fn asymmetric_terrain_breaks_declared_symmetry() {
    let mut grid = dot_grid();
    rock(&mut grid, 8, 2); // rotation (7, 13) stays ground
    let text = map_text(&grid, true, None);
    let err = ContentTree::load(&factory().map("testing.ron", &text)).unwrap_err();
    assert!(matches!(err, ContentError::SymmetryBroken { .. }));
    assert!(
        err.to_string().contains("(8,2)"),
        "message should name the asymmetric cell: {err}"
    );
}

// ---------------------------------------------------------------------------
// Hash identity and the sim seam
// ---------------------------------------------------------------------------

#[test]
fn loading_is_deterministic_and_identity_separates_content() {
    let a = ContentTree::load(&factory()).unwrap();
    let b = ContentTree::load(&factory()).unwrap();
    let bundle_a = a.single_map_bundle().unwrap();
    let bundle_b = b.single_map_bundle().unwrap();
    assert_eq!(bundle_a.content_hash(), bundle_b.content_hash());
    assert_eq!(bundle_a.map_id(), bundle_b.map_id());
    assert_eq!(bundle_a.world(), bundle_b.world());

    // Any authored change is a different match.
    let tweaked = WORKER.replace("max: 40", "max: 41");
    let c = ContentTree::load(&factory().entity("worker.ron", &tweaked))
        .unwrap()
        .single_map_bundle()
        .unwrap();
    assert_ne!(bundle_a.content_hash(), c.content_hash());
}

#[test]
fn world_maps_content_into_the_simulation_vocabulary() {
    let bundle = ContentTree::load(&factory())
        .unwrap()
        .single_map_bundle()
        .unwrap();
    let world = bundle.world();

    // Entities are sorted by id: base, ore_node, worker -> kinds 0, 1, 2.
    assert_eq!(world.kinds.len(), 3);
    assert_eq!(world.map_id, bundle.map_id());
    assert_eq!(world.width_tiles, 16);
    assert_eq!(world.height_tiles, 16);
    assert_eq!(world.spawn_jitter_milli, 0);
    assert!(world.scheduled_spawns.is_empty());

    // Resources carry the registry with starting balances.
    assert_eq!(world.resources.len(), 1);

    // Spawn order: starts in authored order, forces in faction order, then the
    // map's ore nodes in authored order. Positions are footprint centers in
    // fixed-point tile units (milli-tiles shown as the authored input).
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::{PlayerId, Vec2Fx};
    let milli = |x: i32, y: i32| Vec2Fx::new(Fx::from_milli(x), Fx::from_milli(y));
    let spawns: Vec<(u8, u32, Vec2Fx)> = world
        .initial_spawns
        .iter()
        .map(|spawn| (spawn.owner.0, spawn.kind.0, spawn.pos))
        .collect();
    assert_eq!(
        spawns,
        vec![
            // player 0: base (2x2 at (4,4) -> center (5,5)) then workers.
            (0, 0, milli(5000, 5000)),
            (0, 2, milli(3500, 7500)), // worker at tile (3,7) -> center
            (0, 2, milli(5500, 7500)), // worker at tile (5,7) -> center
            // player 1: base at (10,10) -> center (11,11).
            (1, 0, milli(11000, 11000)),
            (1, 2, milli(9500, 13500)),
            (1, 2, milli(11500, 13500)),
            // ore nodes, neutral, at footprint centers.
            (PlayerId::NEUTRAL.0, 1, milli(2000, 2000)),
            (PlayerId::NEUTRAL.0, 1, milli(14000, 14000)),
        ]
    );

    // Capability mapping (the M5 economy set rides along): worker carries
    // Health + Move + Gather + Vision (this factory's worker has no Build);
    // base carries Health + Vision + Footprint + Produce; ore_node carries
    // Resource + Footprint.
    assert_eq!(world.kinds[2].caps.len(), 4);
    assert_eq!(world.kinds[0].caps.len(), 4);
    assert_eq!(world.kinds[1].caps.len(), 2);
    assert!(world.kinds[1]
        .caps
        .iter()
        .any(|cap| matches!(cap, pandemonium_sim::CapTemplate::Resource { .. })));

    // And the seam runs: the simulation accepts the loaded world as-is.
    use pandemonium_sim::Sim;
    use pandemonium_sim_api::{ControllerKind, MatchSetup, PlayerSetup};
    let setup = MatchSetup {
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
    };
    let sim = Sim::new(&world, setup);
    assert_eq!(sim.snapshot().entities.len(), 8);
    assert_eq!(sim.next_entity_id(), 9);
}

#[test]
fn bundle_requires_exactly_one_rules_file_and_rejects_unknown_maps() {
    let err = ContentTree::load(&factory().rules("beta.ron", RULES)).unwrap_err();
    assert!(matches!(err, ContentError::Bundle { .. }));
    assert!(
        err.to_string().contains("exactly one rules file"),
        "message should state the count rule: {err}"
    );

    let tree = ContentTree::load(&factory()).unwrap();
    let err = tree.bundle("nonexistent").unwrap_err();
    assert!(matches!(err, ContentError::Bundle { .. }));
    assert!(err.to_string().contains("nonexistent"));
}

#[test]
fn directory_loading_is_sorted_and_deterministic() {
    let root = temp_root("sorted-load");
    write_tree(
        &root,
        &[
            ("rules/alpha.ron", RULES),
            ("entities/base.ron", BASE),
            ("entities/worker.ron", WORKER),
            ("entities/ore_node.ron", ORE_NODE),
            ("factions/testers.ron", FACTION),
            ("maps/testing.ron", &map_text(&dot_grid(), false, None)),
            ("maps/notes.md", "# designer notes — not content"),
        ],
    );
    let a = ContentBundle::load_dir(&root).unwrap();
    let b = ContentBundle::load_dir(&root).unwrap();
    assert_eq!(a.content_hash(), b.content_hash());
    assert_eq!(a.world(), b.world());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_category_directories_are_precise_errors() {
    let root = temp_root("missing-maps");
    write_tree(
        &root,
        &[
            ("rules/alpha.ron", RULES),
            ("entities/base.ron", BASE),
            ("factions/testers.ron", FACTION),
        ],
    );
    let err = ContentBundle::load_dir(&root).unwrap_err();
    assert!(
        matches!(err, ContentError::MissingDirectory { ref category, .. } if category == "maps")
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pandemonium-content-{tag}-{}-{}",
        std::process::id(),
        crate::ms_to_ticks(1, 1000) // a stable-ish extra disambiguator
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn write_tree(root: &std::path::Path, files: &[(&str, &str)]) {
    for (relative, text) in files {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("nested path")).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

#[test]
fn economy_stats_and_production_flow_through_the_seam() {
    let bundle = ContentTree::load(&factory())
        .unwrap()
        .single_map_bundle()
        .unwrap();
    let world = bundle.world();
    use pandemonium_sim::CapTemplate;
    use pandemonium_sim_api::{KindId, ResourceId};

    // Worker (kind 2): 50 Ore, 12000 ms -> 360 ticks, pop 1, requires base.
    let worker = &world.kinds[2];
    assert_eq!(
        worker.economy.cost,
        vec![(ResourceId(0), 50)],
        "cost.ore maps onto the registry's Ore id"
    );
    assert_eq!(worker.economy.build_time_ticks, 360);
    assert_eq!(worker.economy.population, 1);
    assert_eq!(worker.economy.requires, vec![KindId(0)]);

    // Base (kind 0): 100 Ore, 10000 ms -> 300 ticks, no population, no needs.
    let base = &world.kinds[0];
    assert_eq!(base.economy.cost, vec![(ResourceId(0), 100)]);
    assert_eq!(base.economy.build_time_ticks, 300);
    assert_eq!(base.economy.population, 0);
    assert!(base.economy.requires.is_empty());

    // Ore node (kind 1): free, instant, requirement-free.
    let node = &world.kinds[1];
    assert!(node.economy.cost.is_empty());
    assert_eq!(node.economy.build_time_ticks, 0);

    // The Gather parameters ride on the worker's capability (A-025).
    assert!(worker.caps.iter().any(|cap| matches!(
        cap,
        CapTemplate::Gather {
            carry_amount: 10,
            gather_time_ms: 2000,
        }
    )));

    // Production lists: the base trains workers (faction data -> the seam).
    assert_eq!(world.production, vec![(KindId(0), vec![KindId(2)])]);

    // The base population cap comes from the rules.
    assert_eq!(world.base_population_cap, 0);
}

#[test]
fn cost_requires_the_priced_resource_in_the_registry() {
    // A ruleset without Ore cannot price anything in Ore.
    let no_ore = RULES.replace(
        r#"resources: [ ( id: "ore", display_name: "Ore", starting: 200 ) ]"#,
        r#"resources: [ ( id: "gold", display_name: "Gold", starting: 200 ) ]"#,
    );
    let err = ContentTree::load(&factory().rules("alpha.ron", &no_ore)).unwrap_err();
    assert!(
        matches!(err, ContentError::UnknownResourceRef { ref what, .. } if what.contains("cost.ore")),
        "expected a cost.ore resource rejection, got: {err}"
    );
}
