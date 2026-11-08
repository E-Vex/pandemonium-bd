//! The milestone-M1 *trivial world* fixture (plan §14 M1; handoff §8 guidance).
//!
//! `TrivialWorld` is the in-code stand-in for the M2 `ContentBundle`: a tiny,
//! fully deterministic world description the simulator and its acceptance tests
//! can construct without touching the content pipeline. It deliberately stays in
//! this crate so that M1 does not jump ahead into RON loading; when M2 lands,
//! real matches are built from loaded content and this type remains the minimal
//! fixture for spine tests (determinism, ids, iteration order).
//!
//! Authoring units follow plan §10.2 (integers only): speeds in milli-tiles per
//! second, radii in milli-tiles, durations implicitly in ticks; the simulator
//! converts them to fixed point once, at world construction.

use pandemonium_fx::{Fx, XxHash64};
use pandemonium_sim_api::{KindId, PlayerId, ResourceId, Tick, Vec2Fx};

use crate::world::{BuildDef, GatherDef, PopulationDef, ProduceDef, StorageDef, VisionDef};

/// The encoding version of the fixture hash, so an intentional change to the
/// canonical encoding shows up as a golden-hash change in review rather than a
/// silent collision.
const FIXTURE_ENCODING_VERSION: u32 = 4;

/// A world template: kinds as capability compositions (with their economy
/// stats), a resource registry with starting balances, the production lists,
/// the terrain grids, and the spawns that bring it to life (plan §7.4's spirit
/// on a trivial scale).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrivialWorld {
    /// Opaque map identity, echoed into replays alongside the content hash.
    pub map_id: u64,
    /// Map width in tiles.
    pub width_tiles: u32,
    /// Map height in tiles.
    pub height_tiles: u32,
    /// Terrain passability, row-major, one byte per tile (`width * height`
    /// entries; 1 passable, 0 blocked) — the M4 navigation layer's input
    /// (plan §9.1.1, from the map's terrain classes since M2).
    pub passability: Vec<u8>,
    /// Terrain buildability, row-major, one byte per tile (1 buildable,
    /// 0 not) — the Build command's placement input (plan §10.5: the map's
    /// buildable flags as data; M5).
    pub buildability: Vec<u8>,
    /// Kind templates in [`KindId`] order — a kind's id is its index here.
    pub kinds: Vec<KindTemplate>,
    /// The resource registry; each player's ledger starts with these balances.
    pub resources: Vec<ResourceDef>,
    /// Production lists — producer kind id → trainable kind ids, in authored
    /// (faction) order. Faction data flows to the simulation through this
    /// seam (plan §10.4: which kinds a producer trains is faction data; the
    /// Produce capability itself stays parameterless). M5.
    pub production: Vec<(KindId, Vec<KindId>)>,
    /// The build roster: the structure kinds this world's faction fields —
    /// the Build gate's authority (B-016, ADR-0201 §3's closed asymmetry:
    /// Train has always been gated by the faction's production lists; Build
    /// is now gated by the faction's roster). Faction data flows to the
    /// simulation through this seam exactly like `production` (FD-4:
    /// behaviour is data — a restricted roster is a data decision, never a
    /// code path). Deliberately NOT part of [`Self::content_hash`]: the
    /// fixture hash stays the world's content identity, and a restricted
    /// roster's identity rides the mission id on recordings that carry one
    /// (AUD-002), never a silent hash change.
    pub roster: Vec<KindId>,
    /// The population cap before structures add to it (plan §10.4: 0).
    pub base_population_cap: u32,
    /// Entities present when the match begins, spawned in this order.
    pub initial_spawns: Vec<SpawnDef>,
    /// Entities that enter mid-match at scheduled ticks (the M1 stand-in for
    /// production spawning; plan §6.3 stage 3).
    pub scheduled_spawns: Vec<ScheduledSpawnDef>,
    /// Uniform spawn-position jitter radius in milli-tiles, applied with the
    /// simulation RNG in fixed spawn order. Zero disables it. This exercises the
    /// seeded RNG inside the hashed state from day one (plan §5.4).
    pub spawn_jitter_milli: i32,
}

impl TrivialWorld {
    /// The hand-built-fixture roster default: every structure kind in
    /// `kinds`, in kind order (a kind is a structure when it carries a
    /// Footprint). Content-loaded worlds never use this —
    /// `ContentBundle::world` fills the roster from the faction's own list
    /// (the production lists' exact pattern).
    pub fn default_roster(&self) -> Vec<KindId> {
        self.kinds
            .iter()
            .enumerate()
            .filter(|(_, kind)| {
                kind.caps
                    .iter()
                    .any(|cap| matches!(cap, CapTemplate::Footprint { .. }))
            })
            .map(|(index, _)| KindId(index as u32))
            .collect()
    }

