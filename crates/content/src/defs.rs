//! The canonical, validated content types — what everything downstream (the
//! bundle, the hash, the sim seam) consumes.
//!
//! Raw RON types ([`crate::schema`]) are parsed, migrated, semantically
//! validated, and converted into these. The difference matters: a `MapDef` is
//! *known good* (dimensions checked, grid chars in the legend, references
//! resolved, spawn placements legal), so later milestones can consume it without
//! re-validating. All fields are integers/strings/bools (FD-5).

/// A registered resource with its starting balance (plan §9.3, §10.4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResourceDef {
    /// Stable resource id ("ore").
    pub id: String,
    /// Human-facing name ("Ore").
    pub display_name: String,
    /// Every player's starting balance.
    pub starting: i64,
}

/// The victory condition a match evaluates (parameters are data; evaluation is
/// code — plan §10.1, milestone M8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VictoryKind {
    /// A player is defeated when they have zero structures (plan §10.4).
    Elimination,
}

impl VictoryKind {
    /// The schema spelling of this kind (what appears in the rules file).
    pub fn as_str(self) -> &'static str {
        match self {
            VictoryKind::Elimination => "elimination",
        }
    }

    /// Parses a schema spelling. Unknown spellings are `None` so the loader can
    /// reject them with a precise error — never silently mapped.
    pub fn from_schema(kind: &str) -> Option<Self> {
        match kind {
            "elimination" => Some(VictoryKind::Elimination),
            _ => None,
        }
    }
}

/// The validated rules file: resource registry, base population cap, victory
/// parameters.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RulesDef {
    /// Stable ruleset id.
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// The resource registry, sorted by id (ledger order everywhere downstream).
    pub resources: Vec<ResourceDef>,
    /// Population cap before structures add to it (plan §10.4: 0).
    pub base_population_cap: u32,
    /// Which victory condition the match evaluates.
    pub victory: VictoryKind,
    /// Whether resignation is enabled (plan §10.4).
    pub allow_resignation: bool,
    /// The source file (`rules/<id>.ron`) — carried for precise errors and tool
    /// output.
    pub file: String,
}

/// One capability block, validated, in authoring units plus the tick-converted
/// durations (plan §10.2: conversion happens once, at load).
///
/// The vocabulary is the full Alpha set (plan §7.4/§10.4). The simulation
/// consumes Health/Move/Vision today; the economy (M5) and combat (M6)
/// milestones consume the rest — the data arrives first, the systems catch up.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CapabilityDef {
    /// Hit-point pool and regeneration.
    Health {
        /// Maximum hit points.
        max_hp: i32,
        /// Hit points regenerated (negative: lost) per tick.
        regen_per_tick: i32,
    },
    /// Locomotion and collision radius.
    Move {
        /// Top speed in milli-tiles per second.
        speed_milli_tiles_per_s: i32,
        /// Collision radius in milli-tiles (plan §9.1).
        radius_milli_tiles: i32,
    },
    /// Immediate-hit attack parameters.
    Attack {
        /// Integer damage per hit.
        damage: i32,
        /// Attack range in milli-tiles.
        range_milli_tiles: i32,
        /// Cooldown in milliseconds, as authored.
        cooldown_ms: u32,
        /// Cooldown in whole ticks (`(ms * 30 + 999) / 1000` — plan §10.2).
        cooldown_ticks: u32,
        /// Target acquisition range in milli-tiles.
        acquire_range_milli_tiles: i32,
    },
    /// Resource harvesting.
    Gather {
        /// How much one full trip carries.
        carry_amount: i64,
        /// One gather cycle, in milliseconds, as authored.
        gather_time_ms: u32,
        /// One gather cycle, in whole ticks.
        gather_time_ticks: u32,
    },
    /// Can construct structures (no parameters in the Alpha).
    Build,
    /// Owns a production queue (which kinds are trainable is faction data).
    Produce,
    /// Accepts deposits of these resources.
    Storage {
        /// Storable resource ids.
        resources: Vec<String>,
    },
    /// Adds to the population cap while standing.
    ProvidesPopulation {
        /// Cap added.
        amount: i32,
    },
    /// A finite resource node body.
    Resource {
        /// Which resource this node holds.
        resource: String,
        /// Amount before depletion.
        amount: i64,
    },
    /// Perception radius.
    Vision {
        /// Sight radius in milli-tiles.
        radius_milli_tiles: i32,
    },
    /// Static tile occupancy.
    Footprint {
        /// Footprint width in tiles.
        w: u32,
        /// Footprint height in tiles.
        h: u32,
    },
}

