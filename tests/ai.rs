//! M7 exit suite (plan §14 M7, §13 A5 + A11): **is AI parity real?**
//!
//! - **A5 (AI parity)** — the AI acts only through commands, and its economy
//!   obeys the player rules. Structurally: the `ai` crate reaches only `fx`
//!   and `sim_api` (the dependency law, re-asserted here), and the simulation
//!   sources never reference the AI crate — no AI-specific entry point exists.
//!   Behaviorally: a controller-driven match's ledger balances to the cent
//!   (starting + deliveries − accepted command costs, at the *data-defined*
//!   prices), and every entity beyond the starting forces traces to an
//!   accepted `Train` or `Build` command — nothing enters the world any other
//!   way.
//! - **A11 (command parity for all issuers)** — the same command from a
//!   "player" issuer and an "AI" issuer produces the same validation outcome.
//!   A battery of mirrored command pairs over the real content exercises
//!   every rejection class the gate can produce at match start (plus the
//!   valid ones); a proptest fuzz generates random mirrored commands; and
//!   controller *labels* (Human/Ai in the setup) never change an outcome —
//!   they are recorded match identity, not behavior.
//! - **AI-vs-AI headless matches complete** — several seeded matches over the
//!   real Alpha content run to their tick budget under the debug invariant
//!   checker, with real economies (deliveries), real infrastructure
//!   (constructions), real armies (productions), and real combat (hits,
//!   deaths); matches are bit-identical run-to-run (log, checkpoints, final
//!   hash), seeds diverge, and a golden hash pins the flagship match.
//! - **A2 over an AI-driven log** — the flagship match replays from its
//!   recorded command log alone (no controllers re-run) to every checkpoint
//!   and the final hash, through the codec round-trip.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pandemonium_ai::Controller;
use pandemonium_content::ContentBundle;
use pandemonium_engine::{alpha_controller, AiMatchHost};
use pandemonium_fx::Vec2Fx;
use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::{run_command_log, Sim};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, MatchSetup, PlayerId, PlayerSetup,
    RejectReason, Tick, TilePos,
};
use proptest::prelude::*;

/// The flagship match: four minutes of game time — long enough for both
/// scripts to raise supply, a barracks, armies, and trade blows.
const FLAGSHIP_TICKS: u32 = 7200;

/// The flagship match's seed.
const FLAGSHIP_SEED: u64 = 7;

/// How long each completion match runs (each seeded twice for the identity).
const COMPLETION_TICKS: u32 = 2400;

/// The kinds of the Alpha content, by entity id order (validated by the
/// content suite; pinned here for the battery's readability).
mod kinds {
    pub const BARRACKS: u32 = 0;
    pub const COMMAND_CENTER: u32 = 1;
    pub const ORE_NODE: u32 = 3;
    pub const RIFLEMAN: u32 = 5;
    pub const WORKER: u32 = 8;
}

fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
        .join("content")
}

fn bundle() -> &'static ContentBundle {
    static BUNDLE: OnceLock<ContentBundle> = OnceLock::new();
    BUNDLE.get_or_init(|| ContentBundle::load_dir(&repo_content()).expect("content loads"))
}

fn ai_setup(seed: u64, controllers: [ControllerKind; 2]) -> MatchSetup {
    MatchSetup {
        seed,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: controllers[0],
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: controllers[1],
            },
        ],
    }
}

fn slot_of(player: PlayerId) -> Option<usize> {
    match player.0 {
        0 => Some(0),
        1 => Some(1),
        _ => None,
    }
}

/// One controller-driven match with both Alpha opponents, plus everything the
/// suite asserts on (the tools driver's shape, in-test).
struct MatchRun {
    host_log: Vec<Command>,
    checkpoints: Vec<Checkpoint>,
    final_hash: u64,
    /// Every event, labeled with the post-step tick.
    events: Vec<(Tick, Event)>,
    /// The final snapshot's entity list.
    entities: Vec<pandemonium_sim_api::EntityView>,
    /// Final per-player Ore balances, indexed by slot 0/1.
    ore: [i64; 2],
}

