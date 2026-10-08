# Changelog

All notable changes to Pandemonium: Build & Destroy are recorded here, one
entry per milestone (the project ships milestone-by-milestone; see
`plan.md` §14 and `AI-Handoff.md` §6 for the live status board). The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project
does not use SemVer yet (the Alpha is undeclared), so each entry is keyed to
its milestone tag.

## [M10.2 Phase 5] — verification and handoff (the closeout)

The final phase of the milestone: prove it, don't extend it. Delivered as
its own patch series on branch `m10.2-phase5` from `652f6cf` (master's
Phase 4 closeout). Scope: **Phase 5 only** — the full green gate, the
Xvfb + XTEST machine pass over the M10.2 paths, the owner's Phase 4
re-test verdict recorded, the A14 playtest scheduled, and the register/
doc closeout. A docs-only series (no code moved; the tree is code-
identical to Phase 4's) — exactly the plan's letter: "verification and
handoff".

- **the gate**: `cargo fmt --all -- --check` and clippy `-D warnings`
  clean; **546 dev / 541 release tests green** (the five should-panic
  invariant tests are debug-only); the three goldens **bit-identical** —
  demo `0xb6fff6659cfb7709` (seed 7, 300 ticks), flagship
  `0x6e9a18bd7c5f699f` (seed 7, 7200 ticks AI-vs-AI), content
  `0x9bc18c521107b262` (map id `0xd38136401ab02ff1`); the replay
  round-trip (A2) PASS (11 checkpoints); the headless client smoke PASS
  with its audio evidence line ("null fallback (no device)", cues
  counted).
- **the machine pass (Xvfb + XTEST, llvmpipe GL, no audio device — the
  null arm; the AI-Handoff §9 recipe, harness built locally as the prior
  phases did)**: five sessions, 29 checks, all green. S1 — a seed-7
  Player-vs-AI match driven end to end: menu → New Match → Start;
  drag-box selection of the 5 start entities; one right-click ground
  order (the summary's human command counter, `commands submitted 1`);
  a right-drag scroll with the 6 px threshold honored (an 85 % frame
  pixel diff and **no** extra order); the Esc ladder through
  clear-selection → pause menu (presented, 99.6 % diff) → Resume; the
  selection re-held at exit. S2 — a menu-only session whose New Match
  row was mouse-activated (the focused row's bright plate located from
  the screenshot; the lesson is now an AI-Handoff §9 gotcha), then
  Esc-back, Esc-quit. S3/S3b — the settings persistence pair: volume
  Left to `master_volume=0.95`, mute row activated (`muted=true`),
  Done saves; the relaunch prints "(loaded the file)" and the audio
  evidence line reports `[muted]`. S4 — the Esc-discard run: no file
  written.
- **the audio evidence lines**: every windowed run states its arm and
  the cues' fates — the S1 match's line read "null fallback (no
  device), cues fed/voiced/dropped 24/21/3 (5 client-side)": the
  machine counts, it never claims sound.
- **the owner's verdict (A-126, 2026-10-09)**: the Phase 4
  real-hardware audio re-test — **pass, clean**. The nine cues fire at
  their moments and sound distinct; volume changes loudness live, mute
  silences everything, both survive Done + relaunch; a big fight reads
  as a heartbeat, not noise. No incidents filed. The go for this
  closeout; DEBT-011 is now fully repaid (backend + ears).
- **A14 scheduling**: five first-time tester slots T1–T5 in
  `docs/PLAYTEST.md` §7 (three waves to 2026-10-31, the spectator half
  explicit, a focus steer per slot that keeps every session unaided);
  the audio probe stays armed as a regression guard; the declaration
  flip rule unchanged — five honest rows or a recorded finding.
- **registers & docs**: A-126 logged; DEBT-011's re-test half closed
  and DEBT-008 pointed at the A14 sessions; AI-Handoff (board
  COMPLETE, snapshot, next steps, the menu-targeting gotcha); README
  (badge "M10.2 complete", the closeout paragraph, `muted` joins the
  settings key list, the CI count catches up to 546/541); the PLAYTEST
  controls card speaks in the present tense (volume and mute are real
  and hardware-verified).

## [M10.2 Phase 4] — playtest-1 findings: audio

The last of the owner's four playtest-1 findings ("simple sounds that make
events feel real"), delivered as its own patch series on branch
`m10.2-phase4` from `1210cbd` (Phase 3's re-test came back a go — the
owner's Phase 4 direction is registered as A-119; DEBT-015's last deferred
scope is spent). Scope: **Phase 4 (audio) only**. The plan's exit bar:
one backend implementing `AudioSink` behind the existing seam, synthesized
sounds (no assets), the silent no-device fallback, master volume + mute in
settings, rate-limited cues, and the tests for all of it. Presentation-only
(`crates/client` + `crates/engine/src/audio.rs`); zero golden movement
(demo `0xb6fff6659cfb7709`, flagship `0x6e9a18bd7c5f699f`, content
`0x9bc18c521107b262` all bit-identical); 546 dev / 541 release tests green.

- **client (PLAN §4.1, ADR-0002, DEBT-011 repaid)**: rodio 0.22.2 joins
  the client as its only new external crate — `default-features = false,
  features = ["playback"]` (the decoder features and the noise/rand
  machinery stay off; every sound this pass makes is synthesized PCM).
  The allow-list amendment and the ADR (the owner's pre-choice A-100, the
  resolved version, the feature trim, the alternatives) landed with it.
- **client (PLAN §4.2, the sound module)**: `client/src/sound.rs` — the
  `CueBank` (the nine cues' mono 44100 Hz PCM, synthesized at startup by
  pure functions: deterministic, every buffer under a second, pairwise
  distinct by construction, soft-limited into the rail; no asset files),
  the `RateLimiter` (a per-cue 80 ms window — the first cue of a kind
  voices, repeats inside the window drop, each cue independent), and
  `Sound`, the two-arm sink behind the engine's `AudioSink` trait: the
  constructor probes the device once (a Linux `/dev/snd` pre-check keeps
  the no-device path from waking libasound's chatty config parser — the
  fallback is silent, A-124), a live device takes the rodio arm whose
  stream owner (`MixerDeviceSink`, 0.22's `OutputStream` successor) is
  boxed in the arm and lives for the App's whole lifetime (dropping it
  silences everything), and ANY failure — no device, Xvfb, CI — or a
  device lost mid-session (the stream's error callback flags it; one
  line, one degrade, never a panic, never a block) takes the null arm:
  the engine's `NullAudioSink` counting what would have played.
- **engine (PLAN §4.2, the seam's other half)**: `AudioCue` gains the
  three client-side cues — CommandAck, SelectionClick, UiClick — with
  `ALL`/`index`/`name` (the cue bank's layout), and `AudioSink` gains
  `on_cue` (the direct path). The three have no `cue_for` arm BY DESIGN
  (no sim event exists for them — the mapping is untouched); the null
  sink counts them on their own counter (the wiring oracle).
- **client (the wiring, thin and in `main.rs`)**: the step's events still
  feed the sink after the step (FD-9's order kept); a submitted order
  acknowledges on the direct path (A-120: submitted, not non-rejected —
  the sim's verdict surfaces separately through the refusal feedback;
  spectate's silence carries over); every user-driven selection change
  funnels through one `set_selection` gate that cues the membership
  change (A-122); every menu row activation — Enter, click, or the
  end-screen R shortcut — funnels through one `menu_activate` core that
  cues the UI click (A-121).
- **client + config (PLAN §4.4, the controls)**: master volume is live
  (the settings row drives the sink per A-115: changes apply immediately,
  Done persists, Esc leaves the run's value unsaved), and `muted` joins
  as the eighth key right after it (row 5; fullscreen/debug shift to
  rows 6/7, Done to 8; keyboard and mouse both reach it — the focus map
  and the ui.rs button-id tests pin the nine-row layout). Mute beats
  volume and never zeros it (A-125: unmuting restores the set loudness);
  a Phase 3 seven-key file parses with mute off (backward compatible).
- **honesty**: the machine cannot hear. Every machine run (CI, Xvfb) takes
  the null arm, and the evidence lines say exactly what audio did — the
  arm (active/fallback, `[muted]` when muted) and the cues' three fates
  (fed/voiced/dropped, plus the client-side count) — in the `--frames`
  summary and the headless smoke. "The sounds are good" is a claim only
  the owner's re-test can make.
- **verification**: 23 new tests (the bank's shape/distinctness/
  determinism, the limiter's window/independence/reset, the forced null
  arm + the oracle counter, the shared limiter across cue sources, the
  match reset, volume/mute precedence, the evidence line, the three
  wiring pins through a real `App`, the config backward compatibility and
  the shifted rows). Full gate in dev AND release; all three goldens
  bit-identical; the headless smoke constructs the real sink (the null
  arm's evidence under CI); the Xvfb machine pass ran a menu-only session,
  a 600-frame match (event cues counted), the volume/mute rows adjusted
  and persisted through Done + relaunch (`master_volume=0.95`,
  `muted=true`, the relaunch loads and reports `[muted]`), and an
  Esc-discard run that writes nothing. Registers: A-119..A-125, DEBT-011
  repaid, DEBT-015 closed (A14's ≥5-tester playtest gate is the owner's).

## [M10.2 Phase 3] — playtest-1 findings: menu and settings

The third of the owner's four playtest-1 findings, delivered as its own
patch series on branch `m10.2-phase3` (Phase 2's re-test came back a pass —
the owner's go-ahead for Phase 3 is A-107). Scope: **Phase 3 (menu and
settings) only** — Phase 4 (audio) stays gated on this phase's re-test. The
plan's exit bar: the game starts at the menu, a match can be started in each
mode, settings persist across runs, and tests cover the state machine and
the config parser (including corrupt files). Presentation/input only
(`crates/client`, plus two presentation-only `crates/engine` touches: the
public `DISTANCE_RANGE` and `RtsCamera::clamp_distance_to`); zero golden
movement (demo `0xb6fff6659cfb7709`, flagship `0x6e9a18bd7c5f699f`, content
`0x9bc18c521107b262` all bit-identical); 523 dev tests green.

- **client (PLAN §3.1, the state machine)**: `screens.rs` — the pure
  application state machine `MainMenu -> (Settings | NewMatch) -> InMatch ->
  (PauseMenu | EndScreen) -> MainMenu` with transitions returning
  (state, effect) pairs, the effects vocabulary the app layer interprets
  (StartMatch, RestartMatch, DropMatch, Pause/Unpause, SaveSettings,
  AdjustSetting, QuitApp), a wrapping focus model shared by keyboard and
  mouse, and the match modes (Player vs AI / AI vs AI spectate / Sandbox —
  P2 present but controller-less, A-112). 33 unit tests: every diagram
  edge, back-navigation, settings return-memory, focus movement, seed
  editing, the mode/setup/controller mapping.
- **client (PLAN §3.2/§3.3, the start flow)**: the host construction is
  deferred — `App` starts at the main menu over the (empty) world and
  `begin_match` builds the host only when Start is pressed (the A15 drop +
  reconstruct discipline; determinism unchanged). The New Match screen
  carries the mode, the seed (random by default from std's OS-entropy
  `RandomState`, never the sim's RNG — A-111; `--seed N` pre-fills), and
  the map from the content tree. Spectate silences the human order path at
  the orders layer (A-113). The render fix the menu-first world exposed:
  the entity draw now re-binds the camera group when no decals exist.
- **client (PLAN §3.4, settings)**: `config.rs` — a hand-written
  `key=value` file (no serde, no new crates; the allow-list is untouched),
  seven keys (edge_scroll, pan_speed, zoom_min, zoom_max, master_volume
  [Phase 4 placeholder], fullscreen, debug_overlay), per-key salvage on
  malformed values (A-108), range clamping, and std-env config-dir
  resolution with the no-HOME/APPDATA fallback (A-109, persistence
  silently disabled — never a panic). 14 unit tests including the
  corrupt-file cases and the round trip. DEBT-014 repaid: the edge-scroll
  toggle is the settings row (the Phase 1 flag loads from the file and
  flips live). The engine's `DISTANCE_RANGE` is public and the camera
  clamps user zoom limits through `clamp_distance_to`.
- **client (PLAN §3.5, the pause menu)**: Esc's exhausted rung opens the
  pause menu instead of quitting the application (the earlier rungs stay
  exactly as Phase 1 re-tested them; the rung test and both control cards
  carry the change). Resume / Settings / Restart / Quit to Menu, over the
  paused match; opening force-pauses through the existing `MatchHost`
  pause and Resume/Escape unpause (A-114); P stays a direct toggle. The
  end screen promotes from InMatch when the host reports an outcome and
  offers Rematch / Main Menu (R remains its shortcut).
- **client (PLAN §3.6, the menu UI)**: `ui::build_menu` renders every
  screen through the existing fontdue overlay pass — solid panels, rows
  with the label left and value right, the focused row brightened with the
  Phase 2 blue/orange accents, hit rects in focus-index order so the
  machine and the pixel geometry agree by construction. Every screen is
  keyboard AND mouse navigable; menus own the pointer and the keyboard
  (stale camera keys clear — a resumed match never pans on a ghost of a
  held key, A-116); nothing in a menu sends a sim command or touches a
  replay (a menu-only session records nothing).
- **evidence lines**: the windowed smoke summary reports the session
  honestly (matches started vs none live, the screen at exit, the seed);
  a live "match ended" line prints the resolution once with the seed.
- **Xvfb + XTEST machine half (DEBT-008 recipe)**: keyboard alone and
  mouse alone each navigate menu -> New Match -> Start (matches ongoing
  at tick 216/217); all three modes start and run; the pause menu opens
  by Esc, resumes, restarts (tick reset), and quits to menu; settings
  toggle, save on Done, and reload across runs (the config file and the
  startup source line verified); spectate at seed 7 resolved naturally
  ("player 0 wins (tick 10446, seed 7)" — matching the tools' headless
  reference), the end screen took the input, and R rematched (exit at
  tick 99 of the fresh match).
- **registers/docs**: A-107..A-118, DEBT-014 repaid, DEBT-015 narrowed to
  Phase 4 only, DEBT-017 (the one-map terrain-mesh renderer limitation),
  the PLAYTEST quickstart + controls card, the README quickstart + card,
  this CHANGELOG, and the AI-Handoff refresh.

## [M10.2 Phase 2] — playtest-1 findings: visual legibility

The second of the owner's four playtest-1 findings, delivered as its own
patch series on branch `m10.2-phase2` (Phase 1's re-test verdict came back
"pass, with notes" — A-101 — so this pass builds on it; the notes arrive
post-delivery by the owner's own account). Scope: **Phase 2 (visual
legibility) only** — Phases 3-4 stay gated on the owner's re-test. The
plan's exit bar: a stranger can identify the command center, a worker, and
a tank in a screenshot without a legend. Presentation/input only
(`crates/client`, `crates/engine`); zero golden movement (demo
`0xb6fff6659cfb7709`, flagship `0x6e9a18bd7c5f699f`, content
`0x9bc18c521107b262` all bit-identical); 465 dev / 460 release tests green.

- **client (PLAN §2.1, silhouettes)**: the single-tinted-box shapes are
  replaced by distinct multi-part silhouettes (`silhouette.rs`, pure, 10
  tests) — worker: round body + visible steel tool; rifleman: capsule body
  + thin rifle; raider: low buggy (chassis, wedge nose, cabin, roll bar);
  guardian: hull + dark treads + turret + steel barrel; command center:
  tall slab + corner tower capped in the team band; barracks: wide slab +
  door + flag pole flying the team color; supply depot: stacked crates;
  turret: pedestal + cap + raised barrel; ore node: four bright amber
  crystals. Mapping keyed by the bundle's kind *name* with the
  capability-shape fallback (Resource → cluster, Attack+Footprint →
  turret, Footprint → building, Move → unit, else plain box) — no
  presentation data touches `content/` (DEBT-016 logs the footprint
  coupling).
- **client (PLAN §2.2, team identification)**: P1 shifts off content-red
  onto orange `[0.95, 0.55, 0.15]` (the owner's colorblind-safe
  blue/orange pick, A-102 — a client-side mapping, content hash unmoved);
  every unit gets a team-color ground ring; every structure carries a
  team-band part (CC tower cap, barracks flag, depot/turret trim); the
  minimap dots re-tint to the same pair.
- **client (PLAN §2.3, selection & info)**: the selected entity draws a
  larger health bar (64×7 px vs 36×4) that shows even at full health; a
  hover tooltip near the cursor names any visible entity with an owner
  tag (yours/enemy/neutral), suppressed over the bottom bar; the
  single-selection info panel gains a production-queue summary (head item
  + percent + waiting count; `queue: idle` otherwise) while the queue
  strip keeps its cancel buttons.
- **client (PLAN §2.4, minimap)**: markers are now distinct per role —
  units one tile, buildings a solid 2×2 block (visibly larger), ore nodes
  a 3×3 amber diamond (color AND shape AND size).
- **engine (PLAN §2.5, ground contrast)**: terrain ground drops its green
  dominance (channel spread under 0.13) so entities pop; rock darkens
  from `[0.41, 0.38, 0.35]` to `[0.30, 0.27, 0.25]` so walls read as
  obstacles; the minimap's baked base inherits the calmer palette.
- **tests (PLAN §2.6)**: 25 new tests across `silhouette.rs` (unique mesh
  per kind, per-kind part/vertex counts, above-ground + inside-radius
  invariants, tone/team separability, fallback paths), `render.rs`
  (multi-part instance building, facing rotation at yaw 0/90°, ore amber
  team-independence, team rings, minimap markers), `feedback.rs` (the
  bigger selected bar), `orders.rs` (hover picks), and `ui.rs` (owner
  tags, tooltip placement, queue summary, the shared display-name
  lookup).

## [M10.2] — playtest-1 findings, Phase 1: the Generals ZH controls

The project owner's first human playtest of the windowed client (post-M10.1)
blocked the Alpha declaration (A14) on four findings: controls, visual
legibility, menus/settings, audio. `docs/PLAN-M10.2.md` is the milestone
spec; `docs/PLAYTEST.md` result #1 records the finding. This delivery is
**Phase 1 (Controls) only** — the owner scoped it that way (A-100, DEBT-015);
Phases 2-4 wait on their re-test verdict. Presentation/input only, zero
golden movement (demo `0xb6fff6659cfb7709`, flagship `0x6e9a18bd7c5f699f`,
content `0x9bc18c521107b262` all bit-identical), delivered as one
`git format-patch` series on the branch `m10.2` (never squashed).

- **client (PLAN §1.1, the audit)**: recorded in PLAYTEST result #1 — a
  plain left click never issues an order in the M10.1 code (context orders
  have been right-button-only since M9.1; the owner's "movement on the left
  button" traces to the armed-A path, the command card, or the placement
  confirm). The audit also found the armed minimap attack-move branch was
  dead code (the right-press disarm preceded the check) — fixed in the
  state-machine rework below.
- **client (PLAN §1.2, the camera)**: the right button is a press/release
  state machine (`crates/client/src/input.rs`, pure) — under 6 px of
  press-to-release travel it is the command click, beyond it the
  Generals-style grab-and-drag map scroll and the release orders nothing.
  Ordering moved to the release (sub-threshold): the context order at the
  release position, the minimap order at the mapped point (attack-move when
  the gesture began armed — the fixed dead branch), UI buttons swallow,
  placement-cancel consumes the gesture. Middle-drag now rotates (it panned;
  0.005 rad/px, direction pinned to match E). Edge scrolling: the old 24 px
  binary zone became a 14 px band whose speed scales with the cursor's
  depth into it, on all four edges and corners — the owner's "only the very
  last pixel works" finding was the snap. A wired `edge_scroll_enabled`
  flag awaits Phase 3's settings toggle (DEBT-014).
- **client (PLAN §1.3, selection parity)**: shift+click toggles membership,
  shift+drag unions the box, double-click selects every visible unit of the
  clicked one's kind (same owner, fog-filtered view), Ctrl+digit assigns /
  digit recalls / double-tap centers the camera on the group's live center,
  S stops, A-then-left-click attack-moves (the armed flow, now with the
  crosshair cursor), Escape climbs one rung per press (armed command ->
  placement -> selection -> quit — it used to quit on the first press),
  Space jumps to the last death or the base, and a click on empty ground
  does nothing at all (A-091: no order, and no silent deselect either).
- **client (PLAN §1.3, cursor feedback)**: a small ground marker names the
  order the next right-click would issue — green Move, red Attack, amber
  Gather, blue rally, orange armed attack-move — resolved through the same
  context machinery that issues it; the command ping stays as the
  confirmation. Silence when nothing is orderable (A-098).
- **tests**: 26 new client tests (23 in `input.rs`: the state machine's
  press/move/release paths, the band math at real window sizes, direction
  pinning for the grab-scroll and the rotate at yaw 0 and 90, the selection
  parity, the double-tap centroid, the Escape ladder, the hint table; 3 in
  `feedback.rs`: the marker's colors, quads, silences). 440 dev / 435
  release green. The windowed input paths re-verified under Xvfb + XTEST.
- **registers**: A-091..A-100 (the interpretations and the owner's delivery
  answers); DEBT-014 (the edge-scroll toggle's missing UI), DEBT-015
  (Phases 2-4 deferred, gated on the owner re-test); DEBT-008's next human
  pass is the Phase 1 re-test. README + PLAYTEST controls cards rewritten
  as the Generals ZH card.

## [M10.1] — pre-declaration hardening & A14 enablement

The pass between M10 and the Alpha declaration: the one open gate is A14 (the
human playtest), and it lacked its enabling machinery. This pass builds it,
repays the one debt whose trigger had fired (DEBT-001), hardens the nightly
soak, and closes out the registers — leaving the project one human playtest
away from the declaration. Not new gameplay, content, balance, audio,
graphics, multiplayer, or refactoring (plan §0 governs).

- **fx (DEBT-001 repaid — the pass's single hash-moving commit)**:
  `XxHash64` (the reference XXH64 algorithm, written in-repo like PCG32 —
  the dependency law forbids `twox-hash`) is now the canonical hasher behind
  the same incremental `write_*`/`finish` surface; the state hash, the
  fixture/content hashes, and every checkpoint digest moved in one commit.
  Golden-tested against reference vectors (stripe boundaries 31/32/33, a
  nonzero seed) plus a chunking-invariance property. `STATE_ENCODING_VERSION`
  and the replay `format_version` unchanged (byte layouts identical —
  A-087; no replay files existed in the wild). `Fnv1a64` remains as the
  replay *file* checksum. Every pinned golden re-pinned with its written
  reason (one commit, no split: demo `0xb6fff6659cfb7709`, flagship
  `0x6e9a18bd7c5f699f`, content hash `0x9bc18c521107b262` / map id
  `0xd38136401ab02ff1`).
- **client (plan §6.5/§11.6)**: the windowed player can now participate in
  the reproducibility promise — `--seed <u64>` (default 7) names the match,
  `--record <path>` writes a replay at match end / clean exit that
  `tools replay-verify` accepts, and **F8** is the §11.6 deterministic
  bug-report dump: a `pandemonium-report-<seed>-tick<tick>.pdrp` replay plus
  a sidecar `…-info.txt` (seed, tick, content identity, player slot, frame
  count, selection size, the controls line in effect — no wall-clock), both
  filenames printed, an on-screen ping cue, works while paused. The assembly
  lives in the new `crates/client/src/report.rs` as pure functions with
  tests (the A2 re-sim pattern at the client seam, pause validity, sidecar
  determinism); parser tests cover `--seed`/`--record`. Verified windowed
  under Xvfb + XTEST (DEBT-008's recipe): seeds diverge, records verify,
  two F8 dumps (one mid-run, one paused) replay-verify PASS and the `.pdrp`
  is byte-identical across same-tick presses. Zero golden movement —
  client-only.
- **docs**: `docs/PLAYTEST.md` — the A14 instrument (per-OS quickstart, the
  controls card, the unaided-loop checklist, spectator legibility, the
  debt-arming probes, the F8 procedure, the five-row results table, the
  pass bar).
- **ci**: the nightly soak is sharded 10×100 (seeds disjoint by construction
  — the arithmetic proven in comments), `timeout-minutes` on every job
  (90/shard, 30 smoke, 30 bench), every shard tees + uploads its summary,
  and an aggregate job concatenates the evidence, sums win rates, and fails
  on any nonzero `crashed` count. Zero Rust changes.
- **registers**: DEBT-001 repaid; DEBT-013 (client monoliths) logged with
  the post-alpha split plan; DEBT-011/012 triggers sharpened to name their
  PLAYTEST.md probes; A-087..A-090 logged (the no-bump swap, the F8 dump
  format + sidecar scope, the `--record` per-segment semantics, the shard
  shape); AI-Handoff §2/§4/§6 refreshed; ARCHITECTURE carries the
  post-alpha refactor map.

414 dev / 409 release tests green (20 new: 8 fx, 12 client); fmt + clippy
clean; replay round-trip and `content-validate` re-run PASS at the new
identity; a 4-match soak spot-check clean.

## [M10] — stabilization & declaration (incl. the pre-M10 cleanup)

The pre-M10 cleanup review pass (formerly `[Unreleased]`) shipped as part of
the M10 series. No frozen decision changed; no golden hash moved at the time
(the M10.1 hasher swap above is what moved them); no dependency bumped.

- **ci**: `Swatinem/rust-cache@v2` added to every CI job, keyed on
  `Cargo.lock` — cuts typical CI time roughly in half on a cache hit.
- **ci**: a separate `.github/workflows/audit.yml` runs `cargo audit` on
  every `Cargo.lock` change and nightly.
- **docs**: refreshed `docs/ARCHITECTURE.md`'s status header from M8 to
  M9.1; added `tests/alpha_loop.rs` to its test list.
- **docs**: per-constant doc comments on the scripted-opponent tuning
  block in `crates/ai/src/scripted.rs`.
- **community**: added `CHANGELOG.md`, `SECURITY.md`, `.editorconfig`,
  `.github/PULL_REQUEST_TEMPLATE.md`, and
  `.github/ISSUE_TEMPLATE/bug_report.md`.
- **refactor**: `crates/content/src/bundle.rs` — `map_passability` and
  `map_buildability` now share one helper; output bytes unchanged.
- **fix**: `crates/engine/src/host.rs` — the A15 "checkpoint trail"
  assertion was a no-op (`assert_eq!(x.len(), x.len())` plus an empty
  loop); replaced with a real `assert_eq!(oa.hashes, ob.hashes)`.
- **test**: `crates/fx/tests/properties.rs` — added property tests for
  `Vec2Fx::dist` ↔ `(a - b).len()`, `Fnv1a64` integer-write LE encoding,
  and `Fx::abs`/`Fx::square` contracts.

The M10 declaration itself: the A1–A15 acceptance sweep recorded with
evidence in `docs/ALPHA_DECLARATION.md` — thirteen criteria pass
automated, A14 recorded as an honest finding pending the human playtest;
the 1000-match A7 tier wired nightly; §15 bench baselines recorded
(0.25 ms avg, 2.22 ms p99, 4060 t/s — all budgets met); the M10
playability/visual series (command card, fog rendering, minimap,
lighting, silhouettes, shadows, death fades) landed with zero golden
movement.

## [M9.1] — input hotfix (DEBT-008 human pass findings)

The first human playtest of the windowed client (the project owner, on a
real display) reported every input-layer defect the machine pass could
not see. The simulation itself was machine-verified through M9 and
untouched.

- Camera pan axes had been mirrored since M3 — D panned left, A right, W
  retreated, S advanced. Corrected, with direction-pinning tests at yaw 0
  and 90°.
- Mouse could not move the camera at all — plan §11.3's edge pan and
  zoom-toward-cursor were unimplemented, and the wheel direction was
  inverted. All wired, with frame-rate-independent pan speed.
- Right-click only ever issued `Move` — the client had no path to order an
  attack. Added `client::orders` context resolution: enemy → Attack,
  node → Gather (for workers), ground → Move.
- 'A' fired attack-move on key-down *and* on every OS key repeat while
  also being a pan key. Now 'A' arms attack-move for the next left-click,
  with key-repeat guards; Esc/right-click cancels.
- Rejected orders were silent — `CommandRejected` events were dropped.
  Now refused orders flash a red square at the click point plus a HUD
  reason line; the smoke run pins the wiring with a deliberately invalid
  order.
- The opening camera framed the map center at 58 tiles out — the starting
  force read as specks. Now focused on the player's start at 26 tiles,
  re-framed on restart, with map-bounds clamping.
- `frames_presented` only advanced in `--frames` verification mode, so
  M9's flashes and pings never expired and accumulated forever in normal
  play. The feedback clock now advances every presented frame.

Zero golden movement — the hotfix is 100% presentation/input. 19 new
tests (7 camera, 8 context-resolution, 4 refusal/bracket). DEBT-008
narrowed to the human *re-verification* of the fixed build.

## [M9] — Alpha content & feel pass

The M8 gap closed: the loop resolves, the AI presses, and the client
feels like a game.

- **fix(sim)**: a chase order whose commanded target died no longer
  froze both armies — combat stage 1 pops it now (the "frozen-army"
  bug that kept AI matches from resolving). Two unit tests pin the
  behavior.
- **ai**: the wave machine re-marches on the pressure cadence while a
  live wave is out (a wave never parks) and focuses the sighted enemy
  command center. Waves of ten / army cap sixteen — matches resolve
  in 9.1k–23.3k ticks across a 32-seed sweep.
- **client**: the feel pass — hit flashes (entities flash toward hot
  white when hit), health bars over damaged units, command-acknowledgment
  pings at the clicked ground.
- **engine**: the `AudioSink` seam — a pure event → cue mapping, a null
  counter sink, the placeholder cue set (plan §11.5). No audible backend
  (DEBT-011).
- **test**: `tests/alpha_loop.rs` — the M9 exit suite: resolution within
  the fifteen-minute budget across seeds, end-to-end resolution
  determinism, and the windowed host's vs-AI loop closing naturally.

The demo hash is untouched (its lone Attack command is gate-refused on
the trivial world, so the chase-order pop never fires there). The
AI-vs-AI flagship re-pinned twice with written reasons (the chase-order
pop, then the AI tuning).

## [M8] — Match rules & full loop (P5)

Stage 10 (plan §6.3.10) lands: defeat = zero owned Footprint structures
OR resigned; victory = one survivor; `MatchEnded` fires exactly once
(idempotent — `Sim::outcome` caches the result).

- **sim**: `match_rules.rs` — the defeat check only fires in
  structure-bearing matches (A-067) so the M1 spine-test fixture and the
  M3 headless smoke stay green. The outcome is derived state (A-066: not
  part of the canonical hash) — the M7 goldens stay green by
  construction. No `STATE_ENCODING_VERSION` bump.
- **engine**: `MatchHost` extended with optional AI controllers
  (`with_controllers`), a command log (`log()`), and the match outcome
  (`outcome()` / `is_finished()`).
- **client**: hosts the vs-AI opponent through the same seam, renders the
  end-screen panel (VICTORY/DEFEAT/MUTUAL DESTRUCTION), supports restart
  (R), control groups (1-9), and Stop/AttackMove hotkeys (S/A).
- **tools**: the `--p1 ai --p2 ai` runner prints a `match ended:` line.
- **test**: `tests/match_rules.rs` — A15 restart cleanliness (two fresh
  hosts from the same seed → identical hashes; with AI controllers →
  identical logs too); stage 10 is hash-neutral (the M7 golden holds
  with match rules active).

## [M7] — AI through commands (P4)

The scripted Alpha opponent. Parity is structural, not a policy.

- **ai**: the `Controller` trait (plan §9.6) + `ScriptedController` —
  workers → depot → barracks → mixed army; idle-worker gather
  management; attack waves on size + timer; base defense; pending /
  build bookkeeping verified by sight (never a rejection channel).
- **sim**: `run_command_log` — the one canonical headless re-simulation
  of a command log. Tools' recorder/verifier and the acceptance suites
  all drive through it (DEBT-005 repaid).
- **engine**: `AiMatchHost` — controllers hosted on the tick boundary,
  ascending slot order, every fed command recorded (rejections
  included). Plus `alpha_plan`/`alpha_controller` (capability-shaped
  kind resolution, costs through the Ore seam, ring-scanned build
  ground clear of static claims and node doorsteps).
- **test**: `tests/ai.rs` — A5 (structural audits + ledger identity),
  A11 (mirrored battery + label-blindness + proptest fuzz), AI-vs-AI
  completion + determinism, the golden hash, and the log-alone replay.

## [M6] — Combat & vision (P2)

The immediate-hit attack pipeline and the three-state fog model.

- **sim**: `combat.rs` — acquire → validate → wind_up → hit → mitigate
  → apply → death-via-stage-8 → credit. Cooldowns in ticks; squared-
  distance range checks; acquisition priority (commanded > units-
  attacking-friendly > other units > structures; ties to lower id).
- **sim**: `vision.rs` — per-player per-tile Hidden/Explored/Visible,
  maintained incrementally each tick. `FogState` is derived (NOT hashed
  — A-059). DEBT-004 repaid.
- **sim**: Attack/AttackMove/Stop semantics; the turret kind (no Move,
  Attack+Footprint). DEBT-006/007/010 retired. Encoding v4.
- **content**: the seam maps `Attack` into `CapTemplate::Attack` and
  the runtime `CapabilityData::Attack` block.
- **test**: `tests/combat.rs` (composition + position matter, bit-
  identical run-to-run, turret, A12 combat invariants, legibility) and
  `tests/vision.rs` (A10 fog integrity, targeting both directions,
  three-state transition, FD-8).

## [M5] — Economy & production (P3)

The worker gather loop, production queues, and construction lifecycle.

- **sim**: `economy.rs` (travel → gather timer → extraction capped by
  capacity and node remaining → carry → deposit → repeat; depletion
  removes nodes with same-tick auto-seek; ghost BuildAt orders
  hygiene-cleaned) and `production.rs` (ordered queues, costs paid on
  enqueue and refunded verbatim on cancel, only the head progresses,
  completed items hold at the queue front until population headroom
  returns, ring-tile spawns with rally orders, construction with a
  committed builder completing into Active).
- **sim**: `invariants.rs` — the A12 checker (debug assertions over the
  whole state at the end of every step in debug builds).
- **content**: the world definition carries the buildability grid, the
  faction production lists, the base population cap, and each kind's
  economy stats.
- **test**: `tests/economy.rs` — divergent scripted openings produce
  measurably different timelines; scripted-match determinism; the A12
  economy soak (3 openings × 2 seeds × 900 ticks of both-player soak,
  every invariant held — caught and fixed a real spawn-position bug).

## [M4] — Movement (P1)

The three-layer movement contract: nav grid + A* with deterministic
tie-breaks, waypoint steering with a terrain guard, spatial-hash
push-apart, stuck detection with `MoveFailed`. DEBT-003 repaid.

- **sim**: `nav.rs` (NavGrid + per-tile static-body claim count;
  8-connected A* with no corner cutting, integer milli-tile costs, the
  octile heuristic, the (f, h, tile) total-order heap key) and
  `movement.rs` (the three layers).
- **test**: `tests/movement.rs` — 50 units respond within 2 ticks under
  spam-clicked orders; no permanent stuck units (every order resolves —
  arrival, crowded arrival, or `MoveFailed`); the real-map detour
  (4 workers cross Crossroads around the rock walls, never on blocked
  terrain); push-apart separates stacks and never pushes into terrain.

## [M3] — Engine shell

The windowed 3D client. ADR-0001 (3D presentation over the 2D logical
ground plane) accepted.

- **engine**: `FixedTimestep` (30 Hz, 5-tick catch-up cap), `Interpolator`
  (snapshot blending, spawn/death handling, Q16.16 ↔ ground-plane
  boundary conversions), `MatchHost` (owns `Sim` privately; `submit` +
  `advance` are the only mutation paths), `RtsCamera` (perspective
  orbit, ground picking, box select), terrain mesh, `Renderer` trait +
  `NullRenderer`.
- **client**: winit 0.30 + wgpu 26 windowed 3D renderer (depth buffer,
  terrain mesh with heightmap displacement, instanced placeholder entity
  boxes), selection + right-click Move, fontdue text atlas + HUD/debug
  overlay, `--frames N` windowed smoke, headless fallback for CI.
- **test**: windowed path machine-verified on Xvfb + llvmpipe (selection
  + Move commands provably work). HUD/overlay slice landed (DEBT-009
  repaid). The human visual pass remains open as DEBT-008.

## [M2] — Content pipeline

RON schemas, versioned loaders, validators, content hash.

- **content**: strict RON schema with versioned loaders and forward
  migration (map v1 → v2), precise validators at file and placement
  level, `ContentBundle` with canonical content hash and map id,
  `world()` seam producing the plain `TrivialWorld` the simulation
  receives.
- **tools**: `content-validate` subcommand.
- **test**: `tests/content_pipeline.rs` — §10.4 stat pin, loaded-bundle
  determinism, the A3 add-a-unit scaffold (data-only load + spawn +
  Move; the §10.6 built-and-fights extension is DEBT-007 for M5/M6).

## [M1] — Simulation core

The tick pipeline, entity and capability stores, the command gate, the
state hash, the replay codec, and the determinism proofs.

- **fx**: Q16.16 fixed-point math with 64-bit intermediates, round-
  toward-zero and saturating contracts, exact integer square root over
  the full u64 range, PCG32 RNG with canonical seeding, FNV-1a 64-bit
  hashing — all property-tested.
- **sim_api**: the boundary vocabulary (Command, Event, Snapshot,
  PlayerView, Reject, MatchSetup).
- **sim**: `step()` runs plan §6.3's eleven stages; entity + capability
  stores kept in ascending-ID order by construction; the shared command
  gate (stable (issuer, seq) sort, tick check, per-kind existence /
  ownership / capability / target checks); canonical little-endian
  state hash through `fx::Fnv1a64`. Snapshots and `PlayerView`s project
  entities ascending by id.
- **replay**: a checksummed canonical little-endian codec (record, load,
  validate) with typed errors. No serde — the dependency law forbids it
  here.
- **test**: `tests/determinism.rs` (A1/A2 with pinned golden hashes),
  `tests/architecture_law.rs` (the dependency graph + the source-level
  determinism bans).

## [M0] — Skeleton & guardrails

The workspace, the toolchain pin, the guardrails, and the `fx` crate.

- Workspace (virtual `[workspace]`, resolver 2), shared package
  metadata, `rust-toolchain.toml` pinning 1.98.1.
- `clippy.toml` disallowing unordered-map types workspace-wide.
- CI authored: fmt, clippy `-D warnings`, tests × {Linux, Windows,
  macOS} × {dev, release}, plus the replay round-trip.
- `tests/architecture_law.rs` parses every member manifest on every test
  run, checks the internal edge allow-list and the per-crate external
  allow-list, fails on the forbidden-crate list, and scans the
  determinism crates' sources for banned types. Breaking the
  architecture is a build failure, not a conversation.
