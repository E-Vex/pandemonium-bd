//! B-021 — mission-controlled AI tuning (ADR-0201 Phase 2, the closing work
//! package): the scripted Alpha opponent's pacing consts become
//! mission-overridable through [`pandemonium_sim_api::AiSettings`] on
//! [`pandemonium_sim_api::MissionData`], with byte-exact defaults.
//!
//! This file owns the **baseline and identity surface**:
//!
//! - **The mission-less baseline** — the flagship-shape run (seed 7, both
//!   slots scripted, the default map's world) reproduces the registered
//!   flagship final hash AND this suite's own digests: the command
//!   stream's (the replay-wire encoding of the full feed order) and each
//!   controller RNG's full `(state, inc)` at ticks 300 and 7200. The
//!   controller is engine-side state that never enters the simulation's
//!   canonical state hash, so its random stream is pinned through
//!   `ScriptedController::rng_state`.
//! - **The None identity** — a mission whose `ai_settings` block is
//!   all-`None` drives the exact same run: same final hash, same command
//!   stream, same controller RNG states. `None` means today's const (the
//!   brief's byte-exact-defaults law).
//! - **The override direction** — tuning values derived from the script's
//!   own consts (never hardcoded) move observable behavior in the known
//!   direction, each against a same-shape all-`None` run: a shorter
//!   pressure cadence re-presses earlier, a later first-attack delay and a
//!   higher minimum wave delay the first march, a higher worker target
//!   grows the workforce further, a lower army cap holds the army at the
//!   cap.
//! - **The plan-builder invariants** — the cross-default shapes the
//!   content loader cannot see (content sits below the ai crate in the
//!   dependency law) are refused loudly at plan construction.
//! - **The replay round-trip** — a recording of a tuned mission carries
//!   the mission's id (which the tuning changes) and re-simulates to
//!   itself through the mission-carrying runner.
//!
//! The hosted runs step the exact `AiMatchHost::advance` contract (perceive
//! on the fog-filtered view, decide, act through the shared gate),
//! manually, so the concrete controllers stay inspectable at the tick
//! boundaries — the same shape `tests/mission_phase1.rs` uses for
//! mission-carrying hosted runs.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pandemonium_ai::{Controller, ScriptedController};
use pandemonium_content::{ContentBundle, ContentTree, SourceMap};
use pandemonium_fx::XxHash64;
use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::{run_command_log_with_mission, Sim};
use pandemonium_sim_api::{
    AiSettings, Command, CommandKind, ControllerKind, FogPreset, MatchSetup, MissionData, PlayerId,
    PlayerSetup,
};

/// The repository's content directory.
fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("content")
}

/// The flagship shape's fixed inputs (the CLI's `tools headless --seed 7
/// --ticks 7200 --p1 ai --p2 ai` over the default content).
const SEED: u64 = 7;
/// The flagship budget.
const TICKS: u32 = 7200;
/// The digested tick boundaries (the brief's pre-flight: 300 and 7200).
const PROBE_TICKS: [u32; 2] = [300, 7200];
/// The worker-count observation points (pre-combat through the full
/// budget — the workforce curve the worker-target test reads).
const WORKER_PROBES: [u32; 7] = [300, 1200, 2400, 3600, 4800, 6000, 7200];

/// One hosted run's evidence: the command log, the final hash, each
/// controller's RNG state at the probe boundaries, the checkpoint trail,
/// and the observable curves the direction tests read.
struct Run {
    /// Every command fed to the simulation, in feed order.
    log: Vec<Command>,
    /// The canonical state hash after the budget.
    final_hash: u64,
    /// The checkpoint trail (tick 0, every periodic hash, the forced
    /// final) — the record shape `run_ai_match` produces.
    checkpoints: Vec<Checkpoint>,
    /// Per controller (slot order), the RNG `(state, inc)` at each probe
    /// tick — `None`-initialized; set when the boundary is stepped.
    rng_states: [[(u64, u64); 2]; 2],
    /// The maximum army size observed per slot (own entities of the army
    /// kinds, pre-step state at every tick).
    army_max: [u32; 2],
    /// The own-worker count per slot at each worker probe tick reached.
    workers: Vec<(u32, [u32; 2])>,
}