impl CapabilityDef {
    /// The schema name of this capability (for duplicate and error messages).
    pub fn name(&self) -> &'static str {
        match self {
            CapabilityDef::Health { .. } => "Health",
            CapabilityDef::Move { .. } => "Move",
            CapabilityDef::Attack { .. } => "Attack",
            CapabilityDef::Gather { .. } => "Gather",
            CapabilityDef::Build => "Build",
            CapabilityDef::Produce => "Produce",
            CapabilityDef::Storage { .. } => "Storage",
            CapabilityDef::ProvidesPopulation { .. } => "ProvidesPopulation",
            CapabilityDef::Resource { .. } => "Resource",
            CapabilityDef::Vision { .. } => "Vision",
            CapabilityDef::Footprint { .. } => "Footprint",
        }
    }
}

/// A validated entity kind: capabilities plus economy stats (plan §10.4's table).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntityDef {
    /// Stable entity id.
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// The capability blocks, in authored order.
    pub capabilities: Vec<CapabilityDef>,
    /// Ore cost to build/train.
    pub cost_ore: i64,
    /// Build/train time in milliseconds, as authored.
    pub build_time_ms: u32,
    /// Build/train time in whole ticks (`(ms * 30 + 999) / 1000` — plan §10.2).
    pub build_time_ticks: u32,
    /// Population cost when fielded.
    pub population: i32,
    /// Entity ids required before this one can be produced.
    pub requires: Vec<String>,
    /// The source file (`entities/<id>.ron`) — carried for precise errors and
    /// tool output.
    pub file: String,
}

impl EntityDef {
    /// The first capability of a given name, if any.
    pub fn capability(&self, name: &str) -> Option<&CapabilityDef> {
        self.capabilities.iter().find(|cap| cap.name() == name)
    }

    /// The tile footprint, if this kind is a static occupant (structures, nodes).
    /// Movers have a collision radius instead (plan §9.1).
    pub fn footprint(&self) -> Option<(u32, u32)> {
        match self.capability("Footprint") {
            Some(CapabilityDef::Footprint { w, h }) => Some((*w, *h)),
            _ => None,
        }
    }

    /// Whether this kind is a structure (has a static footprint).
    pub fn is_structure(&self) -> bool {
        self.footprint().is_some()
    }
}

/// A validated faction: roster, production lists, starting forces (plan §10.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FactionDef {
    /// Stable faction id.
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// Entity ids this faction can field, in authored order.
    pub roster: Vec<String>,
    /// Producer entity id -> trainable kinds, producers in sorted order.
    pub production: Vec<(String, Vec<String>)>,
    /// Starting forces relative to a map start anchor, in authored order.
    pub starting_forces: Vec<StartingForce>,
    /// The source file (`factions/<id>.ron`) — carried for precise errors and
    /// tool output.
    pub file: String,
}

/// One starting force entry: which kind, at what tile offset from the anchor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StartingForce {
    /// The entity id to spawn.
    pub entity: String,
    /// Tile offset from the start anchor.
    pub offset: (i32, i32),
}

/// One terrain class from the map legend (plan §10.5: passability classes as
/// data — real input for M4's navigator).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TerrainClass {
    /// The single-character grid code.
    pub code: char,
    /// Human-facing name.
    pub name: String,
    /// Whether units may traverse this terrain.
    pub passable: bool,
    /// Whether structures may be placed on this terrain.
    pub buildable: bool,
}

/// A start position: a player slot and its anchor tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StartDef {
    /// The player slot this start belongs to.
    pub player: u8,
    /// The anchor tile (the reference corner for starting-force offsets).
    pub x: i32,
    /// The anchor tile y.
    pub y: i32,
}

/// An ore node placement: which node kind at which tile.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OreNodeDef {
    /// The node's entity kind id.
    pub kind: String,
    /// The node's tile (top-left of its footprint).
    pub x: i32,
    /// The node's tile y.
    pub y: i32,
}

/// The display-only heightmap (ADR-0001): per-tile integer heights for the
/// renderer. Never enters the simulation — not passability, not vision, not
/// combat, not the world definition handed to `Sim::new`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Heightmap {
    /// One row per map row, each of exactly `width` heights.
    pub rows: Vec<Vec<u16>>,
}

