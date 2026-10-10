# ADR-0100: The client app shell — screen machine, settings format, and the monolith split

- **Date:** 2026-10-10
- **Status:** proposed (the PM decides)
- **Frozen decision affected:** none directly — part (b) *conditionally* affects the
  plan §3.2 / §4 dependency law (the client's external allow-list) if its option 2 is
  taken. Parts (a) and (c) are design records inside the existing architecture law,
  recorded as an ADR because the brief (A-001, PRODUCT PHASE) orders the whole shell
  proposal here and because `docs/adr/README.md` is where the PM reads proposals.
- **Proposed by:** Agent A (brief A-001; the evidence is
  `docs/product/GAP_ANALYSIS.md`, same date, same commit)

## Context

The standing orders' PRODUCT PHASE amendment suspended plan §0's "foundation, not a
game" clause for product briefs while keeping the architecture rules binding: the UI
never mutates the sim, no floats below the boundary, content stays data. R1 needs the
client to become a product surface, and three questions have to be answered before
that work starts:

1. **What is the app shell?** M10.2 Phase 3 landed a real one by milestone necessity
   (`crates/client/src/screens.rs:1-18`):
   `MainMenu -> (Settings | NewMatch) -> InMatch -> (PauseMenu | EndScreen) -> MainMenu`,
   pure data and transitions with an `Effect` vocabulary, no winit types, no sim
   types. It works — it is also undocumented as a *shape*, and the R1 gap list
   (`GAP_ANALYSIS.md` §3, §12, §10) adds screens to it: a Controls reference screen,
   a visible error surface, and quit/confirm rungs.

2. **How do settings persist as they grow?** The current format is a hand-written
   `key=value` file, deliberately serde-free (`crates/client/src/config.rs:1-9`):
   eight keys, per-key salvage, range clamps, deterministic bytes, per-OS paths —
   with a wall of unit tests behind it (`config.rs:319-593`). The dependency law
   (`plan §3.2`, machine-encoded in `tests/architecture_law.rs:139-154`) allows
   serde/ron **only** in `pandemonium-content`; the client's external allow-list has
   no serialization crate.

3. **How does the client stop being two monoliths?** DEBT-013 records the risk;
   the live sizes at `23012bf` are `main.rs` 3231 lines, `render.rs` 2698, `ui.rs`
   1461, `screens.rs` 1167 (the register's 2473/1782 are pre-M10.1 and stale). The
   intended module boundaries already exist as guidance
   (`docs/ARCHITECTURE.md` "Post-alpha refactor map": `render/` split by pipeline,
   `app/`, `input/`, `screens/`), with the seam to preserve stated outright:
   `MatchHost::submit`/`advance` stay the only mutation paths and `report.rs` stays
   the pure F8/`--record` seam. The register's own trigger is "the post-Alpha
   renderer/UI rewrite (plan §19)" — but the product phase needs faster iteration on
   exactly these files *before* any rewrite, and the gap analysis' rows 5–10 all
   land in the monoliths.

## Proposal

### (a) The screen state machine

Formalize the current machine as the shell, with the R1 additions:

```text
Title (MainMenu) ── New Match ──► InMatch ── outcome ──► Results (EndScreen)
   │  │                            │  ▲                    │
   │  └──► Settings ──(return_to)──┘  ├──► PauseMenu ──────┘
   │            ▲   │                 │      │  ├─ Resume
   ├──► Controls │   └─(from pause too)│      │  ├─ Settings (return_to: pause)
   │             │                     │      │  ├─ Restart (A15 drop+reconstruct)
   ▼             ▼                     ▼      │  └─ Quit to Menu (confirm rung)
 Error surface (startup failures:      └──────┘  Esc ladder unchanged
 content, adapter, display)
```

Rules that keep it inside the architecture law:

- The machine stays **pure** — `screens.rs` continues to hold no winit, no window,
  no sim types; transitions return `(state, effect)` pairs the app layer executes
  (the existing discipline, `screens.rs:5-11`). The UI never mutates the sim; a
  screen's effects speak only `MatchHost::submit` / `set_paused` / drop-reconstruct.
- **Naming:** keep `MainMenu`/`EndScreen` as the code names (renames are churn with
  no behavior); "Title"/"Results" are the doc-level vocabulary. New states:
  `Controls` (reached from Title and PauseMenu; renders the standing
  `CONTROLS_LINE` card plus the camera/mouse sections as rows — data that already
  exists as consts), and a startup **error surface** that shows the anyhow chain
  (content/adapter/display) in the window when one exists, and falls back to the
  honest console line when none does (the current headless path,
  `main.rs:109-120`, stays).
- **Quit/confirm:** the pause menu's "Quit to Menu" gains a confirm rung; Esc at
  Title quits directly (nothing is live at the menu — unchanged).
- **Version row:** Title and Results display the build identity (tag or
  `CARGO_PKG_VERSION` + short commit hash), feeding the F8 sidecar too.

### (b) Settings persistence under the dependency law

**Option 1 — keep the hand-rolled `key=value` file (recommended for R1).**
Zero law change; the format is tested (round-trip, salvage, clamps, per-OS paths —
`config.rs:319-593`) and already survived one key addition (`muted`, Phase 4) with a
backward-compatibility test (`config.rs:484-505`). Growth path: keep
`SETTING_KEYS` as the single registry, add a `# v2` style header comment when a key
*renames* (never silently), and rely on unknown-key-ignoring for additions (already
the parse behavior). Cost: every new key is hand-written parse+clamp+step code —
roughly fifteen tested lines per scalar today, acceptable to ~15 keys.

**Option 2 — amend the dependency law to allow `serde` (+ a data format) in the
client.** The law's *purpose* is protecting the sim: determinism, no floats, no I/O
below the boundary (FD-5/FD-6). The client sits above that boundary — serde in the
client threatens nothing the law actually protects, and ADR-0002 (rodio) already
established the precedent of ADR'd client-side additions to plan §3.2's table. What
it costs is different: the allow-list test changes (`architecture_law.rs` line ~139),
plan §3.2's table gains a row, and every future "just one more crate" conversation
gets a cited precedent. Pay it only when the *shape* of settings demands it —
**trigger: key rebinding** (a nested map of bindings is miserable in `key=value`),
or a settings schema past ~20 keys with structure.

**Option 3 — reuse `content`'s RON pipeline for settings.** Rejected: settings are
application state, not game data; routing them through the content crate's loaders
blurs FD-4's data boundary (content = things the *simulation* consumes) and couples
app preferences to content versioning/migration. The dependency direction would be
legal (client already depends on content) but the layering is wrong.

**Recommendation: Option 1 for R1, Option 2 held at the rebinding trigger.** No ADR
acceptance is required to keep Option 1 — it is the status quo; this section exists
so the PM can pre-approve Option 2's trigger now and spare a future brief the
round-trip.

### (c) The DEBT-013 split — a behaviour-preserving step plan

Target module map (the ARCHITECTURE.md refactor map, made committable):

- `render/` — `pipeline/` (device, surface, common bind groups), `terrain/`
  (mesh + fog decal), `entities/` (instanced boxes, silhouettes, shadows, fades),
  `overlay/` (brackets, health bars, pings, refusal squares), `minimap/`.
- `app/` — the `App` struct, match hosting + restart, checkpoint trail,
  `--record`/F8 wiring (`report.rs` stays the pure seam).
- `input/` — winit event translation: camera, selection, context orders,
  placement, control groups, hotkeys.
- `screens/` + `ui.rs` stay (already pure / already separate).

**Steps (one commit each, ~8 commits, each independently green):**

1. Extract `render/` module dir with `mod.rs` re-exporting the current `Renderer`
   surface *unchanged* — move-only, no signature edits.
2–5. Move `pipeline/`, `terrain/`, `entities/`, `overlay/`+`minimap/` out of the
   renderer one commit each, in that order (each is self-contained state + its
   draw pass; the decal stream ordering gotcha in AI-Handoff §9 stays intact by
   keeping the shared buffer in `pipeline/`).
6. Extract `app/` (App struct + hosting + evidence lines), leaving `main.rs` as
   the winit bootstrap.
7. Extract `input/` (event translation + `RightButton` machine + orders routing).
8. Registers: DEBT-013 repaid; ARCHITECTURE.md refactor map marked built.

**Verification per step — because rendering cannot be golden-hashed:**

- **The hard gate stays hard:** `cargo fmt --check`, clippy `-D warnings`, and the
  full suite in dev **and** release after every step (553/548 at `23012bf`; the
  sim/engine layers remain pinned by their own tests and the three goldens, which
  this work cannot move — presentation only).
- **Behaviour pins that already exist and must stay green unedited:** the client's
  181 unit tests (input state machine, screen transitions and focus rules, config
  parse/round-trip, sound counting, report determinism). Path-only import changes
  are allowed; assertion edits are not — a split step that needs to weaken a test
  is the stop condition.
- **The machine-visible surface:** the `--frames` windowed smoke's evidence lines
  (frames presented, screen name at exit, the audio arm line, cues fed/voiced/dropped,
  the content hash — the same contract CI and the Phase 5 machine pass read) must be
  **line-identical** pre/post each step, captured under the Xvfb+llvmpipe recipe.
- **The visual surface, honestly bounded:** capture the three reference frames
  (menu, new-match, match — the `docs/product/screenshots/` trio) pre/post each
  step under identical env and frame budget. Rendering is time-parameterized
  (interpolation alpha, frame-delta camera motion), so pixel-identity is *not* a
  sound gate — the rule is: structural comparison (window geometry, panel rects,
  row pitch — the Phase 5 harness already knows how to find the 30 px focused-row
  plate at 38 px pitch) plus a triaged diff for anything else, and the owner's eyes
  on the step-8 closeout. This is the same honesty split the project already runs:
  machines prove wiring and counting; humans prove look.
- **Rollback:** any step reverts as one commit. No step lands with a red gate or a
  changed evidence line.

## Evidence

- Baseline green gate and goldens at `23012bf`: `GAP_ANALYSIS.md` Method (this
  sandbox, 2026-10-10): 553/548, three goldens bit-identical.
- Windowed smoke evidence lines and the three reference captures: same run
  (`docs/product/screenshots/`, **observed**).
- The current shell's purity and test wall: `screens.rs:1-18`, client 181 unit
  tests (dev profile count at `23012bf`).
- The settings format's test wall: `config.rs:319-593` (round-trip, per-key
  salvage, clamps, per-OS paths, backward compat, deterministic ASCII bytes).
- Monolith sizes: `wc -l` at `23012bf` — `main.rs` 3231, `render.rs` 2698,
  `ui.rs` 1461, `screens.rs` 1167; DEBT-013 and the ARCHITECTURE.md refactor map
  as cited above.
- The dependency law's machine encoding: `tests/architecture_law.rs:119-162`
  (allow-lists), the client's current list at lines 139-154.

## Consequences

- **Cheaper:** every R1 row in the gap analysis' top ten that touches the client
  (stats panel, controls screen, error surface, auto-pause, UI scale) lands in a
  module sized for review instead of a 3k-line monolith; the app-shell rule set
  (pure machine, effect vocabulary, no sim mutation) gets one canonical doc.
- **More expensive:** ~8 commits of pure motion with a full both-profile gate each
  (hours of CI, no feature value delivered mid-sequence); a settings-law amendment
  (part b, option 2) spends precedent from the allow-list's clean letter.
- **Nothing breaks if rejected:** each part is independently adoptable — the
  machine exists, the format exists, the split is a register trigger away. The ADR
  exists so R1 briefs cite one shape instead of re-deriving three.
- **Goldens and replays:** unaffected by all three parts (presentation-only);
  the invariant is restated as a stop condition, not an afterthought.

## Alternatives considered

- **Do nothing until the plan §19 post-Alpha rewrite.** The letter of DEBT-013's
  trigger — but the product phase lands R1 features inside these files *first*,
  and every row pays the monolith tax in review cost and regression risk. The
  rewrite stays scheduled; the split makes the interim cheap without spending the
  rewrite's budget.
- **Big-bang split in one PR.** Rejected: unverifiable-by-review motion at 6k
  lines, and it forfeits the per-step evidence-line gate that makes the move
  auditable at all.
- **Part (b) Option 3 (content-pipeline settings).** Rejected in-place above:
  layering, not legality, is the fault.
- **Rename screens to the doc vocabulary.** Rejected: churn without behavior;
  the code names stay, the docs translate.

## Decision

*(Pending — the PM decides; per this ADR's own framing, part (b) Option 1 needs no
decision, Option 2's trigger may be pre-approved, and parts (a) and (c) are
adopted or deferred as the R1 briefs demand.)*
