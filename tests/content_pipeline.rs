//! Content pipeline acceptance suite (plan §14 M2, §10.6/A3):
//!
//! - **Alpha manifest loads** — the repository's `content/` tree loads as a
//!   valid bundle, and every stat matches plan §10.4's table exactly.
//! - **A3 scaffold — add-a-unit is data-only** — a new unit kind is added
//!   purely by writing data files into a copy of the content tree (new entity
//!   file + faction roster/production/starting-force edits), loads, spawns,
//!   and obeys a Move command. The simulation sources contain zero knowledge
//!   of the new kind (pinned by a source scan), and the git-diff check that no
//!   file under `crates/sim/` changed is part of the commit protocol for the
//!   fixture — see AI-Handoff §8. The full §10.6 test ("gets built and
//!   fights") extends this scaffold when production (M5) and combat (M6) land.
//! - **Loaded-bundle determinism** — loading is a pure function of the
//!   directory, and a match built from a bundle is as deterministic as the
//!   fixture world (A1's properties on the real content path).

use std::path::{Path, PathBuf};

use pandemonium_content::{CapabilityDef, ContentBundle};
use pandemonium_sim::Sim;
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, MatchSetup, MoveState, PlayerId,
    PlayerSetup, TilePos, Vec2Fx,
};

/// The repository's content directory.
fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the tests package sits inside the workspace root")
        .join("content")
}