/// A validated map (plan §10.5): dimensions, terrain legend, the grid, starts,
/// ore placements, the optional symmetry declaration, and the optional
/// display-only heightmap.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MapDef {
    /// Stable map id.
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// Width in tiles.
    pub width: u32,
    /// Height in tiles.
    pub height: u32,
    /// The terrain legend, in authored order.
    pub terrain: Vec<TerrainClass>,
    /// The grid: one string per row (each `width` chars).
    pub grid: Vec<String>,
    /// Start positions, in authored order.
    pub starts: Vec<StartDef>,
    /// Ore node placements, in authored order.
    pub ore_nodes: Vec<OreNodeDef>,
    /// Whether the map declares rotational symmetry (validated when true).
    pub symmetric: bool,
    /// The display-only heightmap, if any (ADR-0001).
    pub heightmap: Option<Heightmap>,
    /// The source file (`maps/<id>.ron`) — carried for precise errors and tool
    /// output.
    pub file: String,
}

impl MapDef {
    /// The terrain class of a tile, by grid code. Coordinates are checked against
    /// the dimensions.
    pub fn terrain_at(&self, x: i32, y: i32) -> Option<&TerrainClass> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        let row = &self.grid[y as usize];
        let code = row.chars().nth(x as usize)?;
        self.terrain.iter().find(|class| class.code == code)
    }

    /// Whether a tile is passable terrain (legend lookup; out of bounds is
    /// impassable).
    pub fn passable(&self, x: i32, y: i32) -> bool {
        self.terrain_at(x, y).is_some_and(|class| class.passable)
    }

    /// Whether a tile is buildable terrain.
    pub fn buildable(&self, x: i32, y: i32) -> bool {
        self.terrain_at(x, y).is_some_and(|class| class.buildable)
    }

    /// The terrain legend entry for a grid code, if declared.
    pub fn terrain_class(&self, code: char) -> Option<&TerrainClass> {
        self.terrain.iter().find(|class| class.code == code)
    }
}

// ---------------------------------------------------------------------------
// Missions (ADR-0201)
// ---------------------------------------------------------------------------

/// A mission objective's kind (ADR-0201 §2): what the stage-10 evaluator
/// checks. Phase 1 carried the vertical slice's two kinds; Phase 2's B-017
/// added survive and protect, B-018 adds reach and gather — ADR-0201 §9's
/// Phase 2 objective set is complete.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectiveKind {
    /// Destroy all enemy structures — or every enemy structure of a target
    /// kind (`ObjectiveParameters::Destroy`).
    Destroy,
    /// Build/train N entities of a kind (`ObjectiveParameters::BuildTrain`).
    BuildTrain,
    /// Survive N ticks from mission start (`ObjectiveParameters::Survive`,
    /// B-017): pending until the mission tick reaches the authored count.
    Survive,
    /// Protect the mission player's entities of a kind
    /// (`ObjectiveParameters::Protect`, B-017): a fail-only constraint —
    /// pending while one stands, failed when the last falls. It never
    /// completes by itself.
    Protect,
    /// Reach an area with any mission player entity
    /// (`ObjectiveParameters::ReachArea`, B-018): pending until one stands
    /// inside the authored area (an `AreaSpec` — a rectangle in tiles or a
    /// circle in milli-tiles).
    ReachArea,
    /// Gather N ore (`ObjectiveParameters::Gather`, B-018): pending until
    /// the mission player's ore crosses the authored amount under the
    /// mission's chosen semantics — cumulative delivered, or held balance.
    Gather,
}

impl ObjectiveKind {
    /// The schema spelling of this kind (what appears in the mission file).
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectiveKind::Destroy => "Destroy",
            ObjectiveKind::BuildTrain => "BuildTrain",
            ObjectiveKind::Survive => "Survive",
            ObjectiveKind::Protect => "Protect",
            ObjectiveKind::ReachArea => "ReachArea",
            ObjectiveKind::Gather => "Gather",
        }
    }

    /// Parses a schema spelling. Unknown spellings are `None` so the loader
    /// can reject them loudly — never silently mapped.
    pub fn from_schema(kind: &str) -> Option<Self> {
        match kind {
            "Destroy" => Some(ObjectiveKind::Destroy),
            "BuildTrain" => Some(ObjectiveKind::BuildTrain),
            "Survive" => Some(ObjectiveKind::Survive),
            "Protect" => Some(ObjectiveKind::Protect),
            "ReachArea" => Some(ObjectiveKind::ReachArea),
            "Gather" => Some(ObjectiveKind::Gather),
            _ => None,
        }
    }
}