fn run_ai_match(bundle: &ContentBundle, seed: u64, ticks: u32) -> MatchRun {
    let world = bundle.world();
    let setup = ai_setup(seed, [ControllerKind::Ai, ControllerKind::Ai]);
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = [PlayerId(0), PlayerId(1)]
        .into_iter()
        .map(|player| {
            (
                player,
                Box::new(alpha_controller(bundle, &world, player, seed)) as Box<dyn Controller>,
            )
        })
        .collect();
    let mut host = AiMatchHost::new(&world, setup, controllers);
    let mut checkpoints = vec![Checkpoint {
        tick: 0,
        hash: host.state_hash(),
    }];
    let mut events = Vec::new();
    while host.tick() < ticks {
        // The step applies the commands fed at this tick: its events belong
        // to this tick (the checkpoint hashes label the post-step tick).
        let applied = host.tick();
        let out = host.advance();
        events.extend(out.events.into_iter().map(|event| (applied, event)));
        if let Some(hash) = out.hash {
            checkpoints.push(Checkpoint {
                tick: host.tick(),
                hash,
            });
        }
    }
    let final_hash = host.state_hash();
    if checkpoints.last().map(|cp| cp.tick) != Some(host.tick()) {
        checkpoints.push(Checkpoint {
            tick: host.tick(),
            hash: final_hash,
        });
    }
    let ore = [0, 1].map(|slot| {
        host.player_view(PlayerId(slot as u8))
            .resources
            .first()
            .map(|resource| resource.amount)
            .unwrap_or(0)
    });
    MatchRun {
        host_log: host.log().to_vec(),
        checkpoints,
        final_hash,
        events,
        entities: host.snapshot().entities,
        ore,
    }
}

/// Rejection outcomes by (issuer, seq); absent = accepted.
type Outcomes = BTreeMap<(u8, u32), RejectReason>;

fn outcomes_of(events: &[Event]) -> Outcomes {
    let mut outcomes = Outcomes::new();
    for event in events {
        if let Event::CommandRejected {
            issuer,
            seq,
            reject,
            ..
        } = event
        {
            outcomes.insert((issuer.0, *seq), reject.reason);
        }
    }
    outcomes
}

// ---------------------------------------------------------------------------
// A5: parity is structural
// ---------------------------------------------------------------------------

#[test]
fn a5_the_ai_crate_reaches_only_fx_and_sim_api() {
    // The compile-time half of A5, re-asserted inside the acceptance suite:
    // "AI reaches into game state" cannot even link (the architecture law
    // enforces it on every build; this is the belt to its braces).
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let manifest = std::fs::read_to_string(root.join("crates/ai/Cargo.toml"))
        .expect("the ai manifest is readable");
    let mut deps = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_deps = line == "[dependencies]";
            continue;
        }
        if in_deps && line.contains('=') {
            deps.push(line.split('=').next().unwrap().trim().to_string());
        }
    }
    assert_eq!(
        deps,
        vec!["pandemonium-fx", "pandemonium-sim-api"],
        "the ai crate depends on exactly fx + sim_api (plan §4, FD-7)"
    );
}

#[test]
fn a5_the_simulation_never_references_the_ai_crate() {
    // The audit half of A5: the simulation exposes no AI-specific entry point
    // — it cannot even name the AI crate. (`ControllerKind` in sim_api is
    // shared vocabulary recorded by replays, not a hook; the label test
    // below proves labels never change behavior.)
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut violations = Vec::new();
    for crate_name in ["sim", "sim_api"] {
        let dir = root.join("crates").join(crate_name).join("src");
        let mut sources = Vec::new();
        collect_rust_sources(&dir, &mut sources);
        assert!(
            !sources.is_empty(),
            "expected sources in crates/{crate_name}"
        );
        for path in sources {
            let text = std::fs::read_to_string(&path).expect("readable");
            for token in ["pandemonium-ai", "pandemonium_ai"] {
                if text.contains(token) {
                    violations.push(format!("{} mentions {token}", path.display()));
                }
            }
        }
    }
    assert!(violations.is_empty(), "{violations:?}");
}

fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn a5_the_ai_pays_the_player_prices_and_owns_nothing_it_did_not_command() {
    let run = run_ai_match(bundle(), FLAGSHIP_SEED, FLAGSHIP_TICKS);

    let rejected: BTreeMap<(u32, u8, u32), ()> = run
        .events
        .iter()
        .filter_map(|(tick, event)| match event {
            Event::CommandRejected { issuer, seq, .. } => Some(((*tick, issuer.0, *seq), ())),
            _ => None,
        })
        .collect();
    assert!(
        !rejected.is_empty(),
        "the scripted opponent does get refused sometimes"
    );
    assert!(
        run.host_log
            .iter()
            .all(|command| !matches!(command.kind, CommandKind::CancelQueueItem { .. })),
        "the scripted opponent never cancels: the ledger identity needs no refunds"
    );

    // Entity ownership bookkeeping from the event stream: spawns carry owner
    // and kind; sites arrive through their builder.
    let mut owners: BTreeMap<EntityId, PlayerId> = BTreeMap::new();
    let mut starting = [0u32; 2];
    let mut produced = [0u32; 2];
    let mut constructed = [0u32; 2];
    let mut died = [0u32; 2];
    let mut delivered = [0i64; 2];
    let mut hits = [0u32; 2];
    // The initial batch is the leading run of Spawned events (Sim::new
    // buffers exactly those before the first step; production spawns only
    // appear after strictly-positive build times, i.e. after other events).
    let mut in_initial = true;
    for (_, event) in &run.events {
        match event {
            Event::Spawned { entity, owner, .. } => {
                if in_initial && slot_of(*owner).is_some() {
                    starting[slot_of(*owner).unwrap()] += 1;
                }
                owners.insert(*entity, *owner);
            }
            Event::ConstructionStarted { builder, site } => {
                if let Some(owner) = owners.get(builder).copied() {
                    owners.insert(*site, owner);
                    constructed[slot_of(owner).unwrap_or(0)] += 1;
                }
            }
            Event::Died { entity } => {
                if let Some(slot) = owners.get(entity).and_then(|owner| slot_of(*owner)) {
                    died[slot] += 1;
                }
            }
            Event::ResourceDelivered { worker, .. } => {
                if let Some(slot) = owners.get(worker).and_then(|owner| slot_of(*owner)) {
                    delivered[slot] += 1;
                }
            }
            Event::ProductionCompleted { producer, .. } => {
                if let Some(slot) = owners.get(producer).and_then(|owner| slot_of(*owner)) {
                    produced[slot] += 1;
                }
            }
            Event::AttackHit { attacker, .. } => {
                if let Some(slot) = owners.get(attacker).and_then(|owner| slot_of(*owner)) {
                    hits[slot] += 1;
                }
            }
            _ => {
                in_initial = false;
            }
        }
    }

    for (slot, player) in [PlayerId(0), PlayerId(1)].iter().enumerate() {
        let final_count = run
            .entities
            .iter()
            .filter(|entity| entity.owner == *player)
            .count() as u32;
        assert_eq!(
            final_count,
            starting[slot] + produced[slot] + constructed[slot] - died[slot],
            "player {}: everything beyond the starting forces is an accepted \
             Train or Build, minus deaths — no other door into the world",
            player.0
        );
    }
    // Both sides fought (the parity claim is about a *playing* AI).
    assert!(hits.iter().all(|count| *count > 0), "{hits:?}");

    // The ledger identity: the AI pays exactly the data-defined prices.
    let cost_of = |kind: u32| -> i64 {
        bundle()
            .entities
            .get(kind as usize)
            .map(|entity| entity.cost_ore)
            .unwrap_or(0)
    };
    let mut spent = [0i64; 2];
    for command in &run.host_log {
        if rejected.contains_key(&(command.tick, command.issuer.0, command.seq)) {
            continue;
        }
        let cost = match &command.kind {
            CommandKind::Train { unit, .. } => cost_of(unit.0),
            CommandKind::Build { structure, .. } => cost_of(structure.0),
            _ => 0,
        };
        if cost != 0 {
            spent[command.issuer.0 as usize] += cost;
        }
    }
    let rules = &bundle().rules;
    let ore = rules
        .resources
        .iter()
        .find(|resource| resource.id == "ore")
        .expect("the Alpha prices everything in Ore");
    let carry = bundle()
        .entities
        .get(kinds::WORKER as usize)
        .and_then(|worker| worker.capability("Gather"))
        .map(|gather| match gather {
            pandemonium_content::CapabilityDef::Gather { carry_amount, .. } => *carry_amount,
            _ => 10,
        })
        .unwrap_or(10);
    for slot in 0..2 {
        assert_eq!(
            run.ore[slot],
            ore.starting + delivered[slot] * carry - spent[slot],
            "player {slot}: starting + deliveries (carry {carry} per trip) - \
             accepted command costs at the data-defined prices",
        );
    }
}

// ---------------------------------------------------------------------------
// A11: command parity for all issuers
// ---------------------------------------------------------------------------

