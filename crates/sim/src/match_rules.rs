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
//! **Missions (ADR-0201 §2a, Phase 1):** stage 10 generalizes over an
//! optional objective set. Mission-less (the default), the evaluator is
//! exactly the elimination rule above — the byte-equivalence test pins the
//! mission-less path to today's behavior (same command stream, same state
//! hash at every tick, same `MatchEnded` outcome). With a mission loaded,
//! stage 10 first resolves the mission's objectives per tick — objective
//! resolutions are monotonic and emit one outward [`Event::ObjectiveState`]
//! each — and then the AND-composition: every non-optional objective
//! completed fires `MatchEnded` with the mission player as winner; any
//! non-optional objective failed fires it with the opponent as winner
//! (Phase 1 kinds never fail — the path exists for Phase 2's protect and
//! survive). The elimination rule still applies either way: a mission player
//! with zero structures is defeated exactly as today. The mission state is
//! derived in the A-066 pattern — a pure function of the hashed world
//! inputs, never part of the canonical hash.
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
//! green by construction — no `STATE_ENCODING_VERSION` bump (A-066). The
//! mission objective state carries the same immunity: it is derived from the
//! same hashed inputs, so toggling a mission on or off cannot move a
//! checkpoint (ADR-0201 §2a).
//!
//! The post-MatchEnded sim keeps stepping (commands still apply, state still
//! evolves); the host's `is_finished()` gate is what stops the windowed
//! client's match loop. The headless AI-vs-AI runner keeps running its tick
//! budget regardless — the M7 "matches complete" contract is "ran the budget
//! under the A12 checker without crashing", not "the match resolved".

use pandemonium_sim_api::{
    Event, KindId, MissionData, MissionObjective, MissionObjectiveKind, MissionObjectiveParameters,
    ObjectiveState, PlayerId,
};

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

/// The live evaluation state of one loaded mission (ADR-0201 §2a): the
/// mission data as resolved at the content seam (the mission player, the
/// ordered objective list, and — carried for construction only — the fog
/// preset, which `Sim::build` consumed before this state existed), plus each
/// objective's resolution. Derived state in the A-066 pattern — a pure
/// function of the hashed world inputs, evaluated per tick, cached once
/// resolved, and never part of the canonical state hash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct MissionState {
    /// The mission data: the mission player plus the ordered objective list.
    data: MissionData,
    /// Each objective's resolution, indexed with the objective list.
    statuses: Vec<ObjectiveStatus>,
}

impl MissionState {
    /// The state a loaded mission starts in: every objective pending.
    pub(crate) fn new(data: MissionData) -> Self {
        let statuses = vec![ObjectiveStatus::Pending; data.objectives.len()];
        Self { data, statuses }
    }
}

/// One objective's resolution, tracked across ticks. Resolutions are
/// monotonic: once an objective completes or fails, it stays resolved — the
/// [`Event::ObjectiveState`] fires exactly once per objective.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ObjectiveStatus {
    /// Not yet resolved.
    Pending,
    /// Resolved complete.
    Completed,
    /// Resolved failed. The failure path ships now (ADR-0201 §2's
    /// AND-composition names it: a failed non-optional objective ends the
    /// match with the opponent as winner) — no Phase 1 objective kind can
    /// produce it yet; the Phase 2 kinds (protect, survive) will.
    #[allow(dead_code)]
    Failed,
}