    /// The hand-built constructor's closing step: defaults the roster to
    /// every structure kind in `kinds` ([`Self::default_roster`]), so a
    /// hand-built world builds exactly what it always built — existing
    /// fixtures behave identically under the roster-gated Build command
    /// (B-016).
    pub fn with_default_roster(mut self) -> Self {
        self.roster = self.default_roster();
        self
    }

    /// The explicit roster override: replaces the roster with `roster` — the
    /// restricted-faction shape (gate tests today; mission-file variants
    /// later, ADR-0201 §6: behaviour is data selecting code). Never hashed.
    pub fn with_roster(mut self, roster: Vec<KindId>) -> Self {
        self.roster = roster;
        self
    }

    /// A fully-passable `width` x `height` grid — the spine-test default for
    /// worlds that do not care about terrain.
    pub fn open_passability(width_tiles: u32, height_tiles: u32) -> Vec<u8> {
        vec![1; (width_tiles as u64 * height_tiles as u64) as usize]
    }

    /// A fully-buildable `width` x `height` grid — the spine-test default
    /// (paired with [`Self::open_passability`]).
    pub fn open_buildability(width_tiles: u32, height_tiles: u32) -> Vec<u8> {
        vec![1; (width_tiles as u64 * height_tiles as u64) as usize]
    }

    /// The canonical content identity of this world: a hash over every field in a
    /// fixed little-endian order. Replays record it so a replay can be checked
    /// against the world it was recorded on (plan §6.5 `content_hash`).
    pub fn content_hash(&self) -> u64 {
        let mut h = XxHash64::new();
        h.write_u32(FIXTURE_ENCODING_VERSION);
        h.write_u64(self.map_id);
        h.write_u32(self.width_tiles);
        h.write_u32(self.height_tiles);
        h.write_u32(self.passability.len() as u32);
        for tile in &self.passability {
            h.write_u8(*tile);
        }
        h.write_u32(self.buildability.len() as u32);
        for tile in &self.buildability {
            h.write_u8(*tile);
        }
        h.write_u32(self.kinds.len() as u32);
        for kind in &self.kinds {
            h.write_u32(kind.caps.len() as u32);
            for cap in &kind.caps {
                cap.encode(&mut h);
            }
            kind.economy.encode(&mut h);
        }
        h.write_u32(self.resources.len() as u32);
        for res in &self.resources {
            h.write_u32(res.resource.0);
            h.write_i64(res.starting);
        }
        h.write_u32(self.production.len() as u32);
        for (producer, producibles) in &self.production {
            h.write_u32(producer.0);
            h.write_u32(producibles.len() as u32);
            for producible in producibles {
                h.write_u32(producible.0);
            }
        }
        h.write_u32(self.base_population_cap);
        h.write_u32(self.initial_spawns.len() as u32);
        for spawn in &self.initial_spawns {
            spawn.encode(&mut h);
        }
        h.write_u32(self.scheduled_spawns.len() as u32);
        for scheduled in &self.scheduled_spawns {
            h.write_u32(scheduled.tick);
            scheduled.spawn.encode(&mut h);
        }
        h.write_i32(self.spawn_jitter_milli);
        h.finish()
    }
}

/// A kind's economy stats (plan §10.4): what it costs to build/train, how
/// long that takes, what population it occupies, and what must already
/// exist (one shared requirements checker consumes `requires` — plan §9.4).
/// Authored per resource, so a second resource is data-only (plan §9.3).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct KindEconomy {
    /// The cost, per resource, ascending by resource id. Empty = free.
    pub cost: Vec<(ResourceId, i64)>,
    /// Build (structure) or train (unit) time in whole ticks (plan §10.2
    /// conversion at load).
    pub build_time_ticks: u32,
    /// Population occupied when fielded (plan §10.4).
    pub population: i32,
    /// Kinds that must already exist (completed, owned by the same player)
    /// before this one can be produced or built — the requirements list.
    pub requires: Vec<KindId>,
}

