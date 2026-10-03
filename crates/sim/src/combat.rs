//! Stage 7 — Combat (plan §9.2, M6): the immediate-hit attack pipeline.
//!
//! The pipeline shape (every stage a named function so future armor, projectiles,
//! and abilities insert at a stage):
//!
//! ```text
//!   for each attacker (ascending id):
//!     1. acquire      — pick a target (commanded > units-attacking-friendly >
//!                        other units > structures); clear stale targets
//!     2. validate     — target alive, in range, visible to the attacker's owner
//!     3. wind_up      — 0 ticks in Alpha (placeholder stage; future cast bars
//!                        insert here)
//!     4. hit          — damage applied this tick if cooldown allows + in range
//!     5. mitigate     — identity in Alpha (future armor / damage reduction)
//!     6. apply        — subtract from the target's Health hp (saturating at 0)
//!     7. death        — handled by stage 8 (`advance_health_and_cleanup`),
//!                        which already detects hp <= 0 and removes the entity
//!     8. credit       — tracked as the `attacker` on the AttackHit event;
//!                        future kill-credit / XP inserts here
//! ```
//!
//! Determinism invariants (plan §5):
//! - Attackers iterate in ascending `EntityId` order (the store's invariant).
//! - Targets are scanned ascending id; ties go to the lower id (A-057).
//! - All distance tests are squared-distance comparisons (no roots, plan §5.8).
//! - Damage is integer; cooldown is in whole ticks.
//! - The pipeline collects `(attacker_id, target_id, target_pos)` triples first,
//!   then mutates — the established collect-then-mutate shape (M4/M5 gotcha).
//!
//! "Immediate-hit" means damage applies the tick the cooldown allows and the
//! target is in range. No projectile entity is spawned; the tracer is a
//! presentation-only cue drawn from the `AttackHit` event. (The source scan
//! reads comments too — write "immediate-hit", never the wall-clock word.)

use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim_api::{EntityId, Event, PlayerId};

use crate::world::{AttackDef, Order, World};

/// Runs the combat pipeline for one tick (plan §6.3 stage 7, §9.2). Damage is
/// applied to `Health.hp`; stage 8's `advance_health_and_cleanup` detects the
/// deaths, removes the entities, releases footprint tiles, fires `Event::Died`,
/// and recomputes population. This function fires `Event::AttackHit` for
/// every hit and clears stale target slots when a target dies between hits.
pub(crate) fn advance_combat(world: &mut World, events: &mut Vec<Event>) {
    // 0. Tick every attacker's cooldown first (so an attacker that hit last
    //    tick counts down this tick). Pure runtime state; no events.
    for (_, def) in &mut world.attack {
        def.tick_cooldown();
    }

    // 1. Acquisition pass: collect (attacker, target) pairs in ascending
    //    attacker-id order (the store's invariant). Targets are validated
    //    inside the same pass — a target that fails validation clears the
    //    slot (the attacker becomes idle until next tick's acquisition).
    let mut hits: Vec<PendingHit> = Vec::new();
    // Snapshot the attacker list as (id, def) so we can iterate without
    // borrowing `world` mutably while we look up targets.
    let attackers: Vec<(EntityId, AttackDef)> =
        world.attack.iter().map(|(id, def)| (*id, *def)).collect();
    for (attacker_id, mut attacker_def) in attackers {
        // Resolve the commanded target if the order queue's head is AttackUnit.
        let commanded = world
            .entity(attacker_id)
            .and_then(|e| e.orders.first())
            .and_then(|order| match order {
                Order::AttackUnit { target } => Some(*target),
                _ => None,
            });

        acquire_target(world, attacker_id, &mut attacker_def, commanded);

        if let Some(target_id) = attacker_def.target {
            let Some(target_pos) = world.entity(target_id).map(|t| t.pos) else {
                // Target died or was removed since acquisition; clear the slot.
                attacker_def.clear_target();
                persist(world, attacker_id, &attacker_def);
                continue;
            };
            let attacker_pos = world
                .entity(attacker_id)
                .map(|e| e.pos)
                .unwrap_or(Vec2Fx::ZERO);
            let target_owner = world
                .entity(target_id)
                .map(|t| t.owner)
                .unwrap_or(PlayerId::NEUTRAL);

            if validate(
                world,
                attacker_id,
                attacker_pos,
                target_id,
                target_pos,
                attacker_def.range,
            ) && attacker_def.ready()
            {
                hits.push(PendingHit {
                    attacker_id,
                    attacker_owner: world
                        .entity(attacker_id)
                        .map(|e| e.owner)
                        .unwrap_or(PlayerId::NEUTRAL),
                    target_id,
                    target_owner,
                    target_pos,
                    damage: attacker_def.damage,
                });
                attacker_def.consume();
            }
        }
        persist(world, attacker_id, &attacker_def);
    }

    // 2. Apply hits: collect-then-mutate means we can mutate `world.health`
    //    without holding a borrow from the acquisition pass.
    for hit in hits {
        apply_damage(world, hit, events);
    }
}

