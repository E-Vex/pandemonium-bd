# Product-Surface Gap Analysis — the player journey vs the shipped client

> **Brief A-001** (PRODUCT PHASE, docs only). This is the evidence-based map of the
> distance between the client as it runs today and a product release ("R1"), plus the
> raw material for the app-shell ADR (`docs/adr/0100-client-app-shell.md`, proposed).
> It changes no code and moves no goldens.

**Method.** The audit was performed at `master` `23012bf` (branch
`a/a-001-product-surface-audit`) after the full §4 gate re-ran green in this
environment: `cargo fmt --check` clean, clippy `-D warnings` clean, **553 tests green
in dev / 548 in release** (engine 59, client 181 unit tests in dev), and the three
goldens bit-identical (`0xb6fff6659cfb7709`, `0x6e9a18bd7c5f699f`,
`0x9bc18c521107b262` / map `0xd38136401ab02ff1`). The windowed client was then run
for real under the AI-Handoff §9 recipe (Xvfb + llvmpipe, GL backend, userland EGL;
one environment deviation: Xvfb needed `-ac`, this sandbox reaps background processes
so Xvfb and the client must share one invocation) — 900 frames presented at the main
menu, clean exit. Three screenshots were captured through the running window
(`docs/product/screenshots/`) and every "observed" citation below traces to that run
or its evidence lines.

**Evidence conventions.** `file:line` cites the code at `23012bf`; "test" names a
test that exists in the repo; **observed** means this run produced the fact; a doc
name cites the document. Severity: **R1-blocker** (R1 cannot honestly ship without
it), **R1-should** (R1 ships noticeably worse without it), **later** (post-R1 unless
evidence promotes it). Size: **S** (a small focused change), **M** (a phase-sized
work package), **L** (multi-phase).

---

## 1. Install

