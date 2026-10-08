# Pandemonium — AI Handoff Document (lean)

> **Purpose.** Orient any agent (or human) resuming work: what the project is, how to
> work on it, what exists, what is next. A living status map — it never replaces the
> authoritative plan. **Read this top to bottom, then `plan.md` §0 (Operating Contract)
> and §14 (Milestones) before writing code.** Per-milestone detail lives in
> `docs/ARCHITECTURE.md` and the crate sources, not here.

---

## 1. Read this first (source-of-truth order)

1. `plan.md` — the authoritative spec (v2.0): §0 operating contract, §2 frozen decisions
   FD-1..FD-10 (never change silently), §4 workspace + dependency law, §5 determinism rules,
   §13 acceptance criteria A1–A15, §14 milestones.
2. This file — current status and orientation.
3. `docs/ASSUMPTIONS.md` — every guessed interpretation (A-001…, each citing its plan section).
4. `docs/DEBT.md` — every deliberate simplification, with a repayment trigger.
5. `docs/ARCHITECTURE.md` — the working map of the code as built.
6. `docs/adr/` — the process for proposing changes to frozen decisions.
7. `docs/PLAN-M10.2.md` — the spec for the current milestone.

## 2. Project snapshot

| Field | Value |
|---|---|
| What | Pandemonium — an RTS **foundation** that grows into a game (architecture first, content is data) |
| Stack | Rust 1.98.1 (pinned by `rust-toolchain.toml`), edition 2021, fully custom engine on winit/wgpu — no game engine, no ECS framework |
| Repo | `github.com/E-Vex/pandemonium-bd`, branch `master`. `git status` must be clean before work begins |
| Presentation | **3D perspective over the 2D logical ground plane** — the sim stays 2D fixed-point; 3D is presentation-only ([ADR-0001](docs/adr/0001-3d-presentation.md)) |
| Status | **M10.2 (playtest-1 findings): Phases 1-3 complete. Phase 4 (audio, rodio per A-100) is the active task** — the owner gave the go on 2026-10-08. Then: owner re-test → M10.2 closeout → A14 human playtest (≥5 testers) → Alpha declaration |
| Goldens (never move) | demo `0xb6fff6659cfb7709` (seed 7, 300 ticks) · flagship `0x6e9a18bd7c5f699f` (seed 7, 7200 ticks AI-vs-AI; tick-0 `0x71a924ad5799e4b3`) · content `0x9bc18c521107b262` (map id `0xd38136401ab02ff1`) |
| Tests | 523 dev / 518 release (5 should-panic invariant tests are debug-only) |
| Registers | ASSUMPTIONS last = A-118. Key open debt: DEBT-008 (human visual pass), DEBT-011 (no audible backend — Phase 4 repays), DEBT-013 (client monoliths), DEBT-015 (M10.2 deferral — Phase 4 closes it), DEBT-016, DEBT-017 |
| Spirit | The Alpha is judged by system properties (plan §13), not content volume. Do not add what no acceptance test requires |

M10.2 in one paragraph: **Phase 1** controls (Generals ZH-style right-button state machine, middle-drag rotate, edge scroll, Escape ladder) re-tested pass-with-notes (A-101). **Phase 2** visual legibility (per-kind silhouettes, blue/orange teams, health bars, tooltips, minimap markers) re-tested pass (A-107). **Phase 3** menus/settings — pure state machine `client/src/screens.rs`, host built on Start (game opens at the menu), `client/src/config.rs` seven-key settings file (no serde), pause menu, end screen with Rematch/Main Menu — Xvfb+XTEST machine-verified; `master_volume` is stored and shown but not audible yet. Registers A-107..A-118.

## 3. Non-negotiable working rules (digest of plan §0)

- Work milestone by milestone; the game must build and run at the end of every one.
- Every task ends with the green gate (§4); commit only when green; one logical change per commit.
- Frozen decisions (plan §2) are frozen — to change one, write an ADR with evidence.
- Log every guess in `docs/ASSUMPTIONS.md`, every simplification in `docs/DEBT.md`. Prefer a small reversible guess + a log entry over stopping to ask.
- Prove, don't assert: every claim needs an automated test in the repo.