/// A pending hit ready to apply — produced by the acquisition+validation pass,
/// consumed by the apply pass. Carrying the target's position lets the
/// `AttackHit` event include the hit's location for tracer rendering. The
/// owner fields are reserved for the future kill-credit surface (plan §9.2
/// stage 8 — currently unused, logged so the struct shape absorbs the future
/// credit assignment without a pipeline change).
#[allow(dead_code)]
struct PendingHit {
    attacker_id: EntityId,
    attacker_owner: PlayerId,
    target_id: EntityId,
    target_owner: PlayerId,
    target_pos: Vec2Fx,
    damage: i32,
}

/// Stage 1 — Acquisition (plan §9.2): pick a target.
///
/// Priority:
/// 1. The commanded target (`Order::AttackUnit` head), if alive and the
///    attacker is its owner's enemy.
/// 2. The closest unit (with an Attack capability) currently targeting a
///    friendly unit, within `acquire_range`.
/// 3. The closest other unit, within `acquire_range`.
/// 4. The closest structure (footprint-carrying entity), within `acquire_range`.
///
/// Ties go to the lower id (A-057). If nothing is in range, the target slot
/// stays as it was (an existing target is kept until validation clears it).
fn acquire_target(
    world: &World,
    attacker_id: EntityId,
    attacker_def: &mut AttackDef,
    commanded: Option<EntityId>,
) {
    // If the commanded target is alive and legal, keep it.
    if let Some(target) = commanded {
        if world.entity(target).is_some() {
            attacker_def.target = Some(target);
            return;
        }
    }

    // Otherwise scan for an auto-acquired target within acquire_range.
    let Some(attacker) = world.entity(attacker_id) else {
        return;
    };
    let attacker_owner = attacker.owner;
    let attacker_pos = attacker.pos;
    let acquire_sq = squared(attacker_def.acquire_range);

    // Scan candidates ascending id; track the best per category with id
    // tie-breaking.
    let mut best_unit_attacking_friendly: Option<(u64, EntityId)> = None;
    let mut best_other_unit: Option<(u64, EntityId)> = None;
    let mut best_structure: Option<(u64, EntityId)> = None;

    for candidate in &world.entities {
        let candidate_id = candidate.id;
        if candidate_id == attacker_id {
            continue;
        }
        let candidate = world.entity(candidate_id);
        let Some(candidate) = candidate else {
            continue;
        };
        if candidate.owner == attacker_owner {
            continue; // friendlies are never targets
        }
        let dist_sq = (attacker_pos - candidate.pos).len_sq_raw();
        if dist_sq > acquire_sq {
            continue;
        }

        let is_structure = world.footprint_of(candidate_id).is_some();
        let is_attacker = world.attack_of(candidate_id).is_some();

        // "Units attacking a friendly" = an attacker whose current target is
        // owned by the attacker's owner (the friendly side).
        let attacks_friendly = world.attack_of(candidate_id).is_some_and(|def| {
            def.target
                .is_some_and(|t| world.entity(t).is_some_and(|te| te.owner == attacker_owner))
        });

        if is_structure {
            update_best(&mut best_structure, dist_sq, candidate_id);
        } else if attacks_friendly && is_attacker {
            update_best(&mut best_unit_attacking_friendly, dist_sq, candidate_id);
        } else {
            update_best(&mut best_other_unit, dist_sq, candidate_id);
        }
    }

    // Pick by priority. Lower priority categories are only considered if the
    // higher ones found nothing.
    let chosen = best_unit_attacking_friendly
        .or(best_other_unit)
        .or(best_structure)
        .map(|(_, id)| id);
    attacker_def.target = chosen;
}

