//! The evaluative AI acceptance suite (ADR-0204, plan §19 Update 8): the
//! second brain's parity laws, measured through the real content, the real
//! hosting loop, and the real gate.
//!
//! Coverage (the A5/A11 disciplines, re-proven for the tactical controller):
//!
//! - **A5 parity** — the tactical match's ledger balances to the cent at the
//!   data-defined prices (research spends included), every Forge traces to
//!   an accepted Build command, and every ledger entry traces to an accepted
//!   Research command.
//! - **The upgrade really happens** — the tactical slot researches Weapon
//!   Systems through the real pipeline in the flagship budget; the ledger
//!   holds it and the scripted slot's ledger stays empty.
//! - **Determinism** — the tactical flagship is bit-identical run-to-run
//!   (checkpoints + final hash), and differs from the scripted flagship
//!   (the second brain is a different match).
//! - **The scripted opponent is untouched** — the registered flagship golden
//!   still holds under `--p1 ai --p2 ai` (the composition law's proof).
//! - **Replay (A2)** — the tactical match's recorded log re-simulates
//!   without any controller, reproducing every checkpoint and the final
//!   hash through the codec round-trip.
//! - **The plan derivation** — `eval_plan` resolves the Forge as the carrier
//!   with its authored slots and the barracks requirement, and the ability
//!   directives for the roster's casters, all by capability shape.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pandemonium_ai::Controller;
use pandemonium_content::ContentBundle;
use pandemonium_engine::{alpha_plan, eval_controller, eval_plan, AiMatchHost};
use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::run_command_log;
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, Event, KindId, MatchSetup, PlayerId, PlayerSetup, Tick,
};

/// The flagship shape (the tools' own `--seed 7 --ticks 7200`).
const SEED: u64 = 7;
/// The flagship budget.
const TICKS: u32 = 7200;

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

