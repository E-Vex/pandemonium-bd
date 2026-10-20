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

use pandemonium_content::{ContentTree, SourceMap};
use pandemonium_fx::Fx;
use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::{
    run_command_log_with_mission, CapTemplate, KindEconomy, KindTemplate, ScheduledSpawnDef, Sim,
    SpawnDef, TrivialWorld,
};
use pandemonium_sim_api::{
    AreaSpec, Command, CommandKind, ControllerKind, EntityId, Event, FogPreset, GatherSemantics,
    KindId, MatchSetup, MissionObjective, MissionObjectiveKind, MissionObjectiveParameters,
    ObjectiveState, PlayerId, PlayerSetup, Reject, RejectReason, ResourceId, TileFog, TilePos,
    Vec2Fx,
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
        roster: Vec::new(),
    }
    .with_default_roster()
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

// -------------------------------------------------------------------------
// B-015 — fog presets (ADR-0201 Phase 2): the preset rides MissionData,
// initializes the fog cache, and never touches the canonical hash.
// -------------------------------------------------------------------------

/// crossroads_01's registered mission id, captured before B-015 touched
/// anything (pre-flight at master 428d55d) and re-checked after every
/// identity-bearing step. The mission does not use the fog-preset field, so
/// its id must never move — recordings carry `mission_id` (AUD-002) and a
/// silent id change breaks replays.
const CROSSROADS_01_MISSION_ID: u64 = 0xc837_9403_777e_4286;

#[test]
fn crossroads_01s_mission_id_survives_b015() {
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    assert_eq!(
        tree.mission_id("crossroads_01"),
        CROSSROADS_01_MISSION_ID,
        "a mission that does not use the fog-preset field keeps exactly the \
         same mission id (the encode-only-if-non-default rule)"
    );
    // And the bundle identities the content goldens pin are equally intact
    // (the mission category appends nothing to their encoders).
    let bundle = tree.bundle("crossroads_64").expect("crossroads_64 bundles");
    assert_eq!(
        bundle.content_hash(),
        0x9bc1_8c52_1107_b262,
        "the registered content hash never moves"
    );
    assert_eq!(
        bundle.map_id(),
        0xd381_3640_1ab0_2ff1,
        "the registered map id never moves"
    );
}

#[test]
fn the_default_preset_matches_the_mission_less_path_at_the_checked_ticks() {
    // B-015 acceptance: `Sim::new` vs `Sim::with_mission(fog_preset=Default)`
    // — identical state hash at every tick checked (0, 300, 7200) — on the
    // real content's world, the exact world the client's begin_mission
    // builds. Fog state is derived and non-hashed (A-059), and the Default
    // preset is byte-identical to the mission-less initialization, so the
    // hashes agree tick for tick.
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    assert_eq!(
        mission.fog_preset,
        FogPreset::Default,
        "crossroads_01 carries no fog preset — the seam resolves Default"
    );

    let world = tree
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles")
        .world();
    let setup = two_player_setup();
    let mut plain = Sim::new(&world, setup.clone());
    let mut wrapped = Sim::with_mission(&world, setup, mission);

    assert_eq!(plain.state_hash(), wrapped.state_hash(), "tick 0");
    for tick in 1..=7200u32 {
        plain.step(&[]);
        wrapped.step(&[]);
        if tick == 300 || tick == 7200 {
            assert_eq!(
                plain.state_hash(),
                wrapped.state_hash(),
                "tick {tick}: the Default preset never diverges from the mission-less path"
            );
        }
    }
    // The mission-less match stays unresolved either way (both command
    // centers stand; nothing ever dies in an idle opening).
    assert!(plain.outcome().is_none() && wrapped.outcome().is_none());
}

/// The fog row of one player, straight out of the `player_view` the client's
/// fog renderer and the AI read.
fn fog_row(sim: &Sim, player: PlayerId) -> Vec<TileFog> {
    let view = sim.player_view(player);
    assert!(!view.fog.is_empty(), "the player is in the match");
    view.fog
}

#[test]
fn each_preset_initializes_the_fog_rows_as_its_kind_promises() {
    // B-015 acceptance: Default starts all Hidden (outside vision), Explored
    // starts every tile at least Explored (vision lights Visible on top),
    // Visible starts — and keeps — every tile Visible. The same init applies
    // to every player.
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let world = tree
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles")
        .world();
    let base = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");

    let mut sim_default = Sim::with_mission(&world, two_player_setup(), base.clone());
    let mut explored = base.clone();
    explored.fog_preset = FogPreset::Explored;
    let mut sim_explored = Sim::with_mission(&world, two_player_setup(), explored);
    let mut visible = base;
    visible.fog_preset = FogPreset::Visible;
    let mut sim_visible = Sim::with_mission(&world, two_player_setup(), visible);

    // Default at tick 0: today's fog — the far corners are Hidden, the
    // starting positions' surroundings are Visible.
    let default_row = fog_row(&sim_default, PlayerId(0));
    assert!(
        default_row.contains(&TileFog::Hidden),
        "Default starts Hidden outside vision"
    );
    assert!(
        default_row.contains(&TileFog::Visible),
        "Default marks what units see Visible"
    );
    assert!(
        !default_row.contains(&TileFog::Explored),
        "nothing is Explored at tick 0"
    );

    // Explored at tick 0: nothing is Hidden; everything outside vision is
    // exactly Explored; vision still lights Visible on top.
    let explored_row = fog_row(&sim_explored, PlayerId(0));
    assert!(
        !explored_row.contains(&TileFog::Hidden),
        "Explored starts with no Hidden tile"
    );
    assert!(explored_row.contains(&TileFog::Explored));
    assert!(
        explored_row.contains(&TileFog::Visible),
        "vision lights Visible on the base"
    );
    // Same init for every player.
    let explored_row_p1 = fog_row(&sim_explored, PlayerId(1));
    assert!(!explored_row_p1.contains(&TileFog::Hidden));

    // Visible at tick 0: the whole map, every player.
    let visible_row = fog_row(&sim_visible, PlayerId(0));
    assert!(
        visible_row.iter().all(|tile| *tile == TileFog::Visible),
        "Visible starts every tile Visible"
    );
    let visible_row_p1 = fog_row(&sim_visible, PlayerId(1));
    assert!(visible_row_p1.iter().all(|tile| *tile == TileFog::Visible));

    // And after 300 idle ticks the initializations hold: Default keeps its
    // Hidden frontier (nothing moved, so nothing new was explored), Explored
    // keeps its no-Hidden floor, Visible keeps every tile Visible.
    for _ in 0..300 {
        sim_default.step(&[]);
        sim_explored.step(&[]);
        sim_visible.step(&[]);
    }
    let default_row = fog_row(&sim_default, PlayerId(0));
    assert!(
        default_row.contains(&TileFog::Hidden),
        "Default keeps its Hidden frontier"
    );
    let explored_row = fog_row(&sim_explored, PlayerId(0));
    assert!(
        !explored_row.contains(&TileFog::Hidden),
        "Explored never regains Hidden tiles"
    );
    let visible_row = fog_row(&sim_visible, PlayerId(0));
    assert!(
        visible_row.iter().all(|tile| *tile == TileFog::Visible),
        "Visible keeps every tile Visible after many ticks"
    );
}

#[test]
fn the_visible_preset_outlasts_vision_churn_through_the_public_view() {
    // The persistence proof: `advance_vision` demotes every previously-
    // Visible tile that is no longer inside a radius to Explored, so an
    // init-only reveal would collapse within one moving unit's wake. The
    // preset instead holds the whole map Visible for the entire match —
    // through the same `player_view` the client's fog renderer and the AI's
    // targeting read — while the sim state underneath is never touched
    // (FD-8: fog filters, never alters).
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let bundle = tree.bundle("crossroads_64").expect("crossroads_64 bundles");
    let world = bundle.world();
    let mut mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    mission.fog_preset = FogPreset::Visible;
    let mut sim = Sim::with_mission(&world, two_player_setup(), mission);

    // A player-0 worker marches away from its starting cluster: the tiles it
    // leaves are exactly the ones `advance_vision` would demote.
    let worker = sim
        .snapshot()
        .entities
        .iter()
        .find(|entity| entity.owner == PlayerId(0) && entity.kind == worker_kind(&tree))
        .expect("the Legion fields workers")
        .id;
    // The opposite corner of the 64x64 map — the worker's vision cannot
    // reach its origin from there.
    let tick = sim.tick();
    sim.step(&[Command::new(
        PlayerId(0),
        tick,
        0,
        CommandKind::Move {
            units: vec![worker],
            target: Vec2Fx::from_ints(63, 63),
        },
    )]);
    for _ in 0..1200 {
        sim.step(&[]);
    }
    // The worker really did cross the map (the order executed).
    let arrived = sim
        .snapshot()
        .entities
        .iter()
        .find(|entity| entity.id == worker)
        .expect("the worker lives")
        .pos;
    assert!(
        arrived.x.raw() > (40 << 16),
        "the worker marched to the far side (pos {arrived:?})"
    );
    // And the fog never darkened: every tile of every player is still
    // Visible — the origin tiles included, long after their unit left.
    for player in [PlayerId(0), PlayerId(1)] {
        let row = fog_row(&sim, player);
        assert!(
            row.iter().all(|tile| *tile == TileFog::Visible),
            "player {player:?}: the whole map stays Visible for the entire match"
        );
    }
}

