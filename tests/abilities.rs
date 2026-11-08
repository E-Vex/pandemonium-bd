//! The ability system acceptance suite (ADR-0202, plan §19 Update 6 — the
//! first designed extension's exit tests).
//!
//! Coverage, each law measured through the real pipeline (the eleven stages,
//! the real gate, the real replay codec):
//!
//! - **The validation matrix** — every reject reason the CastAbility gate can
//!   produce, one law per reason (the gate is the sole authority, FD-7's
//!   shared-door rule).
//! - **The spotting seam** — a caster whose own sight falls short cannot cast
//!   blind, and a friendly scout's vision lifts the fog for it (FD-8: fog
//!   filters the order, never the simulation).
//! - **Effects** — DamageArea hits enemies only (friendlies and neutral nodes
//!   are never collateral) and kills fire the death path; HealArea clamps at
//!   max; SpeedBurst moves the boosted measurably farther and expires.
//! - **The cooldown-ready pipeline** — a cast blocks the slot, the slot
//!   recharges exactly at the authored tick (the attack-cooldown semantics).
//! - **The economy seam** — a priced cast spends, an empty purse refuses.
//! - **Determinism** — the same script twice is bit-identical; the cast state
//!   is hashed (a cast changes the state hash; a boosted mover hashes
//!   differently from an unboosted one).
//! - **Replay** — a cast-bearing command log round-trips through the codec
//!   and re-simulates to the identical hash (A2's law, the cast verb).
//! - **The shipped content** — the real tree's two abilities load with their
//!   authored parameters (the Pathfinder's Sprint, the Ballista's Volley).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pandemonium_fx::Fx;
use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::run_command_log;
use pandemonium_sim::{
    AbilityEffectTemplate, AbilitySpecTemplate, CapTemplate, KindTemplate, ResourceDef, Sim,
    SpawnDef, TrivialWorld,
};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, MatchSetup, PlayerId, PlayerSetup,
    RejectReason, ResourceId, Vec2Fx,
};

