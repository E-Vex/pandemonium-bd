//! Loading: named RON sources, the on-disk directory reader, and the per-file
//! parse + migrate + convert pipeline.
//!
//! Determinism note (plan §5): directory iteration order is OS-dependent, so
//! [`SourceMap::from_dir`] sorts every category's files by name before parsing,
//! and the canonical types are sorted by id afterwards. Loading the same
//! directory twice therefore produces bit-identical bundles — pinned by tests.

use std::fs;
use std::path::Path;

use crate::defs;
use crate::error::ContentError;
use crate::schema::{self, RawCapability, RawEntity, RawFaction, RawMapV2, RawRules};
use crate::validate;
use crate::version;

/// One named RON source: a file name (like `worker.ron`) and its text. The
/// category prefix (`entities/`) is added in error messages by the loader.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Source {
    /// The file name with extension (the stem must equal the declared id).
    pub name: String,
    /// The file's full text.
    pub text: String,
}

impl Source {
    /// Builds a source from name and text (for tests and embedded content).
    pub fn new(name: &str, text: &str) -> Self {
        Self {
            name: name.to_string(),
            text: text.to_string(),
        }
    }

    /// The file name without its `.ron` extension.
    pub(crate) fn stem(&self) -> &str {
        self.name.strip_suffix(".ron").unwrap_or(&self.name)
    }
}

/// Every content source, by category. This is the loader's input; the tests
/// build these in memory, [`SourceMap::from_dir`] builds them from disk.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SourceMap {
    /// Rules files (exactly one is required — the registry is global).
    pub rules: Vec<Source>,
    /// Entity files (any count).
    pub entities: Vec<Source>,
    /// Faction files (any count).
    pub factions: Vec<Source>,
    /// Map files (any count).
    pub maps: Vec<Source>,
}

impl SourceMap {
    /// An empty source map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a content tree from disk: `root/{rules,entities,factions,maps}/*.ron`.
    ///
    /// Files are sorted by name within each category (deterministic load order),
    /// non-`.ron` files are ignored, and subdirectories are refused loudly — a
    /// surprise must be an error, not a silent skip.
    pub fn from_dir(root: &Path) -> Result<Self, ContentError> {
        Ok(Self {
            rules: read_category(root, "rules")?,
            entities: read_category(root, "entities")?,
            factions: read_category(root, "factions")?,
            maps: read_category(root, "maps")?,
        })
    }

    /// Adds (or replaces) a rules file. Replacing by name lets callers derive a
    /// mutated copy of an existing source map without duplicating entries.
    pub fn rules(mut self, name: &str, text: &str) -> Self {
        self.rules.retain(|source| source.name != name);
        self.rules.push(Source::new(name, text));
        self
    }

    /// Adds (or replaces) an entity file.
    pub fn entity(mut self, name: &str, text: &str) -> Self {
        self.entities.retain(|source| source.name != name);
        self.entities.push(Source::new(name, text));
        self
    }

    /// Adds (or replaces) a faction file.
    pub fn faction(mut self, name: &str, text: &str) -> Self {
        self.factions.retain(|source| source.name != name);
        self.factions.push(Source::new(name, text));
        self
    }

    /// Adds (or replaces) a map file.
    pub fn map(mut self, name: &str, text: &str) -> Self {
        self.maps.retain(|source| source.name != name);
        self.maps.push(Source::new(name, text));
        self
    }
}

/// Reads one category directory, sorted by file name. See
/// [`SourceMap::from_dir`] for the ordering and error rules.
fn read_category(root: &Path, category: &str) -> Result<Vec<Source>, ContentError> {
    let dir = root.join(category);
    if !dir.is_dir() {
        return Err(ContentError::MissingDirectory {
            path: dir.display().to_string(),
            category: category.to_string(),
        });
    }
    let mut sources = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|error| ContentError::Io {
        file: dir.display().to_string(),
        detail: error.to_string(),
    })? {
        let entry = entry.map_err(|error| ContentError::Io {
            file: dir.display().to_string(),
            detail: error.to_string(),
        })?;
        let path = entry.path();
        if path.is_dir() {
            return Err(ContentError::Io {
                file: path.display().to_string(),
                detail: format!(
                    "subdirectories are not supported inside content/{category}; \
                     every file must sit directly in its category"
                ),
            });
        }
        if path.extension().is_none_or(|ext| ext != "ron") {
            continue; // Documentation and stray files are not content.
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let text = fs::read_to_string(&path).map_err(|error| ContentError::Io {
            file: format!("{category}/{name}"),
            detail: error.to_string(),
        })?;
        sources.push(Source { name, text });
    }
    // OS directory order is not deterministic across platforms — sort.
    sources.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sources)
}

/// The display path of a source within a category (`entities/worker.ron`).
fn display_path(category: &str, source: &Source) -> String {
    format!("{category}/{}", source.name)
}

// ---------------------------------------------------------------------------
// Per-file-kind parsing (version gate -> strict parse -> id check -> convert)
// ---------------------------------------------------------------------------