impl KindEconomy {
    /// Encodes the economy block into the fixture hash.
    fn encode(&self, h: &mut XxHash64) {
        h.write_u32(self.cost.len() as u32);
        for (resource, amount) in &self.cost {
            h.write_u32(resource.0);
            h.write_i64(*amount);
        }
        h.write_u32(self.build_time_ticks);
        h.write_i32(self.population);
        h.write_u32(self.requires.len() as u32);
        for requirement in &self.requires {
            h.write_u32(requirement.0);
        }
    }
}

/// One kind as a composition of capability templates plus its economy stats
/// (plan §7.3/§7.4, §10.4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KindTemplate {
    /// The capabilities an entity of this kind carries, in fixed order.
    pub caps: Vec<CapTemplate>,
    /// What the kind costs to produce and what it requires (M5).
    pub economy: KindEconomy,
}

impl KindTemplate {
    /// A template with capabilities and a default (free, instant,
    /// requirement-free) economy — the shape spine tests use.
    pub fn from_caps(caps: Vec<CapTemplate>) -> Self {
        Self {
            caps,
            economy: KindEconomy::default(),
        }
    }
}

/// A capability template in authoring units (plan §10.2). The simulator converts
/// these to the runtime [`crate::CapabilityData`] when spawning.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CapTemplate {
    /// Health pool and per-tick regeneration (may be negative — the M1 death path
    /// that exercises death & cleanup and the never-reused id allocator).
    Health {
        /// Maximum hit points; current hp starts here at spawn.
        max_hp: i32,
        /// Hit points gained (or lost) every tick, clamped to `0..=max_hp`.
        regen_per_tick: i32,
    },
    /// Locomotion.
    Move {
        /// Top speed in milli-tiles per second (plan §10.2).
        speed_milli_tiles_per_s: i32,
        /// Collision radius in milli-tiles (plan §9.1.3: integer radii from
        /// data; the M4 collision layer consumes it).
        radius_milli_tiles: i32,
    },
    /// Perception radius for the fog-filtered [`crate::Sim::player_view`] and
    /// targeting visibility (plan §9.5).
    Vision {
        /// Sight radius in milli-tiles.
        radius_milli_tiles: i32,
    },
    /// Resource harvesting (plan §9.3, §10.4: 10 Ore per trip, 2000 ms per
    /// gather — per-worker data on the Gather capability).
    Gather {
        /// How much one full trip carries.
        carry_amount: i64,
        /// One gather cycle in milliseconds, as authored.
        gather_time_ms: u32,
    },
    /// Can construct structures (parameterless in the Alpha — plan §7.4).
    Build {},
    /// Owns a production queue (which kinds are trainable is world-level
    /// faction data — `TrivialWorld::production`; plan §10.4).
    Produce {},
    /// Accepts deposits of these resources (plan §7.4: Storage(ore)).
    Storage {
        /// Accepted resource ids, ascending.
        resources: Vec<ResourceId>,
    },
    /// Adds to the population cap while standing (plan §10.4).
    ProvidesPopulation {
        /// Cap added.
        amount: i32,
    },
    /// A finite resource node body (plan §9.3).
    Resource {
        /// Which resource this node holds.
        resource: ResourceId,
        /// Amount before depletion.
        amount: i64,
    },
    /// Static tile occupancy (plan §7.4: structures and nodes block tiles).
    Footprint {
        /// Footprint width in tiles.
        w: u32,
        /// Footprint height in tiles.
        h: u32,
    },
    /// Immediate-hit attack (plan §9.2, M6). Authored in milli-tiles + ms; the
    /// simulator converts them to fixed-point + ticks at load (plan §10.2).
    Attack {
        /// Damage per hit (integer).
        damage: i32,
        /// Attack range in milli-tiles (the hit-test radius).
        range_milli_tiles: i32,
        /// Cooldown between hits, in milliseconds.
        cooldown_ms: u32,
        /// Acquisition range in milli-tiles (auto-target scan radius).
        acquire_range_milli_tiles: i32,
    },
    /// Active castable abilities (ADR-0202, plan §19 Update 6): the kind's
    /// authored ability list. Authored in milli-tiles + ms; the simulator
    /// converts them to fixed-point + ticks at load (plan §10.2).
    Abilities {
        /// The ability specs, in authored slot order.
        abilities: Vec<AbilitySpecTemplate>,
    },
    /// Researchable upgrades (ADR-0203, plan §19 Update 7): the kind's
    /// authored upgrade list. Authored in ms + thousandths + resolved ids;
    /// the simulator converts durations to ticks at load (plan §10.2).
    Upgrades {
        /// The upgrade specs, in authored slot order.
        upgrades: Vec<UpgradeSpecTemplate>,
    },
}

