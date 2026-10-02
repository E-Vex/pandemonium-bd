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

use pandemonium_fx::Fnv1a64;
use pandemonium_sim::TrivialWorld;
use pandemonium_sim::{
    CapTemplate, KindEconomy, KindTemplate, ResourceDef as SimResourceDef, SpawnDef,
};
use pandemonium_sim_api::{PlayerId, ResourceId, Vec2Fx};

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

/// Every validated file of a content directory. Maps, factions, and entities are
/// plural (a directory may carry several); the ruleset is singular (the resource
/// registry is global).
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
            let mut h = Fnv1a64::new();
            h.write_u32(CONTENT_ENCODING_VERSION);
            encode_rules(&mut h, &rules);
            encode_entities(&mut h, &entities);
            encode_factions(&mut h, &factions);
            encode_map(&mut h, &map);
            h.finish()
        };
        let map_hash = {
            let mut h = Fnv1a64::new();
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
    /// little-endian order through `fx::Fnv1a64`, never over `Debug` output or
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
    /// `Attack` remains validated-and-carried for combat's milestone (M6 — see
    /// docs/DEBT.md DEBT-006).
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
        // Attack arrives with combat (M6) — validated, carried, unmapped.
        defs::CapabilityDef::Attack { .. } => None,
    }
}

/// The map's terrain as the simulation's passability grid: one byte per tile,
/// row-major, 1 passable and 0 blocked (plan §9.1.1). The grid's shape is
/// already validated against the terrain classes (loader guarantees).
fn map_passability(map: &defs::MapDef) -> Vec<u8> {
    let class_of: BTreeMap<char, &defs::TerrainClass> = map
        .terrain
        .iter()
        .map(|class| (class.code, class))
        .collect();
    let mut passability = Vec::with_capacity((map.width as u64 * map.height as u64) as usize);
    for row in &map.grid {
        for code in row.chars() {
            let passable = class_of
                .get(&code)
                .map(|class| class.passable)
                .unwrap_or(false); // unknown codes never validate; fail closed
            passability.push(u8::from(passable));
        }
    }
    passability
}

/// The map's terrain as the simulation's buildability grid: one byte per tile,
/// row-major, 1 buildable and 0 not — the Build command's placement input
/// (plan §10.5). Mirrors [`map_passability`].
fn map_buildability(map: &defs::MapDef) -> Vec<u8> {
    let class_of: BTreeMap<char, &defs::TerrainClass> = map
        .terrain
        .iter()
        .map(|class| (class.code, class))
        .collect();
    let mut buildability = Vec::with_capacity((map.width as u64 * map.height as u64) as usize);
    for row in &map.grid {
        for code in row.chars() {
            let buildable = class_of
                .get(&code)
                .map(|class| class.buildable)
                .unwrap_or(false); // unknown codes never validate; fail closed
            buildability.push(u8::from(buildable));
        }
    }
    buildability
}

// ---------------------------------------------------------------------------
// Canonical encoding (the content hash) — plan §5.10: explicit little-endian
// byte encoding through fx::Fnv1a64, fixed field order, never Debug output.
// ---------------------------------------------------------------------------

fn encode_str(h: &mut Fnv1a64, text: &str) {
    h.write_u32(text.len() as u32);
    h.write_bytes(text.as_bytes());
}

fn encode_bool(h: &mut Fnv1a64, value: bool) {
    h.write_u8(u8::from(value));
}

fn encode_rules(h: &mut Fnv1a64, rules: &defs::RulesDef) {
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

fn encode_entities(h: &mut Fnv1a64, entities: &[defs::EntityDef]) {
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

fn encode_capability(h: &mut Fnv1a64, cap: &defs::CapabilityDef) {
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

fn encode_factions(h: &mut Fnv1a64, factions: &[defs::FactionDef]) {
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

fn encode_map(h: &mut Fnv1a64, map: &defs::MapDef) {
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
