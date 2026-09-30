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