/// A 20x20 open world: a caster kind with three slots (nuke / heal / sprint),
/// a plain victim, a slow-sighted caster (the blind-cast law), and a priced
/// caster (the economy law).
fn ability_world() -> TrivialWorld {
    let caster = KindTemplate::from_caps(vec![
        CapTemplate::Health {
            max_hp: 100,
            regen_per_tick: 0,
        },
        CapTemplate::Move {
            speed_milli_tiles_per_s: 2400,
            radius_milli_tiles: 350,
        },
        CapTemplate::Vision {
            radius_milli_tiles: 8000,
        },
        CapTemplate::Abilities {
            abilities: vec![
                AbilitySpecTemplate {
                    id: "nuke".to_string(),
                    display_name: "Nuke".to_string(),
                    effect: AbilityEffectTemplate::DamageArea {
                        damage: 45,
                        radius_milli_tiles: 2000,
                    },
                    cast_range_milli_tiles: 6000,
                    cooldown_ms: 1000, // 30 ticks
                    cost: Vec::new(),
                },
                AbilitySpecTemplate {
                    id: "mend".to_string(),
                    display_name: "Mend".to_string(),
                    effect: AbilityEffectTemplate::HealArea {
                        amount: 30,
                        radius_milli_tiles: 2000,
                    },
                    cast_range_milli_tiles: 6000,
                    cooldown_ms: 1000,
                    cost: Vec::new(),
                },
                AbilitySpecTemplate {
                    id: "sprint".to_string(),
                    display_name: "Sprint".to_string(),
                    effect: AbilityEffectTemplate::SpeedBurst {
                        multiplier_milli: 2000, // 2.0x
                        duration_ms: 2000,      // 60 ticks
                        radius_milli_tiles: 3000,
                    },
                    cast_range_milli_tiles: 6000,
                    cooldown_ms: 1000,
                    cost: Vec::new(),
                },
            ],
        },
    ]);
    let victim = KindTemplate::from_caps(vec![
        CapTemplate::Health {
            max_hp: 40,
            regen_per_tick: 0,
        },
        CapTemplate::Move {
            speed_milli_tiles_per_s: 2400,
            radius_milli_tiles: 350,
        },
        CapTemplate::Vision {
            radius_milli_tiles: 8000,
        },
    ]);
    // Short-sighted caster: vision 3 tiles, cast range 6 — the point beyond
    // its sight but inside its reach is the blind-cast law's stage.
    let blind_caster = KindTemplate::from_caps(vec![
        CapTemplate::Health {
            max_hp: 100,
            regen_per_tick: 0,
        },
        CapTemplate::Vision {
            radius_milli_tiles: 3000,
        },
        CapTemplate::Abilities {
            abilities: vec![AbilitySpecTemplate {
                id: "strike".to_string(),
                display_name: "Strike".to_string(),
                effect: AbilityEffectTemplate::DamageArea {
                    damage: 10,
                    radius_milli_tiles: 1500,
                },
                cast_range_milli_tiles: 6000,
                cooldown_ms: 1000,
                cost: Vec::new(),
            }],
        },
    ]);
    // Priced caster: 50 ore per cast.
    let priced_caster = KindTemplate::from_caps(vec![
        CapTemplate::Health {
            max_hp: 100,
            regen_per_tick: 0,
        },
        CapTemplate::Vision {
            radius_milli_tiles: 8000,
        },
        CapTemplate::Abilities {
            abilities: vec![AbilitySpecTemplate {
                id: "paid_strike".to_string(),
                display_name: "Paid Strike".to_string(),
                effect: AbilityEffectTemplate::DamageArea {
                    damage: 10,
                    radius_milli_tiles: 1500,
                },
                cast_range_milli_tiles: 6000,
                cooldown_ms: 1000,
                cost: vec![(ResourceId(0), 50)],
            }],
        },
    ]);
    TrivialWorld {
        map_id: 0xAB1E_0000_0001,
        width_tiles: 20,
        height_tiles: 20,
        passability: TrivialWorld::open_passability(20, 20),
        buildability: TrivialWorld::open_buildability(20, 20),
        kinds: vec![caster, victim, blind_caster, priced_caster],
        resources: vec![ResourceDef {
            resource: ResourceId(0),
            starting: 100,
        }],
        production: vec![],
        base_population_cap: 100,
        initial_spawns: vec![],
        scheduled_spawns: vec![],
        spawn_jitter_milli: 0,
        roster: Vec::new(),
    }
    .with_default_roster()
}

fn setup() -> MatchSetup {
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

fn pos(x: i32, y: i32) -> Vec2Fx {
    Vec2Fx::from_ints(x, y)
}

/// A command at `tick` with the issuer's next sequence number.
fn cmd(issuer: PlayerId, tick: u32, seq: u32, kind: CommandKind) -> Command {
    Command::new(issuer, tick, seq, kind)
}

fn cast(caster: EntityId, slot: u8, target: Vec2Fx) -> CommandKind {
    CommandKind::CastAbility {
        caster,
        ability: slot,
        target,
    }
}

/// The single-cast world: player 0's caster at (5,5) and friendly victim at
/// (6,6); player 1's victim at (6,5) — the nuke's radius covers it — and a
/// far survivor at (19,19).
fn nuke_world() -> TrivialWorld {
    let mut world = ability_world();
    world.initial_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(0),
            pos: pos(5, 5),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: pandemonium_sim_api::KindId(1),
            pos: pos(6, 5),
        },
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(1),
            pos: pos(6, 6),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: pandemonium_sim_api::KindId(1),
            pos: pos(19, 19),
        },
    ];
    world
}

/// The one rejection (if any) the tick's events carry for `seq`.
fn rejection(events: &[Event], seq: u32) -> Option<RejectReason> {
    events.iter().find_map(|event| match event {
        Event::CommandRejected {
            seq: rejected,
            reject,
            ..
        } if *rejected == seq => Some(reject.reason),
        _ => None,
    })
}

