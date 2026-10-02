//! M5 exit suite (plan §14 M5, §13 A12): divergent scripted openings and an
//! economy-only invariant soak over the real Crossroads content.
//!
//! - **Divergence** — two scripted openings for player 0 over the same seed
//!   and content: *worker-heavy* (gather with all four workers, keep training
//!   workers) versus *early Raider* (gather, raise a barracks, train
//!   raiders). The timelines must measurably diverge: worker counts, built
//!   structures, trained units, income, and balances all branch, and the two
//!   runs' checkpoint hashes differ from the first economy tick on.
//! - **Determinism** — the same scripted opening run twice is bit-identical
//!   (checkpoints, final hash, timelines); the A1 property carried through
//!   the economy systems.
//! - **A12 soak** — three scripted economy openings × two seeds × 900 ticks,
//!   both players scripted: the debug invariant checker runs inside every
//!   step, and the observable invariants (non-negative ledgers, population
//!   within the cap) hold at every checkpoint. A fourth soak run repeats one
//!   configuration exactly and compares hashes.
//!
//! The scripts are deterministic functions of the tick and the player's view
//! (the same door an AI controller will use, plan §9.6) — no peeking at sim
//! internals beyond the public boundary types.

use std::path::{Path, PathBuf};

use pandemonium_content::ContentBundle;
use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim::Sim;
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, MatchSetup, MoveState, PlayerId,
    PlayerSetup, TilePos,
};

/// How long the divergence matches run (40 seconds of game time — enough for
/// a first barracks cycle and several trained units).
const DIVERGENCE_TICKS: u32 = 1200;

/// How long each soak match runs (30 seconds).
const SOAK_TICKS: u32 = 900;

fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
        .join("content")
}

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

/// The per-player bits a script needs: known entity ids and its own command
/// sequence counter.
struct ScriptCtx {
    player: PlayerId,
    workers: Vec<EntityId>,
    command_center: EntityId,
    /// Nodes ordered by distance from the command center (deterministic).
    nodes: Vec<EntityId>,
    seq: u32,
    /// Script-local bookkeeping (cooldowns, flags).
    last_train_tick: Option<u32>,
    barracks_seen: bool,
    last_build_attempt: u32,
    last_train_tick_barracks: Option<u32>,
}

impl ScriptCtx {
    fn next_seq(&mut self) -> u32 {
        self.seq += 1;
        self.seq
    }
}

/// A player-relative build spot (mirrored for player 1): east of the start
/// anchor, clear of the ore nodes and their approach lanes.
fn build_spot(player: PlayerId) -> TilePos {
    match player.0 {
        1 => TilePos { x: 43, y: 51 },
        _ => TilePos { x: 17, y: 11 },
    }
}

/// Builds both players' script contexts from the initial snapshot (the
/// documented spawn order: per player, CC then workers, then the nodes).
fn script_contexts(sim: &Sim, bundle: &ContentBundle) -> Vec<ScriptCtx> {
    let worker_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "worker")
        .expect("the worker kind exists") as u32;
    let cc_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "command_center")
        .expect("the command center kind exists") as u32;
    let node_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "ore_node")
        .expect("the ore node kind exists") as u32;
    let snapshot = sim.snapshot();
    let mut contexts = Vec::new();
    for (player, setup) in [PlayerId(0), PlayerId(1)]
        .iter()
        .zip(two_player_setup(0).players.iter())
    {
        let _ = setup;
        let command_center = snapshot
            .entities
            .iter()
            .find(|entity| entity.owner == *player && entity.kind.0 == cc_kind)
            .expect("each player starts with a command center")
            .id;
        let cc_pos = snapshot
            .entities
            .iter()
            .find(|entity| entity.id == command_center)
            .unwrap()
            .pos;
        let mut workers: Vec<EntityId> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.owner == *player && entity.kind.0 == worker_kind)
            .map(|entity| entity.id)
            .collect();
        workers.sort();
        // Nodes by distance from the command center, ties by id — a stable
        // assignment order for the initial gather orders.
        let mut nodes: Vec<(u64, EntityId)> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.kind.0 == node_kind)
            .map(|entity| ((entity.pos - cc_pos).len_sq_raw(), entity.id))
            .collect();
        nodes.sort();
        contexts.push(ScriptCtx {
            player: *player,
            workers,
            command_center,
            nodes: nodes.iter().map(|(_, id)| *id).collect(),
            seq: 0,
            last_train_tick: None,
            barracks_seen: false,
            last_build_attempt: 0,
            last_train_tick_barracks: None,
        });
    }
    contexts
}

