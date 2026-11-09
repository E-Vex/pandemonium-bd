//! Phase 3 campaign acceptance suite (B-025, ADR-0201 §6/§9): **does the
//! real content prove the campaign?**
//!
//! The pilot campaign (`content/campaign/alpha_campaign.ron`) and its four
//! missions are real content; this suite loads the REAL tree, resolves the
//! campaign's play order, and drives every campaign mission through the
//! content seam (`ContentTree::mission_data`) end-to-end:
//!
//! - **Play order** — the chapters load in authored order (the one
//!   deliberately unsorted collection) and the challenge names a chapter.
//! - **Resolution** — every campaign mission resolves to the objective set
//!   it authored, with the Phase 2 kinds (survive, protect, gather)
//!   resolving to their canonical sim_api parameters against the real
//!   Legion tree. Resolution is deterministic.
//! - **The AUD-003 law on real content** — every count target exceeds the
//!   Legion's starting four and the gather amount exceeds the 200 starting
//!   ore (the loader gates refuse otherwise; the test pins the surviving
//!   values so a retuning that re-vacuums an objective is a visible diff).
//! - **Identity** — the campaign category appends nothing to the content
//!   identity: the tree's bundled content hash and map id are the
//!   registered goldens with the campaign present (the bundle test's
//!   property, on the real tree; the registered pins are re-asserted by
//!   `content_pipeline`'s own pin test).

use std::path::{Path, PathBuf};

use pandemonium_content::ContentTree;
use pandemonium_engine::MatchHost;
use pandemonium_sim::{rehost_sim, run_command_log_with_mission};
use pandemonium_sim_api::{
    CommandKind, ControllerKind, EntityId, GatherSemantics, KindId, MatchSetup,
    MissionObjectiveKind, MissionObjectiveParameters, PlayerId, PlayerSetup, ResourceId, Vec2Fx,
};

/// The repository's content directory (mission_phase1's helper, verbatim).
fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("content")
}

/// The loaded real tree — the campaign's fixture.
fn repo_tree() -> ContentTree {
    ContentTree::load_dir(&repo_content()).expect("the repository content loads")
}

#[test]
fn the_pilot_campaign_loads_with_its_authored_play_order() {
    let tree = repo_tree();
    let campaign = tree.campaign.as_ref().expect("the pilot campaign loads");

    assert_eq!(campaign.id, "alpha_campaign");
    assert_eq!(campaign.display_name, "Alpha Campaign");
    // Play order is authored order — deliberately unsorted (the mission
    // collection itself sorts by id; the campaign's chapters must not).
    // B-032's fourth chapter debuts the second faction on Iron Basin.
    assert_eq!(
        campaign.chapters,
        vec![
            "crossroads_01",
            "crossroads_02",
            "crossroads_03",
            "anvil_01"
        ],
        "the chapters keep authored play order"
    );
    assert_eq!(campaign.challenges.len(), 1);
    assert_eq!(campaign.challenges[0].mission, "crossroads_01_challenge");
    assert_eq!(campaign.challenges[0].extends, "crossroads_01");

    // Every referenced mission exists in the tree (the closed reference
    // set — the loader guarantees this; the test pins the resolution).
    for id in campaign
        .chapters
        .iter()
        .chain(campaign.challenges.iter().map(|entry| &entry.mission))
    {
        let (index, mission) = tree
            .mission_by_id(id)
            .unwrap_or_else(|| panic!("campaign mission '{id}' resolves in the tree"));
        assert_eq!(tree.missions[index].id, mission.id);
    }
}

#[test]
fn hold_the_line_resolves_survive_and_protect() {
    let tree = repo_tree();
    let objectives = tree
        .mission_data("crossroads_02", PlayerId(0))
        .expect("crossroads_02 resolves through the content seam");

    assert_eq!(objectives.objectives.len(), 2);

    // Objective 0: survive 3600 ticks — twenty times the loader's floor,
    // two minutes of game time at the fixed 30 ticks/s.
    let objective = &objectives.objectives[0];
    assert_eq!(objective.kind, MissionObjectiveKind::Survive);
    assert_eq!(
        objective.parameters,
        MissionObjectiveParameters::Survive { ticks: 3600 }
    );
    assert!(!objective.optional, "the survive window is required");

    // Objective 1: protect the command center — resolved to the real
    // Legion kind id, optional (fail-only, never gating).
    let command_center_index = tree
        .entities
        .iter()
        .position(|entity| entity.id == "command_center")
        .expect("command_center exists in the tree");
    let objective = &objectives.objectives[1];
    assert_eq!(objective.kind, MissionObjectiveKind::Protect);
    assert_eq!(
        objective.parameters,
        MissionObjectiveParameters::Protect {
            kind: KindId(command_center_index as u32),
        }
    );
    assert!(objective.optional, "the protect flag is optional");
}

