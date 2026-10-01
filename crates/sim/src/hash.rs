//! The canonical state hash (plan §6.4): an explicit little-endian byte encoding
//! of the simulation state, in a fixed field order, through `fx::Fnv1a64` — never
//! `Debug` output, never memory layout.
//!
//! What is encoded (the plan's list, minus state that does not exist yet):
//! the tick, the full RNG state, the id allocator, every entity in id order with
//! identity, position, facing, order queue and all capability data, and every
//! player's controller, resignation flag, resource ledger and population numbers.
//! Per-tile visibility bitsets join the encoding when fog state exists (M6, see
//! docs/ASSUMPTIONS.md). Events are outputs and never hashed.
//!
//! The encoding carries a version word: an intentional change to this function
//! shows up as a golden-hash change in review (tests/determinism.rs) rather than
//! as a silent value shift.

use pandemonium_fx::{Fnv1a64, Rng};
use pandemonium_sim_api::Tick;

use crate::world::{CapabilityData, Lifecycle, Order, World};

/// Version of the canonical state encoding. Bump (and regenerate the golden
/// hashes) whenever the encoded field set or order changes deliberately.
/// v2 adds the M4 movement fields: the Move collision radius, the remaining
/// path waypoints, and the stuck-detection counters.
pub(crate) const STATE_ENCODING_VERSION: u32 = 2;

/// Encodes the whole state into the hasher, in canonical order.
pub(crate) fn hash_state(world: &World, tick: Tick, rng: &Rng, next_entity_id: u64) -> u64 {
    let mut h = Fnv1a64::new();
    h.write_u32(STATE_ENCODING_VERSION);
    h.write_u32(tick);
    let (state, inc) = rng.state_parts();
    h.write_u64(state);
    h.write_u64(inc);
    h.write_u64(next_entity_id);

    // Entities, ascending by id (the store invariant).
    h.write_u32(world.entities.len() as u32);
    for entity in &world.entities {
        h.write_u64(entity.id.0);
        h.write_u8(entity.owner.0);
        h.write_u32(entity.kind.0);
        h.write_u8(lifecycle_tag(entity.lifecycle));
        h.write_i32(entity.pos.x.raw());
        h.write_i32(entity.pos.y.raw());
        h.write_i32(entity.facing.x.raw());
        h.write_i32(entity.facing.y.raw());
        h.write_u32(entity.orders.len() as u32);
        for order in &entity.orders {
            encode_order(&mut h, order);
        }
        // Capability presence bitmask over the fixed order (Health, Move, Vision),
        // then the present blocks in that same fixed order.
        let mask = capability_mask(world, entity.id);
        h.write_u8(mask);
        if let Some(CapabilityData::Health(def)) = capability_of(world, entity.id, mask, 0) {
            h.write_i32(def.hp);
            h.write_i32(def.max_hp);
            h.write_i32(def.regen_per_tick);
        }
        if let Some(CapabilityData::Move(def)) = capability_of(world, entity.id, mask, 1) {
            h.write_i32(def.speed_per_tick.raw());
            h.write_i32(def.radius.raw());
            h.write_u32(def.path.len() as u32);
            for waypoint in &def.path {
                h.write_i32(waypoint.x.raw());
                h.write_i32(waypoint.y.raw());
            }
            h.write_u32(def.stuck_ticks);
            h.write_u32(def.repaths);
        }
        if let Some(CapabilityData::Vision(def)) = capability_of(world, entity.id, mask, 2) {
            h.write_i32(def.radius.raw());
        }
    }

    // Players, ascending by slot.
    h.write_u32(world.players.len() as u32);
    for player in &world.players {
        h.write_u8(player.player.0);
        h.write_u8(controller_tag(player));
        h.write_u8(u8::from(player.resigned));
        h.write_u32(player.resources.len() as u32);
        for (resource, amount) in &player.resources {
            h.write_u32(resource.0);
            h.write_i64(*amount);
        }
        h.write_u32(player.population);
        h.write_u32(player.population_cap);
    }

    h.finish()
}

fn encode_order(h: &mut Fnv1a64, order: &Order) {
    match order {
        Order::MoveTo { target } => {
            h.write_u8(1);
            h.write_i32(target.x.raw());
            h.write_i32(target.y.raw());
        }
    }
}