/// One timeline sample: the economy shape of a player at a checkpoint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Sample {
    tick: u32,
    ore: i64,
    population: u32,
    population_cap: u32,
    workers: u32,
    structures: u32,
    deposits: i64,
}

/// Which scripted opening a context runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opening {
    /// Gather with everyone; keep training workers while Ore allows.
    WorkerHeavy,
    /// Gather; raise a barracks; train Raiders from it.
    EarlyRaider,
    /// Gather only (the soak baseline).
    GatherOnly,
    /// Gather; raise a supply depot (population cap growth).
    Depot,
}

/// The player's commands for this tick, derived from the view.
fn script_tick(
    opening: Opening,
    ctx: &mut ScriptCtx,
    sim: &Sim,
    bundle: &ContentBundle,
) -> Vec<Command> {
    let tick = sim.tick();
    let player = ctx.player;
    let view = sim.player_view(player);
    let mut commands = Vec::new();
    let ore = view
        .resources
        .first()
        .map(|resource| resource.amount)
        .unwrap_or(0);

    // Everyone gathers: workers spread over the nodes in order.
    let worker_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "worker")
        .unwrap() as u32;
    let live_workers: Vec<EntityId> = sim
        .snapshot()
        .entities
        .iter()
        .filter(|entity| entity.owner == player && entity.kind.0 == worker_kind)
        .map(|entity| entity.id)
        .collect();
    let idle: Vec<EntityId> = live_workers
        .iter()
        .copied()
        .filter(|id| {
            sim.snapshot()
                .entities
                .iter()
                .any(|entity| entity.id == *id && entity.move_state == MoveState::Idle)
        })
        .collect();
    // Order idle workers to gather (one node each, round-robin over the
    // context's ordered node list; auto-seek handles the rest).
    for (index, worker) in idle.iter().enumerate() {
        let node = ctx.nodes[index % ctx.nodes.len()];
        commands.push(Command::new(
            player,
            tick,
            ctx.next_seq(),
            CommandKind::Gather {
                units: vec![*worker],
                node,
            },
        ));
    }

    match opening {
        Opening::GatherOnly => {}
        Opening::WorkerHeavy => {
            // Train a worker whenever Ore allows, at most every 60 ticks.
            if ore >= 50
                && ctx
                    .last_train_tick
                    .is_none_or(|last| tick >= last.saturating_add(60))
            {
                ctx.last_train_tick = Some(tick);
                commands.push(Command::new(
                    player,
                    tick,
                    ctx.next_seq(),
                    CommandKind::Train {
                        producer: ctx.command_center,
                        unit: pandemonium_sim_api::KindId(worker_kind),
                    },
                ));
            }
        }
        Opening::EarlyRaider => {
            let barracks_kind = bundle
                .entities
                .iter()
                .position(|entity| entity.id == "barracks")
                .unwrap() as u32;
            let raider_kind = bundle
                .entities
                .iter()
                .position(|entity| entity.id == "raider")
                .unwrap() as u32;
            // Raise a barracks once Ore allows (retries handle transient
            // placement blocks from traveling workers).
            let barracks_exists = sim
                .snapshot()
                .entities
                .iter()
                .any(|entity| entity.owner == player && entity.kind.0 == barracks_kind);
            ctx.barracks_seen |= barracks_exists;
            if !ctx.barracks_seen && ore >= 150 && tick >= ctx.last_build_attempt.saturating_add(5)
            {
                ctx.last_build_attempt = tick;
                // A deterministic free spot east of the start: the CC anchor
                // is (12,12), so (17,13) sits on open ground clear of nodes.
                commands.push(Command::new(
                    player,
                    tick,
                    ctx.next_seq(),
                    CommandKind::Build {
                        worker: ctx.workers[0],
                        structure: pandemonium_sim_api::KindId(barracks_kind),
                        at: TilePos { x: 17, y: 13 },
                    },
                ));
            }
            // Train Raiders from the barracks once it exists (refused while
            // the site is still under construction — self-correcting).
            if barracks_exists
                && ore >= 60
                && ctx
                    .last_train_tick_barracks
                    .is_none_or(|last| tick >= last.saturating_add(90))
            {
                let barracks = sim
                    .snapshot()
                    .entities
                    .iter()
                    .find(|entity| entity.owner == player && entity.kind.0 == barracks_kind)
                    .unwrap()
                    .id;
                ctx.last_train_tick_barracks = Some(tick);
                commands.push(Command::new(
                    player,
                    tick,
                    ctx.next_seq(),
                    CommandKind::Train {
                        producer: barracks,
                        unit: pandemonium_sim_api::KindId(raider_kind),
                    },
                ));
            }
        }
        Opening::Depot => {
            let depot_kind = bundle
                .entities
                .iter()
                .position(|entity| entity.id == "supply_depot")
                .unwrap() as u32;
            let depot_exists = sim
                .snapshot()
                .entities
                .iter()
                .any(|entity| entity.owner == player && entity.kind.0 == depot_kind);
            ctx.barracks_seen |= depot_exists;
            if !ctx.barracks_seen && ore >= 100 && tick >= ctx.last_build_attempt.saturating_add(5)
            {
                ctx.last_build_attempt = tick;
                commands.push(Command::new(
                    player,
                    tick,
                    ctx.next_seq(),
                    CommandKind::Build {
                        worker: ctx.workers[0],
                        structure: pandemonium_sim_api::KindId(depot_kind),
                        at: build_spot(player),
                    },
                ));
            }
        }
    }
    commands
}

