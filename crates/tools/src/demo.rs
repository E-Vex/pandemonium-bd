//! The demo trivial world and the scripted headless match (plan §12 `tools
//! headless`, milestone M1): a deterministic two-player skirmish over the
//! in-code fixture world, driven by a fixed command script, ending in a printed
//! final hash. The same driver records replays for `tools replay-verify`.

use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::Sim;
use pandemonium_sim::{
    CapTemplate, KindEconomy, KindTemplate, ResourceDef, ScheduledSpawnDef, SpawnDef, TrivialWorld,
};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, MatchSetup, PlayerId, PlayerSetup, ResourceId,
    Vec2Fx,
};

/// The demo map identity (echoed into the replay).
pub const DEMO_MAP_ID: u64 = 0x0044_454D_4F00_0001;

/// Kinds of the demo world, by [`pandemonium_sim_api::KindId`].
pub mod kinds {
    use pandemonium_sim_api::KindId;

    /// A line grunt: health + move + vision.
    pub const GRUNT: KindId = KindId(0);
    /// A watcher: vision only — orders to move it are refused (capability gate).
    pub const WATCHER: KindId = KindId(1);
    /// A decayer: health that decays — exercises death & cleanup mid-match.
    pub const DECAYER: KindId = KindId(2);
}

/// Builds the demo world (deterministic, seed-independent).
pub fn demo_world() -> TrivialWorld {
    TrivialWorld {
        // The demo map is open ground: no terrain obstacles, so every demo
        // order exercises the straight-line fast path of the M4 mover.
        passability: TrivialWorld::open_passability(64, 64),
        buildability: TrivialWorld::open_buildability(64, 64),
        map_id: DEMO_MAP_ID,
        width_tiles: 64,
        height_tiles: 64,
        kinds: vec![
            KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 350,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ],
                economy: KindEconomy::default(),
            },
            KindTemplate {
                caps: vec![CapTemplate::Vision {
                    radius_milli_tiles: 9000,
                }],
                economy: KindEconomy::default(),
            },
            KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 2,
                        regen_per_tick: -1,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 1600,
                        radius_milli_tiles: 350,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 5000,
                    },
                ],
                economy: KindEconomy::default(),
            },
        ],
        resources: vec![ResourceDef {
            resource: ResourceId(0),
            starting: 200,
        }],
        production: vec![],
        base_population_cap: 0,
        initial_spawns: vec![
            // Player 0: three grunts and a watcher.
            SpawnDef {
                owner: PlayerId(0),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(10, 10),
            },
            SpawnDef {
                owner: PlayerId(0),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(12, 10),
            },
            SpawnDef {
                owner: PlayerId(0),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(14, 10),
            },
            SpawnDef {
                owner: PlayerId(0),
                kind: kinds::WATCHER,
                pos: Vec2Fx::from_ints(12, 14),
            },
            // Player 1: mirrored.
            SpawnDef {
                owner: PlayerId(1),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(54, 54),
            },
            SpawnDef {
                owner: PlayerId(1),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(52, 54),
            },
            SpawnDef {
                owner: PlayerId(1),
                kind: kinds::GRUNT,
                pos: Vec2Fx::from_ints(50, 54),
            },
            SpawnDef {
                owner: PlayerId(1),
                kind: kinds::WATCHER,
                pos: Vec2Fx::from_ints(52, 50),
            },
        ],
        scheduled_spawns: vec![
            ScheduledSpawnDef {
                tick: 10,
                spawn: SpawnDef {
                    owner: PlayerId(0),
                    kind: kinds::GRUNT,
                    pos: Vec2Fx::from_ints(20, 20),
                },
            },
            ScheduledSpawnDef {
                tick: 20,
                spawn: SpawnDef {
                    owner: PlayerId(1),
                    kind: kinds::DECAYER,
                    pos: Vec2Fx::from_ints(44, 44),
                },
            },
            ScheduledSpawnDef {
                tick: 50,
                spawn: SpawnDef {
                    owner: PlayerId(0),
                    kind: kinds::GRUNT,
                    pos: Vec2Fx::from_ints(30, 30),
                },
            },
        ],
        // Nonzero jitter exercises the seeded RNG inside the hashed state.
        spawn_jitter_milli: 250,
    }
}

