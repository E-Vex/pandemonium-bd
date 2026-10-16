//! Phase 1 mission acceptance suite (B-013, ADR-0201 §9): **does the real
//! content prove the objective model?**
//!
//! B-012 authored `crossroads_01` with `target_kind: None` — the elimination
//! rule restated as a mission objective, a silent reinterpretation of the
//! ADR's "destroy the enemy command center." The corrected fixture names the
//! surgical target: `Some("command_center")`. These tests load the REAL
//! content tree, resolve the REAL mission through the content seam
//! (`ContentTree::mission_data`), and drive the REAL simulation
//! (`Sim::with_mission`) end-to-end:
//!
//! - **Resolution** — the fixture resolves to exactly two objectives: a
//!   required Destroy aimed at the command center's `KindId`, and the
//!   optional BuildTrain of eight workers (AUD-003: the owner kept the
//!   "N exist" semantics and raised the target above the Legion's starting
//!   four — "grow to 8 workers"); resolution is deterministic.
//! - **Behavior** — destroying the enemy command center completes the
//!   required objective and ends the match with the mission player the
//!   winner, while other enemy structures remain standing (NOT the
//!   elimination rule) and the incomplete optional objective does not block
//!   victory.
//! - **Byte equivalence** — the mission-less path (`Sim::new`) over the same
//!   world and setup is unchanged: identical state hashes, no mission
//!   resolution, no elimination either (the enemy's surviving structure
//!   stands). The fixture is content data; `Sim::new` never reads it.

use std::path::{Path, PathBuf};

use pandemonium_content::ContentTree;
use pandemonium_sim::{CapTemplate, KindEconomy, KindTemplate, Sim, SpawnDef, TrivialWorld};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, Event, FogPreset, KindId, MatchSetup, MissionData,
    MissionObjectiveKind, MissionObjectiveParameters, ObjectiveState, PlayerId, PlayerSetup,
    TileFog, Vec2Fx,
};

/// The repository's content directory.
fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("content")
}

/// A two-player setup, seed 7 (the match_rules.rs spine shape — controller
/// kinds are labels at the Sim level).
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