/// Runs one scripted match and records player 0's economy timeline plus the
/// checkpoint hashes.
struct Run {
    samples: Vec<Sample>,
    checkpoint_hashes: Vec<(u32, u64)>,
    final_hash: u64,
}

#[allow(clippy::too_many_arguments)]
fn run_scripted(bundle: &ContentBundle, seed: u64, p0: Opening, p1: Opening, ticks: u32) -> Run {
    let world = bundle.world();
    let mut sim = Sim::new(&world, two_player_setup(seed));
    let mut contexts = script_contexts(&sim, bundle);
    let worker_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "worker")
        .unwrap() as u32;
    let structure_kinds: Vec<u32> = ["command_center", "barracks", "supply_depot", "turret"]
        .iter()
        .filter_map(|id| bundle.entities.iter().position(|entity| entity.id == *id))
        .map(|index| index as u32)
        .collect();

    let mut run = Run {
        samples: Vec::new(),
        checkpoint_hashes: Vec::new(),
        final_hash: 0,
    };
    let mut deposits = 0i64;
    while sim.tick() < ticks {
        let commands = [
            script_tick(p0, &mut contexts[0], &sim, bundle),
            script_tick(p1, &mut contexts[1], &sim, bundle),
        ]
        .concat();
        let out = sim.step(&commands);
        if let Some(hash) = out.hash {
            run.checkpoint_hashes.push((sim.tick(), hash));
        }
        // Attribute deliveries to player 0's workers (the timeline's income).
        let p0_workers: Vec<EntityId> = sim
            .snapshot()
            .entities
            .iter()
            .filter(|entity| entity.owner == PlayerId(0) && entity.kind.0 == worker_kind)
            .map(|entity| entity.id)
            .collect();
        for event in &out.events {
            if let Event::ResourceDelivered { worker, amount, .. } = event {
                if p0_workers.contains(worker) {
                    deposits += *amount as i64;
                }
            }
        }
        if sim.tick().is_multiple_of(30) {
            let view = sim.player_view(PlayerId(0));
            let snapshot = sim.snapshot();
            run.samples.push(Sample {
                tick: sim.tick(),
                ore: view.resources.first().map(|r| r.amount).unwrap_or(0),
                population: view.population,
                population_cap: view.population_cap,
                workers: snapshot
                    .entities
                    .iter()
                    .filter(|e| e.owner == PlayerId(0) && e.kind.0 == worker_kind)
                    .count() as u32,
                structures: snapshot
                    .entities
                    .iter()
                    .filter(|e| e.owner == PlayerId(0) && structure_kinds.contains(&e.kind.0))
                    .count() as u32,
                deposits,
            });
        }
    }
    run.final_hash = sim.state_hash();
    run
}