/// Parses and validates the one rules file.
pub(crate) fn parse_rules(source: &Source) -> Result<defs::RulesDef, ContentError> {
    let file = display_path("rules", source);
    let found = version::read_version(&file, &source.text)?;
    version::check_version(&file, found, schema::RULES_SCHEMA_VERSION)?;
    let raw: RawRules = ron::from_str(&source.text).map_err(|error| ContentError::Parse {
        file: file.clone(),
        detail: error.to_string(),
    })?;
    version::confirm_declared_version(&file, raw.schema_version, found)?;
    if raw.id != source.stem() {
        return Err(ContentError::IdMismatch {
            file,
            declared: raw.id,
            stem: source.stem().to_string(),
        });
    }
    let victory = defs::VictoryKind::from_schema(&raw.victory.kind).ok_or_else(|| {
        ContentError::InvalidRules {
            file: file.clone(),
            detail: format!(
                "unknown victory kind '{}' (known kinds: 'elimination')",
                raw.victory.kind
            ),
        }
    })?;
    let def = defs::RulesDef {
        id: raw.id,
        display_name: raw.display_name,
        resources: raw
            .resources
            .into_iter()
            .map(|res| defs::ResourceDef {
                id: res.id,
                display_name: res.display_name,
                starting: res.starting,
            })
            .collect(),
        base_population_cap: raw.base_population_cap,
        victory,
        allow_resignation: raw.victory.allow_resignation,
        file,
    };
    validate::validate_rules(&def.file, &def)?;
    Ok(def)
}

/// Parses, validates, and converts one entity file into a canonical
/// [`defs::EntityDef`]. `rules` supplies the resource registry for
/// resource-reference checks.
pub(crate) fn parse_entity(
    source: &Source,
    rules: &defs::RulesDef,
) -> Result<defs::EntityDef, ContentError> {
    let file = display_path("entities", source);
    let found = version::read_version(&file, &source.text)?;
    version::check_version(&file, found, schema::ENTITY_SCHEMA_VERSION)?;
    let raw: RawEntity = ron::from_str(&source.text).map_err(|error| ContentError::Parse {
        file: file.clone(),
        detail: error.to_string(),
    })?;
    version::confirm_declared_version(&file, raw.schema_version, found)?;
    if raw.id != source.stem() {
        return Err(ContentError::IdMismatch {
            file,
            declared: raw.id,
            stem: source.stem().to_string(),
        });
    }
    let capabilities = raw
        .capabilities
        .iter()
        .map(convert_capability)
        .collect::<Vec<_>>();
    let def = defs::EntityDef {
        id: raw.id,
        display_name: raw.display_name,
        capabilities,
        cost_ore: raw.cost.ore,
        build_time_ms: raw.build_time_ms,
        build_time_ticks: ms_to_ticks(raw.build_time_ms, pandemonium_sim::TICKS_PER_SECOND),
        population: raw.population,
        requires: raw.requires,
        file,
    };
    validate::validate_entity(&def.file, &def, rules)?;
    Ok(def)
}

/// Converts one raw capability into its canonical form (durations gain their
/// tick-converted twins — plan §10.2: conversion happens once, at load).
fn convert_capability(raw: &RawCapability) -> defs::CapabilityDef {
    match raw {
        RawCapability::Health(cap) => defs::CapabilityDef::Health {
            max_hp: cap.max,
            regen_per_tick: cap.regen_per_tick,
        },
        RawCapability::Move(cap) => defs::CapabilityDef::Move {
            speed_milli_tiles_per_s: cap.speed_milli_tiles_per_s,
            radius_milli_tiles: cap.radius_milli_tiles,
        },
        RawCapability::Attack(cap) => defs::CapabilityDef::Attack {
            damage: cap.damage,
            range_milli_tiles: cap.range_milli_tiles,
            cooldown_ms: cap.cooldown_ms,
            cooldown_ticks: ms_to_ticks(cap.cooldown_ms, pandemonium_sim::TICKS_PER_SECOND),
            acquire_range_milli_tiles: cap.acquire_range_milli_tiles,
        },
        RawCapability::Gather(cap) => defs::CapabilityDef::Gather {
            carry_amount: cap.carry_amount,
            gather_time_ms: cap.gather_time_ms,
            gather_time_ticks: ms_to_ticks(cap.gather_time_ms, pandemonium_sim::TICKS_PER_SECOND),
        },
        RawCapability::Build(_) => defs::CapabilityDef::Build,
        RawCapability::Produce(_) => defs::CapabilityDef::Produce,
        RawCapability::Storage(cap) => defs::CapabilityDef::Storage {
            resources: cap.resources.clone(),
        },
        RawCapability::ProvidesPopulation(cap) => {
            defs::CapabilityDef::ProvidesPopulation { amount: cap.amount }
        }
        RawCapability::Resource(cap) => defs::CapabilityDef::Resource {
            resource: cap.resource.clone(),
            amount: cap.amount,
        },
        RawCapability::Vision(cap) => defs::CapabilityDef::Vision {
            radius_milli_tiles: cap.radius_milli_tiles,
        },
        RawCapability::Footprint(cap) => defs::CapabilityDef::Footprint { w: cap.w, h: cap.h },
    }
}

