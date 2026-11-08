//! The ability system (ADR-0202, plan §19 Update 6): the cooldown-ready
//! pipeline and the effect vocabulary's stage hooks.
//!
//! Two responsibilities, both anchored to plan §8.3's reservation that
//! "every stage is a named function so future armor, projectiles, and
//! abilities insert at a stage":
//!
//! 1. **The per-tick pass** ([`advance_abilities`]) runs at the head of stage
//!    7 (combat), before combat's own work: every slot's cooldown ticks down
//!    (the attack-cooldown semantics exactly — a cast at tick T sets the
//!    countdown to `cooldown_ticks`, the same tick's pass decrements it, the
//!    slot is ready again at `T + cooldown_ticks`), and every live speed
//!    boost's remaining ticks tick down with expired blocks removed. Stage 6
//!    (movement) runs before stage 7, so a boost is live for movement from
//!    the cast tick through the tick it hits zero — its last effective tick
//!    is the one where it expires.
//!
//! 2. **The cast effects** ([`cast`]) run at stage 1, inside the command
//!    apply step, after the gate validated the cast: the cost is spent, the
//!    cooldown consumed, the outward [`Event::AbilityCast`] fires, and the
//!    effect resolves over the world in ascending entity-id order — flat
//!    integer math, no variance rolls, squared-distance compares only
//!    (FD-5, plan §5.8).
//!
//! The effect vocabulary is closed in code (ADR-0202 §2): `DamageArea`,
//! `HealArea`, `SpeedBurst`. Data selects and parameterizes; content never
//! carries logic (plan §10.1). A new effect kind is a new variant plus a new
//! arm here — the same review-visible growth every capability has taken.

use pandemonium_fx::Fx;
use pandemonium_sim_api::{EntityId, Event, PlayerId, Vec2Fx};

use crate::world::{AbilityEffect, SpeedBoostDef, World};

/// Stage 7, head: the cooldown-ready pipeline and the boost-expiry pass.
///
/// Cooldown semantics mirror the attack cooldown exactly (`AttackDef::
/// tick_cooldown`): decrement, floor at zero. Boost expiry decrements every
/// live block and removes the ones that reach zero — the store walk runs
/// back-to-front so removals never move a pending index, and survivors keep
/// their order (the ascending-id invariant).
pub(crate) fn advance_abilities(world: &mut World) {
    for (_, def) in &mut world.abilities {
        for remaining in &mut def.cooldowns {
            if *remaining > 0 {
                *remaining -= 1;
            }
        }
    }
    for index in (0..world.speed_boosts.len()).rev() {
        let expired = {
            let (_, def) = &mut world.speed_boosts[index];
            if def.remaining_ticks > 0 {
                def.remaining_ticks -= 1;
            }
            def.remaining_ticks == 0
        };
        if expired {
            world.speed_boosts.remove(index);
        }
    }
}

