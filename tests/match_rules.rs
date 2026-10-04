//! M8 exit suite (plan §14 M8, §13 A15): **does the full loop close?**
//!
//! - **Match rules** (plan §9.7, stage 10) — `MatchEnded` fires exactly once,
//!   when a player is defeated (zero owned Footprint structures) or resigns.
//!   The winner is the sole survivor; mutual destruction yields `NEUTRAL`.
//!   The outcome is derived state (A-066: not part of the canonical hash —
//!   a pure function of the entity set and the players' `resigned` flags,
//!   both of which ARE hashed), so the M7 goldens stay green by construction.
//! - **A15 (restart cleanliness)** — two fresh `MatchHost` instances from the
//!   same seed produce identical state hashes at every checkpoint (plan §9.7:
//!   "new Sim from the same setup with a new seed, with no leaked state").
//!   With AI controllers, the deterministic controller RNG (seeded from the
//!   match seed) makes the command logs identical too — a same-seed restart
//!   reproduces the same match bit-for-bit.
//! - **The host surfaces the outcome** — `MatchHost::outcome()` /
//!   `MatchHost::is_finished()` let the client stop driving the match loop
//!   and show the end screen (the windowed client's M8 UX).
//!
//! The M7 golden pin (0x679f4713114765c9 at seed 7 / 7200 ticks) lives in
//! `tests/ai.rs` and asserts that stage 10 is hash-neutral — the AI-vs-AI
//! flagship runs unchanged. This suite focuses on the M8-specific behaviors.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pandemonium_ai::Controller;
use pandemonium_content::ContentBundle;
use pandemonium_engine::{alpha_controller, AiMatchHost, MatchHost};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, Event, MatchSetup, PlayerId, PlayerSetup, Tick,
};

/// The flagship seed (matches tests/ai.rs).
const SEED: u64 = 7;

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

/// A controller that resigns on tick 0 — the simplest deterministic behavior
/// to force a `MatchEnded` event through the hosting loop.
struct ResignOnTickZero {
    player: PlayerId,
}

impl Controller for ResignOnTickZero {
    fn think(
        &mut self,
        _view: &pandemonium_sim_api::PlayerView,
        tick: Tick,
        out: &mut Vec<Command>,
    ) {
        if tick == 0 {
            out.push(Command::new(self.player, tick, 1, CommandKind::Resign {}));
        }
    }
}

/// Two-player Human+Ai setup (the windowed client's shape).
fn human_vs_ai_setup(seed: u64) -> MatchSetup {
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

// ---------------------------------------------------------------------------
// Match rules: MatchEnded fires on resignation and surfaces through the host
// ---------------------------------------------------------------------------

#[test]
fn resignation_ends_the_match_with_the_other_player_winning() {
    // Player 1 (the AI slot) is driven by a controller that resigns on tick 0.
    // Stage 10 sees player 1 resigned → defeated → the match ends with player
    // 0 the winner. The real content (Crossroads) is structure-bearing, so
    // the A-067 guard doesn't suppress the defeat check.
    let world = bundle().world();
    let setup = human_vs_ai_setup(SEED);
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
        PlayerId(1),
        Box::new(ResignOnTickZero {
            player: PlayerId(1),
        }),
    )];
    let mut host = MatchHost::with_controllers(&world, setup, controllers);
    assert!(!host.is_finished());
    // One step: the controller resigns, stage 10 fires MatchEnded.
    host.step_once();
    let outcome = host
        .outcome()
        .expect("resignation ends the match with an outcome");
    assert_eq!(outcome.winner, PlayerId(0));
    assert!(host.is_finished());
}

#[test]
fn match_ended_fires_exactly_once_even_after_more_ticks() {
    let world = bundle().world();
    let setup = human_vs_ai_setup(SEED);
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
        PlayerId(1),
        Box::new(ResignOnTickZero {
            player: PlayerId(1),
        }),
    )];
    let mut host = MatchHost::with_controllers(&world, setup, controllers);
    host.step_once();
    let first = host.outcome().expect("ended");
    // Run more ticks — the outcome is cached, no second MatchEnded.
    for _ in 0..5 {
        host.step_once();
    }
    assert_eq!(host.outcome(), Some(first));
    // The MatchEnded event appears exactly once in the log of all events.
    // (The host doesn't expose a per-tick event log directly; the outcome
    // cache IS the idempotency proof. The sim-level test pins the event
    // emission count.)
}

#[test]
fn the_human_resigning_ends_the_match_with_the_ai_winning() {
    // The mirror: player 0 (the human slot) resigns via a submitted command
    // (not a controller — the human's commands go through submit()). Stage 10
    // sees player 0 resigned → defeated → player 1 wins.
    let world = bundle().world();
    let setup = human_vs_ai_setup(SEED);
    // Player 1 driven by the Alpha AI (so it's a real opponent); player 0
    // resigns via submit().
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
        PlayerId(1),
        Box::new(alpha_controller(bundle(), &world, PlayerId(1), SEED)),
    )];
    let mut host = MatchHost::with_controllers(&world, setup, controllers);
    // The human resigns at tick 0.
    host.submit(Command::new(PlayerId(0), 0, 1, CommandKind::Resign {}));
    host.step_once();
    let outcome = host.outcome().expect("the human resigned");
    assert_eq!(outcome.winner, PlayerId(1));
}