/// The worker kind's id in the tree's entity order (the same order the
/// bundle's world builds its kinds in).
fn worker_kind(tree: &ContentTree) -> KindId {
    KindId(
        tree.entities
            .iter()
            .position(|entity| entity.id == "worker")
            .expect("worker exists in the tree") as u32,
    )
}

// -------------------------------------------------------------------------
// B-017 — Survive + Protect (ADR-0201 Phase 2): the two new objective
// kinds against the REAL content tree and the REAL seam. Loader-level
// gates run over in-memory SourceMaps built on the real content (no
// content file is added to the repository — the frozen tree is untouched);
// behavioral acceptance runs over the real bundle world or a fixture
// world whose kinds resolve through the real tree's entity order.
// -------------------------------------------------------------------------

/// A probe mission file over the real content's closed reference set
/// (crossroads_64 / legion / alpha), carrying the given objectives text.
/// Used with `SourceMap::from_dir(&repo_content()).mission(...)` — the
/// tree-level gates (references, protect starting forces, the
/// all-Protect-required refusal) run exactly as they would for a file on
/// disk, without the repository's frozen content gaining a file.
fn probe_mission(id: &str, objectives: &str) -> String {
    format!(
        r#"
(
    schema_version: 1,
    id: "{id}",
    display_name: "Probe",
    map: "crossroads_64",
    faction: "legion",
    rules: "alpha",
    objectives: [
{objectives}
    ],
)
"#
    )
}

/// The real content sources with one probe mission added in memory.
fn sources_with_probe(id: &str, objectives: &str) -> SourceMap {
    SourceMap::from_dir(&repo_content())
        .expect("the real content reads")
        .mission(&format!("{id}.ron"), &probe_mission(id, objectives))
}

#[test]
fn survive_300_is_pending_at_tick_299_and_completes_at_tick_300() {
    // The brief's binding acceptance, at the authored scale: pending at
    // tick 299, complete at 300. The mission data is crossroads_01 resolved
    // through the real seam with its objective list replaced by a single
    // required Survive (the fog tests' mutation pattern — the carrier is
    // the same seam the client uses); the world is the real bundle's, the
    // same one begin_mission builds. Idle play: nothing dies, so only the
    // survive window can resolve the match.
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let world = tree
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles")
        .world();
    let mut mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    mission.objectives = vec![MissionObjective {
        kind: MissionObjectiveKind::Survive,
        parameters: MissionObjectiveParameters::Survive { ticks: 300 },
        optional: false,
    }];
    let setup = two_player_setup();
    let mut plain = Sim::new(&world, setup.clone());
    let mut sim = Sim::with_mission(&world, setup, mission);

    // Ticks 0 through 299 — 300 steps, the last of which evaluates stage 10
    // at tick 299: the objective is pending, the match unresolved, and the
    // mission state never touches the canonical hash (A-066).
    for _ in 0..300 {
        let out = sim.step(&[]);
        plain.step(&[]);
        assert!(
            !out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState { .. } | Event::MatchEnded { .. }
            )),
            "pending through tick 299: {:?}",
            out.events
        );
    }
    assert_eq!(sim.tick(), 300);
    assert!(sim.outcome().is_none(), "tick 299: pending, no resolution");
    assert_eq!(
        sim.state_hash(),
        plain.state_hash(),
        "the mission data never enters the canonical hash"
    );

    // Tick 300 — the 301st step's stage-10 evaluation carries tick 300: the
    // objective completes and the AND-composition (its only required
    // objective) ends the match with the mission player the winner.
    let out = sim.step(&[]);
    plain.step(&[]);
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::ObjectiveState {
                index: 0,
                state: ObjectiveState::Completed
            }
        )),
        "the survive objective completes at tick 300: {:?}",
        out.events
    );
    let outcome = sim
        .outcome()
        .expect("the survive window resolves the match");
    assert_eq!(outcome.winner, PlayerId(0));
    assert_eq!(outcome.ended_tick, 300);
    // The mission-less twin stays unresolved through the same ticks (the
    // goldens' byte path, unchanged).
    assert!(plain.outcome().is_none());
    assert_eq!(
        sim.state_hash(),
        plain.state_hash(),
        "even the resolution tick's hash is mission-independent (derived state)"
    );
}

#[test]
fn the_loader_rejects_survive_windows_below_the_floor() {
    // The brief's loader acceptance: ticks 0 and 299 are refused, 300 is
    // the first legal window. The probes run against the REAL content tree
    // (in-memory sources) so the gate fires exactly as it would for an
    // authored file.
    for ticks in [0u32, 299] {
        let sources = sources_with_probe(
            "survive_bad",
            &format!(
                "( kind: \"Survive\", parameters: Survive(( ticks: {ticks} )), optional: false, )"
            ),
        );
        let err = ContentTree::load(&sources).unwrap_err();
        assert!(
            err.to_string().contains(&format!(
                "has ticks {ticks} (survive objectives need at least 300 ticks)"
            )),
            "ticks {ticks}: got: {err}"
        );
    }
    let sources = sources_with_probe(
        "survive_ok",
        "( kind: \"Survive\", parameters: Survive(( ticks: 300 )), optional: false, )",
    );
    let tree = ContentTree::load(&sources).expect("300 ticks is the first legal window");
    // And the resolved objective set carries the authored window verbatim.
    let objectives = tree
        .mission_data("survive_ok", PlayerId(0))
        .expect("the probe resolves");
    assert_eq!(
        objectives.objectives,
        vec![MissionObjective {
            kind: MissionObjectiveKind::Survive,
            parameters: MissionObjectiveParameters::Survive { ticks: 300 },
            optional: false,
        }]
    );
}

#[test]
fn the_loader_rejects_protect_kinds_the_legion_does_not_start_with() {
    // The brief's loader acceptance: a kind not present in the mission
    // player's starting forces is refused. The ore node is a real entity
    // in the tree (the reference check passes) but the Legion starts with
    // one command center and four workers — protecting anything else
    // would fail on the first tick (the vacuous-failure guard, AUD-003).
    // A completable required objective rides along so the all-Protect gate
    // stays out of the way.
    let objectives = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"Protect\", parameters: Protect(( kind: \"ore_node\" )), optional: true, )";
    let sources = sources_with_probe("protect_bad", objectives);
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string().contains(
            "mission 'protect_bad' protects kind 'ore_node' that the starting forces of \
             faction 'legion' do not carry"
        ),
        "got: {err}"
    );
    // A kind the tree does not carry at all is the entity-reference law's
    // error (the same check BuildTrain kinds and Destroy targets get).
    let objectives = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"Protect\", parameters: Protect(( kind: \"battlecruiser\" )), optional: true, )";
    let sources = sources_with_probe("protect_unknown", objectives);
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string()
            .contains("references unknown entity 'battlecruiser'"),
        "got: {err}"
    );
    // A kind the Legion starts with (worker) is legal, and the seam
    // resolves it through the tree's entity order.
    let objectives = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"Protect\", parameters: Protect(( kind: \"worker\" )), optional: true, )";
    let sources = sources_with_probe("protect_ok", objectives);
    let tree = ContentTree::load(&sources).expect("protecting a starting kind is legal");
    let resolved = tree
        .mission_data("protect_ok", PlayerId(0))
        .expect("the probe resolves");
    assert_eq!(
        resolved.objectives[1],
        MissionObjective {
            kind: MissionObjectiveKind::Protect,
            parameters: MissionObjectiveParameters::Protect {
                kind: worker_kind(&tree)
            },
            optional: true,
        }
    );
}

#[test]
fn a_mission_whose_required_objectives_are_all_protect_is_refused_at_load() {
    // The victory-reachability gate against the real tree: a mission whose
    // required objectives are all Protect has no path to victory (protect
    // never completes), so the loader refuses it. The escape hatches both
    // load: a completable required objective beside a required protect, and
    // the all-optional law (the pre-existing shape that ends only through
    // the elimination rule).
    let all_protect = "( kind: \"Protect\", parameters: Protect(( kind: \"worker\" )), optional: false, ),\n        ( kind: \"Protect\", parameters: Protect(( kind: \"command_center\" )), optional: false, )";
    let sources = sources_with_probe("all_protect", all_protect);
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string().contains(
            "every required objective of mission 'all_protect' is Protect — protect \
             objectives never complete (they are fail-only), so victory would be unreachable"
        ),
        "got: {err}"
    );
    // A completable required objective (Survive) beside a required protect
    // keeps victory reachable — the composition the kinds exist for.
    let mixed = "( kind: \"Survive\", parameters: Survive(( ticks: 300 )), optional: false, ),\n        ( kind: \"Protect\", parameters: Protect(( kind: \"worker\" )), optional: false, )";
    let sources = sources_with_probe("mixed", mixed);
    ContentTree::load(&sources).expect("a required protect beside a completable objective loads");
    // All-optional protects stay legal (optional never gates the match).
    let optional_only = "( kind: \"Protect\", parameters: Protect(( kind: \"worker\" )), optional: true, ),\n        ( kind: \"Protect\", parameters: Protect(( kind: \"command_center\" )), optional: true, )";
    let sources = sources_with_probe("optional_only", optional_only);
    ContentTree::load(&sources).expect("an all-optional protect mission loads");
}