/// The demo match setup for a seed (two AI slots, per plan §12's `--p1 ai --p2 ai`
/// shape; controller kinds are labels until the AI crate lands in M7).
pub fn demo_setup(seed: u64) -> MatchSetup {
    MatchSetup {
        seed,
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
    }
}

/// The scripted command log: exactly the stream the driver feeds to `Sim::step`,
/// tick by tick — valid orders, queued orders, and deliberately invalid commands
/// whose rejections are part of the deterministic record.
pub fn demo_script() -> Vec<Command> {
    let p0 = PlayerId(0);
    let p1 = PlayerId(1);
    let mut script: Vec<Command> = Vec::new();
    // Per-issuer sequence counters (an issuer outside the match gets a synthetic
    // sequence — its commands are refused by the gate regardless).
    let mut seqs: Vec<(PlayerId, u32)> = vec![(p0, 0), (p1, 0)];
    let mut command = |issuer: PlayerId, tick: u32, queue: bool, kind: CommandKind| {
        let seq = match seqs.iter_mut().find(|(player, _)| *player == issuer) {
            Some(entry) => {
                entry.1 += 1;
                entry.1
            }
            None => 1,
        };
        Command {
            issuer,
            tick,
            seq,
            queue,
            kind,
        }
    };

    // Tick 0: both players march toward the middle.
    script.push(command(
        p0,
        0,
        false,
        CommandKind::Move {
            units: vec![EntityId(1), EntityId(2), EntityId(3)],
            target: Vec2Fx::from_ints(32, 32),
        },
    ));
    script.push(command(
        p1,
        0,
        false,
        CommandKind::Move {
            units: vec![EntityId(5), EntityId(6), EntityId(7)],
            target: Vec2Fx::from_ints(32, 32),
        },
    ));
    // Tick 0: an order on the watcher is refused (no Move capability) — recorded
    // all the same; the rejection is deterministic.
    script.push(command(
        p0,
        0,
        false,
        CommandKind::Move {
            units: vec![EntityId(4)],
            target: Vec2Fx::from_ints(31, 31),
        },
    ));

    // Tick 2: p0 queues a follow-up order.
    script.push(command(
        p0,
        2,
        true,
        CommandKind::Move {
            units: vec![EntityId(1)],
            target: Vec2Fx::from_ints(28, 36),
        },
    ));
    // Tick 2: attack orders are refused in M1 (no Attack capability yet).
    script.push(command(
        p0,
        2,
        false,
        CommandKind::Attack {
            units: vec![EntityId(1)],
            target: EntityId(5),
        },
    ));

    // Tick 4: an unknown issuer and an unknown entity (both refused). Note that
    // stale-tick rejections are a live-client phenomenon: a replay feeds every
    // command at its declared tick, so the recorded log itself is always
    // chronologically consistent (the gate's TickMismatch path is pinned by the
    // sim unit tests and the determinism suite instead).
    script.push(command(PlayerId(9), 4, false, CommandKind::Resign {}));
    script.push(command(
        p1,
        4,
        false,
        CommandKind::Move {
            units: vec![EntityId(999)],
            target: Vec2Fx::from_ints(1, 1),
        },
    ));

    // Tick 6: p1 stops one grunt.
    script.push(command(
        p1,
        6,
        false,
        CommandKind::Stop {
            units: vec![EntityId(6)],
        },
    ));

    // Tick 15: both sides re-route as the mid-match spawns arrive.
    script.push(command(
        p0,
        15,
        false,
        CommandKind::Move {
            units: vec![EntityId(9)],
            target: Vec2Fx::from_ints(26, 26),
        },
    ));
    script.push(command(
        p1,
        15,
        false,
        CommandKind::Move {
            units: vec![EntityId(5), EntityId(7)],
            target: Vec2Fx::from_ints(38, 38),
        },
    ));

    // Tick 30: p0 resigns (marks state; match rules are M8).
    script.push(command(p0, 30, false, CommandKind::Resign {}));

    // Ticks 60 and 120: late re-routes.
    script.push(command(
        p1,
        60,
        false,
        CommandKind::Move {
            units: vec![EntityId(5)],
            target: Vec2Fx::from_ints(40, 40),
        },
    ));
    script.push(command(
        p1,
        120,
        false,
        CommandKind::Move {
            units: vec![EntityId(7)],
            target: Vec2Fx::from_ints(45, 45),
        },
    ));
    script
}