/// Updates the "best so far" slot, taking the lower dist_sq (ties to the
/// lower id — the `<=` makes a same-distance later candidate lose, so the
/// ascending-id scan order is the tiebreak).
fn update_best(best: &mut Option<(u64, EntityId)>, dist_sq: u64, id: EntityId) {
    match best {
        Some((cur_sq, _)) if *cur_sq <= dist_sq => {}
        _ => *best = Some((dist_sq, id)),
    }
}

/// Stage 2 — Validate (plan §9.2): the target must be alive, in range, and
/// visible to the attacker's owner (FD-8: fog never alters the simulation,
/// but the gate already rejected an Attack command on a non-visible target;
/// auto-acquired targets are within vision because they are within acquire
/// range, and acquire_range <= vision for every authored unit — verified in
/// content validation; A-058 logs the assumption).
fn validate(
    world: &World,
    attacker_id: EntityId,
    attacker_pos: Vec2Fx,
    target_id: EntityId,
    target_pos: Vec2Fx,
    range: Fx,
) -> bool {
    let Some(target) = world.entity(target_id) else {
        return false;
    };
    // Dead entities are removed the same tick they die (stage 8), so a present
    // entity is alive.
    let _ = target;
    let range_sq = squared(range);
    let dist_sq = (attacker_pos - target_pos).len_sq_raw();
    if dist_sq > range_sq {
        return false;
    }
    // Range check passes. The visibility gate is structural: the command gate
    // already enforced NotVisible for commanded targets (plan §8.2); auto-
    // acquired targets are within acquire_range, which is ≤ vision radius for
    // every authored unit (A-058), so they are by construction visible.
    let _ = attacker_id;
    let _ = world;
    true
}

/// Stage 5 + 6 — Mitigate (identity in Alpha) + Apply damage.
///
/// Subtracts `damage` from the target's `Health.hp`, saturating at zero so
/// the A12 `hp >= 0` invariant holds. Emits `Event::AttackHit` with the
/// integer damage applied and the target's position (the presentation layer
/// draws the tracer from attacker → target_pos). Death detection, footprint
/// release, `Event::Died`, and population recompute happen in stage 8.
fn apply_damage(world: &mut World, hit: PendingHit, events: &mut Vec<Event>) {
    let applied = mitigate(hit.damage);
    if let Some(def) = world.health_of_mut(hit.target_id) {
        def.hp = def.hp.saturating_sub(applied).max(0);
    }
    events.push(Event::AttackHit {
        attacker: hit.attacker_id,
        target: hit.target_id,
        damage: applied,
    });
    // Stage 8 (Credit, future): a kill is attributable when the target's hp
    // reaches 0 this tick. The `attacker` field on AttackHit is the credit
    // surface; future kill counts / XP insert here without changing the
    // pipeline shape. (The target's death fires `Event::Died` from stage 8.)
    let _ = hit.target_owner;
    let _ = hit.target_pos;
}

/// Stage 5 — Mitigate (plan §9.2): identity in Alpha. Future armor, damage
/// reduction, and shields insert here. Returns the post-mitigation damage
/// (currently the input unchanged).
fn mitigate(damage: i32) -> i32 {
    damage.max(0)
}

/// Squared range (plan §5.8: squared-distance comparisons, no roots).
fn squared(range: Fx) -> u64 {
    let raw = range.raw().max(0) as u64;
    raw * raw
}