// ---------------------------------------------------------------------------
// A15: restart cleanliness — two fresh hosts, same seed, identical hashes
// ---------------------------------------------------------------------------

#[test]
fn a15_two_fresh_hosts_produce_identical_state_hashes() {
    // Plan §9.7, A15: "two consecutive matches from fresh sims with the same
    // seed have identical hashes". Two MatchHost instances (no controllers)
    // from the same setup, advanced by the same frame deltas, produce
    // identical state hashes at every step.
    let world = bundle().world();
    let setup = human_vs_ai_setup(SEED);
    let mut a = MatchHost::new(&world, setup.clone());
    let mut b = MatchHost::new(&world, setup);
    let frame = std::time::Duration::from_millis(16);
    for _ in 0..62 {
        let _ = a.advance(frame);
        let _ = b.advance(frame);
        assert_eq!(a.tick(), b.tick());
        assert_eq!(a.state_hash(), b.state_hash(), "hashes diverge at tick");
    }
    assert_eq!(a.state_hash(), b.state_hash());
}

#[test]
fn a15_two_fresh_hosts_with_ai_produce_identical_logs_and_hashes() {
    // The stricter A15: with AI controllers, the deterministic controller RNG
    // (seeded from the match seed) makes the command logs identical too. A
    // same-seed restart reproduces the same match bit-for-bit — the windowed
    // client's "Press R to restart" UX relies on this.
    let world = bundle().world();
    let setup = human_vs_ai_setup(SEED);
    let make_host = || {
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
            PlayerId(1),
            Box::new(alpha_controller(bundle(), &world, PlayerId(1), SEED)),
        )];
        MatchHost::with_controllers(&world, setup.clone(), controllers)
    };
    let mut a = make_host();
    let mut b = make_host();
    let frame = std::time::Duration::from_millis(16);
    for _ in 0..62 {
        let _ = a.advance(frame);
        let _ = b.advance(frame);
        assert_eq!(a.tick(), b.tick());
        assert_eq!(a.state_hash(), b.state_hash(), "hashes diverge");
        assert_eq!(a.log().len(), b.log().len(), "log lengths diverge");
    }
    // The full command logs are identical (the controller RNG is seeded from
    // the match seed, so the same seed produces the same commands).
    assert_eq!(a.log(), b.log(), "the command logs diverge");
    assert_eq!(a.state_hash(), b.state_hash());
}

// ---------------------------------------------------------------------------
// Stage 10 is hash-neutral: the M7 golden holds with match rules active
// ---------------------------------------------------------------------------

#[test]
fn stage_10_does_not_change_the_m7_golden_checkpoint_trail() {
    // The M7 flagship golden (tests/ai.rs pins the final hash at seed 7 /
    // 7200 ticks) was pinned when stage 10 was a no-op. Stage 10 (M8) is
    // hash-neutral (A-066: the outcome is derived, not hashed), so the same
    // match run with stage 10 active produces the same checkpoint trail and
    // final hash. This was the M8 invariant: match rules landed without
    // breaking a single M7 golden — proven then, against the M7 value.
    //
    // M9 re-pin (written reason): the trail moved — not because stage 10
    // started hashing anything (it still derives its result from the entity
    // set and the resigned flags, both already hashed; A-066's reasoning is
    // untouched) but because M9's first work item fixed a simulation bug the
    // M8 flagship exposed: a chase order whose commanded target died was
    // never popped, so the AI's defense orders outlived their dead intruders
    // and its armies froze mid-chase forever (movement.rs documents the pop
    // as the combat pipeline's job; the code never did it). Orders are
    // canonical hashed state, so the fix moves every post-combat checkpoint.
    // The test stays as the pinned-trail regression guard it has been since
    // M8; the hash-neutrality property itself is a static argument (derived
    // state, not in hash_state — see A-066), not something a value can prove.
    let world = bundle().world();
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
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = [PlayerId(0), PlayerId(1)]
        .into_iter()
        .map(|player| {
            (
                player,
                Box::new(alpha_controller(bundle(), &world, player, SEED)) as Box<dyn Controller>,
            )
        })
        .collect();
    let mut host = AiMatchHost::new(&world, setup, controllers);
    while host.tick() < 7200 {
        host.advance();
    }
    assert_eq!(host.state_hash(), 0x01b3_b60b_741f_03e9);
}

// ---------------------------------------------------------------------------
// The host's outcome gate: the client's match loop knows when to stop
// ---------------------------------------------------------------------------

#[test]
fn a_resolved_match_surfaces_match_ended_in_the_event_stream() {
    // The MatchEnded event flows through the host's FrameOutcome events —
    // the client's event consumer sees it (for the end-screen trigger, the
    // audio cue, etc.). The outcome cache is the idempotency guarantee; the
    // event is the outward notification (FD-9).
    let world = bundle().world();
    let setup = human_vs_ai_setup(SEED);
    let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
        PlayerId(1),
        Box::new(ResignOnTickZero {
            player: PlayerId(1),
        }),
    )];
    let mut host = MatchHost::with_controllers(&world, setup, controllers);
    let out = host.step_once();
    assert!(
        out.events.iter().any(|event| matches!(
            event,
            Event::MatchEnded {
                winner: PlayerId(0)
            }
        )),
        "MatchEnded fires with player 0 the winner: {:?}",
        out.events
    );
}
