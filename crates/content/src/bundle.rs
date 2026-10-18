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
    AreaSpec, FogPreset, GatherSemantics, KindId, MissionData, MissionObjective,
    MissionObjectiveKind, MissionObjectiveParameters, PlayerId, ResourceId, Vec2Fx,
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
            // B-017's vacuous-failure guard (AUD-003): a Protect objective
            // must name a kind the mission player's faction actually
            // starts with — the faction's starting forces are the effective
            // force at tick 0 (placement legality is validated below), so
            // protecting anything else would fail on the first tick the
            // evaluator runs. The check is tree-level because it needs the
            // mission's faction — the same reason the faction reference
            // check above lives here.
            if let Some(faction) = factions
                .iter()
                .find(|faction| faction.id == mission.faction)
            {
                for objective in &mission.objectives {
                    if let defs::ObjectiveParameters::Protect { kind } = &objective.parameters {
                        if !faction
                            .starting_forces
                            .iter()
                            .any(|force| &force.entity == kind)
                        {
                            return Err(ContentError::Bundle {
                                detail: format!(
                                    "mission '{}' protects kind '{}' that the starting \
                                     forces of faction '{}' do not carry (starting \
                                     forces: {}): a protect objective with nothing to \
                                     protect fails on the first tick",
                                    mission.id,
                                    kind,
                                    faction.id,
                                    faction
                                        .starting_forces
                                        .iter()
                                        .map(|force| force.entity.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            });
                        }
                    }
                }
                // B-018's vacuous-completion guard (AUD-003), tree-level
                // because it needs the mission's map and faction: a
                // ReachArea objective must demand actual movement. Three
                // shapes are refused — an area that leaves the map (the
                // mission would reference ground no entity can stand on),
                // an area that covers the whole map (every position is
                // already "inside" — vacuous by authoring), and an area
                // that already contains a starting entity of the mission
                // player at tick 0 (the objective completes on the first
                // evaluated tick without a single order). The tick-0
                // check runs the SIM's own containment predicate over the
                // exact positions the world seam builds (tile_center),
                // so the gate and the evaluator can never disagree.
                if let Some(map) = maps.iter().find(|map| map.id == mission.map) {
                    for objective in &mission.objectives {
                        if let defs::ObjectiveParameters::ReachArea { area } = &objective.parameters
                        {
                            validate_reach_area(mission, area, map, faction, &entities)?;
                        }
                    }
                }
            }

            // B-018's gather gate (AUD-003), tree-level because it
            // needs the ruleset: a gather objective must demand play —
            // an amount at or below the ruleset's effective starting
            // ore (200 in the Alpha) would complete at (or before) the
            // first delivery for BOTH semantics: the balance starts
            // there, and the cumulative total crosses it with the
            // first trip. Also, the resource is ore by the
            // objective's definition, so a ruleset that registers no
            // "ore" cannot carry a gather objective at all.
            for objective in &mission.objectives {
                if let defs::ObjectiveParameters::Gather { amount, .. } = &objective.parameters {
                    let Some(ore) = rules.resources.iter().find(|r| r.id == "ore") else {
                        return Err(ContentError::Bundle {
                            detail: format!(
                                "mission '{}' gathers ore but ruleset '{}' registers no \
                                 'ore' resource (registered: {}): the gather objective \
                                 counts ore by definition",
                                mission.id,
                                rules.id,
                                rules
                                    .resources
                                    .iter()
                                    .map(|resource| resource.id.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        });
                    };
                    if *amount <= ore.starting {
                        return Err(ContentError::Bundle {
                            detail: format!(
                                "mission '{}' gathers {} ore, at or below the starting \
                                 ore {} of ruleset '{}' (both semantics): the objective \
                                 must demand play — raise the amount above the \
                                 starting ore",
                                mission.id, amount, ore.starting, rules.id
                            ),
                        });
                    }
                }
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

    /// Resolves a mission's data set into the simulation's vocabulary
    /// (ADR-0201 §2a): kind-id strings become `KindId`s through this tree's
    /// entity order — the exact order [`ContentBundle::world`] builds its
    /// kinds in, so the ids the objectives carry are the ids the world's
    /// entities carry — and the fog preset (B-015) converts from the content
    /// mirror into the sim's `FogPreset`. This is the mission data seam
    /// beside `world()` (FD-6: mission data reaches the simulation only
    /// through the content seam, and the sim depends on nothing above it).
    ///
    /// `player` is the mission player the objectives belong to — runtime
    /// data, not mission data: the caller picks the slot (Phase 1 tools and
    /// clients use the first map start's player).
    pub fn mission_data(
        &self,
        mission_id_str: &str,
        player: PlayerId,
    ) -> Result<MissionData, ContentError> {
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
                // B-017: survive parameters are plain data — the integer
                // tick count crosses the seam verbatim (FD-5: no floats, no
                // unit conversion; the tick is the sim's own unit).
                defs::ObjectiveParameters::Survive { ticks } => {
                    MissionObjectiveParameters::Survive { ticks: *ticks }
                }
                // B-017: the protected kind resolves through the entity
                // order — the exact resolution BuildTrain's kind gets, so
                // the objective's KindId is the id the world's entities
                // carry.
                defs::ObjectiveParameters::Protect { kind } => {
                    let Some(resolved) = kind_index.get(kind.as_str()).copied() else {
                        return Err(ContentError::Bundle {
                            detail: format!(
                                "mission '{}' references kind '{}' that the tree's \
                                 entities do not carry — the loader rejected this shape, \
                                 so this is a content-seam bug, please report it",
                                mission.id, kind
                            ),
                        });
                    };
                    MissionObjectiveParameters::Protect {
                        kind: KindId(resolved),
                    }
                }
                // B-018: the area crosses the seam as plain data — the
                // shapes are unit-tagged (tiles / milli-tiles by variant),
                // so there is nothing to resolve and nothing to convert
                // except the type mirror itself (the FogPreset pattern).
                // The containment law lives on the sim's type; the
                // loader's gates already proved the area lies inside the
                // mission's map, covers something less than the whole
                // map, and starts empty of the mission player's forces.
                defs::ObjectiveParameters::ReachArea { area } => {
                    MissionObjectiveParameters::ReachArea {
                        area: sim_area(area),
                    }
                }
                // B-018: the gather objective's resource is ore by
                // definition, resolved through the rules registry's order
                // — the exact order `world()` assigns ResourceIds (the
                // ledger order), so the id the objective carries is the
                // id the world's ledger carries. The semantics converts
                // mirror-to-sim, variant for variant. The tree-level gate
                // guarantees the ruleset registers ore; the missing
                // lookup arm is unreachable defense that errors loudly.
                defs::ObjectiveParameters::Gather { amount, semantics } => {
                    let resource_index = self
                        .rules
                        .resources
                        .iter()
                        .position(|resource| resource.id == "ore")
                        .ok_or_else(|| ContentError::Bundle {
                            detail: format!(
                                "mission '{}' gathers ore but ruleset '{}' registers no \
                                 'ore' resource — the loader rejected this shape, so this \
                                 is a content-seam bug, please report it",
                                mission.id, self.rules.id
                            ),
                        })?;
                    MissionObjectiveParameters::Gather {
                        resource: ResourceId(resource_index as u32),
                        amount: *amount,
                        semantics: match semantics {
                            defs::GatherSemantics::Cumulative => GatherSemantics::Cumulative,
                            defs::GatherSemantics::Balance => GatherSemantics::Balance,
                        },
                    }
                }
            };
            let kind = match parameters {
                MissionObjectiveParameters::Destroy { .. } => MissionObjectiveKind::Destroy,
                MissionObjectiveParameters::BuildTrain { .. } => MissionObjectiveKind::BuildTrain,
                MissionObjectiveParameters::Survive { .. } => MissionObjectiveKind::Survive,
                MissionObjectiveParameters::Protect { .. } => MissionObjectiveKind::Protect,
                MissionObjectiveParameters::ReachArea { .. } => MissionObjectiveKind::ReachArea,
                MissionObjectiveParameters::Gather { .. } => MissionObjectiveKind::Gather,
            };
            objectives.push(MissionObjective {
                kind,
                parameters,
                optional: objective.optional,
            });
        }
        Ok(MissionData {
            player,
            objectives,
            // B-015: the fog preset crosses the seam with the objectives —
            // the mirror-to-sim conversion, variant for variant.
            fog_preset: match mission.fog_preset {
                defs::FogPreset::Default => FogPreset::Default,
                defs::FogPreset::Explored => FogPreset::Explored,
                defs::FogPreset::Visible => FogPreset::Visible,
            },
        })
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

    /// Builds a bundle for the first map in canonical id order. Use this for
    /// legacy/default flows; match selection should use [`Self::bundle`].
    pub fn default_map_bundle(&self) -> Result<ContentBundle, ContentError> {
        let Some(map) = self.maps.first() else {
            return Err(ContentError::Bundle {
                detail: "expected at least one map to bundle a match".to_string(),
            });
        };
        self.bundle(&map.id)
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

    /// Loads a content directory and bundles its default map (first by id).
    pub fn load_dir(root: &Path) -> Result<Self, ContentError> {
        ContentTree::load_dir(root)?.default_map_bundle()
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
    /// consumes it as the immediate-hit capability (plan §9.2). Since B-016,
    /// the faction's `roster` flows in as well (in authored order, resolved
    /// through the entity order like the production lists) — the Build gate's
    /// authority, beside the Train gate's production lists.
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

        // The faction's build roster, in authored order, resolved through the
        // entity order — the production lists' exact pattern (plan §10.6: the
        // kinds a faction fields are faction data). The Build gate reads it
        // (B-016, ADR-0201 §3's closed asymmetry — Train has always been
        // gated by the production lists; Build is now gated by the roster).
        // The bundle's content hash already encodes the authored roster
        // through `encode_factions`; the world's fixture hash deliberately
        // does not see this field.
        let roster: Vec<pandemonium_sim_api::KindId> = faction
            .roster
            .iter()
            .filter_map(|member| kind_of(member))
            .map(pandemonium_sim_api::KindId)
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
            roster,
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

/// Converts a validated content area into the sim's vocabulary (B-018, the
/// `FogPreset` conversion's pattern — plain data, variant for variant, no
/// unit conversion anywhere: the shapes are unit-tagged). Shared by the
/// mission seam (`mission_data`) and the tree-level tick-0 gate, so both
/// evaluate the exact area the evaluator will see.
fn sim_area(area: &defs::AreaSpec) -> AreaSpec {
    match *area {
        defs::AreaSpec::Rect { x, y, w, h } => AreaSpec::Rect { x, y, w, h },
        defs::AreaSpec::Circle { cx, cy, radius } => AreaSpec::Circle { cx, cy, radius },
    }
}

/// B-018's tree-level reach-area gates (AUD-003) — every rejection names the
/// mission, the objective's area, and the law it broke. Two checks, coverage
/// first (the semantic authoring error — "every position is already
/// inside"), bounds second (the structural one — "ground no entity can
/// stand on"):
///
/// 1. **Whole-map coverage** — a rect spanning the full grid (`x <= 0`,
///    `y <= 0`, `x + w >= width`, `y + h >= height` — the exact grid match
///    and any overspan of it), or a circle containing all four map corners
///    (the map rectangle is the convex hull of its corners and the disk is
///    convex, so corners inside imply every point inside), makes "reach the
///    area" vacuous by authoring. Coverage is checked before bounds
///    deliberately: a circle that covers the whole map ALWAYS leaves it
///    (containing the corners needs `2r >= diagonal > min(W, H) >= 2r`
///    in-bounds — a contradiction), so bounds-first would swallow the
///    semantic error into the structural one.
/// 2. **Bounds** — the area must lie inside the mission's map. A rect fits
///    when `[x, x+w) x [y, y+h)` sits in the tile grid; a circle fits when
///    its disk fits the map rectangle, which is exactly its bounding box
///    `[cx-r, cx+r] x [cy-r, cy+r]` fitting (the disk is a subset of its
///    bounding box, so the box inside the map implies the disk inside).
///    i64 math throughout — authored values are far from the
///    representable edge, and the gate stays total over absurd inputs.
/// 3. **Tick-0 containment** — the area must not already contain a
///    starting-force entity of the mission player (the mission's faction
///    placed at the map's first start — the mission-player convention
///    every Phase 1/2 caller uses; `verify.rs`'s re-simulation picks the
///    same slot). The check evaluates each force's exact spawn position —
///    `tile_center(start + offset, footprint)` — through the SIM's
///    `AreaSpec::contains`, the same predicate the stage-10 evaluator
///    runs, so the gate and the evaluation can never drift. Content
///    worlds carry no spawn jitter (`spawn_jitter_milli: 0`), so the
///    positions at tick 0 are exactly these.
fn validate_reach_area(
    mission: &defs::MissionDef,
    area: &defs::AreaSpec,
    map: &defs::MapDef,
    faction: &defs::FactionDef,
    entities: &[defs::EntityDef],
) -> Result<(), ContentError> {
    let width = map.width as i64;
    let height = map.height as i64;
    match *area {
        defs::AreaSpec::Rect { x, y, w, h } => {
            let (x, y) = (x as i64, y as i64);
            let (w, h) = (w as i64, h as i64);
            if x <= 0 && y <= 0 && x + w >= width && y + h >= height {
                return Err(ContentError::Bundle {
                    detail: format!(
                        "mission '{}' reaches a rect [x {}, y {}, w {}, h {}] covering the \
                         whole map '{}' ({}x{} tiles): every position is already inside — \
                         the objective would complete without a single order",
                        mission.id, x, y, w, h, map.id, map.width, map.height
                    ),
                });
            }
            if x < 0 || y < 0 || x + w > width || y + h > height {
                return Err(ContentError::Bundle {
                    detail: format!(
                        "mission '{}' reaches a rect [x {}, y {}, w {}, h {}] that leaves \
                         the map '{}' ({}x{} tiles): reach areas must lie inside the map",
                        mission.id, x, y, w, h, map.id, map.width, map.height
                    ),
                });
            }
        }
        defs::AreaSpec::Circle { cx, cy, radius } => {
            let (cx, cy, radius) = (cx as i64, cy as i64, radius as i64);
            let width_milli = width * 1000;
            let height_milli = height * 1000;
            // Corners in milli-tiles; the disk is convex, so all four
            // corners inside implies the whole map rectangle inside.
            let corners = [
                (0, 0),
                (width_milli, 0),
                (0, height_milli),
                (width_milli, height_milli),
            ];
            let covers = corners.iter().all(|&(corner_x, corner_y)| {
                let dx = corner_x - cx;
                let dy = corner_y - cy;
                dx * dx + dy * dy <= radius * radius
            });
            if covers {
                return Err(ContentError::Bundle {
                    detail: format!(
                        "mission '{}' reaches a circle (center {}x{} milli-tiles, radius \
                         {}) containing all four corners of the map '{}': every position \
                         is already inside — the objective would complete without a \
                         single order",
                        mission.id, cx, cy, radius, map.id
                    ),
                });
            }
            if cx - radius < 0
                || cy - radius < 0
                || cx + radius > width_milli
                || cy + radius > height_milli
            {
                return Err(ContentError::Bundle {
                    detail: format!(
                        "mission '{}' reaches a circle (center {}x{} milli-tiles, radius \
                         {}) that leaves the map '{}' ({}x{} tiles): reach areas must lie \
                         inside the map",
                        mission.id, cx, cy, radius, map.id, map.width, map.height
                    ),
                });
            }
        }
    }
    // The tick-0 gate: the mission player's convention slot is the map's
    // first start (the slot every Phase 1/2 caller passes `mission_data`);
    // its forces are the mission's faction placed at that anchor. Each
    // force's tick-0 position is the exact `tile_center` the world seam
    // builds (footprint from the entity kind, defaulting to 1x1 for units
    // — the same default), tested through the sim's own predicate.
    let Some(start) = map.starts.first() else {
        // A map with no starts cannot carry a mission at all (the world
        // seam would spawn nothing); the placement validator below refuses
        // start-less maps for every faction. Defensive only.
        return Ok(());
    };
    let sim = sim_area(area);
    for force in &faction.starting_forces {
        let (w, h) = entities
            .iter()
            .find(|entity| entity.id == force.entity)
            .and_then(|entity| entity.footprint())
            .unwrap_or((1, 1));
        let pos = tile_center(start.x + force.offset.0, start.y + force.offset.1, w, h);
        if sim.contains(pos) {
            return Err(ContentError::Bundle {
                detail: format!(
                    "mission '{}' reaches an area that already contains the starting {} \
                     of faction '{}' at tick 0 (tile {}x{}): a reach objective must \
                     demand movement",
                    mission.id,
                    force.entity,
                    faction.id,
                    start.x + force.offset.0,
                    start.y + force.offset.1
                ),
            });
        }
    }
    Ok(())
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
        // The kind tag: fixed, ordered, exhaustive over the vocabulary.
        // New kinds append (B-017's Survive is 3, Protect is 4; B-018's
        // ReachArea is 5, Gather is 6), so every pre-existing mission's
        // byte stream is verbatim — the mission identity rule.
        h.write_u8(match objective.kind {
            defs::ObjectiveKind::Destroy => 1,
            defs::ObjectiveKind::BuildTrain => 2,
            defs::ObjectiveKind::Survive => 3,
            defs::ObjectiveKind::Protect => 4,
            defs::ObjectiveKind::ReachArea => 5,
            defs::ObjectiveKind::Gather => 6,
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
            // B-017: the survive window's integer tick count, in the same
            // fixed little-endian discipline. The kind tag ahead of it is
            // the domain separation — no pre-existing objective's bytes can
            // misparse it, and a mission without a survive objective keeps
            // exactly the id it had before.
            defs::ObjectiveParameters::Survive { ticks } => {
                h.write_u32(*ticks);
            }
            // B-017: the protected kind's entity id, the same string
            // encoding BuildTrain's kind gets. The kind tag (4) is the
            // domain separation; the loader's reference check guarantees
            // the string names a real entity in the tree.
            defs::ObjectiveParameters::Protect { kind } => {
                encode_str(h, kind);
            }
            // B-018: the area — a shape tag (Rect 1, Circle 2) followed by
            // the shape's fields in the fixed little-endian discipline.
            // The kind tag (5) ahead of the block is the domain
            // separation, and the shape tag delimits the two forms so
            // neither's bytes can misparse the other's; a mission without
            // a reach objective keeps exactly the id it had before.
            defs::ObjectiveParameters::ReachArea { area } => match *area {
                defs::AreaSpec::Rect { x, y, w, h: height } => {
                    h.write_u8(1);
                    h.write_i32(x);
                    h.write_i32(y);
                    h.write_u32(w);
                    h.write_u32(height);
                }
                defs::AreaSpec::Circle { cx, cy, radius } => {
                    h.write_u8(2);
                    h.write_i32(cx);
                    h.write_i32(cy);
                    h.write_i32(radius);
                }
            },
            // B-018: the gather amount and a semantics tag (Cumulative 1,
            // Balance 2) in the fixed little-endian discipline, behind
            // kind tag 6 — the domain separation. The resource is ore by
            // definition and rides the ruleset reference the encoding
            // already carries, so only the counting choice is
            // identity-bearing; a mission without a gather objective
            // keeps exactly the id it had before.
            defs::ObjectiveParameters::Gather { amount, semantics } => {
                h.write_i64(*amount);
                h.write_u8(match semantics {
                    defs::GatherSemantics::Cumulative => 1,
                    defs::GatherSemantics::Balance => 2,
                });
            }
        }
        encode_bool(h, objective.optional);
    }
    // The fog preset (B-015) enters the encoding ONLY when non-default and
    // only behind its own domain tag: a mission that does not use the field
    // keeps exactly the id it had before (the mission identity rule), and
    // the tag delimits the bytes so a future tail field can never misread
    // them. Default writes nothing — the historical byte stream, verbatim.
    if mission.fog_preset != defs::FogPreset::Default {
        encode_bool(h, true);
        h.write_u8(FOG_PRESET_ENCODING_TAG);
        h.write_u8(match mission.fog_preset {
            defs::FogPreset::Explored => 1,
            defs::FogPreset::Visible => 2,
            defs::FogPreset::Default => 0,
        });
    }
}

/// The domain tag of the optional fog-preset tail (B-015): a fixed byte
/// between the presence flag and the preset value, so the non-default tail
/// is self-describing and future mission fields appended after it can never
/// collide with or misparse these bytes.
const FOG_PRESET_ENCODING_TAG: u8 = 0xF0;

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
    fn world_resolves_the_faction_roster_through_the_entity_order() {
        // B-016: the build roster flows to the simulation through the world
        // seam — the production lists' exact pattern. The factory's faction
        // fields base + worker (authored order); the entities sort to
        // base = 0, ore_node = 1, worker = 2, so the authored roster
        // resolves to [0, 2].
        let tree = ContentTree::load(&factory()).expect("the factory loads");
        let bundle = tree.bundle("testing").expect("bundle");
        let world = bundle.world();
        assert_eq!(
            world.roster,
            vec![
                pandemonium_sim_api::KindId(0),
                pandemonium_sim_api::KindId(2)
            ],
            "the roster is the faction's authored list in entity-order ids"
        );
        // Membership is the faction's decision, not "every structure kind":
        // the ore node carries a Footprint, but the faction does not field
        // it — so it is not in the roster (the Build gate's future refusal).
        let ore_node_kind = bundle
            .entities
            .iter()
            .position(|entity| entity.id == "ore_node")
            .expect("the factory registers the ore node");
        assert!(!world
            .roster
            .contains(&pandemonium_sim_api::KindId(ore_node_kind as u32)));
        // The authored order survives the resolution: roster[i] is the
        // entity-order id of faction.roster[i].
        for (member, kind) in bundle.faction().roster.iter().zip(world.roster.iter()) {
            let expected = bundle
                .entities
                .iter()
                .position(|entity| entity.id == *member)
                .expect("validators reject a roster naming an unknown entity");
            assert_eq!(*kind, pandemonium_sim_api::KindId(expected as u32));
        }
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
    fn a_default_fog_preset_keeps_exactly_the_same_mission_id() {
        // The mission identity rule (B-015): a mission that does not use the
        // new field keeps exactly the same mission id as before. Absent,
        // explicit `Some("Default")`, and the pre-B-015 byte stream are the
        // same mission — the fog-preset tail enters the encoding only when
        // non-default, and Default writes nothing at all.
        let absent = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the absent-field mission loads");
        let explicit = MISSION.replace(
            "rules: \"alpha\",",
            "rules: \"alpha\",\n    fog_preset: Some(\"Default\"),",
        );
        let tree_explicit = ContentTree::load(&factory().mission("testing_01.ron", &explicit))
            .expect("the explicit-Default mission loads");
        assert_eq!(
            absent.mission_id("testing_01"),
            tree_explicit.mission_id("testing_01"),
            "explicit Default is the same mission as absent"
        );
        assert_eq!(
            absent.mission("testing_01").expect("found").fog_preset,
            defs::FogPreset::Default
        );
    }

    #[test]
    fn a_non_default_fog_preset_is_a_different_mission() {
        // A mission's fog preset is identity-bearing the moment it is used:
        // Explored and Visible change what every player sees, so two trees
        // differing only in a non-default preset are different missions
        // (and a Default-carrying mission is still the pre-B-015 mission).
        let absent = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the absent-field mission loads");
        let explored = MISSION.replace(
            "rules: \"alpha\",",
            "rules: \"alpha\",\n    fog_preset: Some(\"Explored\"),",
        );
        let tree_explored = ContentTree::load(&factory().mission("testing_01.ron", &explored))
            .expect("the Explored mission loads");
        let visible = MISSION.replace(
            "rules: \"alpha\",",
            "rules: \"alpha\",\n    fog_preset: Some(\"Visible\"),",
        );
        let tree_visible = ContentTree::load(&factory().mission("testing_01.ron", &visible))
            .expect("the Visible mission loads");
        assert_ne!(
            absent.mission_id("testing_01"),
            tree_explored.mission_id("testing_01"),
            "Explored is a different mission from Default"
        );
        assert_ne!(
            absent.mission_id("testing_01"),
            tree_visible.mission_id("testing_01"),
            "Visible is a different mission from Default"
        );
        assert_ne!(
            tree_explored.mission_id("testing_01"),
            tree_visible.mission_id("testing_01"),
            "Explored and Visible are different missions from each other"
        );
        assert_eq!(
            tree_explored
                .mission("testing_01")
                .expect("found")
                .fog_preset,
            defs::FogPreset::Explored
        );
        assert_eq!(
            tree_visible
                .mission("testing_01")
                .expect("found")
                .fog_preset,
            defs::FogPreset::Visible
        );
        // And the identity move is mission-scoped: the bundle identities the
        // goldens pin never move, whatever the missions carry.
        let plain_bundle = absent.bundle("testing").expect("bundle");
        let explored_bundle = tree_explored.bundle("testing").expect("bundle");
        assert_eq!(plain_bundle.content_hash(), explored_bundle.content_hash());
        assert_eq!(plain_bundle.map_id(), explored_bundle.map_id());
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
    fn survive_objectives_are_identity_bearing_and_domain_separated() {
        // B-017: the survive window enters the mission id — a mission whose
        // objectives carry a survive window is a different mission from the
        // same mission without one, and a different window is a different
        // mission. The kind tag (3, after Destroy's 1 and BuildTrain's 2)
        // keeps the byte streams apart, so the identity is domain-separated
        // by construction, never by hash accident.
        let base = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the BuildTrain-bearing mission loads");
        let survive = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Survive\",\n            parameters: Survive(( ticks: 300 )),\n            optional: true,",
        );
        let tree_survive = ContentTree::load(&factory().mission("testing_01.ron", &survive))
            .expect("the Survive-bearing mission loads");
        assert_ne!(
            base.mission_id("testing_01"),
            tree_survive.mission_id("testing_01"),
            "replacing an objective with a survive objective changes the mission id"
        );
        let longer = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Survive\",\n            parameters: Survive(( ticks: 600 )),\n            optional: true,",
        );
        let tree_longer = ContentTree::load(&factory().mission("testing_01.ron", &longer))
            .expect("the longer survive window loads");
        assert_ne!(
            tree_survive.mission_id("testing_01"),
            tree_longer.mission_id("testing_01"),
            "a different survive window is a different mission"
        );
        // And the identity move is mission-scoped: the bundle identities the
        // goldens pin never move, whatever the missions carry.
        let base_bundle = base.bundle("testing").expect("bundle");
        let survive_bundle = tree_survive.bundle("testing").expect("bundle");
        assert_eq!(base_bundle.content_hash(), survive_bundle.content_hash());
        assert_eq!(base_bundle.map_id(), survive_bundle.map_id());
    }

    #[test]
    fn mission_data_resolves_survive_verbatim_through_the_seam() {
        // B-017: survive parameters are plain data — the integer tick count
        // crosses the content seam verbatim (no kind to resolve, no unit to
        // convert), and the kind spelling resolves to the sim's vocabulary.
        let survive = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Survive\",\n            parameters: Survive(( ticks: 300 )),\n            optional: true,",
        );
        let tree = ContentTree::load(&factory().mission("testing_01.ron", &survive))
            .expect("the Survive-bearing mission loads");
        let objectives = tree
            .mission_data("testing_01", PlayerId(0))
            .expect("the objective set resolves");
        assert_eq!(
            objectives.objectives[1],
            MissionObjective {
                kind: MissionObjectiveKind::Survive,
                parameters: MissionObjectiveParameters::Survive { ticks: 300 },
                optional: true,
            }
        );
    }

    #[test]
    fn protect_objectives_are_identity_bearing_and_domain_separated() {
        // B-017: the protected kind enters the mission id — a mission whose
        // objectives carry a protect objective is a different mission from
        // the same mission without one, and a different protected kind is a
        // different mission. The kind tag (4, after Destroy's 1,
        // BuildTrain's 2, and Survive's 3) keeps the byte streams apart.
        let base = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the BuildTrain-bearing mission loads");
        let protect_worker = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Protect\",\n            parameters: Protect(( kind: \"worker\" )),\n            optional: true,",
        );
        let tree_worker = ContentTree::load(&factory().mission("testing_01.ron", &protect_worker))
            .expect("the Protect-bearing mission loads");
        assert_ne!(
            base.mission_id("testing_01"),
            tree_worker.mission_id("testing_01"),
            "replacing an objective with a protect objective changes the mission id"
        );
        let protect_base = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Protect\",\n            parameters: Protect(( kind: \"base\" )),\n            optional: true,",
        );
        let tree_base = ContentTree::load(&factory().mission("testing_01.ron", &protect_base))
            .expect("the other protected kind loads");
        assert_ne!(
            tree_worker.mission_id("testing_01"),
            tree_base.mission_id("testing_01"),
            "a different protected kind is a different mission"
        );
        // And the identity move is mission-scoped: the bundle identities the
        // goldens pin never move, whatever the missions carry.
        let base_bundle = base.bundle("testing").expect("bundle");
        let protect_bundle = tree_worker.bundle("testing").expect("bundle");
        assert_eq!(base_bundle.content_hash(), protect_bundle.content_hash());
        assert_eq!(base_bundle.map_id(), protect_bundle.map_id());
    }

    #[test]
    fn the_loader_rejects_a_protect_kind_the_starting_forces_do_not_carry() {
        // B-017's vacuous-failure guard (AUD-003): the factory's ore_node is
        // a real entity in the tree (the reference check passes), but the
        // testers faction does not start with one — protecting it would fail
        // on the first tick, so the tree assembly refuses the mission
        // loudly, naming the faction and its actual starting forces.
        let protect_node = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Protect\",\n            parameters: Protect(( kind: \"ore_node\" )),\n            optional: true,",
        );
        let err =
            ContentTree::load(&factory().mission("testing_01.ron", &protect_node)).unwrap_err();
        assert!(
            err.to_string().contains(
                "mission 'testing_01' protects kind 'ore_node' that the starting forces of \
                 faction 'testers' do not carry"
            ) && err
                .to_string()
                .contains("starting forces: base, worker, worker"),
            "got: {err}"
        );
    }

    #[test]
    fn mission_data_resolves_protect_through_the_entity_order() {
        // B-017: the protected kind resolves through the tree's entity order
        // — worker is KindId(2), the same id the world's entities carry —
        // and the loader's starting-forces gate guarantees the faction
        // actually fields the kind at tick 0.
        let protect = MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            "kind: \"Protect\",\n            parameters: Protect(( kind: \"worker\" )),\n            optional: true,",
        );
        let tree = ContentTree::load(&factory().mission("testing_01.ron", &protect))
            .expect("the Protect-bearing mission loads");
        let objectives = tree
            .mission_data("testing_01", PlayerId(0))
            .expect("the objective set resolves");
        assert_eq!(
            objectives.objectives[1],
            MissionObjective {
                kind: MissionObjectiveKind::Protect,
                parameters: MissionObjectiveParameters::Protect { kind: KindId(2) },
                optional: true,
            }
        );
    }

    /// A probe mission whose second objective is a ReachArea objective with
    /// the given area text (the first stays the factory's Destroy).
    fn reach_mission(area: &str) -> String {
        MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            &format!(
                "kind: \"ReachArea\",\n            parameters: ReachArea(( area: {area} )),\n            optional: true,"
            ),
        )
    }

    #[test]
    fn reach_areas_that_leave_the_map_are_refused() {
        // B-018's bounds gate, tree-level (it needs the mission's map): a
        // rect whose span exits the 16x16 grid and a circle whose disk
        // exits it — each refused loudly with the area's numbers and the
        // map's dimensions.
        for (area, needle) in [
            (
                "Rect(( x: 10, y: 10, w: 8, h: 4 ))",
                "reaches a rect [x 10, y 10, w 8, h 4] that leaves the map 'testing' (16x16 tiles)",
            ),
            (
                "Rect(( x: -2, y: 12, w: 8, h: 4 ))",
                "reaches a rect [x -2, y 12, w 8, h 4] that leaves the map 'testing'",
            ),
            (
                "Circle(( cx: 15000, cy: 8000, radius: 2000 ))",
                "reaches a circle (center 15000x8000 milli-tiles, radius 2000) that leaves \
                 the map 'testing'",
            ),
            (
                "Circle(( cx: 8000, cy: 3000, radius: 4000 ))",
                "reaches a circle (center 8000x3000 milli-tiles, radius 4000) that leaves \
                 the map 'testing'",
            ),
        ] {
            let err = ContentTree::load(&factory().mission("testing_01.ron", &reach_mission(area)))
                .unwrap_err();
            assert!(
                err.to_string().contains(needle),
                "area {area} must be refused, got: {err}"
            );
        }
    }

    #[test]
    fn reach_areas_that_cover_the_whole_map_are_refused() {
        // B-018's coverage gate: the exact grid match and any overspan of
        // it, plus a circle containing all four corners. Coverage is
        // checked BEFORE bounds deliberately — the overspan rect and the
        // covering circle both leave the map too, and the semantic error
        // ("every position is already inside") is the author's actual
        // mistake, so it is the one reported.
        for (area, needle) in [
            (
                "Rect(( x: 0, y: 0, w: 16, h: 16 ))",
                "reaches a rect [x 0, y 0, w 16, h 16] covering the whole map 'testing'",
            ),
            (
                "Rect(( x: -2, y: -2, w: 20, h: 20 ))",
                "reaches a rect [x -2, y -2, w 20, h 20] covering the whole map 'testing'",
            ),
            // The diagonal of the 16x16 map is ~22627 milli-tiles; a
            // 22628-radius circle at the center contains all four corners.
            (
                "Circle(( cx: 8000, cy: 8000, radius: 22628 ))",
                "containing all four corners of the map 'testing'",
            ),
        ] {
            let err = ContentTree::load(&factory().mission("testing_01.ron", &reach_mission(area)))
                .unwrap_err();
            assert!(
                err.to_string().contains(needle),
                "area {area} must be refused, got: {err}"
            );
        }
    }

    #[test]
    fn reach_areas_that_already_hold_a_starting_entity_are_refused() {
        // B-018's tick-0 gate (AUD-003): the factory's first start (3, 3)
        // fields a 2x2 base centered at tile (4, 4) — tile_center(3, 3,
        // 2, 2) = ((3*2+2)*500, ...) milli = tile 4.0 — and workers at
        // tiles (2, 6) and (4, 6) (centers (2.5, 6.5) and (4.5, 6.5)). An
        // area that already contains any of them completes the objective
        // on the first evaluated tick without a single order — refused
        // loudly, naming the force and its tile.
        for (area, needle) in [
            (
                // [3,5) x [3,5) holds the base's center (4.0, 4.0).
                "Rect(( x: 3, y: 3, w: 2, h: 2 ))",
                "already contains the starting base of faction 'testers' at tick 0 (tile 3x3)",
            ),
            (
                // [2,3) x [6,7) holds worker A's center (2.5, 6.5).
                "Rect(( x: 2, y: 6, w: 1, h: 1 ))",
                "already contains the starting worker of faction 'testers' at tick 0 (tile 2x6)",
            ),
            (
                // A circle on the base's center.
                "Circle(( cx: 4000, cy: 4000, radius: 500 ))",
                "already contains the starting base of faction 'testers' at tick 0 (tile 3x3)",
            ),
        ] {
            let err = ContentTree::load(&factory().mission("testing_01.ron", &reach_mission(area)))
                .unwrap_err();
            assert!(
                err.to_string()
                    .contains("a reach objective must demand movement")
                    && err.to_string().contains(needle),
                "area {area} must be refused, got: {err}"
            );
        }
    }

    #[test]
    fn a_reach_area_beside_the_opponents_start_loads() {
        // The gate's scope is the MISSION PLAYER's starting forces (the
        // first start's convention slot), not every start: an area around
        // the opponent's base — [12, 15) x [12, 15) — holds none of player
        // 0's forces at tick 0, so it loads: "reach the enemy base" is a
        // real objective, even though the opponent stands inside it from
        // tick 0. The area crosses the seam verbatim (the sim mirror).
        for area in [
            "Rect(( x: 12, y: 12, w: 3, h: 3 ))",
            // Centered on player 1's base center (tile 11, 11).
            "Circle(( cx: 11000, cy: 11000, radius: 1500 ))",
        ] {
            let tree =
                ContentTree::load(&factory().mission("testing_01.ron", &reach_mission(area)))
                    .expect("an area beside the opponent's start loads");
            let objectives = tree
                .mission_data("testing_01", PlayerId(0))
                .expect("the objective set resolves");
            let expected_area = match area {
                "Rect(( x: 12, y: 12, w: 3, h: 3 ))" => AreaSpec::Rect {
                    x: 12,
                    y: 12,
                    w: 3,
                    h: 3,
                },
                _ => AreaSpec::Circle {
                    cx: 11000,
                    cy: 11000,
                    radius: 1500,
                },
            };
            assert_eq!(
                objectives.objectives[1],
                MissionObjective {
                    kind: MissionObjectiveKind::ReachArea,
                    parameters: MissionObjectiveParameters::ReachArea {
                        area: expected_area
                    },
                    optional: true,
                },
                "area {area} crosses the seam verbatim"
            );
        }
    }

    /// A probe mission whose second objective is a Gather objective with
    /// the given amount and semantics (the first stays the factory's
    /// Destroy).
    fn gather_mission(amount: i64, semantics: &str) -> String {
        MISSION.replace(
            "kind: \"BuildTrain\",\n            parameters: BuildTrain(( kind: \"worker\", count: 4 )),\n            optional: true,",
            &format!(
                "kind: \"Gather\",\n            parameters: Gather(( amount: {amount}, semantics: \"{semantics}\" )),\n            optional: true,"
            ),
        )
    }

    #[test]
    fn gather_amounts_at_or_below_the_starting_ore_are_refused_for_both_semantics() {
        // B-018's tree-level gather gate (AUD-003): the factory's rules
        // start every player at 200 ore, so a gather objective demanding
        // 200 or less would complete at (or before) the first delivery for
        // BOTH semantics — the balance starts there, the cumulative total
        // crosses it with the first trip. Refused loudly, naming the
        // amount, the ruleset's starting value, and both semantics; 201 is
        // the first legal amount.
        for (amount, semantics) in [
            (200i64, "Cumulative"),
            (200, "Balance"),
            (199, "Cumulative"),
            (0, "Balance"),
        ] {
            let err = ContentTree::load(
                &factory().mission("testing_01.ron", &gather_mission(amount, semantics)),
            )
            .unwrap_err();
            assert!(
                err.to_string().contains(&format!(
                    "mission 'testing_01' gathers {amount} ore, at or below the starting ore \
                     200 of ruleset 'alpha' (both semantics)"
                )),
                "amount {amount} ({semantics}) must be refused, got: {err}"
            );
        }
        // The first legal amount loads, both semantics.
        for semantics in ["Cumulative", "Balance"] {
            ContentTree::load(
                &factory().mission("testing_01.ron", &gather_mission(201, semantics)),
            )
            .expect("an amount above the starting ore loads");
        }
    }

    #[test]
    fn a_ruleset_without_ore_refuses_any_gather_objective() {
        // The resource is ore by the objective's definition (the Alpha's
        // single registered resource): a ruleset that registers something
        // else cannot carry a gather objective at all — loud, naming what
        // it does register. The factory's entity costs also drop to zero
        // so the entity-cost reference law (which fires first, exactly as
        // it should for a real tree) stays out of the probe's way and the
        // gather gate's own message is the one under test.
        let no_ore_rules = RULES.replace(
            "( id: \"ore\", display_name: \"Ore\", starting: 200 )",
            "( id: \"timber\", display_name: \"Timber\", starting: 100 )",
        );
        let free_base = BASE.replace("cost: ( ore: 100 ),", "cost: ( ore: 0 ),");
        let free_worker = WORKER.replace("cost: ( ore: 50 ),", "cost: ( ore: 0 ),");
        let timber_node = ORE_NODE.replace(
            "Resource(( resource: \"ore\", amount: 1500 ))",
            "Resource(( resource: \"timber\", amount: 1500 ))",
        );
        let sources = factory()
            .rules("alpha.ron", &no_ore_rules)
            .entity("base.ron", &free_base)
            .entity("worker.ron", &free_worker)
            .entity("ore_node.ron", &timber_node)
            .mission("testing_01.ron", &gather_mission(400, "Cumulative"));
        let err = ContentTree::load(&sources).unwrap_err();
        assert!(
            err.to_string().contains(
                "mission 'testing_01' gathers ore but ruleset 'alpha' registers no 'ore' \
                 resource (registered: timber)"
            ),
            "got: {err}"
        );
    }

    #[test]
    fn mission_data_resolves_the_gather_resource_through_the_rules_registry() {
        // The seam: the objective's ResourceId is the registry's ore index
        // — the exact id `world()` assigns the ledger (0 in the factory) —
        // and the semantics converts mirror-to-sim, variant for variant.
        for (semantics, expected) in [
            (
                "Cumulative",
                pandemonium_sim_api::GatherSemantics::Cumulative,
            ),
            ("Balance", pandemonium_sim_api::GatherSemantics::Balance),
        ] {
            let tree = ContentTree::load(
                &factory().mission("testing_01.ron", &gather_mission(400, semantics)),
            )
            .expect("the gather-bearing mission loads");
            let objectives = tree
                .mission_data("testing_01", PlayerId(0))
                .expect("the objective set resolves");
            assert_eq!(
                objectives.objectives[1],
                MissionObjective {
                    kind: MissionObjectiveKind::Gather,
                    parameters: MissionObjectiveParameters::Gather {
                        resource: ResourceId(0),
                        amount: 400,
                        semantics: expected,
                    },
                    optional: true,
                },
                "semantics {semantics}"
            );
            // The world's fixture registry assigns ore the same id — the
            // id pair the evaluator reads the ledger by.
            let world = tree.bundle("testing").expect("bundle").world();
            assert_eq!(
                world.resources.first().map(|def| def.resource),
                Some(ResourceId(0)),
                "the ledger's first resource is the objective's resource id"
            );
        }
    }

    #[test]
    fn mission_data_resolve_kind_ids_through_the_tree_entity_order() {
        // The content seam (ADR-0201 §2a): string kind ids become KindIds in
        // the tree's entity order — base sorts before ore_node before worker
        // (id order), so worker is KindId(2) — the same index the world's
        // kinds vector assigns.
        let tree = ContentTree::load(&factory().mission("testing_01.ron", MISSION))
            .expect("the mission-bearing tree loads");
        let objectives = tree
            .mission_data("testing_01", PlayerId(0))
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
            .mission_data("testing_01", PlayerId(0))
            .expect("resolves");
        assert_eq!(
            objectives.objectives[0].parameters,
            MissionObjectiveParameters::Destroy {
                target_kind: Some(KindId(0)), // "base" sorts first
            }
        );
        // Unknown missions are refused loudly.
        let err = tree.mission_data("missing", PlayerId(0)).unwrap_err();
        assert!(
            err.to_string().contains("unknown mission 'missing'"),
            "got: {err}"
        );
    }
}