Today "installing" means: install rustup, clone, `cargo run`. That is the correct
shape for testers and contributors, and it is what `docs/PLAYTEST.md`'s quickstart
assumes — but it is not a product. A player-facing R1 needs something double-clickable.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Binary distribution | None. `.github/workflows/` holds `ci.yml`, `nightly.yml`, `audit.yml` only — no release workflow, no artifacts, no channels | R1's defining deliverable: per-OS builds from tagged commits | **R1-blocker** | M (L if all three OS are first-class) |
| Versioning | Workspace `version = "0.1.0"` (`Cargo.toml:6`); **zero git tags**; `CHANGELOG.md` keys entries to milestone tags that do not exist in the repo | A shipped build must be identifiable: tag, version string, and a visible build id (see §3) | **R1-blocker** | S |
| License | "No license has been published yet" (`README.md`, License section); the brief fixes the wording to "to be decided by the owner" | Nothing can be redistributed until the owner decides terms | **R1-blocker** | S (owner's decision; docs follow) |
| Install footprint | The binary resolves content from the crate manifest (`main.rs:208` `content_path()` walks from `CARGO_MANIFEST_DIR`); run the raw exe elsewhere and it cannot find the content tree | exe-relative (or installed-asset) content discovery | **R1-blocker** (pairs with packaging) | S |

## 2. Launch

The launch path is honest and debuggable for a source checkout, and the no-display
fallback is genuinely graceful. For a packaged product the failure modes are still
console-shaped.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Launch from source | `cargo run -p pandemonium-client` — windowed on a display, else the built-in headless smoke pass with an honest one-line explanation (`main.rs:109-120`) | none for the source path | — | — |
| **observed** | 900-frame windowed smoke under Xvfb: settings line, null-audio arm line, `windowed smoke: 900 frames presented, at the main menu`, clean exit 0 | the launch path is real and evidenced | — | — |
| Startup failure surface | Content load failure → anyhow chain to stderr, exit 1 (`main.rs:97-107`); empty map list → error; GPU adapter request failure → `.context("requesting a GPU adapter (software rasterizers count)")?` (`render.rs:499-514`) | A double-clicked packaged app has no console: every startup failure is an invisible flash and exit. A product needs a visible error surface (window or dialog with the anyhow chain) | **R1-should** (becomes a blocker the moment R1 ships console-less binaries) | M |
| Window defaults | winit default 800×600 logical, title "Pandemonium" (`main.rs:1219`; **observed** in the captures) | No window-size setting, no maximized start, no remembered window geometry | later | S |

## 3. Title / menu

The menu-first flow landed in M10.2 Phase 3 and is machine-verified plus owner
re-tested (A-119). What exists is a clean, fast, keyboard-and-mouse-navigable menu;
what is missing is the furniture players expect around a title screen.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Main menu | `MainMenu -> (Settings | NewMatch)` (`screens.rs:1-18`); rows New Match / Settings / Quit; focus starts at row 0; hover moves focus; keyboard and mouse share one navigation core | nothing structural | — | — |
| **observed** | `screenshots/menu.png` — "PANDEMONIUM" panel over the default map backdrop, three rows, fully legible | real capture, suitable for the README | — | — |
| Version / build identity on menu | Absent — no version string anywhere in the UI | A support conversation starts with "what build are you on?" — the menu (and F8 sidecar) should carry it | **R1-should** | S |
| Menu ambience | None — audio is event-cue-only by design (ADR-0002); menus are silent | Menu music / ambience is a product-phase content question, not a system gap | later | M |
| Quit confirmation | Esc at the main menu quits directly (AI-Handoff §9, menu gotchas); there is no live match at the menu, so nothing is lost today | Once a "quit to menu" mid-match exists it needs a confirm (see §8) | later | S |
| Credits / about / links | Absent | Standard product furniture; the docs exist and are one row away | later | S |

## 4. Match setup

The New Match screen is complete for the Alpha's one-map, one-faction, one-AI world
and is a pure state machine (`screens.rs` — `NewMatchState` owns mode, seed text,
map index; `MatchMode::label()` at `screens.rs:40-46`).

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Modes | Player vs AI / AI vs AI (spectate) / Sandbox (`screens.rs:25-35`), both slots always present (A-112) | no difficulty tiers — the scripted AI is single-level by Alpha scope | later | L |
| Seed | Editable field, Enter randomizes, `--seed N` prefills (`main.rs:167-`, `screens.rs` seed field); the seed is shown so it can be reported | none — this is a product-grade reproducibility affordance already | — | — |
| Map | Row lists the content tree's maps (`ContentTree::load_dir`, `main.rs:93-107`); Alpha ships one (Crossroads) | no map preview image; no map metadata on the row (size, players) — data exists in the map file | **R1-should** | S |
| Setup memory | In-run only: the prefilled seed and mode carry across restarts in the session (`main.rs:2073`); `SETTING_KEYS` (`config.rs:73-82`) has no seed/map keys, so choices do not persist across runs | "Remembers your last setup" is a small, felt product touch | later | S |
| Player identity | No player name, color choice, or faction pick (one faction, fixed colors) | later by the plan's own arc (§17: factions-as-data) | later | M |
| Loading transition | None — bundle+world construction is instant at Alpha scale | needs eyes only if R1 content grows | later | S |

## 5. Play

The match loop is the Alpha's proven core: all eleven sim stages live, the M10.2
controls/legibility/menus/audio passes are owner-re-tested clean (A-101, A-107,
A-119, A-126), and A14 — five first-time tester slots, scheduled in
`docs/PLAYTEST.md` §7 — is the pending judgment. This section deliberately does not
re-litigate feel; it lists what is *known* open.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| **observed** | `screenshots/match.png` — HUD (Ore 200 / POP 4/10), command card hint, standing controls line, terrain, units, fog | the loop is visually real | — | — |
| Known sim/behavior limits | formation-less jams (A-040), staircase paths, stalled construction when the builder dies (A-045), scripted-AI sight latency (A-062) — AI-Handoff §8 "What is NOT built yet" | triage belongs to A14 findings, not pre-judgment | later (promote on evidence) | M–L each |
| Perf headroom | §15 budgets held in release (`docs/ALPHA_DECLARATION.md`, Performance baselines); O(N·V) fog recompute is the known scaling wall | fine at Alpha scale | later | M |
| In-match help | The standing controls line (`report.rs` `CONTROLS_LINE`, mirrored in `ui.rs`), the armed-attack-move instruction, refusal notices with reasons | A pause-menu "Controls" screen would make the reference reachable without leaving the match (see §12) | **R1-should** | S |

## 6. Pause

Pausing is solid and honest: the pause menu owns input, the dump works while paused,
and the A15 restart path is tested. The one product-shaped hole is focus behavior.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Pause menu | Resume / Settings / Restart / Quit to Menu (`screens.rs` PauseMenu rows); `P` pauses without the menu; the sim pauses through `MatchHost::set_paused` (`host.rs:203`) | none structural | — | — |
| Focus loss / minimize | `WindowEvent::Focused` is recorded and nothing else (`main.rs:1782-1784`); the event loop keeps advancing the host while unfocused or minimized | Classic RTS behavior is auto-pause on focus loss (at least when not spectating) — today a minimized player returns to a match that ran on without them | **R1-should** | S |
| Pause state legibility | The HUD prints paused state (debug overlay / evidence lines); the visual "PAUSED" treatment is minimal | A first-time player should never wonder whether time is moving | **R1-should** | S |

## 7. End screen

The end screen exists and the loop closes (A15; `EndScreen` with
VICTORY/DEFEAT/MUTUAL DESTRUCTION, Rematch/Main Menu). What it lacks is the summary
players read *why* they won or lost — and the raw material for that summary already
flows through the event stream every match.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Verdict panel | End screen with outcome + Rematch (R) / Main Menu (README M8 section; `screens.rs` EndScreen) | none for the verdict itself | — | — |
| Match statistics | Absent — no duration, units produced/lost, resources gathered, peak pop. The events to derive all of them are already emitted and logged (FD-9, `sim_api` events; the command log exists via `MatchHost::log()`, `host.rs:231`) | The cheapest large "feels like a product" win on this list: a stats panel from events, no sim change, goldens untouched | **R1-should** | M |
| Replay handoff | `--record` writes a verifiable `.pdrp` at match end (`report.rs`), but the end screen offers no "keep this replay" affordance; a recorded file is already in hand at that moment | one row: "replay saved: <path>" (or an opt-in) | later | S |

## 8. Restart / quit

Restart is the A15 drop-and-reconstruct path, tested bit-identical. Quit paths are
all present; they are merely *unprotected*.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Rematch / restart | End screen R; pause menu Restart; two fresh hosts from one seed hash identically (`tests/match_rules.rs`, A15) | none | — | — |
| Quit to menu mid-match | Pause menu row, and the Esc ladder reaches it (AI-Handoff §9) — the live match is discarded with **no confirmation** | A mis-press (Esc ladder + Enter) throws away a 10-minute match; a confirm row or a two-press guard is the fix | **R1-should** | S |
| Window close | Graceful: the exit line reports how the run ended (**observed** pattern in the smoke; `exiting` handler, `main.rs`) | none | — | — |

## 9. Settings

The settings system is one of the strongest product surfaces the client has: eight
keys, live application, forgiving parse, per-OS paths, and a test wall behind it. The
gaps are the settings a *product* adds next — none of them structural.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Persistence | `key=value` file, per-key salvage, range clamps, unknown keys ignored, deterministic serialization (`config.rs:88-164`, tests `config.rs:319-593`); Done saves, Esc discards (A-115) | none at this key count — the ADR compares formats for the growth case (`docs/adr/0100`, part b) | — | — |
| **observed** | Startup line: `settings: /home/z/.config/pandemonium/settings.cfg (absent, defaults the file)`; no directory is created until the first save | graceful, honest, exactly as documented | — | — |
| Live application | volume/mute live on the sink, fullscreen live on the window (`main.rs:723-751`), edge scroll and pan speed read live | camera feel keys apply without restart | — | — |
| Missing settings | UI scale (see §13), key rebinding, window geometry, language, audio device choice | prioritized below; UI scale is the accessibility-critical one | **R1-should** (UI scale only) | S (UI scale) / later (rest) |
| Settings screen UX | Arrows/click step values; booleans toggle; steps clamp at range edges (`config.rs:195-219`, tested) | a typed-value editor is overkill at eight keys | later | M |

## 10. Error cases

The brief's enumerated list, one row each. The pattern across all of them: the
*mechanisms* exist and are graceful — but the *player-visible* surface is a console
line, and a packaged player has no console.

| Case | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| No GPU adapter | `.context("requesting a GPU adapter (software rasterizers count)")?` then exit 1 (`render.rs:499-514`); llvmpipe counts as an adapter, so true no-adapter is rare | visible error surface (see §2); a fallback-to-llvmpipe hint line when the adapter is software | **R1-should** | S |
| Missing content | anyhow chain with the attempted path, exit 1 (`main.rs:97-107`) | unreachable from a cargo run, reachable from a packaged exe (see §1) — same friendly-surface fix | **R1-should** (blocker with packaging) | S |
| Audio failure | Two-arm sink: startup probe with a `/dev/snd` pre-check (`sound.rs:521-541`, A-124), null fallback counts cues, run continues — **observed** under Xvfb | The player is never *told* the game is silent: the arm is reported to stdout only. A one-row settings/menu notice ("audio: no device found") closes it | **R1-should** | S |
| Resize | `renderer.resize` + camera aspect on every `Resized` (`main.rs:1247-1253`) | none | — | — |
| Minimize | No handler (no `Minimized` match arm in `main.rs`) | pairs with auto-pause (§6) | **R1-should** (with §6) | S |
| DPI / scale factor | Zero handling — no `scale_factor` reference in the client; the UI lays out against physical pixels (`window.inner_size()` throughout `main.rs`/`ui.rs`), and the embedded font rasterizes at fixed sizes (`text.rs`) | On HiDPI displays the window renders at 2× physical pixels with same-size UI text → small text; on fractional-scale Wayland, blur. Needs a UI-scale factor derived from winit's scale factor (see §13) | **R1-should** | M |
| Fullscreen | Startup + live toggle, borderless (`main.rs:723-751`, applied at `main.rs:1233-1235`) | Alt+Enter not bound; exclusive fullscreen not offered (borderless is the right default) | later | S |
| Panic path | No custom panic hook; a panic in the client prints to stderr and exits — while the sim crates' invariants are soak-policed, client-side panics lose the F8 evidence | A panic hook that dumps the F8 report before exiting turns every client crash into a filed repro | **R1-should** | S |

## 11. Logs, config, and report locations per OS

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| Config file | Windows `%APPDATA%\pandemonium\settings.cfg`; macOS `$HOME/Library/Application Support/pandemonium/settings.cfg`; Linux `$XDG_CONFIG_HOME` else `$HOME/.config` (`config.rs:234-288`, tested at `config.rs:437-482`); **observed** on Linux | none | — | — |
| Config with no home | In-memory defaults, persistence silently disabled, never a panic (A-109) | none — correct for CI/containers | — | — |
| Log output | ~25 `println!`/`eprintln!` evidence lines to stdout/stderr (`main.rs`, `sound.rs`) — settings path, audio arm, smoke/exit summaries, record failures | No persistent log file, no timestamps, no level filter: a support request currently has nothing to attach | **R1-should** | S (tee the existing lines to a per-run file in the config dir) |
| F8 report files | `pandemonium-report-<seed>-tick<tick>.pdrp` + `…-info.txt` beside `--record`'s path or in the working directory (`main.rs:812-817`, `report.rs:138-142`), deterministic bytes (test in `report.rs`) | CWD is wherever the player launched from; a stable reports dir under the config root is friendlier and greppable | **R1-should** | S |

## 12. Controls help

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| In-match reference | Standing one-line card at the HUD's edge (`report.rs` `CONTROLS_LINE`, rendered by `ui.rs`; **observed** visible in `screenshots/match.png`), plus the armed-mode instruction line and refusal reasons | none for the line itself | — | — |
| Card accuracy | The card mirrors the README/PLAYTEST copy (M10.2 Phase 1 re-test A-101 pinned the controls themselves) | keep the three copies in sync mechanically (a shared const already exists client-side) | later | S |
| Dedicated screen | None — the line vanishes in menus/pause | a Controls screen (pause menu row / main menu row) is the S-sized fix and the A14-friendliest one on this list | **R1-should** | S |

## 13. Accessibility basics

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| UI scale | None — fixed-px UI metrics, fixed-size font atlas (`text.rs`, `ui.rs`) | The UI-scale factor (default from winit `scale_factor`, overridable in settings) is the legibility feature for HiDPI and low-vision users alike; composes with §10 DPI | **R1-should** | M |
| Team colours | Blue/orange per M10.2 Phase 2 (AI-Handoff §2) — the standard CVD-safe pairing for deuteranopia/protanopia; selection rings, team rings, minimap markers inherit it | Not declared, not tested against a CVD simulation, and no shape/pattern redundancy exists for the total-acromat edge case; a one-page audit + optional icon-on-minimap closes it honestly | **R1-should** | S |
| Input remapping | None — fixed Generals-ZH map (`input.rs`) | later by any reasonable R1 scope | later | M |
| Motion / flashes | Hit flashes exist (toward white, short); no screen shake; no reduced-motion need recorded | photosensitivity review is cheap and empty of findings at current effect intensity — record that | later | S |

## 14. Hard-coded strings

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| String inventory | All UI text is English ASCII literals in place (`ui.rs` ~124 quoted lines, `screens.rs` labels, `main.rs` evidence lines, `report.rs` consts) | No string table: text is scattered, and any future copy pass or localization touches everything | later (centralize S; localize L) | S / L |
| Font constraint | "Pandemonium Sans" is an ASCII subset of DejaVu Sans (`text.rs:8`); non-ASCII renders a fallback box (`text.rs:166,212`) — tested (`text.rs:252`) | Any non-ASCII or non-Latin UI text is impossible until the font subset grows; licensing is already clean (bundled LICENSE.txt) | later | M |

## 15. Engineering health that gates R1

These are not player-journey rows, but they bound how fast the R1 rows above can be
delivered, and they are cited by the ADR.

| Item | Current state | Gap | Severity | Size |
|---|---|---|---|---|
| DEBT-013 split | `main.rs` 3231 lines, `render.rs` 2698, `ui.rs` 1461, `screens.rs` 1167 (wc -l at `23012bf` — the register's 2473/1782 are pre-M10.1 and stale); the intended module map already exists (`docs/ARCHITECTURE.md` "Post-alpha refactor map") | The ADR proposes the behaviour-preserving step plan (part c); the sizes make every product row above slower to land until it happens | **R1-should** (the plan: ADR 0100 part c) | M |
| DEBT-016 / DEBT-017 | Silhouettes hardcode the nine shipped kinds; terrain mesh is built from the tree's first map | Both fire on content growth — R1 content plans must schedule them | later (tripwired) | S each |
| Golden discipline | Three goldens bit-identical through M10.2 (**observed** again in this audit's baseline) | none — the discipline holds | — | — |
| Registers | A-127 latest assumption; open debt DEBT-002/008/012/013/016/017/018 | DEBT-013's registered sizes drifted from reality (noted above; this audit does not edit registers beyond its own entries) | later | S |

---

## The ordered top 10 for R1

Ordering rationale: first the things without which "R1" is not a sentence (ship,
identify, install, judge), then the cheapest large product wins, then the
supportability floor. Sizes are the audit's S/M/L.

1. **Release pipeline** (M) — `.github/workflows/release.yml`, tagged builds, per-OS
   artifacts; the R1-blocker that defines everything else's delivery channel.
2. **License decision** (S, owner) — "to be decided by the owner"; every
   redistribution path waits on it.
3. **Exe-relative content discovery + visible startup errors** (S+M) — the packaged
   app must find its content and must *show* the anyhow chain when it cannot; today
   both are console-only (`main.rs:97-107`, `render.rs:499-514`).
4. **A14 execution and findings triage** (the scheduled playtest) — R1's scope
   setters; five honest rows in `docs/PLAYTEST.md` §7 decide what "play" needs.
5. **End-of-match statistics panel** (M) — derived from the event stream that already
   flows (FD-9); no sim change, no golden movement, the largest perceived-product
   gain per hour on this list.
6. **Auto-pause on focus loss / minimize + quit confirmation** (S) — the two cheap
   guards that stop a product from *losing a player's match* (`main.rs:1782`,
   pause-menu quit path).
7. **UI scale / HiDPI** (M) — one scale factor from winit, one settings row, one
   accessibility win (`text.rs`, `ui.rs`, §10/§13).
8. **In-game Controls screen** (S) — the reference exists as a line; make it a
   screen reachable from pause and the menu (§12).
9. **Audio-device status surfaced in UI** (S) — the null arm already knows; tell the
   player instead of the console (§10, `sound.rs:521-541`).
10. **Supportability floor** (S/M) — tee the evidence lines to a log file in the
    config dir, move F8 reports to a stable reports dir, and hook client panics to
    the F8 dump (§10/§11).

Not in the top ten and not forgotten: the DEBT-013 split rides as ADR 0100 part (c)
— an engineering-health investment that makes rows 5–10 cheaper to build, scheduled
at the PM's discretion rather than the player's.
