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

use pandemonium_fx::{Fnv1a64, Fx};
use pandemonium_sim_api::{KindId, PlayerId, ResourceId, Tick, Vec2Fx};

use crate::world::{HealthDef, MoveDef, VisionDef};

/// The encoding version of the fixture hash, so an intentional change to the
/// canonical encoding shows up as a golden-hash change in review rather than a
/// silent collision.
const FIXTURE_ENCODING_VERSION: u32 = 1;

/// A world template: kinds as capability compositions, a resource registry with
/// starting balances, and the spawns that bring it to life (plan §7.4's spirit
/// on a trivial scale).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrivialWorld {
    /// Opaque map identity, echoed into replays alongside the content hash.
    pub map_id: u64,
    /// Map width in tiles (advisory for M1: no collision layer exists yet).
    pub width_tiles: u32,
    /// Map height in tiles (advisory for M1).
    pub height_tiles: u32,
    /// Kind templates in [`KindId`] order — a kind's id is its index here.
    pub kinds: Vec<KindTemplate>,
    /// The resource registry; each player's ledger starts with these balances.
    pub resources: Vec<ResourceDef>,
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
    /// The canonical content identity of this world: a hash over every field in a
    /// fixed little-endian order. Replays record it so a replay can be checked
    /// against the world it was recorded on (plan §6.5 `content_hash`).
    pub fn content_hash(&self) -> u64 {
        let mut h = Fnv1a64::new();
        h.write_u32(FIXTURE_ENCODING_VERSION);
        h.write_u64(self.map_id);
        h.write_u32(self.width_tiles);
        h.write_u32(self.height_tiles);
        h.write_u32(self.kinds.len() as u32);
        for kind in &self.kinds {
            h.write_u32(kind.caps.len() as u32);
            for cap in &kind.caps {
                cap.encode(&mut h);
            }
        }
        h.write_u32(self.resources.len() as u32);
        for res in &self.resources {
            h.write_u32(res.resource.0);
            h.write_i64(res.starting);
        }
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

/// One kind as a composition of capability templates (plan §7.3/§7.4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KindTemplate {
    /// The capabilities an entity of this kind carries, in fixed order.
    pub caps: Vec<CapTemplate>,
}

/// A capability template in authoring units (plan §10.2). The simulator converts
/// these to the runtime [`crate::CapabilityData`] when spawning.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    },
    /// Perception radius for the fog-filtered [`crate::Sim::player_view`] and
    /// targeting visibility (plan §9.5).
    Vision {
        /// Sight radius in milli-tiles.
        radius_milli_tiles: i32,
    },
}

impl CapTemplate {
    /// Encodes one template into the fixture hash (fixed tag + fields).
    fn encode(self, h: &mut Fnv1a64) {
        match self {
            CapTemplate::Health {
                max_hp,
                regen_per_tick,
            } => {
                h.write_u8(1);
                h.write_i32(max_hp);
                h.write_i32(regen_per_tick);
            }
            CapTemplate::Move {
                speed_milli_tiles_per_s,
            } => {
                h.write_u8(2);
                h.write_i32(speed_milli_tiles_per_s);
            }
            CapTemplate::Vision { radius_milli_tiles } => {
                h.write_u8(3);
                h.write_i32(radius_milli_tiles);
            }
        }
    }

    /// Converts a template into runtime capability data (plan §10.2: authoring
    /// integers become fixed point once, at load). Negative speeds and radii are
    /// clamped to zero — data mistakes degrade, never panic.
    pub(crate) fn to_runtime(self, ticks_per_second: u32) -> crate::CapabilityData {
        match self {
            CapTemplate::Health {
                max_hp,
                regen_per_tick,
            } => crate::CapabilityData::Health(HealthDef {
                max_hp: max_hp.max(0),
                hp: max_hp.max(0),
                regen_per_tick,
            }),
            CapTemplate::Move {
                speed_milli_tiles_per_s,
            } => {
                let speed_milli = speed_milli_tiles_per_s.max(0);
                let per_second = Fx::from_milli(speed_milli);
                // Whole-tick conversion: per-second fixed point divided by the tick
                // rate, rounding toward zero (the documented Fx division contract).
                let per_tick = per_second.div(Fx::from_int(ticks_per_second as i32));
                crate::CapabilityData::Move(MoveDef {
                    speed_per_tick: per_tick,
                })
            }
            CapTemplate::Vision { radius_milli_tiles } => {
                crate::CapabilityData::Vision(VisionDef {
                    radius: Fx::from_milli(radius_milli_tiles.max(0)),
                })
            }
        }
    }
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
    fn encode(self, h: &mut Fnv1a64) {
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
            kinds: vec![
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 40,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Move {
                            speed_milli_tiles_per_s: 2600,
                        },
                        CapTemplate::Vision {
                            radius_milli_tiles: 7000,
                        },
                    ],
                },
                KindTemplate {
                    caps: vec![CapTemplate::Vision {
                        radius_milli_tiles: 9000,
                    }],
                },
            ],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            initial_spawns: vec![SpawnDef {
                owner: PlayerId(0),
                kind: KindId(0),
                pos: Vec2Fx::from_ints(10, 10),
            }],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    #[test]
    fn fixture_hash_is_stable_for_equal_worlds() {
        assert_eq!(demo_world().content_hash(), demo_world().content_hash());
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
        }
        .to_runtime(30) else {
            panic!("expected move capability");
        };
        assert_eq!(m.speed_per_tick, Fx::ZERO);
    }
}
