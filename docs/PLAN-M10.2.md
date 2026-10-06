# Pandemonium — Playtest-1 Findings: Implementation Plan (M10.2)

> Location in repo: `docs/PLAN-M10.2.md`
> Source: the project owner's first human playtest of the windowed client (after M10.1).
> Authority: this file is the specification for milestone M10.2. It does not replace `plan.md`; it extends it for this milestone only.

## Context

The project owner played the windowed client on a real display (first human pass after M10.1). The Alpha declaration (A14) is blocked until these findings are fixed and re-tested.

Four findings, in priority order:

1. Controls
2. Visual legibility
3. Menu and settings
4. Audio

Read `AI-Handoff.md` and `plan.md` §0, §2, §11 first.

## Global rules for this milestone

- All work is presentation/input only: it lives in `crates/client` and `crates/engine`, above the sim boundary. **Zero golden hash movement.** If any golden moves, stop and revert.
- Do not touch `crates/sim`, `sim_api`, `fx`, `ai`, `content`. Do not change `content/` files (that would move the content hash).
- **Commit after every small change.** One small step = one commit; 100+ commits is expected and wanted. Never squash.
- Every commit must pass: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (dev). The release-profile tests (`cargo test --workspace --release`) run at the end of every phase and before the final report.
- Every input or camera change needs direction-pinning tests (which way in world coordinates, at yaw 0 and 90), per the M9.1 lesson. Filter winit key repeats.
- New external crates need an update to the dependency allow-list in `tests/architecture_law.rs` and an ADR in `docs/adr/` with the reason. Prefer no new crates where practical.
- Log guesses in `docs/ASSUMPTIONS.md` (continue from A-090), simplifications in `docs/DEBT.md`. Update `AI-Handoff.md` at the end (status board, snapshot, gotchas).
- Record the owner's findings in `docs/PLAYTEST.md` as tester result #1.

## Phase 1 — Controls (highest priority): match C&C Generals: Zero Hour

Goal: a player who knows Generals ZH can play without reading anything.

### 1.1 Audit first

Write down what each mouse button and key currently does in `crates/client` (`main.rs`, `orders.rs`). The owner reports that unit movement happens with the LEFT button. The handoff says M9.1 moved it to right-click context orders. Find out which is true in the current code. Rule to enforce:

- Left click on ground = never issues an order. Left click selects, left drag box-selects.
- Right click = the only way to command units (context-sensitive: ground = Move, enemy = Attack, ore node = Gather, own production building = rally point).
- Click on empty ground with a selection and nothing else = nothing happens.

### 1.2 Camera scroll, Generals ZH style

- Hold the RIGHT mouse button and drag to scroll the map. The map follows the drag direction in a natural way (like grabbing the ground). This is the main way the owner wants to move.
- The right button also issues commands. Resolve the conflict with a drag threshold: press and release with less than about 6 pixels of movement = command; movement beyond the threshold = camera scroll and NO command on release. Make the threshold a named constant.
- Edge scroll: widen the trigger zone to a sensible band (about 12–16 px), scale speed with depth into the band, and make it work on all four edges and corners. Add a toggle in settings (Phase 3). The owner currently experiences it as unusable (the screen only moves when the cursor reaches the very last pixel), so test at the real window sizes, including fullscreen.
- Keep WASD and arrow keys. Keep wheel zoom toward cursor. Keep Q/E rotate.
- Middle-drag rotates the camera (Generals does this). Keep Ctrl+wheel pitch if it exists.

### 1.3 Unit control parity with Generals ZH

- Double-click a unit selects all visible units of the same kind.
- Shift+click adds or removes from the selection; shift+box adds.
- Ctrl+number sets a control group, number recalls it, double-tap number centers the camera on the group. Check the existing 1–9 implementation against this.
- S = Stop, A then click = attack-move (keep the existing armed-A flow), Escape cancels an armed command or clears the selection.
- Space or a similar key to jump to the last event or base (optional, only if cheap).
- Cursor feedback: change the cursor or draw a small marker for Move / Attack / invalid, and keep the command ping.

### 1.4 Tests

Pure functions for the right-button state machine (press, move, release with and without threshold), edge-scroll vector from cursor position and window size, drag-scroll direction at yaw 0 and 90. Machine-verify with Xvfb + XTEST per DEBT-008.

**Exit:** documented control card (update the README and `docs/PLAYTEST.md` controls card), all tests green, owner re-test.

## Phase 2 — Visual legibility

Goal: at a glance, the player knows what each thing is. Not pretty, just clear.

### 2.1 Distinct silhouettes per kind

Build simple procedural meshes from primitives in the client (no asset files needed):

- Worker: small, round-ish body with a visible tool shape.
- Rifleman: small humanoid-ish capsule or box with a thin rifle.
- Raider: medium fast-looking wedge or buggy shape.
- Guardian/tank: large body with a turret and a barrel.
- Command center: large, tall, with a distinct roof or tower so it reads as the base.
- Barracks: wide, low, with a flag or door.
- Supply depot: small box with a stacked-crates look.
- Turret: pedestal with a rotating-looking barrel.
- Ore node: crystal cluster in a bright color that contrasts with the ground.

The mapping from kind to mesh lives in the client, keyed by the kind name from the bundle, with a fallback based on capability shape (has Footprint = building box, has Move = unit box). Do NOT add presentation data to `content/` files.

### 2.2 Team identification

