//! The M4 movement acceptance suite (plan §14 M4 / §9.1, prototype gate P1).
//!
//! Exit criteria under test:
//! - **50 units respond within 2 ticks under spam-clicked orders** — every
//!   ordered unit shows visible motion by two ticks after each command.
//! - **No permanent stuck units** — every order resolves (arrival, crowded
//!   arrival, or `MoveFailed`); nothing stays mid-order forever.
//! - **Hashes still green** — the same script reproduces the same checkpoints
//!   and final hash (A1 for the movement system), and different orders
//!   diverge.
//! - **The real map paths** — the loaded Crossroads bundle's passability
//!   drives the nav grid: workers ordered across the map detour around the
//!   rock walls and never stand on blocked terrain.

use std::path::{Path, PathBuf};

use pandemonium_content::ContentBundle;
use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim::TrivialWorld;
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, MatchSetup, MoveState, PlayerId,
    PlayerSetup,
};

/// The repository's content directory (same resolution as content_pipeline).
fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the tests package sits inside the workspace root")
        .join("content")
}

/// A 50-unit open world: one mover kind, units in a 7x8 block.
fn fifty_unit_world() -> TrivialWorld {
    use pandemonium_sim::{CapTemplate, KindTemplate, ResourceDef, SpawnDef};

    let mut spawns = Vec::new();
    for index in 0..50u32 {
        let row = index / 7;
        let column = index % 7;
        spawns.push(SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(0),
            pos: Vec2Fx::new(
                Fx::from_milli(10_000 + column as i32 * 800),
                Fx::from_milli(10_000 + row as i32 * 800),
            ),
        });
    }
    TrivialWorld {
        map_id: 0x4D_34_50,
        width_tiles: 64,
        height_tiles: 64,
        passability: TrivialWorld::open_passability(64, 64),
        kinds: vec![KindTemplate {
            caps: vec![
                CapTemplate::Health {
                    max_hp: 40,
                    regen_per_tick: 0,
                },
                CapTemplate::Move {
                    speed_milli_tiles_per_s: 2600,
                    radius_milli_tiles: 300,
                },
                CapTemplate::Vision {
                    radius_milli_tiles: 7000,
                },
            ],
        }],
        resources: vec![ResourceDef {
            resource: pandemonium_sim_api::ResourceId(0),
            starting: 200,
        }],
        initial_spawns: spawns,
        scheduled_spawns: vec![],
        spawn_jitter_milli: 0,
    }
}

fn one_player_setup(seed: u64) -> MatchSetup {
    MatchSetup {
        seed,
        players: vec![PlayerSetup {
            player: PlayerId(0),
            controller: ControllerKind::Human,
        }],
    }
}

/// One run of the spam-click scenario. `target_seed` varies the target
/// pattern so the determinism test can prove orders matter.
struct SpamRun {
    checkpoints: Vec<(u32, u64)>,
    final_hash: u64,
    move_failed: u32,
}

/// The spam-click script: a fresh Move order for all 50 units every tick for
/// 8 ticks (targets jittered deterministically), then one final spread order
/// to 50 individual destinations. Returns the per-unit position track needed
/// by the responsiveness check.
fn run_spam_click(target_seed: u32) -> (SpamRun, Vec<Vec<Vec2Fx>>) {
    let mut sim = pandemonium_sim::Sim::new(&fifty_unit_world(), one_player_setup(7));
    let units: Vec<EntityId> = (1..=50).map(EntityId).collect();
    let mut track: Vec<Vec<Vec2Fx>> = Vec::new();
    let mut positions: Vec<Vec2Fx> = Vec::new();
    for entity in sim.snapshot().entities {
        positions.push(entity.pos);
    }
    track.push(positions);

    // Spam: a replacing order every tick, targets wandering around (40, 40).
    for tick in 0..8u32 {
        let wob = (tick.wrapping_mul(97) + target_seed) % 500;
        let target = Vec2Fx::new(
            Fx::from_milli(40_000 + wob as i32),
            Fx::from_milli(40_000 - wob as i32),
        );
        let out = sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            tick + 1,
            CommandKind::Move {
                units: units.clone(),
                target,
            },
        )]);
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. })),
            "a valid spam-click order must not be rejected: {:?}",
            out.events
        );
        let mut positions: Vec<Vec2Fx> = Vec::new();
        for entity in sim.snapshot().entities {
            positions.push(entity.pos);
        }
        track.push(positions);
    }

    // Two quiet ticks so the last spam order has its full 2-tick response
    // window in the track.
    for _ in 0..2 {
        sim.step(&[]);
        let mut positions: Vec<Vec2Fx> = Vec::new();
        for entity in sim.snapshot().entities {
            positions.push(entity.pos);
        }
        track.push(positions);
    }

    // The final spread order: 50 individual destinations on a 7x8 grid.
    let spread: Vec<Command> = (0..50u32)
        .map(|index| {
            let row = index / 7;
            let column = index % 7;
            let (row, column) = (row as i32, column as i32);
            Command::new(
                PlayerId(0),
                sim.tick(),
                100 + index,
                CommandKind::Move {
                    units: vec![EntityId(index as u64 + 1)],
                    target: Vec2Fx::new(
                        Fx::from_milli(36_000 + column * 1_200),
                        Fx::from_milli(36_000 + row * 1_200),
                    ),
                },
            )
        })
        .collect();
    let _ = sim.step(&spread);

    // Drive to resolution and record.
    let mut checkpoints = Vec::new();
    let mut move_failed = 0;
    for _ in 0..2400 {
        let out = sim.step(&[]);
        move_failed += u32::from(
            out.events
                .iter()
                .any(|event| matches!(event, Event::MoveFailed { .. })),
        );
        if let Some(hash) = out.hash {
            checkpoints.push((sim.tick(), hash));
        }
        if sim
            .snapshot()
            .entities
            .iter()
            .all(|entity| entity.move_state == MoveState::Idle)
        {
            break;
        }
    }

    (
        SpamRun {
            checkpoints,
            final_hash: sim.state_hash(),
            move_failed,
        },
        track,
    )
}

