//! The RON-facing schema (plan §10): what content files look like on disk.
//!
//! These are the *raw* deserialization types — one per file kind, plus the map's
//! two historical versions. They exist only to be parsed, migrated forward, and
//! converted into the canonical validated types in [`crate::defs`]. Everything
//! here is integer/string/bool: authored content carries no floating-point types
//! (FD-5, plan §10.2 — durations in milliseconds, distances in milli-tiles,
//! counts as plain integers).
//!
//! Strict mode (plan §10.2: "unknown fields are errors in strict validation
//! mode") is the only mode: every struct denies unknown fields, so a typo'd field
//! name is a precise parse error, never a silently-ignored value.

use std::collections::BTreeMap;

/// `schema_version` supported by this build for entity files.
pub(crate) const ENTITY_SCHEMA_VERSION: u32 = 1;
/// `schema_version` supported by this build for faction files.
pub(crate) const FACTION_SCHEMA_VERSION: u32 = 1;
/// `schema_version` supported by this build for rules files.
pub(crate) const RULES_SCHEMA_VERSION: u32 = 1;
/// `schema_version` supported by this build for map files. Version 2 added the
/// optional display-only heightmap (ADR-0001); version 1 files migrate forward.
pub(crate) const MAP_SCHEMA_VERSION: u32 = 2;

/// Extracts only `schema_version` from a file so the loader can pick the right
/// raw type (and migrate) before the strict parse. Deliberately lenient: every
/// other field is ignored here and checked by the real schema.
#[derive(serde::Deserialize)]
pub(crate) struct VersionHeader {
    /// The version the file declares.
    pub schema_version: u32,
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// The rules file: the resource registry, global economy parameters, and match
/// condition parameters (plan §10.1 — victory evaluation itself is code).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRules {
    /// File schema version.
    pub schema_version: u32,
    /// Stable ruleset id (must equal the file name stem).
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// The resource registry (plan §9.3); ledgers start with these balances.
    pub resources: Vec<RawResource>,
    /// Population cap before structures add to it (plan §10.4: starts at 0).
    pub base_population_cap: u32,
    /// The victory condition parameters; code evaluates them (M8).
    pub victory: RawVictory,
}

/// One registered resource (Alpha has one: Ore).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawResource {
    /// Stable resource id ("ore").
    pub id: String,
    /// Human-facing name ("Ore").
    pub display_name: String,
    /// Every player's starting balance (plan §10.4: 200).
    pub starting: i64,
}

/// The victory condition selection (plan §10.1: match condition parameters are
/// data; the evaluation is code).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawVictory {
    /// Which condition ends the match ("elimination" in the Alpha).
    pub kind: String,
    /// Whether resignation ends the match for the resigner (plan §10.4).
    pub allow_resignation: bool,
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

/// One entity kind as a composition of capabilities (plan §10.3's shape, verbatim).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEntity {
    /// File schema version.
    pub schema_version: u32,
    /// Stable entity id (must equal the file name stem).
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// The capability blocks this kind carries, in authored order.
    pub capabilities: Vec<RawCapability>,
    /// The build/train cost (plan §10.3).
    pub cost: RawCost,
    /// Build (structure) or train (unit) time in milliseconds (plan §10.2).
    pub build_time_ms: u32,
    /// Population cost when this kind is fielded (plan §10.4).
    pub population: i32,
    /// Entity ids that must exist before this one can be produced/built
    /// (plan §9.4 requirements).
    pub requires: Vec<String>,
}

/// Costs in resource units (plan §10.3 `cost: ( ore: 75 )`).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCost {
    /// Ore cost.
    pub ore: i64,
}