/// Parses, validates, and converts one faction file.
pub(crate) fn parse_faction(
    source: &Source,
    entities: &[defs::EntityDef],
) -> Result<defs::FactionDef, ContentError> {
    let file = display_path("factions", source);
    let found = version::read_version(&file, &source.text)?;
    version::check_version(&file, found, schema::FACTION_SCHEMA_VERSION)?;
    let raw: RawFaction = ron::from_str(&source.text).map_err(|error| ContentError::Parse {
        file: file.clone(),
        detail: error.to_string(),
    })?;
    version::confirm_declared_version(&file, raw.schema_version, found)?;
    if raw.id != source.stem() {
        return Err(ContentError::IdMismatch {
            file,
            declared: raw.id,
            stem: source.stem().to_string(),
        });
    }
    let def = defs::FactionDef {
        id: raw.id,
        display_name: raw.display_name,
        roster: raw.roster,
        production: raw.production.into_iter().collect(),
        starting_forces: raw
            .starting_forces
            .into_iter()
            .map(|force| defs::StartingForce {
                entity: force.entity,
                offset: (force.offset.x, force.offset.y),
            })
            .collect(),
        file,
    };
    validate::validate_faction(&def.file, &def, entities)?;
    Ok(def)
}

/// Parses, migrates, validates, and converts one map file (whatever supported
/// version it declares) into a canonical [`defs::MapDef`].
pub(crate) fn parse_map(
    source: &Source,
    entities: &[defs::EntityDef],
) -> Result<defs::MapDef, ContentError> {
    let file = display_path("maps", source);
    let migrated: RawMapV2 = match version::parse_map_at_declared_version(&file, &source.text)? {
        version::MapAtCurrent::Current(raw) => raw,
        version::MapAtCurrent::NeedsMigration(v1) => version::migrate_map_v1_to_v2(v1),
    };
    if migrated.id != source.stem() {
        return Err(ContentError::IdMismatch {
            file,
            declared: migrated.id,
            stem: source.stem().to_string(),
        });
    }
    let terrain = convert_terrain(&file, &migrated.id, migrated.terrain)?;
    let def = defs::MapDef {
        id: migrated.id,
        display_name: migrated.display_name,
        width: migrated.width,
        height: migrated.height,
        terrain,
        grid: migrated.grid,
        starts: migrated
            .starts
            .into_iter()
            .map(|start| defs::StartDef {
                player: start.player,
                x: start.anchor.x,
                y: start.anchor.y,
            })
            .collect(),
        ore_nodes: migrated
            .ore_nodes
            .into_iter()
            .map(|node| defs::OreNodeDef {
                kind: node.kind,
                x: node.tile.x,
                y: node.tile.y,
            })
            .collect(),
        symmetric: migrated.symmetric,
        heightmap: migrated.heightmap.map(|heightmap| defs::Heightmap {
            rows: heightmap.rows,
        }),
        file,
    };
    validate::validate_map_file(&def.file, &def, entities)?;
    Ok(def)
}

/// Converts the terrain legend, rejecting codes that are not exactly one
/// character (they are grid lookup keys — multi-character codes cannot index a
/// grid cell).
fn convert_terrain(
    file: &str,
    map: &str,
    raw: Vec<schema::RawTerrain>,
) -> Result<Vec<defs::TerrainClass>, ContentError> {
    let mut terrain = Vec::with_capacity(raw.len());
    for class in raw {
        let mut codes = class.code.chars();
        let code = codes.next().ok_or_else(|| ContentError::InvalidMap {
            file: file.to_string(),
            map: map.to_string(),
            detail: format!(
                "terrain '{}' has an empty code (codes index the grid and must be a \
                 single character)",
                class.name
            ),
        })?;
        if codes.next().is_some() {
            return Err(ContentError::InvalidMap {
                file: file.to_string(),
                map: map.to_string(),
                detail: format!(
                    "terrain '{}' has code '{}' (codes must be a single character)",
                    class.name, class.code
                ),
            });
        }
        terrain.push(defs::TerrainClass {
            code,
            name: class.name,
            passable: class.passable,
            buildable: class.buildable,
        });
    }
    Ok(terrain)
}

/// Plan §10.2's duration conversion, integer-only: milliseconds to whole ticks,
/// rounding up so a duration is never rounded away — `(ms * 30 + 999) / 1000`,
/// i.e. the ceiling of `ms * ticks_per_second / 1000`.
pub fn ms_to_ticks(ms: u32, ticks_per_second: u32) -> u32 {
    ((ms as u64 * ticks_per_second as u64).div_ceil(1000)) as u32
}