/// Runs the flagship shape over one bundle, optionally mission-carrying,
/// stepping the exact `AiMatchHost::advance` body (perceive fog-filtered
/// view, decide, act through the one gate; controllers invoked in
/// ascending slot order) with the controllers kept concrete so their RNG
/// state stays readable at the boundaries. The views are taken before any
/// controller thinks — identical to the host's own per-controller
/// timing, because a think never mutates the simulation (only the step
/// does).
fn hosted_run(bundle: &ContentBundle, mission: Option<&MissionData>, ticks: u32) -> Run {
    let world = bundle.world();
    let starts = &bundle.map.starts;
    assert!(
        starts.len() == 2,
        "the flagship shape needs exactly two starts"
    );
    let players = [PlayerId(starts[0].player), PlayerId(starts[1].player)];
    let setup = MatchSetup {
        seed: SEED,
        players: players
            .iter()
            .map(|player| PlayerSetup {
                player: *player,
                controller: ControllerKind::Ai,
            })
            .collect(),
    };
    let mut controllers: Vec<(PlayerId, ScriptedController)> = players
        .iter()
        .map(|player| {
            (
                *player,
                ScriptedController::new(pandemonium_engine::alpha_plan(
                    bundle, &world, *player, SEED, mission,
                )),
            )
        })
        .collect();
    // The host's own discipline: ascending slot order every tick.
    controllers.sort_by_key(|(player, _)| *player);

    // The kind ids the observables count (capability-shaped derivation,
    // the exact ids the plans carry).
    let probe_plan = pandemonium_engine::alpha_plan(bundle, &world, PlayerId(0), SEED, None);
    let worker_kind = probe_plan.worker;
    let army_kinds = probe_plan.army;

    let mut sim = match mission {
        Some(mission) => Sim::with_mission(&world, setup, mission.clone()),
        None => Sim::new(&world, setup),
    };
    let mut log: Vec<Command> = Vec::new();
    let mut checkpoints = vec![Checkpoint {
        tick: 0,
        hash: sim.state_hash(),
    }];
    let mut rng_states = [[(0u64, 0u64); 2]; 2];
    let mut army_max = [0u32; 2];
    let mut workers: Vec<(u32, [u32; 2])> = Vec::new();
    while sim.tick() < ticks {
        let tick = sim.tick();
        let views: Vec<_> = players
            .iter()
            .map(|player| sim.player_view(*player))
            .collect();
        // The observables: own army size (pre-step) and the worker curve.
        for (slot, view) in views.iter().enumerate() {
            let army = view
                .entities
                .iter()
                .filter(|entity| entity.owner == players[slot] && army_kinds.contains(&entity.kind))
                .count() as u32;
            army_max[slot] = army_max[slot].max(army);
        }
        if let Some(index) = WORKER_PROBES.iter().position(|probe| *probe == tick) {
            let _ = index;
            let counts = views
                .iter()
                .enumerate()
                .map(|(slot, view)| {
                    view.entities
                        .iter()
                        .filter(|entity| {
                            entity.owner == players[slot] && entity.kind == worker_kind
                        })
                        .count() as u32
                })
                .collect::<Vec<_>>()
                .try_into()
                .expect("two slots");
            workers.push((tick, counts));
        }
        let mut feed: Vec<Command> = Vec::new();
        for (slot, (player, controller)) in controllers.iter_mut().enumerate() {
            let mut out = Vec::new();
            controller.think(&views[slot], tick, &mut out);
            debug_assert!(
                out.iter()
                    .all(|command| command.tick == tick && command.issuer == *player),
                "controllers emit for their own slot at the view's tick"
            );
            feed.extend(out);
        }
        log.extend(feed.iter().cloned());
        let out = sim.step(&feed);
        if let Some(hash) = out.hash {
            checkpoints.push(Checkpoint {
                tick: sim.tick(),
                hash,
            });
        }
        if let Some(probe) = PROBE_TICKS.iter().position(|tick| *tick == sim.tick()) {
            rng_states[probe] = [controllers[0].1.rng_state(), controllers[1].1.rng_state()];
        }
    }
    // The workforce at the budget's end (post-final-step state).
    let end_counts = players
        .iter()
        .map(|player| {
            sim.player_view(*player)
                .entities
                .iter()
                .filter(|entity| entity.owner == *player && entity.kind == worker_kind)
                .count() as u32
        })
        .collect::<Vec<_>>()
        .try_into()
        .expect("two slots");
    workers.push((sim.tick(), end_counts));
    // The forced final checkpoint (the recorder's law: the last checkpoint
    // names the final hash, unambiguously).
    let final_hash = sim.state_hash();
    if checkpoints.last().map(|cp| cp.tick) != Some(sim.tick()) {
        checkpoints.push(Checkpoint {
            tick: sim.tick(),
            hash: final_hash,
        });
    }
    Run {
        final_hash,
        log,
        checkpoints,
        rng_states,
        army_max,
        workers,
    }
}