/// A mission objective's parameters, in content ids (ADR-0201 §2). The
/// simulation's resolved form — kind ids as `KindId`s — lives at the sim seam
/// (`pandemonium_sim_api::MissionObjectiveParameters`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ObjectiveParameters {
    /// `None` = all enemy structures (today's elimination rule); `Some(kind)`
    /// = every enemy structure of that entity kind.
    Destroy {
        /// The target structure kind, if the objective names one.
        target_kind: Option<String>,
    },
    /// The entity kind to build/train, and how many.
    BuildTrain {
        /// The kind id to build or train.
        kind: String,
        /// How many must exist.
        count: u32,
    },
    /// The tick the mission tick must reach, counted from mission start
    /// (B-017). The loader enforces the 300-tick floor, so the objective
    /// can never complete vacuously (AUD-003).
    Survive {
        /// The tick the objective completes at.
        ticks: u32,
    },
    /// The entity kind the mission player must keep alive (B-017). The
    /// loader resolves the reference and rejects a kind the mission
    /// player's faction does not start with — the vacuous-failure guard
    /// (AUD-003): a protect objective with nothing to protect would fail
    /// on the first tick.
    Protect {
        /// The protected kind's entity id.
        kind: String,
    },
    /// The area the mission player must reach with any entity (B-018) —
    /// authored as an [`AreaSpec`]: a rectangle in whole tiles or a circle
    /// in milli-tiles. Pure data here (FD-4): the containment law is code
    /// living at the sim seam (`pandemonium_sim_api::AreaSpec::contains`);
    /// the loader's gates (map bounds, whole-map coverage, the tick-0
    /// starting-entity guard) keep every vacuous shape out of the tree
    /// (AUD-003).
    ReachArea {
        /// The area to reach.
        area: AreaSpec,
    },
    /// The ore amount the mission player must gather, and the semantics
    /// the objective counts (B-018 — the ADR §2 "semantics choice:
    /// mission data picks"): `Cumulative` counts the mission player's
    /// total delivered ore (never reduced by spending), `Balance` counts
    /// the current ledger (spending sets it back). The tree-level gate
    /// rejects an amount at or below the ruleset's effective starting
    /// ore (200 in the Alpha) for BOTH semantics — the objective must
    /// demand play (AUD-003).
    Gather {
        /// How much ore the objective demands.
        amount: i64,
        /// Which ore total the objective counts.
        semantics: GatherSemantics,
    },
}

/// A gather objective's counting semantics (B-018, ADR-0201 §2's table:
/// "cumulative-delivered vs hold-balance (mission data picks)"). The
/// resource is ore by the objective's definition — the Alpha's single
/// registered resource — so the choice is only HOW the total is counted;
/// the seam resolves the resource id through the rules registry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GatherSemantics {
    /// The mission player's cumulative delivered ore: the total of their
    /// `ResourceDelivered` amounts over the match, derived by the
    /// evaluator from the delivery events (FD-9 — the ADR's own expressible
    /// basis). Spending never reduces it; the objective is monotone by
    /// construction.
    Cumulative,
    /// The mission player's current ore balance (the hashed ledger the
    /// economy credits): spending sets it back below the target, and the
    /// objective completes only while the balance actually holds the
    /// amount (the resolution itself is monotone once fired — one
    /// `ObjectiveState` event, exactly once, like every objective).
    Balance,
}

impl GatherSemantics {
    /// The schema spelling of this semantics (what appears in the mission
    /// file).
    pub fn as_str(self) -> &'static str {
        match self {
            GatherSemantics::Cumulative => "Cumulative",
            GatherSemantics::Balance => "Balance",
        }
    }

    /// Parses a schema spelling. Unknown spellings are `None` so the loader
    /// can reject them loudly — never silently mapped.
    pub fn from_schema(semantics: &str) -> Option<Self> {
        match semantics {
            "Cumulative" => Some(GatherSemantics::Cumulative),
            "Balance" => Some(GatherSemantics::Balance),
            _ => None,
        }
    }
}

