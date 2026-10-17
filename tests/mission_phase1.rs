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
    Command, CommandKind, ControllerKind, Event, FogPreset, KindId, MatchSetup,
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