/// The mission-less default bundle (the flagship CLI's own world).
fn plain_bundle() -> ContentBundle {
    ContentBundle::load_dir(&repo_content()).expect("repo content loads")
}

/// The baseline digests, recorded at the pristine base (`6eaa82b`, master
/// after B-020) through this probe before any B-021 feature change:
///
/// - final hash `0x6e9a18bd7c5f699f` — the registered flagship golden (the
///   run's own cross-check: the loop here is `AiMatchHost::advance`'s exact
///   body, so it must reproduce the CLI value);
/// - command stream — 612 commands, digest `0x32ebb1e153a446e7`;
/// - controller RNG `(state, inc)` — the `inc` half is the PCG32 default
///   stream constant (`Rng::DEFAULT_STREAM` shifted odd) on every slot, the
///   sanity check that these are the real generators; the `state` halves
///   are the consumed streams at the probe boundaries.
const BASELINE_COMMAND_DIGEST: u64 = 0x32eb_b1e1_53a4_46e7;
const BASELINE_RNG_300_SLOT0: (u64, u64) = (5_309_441_344_704_258_470, 4_354_685_564_936_845_355);
const BASELINE_RNG_300_SLOT1: (u64, u64) = (7_103_407_414_744_833_111, 4_354_685_564_936_845_355);
const BASELINE_RNG_7200_SLOT0: (u64, u64) = (2_037_330_920_411_874_478, 4_354_685_564_936_845_355);
const BASELINE_RNG_7200_SLOT1: (u64, u64) = (2_060_032_329_015_198_335, 4_354_685_564_936_845_355);

#[test]
fn the_missionless_flagship_baseline_is_pinned() {
    let run = hosted_run(&plain_bundle(), None, TICKS);
    // The hosted loop must reproduce the registered flagship golden (the
    // run's own cross-check: this loop is AiMatchHost::advance's exact
    // body, so it must match the CLI value byte for byte).
    assert_eq!(
        run.final_hash, 0x6e9a_18bd_7c5f_699f,
        "the hosted loop must reproduce the registered flagship golden"
    );
    // The command count the CLI reports for the same run.
    assert_eq!(
        run.log.len(),
        612,
        "the command count the CLI reports for the flagship run"
    );
    // The pinned baseline digests (observed at the pristine base through
    // this probe; a moved value is a stop condition, never a re-pin).
    assert_eq!(
        command_stream_digest(&run.log),
        BASELINE_COMMAND_DIGEST,
        "the full command stream is byte-identical to the recorded baseline"
    );
    assert_eq!(
        run.rng_states[0][0], BASELINE_RNG_300_SLOT0,
        "slot 0 RNG @300"
    );
    assert_eq!(
        run.rng_states[0][1], BASELINE_RNG_300_SLOT1,
        "slot 1 RNG @300"
    );
    assert_eq!(
        run.rng_states[1][0], BASELINE_RNG_7200_SLOT0,
        "slot 0 RNG @7200"
    );
    assert_eq!(
        run.rng_states[1][1], BASELINE_RNG_7200_SLOT1,
        "slot 1 RNG @7200"
    );
    // The wave-timing and workforce observables (reported for the record;
    // the override tests derive their expectations from these by
    // comparison, not by hardcoding).
    println!(
        "baseline AttackMove ticks per slot: {:?}",
        attack_moves(&run.log)
    );
    println!("baseline army max per slot: {:?}", run.army_max);
    println!("baseline worker curve per slot: {:?}", run.workers);
}