/// The mission world for the behavioral tests: kind `command_center` is a
/// 1-hp structure that decays away on tick 0 (the "surgical strike"); every
/// other kind up to that index is a 10-hp structure that survives. Player 0
/// (the mission player) owns one surviving structure; player 1 owns the
/// command center plus one surviving structure — the "other enemy structure"
/// that must remain standing when the mission resolves.
fn command_center_world(command_center: KindId) -> TrivialWorld {
    assert!(
        command_center.0 > 0,
        "the surviving-structure kind (0) must differ from the target kind"
    );
    let kinds = (0..=command_center.0)
        .map(|index| KindTemplate {
            caps: if index == command_center.0 {
                // The target: dies on tick 0 (1 hp, -1 regen).
                vec![
                    CapTemplate::Health {
                        max_hp: 1,
                        regen_per_tick: -1,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                ]
            } else {
                // Every other kind: survives.
                vec![
                    CapTemplate::Health {
                        max_hp: 10,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                ]
            },
            economy: KindEconomy::default(),
        })
        .collect();
    TrivialWorld {
        map_id: 0x004D_3854_4553_5406,
        width_tiles: 32,
        height_tiles: 32,
        passability: TrivialWorld::open_passability(32, 32),
        buildability: TrivialWorld::open_buildability(32, 32),
        kinds,
        roster: Vec::new(),
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
            // The enemy command center — the surgical target, gone on tick 0.
            SpawnDef {
                owner: PlayerId(1),
                kind: command_center,
                pos: Vec2Fx::from_ints(24, 24),
            },
            // The enemy's other structure — still standing after the mission
            // resolves (the NOT-elimination proof).
            SpawnDef {
                owner: PlayerId(1),
                kind: KindId(0),
                pos: Vec2Fx::from_ints(20, 20),
            },
        ],
        scheduled_spawns: vec![],
        spawn_jitter_milli: 0,
    }
}

#[test]
fn crossroads_01_resolves_command_center_objective() {
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let objectives = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");

    // The Phase 1 objective set: exactly two objectives.
    assert_eq!(objectives.objectives.len(), 2);

    // Objective 0: the required Destroy, aimed at the command center's
    // KindId — resolved through the tree's entity order, the exact order the
    // bundle's world builds its kinds in.
    let command_center_index = tree
        .entities
        .iter()
        .position(|entity| entity.id == "command_center")
        .expect("command_center exists in the tree");
    let objective = &objectives.objectives[0];
    assert_eq!(objective.kind, MissionObjectiveKind::Destroy);
    assert_eq!(
        objective.parameters,
        MissionObjectiveParameters::Destroy {
            target_kind: Some(KindId(command_center_index as u32)),
        }
    );
    assert!(!objective.optional, "the Destroy objective is required");

    // Objective 1: the optional BuildTrain of eight workers — above the
    // Legion's starting four (the owner's AUD-003 ruling; the semantics
    // stay "N exist", so the target must exceed the starting force for
    // the objective to demand any play).
    let worker_index = tree
        .entities
        .iter()
        .position(|entity| entity.id == "worker")
        .expect("worker exists in the tree");
    let objective = &objectives.objectives[1];
    assert_eq!(objective.kind, MissionObjectiveKind::BuildTrain);
    assert_eq!(
        objective.parameters,
        MissionObjectiveParameters::BuildTrain {
            kind: KindId(worker_index as u32),
            count: 8,
        }
    );
    assert!(objective.optional, "the BuildTrain objective is optional");

    // Determinism: resolving twice produces the same objective set, and the
    // mission id is stable across calls.
    let again = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("the second resolution succeeds");
    assert_eq!(objectives, again);
    assert_eq!(
        tree.mission_id("crossroads_01"),
        tree.mission_id("crossroads_01")
    );
}

#[test]
fn command_center_destruction_completes_mission_with_other_structures_standing() {
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let objectives = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    let command_center = match objectives.objectives[0].parameters {
        MissionObjectiveParameters::Destroy {
            target_kind: Some(kind),
        } => kind,
        _ => panic!("objective 0 must name its target kind"),
    };

    let world = command_center_world(command_center);
    let mut sim = Sim::with_mission(&world, two_player_setup(), objectives);
    let out = sim.step(&[]);

    // (a) The required objective resolves the tick the command center falls.
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::ObjectiveState {
                index: 0,
                state: ObjectiveState::Completed
            }
        )),
        "the destroy objective completes: {:?}",
        out.events
    );

    // (b) The mission ends with the mission player as winner — the
    // AND-composition fires MatchEnded the same tick.
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::MatchEnded {
                winner: PlayerId(0)
            }
        )),
        "MatchEnded names the mission player: {:?}",
        out.events
    );
    let outcome = sim
        .outcome()
        .expect("the mission resolves the match for the mission player");
    assert_eq!(outcome.winner, PlayerId(0));

    // (c) The enemy's other structure remains standing — the surgical
    // target fell, the side did not (NOT the elimination rule), and no
    // command-center entity is left in the world.
    assert!(
        sim.snapshot()
            .entities
            .iter()
            .any(|entity| entity.owner == PlayerId(1) && entity.kind == KindId(0)),
        "the enemy's surviving structure is still in the world"
    );
    assert!(
        sim.snapshot()
            .entities
            .iter()
            .all(|entity| entity.kind != command_center),
        "no command center is left standing"
    );

    // (d) The optional BuildTrain never completed (this fixture world
    // carries no workers at all) and did not block the mission's victory.
    assert!(
        !out.events.iter().any(|event| matches!(
            event,
            Event::ObjectiveState {
                index: 1,
                state: ObjectiveState::Completed
            }
        )),
        "the optional objective stays incomplete and never gates the match: {:?}",
        out.events
    );
}