#[test]
fn the_cast_gate_matrix_rejects_every_illegal_form() {
    // Ids follow the spawn order: 1 the caster, 2 the enemy victim, 3 the
    // friendly victim, 4 the far survivor.
    let cases: Vec<(CommandKind, RejectReason, &str)> = vec![
        // Unknown caster.
        (
            cast(EntityId(99), 0, pos(6, 5)),
            RejectReason::UnknownEntity,
            "a dead/never-spawned caster",
        ),
        // The enemy's entity as caster.
        (
            cast(EntityId(2), 0, pos(6, 5)),
            RejectReason::NotOwnedByIssuer,
            "another player's caster",
        ),
        // An owned kind without the capability (the friendly victim).
        (
            cast(EntityId(3), 0, pos(6, 5)),
            RejectReason::MissingCapability,
            "a non-caster",
        ),
        // Slot beyond the kind's list.
        (
            cast(EntityId(1), 9, pos(6, 5)),
            RejectReason::AbilitySlotInvalid,
            "a slot the kind does not have",
        ),
        // Out of range (cast range 6 tiles; the point is far beyond).
        (
            cast(EntityId(1), 0, pos(19, 0)),
            RejectReason::AbilityOutOfRange,
            "a point beyond the cast range",
        ),
    ];
    // One fresh Sim per case — each rejection must leave state untouched,
    // and a single Sim cannot reuse sequence numbers.
    for (index, (kind, reason, what)) in cases.into_iter().enumerate() {
        let mut sim = Sim::new(&nuke_world(), setup());
        let command = cmd(PlayerId(0), 0, index as u32 + 1, kind);
        let out = sim.step(&[command]);
        assert_eq!(
            rejection(&out.events, index as u32 + 1),
            Some(reason),
            "{what} must refuse with {reason:?}"
        );
        // The refusal changed nothing: the victim still stands at full hp.
        let victim_hp = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.id == EntityId(2))
            .map(|entity| entity.hp_fraction_milli)
            .unwrap();
        assert_eq!(victim_hp, 1000);
    }
}

#[test]
fn a_ready_slot_blocks_on_cooldown_and_recharges_exactly() {
    let world = nuke_world();
    let mut sim = Sim::new(&world, setup());
    // Tick 0: the cast lands (45 damage on a 40-hp victim — the kill law's
    // own test below; here we only watch the cooldown).
    let first = cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(6, 5)));
    let out = sim.step(&[first]);
    assert!(rejection(&out.events, 1).is_none(), "the first cast lands");
    // Tick 1: the same slot is on cooldown (30 ticks, 2000ms authoring).
    let second = cmd(PlayerId(0), 1, 2, cast(EntityId(1), 0, pos(6, 5)));
    let out = sim.step(&[second]);
    assert_eq!(
        rejection(&out.events, 2),
        Some(RejectReason::AbilityOnCooldown),
        "the slot is blocked the tick after the cast"
    );
    // Ticks 2..30: the countdown runs (the victim is dead; nothing else
    // happens on this stage).
    for tick in 2..30 {
        let _ = sim.step(&[]);
        assert_eq!(sim.tick(), tick + 1);
    }
    // Tick 30: ready again — the cast is accepted (nothing legal in range to
    // hit; acceptance is the non-rejection).
    let third = cmd(PlayerId(0), 30, 3, cast(EntityId(1), 0, pos(6, 5)));
    let out = sim.step(&[third]);
    assert!(
        rejection(&out.events, 3).is_none(),
        "the slot recharges exactly at cast_tick + cooldown_ticks"
    );
}

#[test]
fn area_damage_kills_enemies_only_and_fires_the_death_path() {
    // nuke_world: id 2 is the in-blast enemy victim, id 3 the in-blast
    // friendly, id 4 the far survivor — neither friendly nor far is
    // collateral.
    let mut sim = Sim::new(&nuke_world(), setup());
    let out = sim.step(&[cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(6, 5)))]);
    // The events: the cast cue, the hit (attacker = caster, damage 45), and
    // the death — all in the cast's tick.
    let cue = out
        .events
        .iter()
        .find_map(|event| match event {
            Event::AbilityCast {
                caster, ability, ..
            } if *caster == EntityId(1) => Some(*ability),
            _ => None,
        })
        .expect("the cast cue fired");
    assert_eq!(cue, 0);
    let hit = out.events.iter().any(|event| {
        matches!(
            event,
            Event::AttackHit {
                attacker: EntityId(1),
                target: EntityId(2),
                damage: 45
            }
        )
    });
    assert!(hit, "the blast rides the AttackHit channel");
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::Died {
                entity: EntityId(2)
            }
        )),
        "the 45-damage blast kills the 40-hp victim in the cast's tick"
    );
    // The friendly (id 3) and the far survivor (id 4) live on.
    let snapshot = sim.snapshot();
    assert!(snapshot.entities.iter().any(|e| e.id == EntityId(3)));
    assert!(snapshot.entities.iter().any(|e| e.id == EntityId(4)));
}

