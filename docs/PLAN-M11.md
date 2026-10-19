# Pandemonium — M11: Zero Hour Controls Parity

> **TASK ID:** A-013 (proposed; branch `a/A-013-zh-controls`)
> **Authority:** this file is the specification for milestone M11. It extends, and does not
> replace, `plan.md` and `AI-Handoff.md`. The owner issues it under STANDING_ORDERS v1.2 with
> the PRODUCT PHASE amendment in force.
> **One sentence:** make the shipped control scheme *actually* the Command & Conquer:
> Generals – Zero Hour scheme the README claims it already is — then make the documentation
> tell the exact truth about every binding.

---

## 0. Read first, and the rules that bind this work

Read, in order: `AI-Handoff.md` (all of it, especially §9 "Client and presentation"
gotchas), `plan.md` §0 and §2, `docs/pm/STANDING_ORDERS.md`, `docs/PLAN-M10.2.md` §1
(the previous controls pass — much of its plumbing survives, its key map does not), and
this file.

- **Presentation/input only, with one declared exception.** Everything lives in
  `crates/client` and `crates/engine` above the sim boundary — except Phase 6 (the
  force-fire sim seam), which touches `sim_api`/`sim`/`replay` and is explicitly
  authorized in §6 with its own safety case. **Zero golden-hash movement.** If any golden
  (demo `0xb6fff6659cfb7709`, flagship `0x6e9a18bd7c5f699f`, content `0x9bc18c521107b262`,
  map id `0xd38136401ab02ff1`) moves at any point: STOP and report, per STANDING_ORDERS §7.
- **Green gate on every commit:** `cargo fmt --all -- --check`; `cargo clippy --workspace
  --all-targets -- -D warnings`; `cargo test --workspace` (dev). Release-profile tests at
  the end of every phase. One logical change per commit; never squash. Branch
  `a/A-013-zh-controls` from master; rebase before reporting; never push to master.
- Every camera/feel change needs **direction-pinned tests** (world coordinates, yaw 0 and
  yaw 90) — the M9.1 lesson. Filter winit key repeats (`event.repeat`) before any hotkey
  logic. Frame-counted presentation state uses the presented-frame clock.
- No new external crates. No floats enter the sim. UI never mutates the sim; the client
  speaks only in `Command`s through `submit_order`.
- Registers: log guesses in `docs/ASSUMPTIONS.md` continuing from **A-131** (the A-lane
  range), simplifications in `docs/DEBT.md`. Shared docs (ASSUMPTIONS, DEBT, CHANGELOG,
  AI-Handoff) are edited only as §8 authorizes, in ONE final commit (MP-3).
- Register the owner's verdict in `docs/PLAYTEST.md` per §8. The A14 playtest is scheduled
  through 2026-10-31 — this brief changes the tester-facing card, so the docs pass (§8) is
  not cosmetic; it is what A14 testers will read.

## 1. Why this milestone exists

The README ("The Field Manual"), `docs/PLAYTEST.md` §2, and `docs/product/QUICKSTART.md`
all claim: *Controls follow Generals: Zero Hour muscle memory.* The owner's verdict: that
claim is false. The M10.2 pass fixed the mouse skeleton (left selects, right commands,
right-drag scrolls) — and then kept a camera/keyboard map that contradicts Zero Hour at
nearly every key:

| Zero Hour binding | What the client does today | Verdict |
|---|---|---|
| **A** = arm attack-move | A pans camera left *and* arms attack-move | conflict |
| **S** = stop | S pans camera down *and* stops | conflict |
| **W** = select air combat units | W pans camera up | conflict |
| **Q** = select all combat units on screen | Q rotates camera | conflict |
| **E** = select same-type units on screen | E rotates camera | conflict |
| **H** = jump to Command Center | unbound | missing |
| **Space** = jump to last radar event | jump to last death (subset) | partial |
| **X** = scatter, **G** = guard, **Alt+click** = waypoints, **Ctrl+click** = force-fire | unbound | missing |
| **1–0** = ten control groups | nine groups (Digit0 dead) | partial |
| **Alt+digit** = view group without selecting | unbound | missing |
| **Ctrl+←/→** cycle units, **Ctrl+↑/↓** cycle workers | unbound | missing |
| **Left-click empty ground** = deselect | deliberately does nothing (A-091) | reversed |
| **F1–F8** = camera bookmarks | F3 debug overlay, F8 bug report occupy the row | conflict |
| **F9** = toggle command bar, **F12** = screenshot, **Tab** = objectives | unbound | missing |
| **Numpad 4/6** rotate, **8/2** zoom, **5** reset | unbound | missing |
| WASD camera pan, Q/E camera rotate | present | **not Zero Hour** |

The goal is not "inspired by Zero Hour." The goal: **a Zero Hour player sits down and
every binding they try works, and nothing they try does something else.** Superset
bindings that ZH itself leaves unbound (middle-drag rotate, Ctrl+wheel pitch, P, R, `.`,
F10/F11 tools) may remain — they cannot break muscle memory. Anything ZH binds must do
exactly what ZH does, or the card must say it doesn't.

## 2. The Zero Hour reference (the target, sourced)