/// One ability spec in authoring units (ADR-0202 §1) — the fixture-side form
/// of content's `AbilitySpecDef` (the content seam maps the two; hand-built
/// fixtures author these directly). The cost arrives already resource-
/// resolved (the `Storage`/`KindEconomy` pattern: names stop at the content
/// seam, ids flow into the simulation).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AbilitySpecTemplate {
    /// Stable ability id (unique within the kind; humans and tooling).
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// What the ability does (one of the code-defined effect kinds,
    /// parameterized).
    pub effect: AbilityEffectTemplate,
    /// Cast range in milli-tiles.
    pub cast_range_milli_tiles: i32,
    /// Cooldown between casts, in milliseconds.
    pub cooldown_ms: u32,
    /// The cast cost, resource-resolved (empty = free; the Alpha's Ore
    /// prices arrive as one pair from the content seam).
    pub cost: Vec<(ResourceId, i64)>,
}

/// One effect kind's parameters in authoring units (ADR-0202 §2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AbilityEffectTemplate {
    /// Immediate integer damage to every enemy-player entity in radius of the
    /// target point.
    DamageArea {
        /// Integer damage per hit entity.
        damage: i32,
        /// Effect radius in milli-tiles.
        radius_milli_tiles: i32,
    },
    /// Immediate integer healing to every friendly entity with Health in
    /// radius, clamped at max.
    HealArea {
        /// Integer healing per hit entity.
        amount: i32,
        /// Effect radius in milli-tiles.
        radius_milli_tiles: i32,
    },
    /// A timed movement-speed multiplier for every friendly entity with Move
    /// in radius (one boost per entity; a later cast overwrites).
    SpeedBurst {
        /// Speed multiplier in thousandths (1500 = 1.5×).
        multiplier_milli: i32,
        /// Boost duration in milliseconds.
        duration_ms: u32,
        /// Effect radius in milli-tiles.
        radius_milli_tiles: i32,
    },
}

/// One upgrade spec in authoring units (ADR-0203 §1) — the fixture-side form
/// of content's `UpgradeSpecDef`. Like the ability specs, the cost arrives
/// already resource-resolved and the prerequisite upgrade ids arrive already
/// resolved to (carrier kind, slot) pairs: names stop at the content seam,
/// ids flow into the simulation (the `KindEconomy.requires` pattern).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UpgradeSpecTemplate {
    /// Stable upgrade id (unique across the whole tree; humans, tooling, and
    /// prerequisite resolution read it).
    pub id: String,
    /// Human-facing name.
    pub display_name: String,
    /// What the upgrade does (one of the code-defined effect kinds,
    /// parameterized).
    pub effect: UpgradeEffectTemplate,
    /// Research duration in milliseconds.
    pub research_time_ms: u32,
    /// The research cost, resource-resolved (empty = free).
    pub cost: Vec<(ResourceId, i64)>,
    /// Prerequisites as resolved (carrier kind, slot) pairs.
    pub requires: Vec<(KindId, u8)>,
}

/// One upgrade effect kind's parameters in authoring units (ADR-0203 §2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UpgradeEffectTemplate {
    /// A permanent attack-damage multiplier for the owner's entities of the
    /// targeted kinds (read live by the combat system).
    AttackDamage {
        /// Damage multiplier in thousandths (1250 = 1.25×).
        multiplier_milli: i32,
        /// Which kinds benefit (resolved kind ids).
        targets: Vec<KindId>,
    },
    /// A permanent movement-speed multiplier for the owner's entities of the
    /// targeted kinds (read live by the movement system).
    MoveSpeed {
        /// Speed multiplier in thousandths (1150 = 1.15×).
        multiplier_milli: i32,
        /// Which kinds benefit (resolved kind ids).
        targets: Vec<KindId>,
    },
}