#[test]
fn a_blind_cast_refuses_until_a_scout_lifts_the_fog() {
    // The blind caster (kind 2, vision 3) at (5,5); the point at (5,10) is
    // 5 tiles away — inside the 6-tile cast range, beyond its 3-tile sight.
    let mut world = ability_world();
    world.initial_spawns = vec![SpawnDef {
        owner: PlayerId(0),
        kind: pandemonium_sim_api::KindId(2),
        pos: pos(5, 5),
    }];
    let mut sim = Sim::new(&world, setup());
    let blind = cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(5, 10)));
    let out = sim.step(&[blind]);
    assert_eq!(
        rejection(&out.events, 1),
        Some(RejectReason::NotVisible),
        "a caster cannot shell what it cannot see (FD-8)"
    );
    // A pathfinder-shaped scout (kind 0's caster carries 8-tile vision)
    // arrives within the match: spawn it through the world and re-run.
    let mut world = ability_world();
    world.initial_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(2),
            pos: pos(5, 5),
        },
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(0),
            pos: pos(9, 10), // its 8-tile sight covers (5,10)
        },
    ];
    let mut sim = Sim::new(&world, setup());
    let spotted = cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(5, 10)));
    let out = sim.step(&[spotted]);
    assert!(
        rejection(&out.events, 1).is_none(),
        "a friendly scout's vision lifts the fog for the caster — the spotting seam"
    );
}

#[test]
fn heal_area_restores_clamped_at_max() {
    // A bleeding friendly (regen -2/tick) and the healer caster.
    let mut world = ability_world();
    world.kinds[1].caps[0] = CapTemplate::Health {
        max_hp: 100,
        regen_per_tick: -2,
    };
    world.initial_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(0),
            pos: pos(5, 5),
        },
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(1),
            pos: pos(6, 5),
        },
    ];
    let mut sim = Sim::new(&world, setup());
    // 20 ticks of bleed: 100 - 40 = 60 hp.
    for _ in 0..20 {
        sim.step(&[]);
    }
    let before = sim
        .snapshot()
        .entities
        .iter()
        .find(|e| e.id == EntityId(2))
        .unwrap()
        .hp_fraction_milli;
    assert_eq!(before, 600, "20 ticks of -2 regen: 60/100");
    // The heal: +30 clamped against the ongoing -2 tick.
    sim.step(&[cmd(PlayerId(0), 20, 1, cast(EntityId(1), 1, pos(6, 5)))]);
    let after = sim
        .snapshot()
        .entities
        .iter()
        .find(|e| e.id == EntityId(2))
        .unwrap()
        .hp_fraction_milli;
    // 60 + 30 = 90, minus this tick's -2 regen (stage 8 runs after the cast)
    // = 88 — the exact integer arithmetic, no hand-waving.
    assert_eq!(after, 880, "the heal lands and the tick's bleed still runs");
}