/// The protect fixture: the real worker kind (resolved through the tree's
/// entity order — the exact id the bundle's world assigns) as a decaying
/// unit (41 hp, -1 per tick, no Footprint: protect guards any owned entity
/// kind, not just structures), every other kind up to that index a
/// surviving structure. Player 0 (the mission player) owns a surviving
/// structure, one initial worker, and one worker scheduled for tick 30 —
/// the initial worker dies at tick 40, the scheduled one at tick 70, so
/// the protected set is non-empty through tick 69 and empty from tick 70.
/// Player 1 owns a surviving structure (the failure's winner).
fn dying_workers_world(worker: KindId) -> TrivialWorld {
    let kinds = (0..=worker.0)
        .map(|index| KindTemplate {
            caps: if index == worker.0 {
                vec![CapTemplate::Health {
                    max_hp: 41,
                    regen_per_tick: -1,
                }]
            } else {
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
        map_id: 0x0050_524F_5445_4230,
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
            // The mission player's protected worker — dies at tick 40.
            SpawnDef {
                owner: PlayerId(0),
                kind: worker,
                pos: Vec2Fx::from_ints(10, 10),
            },
            // The opponent's surviving structure.
            SpawnDef {
                owner: PlayerId(1),
                kind: KindId(0),
                pos: Vec2Fx::from_ints(24, 24),
            },
        ],
        // The second protected worker — spawns during tick 30, dies at
        // tick 70: the stagger proves "pending while at least one living
        // entity of the kind remains" (not "while any ever lived", not
        // "until the first one dies").
        scheduled_spawns: vec![ScheduledSpawnDef {
            tick: 30,
            spawn: SpawnDef {
                owner: PlayerId(0),
                kind: worker,
                pos: Vec2Fx::from_ints(12, 12),
            },
        }],
        spawn_jitter_milli: 0,
        roster: Vec::new(),
    }
    .with_default_roster()
}

#[test]
fn protect_is_pending_while_any_worker_lives_and_fails_when_the_last_dies() {
    // The brief's behavioral acceptance on a fixture world whose worker
    // kind is the REAL tree's: pending while at least one living worker
    // remains — including after the first one dies (tick 40) — failed on
    // the first tick none remains (tick 70, the same step as the last
    // worker's Died event, FD-9), and a required failure ends the match
    // as lost for the mission player through the same path other required
    // failures use, while their structure still stands (NOT elimination).
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let worker = worker_kind(&tree);
    let mut mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    mission.objectives = vec![MissionObjective {
        kind: MissionObjectiveKind::Protect,
        parameters: MissionObjectiveParameters::Protect { kind: worker },
        optional: false,
    }];
    let world = dying_workers_world(worker);
    let setup = two_player_setup();
    let mut plain = Sim::new(&world, setup.clone());
    let mut sim = Sim::with_mission(&world, setup, mission);

    // Ticks 0..39: both workers live — pending, no events.
    for _ in 0..40 {
        let out = sim.step(&[]);
        plain.step(&[]);
        assert!(
            !out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState { .. } | Event::MatchEnded { .. }
            )),
            "pending while the workers live: {:?}",
            out.events
        );
    }
    // Tick 40: the first worker dies — its Died event fires, but the
    // second still lives, so the protect objective stays PENDING (the
    // "at least one living entity" law, not "no death ever").
    let out = sim.step(&[]);
    plain.step(&[]);
    assert!(
        out.events
            .iter()
            .any(|event| matches!(event, Event::Died { .. })),
        "the first worker dies at tick 40: {:?}",
        out.events
    );
    assert!(
        !out.events.iter().any(|event| matches!(
            event,
            Event::ObjectiveState { .. } | Event::MatchEnded { .. }
        )),
        "one worker still lives — the protect stays pending: {:?}",
        out.events
    );
    assert_eq!(sim.state_hash(), plain.state_hash(), "A-066 at tick 40");

    // Ticks 41..69: the second worker lives — still pending.
    for _ in 0..29 {
        let out = sim.step(&[]);
        plain.step(&[]);
        assert!(
            !out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState { .. } | Event::MatchEnded { .. }
            )),
            "pending through tick 69: {:?}",
            out.events
        );
    }

    // Tick 70: the last worker dies — Died, then the failure, then
    // MatchEnded with the opponent the winner, in that order in the same
    // step's stream.
    let out = sim.step(&[]);
    plain.step(&[]);
    let died = out
        .events
        .iter()
        .position(|event| matches!(event, Event::Died { .. }))
        .expect("the last worker died this tick");
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
        .expect("the protect objective fails when none remains");
    let ended = out
        .events
        .iter()
        .position(|event| matches!(event, Event::MatchEnded { .. }))
        .expect("a required failure ends the match as lost");
    assert!(died < failed, "FD-9: {out:?}");
    assert!(failed < ended, "{out:?}");
    let outcome = sim.outcome().expect("the failure resolved the match");
    assert_eq!(outcome.winner, PlayerId(1), "the mission player lost");
    assert_eq!(outcome.ended_tick, 70);
    // The mission player was NOT eliminated — the failure path, not the
    // defeat path, ended the match.
    assert!(
        sim.snapshot()
            .entities
            .iter()
            .any(|entity| entity.owner == PlayerId(0) && entity.kind == KindId(0)),
        "the mission player's structure still stands"
    );
    // The mission-less twin: both workers die, no one is eliminated, the
    // match runs on (the same world, the same ticks).
    assert!(plain.outcome().is_none());
    assert_eq!(sim.state_hash(), plain.state_hash(), "A-066 past the end");
}

#[test]
fn an_optional_protect_failure_reports_but_never_ends_the_match() {
    // The optional law at the seam: the failed protect objective fires its
    // ObjectiveState::Failed event (the end screen's story hook) but never
    // gates MatchEnded — the match runs on under the elimination rule, and
    // no one here is eliminated.
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let worker = worker_kind(&tree);
    let mut mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    mission.objectives = vec![MissionObjective {
        kind: MissionObjectiveKind::Protect,
        parameters: MissionObjectiveParameters::Protect { kind: worker },
        optional: true,
    }];
    let world = dying_workers_world(worker);
    let mut sim = Sim::with_mission(&world, two_player_setup(), mission);
    for _ in 0..70 {
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
fn a_survive_mission_replay_roundtrips() {
    // The brief's replay acceptance: a mission-carrying record of a Survive
    // mission round-trips — the codec carries the mission id (AUD-002),
    // the record validates, and the mission-carrying re-simulation through
    // the same `run_command_log_with_mission` seam the tools' verifier
    // uses reproduces every checkpoint and the final hash. The Survive
    // mission lives in an in-memory tree built on the real content (no
    // repository content file is added — the frozen tree is untouched),
    // and the tree's bundle identities are the registered goldens exactly
    // (a mission file changes nothing the content hash or map id reads).
    let sources = sources_with_probe(
        "survive_probe",
        "( kind: \"Survive\", parameters: Survive(( ticks: 300 )), optional: false, )",
    );
    let tree = ContentTree::load(&sources).expect("the tree with the survive probe loads");
    let mission_id = tree.mission_id("survive_probe");
    let mission = tree
        .mission_data("survive_probe", PlayerId(0))
        .expect("the probe resolves");
    let bundle = tree.bundle("crossroads_64").expect("crossroads_64 bundles");
    let world = bundle.world();
    // A mission file changes nothing the registered identities read: the
    // bundle's content hash and map id are the goldens exactly (the mission
    // category appends nothing to their encoders).
    assert_eq!(
        bundle.content_hash(),
        0x9bc1_8c52_1107_b262,
        "the content hash never moves"
    );
    assert_eq!(
        bundle.map_id(),
        0xd381_3640_1ab0_2ff1,
        "the map id never moves"
    );
    assert_ne!(
        mission_id, CROSSROADS_01_MISSION_ID,
        "the probe is its own mission (identity-bearing)"
    );

    // The recording: seed 7, two players, 400 idle ticks — the window at
    // 300 resolves inside the recording, and the checkpoints pin the same
    // hashed trail the mission-less path produces (derived state, A-066).
    let setup = two_player_setup();
    let run = run_command_log_with_mission(&world, &setup, &[], 400, Some(mission.clone()));
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        mission_id: Some(mission_id),
        seed: setup.seed,
        player_setup: setup.players.clone(),
        commands: Vec::new(),
        checkpoints: run
            .checkpoints
            .iter()
            .map(|&(tick, hash)| Checkpoint { tick, hash })
            .collect(),
        final_hash: run.final_hash,
    };
    replay.validate().expect("the record is structurally sound");

    // The codec round-trip: bytes carry the mission id; the decoded record
    // equals the written one field for field.
    let bytes = replay.encode();
    let decoded = ReplayFile::decode(&bytes).expect("the record decodes");
    assert_eq!(decoded, replay);
    assert_eq!(decoded.mission_id, Some(mission_id));

    // The re-simulation: mission-carrying, through the same seam — every
    // checkpoint and the final hash reproduce (the verifier's comparison,
    // run in-process; the CLI run lives in the B-017 report's verification
    // table).
    let resim = run_command_log_with_mission(&world, &setup, &[], 400, Some(mission));
    assert_eq!(
        resim.checkpoints, run.checkpoints,
        "every checkpoint reproduces"
    );
    assert_eq!(
        resim.final_hash, run.final_hash,
        "the final hash reproduces"
    );
}

// -------------------------------------------------------------------------
// B-018 — ReachArea + Gather (ADR-0201 Phase 2): the two new objective
// kinds against the REAL content tree and the REAL seam. Loader-level
// gates run over in-memory SourceMaps built on the real content (no
// content file is added to the repository — the frozen tree is untouched);
// behavioral acceptance runs over the real bundle world with real
// commands (Move marches a worker into the area, Gather drives the
// economy loop), the authored scale respected (gather amounts above the
// 200 starting ore).
// -------------------------------------------------------------------------

/// The B-018 geometry vocabulary, resolved through the real seam: the
/// rect's half-open law and the circle's inclusive boundary, observed at
/// the loader's tick-0 gate against the REAL starting forces (the
/// command center's center is exactly tile (14, 14) — anchor (12, 12)
/// offset (0, 0), footprint 4x4).
#[test]
fn reach_gates_pin_the_exact_boundaries_against_the_real_starting_forces() {
    // The rect [14, 15) x [14, 15) holds the command center's exact center
    // (14.0, 14.0) — the left/top edges are INCLUSIVE, so this area is
    // refused at tick 0 even though the point sits exactly ON the edge.
    let on_left_edge = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 14, y: 14, w: 1, h: 1 )) )), optional: true, )";
    let err = ContentTree::load(&sources_with_probe("reach_edge", on_left_edge)).unwrap_err();
    assert!(
        err.to_string().contains(
            "already contains the starting command_center of faction 'legion' at tick 0 \
             (tile 12x12)"
        ),
        "the inclusive left edge holds the CC's center exactly on it, got: {err}"
    );
    // One tile to the right the area is empty of the starting forces and
    // loads — the half-open law's other side.
    let one_right = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 15, y: 14, w: 1, h: 1 )) )), optional: true, )";
    ContentTree::load(&sources_with_probe("reach_right", one_right))
        .expect("one tile right of the edge holds nothing at tick 0");
    // A circle centered 500 milli-tiles left of the CC's center with
    // radius 500 puts the center EXACTLY on the boundary — inclusive, so
    // the area is refused at tick 0.
    let on_circle = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"ReachArea\", parameters: ReachArea(( area: Circle(( cx: 13500, cy: 14000, radius: 500 )) )), optional: true, )";
    let err = ContentTree::load(&sources_with_probe("reach_circle", on_circle)).unwrap_err();
    assert!(
        err.to_string()
            .contains("already contains the starting command_center of faction 'legion' at tick 0"),
        "the inclusive circle boundary holds the CC's center exactly on it, got: {err}"
    );
    // One milli-tile further left (distance 501 > 500) and the area is
    // legal — the boundary's other side, at the authored scale.
    let just_outside = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"ReachArea\", parameters: ReachArea(( area: Circle(( cx: 13499, cy: 14000, radius: 500 )) )), optional: true, )";
    let tree = ContentTree::load(&sources_with_probe("reach_out", just_outside))
        .expect("just outside the boundary holds nothing at tick 0");
    let resolved = tree
        .mission_data("reach_out", PlayerId(0))
        .expect("the probe resolves");
    assert_eq!(
        resolved.objectives[1].parameters,
        MissionObjectiveParameters::ReachArea {
            area: AreaSpec::Circle {
                cx: 13499,
                cy: 14000,
                radius: 500
            },
        },
        "the area crosses the seam verbatim"
    );
}