impl CapTemplate {
    /// Encodes one template into the fixture hash (fixed tag + fields).
    fn encode(&self, h: &mut XxHash64) {
        match self {
            CapTemplate::Health {
                max_hp,
                regen_per_tick,
            } => {
                h.write_u8(1);
                h.write_i32(*max_hp);
                h.write_i32(*regen_per_tick);
            }
            CapTemplate::Move {
                speed_milli_tiles_per_s,
                radius_milli_tiles,
            } => {
                h.write_u8(2);
                h.write_i32(*speed_milli_tiles_per_s);
                h.write_i32(*radius_milli_tiles);
            }
            CapTemplate::Vision { radius_milli_tiles } => {
                h.write_u8(3);
                h.write_i32(*radius_milli_tiles);
            }
            CapTemplate::Gather {
                carry_amount,
                gather_time_ms,
            } => {
                h.write_u8(4);
                h.write_i64(*carry_amount);
                h.write_u32(*gather_time_ms);
            }
            CapTemplate::Build {} => h.write_u8(5),
            CapTemplate::Produce {} => h.write_u8(6),
            CapTemplate::Storage { resources } => {
                h.write_u8(7);
                h.write_u32(resources.len() as u32);
                for resource in resources {
                    h.write_u32(resource.0);
                }
            }
            CapTemplate::ProvidesPopulation { amount } => {
                h.write_u8(8);
                h.write_i32(*amount);
            }
            CapTemplate::Resource { resource, amount } => {
                h.write_u8(9);
                h.write_u32(resource.0);
                h.write_i64(*amount);
            }
            CapTemplate::Footprint { w, h: height } => {
                h.write_u8(10);
                h.write_u32(*w);
                h.write_u32(*height);
            }
            CapTemplate::Attack {
                damage,
                range_milli_tiles,
                cooldown_ms,
                acquire_range_milli_tiles,
            } => {
                h.write_u8(11);
                h.write_i32(*damage);
                h.write_i32(*range_milli_tiles);
                h.write_u32(*cooldown_ms);
                h.write_i32(*acquire_range_milli_tiles);
            }
            // ADR-0202 (Update 6): tag 12, additive — existing fixture hashes
            // are unchanged (no existing template carries the block).
            CapTemplate::Abilities { abilities } => {
                h.write_u8(12);
                h.write_u32(abilities.len() as u32);
                for spec in abilities {
                    h.write_u32(spec.id.len() as u32);
                    h.write_bytes(spec.id.as_bytes());
                    h.write_u32(spec.display_name.len() as u32);
                    h.write_bytes(spec.display_name.as_bytes());
                    match &spec.effect {
                        AbilityEffectTemplate::DamageArea {
                            damage,
                            radius_milli_tiles,
                        } => {
                            h.write_u8(1);
                            h.write_i32(*damage);
                            h.write_i32(*radius_milli_tiles);
                        }
                        AbilityEffectTemplate::HealArea {
                            amount,
                            radius_milli_tiles,
                        } => {
                            h.write_u8(2);
                            h.write_i32(*amount);
                            h.write_i32(*radius_milli_tiles);
                        }
                        AbilityEffectTemplate::SpeedBurst {
                            multiplier_milli,
                            duration_ms,
                            radius_milli_tiles,
                        } => {
                            h.write_u8(3);
                            h.write_i32(*multiplier_milli);
                            h.write_u32(*duration_ms);
                            h.write_i32(*radius_milli_tiles);
                        }
                    }
                    h.write_i32(spec.cast_range_milli_tiles);
                    h.write_u32(spec.cooldown_ms);
                    h.write_u32(spec.cost.len() as u32);
                    for (resource, amount) in &spec.cost {
                        h.write_u32(resource.0);
                        h.write_i64(*amount);
                    }
                }
            }
            // ADR-0203 (Update 7): tag 13, additive — existing fixture
            // hashes are unchanged (no existing template carries the block;
            // the B-033 tag-12 precedent's rule).
            CapTemplate::Upgrades { upgrades } => {
                h.write_u8(13);
                h.write_u32(upgrades.len() as u32);
                for spec in upgrades {
                    h.write_u32(spec.id.len() as u32);
                    h.write_bytes(spec.id.as_bytes());
                    h.write_u32(spec.display_name.len() as u32);
                    h.write_bytes(spec.display_name.as_bytes());
                    match &spec.effect {
                        UpgradeEffectTemplate::AttackDamage {
                            multiplier_milli,
                            targets,
                        } => {
                            h.write_u8(1);
                            h.write_i32(*multiplier_milli);
                            h.write_u32(targets.len() as u32);
                            for target in targets {
                                h.write_u32(target.0);
                            }
                        }
                        UpgradeEffectTemplate::MoveSpeed {
                            multiplier_milli,
                            targets,
                        } => {
                            h.write_u8(2);
                            h.write_i32(*multiplier_milli);
                            h.write_u32(targets.len() as u32);
                            for target in targets {
                                h.write_u32(target.0);
                            }
                        }
                    }
                    h.write_u32(spec.research_time_ms);
                    h.write_u32(spec.cost.len() as u32);
                    for (resource, amount) in &spec.cost {
                        h.write_u32(resource.0);
                        h.write_i64(*amount);
                    }
                    h.write_u32(spec.requires.len() as u32);
                    for (carrier, slot) in &spec.requires {
                        h.write_u32(carrier.0);
                        h.write_u8(*slot);
                    }
                }
            }
        }
    }

