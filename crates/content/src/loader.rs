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
use crate::schema::{
    self, RawCapability, RawEntity, RawFaction, RawMapV2, RawObjective, RawObjectiveParameters,
    RawRules,
};
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
    /// Mission files (any count; zero is legal — the mission-less default,
    /// ADR-0201 §3).
    pub missions: Vec<Source>,
}

impl SourceMap {
    /// An empty source map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a content tree from disk:
    /// `root/{rules,entities,factions,maps,missions}/*.ron`.
    ///
    /// Files are sorted by name within each category (deterministic load order),
    /// non-`.ron` files are ignored, and subdirectories are refused loudly — a
    /// surprise must be an error, not a silent skip. The missions category is
    /// the one exception on strictness: the directory itself may be absent
    /// (see [`read_category_optional`]).
    pub fn from_dir(root: &Path) -> Result<Self, ContentError> {
        Ok(Self {
            rules: read_category(root, "rules")?,
            entities: read_category(root, "entities")?,
            factions: read_category(root, "factions")?,
            maps: read_category(root, "maps")?,
            missions: read_category_optional(root, "missions")?,
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

    /// Adds (or replaces) a mission file.
    pub fn mission(mut self, name: &str, text: &str) -> Self {
        self.missions.retain(|source| source.name != name);
        self.missions.push(Source::new(name, text));
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

/// Reads one category directory whose absence is legal content: an empty
/// vector instead of an error. Missions use this (ADR-0201 §3) — a tree
/// without a `missions/` directory is a tree whose matches are all
/// mission-less (the default match, today's elimination), and trees that
/// predate the category keep loading unchanged (FD-10's spirit: old content
/// keeps loading). Present directories follow the exact `read_category`
/// rules: sorted scan, non-`.ron` ignored, subdirectories refused loudly.
fn read_category_optional(root: &Path, category: &str) -> Result<Vec<Source>, ContentError> {
    if !root.join(category).is_dir() {
        return Ok(Vec::new());
    }
    read_category(root, category)
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

/// Parses and validates one mission file (ADR-0201 §3): the version gate, the
/// strict parse, the id/stem check, the ASCII display-name rule (the embedded
/// font constraint, ADR-0201 §5), and the objective conversion — unknown kinds
/// are loud errors, kind/parameter mismatches are loud errors, and entity
/// references resolve against the tree's entities exactly like faction rosters
/// and map ore nodes do. The map/faction/rules references are validated at
/// tree assembly (they need the whole tree — see `ContentTree::load`).
/// Version 1 files migrate forward to the current shape (the fog preset
/// defaults — ADR-0201 §3, Phase 2).
pub(crate) fn parse_mission(
    source: &Source,
    entities: &[defs::EntityDef],
) -> Result<defs::MissionDef, ContentError> {
    let file = display_path("missions", source);
    let raw = match version::parse_mission_at_declared_version(&file, &source.text)? {
        version::MissionAtCurrent::Current(raw) => raw,
        version::MissionAtCurrent::NeedsMigration(v1) => version::migrate_mission_v1_to_v2(v1),
    };
    if raw.id != source.stem() {
        return Err(ContentError::IdMismatch {
            file,
            declared: raw.id,
            stem: source.stem().to_string(),
        });
    }
    if raw.display_name.is_empty() || !raw.display_name.is_ascii() {
        return Err(ContentError::Parse {
            file: file.clone(),
            detail: format!(
                "display_name must be non-empty ASCII (the embedded font renders \
                 printable ASCII only — ADR-0201 §5), found '{}'",
                raw.display_name
            ),
        });
    }
    let objectives = raw
        .objectives
        .into_iter()
        .enumerate()
        .map(|(index, objective)| convert_objective(&file, &raw.id, index, objective, entities))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(defs::MissionDef {
        id: raw.id,
        display_name: raw.display_name,
        schema_version: raw.schema_version,
        map: raw.map,
        faction: raw.faction,
        rules: raw.rules,
        objectives,
        fog_preset: defs::FogPreset::from(raw.fog_preset),
    })
}

/// Converts one raw objective into its canonical form (ADR-0201 §2): resolves
/// the kind spelling — the vocabulary is fixed and unknown spellings are loud
/// errors, never silently mapped — cross-checks the spelling against the
/// parameters block, and resolves the entity references.
fn convert_objective(
    file: &str,
    mission: &str,
    index: usize,
    raw: RawObjective,
    entities: &[defs::EntityDef],
) -> Result<defs::ObjectiveDef, ContentError> {
    let Some(kind) = defs::ObjectiveKind::from_schema(&raw.kind) else {
        return Err(ContentError::Parse {
            file: file.to_string(),
            detail: format!(
                "objective {} of mission '{}' has unknown kind '{}' (known kinds: '{}', '{}', \
                 '{}', '{}')",
                index + 1,
                mission,
                raw.kind,
                defs::ObjectiveKind::Destroy.as_str(),
                defs::ObjectiveKind::BuildTrain.as_str(),
                defs::ObjectiveKind::Survive.as_str(),
                defs::ObjectiveKind::Protect.as_str(),
            ),
        });
    };
    let (block_kind, parameters) = match raw.parameters {
        RawObjectiveParameters::Destroy(params) => {
            if let Some(target) = &params.target_kind {
                check_objective_entity_ref(
                    file,
                    mission,
                    index,
                    "Destroy target_kind",
                    target,
                    entities,
                )?;
            }
            (
                defs::ObjectiveKind::Destroy,
                defs::ObjectiveParameters::Destroy {
                    target_kind: params.target_kind,
                },
            )
        }
        RawObjectiveParameters::BuildTrain(params) => {
            check_objective_entity_ref(
                file,
                mission,
                index,
                "BuildTrain kind",
                &params.kind,
                entities,
            )?;
            if params.count == 0 {
                return Err(ContentError::Parse {
                    file: file.to_string(),
                    detail: format!(
                        "objective {} of mission '{}' has count 0 (build/train objectives \
                         need at least one)",
                        index + 1,
                        mission
                    ),
                });
            }
            (
                defs::ObjectiveKind::BuildTrain,
                defs::ObjectiveParameters::BuildTrain {
                    kind: params.kind,
                    count: params.count,
                },
            )
        }
        RawObjectiveParameters::Survive(params) => {
            // R4 (the AUD-003 lesson): a zero-tick survive objective is
            // vacuously complete at tick 0 — the loader refuses it loudly.
            if params.ticks == 0 {
                return Err(ContentError::Parse {
                    file: file.to_string(),
                    detail: format!(
                        "objective {} of mission '{}' has ticks 0 (survive objectives \
                         need at least one tick — a zero-tick timer is vacuously \
                         complete)",
                        index + 1,
                        mission
                    ),
                });
            }
            (
                defs::ObjectiveKind::Survive,
                defs::ObjectiveParameters::Survive {
                    ticks: params.ticks,
                },
            )
        }
        RawObjectiveParameters::Protect(params) => {
            check_objective_entity_ref(
                file,
                mission,
                index,
                "Protect kind",
                &params.kind,
                entities,
            )?;
            (
                defs::ObjectiveKind::Protect,
                defs::ObjectiveParameters::Protect { kind: params.kind },
            )
        }
    };
    if kind != block_kind {
        return Err(ContentError::Parse {
            file: file.to_string(),
            detail: format!(
                "objective {} of mission '{}' declares kind '{}' but carries '{}' \
                 parameters (the kind spelling and the parameters block must agree)",
                index + 1,
                mission,
                kind.as_str(),
                block_kind.as_str(),
            ),
        });
    }
    Ok(defs::ObjectiveDef {
        kind,
        parameters,
        optional: raw.optional,
    })
}

/// Resolves one objective entity reference against the tree's entities — the
/// same check faction rosters and map ore nodes get.
fn check_objective_entity_ref(
    file: &str,
    mission: &str,
    index: usize,
    what: &str,
    reference: &str,
    entities: &[defs::EntityDef],
) -> Result<(), ContentError> {
    if entities.iter().any(|entity| entity.id == reference) {
        return Ok(());
    }
    Err(ContentError::UnknownEntityRef {
        file: file.to_string(),
        what: format!("{what} of objective {} in mission '{mission}'", index + 1),
        reference: reference.to_string(),
    })
}

/// Plan §10.2's duration conversion, integer-only: milliseconds to whole ticks,
/// rounding up so a duration is never rounded away — `(ms * 30 + 999) / 1000`,
/// i.e. the ceiling of `ms * ticks_per_second / 1000`.
pub fn ms_to_ticks(ms: u32, ticks_per_second: u32) -> u32 {
    ((ms as u64 * ticks_per_second as u64).div_ceil(1000)) as u32
}

#[cfg(test)]
mod tests {
    //! Mission parsing (ADR-0201 §3, B-012 Task 1): the strict schema, the
    //! loud rejections, and the reference checks. The in-memory shape mirrors
    //! `tests.rs`'s factory pattern; these live here (not in `tests.rs`) so
    //! the mission surface is tested in the file that owns it.

    use super::*;
    use crate::Source;

    /// A minimal entity the objectives can reference.
    fn test_entity(id: &str) -> defs::EntityDef {
        defs::EntityDef {
            id: id.to_string(),
            display_name: id.to_string(),
            capabilities: vec![],
            cost_ore: 0,
            build_time_ms: 0,
            build_time_ticks: 0,
            population: 0,
            requires: vec![],
            file: format!("entities/{id}.ron"),
        }
    }

    fn entities() -> Vec<defs::EntityDef> {
        vec![test_entity("command_center"), test_entity("worker")]
    }

    const MISSION: &str = r#"
(
    schema_version: 1,
    id: "crossroads_01",
    display_name: "Crossroads",
    map: "crossroads_64",
    faction: "legion",
    rules: "alpha",
    objectives: [
        (
            kind: "Destroy",
            parameters: Destroy(( target_kind: None )),
            optional: false,
        ),
        (
            kind: "BuildTrain",
            parameters: BuildTrain(( kind: "worker", count: 4 )),
            optional: true,
        ),
    ],
)
"#;

    fn parse(text: &str) -> Result<defs::MissionDef, ContentError> {
        parse_mission(&Source::new("crossroads_01.ron", text), &entities())
    }

    #[test]
    fn a_mission_parses_into_its_canonical_form() {
        let def = parse(MISSION).expect("the factory mission parses");
        assert_eq!(def.id, "crossroads_01");
        assert_eq!(def.display_name, "Crossroads");
        // The v1 factory migrates forward to the current shape (FD-10): the
        // declared version is the file's, the canonical form is current.
        assert_eq!(def.schema_version, 2);
        assert_eq!(def.map, "crossroads_64");
        assert_eq!(def.faction, "legion");
        assert_eq!(def.rules, "alpha");
        assert_eq!(def.objectives.len(), 2);
        assert_eq!(
            def.objectives[0],
            defs::ObjectiveDef {
                kind: defs::ObjectiveKind::Destroy,
                parameters: defs::ObjectiveParameters::Destroy { target_kind: None },
                optional: false,
            }
        );
        assert_eq!(
            def.objectives[1],
            defs::ObjectiveDef {
                kind: defs::ObjectiveKind::BuildTrain,
                parameters: defs::ObjectiveParameters::BuildTrain {
                    kind: "worker".to_string(),
                    count: 4,
                },
                optional: true,
            }
        );
    }

    #[test]
    fn a_destroy_target_kind_resolves_like_any_entity_reference() {
        let targeted = MISSION.replace(
            "Destroy(( target_kind: None ))",
            "Destroy(( target_kind: Some(\"command_center\") ))",
        );
        let def = parse(&targeted).expect("a named target parses");
        assert_eq!(
            def.objectives[0].parameters,
            defs::ObjectiveParameters::Destroy {
                target_kind: Some("command_center".to_string()),
            }
        );
        let unknown = MISSION.replace(
            "Destroy(( target_kind: None ))",
            "Destroy(( target_kind: Some(\"barracks\") ))",
        );
        let err = parse(&unknown).unwrap_err();
        assert!(
            err.to_string()
                .contains("references unknown entity 'barracks'"),
            "got: {err}"
        );
    }

    #[test]
    fn unknown_objective_kinds_are_rejected_loudly() {
        // The vocabulary is fixed and forward-only: 'Survive' and 'Protect'
        // joined it in Phase 2, so the unknown-spelling probe uses a
        // genuinely unknown word, and the error names every known kind.
        let bad = MISSION.replace("\"BuildTrain\",", "\"TimeLimit\",");
        let err = parse(&bad).unwrap_err();
        assert!(
            err.to_string().contains(
                "unknown kind 'TimeLimit' (known kinds: 'Destroy', 'BuildTrain', 'Survive', \
                 'Protect')"
            ),
            "got: {err}"
        );
    }

    #[test]
    fn kind_and_parameters_must_agree() {
        let mismatched = MISSION.replace(
            "kind: \"Destroy\",\n            parameters: Destroy(( target_kind: None )),",
            "kind: \"BuildTrain\",\n            parameters: Destroy(( target_kind: None )),",
        );
        let err = parse(&mismatched).unwrap_err();
        assert!(
            err.to_string()
                .contains("declares kind 'BuildTrain' but carries 'Destroy' parameters"),
            "got: {err}"
        );
    }

    #[test]
    fn unknown_fields_are_errors_in_strict_mode() {
        let extra = MISSION.replace(
            "    rules: \"alpha\",",
            "    rules: \"alpha\",\n    ai_settings: (),",
        );
        let err = parse(&extra).unwrap_err();
        assert!(matches!(err, ContentError::Parse { .. }), "got: {err}");
    }

    #[test]
    fn a_version_one_mission_migrates_to_the_default_fog_preset() {
        // FD-10 / ADR-0201 §3 Phase 2: a v1 mission (no fog_preset field)
        // migrates forward with `Default` — today's all-Hidden init, so the
        // migrated mission's behavior and identity are unchanged.
        let def = parse(MISSION).expect("the v1 mission parses and migrates");
        assert_eq!(def.schema_version, 2);
        assert_eq!(def.fog_preset, defs::FogPreset::Default);
    }

    #[test]
    fn a_version_two_mission_carries_its_fog_preset() {
        let explored = MISSION
            .replace("schema_version: 1,", "schema_version: 2,")
            .replace(
                "    rules: \"alpha\",",
                "    rules: \"alpha\",\n    fog_preset: Explored,",
            );
        let def = parse(&explored).expect("the v2 mission with a preset parses");
        assert_eq!(def.schema_version, 2);
        assert_eq!(def.fog_preset, defs::FogPreset::Explored);
        // Absent on a v2 file: the serde default is `Default`.
        let bare = MISSION.replace("schema_version: 1,", "schema_version: 2,");
        let def = parse(&bare).expect("the v2 mission without a preset parses");
        assert_eq!(def.fog_preset, defs::FogPreset::Default);
        // All three spellings are distinct values.
        let visible = MISSION
            .replace("schema_version: 1,", "schema_version: 2,")
            .replace(
                "    rules: \"alpha\",",
                "    rules: \"alpha\",\n    fog_preset: Visible,",
            );
        let def = parse(&visible).expect("the visible preset parses");
        assert_eq!(def.fog_preset, defs::FogPreset::Visible);
    }

    #[test]
    fn unknown_fog_presets_are_rejected_loudly() {
        let bad = MISSION
            .replace("schema_version: 1,", "schema_version: 2,")
            .replace(
                "    rules: \"alpha\",",
                "    rules: \"alpha\",\n    fog_preset: Revealed,",
            );
        let err = parse(&bad).unwrap_err();
        assert!(matches!(err, ContentError::Parse { .. }), "got: {err}");
    }

    #[test]
    fn newer_schema_versions_are_refused() {
        let future = MISSION.replace("schema_version: 1,", "schema_version: 3,");
        let err = parse(&future).unwrap_err();
        assert!(
            err.to_string().contains("schema_version 3 is not loadable"),
            "got: {err}"
        );
    }

    #[test]
    fn the_id_must_match_the_file_stem() {
        let renamed = MISSION.replace("id: \"crossroads_01\",", "id: \"crossroads_02\",");
        let err = parse(&renamed).unwrap_err();
        assert!(matches!(err, ContentError::IdMismatch { .. }), "got: {err}");
    }

    #[test]
    fn display_names_must_be_ascii() {
        let unicode = MISSION.replace("Crossroads", "Kreuzungen — Süd");
        let err = parse(&unicode).unwrap_err();
        assert!(
            err.to_string().contains("must be non-empty ASCII"),
            "got: {err}"
        );
        let empty = MISSION.replace("display_name: \"Crossroads\",", "display_name: \"\",");
        let err = parse(&empty).unwrap_err();
        assert!(
            err.to_string().contains("must be non-empty ASCII"),
            "got: {err}"
        );
    }

    #[test]
    fn build_train_counts_must_be_positive() {
        let zero = MISSION.replace("count: 4", "count: 0");
        let err = parse(&zero).unwrap_err();
        assert!(err.to_string().contains("count 0"), "got: {err}");
    }

    #[test]
    fn survive_objectives_parse_and_carry_their_ticks() {
        // Phase 2 (ADR-0201 §2): the Survive kind reaches the canonical form
        // with its ticks intact — the authored unit stays ticks everywhere
        // downstream (FD-5: plain integers).
        let survive = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Survive\",\n            parameters: Survive(( ticks: 900 )),\n            optional: true,",
        );
        let def = parse(&survive).expect("the survive objective parses");
        assert_eq!(
            def.objectives[1],
            defs::ObjectiveDef {
                kind: defs::ObjectiveKind::Survive,
                parameters: defs::ObjectiveParameters::Survive { ticks: 900 },
                optional: true,
            }
        );
    }

    #[test]
    fn zero_tick_survive_timers_are_refused() {
        // R4 (the AUD-003 lesson): a zero-tick timer is vacuously complete —
        // the loader refuses it loudly, never silently completes it.
        let zero = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Survive\",\n            parameters: Survive(( ticks: 0 )),\n            optional: true,",
        );
        let err = parse(&zero).unwrap_err();
        assert!(err.to_string().contains("has ticks 0"), "got: {err}");
    }

    #[test]
    fn protect_objectives_parse_and_carry_their_kind() {
        // Phase 2 (ADR-0201 §2): the Protect kind reaches the canonical form;
        // the kind reference resolves against the tree's entities exactly
        // like every other objective reference.
        let protect = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Protect\",\n            parameters: Protect(( kind: \"command_center\" )),\n            optional: false,",
        );
        let def = parse(&protect).expect("the protect objective parses");
        assert_eq!(
            def.objectives[1],
            defs::ObjectiveDef {
                kind: defs::ObjectiveKind::Protect,
                parameters: defs::ObjectiveParameters::Protect {
                    kind: "command_center".to_string(),
                },
                optional: false,
            }
        );
        // Unknown protected kinds are loud errors, like every reference.
        let unknown = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Protect\",\n            parameters: Protect(( kind: \"watchtower\" )),\n            optional: false,",
        );
        let err = parse(&unknown).unwrap_err();
        assert!(
            err.to_string()
                .contains("references unknown entity 'watchtower'"),
            "got: {err}"
        );
    }

    #[test]
    fn kind_and_parameters_must_agree_for_the_phase_2_kinds() {
        // The spelling/block agreement check covers the new arms: a Survive
        // kind with BuildTrain parameters (and vice versa) is a loud error.
        let mismatched = MISSION.replace("kind: \"BuildTrain\",", "kind: \"Survive\",");
        let err = parse(&mismatched).unwrap_err();
        assert!(
            err.to_string()
                .contains("declares kind 'Survive' but carries 'BuildTrain' parameters"),
            "got: {err}"
        );
    }

    #[test]
    fn an_absent_missions_directory_loads_as_no_missions() {
        // read_category_optional's contract (ADR-0201 §3): a tree without
        // missions/ is valid mission-less content — the FD-10 spirit, proven
        // against a real directory scan.
        let root =
            std::env::temp_dir().join(format!("pandemonium-no-missions-{}", std::process::id()));
        for category in ["rules", "entities", "factions", "maps"] {
            std::fs::create_dir_all(root.join(category)).expect("category dir");
        }
        assert!(!root.join("missions").exists());
        let sources = SourceMap::from_dir(&root).expect("a tree without missions loads");
        assert!(sources.missions.is_empty());
        // Present but empty: also legal, also empty.
        std::fs::create_dir_all(root.join("missions")).expect("missions dir");
        let sources = SourceMap::from_dir(&root).expect("an empty missions dir loads");
        assert!(sources.missions.is_empty());
        // And a stray subdirectory inside missions/ is refused loudly, exactly
        // like every other category.
        std::fs::create_dir_all(root.join("missions").join("nested")).expect("nested dir");
        let err = SourceMap::from_dir(&root).unwrap_err();
        assert!(
            err.to_string().contains("subdirectories are not supported"),
            "got: {err}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
