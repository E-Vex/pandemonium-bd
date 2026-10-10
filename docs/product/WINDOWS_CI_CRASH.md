# The Windows CI crash — root cause, proven (A-002 Part 1)

Status: **closed.** Proven on the A-002 branch (run
[38050523319](https://github.com/E-Vex/pandemonium-bd/actions/runs/38050523319),
sha `7e4cd1e`: `tests (windows-latest, dev)` and `tests (windows-latest,
release)` both pass, 194/194 client tests, alongside every other job), and
re-proven on the merged master by A-003 (the section below): the gate run
plus three re-runs, all four attempts green in every job, both Windows
test legs included.
The root cause is named below, with the evidence chain that named it.

## The root cause, in one paragraph

**Two or more real rodio/cpal WASAPI probe rounds in one process, on a
machine with no audio endpoint, crash the process with
`STATUS_ACCESS_VIOLATION`.** The first probe round survives and honestly
reports "no device"; the second dies inside the cpal/WASAPI host path.
Master's client test binary ran **five** probe rounds (five
`audio_wiring` tests each constructed a full `App`, whose constructor
probed the device), so it crashed. This was a **test-architecture
defect** — a leak of the machine into the test environment — not a
product defect: the shipped binary probes exactly **once**, before the
GPU stack wakes up, and is verified clean end-to-end on the exact
machine class that crashes under multi-probe (cell M5 below).

## The corrected timeline (primary logs, not reports-about-logs)

B-001's baseline claimed the same crash signature existed at `08748d9`,
"before rodio existed," implying a pre-audio cause. The primary logs
contradict that. Every row below was read from the job logs directly:

| master commit | rodio in client | client test binary on windows-latest | the Windows leg fails on |
|---|---|---|---|
| `08748d9` (run [#38](https://github.com/E-Vex/pandemonium-bd/actions/runs/37759469619), Oct 8 09:50) | absent | **161/161 green** | `content_pipeline::add_a_unit_is_data_only` — exit 1, no crash (see below) |
| `39fcefa` (sound.rs lands) → `652f6cf` (run [#39](https://github.com/E-Vex/pandemonium-bd/actions/runs/37818836200)) | present, 5 probing tests | 181 tests start, first passes, then **`0xc0000005`** | the crash (which masked the CRLF failure: `cargo test` stops at the first failing binary) |
| runs #40, #41 … master `9d3874a` (run [38044206117](https://github.com/E-Vex/pandemonium-bd/actions/runs/38044206117)) | present | same signature, 41/41 runs | same |
| this branch `5e74732` (fix `a2bc546`: tests no longer probe) | present, **0 probing tests** (1 honest probe remains: `the_no_device_constructor` pins "the real constructor must not panic") | **194/194 green** | the CRLF test bug alone — the crash is gone |
| this branch `7e4cd1e` (CRLF fix) | present | **194/194 green** | **nothing — all legs green** |

The crash appeared exactly when the five probing tests landed and
disappeared exactly when they stopped probing. The "predates rodio"
hypothesis was an artifact of reading failure *conclusions* rather than
failure *logs*: run #38's Windows legs were red (the CRLF bug), which
reads as "same failure" from a distance.

## The isolation evidence (round 1, run 38049622201)

On windows-latest, dev profile, the client test binary:

1. **Process start/exit: clean.** `--help` and `--list` exit 0.
2. **Every test alone: clean.** All 194 tests, one at a time,
   `RUST_BACKTRACE=full`, exit 0.
3. **The whole suite, serial, hardware gate closed: clean.** 194/194,
   exit 0.
4. **Each hardware canary alone (gate open): clean.** wgpu adapter
   enumeration answers (the WARP software adapter); the rodio probe
   finds no device and returns.
5. **The whole suite, serial, gate open (`PANDEMONIUM_HW_TESTS=1`):
   dies.** Exit 139 — in `sound::tests::
   the_no_device_constructor_takes_the_null_arm_without_panicking`,
   at its final line (`Sound::new` — the real probe), *after* the
   canary's probe had already run earlier in the same process.

## The interaction matrix (round 2, run 38051093535)

| cell | configuration | exit |
|---|---|---|
| M1a | audio canary alone — 1 probe | 0 |
| M1b | `Sound::new` probe alone — 1 probe | 0 |
| M1c | wgpu adapter request alone | 0 |
| **M2** | **two probes, NO wgpu** | **139 — crash mid-test** |
| **M3** | **wgpu request, then one probe** | **0** |
| M4 | full suite, serial, gate open (≥2 probes) | 139 |
| **M5** | **the real binary: windowed startup, probe → EventLoop → wgpu/WARP → 30 frames → menu exit** | **0, clean exit** |

Verdict: **H-count confirmed, H-order refuted.** The crash follows the
*number of probe rounds* (M2 dies with no GPU involvement at all), not
the wgpu/probe ordering (M3 lives). wgpu was a red herring in the
gate-open signature — it merely shared the process.

## The product's clean bill (and its one caveat)

- The shipped binary probes **once** (`main.rs`: the probe feeds
  `App::new`; the App constructor itself never probes), *before* the
  event loop and GPU init. The design invariant "reset never re-probes"
  (the M10.2 gotcha list) already guarded the other side of this cliff.
- **M5 is the direct proof on the dangerous machine class**: the
  GPU-less, endpoint-less Windows runner — the brief's "weird GPU, no
  audio" corner — runs the real binary end-to-end: honest one-line
  fallback ("audio: no device — null fallback"), 30 presented frames on
  WARP, clean exit at the menu. Exit 0.
- `PANDEMONIUM_AUDIO_PROBE=0` remains the documented escape hatch for
  any player whose audio stack misbehaves in the single probe.
- **Caveat, stated honestly:** the exact faulting frame inside
  cpal/WASAPI was never captured — a raw 0xC0000005 yields no backtrace,
  and naming it would need a Windows debugger/procdump run (out of this
  pass's scope, and unnecessary for the fix). And machines with *real*
  audio endpoints follow a different code path through the probe (a
  stream is built), which CI cannot represent; the multi-probe defect
  itself is proven, its absence on endpoint-having machines is the
  expected-but-unverified direction.

## The second, older Windows red: the CRLF bug

With the crash gone, the legs were still red — and had been since
before B-001's baseline, masked by the crash aborting the workspace run
before `pandemonium-tests` executed. `add_a_unit_is_data_only` edits
`content/factions/legion.ron` by exact string replacement; two of its
three needles span line breaks. Git-for-Windows checkouts default to
`core.autocrlf=true` (the repo pins no `.gitattributes`), so the
working tree had CRLF: the single-line barracks needle matched, the
multi-line roster needle silently did not, and the loader correctly
rejected the half-edited faction ("production trains 'skirmisher'
which is not in the roster"). The fix normalizes the read to LF before
the needles. Run #38's logs show this failure verbatim, pre-rodio —
it is the oldest Windows red in the repo's history.

## What the fix landed (summary)

1. **`a2bc546` — tests never touch the machine's hardware.** `App::new`
   receives its audio sink; the wiring layer (production `main`, or the
   test's explicit `Sound::without_device`) owns whether a probe ever
   runs. One probe per process maximum, by construction.
2. **Product degradation paths** (window-creation failure → logged clean
   exit; probe failure → silent null arm; the env-var escape hatch) —
   no panic path was added anywhere.
3. **The hardware-test gate** (`hw.rs`) keeps genuinely
   hardware-dependent tests behind a positively-plausible check with
   visible skips and a `PANDEMONIUM_HW_TESTS` override — so this class
   of machine-leak cannot silently return.
4. **`7e4cd1e` — the CRLF fix** (test scaffolding, `tests/
   content_pipeline.rs`).

---

## A-003 close-out — the stability evidence on the merged master (2026-10-11)

The question this section closes: does the merged master's Windows CI stay
green, or was the A-002 close-out a lucky pass? The merged master is
`2c94a9b` (PR #4, the B-002 runner pinning). Its push-triggered gate run —
[38071980300](https://github.com/E-Vex/pandemonium-bd/actions/runs/38071980300)
— was green in all fourteen jobs; that run was then re-run three more times
back-to-back (the Actions re-run API re-runs a whole run, so every attempt
below re-ran all fourteen jobs; the two Windows test legs are the ones this
document tracks, and they were green in every single attempt):

| attempt | `tests (windows-2025, dev)` | `tests (windows-2025, release)` | every other job |
|---|---|---|---|
| 1 (the merge push, 17:29 UTC) | green — 573/573 | green — 568/568 | all green |
| 2 (re-run, 17:43) | green — 573/573 | green — 568/568 | all green ([dev](https://github.com/E-Vex/pandemonium-bd/actions/runs/38071980300/job/114273456621), [release](https://github.com/E-Vex/pandemonium-bd/actions/runs/38071980300/job/114273456497)) |
| 3 (re-run, 17:49) | green — 573/573 | green — 568/568 | all green ([dev](https://github.com/E-Vex/pandemonium-bd/actions/runs/38071980300/job/114275274199), [release](https://github.com/E-Vex/pandemonium-bd/actions/runs/38071980300/job/114275274171)) |
| 4 (re-run, 17:57) | green — 573/573 | green — 568/568 | all green ([dev](https://github.com/E-Vex/pandemonium-bd/actions/runs/38071980300/job/114276905627), [release](https://github.com/E-Vex/pandemonium-bd/actions/runs/38071980300/job/114276905633)) |

Counts are summed from each leg's `test result:` lines (29 test binaries
per leg); `573 dev / 568 release` is the workspace count on master (the
debug-only 5 are the `#[should_panic]` invariant tier; the 2 ignored are
the content crate's two doctests, pinned `ignore` in their fences) —
identical numbers in all four attempts, zero flakiness, zero red. The
counts were extracted from each attempt's job logs, not from the run's
summary page.

**Was the pre-rodio crash ever named? Plainly: there was no pre-rodio
crash, and the crash that did exist was named.** The "predates rodio"
reading came from failure *conclusions* rather than failure *logs*: run #38
at `08748d9` (before rodio existed) was red on the Windows legs because of
the CRLF test bug — `exit 1`, no crash signature — while the actual
`STATUS_ACCESS_VIOLATION` appeared exactly when the five probing tests
landed with rodio and disappeared exactly when they stopped probing (the
corrected timeline above, read from the primary job logs). Windows did not
"simply go green after the test/probe changes" by accident: the changes
removed the cause the isolation round and the interaction matrix had
already named — two or more real WASAPI probe rounds in one process on an
endpoint-less machine (a test-architecture defect, never a product
defect). Both fixes (tests-never-probe `a2bc546`, CRLF `7e4cd1e`) are on
the merged master, and the four attempts above are the stability proof
that neither has regressed.

**The diagnostics workflow is gone, as briefed.** `a002-diagnostics.yml`
was removed in dedicated commit `b16b414` ("retire the a002-diagnostics
workflow — the matrix is in, the evidence chain is complete") on the
A-002 branch, merged to master via PR #5; this session verified it is
absent on master (`git log --follow` shows the deletion; nothing under
`.github/workflows/` and no reference anywhere in the tree remains), so no
removal commit was possible or needed here. The evidence it produced lives
on in this document's isolation and matrix sections.