    /// Converts a template into runtime capability data (plan §10.2: authoring
    /// integers become fixed point once, at load). Negative speeds and radii are
    /// clamped to zero — data mistakes degrade, never panic.
    pub(crate) fn to_runtime(&self, ticks_per_second: u32) -> crate::CapabilityData {
        match self {
            CapTemplate::Health {
                max_hp,
                regen_per_tick,
            } => crate::CapabilityData::Health(crate::world::HealthDef {
                max_hp: (*max_hp).max(0),
                hp: (*max_hp).max(0),
                regen_per_tick: *regen_per_tick,
            }),
            CapTemplate::Move {
                speed_milli_tiles_per_s,
                radius_milli_tiles,
            } => {
                let speed_milli = (*speed_milli_tiles_per_s).max(0);
                let per_second = Fx::from_milli(speed_milli);
                // Whole-tick conversion: per-second fixed point divided by the tick
                // rate, rounding toward zero (the documented Fx division contract).
                let per_tick = per_second.div(Fx::from_int(ticks_per_second as i32));
                crate::CapabilityData::Move(crate::world::MoveDef::new(
                    per_tick,
                    Fx::from_milli((*radius_milli_tiles).max(0)),
                ))
            }
            CapTemplate::Vision { radius_milli_tiles } => {
                crate::CapabilityData::Vision(VisionDef {
                    radius: Fx::from_milli((*radius_milli_tiles).max(0)),
                })
            }
            CapTemplate::Gather {
                carry_amount,
                gather_time_ms,
            } => crate::CapabilityData::Gather(GatherDef::new(
                *carry_amount,
                ms_to_ticks(*gather_time_ms, ticks_per_second),
            )),
            CapTemplate::Build {} => crate::CapabilityData::Build(BuildDef {}),
            CapTemplate::Produce {} => crate::CapabilityData::Produce(ProduceDef::new()),
            CapTemplate::Storage { resources } => crate::CapabilityData::Storage(StorageDef {
                resources: resources.clone(),
            }),
            CapTemplate::ProvidesPopulation { amount } => {
                crate::CapabilityData::ProvidesPopulation(PopulationDef { amount: *amount })
            }
            CapTemplate::Resource { resource, amount } => {
                crate::CapabilityData::Resource(crate::world::ResourceBodyDef {
                    resource: *resource,
                    amount: (*amount).max(0),
                })
            }
            CapTemplate::Footprint { w, h } => {
                crate::CapabilityData::Footprint(crate::world::FootprintDef {
                    w: (*w).max(1),
                    h: (*h).max(1),
                })
            }
            CapTemplate::Attack {
                damage,
                range_milli_tiles,
                cooldown_ms,
                acquire_range_milli_tiles,
            } => crate::CapabilityData::Attack(crate::world::AttackDef::new(
                *damage,
                Fx::from_milli((*range_milli_tiles).max(0)),
                ms_to_ticks(*cooldown_ms, ticks_per_second),
                Fx::from_milli((*acquire_range_milli_tiles).max(0)),
            )),
            // ADR-0202 (Update 6): the specs convert with the same
            // once-at-load rule; every slot starts ready.
            CapTemplate::Abilities { abilities } => {
                let specs = abilities
                    .iter()
                    .map(|spec| crate::world::AbilitySpec {
                        id: spec.id.clone(),
                        effect: match &spec.effect {
                            AbilityEffectTemplate::DamageArea {
                                damage,
                                radius_milli_tiles,
                            } => crate::world::AbilityEffect::DamageArea {
                                damage: *damage,
                                radius: Fx::from_milli((*radius_milli_tiles).max(0)),
                            },
                            AbilityEffectTemplate::HealArea {
                                amount,
                                radius_milli_tiles,
                            } => crate::world::AbilityEffect::HealArea {
                                amount: *amount,
                                radius: Fx::from_milli((*radius_milli_tiles).max(0)),
                            },
                            AbilityEffectTemplate::SpeedBurst {
                                multiplier_milli,
                                duration_ms,
                                radius_milli_tiles,
                            } => crate::world::AbilityEffect::SpeedBurst {
                                multiplier: Fx::from_milli((*multiplier_milli).max(0)),
                                duration_ticks: ms_to_ticks(*duration_ms, ticks_per_second),
                                radius: Fx::from_milli((*radius_milli_tiles).max(0)),
                            },
                        },
                        cast_range: Fx::from_milli(spec.cast_range_milli_tiles.max(0)),
                        cooldown_ticks: ms_to_ticks(spec.cooldown_ms, ticks_per_second),
                        cost: spec.cost.clone(),
                    })
                    .collect();
                crate::CapabilityData::Abilities(crate::world::AbilitiesDef::new(specs))
            }
            // ADR-0203 (Update 7): the specs convert with the same
            // once-at-load rule (ms to ticks, thousandths to fixed point);
            // the runtime block is NOT here — the Research command injects
            // it, the Construction pattern.
            CapTemplate::Upgrades { upgrades } => {
                let specs = upgrades
                    .iter()
                    .map(|spec| crate::world::UpgradeSpec {
                        id: spec.id.clone(),
                        effect: match &spec.effect {
                            UpgradeEffectTemplate::AttackDamage {
                                multiplier_milli,
                                targets,
                            } => crate::world::UpgradeEffect::AttackDamage {
                                multiplier: Fx::from_milli((*multiplier_milli).max(0)),
                                targets: targets.clone(),
                            },
                            UpgradeEffectTemplate::MoveSpeed {
                                multiplier_milli,
                                targets,
                            } => crate::world::UpgradeEffect::MoveSpeed {
                                multiplier: Fx::from_milli((*multiplier_milli).max(0)),
                                targets: targets.clone(),
                            },
                        },
                        research_ticks: ms_to_ticks(spec.research_time_ms, ticks_per_second),
                        cost: spec.cost.clone(),
                        requires: spec.requires.clone(),
                    })
                    .collect();
                crate::CapabilityData::Upgrades(crate::world::UpgradesDef { specs })
            }
        }
    }
}