A strong team color band or banner on every entity, plus a ground ring under each unit. Colors must be distinguishable for color-blind players (check blue/orange or similar, not red/green).

### 2.3 Selection and info

A bright selection ring, a larger health bar for selected entities, and an info panel in the HUD that shows the selected entity's NAME, health, and (for buildings) production queue. Hovering an entity shows its name tooltip.

### 2.4 Minimap

Distinct markers for own, enemy, ore. Make sure buildings are visibly larger than units.

### 2.5 Ground contrast

Make terrain colors calmer and lower in saturation than units, so entities pop. Rock walls and blocked terrain must read as obstacles.

### 2.6 Tests

Mesh generation is pure data: test vertex counts and that each kind resolves to a unique mesh. Test the name lookup for the info panel.

**Exit:** a stranger can identify command center, worker, and tank in a screenshot without a legend. Capture screenshots under Xvfb and include them in the delivery notes.

## Phase 3 — Menu and settings

Goal: a proper start flow instead of dropping straight into a match.

### 3.1 Application state machine

Client-side state machine: `MainMenu -> (Settings | NewMatch) -> InMatch -> (PauseMenu | EndScreen) -> MainMenu`. Pure data and transitions, unit-tested. Nothing here touches the sim.

### 3.2 Main menu

New Match, Settings, Quit.

### 3.3 New Match screen

- Mode: Player vs AI, AI vs AI (spectate), Sandbox (no opponent, for testing).
- Seed: random by default, editable (this uses the existing `--seed` path).
- Map: the existing Crossroads, listed from the bundle (supports more maps later).
- Start button. Starting a match reuses the existing drop + reconstruct path (A-014/A15 style), so determinism is unchanged.

### 3.4 Settings

Edge scroll on/off, scroll speed, camera zoom limits, master volume (Phase 4), fullscreen toggle, show debug overlay by default. Persist to a small config file in the user config directory using a simple hand-written `key=value` format (no serde in the client path unless the allow-list already permits it). Load with safe defaults when the file is missing or malformed.

### 3.5 Pause menu

Esc during a match opens Resume, Settings, Restart, Quit to Menu. The match pauses through the existing MatchHost pause.

### 3.6 UI implementation

Reuse the existing fontdue text renderer and UI overlay pass. Buttons are plain rectangles with hover states. Keep it keyboard and mouse navigable.

**Exit:** the game starts at the menu, a match can be started in each mode, settings persist across runs, and tests cover the state machine and config parser (including corrupt files).

## Phase 4 — Audio

Goal: simple sounds that make events feel real. The seam already exists: `engine/src/audio.rs` has the `AudioSink` trait and the `cue_for` mapping.

- **4.1** Add one audio backend implementing `AudioSink` in the client. Choose the smallest dependable option (for example rodio or cpal-based) and justify it in an ADR; update the architecture-law allow-list for the client crate only.
- **4.2** Sounds are generated procedurally at startup (simple synthesized beeps, clicks, thuds, short noise bursts) so no asset licensing is needed and the repo stays small. One distinct sound per cue: attack landed, unit lost, unit ready, structure done, resource delivery, match ended, command acknowledged, selection click, UI click.
- **4.3** If no audio device exists (CI, headless), fall back to the `NullAudioSink` silently. Never crash or block the render loop.
- **4.4** Master volume and a mute toggle in settings. Rate-limit repeated cues (for example many attacks per tick) so it does not become noise.
- **4.5** Tests: cue-to-sound mapping, rate limiting logic, fallback path. Log DEBT-011 progress.

## Phase 5 — Verification and handoff

- Full green gate in dev and release; golden hashes bit-identical to the M10.1 values (demo `0xb6fff6659cfb7709`, flagship `0x6e9a18bd7c5f699f`, content `0x9bc18c521107b262`).
- Xvfb + XTEST smoke for the new input paths, plus screenshots for Phase 2.
- Update the `docs/PLAYTEST.md` controls card and add the owner's feedback as result #1; schedule the full A14 playtest (5+ testers) only after the owner confirms the fixes feel right.
- Update `AI-Handoff.md` (status board M10.2, snapshot, gotchas such as the right-button threshold and the menu state machine), the README controls, and the CHANGELOG.
- **Delivery:** export every commit as one single patch file (`git format-patch --stdout`), verify it applies cleanly with `git am` on a fresh clone of the base commit, and hand it to the owner as specified in the agent prompt.

## Out of scope for this milestone

New units, new factions, AI behavior changes, real art assets, networking, any sim change.

## Delivery (the agent prompt's instructions, restated for the record)

- The plan lives in this file (`docs/PLAN-M10.2.md`); if it is not in the repo yet, the first commit adds it.
- Do not push to E-Vex/pandemonium-bd. Create a local branch `m10.2` from `master` and commit every small change there, per the commit policy. Never squash.
- At the end, export all commits as ONE patch file that keeps every commit and its message: `git format-patch --stdout <base-commit>..m10.2 > m10.2-all-commits.patch`.
- Verify the patch: fresh clone, check out the base commit, run `git am m10.2-all-commits.patch`, then the full green gate (fmt, clippy, dev tests, release tests) and confirm the three golden hashes are unchanged.
- Upload only that one file, plus a short README.txt with the base commit hash, to the owner's delivery repository (folder `m10.2/`), using the token supplied out-of-band in the agent prompt. **The token is never written into any file, commit message, log, or output.**
- Tell the owner the base commit hash and the exact command to apply the patch.