/// A capability block in authoring units. Code defines the behavior; data only
/// selects and parameterizes (plan §10.1). The M2 schema carries the full Alpha
/// vocabulary (plan §7.4/§10.4) even though the simulation only consumes
/// Health/Move/Vision until the economy (M5) and combat (M6) milestones land.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) enum RawCapability {
    /// Hit-point pool.
    Health(RawHealth),
    /// Locomotion and collision radius.
    Move(RawMove),
    /// Immediate-hit attack (plan §9.2 pipeline parameterized).
    Attack(RawAttack),
    /// Resource harvesting.
    Gather(RawGather),
    /// Can construct structures.
    Build(RawBuild),
    /// Owns a production queue.
    Produce(RawProduce),
    /// Accepts resource deposits.
    Storage(RawStorage),
    /// Adds to the population cap.
    ProvidesPopulation(RawProvidesPopulation),
    /// A finite resource node (placed by maps).
    Resource(RawResourceNode),
    /// Perception radius for fog and targeting.
    Vision(RawVision),
    /// Static tile occupancy (structures and nodes).
    Footprint(RawFootprint),
}

/// Health parameters.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawHealth {
    /// Maximum hit points.
    pub max: i32,
    /// Hit points regenerated (negative: lost) per tick; optional, default 0.
    #[serde(default)]
    pub regen_per_tick: i32,
}

/// Movement parameters.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMove {
    /// Top speed in milli-tiles per second (plan §10.2).
    pub speed_milli_tiles_per_s: i32,
    /// Collision radius in milli-tiles (plan §9.1 layer 3).
    pub radius_milli_tiles: i32,
}

/// Attack parameters.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAttack {
    /// Integer damage per hit.
    pub damage: i32,
    /// Attack range in milli-tiles.
    pub range_milli_tiles: i32,
    /// Cooldown in milliseconds (converted to ticks at load — plan §10.2).
    pub cooldown_ms: u32,
    /// Target acquisition range in milli-tiles.
    pub acquire_range_milli_tiles: i32,
}

/// Gathering parameters (plan §10.4: 10 Ore per trip, 2000 ms per gather).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawGather {
    /// How much one full trip carries.
    pub carry_amount: i64,
    /// Time one gather cycle takes, in milliseconds.
    pub gather_time_ms: u32,
}

/// Build capability (marker — parameters arrive with construction systems, M5).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBuild {}

/// Produce capability (marker — the queue model arrives with M5; which kinds a
/// producer may train is faction data, plan §10.6).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProduce {}

/// Storage capability: which resources this entity accepts as deposits.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawStorage {
    /// Storable resource ids.
    pub resources: Vec<String>,
}

/// Population provision.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProvidesPopulation {
    /// Cap added while this entity stands.
    pub amount: i32,
}

/// A finite resource node body.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawResourceNode {
    /// Which resource this node holds.
    pub resource: String,
    /// How much remains before depletion.
    pub amount: i64,
}

/// Vision parameters.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawVision {
    /// Sight radius in milli-tiles.
    pub radius_milli_tiles: i32,
}

/// Static footprint size in tiles.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFootprint {
    /// Footprint width in tiles.
    pub w: u32,
    /// Footprint height in tiles.
    pub h: u32,
}

// ---------------------------------------------------------------------------
// Factions
// ---------------------------------------------------------------------------

/// A faction: a bundle of entity references (plan §10.1), what its producers can
/// train, and the forces it starts with relative to a map start anchor.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFaction {
    /// File schema version.
    pub schema_version: u32,
    /// Stable faction id (must equal the file name stem).
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// Entity ids this faction can field.
    pub roster: Vec<String>,
    /// Producer entity id -> the kinds it may train (plan §10.6 "a faction's
    /// production list"). An ordered map keeps the data deterministic.
    pub production: BTreeMap<String, Vec<String>>,
    /// The forces this faction starts with, placed relative to the start anchor
    /// in tiles (plan §9.7: start conditions from map data + faction data).
    pub starting_forces: Vec<RawStartingForce>,
}

/// One starting force entry.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawStartingForce {
    /// The entity id to spawn.
    pub entity: String,
    /// Tile offset from the map's start anchor (the anchor is this force group's
    /// reference corner).
    pub offset: RawTile,
}

/// A whole-tile position (plan §8.1 `TilePos` in authoring form).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTile {
    /// Tile x.
    pub x: i32,
    /// Tile y.
    pub y: i32,
}

// ---------------------------------------------------------------------------
// Maps
// ---------------------------------------------------------------------------

