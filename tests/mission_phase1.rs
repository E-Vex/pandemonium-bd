//! Phase 1 mission acceptance suite (B-013, ADR-0201 §9): **does the real
//! content prove the objective model?**
//!
//! B-012 authored `crossroads_01` with `target_kind: None` — the elimination
//! rule restated as a mission objective, a silent reinterpretation of the
//! ADR's "destroy the enemy command center." The corrected fixture names the
//! surgical target: `Some("command_center")`. These tests load the REAL
//! content tree, resolve the REAL mission through the content seam
//! (`ContentTree::mission_objectives`), and drive the REAL simulation
//! (`Sim::with_mission`) end-to-end:
//!
//! - **Resolution** — the fixture resolves to exactly two objectives: a
//!   required Destroy aimed at the command center's `KindId`, and the
//!   optional BuildTrain of four workers; resolution is deterministic.
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
    ControllerKind, Event, KindId, MatchSetup, MissionObjectiveKind, MissionObjectiveParameters,
    ObjectiveState, PlayerId, PlayerSetup, Vec2Fx,
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
        .mission_objectives("crossroads_01", PlayerId(0))
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

    // Objective 1: the optional BuildTrain of four workers.
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
            count: 4,
        }
    );
    assert!(objective.optional, "the BuildTrain objective is optional");

    // Determinism: resolving twice produces the same objective set, and the
    // mission id is stable across calls.
    let again = tree
        .mission_objectives("crossroads_01", PlayerId(0))
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
        .mission_objectives("crossroads_01", PlayerId(0))
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

    // (d) The optional BuildTrain never completed (no four workers exist)
    // and did not block the mission's victory.
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
        .mission_objectives("crossroads_01", PlayerId(0))
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
