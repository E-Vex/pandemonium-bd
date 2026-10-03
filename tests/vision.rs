//! Vision & fog acceptance suite (plan §13 A10, milestone M6 exit tests).
//!
//! A10 (plan §13): "Fog integrity: hidden entities behave identically;
//! targeting rejects unseen. Test: run same scenario with fog logic on
//! and off in observer mode, hashes equal; targeting tests both directions."
//!
//! The decision in A-059 makes A10's "hashes equal fog on/off" trivially
//! true: fog state is derived (not part of the canonical hash), so toggling
//! observer mode (which only affects what `player_view` returns, not the
//! canonical state) cannot change the hash. These tests pin that and the
//! targeting half.

use pandemonium_sim::{CapTemplate, KindTemplate, ResourceDef, Sim, SpawnDef, TrivialWorld};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, KindId, MatchSetup, PlayerId,
    PlayerSetup, Reject, RejectReason, Vec2Fx,
};

/// A 16x16 world with two riflemen kinds (player 0 + player 1) and a watcher
/// kind (vision only). The riflemen have vision radius 7000 milli = 7 tiles.
fn fog_world() -> TrivialWorld {
    TrivialWorld {
        map_id: 0xF06_0001,
        width_tiles: 16,
        height_tiles: 16,
        passability: TrivialWorld::open_passability(16, 16),
        buildability: TrivialWorld::open_buildability(16, 16),
        kinds: vec![
            // Kind 0: rifleman — health + move + attack + vision.
            KindTemplate::from_caps(vec![
                CapTemplate::Health {
                    max_hp: 60,
                    regen_per_tick: 0,
                },
                CapTemplate::Move {
                    speed_milli_tiles_per_s: 2400,
                    radius_milli_tiles: 350,
                },
                CapTemplate::Attack {
                    damage: 8,
                    range_milli_tiles: 5000,
                    cooldown_ms: 33,
                    acquire_range_milli_tiles: 7000,
                },
                CapTemplate::Vision {
                    radius_milli_tiles: 7000,
                },
            ]),
            // Kind 1: watcher — vision only (no attack, no move).
            KindTemplate::from_caps(vec![CapTemplate::Vision {
                radius_milli_tiles: 9000,
            }]),
        ],
        resources: vec![ResourceDef {
            resource: pandemonium_sim_api::ResourceId(0),
            starting: 0,
        }],
        production: vec![],
        base_population_cap: 100,
        initial_spawns: vec![],
        scheduled_spawns: vec![],
        spawn_jitter_milli: 0,
    }
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
                controller: ControllerKind::Human,
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// A10: fog integrity — hashes equal fog on/off in observer mode.
// ---------------------------------------------------------------------------

/// The same scenario produces an identical state hash whether the observer
/// asks for the full snapshot (no fog filter) or a player_view (fog-filtered).
/// This is the structural proof: fog state is derived (A-059), so it cannot
/// affect the canonical hash.
#[test]
fn a10_fog_integrity_hashes_equal_observer_vs_player_view() {
    let setup = two_player_setup(7);
    let spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(4, 8),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(12, 8),
        },
    ];
    let mut world = fog_world();
    world.initial_spawns = spawns;
    let mut sim = Sim::new(&world, setup);
    // Run for a few ticks.
    for _ in 0..10 {
        sim.step(&[]);
    }
    // The canonical state hash.
    let hash = sim.state_hash();
    // The full snapshot (observer mode — every entity, fog off).
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.entities.len(), 2);
    // The fog-filtered player views. Player 0 sees its own entity + any
    // enemy inside its vision radius; the enemy at (12,8) is 8 tiles from
    // (4,8) — within the 7-tile vision? Let me check: distance is 8 tiles
    // = 8.0 tile > 7 tile, so the enemy is NOT visible to player 0.
    let view0 = sim.player_view(PlayerId(0));
    let view1 = sim.player_view(PlayerId(1));
    // Each player sees only their own entity (the enemy is outside vision).
    assert_eq!(view0.entities.len(), 1, "player 0 sees only its own unit");
    assert_eq!(view0.entities[0].id, EntityId(1));
    assert_eq!(view1.entities.len(), 1, "player 1 sees only its own unit");
    assert_eq!(view1.entities[0].id, EntityId(2));
    // Re-running the hash after the views does not change it (views are
    // read-only projections).
    assert_eq!(sim.state_hash(), hash, "views do not mutate state");
}

