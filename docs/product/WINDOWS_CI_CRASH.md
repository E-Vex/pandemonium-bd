# The Windows CI crash — findings (A-002 Part 1)

Status: **fix landed on this branch; Windows-leg CI verification pending**
(the push token was lost with this session's context reset — see the A-002
report's blocker section; the isolation workflow below is staged and runs
on the first push).

## The evidence (what is actually known)

- **Signature (from B-001's report, quoting the run logs):** on every ci
  run ever, both Windows test legs fail the same way — the
  `pandemonium-client` **unit-test binary** starts ("running 181 tests",
  first test `audio_wiring::activating_a_menu_row_clicks_the_ui_cue ... ok`)
  and then the process dies with **`STATUS_ACCESS_VIOLATION` (0xc0000005)**.
  `cargo test` stops at the first failing binary, so the acceptance suites
  never run on Windows. Same signature at `08748d9` — **before rodio
  existed** (there was no `sound.rs`; `App.audio` was the pure
  `NullAudioSink`) — so the crash is not an ALSA/rodio artifact of the
  M10.2 audio landing.
- **Master baseline (my own API evidence, run
  [38044206117](https://github.com/E-Vex/pandemonium-bd/actions/runs/38044206117),
  sha `9d3874a`):** `tests (windows-latest, dev)` and `tests
  (windows-latest, release)` both fail (the test-suite step, after ~2–4
  minutes of building and running); every other job is green —
  **including `goldens (windows-latest)`, which builds and runs the
  tools binary on the same runners.** The crash is specific to the
  client test binary, not to Rust-on-Windows-runner generally.
- **Code-level inventory (this branch, verified by reading every test
  module):** after the fix below, **no client unit test touches wgpu,
  winit, or rodio.** Before it, exactly one test path reached real
  hardware: `audio_wiring`'s helper constructed a full `App`, whose
  constructor probed the audio device (`Sound::new` → rodio/WASAPI) —
  five tests, concurrently, each opening a real output stream, then
  discarding it for the null arm. That is the only FFI reachable from
  the current test binary — and it did not exist before the audio
  landing, so it cannot be the *original* crasher, though it could be a
  second crash site with the same external signature.

## What the fix landed (regardless of which site crashes)

1. **Tests never touch the machine's hardware.** `App::new` receives its
   audio sink from the wiring layer (`main()` probes exactly once —
   product behavior unchanged); the `audio_wiring` tests construct
   `Sound::without_device` directly. A unit test opening a real audio
   output stream was a machine-leak into the test environment even where
   it did not crash — and would have *played sound* during `cargo test`
   on a machine with a device.
2. **Product degradation paths, never a panic/abort:**
   - window creation failure → logged precise error + clean exit (the
     GPU path's existing contract);
   - audio probe failure → the documented silent null arm (unchanged);
   - `PANDEMONIUM_AUDIO_PROBE=0` → the documented escape hatch that
     skips the probe entirely (a player whose audio stack misbehaves
     gets a silent, running game).
3. **The hardware-test gate** (`crates/client/src/hw.rs`): tests that
   need real GPU/audio run only where hardware is *positively* plausible
   (Linux: display session + `/dev/dri` for GPU, `/dev/snd` for audio),
   else they skip cleanly and visibly. `PANDEMONIUM_HW_TESTS=1|0`
   overrides in either direction. Two canaries live behind the gate:
   wgpu adapter enumeration (render.rs) and the real rodio probe
   (sound.rs).

## What remains (the CI half of Part 1)

The brief's method — push diagnostic commits, watch the Windows legs —
is staged as `.github/workflows/a002-diagnostics.yml` (runs on
`a/A-002-*` pushes; **temporary by design, removed in the cleanup
commit**). On its first run it answers, with per-test exit codes and
full backtraces:

1. `--help` / `--list` on the raw test binary — process-start/exit vs
   in-test (the pre-rodio signature can only be the former or a
   dependency-level teardown issue, since no pre-rodio test reached FFI);
2. every test one at a time (`RUST_BACKTRACE=full`) — the crashing test
   named, or none;
3. the suite serial vs parallel — test-thread interaction vs a lone
   crasher;
4. the hardware gate deliberately **open** — wgpu adapter enumeration
   and the real rodio probe run on the GPU-less, endpoint-less runner:
   if the process dies there, the log names the call. This is the
   rule-in/rule-out the brief asks for (wgpu instance/adapter creation;
   rodio/cpal output-device creation; unsafe/FFI; test-thread
   interactions);
5. the fix's claim — `PANDEMONIUM_AUDIO_PROBE=0` with the audio wiring
   still green.

**Honest limitation:** the sandbox that produced this branch has no
Windows execution and (this session) no push credentials; the
root-cause statement above is therefore *evidence-bounded*, not
*CI-verified*. The one-at-a-time isolation will either name the
crashing test (then the root cause is in reach) or show every test
green post-fix (then the fix was the cause and the Windows legs go
green — the acceptance criterion). If the canaries crash the process
with the gate open, the root cause is a wgpu- or cpal-level defect on
GPU-less Windows — **that would be a player-facing defect** (the same
call runs on real machines), the finding gets flagged to the PM, and
the fix moves from "contained change" to "upstream/dependency"
territory (a wgpu or rodio version decision, PM approval required).

## The standing suspicion, stated honestly

The pre-rodio crasher is *not yet named*. Candidate classes, in
evidence order: (a) a dependency-level crash triggered by the test
binary's linked-but-dormant graphics stack at process start/exit
(wgpu/winit's `windows`-crate imports — the binary links three
`windows`-crate generations); (b) the rodio probe (post-audio only —
but it is *reachable* now and five tests hit it concurrently);
(c) something in a test that the code inventory missed (the
one-at-a-time run falsifies this in minutes). The diagnostics workflow
discriminates all three in a single run.
