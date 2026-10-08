# Playtest — the A14 instrument

**The pass bar (plan §13 A14):** at least **5 testers** run the unaided loop and
are honestly recorded in the results table below. A tester who fails a step is a
**finding, not a wave-through** — the declaration rule (plan §13) says a
criterion that cannot be met honestly is recorded as it is. A14's two halves:
*first-time players complete the loop unaided*, and *spectators can tell who is
winning*. When the table holds five honest rows, the owner flips A14 in
`docs/ALPHA_DECLARATION.md` (or records the finding) citing this file.

"Unaided" means exactly this: **no documentation beyond the game itself and the
controls card in section 2 of this file.** No README, no tutorial, no coaching,
no hovering helper. If you (the tester) have to ask how to do something, that is
a finding — write it down.

---

## 1. Quickstart (per-OS)

Prerequisites: **git** and **Rust via rustup** — nothing else.
[`rust-toolchain.toml`](../rust-toolchain.toml) pins the exact compiler version
(1.98.1); rustup installs it automatically on first use (one network round-trip,
then offline builds).

**Windows** (PowerShell), **macOS** (Terminal), and **Linux** (any shell) run
the same four commands:

```bash
git clone https://github.com/E-Vex/pandemonium-bd.git
cd pandemonium-bd
cargo build --release -p pandemonium-client
cargo run --release -p pandemonium-client
```

Run **from the repository root** (the clone directory): the client resolves its
`content/` data relative to the repo, so launching from elsewhere finds no
content and refuses to start. On a desktop this opens the windowed 3D client
**at the main menu** (M10.2 Phase 3 — the game no longer drops you straight
into a match); on a machine with no display it prints a finding and runs a
headless smoke pass instead (that is expected there, not a failure — but it is
not a playtest either; use a desktop).

From the menu: **New Match** picks the mode (Player vs AI / AI vs AI
spectate / Sandbox — no opponent, for testing), the seed, and the map, then
**Start** drops you into the match. Every screen works by keyboard alone
(arrows + Enter, Esc backs out) and by mouse alone (hover highlights, click
activates). **Settings** adjusts edge scroll, pan speed, camera zoom limits,
master volume (audible in Phase 4), fullscreen, and the debug-overlay
default; Done saves to the user config directory and the next run remembers
(A-115). In a match, Esc's final rung opens the **pause menu**
(Resume / Settings / Restart / Quit to Menu).

**Seeding a match for reproducibility:** the seed field is **random by
default** and always visible on the New Match screen — type digits to set it
exactly, Enter re-rolls, and `--seed N` pre-fills it:

```bash
cargo run --release -p pandemonium-client -- --seed 42
```

The same seed reproduces the same match bit-for-bit (plan FD-1): identical map,
identical AI decisions. **Record the seed you played** — with a tick it is a
complete bug report (section 6).

**Recording a replay of your session** (optional but encouraged):

```bash
cargo run --release -p pandemonium-client -- --seed 42 --record my-match.pdrp
```

The replay is written at match end or when you close the window, and it
re-verifies headlessly:

```bash
cargo run --release -p pandemonium-tools -- replay-verify my-match.pdrp
```

A mid-match dump on demand exists too — F8, section 6.

## 2. Controls card

Copy of the README's controls paragraph, verbatim, plus the F8 bug-report key:

Controls (Generals: Zero Hour muscle memory):

**Selecting** — **left-click** selects, **left-drag** box-selects; left never
issues an order, and a click on empty ground does nothing (Esc clears the
selection). **Shift+click** adds or removes one unit, **shift+drag** adds the
box's picks, **double-click** selects every visible unit of the same kind.

**Commanding** — **right-click** is the only way to command: on an enemy it
attacks, on an ore node it gathers (workers), on open ground it moves, and on
a single selected producer it sets the rally point. A click under 6 px of
travel orders; **right-drag** beyond that grabs and scrolls the map (the
release then orders nothing). **A** arms attack-move for the next left-click,
**S** stops.