/// The command-stream digest: the replay-wire encoding of the whole feed
/// order (`ReplayFile::encode` over the commands; every other field a
/// fixed constant) hashed through the canonical `fx::XxHash64` — the same
/// bytes a recorded replay of this run would carry in its command
/// section, so an identical digest is an identical stream.
fn command_stream_digest(log: &[Command]) -> u64 {
    let file = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: 0,
        map_id: 0,
        mission_id: None,
        seed: SEED,
        player_setup: Vec::new(),
        commands: log.to_vec(),
        checkpoints: Vec::new(),
        final_hash: 0,
    };
    let mut h = XxHash64::new();
    h.write_bytes(&file.encode());
    h.finish()
}

/// The `AttackMove` (wave march) tick sequences per slot, in issue order —
/// the wave-timing observable the override tests compare against the
/// baseline.
fn attack_moves(log: &[Command]) -> Vec<(PlayerId, Vec<u32>)> {
    let mut slots: Vec<(PlayerId, Vec<u32>)> = Vec::new();
    for command in log {
        if matches!(command.kind, CommandKind::AttackMove { .. }) {
            match slots
                .iter_mut()
                .find(|(player, _)| *player == command.issuer)
            {
                Some((_, ticks)) => ticks.push(command.tick),
                None => slots.push((command.issuer, vec![command.tick])),
            }
        }
    }
    slots
}

/// The first `AttackMove` tick per slot (the first march), or `None` when
/// the slot never marched within the budget.
fn first_march(run: &Run, slot: usize) -> Option<u32> {
    attack_moves(&run.log)
        .into_iter()
        .find(|(player, _)| player.0 == slot as u8)
        .and_then(|(_, ticks)| ticks.first().copied())
}

/// An in-memory mission over the repository's real content references
/// with an authorable `ai_settings` block — the tuning probe shape. The
/// objective list is empty (the all-optional law: the mission never ends
/// through its objectives, exactly like the elimination-only default),
/// so the mission rides the sim without touching stage 10's outcome —
/// the identity tests isolate exactly the tuning.
fn tuning_mission_text(id: &str, ai_settings: &str) -> String {
    format!(
        r#"
(
    schema_version: 1,
    id: "{id}",
    display_name: "Tuning Probe",
    map: "crossroads_64",
    faction: "legion",
    rules: "alpha",
    ai_settings: ( {ai_settings} ),
    objectives: [],
)
"#
    )
}

fn tuning_tree(id: &str, ai_settings: &str) -> ContentTree {
    ContentTree::load(
        &SourceMap::from_dir(&repo_content())
            .expect("the repo content reads")
            .mission(&format!("{id}.ron"), &tuning_mission_text(id, ai_settings)),
    )
    .expect("the tuning probe loads")
}

/// The all-`None` baseline mission — the shared comparison run for every
/// direction test (the identity rule: an explicit no-op block is the
/// same mission as no block at all, id included).
fn all_none_tree() -> ContentTree {
    tuning_tree(
        "tuning_none",
        "wave_timer_ticks: None, wave_size: None, min_wave: None, \
         army_cap: None, worker_target: None, first_attack_delay: None",
    )
}