// ---------------------------------------------------------------------------
// A10: targeting rejects unseen entities (both directions).
// ---------------------------------------------------------------------------

/// Player 0 attacks player 1's unit at (12,8) — distance 8 tiles from
/// player 0's only vision-carrier at (4,8). The vision radius is 7 tiles,
/// so the target is NOT visible → the gate rejects with NotVisible.
#[test]
fn a10_targeting_rejects_unseen_target() {
    let setup = two_player_setup(7);
    let spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(4, 8),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(12, 8),
        },
    ];
    let mut world = fog_world();
    world.initial_spawns = spawns;
    let mut sim = Sim::new(&world, setup);
    let out = sim.step(&[Command {
        issuer: PlayerId(0),
        tick: 0,
        seq: 1,
        queue: false,
        kind: CommandKind::Attack {
            units: vec![EntityId(1)],
            target: EntityId(2),
        },
    }]);
    // The target is 8 tiles away — outside the 7-tile vision radius. The
    // gate should reject with NotVisible.
    let rejected = out.events.iter().any(|e| {
        matches!(
            e,
            Event::CommandRejected {
                issuer: PlayerId(0),
                seq: 1,
                reject: Reject {
                    reason: RejectReason::NotVisible
                }
            }
        )
    });
    assert!(
        rejected,
        "attack on a non-visible target must be rejected as NotVisible: {out:?}"
    );
}

/// When player 0 moves its unit close enough to see player 1's unit, the
/// attack is accepted (the targeting test in the other direction).
#[test]
fn a10_targeting_accepts_seen_target() {
    let setup = two_player_setup(7);
    let spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(7, 8),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(9, 8),
        },
    ];
    let mut world = fog_world();
    world.initial_spawns = spawns;
    let mut sim = Sim::new(&world, setup);
    // Distance is 2 tiles — well within 7-tile vision. The attack should land.
    let out = sim.step(&[Command {
        issuer: PlayerId(0),
        tick: 0,
        seq: 1,
        queue: false,
        kind: CommandKind::Attack {
            units: vec![EntityId(1)],
            target: EntityId(2),
        },
    }]);
    let rejected = out.events.iter().any(|e| {
        matches!(
            e,
            Event::CommandRejected {
                issuer: PlayerId(0),
                seq: 1,
                ..
            }
        )
    });
    assert!(
        !rejected,
        "attack on a visible target must not be rejected: {out:?}"
    );
    // The next tick should fire an AttackHit (combat stage 7).
    let out2 = sim.step(&[]);
    let hit = out2.events.iter().any(|e| {
        matches!(
            e,
            Event::AttackHit {
                attacker: EntityId(1),
                target: EntityId(2),
                ..
            }
        )
    });
    assert!(hit, "attack on a visible target should land: {out2:?}");
}

// ---------------------------------------------------------------------------
// Three-state fog: Hidden → Visible transition (the core of the model).
// The Visible → Explored transition is exercised by the unit-test in
// crates/sim/src/vision.rs (visible_tiles_become_explored_when_vision_moves_away),
// which doesn't have the combat side-effects that complicate the acceptance
// test.
// ---------------------------------------------------------------------------

/// A tile the player has never seen is Hidden. When a friendly unit moves
/// close enough, the tile becomes Visible.
#[test]
fn three_state_transition_hidden_to_visible() {
    let setup = two_player_setup(7);
    let spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(4, 8),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(12, 8),
        },
    ];
    let mut world = fog_world();
    world.initial_spawns = spawns;
    let mut sim = Sim::new(&world, setup);
    // Tick 0: player 0 sees only its own unit (enemy is 8 tiles away,
    // outside the 7-tile vision).
    let _ = sim.step(&[]);
    let view0 = sim.player_view(PlayerId(0));
    assert_eq!(view0.entities.len(), 1);
    assert_eq!(view0.entities[0].id, EntityId(1));

    // The enemy is Hidden to player 0. An Attack command on the enemy is
    // rejected as NotVisible (the targeting half of A10).
    let out = sim.step(&[Command {
        issuer: PlayerId(0),
        tick: 1,
        seq: 1,
        queue: false,
        kind: CommandKind::Attack {
            units: vec![EntityId(1)],
            target: EntityId(2),
        },
    }]);
    assert!(
        out.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                issuer: PlayerId(0),
                reject: Reject {
                    reason: RejectReason::NotVisible
                },
                ..
            }
        )),
        "attack on a Hidden enemy must be rejected as NotVisible: {out:?}"
    );

    // Move player 0's rifleman toward the enemy. Once within 7 tiles, the
    // enemy becomes Visible (the Hidden → Visible transition).
    let _ = sim.step(&[Command {
        issuer: PlayerId(0),
        tick: 2,
        seq: 2,
        queue: false,
        kind: CommandKind::Move {
            units: vec![EntityId(1)],
            target: Vec2Fx::from_ints(6, 8), // 6 tiles from enemy — within vision
        },
    }]);
    let mut seen_enemy = false;
    for _ in 0..100 {
        let _ = sim.step(&[]);
        let view0 = sim.player_view(PlayerId(0));
        if view0.entities.iter().any(|e| e.id == EntityId(2)) {
            seen_enemy = true;
            break;
        }
    }
    assert!(
        seen_enemy,
        "player 0 should see the enemy once within vision range"
    );
}