/// An area as canonically validated (B-018): a rectangle in whole tiles or
/// a circle in milli-tiles, plain integers (FD-5). The authoring mirror of
/// `pandemonium_sim_api::AreaSpec`; the mission seam converts one into the
/// other at load — content never hands the sim its own types (FD-6), and
/// the sim's type carries the containment predicate the evaluator runs.
/// This type is data only: no behavior, exactly like every other defs
/// type (the geometry code lives at the seam where the evaluator can
/// reach it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AreaSpec {
    /// An axis-aligned rectangle in whole tiles: the half-open tile span
    /// `[x, x+w) x [y, y+h)` (left/top edges inclusive, right/bottom
    /// exclusive).
    Rect {
        /// The left edge's tile column (inclusive).
        x: i32,
        /// The top edge's tile row (inclusive).
        y: i32,
        /// The width in tiles (non-zero; the loader rejects 0).
        w: u32,
        /// The height in tiles (non-zero; the loader rejects 0).
        h: u32,
    },
    /// A circle in milli-tiles, boundary inclusive.
    Circle {
        /// The center's x, in milli-tiles.
        cx: i32,
        /// The center's y, in milli-tiles.
        cy: i32,
        /// The radius, in milli-tiles (positive; the loader rejects 0 and
        /// negatives).
        radius: i32,
    },
}

/// One mission objective (ADR-0201 §2): the kind, its parameters, and whether
/// it is optional. Optional objectives never gate `MatchEnded`; they feed the
/// end screen and story hooks.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ObjectiveDef {
    /// What the evaluator checks.
    pub kind: ObjectiveKind,
    /// The kind's parameters.
    pub parameters: ObjectiveParameters,
    /// Whether the objective is optional.
    pub optional: bool,
}

/// A mission's fog preset (B-015): the canonical mirror of
/// `pandemonium_sim_api::FogPreset`. The mission seam converts one into the
/// other at load — content never hands the sim its own types (FD-6), and
/// the sim never sees a content type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FogPreset {
    /// Every tile starts `Hidden` — the historical initialization,
    /// byte-identical; the default when the mission file omits the field.
    Default,
    /// Every tile starts `Explored`; vision still lights what units see.
    Explored,
    /// The whole map stays `Visible` for the entire match.
    Visible,
}

impl FogPreset {
    /// The schema spelling of this preset (what appears in the mission file).
    pub fn as_str(self) -> &'static str {
        match self {
            FogPreset::Default => "Default",
            FogPreset::Explored => "Explored",
            FogPreset::Visible => "Visible",
        }
    }

    /// Parses a schema spelling. Unknown spellings are `None` so the loader
    /// can reject them loudly — never silently mapped.
    pub fn from_schema(preset: &str) -> Option<Self> {
        match preset {
            "Default" => Some(FogPreset::Default),
            "Explored" => Some(FogPreset::Explored),
            "Visible" => Some(FogPreset::Visible),
            _ => None,
        }
    }
}

/// The faction variant overrides, canonically validated (B-019, ADR-0201
/// §3's restrictions-as-mission-data): the inline reshaping of the mission
/// faction. Every sub-field is optional — an override block names only
/// what it changes, and a block that changes nothing (both sub-fields
/// `None`) behaves exactly like no block at all, mission id included.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FactionOverridesDef {
    /// The replacement roster: the entity ids the mission faction can
    /// field. The tree-level gate guarantees it is a subset of the base
    /// faction's roster (an override restricts, it never adds — no new
    /// kinds, FD-4's data law) and that every name resolves in the tree.
    /// `None` keeps the base roster.
    pub roster: Option<Vec<String>>,
    /// The replacement starting forces, placed relative to the map start
    /// anchors exactly like the base faction's (plan §9.7). The loader
    /// guarantees every kind exists in the tree and is not a resource
    /// node. `None` keeps the base starting forces.
    pub starting_forces: Option<Vec<StartingForce>>,
}

/// The rules variant overrides, canonically validated (B-020, ADR-0201
/// §3's restrictions-as-mission-data): the inline reshaping of the
/// mission's ruleset. Only plain integer fields may be overridden: the
/// starting ore balance and the base population cap. Every sub-field is
/// optional — an override block names only what it changes, and a block
/// that changes nothing (both sub-fields `None`) behaves exactly like no
/// block at all, mission id included.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RulesOverridesDef {
    /// Override for the starting ore balance (resources[ore].starting).
    /// `None` keeps the base ruleset's starting ore.
    pub starting_ore: Option<i64>,
    /// Override for the base population cap (base_population_cap).
    /// `None` keeps the base ruleset's base population cap.
    pub base_population_cap: Option<u32>,
}