#[test]
fn every_reach_rejection_is_refused_against_the_real_tree() {
    // The brief's rejection list, one probe each over the real content
    // (crossroads_64 is 64x64): out of map bounds (both shapes), zero
    // extent, whole-map coverage, and the tick-0 guard — plus the legal
    // shape that loads.
    let destroy = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ";
    for (objectives, needle) in [
        // Bounds: the rect's span exits the 64x64 grid.
        (
            format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 60, y: 60, w: 8, h: 4 )) )), optional: true, )"),
            "reaches a rect [x 60, y 60, w 8, h 4] that leaves the map 'crossroads_64' (64x64 tiles)",
        ),
        // Bounds: the circle's disk exits the map (in milli-tiles).
        (
            format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Circle(( cx: 62000, cy: 32000, radius: 3000 )) )), optional: true, )"),
            "leaves the map 'crossroads_64'",
        ),
        // Zero extent: the empty rect.
        (
            format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 40, y: 40, w: 0, h: 4 )) )), optional: true, )"),
            "reaches a rect with w 0 x h 4",
        ),
        // Zero extent: the empty disk.
        (
            format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Circle(( cx: 32000, cy: 32000, radius: 0 )) )), optional: true, )"),
            "reaches a circle with radius 0",
        ),
        // Whole-map coverage: the exact grid match.
        (
            format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 0, y: 0, w: 64, h: 64 )) )), optional: true, )"),
            "covering the whole map 'crossroads_64'",
        ),
        // Whole-map coverage: a circle containing all four corners (the
        // diagonal is ~90499 milli-tiles).
        (
            format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Circle(( cx: 32000, cy: 32000, radius: 90500 )) )), optional: true, )"),
            "containing all four corners of the map 'crossroads_64'",
        ),
        // Tick 0: the rect around the mission player's workers (the
        // worker at anchor + (0, 4) stands at tile center (12.5, 16.5)).
        (
            format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 12, y: 16, w: 1, h: 1 )) )), optional: true, )"),
            "already contains the starting worker of faction 'legion' at tick 0 (tile 12x16)",
        ),
    ] {
        let err = ContentTree::load(&sources_with_probe("reach_bad", &objectives)).unwrap_err();
        assert!(
            err.to_string().contains(needle),
            "must be refused: {needle}, got: {err}"
        );
    }
    // The legal shape: a rect around the map's contested plaza, empty of
    // both players' starting forces — loads, and the seam resolves it
    // verbatim.
    let legal = format!("{destroy}( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 28, y: 29, w: 8, h: 4 )) )), optional: true, )");
    let tree = ContentTree::load(&sources_with_probe("reach_ok", &legal))
        .expect("the plaza rect is a legal reach area");
    let resolved = tree
        .mission_data("reach_ok", PlayerId(0))
        .expect("the probe resolves");
    assert_eq!(
        resolved.objectives[1].parameters,
        MissionObjectiveParameters::ReachArea {
            area: AreaSpec::Rect {
                x: 28,
                y: 29,
                w: 8,
                h: 4
            },
        },
        "the plaza area crosses the seam verbatim"
    );
}

#[test]
fn a_reach_objective_is_pending_at_tick_0_and_completes_when_a_worker_enters() {
    // The brief's behavioral acceptance, both shapes, on the real bundle
    // world with a real Move order: pending through tick 0 (the area is
    // far from the start), completed the first tick the worker's position
    // enters the area, the mission ending that tick for the mission
    // player with the enemy's structures still standing (NOT the
    // elimination rule), and the mission-less twin hashing identically at
    // every checked tick.
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let world = tree
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles")
        .world();
    let worker = worker_kind(&tree);
    for area in [
        AreaSpec::Rect {
            x: 28,
            y: 29,
            w: 8,
            h: 4,
        },
        AreaSpec::Circle {
            cx: 32000,
            cy: 31000,
            radius: 4000,
        },
    ] {
        let mut mission = tree
            .mission_data("crossroads_01", PlayerId(0))
            .expect("crossroads_01 resolves through the content seam");
        mission.objectives = vec![MissionObjective {
            kind: MissionObjectiveKind::ReachArea,
            parameters: MissionObjectiveParameters::ReachArea { area },
            optional: false,
        }];
        let setup = two_player_setup();
        let mut plain = Sim::new(&world, setup.clone());
        let mut sim = Sim::with_mission(&world, setup, mission);

        // Tick 0: pending — nothing stands inside, no events.
        let out = sim.step(&[]);
        plain.step(&[]);
        assert!(
            !out.events.iter().any(|event| matches!(
                event,
                Event::ObjectiveState { .. } | Event::MatchEnded { .. }
            )),
            "tick 0: pending, got {:?}",
            out.events
        );

        // A real Move order: march the farthest worker (16.5, 16.5) into
        // the area's heart.
        let marcher = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.owner == PlayerId(0) && entity.kind == worker)
            .expect("the Legion fields workers")
            .id;
        let target = match area {
            AreaSpec::Rect { x, y, w, h } => Vec2Fx::from_ints(x + w as i32 / 2, y + h as i32 / 2),
            AreaSpec::Circle { cx, cy, .. } => Vec2Fx::new(Fx::from_milli(cx), Fx::from_milli(cy)),
        };
        let order = Command::new(
            PlayerId(0),
            sim.tick(),
            1,
            CommandKind::Move {
                units: vec![marcher],
                target,
            },
        );
        // The twin marches too — the same command stream, so the hashes
        // compare like for like.
        sim.step(std::slice::from_ref(&order));
        plain.step(&[order]);

        // March: the objective completes the tick the worker's position
        // enters the area, and the match ends that tick. The loop's step
        // evaluates the sim's CURRENT tick (stage 10 runs before stage
        // 11's increment), so the evaluated tick is captured before each
        // step.
        let mut resolved_tick = None;
        for _ in 1..=1500u32 {
            let evaluated = sim.tick();
            let out = sim.step(&[]);
            plain.step(&[]);
            if out.events.iter().any(|event| {
                matches!(
                    event,
                    Event::ObjectiveState {
                        index: 0,
                        state: ObjectiveState::Completed
                    }
                )
            }) {
                assert!(
                    out.events.iter().any(|event| matches!(
                        event,
                        Event::MatchEnded {
                            winner: PlayerId(0)
                        }
                    )),
                    "the required completion ends the match for the mission player"
                );
                let outcome = sim.outcome().expect("the match resolved");
                assert_eq!(outcome.winner, PlayerId(0));
                assert_eq!(outcome.ended_tick, evaluated);
                // The marcher really is inside the area at the resolving
                // tick, and the enemy's structures stand (NOT elimination).
                let marcher_pos = sim
                    .snapshot()
                    .entities
                    .iter()
                    .find(|entity| entity.id == marcher)
                    .expect("the marcher lives")
                    .pos;
                assert!(
                    area.contains(marcher_pos),
                    "the marcher stands inside the area at the resolving tick"
                );
                resolved_tick = Some(evaluated);
                break;
            }
            if evaluated.is_multiple_of(100) {
                assert_eq!(
                    sim.state_hash(),
                    plain.state_hash(),
                    "tick {evaluated}: the mission state is derived, never hashed"
                );
            }
        }
        resolved_tick.expect("the marcher reaches the area within the tick budget");
        // The twin stepped in lockstep through the same command stream:
        // identical hashes at the resolving tick itself (A-066).
        assert_eq!(
            sim.state_hash(),
            plain.state_hash(),
            "the resolving tick: identical hashes"
        );
    }
}