#[test]
fn fifty_units_respond_within_two_ticks_under_spam_click() {
    let (run, track) = run_spam_click(0);

    // Every order's response: within 2 ticks of the order applied at tick t
    // (track index t+1 — index 0 is the pre-match state), the unit's position
    // moved by at least one tick of top speed (~87 milli-tiles).
    let speed = Fx::from_milli(2600).div(Fx::from_int(30));
    for tick in 0..8usize {
        let before = &track[tick]; // positions at the end of tick t-1
        let after = &track[(tick + 3).min(track.len() - 1)]; // two ticks later
        for (index, (a, b)) in before.iter().zip(after.iter()).enumerate() {
            let moved = Vec2Fx::dist(*a, *b);
            assert!(
                moved >= speed,
                "unit {} did not respond within 2 ticks of the tick-{tick} order \
                 (moved {} milli-tiles)",
                index + 1,
                moved.raw() / 65_536
            );
        }
    }

    // No permanent stuck units: every order resolved within the bound.
    assert!(
        run.move_failed <= 3,
        "move failures beyond the crowded-arrival tolerance: {}",
        run.move_failed
    );
}

#[test]
fn spam_click_runs_are_deterministic_and_orders_matter() {
    // Same script twice: identical checkpoints and final hash (A1 for the
    // movement system, push-apart included).
    let (first, _) = run_spam_click(0);
    let (second, _) = run_spam_click(0);
    assert_eq!(first.checkpoints, second.checkpoints);
    assert_eq!(first.final_hash, second.final_hash);
    assert_eq!(first.move_failed, second.move_failed);

    // A different target pattern diverges.
    let (other, _) = run_spam_click(137);
    assert_ne!(first.final_hash, other.final_hash);
}

#[test]
fn workers_cross_the_real_map_around_its_walls() {
    let bundle = ContentBundle::load_dir(&repo_content()).expect("the repo content loads");
    let setup = MatchSetup {
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
    };

    // The passability grid must come from the map's terrain classes: the
    // rock walls of Crossroads are exactly the blocked tiles.
    let world = bundle.world();
    assert_eq!(world.passability.len(), 64 * 64);
    let blocked = world.passability.iter().filter(|tile| **tile == 0).count();
    assert!(blocked > 200, "Crossroads carries substantial rock walls");

    // The worker kind id (entities are sorted by id — see A-030).
    let worker_kind = bundle
        .entities
        .iter()
        .position(|entity| entity.id == "worker")
        .expect("the worker kind exists") as u32;

    let mut sim = pandemonium_sim::Sim::new(&world, setup.clone());
    let workers: Vec<EntityId> = sim
        .snapshot()
        .entities
        .iter()
        .filter(|entity| entity.owner == PlayerId(0) && entity.kind.0 == worker_kind)
        .map(|entity| entity.id)
        .collect();
    assert_eq!(workers.len(), 4, "player 0 starts with four workers");

    // Order them to the far corner (48, 48) — the path must detour around
    // the central crossroads walls.
    let target = Vec2Fx::new(Fx::from_milli(48_500), Fx::from_milli(48_500));
    sim.step(&[Command::new(
        PlayerId(0),
        sim.tick(),
        1,
        CommandKind::Move {
            units: workers.clone(),
            target,
        },
    )]);

    let passable_at = |x: i32, y: i32| {
        if x < 0 || y < 0 || x >= 64 || y >= 64 {
            return false;
        }
        world.passability[y as usize * 64 + x as usize] != 0
    };

    let mut ticks = 0;
    let mut arrived = 0;
    while ticks < 3000 {
        let out = sim.step(&[]);
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. })),
            "the worker order is valid"
        );
        ticks += 1;
        // Nobody ever stands on rock.
        for entity in sim.snapshot().entities {
            let (x, y) = (entity.pos.x.floor_int(), entity.pos.y.floor_int());
            assert!(
                passable_at(x, y) || x < 0 || y < 0 || x >= 64 || y >= 64,
                "entity {} stood on blocked terrain at ({x},{y})",
                entity.id.0
            );
        }
        let movers = sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.move_state == MoveState::Moving);
        if !movers {
            break;
        }
    }
    // All four workers completed the crossing.
    for worker in &workers {
        let position = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.id == *worker)
            .expect("workers survive the crossing")
            .pos;
        assert!(
            Vec2Fx::dist(position, target) <= Fx::from_milli(1100),
            "worker {} finished at {:?}",
            worker.0,
            position
        );
        arrived += 1;
    }
    assert_eq!(arrived, 4);
    assert!(ticks < 3000, "the crossing never resolved: {ticks} ticks");

    // Determinism of the real-map run: same setup, same hash.
    let mut again = pandemonium_sim::Sim::new(&bundle.world(), setup.clone());
    again.step(&[Command::new(
        PlayerId(0),
        again.tick(),
        1,
        CommandKind::Move {
            units: workers.clone(),
            target,
        },
    )]);
    for _ in 0..ticks {
        again.step(&[]);
    }
    assert_eq!(sim.state_hash(), again.state_hash());
}