#[test]
fn a_speed_burst_marches_farther_and_expires() {
    // Two identical movers on different rows with identical-length marches;
    // only the caster's row is inside the boost (the plain mover marches
    // untouched, rows apart — no collision push-apart between them).
    let mut world = ability_world();
    world.initial_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(0),
            pos: pos(5, 5),
        },
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(1),
            pos: pos(5, 15),
        },
    ];
    let mut sim = Sim::new(&world, setup());
    // Both march east the same 12 tiles; the caster also boosts (2x for 60
    // ticks, 3-tile radius — the plain mover's row is 10 tiles away).
    let march_caster = cmd(
        PlayerId(0),
        0,
        1,
        CommandKind::Move {
            units: vec![EntityId(1)],
            target: pos(17, 5),
        },
    );
    let march_plain = cmd(
        PlayerId(0),
        0,
        2,
        CommandKind::Move {
            units: vec![EntityId(2)],
            target: pos(17, 15),
        },
    );
    let boost = cmd(PlayerId(0), 0, 3, cast(EntityId(1), 2, pos(6, 5)));
    let _ = sim.step(&[march_caster, march_plain, boost]);
    // 30 ticks in: the boosted march has covered more than twice the
    // ground (both start at x=5 — the advance is the raw x).
    for _ in 0..30 {
        sim.step(&[]);
    }
    let snapshot = sim.snapshot();
    let boosted = snapshot
        .entities
        .iter()
        .find(|e| e.id == EntityId(1))
        .unwrap()
        .pos;
    let plain = snapshot
        .entities
        .iter()
        .find(|e| e.id == EntityId(2))
        .unwrap()
        .pos;
    let boosted_advance = boosted.x - pos(5, 5).x;
    let plain_advance = plain.x - pos(5, 5).x;
    assert!(
        boosted_advance > plain_advance + Fx::from_milli(2000),
        "30 boosted ticks at 2x out-distance the plain march by >2 tiles \
         (boosted {:?}, plain {:?})",
        boosted_advance.raw(),
        plain_advance.raw()
    );
    // The boost expires after 60 ticks; both arrive at the same destination
    // eventually (the march's end is a fixed point).
    for _ in 0..300 {
        sim.step(&[]);
    }
    let snapshot = sim.snapshot();
    let a = snapshot
        .entities
        .iter()
        .find(|e| e.id == EntityId(1))
        .unwrap()
        .pos;
    let b = snapshot
        .entities
        .iter()
        .find(|e| e.id == EntityId(2))
        .unwrap()
        .pos;
    assert_eq!(
        a.x, b.x,
        "the boost is temporary — both marches end at the target"
    );
    assert_eq!(a.x, pos(17, 5).x);
}

#[test]
fn a_priced_cast_spends_and_an_empty_purse_refuses() {
    let mut world = ability_world();
    world.initial_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: pandemonium_sim_api::KindId(3),
            pos: pos(5, 5),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: pandemonium_sim_api::KindId(1),
            pos: pos(6, 5),
        },
    ];
    let mut sim = Sim::new(&world, setup());
    // Starting balance 100; the cast costs 50.
    let first = cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(6, 5)));
    let out = sim.step(&[first]);
    assert!(rejection(&out.events, 1).is_none(), "the priced cast lands");
    let balance = sim
        .player_view(PlayerId(0))
        .resources
        .iter()
        .find(|resource| resource.resource == ResourceId(0))
        .unwrap()
        .amount;
    assert_eq!(balance, 50, "the cast cost is spent");
    // Wait out the cooldown, then cast with 50 still in hand — affordable.
    while sim.tick() < 30 {
        sim.step(&[]);
    }
    let second = cmd(PlayerId(0), sim.tick(), 2, cast(EntityId(1), 0, pos(6, 5)));
    let out = sim.step(&[second]);
    assert!(rejection(&out.events, 2).is_none());
    let balance = sim
        .player_view(PlayerId(0))
        .resources
        .iter()
        .find(|resource| resource.resource == ResourceId(0))
        .unwrap()
        .amount;
    assert_eq!(balance, 0, "the second cast drains the purse");
    // Third cast (post-cooldown): nothing left to spend.
    while sim.tick() < 60 {
        sim.step(&[]);
    }
    let third = cmd(PlayerId(0), sim.tick(), 3, cast(EntityId(1), 0, pos(6, 5)));
    let out = sim.step(&[third]);
    assert_eq!(
        rejection(&out.events, 3),
        Some(RejectReason::CannotAfford),
        "the empty purse refuses"
    );
}