#[test]
fn every_gather_rejection_is_refused_against_the_real_tree() {
    // The brief's rejection list over the real content: the amount at or
    // below the effective starting ore (200, both semantics), and the
    // unknown semantics spelling; the first legal amount (201) loads and
    // resolves through the seam with the ore ResourceId (the registry's
    // first resource) and the authored semantics.
    let destroy = "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ";
    for (amount, semantics, needle) in [
        (
            200i64,
            "Cumulative",
            "gathers 200 ore, at or below the starting ore 200 of ruleset 'alpha' (both semantics)",
        ),
        (
            200,
            "Balance",
            "gathers 200 ore, at or below the starting ore 200 of ruleset 'alpha' (both semantics)",
        ),
        (
            199,
            "Cumulative",
            "gathers 199 ore, at or below the starting ore 200 of ruleset 'alpha' (both semantics)",
        ),
    ] {
        let objectives = format!(
            "{destroy}( kind: \"Gather\", parameters: Gather(( amount: {amount}, semantics: \"{semantics}\" )), optional: true, )"
        );
        let err = ContentTree::load(&sources_with_probe("gather_bad", &objectives)).unwrap_err();
        assert!(
            err.to_string().contains(needle),
            "must be refused: {needle}, got: {err}"
        );
    }
    // Unknown semantics spellings are loud parse errors naming the
    // vocabulary (never silently defaulted).
    let objectives = format!(
        "{destroy}( kind: \"Gather\", parameters: Gather(( amount: 400, semantics: \"Total\" )), optional: true, )"
    );
    let err = ContentTree::load(&sources_with_probe("gather_spelling", &objectives)).unwrap_err();
    assert!(
        err.to_string().contains("unknown gather semantics 'Total'"),
        "got: {err}"
    );
    // The first legal amount, both semantics: the seam resolves the ore
    // ResourceId (the registry's first and only resource — id 0) and the
    // authored semantics, exactly as the world's ledger carries them.
    for (semantics, expected) in [
        ("Cumulative", GatherSemantics::Cumulative),
        ("Balance", GatherSemantics::Balance),
    ] {
        let objectives = format!(
            "{destroy}( kind: \"Gather\", parameters: Gather(( amount: 201, semantics: \"{semantics}\" )), optional: true, )"
        );
        let tree = ContentTree::load(&sources_with_probe("gather_ok", &objectives))
            .expect("201 is the first legal amount");
        let resolved = tree
            .mission_data("gather_ok", PlayerId(0))
            .expect("the probe resolves");
        assert_eq!(
            resolved.objectives[1].parameters,
            MissionObjectiveParameters::Gather {
                resource: ResourceId(0),
                amount: 201,
                semantics: expected,
            },
            "semantics {semantics}"
        );
    }
}

#[test]
fn a_balance_gather_objective_completes_on_the_first_real_delivery() {
    // The brief's behavioral acceptance at the authored scale, Balance
    // semantics: the mission player's ledger starts at 200 (the Alpha's
    // starting ore), so amount 201 completes on the first delivery's tick
    // (200 + 10 = 210 >= 201) — the same step as the ResourceDelivered
    // event, which precedes ObjectiveState in the outward stream, and the
    // match ends for the mission player while every structure stands
    // (NOT elimination). Real content world, real Gather order.
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let world = tree
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles")
        .world();
    let worker = worker_kind(&tree);
    let mut mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    mission.objectives = vec![MissionObjective {
        kind: MissionObjectiveKind::Gather,
        parameters: MissionObjectiveParameters::Gather {
            resource: ResourceId(0),
            amount: 201,
            semantics: GatherSemantics::Balance,
        },
        optional: false,
    }];
    let mut sim = Sim::with_mission(&world, two_player_setup(), mission);

    // A real Gather order: the nearest worker to the (17, 17) node (its
    // center is tile (18, 18)) — the worker at tile center (16.5, 16.5),
    // the Legion's fourth starting worker.
    let node = sim
        .snapshot()
        .entities
        .iter()
        .find(|entity| {
            entity.kind.0
                == tree
                    .entities
                    .iter()
                    .position(|def| def.id == "ore_node")
                    .expect("ore_node is registered") as u32
                && entity.pos == Vec2Fx::new(Fx::from_milli(18000), Fx::from_milli(18000))
        })
        .expect("the (17, 17) node stands")
        .id;
    let marcher = sim
        .snapshot()
        .entities
        .iter()
        .find(|entity| entity.owner == PlayerId(0) && entity.kind == worker)
        .expect("the Legion fields workers")
        .id;
    sim.step(&[Command::new(
        PlayerId(0),
        0,
        1,
        CommandKind::Gather {
            units: vec![marcher],
            node,
        },
    )]);

    let mut resolved_tick = None;
    for tick in 1..=1200u32 {
        let out = sim.step(&[]);
        let delivered_at = out
            .events
            .iter()
            .position(|event| matches!(event, Event::ResourceDelivered { owner, .. } if *owner == PlayerId(0)));
        if let Some(delivered_at) = delivered_at {
            // The delivery completes the objective THIS tick, after the
            // ResourceDelivered event in the same step's stream.
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
                .expect("the objective resolved on the delivery tick");
            assert!(
                delivered_at < resolved_at,
                "ResourceDelivered precedes ObjectiveState: {:?}",
                out.events
            );
            let outcome = sim.outcome().expect("the match resolved");
            assert_eq!(outcome.winner, PlayerId(0));
            assert_eq!(outcome.ended_tick, tick);
            resolved_tick = Some(tick);
            break;
        }
    }
    assert!(
        resolved_tick.is_some(),
        "the worker delivers within the tick budget"
    );
}

#[test]
fn a_cumulative_gather_objective_completes_at_the_authored_scale() {
    // The cumulative semantics at the authored scale: amount 201 needs
    // 21 ten-ore trips, so all four starting workers gather the two nodes
    // beside the start. The objective completes the tick the 21st
    // delivery lands (210 >= 201) and the mission player wins.
    let tree = ContentTree::load_dir(&repo_content()).expect("the repository content loads");
    let world = tree
        .bundle("crossroads_64")
        .expect("crossroads_64 bundles")
        .world();
    let worker = worker_kind(&tree);
    let node_kind = tree
        .entities
        .iter()
        .position(|def| def.id == "ore_node")
        .expect("ore_node is registered") as u32;
    let mut mission = tree
        .mission_data("crossroads_01", PlayerId(0))
        .expect("crossroads_01 resolves through the content seam");
    mission.objectives = vec![MissionObjective {
        kind: MissionObjectiveKind::Gather,
        parameters: MissionObjectiveParameters::Gather {
            resource: ResourceId(0),
            amount: 201,
            semantics: GatherSemantics::Cumulative,
        },
        optional: false,
    }];
    let mut sim = Sim::with_mission(&world, two_player_setup(), mission);

    // Split the four workers over the two nearest nodes: (7, 17) center
    // (8, 18) and (17, 17) center (18, 18).
    let snapshot = sim.snapshot();
    let workers: Vec<EntityId> = snapshot
        .entities
        .iter()
        .filter(|entity| entity.owner == PlayerId(0) && entity.kind == worker)
        .map(|entity| entity.id)
        .collect();
    assert_eq!(workers.len(), 4, "the Legion starts four workers");
    let node_at = |milli_x: i32, milli_y: i32| {
        snapshot
            .entities
            .iter()
            .find(|entity| {
                entity.kind.0 == node_kind
                    && entity.pos == Vec2Fx::new(Fx::from_milli(milli_x), Fx::from_milli(milli_y))
            })
            .expect("the node stands")
            .id
    };
    let node_near = node_at(8000, 18000);
    let node_far = node_at(18000, 18000);
    let mut commands = Vec::new();
    for (index, worker_id) in workers.iter().enumerate() {
        let node = if index < 2 { node_near } else { node_far };
        commands.push(Command::new(
            PlayerId(0),
            0,
            index as u32 + 1,
            CommandKind::Gather {
                units: vec![*worker_id],
                node,
            },
        ));
    }
    sim.step(&commands);

    let mut delivered = 0i64;
    for tick in 1..=4000u32 {
        let out = sim.step(&[]);
        for event in &out.events {
            if let Event::ResourceDelivered { owner, amount, .. } = event {
                if *owner == PlayerId(0) {
                    delivered += *amount as i64;
                }
            }
        }
        if delivered >= 201 {
            let outcome = sim.outcome().expect("the match resolved");
            assert_eq!(outcome.winner, PlayerId(0));
            assert_eq!(outcome.ended_tick, tick);
            assert_eq!(delivered, 210, "the 21st trip completes the objective");
            return;
        }
        assert!(
            sim.outcome().is_none(),
            "pending until the 21st delivery (delivered {delivered})"
        );
    }
    panic!("the workers deliver {delivered} ore within the budget — below 201");
}