## 4. How to verify the current state

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                       # dev
cargo test --workspace --release             # determinism must hold in release too
cargo run -p pandemonium-tools -- headless --seed 7 --ticks 300                    # ends 0xb6fff6659cfb7709
cargo run -p pandemonium-tools -- headless --seed 7 --ticks 7200 --p1 ai --p2 ai   # ends 0x6e9a18bd7c5f699f
cargo run -p pandemonium-tools -- content-validate content                         # PASS, content hash above
cargo run -p pandemonium-client                    # window on a desktop; headless smoke without a display
cargo run -p pandemonium-client -- --frames 900    # windowed smoke: auto-exit + evidence summary
cargo run -p pandemonium-client -- --seed 42 --record run.pdrp && \
cargo run -p pandemonium-tools -- replay-verify run.pdrp                           # A2
```

Expected: everything succeeds; 523 tests pass in dev (518 release); all three goldens bit-identical.
`--p1 ai --p2 ai --ticks 27000` shows a full match resolving naturally. The headless smoke drives the
windowed path's exact hosting seam (AI opponent included) and prints the feedback-wiring evidence line.
The windowed path is verified on Xvfb + llvmpipe (recipe in §9 / DEBT-008); the *human* visual pass is the
owner's. CI runs fmt + clippy + tests on Linux/Windows/macOS in dev and release, plus replay round-trips.

## 5. Workspace map (details: `docs/ARCHITECTURE.md`)

| Crate | What it is |
|---|---|
| `fx` | Q16.16 fixed-point, isqrt, PCG32 Rng, xxHash64 canonical hasher (FNV-1a only for the replay file checksum) |
| `sim_api` | Boundary vocabulary: ids, Command/CommandKind, Event, Reject, Snapshot, PlayerView, MatchSetup |
| `sim` | The spine: 11-stage `step` (FD-2: the only mutator), entity + capability stores, command gate, nav/A*, movement, economy, production/construction, combat, three-state fog, match rules (stage 10), A12 invariant checker, `run_command_log` (the one re-simulation driver) |
| `content` | Strict RON schema, versioned loaders, validators, `ContentBundle` + hash, the `world()` seam |
| `ai` | `Controller` trait + `ScriptedController` (the Alpha opponent). Depends only on fx + sim_api |
| `replay` | Canonical LE byte codec + checksummed replay record/validate |
| `engine` | FixedTimestep, Interpolator, `MatchHost` (+ pause/step, AI controllers, log, outcome), `RtsCamera`, terrain mesh, Renderer trait, `ai_host`, **`audio.rs`** (`AudioSink`, `cue_for`, `NullAudioSink`) |
| `client` | winit + wgpu 3D renderer, fontdue text, HUD/overlay, input (`input.rs`, `orders.rs`), `feedback.rs`, `ui.rs`, `report.rs` (F8 bug report), `silhouette.rs`, `screens.rs`, `config.rs`. `main.rs`/`render.rs` are monoliths (DEBT-013) |
| `tools` | `headless`, `replay-verify`, `content-validate`, `soak`, `bench` (clap CLI) |
| `tests/` | `pandemonium-tests` package: architecture_law (A13), determinism, content_pipeline, movement, economy, combat, vision, ai, match_rules, alpha_loop. New acceptance tests need an explicit `[[test]]` entry in `tests/Cargo.toml` |

The dependency law is **enforced by tests**: `tests/architecture_law.rs` checks the internal edge
allow-list, the per-crate external allow-list (plan §3.2), the forbidden-crate list (bevy, ECS, rapier,
rand/getrandom, …), and scans the determinism crates (fx, sim, sim_api, ai, content) for unordered-map
types, wall-clock reads, and floating-point type names.

## 6. Development stages — status board

| Stage | Title | Status | What closed it |
|---|---|---|---|
| M0 | Skeleton & guardrails | ✅ | fx property tests; architecture-law test; CI authored |
| M1 | Simulation core | ✅ | A1/A2 on the trivial world; ID-never-reused; iteration-order tests |
| M2 | Content pipeline | ✅ | all Alpha content loads and validates with precise errors; A3 scaffold; map schema v2 (display-only heightmap, ADR-0001) |
| M3 | Engine shell | ✅ | FixedTimestep, MatchHost (step-only mutation is structural), camera/picking, wgpu client; windowed path machine-verified on Xvfb (DEBT-008) |
| M4 | Movement (P1) | ✅ | 50-unit spam-click responsiveness; no permanent stuck units; real-map detour |
| M5 | Economy/production/construction (P3) | ✅ | divergent openings produce measurably different timelines; A12 checker + soaks |
| M6 | Combat & vision (P2) | ✅ | immediate-hit pipeline; Attack/AttackMove/Stop; three-state fog; A10 fog integrity |
| M7 | AI through commands (P4) | ✅ | A5 + A11 (label-blind mirrored battery, fuzz); AI-vs-AI completes deterministically; DEBT-005 repaid |
| M8 | Match rules & full loop (P5) | ✅ | stage 10 defeat/victory/resignation, `MatchEnded` fires once; vs-AI loop with end screen/restart/control groups; A15 |
| M9 | Alpha content & feel pass | ✅ | chase-order-pop bug fixed so matches resolve; wave tuning; hit flashes/health bars/pings; `AudioSink` seam (no backend — DEBT-011) |
| M9.1 | Input hotfix | ✅ | first human playtest findings: mirrored axes, no mouse camera, no attack path, key-repeat spam, never-expiring cues — all fixed and pinned |
| M10 | Stabilization & declaration | ✅ | A1–A15 sweep in `docs/ALPHA_DECLARATION.md` (13 pass with evidence; A14 an honest open finding); soak 32/32 resolved; bench within §15 budgets; nightly 1000-match tier |
| M10.1 | Pre-declaration hardening | ✅ | DEBT-001 repaid (xxHash64 canonical, all goldens re-pinned in one commit); `--seed`/`--record`/F8 report; `docs/PLAYTEST.md` (the A14 instrument); sharded nightly soak |
| M10.2 | Playtest-1 findings | 🔄 Phases 1-3 ✅ | Controls, visual legibility, menus/settings done (523 dev tests, zero golden movement). **Phase 4 (audio) active**; Phase 5 = closeout + A14 scheduling |

## 7. Inventory — what is worth knowing (the rest is in `docs/ARCHITECTURE.md`)

- **fx:** `Fx` is Q16.16 over `i32`; mul/div round toward zero, overflow **saturates** (never panics or wraps, same in debug and release); `from_milli` is the §10.2 authoring conversion; total ordering on raw value.
- **sim:** `Sim::new(world, setup)`, `step(&[Command])`. Canonical state hash is entity-major little-endian (encoding v4). Fog (A-059) and match outcome (A-066) are **derived, not hashed**. Command gate sorts by (issuer, seq); refusal changes zero state.
- **content:** `Sim::new(&bundle.world(), setup)` is the loaded-content path. Spawn order (and entity ids): map starts in authored order → each start's forces in faction order → ore nodes in map order.
- **replay:** the log records exactly the fed command stream, rejections included (A-019); `run_command_log` is the one re-simulation driver.
- **engine/audio:** `AudioSink::on_events(&mut self, &[Event])`, pure `cue_for(&Event) -> Option<AudioCue>` (6 cues: attack landed, unit lost, unit ready, structure done, delivery, match ended), `NullAudioSink` counts cues. Fed after each step from the presentation layer; the sim never sees it (FD-9).
- **client (M10.2):** `screens.rs` (pure application state machine; `App.host` is `Option<MatchHost>`, None exactly while at the menu), `config.rs` (seven keys: incl. `master_volume` 0.0..=1.0), `input.rs` (right-button state machine, edge scroll, Escape ladder), `silhouette.rs`, `ui.rs` (menu pass over the fontdue overlay, keyboard + mouse).

## 8. What is NOT built yet (in order)

1. **M10.2 Phase 4 — audio** (active): rodio backend + ADR + allow-list amendment, nine synthesized cues, null fallback, mute + volume, rate limiter. Then the owner's re-test.
2. **M10.2 Phase 5 — closeout** and scheduling of the A14 playtest.
3. **A14 — the human playtest** (the declaration's one open gate): `docs/PLAYTEST.md` is the instrument; ≥5 testers run the loop unaided; a failed tester is a finding, never a wave-through. The same sessions close DEBT-008 and arm DEBT-011/012 triggers.
4. **Declare the Alpha (or file the findings):** flip A14 in `docs/ALPHA_DECLARATION.md`, README badge, decide the LICENSE (the owner's call).
5. **Known limitations to carry forward:** formation-less jams (A-040), staircase paths, stalled construction sites when the builder dies (A-045; the scripted AI shares the hole), O(N·V) per-tick fog recompute, scripted-AI sight-verification latency (A-062), and the AI's scriptedness itself.

## 9. Sharp edges and gotchas

**Determinism and the sim**
- **Presentation direction is invisible to the sim's test wall** (M9.1): hashes pin behavior, not feel — the camera's right axis was negated from M3 until a human pressed D. Camera/feel primitives need direction-pinned tests (yaw 0 and 90°) AND the human pass.
- **Fog and match outcome stay out of the hash** (A-059, A-066). Adding them breaks A10 and every golden.
- **The defeat check needs the "structure-bearing" guard** (A-067) or the M1 trivial-world fixture ends at tick 0.
- **Combat:** a chase order outlives its dead target unless combat stage 1 pops it (M9's freeze bug) — any new removal path must keep that pop. `clear_dead_targets` runs in stage 8 *after* removal; new death paths must go through it. `acquire_target` ties go to the lower id (scan ascending, `<=`).
- **Command logs stay chronological;** feed order within a tick matters for duplicate (issuer, seq) — replay drivers sort by tick only (stable).
- **Fx traps:** saturation on extreme mul→div; `from_milli` rounds toward zero (compare `dist <= speed`, don't assume divisibility); tile centers are `x*1000+500` milli; `radius_milli * 65_536` is *tiles* — milli-tiles need `*65_536/1_000`.
- **Banned-token scan is a substring scan, docs included** — write "unordered-map types", "wall-clock reads", "floating-point types", "immediate-hit" (not "Instant-hit").
- **Content loader sorts each category by file name** (directory order is OS-dependent); serde maps use `BTreeMap`.
- **Debug-only `#[should_panic]` tests need `#[cfg(debug_assertions)]`.**