/// A two-player setup (one human, one AI — controller kinds are labels until
/// the AI crate lands, M7).
fn two_player_setup(seed: u64) -> MatchSetup {
    MatchSetup {
        seed,
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

/// One stat row of plan §10.4's table, as a predicate over the entity def.
struct ExpectedStat {
    entity: &'static str,
    field: &'static str,
    value: i64,
}

fn assert_table_stats(bundle: &ContentBundle, stats: &[ExpectedStat]) {
    for expected in stats {
        let entity = bundle
            .entity(expected.entity)
            .unwrap_or_else(|| panic!("entity '{}' must exist", expected.entity));
        let actual = stat_value(entity, expected.field);
        assert_eq!(
            actual, expected.value,
            "{}.{} must match plan §10.4 exactly",
            expected.entity, expected.field
        );
    }
}

/// Reads one stat by field name (plan §10.4 column spelling).
fn stat_value(entity: &pandemonium_content::EntityDef, field: &str) -> i64 {
    let cap = |name: &str| entity.capability(name);
    match field {
        "hp" => match cap("Health") {
            Some(CapabilityDef::Health { max_hp, .. }) => *max_hp as i64,
            _ => -1,
        },
        "cost_ore" => entity.cost_ore,
        "build_time_ms" => entity.build_time_ms as i64,
        "speed" => match cap("Move") {
            Some(CapabilityDef::Move {
                speed_milli_tiles_per_s,
                ..
            }) => *speed_milli_tiles_per_s as i64,
            _ => -1,
        },
        "range" => match cap("Attack") {
            Some(CapabilityDef::Attack {
                range_milli_tiles, ..
            }) => *range_milli_tiles as i64,
            _ => -1,
        },
        "damage" => match cap("Attack") {
            Some(CapabilityDef::Attack { damage, .. }) => *damage as i64,
            _ => -1,
        },
        "cooldown_ms" => match cap("Attack") {
            Some(CapabilityDef::Attack { cooldown_ms, .. }) => *cooldown_ms as i64,
            _ => -1,
        },
        "vision" => match cap("Vision") {
            Some(CapabilityDef::Vision { radius_milli_tiles }) => *radius_milli_tiles as i64,
            _ => -1,
        },
        "population" => entity.population as i64,
        "provides_population" => match cap("ProvidesPopulation") {
            Some(CapabilityDef::ProvidesPopulation { amount }) => *amount as i64,
            _ => 0,
        },
        "ore_amount" => match cap("Resource") {
            Some(CapabilityDef::Resource { amount, .. }) => *amount,
            _ => -1,
        },
        other => panic!("unknown table field {other}"),
    }
}

#[test]
fn alpha_manifest_loads_and_pins_the_plan_10_4_stat_table() {
    let bundle = ContentBundle::load_dir(&repo_content()).expect("the Alpha content loads");

    // The manifest's shape (plan §10.4): 1 faction, 1 map (64x64), 1 resource,
    // 4 buildings + ore node + 4 units = 9 entity kinds.
    assert_eq!(bundle.factions.len(), 1);
    assert_eq!(bundle.map.width, 64);
    assert_eq!(bundle.map.height, 64);
    assert_eq!(bundle.rules.resources.len(), 1);
    assert_eq!(bundle.entities.len(), 9);
    assert_eq!(bundle.rules.resources[0].id, "ore");
    assert_eq!(bundle.rules.resources[0].starting, 200);
    assert_eq!(bundle.rules.base_population_cap, 0);

    // Every cell of plan §10.4's table, exactly as written.
    use ExpectedStat as S;
    assert_table_stats(
        &bundle,
        &[
            S {
                entity: "worker",
                field: "hp",
                value: 40,
            },
            S {
                entity: "worker",
                field: "cost_ore",
                value: 50,
            },
            S {
                entity: "worker",
                field: "build_time_ms",
                value: 12000,
            },
            S {
                entity: "worker",
                field: "speed",
                value: 2600,
            },
            S {
                entity: "worker",
                field: "vision",
                value: 7000,
            },
            S {
                entity: "worker",
                field: "population",
                value: 1,
            },
            S {
                entity: "rifleman",
                field: "hp",
                value: 60,
            },
            S {
                entity: "rifleman",
                field: "cost_ore",
                value: 75,
            },
            S {
                entity: "rifleman",
                field: "build_time_ms",
                value: 10000,
            },
            S {
                entity: "rifleman",
                field: "speed",
                value: 2400,
            },
            S {
                entity: "rifleman",
                field: "range",
                value: 5000,
            },
            S {
                entity: "rifleman",
                field: "damage",
                value: 8,
            },
            S {
                entity: "rifleman",
                field: "cooldown_ms",
                value: 1000,
            },
            S {
                entity: "rifleman",
                field: "vision",
                value: 8000,
            },
            S {
                entity: "rifleman",
                field: "population",
                value: 1,
            },
            S {
                entity: "raider",
                field: "hp",
                value: 45,
            },
            S {
                entity: "raider",
                field: "cost_ore",
                value: 60,
            },
            S {
                entity: "raider",
                field: "build_time_ms",
                value: 8000,
            },
            S {
                entity: "raider",
                field: "speed",
                value: 4000,
            },
            S {
                entity: "raider",
                field: "range",
                value: 1500,
            },
            S {
                entity: "raider",
                field: "damage",
                value: 6,
            },
            S {
                entity: "raider",
                field: "cooldown_ms",
                value: 600,
            },
            S {
                entity: "raider",
                field: "vision",
                value: 8000,
            },
            S {
                entity: "raider",
                field: "population",
                value: 1,
            },
            S {
                entity: "guardian",
                field: "hp",
                value: 220,
            },
            S {
                entity: "guardian",
                field: "cost_ore",
                value: 200,
            },
            S {
                entity: "guardian",
                field: "build_time_ms",
                value: 25000,
            },
            S {
                entity: "guardian",
                field: "speed",
                value: 1600,
            },
            S {
                entity: "guardian",
                field: "range",
                value: 8000,
            },
            S {
                entity: "guardian",
                field: "damage",
                value: 30,
            },
            S {
                entity: "guardian",
                field: "cooldown_ms",
                value: 2000,
            },
            S {
                entity: "guardian",
                field: "vision",
                value: 9000,
            },
            S {
                entity: "guardian",
                field: "population",
                value: 3,
            },
            S {
                entity: "command_center",
                field: "hp",
                value: 1000,
            },
            S {
                entity: "command_center",
                field: "cost_ore",
                value: 400,
            },
            S {
                entity: "command_center",
                field: "build_time_ms",
                value: 40000,
            },
            S {
                entity: "command_center",
                field: "vision",
                value: 10000,
            },
            S {
                entity: "command_center",
                field: "provides_population",
                value: 10,
            },
            S {
                entity: "barracks",
                field: "hp",
                value: 600,
            },
            S {
                entity: "barracks",
                field: "cost_ore",
                value: 150,
            },
            S {
                entity: "barracks",
                field: "build_time_ms",
                value: 25000,
            },
            S {
                entity: "barracks",
                field: "vision",
                value: 8000,
            },
            S {
                entity: "supply_depot",
                field: "hp",
                value: 300,
            },
            S {
                entity: "supply_depot",
                field: "cost_ore",
                value: 100,
            },
            S {
                entity: "supply_depot",
                field: "build_time_ms",
                value: 15000,
            },
            S {
                entity: "supply_depot",
                field: "vision",
                value: 6000,
            },
            S {
                entity: "supply_depot",
                field: "provides_population",
                value: 10,
            },
            S {
                entity: "turret",
                field: "hp",
                value: 350,
            },
            S {
                entity: "turret",
                field: "cost_ore",
                value: 125,
            },
            S {
                entity: "turret",
                field: "build_time_ms",
                value: 20000,
            },
            S {
                entity: "turret",
                field: "range",
                value: 7000,
            },
            S {
                entity: "turret",
                field: "damage",
                value: 10,
            },
            S {
                entity: "turret",
                field: "cooldown_ms",
                value: 1000,
            },
            S {
                entity: "turret",
                field: "vision",
                value: 9000,
            },
            S {
                entity: "ore_node",
                field: "ore_amount",
                value: 1500,
            },
        ],
    );

    // Footprints from the table's size column.
    assert_eq!(
        bundle.entity("command_center").unwrap().footprint(),
        Some((4, 4))
    );
    assert_eq!(bundle.entity("barracks").unwrap().footprint(), Some((3, 3)));
    assert_eq!(
        bundle.entity("supply_depot").unwrap().footprint(),
        Some((2, 2))
    );
    assert_eq!(bundle.entity("turret").unwrap().footprint(), Some((2, 2)));
    assert_eq!(bundle.entity("ore_node").unwrap().footprint(), Some((2, 2)));

    // Gathering (plan §10.4's line under the table).
    match bundle.entity("worker").unwrap().capability("Gather") {
        Some(CapabilityDef::Gather {
            carry_amount,
            gather_time_ms,
            ..
        }) => {
            assert_eq!(*carry_amount, 10, "workers carry 10 Ore per trip");
            assert_eq!(*gather_time_ms, 2000, "2000 ms per gather");
        }
        other => panic!("worker must gather, got {other:?}"),
    }

    // The map declares and passes rotational symmetry, carries the
    // display-only heightmap (ADR-0001), and places ore for both starts plus
    // the contested expansion.
    assert!(bundle.map.symmetric);
    let heightmap = bundle.map.heightmap.as_ref().expect("ADR-0001 heightmap");
    assert_eq!(heightmap.rows.len(), 64);
    assert!(heightmap.rows.iter().all(|row| row.len() == 64));
    assert_eq!(bundle.map.ore_nodes.len(), 12);
    assert_eq!(bundle.map.starts.len(), 2);

    // The starting forces (plan §10.4: 1 Command Center + 4 Workers per player).
    let legion = bundle.faction();
    assert_eq!(legion.starting_forces.len(), 5);
    assert_eq!(legion.starting_forces[0].entity, "command_center");
    assert_eq!(
        legion
            .starting_forces
            .iter()
            .filter(|f| f.entity == "worker")
            .count(),
        4
    );

    // The faction's production lists (plan §10.6: a faction's production list).
    let barracks = legion
        .production
        .iter()
        .find(|(producer, _)| producer == "barracks")
        .expect("barracks produce");
    assert_eq!(barracks.1, vec!["rifleman", "raider", "guardian"]);
}

#[test]
fn a_match_from_loaded_content_spawns_the_full_starting_state() {
    let bundle = ContentBundle::load_dir(&repo_content()).unwrap();
    let sim = Sim::new(&bundle.world(), two_player_setup(7));

    // 2 x (1 CC + 4 workers) + 12 neutral ore nodes.
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.entities.len(), 22);
    let ore_nodes = snapshot
        .entities
        .iter()
        .filter(|entity| entity.owner == PlayerId::NEUTRAL)
        .count();
    assert_eq!(ore_nodes, 12);

    // Both players start with the registry's Ore balance (plan §10.4: 200).
    for player in [PlayerId(0), PlayerId(1)] {
        let view = sim.player_view(player);
        assert_eq!(view.resources.len(), 1);
        assert_eq!(view.resources[0].amount, 200);
    }

    // Command Center ids are deterministic: player 0's CC is the first spawn.
    assert_eq!(snapshot.entities[0].owner, PlayerId(0));
    assert_eq!(snapshot.entities[0].kind, pandemonium_sim_api::KindId(1));
}

#[test]
fn loading_and_matches_from_content_are_deterministic() {
    // Loading is a pure function of the directory.
    let a = ContentBundle::load_dir(&repo_content()).unwrap();
    let b = ContentBundle::load_dir(&repo_content()).unwrap();
    assert_eq!(a.content_hash(), b.content_hash());
    assert_eq!(a.map_id(), b.map_id());
    assert_eq!(a.world(), b.world());

    // Matches from the loaded bundle reproduce A1's property: same seed +
    // same (empty) log -> identical checkpoints; different seed -> divergence
    // (the RNG is exercised by the world itself only when jitter is nonzero,
    // so the divergence check stays on the fixture world — here we pin
    // equality, which is the property content must not break).
    let run = |seed: u64| -> Vec<u64> {
        let mut sim = Sim::new(&a.world(), two_player_setup(seed));
        let mut hashes = vec![sim.state_hash()];
        for _ in 0..90 {
            if let Some(hash) = sim.step(&[]).hash {
                hashes.push(hash);
            }
        }
        hashes.push(sim.state_hash());
        hashes
    };
    assert_eq!(run(7), run(7), "same seed must give identical checkpoints");
}

/// Copies the repository content tree into a fresh temp directory (the A3
/// scaffold's sandbox: designers edit copies, the loader never sees the repo).
fn copy_content_to_temp(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pandemonium-a3-{tag}-{}-{:p}",
        std::process::id(),
        &repo_content()
    ));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&repo_content(), &root);
    root
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &target);
        } else {
            std::fs::copy(&source, &target).unwrap();
        }
    }
}