/// The AI tuning overrides, canonically validated (B-021, ADR-0201
/// Phase 2's closing package): the inline, mission-carried pacing block
/// for the scripted Alpha opponent. Every sub-field is an optional plain
/// integer; `None` keeps the script's own const — the byte-exact default
/// law — and a block that changes nothing behaves exactly like no block
/// at all, mission id included (the loader normalizes it away).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AiSettingsDef {
    /// Override for the pressure cadence — the re-press interval while a
    /// wave is out (the script's `WAVE_TIMER_TICKS`).
    pub wave_timer_ticks: Option<u32>,
    /// Override for the full-wave size (the script's `WAVE_SIZE`).
    pub wave_size: Option<u32>,
    /// Override for the minimum marching army (the script's `MIN_WAVE`).
    pub min_wave: Option<u32>,
    /// Override for the army-size cap (the script's `ARMY_CAP`).
    pub army_cap: Option<u32>,
    /// Override for the worker-count target (the script's
    /// `WORKER_TARGET`).
    pub worker_target: Option<u32>,
    /// Override for the initial pressure timer — the first march delay
    /// (the script's `WAVE_TIMER_TICKS` in its initial role).
    pub first_attack_delay: Option<u32>,
}

impl AiSettingsDef {
    /// Whether the block actually overrides something — at least one
    /// sub-field is `Some`. The no-op all-`None` block is normalized away
    /// by the loader, so a canonical block that exists always carries at
    /// least one value; the encoder writes bytes only when this holds
    /// (the mission identity rule).
    pub fn carries_override(&self) -> bool {
        self.wave_timer_ticks.is_some()
            || self.wave_size.is_some()
            || self.min_wave.is_some()
            || self.army_cap.is_some()
            || self.worker_target.is_some()
            || self.first_attack_delay.is_some()
    }
}

/// A validated mission (ADR-0201 §3): a closed reference set into the content
/// tree — map, faction, rules — plus the ordered, AND-composed objective
/// list. Reference existence is validated at load (the loader resolves
/// objective entity references; the tree assembly resolves the map, faction
/// and rules references); the objectives' content ids become `KindId`s at the
/// sim seam.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MissionDef {
    /// Stable mission id.
    pub id: String,
    /// Human-facing name (ASCII — the embedded font constraint, ADR-0201 §5).
    pub display_name: String,
    /// The mission schema version the file declared (forward-only, FD-10).
    pub schema_version: u32,
    /// The map this mission runs on (validated to exist in the tree).
    pub map: String,
    /// The faction the mission player runs (validated to exist in the tree).
    pub faction: String,
    /// The ruleset the mission runs under (validated to exist in the tree).
    pub rules: String,
    /// The fog preset (B-015): how the fog cache initializes for every
    /// player. `Default` when the file omits the field — the historical
    /// initialization, so a mission that does not use the new field keeps
    /// exactly the same mission id and behavior as before.
    pub fog_preset: FogPreset,
    /// The faction variant overrides (B-019): the inline reshaping of the
    /// mission faction — a restricted roster and/or replacement starting
    /// forces, never a new content file. `None` when the file omits the
    /// block (or carries a no-op block): the mission plays the base faction
    /// and keeps exactly the mission id it had before (the identity rule).
    pub faction_overrides: Option<FactionOverridesDef>,
    /// The rules variant overrides (B-020): the inline reshaping of the
    /// mission's ruleset — plain integer fields only (starting ore, base
    /// population cap). `None` when the file omits the block (or carries a
    /// no-op block): the mission plays the base ruleset and keeps exactly
    /// the mission id it had before (the identity rule).
    pub rules_overrides: Option<RulesOverridesDef>,
    /// The AI tuning overrides (B-021): the inline pacing block for the
    /// scripted Alpha opponent. `None` when the file omits the block (or
    /// carries a no-op block): the script plays its own consts and the
    /// mission keeps exactly the id it had before (the identity rule).
    /// The simulation never reads these; the engine's plan builder copies
    /// them into the `AiPlan` the controller reads.
    pub ai_settings: Option<AiSettingsDef>,
    /// The ordered objective list — the indices are the objective-state event
    /// indices (ADR-0201 §2).
    pub objectives: Vec<ObjectiveDef>,
}
