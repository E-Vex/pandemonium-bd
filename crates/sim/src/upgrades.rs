//! The upgrade system (ADR-0203, plan §19 Update 7): the research pipeline
//! and the permanent-modifier reads.
//!
//! Two responsibilities, both anchored to the production system's own model
//! (plan §9.4 — research is production's shape with a different completion
//! surface):
//!
//! 1. **The per-tick pass** ([`advance_research`]) runs at the head of stage
//!    3 (production), before the queues' own work: every researcher's
//!    in-flight project advances one tick (the `advance_queues` head
//!    semantics exactly — a project enqueued at tick T first advances at
//!    T+1, fires [`Event::ResearchStarted`] on that first advance, and
//!    completes the tick its elapsed count reaches the spec's authored
//!    `research_ticks`). Completion removes the runtime block, appends the
//!    upgrade to the player's permanent researched ledger, and fires
//!    [`Event::ResearchCompleted`]. Researchers in ascending id order,
//!    collect-then-mutate — the stage's own law.
//!
//! 2. **The enqueue half** ([`begin`]) runs at stage 1, inside the command
//!    apply step, after the gate validated the research: the cost is spent
//!    (the Train economy rule — paid at enqueue, never refunded) and the
//!    runtime block is injected (the `Construction` pattern: never authored
//!    in content, injected by the command, removed at completion or with
//!    the researcher's death).
//!
//! The effects themselves are not applied here: the two Update-7 effect
//! kinds are permanent stat multipliers read live at the consumers —
//! combat builds [`PendingHit`] damage through
//! [`World::effective_damage`](crate::world::World::effective_damage) and
//! movement reads
//! [`World::effective_speed`](crate::world::World::effective_speed) — so a
//! completed upgrade applies to every present and future entity of the
//! targeted kinds with zero mutation at completion (ADR-0203 §2's own
//! design: no entity walks, no spawn hook, no migration when the effect
//! lands). The effect vocabulary is closed in code (plan §10.1): data
//! selects and parameterizes; a new effect kind is a new variant plus a new
//! consumer read.

use pandemonium_sim_api::{EntityId, Event};

use crate::world::{ResearchedUpgrade, World};

/// Stage 3, head: the research pipeline (researchers in ascending id order).
///
/// Progress semantics mirror the production queue's head exactly: `advanced
/// = progress + 1`, `started` when `progress == 0`, completion when
/// `advanced >= total`. The started event fires on the first advancing tick
/// (the cost was already spent at enqueue — the event is the countdown's
/// cue, not the purchase's), and the completed event fires the tick the
/// project lands, in the same pass that removes the block.
pub(crate) fn advance_research(world: &mut World, events: &mut Vec<Event>) {
    let researchers: Vec<EntityId> = world.research.iter().map(|(id, _)| *id).collect();
    for researcher in researchers {
        let Some(entity) = world.entity(researcher) else {
            continue;
        };
        // Sites do not research while under construction (their blocks are
        // unreachable by the gate; defensive skip, the production rule).
        if entity.lifecycle != crate::world::Lifecycle::Active {
            continue;
        }
        let owner = entity.owner;
        let carrier = entity.kind;
        let Some(block) = world.research_of(researcher) else {
            continue;
        };
        let slot = block.slot;
        let progress = block.progress_ticks;
        // The spec lives on the same entity's Upgrades capability — the
        // block and the capability are injected/authored together, so the
        // lookup cannot miss on a well-formed world (defensive skip keeps a
        // hand-built fixture from panicking).
        let Some(total) = world
            .upgrades_of(researcher)
            .and_then(|def| def.specs.get(slot as usize))
            .map(|spec| spec.research_ticks)
        else {
            continue;
        };
        let started = progress == 0;
        let advanced = progress + 1;
        // Mutate: advance the block (collect-then-mutate, the stage's rule).
        if let Some(block) = world.research_of_mut(researcher) {
            block.progress_ticks = advanced;
        }
        if started {
            events.push(Event::ResearchStarted {
                researcher,
                upgrade: slot,
            });
        }
        if advanced >= total {
            // Complete: remove the block, append to the player's ledger.
            world.clear_research(researcher);
            let effect = world
                .upgrades_of(researcher)
                .and_then(|def| def.specs.get(slot as usize))
                .map(|spec| spec.effect.clone());
            if let Some(effect) = effect {
                insert_researched(
                    world,
                    ResearchedUpgrade {
                        player: owner,
                        carrier,
                        slot,
                        effect,
                    },
                );
                events.push(Event::ResearchCompleted {
                    researcher,
                    upgrade: slot,
                });
            }
        }
    }
}