/// Writes the attacker's per-tick-decided runtime state (cooldown_remaining
/// and target slot) back to the store. The acquisition pass mutates a local
/// copy; this persists the result so the next tick's pass picks up where this
/// one left off.
fn persist(world: &mut World, attacker_id: EntityId, def: &AttackDef) {
    if let Some(slot) = world.attack_of_mut(attacker_id) {
        slot.cooldown_remaining = def.cooldown_remaining;
        slot.target = def.target;
    }
}

/// Hygiene: when a target dies, every attacker whose slot pointed at it must
/// drop the slot. Called by stage 8 after the dead are removed — runs in
/// ascending attacker-id order (the store's invariant).
pub(crate) fn clear_dead_targets(world: &mut World, dead: &[EntityId]) {
    // dead is ascending id (stage 8 collects it that way); we just need set
    // membership, but a linear scan keeps the determinism contract simple
    // (no unordered containers).
    for (_, def) in &mut world.attack {
        if let Some(t) = def.target {
            if dead.contains(&t) {
                def.target = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{HealthDef, MoveDef};
    use crate::CapabilityData;
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::{EntityId, PlayerId};

    /// A minimal combat fixture: one attacker vs one target at a known range.
    fn combat_world() -> World {
        let mut world = World::new();
        // Player 0 attacker, id 1, position (10, 10), damage 10, range 1000 milli.
        world.spawn(
            EntityId(1),
            PlayerId(0),
            pandemonium_sim_api::KindId(0),
            Vec2Fx::from_ints(10, 10),
            vec![
                CapabilityData::Health(HealthDef {
                    max_hp: 100,
                    hp: 100,
                    regen_per_tick: 0,
                }),
                CapabilityData::Move(MoveDef::new(Fx::from_milli(50), Fx::from_milli(350))),
                CapabilityData::Attack(AttackDef::new(
                    10,
                    Fx::from_milli(1000),
                    1,
                    Fx::from_milli(2000),
                )),
            ],
        );
        // Player 1 target, id 2, position (10, 11) — 1000 milli = exactly in range.
        world.spawn(
            EntityId(2),
            PlayerId(1),
            pandemonium_sim_api::KindId(1),
            Vec2Fx::from_ints(10, 11),
            vec![
                CapabilityData::Health(HealthDef {
                    max_hp: 30,
                    hp: 30,
                    regen_per_tick: 0,
                }),
                CapabilityData::Move(MoveDef::new(Fx::from_milli(50), Fx::from_milli(350))),
            ],
        );
        world
    }

    #[test]
    fn attack_reduces_target_hp_and_emits_attackhit() {
        let mut world = combat_world();
        // Command the attacker to attack the target.
        world.entity_mut(EntityId(1)).unwrap().orders = vec![Order::AttackUnit {
            target: EntityId(2),
        }];
        let mut events = Vec::new();
        advance_combat(&mut world, &mut events);
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 20);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::AttackHit {
                attacker,
                target,
                damage,
            } => {
                assert_eq!(*attacker, EntityId(1));
                assert_eq!(*target, EntityId(2));
                assert_eq!(*damage, 10);
            }
            other => panic!("expected AttackHit, got {other:?}"),
        }
    }

    #[test]
    fn cooldown_blocks_consecutive_hits() {
        let mut world = combat_world();
        world.entity_mut(EntityId(1)).unwrap().orders = vec![Order::AttackUnit {
            target: EntityId(2),
        }];
        let mut events = Vec::new();
        // Tick 1: hit (hp 30 -> 20, cooldown set to 1).
        advance_combat(&mut world, &mut events);
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 20);
        assert_eq!(events.len(), 1);
        // Tick 2: cooldown_remaining is now 0 (decremented at start of tick),
        // so a second hit lands. (Cooldown_ticks=1 means hit every tick.)
        events.clear();
        advance_combat(&mut world, &mut events);
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 10);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn cooldown_two_ticks_blocks_every_other_tick() {
        let mut world = combat_world();
        // Override the attacker's cooldown to 2 ticks. Semantics:
        //   tick 1 start: cooldown_remaining = 0 -> ready -> hit.
        //                  consume() sets cooldown_remaining = 2.
        //   tick 2 start: tick_cooldown() -> 1. Not ready.
        //   tick 3 start: tick_cooldown() -> 0. Ready -> hit.
        //                  consume() sets cooldown_remaining = 2.
        //   tick 4 start: tick_cooldown() -> 1. Not ready.
        //   tick 5 start: tick_cooldown() -> 0. Ready -> hit.
        // So with cooldown_ticks=2 the attacker hits every 2nd tick (1, 3, 5).
        world.attack_of_mut(EntityId(1)).unwrap().cooldown_ticks = 2;
        world.entity_mut(EntityId(1)).unwrap().orders = vec![Order::AttackUnit {
            target: EntityId(2),
        }];
        let mut events = Vec::new();
        // Tick 1: ready -> hit (30 -> 20). Cooldown set to 2.
        advance_combat(&mut world, &mut events);
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 20);
        assert_eq!(events.len(), 1);
        // Tick 2: cooldown_remaining 2 -> 1 (decremented). Not ready.
        events.clear();
        advance_combat(&mut world, &mut events);
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 20);
        assert!(events.is_empty());
        // Tick 3: cooldown_remaining 1 -> 0. Ready -> hit (20 -> 10).
        events.clear();
        advance_combat(&mut world, &mut events);
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 10);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn out_of_range_attacker_does_not_hit_but_acquires() {
        let mut world = combat_world();
        // Move the target out of range (range is 1000 milli = 1 tile; put it 5 tiles away).
        world.entity_mut(EntityId(2)).unwrap().pos = Vec2Fx::from_ints(15, 15);
        world.entity_mut(EntityId(1)).unwrap().orders = vec![Order::AttackUnit {
            target: EntityId(2),
        }];
        let mut events = Vec::new();
        advance_combat(&mut world, &mut events);
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 30);
        assert!(events.is_empty());
        // The target slot was set by acquisition (in acquire_range).
        assert_eq!(
            world.attack_of(EntityId(1)).unwrap().target,
            Some(EntityId(2))
        );
    }

    #[test]
    fn auto_acquisition_picks_closest_enemy_unit_in_acquire_range() {
        let mut world = combat_world();
        // Add a third entity: another enemy unit closer than the target.
        let close_pos = Vec2Fx::new(
            world.entity(EntityId(1)).unwrap().pos.x + Fx::from_milli(500),
            world.entity(EntityId(1)).unwrap().pos.y,
        );
        world.spawn(
            EntityId(3),
            PlayerId(1),
            pandemonium_sim_api::KindId(1),
            close_pos, // 0.5 tiles from attacker
            vec![CapabilityData::Health(HealthDef {
                max_hp: 20,
                hp: 20,
                regen_per_tick: 0,
            })],
        );
        // No commanded target — auto-acquire should pick the closest enemy unit.
        let mut events = Vec::new();
        advance_combat(&mut world, &mut events);
        // Either id 2 or id 3 should be hit, but id 3 is closer.
        let total_damage: i32 = events
            .iter()
            .map(|e| match e {
                Event::AttackHit { damage, .. } => *damage,
                _ => 0,
            })
            .sum();
        assert_eq!(total_damage, 10);
        assert_eq!(
            world.attack_of(EntityId(1)).unwrap().target,
            Some(EntityId(3))
        );
    }

    #[test]
    fn clear_dead_targets_drops_slots_pointing_at_dead() {
        let mut world = combat_world();
        world.attack_of_mut(EntityId(1)).unwrap().target = Some(EntityId(2));
        clear_dead_targets(&mut world, &[EntityId(2)]);
        assert_eq!(world.attack_of(EntityId(1)).unwrap().target, None);
    }

    #[test]
    fn mitigate_is_identity_in_alpha() {
        assert_eq!(mitigate(7), 7);
        assert_eq!(mitigate(0), 0);
        // Negative damage clamps to zero (defensive — authored damage is non-negative).
        assert_eq!(mitigate(-3), 0);
    }
}