#[test]
fn mission_less_path_is_byte_equivalent() {
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let objectives = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    let command_center = match objectives.objectives[0].parameters {
        MissionObjectiveParameters::Destroy {
            target_kind: Some(kind),
        } => kind,
        _ => panic!("objective 0 must name its target kind"),
    };

    // The same world and setup through both constructors: the mission-less
    // `Sim::new` (which never reads mission data — the fixture is content,
    // not code) and the mission-wrapped `Sim::with_mission`.
    let world = command_center_world(command_center);
    let setup = two_player_setup();
    let mut plain = Sim::new(&world, setup.clone());
    let mut wrapped = Sim::with_mission(&world, setup, objectives);

    // Byte equivalence: identical state hashes before and after the step —
    // the mission state is derived (A-066 pattern), never hashed.
    assert_eq!(plain.state_hash(), wrapped.state_hash());
    let plain_out = plain.step(&[]);
    let wrapped_out = wrapped.step(&[]);
    assert_eq!(
        plain.state_hash(),
        wrapped.state_hash(),
        "mission data never enters the canonical state hash"
    );

    // The mission-less path does NOT resolve via mission completion — the
    // enemy still has a surviving structure, so elimination does not fire
    // either.
    assert!(plain.outcome().is_none(), "no mission, no resolution");
    assert!(
        !plain_out
            .events
            .iter()
            .any(|event| matches!(event, Event::ObjectiveState { .. })),
        "the mission-less path emits no objective-state events"
    );

    // The contrast that proves the seam: the SAME hashed state carries a
    // different derived outcome — the mission-wrapped match resolves for the
    // mission player, the mission-less one keeps running.
    assert!(wrapped.outcome().is_some());
    assert!(
        wrapped_out
            .events
            .iter()
            .any(|event| matches!(event, Event::ObjectiveState { .. })),
        "the mission-wrapped path evaluates its objectives"
    );
}