#[test]
fn supply_run_resolves_the_gather_quota() {
    let tree = repo_tree();
    let objectives = tree
        .mission_data("crossroads_03", PlayerId(0))
        .expect("crossroads_03 resolves through the content seam");

    assert_eq!(objectives.objectives.len(), 2);

    // Objective 0: gather 500 ore, cumulative — deliveries count,
    // spending does not (B-018's delivery-attribution semantics).
    let objective = &objectives.objectives[0];
    assert_eq!(objective.kind, MissionObjectiveKind::Gather);
    assert_eq!(
        objective.parameters,
        MissionObjectiveParameters::Gather {
            resource: ResourceId(0),
            amount: 500,
            semantics: GatherSemantics::Cumulative,
        }
    );
    assert!(!objective.optional, "the quota is required");

    // Objective 1: grow to ten workers — above the starting four.
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
            count: 10,
        }
    );
    assert!(objective.optional, "the growth goal is optional");
}

#[test]
fn worker_rush_resolves_the_race_target() {
    let tree = repo_tree();
    let objectives = tree
        .mission_data("crossroads_01_challenge", PlayerId(0))
        .expect("the challenge resolves through the content seam");

    assert_eq!(objectives.objectives.len(), 2);

    // Objective 0: twelve workers exist — the race, above the starting
    // four (AUD-003: the count must demand production under fire).
    let worker_index = tree
        .entities
        .iter()
        .position(|entity| entity.id == "worker")
        .expect("worker exists in the tree");
    let objective = &objectives.objectives[0];
    assert_eq!(objective.kind, MissionObjectiveKind::BuildTrain);
    assert_eq!(
        objective.parameters,
        MissionObjectiveParameters::BuildTrain {
            kind: KindId(worker_index as u32),
            count: 12,
        }
    );
    assert!(!objective.optional, "the race is required");

    // Objective 1: the optional destroy line, aimed at the enemy command
    // center — the flourish, not the win condition.
    let command_center_index = tree
        .entities
        .iter()
        .position(|entity| entity.id == "command_center")
        .expect("command_center exists in the tree");
    let objective = &objectives.objectives[1];
    assert_eq!(objective.kind, MissionObjectiveKind::Destroy);
    assert_eq!(
        objective.parameters,
        MissionObjectiveParameters::Destroy {
            target_kind: Some(KindId(command_center_index as u32)),
        }
    );
    assert!(objective.optional, "the overrun line is optional");
}

#[test]
fn every_campaign_count_sits_above_its_starting_force() {
    // The AUD-003 law, pinned against each mission's OWN faction (B-032:
    // the campaign's fourth chapter runs the Phalanx, so the law reads
    // the mission's named faction, never a hardcoded first): any
    // BuildTrain target in the campaign must exceed the starting count of
    // its kind under the mission's faction, and any Gather amount must
    // exceed the ruleset's starting balance. A retuning that re-vacuums
    // an objective fails here — a visible content diff, never a silent
    // one.
    let tree = repo_tree();
    let campaign = tree.campaign.as_ref().expect("the pilot campaign loads");
    // The starting balance lives in the ruleset (the registry is global —
    // the Alpha's single Ore resource).
    let starting_ore = tree
        .rules
        .resources
        .iter()
        .find(|resource| resource.id == "ore")
        .expect("the Ore resource exists")
        .starting;

    for id in campaign
        .chapters
        .iter()
        .chain(campaign.challenges.iter().map(|entry| &entry.mission))
    {
        let mission = tree.mission(id).expect("the campaign mission exists");
        let faction = tree
            .factions
            .iter()
            .find(|faction| faction.id == mission.faction)
            .unwrap_or_else(|| panic!("mission '{id}' faction '{}' exists", mission.faction));
        let objectives = tree
            .mission_data(id, PlayerId(0))
            .unwrap_or_else(|error| panic!("mission '{id}' resolves: {error}"));
        for objective in &objectives.objectives {
            match &objective.parameters {
                MissionObjectiveParameters::BuildTrain { kind, count } => {
                    let kind_id = tree.entities.get(kind.0 as usize).expect("kind resolves");
                    let starting = faction
                        .starting_forces
                        .iter()
                        .filter(|force| force.entity == kind_id.id)
                        .count() as u32;
                    assert!(
                        *count > starting,
                        "mission '{id}' BuildTrain {} target {count} must exceed the \
                         starting {starting} under faction '{}'",
                        kind_id.id,
                        faction.id,
                    );
                }
                MissionObjectiveParameters::Gather { amount, .. } => {
                    assert!(
                        *amount > starting_ore,
                        "mission '{id}' gather {amount} must exceed the starting balance",
                    );
                }
                _ => {}
            }
        }
    }
}