#[test]
fn a_reach_and_gather_mission_replay_roundtrips() {
    // The brief's replay acceptance: a mission-carrying record of a
    // Reach + Gather mission round-trips — the codec carries the mission
    // id (AUD-002), the record validates, and the mission-carrying
    // re-simulation through `run_command_log_with_mission` reproduces
    // every checkpoint and the final hash, resolving the reach objective
    // from the recorded Move order exactly as the recording did. The
    // probe mission lives in an in-memory tree built on the real content
    // (no repository content file is added — the frozen tree is
    // untouched), and the tree's bundle identities are the registered
    // goldens exactly.
    let sources = sources_with_probe(
        "reach_gather_probe",
        "( kind: \"ReachArea\", parameters: ReachArea(( area: Rect(( x: 24, y: 24, w: 4, h: 4 )) )), optional: false, ),\n        ( kind: \"Gather\", parameters: Gather(( amount: 210, semantics: \"Cumulative\" )), optional: true, )",
    );
    let tree = ContentTree::load(&sources).expect("the tree with the probe loads");
    let mission_id = tree.mission_id("reach_gather_probe");
    let mission = tree
        .mission_data("reach_gather_probe", PlayerId(0))
        .expect("the probe resolves");
    let bundle = tree.bundle("crossroads_64").expect("crossroads_64 bundles");
    let world = bundle.world();
    // The probe tree's bundle identities are the registered goldens
    // exactly — a mission file moves nothing.
    assert_eq!(
        bundle.content_hash(),
        0x9bc1_8c52_1107_b262,
        "the content hash never moves"
    );
    assert_eq!(
        bundle.map_id(),
        0xd381_3640_1ab0_2ff1,
        "the map id never moves"
    );
    assert_ne!(
        mission_id, CROSSROADS_01_MISSION_ID,
        "the probe is its own mission (identity-bearing)"
    );

    // The recording: seed 7, two players, 400 ticks with one Move order
    // — the mission player's farthest worker (16.5, 16.5) marches into
    // the rect [24, 28) x [24, 28), resolving the required reach inside
    // the recording; the optional gather stays pending (no gathering
    // order), feeding the end screen.
    let setup = two_player_setup();
    let worker = worker_kind(&tree);
    // The marcher's entity id: initial spawns take ids 1..N in the
    // fixture's fixed order (players in map-start order, each start's
    // forces in faction order, then the nodes) — the world seam's
    // documented determinism contract, so the spawn's INDEX is its id
    // minus one.
    let marcher = EntityId(
        1 + world
            .initial_spawns
            .iter()
            .position(|spawn| {
                spawn.owner == PlayerId(0)
                    && spawn.kind == worker
                    && spawn.pos == Vec2Fx::new(Fx::from_milli(16500), Fx::from_milli(16500))
            })
            .expect("the farthest starting worker stands at (16.5, 16.5)") as u64,
    );
    let command = Command::new(
        PlayerId(0),
        0,
        1,
        CommandKind::Move {
            units: vec![marcher],
            target: Vec2Fx::from_ints(26, 26),
        },
    );
    let run = run_command_log_with_mission(
        &world,
        &setup,
        std::slice::from_ref(&command),
        400,
        Some(mission.clone()),
    );
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        mission_id: Some(mission_id),
        seed: setup.seed,
        player_setup: setup.players.clone(),
        commands: vec![command.clone()],
        checkpoints: run
            .checkpoints
            .iter()
            .map(|&(tick, hash)| Checkpoint { tick, hash })
            .collect(),
        final_hash: run.final_hash,
    };
    replay.validate().expect("the record is structurally sound");

    // The codec round-trip: bytes carry the mission id; the decoded
    // record equals the written one field for field.
    let bytes = replay.encode();
    let decoded = ReplayFile::decode(&bytes).expect("the record decodes");
    assert_eq!(decoded, replay);
    assert_eq!(decoded.mission_id, Some(mission_id));

    // The re-simulation: mission-carrying, through the same seam — every
    // checkpoint and the final hash reproduce (the verifier's
    // comparison, run in-process; the CLI run lives in the B-018
    // report's verification table).
    let resim = run_command_log_with_mission(&world, &setup, &[command], 400, Some(mission));
    assert_eq!(
        resim.checkpoints, run.checkpoints,
        "every checkpoint reproduces"
    );
    assert_eq!(
        resim.final_hash, run.final_hash,
        "the final hash reproduces"
    );
}

// -------------------------------------------------------------------------
// B-019 — Faction variants, inline (ADR-0201 §3, the owner's binding
// decision: overrides live in the mission file, never a new content file).
// The acceptance runs against the REAL tree, the REAL seam, and the REAL
// simulation: the restricted roster gates Build and Train through the exact
// world fields the mission-aware bundle bakes; the scripted AI derives its
// whole plan from the effective faction's production lists (no AI code
// change); the overrides' identity rides the mission id; and the mission
// without overrides keeps its world, its id, and its goldens. Loader-level
// gates run over in-memory SourceMaps built on the real content — the
// frozen tree gains no file (B-017's discipline).
// -------------------------------------------------------------------------

/// The B-019 probe faction restriction: the Legion field minus raider,
/// guardian, and turret. Every producer and the AI's build order stay
/// fieldable (worker, rifleman, command_center, barracks, supply_depot),
/// so the restricted mission plays like a real mission — the point under
/// proof is what the AI and the gates do with the kinds that LEFT.
const RESTRICTED_ROSTER: &str = r#"    faction_overrides: (
        roster: Some([
            "worker", "rifleman", "command_center", "barracks", "supply_depot",
        ]),
    ),"#;

/// A probe mission carrying a faction_overrides block: the given overrides
/// text and objective list over the real content's closed reference set.
fn probe_mission_with_overrides(id: &str, overrides: &str, objectives: &str) -> String {
    format!(
        r#"
(
    schema_version: 1,
    id: "{id}",
    display_name: "Probe",
    map: "crossroads_64",
    faction: "legion",
    rules: "alpha",
{overrides}
    objectives: [
{objectives}
    ],
)
"#
    )
}

/// The real content sources with one restricted-roster probe mission added
/// in memory (Destroy-only objectives — the AI acceptance's carrier).
fn sources_with_restricted_roster(id: &str) -> SourceMap {
    SourceMap::from_dir(&repo_content())
        .expect("the real content reads")
        .mission(
            &format!("{id}.ron"),
            &probe_mission_with_overrides(
                id,
                RESTRICTED_ROSTER,
                "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, )",
            ),
        )
}

/// An entity kind's id, by name, through the tree's entity order.
fn kind_of(tree: &ContentTree, name: &str) -> KindId {
    KindId(
        tree.entities
            .iter()
            .position(|entity| entity.id == name)
            .unwrap_or_else(|| panic!("{name} exists in the tree")) as u32,
    )
}

/// The kinds a restricted mission's command log must never Build or Train.
const EXCLUDED_KINDS: [&str; 3] = ["raider", "guardian", "turret"];