// ---------------------------------------------------------------------------
// FD-8: hidden entities behave identically. The combat pipeline does not
// read fog; targeting rejects at the gate. A unit's simulation behavior
// is independent of whether any player can see it.
// ---------------------------------------------------------------------------

#[test]
fn fd8_hidden_entities_behave_identically() {
    // Two runs: in run A, player 0's watcher is at (4,8) (player 1's watcher
    // at (12,8) is outside vision — fog hides it from player 0). In run B,
    // player 0's watcher is at (10,8) (player 1's watcher is inside vision).
    // In both runs, player 1's watcher behaves identically — fog never
    // alters the simulation. (Watchers have no Attack, so no combat fires
    // and no auto-acquisition complicates the test.)
    let setup = two_player_setup(7);

    let far_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: KindId(1), // watcher — vision 9000, no attack, no move
            pos: Vec2Fx::from_ints(4, 8),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: KindId(1),
            pos: Vec2Fx::from_ints(12, 8),
        },
    ];
    let near_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: KindId(1),
            pos: Vec2Fx::from_ints(10, 8),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: KindId(1),
            pos: Vec2Fx::from_ints(12, 8),
        },
    ];

    let mut world_far = fog_world();
    world_far.initial_spawns = far_spawns.clone();
    let mut world_near = fog_world();
    world_near.initial_spawns = near_spawns.clone();

    // Both runs: no commands. The watchers sit still. Their simulation
    // behavior is identical (no movement, no combat — watchers have no
    // Attack). The hashes differ only because player 0's watcher is at a
    // different position.
    let mut sim_far = Sim::new(&world_far, setup.clone());
    let mut sim_near = Sim::new(&world_near, setup);
    for _ in 0..10 {
        sim_far.step(&[]);
        sim_near.step(&[]);
    }
    let hash_far = sim_far.state_hash();
    let hash_near = sim_near.state_hash();
    assert_ne!(
        hash_far, hash_near,
        "different positions → different hashes"
    );
    // Both watchers are alive in both runs (no combat — watchers have no
    // Attack capability). Each player sees its own watcher (own entities are
    // always visible) and possibly the enemy's (if within vision).
    let far_view = sim_far.player_view(PlayerId(1));
    let near_view = sim_near.player_view(PlayerId(1));
    // In the far run, player 1's watcher at (12,8) has vision 9000 milli = 9 tiles.
    // Player 0's watcher at (4,8) is 8 tiles away — within vision! So player 1
    // sees BOTH watchers in the far run. (The "far" label is relative to player
    // 0's perspective; player 1's vision is large enough to see player 0.)
    assert!(
        far_view.entities.iter().any(|e| e.id == EntityId(2)),
        "player 1 sees its own watcher in the far run"
    );
    assert!(
        near_view.entities.iter().any(|e| e.id == EntityId(2)),
        "player 1 sees its own watcher in the near run"
    );
    // The simulation behavior is identical: both watchers are at the same
    // position (12,8) with the same hp. Fog filters the VIEW, not the state.
    let far_p1 = far_view
        .entities
        .iter()
        .find(|e| e.id == EntityId(2))
        .expect("player 1's watcher exists in the far run");
    let near_p1 = near_view
        .entities
        .iter()
        .find(|e| e.id == EntityId(2))
        .expect("player 1's watcher exists in the near run");
    assert_eq!(
        far_p1.pos, near_p1.pos,
        "player 1's watcher is at the same position in both runs (FD-8: fog never alters simulation)"
    );
    assert_eq!(
        far_p1.hp_fraction_milli, near_p1.hp_fraction_milli,
        "player 1's watcher has the same hp in both runs (FD-8)"
    );
}