/// A start position: a player slot plus the anchor tile its faction's starting
/// forces are placed relative to (plan §10.5).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawStart {
    /// The player slot this start belongs to.
    pub player: u8,
    /// The anchor tile.
    pub anchor: RawTile,
}

/// An ore node placement: which resource-node kind spawns at which tile.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawOreNode {
    /// The entity id of the node kind (e.g. "ore_node"; its Resource capability
    /// carries the amount and its Footprint the size).
    pub kind: String,
    /// The node's tile (top-left of its footprint).
    pub tile: RawTile,
}

/// One terrain class in the map legend (plan §10.5: passability classes as data).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTerrain {
    /// The single-character grid code.
    pub code: String,
    /// Human-facing terrain name.
    pub name: String,
    /// Whether units may traverse tiles of this class.
    pub passable: bool,
    /// Whether structures may be placed on tiles of this class.
    pub buildable: bool,
}

/// The map schema, version 1 — the pre-ADR-0001 shape (no heightmap). Kept so
/// old content keeps loading (FD-10); [`crate::version`] migrates it forward.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMapV1 {
    /// File schema version (always 1 in this shape).
    pub schema_version: u32,
    /// Stable map id (must equal the file name stem).
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// Map width in tiles.
    pub width: u32,
    /// Map height in tiles.
    pub height: u32,
    /// The terrain legend.
    pub terrain: Vec<RawTerrain>,
    /// The grid: one string per row, one character per tile.
    pub grid: Vec<String>,
    /// The start positions.
    pub starts: Vec<RawStart>,
    /// The ore node placements.
    pub ore_nodes: Vec<RawOreNode>,
    /// Whether the symmetry validator should check this map.
    pub symmetric: bool,
}

/// The map schema, version 2 — adds the optional display-only heightmap
/// (ADR-0001). The heightmap never affects gameplay: the simulation receives no
/// part of it.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMapV2 {
    /// File schema version.
    pub schema_version: u32,
    /// Stable map id (must equal the file name stem).
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// Map width in tiles.
    pub width: u32,
    /// Map height in tiles.
    pub height: u32,
    /// The terrain legend.
    pub terrain: Vec<RawTerrain>,
    /// The grid: one string per row, one character per tile.
    pub grid: Vec<String>,
    /// The start positions.
    pub starts: Vec<RawStart>,
    /// The ore node placements.
    pub ore_nodes: Vec<RawOreNode>,
    /// Whether the symmetry validator should check this map.
    pub symmetric: bool,
    /// Display-only per-tile heights (ADR-0001); absent means flat terrain.
    #[serde(default)]
    pub heightmap: Option<RawHeightmap>,
}

/// The display-only heightmap (ADR-0001): one row of `width` integer heights per
/// map row. Consumed exclusively by the renderer; never part of simulation
/// state, passability, vision, or combat.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawHeightmap {
    /// The height rows, top to bottom. Each row has exactly `width` entries.
    pub rows: Vec<Vec<u16>>,
}

// ---------------------------------------------------------------------------
// Missions (ADR-0201 §3)
// ---------------------------------------------------------------------------

/// `schema_version` supported by this build for mission files (ADR-0201 §3:
/// the mission schema is versioned and forward-only like every other kind).
pub(crate) const MISSION_SCHEMA_VERSION: u32 = 1;