/// Stage 1, inside the command apply: resolve a validated cast.
///
/// The gate ([`crate::command`]) has already checked ownership, capability,
/// slot, cooldown, range, visibility, and affordability. This spends the
/// cost, consumes the cooldown, fires the outward cue, and applies the
/// effect. Deaths from `DamageArea` are detected by stage 8 (the same tick),
/// exactly like combat damage.
pub(crate) fn cast(
    world: &mut World,
    caster: EntityId,
    slot: u8,
    target: Vec2Fx,
    events: &mut Vec<Event>,
) {
    let (spec, owner) = match world.entity(caster) {
        Some(entity) => match world.abilities_of(caster) {
            Some(def) => match def.specs.get(slot as usize) {
                Some(spec) => (spec.clone(), entity.owner),
                None => return,
            },
            None => return,
        },
        None => return,
    };
    // Spend (validated affordable; the degenerate mid-tick edge saturates at
    // zero, never negative — the ledger invariant).
    if let Some(player) = world.player_mut(owner) {
        player.spend(&spec.cost);
    }
    // Consume the cooldown — the ready slot's zero becomes the authored
    // cooldown, the attack's `consume()` rule.
    if let Some(def) = world.abilities_of_mut(caster) {
        if let Some(remaining) = def.cooldowns.get_mut(slot as usize) {
            *remaining = spec.cooldown_ticks;
        }
    }
    // The outward cue (ADR-0202 §7).
    events.push(Event::AbilityCast {
        caster,
        ability: slot,
        at: target,
    });
    // The effect, over the world in ascending entity-id order (the store's
    // invariant — deterministic application regardless of who is where).
    match spec.effect {
        AbilityEffect::DamageArea { damage, radius } => {
            let victims: Vec<EntityId> = world
                .entities
                .iter()
                .filter(|entity| entity.owner != owner && entity.owner != PlayerId::NEUTRAL)
                .filter(|entity| within(entity.pos, target, radius))
                .map(|entity| entity.id)
                .collect();
            for victim in victims {
                if let Some(def) = world.health_of_mut(victim) {
                    def.hp = def.hp.saturating_sub(damage.max(0)).max(0);
                }
                // Ability damage rides the attack-hit channel (ADR-0202 §7):
                // an ability hit is outwardly an attack-hit-shaped fact — the
                // attacker is the caster, so every existing hit consumer (hit
                // cues, attack radar) works unmodified.
                events.push(Event::AttackHit {
                    attacker: caster,
                    target: victim,
                    damage: damage.max(0),
                });
            }
        }
        AbilityEffect::HealArea { amount, radius } => {
            let patients: Vec<EntityId> = world
                .entities
                .iter()
                .filter(|entity| entity.owner == owner)
                .filter(|entity| within(entity.pos, target, radius))
                .map(|entity| entity.id)
                .collect();
            // No per-target event (ADR-0202's recorded limit): the health
            // bars rise and the cast flash is the cue.
            for patient in patients {
                if let Some(def) = world.health_of_mut(patient) {
                    def.hp = (def.hp + amount.max(0)).min(def.max_hp.max(0));
                }
            }
        }
        AbilityEffect::SpeedBurst {
            multiplier,
            duration_ticks,
            radius,
        } => {
            let runners: Vec<EntityId> = world
                .entities
                .iter()
                .filter(|entity| entity.owner == owner)
                .filter(|entity| within(entity.pos, target, radius))
                .map(|entity| entity.id)
                .collect();
            for runner in runners {
                if world.move_of(runner).is_some() {
                    world.set_speed_boost(
                        runner,
                        SpeedBoostDef {
                            multiplier,
                            remaining_ticks: duration_ticks,
                        },
                    );
                }
            }
        }
    }
}

