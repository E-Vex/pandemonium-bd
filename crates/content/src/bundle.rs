//! The content tree and the single-match bundle: what everything above the
//! content crate consumes.
//!
//! [`ContentTree`] is every validated file in a content directory (all entities,
//! all factions, all maps, one ruleset). [`ContentBundle`] is one match's
//! content: a tree focused on one map, carrying the canonical content hash and
//! the plain world definition the simulation receives.
//!
//! The sim seam (plan §4, A-004): the bundle's [`ContentBundle::world`] produces
//! the simulation's own plain structs — a `TrivialWorld` — so `Sim::new` gains
//! its loaded-bundle path (`Sim::new(&bundle.world(), setup)`) without the
//! simulation learning that content files exist. serde/ron stay below the sim's
//! dependency line, exactly as the dependency law demands.

use std::collections::BTreeMap;
use std::path::Path;

use pandemonium_fx::XxHash64;
use pandemonium_sim::TrivialWorld;
use pandemonium_sim::{
    CapTemplate, KindEconomy, KindTemplate, ResourceDef as SimResourceDef, SpawnDef,
};
use pandemonium_sim_api::{
    KindId, MissionObjective, MissionObjectiveKind, MissionObjectiveParameters, MissionObjectives,
    PlayerId, ResourceId, Vec2Fx,
};

use crate::defs;
use crate::error::ContentError;
use crate::loader::{self, SourceMap};
use crate::validate;

/// The encoding version of the content hash: bumping it is a review-visible
/// change of the canonical encoding, never a silent hash change.
const CONTENT_ENCODING_VERSION: u32 = 1;

/// Domain-separation tag for map ids (so a map id can never collide with a
/// full-bundle hash by construction — different first block).
const MAP_ID_DOMAIN: u32 = 0x004D_4150;

/// Domain-separation tag for mission ids (ADR-0201 §7): the `MAP_ID_DOMAIN`
/// pattern with its own domain word, so a mission id can never collide with
/// a map id or a full-bundle hash by construction — the domain word is hashed
/// first and differs in every case.
const MISSION_ID_DOMAIN: u32 = 0x004D_4953;

/// Every validated file of a content directory. Maps, factions, entities, and
/// missions are plural (a directory may carry several; a missions directory
/// may be absent — the mission-less default, ADR-0201 §3); the ruleset is
/// singular (the resource registry is global).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContentTree {
    /// The one ruleset.
    pub rules: defs::RulesDef,
    /// Every entity kind, sorted by id.
    pub entities: Vec<defs::EntityDef>,
    /// Every faction, sorted by id.
    pub factions: Vec<defs::FactionDef>,
    /// Every map, sorted by id.
    pub maps: Vec<defs::MapDef>,
    /// Every mission, sorted by id (ADR-0201 §3) — empty when the tree carries
    /// no missions (the mission-less default).
    pub missions: Vec<defs::MissionDef>,
}