/// Stage 1, inside the command apply: begin a validated research project.
///
/// The gate ([`crate::command`]) has already checked ownership, capability,
/// lifecycle, slot, prior research, prerequisites, the idle research slot,
/// and affordability. This spends the cost and injects the runtime block
/// (the Train rule — paid at enqueue, no cancel path in Update 7 — and the
/// Construction pattern — the block is runtime, never authored).
pub(crate) fn begin(world: &mut World, researcher: EntityId, slot: u8) {
    let owner = world.entity(researcher).map(|entity| entity.owner);
    let cost = world
        .upgrades_of(researcher)
        .and_then(|def| def.specs.get(slot as usize))
        .map(|spec| spec.cost.clone())
        .unwrap_or_default();
    if let Some(owner) = owner {
        if let Some(player) = world.player_mut(owner) {
            player.spend(&cost);
        }
    }
    world.set_research(
        researcher,
        crate::world::ResearchDef {
            slot,
            progress_ticks: 0,
        },
    );
}

/// Appends one completed upgrade to the researched ledger, keeping the
/// (player, carrier, slot) sort (the store's invariant — deterministic
/// walks for the combat/movement reads and a stable hash encoding).
fn insert_researched(world: &mut World, entry: ResearchedUpgrade) {
    let key = (entry.player, entry.carrier, entry.slot);
    match world
        .researched
        .binary_search_by(|probe| (probe.player, probe.carrier, probe.slot).cmp(&key))
    {
        Ok(_) => {} // The gate refuses re-research; a duplicate cannot land.
        Err(index) => world.researched.insert(index, entry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{ResearchDef, UpgradeEffect, UpgradeSpec, UpgradesDef};
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::{KindId, PlayerId, Vec2Fx};

    use crate::CapabilityData;

    /// A researcher (kind 0) offering two upgrades — a damage upgrade on
    /// troopers and a speed upgrade that requires the first — plus the
    /// fixture forms the runtime reads.
    fn research_world() -> World {
        let mut world = World::new();
        world.kind_count = 2;
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(5, 5),
            vec![
                CapabilityData::Health(crate::world::HealthDef {
                    max_hp: 500,
                    hp: 500,
                    regen_per_tick: 0,
                }),
                CapabilityData::Upgrades(UpgradesDef {
                    specs: vec![
                        UpgradeSpec {
                            id: "guns".to_string(),
                            effect: UpgradeEffect::AttackDamage {
                                multiplier: Fx::from_milli(1250),
                                targets: vec![KindId(1)],
                            },
                            research_ticks: 3,
                            cost: Vec::new(),
                            requires: Vec::new(),
                        },
                        UpgradeSpec {
                            id: "engines".to_string(),
                            effect: UpgradeEffect::MoveSpeed {
                                multiplier: Fx::from_milli(1150),
                                targets: vec![KindId(1)],
                            },
                            research_ticks: 2,
                            cost: Vec::new(),
                            requires: vec![(KindId(0), 0)],
                        },
                    ],
                }),
            ],
        );
        world
    }

    #[test]
    fn begin_spends_and_injects_the_block() {
        let mut world = research_world();
        // Give the player an ore ledger the spec prices against.
        world.players.push(crate::world::PlayerState::from_setup(
            &pandemonium_sim_api::PlayerSetup {
                player: PlayerId(0),
                controller: pandemonium_sim_api::ControllerKind::Human,
            },
            &[(pandemonium_sim_api::ResourceId(0), 100)],
        ));
        // Price the first spec at 40 ore.
        if let Some(def) = world.upgrades_of_mut(EntityId(1)) {
            def.specs[0].cost = vec![(pandemonium_sim_api::ResourceId(0), 40)];
        }
        begin(&mut world, EntityId(1), 0);
        assert_eq!(
            world.player(PlayerId(0)).unwrap().resources[0].1,
            60,
            "the cost is spent at enqueue, the Train rule"
        );
        assert_eq!(
            world.research_of(EntityId(1)).map(|def| def.slot),
            Some(0),
            "the block is injected at progress zero"
        );
    }

    #[test]
    fn research_starts_completes_and_ledgers() {
        let mut world = research_world();
        world.set_research(
            EntityId(1),
            ResearchDef {
                slot: 0,
                progress_ticks: 0,
            },
        );
        let mut events = Vec::new();
        // Three ticks of research (total = 3): tick 1 starts, tick 3 lands.
        for expected_progress in [1u32, 2, 3] {
            advance_research(&mut world, &mut events);
            if expected_progress < 3 {
                assert_eq!(
                    world.research_of(EntityId(1)).unwrap().progress_ticks,
                    expected_progress
                );
            }
        }
        assert!(matches!(
            events[0],
            Event::ResearchStarted {
                researcher: EntityId(1),
                upgrade: 0
            }
        ));
        assert!(matches!(
            events[1],
            Event::ResearchCompleted {
                researcher: EntityId(1),
                upgrade: 0
            }
        ));
        assert!(
            world.research_of(EntityId(1)).is_none(),
            "the block is removed at completion"
        );
        assert_eq!(world.researched.len(), 1);
        assert!(world.researched_by(PlayerId(0), KindId(0), 0));
    }

    #[test]
    fn a_dead_researcher_loses_only_the_in_flight_project() {
        let mut world = research_world();
        world.set_research(
            EntityId(1),
            ResearchDef {
                slot: 0,
                progress_ticks: 0,
            },
        );
        // Two ticks in, the researcher falls: stage 8's removal path drops
        // the block (the remove/remove_batch law).
        let mut events = Vec::new();
        advance_research(&mut world, &mut events);
        advance_research(&mut world, &mut events);
        world.remove(EntityId(1));
        assert!(world.research_of(EntityId(1)).is_none());
        assert!(world.researched.is_empty());
        // The ledger only ever grows through completion — a later world
        // where the player finished first keeps it through any death.
        insert_researched(
            &mut world,
            ResearchedUpgrade {
                player: PlayerId(0),
                carrier: KindId(0),
                slot: 0,
                effect: UpgradeEffect::AttackDamage {
                    multiplier: Fx::from_milli(1250),
                    targets: vec![KindId(1)],
                },
            },
        );
        world.remove(EntityId(1));
        assert!(world.researched_by(PlayerId(0), KindId(0), 0));
    }

    #[test]
    fn the_effects_apply_at_the_consumer_reads() {
        let mut world = research_world();
        world.spawn(
            EntityId(2),
            PlayerId(0),
            KindId(1),
            Vec2Fx::from_ints(10, 10),
            vec![
                CapabilityData::Health(crate::world::HealthDef {
                    max_hp: 100,
                    hp: 100,
                    regen_per_tick: 0,
                }),
                CapabilityData::Move(crate::world::MoveDef::new(
                    Fx::from_milli(80),
                    Fx::from_milli(300),
                )),
            ],
        );
        // Before: authored stats.
        assert_eq!(world.effective_damage(EntityId(2), 8), 8);
        assert_eq!(
            world.effective_speed(EntityId(2)).unwrap().raw(),
            Fx::from_milli(80).raw()
        );
        // The damage upgrade lands: 8 x 1.25 = 10 exactly.
        insert_researched(
            &mut world,
            ResearchedUpgrade {
                player: PlayerId(0),
                carrier: KindId(0),
                slot: 0,
                effect: UpgradeEffect::AttackDamage {
                    multiplier: Fx::from_milli(1250),
                    targets: vec![KindId(1)],
                },
            },
        );
        assert_eq!(world.effective_damage(EntityId(2), 8), 10);
        // A second matching upgrade composes in store order: 10 x 1.25
        // floors at 12 (the documented round-toward-zero).
        insert_researched(
            &mut world,
            ResearchedUpgrade {
                player: PlayerId(0),
                carrier: KindId(0),
                slot: 1,
                effect: UpgradeEffect::AttackDamage {
                    multiplier: Fx::from_milli(1250),
                    targets: vec![KindId(1)],
                },
            },
        );
        assert_eq!(world.effective_damage(EntityId(2), 8), 12);
        // The enemy holds the same upgrade: no cross-player bleed.
        insert_researched(
            &mut world,
            ResearchedUpgrade {
                player: PlayerId(1),
                carrier: KindId(0),
                slot: 0,
                effect: UpgradeEffect::MoveSpeed {
                    multiplier: Fx::from_milli(1500),
                    targets: vec![KindId(1)],
                },
            },
        );
        assert_eq!(
            world.effective_speed(EntityId(2)).unwrap().raw(),
            Fx::from_milli(80).raw(),
            "the enemy's upgrade never touches our mover"
        );
    }
}