/// The shared all-`None` baseline run (7200 ticks, the flagship shape
/// over the mission-aware bundle) — computed once, compared by every
/// direction test.
fn all_none_baseline() -> &'static Run {
    static BASELINE: OnceLock<Run> = OnceLock::new();
    BASELINE.get_or_init(|| {
        let tree = all_none_tree();
        let mission = tree
            .mission_data("tuning_none", PlayerId(0))
            .expect("the all-None probe resolves");
        assert_eq!(
            mission.ai_settings,
            AiSettings::default(),
            "the explicit no-op block resolves the all-None settings"
        );
        let bundle = tree
            .bundle_for_mission("tuning_none")
            .expect("the mission-aware bundle builds");
        hosted_run(&bundle, Some(&mission), TICKS)
    })
}

/// The tuned mission's hosted run: the tree with the given block, its
/// resolved mission data, the mission-aware bundle, and the run.
fn tuned_run(id: &str, ai_settings: &str, ticks: u32) -> Run {
    let tree = tuning_tree(id, ai_settings);
    let mission = tree
        .mission_data(id, PlayerId(0))
        .expect("the tuning probe resolves");
    let bundle = tree
        .bundle_for_mission(id)
        .expect("the mission-aware bundle builds");
    hosted_run(&bundle, Some(&mission), ticks)
}

#[test]
fn an_all_none_ai_settings_block_is_the_byte_exact_default() {
    // The None identity, the brief's first acceptance: every override
    // None — here through an explicit all-None block on a mission riding
    // the sim — reproduces the mission-less baseline exactly: the
    // flagship golden final hash, the command-stream digest, and each
    // controller RNG's full state at the probe boundaries. The RNG
    // consumption order is pinned by the same equality (the streams are
    // identical, so the draws fell identically).
    let run = all_none_baseline();
    assert_eq!(
        run.final_hash, 0x6e9a_18bd_7c5f_699f,
        "the all-None mission reproduces the flagship golden"
    );
    assert_eq!(run.log.len(), 612, "the command count is unchanged");
    assert_eq!(
        command_stream_digest(&run.log),
        BASELINE_COMMAND_DIGEST,
        "the command stream is byte-identical to the mission-less baseline"
    );
    assert_eq!(
        run.rng_states[0][0], BASELINE_RNG_300_SLOT0,
        "slot 0 RNG @300"
    );
    assert_eq!(
        run.rng_states[0][1], BASELINE_RNG_300_SLOT1,
        "slot 1 RNG @300"
    );
    assert_eq!(
        run.rng_states[1][0], BASELINE_RNG_7200_SLOT0,
        "slot 0 RNG @7200"
    );
    assert_eq!(
        run.rng_states[1][1], BASELINE_RNG_7200_SLOT1,
        "slot 1 RNG @7200"
    );
}

#[test]
fn wave_timer_ticks_below_the_default_re_presses_earlier() {
    // The pressure cadence (the re-issue interval while a wave is out,
    // and the regroup re-arm): halved against the script's own const. The
    // first march is army-gated in this content (the army reaches
    // MIN_WAVE only after the initial timer has already passed), so the
    // first march stays where it was — and the first march sequence
    // divergence moves EARLIER: every re-arm fires at half the interval.
    let baseline = all_none_baseline();
    let run = tuned_run("tuning_cadence", "wave_timer_ticks: Some(1200)", TICKS);
    let baseline_marches = attack_moves(&baseline.log);
    let tuned_marches = attack_moves(&run.log);
    for (slot, (base, tuned)) in baseline_marches
        .iter()
        .zip(tuned_marches.iter())
        .enumerate()
    {
        assert_eq!(
            tuned.1.first(),
            base.1.first(),
            "slot {slot}: the first march is army-gated, not cadence-gated"
        );
        let first_divergence = base.1.iter().zip(tuned.1.iter()).find(|(a, b)| a != b);
        let Some((at_base, at_tuned)) = first_divergence else {
            panic!("slot {slot}: the halved cadence must change the march schedule");
        };
        assert!(
            at_tuned < at_base,
            "slot {slot}: the first divergent march issues earlier (baseline {at_base}, tuned \
             {at_tuned})"
        );
    }
    // And the tuning is real play, not silence: the tuned run still
    // marches, still gathers, still trains (the command count is
    // substantial and the run is not the baseline's).
    assert!(!run.log.is_empty());
    assert_ne!(
        command_stream_digest(&run.log),
        BASELINE_COMMAND_DIGEST,
        "a carried tuning is a different match"
    );
}