#[test]
fn the_campaign_rides_outside_the_real_identity() {
    // The bundle test's property on the real tree: the campaign category
    // appends nothing to the identity encoders. The registered content
    // hash and map id (AUD-005's pins) hold WITH the campaign present —
    // the same values content-validate pins in CI.
    let tree = repo_tree();
    assert!(tree.campaign.is_some(), "the pilot campaign is loaded");
    let bundle = tree.bundle("crossroads_64").expect("the map bundles");
    assert_eq!(bundle.content_hash(), 0x4d362e1a17b9eefc);
    assert_eq!(bundle.map_id(), 0xd38136401ab02ff1);
}

// Phase 4 (B-027, ADR-0201 §6/§9): the mid-mission save's rehost seam over
// the REAL content — the rehost is the canonical driver's own loop, so the
// same law the replay gate verifies over a record holds over the real
// chapter's bundle: the rehost at N ticks is the simulation the canonical
// trail describes at N.

/// The Player vs AI setup a mission match runs (the client's
/// `MatchMode::PlayerVsAi.match_setup` shape, seed 7).
fn player_vs_ai() -> MatchSetup {
    MatchSetup {
        seed: 7,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Ai,
            },
        ],
    }
}

#[test]
fn rehost_over_the_real_chapter_matches_the_canonical_driver() {
    let tree = repo_tree();
    let (_, def) = tree.mission_by_id("crossroads_01").expect("resolves");
    let bundle = tree
        .bundle_for_mission(&def.id)
        .expect("the mission's map bundles");
    let world = bundle.world();
    let mission = tree
        .mission_data(&def.id, PlayerId(0))
        .expect("the chapter resolves through the content seam");
    let setup = player_vs_ai();
    let run = run_command_log_with_mission(&world, &setup, &[], 60, Some(mission.clone()));
    let sim = rehost_sim(&world, &setup, &[], 60, Some(mission));
    assert_eq!(sim.tick(), 60);
    assert_eq!(sim.state_hash(), run.final_hash);
    assert_eq!(sim.next_entity_id(), run.next_entity_id);
    // The objective-status query rides the rehost (the resumed HUD seeds
    // from it); nothing resolves in the first two seconds of the real
    // chapter (destroy + the optional grow — the strong resolved case is
    // pinned at the sim and engine seams).
    assert_eq!(sim.objective_states(), vec![None, None]);
}

#[test]
fn resume_with_mission_rehosts_over_the_real_chapter() {
    // The engine seam over the real bundle: the resumed host carries the
    // recorded log as its seed, exactly at the saved tick, with the hash
    // the canonical trail describes there — the mechanics only, never the
    // continuation's content (the ADR's §6 caveat iii).
    let tree = repo_tree();
    let (_, def) = tree.mission_by_id("crossroads_01").expect("resolves");
    let bundle = tree
        .bundle_for_mission(&def.id)
        .expect("the mission's map bundles");
    let world = bundle.world();
    let mission = tree
        .mission_data(&def.id, PlayerId(0))
        .expect("the chapter resolves through the content seam");
    let setup = player_vs_ai();
    let commands = vec![pandemonium_sim_api::Command::new(
        PlayerId(0),
        0,
        1,
        CommandKind::Move {
            units: vec![EntityId(1)],
            target: Vec2Fx::from_ints(8, 8),
        },
    )];
    let run = run_command_log_with_mission(&world, &setup, &commands, 60, None);
    let host = MatchHost::resume_with_mission(&world, setup, Vec::new(), mission, &commands, 60);
    assert_eq!(host.tick(), 60);
    assert_eq!(host.state_hash(), run.final_hash);
    assert_eq!(
        host.log(),
        commands.as_slice(),
        "the log starts seeded with the recorded commands"
    );
}
