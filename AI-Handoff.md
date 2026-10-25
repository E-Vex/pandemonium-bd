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
| Presentation | **3D perspective over the 2D logical ground plane** — the sim stays 2D fixed-point; 3D is presentation-only ([ADR-0001](docs/adr/0001-3d-presentation.md)); the authoritative-vs-presentation content boundary is accepted ([ADR-0200](docs/adr/0200-authoritative-vs-presentation-data.md)); the mission/campaign objective model is accepted ([ADR-0201](docs/adr/0201-missions-campaigns-cutscenes.md) — the owner's verdict, 2026-10-16) |
| Status | **Mission Phase 1 COMPLETE and ACCEPTED; the independent audit's backlog REMEDIATED (B-014).** ADR-0201's vertical slice shipped through B-010…B-013 (the stage-10 objective model, the mission content class with its domain-separated id, `crossroads_01` on `crossroads_64`, the client's mission picker/objective HUD/start cutscene/end screen) and the owner accepted the ADR on 2026-10-16. B-014 closed the audit's six findings: the ADR verdict recorded (AUD-001), mission replay identity wired end-to-end — the client stamps the id, `replay-verify` re-simulates mission-carrying (AUD-002) —, the soak's stuck signal gated (AUD-004), the golden-pin claims made truthful + the content identity pinned executably in CI and the test suite (AUD-005), the `crossroads_01` optional objective grown to 8 workers with a real-content regression test (AUD-003), and this map refreshed (AUD-006). Next: the A14 human playtest → Alpha declaration; mission Phase 2 (restrictions + AI settings) per ADR-0201 §9 |
| Goldens (never move) | demo `0xb6fff6659cfb7709` (seed 7, 300 ticks) · flagship `0x6e9a18bd7c5f699f` (seed 7, 7200 ticks AI-vs-AI; tick-0 `0x71a924ad5799e4b3`) · content `0x9bc18c521107b262` (map id `0xd38136401ab02ff1`) — where each is pinned, and the only way any of them moves, is the block just below |
| Tests | 667 dev / 662 release (5 should-panic invariant tests are debug-only) |
| Registers | ASSUMPTIONS last = A-207 (B-014's three interpretation records). Key open debt: DEBT-008 (the human visual pass — the A14 sessions close it), DEBT-016, DEBT-017, DEBT-018 (replay decode pre-allocation, out of the declared threat model), DEBT-200 (cargo-audit unmaintained advisories, doc-dispositioned in `.cargo/audit.toml`), DEBT-202 (plan §15's 10× scale row — narrowed by B-006 to the semantics-bound A* remainder, still open), DEBT-203 (the mission HUD's naive pluralization, B-014) |
| Spirit | The Alpha is judged by system properties (plan §13), not content volume. Do not add what no acceptance test requires |

**Golden baselines — where each value is pinned, and the only way one moves (one place; AUD-005).**

| Value | Pinned executably by |
|---|---|
| demo `0xb6fff6659cfb7709` | `ci.yml`'s cross-OS gate (all three OSes, release) + the release pipeline's packaged smoke — **no test pins it** |
| flagship `0x6e9a18bd7c5f699f` | those two + three tests: `tests/ai.rs`, `tests/match_rules.rs`, and the tools crate's `ai_match.rs` |
| content `0x9bc18c521107b262` + map id `0xd38136401ab02ff1` | `ci.yml`'s `content-identity` job (`content-validate --expect-content-hash … --expect-map-id …`), `tests/content_pipeline.rs`'s pin test, and the release smoke |

The authorized-move procedure (STANDING_ORDERS §7, restated as the one place to look): a golden moves **only** by the owner's explicit authorization in a brief. The move lands in **one dedicated commit** that (a) carries the reason in its message, (b) updates **every** pinned location above in that same commit — the workflow values, the test pins, and the registered row here in §2 — and (c) re-derives the old and new values live in the brief's report (command + output). Anything else is a STOP-and-report failure: a value that drifts, a pin edited to match an unexplained change, or any env/flag that skips a gate — the `--expect-*` flags exist to *pin*, and an authorized move edits the pinned values, never bypasses them.

The mission chapter in one paragraph: **ADR-0201** (missions, campaigns, cutscenes) was proposed in B-010, implemented as the Phase 1 vertical slice through B-012/B-013, and **accepted by the owner on 2026-10-16** — objectives are stage-10 derived state (`match_rules.rs`'s evaluator, never hashed), missions are a versioned content class *outside* the content hash carrying their own domain-separated id (`ContentTree::mission_id`), the client carries the mission picker/objective HUD/start cutscene/end screen (A-011), and replays record and verify the mission identity (B-014's AUD-002: the client's recorders stamp the id; `replay-verify` resolves the named mission's own map bundle, refuses a mission record against content that does not carry its id **before any tick is spent**, and re-simulates mission-carrying). B-014 also closed the rest of the independent audit's backlog — the soak's stuck signal now gates its exit (`--allow-unresolved` is the explicit evidence-run escape), the golden-pin claims in `ci.yml`/`ALPHA_DECLARATION.md` are truthful, and `crossroads_01`'s optional objective is "Grow to 8 Workers" (AUD-003: the count must sit above the starting force under the owner's kept "N exist" semantics). The A14 human playtest remains the declaration's one open gate.

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
cargo run -p pandemonium-tools -- soak --matches 4 --ticks 900                # dev: the A12 invariant sweep, crash-isolated per match
cargo run --release -p pandemonium-tools -- soak --matches 8 --ticks 4000     # release: the panics/stuck/resolution tier
cargo run -p pandemonium-client                    # window on a desktop; headless smoke without a display
cargo run -p pandemonium-client -- --frames 900    # windowed smoke: auto-exit + evidence summary
cargo run -p pandemonium-client -- --seed 42 --record run.pdrp && \
cargo run -p pandemonium-tools -- replay-verify run.pdrp                           # A2
```

Expected: everything succeeds; 667 tests pass in dev (662 release); all three goldens bit-identical.
`--p1 ai --p2 ai --ticks 27000` shows a full match resolving naturally. The headless smoke drives the
windowed path's exact hosting seam (AI opponent included) and, since M10.2 Phase 4, constructs the real audio sink — the exit evidence says what audio did (the arm: active or null fallback; the cues' fed/voiced/dropped fates) alongside the feedback-wiring line. The same run that is silent under Xvfb/CI is audible on real hardware.
The windowed path is verified on Xvfb + llvmpipe (recipe in §9 / DEBT-008); the *human* visual pass is the
owner's. CI runs fmt + clippy + tests on Linux/Windows/macOS in dev and release, plus replay round-trips.

## 5. Workspace map (details: `docs/ARCHITECTURE.md`)

| Crate | What it is |
|---|---|
| `fx` | Q16.16 fixed-point, isqrt, PCG32 Rng, xxHash64 canonical hasher (FNV-1a only for the replay file checksum) |
| `sim_api` | Boundary vocabulary: ids, Command/CommandKind, Event, Reject, Snapshot, PlayerView, MatchSetup, and the mission vocabulary (MissionObjectives, MissionObjectiveKind/Parameters, ObjectiveState — ADR-0201) |
| `sim` | The spine: 11-stage `step` (FD-2: the only mutator), entity + capability stores, command gate, nav/A*, movement, economy, production/construction, combat, three-state fog, match rules (stage 10) **with the mission objective evaluator (ADR-0201: derived, unhashed state)**, A12 invariant checker, `run_command_log` + `run_command_log_with_mission` (the one re-simulation driver, mission-aware since B-014) |
| `content` | Strict RON schema, versioned loaders, validators, `ContentBundle` + hash, the `world()` seam, **the mission class: `ContentTree::mission_id` (domain-separated), `mission_objectives()` (the KindId resolution seam — ADR-0201)** |
| `ai` | `Controller` trait + `ScriptedController` (the Alpha opponent). Depends only on fx + sim_api |
| `replay` | Canonical LE byte codec + checksummed replay record/validate; **the v2 header carries the optional mission id (ADR-0201 §7)** |
| `engine` | FixedTimestep, Interpolator, `MatchHost` (+ pause/step, AI controllers, log, outcome, **`with_mission` — the engine mission seam, ADR-0201 Phase 1**), `RtsCamera`, terrain mesh, Renderer trait, `ai_host`, **`audio.rs`** (`AudioSink` + `on_cue`, `cue_for`, the nine-cue enum with ALL/index/name, `NullAudioSink` — also the client-cue oracle) |
| `client` | winit + wgpu 3D renderer, fontdue text, HUD/overlay, `feedback.rs`, `silhouette.rs`, `screens.rs`, `ui.rs`, `config.rs`, `report.rs` (F8 bug report + `--record`, **mission-id-stamped since B-014**), `sound.rs` (two-arm audio sink), **`cutscene.rs` (the mission-start scene) + the mission picker/HUD/end-screen surfaces (A-011)**; the presentation split (A-006, ADR-0100 part c, DEBT-013 repaid) lives in `render/{pipeline,terrain,entities,overlay,minimap}`, `app/`, `input/{mod,event,orders}` with `main.rs` the winit bootstrap — see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)'s "Post-alpha refactor map" |
| `tools` | `headless`, `replay-verify` (**mission-aware since B-014: the third identity comparison + mission-carrying re-simulation**), `content-validate` (**`--expect-content-hash`/`--expect-map-id` identity pins, AUD-005**), `soak` (A7: per-match panic-isolated crash telemetry — a panic is counted with its seed instead of aborting the run; a *dev-profile* soak is the only tier that observes A12 violations; **the exit gates on crashed AND unresolved — AUD-004**), `bench` (clap CLI) |
| `tests/` | `pandemonium-tests` package: architecture_law (A13), determinism, content_pipeline (**incl. the content-identity pin**), movement, economy, combat, vision, ai, match_rules, alpha_loop, **mission_phase1 (the real-content mission suite)**. New acceptance tests need an explicit `[[test]]` entry in `tests/Cargo.toml` |

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
| M10.2 | Playtest-1 findings | ✅ | Phases 1-3 (controls, visual legibility, menus/settings) delivered and re-tested (A-101, A-107, A-119). Phase 4 (audio, DEBT-011 repaid): rodio 0.22.2 behind the `AudioSink` seam (ADR-0002, A-100), nine synthesized cues + three client-side through `on_cue`, per-cue 80 ms limiter, silent null fallback, live volume + `muted` eighth key (A-125); 546 dev tests, zero golden movement; Xvfb machine-verified (the machine cannot hear — it proved the wiring and the counting; the owner re-tested sound on real hardware). **Phase 5 (closeout)**: the owner's re-test came back clean (A-126, 2026-10-09); the full gate re-ran green in dev and release with the goldens bit-identical; the Xvfb+XTEST machine pass re-ran the M10.2 paths (29 checks); the A14 playtest is scheduled (T1–T5 in `docs/PLAYTEST.md` §7) — the milestone is complete |
| Mission Phase 1 (B-012/B-013) | ADR-0201's vertical slice | ✅ | The objective model (stage 10, derived unhashed state), the mission content class with its domain-separated id, `crossroads_01` on `crossroads_64`, the client's mission picker/objective HUD/start cutscene/end screen (A-011); B-013 corrected the Destroy objective to the surgical command-center target; **accepted by the owner 2026-10-16** — ADR-0201's Decision records the verdict |
| M11 (A-013, phases 0-5) | Zero Hour controls parity | ✅ | The keyboard freed (WASD/QE camera duties gone; tools → F10/F11), selection parity (E/Q, deselect, ten groups, Alt+digit view, Ctrl+arrow cycling, radar-event Space, H), unit modes (G guard, X scatter, Alt+click waypoints — the queued path walked in order by test), camera parity (numpad, F1-F8 bookmarks, minimap drag, bounds regression), interface parity (F9, Tab, confirm-gated Resign through the existing CommandKind); the cards rewritten literally true (Ctrl+click force-fire marked "not yet available" — phase 6 is A-015's own brief); `tests/xvfb/a013_zh_controls.sh` (9 checks) + a010 re-run green; goldens bit-identical — `docs/pm/reports/A-013.md` |
| B-014 | Independent-audit remediation | ✅ | The Sonnet audit's six findings (AUD-001…006): the ADR-0201 verdict recorded; mission replay identity wired end-to-end (record + verify + mission-carrying re-simulation, three regression tests); the soak's stuck signal gated with the explicit `--allow-unresolved` evidence escape; the golden-pin claims made truthful + the content identity pinned executably (a CI job + a test); `crossroads_01`'s optional objective grown to 8 workers with the real-content regression test; this living map refreshed — `docs/pm/reports/B-014.md` |

## 7. Inventory — what is worth knowing (the rest is in `docs/ARCHITECTURE.md`)

- **fx:** `Fx` is Q16.16 over `i32`; mul/div round toward zero, overflow **saturates** (never panics or wraps, same in debug and release); `from_milli` is the §10.2 authoring conversion; total ordering on raw value.
- **sim:** `Sim::new(world, setup)`, `step(&[Command])`. Canonical state hash is entity-major little-endian (encoding v4). Fog (A-059) and match outcome (A-066) are **derived, not hashed**. Command gate sorts by (issuer, seq); refusal changes zero state.
- **content:** `Sim::new(&bundle.world(), setup)` is the loaded-content path. Spawn order (and entity ids): map starts in authored order → each start's forces in faction order → ore nodes in map order.
- **replay:** the log records exactly the fed command stream, rejections included (A-019); `run_command_log` is the one re-simulation driver (`run_command_log_with_mission` is its mission-carrying form — identical checkpoints, the mission rides for semantics).
- **missions (ADR-0201):** objectives are stage-10 derived state — never hashed, optional ones never gating; a mission file is outside the content hash but carries its own domain-separated id (an edited objective set IS a different mission); replays stamp the id and `replay-verify` refuses a mission record against content that does not carry it before any tick is spent; the mission player is the first map start's player (the Phase 1 law, client and tools alike); BuildTrain counts current ownership including starting units — author the count ABOVE the faction's starting force or the objective is vacuous (AUD-003's lesson, regression-tested on the real content).
- **engine/audio:** `AudioSink::on_events(&mut self, &[Event])`, pure `cue_for(&Event) -> Option<AudioCue>` (6 cues: attack landed, unit lost, unit ready, structure done, delivery, match ended), `on_cue(&mut self, AudioCue)` for the three client-side cues (CommandAck, SelectionClick, UiClick), `NullAudioSink` counts both kinds. Fed after each step from the presentation layer; the sim never sees it (FD-9).
- **client (M10.2):** `screens.rs` (pure application state machine; `App.host` is `Option<MatchHost>`, None exactly while at the menu), `config.rs` (eight keys: incl. `master_volume` 0.0..=1.0 and `muted`), `input.rs` (right-button state machine, edge scroll, Escape ladder), `silhouette.rs`, `ui.rs` (menu pass over the fontdue overlay, keyboard + mouse), `sound.rs` (Phase 4: two-arm sink — rodio arm or null fallback, the synthesized nine-cue bank, the per-cue rate limiter, live volume/mute).
- **client key map (M11/PLAN-M11):** the in-match dispatch reads ONE pure table — `input/actions.rs::world_key_action(KeyCode, KeyMods, pressed)` — shared with its tests. The Zero Hour scheme: arrows/edge/right-drag scroll; middle-drag + Numpad 4/6 rotate, Numpad 8/2 zoom, Numpad 5 resets; F1-F8 bookmarks (Ctrl sets); E/Q select (double-tap = whole visible map); 1-0 groups (Ctrl assign, double-tap center, Alt view); Ctrl+arrows cycle; Space = last radar event; H = Command Center; A/S/G/X the modes; Alt+click waypoints (`submit_order_flagged`'s queue flag); F9 the command bar, Tab the objectives; F10 overlay, F11 bug report (F3/F8 are bookmark slots now — the tools never return to that row); F12 not bound (DEBT-100); W and Ctrl+H reserved (air/hero — no content). Left-click on empty ground DESELECTS (the A-091 reversal) and shift+click is add-only.

## 8. What is NOT built yet (in order)

1. **A14 — the human playtest** (the declaration's one open gate, now scheduled: five first-time slots T1–T5 in `docs/PLAYTEST.md` §7, three waves to 2026-10-31): `docs/PLAYTEST.md` is the instrument; ≥5 testers run the loop unaided; a failed tester is a finding, never a wave-through. The same sessions close DEBT-008 and arm the DEBT-012 trigger (DEBT-011 is fully repaid — the owner's re-test passed clean, A-126).
2. **Declare the Alpha (or file the findings):** flip A14 in `docs/ALPHA_DECLARATION.md`, README badge, the licence is decided — MIT (A-005, `LICENSE` at the repo root).
3. **Mission Phase 2 — restrictions and AI settings** (ADR-0201 §9, size M): faction/rules variant references, the Build-gate membership check (a real gate change — the mission seam it lands through is now wired and mission-aware), fog presets, the remaining objective kinds (survive, reach, protect, gather), the AI plan tuning overrides with byte-exact defaults. The mission replay verification is structurally ready: a Phase 2 mission replay that re-simulates mission-less would diverge, and `replay-verify` now never does that (AUD-002).
4. **Known limitations to carry forward:** formation-less jams (A-040), staircase paths, stalled construction sites when the builder dies (A-045; the scripted AI shares the hole), O(N·V) per-tick fog recompute, scripted-AI sight-verification latency (A-062), and the AI's scriptedness itself.

**Release pipeline.** The release channel is `.github/workflows/release.yml` (operational counterpart: `docs/product/RELEASING.md`): a `v*` tag push builds the client + tools in release on the three pinned runners (ubuntu-22.04, windows-2022, macos-15), packages one archive per OS (Linux `tar.gz`, Windows `zip`, macOS `tar.gz`) with `LICENSE`, `content/`, `README.md`, `QUICKSTART.md`, `THIRD_PARTY_NOTICES.md` and the two binaries, smoke-tests each archive on its own OS (the Linux archive on 22.04 and 24.04), then publishes a GitHub **pre-release** with the three archives and a consolidated `checksums.txt`. Tag `v0.1.0` exists on `4477d83` (the master head) and the pre-release was published on 2026-10-12; A14 (the human playtest) remains OPEN — the pipeline ships the build, the playtest gates the declaration.

## 9. Sharp edges and gotchas

**Sim, tools, and the A7 instrument**
- **The soak's crash telemetry is real since the post-M10.2 instrument pass**: every match runs inside `catch_unwind`, a panic becomes `crashed: N` with the seed and panic message in the report (the `panic:` prefix marks it), the sweep continues, and `main` exits non-zero at the end. Isolation is observability, never recovery (A-127) — never widen it into "keep going with suspect state".
- **Tier semantics matter**: the A12 checker is `debug_assert`-only, so a *release* soak observes panics/stuck/resolution/throughput, and only a *dev* soak observes invariant violations (each violation is one isolated, counted crash). The nightly's release tiers are the 1000-match sweep; `soak-dev` (16 rotating matches) is the invariant tier; the aggregate job runs on tier failure (`!cancelled()`).
- `cargo test --workspace` runs the tests in dev — the A12 checker is live in every dev-profile match the suites drive.

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

**Missions, replays and the goldens (ADR-0201, B-014)**
- Objectives are derived, unhashed state: a mission match and its mission-less twin share every checkpoint (`mission_less_path_is_byte_equivalent` pins it) — the mission id in the replay header is the only structural difference.
- A mission replay with the id stripped still verifies as a plain match today (the Phase 1 masking, pinned explicitly in the tools verify tests) — Phase 2 restrictions are designed to break that masking; the wiring is ready for it.
- The content hash and map id never cover mission files (the encoders never read them) — a mission edit moves only the mission's own id; author BuildTrain counts above the starting forces (AUD-003).
- A golden's pin locations are inventoried in §2's baseline table; a moved value without the owner's authorization is a STOP — never re-pin to match an unexplained change.

**AI and controllers**
- `(issuer, seq)` is unique only within one tick; cross-tick accounting must key on `(tick, issuer, seq)`.
- Build verification is count-based, not proximity-based. `MoveState::Idle` means *no orders*, not "standing still". Construction sites spawn without `Spawned` events (use `ConstructionStarted`'s builder).
- A wave that merely exists does not press — liveness needs a timer (re-march cadence). The wave-mass numbers (WAVE_SIZE 10, ARMY_CAP 16) are load-bearing: re-run the 32-seed resolution sweep after any content timing/HP change.
- Restart is drop + reconstruct (A15), not an in-place reset.

**Client and presentation**
- **Filter winit key repeats** (`event.repeat`) before any hotkey logic.
- **The key map lives in ONE pure table** (`input/actions.rs`, M11): new
  bindings go there, never inline in the winit match — the dispatch and the
  tests read the same function. Held-scroll keys must ALWAYS clear their
  held-set entry on release whatever the modifier state (a stuck "up"
  drifts the camera forever); a Ctrl+arrow cycle press never sets it.
- **The pause menu has six rows** (M11: Resign is row 4; Quit to Menu is
  row 5) — Xvfb navigation scripts and focus arithmetic count from there.
- **The client window defaults to 800x600 at (0,0)** — XTEST pointer
  targets beyond that land on the root, not the window (the a013 lesson).
- **WASD/Q/E are not camera keys** (M11): the M9.1 direction tests for the
  arrow scroll and the middle-drag/numpad rotate are the surviving pins.
- **Frame-counted presentation state** (flashes, pings) uses the always-advancing presented-frame clock, never the `--frames` counter, and never the tick counter.
- **The right button is a state machine** (`input.rs::RightButton`): the press only records, the release routes. Any new right-press special case must consume the whole gesture or let the release run. The armed minimap attack-move reads `right_press_armed` (captured at press). Empty-ground clicks no longer deselect (A-091) — Escape is *the* deselect.
- **The entity shader's yaw is mirrored against `atan2`:** forward-offset silhouette parts rotate with the transposed form (`world_x = c*ox - s*oz`, `world_z = s*ox + c*oz`). New offset parts need yaw-0 AND yaw-90 tests.
- **The decal instance buffer is one shared, order-dependent stream** (shadows+ghost, then team rings, then selection rings). Add a group → decide its blend order explicitly and keep the range arithmetic exact.
- **Minimap markers paint over fog** — feed them the fog-filtered PlayerView entities, never a full snapshot.
- `Silhouette` is not `Copy`; the kind→shape mapping resolves once at startup (`main.rs::kind_shapes`) — a content kind-id rename silently changes shapes (DEBT-016).
- **Menu-first app (Phase 3):** every host access is guarded on `Option<MatchHost>`; a menu-only session never panics, records, or submits a command. A menu owns the input (world input dead, held-key set cleared). The state machine starts every entered screen on focus row 0. Esc's exhausted rung opens the pause menu (Esc at the main menu quits; on the end screen does nothing).
- **Config is hand-rolled and forgiving** (A-108/A-109): per-key salvage, unknown keys ignored, range clamps, std-env config dir; no dir → in-memory defaults, never a panic. Done saves, Esc leaves without saving (A-115).
- **Phase 4 audio: the rodio stream owner lives in the sink — dropping it silences everything.** `Sound`'s device arm owns `MixerDeviceSink` (rodio 0.22's `OutputStream` successor), boxed in the arm enum (clippy's large-variant lint), constructed ONCE per run (the probe is startup-only, never per match); `reset_match_state` calls `Sound::reset()` (counters + limiter windows), never a fresh `Sound`. Its `log_on_drop(false)` is load-bearing — rodio's default drop notice prints to stderr.
- **Client-side cues arrive through `on_cue`, NOT `cue_for`; the limiter sits between the wiring and the arm.** CommandAck fires in `submit_order` after `host.submit` (submitted, not non-rejected — A-120; behind the spectate gate), SelectionClick on a membership change (one `set_selection` gate; the match reset bypasses it), UiClick in one `menu_activate` core (Enter, mouse click, and the end-screen R shortcut all share it — A-121). `NullAudioSink::client_cues` counts POST-limiter — rapid-fire wiring tests use one call per fresh app, or the pre-limiter `Sound::client_cues` counter.
- **The machine cannot hear — audio evidence lines claim counting, never sound.** No device under Xvfb/CI: `device_plausible()`'s `/dev/snd` pre-check short-circuits the probe before libasound's chatty config parser wakes (A-124), the null arm counts, and the evidence says the arm first ("active (rodio, 44100 Hz)" / "null fallback (no device)" [+ "muted"]) then the cues' fed/voiced/dropped fates. "Sounds good" is only the owner's re-test claim. The limiter is 80 ms PER CUE (an AttackLanded storm does not silence UnitLost); per-voice work after the limiter is one Vec clone + one channel send.
- **A zero-entity frame needs the camera bind group re-bound** before the entity draw (wgpu validates even zero-instance draws).
- The embedded font is an ASCII subset (`text.rs`) — on-screen labels are ASCII only. fontdue metrics are y-up (`bearing_y`); the glyph-atlas buffer must be built with `resize`, not `truncate`.

**Environment (Xvfb + XTEST recipe — DEBT-008 has the long form)**
- Start `Xvfb :99 -screen 0 1280x800x24` directly (`xvfb-run` is broken here); the screen must be at least as big as the window.
- **No window manager** (twm breaks the GL surface): set focus with `xdotool windowfocus` or python-xlib `set_input_focus`; inject input via XTEST (python-xlib `xtest_fake_input` when libXtst/xdotool are missing).
- `WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=1`, a writable `XDG_RUNTIME_DIR`, and a userland EGL stack without root: `apt-get download libegl1 libegl-mesa0 libgles2 libxtst6 libxkbcommon-x11-0 libxcb-xkb1` + `dpkg -x`, `LD_LIBRARY_PATH` at the extracted dir, **unversioned** `libxkbcommon*.so` symlinks, and a glvnd json via `__EGL_VENDOR_LIBRARY_DIRS` pointing at `libEGL_mesa.so.0`.
- Key cadence ≥0.25–0.4 s under llvmpipe load; pass mode-specific CLI args BEFORE `Popen`; verify the seed in the evidence lines. Graceful close = WM_DELETE_WINDOW ClientMessage (32-bit data).
- **Menu-row mouse targeting (the Phase 5 harness lesson):** the bright **30 px band is the FOCUSED row's plate** — the panel border is bright too but only 1–2 px, and the title strip sits between the border and row 0 (a click there is a by-design no-op: "a click off every button: nothing"). Locate the plate from a screenshot column scan, not the border; row pitch is 38 px (30 + 8).
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