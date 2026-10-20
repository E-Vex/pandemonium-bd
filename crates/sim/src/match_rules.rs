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
//! (Phase 1's kinds never fail; B-017's protect is the fail-only kind, and
//! survive completes when the mission tick reaches its window). The
//! elimination rule still applies either way: a mission player
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
    AreaSpec, Event, GatherSemantics, KindId, MissionObjective, MissionObjectiveKind,
    MissionObjectiveParameters, ObjectiveState, PlayerId, ResourceId,
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
/// objective set as resolved at the content seam, plus each objective's
/// resolution. Derived state in the A-066 pattern — a pure function of the
/// hashed world inputs, evaluated per tick, cached once resolved, and never
/// part of the canonical state hash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct MissionState {
    /// The objective set: the mission player plus the ordered objective list.
    objectives: pandemonium_sim_api::MissionData,
    /// Each objective's resolution, indexed with the objective list.
    statuses: Vec<ObjectiveStatus>,
    /// The mission player's cumulative delivered ore (B-018): the running
    /// total of their `ResourceDelivered` amounts, accumulated by
    /// `accumulate_deliveries` from the delivery events each tick — the
    /// ADR §2's expressible basis for the cumulative gather semantics,
    /// derived (never hashed: the accumulator is a deterministic function
    /// of the match inputs, rebuilt identically in every re-simulation).
    /// Zero for missions with no Cumulative gather objective (the scan
    /// only runs when one is present).
    delivered: i64,
}

