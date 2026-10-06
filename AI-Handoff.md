# Pandemonium — AI Handoff Document

> **Purpose.** Orient any agent (or human) resuming work on this project: what the
> project is, how to work on it, what is already built, and what to build next.
> This file is a living status map — it never replaces the authoritative plan.
>
> **If you are the next agent: read this file top to bottom, then read `plan.md`
> §0 (Operating Contract) and §14 (Milestones) before writing any code.**

---

## 1. Read this first (source-of-truth order)

1. `plan.md` — the authoritative specification (v2.0). Especially §0 (operating
   contract), §2 (frozen decisions FD-1..FD-10 — never change silently), §4
   (workspace + dependency law), §5 (determinism rules), §13 (acceptance criteria
   A1–A15), §14 (milestones M0–M10).
2. This file — current status and orientation.
3. `docs/ASSUMPTIONS.md` — every interpretation the implementing agent had to guess
   (numbered A-001…, each citing the plan section it interprets).
4. `docs/DEBT.md` — every deliberate simplification, with a repayment trigger.
5. `docs/ARCHITECTURE.md` — the working map of the code as built.
6. `docs/adr/` — the process for proposing changes to frozen decisions.

## 2. Project snapshot

| Field | Value |
|---|---|
| What | Pandemonium — an RTS **foundation** that grows into a game (architecture first, content is data) |
| Stack | Rust, fully custom engine (own loop, entity store, renderer on top of winit/wgpu — no game engine, no ECS framework) |
| Repo | `github.com/E-VEx/pandemonium-bd` (git, branch `master`; local clone at `/home/z/my-project/bd` — the M9.1 session's path; wherever it lives, `git status` must be clean before work begins) |
| Toolchain | Rust 1.98.1, edition 2021, pinned by `rust-toolchain.toml` |
| Presentation | **3D perspective over the 2D logical ground plane** — the simulation stays 2D fixed-point; 3D is presentation-only ([ADR-0001](docs/adr/0001-3d-presentation.md), 2026-10-01) |
| Status | **M9.1 (input hotfix) COMPLETE on top of M9 — the loop is playable, and now *controllable*.** The DEBT-008 human pass finally ran (the project owner played the windowed build) and reported: *"nothing is moving, the control is bad, D goes left and A goes right, I can't move with the mouse, I can't order my army to move or attack, nothing appears on the screen."* Every symptom was real and every root cause was client-side (the simulation itself was machine-verified through M9 and untouched): (1) the camera's screen-right axis had been **negated since M3** — D panned left, A right, W retreated, S advanced; no sim-side test can catch presentation direction, and nobody had ever played it; (2) the mouse **could not move the camera at all** — plan §11.3's "pan by edge + keys" and "zoom toward the cursor" were never implemented, and the wheel direction was inverted (orbit-only); (3) right-click only ever issued `Move` — **the client had no way to order an attack at all** (§11.3's right-click *context* command was unimplemented); (4) 'A' fired attack-move on key-down *and on every OS key repeat* while also being a pan key; (5) **rejected orders were silent** — `CommandRejected` events were dropped, so every illegal order read as "the controls don't work"; (6) the opening camera framed the map center at 58 tiles out — the starting force read as specks ("nothing appears"); (7) `frames_presented` only advanced in `--frames` verification mode, so M9's flashes and pings **never expired and accumulated forever** in normal play. M9.1 fixes all seven: corrected pan axes (with direction-pinning tests at yaw 0 and 90°), WASD + arrow-key panning at frame-rate-independent speed, edge scrolling + middle-drag grab-pan + wheel zoom-toward-cursor, right-click context resolution (enemy → Attack, node → Gather for workers, ground → Move; `client::orders`), 'A' arms attack-move for the next left-click (HUD line included; Esc/right-click cancels), key-repeat guards, refusal cues (red square at the click point + HUD reason line; the smoke run now pins the wiring with a deliberately invalid order), selection corner brackets, an opening camera focused on the player's start at 26 tiles (re-framed on restart), map-bounds clamping, and a viewport-centered end screen. Machine-verified end to end under Xvfb + llvmpipe with XTEST injection (the DEBT-008 recipe, extended — see §8): a 900-frame windowed run with drag-box selection of the 5 start entities and 2 submitted commands (a context Move and an armed attack-move), the loop healthy through pan, zoom, and middle-drag. 352 dev / 347 release tests green (19 new: 7 camera, 8 context-resolution, 4 feedback). **Zero golden movement** — the fix is 100% presentation/input (no sim/ai/content files touched): demo `0x9d5ba9b565060336`, flagship `0x01b3b60b741f03e9`, content `0x249b69f0ee343a10` all re-verified bit-identical. (M10.1: those were the FNV-era values — the canonical hasher swap repaid DEBT-001 and moved every digest; §4 carries the current pins.) M9 remains complete underneath (the frozen-army chase-order pop, the re-marching wave machine with CC focus, waves of ten — matches resolve in 9.1k–23.3k ticks across a 32-seed sweep, `tests/alpha_loop.rs`; the feel pass: hit flashes, health bars, command pings, the §11.5 audio seam). DEBT-008 is narrowed to its last inch: the human *re-verification* of the fixed build on a real display (the feel judgment). **Next: M10 is complete — the acceptance sweep is recorded in docs/ALPHA_DECLARATION.md** | defeat = zero owned Footprint structures OR resigned; victory = one survivor; `MatchEnded` fires exactly once (idempotent — `Sim::outcome` caches the result, derived state per A-066, not hashed — the same reasoning as fog A-059; the defeat check fires only in structure-bearing matches, A-067). `MatchHost` hosts the vs-AI opponent (`with_controllers`), keeps the command log, surfaces the outcome; the client's full loop (hosted AI, end screen, R restart, control groups, context orders, feedback cues) runs through it. A15 green (two fresh hosts from the same seed: identical hashes; with AI controllers: identical logs too). |
| Spirit | The Alpha is judged by system properties (plan §13), not content volume. Do not add what no acceptance test requires. |

## 3. Non-negotiable working rules (digest of plan §0)

- Work milestone by milestone. Never start M(N+1) until every exit test of M(N) is
  green. The game must build and run at the end of every milestone.
- Every task ends with the green gate (§4 below); commit only when green; one
  logical change per commit.
- Frozen decisions (plan §2) are frozen. To change one: stop, write an ADR in
  `docs/adr/` with evidence. Never silently.
- Log every guessed decision in `docs/ASSUMPTIONS.md`; log every simplification in
  `docs/DEBT.md`. Prefer a small reversible guess + a log entry over stopping to ask.
- Prove, don't assert: every claim ("deterministic", "AI parity", "add-a-unit is
  data-only") needs an automated test that exists in the repo.
- Placeholder art only. No time on graphics before M3.

## 4. How to verify the current state

Run from the repo root (the clone — `/home/z/my-project/bd` this handoff):

```bash
cargo fmt --all -- --check                 # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints, zero tolerance
cargo test --workspace                      # all tests (debug)
cargo test --workspace --release            # determinism must hold in release too
cargo run -p pandemonium-tools -- headless --seed 7 --ticks 300
cargo run -p pandemonium-tools -- headless --seed 7 --ticks 7200 --p1 ai --p2 ai
cargo run -p pandemonium-client             # windowed 3D client; headless smoke pass without a display
cargo run -p pandemonium-client -- --frames 900   # windowed smoke: auto-exit + evidence summary
```

Expected at this handoff: all commands succeed; 352 tests pass in dev
(347 in release — the 5 should-panic invariant-checker tests are
debug-only by nature — `debug_assert` compiles out in release): 39 fx
unit tests, 15 fx property tests, 16 ai unit tests (15 + M9's
re-march pin), 105 sim unit tests (103 + M9's two chase-order-pop
tests), 4 sim_api unit tests, 8 replay codec tests, 48 engine tests
(41 + M9.1's seven camera direction/pan_world/zoom_toward/focus
pins), 22 client tests (10 + M9.1's eight context-resolution and
four refusal/bracket tests), 11 tools tests, 33 content unit tests,
and the acceptance suite: 9 determinism (A1/A2), 4 content-pipeline, 3 movement, 4
economy, 7 combat, 5 vision, 2 architecture-law, 8 M7 tests
(`tests/ai.rs`: two A5 structural audits, the A5 ledger identity, the
A11 battery, the A11 proptest fuzz, completion + determinism, the
golden pin, and the log-alone replay), 7 M8 tests
(`tests/match_rules.rs`: resignation ends the match, MatchEnded fires
once, the human resigning ends with the AI winning, A15 two fresh
hosts identical hashes, A15 two fresh hosts with AI identical logs +
hashes, the pinned-trail regression guard, MatchEnded surfaces in the
event stream), and 3 M9 tests (`tests/alpha_loop.rs`: resolution
within the fifteen-minute budget across seeds, end-to-end resolution
determinism, the windowed host's vs-AI loop closing naturally).
The headless demo's final hash is 0xb6fff6659cfb7709 (seed 7, 300
ticks — re-pinned at M10.1: the canonical hasher swap, FNV-1a ->
xxHash64, DEBT-001 repaid; the FNV-era value was 0x9d5ba9b565060336,
which had been unchanged through M9 and M9.1); the AI-vs-AI flagship
(seed 7, 7200 ticks) pins 0x6e9a18bd7c5f699f with tick-0 hash
0x71a924ad5799e4b3 (both re-pinned at M10.1 by the same swap; the
FNV-era values were 0x01b3b60b741f03e9 / 0x61613bca16b8f00e); the final hash
moved twice in M9 (the chase-order pop, then the AI tuning), each
re-pinned with its written reason in the test — and M9.1 moved
nothing (all three hashes re-verified bit-identical: the hotfix
touches only the client and the engine's camera, both above the sim
boundary). The `--p1 ai --p2 ai`
runner prints a `match ended:` line; at longer budgets it now ends
naturally (player N wins) — run `--ticks 27000 --p1 ai --p2 ai` to
watch a full match resolve.
`tools content-validate content` prints the bundle identity and PASS
(content hash 0x9bc18c521107b262, map id 0xd38136401ab02ff1 — re-issued
at M10.1 by the hasher swap; the FNV-era values were
0x249b69f0ee343a10 / 0xbc0970c3cf14e9cf, which M9's manifest review
had left unchanged); `cargo run -p
pandemonium-client` prints the no-display finding and runs the headless
smoke pass — which since M9 drives the windowed path's exact hosting
seam (the AI opponent included) and prints the feedback-wiring
evidence line (events fed the audio sink, cues mapped, attacks
flashed, and since M9.1 refused orders surfaced — the smoke submits
one deliberately invalid Move and pins its rejection through the
refusal cues); on a desktop it opens the window; with `--frames N` the
windowed run exits after N frames and prints the evidence summary,
including whether the match ended and who won. The M9 full-windowed
verification: under Xvfb + llvmpipe a `--frames 40000` run hosted the
AI opponent through the real wgpu pipeline and the match resolved
naturally — "match ended, player 1 wins (tick 5250)" — recorded in
DEBT-008 with the environment recipe. The M9.1 input verification
(same recipe, extended — see §8): a 900-frame windowed run with XTEST
injection drag-box-selected the 5 start entities, submitted 2 commands
(a right-click context Move and an armed-'A' attack-move), and kept
the loop healthy through pan keys, wheel zoom, and middle-drag —
"windowed smoke: 900 frames presented, 2 commands submitted,
selection 5, match ongoing". On a headless
machine the windowed path is verified on Xvfb + llvmpipe per DEBT-008
(selection + Move commands provably work; the M8 additions — end screen,
restart, control groups, Stop/AttackMove — are compiled and clippy-clean
but the human visual pass over the full vs-AI match loop remains — DEBT-008
narrowed by M8).
CI additionally runs fmt + clippy + tests on Linux/Windows/macOS in dev and
release, plus the replay round-trip for BOTH the demo and an AI-vs-AI
match, and the binaries job starts a short `--p1 ai --p2 ai` run, when
pushed to GitHub (`.github/workflows/ci.yml`) — see A-010/A-020 above.

## 5. Workspace map (as built; see `docs/ARCHITECTURE.md` for detail)

```text
crates/fx        DONE    Q16.16 fixed-point math, isqrt, PCG32 Rng, xxHash64 canonical hasher (M10.1; FNV-1a kept for the replay file checksum)
crates/sim_api   DONE    vocabulary: ids, Command/CommandKind, Event, Reject, Snapshot, PlayerView, MatchSetup
crates/sim       DONE    Sim spine + M4 movement + M5 economy + M6 combat & vision + M7's run_command_log (the one canonical command-log re-simulation driver — DEBT-005 repaid): step pipeline (11 stages, 5/7/9 now wired), entity + capability stores (12 capability types), command gate w/ economy + combat checks, state hash (v4), trivial-world fixture, nav grid + A* + footprint occupancy, mover, gather loop, production queues, construction, combat pipeline, three-state fog (derived not hashed — A-059), A12 invariant checker, + the headless driver returning the checkpoint trail + allocator watermark
crates/content   DONE    RON schema (strict), versioned loaders + map v1->v2 migration, precise validators, ContentBundle + content hash, world() seam (all 12 capabilities mapped — Attack mapped in M6, DEBT-006 retired)
crates/ai        DONE    Controller trait (plan §9.6 verbatim) + ScriptedController, the scripted Alpha opponent: workers -> depot -> barracks -> mixed army, idle-worker gather management, attack waves on size + timer that RE-MARCH on the pressure cadence while alive and focus the sighted enemy CC (M9 — matches resolve), base defense, pending/build bookkeeping verified by sight (A-062) — never a rejection channel
crates/replay    DONE    canonical LE byte codec + checksummed replay record/validate (re-sim driver lives in sim since M7)
crates/engine    DONE*   FixedTimestep, Interpolator, MatchHost (structural FD-2) + pause/single-step, RtsCamera (glam) + picking/box-select, terrain mesh, Renderer trait + null renderer (Frame carries selection + M9's flash set), HudState on the Frame boundary, ai_host (M7): AiMatchHost — controllers on the tick boundary, ascending slot order, every fed command recorded — and the alpha plan builder (capability-shaped kind resolution, costs through the Ore seam, ring-scanned build ground clear of static claims + node doorsteps), + audio (M9, §11.5): AudioSink trait, the pure event -> cue mapping, the null counter sink (*engine core)
                   (*engine core)
crates/client    DONE*   winit window + wgpu 26 3D renderer (depth buffer, terrain mesh + heightmap, instanced placeholder boxes), selection + right-click Move, fontdue text atlas + HUD/debug overlay pass, --frames windowed smoke, headless smoke fallback (drives the AI-hosting seam since M9), + the M9 feel pass: feedback.rs (hit flashes, health bars, command pings — pure, unit-tested geometry) wired into draw() with the audio sink (*DEBT-008 keeps the human visual pass)
crates/tools     DONE    headless (the M1 demo + M7's --p1/--p2 ai|idle controller slots over the real content, per-player evidence lines, --record for both), replay-verify (world resolution by content identity), content-validate (clap CLI)
tests/           ACTIVE  pandemonium-tests: architecture_law.rs (A13) + determinism.rs (A1/A2 + spine proofs) + content_pipeline.rs (M2/A3) + movement.rs (M4/P1) + economy.rs (M5/P3) + combat.rs (M6/P2) + vision.rs (M6/A10) + ai.rs (M7: A5 + A11 + AI-vs-AI completion/determinism/golden/log-alone replay)
content/         DONE    rules/, entities/ (9 kinds), factions/ (Legion), maps/ (Crossroads 64x64, symmetric, heightmap)
docs/            ACTIVE  ARCHITECTURE, DEBT, ASSUMPTIONS, CONTENT_GUIDE, adr/ (ADR-0001 accepted)
```

The dependency law is **enforced by tests, not by convention**: `tests/architecture_law.rs`
parses every member manifest, checks the internal edge allow-list, the per-crate
external allow-list (the full plan §3.2 table), the forbidden-crate list (bevy, ECS
crates, rapier, pathfinding, rand/getrandom, …), and scans the determinism crates
(fx, sim, sim_api, ai, content) line-by-line for unordered-map types, wall-clock
reads, and floating-point type names. Breaking the architecture fails CI.

## 6. Development stages — status board

| Stage | Title (plan §14) | Status | Exit criteria |
|-------|------------------|--------|---------------|
| M0 | Skeleton & guardrails | ✅ **complete** | fx property tests green ✓; architecture-law test green ✓; CI authored, 3-OS green pending a real GitHub run (A-010) |
| M1 | Simulation core | ✅ **complete** | A1/A2 green on the trivial world (two in-process runs + pinned golden hashes; tools replay-verify round-trip PASS; CI matrix makes the 3-OS claim real once it runs, A-020); ID-never-reused test ✓; iteration-order test ✓ |
| M2 | Content pipeline | ✅ **complete** | all Alpha content + the map load (`tools content-validate` PASS; the repo tree is test-pinned); malformed files produce precise errors (every validator has a test); A3 scaffold in place (data-only add-a-unit: load + spawn + Move, sim sources scanned clean; the §10.6 built-and-fights extension is DEBT-007 for M5/M6); map schema v2 with display-only heightmap per ADR-0001, v1 migrates forward; content hash + map id canonical (FNV-1a, plan §5.10) |
| M3 | Engine shell | ✅ **complete** | engine + client implemented and green (183 tests): FixedTimestep loop w/ catch-up cap, interpolation, MatchHost (the step-only mutation guarantee is structural) + pause/single-step, RtsCamera + ground picking + box select, terrain mesh + null renderer, wgpu 26 windowed client w/ depth buffer, terrain + heightmap, instanced placeholder boxes, selection + right-click Move, fontdue text atlas + HUD/debug overlay pass, `--frames` windowed smoke. **Windowed path machine-verified on Xvfb + llvmpipe** (DEBT-008/A-037: 900 frames presented, XTEST drag-box selection of the 5 start entities, 2 Move commands, visible motion); the *human* visual pass remains open in DEBT-008. HUD/overlay slice landed (DEBT-009 repaid) |
| M4 | Movement (P1) | ✅ **complete** | 50 units respond within 2 ticks under spam-clicked orders (per-unit motion check, tests/movement.rs); no permanent stuck units (every order resolves — arrival, crowded arrival, or MoveFailed; unreachable/zero-speed fail immediately); hashes green (A1 + regenerated goldens, encoding v2); real-map detour: 4 workers cross Crossroads around the rock walls, never on blocked terrain; push-apart separates stacks and never pushes into terrain (DEBT-003 repaid) |
| M5 | Economy/production/construction (P3) | ✅ **complete** | divergent scripted openings produce measurably different timelines (worker-heavy vs early-Raider: worker counts, barracks, Raiders fielded, income, balances, hashes branch ≤ tick 60); scripted matches bit-identical run-to-run; A12 invariant checker fires every tick in debug and 3 openings × 2 seeds × 900 ticks of both-player economy soak hold every invariant; the soak caught and fixed a real spawn-position bug |
| M6 | Combat & vision (P2) | ✅ **complete** | immediate-hit attack pipeline (every stage a named function); Attack/AttackMove/Stop semantics; three-state fog (Hidden/Explored/Visible, derived not hashed — A-059); A10 fog integrity green (hashes equal fog on/off, targeting rejects unseen both directions); turret works (no Move, Attack+Footprint); A12 combat invariants (cooldowns sane, target alive, no attack on own); scripted skirmishes show composition + position matter (bit-identical run-to-run); legibility checklist machine-half green (AttackHit + Died fire, hp_fraction_milli on EntityView); DEBT-006/007/010 retired; encoding v4 |
| M7 | AI through commands (P4) | ✅ **complete** | A5 + A11 green (ai depends only on fx+sim_api, re-asserted; the sim never references the ai crate; the ledger identity — starting + deliveries − accepted data-defined costs — holds per player; every entity beyond the starting forces traces to an accepted Train/Build; the 20-pair mirrored battery over every reachable rejection class is label-blind; proptest fuzz of random mirrored commands); AI-vs-AI headless matches complete (3 seeds × 2400 ticks under the A12 checker, bit-identical re-runs, seed divergence); golden pinned (0x679f4713114765c9); AI-driven logs replay through the codec (A2); DEBT-005 repaid |
| M8 | Match rules & full loop (P5) | ✅ **complete** | Stage 10 (plan §6.3.10, §9.7) lands: defeat = zero owned Footprint structures OR resigned; victory = one survivor; `MatchEnded` fires exactly once (idempotent — `Sim::outcome` caches the result). The defeat check only fires when the match is "structure-bearing" (A-067) so the M1 spine-test fixture and the M3 headless smoke stay green. The outcome is derived state (A-066: not part of the canonical hash) — the M7 goldens stay green by construction (no `STATE_ENCODING_VERSION` bump). `MatchHost` extended with optional AI controllers (`with_controllers`), a command log (`log()`), and the match outcome (`outcome()` / `is_finished()`). The windowed client hosts its vs-AI opponent through this seam, renders the end-screen panel (VICTORY/DEFEAT/MUTUAL DESTRUCTION), supports restart (R — drop + reconstruct, A15), control groups (1-9), and Stop/AttackMove hotkeys (S/A). A15 green (two fresh hosts, same seed → identical hashes; with AI → identical logs too). The `--p1 ai --p2 ai` runner prints a `match ended:` line. DEBT-008 narrowed by M8 (the human visual pass over the full vs-AI loop remains) |
| M9 | Alpha content & feel pass | ✅ **complete** | The M8 gap closed: the frozen-chase-order sim bug fixed (defense orders outliving their dead targets froze both armies; combat stage 1 pops them now — 2 unit tests), the wave machine re-marches on the pressure cadence and focuses the sighted enemy CC (re-pinned + a new pin test), waves of ten / army cap sixteen. `tests/alpha_loop.rs`: AI-vs-AI resolves inside 27000 ticks across seeds (measured 9.1k–23.3k over a 32-seed sweep), end-to-end resolution determinism (same seed → same winner, end tick, log, final hash), and the windowed host's vs-AI loop closes naturally (an idle human's base falls to the AI, `outcome()` surfaces through the end-screen boundary). Feel pass: hit flashes, health bars, command pings (5 unit tests, no GPU needed), the `AudioSink` seam + placeholder cue set (3 unit tests; no audible backend — DEBT-011). Manifest review: no value changed — the stall was code, not content (§10.4 pins hold). Goldens: the demo untouched; the AI flagship re-pinned twice with written reasons |
| M9.1 | Input hotfix (the DEBT-008 human pass findings) | ✅ **complete** | The first human playtest of the windowed client (the project owner, on a real display) reported every input-layer defect the machine pass could not see: mirrored WASD/pan axes (the camera's screen-right vector negated since M3), W/S inverted, no mouse camera control (plan §11.3's edge pan + zoom-toward-cursor never implemented; wheel direction inverted), no attack order path (right-click context resolution unimplemented), 'A' firing attack-move on key-down *and key repeats* while doubling as a pan key, silent rejections, an illegible 58-tile opening zoom, and M9's cues never expiring (the feedback clock only advanced in `--frames` mode). All fixed, all pinned: 7 camera direction/API tests, 8 context-resolution tests, 4 refusal/bracket tests; the smoke run pins the refusal wiring with a deliberately invalid order; Xvfb + XTEST injection re-verified the windowed loop (900 frames, 5-entity drag selection, context Move + armed attack-move submitted). Zero golden movement — presentation/input only. DEBT-008 narrowed to the human re-verification of *this* build |
| M10 | Stabilization & declaration | ✅ **complete** | The A1–A15 sweep is recorded with evidence in `docs/ALPHA_DECLARATION.md`: 13 criteria pass with automated evidence (M10 verified them bit-identical at their then-FNV values; M10.1's hasher swap re-pinned them: demo 0xb6fff6659cfb7709, flagship 0x6e9a18bd7c5f699f, content 0x9bc18c521107b262); A14 is recorded as an honest finding (a human playtest needs humans). The soak evidence run: 32/32 resolved, 0 crashed, avg end tick 11877 / max 23265 at the plan's real stuck threshold (A-086). Bench baselines (release): 0.25 ms avg, 2.22 ms p99, 4060 t/s — all §15 budgets met. The nightly workflow runs the 1000-match A7 tier and the bench. The M10-prep review pass also landed the playability/visual series (command card + train/build/queue-cancel/rally, fog rendering, minimap, lighting, silhouettes, shadows, death fades) with zero golden movement |

**The plan was broken into parts along these milestones.** Parts 1–8 (M0–M3,
M4 movement, M5 economy/production/construction, M6 combat & vision, M7
AI through commands, and M8 match rules & full loop) are complete, plus
M9's Alpha content & feel pass — the loop now resolves and feels like a
game (M3's windowed path machine-verified; the human visual pass stays
open as DEBT-008's narrowed scope); M10 (stabilization & declaration)
remains, in strict order.

## 6.5 M10 prep — pre-milestone improvements (review pass)

Before M10 starts, a review pass tightened the code's hot paths and added
the M10 scaffolding the plan calls for. All changes preserve the golden
hashes (no observable behavior change in the sim or replays); every commit
runs the green gate (fmt, clippy `-D warnings`, the full test suite in
both profiles). One logical change per commit, on `master`.

**Performance (no semantic change)**

- `perf(sim): batch entity removal in death & cleanup` — stage 8 removed
  one entity at a time, each call doing up to 13 binary searches + 13
  `Vec::remove` shifts across the capability stores. New
  `World::remove_batch(ids: &[EntityId])` does a single merge-join sweep
  per store (both `ids` and the stores are ascending by id), reducing
  the cost from `O(m · (log n + n))` to `O(n + m)` per store. The
  post-state is byte-identical to the per-id path — pinned by a new
  test that builds two identical worlds, removes the same non-contiguous
  id set via each path, and asserts every store compares equal.
- `perf(sim): lockstep entity_view with health store in snapshot/player_view`
  — `Sim::snapshot` and `Sim::player_view` both projected each entity
  through `entity_view(id)`, which binary-searched the health store per
  entity for `hp_fraction_milli`. New `entity_views_lockstep` walks the
  entity stream and the health store in tandem (both ascending), turning
  the per-entity lookup into an O(n + h) lockstep walk. Output is
  byte-identical; the per-id `entity_view` helper is removed (it was an
  internal implementation detail with no remaining callers).
- `perf(engine): skip state_hash in HUD when debug overlay hidden` —
  `MatchHost::hud_state` used to call `Sim::state_hash()` every frame,
  even though the resulting value is only read inside the F3 debug
  overlay. The cheap path now leaves `state_hash` at zero; the new
  `hud_state_with_hash` variant is called by the client only when the
  overlay is visible. The `HudState` struct shape is unchanged; only
  the always-zero default in the cheap path differs.
- `perf(fx): make Fx::from_milli a const fn` — the conversion can run
  at compile time now (every op is const-evaluable in stable Rust 1.98
  once the clamp is hand-written, since `i64::clamp` is not yet
  const-stable). Callers can build `const` lookup tables for content
  stats without runtime cost.

**Generals: Zero Hour-style feel**

- `feat(client): camera rotation (Q/E) and pitch (Ctrl+wheel)` — the
  camera already had `rotate()` and `set_pitch()` methods but no input
  bindings. Q/E orbit the camera around its target (continuous, scaled
  by the frame delta — frame-rate-independent, like the WASD pan);
  Ctrl+wheel tilts the pitch up/down (clamped to the supported range).
  Cancels an armed attack-move so the player can re-orient mid-order.
  Adds supporting getters on `RtsCamera` (`pitch`, `yaw`, `distance`)
  and an `adjust_pitch(delta)` helper. README controls updated.

**M10 scaffolding (the plan's own deliverables)**

- `feat(tools): add soak subcommand for M10 prep (A7 acceptance gate)` —
  plan §12 calls for `tools soak --matches N` to run many seeded
  AI-vs-AI matches with crash/stall detection and win-rate telemetry.
  This is the sequential single-process version: nightly CI that wants
  parallelism spawns N `headless --seed N --p1 ai --p2 ai` subprocesses.
  Reports resolved/mutual-destruction/unresolved/crashed counts, per-player
  wins, avg/max end tick, resolution rate (A7 wants 100%), and
  ticks/second throughput. Exits non-zero only when a match crashed.
- `feat(tools): add bench subcommand for M10 perf baselines (plan §15)` —
  plan §12 calls for `tools bench` to measure tick cost against plan §15's
  perf budgets (≤ 1 ms avg, ≤ 4 ms p99, ≥ 1000 t/s on Alpha-size). This
  harness samples each tick's wall-clock duration with `Instant::now`
  (presentation-only telemetry — `tools` is exempt from the determinism
  source bans) and reports avg / p50 / p95 / p99 / max + throughput
  against the plan's thresholds. Sample dev-profile run on this commit:
  0.40 ms avg, 0.47 ms p99, 2512 t/s — all three budgets met.

**What M10 still owes**

- The full A1–A15 acceptance sweep with pinned evidence in
  `docs/ALPHA_DECLARATION.md` (plan §14).
- A 1000-match nightly soak run via `tools soak --matches 1000` (the
  A7 acceptance gate). Sequential for now; the parallel subprocess
  version is a CI-script follow-up.
- The release-profile perf baselines (plan §15) recorded via
  `tools bench --ticks 10000` against the real Alpha content with AI
  controllers.
- The DEBT-008 human re-verification of the M9.1 build (the only
  remaining open debt row).

**Verification**

- 352 dev tests green; clippy `-D warnings` clean; fmt check clean.
- Golden hashes unchanged: `tests/determinism.rs`, `tests/combat.rs`,
  `tests/economy.rs`, `tests/movement.rs`, `tests/alpha_loop.rs`,
  `tests/content_pipeline.rs`, `tests/vision.rs`, `tests/ai.rs`,
  `tests/match_rules.rs`, the `tools` golden, and the `engine` host
  tests all green at their pinned hashes.

## 7. Milestone inventory — what exists today, concretely

**Workspace & guardrails**

- Virtual `[workspace]` with `members = ["crates/*", "tests"]`; shared package
  metadata; resolver 2.
- `rust-toolchain.toml` pinning 1.98.1 (+rustfmt, +clippy, minimal profile).
- `clippy.toml` disallowing unordered-map types workspace-wide (A-006 explains the
  scope choice).
- CI: fmt, clippy `-D warnings`, tests × {Linux, Windows, macOS} × {dev, release}.
- `tests/` is itself a workspace member package (`pandemonium-tests`) so the plan's
  `tests/*.rs` acceptance files compile (A-002). New acceptance tests need an
  explicit `[[test]]` entry in `tests/Cargo.toml`.
- Git history: `docs: add plan.md v2.0 …` → `feat: M0 skeleton — workspace,
  guardrails, and the fx math crate` → `docs: add AI-Handoff.md …`.

**The fx crate (the only implemented library; plan §6.1)**

- `Fx` — Q16.16 scalar, raw `i32`. Contracts: 64-bit intermediates; mul/div round
  **toward zero**; unrepresentable results **saturate** (never panic, never wrap —
  identical in debug and release); div-by-zero saturates by dividend sign;
  `checked_mul`/`checked_div` for exactness checks; `from_int`, `from_milli`
  (the plan §10.2 authoring-unit conversion), `floor_int`/`trunc_int`/`round_int`
  (halves round up); total ordering on the raw value (valid deterministic sort key).
- `Vec2Fx` — add/sub/neg/scale/dot; `len_sq_raw()` (u64, for comparisons, plan
  §5.8); `len()` via exact `isqrt`; `dist()`; `normalized()` — integer-only, zero
  maps to zero by contract, precision caveat for very short vectors (DEBT-002).
- `isqrt(n: u64) -> u64` — exact floor integer square root, full u64 range.
- `Rng` — PCG32 (XSH-RR 64/32, constants from O'Neill's pcg_basic.c, A-009).
  `new(seed, stream)` uses canonical pcg32 seeding; `seeded(seed)` on the default
  stream; `next_u32`, `next_u64` (fixed hi-then-lo composition), `bounded(n)`
  (rejection-sampled, unbiased); `state_parts()`/`from_state_parts()` for canonical
  hashing and exact restore (plan §6.4).
- `XxHash64` — incremental xxHash64, the canonical hasher since M10.1 (DEBT-001
  repaid): the same `write_u16/u32/u64/i32/i64` little-endian surface as the old
  `Fnv1a64`, golden-tested against the reference XXH64 vectors (stripe boundaries
  31/32/33, a nonzero seed) and chunking-invariant by property test. `Fnv1a64`
  remains for the replay file checksum only.

**Tests (57, all green dev + release)**

- 39 unit tests across the fx modules (golden values, contracts, extremes).
- 15 proptest properties: conversion round-trips; add commutes/associates in range;
  mul commutes always; mul-by-one identity everywhere; mul→div recovery within 2
  ulps (representable products); div-by-zero saturation; isqrt floor property over
  the whole u64 range; vector length dominance; normalization unit-length band;
  RNG reproducibility and bounded range; incremental-vs-one-shot hashing.
- 2 architecture-law tests (§5 above).
- 1 sim constant test pinning `TICKS_PER_SECOND = 30`.

**M1 — the simulation core (plan §14, citing §6, §7, §8)**

- `crates/sim_api` — the full boundary vocabulary: `Command`/`CommandKind` (§8.1,
  all ten variants including the v2 queue commands), `Event` (§9.8, the planned
  variant set — M1 emits Spawned/Died/CommandRejected), `Reject`/`RejectReason`
  (§8.2), `MatchSetup`/`PlayerSetup`/`ControllerKind` (§6.5), `Snapshot`/`EntityView`/
  `MoveState` (§6.4), `PlayerView`/`ViewResource` (§9.6), `TilePos`, `EntityId::NONE`.
  Re-exports `Vec2Fx` (A-023).
- `crates/sim` — `Sim::new(world, setup)`, `step(&[Command])` as the only mutator
  (FD-2), running plan §6.3's eleven stages verbatim (unimplemented stages are
  documented no-ops with milestone pointers); monotonic id allocation from 1
  (`next_entity_id()` exposes the watermark); entity + capability stores (Health,
  Move, Vision) as Vecs kept ascending by id — deterministic iteration by
  construction, removal preserves survivor order, binary-search lookups; the
  shared command gate (stable (issuer, seq) sort, tick/issuer/duplicate-sequence/
  existence/ownership/capability/target checks, rejection events, zero state
  change on refusal); per-entity order queues with queue/replace semantics and a
  placeholder straight-line mover (DEBT-003); health advance + death & cleanup
  (regen is the M1 death path, A-015); scheduled spawns as the production
  stand-in (A-016); `state_hash()` — canonical LE, entity-major, capability
  presence bitmask, RNG state, allocator, players, `STATE_ENCODING_VERSION = 1`;
  periodic hash every 30 ticks in `StepOutput`; `snapshot()` and `player_view(p)`
  (circular-radius fog filter, A-018); `TrivialWorld` fixture with `content_hash()`
  (A-013) and §10.2 authoring-unit conversion at load.
- `crates/replay` — `ReplayFile`/`Checkpoint` matching plan §6.5's field list;
  own canonical LE codec (`bytes.rs`) with a trailing FNV-1a checksum (no serde —
  dependency law); `encode`/`decode`/`validate` with typed `ReplayError`s; the log
  records exactly the fed command stream, rejections included (A-019).
- `crates/tools` (clap CLI) — `headless [--seed N --ticks T --record FILE]`: runs
  the scripted demo match, prints every checkpoint + the final hash, optionally
  writes a replay; `replay-verify FILE`: decodes, validates, checks the content
  hash, re-simulates, compares every checkpoint (A2).
- `tests/determinism.rs` — 9 acceptance tests: A1 (two runs, full equality of
  checkpoints/final/events/snapshots/allocator; different seed diverges), pinned
  golden hashes, A2 (codec round-trip, checksum corruption detection, full
  re-simulation equality, content-hash check), id-never-reused (monotonic
  allocation, dead id never returns, post-death ids higher), iteration order
  (snapshots and player views strictly ascending every tick), invalid-commands-
  change-no-state (bit-identical hashes vs a quiet run), motion within one tick
  (A8's sim side), queue replace/append semantics.
- 30 sim unit tests, 4 sim_api unit tests, 8 replay codec tests, 6 tools tests.
- `.github/workflows/ci.yml` — authored during M1 because the file was missing
  from the inherited repo despite A-010 (see A-020): fmt + clippy, the
  3-OS × dev/release test matrix, the replay round-trip, and binary runnability.

**M2 — the content pipeline (plan §14, citing §10; see `docs/ARCHITECTURE.md` "The content pipeline" and `docs/CONTENT_GUIDE.md`)**

- `crates/content` — the RON schema (strict: every struct denies unknown fields;
  unknown fields are errors, plan §10.2) for rules/entities/factions/maps;
  schema-version gates with forward migration (map v1 — the pre-ADR-0001 shape —
  migrates to v2 as a flat map; newer-than-loader versions are precise errors,
  FD-10); precise typed errors (`ContentError`: file + entity/map + coordinates
  + the violated rule); validators at file level (stat ranges, duplicate
  capabilities, cross-references, grid shape, bounds, heightmap shape) and
  placement level per (map × faction): spawn legality, ore reachability
  (8-connected BFS per start), and the declared 180-degree symmetry check
  (grid + ore tile multiset + mirrored anchors). serde + ron live here and only
  here (plan §3.2, enforced by the architecture law).
- `ContentTree` / `ContentBundle` — the bundle carries the canonical content
  hash and map id (FNV-1a over an explicit little-endian encoding, plan §5.10;
  the display-only heightmap participates — content identity covers the whole
  bundle) and `world()` produces the plain `TrivialWorld` the simulation
  receives: `Sim::new(&bundle.world(), setup)` is the loaded-bundle path with
  **zero changes under crates/{sim,sim_api,fx,ai}** (A-024). Initial spawn
  order (therefore entity ids): map starts in authored order → each start's
  forces in faction order → ore nodes in map order; positions are footprint
  centers; jitter 0 for loaded content (A-028).
- `content/` — the Alpha manifest as data (plan §10.4 exact, test-pinned cell
  by cell): rules (Ore, 200 start, elimination + resignation), nine entities
  (worker/rifleman/raider/guardian, CC 4×4, barracks 3×3, supply depot, turret,
  ore node 2×2/1500), the Legion (roster, CC+barracks production lists, 1 CC +
  4 workers starting forces), and Crossroads (64×64, symmetric + verified,
  central crossroads choke, 4 ore per start + 4 contested center nodes,
  display-only heightmap — ADR-0001).
- `tools content-validate [PATH]` — strict validation + the bundle identity
  report; the repo's own tree is validated by a tools unit test.
- `tests/content_pipeline.rs` — the M2 acceptance suite: §10.4 stat pin,
  loaded-content starting state (22 entities, 200 Ore per player),
  loading/match determinism, and the A3 add-a-unit scaffold (a new kind defined
  purely by data files loads, spawns, and obeys a Move order; nothing under
  `crates/sim/` mentions it — source-scanned in the test).
- 31 content unit tests, 2 tools tests, 4 acceptance tests (150 total at the M2
  exit; dev and release identical).

**M5 — economy, production, construction (plan §14, citing §9.3, §9.4; see
`crates/sim/src/economy.rs`, `production.rs`, `invariants.rs`,
`tests/economy.rs`, and A-041..A-055 for the semantics)**

- `economy.rs` — stage 4: the worker gather loop as a phase machine (travel to
  the node through stage 6's shared travel targets, gather timer, extraction
  capped by capacity *and* node remaining, carry, deposit at the nearest
  completed storage with `ResourceDelivered`, repeat). Depleted nodes are
  removed (`NodeDepleted`) with their tiles released and affected workers
  auto-seeking the nearest same-resource node the same tick; no nodes left
  drops the order. Ghost `BuildAt` orders are hygiene-cleaned. The approach
  test (nearest footprint-rect point within 1500 milli + body radius) is
  deliberately generous so economy travel needs no crowd machinery (A-043).
- `production.rs` — stage 3: producers' ordered queues (cost paid on enqueue,
  refunded verbatim on cancel, only the head progresses, completed items hold
  at the front when population headroom is missing), spawns on the first free
  ring tile with a rally `MoveTo`, construction progress with a committed
  builder completing into `Lifecycle::Active` (builders' orders popped), the
  population usage/cap recompute (match start, end of stage 3, after deaths),
  and the one requirements checker (completed entities of the required kind,
  owned) shared by both surfaces.
- `command.rs` — the economy gate: Train/Cancel/SetRally/Build validation
  reading the fixture through the seam (capability, kind, faction production
  list, requirements → afford → population; placement = in-bounds + buildable
  + unclaimed + no mover standing), with Build spawning the site inline
  (occupying its tiles immediately, exact placement — no jitter). Rejection
  mappings per A-055.
- `invariants.rs` — the A12 checker: every invariant asserted at the end of
  every step in debug builds; five should-panic tests prove it fires; the
  strict `pop <= cap` reading is flagged for M6 (A-049, DEBT-010).
- The state encoding moved to **v3** and the fixture encoding to **v3** (the
  economy capability blocks, `GatherAt`/`BuildAt` orders, `UnderConstruction`
  lifecycle, kind economy, production lists, buildability, base cap) — the
  determinism goldens regenerated (A-050); the tools headless final hash
  moved to 0xbc86a622e357252d (seed 7, 300 ticks).
- Crossroads' four contested plaza nodes moved one tile outward (A-052): with
  footprints blocking tiles, the old doorstep positions sealed the only
  north-south crossing (independently verified by BFS); the content hash and
  map id moved accordingly.
- `tests/economy.rs` — the exit suite (divergent openings, determinism, the
  A12 soak, soak determinism); `tests/content_pipeline.rs` grew the A3/DEBT-007
  train-from-barracks half.

**M3 — the engine shell (plan §14, citing §11 as amended by ADR-0001; slices 1–2,
see DEBT-008/009 for what remains)**

- `crates/engine` — `FixedTimestep` (30 Hz accumulator, 5-tick catch-up cap,
  spiral-of-death backlog drop, `alpha()`); `Interpolator` (prev/current
  snapshot blending: lerp positions/facing, spawns appear without backward
  extrapolation, deaths drop, discrete fields from current; the Q16.16 ↔
  ground-plane boundary conversions); `MatchHost` — owns the `Sim` privately,
  so submit(commands) + advance(dt) are the only mutation paths (the M3 exit
  criterion as a compile-time property; commands are tick-stamped for the
  next step, so the gate never rejects them; the initial Spawned batch
  surfaces through the first advance); `RtsCamera` (glam: perspective orbit,
  pan/zoom/rotate with clamps, ground-plane ray picking via the inverse
  view-projection, NDC projection, screen-space box selection);
  `mesh::terrain_mesh` (one quad per tile, heightmap displacement, class
  colors — pure data, GPU-free, tested); `Renderer` trait + `NullRenderer`.
- `crates/client` — winit 0.30 window + wgpu 26 renderer with a depth buffer:
  terrain pipeline (the mesh), entity pipeline (36-vertex unit cube, instanced
  per entity: position/half-extent/color, team colors, selection brightening),
  the UI overlay pipeline (screen-space pixels → NDC, glyph atlas as R8Unorm,
  alpha blending, on top of the world), WASD pan + wheel zoom + left click /
  drag-box selection (own units) + right-click Move via ground picking
  converted to fixed point, Escape exits. F3 toggles the §11.6 debug overlay,
  P pauses, `.` single-steps; the HUD (resources by data-defined display name
  + POP) is always visible. `--frames N` exits after N frames with the
  evidence summary. Without a display the binary prints the finding and runs
  the headless smoke pass (real content, 180 frames, state + content hashes,
  exit 0) so CI's every-binary-starts check stays green.
- `crates/client/src/text.rs` — our own text renderer (plan §3.1/§3.2):
  fontdue rasterizes the embedded "Pandemonium Sans" (ASCII subset of DejaVu
  Sans, renamed per the Bitstream Vera license) once into a coverage atlas;
  layout is plain client-side math (5 unit tests pin the packing invariant,
  draw order, baseline conventions, and the fallback box).
- 28 engine tests; 5 client tests; workspace 183/183 in dev and release.

**M4 — movement (plan §14, citing §9.1; see `crates/sim/src/nav.rs` and
`movement.rs`, tests/movement.rs, and A-039/A-040 for the semantics)**

- `nav.rs` — the navigation layer: `NavGrid` over the world's passability
  (the content seam maps the map's terrain classes into it); 8-connected A*
  with no corner cutting, integer costs, the octile heuristic, and the
  total-order `(f, h, tile)` heap key (deterministic pops → deterministic
  paths); the straight beeline validated by an integer supercover of the
  exact segment (corner crossings checked like A* diagonals); tile-center
  routes prefixed with the start tile's center and suffixed with the exact
  goal; blocked/unreachable goals resolve to the nearest reachable tile,
  sealed starts to `None`.
- `movement.rs` — stage 6 as the three layers: path requests (immediate
  `MoveFailed` for unreachable goals and zero-speed distant orders), waypoint
  steering with a terrain guard, spatial-hash push-apart (one-tile cells,
  id-ordered pair resolution, symmetric terrain-guarded halves), stuck
  detection with repath escalation and a body-aware crowded arrival.
  `MoveDef` carries radius + path + counters (encoded in the state hash v2);
  replacing orders reset the runtime path.
- Exit tests (tests/movement.rs): 50-unit spam-click responsiveness, order
  resolution bounds, determinism, and the real-map detour; sim crate +21
  tests; workspace 208/208 in dev and release.

**M7 — AI through commands (plan §14, citing §9.6; see `crates/ai/src/`,
`crates/engine/src/ai_host.rs`, `crates/tools/src/ai_match.rs`,
`tests/ai.rs`, and A-060..A-065 for the semantics)**

- `crates/ai` — the plan's `Controller` trait verbatim
  (`think(&mut self, view: &PlayerView, tick, out: &mut Vec<Command>)`):
  perceive a fog-filtered view, decide, act by emitting commands. The
  crate depends only on fx + sim_api — "AI reaches into game state" is a
  link error (FD-7). `ScriptedController` is the Alpha opponent: gather
  management for idle workers (nearest nodes, round-robin), worker
  training to target with pending bookkeeping, depot + barracks builds
  on headroom/script triggers with candidate spots consumed in order and
  attempts verified by count, round-robin army composition, defense
  (`Attack` on the nearest visible intruder near home), and a
  Massing/Attacking wave machine with the seeded per-wave jitter. 15
  unit tests pin every behavior including controller determinism.
- `crates/engine/src/ai_host.rs` — `AiMatchHost`: the headless hosting of
  controllers on the tick boundary (ascending slot order, every fed
  command recorded — rejections included), `alpha_controller`/`alpha_plan`
  (capability-shaped kind resolution from the bundle: production lists
  decide the roster, supply is a population structure, the node kind
  carries a Resource body; costs through the Ore seam; starts from the
  map), and `build_spots` (the deterministic ring scan: footprint-fitting,
  buildable, clear of t0 static claims, one tile off node doorsteps).
  7 engine tests: fog-filtered views feed the gate, the log records
  rejections verbatim, controller order, outside-slot refusal, and
  log-only re-simulation equality.
- `crates/tools/src/ai_match.rs` — the CLI driver: setup from map starts,
  controllers via the engine host, checkpoints + forced final, the replay
  record, and the per-player evidence summary from the event stream
  (site owners mapped through ConstructionStarted's builder). `--p1/--p2
  demo|ai|idle` (demo default — the M1 match and its pinned hash are
  unchanged); `replay-verify` resolves the world by content identity.
- `crates/sim/src/runner.rs` — `run_command_log`, the one canonical
  re-simulation driver (DEBT-005 repaid): tick-0 checkpoint, stable
  tick-sort feed, periodic hashes, forced final, allocator watermark.
  Tools' recorder and verifier and the acceptance suites all drive
  through it.
- `tests/ai.rs` — the M7 exit suite (8 tests): the two A5 structural
  audits, the A5 ledger identity + entity accounting, the A11 mirrored
  battery (label-blind), the A11 proptest fuzz, AI-vs-AI completion +
  determinism across seeds, the golden pin (re-pinned by M9, twice,
  each with its written reason in the test), and the log-alone replay
  (codec round-trip + re-simulation equality).

**M9 — Alpha content & feel pass (plan §14 M9; see `crates/sim/src/combat.rs`,
`crates/ai/src/scripted.rs`, `crates/engine/src/audio.rs`,
`crates/client/src/feedback.rs`, and `tests/alpha_loop.rs`)**

- `crates/sim/src/combat.rs` — the chase-order pop: combat stage 1 pops a
  head `Order::AttackUnit` whose commanded target is gone (dead and
  removed by stage 8, or removed by its own system), resets the runtime
  path, and the queue behind it advances. The M8 flagship's stall
  diagnosis: both sides' defense `Attack` orders outlived their dead
  intruders; the chasers kept reporting `Moving` (orders non-empty) while
  movement resolved no target, requested no path, and the stuck detector
  skipped their empty paths — frozen forever, still shooting whatever
  entered acquire range. Two unit tests pin the pop and the
  queue-advance (a queued Move takes over when the chase target dies).
- `crates/ai/src/scripted.rs` — the closing tuning: WAVE_SIZE 10,
  ARMY_CAP 16, MIN_WAVE 4, WAVE_TIMER_TICKS 2400; the Attacking state
  re-issues the march on the pressure cadence (a live wave never parks);
  `hunt_focus` aims the march at the sighted enemy command center (the
  elimination target) through the fog, falling back to the enemy start.
  The view-level tests re-pinned; a new test pins the re-march and the
  CC focus.
- `crates/engine/src/audio.rs` — plan §11.5 verbatim: the `AudioSink`
  trait, a pure `cue_for(&Event) -> Option<AudioCue>` mapping (attack
  landed, unit lost, unit ready, structure done, delivery, match ended
  — bookkeeping events stay silent), and the `NullAudioSink` counter
  implementation (headless + tests). 3 unit tests.
- `crates/client/src/feedback.rs` + `main.rs` + `render.rs` — the feel
  pass: `FeedbackState` (hit flashes with frame expiry, command pings,
  counters) fed from the draw loop's event drain; health-bar quads
  projected through the camera (the box-select path); the flash set
  crossing the Frame boundary so the wgpu renderer tints flashing
  entities; pings at the clicked ground on Move/AttackMove; restart
  resets the feedback state. 5 unit tests (pure geometry, no GPU). The
  headless smoke now drives the windowed hosting seam (AI included) and
  prints the feedback-wiring counters; the windowed summary reports the
  match outcome.
- `tests/alpha_loop.rs` — the M9 exit suite (3 tests): resolution within
  the fifteen-minute budget across seeds (7 typical, 3 slow-resolving),
  end-to-end resolution determinism (same seed → same winner, end tick,
  bit-identical log + final hash; a different seed diverges), and the
  windowed host loop resolving against the AI (`MatchHost::with_controllers`
  with an idle human slot; the AI eliminates the human's base; `outcome()`
  surfaces through the end-screen boundary).

## 8. What is NOT built yet

M3 through M8 are complete: the engine shell with its machine-verified
windowed client, the HUD/debug overlay slice (DEBT-009 repaid), the
three-layer movement system (DEBT-003 repaid), the economy stack — ledger,
gather loop, production queues, construction lifecycle, population,
requirements, footprints blocking tiles, and the A12 checker — the
combat & vision stack (the immediate-hit attack pipeline, the three-state
fog model, the A10 fog integrity proof, the A12 combat invariants), the
AI stack (the Controller trait, the scripted Alpha opponent, controller
hosting on the tick boundary, the headless AI-vs-AI runner, and the
A5/A11 parity audits), and now the match-rules & full-loop stack
(stage 10's defeat/victory/resignation evaluation, the `MatchEnded`
event, the `MatchHost` extensions for AI hosting + outcome + log, the
windowed client's vs-AI loop with end screen + restart + control groups +
hotkeys, and the A15 restart cleanliness proof). What remains, in order:

1. **The human *re*-verification pass of DEBT-008 (narrowed to its last
   inch by M9.1)**: the human pass finally RAN (the project owner played
   the M9 windowed build on a real display) — and it did its job: it
   found every input-layer defect the machine pass is structurally blind
   to (mirrored pan axes since M3, no mouse camera control, no attack
   order path, key-repeat spam, silent rejections, the illegible opening
   zoom, never-expiring cues). M9.1 fixed them all; the machine half is
   re-proven (Xvfb + XTEST: 900 frames, 5-entity drag selection, context
   Move + armed attack-move). What still needs a human: play the M9.1
   build — `cargo run -p pandemonium-client` — through a full vs-AI
   match to resolution and judge that the controls now *feel* right
   (pan/zoom/edge-scroll/middle-drag, right-click context orders, the
   armed-A flow, the refusal cues, the selection brackets, the opening
   framing, the end screen + restart). Then DEBT-008 closes.
2. **M10 — Stabilization & declaration (Phase 5)**: the full A1–A15
   acceptance sweep with written evidence, the 1000-match nightly soak
   (A7), benchmark baselines (plan §15), the documentation pass, and
   `docs/ALPHA_DECLARATION.md`. DEBT-011 (no audible audio backend)
   is a declaration-time finding, not a blocker — plan §11.5's letter
   is satisfied by the seam.
3. Known limitations to carry forward honestly: the formation-less jam shape
   (A-040), path-smoothing-free staircases on detours, stalled construction
   sites when the builder dies (no reassignment command — A-045, carried
   forward; the scripted AI also shares this hole — a killed builder's site
   stalls, and the script never reassigns), the O(N*V) per-tick fog
   recompute (truly incremental updates wait for a profile-driven need),
   the scripted AI's sight-verification latency (a rejected order is
   retried only after its cooldown — A-062's deliberate shape), and the
   scripted AI's scriptedness itself (it closes games now — A-072's
   re-march + CC focus + wave-mass tuning — but it is still a script,
   not an evaluator; plan §17 sequences the evaluative AI post-Alpha).

The match rules (M8) are complete: stage 10 fires `MatchEnded` exactly
once, the outcome surfaces through the host, restart cleanliness (A15) is
pinned, and the windowed client hosts the AI opponent through the engine's
existing `ai_host` seam.

## 9. Sharp edges and gotchas discovered along the way

- **Presentation direction is invisible to the simulation's test wall** (the
  M9.1 lesson, and the whole reason the input layer shipped mirrored): the
  canonical hash pins behavior, not feel — the camera's screen-right axis had
  been negated since M3, every milestone green, until a human pressed D and
  the view went left. Two defenses now exist: direction-pinning tests
  (pan/zoom/focus assert *which way* in world coordinates, at yaw 0 and 90°),
  and the DEBT-008 human pass as a first-class milestone exit. Any new
  camera/feel primitive needs both.
- **A winit key repeat re-fires the key-down edge**: `event.repeat` must be
  filtered, or every held hotkey re-fires once per OS repeat (holding S to
  pan spammed Stop orders — and 'A' spammed attack-moves at the cursor).
  The client returns early on repeats before any hotkey logic runs.
- **Frame-counted presentation state needs a clock that always advances**:
  M9's flashes and pings expired by `frames_presented`, but that counter only
  advanced in the `--frames` verification mode — in normal play it was pinned
  at zero, so every cue accumulated forever. It now increments on every
  presented frame. Any future "expires after N frames" state must share that
  clock, and the verification budget must read the counter without owning it.
- **An Xvfb screen must be at least as large as the client window** for XTEST
  input to arrive: the default window is 800x600, and on a 640x360 screen the
  pointer reports the root window — the mapped window never receives button
  events (it "looks" visible; `xdotool search --onlyvisible` lists it). Use
  `Xvfb :99 -screen 0 1280x800x24` for the 800x600 default window.
- **No window manager means no keyboard focus — and installing one breaks the
  GL surface**: keyboard events go nowhere without a WM, but running twm
  under Xvfb + llvmpipe makes wgpu fail adapter creation ("gl not compatible
  with provided surface"). The working recipe is no WM at all +
  `xdotool windowfocus <id>` (XSetInputFocus) for keyboard, XTEST for mouse
  (mouse events need no focus; the pointer just has to be over the window,
  which is what the screen-size rule above guarantees).
- **fontdue reports y-up metrics**: `Metrics::ymin` counts upward from the
  baseline, so a y-down screen-space top edge is `baseline - (ymin + height)`
  (see `text.rs`'s `bearing_y`). Getting this backwards puts glyphs below
  their baseline — the descender test pins the convention.
- **A glyph-atlas coverage buffer must cover exactly `width × height` bytes**
  before `write_texture`: build it with `resize`, not `truncate` (truncate
  never grows; a one-row-short buffer fails wgpu's bounds validation at
  frame one — the first Xvfb run caught exactly this).
- **The wgpu GL backend on a headless box needs a userland EGL stack** (no
  root): `apt-get download libegl1 libegl-mesa0 libgles2 libxtst6
  libxkbcommon-x11-0 libxcb-xkb1` + `dpkg -x`, `LD_LIBRARY_PATH` at the
  extracted `usr/lib/x86_64-linux-gnu`, glvnd via
  `__EGL_VENDOR_LIBRARY_DIRS`, and *unversioned* `libxkbcommon*.so` symlinks
  (xkbcommon-dl dlopens the unversioned names). Then Xvfb +
  `WGPU_BACKEND=gl` + `LIBGL_ALWAYS_SOFTWARE=1` renders on llvmpipe, and
  XTEST (`libXtst`) injects real mouse input (see A-037).
- **Integer segment traversal is a sign-party**: comparing grid-line
  crossings by cross-multiplication is only order-correct when `dx` and `dy`
  share a sign — use step-signed distances (`dist_x * |dy|` vs `dist_y *
  |dx|`) instead. And a goal that sits *exactly on a lattice corner* makes a
  naive traversal step diagonally past the end tile: stop at any crossing at
  or past the goal. Both bugs shipped and were caught by tests
  (`beelines_work_in_every_quadrant_direction`, the Crossroads detour test).
- **Units funnel when lanes converge**: multiple movers sharing tile-center
  waypoints (same start tile, or walled funnels) jostle and can escalate to
  `MoveFailed`; the straight beeline's divergent lanes are also the crowd
  mitigation (A-040). Keep group-click tests on compact-block shapes.
- **proptest macro quirk**: `prop_assert!(x as T < (y + 1) * (y + 1))` fails to
  parse inside `proptest!` (cast-then-`<` breaks the expr fragment). Bind locals
  first; see `isqrt_is_the_floor_of_the_root` for the pattern.
- **Fx saturates on extremes** — including legitimate-seeming mul→div round trips
  when the intermediate product exceeds ±2^31 raw (a·b ≈ 2^47 raw²). The
  mul→div-recovery property is scoped to representable products for this reason;
  saturation itself is separately tested.
- **Clippy 1.98**: `#![cfg_attr(clippy, deny(...))]` triggers
  `clippy::no_mismatched_clippy_cfg`-style errors — use plain
  `#![deny(clippy::float_arithmetic, clippy::disallowed_types, clippy::disallowed_methods)]`
  (rustc accepts and ignores clippy tool lints).
- Inherent `Fx::mul`/`Fx::div` carry `#[allow(clippy::should_implement_trait)]` on
  purpose: the inherent methods are the canonical documented ops and the operator
  impls delegate to them (plan §5.6 favors greppable explicit arithmetic).
- **Docs in the determinism crates must avoid the literal banned tokens**
  (`HashMap`, `HashSet`, `Instant`, `SystemTime`, `f32`, `f64`) — the source scan
  checks comments and doc text too. Write "unordered-map types", "wall-clock
  reads", "floating-point types" instead.
- Rust 1.98.1 was installed via rustup in this environment; the exact-pin in
  `rust-toolchain.toml` auto-installs on first use (network needed once).
- **Clippy 1.98 new-style lints bite old idioms** (all `-D warnings` failures):
  `x % n == 0` → `x.is_multiple_of(n)`; `map_or(true, …)` → `is_none_or(…)`;
  `[b'P', b'D', …]` → `*b"PD…"`. Write the new forms directly.
- **A command log must stay chronological** (weakly ascending tick order): a
  replay feeds every command at its *declared* tick, so a genuinely stale-tick
  command cannot exist in a recorded log — stale arrivals are a live-client
  phenomenon only. Keep testing TickMismatch by feeding `step()` directly.
- **Feed order within a tick matters for duplicate (issuer, seq)**: the gate's
  stable (issuer, seq) sort preserves feed order for exact ties, and the first
  command wins. Replay drivers must sort by tick *only* (stable) and never
  re-sort the whole log, or duplicate resolution diverges.
- **Borrow checker vs. per-system iteration**: iterating `&mut world.entities`
  while reading `world.movement` is fine (disjoint field paths) but NOT through
  `&World` methods. The mover collects `(id, speed)` pairs first, then mutates —
  keep that shape when adding systems.
- **Fx::from_milli rounds toward zero**, so per-tick speeds converted from
  milli-tiles/s are floor-ish and exact-arrival logic compares `dist <= speed`
  (snap to target) rather than assuming divisibility.
- **Events emitted during `Sim::new`** (the initial `Spawned` batch) sit in the
  buffer until the first `step` drains them — expected, tested, do not "fix".
- **The banned-token scan is a substring scan**: "**Instant**-hit" (plan §9.2's
  own wording!) trips the wall-clock `Instant` ban in the determinism crates.
  Write "immediate-hit"; check `crates/content` docs with the scan in mind
  (M2 hit this once).
- **RON strict-mode error wording** is "Unexpected field named `x` in `Struct`,
  expected one of …" — assert on "Unexpected field" + the field name, not on
  serde's classic "unknown field" phrasing.
- **Serde maps in content must deserialize into ordered maps** (`BTreeMap`) —
  unordered map types are banned workspace-wide (clippy.toml) and would also
  break load determinism.
- **Directory iteration order is OS-dependent** — the content loader sorts each
  category by file name before parsing; keep that invariant if you touch
  `loader.rs` (plan §5: iteration order is determinism).
- **Raw strings containing `"#"`** (terrain codes!) need `r##"…"##` delimiters —
  `r#"…"#` terminates at the first `"#`. Bit the map test factory once.
- **`CARGO_MANIFEST_DIR` depth differs per crate**: tests-package paths need one
  `.parent()` to reach the root, tools-package paths need `ancestors().nth(2)`.
  Wrong depth silently points at `crates/content` instead of `content/`.
- **anyhow's default Display shows only the outer context** — in tests, format
  with `{err:#}` to see the underlying `ContentError` chain.
- **`Fx::raw()` vs authored milli-tiles**: Q16.16 raw values are ×65536; assert
  positions via `Fx::from_milli(milli)` equality, never raw integers, unless you
  enjoy arithmetic slips.
- **Tile centers are `x * 1000 + 500` milli-tiles** — the soak caught a
  spawn-position helper that doubled the conversion (`x * 2000 + 1000`) and
  placed player 1's first trained worker at (95, 95) on a 64-tile map. The
  A12 checker is what flagged it; write tile math once and reuse it.
- **Economy orders are long-lived**: movement pops only `MoveTo` on path
  completion, and stuck detection skips empty-path (in-place work) movers.
  A new economy phase must clear the runtime path itself or the worker stands
  on a stale lane. Never compare a path's *terminus* to a blocked goal's
  center — paths legitimately end on the nearest reachable tile, and the
  mismatch loop (clear + rebuild every tick) freezes the worker on the
  start-waypoint no-op leg.
- **Debug-only `#[should_panic]` tests must be `#[cfg(debug_assertions)]`** —
  `debug_assert` compiles out in release, so un-gated should-panic tests fail
  there (the M5 checker tests carry the gate).
- **Same-script soak runs need player-relative build spots** — both players
  running one script with a hardcoded placement tile fight over the same
  ground; mirror the spot for player 1.
- **Footprint blocking reshapes old scenarios**: the M4 real-map exit test
  recalibrated its arrival bound (1100 → 2800 milli, the blocked-destination
  packing envelope) and gained a no-footprint-tile assertion; the map's
  contested nodes moved a tile because the old spots sealed the crossing.
  Content geometry + blocking semantics must be re-validated together.
- **Entity ids in tests follow the documented spawn order** — off-by-one kind
  or entity ids are the recurring test-authoring bug this milestone (kind
  lists reorder when a new kind inserts mid-vector; check `bundle.entities`
  positions, never hand-count).
- **The `is_none_or` / `is_multiple_of` new-style lints** now also bite test
  code (cooldown sentinels as `u32::MAX / 2` fight `saturating_add`; use
  `Option<u32>` + `is_none_or`).
- **M6 combat cooldowns: author in ms, compare in ticks**. `cooldown_ms: 33`
  → `ms_to_ticks(33, 30) = ceil(33*30/1000) = 1` tick. `cooldown_ms: 100` →
  `ceil(100*30/1000) = 3` ticks (NOT 1). The fixture/content loader uses
  `(ms * tps + 999) / 1000` (ceiling division); always check the converted
  value, not the authored ms, when reasoning about hit cadence.
- **M6 fog is derived, not hashed (A-059)**. The `FogState` on `Sim` is a
  per-tick cache recomputed in stage 9; it is NOT part of the canonical
  state hash. A10's "fog on/off, hashes equal" is trivially true because
  the hash never includes the bitsets. Do NOT add fog to `hash_state` —
  it would couple the hash to a per-player derivative and break A10.
- **M6 combat `acquire_target` scans the entity store ascending id**. The
  tie-break is "lower dist_sq wins, ties to the lower id" — `update_best`
  uses `<=` so a same-distance later candidate loses. If you change the
  scan order or the comparison operator, the acquisition tie-break changes
  and the M6 combat tests will catch it.
- **M6 `clear_dead_targets` runs in stage 8, AFTER removal**. Stage 8
  collects the dead ids, removes them, then calls `clear_dead_targets` so
  attacker target slots pointing at the dead don't dangle (ids are never
  reused, so a stale slot would never re-resolve). If you add a new death
  path, make sure it goes through stage 8's removal + clear_dead_targets.
- **M6 visual cues ride DEBT-008**. The machine-verifiable half (AttackHit
  fires, hp_fraction_milli on EntityView, events flow to the client) is
  green. The pixel-level rendering of tracers + health bars in the wgpu
  client is a human-eyeball item — without a display, the windowed path
  can't be verified. Do not claim M6's visual cues are complete without a
  human pass on a real display.
- **M7 controller sequences reset every tick**. `(issuer, seq)` is unique
  only within one tick — a controller restarts its seq counter each think.
  Any cross-tick accounting (rejection maps, log analysis) must key on
  `(tick, issuer, seq)` or silently collide (the A5 ledger test hit this
  exactly: spent costs vanished into phantom rejections).
- **M7 build verification must be count-based, not proximity-based**. A
  successful Build spawns the site instantly, so "an own entity of the kind
  appeared" (count increase) is exact — while a spot-proximity check gave
  false successes on adjacent candidate spots and cycled placements every
  two ticks (the first AI match burned 71 depot attempts for 4 sites).
- **M7 `MoveState::Idle` means NO ORDERS, not "standing still"**. A worker
  mid-gather, mid-build, or mid-march has orders; an auto-acquiring fighter
  in combat does NOT (the Attack target slot is not an order). Re-tasking
  idle entities can therefore never cancel work in progress — and "all
  army idle" is NOT a wave-regroup signal.
- **M7 construction sites spawn without Spawned events**. They surface as
  ConstructionStarted (with their builder); owner bookkeeping must flow
  through the builder id. The tools' evidence summary and the tests' owner
  maps both do this.
- **M7 pre-wave AI logs are seed-independent**. The script's only
  randomness is the wave jitter, so two seeds can produce identical early
  logs — assert state divergence (the hashed RNG stream), not log
  inequality.
- **M7 milli-tiles vs tiles, the classic slip**. `radius_milli * 65,536`
  is a radius in TILES of raw units; milli-tiles need `* 65,536 / 1,000`.
  The first version made the AI's 12-tile defense radius a 12,000-tile
  one — the unit tests caught an "intruder" at 36 tiles being attacked.
- **M7 RefCell test controllers deadlock on held borrows**. A test
  controller that records views must clone the view out before the next
  advance — holding `&seen.borrow()[0]` across `advance()` panics
  (already-borrowed) because the next think does `borrow_mut()`.
- **M8 stage 10 must be hash-neutral or every M7 golden breaks**. The
  match outcome (`Sim::outcome`) is a derived field, NOT part of the
  canonical hash (A-066 — the same reasoning as fog, A-059). Adding it
  to `hash_state` would couple the hash to a derivative of fields that
  are already hashed, and would break every M7 golden (the demo's
  0x9d5ba9b565060336, the AI-vs-AI flagship's 0x679f4713114765c9). The
  outcome is a pure function of the entity set + the players' `resigned`
  flags, both of which ARE hashed. Toggling "evaluate match rules on/off"
  cannot change a checkpoint — the M8 invariant, pinned by
  `stage_10_does_not_change_the_m7_golden_checkpoint_trail`.
- **M8 the defeat check needs the "structure-bearing" guard (A-067)**.
  A strict "zero structures ⇒ defeated" rule would end every M1
  spine-test match at tick 0 — the `TrivialWorld` fixture carries no
  Footprint kinds at all (it predates the economy milestone), so both
  players would be simultaneously defeated with `NEUTRAL` the winner.
  The guard ("at least one player owns a structure") keeps the spine
  tests green; the Alpha content (every player starts with a Command
  Center) fires the rule only when a structure is actually destroyed —
  the intent of plan §9.7.
- **M8 restart is by drop + reconstruct, not an in-place `restart()`**.
  A deterministic controller's RNG state has advanced during the match;
  an in-place `restart()` would need to reset the controller, which
  requires either a `Controller::reset(seed)` trait method (frozen
  decision territory — needs an ADR) or re-creating the controller from
  the bundle + seed. The clean path is drop + reconstruct: the client
  retains its `ContentBundle` and `MatchSetup`, constructs a fresh
  `MatchHost::with_controllers` with a fresh `alpha_controller` (whose
  RNG re-derives from the match seed). A15 tests this by constructing
  two fresh `MatchHost` instances and asserting identical hashes + logs.
- **M8 `MatchHost` lost `Clone`/`Debug` when it gained controllers**.
  `Box<dyn Controller>` carries neither trait. No caller clones a
  `MatchHost` (the M3 tests assert on outcomes, not on the host itself),
  so removing the derives is safe. Restart is by drop + reconstruct,
  not by clone-and-reset.
- **M9 a chase order outlives its target unless combat pops it**. The
  bug shape: `Order::AttackUnit` whose target died stays on the queue;
  `movement_target` resolves to None (no path request), the stuck
  detector skips empty-path movers, and the unit reports `Moving`
  forever while frozen. movement.rs's own doc comment says the combat
  pipeline pops them when the target dies — the code never did. Any
  new death-adjacent removal path must keep stage 1's pop (or route
  through it): the pop checks `world.entity(target).is_none()`, which
  covers every removal (stage 8 deaths, node depletion) one tick after
  the removal lands.
- **M9 a wave that merely exists does not press**. The wave machine's
  `army >= MIN_WAVE` liveness check read parked camps as an active
  press: march orders drain (MoveFailed at a choke, completed over a
  cleared target, replaced by a defense pull) and the army sits idle
  with the state machine still `Attacking`. The fix is the re-march
  cadence — while a wave is alive, its timer re-issues the march. Any
  future scripted behavior with the same "state implies activity"
  assumption needs the same timer-based liveness proof.
- **M9 the wave-mass numbers are load-bearing**. Waves of six traded
  forever in low-count attrition cycles (both sides rebuild ~1 unit per
  300 ticks from one barracks; the cycles never accumulate the ~100
  DPS-seconds needed to level 1900 HP of structures). Waves of ten —
  with the round-robin Guardian in the mix — close in one successful
  press. If a content pass changes build times or HP substantially,
  re-run the 32-seed resolution sweep before believing anything.
- **M9 feedback state is frame-indexed, not tick-indexed**. Flashes and
  pings expire by *presented frame* count (the feel pass lives between
  sim ticks); under the 5-tick catch-up cap a slow frame can run several
  ticks — the cues stay smooth because they key off the draw loop, not
  the tick counter. Keep it that way: tick-indexed cues would strobe
  under catch-up.
- **M9 the full-windowed Xvfb stack works on this machine family**.
  Xvfb + `WGPU_BACKEND=gl` + `LIBGL_ALWAYS_SOFTWARE=1` + a userland
  `LD_LIBRARY_PATH` with `libEGL.so`/`libGLESv2.so`/`libxkbcommon*.so`
  (unversioned symlinks — xkbcommon-dl dlopens the unversioned names)
  + a hand-written glvnd `__EGL_VENDOR_LIBRARY_DIRS` json pointing at
  `libEGL_mesa.so.0` (the system had none) renders on llvmpipe and
  lets the windowed client run a FULL vs-AI match to resolution —
  the summary prints the outcome. `xvfb-run` itself is broken here
  (no xauth): start `Xvfb :99` directly and set `DISPLAY=:99`.

## 10. Maintenance protocol — every future agent, every milestone

1. Read plan.md §0 + §14, this file, `docs/DEBT.md`, `docs/ASSUMPTIONS.md`.
2. Run the §4 verification. Everything must be green **before** you start (else
   you are inheriting breakage — stop and investigate) and **after** you finish.
3. Implement exactly one milestone (or a logged sub-slice of one). Do not skip
   ahead; do not add unrequested features; do not weaken an exit test.
4. Log as you go: assumptions → `docs/ASSUMPTIONS.md`; simplifications →
   `docs/DEBT.md`; frozen-decision challenges → an ADR in `docs/adr/`.
5. Update this file: status board (§6), inventory (§7), gotchas (§9), and the
   snapshot (§2). An out-of-date handoff is a bug.
6. Commit green, one logical change per commit, messages in the existing style.
7. Honest declaration (plan §13): a criterion that cannot be met honestly is
   recorded as a finding, never waved through.

## 11. Pointer index (plan section → where it lives in the repo)

| Plan section | Where |
|---|---|
| §0–§1 contract & vision | `plan.md`; digested in this file §3 |
| §2 frozen decisions | `plan.md`; changes via `docs/adr/` |
| §3.2 dependency policy | `tests/architecture_law.rs` allow-lists (enforced) |
| §4 layout & law | `docs/ARCHITECTURE.md` + `tests/architecture_law.rs` |
| §5 determinism rules | fx API docs, `clippy.toml`, `tests/architecture_law.rs` scan |
| §6 simulation core | `crates/sim/src/` — `sim.rs` (pipeline), `world.rs` (stores), `hash.rs` (state hash), `fixture.rs` (trivial world) |
| §7 entity model | `crates/sim/src/world.rs` + capability stores; `crates/sim_api` ids |
| §8 commands | `crates/sim_api/src/lib.rs` (types) + `crates/sim/src/command.rs` (gate + application) |
| §9 systems | per-milestone; see status board §6 (M1 spine; M4 movement; M5: economy.rs + production.rs + invariants.rs; M6: combat.rs + vision.rs; M7: crates/ai + engine ai_host; M8: sim/match_rules.rs + engine host extensions + client vs-AI loop; M9: combat.rs's chase-order pop + ai/scripted.rs's wave closing + engine audio.rs) |
| §10 content | `docs/CONTENT_GUIDE.md`, `content/` (live), `crates/content/src/` (schema/version/loader/defs/validate/bundle) |
| §11 engine/client | `crates/engine` (clock, interpolate, host + pause/single-step + M8 controllers/log/outcome, camera + M9.1's unmirrored axes/pan_world/zoom_toward/focus/map clamp, mesh, renderer + HudState + M9 flashes on Frame, ai_host, audio) + `crates/client` (wgpu renderer, UI overlay pass + M8 end screen + M9.1 viewport-centered, input — M8 control groups/hotkeys/restart, M9.1's WASD/arrows/edge/middle-drag/zoom-toward + repeat guard + armed-'A' — text.rs, feedback.rs + M9.1's brackets/refusals, orders.rs = the §11.3 right-click context resolution) — 3D per ADR-0001; DEBT-008 keeps only the human re-verification |
| §12 tools | `crates/tools` — headless (demo + `--p1/--p2 ai|idle` + M8 winner line), replay-verify (content-identity world resolution), content-validate live |
| §13 acceptance | `tests/` — A13 live; A1/A2 + spine proofs (M1); A3 scaffold + §10.4 pin (M2, `content_pipeline.rs`); A5 + A11 (M7, `ai.rs`); A15 + match rules (M8, `match_rules.rs`); the M9 resolution exit (M9, `alpha_loop.rs`) |
| §14 milestones | this file §6 status board |
| §15–§19 budgets/risks/debt | `plan.md`; debt live in `docs/DEBT.md` |