/// Runs the scripted match headlessly and returns the checkpoints (tick 0 plus
/// every periodic hash plus the forced final one) and the final hash.
pub fn run_scripted(
    world: &TrivialWorld,
    setup: &MatchSetup,
    commands: &[Command],
    ticks: u32,
) -> (Vec<Checkpoint>, u64) {
    let mut sim = Sim::new(world, setup.clone());
    let mut checkpoints = vec![Checkpoint {
        tick: 0,
        hash: sim.state_hash(),
    }];

    // Feed commands grouped by tick, preserving the log's order within each tick
    // (feed order is significant for duplicate-sequence resolution, so the
    // re-simulation must see exactly the original order).
    let mut sorted: Vec<&Command> = commands.iter().collect();
    sorted.sort_by_key(|cmd| cmd.tick); // stable: preserves within-tick order
    let mut cursor = 0usize;

    while sim.tick() < ticks {
        let tick = sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        while cursor < sorted.len() && sorted[cursor].tick == tick {
            feed.push(sorted[cursor].clone());
            cursor += 1;
        }
        let out = sim.step(&feed);
        if let Some(hash) = out.hash {
            checkpoints.push(Checkpoint {
                tick: sim.tick(),
                hash,
            });
        }
    }

    // Force a final checkpoint exactly at the last simulated tick so the replay's
    // end is unambiguous (validate enforces final == last checkpoint).
    let final_hash = sim.state_hash();
    if checkpoints.last().map(|cp| cp.tick) != Some(sim.tick()) {
        checkpoints.push(Checkpoint {
            tick: sim.tick(),
            hash: final_hash,
        });
    }
    (checkpoints, final_hash)
}

/// Builds the replay record of the scripted match (plan §6.5's field list).
/// The command log records exactly what was fed to `Sim::step` — script entries
/// targeting ticks at or past the run length never ran and are not recorded.
pub fn record_replay(seed: u64, ticks: u32) -> ReplayFile {
    let world = demo_world();
    let setup = demo_setup(seed);
    let script: Vec<Command> = demo_script()
        .into_iter()
        .filter(|cmd| cmd.tick < ticks)
        .collect();
    let (checkpoints, final_hash) = run_scripted(&world, &setup, &script, ticks);
    ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        seed,
        player_setup: setup.players,
        commands: script,
        checkpoints,
        final_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_script_runs_deterministically() {
        let world = demo_world();
        let a = run_scripted(&world, &demo_setup(7), &demo_script(), 90);
        let b = run_scripted(&world, &demo_setup(7), &demo_script(), 90);
        assert_eq!(a, b);
        let c = run_scripted(&world, &demo_setup(8), &demo_script(), 90);
        assert_ne!(a, c, "the seed must matter (jitter)");
    }

    #[test]
    fn recorded_replay_satisfies_its_own_validation() {
        let replay = record_replay(7, 90);
        assert!(replay.validate().is_ok());
        // And it round-trips through the codec.
        let decoded = pandemonium_replay::ReplayFile::decode(&replay.encode()).unwrap();
        assert_eq!(decoded, replay);
    }
}