/// Builds the mirrored battery for one tick: for every case, the player-0
/// instance and the player-1 instance (own entities, mirrored positions),
/// adjacent in the list. Sequences run 1..=n per issuer.
fn battery(sim: &Sim, tick: Tick) -> Vec<Command> {
    let snapshot = sim.snapshot();
    let cc = |player: u8| {
        snapshot
            .entities
            .iter()
            .find(|entity| {
                entity.owner == PlayerId(player) && entity.kind.0 == kinds::COMMAND_CENTER
            })
            .map(|entity| entity.id)
            .expect("each player starts with a command center")
    };
    let worker = |player: u8| {
        snapshot
            .entities
            .iter()
            .find(|entity| entity.owner == PlayerId(player) && entity.kind.0 == kinds::WORKER)
            .map(|entity| entity.id)
            .expect("each player starts with workers")
    };
    let node = |player: u8| {
        // The node nearest that player's command center (visible to them).
        let home = snapshot
            .entities
            .iter()
            .find(|entity| entity.id == cc(player))
            .unwrap()
            .pos;
        let mut nodes: Vec<(u64, EntityId)> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.kind.0 == kinds::ORE_NODE)
            .map(|entity| ((entity.pos - home).len_sq_raw(), entity.id))
            .collect();
        nodes.sort();
        nodes.first().map(|(_, id)| *id).expect("nodes exist")
    };
    let kind = |id: u32| pandemonium_sim_api::KindId(id);

    // Each row: (player-0 kind, player-1 mirror kind).
    let rows: Vec<(CommandKind, CommandKind)> = vec![
        // Valid locomotion (mirrored targets).
        (
            CommandKind::Move {
                units: vec![worker(0)],
                target: Vec2Fx::from_ints(10, 14),
            },
            CommandKind::Move {
                units: vec![worker(1)],
                target: Vec2Fx::from_ints(53, 49),
            },
        ),
        // MissingCapability: structures do not move.
        (
            CommandKind::Move {
                units: vec![cc(0)],
                target: Vec2Fx::from_ints(10, 14),
            },
            CommandKind::Move {
                units: vec![cc(1)],
                target: Vec2Fx::from_ints(53, 49),
            },
        ),
        (
            CommandKind::AttackMove {
                units: vec![cc(0)],
                target: Vec2Fx::from_ints(10, 14),
            },
            CommandKind::AttackMove {
                units: vec![cc(1)],
                target: Vec2Fx::from_ints(53, 49),
            },
        ),
        // UnknownEntity.
        (
            CommandKind::Move {
                units: vec![EntityId(999)],
                target: Vec2Fx::from_ints(10, 14),
            },
            CommandKind::Move {
                units: vec![EntityId(999)],
                target: Vec2Fx::from_ints(53, 49),
            },
        ),
        // NotOwnedByIssuer.
        (
            CommandKind::Stop {
                units: vec![worker(1)],
            },
            CommandKind::Stop {
                units: vec![worker(0)],
            },
        ),
        // MissingCapability: workers cannot attack.
        (
            CommandKind::Attack {
                units: vec![worker(0)],
                target: EntityId(999),
            },
            CommandKind::Attack {
                units: vec![worker(1)],
                target: EntityId(999),
            },
        ),
        // InvalidTarget: attacking your own.
        (
            CommandKind::Attack {
                units: vec![],
                target: cc(0),
            },
            CommandKind::Attack {
                units: vec![],
                target: cc(1),
            },
        ),
        // NotVisible: the far enemy under fog.
        (
            CommandKind::Attack {
                units: vec![],
                target: worker(1),
            },
            CommandKind::Attack {
                units: vec![],
                target: worker(0),
            },
        ),
        // InvalidTarget: gathering at a non-node.
        (
            CommandKind::Gather {
                units: vec![worker(0)],
                node: cc(0),
            },
            CommandKind::Gather {
                units: vec![worker(1)],
                node: cc(1),
            },
        ),
        // Valid gathering (each side's own nearest node).
        (
            CommandKind::Gather {
                units: vec![worker(0)],
                node: node(0),
            },
            CommandKind::Gather {
                units: vec![worker(1)],
                node: node(1),
            },
        ),
        // Valid training (the command center trains workers).
        (
            CommandKind::Train {
                producer: cc(0),
                unit: kind(kinds::WORKER),
            },
            CommandKind::Train {
                producer: cc(1),
                unit: kind(kinds::WORKER),
            },
        ),
        // MissingCapability: the command center does not train combat units.
        (
            CommandKind::Train {
                producer: cc(0),
                unit: kind(kinds::RIFLEMAN),
            },
            CommandKind::Train {
                producer: cc(1),
                unit: kind(kinds::RIFLEMAN),
            },
        ),
        // MissingCapability: a worker is not a producer.
        (
            CommandKind::Train {
                producer: worker(0),
                unit: kind(kinds::WORKER),
            },
            CommandKind::Train {
                producer: worker(1),
                unit: kind(kinds::WORKER),
            },
        ),
        // UnknownKind.
        (
            CommandKind::Train {
                producer: cc(0),
                unit: kind(99),
            },
            CommandKind::Train {
                producer: cc(1),
                unit: kind(99),
            },
        ),
        // InvalidTarget: structures are built, not trained.
        (
            CommandKind::Train {
                producer: cc(0),
                unit: kind(kinds::BARRACKS),
            },
            CommandKind::Train {
                producer: cc(1),
                unit: kind(kinds::BARRACKS),
            },
        ),
        // InvalidTarget: units are trained, not built.
        (
            CommandKind::Build {
                worker: worker(0),
                structure: kind(kinds::WORKER),
                at: TilePos { x: 10, y: 10 },
            },
            CommandKind::Build {
                worker: worker(1),
                structure: kind(kinds::WORKER),
                at: TilePos { x: 52, y: 52 },
            },
        ),
        // CannotAfford: the command center costs 400, the purse holds 200.
        (
            CommandKind::Build {
                worker: worker(0),
                structure: kind(kinds::COMMAND_CENTER),
                at: TilePos { x: 10, y: 10 },
            },
            CommandKind::Build {
                worker: worker(1),
                structure: kind(kinds::COMMAND_CENTER),
                at: TilePos { x: 52, y: 52 },
            },
        ),
        // QueueIndexInvalid: the earlier valid Train queued one item, so an
        // index past it is invalid (a same-tick cancel of the valid item
        // itself would mutate state and muddy the mirror).
        (
            CommandKind::CancelQueueItem {
                producer: cc(0),
                index: 3,
            },
            CommandKind::CancelQueueItem {
                producer: cc(1),
                index: 3,
            },
        ),
        // Valid rally.
        (
            CommandKind::SetRally {
                producer: cc(0),
                target: Vec2Fx::from_ints(12, 12),
            },
            CommandKind::SetRally {
                producer: cc(1),
                target: Vec2Fx::from_ints(51, 51),
            },
        ),
        // Valid stop.
        (
            CommandKind::Stop {
                units: vec![worker(0)],
            },
            CommandKind::Stop {
                units: vec![worker(1)],
            },
        ),
    ];

    let mut commands = Vec::new();
    for (index, (p0_kind, p1_kind)) in rows.into_iter().enumerate() {
        let seq = (index + 1) as u32;
        commands.push(Command::new(PlayerId(0), tick, seq, p0_kind));
        commands.push(Command::new(PlayerId(1), tick, seq, p1_kind));
    }
    commands
}