#[test]
fn first_attack_delay_above_the_default_delays_the_first_march() {
    // The initial pressure timer, doubled against the script's own const
    // (today the first delay and the cadence share WAVE_TIMER_TICKS; the
    // split exists so a mission can stagger them). The first march —
    // which fired when the army crossed MIN_WAVE under an already-passed
    // timer — now waits for the later deadline (or a full WAVE_SIZE army,
    // whichever comes first, itself later than the MIN_WAVE crossing).
    let baseline = all_none_baseline();
    let run = tuned_run(
        "tuning_first_delay",
        "first_attack_delay: Some(4800)",
        TICKS,
    );
    for slot in 0..2 {
        let base_march = first_march(baseline, slot)
            .unwrap_or_else(|| panic!("slot {slot}: the baseline marches"));
        let tuned_march = first_march(&run, slot)
            .unwrap_or_else(|| panic!("slot {slot}: the delayed march still fires"));
        assert!(
            tuned_march > base_march,
            "slot {slot}: the doubled first-attack delay delays the first march \
             (baseline {base_march}, tuned {tuned_march})"
        );
    }
}

#[test]
fn min_wave_above_the_default_waits_for_a_bigger_army() {
    // The minimum marching army, doubled against the script's own const:
    // the first march waits for the larger crossing instead.
    let baseline = all_none_baseline();
    let run = tuned_run("tuning_min_wave", "min_wave: Some(8)", TICKS);
    for slot in 0..2 {
        let base_march = first_march(baseline, slot)
            .unwrap_or_else(|| panic!("slot {slot}: the baseline marches"));
        let tuned_march = first_march(&run, slot)
            .unwrap_or_else(|| panic!("slot {slot}: the bigger minimum still marches"));
        assert!(
            tuned_march > base_march,
            "slot {slot}: the doubled minimum wave delays the first march \
             (baseline {base_march}, tuned {tuned_march})"
        );
    }
}

#[test]
fn worker_target_above_the_default_grows_the_workforce_further() {
    // The worker-count target, raised by four against the script's own
    // const: at a post-cap observation tick the tuned side fields more
    // workers than the all-None run — both sides compared at the same
    // budget, the same shape, the same seed.
    let ticks = 4800;
    let baseline = tuned_run(
        "tuning_worker_none",
        "wave_timer_ticks: None, wave_size: None, min_wave: None, \
         army_cap: None, worker_target: None, first_attack_delay: None",
        ticks,
    );
    let run = tuned_run("tuning_worker", "worker_target: Some(14)", ticks);
    let at = |run: &Run, tick: u32| -> [u32; 2] {
        run.workers
            .iter()
            .find(|(probe, _)| *probe == tick)
            .map(|(_, counts)| *counts)
            .unwrap_or_else(|| panic!("the worker curve covers tick {tick}"))
    };
    for slot in 0..2 {
        let base = at(&baseline, ticks)[slot];
        let tuned = at(&run, ticks)[slot];
        assert!(
            tuned > base,
            "slot {slot}: the raised worker target grows the workforce further \
             at tick {ticks} (baseline {base}, tuned {tuned})"
        );
    }
    // The curves for the record — the baseline reaches its target exactly
    // (10 on both slots by 2400) and then drifts a few above it (the
    // acting-blind pending-expiry overshoot, existing baseline behavior),
    // while the tuned side keeps training to its own higher target.
    println!("worker curve, all-None: {:?}", baseline.workers);
    println!("worker curve, target 14: {:?}", run.workers);
}