impl MissionState {
    /// The state a loaded mission starts in: every objective pending, the
    /// delivered total zero (no deliveries have happened at tick 0).
    pub(crate) fn new(objectives: pandemonium_sim_api::MissionData) -> Self {
        let statuses = vec![ObjectiveStatus::Pending; objectives.objectives.len()];
        Self {
            objectives,
            statuses,
            delivered: 0,
        }
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
    /// Resolved failed. B-017's protect arm constructs it: a failed
    /// non-optional objective ends the match with the opponent as winner
    /// (the AND-composition's failure path, live since Phase 1).
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
        // B-018: the cumulative delivered total first — an immutable read
        // of the events this tick has produced so far (stage 4's
        // `ResourceDelivered`s are in; the evaluator runs at stage 10),
        // before the objective pass takes the vector mutably to push its
        // own events. Accumulating BEFORE the status checks lets a
        // delivery complete its objective on the SAME tick it happens,
        // with `ResourceDelivered` preceding `ObjectiveState` in the
        // outward stream — the causal-order law. Reading the events here
        // is the ADR §2's own expressible basis for the kind
        // ("cumulative ResourceDelivered"); the events still flow
        // outward after the step (FD-9 — nothing flows inward, and no
        // event is consumed).
        accumulate_deliveries(events, mission);
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
    let player = mission.objectives.player;
    for (index, objective) in mission.objectives.objectives.iter().enumerate() {
        if mission.statuses[index] != ObjectiveStatus::Pending {
            continue; // resolutions are monotonic — evaluate each objective once.
        }
        if let Some(status) = objective_status(world, tick, player, objective, mission.delivered) {
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
    for (objective, status) in mission.objectives.objectives.iter().zip(&mission.statuses) {
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

/// Accumulates the mission player's cumulative delivered ore from the
/// events this tick has produced (B-018). No-op unless the mission carries
/// a Cumulative gather objective — the only consumer — so the scan costs
/// nothing for every other mission. The per-event owner field is the
/// attribution (resolved at deposit time, immune to same-tick deaths);
/// the resource match is the objective's own (ore by definition — the
/// schema names no other resource).
fn accumulate_deliveries(events: &[Event], mission: &mut MissionState) {
    let has_cumulative = mission.objectives.objectives.iter().any(|objective| {
        matches!(
            objective.parameters,
            MissionObjectiveParameters::Gather {
                semantics: GatherSemantics::Cumulative,
                ..
            }
        )
    });
    if !has_cumulative {
        return;
    }
    let player = mission.objectives.player;
    let resource = mission
        .objectives
        .objectives
        .iter()
        .find_map(|objective| match objective.parameters {
            MissionObjectiveParameters::Gather { resource, .. } => Some(resource),
            _ => None,
        })
        .expect("a mission with a Cumulative gather objective carries the objective");
    for event in events {
        if let Event::ResourceDelivered {
            owner,
            resource: delivered_resource,
            amount,
            ..
        } = event
        {
            if *owner == player && *delivered_resource == resource {
                mission.delivered += *amount as i64;
            }
        }
    }
}

/// One objective's current satisfaction from the world state: `Some(status)`
/// when the objective resolves this tick, `None` while it stays pending.
/// Kind/parameters mismatches cannot cross the content seam (the loader
/// rejects them loudly); the defensive arm keeps such a shape pending forever
/// rather than resolving it wrongly. `delivered` is the mission player's
/// cumulative delivered ore (B-018) — the accumulator's current value.
fn objective_status(
    world: &World,
    tick: u32,
    player: PlayerId,
    objective: &MissionObjective,
    delivered: i64,
) -> Option<ObjectiveStatus> {
    match (&objective.kind, &objective.parameters) {
        (MissionObjectiveKind::Destroy, MissionObjectiveParameters::Destroy { target_kind }) => {
            destroy_status(world, player, *target_kind)
        }
        (
            MissionObjectiveKind::BuildTrain,
            MissionObjectiveParameters::BuildTrain { kind, count },
        ) => build_train_status(world, player, *kind, *count),
        // B-017: the survive window is a property of the tick counter, not
        // the entity set — the mission starts at tick 0 by construction
        // (mission data rides `Sim::with_mission`), so "counted from mission
        // start" is exactly the stage-10 tick.
        (MissionObjectiveKind::Survive, MissionObjectiveParameters::Survive { ticks }) => {
            survive_status(tick, *ticks)
        }
        // B-017: protect is fail-only — the status is the protected set's
        // emptiness, observed after this tick's cleanup.
        (MissionObjectiveKind::Protect, MissionObjectiveParameters::Protect { kind }) => {
            protect_status(world, player, *kind)
        }
        // B-018: reach completes when any entity of the mission player
        // stands inside the authored area — any entity, unit or structure;
        // the loader's tick-0 gate is what keeps a mission whose starting
        // forces already stand inside from loading (the evaluator itself
        // is total over the entity set, the AUD-003 division).
        (MissionObjectiveKind::ReachArea, MissionObjectiveParameters::ReachArea { area }) => {
            reach_status(world, player, area)
        }
        // B-018: gather crosses when the mission player's ore crosses the
        // authored amount — the ledger (Balance) or the cumulative
        // delivered total (Cumulative), exactly the semantics the mission
        // picked at the seam.
        (
            MissionObjectiveKind::Gather,
            MissionObjectiveParameters::Gather {
                resource,
                amount,
                semantics,
            },
        ) => gather_status(world, player, *resource, *amount, *semantics, delivered),
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

/// Survive (B-017): the mission tick reaches the authored count. The
/// evaluator runs in stage 10 of the tick in progress, so the objective is
/// pending through tick `ticks - 1` and completes on the step whose tick
/// equals `ticks`. The loader's 300-tick floor keeps the window from ever
/// completing vacuously; this arm itself is total over the tick counter and
/// would complete a degenerate 0-window at tick 0 — the loader rejects that
/// shape before it can load (the AUD-003 division: content validates, the
/// evaluator is total).
fn survive_status(tick: u32, ticks: u32) -> Option<ObjectiveStatus> {
    (tick >= ticks).then_some(ObjectiveStatus::Completed)
}

/// Protect (B-017): a fail-only constraint — pending while the mission
/// player owns at least one living entity of the protected kind, failed on
/// the first tick none remains, never completed by itself (victory comes
/// from the other objectives; the loader refuses a mission whose required
/// objectives are all Protect). The evaluator reads the post-cleanup entity
/// set: stage 8 removed this tick's dead and fired their outward `Died`
/// events (FD-9 — the events flow outward, consumed after the step), so
/// "none remains" is observed exactly the tick the last one died, in the
/// same step as the `Died` event that carries the cause.
fn protect_status(world: &World, player: PlayerId, kind: KindId) -> Option<ObjectiveStatus> {
    let protected_stands = world
        .entities
        .iter()
        .any(|entity| entity.owner == player && entity.kind == kind);
    (!protected_stands).then_some(ObjectiveStatus::Failed)
}

/// Reach (B-018): pending until any entity of the mission player stands
/// inside the area, completed the first tick one does — the status is the
/// containment predicate over the entity set, the same `AreaSpec::contains`
/// the loader's tick-0 gate runs (one law, two call sites). The entity set
/// here is the post-stage-3 set (this tick's spawns are in), so an entity
/// spawned inside the area completes the objective on its spawn tick, with
/// `Spawned` preceding `ObjectiveState` in the same step's stream (the
/// causal-order law the protect arm pins for `Died`).
fn reach_status(world: &World, player: PlayerId, area: &AreaSpec) -> Option<ObjectiveStatus> {
    let inside = world
        .entities
        .iter()
        .any(|entity| entity.owner == player && area.contains(entity.pos));
    inside.then_some(ObjectiveStatus::Completed)
}

/// Gather (B-018): pending until the mission player's ore crosses the
/// authored amount, completed the first tick it does — the semantics the
/// mission picked at the seam decides WHICH total: `Balance` reads the
/// hashed ledger (spending sets it back below the target while the
/// objective is still pending; the resolution itself is monotone once
/// fired, like every objective), `Cumulative` reads the event-derived
/// accumulator (spending never touches it). The evaluator is total over
/// both — the loader's amount gate (above the starting ore, both
/// semantics) is what keeps a mission from demanding nothing (the
/// AUD-003 division).
fn gather_status(
    world: &World,
    player: PlayerId,
    resource: ResourceId,
    amount: i64,
    semantics: GatherSemantics,
    delivered: i64,
) -> Option<ObjectiveStatus> {
    let current = match semantics {
        GatherSemantics::Balance => world
            .player(player)
            .map(|state| state.balance(resource))
            .unwrap_or(0),
        GatherSemantics::Cumulative => delivered,
    };
    (current >= amount).then_some(ObjectiveStatus::Completed)
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
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::{
        AiSettings, Command, CommandKind, ControllerKind, FogPreset, MatchSetup, MissionData,
        MissionObjective, MissionObjectiveKind, MissionObjectiveParameters, PlayerId, PlayerSetup,
    };

    // Re-exported at the sim root for the test bodies (fixture types).
    use crate::fixture::SpawnDef;
    use pandemonium_sim_api::{EntityId, KindId, Vec2Fx};

    /// A mission objective set builder for the tests.
    fn mission_for(player: PlayerId, objectives: &[MissionObjective]) -> MissionData {
        MissionData {
            player,
            objectives: objectives.to_vec(),
            // The fog preset only matters to the fog cache's construction;
            // the objective-evaluation tests run the Default initialization.
            fog_preset: FogPreset::Default,
            // The AI tuning (B-021) is carried, never read by the sim — the
            // objective-evaluation tests carry the all-None default.
            ai_settings: AiSettings::default(),
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
            roster: Vec::new(),
        }
        .with_default_roster()
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
            roster: Vec::new(),
        }
        .with_default_roster();
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
            roster: Vec::new(),
        }
        .with_default_roster()
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

    // -----------------------------------------------------------------------
    // Survive (B-017): the mission tick reaches the authored window
    // -----------------------------------------------------------------------

    /// A survive objective with a small window. The loader's 300-tick floor
    /// is a content-side gate (AUD-003); the evaluator itself is total over
    /// the tick counter, so the sim-level semantics test uses a tight window
    /// to pin the exact boundary tick. The acceptance-scale window (300) is
    /// exercised against the real content in `tests/mission_phase1.rs`.
    fn survive_mission(player: PlayerId, ticks: u32, optional: bool) -> MissionData {
        mission_for(
            player,
            &[MissionObjective {
                kind: MissionObjectiveKind::Survive,
                parameters: MissionObjectiveParameters::Survive { ticks },
                optional,
            }],
        )
    }

    #[test]
    fn a_survive_objective_is_pending_through_its_window_and_completes_at_its_tick() {
        // The binding boundary: pending at tick `ticks - 1`, complete at
        // tick `ticks` — the step whose stage-10 evaluation carries that
        // tick. A required survive as the only objective ends the match for
        // the mission player the same tick.
        let world = two_player_world();
        let setup = two_player_setup();
        let mut sim = Sim::with_mission(
            &world,
            setup.clone(),
            survive_mission(PlayerId(0), 3, false),
        );
        // Ticks 0, 1, 2 — all strictly inside the window: pending, no event,
        // no outcome, and the mission state never touches the hash (A-066).
        let mut plain = Sim::new(&world, setup);
        for _ in 0..3 {
            let out = sim.step(&[]);
            plain.step(&[]);
            assert!(
                !out.events
                    .iter()
                    .any(|event| matches!(event, Event::ObjectiveState { .. })),
                "pending through tick 2: {:?}",
                out.events
            );
            assert!(sim.outcome().is_none(), "the match runs through the window");
            assert_eq!(sim.state_hash(), plain.state_hash());
        }
        // Tick 3 — the boundary: the objective completes and the match ends
        // with the mission player the winner (the only required objective).
        let out = sim.step(&[]);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 0,
                    state: ObjectiveState::Completed
                }
            )),
            "the survive objective completes at tick 3: {:?}",
            out.events
        );
        let outcome = sim
            .outcome()
            .expect("the survive window resolves the match");
        assert_eq!(outcome.winner, PlayerId(0));
        assert_eq!(outcome.ended_tick, 3);
        // The objective-state event precedes MatchEnded in the same step.
        let objective = out
            .events
            .iter()
            .position(|event| matches!(event, Event::ObjectiveState { .. }))
            .expect("the objective resolved");
        let ended = out
            .events
            .iter()
            .position(|event| matches!(event, Event::MatchEnded { .. }))
            .expect("the match ended");
        assert!(objective < ended, "{:?}", out.events);
        // And the resolution is monotonic: further ticks never re-emit.
        let mut repeats = 0;
        for _ in 0..5 {
            for event in sim.step(&[]).events {
                if matches!(
                    event,
                    Event::ObjectiveState { .. } | Event::MatchEnded { .. }
                ) {
                    repeats += 1;
                }
            }
        }
        assert_eq!(repeats, 0, "each resolution fires exactly once");
    }

    #[test]
    fn an_optional_survive_objective_completes_but_never_gates_the_match() {
        // The optional law applies to survive exactly as to destroy and
        // build/train: the objective completes at its window, the match
        // keeps running (nothing else resolves it here).
        let world = two_player_world();
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            survive_mission(PlayerId(0), 2, true),
        );
        for _ in 0..2 {
            sim.step(&[]);
        }
        let out = sim.step(&[]);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 0,
                    state: ObjectiveState::Completed
                }
            )),
            "the optional survive objective completes: {:?}",
            out.events
        );
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::MatchEnded { .. })),
            "an optional objective never ends the match"
        );
        assert!(sim.outcome().is_none());
        for _ in 0..50 {
            sim.step(&[]);
        }
        assert!(sim.outcome().is_none(), "the match runs on");
    }

    // -----------------------------------------------------------------------
    // Protect (B-017): the fail-only constraint
    // -----------------------------------------------------------------------

    /// A protect objective over the protected kind.
    fn protect_mission(player: PlayerId, kind: KindId, optional: bool) -> MissionData {
        mission_for(
            player,
            &[MissionObjective {
                kind: MissionObjectiveKind::Protect,
                parameters: MissionObjectiveParameters::Protect { kind },
                optional,
            }],
        )
    }

    /// The protect fixture: kind 0 is a surviving structure (10 hp, no
    /// decay); kind 1 is a decaying unit (41 hp, -1 per tick, no Footprint —
    /// protect guards any owned entity kind, not just structures) that dies
    /// exactly at tick 40 (decay starts at tick 0, so H hp lasts H-1 ticks —
    /// the 1-hp decaying command_center fixture dies at tick 0 by the same
    /// arithmetic). Player 0 (the mission player) owns one of each; player 1
    /// owns a surviving structure.
    fn protect_world() -> TrivialWorld {
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, SpawnDef};
        use pandemonium_fx::Vec2Fx;
        TrivialWorld {
            map_id: 0x004D_3854_4553_5407,
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
                    // kind 1: a decaying unit — 41 hp, -1 per tick, hp
                    // reaches zero in tick 40's cleanup.
                    caps: vec![CapTemplate::Health {
                        max_hp: 41,
                        regen_per_tick: -1,
                    }],
                    economy: KindEconomy::default(),
                },
            ],
            resources: vec![],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![
                // The mission player's surviving structure.
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(8, 8),
                },
                // The mission player's protected unit — dies at tick 40.
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(1),
                    pos: Vec2Fx::from_ints(10, 10),
                },
                // The opponent's surviving structure.
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
            roster: Vec::new(),
        }
        .with_default_roster()
    }

    #[test]
    fn a_required_protect_objective_fails_the_match_when_the_last_of_the_kind_dies() {
        // The binding semantics: pending while the mission player owns a
        // living entity of the kind (ticks 0..39), failed on the first tick
        // none remains (tick 40 — the same step as the `Died` event that
        // carries the cause, FD-9), and a required failure ends the match
        // with the opponent as winner through the same path other required
        // failures use — while the mission player's structure still stands
        // (NOT the elimination rule).
        let world = protect_world();
        let setup = two_player_setup();
        let mut sim = Sim::with_mission(
            &world,
            setup.clone(),
            protect_mission(PlayerId(0), KindId(1), false),
        );
        let mut plain = Sim::new(&world, setup);
        for tick in 0..40 {
            let out = sim.step(&[]);
            plain.step(&[]);
            assert!(
                !out.events.iter().any(|event| matches!(
                    event,
                    Event::ObjectiveState { .. } | Event::MatchEnded { .. }
                )),
                "tick {tick}: pending while the protected unit lives: {:?}",
                out.events
            );
            assert!(sim.outcome().is_none(), "tick {tick}: the match runs");
            assert_eq!(
                sim.state_hash(),
                plain.state_hash(),
                "A-066 through tick {tick}"
            );
        }
        // Tick 40: the unit's health reaches zero, stage 8 removes it and
        // fires `Died`, and stage 10 fails the objective — the death event
        // precedes the objective-state event in the same step's stream.
        let out = sim.step(&[]);
        let died = out
            .events
            .iter()
            .position(|event| matches!(event, Event::Died { .. }))
            .expect("the protected unit died this tick");
        let failed = out
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    Event::ObjectiveState {
                        index: 0,
                        state: ObjectiveState::Failed
                    }
                )
            })
            .expect("the protect objective fails the tick the last one died");
        let ended = out
            .events
            .iter()
            .position(|event| matches!(event, Event::MatchEnded { .. }))
            .expect("a required failure ends the match");
        assert!(died < failed, "Died precedes the failure: {:?}", out.events);
        assert!(
            failed < ended,
            "the failure precedes MatchEnded: {:?}",
            out.events
        );
        let outcome = sim.outcome().expect("the failure resolved the match");
        assert_eq!(
            outcome.winner,
            PlayerId(1),
            "the opponent wins a failed protect"
        );
        assert_eq!(outcome.ended_tick, 40);
        // The mission player was NOT eliminated — their structure stands;
        // the match ended through the failure path, not the defeat path.
        assert!(
            sim.snapshot()
                .entities
                .iter()
                .any(|entity| entity.owner == PlayerId(0) && entity.kind == KindId(0)),
            "the mission player's structure still stands"
        );
        // The contrast that proves the seam: the same world mission-less
        // keeps running (the unit's death defeats no one).
        assert!(plain.outcome().is_none());
        // And the resolution is monotonic — no re-emission after the end.
        let mut repeats = 0;
        for _ in 0..5 {
            for event in sim.step(&[]).events {
                if matches!(
                    event,
                    Event::ObjectiveState { .. } | Event::MatchEnded { .. }
                ) {
                    repeats += 1;
                }
            }
        }
        assert_eq!(repeats, 0);
    }

    #[test]
    fn an_optional_protect_failure_never_ends_the_match() {
        // The optional law: an optional protect failing fires its
        // ObjectiveState::Failed event (the end screen's story hook) but
        // never gates MatchEnded — the match runs on under the elimination
        // rule, and no one here is eliminated.
        let world = protect_world();
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            protect_mission(PlayerId(0), KindId(1), true),
        );
        for _ in 0..40 {
            sim.step(&[]);
        }
        let out = sim.step(&[]);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 0,
                    state: ObjectiveState::Failed
                }
            )),
            "the optional protect objective fails: {:?}",
            out.events
        );
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::MatchEnded { .. })),
            "an optional protect failure never ends the match"
        );
        assert!(sim.outcome().is_none());
        for _ in 0..50 {
            sim.step(&[]);
        }
        assert!(sim.outcome().is_none(), "the match runs on");
    }

    #[test]
    fn a_protect_objective_never_completes_by_itself() {
        // Fail-only by construction: while the protected kind stands, the
        // status is pending — protect alone can never fire MatchEnded with
        // the mission player as winner (the loader separately refuses a
        // mission whose required objectives are all Protect, so this shape
        // can only arrive through optional objectives or a completable
        // required sibling).
        let world = two_player_world();
        // Protect the mission player's own kind-0 structure, which stands
        // forever in this fixture — with an optional flag so the mission
        // loads even though it is the only objective (sim-level data has no
        // loader; the composition law is what is under test).
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            protect_mission(PlayerId(0), KindId(0), true),
        );
        for _ in 0..300 {
            let out = sim.step(&[]);
            assert!(
                !out.events.iter().any(|event| matches!(
                    event,
                    Event::ObjectiveState { .. } | Event::MatchEnded { .. }
                )),
                "a standing protected kind keeps the objective pending, never completed: {:?}",
                out.events
            );
        }
        assert!(sim.outcome().is_none());
    }

    /// A reach mission over one area, required or optional.
    fn reach_mission(player: PlayerId, area: AreaSpec, optional: bool) -> MissionData {
        mission_for(
            player,
            &[MissionObjective {
                kind: MissionObjectiveKind::ReachArea,
                parameters: MissionObjectiveParameters::ReachArea { area },
                optional,
            }],
        )
    }

    /// The reach fixture: player 0 (the mission player) owns a surviving
    /// structure and one worker standing OUTSIDE the area; a second worker
    /// spawns INSIDE the area at tick 20 (the scheduled-spawn stagger — the
    /// completion tick is exact without movement timing). Player 1 owns a
    /// surviving structure (the elimination rule's "other side" stays
    /// standing, so only the reach objective can resolve the match).
    /// Kind 0 is a 2x2 structure; kind 1 is a worker.
    fn reach_world(worker_spawn: SpawnDef) -> TrivialWorld {
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, ScheduledSpawnDef};
        use pandemonium_sim_api::KindId;
        let kinds = vec![
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
                caps: vec![CapTemplate::Health {
                    max_hp: 40,
                    regen_per_tick: 0,
                }],
                economy: KindEconomy::default(),
            },
        ];
        TrivialWorld {
            map_id: 0x0052_4541_4348_4230,
            width_tiles: 32,
            height_tiles: 32,
            passability: TrivialWorld::open_passability(32, 32),
            buildability: TrivialWorld::open_buildability(32, 32),
            kinds,
            resources: vec![],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![
                // The mission player's surviving structure — far outside.
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(4, 4),
                },
                // The mission player's worker — outside the pinned areas.
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(1),
                    pos: Vec2Fx::from_ints(2, 2),
                },
                // The opponent's surviving structure.
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![ScheduledSpawnDef {
                tick: 20,
                spawn: worker_spawn,
            }],
            spawn_jitter_milli: 0,
            roster: Vec::new(),
        }
        .with_default_roster()
    }

    #[test]
    fn a_reach_objective_is_pending_until_an_entity_stands_inside_and_completes_then() {
        // B-018's evaluator law, both shapes: pending while every mission
        // player entity stands outside, completed the first tick one stands
        // inside — here the tick the scheduled worker spawns inside (stage 3
        // precedes stage 10, so `Spawned` precedes `ObjectiveState` in the
        // same step's stream, the causal-order law), then the required
        // completion ends the match for the mission player with the
        // opponent's structure still standing (NOT elimination).
        for area in [
            AreaSpec::Rect {
                x: 10,
                y: 10,
                w: 4,
                h: 4,
            },
            AreaSpec::Circle {
                cx: 12000,
                cy: 12000,
                radius: 2000,
            },
        ] {
            let inside_pos = match area {
                AreaSpec::Rect { x, y, .. } => Vec2Fx::from_ints(x + 2, y + 2),
                AreaSpec::Circle { cx, cy, .. } => {
                    Vec2Fx::new(Fx::from_milli(cx), Fx::from_milli(cy))
                }
            };
            let world = reach_world(SpawnDef {
                owner: PlayerId(0),
                kind: KindId(1),
                pos: inside_pos,
            });
            let setup = two_player_setup();
            let mut plain = Sim::new(&world, setup.clone());
            let mut sim = Sim::with_mission(&world, setup, reach_mission(PlayerId(0), area, false));

            // Ticks 0..19 — the second worker has not spawned; nobody
            // stands inside: pending, no events, hashes byte-identical.
            for _ in 0..20 {
                let out = sim.step(&[]);
                plain.step(&[]);
                assert!(
                    !out.events.iter().any(|event| matches!(
                        event,
                        Event::ObjectiveState { .. } | Event::MatchEnded { .. }
                    )),
                    "pending while nobody stands inside: {:?}",
                    out.events
                );
            }
            assert_eq!(sim.state_hash(), plain.state_hash(), "tick 20 approach");

            // Tick 20 — the scheduled worker spawns inside; the objective
            // completes and the mission player wins, with Spawned before
            // ObjectiveState before MatchEnded in one step's stream.
            let out = sim.step(&[]);
            plain.step(&[]);
            let spawned_at = out
                .events
                .iter()
                .position(|event| matches!(event, Event::Spawned { .. }))
                .expect("the worker spawned this tick");
            let resolved_at = out
                .events
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        Event::ObjectiveState {
                            index: 0,
                            state: ObjectiveState::Completed
                        }
                    )
                })
                .expect("the objective resolved this tick");
            let ended_at = out
                .events
                .iter()
                .position(|event| matches!(event, Event::MatchEnded { .. }))
                .expect("the match ended this tick");
            assert!(
                spawned_at < resolved_at && resolved_at < ended_at,
                "Spawned precedes ObjectiveState precedes MatchEnded: {:?}",
                out.events
            );
            let outcome = sim.outcome().expect("the match resolved");
            assert_eq!(outcome.winner, PlayerId(0));
            assert_eq!(outcome.ended_tick, 20);
            // The mission state is derived (A-066): the hashes agree even
            // on the resolving tick.
            assert_eq!(sim.state_hash(), plain.state_hash(), "tick 20");
        }
    }

    #[test]
    fn an_optional_reach_objective_completes_but_never_gates_the_match() {
        // The optional law: the completion fires its event and nothing
        // else — the match runs on (the elimination rule alone can end it,
        // and nothing dies in this fixture).
        let area = AreaSpec::Rect {
            x: 10,
            y: 10,
            w: 4,
            h: 4,
        };
        let world = reach_world(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(1),
            pos: Vec2Fx::from_ints(12, 12),
        });
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            reach_mission(PlayerId(0), area, true),
        );
        for _ in 0..20 {
            let out = sim.step(&[]);
            assert!(
                !out.events.iter().any(|event| matches!(
                    event,
                    Event::ObjectiveState { .. } | Event::MatchEnded { .. }
                )),
                "pending until the spawn: {:?}",
                out.events
            );
        }
        let out = sim.step(&[]);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 0,
                    state: ObjectiveState::Completed
                }
            )),
            "the optional objective completes: {:?}",
            out.events
        );
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::MatchEnded { .. })),
            "an optional completion never ends the match"
        );
        assert!(sim.outcome().is_none());
        for _ in 0..30 {
            sim.step(&[]);
        }
        assert!(sim.outcome().is_none(), "the match runs on");
    }

    #[test]
    fn a_reach_objective_pins_the_exact_boundaries() {
        // The boundary laws, evaluated through the evaluator (the same
        // predicate the loader's tick-0 gate runs — the sim_api unit tests
        // pin the geometry directly; these pin the evaluator's view):
        // exactly on the rect's right edge is OUTSIDE (pending forever),
        // one raw unit shy of it is INSIDE (completes at tick 0 — the
        // evaluator is total; the loader's gate keeps this shape out of
        // real content, the AUD-003 division); exactly on the circle's
        // boundary is INSIDE.
        // A worker exactly ON the rect's right edge: pending.
        let on_edge = Vec2Fx::new(Fx::from_int(10 + 4), Fx::from_int(12));
        let mut world = reach_world(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(1),
            pos: Vec2Fx::from_ints(30, 30),
        });
        // Replace the standing worker's position with the probe point by
        // adding it as a scheduled spawn at tick 5 (initial spawns are
        // fixed; the probe worker arrives mid-match at an exact position).
        use crate::fixture::ScheduledSpawnDef;
        world.scheduled_spawns.push(ScheduledSpawnDef {
            tick: 5,
            spawn: SpawnDef {
                owner: PlayerId(0),
                kind: KindId(1),
                pos: on_edge,
            },
        });
        let area = AreaSpec::Rect {
            x: 10,
            y: 10,
            w: 4,
            h: 4,
        };
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            reach_mission(PlayerId(0), area, true),
        );
        for _ in 0..30 {
            let out = sim.step(&[]);
            assert!(
                !out.events
                    .iter()
                    .any(|event| matches!(event, Event::ObjectiveState { .. })),
                "a worker exactly on the right edge is outside — pending: {:?}",
                out.events
            );
        }
        // One raw unit shy of the edge: inside — completes on its spawn
        // tick.
        let shy = Vec2Fx::new(Fx::from_raw(14 * 65_536 - 1), Fx::from_int(12));
        let mut world = reach_world(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(1),
            pos: Vec2Fx::from_ints(30, 30),
        });
        world.scheduled_spawns[0].spawn.pos = shy;
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            reach_mission(PlayerId(0), area, true),
        );
        for _ in 0..20 {
            sim.step(&[]);
        }
        let out = sim.step(&[]);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 0,
                    state: ObjectiveState::Completed
                }
            )),
            "one raw unit shy of the right edge is inside — completed: {:?}",
            out.events
        );
        // Exactly on the circle's boundary (dist == radius): inside —
        // completes on its spawn tick, the boundary-inclusive law.
        let circle = AreaSpec::Circle {
            cx: 10000,
            cy: 10000,
            radius: 2000,
        };
        let on_circle = Vec2Fx::new(Fx::from_milli(12000), Fx::from_milli(10000));
        let world = reach_world(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(1),
            pos: on_circle,
        });
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            reach_mission(PlayerId(0), circle, true),
        );
        for _ in 0..20 {
            let out = sim.step(&[]);
            assert!(
                !out.events
                    .iter()
                    .any(|event| matches!(event, Event::ObjectiveState { .. })),
                "pending until the boundary worker spawns: {:?}",
                out.events
            );
        }
        let out = sim.step(&[]);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState {
                    index: 0,
                    state: ObjectiveState::Completed
                }
            )),
            "a worker exactly on the circle's boundary is inside — completed: {:?}",
            out.events
        );
    }

    /// A gather mission over one amount and semantics, required or optional.
    fn gather_mission(
        player: PlayerId,
        amount: i64,
        semantics: GatherSemantics,
        optional: bool,
    ) -> MissionData {
        mission_for(
            player,
            &[MissionObjective {
                kind: MissionObjectiveKind::Gather,
                parameters: MissionObjectiveParameters::Gather {
                    resource: ResourceId(0),
                    amount,
                    semantics,
                },
                optional,
            }],
        )
    }

    /// The gather fixture (the economy test module's shape, two-player):
    /// player 0 (the mission player) owns a worker, a depot that both
    /// stores ore and trains workers (cost 50, the spending path), and
    /// nothing else; player 1 owns a surviving depot (the elimination
    /// rule's "other side"). One rich neutral ore node (500 ore, 10 per
    /// trip, a 30-tick gather cycle) feeds the loop. Kinds: 0 worker,
    /// 1 depot, 2 ore node.
    fn gather_world() -> TrivialWorld {
        use crate::fixture::{CapTemplate, KindEconomy, KindTemplate};
        TrivialWorld {
            map_id: 0x0047_4154_4845_5230,
            width_tiles: 32,
            height_tiles: 32,
            passability: TrivialWorld::open_passability(32, 32),
            buildability: TrivialWorld::open_buildability(32, 32),
            kinds: vec![
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 40,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Move {
                            speed_milli_tiles_per_s: 2600,
                            radius_milli_tiles: 300,
                        },
                        CapTemplate::Gather {
                            carry_amount: 10,
                            gather_time_ms: 1000,
                        },
                        CapTemplate::Vision {
                            radius_milli_tiles: 7000,
                        },
                    ],
                    economy: KindEconomy {
                        cost: vec![(ResourceId(0), 50)],
                        build_time_ticks: 30,
                        population: 1,
                        requires: vec![],
                    },
                },
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 300,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Vision {
                            radius_milli_tiles: 6000,
                        },
                        CapTemplate::Footprint { w: 2, h: 2 },
                        CapTemplate::Storage {
                            resources: vec![ResourceId(0)],
                        },
                        CapTemplate::ProvidesPopulation { amount: 10 },
                        CapTemplate::Produce {},
                    ],
                    economy: KindEconomy::default(),
                },
                KindTemplate {
                    caps: vec![
                        CapTemplate::Resource {
                            resource: ResourceId(0),
                            amount: 500,
                        },
                        CapTemplate::Footprint { w: 2, h: 2 },
                    ],
                    economy: KindEconomy::default(),
                },
            ],
            resources: vec![crate::fixture::ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![(KindId(1), vec![KindId(0)])],
            base_population_cap: 0,
            initial_spawns: vec![
                // Player 0: the worker and the depot (storage + producer).
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(4, 4),
                },
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(1),
                    pos: Vec2Fx::from_ints(8, 3),
                },
                // Player 1: a surviving structure (the other side).
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(1),
                    pos: Vec2Fx::from_ints(24, 24),
                },
                // The rich neutral node.
                SpawnDef {
                    owner: PlayerId::NEUTRAL,
                    kind: KindId(2),
                    pos: Vec2Fx::from_ints(4, 10),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
            roster: Vec::new(),
        }
        .with_default_roster()
    }

    /// Steps the gather fixture until the mission player's worker has made
    /// `wanted` deliveries (each 10 ore), returning the tick they landed
    /// on. The worker is ordered to gather from the node at tick 0 and
    /// never re-commanded (auto-seek retargets after depletion).
    fn run_until_deliveries(sim: &mut Sim, wanted: usize, seq: &mut u32) -> u32 {
        let node = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.kind == KindId(2))
            .expect("the ore node stands")
            .id;
        sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            {
                *seq += 1;
                *seq
            },
            CommandKind::Gather {
                units: vec![EntityId(1)],
                node,
            },
        )]);
        let mut delivered = 0usize;
        let mut guard = 0u32;
        while delivered < wanted && guard < 4000 {
            let out = sim.step(&[]);
            for event in &out.events {
                if let Event::ResourceDelivered { owner, .. } = event {
                    if *owner == PlayerId(0) {
                        delivered += 1;
                    }
                }
            }
            guard += 1;
        }
        assert!(
            delivered >= wanted,
            "only {delivered} deliveries by tick {guard}"
        );
        sim.tick() - 1
    }

    #[test]
    fn a_cumulative_gather_objective_completes_on_the_delivery_tick() {
        // B-018's cumulative law: the objective completes on EXACTLY the
        // tick the mission player's Nth delivery lands (the accumulator
        // runs before the status check, inside the same step — stage 4
        // produced the `ResourceDelivered`, stage 10 consumes it), so the
        // outward stream carries `ResourceDelivered` before
        // `ObjectiveState` in one step — the causal-order law — and the
        // mission-less twin, driven with the SAME command stream in
        // lockstep, hashes identically at every checked tick (A-066: the
        // accumulator is derived, never hashed).
        let world = gather_world();
        let setup = two_player_setup();
        let mut plain = Sim::new(&world, setup.clone());
        let mut sim = Sim::with_mission(
            &world,
            setup,
            gather_mission(PlayerId(0), 15, GatherSemantics::Cumulative, false),
        );
        // The same gather order drives both sims; the worker (entity 1)
        // gathers the rich node from tick 0.
        let node = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.kind == KindId(2))
            .expect("the ore node stands")
            .id;
        let order = Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Gather {
                units: vec![EntityId(1)],
                node,
            },
        );
        sim.step(std::slice::from_ref(&order));
        plain.step(&[order]);
        let mut delivered: Vec<u32> = Vec::new();
        for tick in 1..4000u32 {
            let out = sim.step(&[]);
            plain.step(&[]);
            for event in &out.events {
                if let Event::ResourceDelivered { owner, amount, .. } = event {
                    if *owner == PlayerId(0) {
                        assert_eq!(*amount, 10, "one trip carries 10");
                        delivered.push(tick);
                    }
                }
            }
            // One delivery (10) is below 15: pending.
            if delivered.len() == 1 {
                assert!(sim.outcome().is_none(), "10 delivered is below 15");
            }
            if delivered.len() == 2 {
                // The second delivery (20 >= 15) completed the objective
                // THIS tick, with ResourceDelivered before ObjectiveState
                // in the same step's stream (the causal order).
                let delivered_at = out
                    .events
                    .iter()
                    .position(|event| matches!(event, Event::ResourceDelivered { .. }))
                    .expect("the delivery is in this step's stream");
                let resolved_at = out
                    .events
                    .iter()
                    .position(|event| {
                        matches!(
                            event,
                            Event::ObjectiveState {
                                index: 0,
                                state: ObjectiveState::Completed
                            }
                        )
                    })
                    .expect("the objective resolved this tick");
                let ended_at = out
                    .events
                    .iter()
                    .position(|event| matches!(event, Event::MatchEnded { .. }))
                    .expect("the match ended this tick");
                assert!(
                    delivered_at < resolved_at && resolved_at < ended_at,
                    "ResourceDelivered precedes ObjectiveState precedes MatchEnded: {:?}",
                    out.events
                );
                let outcome = sim.outcome().expect("the objective completed");
                assert_eq!(outcome.winner, PlayerId(0));
                assert_eq!(outcome.ended_tick, tick);
                break;
            }
            if tick.is_multiple_of(100) {
                assert_eq!(
                    sim.state_hash(),
                    plain.state_hash(),
                    "tick {tick}: the accumulator is derived, never hashed"
                );
            }
        }
        assert_eq!(delivered.len(), 2, "the loop ran to the second delivery");
        assert_eq!(
            sim.state_hash(),
            plain.state_hash(),
            "the resolving tick itself: identical hashes"
        );
    }

    #[test]
    fn a_balance_gather_objective_completes_when_the_ledger_crosses() {
        // B-018's balance law: the objective completes the tick the
        // mission player's LEDGER crosses the amount (200 starting + 10
        // delivered = 210 >= 205) — the same tick as the delivery, the
        // same event order.
        let world = gather_world();
        let mut sim = Sim::with_mission(
            &world,
            two_player_setup(),
            gather_mission(PlayerId(0), 205, GatherSemantics::Balance, false),
        );
        let mut seq = 0;
        let resolving = run_until_deliveries(&mut sim, 1, &mut seq);
        let outcome = sim.outcome().expect("the objective completed");
        assert_eq!(outcome.winner, PlayerId(0));
        assert_eq!(outcome.ended_tick, resolving);
    }

    #[test]
    fn spending_reduces_the_balance_semantics_but_not_the_cumulative() {
        // The differential — the brief's acceptance: the SAME command
        // script (gather, gather, TRAIN a 50-ore worker, keep gathering),
        // two missions. Cumulative 25 completes on the third delivery
        // regardless of the spend; Balance 225 stays pending while the
        // spend holds the ledger down, completing only six deliveries
        // later when the balance actually holds the amount again. The
        // balance is read from the ledger (hashed state); the cumulative
        // from the delivery events (derived accumulator) — the two
        // semantics, side by side.
        let script = |sim: &mut Sim| {
            let mut seq = 0;
            // Two deliveries: balance 220, delivered 20.
            run_until_deliveries(sim, 2, &mut seq);
            // Spend 50 on a trained worker: balance 170.
            let depot = sim
                .snapshot()
                .entities
                .iter()
                .find(|entity| entity.owner == PlayerId(0) && entity.kind == KindId(1))
                .expect("the depot stands")
                .id;
            sim.step(&[Command::new(
                PlayerId(0),
                sim.tick(),
                {
                    seq += 1;
                    seq
                },
                CommandKind::Train {
                    producer: depot,
                    unit: KindId(0),
                },
            )]);
            seq
        };
        // Cumulative 25: the third delivery (30 >= 25) completes it — the
        // spend never reduces the delivered total (the trained worker
        // stands idle at the depot; only the original gathers).
        let mut cumulative = Sim::with_mission(
            &gather_world(),
            two_player_setup(),
            gather_mission(PlayerId(0), 25, GatherSemantics::Cumulative, false),
        );
        let mut seq = script(&mut cumulative);
        let resolving = run_until_deliveries(&mut cumulative, 1, &mut seq);
        let outcome = cumulative
            .outcome()
            .expect("the cumulative objective completed");
        assert_eq!(outcome.ended_tick, resolving);
        // Balance 225: after the spend the ledger holds 170; six more
        // deliveries (170 + 60 = 230 >= 225) are what it takes.
        let mut balance = Sim::with_mission(
            &gather_world(),
            two_player_setup(),
            gather_mission(PlayerId(0), 225, GatherSemantics::Balance, false),
        );
        let mut seq = script(&mut balance);
        assert!(
            balance.outcome().is_none(),
            "the spend holds the balance below the amount — pending"
        );
        let resolving = run_until_deliveries(&mut balance, 6, &mut seq);
        let outcome = balance.outcome().expect("the balance objective completed");
        assert_eq!(outcome.ended_tick, resolving);
    }

    #[test]
    fn the_accumulator_attributes_deliveries_by_owner_and_resource() {
        // The attribution law, pinned at the accumulator: only the mission
        // player's deliveries of the objective's resource count — the
        // opponent's ore never inflates the mission player's total, and a
        // hypothetical second resource never matches. The events are
        // constructed directly (the deposit site pins its own side: the
        // owner is resolved at deposit time, economy.rs's own tests).
        let mission = gather_mission(PlayerId(0), 100, GatherSemantics::Cumulative, false);
        let mut state = MissionState::new(mission);
        let events = vec![
            Event::ResourceDelivered {
                worker: EntityId(1),
                owner: PlayerId(1),
                resource: ResourceId(0),
                amount: 10,
            },
            Event::ResourceDelivered {
                worker: EntityId(2),
                owner: PlayerId(0),
                resource: ResourceId(0),
                amount: 10,
            },
            Event::ResourceDelivered {
                worker: EntityId(3),
                owner: PlayerId(0),
                resource: ResourceId(1),
                amount: 10,
            },
        ];
        accumulate_deliveries(&events, &mut state);
        assert_eq!(
            state.delivered, 10,
            "one ore delivery of the mission player's"
        );
        // A step with no deliveries adds nothing (the scan runs every
        // step; only the events it finds move the total).
        accumulate_deliveries(&[], &mut state);
        assert_eq!(state.delivered, 10, "an empty step's scan adds nothing");
        // A Balance-only mission never scans: the accumulator stays zero.
        let balance_mission = gather_mission(PlayerId(0), 100, GatherSemantics::Balance, false);
        let mut balance_state = MissionState::new(balance_mission);
        accumulate_deliveries(&events, &mut balance_state);
        assert_eq!(balance_state.delivered, 0, "no consumer, no scan");
    }

    #[test]
    fn an_optional_gather_objective_completes_but_never_gates_the_match() {
        // The optional law: the completion fires its event and nothing
        // else — the match runs on (nothing dies; the elimination rule
        // cannot end it).
        let mut sim = Sim::with_mission(
            &gather_world(),
            two_player_setup(),
            gather_mission(PlayerId(0), 15, GatherSemantics::Cumulative, true),
        );
        let mut seq = 0;
        run_until_deliveries(&mut sim, 2, &mut seq);
        assert!(sim.outcome().is_none(), "an optional objective never gates");
        for _ in 0..100 {
            sim.step(&[]);
        }
        assert!(sim.outcome().is_none(), "the match runs on");
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
            roster: Vec::new(),
        }
        .with_default_roster();
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
            roster: Vec::new(),
        }
        .with_default_roster();
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
