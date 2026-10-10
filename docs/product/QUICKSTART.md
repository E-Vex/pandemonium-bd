# Pandemonium — quickstart for playtesters

This archive is a self-contained Alpha build: the game, its data, and the
licences for everything inside. Unpack it anywhere and run the client —
no installer, no registry, no dependencies except the one Linux library
noted below.

## What is in here

```text
pandemonium-<version>-<os>-<arch>/
├─ pandemonium-client     the game (windowed; falls back to a smoke pass without a display)
├─ pandemonium-tools      headless runner + content validator (dev/diagnostic tool)
├─ content/               the game's data tree (units, rules, map, factions) — keep next to the client
├─ README.md              the project README (status, build-from-source, architecture)
├─ QUICKSTART.md          this card
└─ THIRD_PARTY_NOTICES.md every third-party licence shipped in the binaries (incl. the font's)
```

The client finds `content/` **next to its own executable** — keep them
together, and start the game from anywhere. If you moved things around,
point at the tree explicitly: `pandemonium-client --content /path/to/content`.

## First run

**Windows** — double-click `pandemonium-client.exe`, or run it from a
terminal to see the startup lines. The build is unsigned, so SmartScreen
will interpose once: click **More info → Run anyway**. See
[RELEASING.md](RELEASING.md#smartscreen-and-gatekeeper) for the why and the
one-time steps.

**macOS** — run `./pandemonium-client` from Terminal (the binary is
unsigned; Gatekeeper blocks double-click starts). The one-time bypass:
`xattr -d com.apple.quarantine pandemonium-client` (or right-click → Open).
This build is aarch64 (Apple silicon).

**Linux** — `./pandemonium-client` from a terminal. One runtime dependency:
`libasound2` (the audio library; most desktops have it). Without it the
game still runs — see below.

## Controls (Generals: Zero Hour muscle memory)

**Selecting** — **left-click** selects, **left-drag** box-selects; left never
issues an order, and a click on empty ground does nothing (**Esc** clears
the selection). **Shift+click** adds or removes one unit, **shift+drag** adds
the box's picks, **double-click** selects every visible unit of the same kind.

**Commanding** — **right-click** is the only way to command: on an enemy it
attacks, on an ore node it gathers (workers), on open ground it moves, and on
a single selected producer it sets the rally point. A click under 6 px of
travel orders; **right-drag** beyond that grabs and scrolls the map (the
release then orders nothing). **A** arms attack-move for the next left-click,
**S** stops.

**Camera** — **right-drag** scrolls, **middle-drag** rotates, **WASD /
arrows / screen edges** pan, **Q / E** rotate, the **wheel** zooms toward the
cursor, **Ctrl+wheel** tilts the pitch.

**Groups & jumps** — **1–9** recall control groups, **Ctrl+1–9** assigns,
double-tap centers on the group. **Space** jumps to the last death (or your
base). **Esc** climbs one rung per press: armed command → placement →
selection → **pause menu**. **P** pauses without the menu, **F3** toggles the
debug overlay, **F8** writes the bug-report dump, **R** restarts after the
match ends.

## Telling us what broke (the 30-second version)

1. Press **F8** in-game — a bug report is written next to the executable.
2. Note the build: run `pandemonium-client --version` (or read the F3
   overlay's first line) — it prints the version, the exact commit, and the
   content hash.
3. The seed you played: set it yourself in New Match (or pass `--seed N`).

## Troubleshooting

| Symptom | What it means / what to do |
|---|---|
| No sound anywhere | The audio device probe found nothing or failed — the game runs silent by design, one startup line says so. If the probe itself misbehaves on your machine: `PANDEMONIUM_AUDIO_PROBE=0` skips it entirely. |
| "content directory not found" on startup | The client can't see `content/`. Keep it next to the executable, or pass `--content <dir>`. The error lists every path it tried. |
| Linux: the game will not start at all, a loader error names `libasound.so.2` | Install the ALSA runtime library: Debian/Ubuntu `sudo apt install libasound2t64` (older releases: `libasound2`), Fedora `sudo dnf install alsa-lib`, Arch `sudo pacman -S alsa-lib`. This is the ONE runtime dependency; without it the loader refuses before the game's own (graceful) audio fallback can engage. |
| A windowed run ends with "GPU initialization failed" | Your machine offered no usable graphics adapter (wgpu found nothing). The message quotes the underlying error — include it in the report. |
| You just want the smoke pass | `pandemonium-client --headless` — no window, no display needed, prints the evidence lines. |

## The tools binary (optional)

`pandemonium-tools` is the developer tool: `content-validate` re-checks the
shipped data tree, `headless --seed 7 --ticks 300` prints a reference match,
`replay-verify <file>` re-simulates a recorded match. Testers mostly need it
for `content-validate` when moving the archive around.

## Linux runtime dependency (the fine print)

The client's audio stack links against `libasound2` (ALSA). The game's own
no-audio fallback is silent and non-fatal **once the process starts** — but
the dynamic loader checks the library at load time, so a machine without
`libasound.so.2` cannot start the client at all. Install it (table above);
audio then either works or degrades honestly, never crashes.