/// Stage 10 (plan §6.3.10, §9.7): evaluate the match rules.
///
/// Idempotent — once `outcome` is `Some`, this function is a no-op. When the
/// match transitions to ended this tick, exactly one [`Event::MatchEnded`] is
/// pushed onto `events` and `outcome` is set.
///
/// `mission` is the loaded mission's evaluation state, when the match runs
/// one (ADR-0201 §2a); `None` is the mission-less default, and then this
/// function is exactly today's elimination rule — byte-for-byte.
pub(crate) fn evaluate(
    world: &World,
    tick: u32,
    mission: Option<&mut MissionState>,
    outcome: &mut Option<MatchOutcome>,
    events: &mut Vec<Event>,
) {
    if outcome.is_some() {
        return; // match already ended — idempotent (plan §9.7: resolve once).
    }
    // With a mission loaded, stage 10 first resolves objective transitions
    // and the AND-composition (ADR-0201 §2a). When the mission resolves the
    // match this tick, the elimination pass below is skipped — the outcome is
    // set, and the two authorities never double-fire. Objective evaluation
    // reads the world only; the elimination rule still applies afterwards
    // either way, so a mission player who loses their last structure is
    // defeated exactly as today.
    if let Some(mission) = mission {
        evaluate_objectives(world, tick, mission, outcome, events);
        if outcome.is_some() {
            return;
        }
    }
    // The defeat check only fires when the match is "structure-bearing" —
    // at least one player owns at least one structure (A-067). A no-structure
    // world (the M1 spine-test fixture) never triggers defeat; the rule fires
    // only on a real match where someone actually had a structure to lose.
    let any_structure = world
        .entities
        .iter()
        .any(|entity| entity.owner != PlayerId::NEUTRAL && world.footprint_of(entity.id).is_some());
    if !any_structure {
        return;
    }
    // A player is defeated when resigned OR when they own zero structures
    // (Footprint-carrying entities — the data-defined "structure" marker).
    // Ore nodes carry Footprint but are neutral and so never count.
    let mut defeated: Vec<PlayerId> = Vec::new();
    for player in &world.players {
        let has_structures = world
            .entities
            .iter()
            .any(|entity| entity.owner == player.player && world.footprint_of(entity.id).is_some());
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

/// The mission objective evaluation (ADR-0201 §2): per tick, in stage 10,
/// before the elimination rule. Objective resolutions are monotonic and emit
/// exactly one [`Event::ObjectiveState`] each. The AND-composition then
/// decides the match: every non-optional objective completed fires
/// `MatchEnded` with the mission player as winner; any non-optional objective
/// failed fires it with the opponent as winner. Optional objectives never
/// gate the match — they feed the end screen and story hooks; a mission whose
/// objectives are all optional never ends here (the elimination rule still
/// governs it).
fn evaluate_objectives(
    world: &World,
    tick: u32,
    mission: &mut MissionState,
    outcome: &mut Option<MatchOutcome>,
    events: &mut Vec<Event>,
) {
    let player = mission.data.player;
    for (index, objective) in mission.data.objectives.iter().enumerate() {
        if mission.statuses[index] != ObjectiveStatus::Pending {
            continue; // resolutions are monotonic — evaluate each objective once.
        }
        if let Some(status) = objective_status(world, player, objective) {
            mission.statuses[index] = status;
            events.push(Event::ObjectiveState {
                index: index as u32,
                state: match status {
                    ObjectiveStatus::Completed => ObjectiveState::Completed,
                    ObjectiveStatus::Failed => ObjectiveState::Failed,
                    ObjectiveStatus::Pending => unreachable!("pending is not a resolution"),
                },
            });
        }
    }
    // AND-composition over the non-optional objectives.
    let mut required_seen = false;
    let mut all_completed = true;
    let mut any_failed = false;
    for (objective, status) in mission.data.objectives.iter().zip(&mission.statuses) {
        if objective.optional {
            continue;
        }
        required_seen = true;
        match status {
            ObjectiveStatus::Failed => any_failed = true,
            ObjectiveStatus::Pending => all_completed = false,
            ObjectiveStatus::Completed => {}
        }
    }
    if any_failed {
        end_match(opponent_of(world, player), tick, outcome, events);
    } else if required_seen && all_completed {
        end_match(player, tick, outcome, events);
    }
}

/// One objective's current satisfaction from the world state: `Some(status)`
/// when the objective resolves this tick, `None` while it stays pending.
/// Kind/parameters mismatches cannot cross the content seam (the loader
/// rejects them loudly); the defensive arm keeps such a shape pending forever
/// rather than resolving it wrongly.
fn objective_status(
    world: &World,
    player: PlayerId,
    objective: &MissionObjective,
) -> Option<ObjectiveStatus> {
    match (&objective.kind, &objective.parameters) {
        (MissionObjectiveKind::Destroy, MissionObjectiveParameters::Destroy { target_kind }) => {
            destroy_status(world, player, *target_kind)
        }
        (
            MissionObjectiveKind::BuildTrain,
            MissionObjectiveParameters::BuildTrain { kind, count },
        ) => build_train_status(world, player, *kind, *count),
        _ => None,
    }
}

/// Destroy (ADR-0201 §2): all enemy structures gone — or, with a target kind,
/// every enemy structure of that kind. Enemy means a non-neutral owner other
/// than the mission player (ore nodes are neutral and never count, exactly as
/// in the elimination rule); structures are the Footprint-carrying entities,
/// the same data-defined marker the elimination rule reads.
fn destroy_status(
    world: &World,
    player: PlayerId,
    target_kind: Option<KindId>,
) -> Option<ObjectiveStatus> {
    let enemy_target_stands = world.entities.iter().any(|entity| {
        entity.owner != PlayerId::NEUTRAL
            && entity.owner != player
            && world.footprint_of(entity.id).is_some()
            && target_kind.is_none_or(|kind| entity.kind == kind)
    });
    (!enemy_target_stands).then_some(ObjectiveStatus::Completed)
}

/// BuildTrain (ADR-0201 §2): N entities of the kind exist, counted from the
/// world entities owned by the mission player — a structure under
/// construction counts from the moment it exists. The count is current; the
/// resolution is monotonic once it fires.
fn build_train_status(
    world: &World,
    player: PlayerId,
    kind: KindId,
    count: u32,
) -> Option<ObjectiveStatus> {
    let owned = world
        .entities
        .iter()
        .filter(|entity| entity.owner == player && entity.kind == kind)
        .count() as u32;
    (owned >= count).then_some(ObjectiveStatus::Completed)
}

/// Fires the match end exactly once: sets the outcome and emits `MatchEnded`.
fn end_match(
    winner: PlayerId,
    tick: u32,
    outcome: &mut Option<MatchOutcome>,
    events: &mut Vec<Event>,
) {
    *outcome = Some(MatchOutcome {
        winner,
        ended_tick: tick,
    });
    events.push(Event::MatchEnded { winner });
}

/// The mission player's opponent for failure resolution: the first
/// non-neutral player in world order that is not the mission player —
/// deterministic; `PlayerId::NEUTRAL` in the degenerate no-opponent case
/// (mirroring the elimination rule's mutual-destruction sentinel).
fn opponent_of(world: &World, player: PlayerId) -> PlayerId {
    world
        .players
        .iter()
        .map(|p| p.player)
        .find(|p| *p != player && *p != PlayerId::NEUTRAL)
        .unwrap_or(PlayerId::NEUTRAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::TrivialWorld;
    use crate::sim::Sim;
    use pandemonium_sim_api::{
        Command, CommandKind, ControllerKind, FogPreset, MatchSetup, MissionData, MissionObjective,
        MissionObjectiveKind, MissionObjectiveParameters, PlayerId, PlayerSetup,
    };

    /// A mission data builder for the tests (the default fog preset — the
    /// evaluator never reads it; construction consumes it).
    fn mission_for(player: PlayerId, objectives: &[MissionObjective]) -> MissionData {
        MissionData {
            player,
            objectives: objectives.to_vec(),
            fog_preset: FogPreset::Default,
        }
    }

    fn destroy_all(player: PlayerId) -> MissionData {
        mission_for(
            player,
            &[MissionObjective {
                kind: MissionObjectiveKind::Destroy,
                parameters: MissionObjectiveParameters::Destroy { target_kind: None },
                optional: false,
            }],
        )
    }

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

    // -----------------------------------------------------------------------
    // Missions (ADR-0201 §2a, Phase 1)
    // -----------------------------------------------------------------------

    /// The two-player world with player 1's structure decaying: player 1 is
    /// eliminated on tick 0 (the defeat path's mission-shaped twin).
    fn decaying_enemy_world() -> TrivialWorld {
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, SpawnDef};
        use pandemonium_fx::Vec2Fx;
        use pandemonium_sim_api::KindId;
        TrivialWorld {
            map_id: 0x004D_3854_4553_5403,
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
                    kind: KindId(0), // surviving structure
                    pos: Vec2Fx::from_ints(8, 8),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(1), // decaying structure
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    #[test]
    fn a_destroy_mission_resolves_the_match_for_the_mission_player() {
        // Player 1's last structure decays away on tick 0: the mission's
        // destroy objective completes the same tick, the AND-composition
        // fires, and `MatchEnded` names the mission player — the same winner
        // and tick the elimination rule produces for the same world.
        let world = decaying_enemy_world();
        let mut sim = Sim::with_mission(&world, two_player_setup(), destroy_all(PlayerId(0)));
        let out = sim.step(&[]);
        let objective = out
            .events
            .iter()
            .position(|event| matches!(event, Event::ObjectiveState { index: 0, .. }));
        let ended = out
            .events
            .iter()
            .position(|event| matches!(event, Event::MatchEnded { .. }));
        assert!(
            objective.is_some(),
            "the destroy objective resolves: {:?}",
            out.events
        );
        assert!(ended.is_some(), "the match ends: {:?}", out.events);
        assert!(
            objective.unwrap() < ended.unwrap(),
            "the objective-state event precedes MatchEnded in the same step: {:?}",
            out.events
        );
        let outcome = sim.outcome().expect("the mission resolved the match");
        assert_eq!(outcome.winner, PlayerId(0));
        assert_eq!(outcome.ended_tick, 0);
        // And the same world, mission-less, resolves identically through the
        // elimination rule (the same winner at the same tick).
        let mut plain = Sim::new(&world, two_player_setup());
        plain.step(&[]);
        assert_eq!(plain.outcome(), sim.outcome());
    }

    #[test]
    fn objective_state_fires_exactly_once_per_objective() {
        // Resolutions are monotonic: further ticks never re-emit.
        let world = decaying_enemy_world();
        let mut sim = Sim::with_mission(&world, two_player_setup(), destroy_all(PlayerId(0)));
        let mut objective_events = 0;
        let mut ended_events = 0;
        for _ in 0..10 {
            for event in sim.step(&[]).events {
                match event {
                    Event::ObjectiveState { .. } => objective_events += 1,
                    Event::MatchEnded { .. } => ended_events += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(objective_events, 1, "one event per objective, exactly once");
        assert_eq!(ended_events, 1, "MatchEnded fires exactly once");
    }

    #[test]
    fn optional_objectives_complete_but_never_gate_the_match() {
        // The optional build/train objective completes on tick 0 (player 0
        // owns one kind-0 structure); the required destroy objective stays
        // pending (the enemy stands) — so no MatchEnded, ever, from an
        // unresolved required objective with a completed optional one.
        let world = two_player_world();
        let mission = mission_for(
            PlayerId(0),
            &[
                MissionObjective {
                    kind: MissionObjectiveKind::Destroy,
                    parameters: MissionObjectiveParameters::Destroy { target_kind: None },
                    optional: false,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::BuildTrain,
                    parameters: MissionObjectiveParameters::BuildTrain {
                        kind: pandemonium_sim_api::KindId(0),
                        count: 1,
                    },
                    optional: true,
                },
            ],
        );
        let mut sim = Sim::with_mission(&world, two_player_setup(), mission);
        let first = sim.step(&[]);
        assert!(
            first.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 1,
                    state: ObjectiveState::Completed
                }
            )),
            "the optional objective completes: {:?}",
            first.events
        );
        assert!(
            sim.outcome().is_none(),
            "an optional objective never gates the match"
        );
        for _ in 0..300 {
            sim.step(&[]);
        }
        assert!(
            sim.outcome().is_none(),
            "the required objective stays pending"
        );
    }

    #[test]
    fn the_elimination_rule_still_ends_a_loaded_mission_when_the_mission_player_falls() {
        // The mission player's structure decays on tick 0 while their
        // objectives stay pending: the elimination rule — still live under a
        // mission — defeats them, and the opponent wins.
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, SpawnDef};
        use pandemonium_fx::Vec2Fx;
        use pandemonium_sim_api::KindId;
        let world = TrivialWorld {
            map_id: 0x004D_3854_4553_5404,
            width_tiles: 32,
            height_tiles: 32,
            passability: TrivialWorld::open_passability(32, 32),
            buildability: TrivialWorld::open_buildability(32, 32),
            kinds: vec![
                KindTemplate {
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
                    kind: KindId(1), // the mission player's decaying structure
                    pos: Vec2Fx::from_ints(8, 8),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0), // the opponent's surviving structure
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        };
        // The mission's objectives cannot resolve (the enemy stands).
        let mut sim = Sim::with_mission(&world, two_player_setup(), destroy_all(PlayerId(0)));
        sim.step(&[]);
        let outcome = sim.outcome().expect("the elimination rule still fires");
        assert_eq!(outcome.winner, PlayerId(1), "the opponent wins");
    }

    #[test]
    fn a_mission_of_only_optional_objectives_never_ends_on_its_own() {
        // A mission whose every objective is optional cannot resolve the
        // match through the AND-composition — the elimination rule governs,
        // and nothing is eliminated here.
        let world = two_player_world();
        let mission = mission_for(
            PlayerId(0),
            &[MissionObjective {
                kind: MissionObjectiveKind::BuildTrain,
                parameters: MissionObjectiveParameters::BuildTrain {
                    kind: pandemonium_sim_api::KindId(0),
                    count: 99, // never reachable
                },
                optional: true,
            }],
        );
        let mut sim = Sim::with_mission(&world, two_player_setup(), mission);
        for _ in 0..300 {
            sim.step(&[]);
        }
        assert!(
            sim.outcome().is_none(),
            "optional-only missions never self-resolve"
        );
    }

    #[test]
    fn build_train_counts_the_mission_players_entities_of_the_kind() {
        // Player 0 owns one kind-0 structure; the objective asks for two of
        // kind 0 — pending at tick 0, and the count includes only the mission
        // player's entities (player 1's kind-0 structure never counts).
        let world = two_player_world();
        let mission = mission_for(
            PlayerId(0),
            &[MissionObjective {
                kind: MissionObjectiveKind::BuildTrain,
                parameters: MissionObjectiveParameters::BuildTrain {
                    kind: pandemonium_sim_api::KindId(0),
                    count: 2,
                },
                optional: false,
            }],
        );
        let mut sim = Sim::with_mission(&world, two_player_setup(), mission);
        let out = sim.step(&[]);
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::ObjectiveState { .. })),
            "one owned structure does not satisfy a count of two: {:?}",
            out.events
        );
        assert!(sim.outcome().is_none());
    }

    #[test]
    fn a_destroy_target_kind_objective_waits_for_that_kind() {
        // Player 1 owns a surviving kind-0 structure AND a decaying kind-1
        // structure. A Destroy objective aimed at kind 0 stays pending (the
        // enemy's kind-0 structure stands, so the match continues); aimed at
        // kind 1 it completes the moment the kind-1 structure dies and the
        // mission player wins — the target kind governs, not "any structure".
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, SpawnDef};
        use pandemonium_fx::Vec2Fx;
        use pandemonium_sim_api::KindId;
        let world = TrivialWorld {
            map_id: 0x004D_3854_4553_5405,
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
                    kind: KindId(0), // the mission player's surviving structure
                    pos: Vec2Fx::from_ints(8, 8),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0), // the enemy's surviving structure
                    pos: Vec2Fx::from_ints(20, 20),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(1), // the enemy's decaying structure
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        };
        // Aimed at the surviving kind: pending, and nothing ends the match
        // (the enemy still owns a structure, so even elimination holds).
        let aimed_at_survivor = mission_for(
            PlayerId(0),
            &[MissionObjective {
                kind: MissionObjectiveKind::Destroy,
                parameters: MissionObjectiveParameters::Destroy {
                    target_kind: Some(KindId(0)),
                },
                optional: false,
            }],
        );
        let mut sim = Sim::with_mission(&world, two_player_setup(), aimed_at_survivor);
        let out = sim.step(&[]);
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::ObjectiveState { .. })),
            "the enemy's kind-0 structure still stands: {:?}",
            out.events
        );
        assert!(sim.outcome().is_none(), "nobody is eliminated either");
        // Aimed at the decaying kind: it completes on tick 0 and the mission
        // player wins through the AND-composition — even though the enemy
        // still owns a kind-0 structure (elimination would not fire).
        let aimed_at_decay = mission_for(
            PlayerId(0),
            &[MissionObjective {
                kind: MissionObjectiveKind::Destroy,
                parameters: MissionObjectiveParameters::Destroy {
                    target_kind: Some(KindId(1)),
                },
                optional: false,
            }],
        );
        let mut sim = Sim::with_mission(&world, two_player_setup(), aimed_at_decay);
        let out = sim.step(&[]);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 0,
                    state: ObjectiveState::Completed
                }
            )),
            "the enemy's kind-1 structure is gone: {:?}",
            out.events
        );
        assert_eq!(
            sim.outcome().expect("the targeted mission resolves").winner,
            PlayerId(0)
        );
    }

    // -----------------------------------------------------------------------
    // The Phase 1 byte-equivalence proof (ADR-0201 §2a, B-012 Task 4)
    // -----------------------------------------------------------------------

    /// Strips the objective-state family from an event stream — the one
    /// outward difference a mission wrapper may add (FD-9: events are
    /// unhashed outputs).
    fn strip_objective_events(events: &[Event]) -> Vec<Event> {
        events
            .iter()
            .filter(|event| !matches!(event, Event::ObjectiveState { .. }))
            .cloned()
            .collect()
    }

    #[test]
    fn the_mission_less_path_is_byte_equivalent_to_today() {
        // THE proof: the same world, setup, seed, and command stream produce
        // identical state hashes at every tick with and without a mission
        // wrapper — while the mission's evaluation is provably live (its
        // optional objective completes on tick 0 and emits an event). The
        // mission state is derived (the A-066 pattern): it can add outward
        // events, never hashed state.
        let world = two_player_world();
        let setup = two_player_setup();
        // The wrapper: a required destroy objective that stays pending (the
        // enemy stands) plus an optional build/train objective that
        // completes on tick 0 (player 0 owns one kind-0 structure) — the
        // evaluation runs and resolves through the whole window.
        let mission = mission_for(
            PlayerId(0),
            &[
                MissionObjective {
                    kind: MissionObjectiveKind::Destroy,
                    parameters: MissionObjectiveParameters::Destroy { target_kind: None },
                    optional: false,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::BuildTrain,
                    parameters: MissionObjectiveParameters::BuildTrain {
                        kind: pandemonium_sim_api::KindId(0),
                        count: 1,
                    },
                    optional: true,
                },
            ],
        );
        let mut plain = Sim::new(&world, setup.clone());
        let mut wrapped = Sim::with_mission(&world, setup, mission);

        // The same command stream feeds both sims (deterministically rejected
        // — the fixture's structures carry no Move capability; rejections are
        // events, and events are unhashed outputs, so the streams are equal
        // and the gate's behavior is exercised all the same).
        let script: Vec<(u32, Command)> = vec![
            (
                0,
                Command::new(
                    PlayerId(0),
                    0,
                    1,
                    CommandKind::Move {
                        units: vec![pandemonium_sim_api::EntityId(1)],
                        target: pandemonium_fx::Vec2Fx::from_ints(4, 4),
                    },
                ),
            ),
            (
                5,
                Command::new(
                    PlayerId(1),
                    5,
                    1,
                    CommandKind::Move {
                        units: vec![pandemonium_sim_api::EntityId(2)],
                        target: pandemonium_fx::Vec2Fx::from_ints(6, 6),
                    },
                ),
            ),
        ];

        // Tick 0, before any step: identical state by construction.
        assert_eq!(plain.state_hash(), wrapped.state_hash());

        let mut objective_event_seen = false;
        let mut other_match_ended = false;
        for tick in 0..7200u32 {
            let due: Vec<Command> = script
                .iter()
                .filter(|(at, _)| *at == tick)
                .map(|(_, command)| command.clone())
                .collect();
            let plain_out = plain.step(&due);
            let wrapped_out = wrapped.step(&due);
            // Every periodic checkpoint hash (each 30 ticks) is identical.
            assert_eq!(
                plain_out.hash,
                wrapped_out.hash,
                "periodic hash diverged entering tick {}",
                tick + 1
            );
            // The event streams are identical modulo the objective-state
            // family — the only outward difference a mission may add.
            assert_eq!(
                strip_objective_events(&plain_out.events),
                strip_objective_events(&wrapped_out.events),
                "event streams diverged entering tick {}",
                tick + 1
            );
            for event in &wrapped_out.events {
                match event {
                    Event::ObjectiveState { .. } => objective_event_seen = true,
                    Event::MatchEnded { .. } => other_match_ended = true,
                    _ => {}
                }
            }
            // The brief's named ticks, hashed explicitly on top of the
            // periodic trail.
            if tick + 1 == 300 || tick + 1 == 7200 {
                assert_eq!(
                    plain.state_hash(),
                    wrapped.state_hash(),
                    "state hash diverged at tick {}",
                    tick + 1
                );
            }
        }
        // The mission's evaluation was live: the optional objective resolved
        // and surfaced, yet nothing hashed moved.
        assert!(
            objective_event_seen,
            "the wrapper's evaluation ran and emitted"
        );
        // The MatchEnded outcome is identical too: both matches are
        // unresolved at the flagship budget (the enemy stands; nobody
        // resigned; nothing decays).
        assert!(!other_match_ended);
        assert_eq!(plain.outcome(), wrapped.outcome());
        assert_eq!(plain.outcome(), None);
        assert_eq!(plain.tick(), wrapped.tick());
        assert_eq!(plain.next_entity_id(), wrapped.next_entity_id());
    }

    #[test]
    fn the_mission_less_path_stays_byte_equivalent_after_the_match_resolves() {
        // The resolved regime: player 1's structure decays away on tick 0, so
        // the plain sim ends through elimination and the wrapped sim ends
        // through the mission's AND-composition — the same winner at the same
        // tick — and both sims keep stepping their identical post-match
        // states to the same hashes at every tick afterwards.
        let world = decaying_enemy_world();
        let setup = two_player_setup();
        let mut plain = Sim::new(&world, setup.clone());
        let mut wrapped = Sim::with_mission(&world, setup, destroy_all(PlayerId(0)));
        assert_eq!(plain.state_hash(), wrapped.state_hash());
        for tick in 0..7200u32 {
            let plain_out = plain.step(&[]);
            let wrapped_out = wrapped.step(&[]);
            assert_eq!(
                plain_out.hash,
                wrapped_out.hash,
                "periodic hash diverged entering tick {}",
                tick + 1
            );
            assert_eq!(
                strip_objective_events(&plain_out.events),
                strip_objective_events(&wrapped_out.events),
                "event streams diverged entering tick {}",
                tick + 1
            );
        }
        // Identical resolution: same winner, same end tick.
        assert_eq!(plain.outcome(), wrapped.outcome());
        let outcome = plain.outcome().expect("both resolved on tick 0");
        assert_eq!(outcome.winner, PlayerId(0));
        assert_eq!(outcome.ended_tick, 0);
    }
}