impl ContentTree {
    /// Loads and validates a whole content tree from named sources.
    ///
    /// Determinism: sources are consumed in the order given (the directory
    /// reader sorts), canonical collections are sorted by id, and duplicate ids
    /// are rejected — loading is a pure function of the sources.
    pub fn load(sources: &SourceMap) -> Result<Self, ContentError> {
        // Rules: exactly one (the registry is global).
        if sources.rules.len() != 1 {
            return Err(ContentError::Bundle {
                detail: format!(
                    "expected exactly one rules file, found {} (the resource registry \
                     is global — plan §9.3)",
                    sources.rules.len()
                ),
            });
        }
        let rules = loader::parse_rules(&sources.rules[0])?;

        // Entities: any count, then sorted by id with duplicate detection.
        let mut entities = Vec::with_capacity(sources.entities.len());
        for source in &sources.entities {
            entities.push(loader::parse_entity(source, &rules)?);
        }
        sort_and_check_dups(&mut entities, "entities")?;
        validate::validate_entity_references(&entities)?;

        // Factions: any count, then sorted by id with duplicate detection.
        let mut factions = Vec::with_capacity(sources.factions.len());
        for source in &sources.factions {
            factions.push(loader::parse_faction(source, &entities)?);
        }
        sort_and_check_dups(&mut factions, "factions")?;
        if factions.is_empty() {
            return Err(ContentError::Bundle {
                detail: "no faction files (a match needs at least one faction — plan §10.4)"
                    .to_string(),
            });
        }

        // Maps: any count, then sorted by id with duplicate detection.
        let mut maps = Vec::with_capacity(sources.maps.len());
        for source in &sources.maps {
            maps.push(loader::parse_map(source, &entities)?);
        }
        sort_and_check_dups(&mut maps, "maps")?;

        // Missions: any count (zero is legal — the mission-less default),
        // then sorted by id with duplicate detection (ADR-0201 §3). The
        // duplicate check runs beside the parse loop so both file names are
        // at hand — `MissionDef` carries no `file` field (the ADR-0201
        // contract's exact shape), so the tree assembly is where the names
        // live.
        let mut missions = Vec::with_capacity(sources.missions.len());
        let mut mission_files: BTreeMap<String, String> = BTreeMap::new();
        for source in &sources.missions {
            let mission = loader::parse_mission(source, &entities)?;
            let file = format!("missions/{}", source.name);
            if let Some(first) = mission_files.get(&mission.id) {
                return Err(ContentError::DuplicateId {
                    id: mission.id.clone(),
                    category: "missions".to_string(),
                    first: first.clone(),
                    second: file,
                });
            }
            mission_files.insert(mission.id.clone(), file);
            missions.push(mission);
        }
        missions.sort_by(|a, b| a.id.cmp(&b.id));

        // Mission references are tree-level (ADR-0201 §3's closed reference
        // set): every mission's map, faction, and rules must exist in the
        // loaded tree — the same discipline as the faction-level entity and
        // map-level ore-node checks.
        for mission in &missions {
            if !maps.iter().any(|map| map.id == mission.map) {
                return Err(ContentError::Bundle {
                    detail: format!(
                        "mission '{}' references unknown map '{}' (loaded maps: {})",
                        mission.id,
                        mission.map,
                        maps.iter()
                            .map(|map| map.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
            if !factions.iter().any(|faction| faction.id == mission.faction) {
                return Err(ContentError::Bundle {
                    detail: format!(
                        "mission '{}' references unknown faction '{}' (loaded factions: {})",
                        mission.id,
                        mission.faction,
                        factions
                            .iter()
                            .map(|faction| faction.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
            if rules.id != mission.rules {
                return Err(ContentError::Bundle {
                    detail: format!(
                        "mission '{}' references unknown rules '{}' (loaded ruleset: {})",
                        mission.id, mission.rules, rules.id
                    ),
                });
            }
        }

        // Placement-level validation: every faction must sit legally on every
        // map (spawn legality, ore reachability, declared symmetry).
        for map in &maps {
            for faction in &factions {
                validate::validate_map_placements(map, faction, &entities)?;
            }
        }

        Ok(Self {
            rules,
            entities,
            factions,
            maps,
            missions,
        })
    }

    /// Loads and validates a content directory.
    pub fn load_dir(root: &Path) -> Result<Self, ContentError> {
        Self::load(&SourceMap::from_dir(root)?)
    }

    /// Looks up an entity by id.
    pub fn entity(&self, id: &str) -> Option<&defs::EntityDef> {
        self.entities.iter().find(|entity| entity.id == id)
    }

    /// Looks up a map by id.
    pub fn map(&self, id: &str) -> Option<&defs::MapDef> {
        self.maps.iter().find(|map| map.id == id)
    }

    /// Looks up a mission by id (ADR-0201 §3).
    pub fn mission(&self, id: &str) -> Option<&defs::MissionDef> {
        self.missions.iter().find(|mission| mission.id == id)
    }

    /// The domain-separated mission id (ADR-0201 §7): a hash over the
    /// mission's canonical encoding — id, display name, schema version, the
    /// map/faction/rules references, and the ordered objective list — through
    /// the `MAP_ID_DOMAIN` pattern with its own domain word. The mission id
    /// rides alongside the content hash and map id (replays record it;
    /// `verify_replay` compares it) exactly as the map id does.
    ///
    /// A mission-less match computes its content hash through the exact
    /// unchanged byte path: the mission category appends nothing to the
    /// existing encoders (`encode_rules`/`encode_entities`/
    /// `encode_factions`/`encode_map` never read missions), so the registered
    /// content hash and map id cannot move.
    ///
    /// An unknown mission id returns `0` — a value no real mission id can
    /// produce (the domain word is hashed first), so callers that accept
    /// user-facing ids check [`ContentTree::mission`] first; the id itself
    /// stays total and deterministic.
    pub fn mission_id(&self, mission_id_str: &str) -> u64 {
        let Some(mission) = self.mission(mission_id_str) else {
            return 0;
        };
        let mut h = XxHash64::new();
        h.write_u32(MISSION_ID_DOMAIN);
        encode_mission(&mut h, mission);
        h.finish()
    }

    /// Resolves a mission's objective set into the simulation's vocabulary
    /// (ADR-0201 §2a): kind-id strings become `KindId`s through this tree's
    /// entity order — the exact order [`ContentBundle::world`] builds its
    /// kinds in, so the ids the objectives carry are the ids the world's
    /// entities carry. This is the mission data seam beside `world()`
    /// (FD-6: mission data reaches the simulation only through the content
    /// seam, and the sim depends on nothing above it).
    ///
    /// `player` is the mission player the objectives belong to — runtime
    /// data, not mission data: the caller picks the slot (Phase 1 tools and
    /// clients use the first map start's player).
    pub fn mission_objectives(
        &self,
        mission_id_str: &str,
        player: PlayerId,
    ) -> Result<MissionObjectives, ContentError> {
        let Some(mission) = self.mission(mission_id_str) else {
            return Err(ContentError::Bundle {
                detail: format!(
                    "unknown mission '{mission_id_str}' (loaded missions: {})",
                    self.missions
                        .iter()
                        .map(|mission| mission.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        };
        let kind_index: BTreeMap<&str, u32> = self
            .entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.id.as_str(), index as u32))
            .collect();
        let mut objectives = Vec::with_capacity(mission.objectives.len());
        for objective in &mission.objectives {
            // The loader guarantees kind/parameters agreement and reference
            // existence; the missing-lookup arms are unreachable defense that
            // error loudly rather than emit a wrong or silently-widened id.
            let parameters = match &objective.parameters {
                defs::ObjectiveParameters::Destroy { target_kind } => {
                    let resolved = target_kind.as_ref().map(|id| {
                        kind_index
                            .get(id.as_str())
                            .copied()
                            .map(KindId)
                            .ok_or_else(|| ContentError::Bundle {
                                detail: format!(
                                    "mission '{}' references kind '{}' that the tree's \
                                     entities do not carry — the loader rejected this shape, \
                                     so this is a content-seam bug, please report it",
                                    mission.id, id
                                ),
                            })
                    });
                    MissionObjectiveParameters::Destroy {
                        target_kind: resolved.transpose()?,
                    }
                }
                defs::ObjectiveParameters::BuildTrain { kind, count } => {
                    let Some(resolved) = kind_index.get(kind.as_str()).copied() else {
                        return Err(ContentError::Bundle {
                            detail: format!(
                                "mission '{}' references kind '{}' that the tree's entities do \
                                 not carry — the loader rejected this shape, so this is a \
                                 content-seam bug, please report it",
                                mission.id, kind
                            ),
                        });
                    };
                    MissionObjectiveParameters::BuildTrain {
                        kind: KindId(resolved),
                        count: *count,
                    }
                }
            };
            let kind = match parameters {
                MissionObjectiveParameters::Destroy { .. } => MissionObjectiveKind::Destroy,
                MissionObjectiveParameters::BuildTrain { .. } => MissionObjectiveKind::BuildTrain,
            };
            objectives.push(MissionObjective {
                kind,
                parameters,
                optional: objective.optional,
            });
        }
        Ok(MissionObjectives { player, objectives })
    }

    /// Builds the bundle for one map. Every faction travels along (the Alpha
    /// has one; which faction a slot plays is a post-Alpha question — plan §17).
    pub fn bundle(&self, map_id: &str) -> Result<ContentBundle, ContentError> {
        let Some(map) = self.map(map_id) else {
            return Err(ContentError::Bundle {
                detail: format!(
                    "unknown map '{map_id}' (loaded maps: {})",
                    self.maps
                        .iter()
                        .map(|map| map.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        };
        Ok(ContentBundle::new(
            self.rules.clone(),
            self.entities.clone(),
            self.factions.clone(),
            map.clone(),
        ))
    }

    /// Builds the bundle for the directory's only map (the Alpha shape).
    pub fn single_map_bundle(&self) -> Result<ContentBundle, ContentError> {
        if self.maps.len() != 1 {
            return Err(ContentError::Bundle {
                detail: format!(
                    "expected exactly one map to bundle a match, found {} ({}) — pick \
                     one explicitly with bundle(map_id)",
                    self.maps.len(),
                    self.maps
                        .iter()
                        .map(|map| map.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        self.bundle(&self.maps[0].id)
    }
}

/// Sorts a canonical collection by id and rejects duplicates with both file
/// names in the error.
fn sort_and_check_dups<T>(items: &mut [T], category: &str) -> Result<(), ContentError>
where
    T: HasIdAndFile,
{
    items.sort_by(|a, b| a.id().cmp(b.id()));
    for index in 0..items.len().saturating_sub(1) {
        if items[index].id() == items[index + 1].id() {
            return Err(ContentError::DuplicateId {
                id: items[index].id().to_string(),
                category: category.to_string(),
                first: items[index].source_file().to_string(),
                second: items[index + 1].source_file().to_string(),
            });
        }
    }
    Ok(())
}

/// Identity + source file, shared by the sortable canonical types.
trait HasIdAndFile {
    /// The item's id.
    fn id(&self) -> &str;
    /// The item's source file.
    fn source_file(&self) -> &str;
}

impl HasIdAndFile for defs::EntityDef {
    fn id(&self) -> &str {
        &self.id
    }
    fn source_file(&self) -> &str {
        &self.file
    }
}

impl HasIdAndFile for defs::FactionDef {
    fn id(&self) -> &str {
        &self.id
    }
    fn source_file(&self) -> &str {
        &self.file
    }
}

impl HasIdAndFile for defs::MapDef {
    fn id(&self) -> &str {
        &self.id
    }
    fn source_file(&self) -> &str {
        &self.file
    }
}

/// One match's content: the ruleset, every entity and faction, and the map the
/// match runs on, plus the canonical content hash computed once at
/// construction.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContentBundle {
    /// The one ruleset.
    pub rules: defs::RulesDef,
    /// Every entity kind, sorted by id (kind indices in the world definition
    /// follow this order).
    pub entities: Vec<defs::EntityDef>,
    /// Every faction, sorted by id.
    pub factions: Vec<defs::FactionDef>,
    /// The map this match runs on.
    pub map: defs::MapDef,
    content_hash: u64,
    map_hash: u64,
}

impl ContentBundle {
    /// Assembles a bundle from already-validated parts and computes its hashes.
    fn new(
        rules: defs::RulesDef,
        entities: Vec<defs::EntityDef>,
        factions: Vec<defs::FactionDef>,
        map: defs::MapDef,
    ) -> Self {
        let content_hash = {
            let mut h = XxHash64::new();
            h.write_u32(CONTENT_ENCODING_VERSION);
            encode_rules(&mut h, &rules);
            encode_entities(&mut h, &entities);
            encode_factions(&mut h, &factions);
            encode_map(&mut h, &map);
            h.finish()
        };
        let map_hash = {
            let mut h = XxHash64::new();
            h.write_u32(MAP_ID_DOMAIN);
            encode_map(&mut h, &map);
            h.finish()
        };
        Self {
            rules,
            entities,
            factions,
            map,
            content_hash,
            map_hash,
        }
    }

    /// Loads a content directory and bundles its only map (the Alpha shape).
    pub fn load_dir(root: &Path) -> Result<Self, ContentError> {
        ContentTree::load_dir(root)?.single_map_bundle()
    }

    /// The canonical content identity of this bundle (plan §6.5 `content_hash`):
    /// a hash over every canonical field — rules, entities, factions, and the
    /// whole map including the display-only heightmap — in a fixed
    /// little-endian order through `fx::XxHash64`, never over `Debug` output or
    /// memory layout (plan §5.10).
    ///
    /// The heightmap participates even though it cannot alter simulation
    /// outcomes: content identity covers the whole bundle, so any content edit
    /// (visual or mechanical) is a different match for replay-matching purposes.
    pub fn content_hash(&self) -> u64 {
        self.content_hash
    }

    /// The canonical map identity: a domain-separated hash over the map section
    /// alone. Echoed into the world definition (and replays) alongside the
    /// content hash.
    pub fn map_id(&self) -> u64 {
        self.map_hash
    }

    /// The faction every player plays in this match. The Alpha has exactly one
    /// faction (plan §10.4); per-slot faction assignment is post-Alpha
    /// (plan §17) and will extend this seam.
    pub fn faction(&self) -> &defs::FactionDef {
        &self.factions[0]
    }

    /// The plain world definition the simulation receives — the loaded-bundle
    /// path into `Sim::new` (plan §6.2):
    ///
    /// ```ignore
    /// let sim = Sim::new(&bundle.world(), setup);
    /// ```
    ///
    /// Everything here is the simulation's own vocabulary: kinds as capability
    /// compositions in authoring units (the sim's §10.2 conversion applies once,
    /// at spawn), the resource registry, the terrain passability and buildability
    /// grids (M4 navigation and M5 placement inputs), the production lists, the
    /// base population cap, and the initial spawns in a fixed, documented order
    /// — players in map-start order, each start's forces in faction order, then
    /// the map's ore nodes in authored order. Entity ids follow that order, so it
    /// is part of the match's determinism contract.
    ///
    /// Capability mapping: `Health`, `Move` (speed + collision radius), `Vision`,
    /// and — since M5 — the economy set flow into the world: `Gather`, `Build`,
    /// `Produce`, `Storage`, `ProvidesPopulation`, `Resource`, and `Footprint`.
    /// Since M6, `Attack` flows in too (DEBT-006 retired): the combat pipeline
    /// consumes it as the immediate-hit capability (plan §9.2).
    pub fn world(&self) -> TrivialWorld {
        let kind_index: BTreeMap<&str, u32> = self
            .entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.id.as_str(), index as u32))
            .collect();
        let kind_of = |id: &str| kind_index.get(id).copied();

        // The resource registry defines ResourceIds (rules are sorted by id;
        // the ledger order is that order).
        let resource_index: BTreeMap<&str, u32> = self
            .rules
            .resources
            .iter()
            .enumerate()
            .map(|(index, resource)| (resource.id.as_str(), index as u32))
            .collect();
        let resource_of = |id: &str| resource_index.get(id).copied();

        let kinds: Vec<KindTemplate> = self
            .entities
            .iter()
            .map(|entity| {
                let economy = KindEconomy {
                    // The Alpha schema prices everything in Ore; a second
                    // resource extends the cost struct and this mapping
                    // (data-only — plan §9.3). A zero cost needs no resource.
                    cost: match (entity.cost_ore > 0, resource_of("ore")) {
                        (true, Some(resource)) => vec![(ResourceId(resource), entity.cost_ore)],
                        _ => Vec::new(),
                    },
                    build_time_ticks: entity.build_time_ticks,
                    population: entity.population,
                    // Validators resolve every requirement; unreachable skips
                    // degrade to an empty list, never a wrong id.
                    requires: entity
                        .requires
                        .iter()
                        .filter_map(|id| kind_of(id))
                        .map(pandemonium_sim_api::KindId)
                        .collect(),
                };
                KindTemplate {
                    caps: entity
                        .capabilities
                        .iter()
                        .filter_map(|cap| capability_template(cap, &resource_index))
                        .collect(),
                    economy,
                }
            })
            .collect();

        // The terrain passability grid: one byte per tile from the map's
        // terrain classes (plan §9.1.1's nav-grid input, real data since M2).
        let passability: Vec<u8> = map_passability(&self.map);

        let resources: Vec<SimResourceDef> = self
            .rules
            .resources
            .iter()
            .enumerate()
            .map(|(index, resource)| SimResourceDef {
                resource: ResourceId(index as u32),
                starting: resource.starting,
            })
            .collect();

        // The faction's production lists, in faction (authored) order — which
        // kinds each producer trains is faction data flowing through the world
        // seam (plan §10.4; A-041).
        let faction = self.faction();
        let production: Vec<(
            pandemonium_sim_api::KindId,
            Vec<pandemonium_sim_api::KindId>,
        )> = faction
            .production
            .iter()
            .filter_map(|(producer, producibles)| {
                let producer_kind = kind_of(producer)?;
                let list = producibles
                    .iter()
                    .filter_map(|producible| kind_of(producible))
                    .map(pandemonium_sim_api::KindId)
                    .collect();
                Some((pandemonium_sim_api::KindId(producer_kind), list))
            })
            .collect();

        let mut initial_spawns: Vec<SpawnDef> = Vec::new();
        for start in &self.map.starts {
            for force in &faction.starting_forces {
                // Validators reject unknown references; the `continue` arms are
                // unreachable defensive skips, never silent fallback ids.
                let Some(kind) = kind_of(&force.entity) else {
                    continue;
                };
                let Some(entity) = self.entity(&force.entity) else {
                    continue;
                };
                let (w, h) = entity.footprint().unwrap_or((1, 1));
                let tile_x = start.x + force.offset.0;
                let tile_y = start.y + force.offset.1;
                initial_spawns.push(SpawnDef {
                    owner: PlayerId(start.player),
                    kind: pandemonium_sim_api::KindId(kind),
                    pos: tile_center(tile_x, tile_y, w, h),
                });
            }
        }
        for node in &self.map.ore_nodes {
            let Some(kind) = kind_of(&node.kind) else {
                continue;
            };
            let (w, h) = self
                .entity(&node.kind)
                .and_then(|entity| entity.footprint())
                .unwrap_or((1, 1));
            initial_spawns.push(SpawnDef {
                owner: PlayerId::NEUTRAL,
                kind: pandemonium_sim_api::KindId(kind),
                pos: tile_center(node.x, node.y, w, h),
            });
        }

        TrivialWorld {
            map_id: self.map_hash,
            width_tiles: self.map.width,
            height_tiles: self.map.height,
            passability,
            buildability: map_buildability(&self.map),
            kinds,
            resources,
            production,
            base_population_cap: self.rules.base_population_cap,
            initial_spawns,
            scheduled_spawns: Vec::new(),
            spawn_jitter_milli: 0,
        }
    }

    /// Looks up an entity by id.
    pub fn entity(&self, id: &str) -> Option<&defs::EntityDef> {
        self.entities.iter().find(|entity| entity.id == id)
    }
}

/// The center of a `w`x`h` footprint whose top-left tile is `(x, y)`, in
/// fixed-point tile units. Units are 1x1 (tile centers); structures sit at their
/// footprint center.
fn tile_center(x: i32, y: i32, w: u32, h: u32) -> Vec2Fx {
    let cx = (x * 2 + w as i32) * 500; // (x + w/2) in milli-tiles
    let cy = (y * 2 + h as i32) * 500;
    Vec2Fx::new(
        pandemonium_fx::Fx::from_milli(cx),
        pandemonium_fx::Fx::from_milli(cy),
    )
}

/// Maps a validated capability into the simulation's capability template, when
/// the simulation has a system for it (the M5 economy mapping — see
/// [`ContentBundle::world`]; Attack remains for M6). `resources` resolves
/// resource-id strings into the registry's [`ResourceId`] order.
fn capability_template(
    cap: &defs::CapabilityDef,
    resources: &BTreeMap<&str, u32>,
) -> Option<CapTemplate> {
    match cap {
        defs::CapabilityDef::Health {
            max_hp,
            regen_per_tick,
        } => Some(CapTemplate::Health {
            max_hp: *max_hp,
            regen_per_tick: *regen_per_tick,
        }),
        defs::CapabilityDef::Move {
            speed_milli_tiles_per_s,
            radius_milli_tiles,
        } => Some(CapTemplate::Move {
            speed_milli_tiles_per_s: *speed_milli_tiles_per_s,
            radius_milli_tiles: *radius_milli_tiles,
        }),
        defs::CapabilityDef::Vision { radius_milli_tiles } => Some(CapTemplate::Vision {
            radius_milli_tiles: *radius_milli_tiles,
        }),
        defs::CapabilityDef::Gather {
            carry_amount,
            gather_time_ms,
            ..
        } => Some(CapTemplate::Gather {
            carry_amount: *carry_amount,
            gather_time_ms: *gather_time_ms,
        }),
        defs::CapabilityDef::Build => Some(CapTemplate::Build {}),
        defs::CapabilityDef::Produce => Some(CapTemplate::Produce {}),
        defs::CapabilityDef::Storage { resources: list } => Some(CapTemplate::Storage {
            resources: list
                .iter()
                .filter_map(|id| resources.get(id.as_str()).copied())
                .map(ResourceId)
                .collect(),
        }),
        defs::CapabilityDef::ProvidesPopulation { amount } => {
            Some(CapTemplate::ProvidesPopulation { amount: *amount })
        }
        defs::CapabilityDef::Resource { resource, amount } => Some(CapTemplate::Resource {
            resource: ResourceId(resources.get(resource.as_str()).copied()?),
            amount: *amount,
        }),
        defs::CapabilityDef::Footprint { w, h } => Some(CapTemplate::Footprint { w: *w, h: *h }),
        // Attack mapped at the seam (M6, DEBT-006 retired). The content
        // encoder already includes Attack data in the bundle's hash
        // (`encode_capability` writes the tag + fields), so flipping this arm
        // does not change the content hash — only what the simulation sees.
        // `cooldown_ticks` is precomputed by the loader (plan §10.2 ms -> ticks
        // ceiling division); we carry the ms field for parity with the other
        // milli-unit templates and let `to_runtime` recompute ticks itself.
        defs::CapabilityDef::Attack {
            damage,
            range_milli_tiles,
            cooldown_ms,
            cooldown_ticks: _,
            acquire_range_milli_tiles,
        } => Some(CapTemplate::Attack {
            damage: *damage,
            range_milli_tiles: *range_milli_tiles,
            cooldown_ms: *cooldown_ms,
            acquire_range_milli_tiles: *acquire_range_milli_tiles,
        }),
    }
}

/// The map's terrain as the simulation's passability grid: one byte per tile,
/// row-major, 1 passable and 0 blocked (plan §9.1.1). The grid's shape is
/// already validated against the terrain classes (loader guarantees).
fn map_passability(map: &defs::MapDef) -> Vec<u8> {
    map_flag(map, |class| class.passable)
}

/// The map's terrain as the simulation's buildability grid: one byte per tile,
/// row-major, 1 buildable and 0 not — the Build command's placement input
/// (plan §10.5). Mirrors [`map_passability`].
fn map_buildability(map: &defs::MapDef) -> Vec<u8> {
    map_flag(map, |class| class.buildable)
}

/// The shared body of [`map_passability`] and [`map_buildability`]: walks the
/// map grid row by row, looks each tile's terrain class up by its character
/// code, and emits `1`/`0` per tile per the `pick` predicate. Unknown codes
/// fail closed (the loader guarantees codes are known, so an unknown here is a
/// schema bug, not a gameplay event). The output vector's length is
/// `width * height` in tile units; the `u64` cast guards against the
/// theoretical u32 overflow on a 65k×65k map (no Alpha map is anywhere near
/// that).
fn map_flag(map: &defs::MapDef, pick: impl Fn(&defs::TerrainClass) -> bool) -> Vec<u8> {
    let class_of: BTreeMap<char, &defs::TerrainClass> = map
        .terrain
        .iter()
        .map(|class| (class.code, class))
        .collect();
    let mut flags = Vec::with_capacity((map.width as u64 * map.height as u64) as usize);
    for row in &map.grid {
        for code in row.chars() {
            let on = class_of
                .get(&code)
                .map(|class| pick(class))
                .unwrap_or(false); // unknown codes never validate; fail closed
            flags.push(u8::from(on));
        }
    }
    flags
}

// ---------------------------------------------------------------------------
// Canonical encoding (the content hash) — plan §5.10: explicit little-endian
// byte encoding through fx::XxHash64, fixed field order, never Debug output.
// ---------------------------------------------------------------------------

fn encode_str(h: &mut XxHash64, text: &str) {
    h.write_u32(text.len() as u32);
    h.write_bytes(text.as_bytes());
}

fn encode_bool(h: &mut XxHash64, value: bool) {
    h.write_u8(u8::from(value));
}

fn encode_rules(h: &mut XxHash64, rules: &defs::RulesDef) {
    encode_str(h, &rules.id);
    encode_str(h, &rules.display_name);
    h.write_u32(rules.resources.len() as u32);
    for resource in &rules.resources {
        encode_str(h, &resource.id);
        encode_str(h, &resource.display_name);
        h.write_i64(resource.starting);
    }
    h.write_u32(rules.base_population_cap);
    h.write_u32(match rules.victory {
        defs::VictoryKind::Elimination => 1,
    });
    encode_bool(h, rules.allow_resignation);
}

fn encode_entities(h: &mut XxHash64, entities: &[defs::EntityDef]) {
    h.write_u32(entities.len() as u32);
    for entity in entities {
        encode_str(h, &entity.id);
        encode_str(h, &entity.display_name);
        h.write_u32(entity.capabilities.len() as u32);
        for cap in &entity.capabilities {
            encode_capability(h, cap);
        }
        h.write_i64(entity.cost_ore);
        h.write_u32(entity.build_time_ms);
        h.write_i32(entity.population);
        h.write_u32(entity.requires.len() as u32);
        for requirement in &entity.requires {
            encode_str(h, requirement);
        }
    }
}

fn encode_capability(h: &mut XxHash64, cap: &defs::CapabilityDef) {
    match cap {
        defs::CapabilityDef::Health {
            max_hp,
            regen_per_tick,
        } => {
            h.write_u8(1);
            h.write_i32(*max_hp);
            h.write_i32(*regen_per_tick);
        }
        defs::CapabilityDef::Move {
            speed_milli_tiles_per_s,
            radius_milli_tiles,
        } => {
            h.write_u8(2);
            h.write_i32(*speed_milli_tiles_per_s);
            h.write_i32(*radius_milli_tiles);
        }
        defs::CapabilityDef::Attack {
            damage,
            range_milli_tiles,
            cooldown_ms,
            acquire_range_milli_tiles,
            ..
        } => {
            h.write_u8(3);
            h.write_i32(*damage);
            h.write_i32(*range_milli_tiles);
            h.write_u32(*cooldown_ms);
            h.write_i32(*acquire_range_milli_tiles);
        }
        defs::CapabilityDef::Gather {
            carry_amount,
            gather_time_ms,
            ..
        } => {
            h.write_u8(4);
            h.write_i64(*carry_amount);
            h.write_u32(*gather_time_ms);
        }
        defs::CapabilityDef::Build => h.write_u8(5),
        defs::CapabilityDef::Produce => h.write_u8(6),
        defs::CapabilityDef::Storage { resources } => {
            h.write_u8(7);
            h.write_u32(resources.len() as u32);
            for resource in resources {
                encode_str(h, resource);
            }
        }
        defs::CapabilityDef::ProvidesPopulation { amount } => {
            h.write_u8(8);
            h.write_i32(*amount);
        }
        defs::CapabilityDef::Resource { resource, amount } => {
            h.write_u8(9);
            encode_str(h, resource);
            h.write_i64(*amount);
        }
        defs::CapabilityDef::Vision { radius_milli_tiles } => {
            h.write_u8(10);
            h.write_i32(*radius_milli_tiles);
        }
        defs::CapabilityDef::Footprint { w, h: height } => {
            h.write_u8(11);
            h.write_u32(*w);
            h.write_u32(*height);
        }
    }
}

fn encode_factions(h: &mut XxHash64, factions: &[defs::FactionDef]) {
    h.write_u32(factions.len() as u32);
    for faction in factions {
        encode_str(h, &faction.id);
        encode_str(h, &faction.display_name);
        h.write_u32(faction.roster.len() as u32);
        for member in &faction.roster {
            encode_str(h, member);
        }
        h.write_u32(faction.production.len() as u32);
        for (producer, producibles) in &faction.production {
            encode_str(h, producer);
            h.write_u32(producibles.len() as u32);
            for producible in producibles {
                encode_str(h, producible);
            }
        }
        h.write_u32(faction.starting_forces.len() as u32);
        for force in &faction.starting_forces {
            encode_str(h, &force.entity);
            h.write_i32(force.offset.0);
            h.write_i32(force.offset.1);
        }
    }
}

fn encode_map(h: &mut XxHash64, map: &defs::MapDef) {
    encode_str(h, &map.id);
    encode_str(h, &map.display_name);
    h.write_u32(map.width);
    h.write_u32(map.height);
    h.write_u32(map.terrain.len() as u32);
    for class in &map.terrain {
        h.write_u32(class.code as u32); // full scalar value — no truncation
        encode_str(h, &class.name);
        encode_bool(h, class.passable);
        encode_bool(h, class.buildable);
    }
    h.write_u32(map.grid.len() as u32);
    for row in &map.grid {
        encode_str(h, row);
    }
    h.write_u32(map.starts.len() as u32);
    for start in &map.starts {
        h.write_u8(start.player);
        h.write_i32(start.x);
        h.write_i32(start.y);
    }
    h.write_u32(map.ore_nodes.len() as u32);
    for node in &map.ore_nodes {
        encode_str(h, &node.kind);
        h.write_i32(node.x);
        h.write_i32(node.y);
    }
    encode_bool(h, map.symmetric);
    match &map.heightmap {
        Some(heightmap) => {
            encode_bool(h, true);
            h.write_u32(heightmap.rows.len() as u32);
            for row in &heightmap.rows {
                h.write_u32(row.len() as u32);
                for height in row {
                    h.write_u16(*height);
                }
            }
        }
        None => encode_bool(h, false),
    }
}

/// The canonical encoding of one mission record (ADR-0201 §7) — fixed
/// little-endian order through `fx::XxHash64`, exactly the discipline of the
/// four encoders above. Reachable only through [`ContentTree::mission_id`]:
/// the mission category appends nothing to the content hash's or the map id's
/// byte paths, which is why the registered goldens cannot move (the
/// mission-less path is byte-identical by construction).
fn encode_mission(h: &mut XxHash64, mission: &defs::MissionDef) {
    encode_str(h, &mission.id);
    encode_str(h, &mission.display_name);
    h.write_u32(mission.schema_version);
    encode_str(h, &mission.map);
    encode_str(h, &mission.faction);
    encode_str(h, &mission.rules);
    h.write_u32(mission.objectives.len() as u32);
    for objective in &mission.objectives {
        // The kind tag: fixed, ordered, exhaustive over the Phase 1 vocabulary.
        h.write_u8(match objective.kind {
            defs::ObjectiveKind::Destroy => 1,
            defs::ObjectiveKind::BuildTrain => 2,
        });
        match &objective.parameters {
            defs::ObjectiveParameters::Destroy { target_kind } => match target_kind {
                Some(kind) => {
                    encode_bool(h, true);
                    encode_str(h, kind);
                }
                None => encode_bool(h, false),
            },
            defs::ObjectiveParameters::BuildTrain { kind, count } => {
                encode_str(h, kind);
                h.write_u32(*count);
            }
        }
        encode_bool(h, objective.optional);
    }
}

#[cfg(test)]
mod tests {
    //! Mission identity (ADR-0201 §7, B-012 Task 2): the domain-separated
    //! mission id, and the golden-preservation property — adding a mission to
    //! a tree changes neither the content hash nor the map id, because the
    //! mission category appends nothing to the existing encoders. The
    //! in-memory factory mirrors `tests.rs`'s shape; these live here so the
    //! identity surface is tested in the file that owns it.

    use super::*;

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
        Vision(( radius_milli_tiles: 7000 )),
    ],
    cost: ( ore: 50 ),
    build_time_ms: 12000,
    population: 1,
    requires: [],
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

    /// A 16x16 all-ground map, two starts, two ore nodes — the factory shape.
    fn map_text() -> String {
        let row = ".".repeat(16);
        let grid = vec![row; 16].join("\",\n        \"");
        format!(
            r#"
(
    schema_version: 2,
    id: "testing",
    display_name: "Testing Grounds",
    width: 16,
    height: 16,
    terrain: [
        ( code: ".", name: "ground", passable: true, buildable: true ),
    ],
    grid: [
        "{grid}",
    ],
    starts: [
        ( player: 0, anchor: ( x: 3, y: 3 ) ),
        ( player: 1, anchor: ( x: 10, y: 10 ) ),
    ],
    ore_nodes: [
        ( kind: "ore_node", tile: ( x: 1, y: 1 ) ),
        ( kind: "ore_node", tile: ( x: 13, y: 13 ) ),
    ],
    symmetric: false,
)
"#
        )
    }

    const MISSION: &str = r#"
(
    schema_version: 1,
    id: "testing_01",
    display_name: "Testing Grounds Assault",
    map: "testing",
    faction: "testers",
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

    fn factory() -> SourceMap {
        SourceMap::new()
            .rules("alpha.ron", RULES)
            .entity("base.ron", BASE)
            .entity("worker.ron", WORKER)
            .entity("ore_node.ron", ORE_NODE)
            .faction("testers.ron", FACTION)
            .map("testing.ron", &map_text())
    }

    #[test]
    fn a_tree_with_a_mission_lists_it_sorted_and_looks_it_up() {
        let tree = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the factory with a mission loads");
        assert_eq!(tree.missions.len(), 1);
        let mission = tree.mission("testing_01").expect("the mission is found");
        assert_eq!(mission.display_name, "Testing Grounds Assault");
        assert!(tree.mission("missing").is_none());
    }

    #[test]
    fn adding_a_mission_moves_neither_the_content_hash_nor_the_map_id() {
        // The golden-preservation property (ADR-0201 §7): the mission
        // category appends nothing to the content hash's or the map id's
        // encoders, so a mission-bearing tree produces the exact same bundle
        // identity as the mission-less one.
        let plain = ContentTree::load(&factory()).expect("the mission-less tree loads");
        let with_mission = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the mission-bearing tree loads");
        let plain_bundle = plain.bundle("testing").expect("bundle");
        let mission_bundle = with_mission.bundle("testing").expect("bundle");
        assert_eq!(
            plain_bundle.content_hash(),
            mission_bundle.content_hash(),
            "the content hash must not move when a mission joins the tree"
        );
        assert_eq!(
            plain_bundle.map_id(),
            mission_bundle.map_id(),
            "the map id must not move when a mission joins the tree"
        );
        // And the two identities stay distinct values.
        assert_ne!(plain_bundle.content_hash(), plain_bundle.map_id());
    }

    #[test]
    fn mission_ids_are_deterministic_and_domain_separated() {
        let tree = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the mission-bearing tree loads");
        let first = tree.mission_id("testing_01");
        let second = tree.mission_id("testing_01");
        assert_eq!(first, second, "the mission id is a pure function");
        assert_ne!(first, 0, "a real mission id is never the unknown sentinel");
        // Domain separation from the map id (different domain words over
        // overlapping fields would still collide only by hash accident — the
        // domain words make the byte paths differ from the first block).
        let bundle = tree.bundle("testing").expect("bundle");
        assert_ne!(first, bundle.map_id());
        assert_ne!(first, bundle.content_hash());
        // The unknown-mission sentinel.
        assert_eq!(tree.mission_id("missing"), 0);
    }

    #[test]
    fn mission_identity_follows_the_mission_bytes() {
        // A different objective set is a different mission (ADR-0201 §7: the
        // mission record is identity-bearing).
        let tree = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the mission-bearing tree loads");
        let edited = MISSION.replace("count: 4", "count: 5");
        let tree_edited =
            ContentTree::load(&factory().mission("testing_01.ron", &edited)).expect("loads");
        assert_ne!(
            tree.mission_id("testing_01"),
            tree_edited.mission_id("testing_01"),
            "editing an objective changes the mission id"
        );
        let renamed = MISSION.replace("Testing Grounds Assault", "Second Assault");
        let tree_renamed =
            ContentTree::load(&factory().mission("testing_01.ron", &renamed)).expect("loads");
        assert_ne!(
            tree.mission_id("testing_01"),
            tree_renamed.mission_id("testing_01"),
            "editing the display name changes the mission id"
        );
    }

    #[test]
    fn mission_references_are_validated_against_the_tree() {
        let bad_map = MISSION.replace("map: \"testing\",", "map: \"elsewhere\",");
        let err = ContentTree::load(&factory().mission("testing_01.ron", &bad_map)).unwrap_err();
        assert!(
            err.to_string()
                .contains("references unknown map 'elsewhere'"),
            "got: {err}"
        );
        let bad_faction = MISSION.replace("faction: \"testers\",", "faction: \"raiders\",");
        let err =
            ContentTree::load(&factory().mission("testing_01.ron", &bad_faction)).unwrap_err();
        assert!(
            err.to_string()
                .contains("references unknown faction 'raiders'"),
            "got: {err}"
        );
        let bad_rules = MISSION.replace("rules: \"alpha\",", "rules: \"beta\",");
        let err = ContentTree::load(&factory().mission("testing_01.ron", &bad_rules)).unwrap_err();
        assert!(
            err.to_string().contains("references unknown rules 'beta'"),
            "got: {err}"
        );
    }

    #[test]
    fn duplicate_mission_ids_are_refused_with_both_files() {
        // `SourceMap` has public fields, so a caller can hand the loader the
        // same mission file twice — the duplicate-id guard catches it (the
        // same discipline as `duplicate_source_names_are_rejected_with_both_files`;
        // file stems on disk are unique, so the directory reader can never
        // produce this).
        let mut sources = factory().mission("testing_01.ron", MISSION);
        sources
            .missions
            .push(crate::Source::new("testing_01.ron", MISSION));
        let err = ContentTree::load(&sources).unwrap_err();
        assert!(
            err.to_string()
                .contains("duplicate id 'testing_01' in category 'missions'"),
            "got: {err}"
        );
    }

    #[test]
    fn mission_objectives_resolve_kind_ids_through_the_tree_entity_order() {
        // The content seam (ADR-0201 §2a): string kind ids become KindIds in
        // the tree's entity order — base sorts before ore_node before worker
        // (id order), so worker is KindId(2) — the same index the world's
        // kinds vector assigns.
        let tree = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the mission-bearing tree loads");
        let objectives = tree
            .mission_objectives("testing_01", PlayerId(0))
            .expect("the objective set resolves");
        assert_eq!(objectives.player, PlayerId(0));
        assert_eq!(objectives.objectives.len(), 2);
        assert_eq!(
            objectives.objectives[0],
            MissionObjective {
                kind: MissionObjectiveKind::Destroy,
                parameters: MissionObjectiveParameters::Destroy { target_kind: None },
                optional: false,
            }
        );
        assert_eq!(
            objectives.objectives[1],
            MissionObjective {
                kind: MissionObjectiveKind::BuildTrain,
                parameters: MissionObjectiveParameters::BuildTrain {
                    kind: KindId(2), // "worker" — the third id in sorted order
                    count: 4,
                },
                optional: true,
            }
        );
        // And a named destroy target resolves too.
        let targeted = MISSION.replace(
            "Destroy(( target_kind: None ))",
            "Destroy(( target_kind: Some(\"base\") ))",
        );
        let tree = ContentTree::load(&factory().mission("testing_01.ron", &targeted))
            .expect("the targeted mission loads");
        let objectives = tree
            .mission_objectives("testing_01", PlayerId(0))
            .expect("resolves");
        assert_eq!(
            objectives.objectives[0].parameters,
            MissionObjectiveParameters::Destroy {
                target_kind: Some(KindId(0)), // "base" sorts first
            }
        );
        // Unknown missions are refused loudly.
        let err = tree.mission_objectives("missing", PlayerId(0)).unwrap_err();
        assert!(
            err.to_string().contains("unknown mission 'missing'"),
            "got: {err}"
        );
    }
}