/// Runs one battery pass under the given labels and returns the outcome map.
fn run_battery(labels: [ControllerKind; 2]) -> Outcomes {
    let world = bundle().world();
    let mut sim = Sim::new(&world, ai_setup(11, labels));
    let commands = battery(&sim, 0);
    let out = sim.step(&commands);
    outcomes_of(&out.events)
}

#[test]
fn a11_command_outcomes_are_issuer_blind() {
    // (a) The battery: player 0 (labeled Human) and player 1 (labeled Ai)
    //     issue structurally identical commands; every mirror pair must
    //     produce the same outcome.
    let outcomes = run_battery([ControllerKind::Human, ControllerKind::Ai]);
    let world = bundle().world();
    let sim = Sim::new(
        &world,
        ai_setup(11, [ControllerKind::Human, ControllerKind::Ai]),
    );
    let commands = battery(&sim, 0);
    let pairs: Vec<(&Command, &Command)> = commands
        .chunks(2)
        .map(|chunk| (&chunk[0], &chunk[1]))
        .collect();
    assert!(
        pairs.len() >= 20,
        "the battery covers every rejection class"
    );
    for (p0, p1) in &pairs {
        assert_eq!(p0.issuer, PlayerId(0));
        assert_eq!(p1.issuer, PlayerId(1));
        let out0 = outcomes.get(&(0, p0.seq)).copied();
        let out1 = outcomes.get(&(1, p1.seq)).copied();
        assert_eq!(
            out0, out1,
            "mirror pair diverged: {:?} vs {:?} ({:?} / {:?})",
            p0.kind, p1.kind, out0, out1
        );
    }

    // The battery exercises every rejection class reachable at match start.
    let mut seen: Vec<RejectReason> = outcomes.values().copied().collect();
    seen.sort_by_key(|reason| format!("{reason:?}"));
    seen.dedup();
    for reason in [
        RejectReason::MissingCapability,
        RejectReason::UnknownEntity,
        RejectReason::NotOwnedByIssuer,
        RejectReason::InvalidTarget,
        RejectReason::NotVisible,
        RejectReason::UnknownKind,
        RejectReason::CannotAfford,
        RejectReason::QueueIndexInvalid,
    ] {
        assert!(
            seen.contains(&reason),
            "the battery must exercise {reason:?} (saw {seen:?})"
        );
    }

    // (b) Labels never change outcomes: the same battery under Human/Human
    //     and Ai/Ai labels produces the identical outcome map.
    for labels in [
        [ControllerKind::Human, ControllerKind::Human],
        [ControllerKind::Ai, ControllerKind::Ai],
    ] {
        assert_eq!(
            run_battery(labels),
            outcomes,
            "controller labels are metadata, never behavior"
        );
    }

    // (c) TickMismatch, DuplicateSeq, PlayerMissing — fed directly.
    let mut sim = Sim::new(
        &world,
        ai_setup(11, [ControllerKind::Human, ControllerKind::Ai]),
    );
    let mut commands = battery(&sim, 0);
    let snapshot = sim.snapshot();
    let worker = |player: u8| {
        snapshot
            .entities
            .iter()
            .find(|entity| entity.owner == PlayerId(player) && entity.kind.0 == kinds::WORKER)
            .unwrap()
            .id
    };
    // Stale ticks, mirrored.
    commands.push(Command::new(
        PlayerId(0),
        9,
        90,
        CommandKind::Move {
            units: vec![worker(0)],
            target: Vec2Fx::from_ints(10, 14),
        },
    ));
    commands.push(Command::new(
        PlayerId(1),
        9,
        90,
        CommandKind::Move {
            units: vec![worker(1)],
            target: Vec2Fx::from_ints(53, 49),
        },
    ));
    // Duplicate sequences, mirrored (the second of each pair loses).
    commands.push(Command::new(
        PlayerId(0),
        0,
        1,
        CommandKind::Stop {
            units: vec![worker(0)],
        },
    ));
    commands.push(Command::new(
        PlayerId(1),
        0,
        1,
        CommandKind::Stop {
            units: vec![worker(1)],
        },
    ));
    let out = sim.step(&commands);
    let outcomes = outcomes_of(&out.events);
    assert_eq!(outcomes.get(&(0, 90)), Some(&RejectReason::TickMismatch));
    assert_eq!(outcomes.get(&(1, 90)), Some(&RejectReason::TickMismatch));
    assert_eq!(outcomes.get(&(0, 1)), Some(&RejectReason::DuplicateSeq));
    assert_eq!(outcomes.get(&(1, 1)), Some(&RejectReason::DuplicateSeq));

    // PlayerMissing: an issuer outside the match is refused identically.
    let mut sim = Sim::new(
        &world,
        ai_setup(11, [ControllerKind::Human, ControllerKind::Ai]),
    );
    let out = sim.step(&[
        Command::new(PlayerId(9), 0, 1, CommandKind::Resign {}),
        Command::new(PlayerId(9), 0, 2, CommandKind::Resign {}),
    ]);
    let outcomes = outcomes_of(&out.events);
    assert_eq!(outcomes.get(&(9, 1)), Some(&RejectReason::PlayerMissing));
    assert_eq!(outcomes.get(&(9, 2)), Some(&RejectReason::PlayerMissing));
}

