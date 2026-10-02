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

use crate::world::{CapabilityData, Order, World};

/// Version of the canonical state encoding. Bump (and regenerate the golden
/// hashes) whenever the encoded field set or order changes deliberately.
/// v2 added the M4 movement fields (Move radius, remaining waypoints, stuck
/// counters). v3 adds the M5 economy state: the Gather/Build/Produce/Storage/
/// ProvidesPopulation/Resource/Footprint/Construction capability blocks, the
/// `GatherAt`/`BuildAt` orders, and the `UnderConstruction` lifecycle.
pub(crate) const STATE_ENCODING_VERSION: u32 = 3;

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
        // Capability presence bitmask over the fixed order (Health, Move,
        // Vision, Gather, Build, Produce, Storage, ProvidesPopulation, Resource,
        // Footprint, Construction), then the present blocks in that same order.
        let mask = capability_mask(world, entity.id);
        h.write_u16(mask);
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
        if let Some(CapabilityData::Gather(def)) = capability_of(world, entity.id, mask, 3) {
            h.write_i64(def.carry_amount);
            h.write_u32(def.gather_time_ticks);
            h.write_u32(def.timer);
            match def.cargo {
                None => h.write_u8(0),
                Some((resource, amount)) => {
                    h.write_u8(1);
                    h.write_u32(resource.0);
                    h.write_i64(amount);
                }
            }
        }
        // Build (slot 4) carries no fields — presence is the whole block.
        if let Some(CapabilityData::Produce(def)) = capability_of(world, entity.id, mask, 5) {
            h.write_u32(def.queue.len() as u32);
            for item in &def.queue {
                h.write_u32(item.producible.0);
                h.write_u32(item.cost_paid.len() as u32);
                for (resource, amount) in &item.cost_paid {
                    h.write_u32(resource.0);
                    h.write_i64(*amount);
                }
                h.write_u32(item.progress_ticks);
            }
            match def.rally {
                None => h.write_u8(0),
                Some(rally) => {
                    h.write_u8(1);
                    h.write_i32(rally.x.raw());
                    h.write_i32(rally.y.raw());
                }
            }
        }
        if let Some(CapabilityData::Storage(def)) = capability_of(world, entity.id, mask, 6) {
            h.write_u32(def.resources.len() as u32);
            for resource in &def.resources {
                h.write_u32(resource.0);
            }
        }
        if let Some(CapabilityData::ProvidesPopulation(def)) =
            capability_of(world, entity.id, mask, 7)
        {
            h.write_i32(def.amount);
        }
        if let Some(CapabilityData::Resource(def)) = capability_of(world, entity.id, mask, 8) {
            h.write_u32(def.resource.0);
            h.write_i64(def.amount);
        }
        if let Some(CapabilityData::Footprint(def)) = capability_of(world, entity.id, mask, 9) {
            h.write_u32(def.w);
            h.write_u32(def.h);
        }
        if let Some(CapabilityData::Construction(def)) = capability_of(world, entity.id, mask, 10) {
            h.write_u64(def.builder.0);
            h.write_u32(def.progress_ticks);
            h.write_u32(def.total_ticks);
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
        Order::GatherAt { node } => {
            h.write_u8(2);
            h.write_u64(node.0);
        }
        Order::BuildAt { site } => {
            h.write_u8(3);
            h.write_u64(site.0);
        }
    }
}

fn lifecycle_tag(lifecycle: crate::world::Lifecycle) -> u8 {
    match lifecycle {
        crate::world::Lifecycle::UnderConstruction => 3,
        crate::world::Lifecycle::Active => 1,
        crate::world::Lifecycle::Dead => 2,
    }
}

fn controller_tag(player: &crate::world::PlayerState) -> u8 {
    match player.controller {
        pandemonium_sim_api::ControllerKind::Human => 0,
        pandemonium_sim_api::ControllerKind::Ai => 1,
    }
}

/// Presence bitmask over the fixed capability order: bit 0 Health, bit 1 Move,
/// bit 2 Vision, bit 3 Gather, bit 4 Build, bit 5 Produce, bit 6 Storage,
/// bit 7 ProvidesPopulation, bit 8 Resource, bit 9 Footprint, bit 10
/// Construction.
fn capability_mask(world: &World, id: pandemonium_sim_api::EntityId) -> u16 {
    let mut mask = 0u16;
    if world.health_of(id).is_some() {
        mask |= 1 << 0;
    }
    if world.move_of(id).is_some() {
        mask |= 1 << 1;
    }
    if world.vision_of(id).is_some() {
        mask |= 1 << 2;
    }
    if world.gather_of(id).is_some() {
        mask |= 1 << 3;
    }
    if world.build_of(id).is_some() {
        mask |= 1 << 4;
    }
    if world.produce_of(id).is_some() {
        mask |= 1 << 5;
    }
    if world.storage_of(id).is_some() {
        mask |= 1 << 6;
    }
    if world.population_of(id).is_some() {
        mask |= 1 << 7;
    }
    if world.resource_of(id).is_some() {
        mask |= 1 << 8;
    }
    if world.footprint_of(id).is_some() {
        mask |= 1 << 9;
    }
    if world.construction_of(id).is_some() {
        mask |= 1 << 10;
    }
    mask
}

/// Fetches the capability block for slot `index` (see [`capability_mask`] for
/// the fixed order) when the mask says it is present — the mask and the
/// lookups can only agree, because both read the same stores.
fn capability_of(
    world: &World,
    id: pandemonium_sim_api::EntityId,
    mask: u16,
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
        2 => world.vision_of(id).map(|def| CapabilityData::Vision(*def)),
        3 => world.gather_of(id).map(|def| CapabilityData::Gather(*def)),
        4 => world.build_of(id).map(|def| CapabilityData::Build(*def)),
        5 => world
            .produce_of(id)
            .map(|def| CapabilityData::Produce(def.clone())),
        6 => world
            .storage_of(id)
            .map(|def| CapabilityData::Storage(def.clone())),
        7 => world
            .population_of(id)
            .map(|def| CapabilityData::ProvidesPopulation(*def)),
        8 => world
            .resource_of(id)
            .map(|def| CapabilityData::Resource(*def)),
        9 => world
            .footprint_of(id)
            .map(|def| CapabilityData::Footprint(*def)),
        _ => world
            .construction_of(id)
            .map(|def| CapabilityData::Construction(*def)),
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