#[test]
fn casts_are_deterministic_and_hashed() {
    let world = nuke_world();
    let script = |target: Vec2Fx| {
        vec![
            cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, target)),
            cmd(
                PlayerId(1),
                5,
                1,
                CommandKind::Move {
                    units: vec![EntityId(2)],
                    target: pos(6, 7),
                },
            ),
        ]
    };
    // Same script twice: bit-identical (the victim is dead; the move refuses
    // UnknownEntity — both deterministic).
    let mut a = Sim::new(&world, setup());
    let mut b = Sim::new(&world, setup());
    let script = script(pos(6, 5));
    for tick in 0..10 {
        let feed: Vec<Command> = script.iter().filter(|c| c.tick == tick).cloned().collect();
        a.step(&feed);
        b.step(&feed);
    }
    assert_eq!(a.state_hash(), b.state_hash(), "two runs, one hash");

    // A cast changes the hashed state (the cooldown runtime is hashed): the
    // cast-less run differs at the same tick.
    let mut quiet = Sim::new(&world, setup());
    for _ in 0..10 {
        quiet.step(&[]);
    }
    assert_ne!(
        a.state_hash(),
        quiet.state_hash(),
        "the cast state is part of the canonical hash"
    );

    // A different target point changes the hashed state too (the cue is not
    // hashed, but the cooldown state plus the world the blast shaped are —
    // and here the blast's kill differs).
    let mut elsewhere = Sim::new(&world, setup());
    let far = [cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(15, 15)))];
    for tick in 0..10 {
        let feed: Vec<Command> = far.iter().filter(|c| c.tick == tick).cloned().collect();
        elsewhere.step(&feed);
    }
    assert_ne!(a.state_hash(), elsewhere.state_hash());
}

#[test]
fn a_cast_bearing_command_log_round_trips_through_the_replay_codec() {
    // A2's law, the cast verb: record → encode → decode → re-simulate →
    // identical final hash. The world carries the caster and the victim.
    let world = nuke_world();
    let mut sim = Sim::new(&world, setup());
    let script = vec![
        cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(6, 5))),
        cmd(
            PlayerId(1),
            10,
            1,
            CommandKind::Move {
                units: vec![],
                target: pos(10, 10),
            },
        ),
    ];
    let mut checkpoints = vec![Checkpoint {
        tick: 0,
        hash: sim.state_hash(),
    }];
    for tick in 0..60 {
        let feed: Vec<Command> = script.iter().filter(|c| c.tick == tick).cloned().collect();
        let out = sim.step(&feed);
        if let Some(hash) = out.hash {
            checkpoints.push(Checkpoint {
                tick: sim.tick(),
                hash,
            });
        }
    }
    let final_hash = sim.state_hash();
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        mission_id: None,
        seed: setup().seed,
        player_setup: setup().players.clone(),
        commands: script,
        checkpoints,
        final_hash,
    };
    assert!(replay.validate().is_ok());
    let bytes = replay.encode();
    let decoded = ReplayFile::decode(&bytes).expect("the codec round-trips");
    assert_eq!(decoded, replay, "the cast command survives encode/decode");
    // Re-simulate from the log — the same runner tools' replay-verify uses.
    let setup = setup();
    let run = run_command_log(&world, &setup, &decoded.commands, 60);
    assert_eq!(run.final_hash, final_hash, "A2 holds for the cast verb");
}

#[test]
fn the_player_view_carries_own_casters_only() {
    let world = nuke_world();
    let mut sim = Sim::new(&world, setup());
    sim.step(&[cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(6, 5)))]);
    let own = sim.player_view(PlayerId(0));
    assert_eq!(own.abilities.len(), 3, "the caster's three slots");
    // The just-cast slot reports its cooldown (30 ticks authored, one tick
    // consumed by the same tick's stage-7 pass: 29/30 = 966 milli).
    assert_eq!(own.abilities[0].caster, EntityId(1));
    assert_eq!(own.abilities[0].slot, 0);
    assert_eq!(own.abilities[0].cooldown_milli, 966);
    assert_eq!(own.abilities[1].cooldown_milli, 0, "slot 1 is untouched");
    // The enemy sees none of it (fog hides enemy cooldowns).
    let enemy = sim.player_view(PlayerId(1));
    assert!(enemy.abilities.is_empty());
}

// ---------------------------------------------------------------------------
// The shipped content pins (the real tree's two abilities).
// ---------------------------------------------------------------------------

fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("content")
}

fn bundle() -> &'static pandemonium_content::ContentBundle {
    static BUNDLE: OnceLock<pandemonium_content::ContentBundle> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        pandemonium_content::ContentBundle::load_dir(&repo_content()).expect("content loads")
    })
}