/// The AUD-003 regression test — the one the fixture worlds above could not
/// express: the REAL mission on the REAL content against the REAL Legion
/// starting forces (1 command center + 4 workers per player, plan §9.7).
/// Before the count was raised, the optional BuildTrain objective (then
/// `count: 4`) was vacuously satisfied at tick 0 by the starting four — the
/// fixture-world tests never caught it because their worlds carried no
/// workers at all. The owner's ruling keeps the "N exist" existence
/// semantics and sets the target at eight: the objective must stay pending
/// through an idle opening and complete exactly when the eighth worker
/// comes to exist, through the real economy (the starting 200 ore is
/// exactly four workers' cost at 50 each — the goal is reachable by pure
/// production from the opening).
#[test]
fn the_optional_objective_demands_growth_beyond_the_starting_forces() {
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let objectives = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    let command_center = match objectives.objectives[0].parameters {
        MissionObjectiveParameters::Destroy {
            target_kind: Some(kind),
        } => kind,
        _ => panic!("objective 0 must name its target kind"),
    };
    let worker = match objectives.objectives[1].parameters {
        MissionObjectiveParameters::BuildTrain { kind, count } => {
            assert_eq!(count, 8, "the owner's AUD-003 ruling: grow to eight");
            kind
        }
        _ => panic!("objective 1 must be the BuildTrain objective"),
    };

    // The real bundle's world: the Legion starting forces ride the content
    // seam exactly as the client's begin_mission builds them.
    let bundle = tree.bundle("crossroads_64").expect("crossroads_64 bundles");
    let world = bundle.world();
    let mut sim = Sim::with_mission(&world, two_player_setup(), objectives);
    let workers_owned = |sim: &Sim| {
        sim.snapshot()
            .entities
            .iter()
            .filter(|entity| entity.owner == PlayerId(0) && entity.kind == worker)
            .count()
    };

    // (a) Tick 0: exactly the starting four workers exist — the count the
    // old objective asked for. The mission has not stepped yet; standing
    // still must never satisfy the corrected objective.
    assert_eq!(
        workers_owned(&sim),
        4,
        "the Legion starts with exactly four workers (plan section 9.7)"
    );

    // (b) The idle opening: 600 ticks (twenty seconds of game time), zero
    // commands. Nothing produces without an order, so the optional
    // objective stays pending — no event, no completion, no match end.
    for _ in 0..600 {
        let out = sim.step(&[]);
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::ObjectiveState { index: 1, .. })),
            "the optional objective must not resolve while only four workers exist"
        );
    }
    assert_eq!(workers_owned(&sim), 4, "no worker was produced unprompted");
    assert!(sim.outcome().is_none(), "the match is still running");

    // (c) Grow to eight: the mission player's command center trains four
    // workers through the real economy — the starting 200 ore is exactly
    // 4 x 50, so the objective is reachable by production alone.
    let producer = sim
        .snapshot()
        .entities
        .iter()
        .find(|entity| entity.owner == PlayerId(0) && entity.kind == command_center)
        .expect("the mission player's command center stands")
        .id;
    let tick = sim.tick();
    let commands: Vec<Command> = (0..4)
        .map(|seq| {
            Command::new(
                PlayerId(0),
                tick,
                seq,
                CommandKind::Train {
                    producer,
                    unit: worker,
                },
            )
        })
        .collect();

    // (d) Step until the objective resolves; it must fire exactly when the
    // eighth worker comes to exist — the equality of the two observed
    // ticks is the not-before proof (an early resolution would leave the
    // counts unequal).
    let mut resolved_at: Option<u32> = None;
    let mut eighth_worker_at: Option<u32> = None;
    let mut fed = false;
    for _ in 0..2400 {
        let feed: Vec<Command> = if fed {
            Vec::new()
        } else {
            fed = true;
            commands.clone()
        };
        let out = sim.step(&feed);
        if resolved_at.is_none()
            && out.events.iter().any(|event| {
                matches!(
                    event,
                    Event::ObjectiveState {
                        index: 1,
                        state: ObjectiveState::Completed
                    }
                )
            })
        {
            resolved_at = Some(sim.tick());
        }
        if eighth_worker_at.is_none() && workers_owned(&sim) == 8 {
            eighth_worker_at = Some(sim.tick());
        }
        if resolved_at.is_some() && eighth_worker_at.is_some() {
            break;
        }
    }
    let resolved_at = resolved_at.expect("the objective completes once eight workers exist");
    let eighth_worker_at = eighth_worker_at.expect("the eighth worker comes to exist");
    assert_eq!(
        resolved_at, eighth_worker_at,
        "the objective completes exactly when the count is genuinely reached"
    );
    // The optional objective never gates the match: the enemy command
    // center still stands (the required objective is pending), so the
    // match runs on with the optional objective completed.
    assert_eq!(workers_owned(&sim), 8);
    assert!(
        sim.outcome().is_none(),
        "an optional objective completes without ending the match"
    );
}

// ---- Phase 2, Package 1: fog presets (ADR-0201 §3, B-015) -----------------
//
// The acceptance set: the default preset is byte-identical to the mission-less
// path (state hash AND fog bytes at every tick), the Explored preset seeds the
// whole map Explored-or-Visible with no Hidden tile left (the terrain is known
// from tick 0), and neither preset can move a state hash (FD-8: the fog is
// derived and unhashed, A-059 — the filter's initial state is not sim state).

/// The real content's mission data with the fog preset swapped — the sim-side
/// vocabulary is public, so the seam's contract is testable directly over the
/// real resolved objective set.
fn mission_with_preset(preset: FogPreset) -> MissionData {
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let mut mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    mission.fog_preset = preset;
    mission
}

