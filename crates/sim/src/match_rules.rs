//! Stage 10 — Match rules (plan §6.3.10, §9.7, M8): evaluate defeat/victory.
//!
//! A player is *defeated* when they have resigned (the `Resign` command marks
//! the flag) or when they have zero structures — entities carrying a
//! `Footprint` capability, the data-defined "structure" marker (every
//! structure has one; units and resource nodes do not). Ore nodes carry
//! `Footprint` but are neutral, so they never count for a player.
//!
//! The match ends when at least one player is defeated **and** at most one
//! non-defeated player remains. That survivor is the winner; if every player
//! was defeated simultaneously (mutual destruction, an edge case) the winner
//! is `PlayerId::NEUTRAL` — well-defined and deterministic, never expected in
//! normal play. The `MatchEnded` event fires exactly once per match:
//! evaluation is *idempotent* — once `Sim::outcome` is `Some`, re-evaluation
//! is a no-op, so further ticks (presentation, replay re-simulation) never
//! re-emit.
//!
//! **The defeat check only fires when the match is "structure-bearing"** —
//! when at least one player in the match owns at least one structure
//! (A-067). The M1 spine-test fixture (`TrivialWorld`) carries no Footprint
//! kinds at all (it predates the economy milestone), so a strict "zero
//! structures ⇒ defeated" rule would end every spine match at tick 0 with
//! every player simultaneously defeated. The Alpha content (the M2+ real
//! path) starts every player with a Command Center, so the rule fires only
//! when a structure is actually destroyed — the intent of plan §9.7. A
//! degenerate "no one ever had a structure" match simply continues; a real
//! match ends the moment a side is eliminated.
//!
//! **Match outcome is derived state, not canonical hashed state** — the same
//! reasoning as fog (A-059). It is a pure function of the entity set (which
//! determines structure counts) and the players' `resigned` flags, both of
//! which ARE part of the canonical hash. Adding the outcome to the hash
//! would be redundant (the same inputs always produce the same result) and
//! would couple the hash to a derivative. The M7 goldens therefore stay
//! green by construction — no `STATE_ENCODING_VERSION` bump (A-066).
//!
//! The post-MatchEnded sim keeps stepping (commands still apply, state still
//! evolves); the host's `is_finished()` gate is what stops the windowed
//! client's match loop. The headless AI-vs-AI runner keeps running its tick
//! budget regardless — the M7 "matches complete" contract is "ran the budget
//! under the A12 checker without crashing", not "the match resolved".

use pandemonium_sim_api::{Event, PlayerId};

use crate::world::World;

/// The resolved match outcome (M8, plan §9.7). Cached on [`crate::Sim`] the
/// tick the match transitions to ended; `MatchEnded` is emitted the same tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MatchOutcome {
    /// The winning slot. `PlayerId::NEUTRAL` when every player was defeated
    /// simultaneously (the mutual-destruction edge case — never expected in
    /// normal play, but well-defined and deterministic).
    pub winner: PlayerId,
    /// The tick the match ended at (the tick `MatchEnded` was emitted at).
    pub ended_tick: u32,
}

