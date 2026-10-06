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
against the AI opponent; on a machine with no display it prints a finding and
runs a headless smoke pass instead (that is expected there, not a failure — but
it is not a playtest either; use a desktop).

**Seeding a match for reproducibility:** the match seed defaults to 7. Any u64
works:

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

Controls: **WASD / arrows / screen edges / middle-drag** pan the camera,
**Q / E** rotate it around its target (Generals-style), the **wheel** zooms
toward the cursor, **Ctrl+wheel** tilts the pitch (Generals-style),
**left-click / drag** selects, **right-click** orders (attack an enemy,
gather a node with workers, or move over open ground), **A** arms
attack-move for the next left-click, **S** stops, **1-9** recall control
groups (**Ctrl+1-9** assigns), **P** pauses, **F3** toggles the debug
overlay, **R** restarts after the match ends.

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
2. **Audio absence (DEBT-011):** did the absence of sound hurt your read of the
   game? (yes/no — this is the debt's declared trigger: a "yes" argues for
   pulling the audible backend forward)
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
| | | | | | | | |
| | | | | | | | |
| | | | | | | | |
| | | | | | | | |

Five rows, ready to fill. A failed tester is a finding — record them anyway;
they are often the most valuable row in the table.

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