/// M5 exit (plan §14): worker-heavy and early-Raider openings produce
/// measurably different timelines over the same seed and content.
#[test]
fn divergent_scripted_openings_produce_different_timelines() {
    let bundle = ContentBundle::load_dir(&repo_content()).expect("the repo content loads");
    let a = run_scripted(
        &bundle,
        7,
        Opening::WorkerHeavy,
        Opening::GatherOnly,
        DIVERGENCE_TICKS,
    );
    let b = run_scripted(
        &bundle,
        7,
        Opening::EarlyRaider,
        Opening::GatherOnly,
        DIVERGENCE_TICKS,
    );

    // The timelines diverge (many samples differ, not just one boundary).
    let differing: usize = a
        .samples
        .iter()
        .zip(b.samples.iter())
        .filter(|(x, y)| x != y)
        .count();
    assert!(
        differing > a.samples.len() / 2,
        "the openings diverged in only {differing} of {} samples",
        a.samples.len()
    );

    let a_end = a.samples.last().copied().unwrap();
    let b_end = b.samples.last().copied().unwrap();

    // Worker-heavy fields more workers...
    assert!(
        a_end.workers > b_end.workers,
        "worker-heavy ended with {} workers vs {}",
        a_end.workers,
        b_end.workers
    );
    // ...and never raises a barracks, while early-Raider does.
    assert_eq!(a_end.structures, 1, "worker-heavy builds nothing");
    assert_eq!(b_end.structures, 2, "early-Raider completes a barracks");
    // The worker-heavy economy out-earns the barracks opener.
    assert!(
        a_end.deposits > b_end.deposits,
        "deposits {} vs {}",
        a_end.deposits,
        b_end.deposits
    );
    // Both economies ran real income and spent real Ore.
    assert!(a_end.deposits >= 100, "A delivered {}", a_end.deposits);
    assert!(b_end.deposits >= 30, "B delivered {}", b_end.deposits);
    assert_ne!(a_end.ore, b_end.ore);
    // The match states themselves diverged from early on.
    assert_ne!(a.final_hash, b.final_hash);
    let first_common = a
        .checkpoint_hashes
        .iter()
        .zip(b.checkpoint_hashes.iter())
        .find(|(x, y)| x.1 != y.1);
    assert!(
        first_common.is_some(),
        "the two openings never diverged in state"
    );
    let (tick, _) = *first_common.unwrap().0;
    assert!(tick <= 60, "divergence only at tick {tick}");

    // The Raider actually trained Raiders: p0's population exceeds its four
    // starting workers plus any trained workers (worker-heavy grows too, but
    // through the CC).
    assert!(b_end.population > b_end.workers, "no Raiders fielded");
}