// ---------------------------------------------------------------------------
// A11: the property test (plan §13: "same command from 'player' and 'AI'
// issuer produces same outcome" — a fuzz over random mirrored commands)
// ---------------------------------------------------------------------------

/// Side-relative entity slots the fuzz can reference.
///
/// 0 own command center, 1 own worker, 2 the enemy's worker, 3 the node
/// nearest the own base, 4 the node nearest the enemy base, 5 an unknown id.
struct Slots {
    own_cc: EntityId,
    own_worker: EntityId,
    enemy_worker: EntityId,
    own_node: EntityId,
    enemy_node: EntityId,
}

fn slots_for(sim: &Sim, side: u8) -> Slots {
    let snapshot = sim.snapshot();
    let other = 1 - side;
    let find = |player: u8, kind: u32| {
        snapshot
            .entities
            .iter()
            .find(|entity| entity.owner == PlayerId(player) && entity.kind.0 == kind)
            .map(|entity| entity.id)
            .expect("the starting forces exist")
    };
    let nearest_node = |player: u8| {
        let home = snapshot
            .entities
            .iter()
            .find(|entity| entity.id == find(player, kinds::COMMAND_CENTER))
            .unwrap()
            .pos;
        let mut nodes: Vec<(u64, EntityId)> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.kind.0 == kinds::ORE_NODE)
            .map(|entity| ((entity.pos - home).len_sq_raw(), entity.id))
            .collect();
        nodes.sort();
        nodes.first().map(|(_, id)| *id).expect("nodes exist")
    };
    Slots {
        own_cc: find(side, kinds::COMMAND_CENTER),
        own_worker: find(side, kinds::WORKER),
        enemy_worker: find(other, kinds::WORKER),
        own_node: nearest_node(side),
        enemy_node: nearest_node(other),
    }
}