/// The plan §10.2 duration conversion: milliseconds to whole ticks, rounding
/// up — a 2000 ms gather is 60 ticks exactly; 2001 ms is 61. (The same
/// conversion as the content loader's, so authored and fixture times agree.)
fn ms_to_ticks(ms: u32, ticks_per_second: u32) -> u32 {
    ((ms as u64 * ticks_per_second as u64).div_ceil(1000)) as u32
}

/// A registered resource and the starting balance every player receives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResourceDef {
    /// Which resource.
    pub resource: ResourceId,
    /// Starting balance.
    pub starting: i64,
}

/// One entity to spawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpawnDef {
    /// The owning slot (the neutral sentinel is allowed — unowned entities).
    pub owner: PlayerId,
    /// Which kind template to instantiate.
    pub kind: KindId,
    /// Spawn position in tile units.
    pub pos: Vec2Fx,
}

impl SpawnDef {
    /// Encodes one spawn into the fixture hash.
    fn encode(self, h: &mut XxHash64) {
        h.write_u8(self.owner.0);
        h.write_u32(self.kind.0);
        h.write_i32(self.pos.x.raw());
        h.write_i32(self.pos.y.raw());
    }
}

/// A spawn scheduled for a mid-match tick (the M1 stand-in for production
/// spawning, plan §6.3 stage 3; see also docs/ASSUMPTIONS.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduledSpawnDef {
    /// The tick the entity enters during (stage 3 of).
    pub tick: Tick,
    /// What spawns.
    pub spawn: SpawnDef,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_world() -> TrivialWorld {
        TrivialWorld {
            map_id: 7,
            width_tiles: 64,
            height_tiles: 64,
            passability: TrivialWorld::open_passability(64, 64),
            buildability: TrivialWorld::open_buildability(64, 64),
            kinds: vec![
                KindTemplate::from_caps(vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 350,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ]),
                KindTemplate::from_caps(vec![CapTemplate::Vision {
                    radius_milli_tiles: 9000,
                }]),
            ],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            roster: Vec::new(),
            base_population_cap: 0,
            initial_spawns: vec![SpawnDef {
                owner: PlayerId(0),
                kind: KindId(0),
                pos: Vec2Fx::from_ints(10, 10),
            }],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
        .with_default_roster()
    }

    #[test]
    fn fixture_hash_is_stable_for_equal_worlds() {
        assert_eq!(demo_world().content_hash(), demo_world().content_hash());
    }

    #[test]
    fn default_roster_lists_exactly_the_structure_kinds() {
        // The demo kinds (0 mover, 1 watcher) carry no Footprint: the default
        // roster is empty — nothing is a structure.
        assert!(demo_world().roster.is_empty());
        // One structure kind joins (kind 2): the default roster names it.
        let mut world = demo_world();
        world.kinds.push(KindTemplate {
            caps: vec![CapTemplate::Footprint { w: 2, h: 2 }],
            economy: KindEconomy::default(),
        });
        let rostered = world.with_default_roster();
        assert_eq!(rostered.roster, vec![KindId(2)]);
        // The explicit override replaces, never unions.
        let restricted = rostered.with_roster(vec![KindId(0)]);
        assert_eq!(restricted.roster, vec![KindId(0)]);
    }

    #[test]
    fn the_roster_is_never_hashed() {
        // Two worlds identical in every content-identity byte except the
        // roster hash the same: the roster is build authority, not content
        // identity (B-016 — the content hash comes from the bundle).
        let unrestricted = demo_world().with_default_roster();
        let restricted = demo_world().with_roster(vec![KindId(1)]);
        assert_ne!(unrestricted.roster, restricted.roster);
        assert_eq!(unrestricted.content_hash(), restricted.content_hash());
    }

    #[test]
    fn fixture_hash_separates_different_worlds() {
        let mut other = demo_world();
        other.map_id = 8;
        assert_ne!(demo_world().content_hash(), other.content_hash());
        let mut third = demo_world();
        third.spawn_jitter_milli = 1;
        assert_ne!(demo_world().content_hash(), third.content_hash());
    }

    #[test]
    fn speed_conversion_rounds_toward_zero_like_fx_division() {
        // 2600 milli-tiles/s -> Fx(2600/1000) then divided by 30 ticks/s.
        let crate::CapabilityData::Move(m) = CapTemplate::Move {
            speed_milli_tiles_per_s: 2600,
            radius_milli_tiles: 350,
        }
        .to_runtime(30) else {
            panic!("expected move capability");
        };
        let expected = Fx::from_milli(2600).div(Fx::from_int(30));
        assert_eq!(m.speed_per_tick, expected);
    }

    #[test]
    fn negative_template_values_clamp_to_zero() {
        let crate::CapabilityData::Vision(v) = CapTemplate::Vision {
            radius_milli_tiles: -5,
        }
        .to_runtime(30) else {
            panic!("expected vision capability");
        };
        assert_eq!(v.radius, Fx::ZERO);
        let crate::CapabilityData::Move(m) = CapTemplate::Move {
            speed_milli_tiles_per_s: -5,
            radius_milli_tiles: -5,
        }
        .to_runtime(30) else {
            panic!("expected move capability");
        };
        assert_eq!(m.speed_per_tick, Fx::ZERO);
    }

    #[test]
    fn duration_conversion_rounds_up_per_plan_10_2() {
        // (ms * 30 + 999) / 1000 — 2000 ms is exactly 60 ticks; 2001 is 61.
        assert_eq!(ms_to_ticks(2000, 30), 60);
        assert_eq!(ms_to_ticks(2001, 30), 61);
        assert_eq!(ms_to_ticks(0, 30), 0);
        let crate::CapabilityData::Gather(g) = CapTemplate::Gather {
            carry_amount: 10,
            gather_time_ms: 2000,
        }
        .to_runtime(30) else {
            panic!("expected gather capability");
        };
        assert_eq!(g.gather_time_ticks, 60);
        assert_eq!(g.carry_amount, 10);
        assert_eq!(g.cargo, None);
        assert_eq!(g.timer, 0);
    }

    #[test]
    fn economy_blocks_convert_with_defaults() {
        // The parameterless blocks convert to their runtime defaults.
        assert!(matches!(
            CapTemplate::Build {}.to_runtime(30),
            crate::CapabilityData::Build(_)
        ));
        let crate::CapabilityData::Produce(p) = CapTemplate::Produce {}.to_runtime(30) else {
            panic!("expected produce capability");
        };
        assert!(p.queue.is_empty());
        assert_eq!(p.rally, None);
        // Footprints clamp to at least one tile; resource bodies clamp amounts.
        let crate::CapabilityData::Footprint(f) =
            CapTemplate::Footprint { w: 0, h: 4 }.to_runtime(30)
        else {
            panic!("expected footprint capability");
        };
        assert_eq!((f.w, f.h), (1, 4));
        let crate::CapabilityData::Resource(r) = CapTemplate::Resource {
            resource: ResourceId(2),
            amount: -7,
        }
        .to_runtime(30) else {
            panic!("expected resource capability");
        };
        assert_eq!(r.amount, 0);
        assert_eq!(r.resource, ResourceId(2));
    }
}