/// Squared-distance containment (plan §5.8: no roots): `point` within
/// `radius` of `center`.
fn within(point: Vec2Fx, center: Vec2Fx, radius: Fx) -> bool {
    let raw = radius.raw().max(0) as u64;
    let r_sq = raw * raw;
    (point - center).len_sq_raw() <= r_sq
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{AbilitiesDef, AbilitySpec, HealthDef, MoveDef};
    use pandemonium_sim_api::KindId;

    fn caster_world() -> World {
        let mut world = World::new();
        world.kind_count = 1;
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(5, 5),
            vec![
                CapabilityData::Health(HealthDef {
                    max_hp: 100,
                    hp: 100,
                    regen_per_tick: 0,
                }),
                CapabilityData::Abilities(AbilitiesDef::new(vec![AbilitySpec {
                    id: "nuke".to_string(),
                    effect: AbilityEffect::DamageArea {
                        damage: 30,
                        radius: Fx::from_milli(2000),
                    },
                    cast_range: Fx::from_milli(5000),
                    cooldown_ticks: 10,
                    cost: Vec::new(),
                }])),
            ],
        );
        world.spawn(
            EntityId(2),
            PlayerId(1),
            KindId(0),
            Vec2Fx::from_ints(6, 5),
            vec![CapabilityData::Health(HealthDef {
                max_hp: 100,
                hp: 100,
                regen_per_tick: 0,
            })],
        );
        world
    }

    use crate::CapabilityData;

    #[test]
    fn damage_area_hits_enemies_only_and_consumes_cooldown() {
        let mut world = caster_world();
        // A friendly at the same spot — never collateral.
        world.spawn(
            EntityId(3),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(6, 6),
            vec![CapabilityData::Health(HealthDef {
                max_hp: 100,
                hp: 100,
                regen_per_tick: 0,
            })],
        );
        let mut events = Vec::new();
        cast(
            &mut world,
            EntityId(1),
            0,
            Vec2Fx::from_ints(6, 5),
            &mut events,
        );
        assert_eq!(world.health_of(EntityId(2)).unwrap().hp, 70);
        assert_eq!(world.health_of(EntityId(3)).unwrap().hp, 100);
        assert_eq!(world.abilities_of(EntityId(1)).unwrap().cooldowns[0], 10);
        // The cue and the hit both fired, in order.
        assert!(matches!(
            events[0],
            Event::AbilityCast {
                caster: EntityId(1),
                ability: 0,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            Event::AttackHit {
                attacker: EntityId(1),
                target: EntityId(2),
                damage: 30
            }
        ));
    }

    #[test]
    fn cooldown_ticks_down_and_is_ready_again() {
        let mut world = caster_world();
        let mut events = Vec::new();
        cast(
            &mut world,
            EntityId(1),
            0,
            Vec2Fx::from_ints(6, 5),
            &mut events,
        );
        assert_eq!(world.abilities_of(EntityId(1)).unwrap().cooldowns[0], 10);
        for _ in 0..10 {
            advance_abilities(&mut world);
        }
        assert_eq!(world.abilities_of(EntityId(1)).unwrap().cooldowns[0], 0);
    }

    #[test]
    fn speed_boost_expires_after_its_duration() {
        let mut world = World::new();
        world.kind_count = 1;
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(5, 5),
            vec![
                CapabilityData::Move(MoveDef::new(Fx::from_milli(50), Fx::from_milli(300))),
                CapabilityData::Abilities(AbilitiesDef::new(vec![AbilitySpec {
                    id: "sprint".to_string(),
                    effect: AbilityEffect::SpeedBurst {
                        multiplier: Fx::from_milli(1500),
                        duration_ticks: 3,
                        radius: Fx::from_milli(2000),
                    },
                    cast_range: Fx::from_milli(5000),
                    cooldown_ticks: 10,
                    cost: Vec::new(),
                }])),
            ],
        );
        let mut events = Vec::new();
        cast(
            &mut world,
            EntityId(1),
            0,
            Vec2Fx::from_ints(5, 5),
            &mut events,
        );
        assert_eq!(
            world.effective_speed(EntityId(1)).unwrap().raw(),
            Fx::from_milli(50).mul(Fx::from_milli(1500)).raw()
        );
        for _ in 0..3 {
            advance_abilities(&mut world);
        }
        // Three ticks of boost, then the block is gone.
        assert!(world.speed_boost_of(EntityId(1)).is_none());
        assert_eq!(
            world.effective_speed(EntityId(1)).unwrap().raw(),
            Fx::from_milli(50).raw()
        );
    }

    #[test]
    fn heal_area_clamps_at_max() {
        let mut world = World::new();
        world.kind_count = 1;
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(5, 5),
            vec![
                CapabilityData::Health(HealthDef {
                    max_hp: 100,
                    hp: 40,
                    regen_per_tick: 0,
                }),
                CapabilityData::Abilities(AbilitiesDef::new(vec![AbilitySpec {
                    id: "mend".to_string(),
                    effect: AbilityEffect::HealArea {
                        amount: 70,
                        radius: Fx::from_milli(2000),
                    },
                    cast_range: Fx::from_milli(5000),
                    cooldown_ticks: 10,
                    cost: Vec::new(),
                }])),
            ],
        );
        let mut events = Vec::new();
        cast(
            &mut world,
            EntityId(1),
            0,
            Vec2Fx::from_ints(5, 5),
            &mut events,
        );
        assert_eq!(world.health_of(EntityId(1)).unwrap().hp, 100);
    }
}