proptest::proptest! {
    /// Random mirrored commands, both issuers, one fresh match per case: the
    /// player-0 instance references its own slots and a position; the player-1
    /// instance (the AI-labeled issuer) references the mirrored slots and the
    /// 180-degree-mirrored position. The Crossroads content is symmetric and
    /// the spawns mirror, so every generated pair is structurally identical —
    /// and structurally identical commands must validate identically.
    #[test]
    fn a11_fuzz_mirrored_commands_validate_identically(
        variant in 0usize..9,
        slot_a in 0u8..6,
        slot_b in 0u8..6,
        kind in 0u32..100,
        x in 0i32..64,
        y in 0i32..64,
        index in 0u16..4,
    ) {
        let world = bundle().world();
        let mut sim = Sim::new(&world, ai_setup(31, [ControllerKind::Human, ControllerKind::Ai]));
        let slots = [slots_for(&sim, 0), slots_for(&sim, 1)];
        let resolve = |side: usize, slot: u8| -> EntityId {
            let s = &slots[side];
            match slot {
                0 => s.own_cc,
                1 => s.own_worker,
                2 => s.enemy_worker,
                3 => s.own_node,
                4 => s.enemy_node,
                _ => EntityId(999),
            }
        };
        // The mirrored position; build placements mirror footprint-aware so
        // the two candidates cover mirrored ground.
        let footprint = bundle()
            .entities
            .get(kind as usize)
            .and_then(|entity| entity.footprint())
            .unwrap_or((1, 1));
        let kind_of = |side: usize| -> CommandKind {
            let pos = Vec2Fx::from_ints(x, y);
            let mirrored = Vec2Fx::from_ints(63 - x, 63 - y);
            let at = TilePos { x, y };
            let mirrored_at = TilePos {
                x: 63 - x - footprint.0 as i32 + 1,
                y: 63 - y - footprint.1 as i32 + 1,
            };
            match variant {
                0 => CommandKind::Move { units: vec![resolve(side, slot_a)], target: if side == 0 { pos } else { mirrored } },
                1 => CommandKind::AttackMove { units: vec![resolve(side, slot_a)], target: if side == 0 { pos } else { mirrored } },
                2 => CommandKind::Stop { units: vec![resolve(side, slot_a)] },
                3 => CommandKind::Gather { units: vec![resolve(side, slot_a)], node: resolve(side, slot_b) },
                4 => CommandKind::Attack { units: vec![resolve(side, slot_a)], target: resolve(side, slot_b) },
                5 => CommandKind::Build { worker: resolve(side, slot_a), structure: pandemonium_sim_api::KindId(kind), at: if side == 0 { at } else { mirrored_at } },
                6 => CommandKind::Train { producer: resolve(side, slot_a), unit: pandemonium_sim_api::KindId(kind) },
                7 => CommandKind::CancelQueueItem { producer: resolve(side, slot_a), index },
                _ => CommandKind::SetRally { producer: resolve(side, slot_a), target: if side == 0 { pos } else { mirrored } },
            }
        };
        let commands = vec![
            Command::new(PlayerId(0), 0, 1, kind_of(0)),
            Command::new(PlayerId(1), 0, 1, kind_of(1)),
        ];
        let out = sim.step(&commands);
        let outcomes = outcomes_of(&out.events);
        let out0 = outcomes.get(&(0, 1)).copied();
        let out1 = outcomes.get(&(1, 1)).copied();
        prop_assert!(out0 == out1, "mirror pair diverged (out0={out0:?} out1={out1:?}): {:?} (slots {}/{} kind {} at ({},{}))", commands[0].kind, slot_a, slot_b, kind, x, y);
    }
}

// ---------------------------------------------------------------------------
// AI-vs-AI: matches complete, deterministically, and replay from the log
// ---------------------------------------------------------------------------

#[test]
fn ai_vs_ai_matches_complete_and_are_deterministic() {
    for seed in [FLAGSHIP_SEED, FLAGSHIP_SEED + 1, FLAGSHIP_SEED + 2] {
        let run = run_ai_match(bundle(), seed, COMPLETION_TICKS);
        // Completion: the full budget ran — a stuck or panicking match never
        // gets here, and the debug A12 invariant checker ran inside every
        // step. The scripts actually played:
        let mut owners: BTreeMap<EntityId, PlayerId> = BTreeMap::new();
        let mut delivered = [0i64; 2];
        let mut constructed = [0u32; 2];
        let mut produced = [0u32; 2];
        for (_, event) in &run.events {
            match event {
                Event::Spawned { entity, owner, .. } => {
                    owners.insert(*entity, *owner);
                }
                Event::ConstructionStarted { builder, site } => {
                    if let Some(owner) = owners.get(builder).copied() {
                        if let Some(slot) = slot_of(owner) {
                            constructed[slot] += 1;
                        }
                        owners.insert(*site, owner);
                    }
                }
                Event::ResourceDelivered { worker, .. } => {
                    if let Some(slot) = owners.get(worker).and_then(|o| slot_of(*o)) {
                        delivered[slot] += 1;
                    }
                }
                Event::ProductionCompleted { producer, .. } => {
                    if let Some(slot) = owners.get(producer).and_then(|o| slot_of(*o)) {
                        produced[slot] += 1;
                    }
                }
                _ => {}
            }
        }
        assert!(
            delivered.iter().all(|ore| *ore > 0),
            "seed {seed}: both economies gathered: {delivered:?}"
        );
        assert!(
            constructed.iter().all(|count| *count > 0),
            "seed {seed}: both sides built structures: {constructed:?}"
        );
        assert!(
            produced.iter().all(|count| *count > 0),
            "seed {seed}: both sides trained units: {produced:?}"
        );

        // Determinism: the same seed twice is bit-identical (log and trail).
        let again = run_ai_match(bundle(), seed, COMPLETION_TICKS);
        assert_eq!(run.host_log, again.host_log, "seed {seed}: the log repeats");
        assert_eq!(run.checkpoints, again.checkpoints);
        assert_eq!(run.final_hash, again.final_hash);
    }
    // Seeds diverge: pre-wave decisions are seed-independent (the script's
    // only randomness is the wave jitter), so two seeds can share an early
    // log — but the hashed RNG state still diverges the states.
    let a = run_ai_match(bundle(), FLAGSHIP_SEED, COMPLETION_TICKS);
    let b = run_ai_match(bundle(), FLAGSHIP_SEED + 1, COMPLETION_TICKS);
    assert_ne!(a.final_hash, b.final_hash);
}