#[test]
fn a_restricted_missions_world_carries_the_effective_faction() {
    // The mission-aware bundle: the probe's roster override replaces the
    // Legion's, the effective production filters raider and guardian out of
    // the barracks' list, the starting forces stay the base faction's (the
    // override named none), and the canonical identities — the content hash
    // golden and the map id — stay the tree's. The plain bundle and the
    // shared tree are untouched.
    let tree = ContentTree::load(&sources_with_restricted_roster("restricted_world"))
        .expect("the restricted probe loads");
    let plain_world = tree.bundle("crossroads_64").expect("plain bundle").world();
    let bundle = tree
        .bundle_for_mission("restricted_world")
        .expect("the mission-aware bundle builds");
    let world = bundle.world();

    let in_roster = [
        "worker",
        "rifleman",
        "command_center",
        "barracks",
        "supply_depot",
    ]
    .map(|name| kind_of(&tree, name));
    for kind in in_roster {
        assert!(
            world.roster.contains(&kind),
            "the effective roster keeps every kind the override names"
        );
    }
    for name in EXCLUDED_KINDS {
        assert!(
            !world.roster.contains(&kind_of(&tree, name)),
            "{name} left the effective roster"
        );
    }
    // The barracks' effective production list: rifleman only — raider and
    // guardian died with their roster membership; the command center still
    // trains workers.
    let barracks = kind_of(&tree, "barracks");
    let command_center = kind_of(&tree, "command_center");
    assert_eq!(
        world.production,
        vec![
            (barracks, vec![kind_of(&tree, "rifleman")]),
            (command_center, vec![kind_of(&tree, "worker")]),
        ],
        "the effective production is the base lists filtered to the effective roster"
    );
    assert_eq!(
        world.initial_spawns, plain_world.initial_spawns,
        "a roster-only override leaves the starting forces standing"
    );
    // The canonical identities stay the tree's (the overrides are mission
    // data — their identity rides the mission id), and the tree itself is
    // never mutated: the plain world is the Legion's, byte for byte.
    assert_eq!(bundle.content_hash(), 0x9bc1_8c52_1107_b262);
    assert_eq!(bundle.map_id(), 0xd381_3640_1ab0_2ff1);
    assert_eq!(
        plain_world.roster,
        [
            "worker",
            "rifleman",
            "raider",
            "guardian",
            "command_center",
            "barracks",
            "supply_depot",
            "turret",
        ]
        .map(|name| kind_of(&tree, name))
        .to_vec()
    );
    assert_eq!(plain_world.production.len(), 2);
}

#[test]
fn a_restricted_roster_rejects_build_and_train_of_excluded_kinds() {
    // The brief's gate acceptance, end to end on the REAL mission world:
    // Build of an excluded structure is refused by the B-016 roster gate
    // (InvalidTarget, before economy and placement — no site, no ore); Train
    // of an excluded unit is refused by the effective production list (the
    // Train gate's only authority) — proven with a barracks scheduled into
    // the world (the construction itself is not the point under proof; the
    // gate is), and the in-roster controls are accepted.
    let tree = ContentTree::load(&sources_with_restricted_roster("restricted_gates"))
        .expect("the restricted probe loads");
    let bundle = tree
        .bundle_for_mission("restricted_gates")
        .expect("the mission-aware bundle builds");
    let mut world = bundle.world();
    let mission = tree
        .mission_data("restricted_gates", PlayerId(0))
        .expect("the probe resolves");

    // A barracks stands from tick 1 — the producer whose effective list the
    // Train gate reads.
    world.scheduled_spawns.push(ScheduledSpawnDef {
        tick: 1,
        spawn: SpawnDef {
            owner: PlayerId(0),
            kind: kind_of(&tree, "barracks"),
            pos: Vec2Fx::from_ints(20, 14),
        },
    });

    let setup = two_player_setup();
    let mut sim = Sim::with_mission(&world, setup, mission);
    let worker = sim
        .player_view(PlayerId(0))
        .entities
        .iter()
        .find(|entity| entity.kind == kind_of(&tree, "worker"))
        .map(|entity| entity.id)
        .expect("the mission player fields a worker");
    let command_center = sim
        .player_view(PlayerId(0))
        .entities
        .iter()
        .find(|entity| entity.kind == kind_of(&tree, "command_center"))
        .map(|entity| entity.id)
        .expect("the mission player fields a command center");

    // Tick 0: the restricted Build is refused; the in-roster control is
    // accepted (the same worker, the same purse).
    let build_turret = Command::new(
        PlayerId(0),
        sim.tick(),
        1,
        CommandKind::Build {
            worker,
            structure: kind_of(&tree, "turret"),
            at: TilePos { x: 18, y: 14 },
        },
    );
    let out = sim.step(std::slice::from_ref(&build_turret));
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::CommandRejected {
                issuer: PlayerId(0),
                seq: 1,
                reject: Reject {
                    reason: RejectReason::InvalidTarget,
                    ..
                },
                ..
            }
        )),
        "an excluded structure is refused by the roster gate: {:?}",
        out.events
    );
    let build_depot = Command::new(
        PlayerId(0),
        sim.tick(),
        2,
        CommandKind::Build {
            worker,
            structure: kind_of(&tree, "supply_depot"),
            at: TilePos { x: 18, y: 14 },
        },
    );
    let out = sim.step(std::slice::from_ref(&build_depot));
    assert!(
        !out.events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { seq: 2, .. })),
        "an in-roster structure builds: {:?}",
        out.events
    );

    // Tick 2+: the barracks stands; the excluded unit is refused by the
    // effective production list, the in-roster unit trains.
    let mut barracks = None;
    for _ in 2..10 {
        let view = sim.player_view(PlayerId(0));
        if let Some(entity) = view
            .entities
            .iter()
            .find(|entity| entity.kind == kind_of(&tree, "barracks"))
        {
            barracks = Some(entity.id);
            break;
        }
        sim.step(&[]);
    }
    let barracks = barracks.expect("the scheduled barracks stands");
    let train_raider = Command::new(
        PlayerId(0),
        sim.tick(),
        3,
        CommandKind::Train {
            producer: barracks,
            unit: kind_of(&tree, "raider"),
        },
    );
    let out = sim.step(std::slice::from_ref(&train_raider));
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::CommandRejected {
                issuer: PlayerId(0),
                seq: 3,
                reject: Reject {
                    reason: RejectReason::MissingCapability,
                    ..
                },
                ..
            }
        )),
        "an excluded unit is refused by the effective production list: {:?}",
        out.events
    );
    let train_rifleman = Command::new(
        PlayerId(0),
        sim.tick(),
        4,
        CommandKind::Train {
            producer: barracks,
            unit: kind_of(&tree, "rifleman"),
        },
    );
    let out = sim.step(std::slice::from_ref(&train_rifleman));
    assert!(
        !out.events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { seq: 4, .. })),
        "an in-roster unit trains through the same gate: {:?}",
        out.events
    );
    let _ = command_center; // the mission player's opening economy stands
}

#[test]
fn the_ai_on_a_restricted_mission_never_commands_an_excluded_kind() {
    // Work item 5's proof, at acceptance scale: the scripted opponent runs
    // BOTH slots on the restricted mission (the headless shape — perceive,
    // decide, act, through the same controller contract the tools run), and
    // the command log never contains a Build or Train of an excluded kind.
    // No AI code changed: the plan derives from the effective faction's
    // production lists — pinned directly below — so the army list is the
    // filtered one and the AI never even CONSIDERS the excluded kinds.
    let tree = ContentTree::load(&sources_with_restricted_roster("restricted_ai"))
        .expect("the restricted probe loads");
    let bundle = tree
        .bundle_for_mission("restricted_ai")
        .expect("the mission-aware bundle builds");
    let world = bundle.world();

    // The plan-level pin: alpha_plan reads the faction's production lists,
    // so on the mission-aware bundle the army is the filtered list and the
    // structures are the in-roster ones. (The AI crate is untouched; this
    // is the derivation the brief claims, held against the restricted
    // content.)
    let plan = pandemonium_engine::alpha_plan(&bundle, &world, PlayerId(0), 7, None);
    assert_eq!(
        plan.army,
        vec![kind_of(&tree, "rifleman")],
        "the AI's army is the effective production list, not the Legion's"
    );
    assert_eq!(plan.barracks, Some(kind_of(&tree, "barracks")));
    assert_eq!(plan.command_center, kind_of(&tree, "command_center"));
    assert_eq!(plan.worker, kind_of(&tree, "worker"));
    assert_eq!(plan.depot, Some(kind_of(&tree, "supply_depot")));

    // The match: both slots scripted, the mission riding the sim.
    let mission = tree
        .mission_data("restricted_ai", PlayerId(0))
        .expect("the probe resolves");
    let setup = MatchSetup {
        seed: 7,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Ai,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Ai,
            },
        ],
    };
    let mut sim = Sim::with_mission(&world, setup, mission);
    let mut controllers: Vec<(PlayerId, Box<dyn pandemonium_ai::Controller>)> = vec![
        (
            PlayerId(0),
            Box::new(pandemonium_engine::alpha_controller(
                &bundle,
                &world,
                PlayerId(0),
                7,
                None,
            )),
        ),
        (
            PlayerId(1),
            Box::new(pandemonium_engine::alpha_controller(
                &bundle,
                &world,
                PlayerId(1),
                7,
                None,
            )),
        ),
    ];

    // The headless drive: perceive (fog-filtered view), decide, act — the
    // AiMatchHost contract, stepped here against Sim::with_mission.
    let mut log: Vec<Command> = Vec::new();
    for tick in 0..4500u32 {
        let mut feed = Vec::new();
        for (player, controller) in controllers.iter_mut() {
            let view = sim.player_view(*player);
            controller.think(&view, tick, &mut feed);
        }
        log.extend(feed.iter().cloned());
        if sim.is_finished() {
            break;
        }
        sim.step(&feed);
    }

    // The acceptance: no Build or Train of an excluded kind, from either
    // slot, in 4500 ticks of scripted play.
    let excluded: Vec<KindId> = EXCLUDED_KINDS.map(|name| kind_of(&tree, name)).to_vec();
    let offenders: Vec<&Command> = log
        .iter()
        .filter(|command| match &command.kind {
            CommandKind::Build { structure, .. } => excluded.contains(structure),
            CommandKind::Train { unit, .. } => excluded.contains(unit),
            _ => false,
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "the AI never commands an excluded kind; offenders: {offenders:?}"
    );
    // And the run is a real one: the AI built structures and trained units
    // of the kinds the restriction keeps — the restriction restricted play,
    // it did not silence it.
    let built: Vec<KindId> = log
        .iter()
        .filter_map(|command| match &command.kind {
            CommandKind::Build { structure, .. } => Some(*structure),
            _ => None,
        })
        .collect();
    let trained: Vec<KindId> = log
        .iter()
        .filter_map(|command| match &command.kind {
            CommandKind::Train { unit, .. } => Some(*unit),
            _ => None,
        })
        .collect();
    assert!(
        built.contains(&kind_of(&tree, "barracks")),
        "the AI builds the barracks its plan derives"
    );
    assert!(
        trained.contains(&kind_of(&tree, "rifleman")),
        "the AI trains the one army kind the effective list keeps"
    );
}