fn lifecycle_tag(lifecycle: Lifecycle) -> u8 {
    match lifecycle {
        Lifecycle::Active => 1,
        Lifecycle::Dead => 2,
    }
}

fn controller_tag(player: &crate::world::PlayerState) -> u8 {
    match player.controller {
        pandemonium_sim_api::ControllerKind::Human => 0,
        pandemonium_sim_api::ControllerKind::Ai => 1,
    }
}

/// Presence bitmask over the fixed capability order: bit 0 Health, bit 1 Move,
/// bit 2 Vision.
fn capability_mask(world: &World, id: pandemonium_sim_api::EntityId) -> u8 {
    let mut mask = 0u8;
    if world.health_of(id).is_some() {
        mask |= 1 << 0;
    }
    if world.move_of(id).is_some() {
        mask |= 1 << 1;
    }
    if world.vision_of(id).is_some() {
        mask |= 1 << 2;
    }
    mask
}

/// Fetches the capability block for slot `index` (0 Health, 1 Move, 2 Vision) when
/// the mask says it is present — the mask and the lookups can only agree, because
/// both read the same stores.
fn capability_of(
    world: &World,
    id: pandemonium_sim_api::EntityId,
    mask: u8,
    index: u8,
) -> Option<CapabilityData> {
    if mask & (1 << index) == 0 {
        return None;
    }
    match index {
        0 => world.health_of(id).map(|def| CapabilityData::Health(*def)),
        1 => world
            .move_of(id)
            .map(|def| CapabilityData::Move(def.clone())),
        _ => world.vision_of(id).map(|def| CapabilityData::Vision(*def)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{HealthDef, MoveDef, PlayerState, VisionDef};
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::{
        ControllerKind, EntityId, KindId, PlayerId, PlayerSetup, ResourceId, Vec2Fx,
    };

    fn world_one_entity() -> (World, u64) {
        let mut world = World::new();
        world.kind_count = 1;
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(3, 4),
            vec![
                CapabilityData::Health(HealthDef {
                    max_hp: 10,
                    hp: 10,
                    regen_per_tick: 0,
                }),
                CapabilityData::Move(MoveDef::new(Fx::from_milli(50), Fx::from_milli(350))),
                CapabilityData::Vision(VisionDef {
                    radius: Fx::from_milli(7000),
                }),
            ],
        );
        (world, 2)
    }

    #[test]
    fn hash_is_stable_for_identical_state() {
        let (world, next) = world_one_entity();
        let rng = Rng::seeded(42);
        let a = hash_state(&world, 5, &rng, next);
        let (world2, next2) = world_one_entity();
        let rng2 = Rng::seeded(42);
        let b = hash_state(&world2, 5, &rng2, next2);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_separates_every_state_component() {
        let (world, next) = world_one_entity();
        let rng = Rng::seeded(42);
        let base = hash_state(&world, 5, &rng, next);

        // Tick.
        assert_ne!(base, hash_state(&world, 6, &rng, next));
        // RNG state.
        let other_rng = Rng::seeded(43);
        assert_ne!(base, hash_state(&world, 5, &other_rng, next));
        // Id allocator.
        assert_ne!(base, hash_state(&world, 5, &rng, next + 1));
        // Position.
        let mut moved = world.clone();
        moved.entity_mut(EntityId(1)).unwrap().pos = Vec2Fx::from_ints(3, 5);
        assert_ne!(base, hash_state(&moved, 5, &rng, next));
        // Order queue.
        let mut ordered = world.clone();
        ordered
            .entity_mut(EntityId(1))
            .unwrap()
            .orders
            .push(Order::MoveTo {
                target: Vec2Fx::from_ints(9, 9),
            });
        assert_ne!(base, hash_state(&ordered, 5, &rng, next));
        // Health.
        let mut hurt = world.clone();
        hurt.health_of_mut(EntityId(1)).unwrap().hp = 9;
        assert_ne!(base, hash_state(&hurt, 5, &rng, next));
        // Resignation flag.
        let mut resigned = world.clone();
        resigned.player_mut(PlayerId(0)).unwrap().resigned = true;
        assert_ne!(base, hash_state(&resigned, 5, &rng, next));
        // Resource balance.
        let mut paid = world.clone();
        paid.player_mut(PlayerId(0)).unwrap().resources[0].1 = 199;
        assert_ne!(base, hash_state(&paid, 5, &rng, next));
    }
}