#[test]
fn ai_vs_ai_golden_hash_is_pinned() {
    let run = run_ai_match(bundle(), FLAGSHIP_SEED, FLAGSHIP_TICKS);

    // Pinned after the first green run (M7). Any change to the controller's
    // decisions, the hosting order, or the simulation shows up as a value
    // change in review; regenerate deliberately when a milestone intends one.
    //
    // M9 re-pin (written reason): the simulation now pops a chase order
    // whose commanded target died (combat stage 1 — the M8 flagship's
    // stall diagnosis: defense Attack orders outlived their dead
    // intruders and the armies froze reporting Moving with empty paths).
    // Orders are canonical hashed state, so the tick-0 checkpoint is
    // unchanged (no orders exist yet) and every post-combat checkpoint
    // moves. The M9 AI tuning re-pins this again on top (see below).
    //
    // M9 tuning re-pin (written reason): the wave machine now re-issues
    // its march on the pressure cadence while a wave is alive (a stalled
    // wave never parks), focuses the sighted enemy command center, and
    // marches waves of ten with an army cap of sixteen — controller
    // decisions are canonical hashed state, so the trail moves again.
    // Tick 0 was still the untouched 0x6161_3bca_16b8_f00e.
    //
    // M10.1 re-pin (written reason): the canonical hasher moved FNV-1a ->
    // xxHash64 (DEBT-001 repaid). Every digest moves — including tick 0 —
    // while the encoded bytes are unchanged (no encoding bump; see
    // docs/DEBT.md and docs/ASSUMPTIONS.md A-087).
    assert_eq!(run.checkpoints.len(), (FLAGSHIP_TICKS / 30) as usize + 1);
    assert_eq!(run.checkpoints[0].hash, 0x71a9_24ad_5799_e4b3);
    assert_eq!(run.final_hash, 0x6e9a_18bd_7c5f_699f);
    assert_eq!(run.final_hash, run.checkpoints.last().unwrap().hash);
}

#[test]
fn ai_matches_replay_from_their_log_alone() {
    let world = bundle().world();
    let run = run_ai_match(bundle(), FLAGSHIP_SEED, FLAGSHIP_TICKS);

    // A2 over an AI-driven log: the record validates, the codec round-trips,
    // and the re-simulation — controllers absent — reproduces every
    // checkpoint and the final hash. FD-1: the log *is* the match.
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        seed: FLAGSHIP_SEED,
        player_setup: ai_setup(FLAGSHIP_SEED, [ControllerKind::Ai, ControllerKind::Ai]).players,
        commands: run.host_log.clone(),
        checkpoints: run.checkpoints.clone(),
        final_hash: run.final_hash,
    };
    assert!(
        replay.validate().is_ok(),
        "the AI match record is a legal replay"
    );
    let decoded = ReplayFile::decode(&replay.encode()).expect("codec round-trip");
    assert_eq!(decoded, replay);

    let resim = run_command_log(
        &world,
        &MatchSetup {
            seed: FLAGSHIP_SEED,
            players: replay.player_setup.clone(),
        },
        &decoded.commands,
        FLAGSHIP_TICKS,
    );
    let checkpoints: Vec<Checkpoint> = resim
        .checkpoints
        .iter()
        .map(|&(tick, hash)| Checkpoint { tick, hash })
        .collect();
    assert_eq!(
        checkpoints, decoded.checkpoints,
        "every checkpoint reproduces from the log alone"
    );
    assert_eq!(
        resim.final_hash, decoded.final_hash,
        "the final hash reproduces from the log alone"
    );

    // The flagship match's evidence: a real war happened.
    let hits = run
        .events
        .iter()
        .filter(|(_, event)| matches!(event, Event::AttackHit { .. }))
        .count();
    assert!(hits > 0, "four minutes of Alpha AI must trade blows");
}