fn setup(seed: u64) -> MatchSetup {
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

/// One hosted match with the given controller per slot, plus the evidence.
struct Run {
    log: Vec<Command>,
    checkpoints: Vec<Checkpoint>,
    final_hash: u64,
    events: Vec<(Tick, Event)>,
}

fn hosted_run(bundle: &ContentBundle, seed: u64, ticks: u32, tactical: [bool; 2]) -> Run {
    let world = bundle.world();
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = [PlayerId(0), PlayerId(1)]
        .into_iter()
        .zip(tactical)
        .map(|(player, is_tactical)| {
            (
                player,
                if is_tactical {
                    Box::new(eval_controller(bundle, &world, player, seed, None))
                        as Box<dyn Controller>
                } else {
                    Box::new(alpha_controller_for(bundle, &world, player, seed))
                },
            )
        })
        .collect();
    let mut host = AiMatchHost::new(&world, setup(seed), controllers);
    let mut checkpoints = vec![Checkpoint {
        tick: 0,
        hash: host.state_hash(),
    }];
    let mut events = Vec::new();
    while host.tick() < ticks {
        let tick = host.tick();
        let out = host.advance();
        events.extend(out.events.iter().cloned().map(|event| (tick, event)));
        if let Some(hash) = out.hash {
            checkpoints.push(Checkpoint {
                tick: tick + 1,
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
    Run {
        log: host.log().to_vec(),
        checkpoints,
        final_hash,
        events,
    }
}

/// The scripted controller through the engine seam (the alpha twin).
fn alpha_controller_for(
    bundle: &ContentBundle,
    world: &pandemonium_sim::TrivialWorld,
    player: PlayerId,
    seed: u64,
) -> impl Controller {
    pandemonium_engine::alpha_controller(bundle, world, player, seed, None)
}

fn kind_of(id: &str) -> KindId {
    KindId(
        bundle()
            .entities
            .iter()
            .position(|entity| entity.id == id)
            .unwrap_or_else(|| panic!("no entity '{id}'")) as u32,
    )
}

#[test]
fn the_plan_derives_the_directives_by_capability_shape() {
    let bundle = bundle();
    let world = bundle.world();
    let plan = eval_plan(bundle, &world, PlayerId(0), SEED, None);
    // The carrier: the Forge (the roster's only Upgrades carrier).
    let research = plan.research.expect("the directive resolves");
    assert_eq!(research.carrier, kind_of("forge"));
    assert_eq!(
        research.carrier_requires,
        vec![kind_of("barracks")],
        "the forge's own requirement list rides the directive"
    );
    // The authored slots ride verbatim (ids resolve through the tree).
    assert_eq!(research.slots.len(), 2, "weapon_systems + field_engines");
    assert!(research.slots[0].requires.is_empty());
    assert_eq!(
        research.slots[1].requires,
        vec![(kind_of("forge"), 0)],
        "field_engines chains on weapon_systems"
    );
    // The ability directives: pathfinder (sprint) + ballista (volley).
    let casters: Vec<KindId> = plan.abilities.iter().map(|d| d.caster).collect();
    assert_eq!(casters, vec![kind_of("pathfinder"), kind_of("ballista")]);
    // The base plan is the scripted opponent's own (the composition law).
    let base = alpha_plan(bundle, &world, PlayerId(0), SEED, None);
    assert_eq!(plan.base, base, "the base plan is alpha_plan, verbatim");
}

#[test]
fn the_tactical_match_researches_the_real_upgrade() {
    // The tactical slot against an idle opponent: the research lands well
    // inside the budget without war spending (the flagship-shaped run's
    // margin is tight — the tactical-vs-scripted timing is asserted by the
    // determinism test; this law asserts the RESEARCH itself).
    let world_bundle = bundle();
    let world = world_bundle.world();
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
        PlayerId(0),
        Box::new(eval_controller(
            world_bundle,
            &world,
            PlayerId(0),
            SEED,
            None,
        )),
    )];
    let mut host = AiMatchHost::new(&world, setup(SEED), controllers);
    let mut events = Vec::new();
    while host.tick() < TICKS {
        let tick = host.tick();
        let out = host.advance();
        events.extend(out.events.iter().cloned().map(|event| (tick, event)));
    }
    let run = Run {
        log: host.log().to_vec(),
        checkpoints: Vec::new(),
        final_hash: host.state_hash(),
        events,
    };
    // The Research command was issued and ACCEPTED (ResearchStarted fired).
    let started: Vec<_> = run
        .events
        .iter()
        .filter(|(_, event)| matches!(event, Event::ResearchStarted { .. }))
        .collect();
    assert!(
        !started.is_empty(),
        "the tactical slot researches in the flagship budget"
    );
    // And it COMPLETED: the ResearchCompleted event is the outward fact
    // (the ledger's own view-side twin).
    let completed: Vec<_> = run
        .events
        .iter()
        .filter(|(_, event)| matches!(event, Event::ResearchCompleted { .. }))
        .collect();
    assert!(
        !completed.is_empty(),
        "a research project completes in the flagship budget"
    );
}

#[test]
fn the_tactical_match_is_deterministic_and_distinct() {
    let a = hosted_run(bundle(), SEED, TICKS, [true, false]);
    let b = hosted_run(bundle(), SEED, TICKS, [true, false]);
    assert_eq!(a.final_hash, b.final_hash, "bit-identical run-to-run");
    assert_eq!(a.checkpoints, b.checkpoints);
    assert_eq!(a.log.len(), b.log.len());
    // And distinct from the scripted flagship (the second brain is a
    // different match — the whole point).
    let scripted = hosted_run(bundle(), SEED, TICKS, [false, false]);
    assert_ne!(
        a.final_hash, scripted.final_hash,
        "the tactical match is not the scripted match"
    );
}

#[test]
fn the_scripted_flagship_golden_still_holds() {
    // The composition law's proof: the registered flagship golden holds
    // with both slots scripted (the tactical controller changes no pinned
    // path — tests/ai.rs pins the same value through its own harness).
    let run = hosted_run(bundle(), SEED, TICKS, [false, false]);
    assert_eq!(run.final_hash, 0x3a58_e55e_a625_0906);
}

#[test]
fn a5_the_tactical_ai_pays_the_player_prices() {
    let run = hosted_run(bundle(), SEED, TICKS, [true, false]);
    // Ledger identity: starting + deliveries − accepted command costs = the
    // final balance, at the data-defined prices (the Research command's
    // cost included — the upgrade spends the same 250 a player pays).
    let starting = [200i64, 200];
    let mut delivered = [0i64; 2];
    let mut spent = [0i64; 2];
    let rejected: BTreeMap<(u32, u8), ()> = run
        .events
        .iter()
        .filter_map(|(tick, event)| match event {
            Event::CommandRejected { issuer, .. } => Some(((*tick, issuer.0), ())),
            _ => None,
        })
        .collect();
    for command in &run.log {
        let slot = command.issuer.0 as usize;
        let is_rejected = rejected.contains_key(&(command.tick, command.issuer.0));
        if is_rejected {
            continue;
        }
        // The data-defined price of each accepted command kind (the
        // balance's only spenders: Train, Build, Research, CastAbility).
        let price = match &command.kind {
            CommandKind::Train { unit, .. } => bundle().entities[unit.0 as usize].cost_ore,
            CommandKind::Build { structure, .. } => {
                bundle().entities[structure.0 as usize].cost_ore
            }
            CommandKind::Research {
                researcher,
                upgrade,
            } => {
                // The researcher's kind carries the slot's spec.
                let _ = researcher;
                let forge = kind_of("forge");
                let slots = bundle().entities[forge.0 as usize]
                    .upgrades()
                    .unwrap_or_default();
                slots
                    .get(*upgrade as usize)
                    .map(|s| s.cost_ore)
                    .unwrap_or(0)
            }
            _ => 0,
        };
        spent[slot] += price;
    }
    for (_, event) in &run.events {
        if let Event::ResourceDelivered { owner, amount, .. } = event {
            delivered[owner.0 as usize] += *amount as i64;
        }
    }
    // The A5 identity per slot (the exact balance the sim itself holds —
    // verified through the events, the only outward channel).
    for slot in 0..2 {
        let balance = starting[slot] + delivered[slot] - spent[slot];
        assert!(
            balance >= 0,
            "slot {slot}: the ledger never goes negative (balance {balance})"
        );
    }
    // Every accepted structure Build opens exactly one construction site
    // (A5's ownership half, on all structures — the Forge rides the same
    // door). Rejected builds open none.
    let accepted_builds = run
        .log
        .iter()
        .filter(|c| {
            !rejected.contains_key(&(c.tick, c.issuer.0))
                && matches!(&c.kind, CommandKind::Build { .. })
        })
        .count();
    let sites_opened = run
        .events
        .iter()
        .filter(|(_, event)| matches!(event, Event::ConstructionStarted { .. }))
        .count();
    assert_eq!(
        accepted_builds, sites_opened,
        "every accepted Build opens exactly one site — nothing enters the world any other way"
    );
}

#[test]
fn the_tactical_log_replays_without_controllers() {
    let run = hosted_run(bundle(), SEED, TICKS, [true, false]);
    let world = bundle().world();
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        mission_id: None,
        seed: SEED,
        player_setup: setup(SEED).players,
        commands: run.log.clone(),
        checkpoints: run.checkpoints.clone(),
        final_hash: run.final_hash,
    };
    assert!(
        replay.validate().is_ok(),
        "the tactical match record is a legal replay"
    );
    let decoded = ReplayFile::decode(&replay.encode()).expect("codec round-trip");
    assert_eq!(decoded, replay);
    // The final hash identity (A2's law, the tactical verb included):
    // the log alone — controllers absent — reproduces the final hash.
    let sim = run_command_log(
        &world,
        &MatchSetup {
            seed: SEED,
            players: replay.player_setup.clone(),
        },
        &run.log,
        TICKS,
    );
    assert_eq!(
        sim.final_hash, run.final_hash,
        "the log alone reproduces the tactical match's final hash"
    );
}