**Camera** — **right-drag** scrolls (the main way to move the map),
**middle-drag** rotates, **WASD / arrows / screen edges** pan (the edge is a
14 px band whose speed scales with depth), **Q / E** rotate, the **wheel**
zooms toward the cursor, **Ctrl+wheel** tilts the pitch.

**Groups & jumps** — **1–9** recall control groups, **Ctrl+1–9** assigns,
double-tapping a digit recalls and centers the camera on the group. **Space**
jumps to the last death (or your base). **Esc** climbs one rung per press:
armed command → placement → selection → **pause menu**. **P** pauses without
the menu, **F3** toggles the debug overlay, **R** restarts after the match
ends (the end screen offers Rematch / Main Menu too).

**Menus** — the game starts at the **main menu**; every screen is reachable
by keyboard alone (arrows + Enter, Esc backs out) and by mouse alone (hover
highlights, click activates). The New Match screen picks the **mode**
(Player vs AI / AI vs AI spectate / Sandbox), the **seed** (random by
default, typeable, Enter re-rolls) and the **map**. The **settings** screen
adjusts edge scroll, pan speed, zoom limits, master volume (audible in
Phase 4), fullscreen, and the debug-overlay default; **Done saves** to the
config file, **Esc leaves without saving**. In a match, the pause menu
(Esc's last rung) offers Resume / Settings / Restart / Quit to Menu.

**[F8] bug report** — press it at (or right after) the moment anything goes
wrong; it writes two files (section 6).

Economy controls worth knowing exist (the game itself shows these as the
first-time bar at the bottom of the screen when nothing is selected): select a
worker to build, a producing building to train and set rally points.

## 3. The unaided-loop checklist

One row per step of the minimum viable loop (plan §13). Mark **done** or
**failed** per step, and comment anything that felt wrong — the comments are
the findings. "Unaided" per the header: this card's controls line is the only
documentation allowed.

| # | Step (done / failed + comment) | Verdict | Comment |
|---|--------------------------------|---------|---------|
| 1 | Start a match (the client runs one immediately) | | |
| 2 | Train workers from the Command Center | | |
| 3 | Gather Ore with workers | | |
| 4 | Build a Supply Depot and a Barracks (placement ghost, legality preview) | | |
| 5 | Train combat units from the Barracks | | |
| 6 | Queue several items, cancel one, set a rally point | | |
| 7 | Scout under fog of war | | |
| 8 | Engage the AI | | |
| 9 | Destroy all enemy structures (or resign) | | |
| 10 | Read the end screen | | |
| 11 | Restart cleanly with R | | |

## 4. Spectator legibility (A14's second half)

Watch any 60 seconds of a fight — yours or two AIs — and answer: **who is
winning?** The machine half is already proven (health bars, team colors, hit
flashes, death fades exist and are tested); this question is the human half.

| Question | Answer |
|---|---|
| Who was winning? | |
| Confidence: **sure / fairly sure / guessing** | |
| What told you? (health bars / unit count / damage flashes / other) | |

## 5. Probe questions

These arm the debt triggers — the answers decide real register entries
(`docs/DEBT.md`), so answer honestly; "yes" is useful data, not a failure of the
tester.

1. **Placement guess (DEBT-012):** did any build-placement preview say **legal**
   when the order was then refused, or **illegal** when it should have
   succeeded? (yes/no; if yes — seed + tick, or an F8 dump)
2. **Audio (DEBT-011, repaid in M10.2 Phase 4; the owner's re-test came
   back clean — A-126 — so the probe now guards against regression):**
   do the nine cues fire at their moments
   and sound distinct from each other? Does a big fight read as a
   heartbeat rather than noise (the per-cue rate limiter)? Does volume
   change loudness live, does mute silence everything, and do both
   survive a restart? A muddled cue, a noise-fight, or a dead volume row
   is a finding — the machine proved the wiring and the counting, the
   owner proved the sound; the testers prove it keeps working.
3. **Feel (A8):** did orders feel instant? Any input that felt dead?

## 6. Bug-report procedure

1. Press **F8** at (or right after — pause first with **P** if you need a
   moment) the moment of the problem. The client prints two filenames and pings
   the screen: a replay record `pandemonium-report-<seed>-tick<tick>.pdrp` and
   a sidecar `…-info.txt` (seed, tick, content identity, player slot, the
   controls state). Files land in the working directory (or beside `--record`'s
   path when that flag was used).
2. Attach **both files** to the report.
3. Note the **seed** you played (the sidecar carries it — see the
   `seed:` line) and the **tick** (`tick:` line). Seed + tick + the dump is a
   complete reproducible incident; the replay re-simulates to identical hashes
   (`tools replay-verify`).

## 7. Results (filled by the owner)

| Tester | Date | OS | Build commit | Loop completed (unaided?) | Spectator verdict | Probes (placement / audio / feel) | Incidents (seed + tick + dump filenames) |
|---|---|---|---|---|---|---|---|
| owner (playtest-1) | 2026-10 (post-M10.1) | desktop, real display | bdf1266 | **no** — controls unusable (finding #1, detail below) | not reached | feel: the four findings below | none filed (findings went to `docs/PLAN-M10.2.md` instead) |
| owner (re-test) | 2026-10 (post-M10.2 Phase 1) | desktop, real display | d84203e | **pass, with notes** — the loop is playable with the new controls card; the notes arrive after the Phase 2 delivery (A-101: the owner re-tests on their machine and reports any issue with its exact location; a reported issue becomes a finding row here) | not reached | feel: pending specifics | none filed yet |
| owner (re-test 2) | 2026-10-08 (post-M10.2 Phase 2) | desktop, real display | 404e906 | **pass** — the Phase 2 visual pass cleared the way for Phase 3 (the owner's go-ahead: "Great now write the task P3 to the next AI agent"; A-107); any residual visual notes fold into the next re-test | not reached | feel: unchanged from the Phase 1 notes | none filed |
| owner (re-test 3) | 2026-10-08 (post-M10.2 Phase 3) | desktop, real display | 1210cbd | **pass** — the Phase 3 menus/settings re-test cleared the way for Phase 4 (the owner's go registered as A-119); the menu-first flow, the three modes, and settings persistence judged right | not reached | feel: carried into the Phase 4 re-test | none filed |
| owner (re-test 4) | 2026-10-09 (post-M10.2 Phase 4) | desktop, real hardware | 652f6cf | **pass, clean** — the Phase 4 audio re-test (A-126): the nine cues fire at their moments and sound distinct; volume changes loudness live, mute silences everything, both survive Done + relaunch; a big fight reads as a heartbeat, not noise. The go for the M10.2 closeout and the A14 scheduling below | not reached (the A14 sessions carry it) | audio: clean end to end; feel: unchanged from the Phase 3 notes | none filed |

### The A14 schedule (set at the M10.2 Phase 5 closeout)

Five first-time-player slots, three waves, honest rows — the declaration's
bar (plan §13) does not move: **a failed tester is a finding, never a
wave-through**, and when the table below holds five honest rows the owner
flips A14 in `docs/ALPHA_DECLARATION.md` (or records the finding) citing
this file. Fill the row as the session runs: date, OS, build commit (the
`master` HEAD you cloned), the loop verdict per section 3, the spectator
verdict per section 4, the section-5 probes, and any F8 incidents.

| Slot | Wave / target date | Focus (the session stays unaided — this only steers the report) |
|---|---|---|
| T1 | 1 — by 2026-10-19 | the full unaided loop, controls under load (spam-click, box select, right-drag scroll) |
| T2 | 1 — by 2026-10-19 | the full unaided loop, economy legibility (train / build / queue / rally without asking) |
| T3 | 2 — by 2026-10-26 | the spectator half (section 4: watch 60 seconds of a fight) plus the placement probe (DEBT-012) |
| T4 | 2 — by 2026-10-26 | the audio and settings probes (section 5: the nine cues, volume/mute live + persistence) |
| T5 | 3 — by 2026-10-31 | the full unaided loop on a second OS (Windows or macOS — the quickstart is three commands) |

| Tester | Date | OS | Build commit | Loop completed (unaided?) | Spectator verdict | Probes (placement / audio / feel) | Incidents (seed + tick + dump filenames) |
|---|---|---|---|---|---|---|---|
| T1 | | | | | | | |
| T2 | | | | | | | |
| T3 | | | | | | | |
| T4 | | | | | | | |
| T5 | | | | | | | |

Recruit first-time players (the declaration's own words: *first-time
players complete the loop unaided*). The owner's re-test rows above are
not substitutes — they steered the fixes; these five honest T-rows are
the gate. A failed tester is a finding — record them anyway; they are
often the most valuable row in the table.

### Result #1 in detail — the owner's playtest-1 (M10.2's source)

The first human pass over the windowed client after M10.1. The formal A14
loop (section 3) was not completed — the controls got in the way first. Four
findings, in the owner's priority order (the full specification is
`docs/PLAN-M10.2.md`):

1. **Controls** — not Generals ZH: no right-drag map scroll, edge scroll
   reads as broken (only the very last pixel row triggers it), the button
   semantics are unclear (the owner reported "unit movement happens with the
   LEFT button").
2. **Visual legibility** — at a glance you cannot tell what anything is
   (silhouettes, team identification, minimap, ground contrast).
3. **Menu and settings** — the game drops straight into a match; no menu,
   no settings, no pause menu.
4. **Audio** — no sound at all (DEBT-011, expected at this stage).

#### The controls audit (PLAN-M10.2 §1.1 — what the code actually did)

Audit of `crates/client/src/main.rs` + `orders.rs` at the base commit
(bdf1266), pre-M10.2:

| Input | Behavior in code |
|---|---|
| Left press | UI button hit-test first (train / build placement / queue cancel), then minimap click-to-focus, then placement confirm, then armed-A attack-move fires, else starts a selection drag |
| Left release | Drag beyond `CLICK_SLOP` (0.01 NDC) = box select; otherwise single-click select (own entities within 0.05 NDC; empty ground **clears** the selection) |
| Right press | Cancels placement, disarms armed-A, then: minimap right-click orders Move at the mapped point, else the context order (rally point for a selected producer / Attack on an enemy / Gather on a node with workers / Move on ground) |
| Right drag | Nothing (no right-drag camera scroll exists) |
| Middle drag | Pan (grab-the-ground) |
| Wheel | Zoom toward cursor; Ctrl+wheel = pitch |
| W/A/S/D + arrows | Pan (A also arms attack-move; S also stops) |
| Q / E | Rotate |
| 1–9 / Ctrl+1–9 | Recall / assign control groups (no double-tap centering) |
| P, `.`, F3, F8 | Pause, single-step, debug overlay, bug report |
| R / Esc | Restart after the match ends / cancel armed order or placement, else quit |
| Edge scroll | 24 px binary full-speed zone on all four edges (focused windows only) |

**Audit verdict on "movement happens with the LEFT button":** false for a
plain left-click in the current code — since M9.1, context orders
(Move/Attack/Gather/rally) issue on the **right** button only
(`orders::resolve_context_order`). What the owner can have hit with the left
button: the armed-**A** attack-move (fires on left-click), the bottom-bar
buttons, and the placement confirm — all three are order-issuing left clicks.
The plan's rule (left = select only) is therefore *almost* already law; the
audit additionally found one real inconsistency: on right-press the code
disarms armed-A **before** the minimap right-click branch checks it, so the
armed minimap attack-move path is dead code — M10.2 §1 reworks the whole
right-button path as a state machine and removes the dead branch.

The re-test for this row: after M10.2 Phase 1, the owner replays the loop
with the updated controls card (section 2) — the row above is then updated
or a second row is added for the re-test pass.

### Phase 2 delivery note (visual legibility — the re-test's second target)

This pass (branch `m10.2-phase2`) builds the legibility half of finding #2's
fix: distinct multi-part silhouettes per kind, blue/orange team bands and
rings, a bigger selected-entity health bar, hover name tooltips, a
production-queue summary in the info panel, distinct minimap markers
(buildings visibly larger; ore a diamond), and calmer ground with darker
rock.

**Machine half of the exit test (Xvfb + llvmpipe, this environment)**: the
windowed client presented frames through the real wgpu GL path (700/1500/
2000/1600-frame smokes, all healthy, zero golden movement) and the captures
verify on sight: the **command center** reads as THE base (tall slab +
corner tower, team blue), **workers** read as small blue units and name
themselves on hover ("Worker / yours"), **ore nodes** pop as bright amber
crystal clusters off the calm ground ("Ore Node / neutral" on hover), the
selected worker draws the **bigger always-on health bar** plus the green
selection brackets and the info panel with its build card, and the
minimap carries the base cluster. The Xvfb pass also caught a live miss —
hovering the Command Center's tower mass fell outside the fixed hover
radius — fixed in the same pass (footprint-scaled reach, unit-pinned).
**Not directly captured: a Guardian on screen** (the start force fields
none and the AI's first wave was not on camera in the captured windows);
its hull/treads/turret/barrel silhouette is pinned by the shape tests, so
the tank-on-sight read is the one item the re-test confirms on a real
display.

The re-test for the row above now also asks: **can you identify the
command center, a worker, and a tank on sight, without a legend? Can you
tell the sides apart at a glance? Can you read the minimap and the health
state?** Report anything that does not read — a failed read is a finding,
not a wave-through.

### Phase 4 delivery note (audio — the re-test's fourth target)

M10.2 Phase 4 landed the audio pass (DEBT-011 repaid): nine synthesized
cues behind the engine's `AudioSink` seam on rodio (ADR-0002) — attack
landed (a low thud), unit lost (a descending two-step), unit ready (a
rising chirp), structure done (a low chord), delivery (a bright ding),
match ended (a three-tone fanfare), command ack / selection click / UI
click (three distinct short blips) — each rate-limited to one voice per
80 ms, with live volume and mute in settings (row 4 and row 5) and a
silent null fallback where no device exists. The machine pass (Xvfb)
proved the wiring and the counting only: this environment cannot hear,
and its evidence lines say "null fallback" and count cues — they never
claim sound. **The re-test for this row is therefore entirely about
ears:** on real hardware, play a match and confirm each of the nine
moments makes its distinct sound, the volume row changes loudness live,
mute silences everything, both survive a restart (Done saves, Esc
discards), and a big fight reads as a heartbeat rather than noise. Any
cue that muddles with another, any moment that stays silent, or a fight
that turns into mush is a finding for the results table above — the
per-cue recipes in `crates/client/src/sound.rs` are one match arm each
and re-tunable in isolation.

### Phase 5 delivery note (verification and handoff — the closeout)

M10.2 Phase 5 verified the milestone and closed it out; no code moved.
The full gate ran green in dev **and** release (fmt, clippy `-D warnings`,
546 / 541 tests — the five should-panic invariants are debug-only), the
three goldens came back bit-identical (demo `0xb6fff6659cfb7709`,
flagship `0x6e9a18bd7c5f699f`, content `0x9bc18c521107b262`), the replay
round-trip (A2) passed, and the headless smoke printed its audio
evidence line.

The Xvfb + XTEST machine pass re-ran the M10.2 paths on the windowed
client (llvmpipe GL, no audio device — the null arm; the recipe is
AI-Handoff §9): a seed-7 Player-vs-AI match driven end to end (menu →
New Match → Start; a drag-box selection of the five start entities; one
right-click ground order — the only way to command; a right-drag scroll
that moved the camera ~85 % of the frame and issued **no** order — the
6 px threshold; the Esc ladder through clear-selection → pause menu →
Resume; the selection re-held at exit), a menu-only session whose New
Match row was mouse-activated (the focused row's plate located from the
screenshot), the settings persistence pair (volume Left to 0.95, mute
on, Done saves; the relaunch loads the file and the audio evidence line
reports `[muted]`), and the Esc-discard run (no file written). The audio
evidence lines stated the arm every time and counted the match's cues —
fed/voiced/dropped 24/21/3 with 5 client-side — the machine counts, it
never claims sound.

**The verdict row above (re-test 4, A-126) is the owner's:** the sounds
play, the nine moments are distinct, volume is live, mute silences, both
persist, and a big fight is a heartbeat. With that, M10.2 is complete
and the A14 schedule above is live — the five T-rows are the only thing
standing between this repo and the Alpha declaration.