**AI and controllers**
- `(issuer, seq)` is unique only within one tick; cross-tick accounting must key on `(tick, issuer, seq)`.
- Build verification is count-based, not proximity-based. `MoveState::Idle` means *no orders*, not "standing still". Construction sites spawn without `Spawned` events (use `ConstructionStarted`'s builder).
- A wave that merely exists does not press — liveness needs a timer (re-march cadence). The wave-mass numbers (WAVE_SIZE 10, ARMY_CAP 16) are load-bearing: re-run the 32-seed resolution sweep after any content timing/HP change.
- Restart is drop + reconstruct (A15), not an in-place reset.

**Client and presentation**
- **Filter winit key repeats** (`event.repeat`) before any hotkey logic.
- **Frame-counted presentation state** (flashes, pings) uses the always-advancing presented-frame clock, never the `--frames` counter, and never the tick counter.
- **The right button is a state machine** (`input.rs::RightButton`): the press only records, the release routes. Any new right-press special case must consume the whole gesture or let the release run. The armed minimap attack-move reads `right_press_armed` (captured at press). Empty-ground clicks no longer deselect (A-091) — Escape is *the* deselect.
- **The entity shader's yaw is mirrored against `atan2`:** forward-offset silhouette parts rotate with the transposed form (`world_x = c*ox - s*oz`, `world_z = s*ox + c*oz`). New offset parts need yaw-0 AND yaw-90 tests.
- **The decal instance buffer is one shared, order-dependent stream** (shadows+ghost, then team rings, then selection rings). Add a group → decide its blend order explicitly and keep the range arithmetic exact.
- **Minimap markers paint over fog** — feed them the fog-filtered PlayerView entities, never a full snapshot.
- `Silhouette` is not `Copy`; the kind→shape mapping resolves once at startup (`main.rs::kind_shapes`) — a content kind-id rename silently changes shapes (DEBT-016).
- **Menu-first app (Phase 3):** every host access is guarded on `Option<MatchHost>`; a menu-only session never panics, records, or submits a command. A menu owns the input (world input dead, held-key set cleared). The state machine starts every entered screen on focus row 0. Esc's exhausted rung opens the pause menu (Esc at the main menu quits; on the end screen does nothing).
- **Config is hand-rolled and forgiving** (A-108/A-109): per-key salvage, unknown keys ignored, range clamps, std-env config dir; no dir → in-memory defaults, never a panic. Done saves, Esc leaves without saving (A-115).
- **A zero-entity frame needs the camera bind group re-bound** before the entity draw (wgpu validates even zero-instance draws).
- The embedded font is an ASCII subset (`text.rs`) — on-screen labels are ASCII only. fontdue metrics are y-up (`bearing_y`); the glyph-atlas buffer must be built with `resize`, not `truncate`.

**Environment (Xvfb + XTEST recipe — DEBT-008 has the long form)**
- Start `Xvfb :99 -screen 0 1280x800x24` directly (`xvfb-run` is broken here); the screen must be at least as big as the window.
- **No window manager** (twm breaks the GL surface): set focus with `xdotool windowfocus` or python-xlib `set_input_focus`; inject input via XTEST (python-xlib `xtest_fake_input` when libXtst/xdotool are missing).
- `WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=1`, a writable `XDG_RUNTIME_DIR`, and a userland EGL stack without root: `apt-get download libegl1 libegl-mesa0 libgles2 libxtst6 libxkbcommon-x11-0 libxcb-xkb1` + `dpkg -x`, `LD_LIBRARY_PATH` at the extracted dir, **unversioned** `libxkbcommon*.so` symlinks, and a glvnd json via `__EGL_VENDOR_LIBRARY_DIRS` pointing at `libEGL_mesa.so.0`.
- Key cadence ≥0.25–0.4 s under llvmpipe load; pass mode-specific CLI args BEFORE `Popen`; verify the seed in the evidence lines. Graceful close = WM_DELETE_WINDOW ClientMessage (32-bit data).
- **Xvfb has no audio device** — windowed runs there exercise the null fallback; whether sounds are *good* is only the owner's real-machine re-test.

**Small recurring slips**
- Clippy 1.98 new-style lints: `is_multiple_of`, `is_none_or`, `*b"PD…"` — write the new forms directly. No `#![cfg_attr(clippy, deny(...))]`; use plain `#![deny(clippy::…)]`.
- proptest: bind locals before `prop_assert!` with a cast-then-`<`. Raw strings containing `"#` need `r##"…"##`.
- `CARGO_MANIFEST_DIR` depth differs per crate (tests: one `.parent()`; tools: `ancestors().nth(2)`).
- anyhow shows only the outer context — format with `{err:#}` in tests. RON strict errors read "Unexpected field named …".
- Entity ids in tests follow the documented spawn order — check `bundle.entities` positions, never hand-count.

## 10. Maintenance protocol — every agent, every milestone

1. Read plan.md §0 + §14, this file, `docs/DEBT.md`, `docs/ASSUMPTIONS.md`.
2. Run the §4 verification — green **before** you start (else stop and investigate) and **after** you finish.
3. Implement exactly one milestone (or a logged sub-slice). No skipping ahead, no unrequested features, never weaken an exit test.
4. Log as you go: assumptions → ASSUMPTIONS.md; simplifications → DEBT.md; frozen-decision challenges → an ADR.
5. Update this file at the end: snapshot (§2), counts (§4), status board (§6), inventory (§7), gotchas (§9). An out-of-date handoff is a bug.
6. Commit green, one logical change per commit, messages in the existing style.
7. Honest declaration (plan §13): a criterion that cannot be met honestly is a recorded finding, never waved through.

## 11. Pointer index (plan section → where it lives)

| Plan § | Where |
|---|---|
| §0–§2 contract, vision, frozen decisions | `plan.md`; changes via `docs/adr/` |
| §3.2 dependency policy, §4 layout, §5 determinism | `tests/architecture_law.rs`, `docs/ARCHITECTURE.md`, `clippy.toml`, fx docs |
| §6–§8 sim core, entities, commands | `crates/sim/src/` (`sim.rs`, `world.rs`, `hash.rs`, `command.rs`), `crates/sim_api` |
| §9 systems | `crates/sim/src/` (`movement`, `economy`, `production`, `combat`, `vision`, `match_rules`, `invariants`), `crates/ai`, `engine/ai_host.rs` |
| §10 content | `docs/CONTENT_GUIDE.md`, `content/`, `crates/content/src/` |
| §11 engine/client (3D per ADR-0001; §11.5 is the audio clause) | `crates/engine`, `crates/client` |
| §12 tools | `crates/tools` |
| §13 acceptance | `tests/` — A13 architecture_law; A1/A2 determinism; A3 content_pipeline; A5/A11 ai; A10 vision; A15 match_rules; M9 exit alpha_loop |
| §14 milestones | this file §6 |
| §15–§19 budgets/risks/debt | `plan.md`; debt live in `docs/DEBT.md` |