#[test]
fn army_cap_below_the_default_holds_the_army_at_the_cap() {
    // The army-size cap, dropped against the script's own consts: the
    // effective-value law pins wave_size <= army_cap, and the default
    // wave_size (10) already sits above this content's observed army
    // (max 8 and 6 on the two slots), so a measurable cap needs the paired
    // wave size — both collapsed to MIN_WAVE: a swarm-of-minimum-waves
    // opponent. The training gate pauses the army at the cap, so the
    // maximum observed army never exceeds it — while the all-None
    // baseline's army grows past it on both slots.
    let baseline = all_none_baseline();
    let run = tuned_run(
        "tuning_army_cap",
        "wave_size: Some(4), army_cap: Some(4)",
        TICKS,
    );
    let cap = pandemonium_ai::scripted::MIN_WAVE;
    for slot in 0..2 {
        assert!(
            run.army_max[slot] <= cap,
            "slot {slot}: the army never exceeds the cap (max {})",
            run.army_max[slot]
        );
        assert!(
            baseline.army_max[slot] > cap,
            "slot {slot}: the baseline's army grows past the tuned cap (max {})",
            baseline.army_max[slot]
        );
    }
}

#[test]
#[should_panic(expected = "resolves min_wave (20) above wave_size (10)")]
fn a_solo_min_wave_above_the_default_wave_size_is_refused_at_plan_construction() {
    // The cross-default shape the loader cannot see: content sits below
    // the ai crate in the dependency law, so a solo min_wave above the
    // DEFAULT wave_size (here 2 * WAVE_SIZE, derived) passes the loader's
    // pairwise checks and is caught where the overrides and the consts
    // finally meet — the plan builder, before any controller runs.
    let bundle = plain_bundle();
    let world = bundle.world();
    let mission = MissionData {
        player: PlayerId(0),
        objectives: Vec::new(),
        fog_preset: FogPreset::Default,
        ai_settings: AiSettings {
            min_wave: Some(2 * pandemonium_ai::scripted::WAVE_SIZE),
            ..Default::default()
        },
    };
    let _ = pandemonium_engine::alpha_plan(&bundle, &world, PlayerId(0), SEED, Some(&mission));
}

#[test]
#[should_panic(expected = "resolves wave_size (32) above army_cap (16)")]
fn a_solo_wave_size_above_the_default_army_cap_is_refused_at_plan_construction() {
    // The other cross-default shape: a solo wave_size above the DEFAULT
    // army cap (2 * ARMY_CAP, derived).
    let bundle = plain_bundle();
    let world = bundle.world();
    let mission = MissionData {
        player: PlayerId(0),
        objectives: Vec::new(),
        fog_preset: FogPreset::Default,
        ai_settings: AiSettings {
            wave_size: Some(2 * pandemonium_ai::scripted::ARMY_CAP),
            ..Default::default()
        },
    };
    let _ = pandemonium_engine::alpha_plan(&bundle, &world, PlayerId(0), SEED, Some(&mission));
}

#[test]
#[should_panic(expected = "must resolve to positive effective values")]
fn a_zero_tuning_value_is_refused_at_plan_construction() {
    // The loader refuses zero values at the content gate; a hand-built
    // mission data (tests, future seams) meets the same law at the plan
    // builder — every effective value must be positive.
    let bundle = plain_bundle();
    let world = bundle.world();
    let mission = MissionData {
        player: PlayerId(0),
        objectives: Vec::new(),
        fog_preset: FogPreset::Default,
        ai_settings: AiSettings {
            wave_timer_ticks: Some(0),
            ..Default::default()
        },
    };
    let _ = pandemonium_engine::alpha_plan(&bundle, &world, PlayerId(0), SEED, Some(&mission));
}