#[test]
fn a_restricted_mission_replay_roundtrips() {
    // The replay acceptance: a mission-carrying record of the restricted
    // mission round-trips — the codec carries the override mission's id
    // (AUD-002), the record's content hash is the EFFECTIVE world's fixture
    // hash (the filtered production is world identity, as it always was),
    // and the mission-carrying re-simulation through the mission-aware
    // world reproduces every checkpoint and the final hash.
    let tree = ContentTree::load(&sources_with_restricted_roster("restricted_replay"))
        .expect("the restricted probe loads");
    let mission_id = tree.mission_id("restricted_replay");
    assert_ne!(
        mission_id, CROSSROADS_01_MISSION_ID,
        "the restricted probe is its own mission"
    );
    assert_eq!(
        tree.mission_id("restricted_replay"),
        mission_id,
        "the mission id is a pure function of the mission bytes"
    );
    let mission = tree
        .mission_data("restricted_replay", PlayerId(0))
        .expect("the probe resolves");
    let bundle = tree
        .bundle_for_mission("restricted_replay")
        .expect("the mission-aware bundle builds");
    let world = bundle.world();
    // The fixture hash moved with the effective production (it always
    // hashed that field) — and the mission id beside it explains why.
    let plain_world = tree.bundle("crossroads_64").expect("plain bundle").world();
    assert_ne!(
        world.content_hash(),
        plain_world.content_hash(),
        "the effective world is a different match identity"
    );

    // The recording: seed 7, two human slots, one Move order (the reach
    // probe's marcher shape — the farthest starting worker), 400 ticks.
    let setup = two_player_setup();
    let worker = kind_of(&tree, "worker");
    let marcher = EntityId(
        1 + world
            .initial_spawns
            .iter()
            .position(|spawn| {
                spawn.owner == PlayerId(0)
                    && spawn.kind == worker
                    && spawn.pos == Vec2Fx::new(Fx::from_milli(16500), Fx::from_milli(16500))
            })
            .expect("the farthest starting worker stands at (16.5, 16.5)") as u64,
    );
    let command = Command::new(
        PlayerId(0),
        0,
        1,
        CommandKind::Move {
            units: vec![marcher],
            target: Vec2Fx::from_ints(26, 26),
        },
    );
    let run = run_command_log_with_mission(
        &world,
        &setup,
        std::slice::from_ref(&command),
        400,
        Some(mission.clone()),
    );
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        mission_id: Some(mission_id),
        seed: setup.seed,
        player_setup: setup.players.clone(),
        commands: vec![command.clone()],
        checkpoints: run
            .checkpoints
            .iter()
            .map(|&(tick, hash)| Checkpoint { tick, hash })
            .collect(),
        final_hash: run.final_hash,
    };
    replay.validate().expect("the record is structurally sound");
    let bytes = replay.encode();
    let decoded = ReplayFile::decode(&bytes).expect("the record decodes");
    assert_eq!(decoded, replay);
    assert_eq!(decoded.mission_id, Some(mission_id));
    // The re-simulation: through the same mission-aware world and the same
    // mission seam — every checkpoint and the final hash reproduce.
    let resim = run_command_log_with_mission(&world, &setup, &[command], 400, Some(mission));
    assert_eq!(resim.checkpoints, run.checkpoints);
    assert_eq!(resim.final_hash, run.final_hash);
}

#[test]
fn every_faction_override_rejection_is_refused_against_the_real_tree() {
    // The loader gates (AUD-003) against the REAL tree, in memory: a roster
    // override naming a kind the Legion does not field (the ore node is a
    // real entity); a BuildTrain objective whose target left the effective
    // roster; a BuildTrain objective nothing fieldable produces anymore;
    // a Protect objective whose kind left the effective starting forces;
    // and an empty replacement forces list. Each refusal names the law.
    // (crossroads_01 itself is the no-override control — its id pin is the
    // suite's standing test.)

    // 1. The subset gate: ore_node exists in the tree but the Legion does
    //    not field it — an override restricts, it never adds.
    let sources = SourceMap::from_dir(&repo_content())
        .expect("the real content reads")
        .mission(
            "subset_bad.ron",
            &probe_mission_with_overrides(
                "subset_bad",
                r#"    faction_overrides: ( roster: Some(["worker", "ore_node"]) ),"#,
                "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, )",
            ),
        );
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string().contains(
            "mission 'subset_bad' overrides the roster of faction 'legion' with kind(s) \
             'ore_node' the base roster does not carry"
        ),
        "got: {err}"
    );

    // 2. BuildTrain outside the effective roster: the turret left with the
    //    restriction, so "build turrets" is unsatisfiable — refused at load.
    let sources = SourceMap::from_dir(&repo_content())
        .expect("the real content reads")
        .mission(
            "buildtrain_bad.ron",
            &probe_mission_with_overrides(
                "buildtrain_bad",
                RESTRICTED_ROSTER,
                "( kind: \"BuildTrain\", parameters: BuildTrain(( kind: \"turret\", count: 2 )), optional: true, )",
            ),
        );
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string().contains(
            "mission 'buildtrain_bad' builds/trains kind 'turret' that the effective \
             roster of faction 'legion' does not carry"
        ),
        "got: {err}"
    );

    // 3. BuildTrain stranded by the production filter: the raider needs the
    //    barracks, but this restriction keeps the barracks WITHOUT the
    //    worker — no builder exists, the barracks can never stand, and the
    //    raider is unreachable. (The worker-left case is refusal 2's shape;
    //    this is the producer-side half: the target is fieldable but its
    //    producer cannot be obtained.)
    let sources = SourceMap::from_dir(&repo_content())
        .expect("the real content reads")
        .mission(
            "producer_bad.ron",
            &probe_mission_with_overrides(
                "producer_bad",
                r#"    faction_overrides: (
        roster: Some(["raider", "guardian", "rifleman", "command_center", "supply_depot", "turret"]),
    ),"#,
                "( kind: \"BuildTrain\", parameters: BuildTrain(( kind: \"raider\", count: 1 )), optional: true, )",
            ),
        );
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string().contains(
            "mission 'producer_bad' builds/trains kind 'raider' that nothing in the \
             effective faction 'legion' can produce"
        ),
        "got: {err}"
    );

    // 4. Protect against the EFFECTIVE forces: the Legion starts with
    //    workers, but this mission's forces do not — protecting one is the
    //    vacuous-failure shape, refused.
    let sources = SourceMap::from_dir(&repo_content())
        .expect("the real content reads")
        .mission(
            "protect_bad.ron",
            &probe_mission_with_overrides(
                "protect_bad",
                r#"    faction_overrides: (
        starting_forces: Some([( entity: "command_center", offset: ( x: 0, y: 0 ) )]),
    ),"#,
                "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, ),\n        ( kind: \"Protect\", parameters: Protect(( kind: \"worker\" )), optional: true, )",
            ),
        );
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string().contains(
            "mission 'protect_bad' protects kind 'worker' that the starting forces of \
             faction 'legion' do not carry"
        ),
        "got: {err}"
    );

    // 5. The empty replacement forces list — every faction fields at least
    //    one starting entity (plan §9.7).
    let sources = SourceMap::from_dir(&repo_content())
        .expect("the real content reads")
        .mission(
            "forces_bad.ron",
            &probe_mission_with_overrides(
                "forces_bad",
                "    faction_overrides: ( starting_forces: Some([]) ),",
                "( kind: \"Destroy\", parameters: Destroy(( target_kind: None )), optional: false, )",
            ),
        );
    let err = ContentTree::load(&sources).unwrap_err();
    assert!(
        err.to_string()
            .contains("faction_overrides starting_forces is empty"),
        "got: {err}"
    );

    // 6. The control: the same restriction with satisfiable objectives
    //    loads, and its mission id is its own.
    let sources = sources_with_restricted_roster("restricted_ok");
    let tree = ContentTree::load(&sources).expect("a satisfiable restriction loads");
    assert_ne!(tree.mission_id("restricted_ok"), 0);
    assert_ne!(
        tree.mission_id("restricted_ok"),
        CROSSROADS_01_MISSION_ID,
        "an override mission is its own identity"
    );
}
