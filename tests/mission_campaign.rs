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
use pandemonium_sim_api::{
    GatherSemantics, KindId, MissionObjectiveKind, MissionObjectiveParameters, PlayerId, ResourceId,
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
    assert_eq!(
        campaign.chapters,
        vec!["crossroads_01", "crossroads_02", "crossroads_03"],
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
    // The AUD-003 law, pinned against the real Legion faction: any
    // BuildTrain target in the campaign must exceed the starting count of
    // its kind, and any Gather amount must exceed the ruleset's starting
    // balance. A retuning that re-vacuums an objective fails here — a
    // visible content diff, never a silent one.
    let tree = repo_tree();
    let campaign = tree.campaign.as_ref().expect("the pilot campaign loads");
    let legion = tree
        .factions
        .iter()
        .find(|faction| faction.id == "legion")
        .expect("the Legion faction exists");
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
        let objectives = tree
            .mission_data(id, PlayerId(0))
            .unwrap_or_else(|error| panic!("mission '{id}' resolves: {error}"));
        for objective in &objectives.objectives {
            match &objective.parameters {
                MissionObjectiveParameters::BuildTrain { kind, count } => {
                    let kind_id = tree.entities.get(kind.0 as usize).expect("kind resolves");
                    let starting = legion
                        .starting_forces
                        .iter()
                        .filter(|force| force.entity == kind_id.id)
                        .count() as u32;
                    assert!(
                        *count > starting,
                        "mission '{id}' BuildTrain {} target {count} must exceed the \
                         starting {starting}",
                        kind_id.id,
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
    assert_eq!(bundle.content_hash(), 0x9bc18c521107b262);
    assert_eq!(bundle.map_id(), 0xd38136401ab02ff1);
}