#[test]
fn the_default_preset_is_byte_identical_to_the_mission_less_path() {
    // Package 1's headline proof: `Sim::new` and `Sim::with_mission` over the
    // same world and setup produce identical state hashes AND identical fog
    // bytes at every tick when the preset is Default — the seeding reproduces
    // the historical all-Hidden init exactly (A-059, FD-8).
    let bundle = ContentTree::load_dir(&repo_content())
        .expect("the repository content loads")
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles");
    let world = bundle.world();
    let setup = two_player_setup();
    let mission = mission_with_preset(FogPreset::Default);
    let mut plain = Sim::new(&world, setup.clone());
    let mut wrapped = Sim::with_mission(&world, setup, mission);
    assert_eq!(plain.state_hash(), wrapped.state_hash(), "tick 0 hash");
    assert_eq!(
        plain.player_view(PlayerId(0)).fog,
        wrapped.player_view(PlayerId(0)).fog,
        "tick 0 fog bytes — the default seed is the historical init"
    );
    for _ in 0..120 {
        plain.step(&[]);
        wrapped.step(&[]);
        assert_eq!(plain.state_hash(), wrapped.state_hash());
        assert_eq!(
            plain.player_view(PlayerId(0)).fog,
            wrapped.player_view(PlayerId(0)).fog
        );
    }
}

#[test]
fn the_explored_preset_knows_the_whole_map_from_tick_0() {
    // FD-8's outward shape: an Explored mission starts with every tile
    // Explored-or-Visible — no Hidden tile anywhere — and the live vision
    // pass maintains the Visible layer on top exactly as always. The state
    // hash stays identical to the mission-less path at every tick: the fog
    // is a derived filter, never canonical state.
    let bundle = ContentTree::load_dir(&repo_content())
        .expect("the repository content loads")
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles");
    let world = bundle.world();
    let setup = two_player_setup();
    let mission = mission_with_preset(FogPreset::Explored);
    let mut plain = Sim::new(&world, setup.clone());
    let mut explored = Sim::with_mission(&world, setup, mission);

    let fog = explored.player_view(PlayerId(0)).fog;
    assert!(
        fog.iter()
            .all(|tile| { matches!(tile, TileFog::Explored | TileFog::Visible) }),
        "the explored preset leaves no Hidden tile ({} tiles)",
        fog.len()
    );
    // The contrast: the default path still has Hidden tiles far from base.
    assert!(
        plain
            .player_view(PlayerId(0))
            .fog
            .iter()
            .any(|tile| matches!(tile, TileFog::Hidden)),
        "the default preset keeps Hidden tiles — the presets are observably distinct"
    );
    for _ in 0..120 {
        plain.step(&[]);
        explored.step(&[]);
        assert_eq!(
            plain.state_hash(),
            explored.state_hash(),
            "the fog preset never enters the canonical state hash"
        );
        // The explored map stays explored across ticks (the vision pass only
        // demotes last-tick-Visible, never a pre-existing Explored tile).
        assert!(explored
            .player_view(PlayerId(0))
            .fog
            .iter()
            .all(|tile| matches!(tile, TileFog::Explored | TileFog::Visible)));
    }
}

#[test]
fn the_visible_preset_settles_to_a_fully_explored_map() {
    // The Visible seed is all-tiles-Visible; the first vision pass then
    // settles everything outside friendly radii to Explored (the fog
    // system's normal demotion — FD-8: the preset seeds the filter, the pass
    // owns the evolution). Observably: no Hidden tile, live vision intact,
    // and the state hash still identical to the mission-less path.
    let bundle = ContentTree::load_dir(&repo_content())
        .expect("the repository content loads")
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles");
    let world = bundle.world();
    let setup = two_player_setup();
    let mission = mission_with_preset(FogPreset::Visible);
    let mut plain = Sim::new(&world, setup.clone());
    let mut revealed = Sim::with_mission(&world, setup, mission);
    assert_eq!(plain.state_hash(), revealed.state_hash(), "tick 0");
    assert!(
        revealed
            .player_view(PlayerId(0))
            .fog
            .iter()
            .all(|tile| matches!(tile, TileFog::Explored | TileFog::Visible)),
        "the visible preset leaves no Hidden tile"
    );
    for _ in 0..60 {
        plain.step(&[]);
        revealed.step(&[]);
        assert_eq!(plain.state_hash(), revealed.state_hash());
    }
}