/// A mission file (ADR-0201 §3): a closed reference set into the content tree
/// — the map, the faction, the rules — plus the ordered objective list
/// (ADR-0201 §2). Phase 1 carries the destroy and build/train kinds; the
/// vocabulary grows forward-only with the schema version (survive and
/// protect landed with Phase 2's B-017, reach and gather with B-018 —
/// ADR-0201 §9's Phase 2 objective set is complete).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMission {
    /// File schema version.
    pub schema_version: u32,
    /// Stable mission id (must equal the file name stem).
    pub id: String,
    /// Human-facing name (ASCII only — the embedded font constraint,
    /// ADR-0201 §5).
    pub display_name: String,
    /// The map this mission runs on (a map id in the same tree).
    pub map: String,
    /// The faction the mission player runs (a faction id in the same tree).
    pub faction: String,
    /// The ruleset the mission runs under (a rules id in the same tree).
    pub rules: String,
    /// The fog preset spelling (`"Default"`, `"Explored"`, or `"Visible"`;
    /// B-015). Optional, default `"Default"` — the field is Phase 2
    /// forward-only (FD-10): absent keeps the historical all-Hidden
    /// initialization. The loader validates the spelling against the
    /// canonical vocabulary; unknown spellings are loud errors.
    #[serde(default)]
    pub fog_preset: Option<String>,
    /// The faction variant overrides (B-019, ADR-0201 §3's "restrictions are
    /// mission data"): an optional inline block that restricts or reshapes
    /// the mission faction — no variant faction file is added to the tree
    /// (the owner's binding decision: overrides are inline in the mission
    /// file). `roster` replaces the base roster (a subset of it);
    /// `starting_forces` replaces the base starting forces. Default is the
    /// all-`None` block — behaviorally the base faction, and the loader
    /// normalizes it to `None` on the canonical mission (the identity rule:
    /// a no-op block keeps exactly the mission id a mission without the
    /// block has).
    #[serde(default)]
    pub faction_overrides: RawFactionOverrides,
    /// The rules variant overrides (B-020, ADR-0201 §3's "restrictions are
    /// mission data"): an optional inline block that overrides plain integer
    /// fields of the mission's ruleset — the starting ore balance and the
    /// base population cap. No variant rules file is added to the tree (the
    /// owner's binding decision: overrides are inline in the mission file).
    /// Default is the all-`None` block — behaviorally the base ruleset, and
    /// the loader normalizes it to `None` on the canonical mission (the
    /// identity rule: a no-op block keeps exactly the mission id a mission
    /// without the block has).
    #[serde(default)]
    pub rules_overrides: RawRulesOverrides,
    /// The ordered objective list, AND-composed (ADR-0201 §2).
    pub objectives: Vec<RawObjective>,
}

/// The faction variant overrides as authored (B-019): the inline,
/// mission-carried reshaping of the mission faction. Every sub-field is
/// optional — an override block names only what it changes, and the
/// all-`None` default block is the base faction (the loader normalizes it
/// to `None` on the canonical mission, so a no-op block and an absent
/// block are the same mission, id included).
#[derive(serde::Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFactionOverrides {
    /// The replacement roster: the entity ids the mission faction can field.
    /// Must be a subset of the base faction's roster (the tree-level gate;
    /// the loader refuses a kind the base faction does not field — an
    /// override restricts, it never adds). `None` keeps the base roster.
    #[serde(default)]
    pub roster: Option<Vec<String>>,
    /// The replacement starting forces, placed relative to the map start
    /// anchors exactly like the base faction's. Every kind must exist in
    /// the tree's content; `None` keeps the base starting forces.
    #[serde(default)]
    pub starting_forces: Option<Vec<RawStartingForce>>,
}

/// The rules variant overrides as authored (B-020): the inline,
/// mission-carried reshaping of the mission's ruleset. Only plain integer
/// fields may be overridden: the starting ore balance and the base
/// population cap. Every sub-field is optional — an override block names
/// only what it changes, and the all-`None` default block is the base
/// ruleset (the loader normalizes it to `None` on the canonical mission,
/// so a no-op block and an absent block are the same mission, id included).
#[derive(serde::Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRulesOverrides {
    /// Override for the starting ore balance (resources[ore].starting).
    /// `None` keeps the base ruleset's starting ore.
    #[serde(default)]
    pub starting_ore: Option<i64>,
    /// Override for the base population cap (base_population_cap).
    /// `None` keeps the base ruleset's base population cap.
    #[serde(default)]
    pub base_population_cap: Option<u32>,
}

/// One objective as authored: the kind spelling, the parameters block, and
/// whether the objective is optional. Optional objectives never gate
/// `MatchEnded`; they feed the end screen and story hooks (ADR-0201 §2).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawObjective {
    /// The objective kind spelling ("Destroy", "BuildTrain", "Survive",
    /// "Protect", "ReachArea", or "Gather"; unknown spellings are loud
    /// errors, never silently mapped).
    pub kind: String,
    /// The kind's parameters block.
    pub parameters: RawObjectiveParameters,
    /// Whether the objective is optional.
    pub optional: bool,
}

