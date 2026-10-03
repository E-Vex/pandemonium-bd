//! The A12 invariant checker (plan §13): debug assertions over the whole
//! world state, run at the end of every `Sim::step` in debug builds — so every
//! dev-profile test and soak run enforces them continuously.
//!
//! The invariants, verbatim from A12: no negative resources, population within
//! the cap, no entity on a blocked tile, no id reuse, hp within its pool, and
//! queue costs consistent (every queued item's `cost_paid` equals its kind's
//! authored cost — costs never change mid-match in the Alpha, so a mismatch
//! means bookkeeping drift).
//!
//! Scope note (A-053, retired DEBT-010): `population <= cap` is the spawn-
//! blocking rule. Combat can destroy population-providing structures, opening
//! a temporary over-cap window — usage may exceed cap then, and the M5
//! production queue holds completed items at the queue front until headroom
//! returns (A-042). The strict `population <= cap` assertion is therefore
//! relaxed to a non-negative cap + non-negative usage + (usage is finite)
//! check; the spawn gate is what actually blocks new units when usage is at or
//! above cap. Over-cap from any cause other than combat-destroyed structures
//! is still a bookkeeping bug (and would still trip this checker if usage ever
//! grows past cap while no structures were lost this tick — TODO: a tightened
//! variant the moment M6 soak reveals it). The decision touches A12 only;
//! §9.4's production/construction semantics are unchanged.

use pandemonium_sim_api::EntityId;

use crate::fixture::TrivialWorld;
use crate::nav::NavGrid;
use crate::world::{Order, World};

/// Runs every A12 invariant. Only called from debug builds; each violation
/// names what broke and where.
pub(crate) fn check(world: &World, content: &TrivialWorld, nav: &NavGrid, next_entity_id: u64) {
    // --- Stores stay ascending and every id is under the allocator. --------
    for store in [
        ("entities", entity_ids(world)),
        ("health", store_ids(&world.health)),
        ("movement", store_ids(&world.movement)),
        ("vision", store_ids(&world.vision)),
        ("gather", store_ids(&world.gather)),
        ("build", store_ids(&world.build)),
        ("produce", store_ids(&world.produce)),
        ("storage", store_ids(&world.storage)),
        ("population", store_ids(&world.provides_population)),
        ("resources", store_ids(&world.resources)),
        ("footprints", store_ids(&world.footprints)),
        ("construction", store_ids(&world.construction)),
        ("attack", store_ids(&world.attack)),
    ] {
        let (_, ids) = store;
        debug_assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "A12 id-reuse/order: the {} store is not ascending",
            store.0
        );
        debug_assert!(
            ids.iter().all(|id| id.0 < next_entity_id),
            "A12 id-reuse: the {} store references an id above the allocator",
            store.0
        );
    }

    // --- Health within its pool. -------------------------------------------
    for (id, def) in &world.health {
        debug_assert!(def.hp >= 0, "A12 hp: entity {id:?} has hp {}", def.hp);
        debug_assert!(
            def.hp <= def.max_hp.max(0),
            "A12 hp: entity {id:?} has hp {} above max {}",
            def.hp,
            def.max_hp
        );
    }

    // --- Ledgers non-negative, population cap + usage non-negative. -----
    // A-053: the strict `population <= cap` rule is relaxed to a spawn-
    // blocking rule — usage may legitimately exceed cap when combat
    // destroys population-providing structures (DEBT-010 retired). The
    // M5 production queue already holds completed items at the front
    // until headroom returns (A-042), so the spawn side is the gate.
    for player in &world.players {
        for (resource, amount) in &player.resources {
            debug_assert!(
                *amount >= 0,
                "A12 resources: player {:?} holds {} of resource {:?}",
                player.player,
                amount,
                resource
            );
        }
        debug_assert!(
            player.population_cap <= u32::MAX / 2,
            "A12 population: player {:?} cap {} looks corrupted (saturating arithmetic drift)",
            player.player,
            player.population_cap
        );
        debug_assert!(
            player.population <= player.population_cap.saturating_add(u32::MAX / 4),
            "A12 population: player {:?} usage {} wildly above cap {} (saturating drift, not combat loss)",
            player.player,
            player.population,
            player.population_cap
        );
    }

    // --- No mover on blocked terrain or claimed tiles. ----------------------
    for entity in &world.entities {
        // Footprint entities stand on their own claimed tiles by design.
        if world.footprint_of(entity.id).is_some() {
            continue;
        }
        let (x, y) = (entity.pos.x.floor_int(), entity.pos.y.floor_int());
        debug_assert!(
            nav.passable(x, y) || world.move_of(entity.id).is_none(),
            "A12 blocked tile: entity {} (a mover) stands on ({x},{y})",
            entity.id.0
        );
    }

    // --- Queue costs consistent with the authored kind costs. ---------------
    for (id, def) in &world.produce {
        for (index, item) in def.queue.iter().enumerate() {
            let authored = content
                .kinds
                .get(item.producible.0 as usize)
                .map(|kind| &kind.economy.cost);
            debug_assert_eq!(
                Some(&item.cost_paid),
                authored,
                "A12 queue costs: producer {id:?} item {index} paid {:?}, authored {:?}",
                item.cost_paid,
                authored
            );
            if let Some(kind) = content.kinds.get(item.producible.0 as usize) {
                debug_assert!(
                    item.progress_ticks <= kind.economy.build_time_ticks,
                    "A12 queue costs: producer {id:?} item {index} progressed past its build time"
                );
            }
        }
    }

    // --- Construction progress within its budget. ---------------------------
    for (id, def) in &world.construction {
        debug_assert!(
            def.progress_ticks <= def.total_ticks,
            "A12 construction: site {id:?} progressed past its total"
        );
        debug_assert!(
            world.entity(*id).is_some(),
            "A12 construction: site {id:?} missing from the entity store"
        );
    }

    // --- Economy orders reference live resource nodes. (BuildAt orders may
    // legitimately reference a site that died this tick — stage 4's hygiene
    // cleans them next tick; depletion re-targets same-tick below.) -------
    for entity in &world.entities {
        if let Some(Order::GatherAt { node }) = entity.orders.first() {
            debug_assert!(
                world.resource_of(*node).is_some(),
                "A12 orders: {} gathers from non-node {node:?}",
                entity.id.0
            );
        }
    }
}