/// A1 carried through the economy systems: the same scripted opening run
/// twice is bit-identical.
#[test]
fn scripted_economy_matches_are_deterministic() {
    let bundle = ContentBundle::load_dir(&repo_content()).expect("the repo content loads");
    let first = run_scripted(
        &bundle,
        7,
        Opening::WorkerHeavy,
        Opening::GatherOnly,
        DIVERGENCE_TICKS,
    );
    let second = run_scripted(
        &bundle,
        7,
        Opening::WorkerHeavy,
        Opening::GatherOnly,
        DIVERGENCE_TICKS,
    );
    assert_eq!(first.checkpoint_hashes, second.checkpoint_hashes);
    assert_eq!(first.final_hash, second.final_hash);
    assert_eq!(first.samples, second.samples);
    // And a different seed diverges.
    let other = run_scripted(
        &bundle,
        8,
        Opening::WorkerHeavy,
        Opening::GatherOnly,
        DIVERGENCE_TICKS,
    );
    assert_ne!(first.final_hash, other.final_hash);
}

/// A12 under an economy-only soak: three openings × two seeds × both players
/// scripted, the debug checker inside every step, the observable invariants
/// asserted at every checkpoint.
#[test]
fn a12_invariants_hold_under_economy_soak() {
    let bundle = ContentBundle::load_dir(&repo_content()).expect("the repo content loads");
    for (opening, name) in [
        (Opening::GatherOnly, "gather-only"),
        (Opening::WorkerHeavy, "gather+train"),
        (Opening::Depot, "gather+depot"),
    ] {
        for seed in [7u64, 99u64] {
            let world = bundle.world();
            let mut sim = Sim::new(&world, two_player_setup(seed));
            let mut contexts = script_contexts(&sim, &bundle);
            let mut ticks = 0;
            while sim.tick() < SOAK_TICKS {
                let commands = [
                    script_tick(opening, &mut contexts[0], &sim, &bundle),
                    script_tick(opening, &mut contexts[1], &sim, &bundle),
                ]
                .concat();
                let _ = sim.step(&commands);
                ticks += 1;
                if sim.tick().is_multiple_of(30) {
                    for player in [PlayerId(0), PlayerId(1)] {
                        let view = sim.player_view(player);
                        assert!(
                            view.resources.iter().all(|r| r.amount >= 0),
                            "{name}/seed {seed}: negative ledger at tick {}",
                            sim.tick()
                        );
                        assert!(
                            view.population <= view.population_cap,
                            "{name}/seed {seed}: pop {}/{} at tick {}",
                            view.population,
                            view.population_cap,
                            sim.tick()
                        );
                    }
                }
            }
            assert_eq!(ticks, SOAK_TICKS, "{name}/seed {seed} stalled");
            // The depot opening must have actually raised the cap.
            if opening == Opening::Depot {
                for player in [PlayerId(0), PlayerId(1)] {
                    assert!(
                        sim.player_view(player).population_cap > 10,
                        "{name}/seed {seed}: the depot never completed"
                    );
                }
            }
        }
    }
}

/// One soak configuration run twice is bit-identical (soak determinism).
#[test]
fn soak_configurations_are_deterministic() {
    let bundle = ContentBundle::load_dir(&repo_content()).expect("the repo content loads");
    let first = run_scripted(
        &bundle,
        42,
        Opening::Depot,
        Opening::WorkerHeavy,
        SOAK_TICKS,
    );
    let second = run_scripted(
        &bundle,
        42,
        Opening::Depot,
        Opening::WorkerHeavy,
        SOAK_TICKS,
    );
    assert_eq!(first.checkpoint_hashes, second.checkpoint_hashes);
    assert_eq!(first.final_hash, second.final_hash);
    assert_eq!(first.samples, second.samples);
    let _ = (Fx::ZERO, Vec2Fx::ZERO); // (imports kept honest)
}