/// Objective parameters, one variant per kind — the same authoring shape as
/// capability blocks (`Health(( max: 500 ))`).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) enum RawObjectiveParameters {
    /// Destroy parameters: `None` = all enemy structures (today's elimination
    /// rule); `Some(kind)` = every enemy structure of that kind.
    Destroy(RawDestroyParams),
    /// Build/train parameters: which kind, and how many.
    BuildTrain(RawBuildTrainParams),
    /// Survive parameters: the tick count the mission tick must reach
    /// (B-017). Integer ticks (FD-5); the loader rejects windows below the
    /// 300-tick floor.
    Survive(RawSurviveParams),
    /// Protect parameters: which entity kind the mission player must keep
    /// alive (B-017). The loader resolves the entity reference and rejects a
    /// kind the mission player's starting forces do not carry.
    Protect(RawProtectParams),
    /// Reach parameters (B-018): the area the mission player must reach
    /// with any entity — a rectangle in whole tiles or a circle in
    /// milli-tiles (ADR-0201 §2's table).
    ReachArea(RawReachParams),
    /// Gather parameters (B-018): how much ore, and which semantics the
    /// objective counts — cumulative delivered or held balance (the
    /// ADR's §2 "semantics choice: mission data picks").
    Gather(RawGatherParams),
}

/// Destroy parameters.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDestroyParams {
    /// The target structure kind, or `None` for all enemy structures.
    pub target_kind: Option<String>,
}

/// Build/train parameters.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBuildTrainParams {
    /// The entity kind to build or train.
    pub kind: String,
    /// How many must exist.
    pub count: u32,
}

/// Survive parameters (B-017).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSurviveParams {
    /// The tick the objective completes at, counted from mission start.
    pub ticks: u32,
}

/// Protect parameters (B-017).
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProtectParams {
    /// The entity kind the mission player must keep alive.
    pub kind: String,
}

/// Reach parameters (B-018): one area, authored in one of the two shapes.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawReachParams {
    /// The area to reach.
    pub area: RawAreaSpec,
}

/// An area as authored (B-018): a rectangle in whole tiles or a circle in
/// milli-tiles — plain integer geometry (FD-5). The shapes are unit-tagged
/// by variant, never converted; `defs::AreaSpec` is the canonical mirror
/// and `pandemonium_sim_api::AreaSpec` (with the containment law) is the
/// form the seam hands the simulation.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) enum RawAreaSpec {
    /// An axis-aligned rectangle in whole tiles: the half-open tile span
    /// `[x, x+w) x [y, y+h)` (left/top edges inclusive, right/bottom
    /// exclusive — the tile-cell law; see the sim-side `contains`).
    Rect(RawRectArea),
    /// A circle in milli-tiles, boundary inclusive.
    Circle(RawCircleArea),
}

/// Rectangle parameters (B-018), in tiles.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRectArea {
    /// The left edge's tile column (inclusive).
    pub x: i32,
    /// The top edge's tile row (inclusive).
    pub y: i32,
    /// The width in tiles (must be non-zero).
    pub w: u32,
    /// The height in tiles (must be non-zero).
    pub h: u32,
}

/// Circle parameters (B-018), in milli-tiles.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCircleArea {
    /// The center's x, in milli-tiles.
    pub cx: i32,
    /// The center's y, in milli-tiles.
    pub cy: i32,
    /// The radius, in milli-tiles (must be positive; boundary inclusive).
    pub radius: i32,
}

/// Gather parameters (B-018): the ore amount and the counting semantics.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawGatherParams {
    /// How much ore the objective demands. The tree-level gate rejects an
    /// amount at or below the ruleset's effective starting ore (200 in the
    /// Alpha) — the objective must demand play (AUD-003).
    pub amount: i64,
    /// The semantics spelling ("Cumulative" or "Balance"); unknown
    /// spellings are loud parse errors, never silently mapped.
    pub semantics: String,
}