#[test]
fn a_tuned_mission_replay_roundtrips() {
    // The replay acceptance: a recording of a tuned mission carries the
    // mission's id (the tuning is identity-bearing — a different id from
    // the no-block mission, pinned in the content crate's identity
    // tests), and the record re-simulates to itself through the
    // mission-carrying runner: every checkpoint and the final hash. The
    // tuned match's commands are the log's bytes; the re-simulation
    // replays them without any controller (FD-1).
    let ticks = 1200;
    let tree = tuning_tree(
        "tuning_replay",
        "wave_timer_ticks: Some(600), worker_target: Some(12)",
    );
    let mission_id = tree.mission_id("tuning_replay");
    let plain_id = all_none_tree().mission_id("tuning_none");
    assert_ne!(
        mission_id, plain_id,
        "a carried tuning is a different mission id"
    );
    let mission = tree
        .mission_data("tuning_replay", PlayerId(0))
        .expect("the replay probe resolves");
    let bundle = tree
        .bundle_for_mission("tuning_replay")
        .expect("the mission-aware bundle builds");
    let world = bundle.world();
    let run = hosted_run(&bundle, Some(&mission), ticks);
    assert!(!run.log.is_empty(), "the tuned match played");

    let setup = MatchSetup {
        seed: SEED,
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
    };
    // The record: the mission's id (AUD-002), the command log, the
    // checkpoint trail, the final hash — the shape `run_ai_match`
    // produces for a mission-less match, plus the mission id.
    let replay = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        mission_id: Some(mission_id),
        seed: SEED,
        player_setup: setup.players.clone(),
        commands: run.log.clone(),
        checkpoints: run.checkpoints.clone(),
        final_hash: run.final_hash,
    };
    assert!(replay.validate().is_ok(), "the record is well-formed");
    let resim = run_command_log_with_mission(&world, &setup, &run.log, ticks, Some(mission));
    assert_eq!(
        resim.final_hash, run.final_hash,
        "the re-simulation reproduces the tuned match's final hash"
    );
    // And the whole checkpoint trail agrees — the re-simulation walks the
    // same hashes at the same ticks.
    let trail: Vec<(u32, u64)> = resim.checkpoints.clone();
    let recorded: Vec<(u32, u64)> = run
        .checkpoints
        .iter()
        .map(|cp| (cp.tick, cp.hash))
        .collect();
    assert_eq!(trail, recorded, "every checkpoint agrees");

    // And the record verifies as a file: encode + decode round-trips the
    // mission id (AUD-002's codec path).
    let decoded = ReplayFile::decode(&replay.encode()).expect("the record decodes");
    assert_eq!(decoded.mission_id, Some(mission_id));
    assert_eq!(decoded.commands, run.log);
    assert_eq!(decoded.final_hash, run.final_hash);

    // The CLI leg's artifacts: a content directory whose mission set
    // carries the tuned probe (the real content plus the probe's file)
    // and the record beside it, both under the test's scratch directory.
    // `tools replay-verify <file> --content <dir>` then verifies the
    // mission-bearing record end to end — the third comparison firing,
    // the mission-aware world resolution (B-019's path), the checkpoint
    // trail. (Written unconditionally: cheap, and the report's CLI leg
    // depends on them existing after the suite runs.)
    let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("b021_verify");
    let content_dir = scratch.join("content");
    for dir in ["rules", "entities", "factions", "maps", "missions"] {
        std::fs::create_dir_all(content_dir.join(dir)).expect("the scratch content dirs build");
        for entry in std::fs::read_dir(repo_content().join(dir))
            .expect("the repo category reads")
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "ron") {
                let name = path.file_name().expect("a file name");
                std::fs::copy(&path, content_dir.join(dir).join(name))
                    .expect("the content file copies");
            }
        }
    }
    std::fs::write(
        content_dir.join("missions").join("tuning_replay.ron"),
        tuning_mission_text(
            "tuning_replay",
            "wave_timer_ticks: Some(600), worker_target: Some(12)",
        ),
    )
    .expect("the probe mission file writes");
    std::fs::write(scratch.join("tuning_replay.pdrp"), replay.encode())
        .expect("the replay file writes");
}