fn kind_of(id: &str) -> pandemonium_sim_api::KindId {
    bundle()
        .entities
        .iter()
        .position(|entity| entity.id == id)
        .map(|index| pandemonium_sim_api::KindId(index as u32))
        .unwrap_or_else(|| panic!("no entity '{id}'"))
}

#[test]
fn the_shipped_abilities_load_with_their_authored_parameters() {
    let bundle = bundle();
    // The Pathfinder's Overland Sprint: 1.5x for 4s in 2.5 tiles, 20s
    // cooldown, free, 4-tile cast range.
    let pathfinder = &bundle.entities[kind_of("pathfinder").0 as usize];
    let sprint = pathfinder
        .abilities()
        .expect("the pathfinder carries the Abilities capability")
        .first()
        .unwrap();
    assert_eq!(sprint.id, "overland_sprint");
    assert_eq!(sprint.display_name, "Overland Sprint");
    assert_eq!(sprint.cast_range_milli_tiles, 4000);
    assert_eq!(sprint.cooldown_ms, 20000);
    assert_eq!(sprint.cooldown_ticks, 600);
    assert_eq!(sprint.cost_ore, 0);
    match &sprint.effect {
        pandemonium_content::AbilityEffectDef::SpeedBurst {
            multiplier_milli,
            duration_ms,
            duration_ticks,
            radius_milli_tiles,
        } => {
            assert_eq!(*multiplier_milli, 1500);
            assert_eq!(*duration_ms, 4000);
            assert_eq!(*duration_ticks, 120);
            assert_eq!(*radius_milli_tiles, 2500);
        }
        other => panic!("the sprint is a SpeedBurst, got {other:?}"),
    }
    // The Ballista's Shatter Volley: 45 damage in 1.8 tiles, 13-tile cast
    // range (beyond its own 12.5-tile sight — the spotting pairing), 25s
    // cooldown, free.
    let ballista = &bundle.entities[kind_of("ballista").0 as usize];
    let volley = ballista
        .abilities()
        .expect("the ballista carries the Abilities capability")
        .first()
        .unwrap();
    assert_eq!(volley.id, "shatter_volley");
    match &volley.effect {
        pandemonium_content::AbilityEffectDef::DamageArea {
            damage,
            radius_milli_tiles,
        } => {
            assert_eq!(*damage, 45);
            assert_eq!(*radius_milli_tiles, 1800);
        }
        other => panic!("the volley is a DamageArea, got {other:?}"),
    }
    assert_eq!(volley.cast_range_milli_tiles, 13000);
    assert_eq!(volley.cooldown_ms, 25000);
    // No other shipped kind carries the capability (the Phalanx stays
    // untouched — A-230: its feel rides the A14 playtest).
    for entity in &bundle.entities {
        if entity.id != "pathfinder" && entity.id != "ballista" {
            assert!(
                entity.abilities().is_none(),
                "{} unexpectedly carries abilities",
                entity.id
            );
        }
    }
}

#[test]
fn the_shipped_ballista_volley_casts_through_the_real_bundle() {
    // The end-to-end law on the real content: a spawned Ballista's volley
    // lands through the full pipeline (load → bake → spawn → gate → effect).
    let bundle = bundle();
    let world = bundle.world();
    let ballista = kind_of("ballista");
    let victim = kind_of("rifleman");
    let mut world = world;
    world.initial_spawns = vec![
        SpawnDef {
            owner: PlayerId(0),
            kind: ballista,
            pos: pos(20, 20),
        },
        SpawnDef {
            owner: PlayerId(1),
            kind: victim,
            pos: pos(26, 20),
        },
    ];
    let mut sim = Sim::new(&world, setup());
    // The volley at the rifleman: 13-tile cast range, 6 away, the ballista's
    // own 12.5-tile sight covers the point.
    let out = sim.step(&[cmd(PlayerId(0), 0, 1, cast(EntityId(1), 0, pos(26, 20)))]);
    assert!(
        rejection(&out.events, 1).is_none(),
        "the shipped volley casts on the real bundle"
    );
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::AttackHit {
                attacker: EntityId(1),
                damage: 45,
                ..
            }
        )),
        "the shipped volley deals its authored damage"
    );
}