#[test]
fn add_a_unit_is_data_only() {
    // ---------------------------------------------------------------------
    // The A3 scaffold (plan §10.6, AI-Handoff §8.5): a new unit exists purely
    // as a new data file plus faction edits. No engine, sim, or tool source
    // knows it. The commit that lands this test must show zero diffs under
    // crates/sim/ (the git-diff half of A3); the runtime half is below.
    // ---------------------------------------------------------------------
    let root = copy_content_to_temp("add-a-unit");

    // 1. A new entity file: a fast scout built from the rifleman's shape with
    //    different stats and a new id (the file name must equal the id).
    let skirmisher = r#"
(
    schema_version: 1,
    id: "skirmisher",
    display_name: "Skirmisher",
    capabilities: [
        Health(( max: 55 )),
        Move(( speed_milli_tiles_per_s: 3400, radius_milli_tiles: 300 )),
        Attack(( damage: 5, range_milli_tiles: 3500, cooldown_ms: 700, acquire_range_milli_tiles: 6000 )),
        Vision(( radius_milli_tiles: 9000 )),
    ],
    cost: ( ore: 65 ),
    build_time_ms: 9000,
    population: 1,
    requires: [ "barracks" ],
)
"#;
    std::fs::write(root.join("entities/skirmisher.ron"), skirmisher).unwrap();

    // 2. Faction edits: roster, the barracks' production list, and one
    //    starting skirmisher per player (data-only spawn for this scaffold).
    let faction_path = root.join("factions/legion.ron");
    let mut faction = std::fs::read_to_string(&faction_path).unwrap();
    faction = faction.replace(
        "    roster: [\n        \"worker\",",
        "    roster: [\n        \"skirmisher\",\n        \"worker\",",
    );
    faction = faction.replace(
        "\"barracks\": [ \"rifleman\", \"raider\", \"guardian\" ],",
        "\"barracks\": [ \"rifleman\", \"raider\", \"guardian\", \"skirmisher\" ],",
    );
    faction = faction.replace(
        "        ( entity: \"worker\", offset: ( x: 4, y: 4 ) ),\n",
        "        ( entity: \"worker\", offset: ( x: 4, y: 4 ) ),\n        ( entity: \"skirmisher\", offset: ( x: -2, y: 6 ) ),\n",
    );
    std::fs::write(&faction_path, faction).unwrap();

    // 3. The loader accepts the extended tree with zero code changes.
    let bundle = ContentBundle::load_dir(&root).expect("the extended content loads");
    let skirmisher = bundle.entity("skirmisher").expect("the new kind exists");
    assert_eq!(skirmisher.cost_ore, 65);
    assert_eq!(skirmisher.build_time_ticks, 270); // 9000 ms -> (9000*30+999)/1000
    assert_eq!(
        stat_value(skirmisher, "speed"),
        3400,
        "authored stats flow through §10.2 conversion unchanged"
    );

    // Kind ids follow the sorted entity order: barracks, command_center,
    // guardian, ore_node, raider, rifleman, skirmisher, supply_depot, turret,
    // worker -> skirmisher is index 6.
    let world = bundle.world();
    assert_eq!(world.kinds.len(), 10);
    let kind = world
        .initial_spawns
        .iter()
        .find(|spawn| {
            spawn.pos
                == Vec2Fx::new(
                    pandemonium_fx::Fx::from_milli(10500),
                    pandemonium_fx::Fx::from_milli(18500),
                )
        })
        .expect("player 0's skirmisher spawns at anchor (12,12)+(-2,6) -> tile (10,18) center");
    assert_eq!(kind.owner, PlayerId(0));
    assert_eq!(kind.kind, pandemonium_sim_api::KindId(6));

    // 4. The simulation runs it: the new kind spawns from data, accepts a
    //    Move order (Health + Move + Vision capabilities all flow), and moves.
    let mut sim = Sim::new(&world, two_player_setup(7));
    let snapshot = sim.snapshot();
    let spawned = snapshot
        .entities
        .iter()
        .find(|entity| entity.kind == pandemonium_sim_api::KindId(6))
        .expect("the data-only kind is in the match");
    let id = spawned.id;
    let spawned_owner = spawned.owner;
    let spawned_hp = spawned.hp_fraction_milli;
    assert_eq!(spawned_owner, PlayerId(0));
    assert_eq!(spawned_hp, 1000, "spawns at full health");
    sim.step(&[Command::new(
        PlayerId(0),
        0,
        1,
        CommandKind::Move {
            units: vec![id],
            target: Vec2Fx::from_ints(20, 18),
        },
    )]);
    let mut ticks = 0;
    while sim
        .snapshot()
        .entities
        .iter()
        .any(|entity| entity.id == id && entity.move_state == MoveState::Moving)
    {
        sim.step(&[]);
        ticks += 1;
        assert!(ticks < 400, "the data-only unit must arrive");
    }
    let snapshot = sim.snapshot();
    let arrived = snapshot
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap();
    assert_eq!(arrived.pos, Vec2Fx::from_ints(20, 18));
    assert_eq!(arrived.kind, pandemonium_sim_api::KindId(6));

    // 4b. DEBT-007's M5 half: the data-defined kind TRAINS from the barracks
    //     (plan §10.6 "gets built [from a production list]"). Gather with the
    //     four workers, raise a barracks from data, and enqueue a skirmisher
    //     — the whole loop with zero changes under crates/sim.
    let barracks_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "barracks")
        .expect("the barracks kind exists") as u32;
    let worker_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "worker")
        .expect("the worker kind exists") as u32;
    let node_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "ore_node")
        .expect("the ore node kind exists") as u32;
    // The workers (ids 2..=5 in the documented spawn order: CC, four workers,
    // then the skirmisher starting force, then nodes) gather; the first also
    // raises the barracks.
    let workers: Vec<EntityId> = sim
        .snapshot()
        .entities
        .iter()
        .filter(|entity| entity.owner == PlayerId(0) && entity.kind.0 == worker_kind)
        .map(|entity| entity.id)
        .collect();
    assert_eq!(workers.len(), 4);
    let nodes: Vec<EntityId> = sim
        .snapshot()
        .entities
        .iter()
        .filter(|entity| entity.kind.0 == node_kind)
        .map(|entity| entity.id)
        .collect();
    let mut seq = 100u32;
    let mut barracks_done_tick: Option<u32> = None;
    let mut ticks = sim.tick();
    while ticks < 2400 {
        let mut commands = Vec::new();
        // Idle workers gather (round-robin over the nodes).
        for (index, worker) in workers.iter().enumerate() {
            let idle = sim
                .snapshot()
                .entities
                .iter()
                .any(|entity| entity.id == *worker && entity.move_state == MoveState::Idle);
            if idle {
                commands.push(Command::new(
                    PlayerId(0),
                    sim.tick(),
                    {
                        seq += 1;
                        seq
                    },
                    CommandKind::Gather {
                        units: vec![*worker],
                        node: nodes[index % nodes.len()],
                    },
                ));
            }
        }
        // Raise the barracks once Ore allows (retries ride out transient
        // placement blocks from traveling workers).
        let barracks_exists = sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.owner == PlayerId(0) && entity.kind.0 == barracks_kind);
        let ore = sim.player_view(PlayerId(0)).resources[0].amount;
        if !barracks_exists && ore >= 150 && ticks.is_multiple_of(5) {
            commands.push(Command::new(
                PlayerId(0),
                sim.tick(),
                {
                    seq += 1;
                    seq
                },
                CommandKind::Build {
                    worker: workers[0],
                    structure: pandemonium_sim_api::KindId(barracks_kind),
                    at: TilePos { x: 17, y: 11 },
                },
            ));
        }
        // Train the skirmisher once the barracks is up and Ore allows. While
        // the site is still under construction the Train is refused
        // (MissingCapability) — retried on a 30-tick cadence until it lands.
        if barracks_exists && ore >= 65 && ticks.is_multiple_of(30) {
            let barracks = sim
                .snapshot()
                .entities
                .iter()
                .find(|entity| entity.owner == PlayerId(0) && entity.kind.0 == barracks_kind)
                .unwrap()
                .id;
            commands.push(Command::new(
                PlayerId(0),
                sim.tick(),
                {
                    seq += 1;
                    seq
                },
                CommandKind::Train {
                    producer: barracks,
                    unit: pandemonium_sim_api::KindId(6),
                },
            ));
        }
        let out = sim.step(&commands);
        ticks = sim.tick();
        if barracks_done_tick.is_none()
            && out
                .events
                .iter()
                .any(|event| matches!(event, Event::ConstructionCompleted { .. }))
        {
            barracks_done_tick = Some(ticks);
        }
        if out
            .events
            .iter()
            .any(|event| matches!(event, Event::ProductionCompleted { .. }))
        {
            // The trained skirmisher exists with the data-defined kind.
            assert!(
                sim.snapshot()
                    .entities
                    .iter()
                    .any(|entity| entity.kind == pandemonium_sim_api::KindId(6) && entity.id != id),
                "the trained skirmisher spawned"
            );
            break;
        }
    }
    assert!(
        barracks_done_tick.is_some(),
        "the barracks never completed in {ticks} ticks"
    );
    assert!(
        ticks < 2400,
        "the skirmisher never trained in {ticks} ticks"
    );

    // 5. The simulation sources contain zero knowledge of the new kind —
    //    nothing under crates/sim/ mentions it (no special-casing, FD-4).
    let sim_dir = repo_content().parent().unwrap().join("crates").join("sim");
    let mut sources = Vec::new();
    collect_rust_sources(&sim_dir, &mut sources);
    assert!(!sources.is_empty());
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("skirmisher"),
            "{} mentions the data-only kind — add-a-unit must stay data-only (A3)",
            path.display()
        );
    }

    let _ = EntityId(0); // (EntityId imported for the command payload above)
    let _ = std::fs::remove_dir_all(&root);
}

/// Recursively collects `.rs` files (mirrors the architecture-law scan).
fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}