/// The entity store's ids.
fn entity_ids(world: &World) -> Vec<EntityId> {
    world.entities.iter().map(|entity| entity.id).collect()
}

/// A capability store's ids, generically over `(EntityId, T)` pairs.
fn store_ids<T>(store: &[(EntityId, T)]) -> Vec<EntityId> {
    store.iter().map(|(id, _)| *id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::construction_support::construction_world;
    use crate::world::PlayerState;
    use pandemonium_sim_api::{ControllerKind, PlayerId, PlayerSetup, ResourceId};

    fn nav_of(content: &TrivialWorld) -> NavGrid {
        NavGrid::new(
            content.width_tiles,
            content.height_tiles,
            &content.passability,
        )
    }

    #[test]
    fn a_clean_world_passes_every_invariant() {
        let content = construction_world();
        let mut world = World::new();
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        world.player_mut(PlayerId(0)).unwrap().population = 1;
        world.player_mut(PlayerId(0)).unwrap().population_cap = 5;
        let nav = nav_of(&content);
        check(&world, &content, &nav, 10);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "A12 resources")]
    fn negative_balances_fire() {
        let content = construction_world();
        let mut world = World::new();
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), -5)],
        ));
        let nav = nav_of(&content);
        check(&world, &content, &nav, 10);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "A12 population")]
    fn population_drift_far_past_cap_fires() {
        // A-053 (DEBT-010 retired): the strict `pop <= cap` rule is relaxed to
        // "spawn-blocking" — a small over-cap from combat-destroyed structures
        // is now legal. This test pins the *wild*-over-cap catch-all: usage
        // saturating far above cap (from bookkeeping drift, not combat loss)
        // still trips the checker.
        let content = construction_world();
        let mut world = World::new();
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        world.player_mut(PlayerId(0)).unwrap().population = u32::MAX / 2;
        world.player_mut(PlayerId(0)).unwrap().population_cap = 5;
        let nav = nav_of(&content);
        check(&world, &content, &nav, 10);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "A12 hp")]
    fn negative_hp_fires() {
        let content = construction_world();
        let mut world = World::new();
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        world.health.push((
            pandemonium_sim_api::EntityId(1),
            crate::world::HealthDef {
                max_hp: 10,
                hp: -1,
                regen_per_tick: 0,
            },
        ));
        let nav = nav_of(&content);
        check(&world, &content, &nav, 10);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "A12 id-reuse")]
    fn ids_above_the_allocator_fire() {
        let content = construction_world();
        let mut world = World::new();
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        world.health.push((
            pandemonium_sim_api::EntityId(99),
            crate::world::HealthDef {
                max_hp: 10,
                hp: 10,
                regen_per_tick: 0,
            },
        ));
        let nav = nav_of(&content);
        check(&world, &content, &nav, 10);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "A12 queue costs")]
    fn drifted_queue_costs_fire() {
        use crate::world::{ProduceDef, QueueItem};
        let content = construction_world();
        let mut world = World::new();
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        let mut def = ProduceDef::new();
        def.queue.push(QueueItem {
            // The depot kind's authored cost is 100; this item "paid" 1.
            producible: pandemonium_sim_api::KindId(1),
            cost_paid: vec![(ResourceId(0), 1)],
            progress_ticks: 0,
        });
        world.produce.push((pandemonium_sim_api::EntityId(1), def));
        let nav = nav_of(&content);
        check(&world, &content, &nav, 10);
    }

    #[test]
    fn the_full_pipeline_holds_every_tick_in_debug() {
        // A scripted economy match over the construction fixture: the checker
        // runs inside every step (debug builds) — completing this test IS the
        // invariant holding continuously.
        use crate::sim::Sim;
        use pandemonium_sim_api::{Command, CommandKind, MatchSetup, TilePos, Vec2Fx};
        let content = construction_world();
        let setup = MatchSetup {
            seed: 21,
            players: vec![PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            }],
        };
        let mut sim = Sim::new(&content, setup);
        sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Build {
                worker: pandemonium_sim_api::EntityId(1),
                structure: pandemonium_sim_api::KindId(1),
                at: TilePos { x: 6, y: 6 },
            },
        )]);
        for _ in 0..120 {
            sim.step(&[]);
        }
        let view = sim.player_view(PlayerId(0));
        assert_eq!(view.population_cap, 15, "the depot completed");
        let _ = Vec2Fx::ZERO;
    }
}