/// Stage 10 (plan §6.3.10, §9.7): evaluate the match rules.
///
/// Idempotent — once `outcome` is `Some`, this function is a no-op. When the
/// match transitions to ended this tick, exactly one [`Event::MatchEnded`] is
/// pushed onto `events` and `outcome` is set.
pub(crate) fn evaluate(
    world: &World,
    tick: u32,
    outcome: &mut Option<MatchOutcome>,
    events: &mut Vec<Event>,
) {
    if outcome.is_some() {
        return; // match already ended — idempotent (plan §9.7: resolve once).
    }
    // The defeat check only fires when the match is "structure-bearing" —
    // at least one player owns at least one structure (A-067). A no-structure
    // world (the M1 spine-test fixture) never triggers defeat; the rule fires
    // only on a real match where someone actually had a structure to lose.
    let any_structure = world.entities.iter().any(|entity| {
        entity.owner != PlayerId::NEUTRAL && world.footprint_of(entity.id).is_some()
    });
    if !any_structure {
        return;
    }
    // A player is defeated when resigned OR when they own zero structures
    // (Footprint-carrying entities — the data-defined "structure" marker).
    // Ore nodes carry Footprint but are neutral and so never count.
    let mut defeated: Vec<PlayerId> = Vec::new();
    for player in &world.players {
        let has_structures = world.entities.iter().any(|entity| {
            entity.owner == player.player && world.footprint_of(entity.id).is_some()
        });
        if player.resigned || !has_structures {
            defeated.push(player.player);
        }
    }
    // The match ends only once at least one player is defeated AND at most
    // one non-defeated player remains. The first clause keeps a one-player
    // smoke match (no opponents, no defeat) running — the M3 headless smoke
    // and the client's no-AI test paths stay green.
    if defeated.is_empty() {
        return;
    }
    let survivors: Vec<PlayerId> = world
        .players
        .iter()
        .map(|p| p.player)
        .filter(|p| !defeated.contains(p))
        .collect();
    if survivors.len() <= 1 {
        // At most one survivor — the match is decided. The survivor wins;
        // no survivor (mutual destruction) is `NEUTRAL`.
        let winner = survivors.first().copied().unwrap_or(PlayerId::NEUTRAL);
        *outcome = Some(MatchOutcome {
            winner,
            ended_tick: tick,
        });
        events.push(Event::MatchEnded { winner });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::TrivialWorld;
    use crate::sim::Sim;
    use pandemonium_sim_api::{
        Command, CommandKind, ControllerKind, MatchSetup, PlayerId, PlayerSetup,
    };

    /// A two-player world where each player owns one structure (kind 0 with a
    /// Footprint). The defeat path: kill one player's structure.
    fn two_player_world() -> TrivialWorld {
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, SpawnDef};
        use pandemonium_fx::Vec2Fx;
        use pandemonium_sim_api::KindId;
        TrivialWorld {
            map_id: 0x004D_3854_4553_5401,
            width_tiles: 32,
            height_tiles: 32,
            passability: TrivialWorld::open_passability(32, 32),
            buildability: TrivialWorld::open_buildability(32, 32),
            kinds: vec![KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 10,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                ],
                economy: KindEconomy::default(),
            }],
            resources: vec![],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(8, 8),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    fn two_player_setup() -> MatchSetup {
        MatchSetup {
            seed: 7,
            players: vec![
                PlayerSetup {
                    player: PlayerId(0),
                    controller: ControllerKind::Human,
                },
                PlayerSetup {
                    player: PlayerId(1),
                    controller: ControllerKind::Human,
                },
            ],
        }
    }

    #[test]
    fn a_running_match_has_no_outcome_and_emits_nothing() {
        let world = two_player_world();
        let mut sim = Sim::new(&world, two_player_setup());
        let out = sim.step(&[]);
        assert!(sim.outcome().is_none());
        assert!(!out
            .events
            .iter()
            .any(|event| matches!(event, Event::MatchEnded { .. })));
    }

    #[test]
    fn resignation_ends_the_match_with_the_other_player_winning() {
        let world = two_player_world();
        let mut sim = Sim::new(&world, two_player_setup());
        // Player 0 resigns at tick 0.
        sim.step(&[Command::new(PlayerId(0), 0, 1, CommandKind::Resign {})]);
        let outcome = sim.outcome().expect("resigning ends the match");
        assert_eq!(outcome.winner, PlayerId(1));
    }

    #[test]
    fn match_ended_fires_exactly_once_even_after_more_ticks() {
        let world = two_player_world();
        let mut sim = Sim::new(&world, two_player_setup());
        sim.step(&[Command::new(PlayerId(0), 0, 1, CommandKind::Resign {})]);
        let first = sim.outcome().expect("ended");
        // Run more ticks — the outcome is cached, no second MatchEnded.
        for _ in 0..5 {
            sim.step(&[]);
        }
        assert_eq!(sim.outcome(), Some(first));
    }

    #[test]
    fn a_one_player_match_does_not_end_at_start() {
        // A single-player setup with no opponent and no defeat must continue
        // (the M3 smoke path's shape — there is nothing to win against yet).
        let world = two_player_world();
        let mut sim = Sim::new(
            &world,
            MatchSetup {
                seed: 7,
                players: vec![PlayerSetup {
                    player: PlayerId(0),
                    controller: ControllerKind::Human,
                }],
            },
        );
        sim.step(&[]);
        assert!(sim.outcome().is_none());
    }

    #[test]
    fn defeat_when_a_player_loses_their_last_structure() {
        // Player 0's structure decays to zero hp on tick 0 and is removed in
        // stage 8; stage 10 then sees zero structures for player 0 and ends
        // the match with player 1 the winner.
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, SpawnDef};
        use pandemonium_fx::Vec2Fx;
        use pandemonium_sim_api::KindId;
        let world = TrivialWorld {
            map_id: 0x004D_3854_4553_5402,
            width_tiles: 32,
            height_tiles: 32,
            passability: TrivialWorld::open_passability(32, 32),
            buildability: TrivialWorld::open_buildability(32, 32),
            kinds: vec![
                KindTemplate {
                    // kind 0: a structure that survives.
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 10,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Footprint { w: 2, h: 2 },
                    ],
                    economy: KindEconomy::default(),
                },
                KindTemplate {
                    // kind 1: a structure that dies on tick 0 (1 hp, -1 regen).
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 1,
                            regen_per_tick: -1,
                        },
                        CapTemplate::Footprint { w: 2, h: 2 },
                    ],
                    economy: KindEconomy::default(),
                },
            ],
            resources: vec![],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(1), // decaying structure
                    pos: Vec2Fx::from_ints(8, 8),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0), // surviving structure
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        };
        let mut sim = Sim::new(&world, two_player_setup());
        sim.step(&[]);
        let outcome = sim.outcome().expect("player 0 lost their last structure");
        assert_eq!(outcome.winner, PlayerId(1));
    }
}