Authoritative list cross-confirmed identical in the [StrategyWiki Generals controls
page](https://strategywiki.org/wiki/Command_%26_Conquer:_Generals/Controls) and the
[Steam Community all-hotkeys guide](https://steamcommunity.com/sharedfiles/filedetails/?id=3479739045);
camera facts additionally per [GameReplays](https://www.gamereplays.org/community/index.php?showtopic=503172).
Where this project has no counterpart unit type, the key is *reserved* (§4 phase 2).

### Selection commands
| ZH input | ZH action | M11 target |
|---|---|---|
| Left click | select unit / pick menubar action | unchanged (exists) |
| Left drag | box select | unchanged (exists) |
| **Left click empty ground** | **deselect** | **restore** (reverses A-091) |
| Right click | perform active cursor action (move/attack/gather/rally) | unchanged (exists) |
| Shift + left click | **add** a unit to the selection | change: add-only, no toggle |
| Shift + drag | add the box's picks | unchanged (exists) |
| Double-click unit | select same-type units on screen | unchanged (exists) — keep |
| **E** | select same-type units on screen | add |
| **E double-tap** | select same-type across the whole map | add |
| **Q** | select all combat units on screen | add |
| **Q double-tap** | select all combat units on the map | add |
| **W** | select all air combat units on screen | reserve (no air content) |
| **Ctrl + 1–0** | define control group | extend to ten groups |
| **1–0** | select numbered group | extend to ten (Digit0) |
| **double-tap 1–0** | select and jump to group | extend to ten (exists for 1–9) |
| **Alt + 1–0** | view group without selecting (camera only) | add |
| **Ctrl + ←/→** | cycle previous/next unit | add |
| **Ctrl + ↑/↓** | cycle previous/next worker | add |
| **Space** | jump to last radar event | broaden (deaths **and** attacks on own forces) |
| **H** | jump to your Command Center | add |
| Ctrl + H | select hero | reserve (no hero) |

### Unit modes
| ZH input | ZH action | M11 target |
|---|---|---|
| **A** then left click | attack-move mode | unchanged (armed-A flow matches ZH) |
| **S** | stop | unchanged — but loses its camera duty |
| **G** then left click | guard mode | add (§4 phase 3 — no sim change needed) |
| **X** | scatter | add (client-side, §4 phase 3) |
| **Alt + left click(s)** | set waypoints for selected units | add (sim `Command.queue` already honored) |
| **Ctrl + left click** | force-fire mode | add — **Phase 6 sim seam** (§6) |
| Ctrl + F | set formation | out of scope (logged as proposal) |

### Camera
| ZH input | ZH action | M11 target |
|---|---|---|
| Arrow keys | scroll | unchanged (exists) |
| Screen edge | edge scroll | unchanged (exists, 14 px band) |
| **Hold right button + drag** | rapid scroll | keep the M10.2 grab-scroll (kept deviation, A-131) |
| Mouse wheel | zoom | unchanged (exists, toward-cursor superset) |
| **Numpad 4 / 6** | rotate camera left/right | add |
| **Numpad 8 / 2** | zoom in/out | add |
| **Numpad 5** | reset camera (default yaw/pitch/zoom) | add |
| **Ctrl + F1–F8** | set camera bookmark | add (relocates F3/F8 tools off the row) |
| **F1–F8** | jump to camera bookmark | add |

### Screen and pop-ups
| ZH input | ZH action | M11 target |
|---|---|---|
| **F9** | toggle the command bar | add (toggles the bottom HUD bar) |
| **Tab** | toggle mission objectives (single player) | add (toggles the objective HUD panel) |
| **Esc** | options screen | keep the M10.2 ladder; first press in a clean state opens the pause menu — document as the ZH "options" equivalent |
| **F12** | capture screenshot | stretch goal (§4 phase 5; DEBT fallback allowed) |
| WASD, Q, E rotate, D | *not camera keys in ZH* | remove camera duty (§4 phase 1) |

### Superset bindings kept (ZH leaves these unbound; they cannot break parity)
Middle-drag rotate · Ctrl+wheel pitch · P pause · `.` single-step · R rematch (post-end) ·
right-click Move/AttackMove on the minimap · zoom-toward-cursor behavior · F10 debug
overlay and F11 bug report (relocated from F3/F8).

### Out of scope
Formations (Ctrl+F) · beacon/cheat/chat (multiplayer: Ctrl+B, Ctrl+C, Enter, Backspace) ·
key remapping UI (GAP_ANALYSIS §11 keeps it "later") · any new content or AI behavior.

## 3. Current-state audit (verified at the base commit; re-verify before phase 1)

The implementing agent re-runs this audit at the base commit and records deltas in the
report. File:line references below are from master `5afd6ae`.

- **Dispatch:** one winit match in `crates/client/src/main.rs` `window_event`
  (main.rs:401–1023); per-key arms at main.rs:808–984; held keys + edge band drive the
  camera in `about_to_wait` (main.rs:1053–1120). Pure logic in
  `crates/client/src/input/mod.rs`; `App` action methods in `input/event.rs`; right-click
  context resolution in `input/orders.rs`.
- **Left button** (main.rs:541–642): UI button → minimap focus → placement → armed
  attack-move → drag start. Release: `CLICK_SLOP = 0.01` NDC drag/click split;
  `DOUBLE_CLICK_MS = 350` (mod.rs:202); pick own entity at `PICK_RADIUS = 0.05` NDC;
  empty ground = no-op (the A-091 reversal target).
- **Right button** (main.rs:650–725; FSM mod.rs:49–135): `RightButton` Idle/Pressed/
  Dragging; `RIGHT_DRAG_COMMAND_MAX_PX = 6.0` (mod.rs:30); press captures
  `right_press_armed` + ground anchor; drag = grab-scroll (`pan_world`); release ≤ 6 px =
  context order (UI swallow → minimap order → rally → `resolve_context_order`).
- **Middle** = drag-rotate (`MIDDLE_DRAG_RAD_PER_PX = 0.005`, mod.rs:42). **Wheel** =
  `zoom_toward` + settings clamp; Ctrl+wheel = pitch (`PITCH_PER_WHEEL_NOTCH`, main.rs:85).
- **Keys:** W/A/S/D + arrows pan (A arms attack-move at main.rs:813–826, S stops at
  main.rs:827–836 — both *while also panning*); Q/E rotate (`ROTATE_RAD_PER_SEC = TAU/6`);
  F3 debug; F8 bug report; P pause; `.` step; digits 1–9 groups
  (`CONTROL_GROUP_COUNT = 9`, main.rs:73; `DOUBLE_TAP_MS = 350` mod.rs:257; double-tap
  centers); R rematch (post-end); Esc ladder `CancelArmed → CancelPlacement →
  ClearSelection → OpenPauseMenu` (mod.rs:288–311); Space = last death else base
  (`jump_target`, mod.rs:316).
- **Edge scroll:** `edge_scroll_vector` (mod.rs:154–175), 14 px band, depth-scaled, all
  four edges + corners; gated by settings/focus/`takes_world_input()`.
- **Camera** (`crates/engine/src/camera.rs`): orbit `RtsCamera`; `PITCH_RANGE`
  (0.25, 1.45), `DISTANCE_RANGE` (8, 160); `pan` camera-relative, `pan_world`
  world-space; `focus`; `rotate`; `clamp_target` bounds clamp. **Known bug to fix:** the
  `App` camera is built once from the *default* map's dimensions (app/mod.rs:280–285) and
  never rebuilt per match — on other maps the clamp is the wrong size.
- **Orders:** single path `submit_order` (event.rs:107–129) always sends
  `Command::new` → `queue: false`. Context resolution orders.rs:59–105 (Attack → Gather
  (workers at nodes) → Move); rally on single selected producer (event.rs:54–63, 143–157).
  Client command vocabulary in use: Move, Attack, AttackMove, Stop, Gather, Build, Train,
  CancelQueueItem, SetRally. `CommandKind::Resign` exists, never sent.
- **Sim facts this brief relies on** (do not re-litigate; cite in tests):
  `Command.queue: bool` is honored by the sim for movement-class orders
  (crates/sim/src/command.rs, `if cmd.queue` arms) — waypoints are client-only work.
  Idle units auto-acquire in-range targets and **never pursue** them (chase only happens
  via a commanded `Order::AttackUnit`, combat.rs:80–110; movement only follows Orders,
  movement.rs:64–101) — a unit standing with no orders *is* guarding in the ZH sense.
- **View data available client-side:** `EntityView { id, owner, kind, pos, facing,
  hp_fraction_milli, move_state }` (sim_api lib.rs:116–137) — `move_state == Idle` is
  the idle-worker signal; kind ids resolve to names/capabilities at startup
  (`main.rs::kind_shapes` seam) — combat kinds (Q) and worker/command-center kinds derive
  from the loaded bundle / engine plan (`node_kind`, `worker_kind` already arrive via the
  engine's `alpha_plan`, app/mod.rs:641–642).
- **Menus/UI:** pure `Screen` machine (screens.rs:267–355) with `NavKey`; mouse +
  keyboard menus; `takes_world_input()` true only in `InMatch`. Minimap: left press =
  focus only (no drag); right = Move/AttackMove. Bottom HUD bar always visible (no F9
  toggle). Config: eight keys (config.rs:22–43); **no bindings are configurable** — this
  brief keeps it that way.
- **Tests pinning today's behavior** (must be updated, not deleted): input/mod.rs tests
  (RightButton FSM 379–434; edge band 438–494; direction-pinned right-drag/middle-drag
  498–547, 790–811; selection parity 551–615; Escape ladder 680–697; cursor hints
  709–786); orders.rs tests; engine camera.rs tests 324–399; Xvfb machine pass
  `tests/xvfb/a010_controls_confirm.sh`.

## 4. Phases

Each phase ends green (dev + release for the phase-end gate), with the new behavior
pinned by tests before moving on. Constants are named, never magic. Every guess gets an
A-131+ entry; every simplification a DEBT entry.

### Phase 0 — Baseline and truth table
Run the §4 verification from `AI-Handoff.md`; confirm green and goldens bit-identical.
Re-run the §3 audit against the actual base commit; record deltas in the report. Create
the branch `a/A-013-zh-controls` and push immediately (MP-6).
**Acceptance:** audit table in the report matches the code or flags where it doesn't.

### Phase 1 — Free the keyboard (the WASD/QE removal)
- Remove W/A/S/D camera panning entirely (held-key arms in main.rs and `about_to_wait`
  lookups). Arrows remain the keyboard scroll (ZH). A becomes *only* the attack-move arm;
  S becomes *only* Stop. W, D end up unbound (W reserved for air, D left free — both
  documented in the card).
- Remove Q/E camera rotation. Q and E become selection keys in phase 2. Camera rotation
  survives via middle-drag (kept superset) and gains numpad 4/6 in phase 4.
- Relocate the two tools off the F1–F8 row: **debug overlay F3 → F10**, **bug report
  F8 → F11**. If F10 proves grabbed by the WM under Xvfb/X11, fall back to `GraveAccent`
  and record the decision as an assumption. Update `report.rs` HUD strings and any
  test/docs references in the same commit.
- Watch for: the `keys` held-set still feeding `about_to_wait`; `event.repeat` filtering;
  menus clearing `self.keys`.
**Acceptance:** unit test pinning the new key→action map (a pure function if you extract
one — recommended: an `input::actions` table keyed by `KeyCode`+modifiers that both the
dispatch and the tests read); direction-pinned camera tests still green (arrows); Xvfb
smoke updated to the new tool keys; no other behavior changed.

### Phase 2 — Selection parity
All pure, client-side, over the fog-filtered view; "on screen" = `camera.project` lands
within NDC ±1; "on map" = every visible own entity of the qualifying set.
- **E** — select same-kind-as-current-selection units on screen; **double-tap E** (reuse
  the 350 ms window) — every visible own unit of that kind on the map. With an empty or
  mixed selection: E acts on the most recently clicked member's kind (record it; if
  nothing was ever selected, E is inert — document).
- **Q** — all own *combat* units on screen (combat = kind has an attack capability,
  derived through the `kind_shapes`/bundle seam, computed once per match, never per
  frame); double-tap Q — all own combat units on the map. Both replace the selection
  (shift adds — union with existing).
- **W** — reserved for air; inert today; the card says so.
- **Left-click empty ground deselects** (through the existing `set_selection` cue gate —
  the cue fires only on membership change). This reverses A-091; the assumption entry
  must say so explicitly. Esc's ladder keeps its ClearSelection rung (ZH players expect
  both).
- **Shift+click = add-only** (drop the toggle-remove). Shift+drag stays add-box.
  Removal from a selection = re-select; document in the card.
- **Ten control groups**: `CONTROL_GROUP_COUNT 9 → 10`, Digit0 maps to group 10. Ctrl+N
  assigns, N recalls, double-tap N centers (existing 350 ms machinery). All four
  behaviors get Digit0 parity tests.
- **Alt + digit** — focus the camera on the group's live center (existing `group_center`)
  *without touching the selection*.
- **Ctrl + ←/→** — cycle selection through own on-map units in ascending id order,
  wrapping; with nothing selected, select the lowest-id own unit. **Ctrl + ↑/↓** — same
  cycle restricted to the worker kind. These must not pan the camera (the Ctrl state
  suppresses the arrow-scroll for that press).
- **Space** — broaden `last_event_ground` to the last *radar event*: deaths (own and
  visible enemy, as today) plus the most recent attack landed on an own entity. Keep the
  base-anchor fallback.
- **H** — jump the camera (keep distance) to the own Command Center: resolve the
  command-center kind through the engine plan seam (extend `alpha_plan` with
  `command_center_kind` if absent — engine is in lane). No own CC → the existing base
  anchor fallback. Ctrl+H reserved for a hero; inert; card says so.
**Acceptance:** per-key unit tests (pure selection functions over a synthetic view: E/E²,
Q/Q², add-only shift, empty-click deselect, group 10 assign/recall/center, Alt-view
leaves selection untouched, cycle wrap-around, worker-cycle filter, radar-event ordering,
H fallback); the double-tap windows share the existing `is_double_tap` tests' style.

### Phase 3 — Unit modes: guard, scatter, waypoints
- **G then left click — guard.** Arm/disarm flow identical to armed-A (crosshair cursor,
  amber HUD line, Esc cancels, right-press cancels). The click issues a plain **Move** to
  the clicked point (`queue: false`): on arrival the order completes, the unit goes idle,
  and idle units engage in-range targets without pursuing (combat.rs/movement.rs facts,
  §3). That *is* ZH guard under this sim's laws — no new CommandKind. Pin with a client
  test: G-armed click submits `Move`, and an assumption entry cites the two sim tests
  that pin the engage-without-pursuit behavior (`out_of_range_attacker_does_not_hit_but_
  acquires`, the chase-pop suite).
- **X — scatter.** For each selected own unit, submit one `Move` to an offset from its
  current position: direction fixed per unit by its index in the *ascending-id* selection
  order around an 8-ray ring (index i → ray i mod 8), distance one acquisition-range-ish
  constant (named, tuned once). Deterministic, no RNG, no floats-in-sim concerns (the
  client computes; the sim receives Moves). Units already moving receive it too (replace,
  `queue: false`). Assumption entry: this is a client approximation of ZH's engine
  scatter; log the deviation.
- **Alt + left click(s) — waypoints.** While Alt is held, each left click on ground
  issues `Move` with **`queue: true`** except the first of the gesture (`queue: false`,
  so an in-flight order is replaced exactly once). Extend `submit_order` to take the
  queue flag (its cue/ping/bookkeeping unchanged). Draw the queued path as chained ground
  markers (reuse `CursorOrderHint`-style quads; frame-counted on the presented-frame
  clock). Waypoint clicks on an *entity* target behave as plain right-click context
  orders would (Attack/Gather queued the same way — same flag discipline). Release of Alt
  ends the gesture; nothing is sent on release. `Command.queue` is already honored by the
  sim (§3) — cite the sim command.rs queue arms in the test comments.
- **A then left click — unchanged** (it already matches ZH); update its HUD line to ZH
  terms ("attack move" not "left-click a target").
**Acceptance:** state-machine tests for the G arm (mirror the A tests); scatter
determinism test (same selection → same command list, twice); waypoint gesture tests
(first click replaces, later clicks queue, Alt-release sends nothing); a sim-side
integration smoke (a queued two-point path is walked in order — may live as a client test
driving a real `MatchHost` in `--frames` style, or an `alpha_loop`-adjacent check; pick
the cheapest honest one).

### Phase 4 — Camera parity
- **Numpad 4 / 6** — rotate left/right (same rate semantics as today's Q/E held rotation;
  these are held keys, not discrete notches — pin direction at yaw 0 and 90). **Numpad
  8 / 2** — zoom in/out at the wheel-notch step (`PITCH_…`-style named constant). 
  **Numpad 5** — reset camera to the defaults (yaw 0, default pitch, `START_CAMERA_
  DISTANCE = 26.0`, target = current target). On numpad-less keyboards middle-drag
  remains (documented superset).
- **Ctrl + F1–F8 set, F1–F8 jump — camera bookmarks.** Eight slots: store
  `(target_x, target_z, yaw, pitch, distance)`; jump restores all five (keep the current
  clamp discipline). Per-match storage, in-memory only (no config, no persistence —
  ZH-parity behavior, DEBT entry for persistence if you want it later). A jump to a
  bookmark whose stored target is off-map after a restart clamps via the existing
  `clamp_target`.
- **Minimap drag-pan**: while the left button is down over the minimap, every mouse move
  refocuses the camera (the current single-press focus becomes a continuous drag). Right
  -click minimap orders unchanged.
- **Fix the camera-bounds bug:** rebuild/reset the camera's map bounds when a match
  begins on a map whose dimensions differ from the default (the `App::new` camera is
  default-map-sized today, app/mod.rs:280–285). Regression test: begin a match on the
  smallest available map; pan into a corner; assert the clamp uses that map's extent.
**Acceptance:** direction-pinned tests for numpad rotate/zoom at yaw 0/90; bookmark
set/jump round-trip restores the five-tuple; minimap drag focus test; the bounds
regression test; Xvfb machine pass exercises rotate+zoom+bookmark end to end.

### Phase 5 — Interface parity
- **F9** — toggle the bottom HUD bar (and its layout reservation) on/off; pure
  presentation; the minimap goes with it (ZH hides the whole command bar). Menus never
  hide (ZH keeps the options UI). Persist nothing; default visible. Guard: minimap hit
  tests and UI button rects must respect the hidden state (input falls through to the
  world when hidden).
- **Tab** — toggle the mission objective HUD panel (visible in missions; in plain
  matches Tab is inert — document). 
- **Pause menu gains a Resign row** (Confirm-gated like other destructive rows): submits
  the existing, never-before-sent `CommandKind::Resign`, then the match flows to the
  existing end screen. This closes the vocabulary gap without touching sim.
- **F12 — screenshot (stretch).** If a clean wgpu readback-to-PNG costs more than a
  day's honest work, skip it, log the DEBT entry, and say so in the report — the card
  will mark it "not bound."
**Acceptance:** F9 toggling hides hit rects (test the layout function); Tab objective
toggle wired to the existing panel; Resign row emits the command and the end screen
follows (test through the screens machine + a `MatchHost` smoke); screenshot or its DEBT
entry.

### Phase 6 — The sim seam: force-fire (Ctrl + left click)
**This is the only phase authorized to touch `sim_api`, `sim`, `replay`, and `tests/`
sim-side files.** It is also *droppable*: if the owner prefers a client-lane-only
delivery, cut phase 6 entirely, log the omission, and report it — the card then marks
force-fire "not bound." Do not attempt a client-side fake.

- **`CommandKind::AttackGround { pos }`** in `sim_api`: additive variant, appended last;
  every existing discriminant and encoding unchanged. Validation: issuer owns an entity
  with an attack capability → the entity (or each selected attacker, one command each,
  as Attack does today) engages any entity within a small radius of `pos` — or simply
  acquires the ground slot as an attack order at `pos` with no movement. Keep it minimal:
  ZH force-fire is fire-at-point; the immediate-hit pipeline resolves per its existing
  laws. `queue: true` respected like other movement-class orders.
- **Replay codec**: a new tag *appended* to the command encoding (never renumber); old
  replay files must decode bit-identically (round-trip test with a pre-phase-6 fixture).
- **Client**: Ctrl+left-click arms force-fire (same arm/cancel family as A/G; the HUD
  line names it); the click submits `AttackGround` at the ground point; Ctrl+click on an
  *entity* submits a plain `Attack` on it (ZH force-fires at anything).
- **Golden safety case (state it in the report, prove it in the gate):** the demo and
  flagship logs contain no `AttackGround` commands, so their simulations are untouched —
  the goldens must come out bit-identical in both profiles. The content hash cannot move
  (no content files touched). Any movement: STOP (§0).
**Acceptance:** sim unit tests (validation refusals per the gate's existing style; a hit
lands near the point; queue behavior); replay round-trip old+new; client arm-state
tests; goldens bit-identical in dev and release.

### Phase 7 — Truth pass, registers, closeout
- Rewrite the cards so every line is literally true: README "The Field Manual" controls
  section, `docs/PLAYTEST.md` §2, `docs/product/QUICKSTART.md`. Format: the four tables
  of §2 (selection / unit modes / camera / interface), plus the superset table and the
  reserved-key notes (W, Ctrl+H, Tab-in-skirmish, F12 if skipped). The phrase "Generals:
  Zero Hour muscle memory" may stay **only if** every §2 row now holds.
- Add the owner's verdict to `docs/PLAYTEST.md` (finding source for this milestone).
- Registers: A-131… entries for every guess (the kept grab-scroll deviation, guard-by-
  move, scatter approximation, waypoint gesture semantics, E's most-recent-click kind
  rule, F10-vs-GraveAccent if it bites, radar-event definition, H fallback); DEBT entries
  for each deferral (F12 if skipped, bookmark persistence, formation). `CHANGELOG.md`
  gains the M11 section. `AI-Handoff.md` §6/§7/§9 updated (new key map, the relocated
  tool keys, the bounds fix, phase 6 if delivered).
- Full gate both profiles; goldens re-verified bit-identical; the Xvfb+XTEST machine pass
  extended to the new paths (numpad camera, bookmarks, E/Q selection, empty-click
  deselect, guard/scatter/waypoint smokes, F9 toggle) — `tests/xvfb/` gains `a013_*.sh`
  in the existing style; key cadence ≥0.25–0.4 s under llvmpipe.
- Report at `docs/pm/reports/A-013.md` in STANDING_ORDERS §10 format, committed in the
  branch's final commit (MP-9). The owner's human re-test is the feel gate; the machine
  proves wiring, not feel — say exactly that.

## 5. What this brief deliberately reverses (owner sign-off is issuing it)

1. **A-091** — left-click on empty ground now *deselects* (ZH behavior; M10.2 made it a
   deliberate no-op).
2. **Shift+click** — add-only, no longer a remove-toggle (ZH behavior).
3. **WASD camera pan** — removed (A/S/W are ZH action keys; ZH scrolls with arrows/edge/
   right-drag only). D also released.
4. **Q/E camera rotation** — removed (Q/E are ZH selection keys). Middle-drag rotate and
   the future numpad keys replace it.
5. **F3 / F8** — relocated to F10 / F11 (the F1–F8 row belongs to camera bookmarks in
   ZH). `docs/PLAYTEST.md` and `QUICKSTART.md` tell A14 testers the new keys.
6. **Kept, not reversed:** right-drag grab-scroll (ZH's is a velocity "rapid scroll";
   the M10.2 grab passed the owner's re-test — recorded as the one camera deviation the
   card owns).

## 6. Touch list (MP-1 declaration)

Declared: `crates/client/src/**` (input/{mod,event,orders}.rs, main.rs, app/mod.rs,
ui.rs, screens.rs, config.rs, feedback.rs, report.rs, render/** as needed) ·
`crates/engine/src/**` (camera.rs, the alpha-plan seam, audio cue list if the new modes
need acknowledgment cues) · `tests/xvfb/**` · `docs/PLAN-M11.md` (this file, first
commit) · `README.md`, `docs/product/QUICKSTART.md`, `docs/PLAYTEST.md`,
`docs/ASSUMPTIONS.md`, `docs/DEBT.md`, `CHANGELOG.md`, `AI-Handoff.md` (one final
registers/docs commit only, MP-3) · `docs/pm/reports/A-013.md` (final commit) ·
**phase 6 only:** `crates/sim_api/src/lib.rs`, `crates/sim/src/{command,combat}.rs`,
`crates/replay/src/**`, `tests/` (new sim-side test file with its `[[test]]` entry).
Not touched: `content/`, `crates/{fx,ai,tools}`, `.github/`, `Cargo.toml` files.

## 7. Exit criteria

1. Every §2 target row implemented (or explicitly marked reserved/not-bound in the card
   and report) with its pinned test — the agent's report lists each row with evidence.
2. Full green gate in dev and release; all four goldens bit-identical (commands +
   output in the report).
3. Xvfb machine pass covering the new input paths; screenshots unnecessary (no visual
   change expected beyond HUD lines).
4. The three controls cards rewritten and literally true; registers updated; handoff
   refreshed; report committed.
5. Delivery per the owner's relayed instruction: branch `a/A-013-zh-controls` rebased on
   master, or a single `git format-patch --stdout` file verified with `git am` on a fresh
   clone — whichever the owner's prompt specifies. State the base commit SHA in the
   report.

## 8. STOP conditions

Any golden moves · the architecture-law test would need weakening · a frozen decision
(FD-1..10) appears to block a phase · phase 6's additive-encoding claim fails anywhere
(an old replay decodes differently) · the brief conflicts with `plan.md` or a standing
order. In every case: stop, write the finding, report — do not improvise a workaround.

— issued for the owner's relay; sources for the reference table: StrategyWiki and the
Steam hotkeys guide (§2), camera behavior cross-checked with GameReplays.

### by E-Vex