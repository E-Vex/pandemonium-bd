# B-001 — Truth Baseline Report

**Task:** B-001 (Truth baseline, CI repair, cross-OS hash gate) · PRODUCT PHASE · critical path
**Agent:** Agent B (lane per docs/pm/STANDING_ORDERS.md §3)
**Date:** 2026-10-10 · **Branch:** `b/B-001-truth-baseline` (base: master `23012bf`) · **Head:** `96bdbf5` at time of the CI evidence below
**Scope touched:** `.github/workflows/ci.yml`, `.github/workflows/nightly.yml`, `docs/pm/` — zero Rust, content, test, client, README, or handoff changes (`git diff master..HEAD --stat`: 3 files, +182/−12, none of them code).

Verification environment (local): Linux x86_64 container, 2 cores, Debian 13, Rust 1.98.1 (pinned), no GPU/display (windowed client not verifiable here — honest limitation, see claim 12). CI evidence: GitHub Actions runs cited by URL/ID.

---

## 1. The verdict table — every handoff claim, verified or not

Sources: `AI-Handoff.md` §2/§4, `ALPHA_DECLARATION.md`, the B-001 brief itself, and the GitHub Actions API (52 runs listed, `actions/runs?per_page=100`).

| # | Claim (source) | Verdict | Evidence |
|---|---|---|---|
| 1 | Tests: 553 dev / 548 release, everything succeeds (AI-Handoff §2, §4 "Expected") | **CONTRADICTED (flaky)** | Actual: 553/548 total tests; **552/547 pass, 1 fails** when the A11 mirror fuzz draws a rare input (see §3). Passed cleanly on the final-tree re-run (553/548, both profiles) — the failure is real but rare. The counts themselves are accurate. |
| 2 | "tests 414 dev / 409 release" (quoted in the B-001 brief) | **STALE — matches no current doc** | That pair appears once, in a M10.1-era `CHANGELOG.md` entry (line 446). The current handoff says 553/548; actuals are 553/548 total. The brief quoted a historical changelog line, not the handoff. |
| 3 | Demo golden `0xb6fff6659cfb7709` (seed 7, 300 ticks) | **VERIFIED** | Local dev + release (`headless --seed 7 --ticks 300`); all three CI OSes via the new cross-OS gate: run [38039227852](https://github.com/E-Vex/pandemonium-bd/actions/runs/38039227852) — ubuntu/windows/macos all emit exactly this value. |
| 4 | Flagship golden `0x6e9a18bd7c5f699f` (seed 7, 7200 ticks AI-vs-AI) | **VERIFIED** | Local dev + release; all three CI OSes in the same gate run. |
| 5 | Content hash `0x9bc18c521107b262`, map id `0xd38136401ab02ff1` | **VERIFIED** | `tools content-validate content` PASS in both profiles locally ("symmetry: declared + verified"). |
| 6 | Release bench: 0.25 ms avg / 2.22 ms p99 / 4060 ticks/s (declaration §"Performance baselines") | **VERIFIED** | `bench --ticks 10000 --seed 7` (release): avg 246.4 µs, p99 2221 µs, throughput 4049 t/s, end entities 36. Deltas vs the claim (0.6 µs, 3 µs, 11 t/s) are machine noise; all §15 budgets MEET. |
| 7 | Soak of at least 32 matches (declaration: 32/32 resolved, 0 crashed, 0 stuck, avg 11877, max 23265) | **VERIFIED — bit-for-bit identical numbers** | `soak --matches 32 --seed-base 0` (release): 32/32 resolved, 0 crashed, 0 stuck, avg end tick 11877, max 23265. Wall 330 s on 2 cores. |
| 8 | Replay round-trip A2 (demo + AI-vs-AI) | **VERIFIED** | Record + `replay-verify` PASS in dev and release: demo (11 checkpoints, final `0xb6fff6659cfb7709`), AI-vs-AI 1200 ticks (41 checkpoints). |
| 9 | "Actions tab shows only two runs, both M1-era invalid-YAML failures; no CI job has ever executed" (brief's premise) | **CONTRADICTED — the view was stale** | The API lists **52 runs**: 41 `ci` push runs (every one failed — see §2), 5 nightly (3 scheduled **success** on Oct 7/8/9), 6 audit (all failed). Jobs have been executing since Oct 3. The M1-era invalid-YAML failure class is real and ended at `08748d9` ("ci: quote a step name so ci.yml parses"). |
| 10 | "AI-Handoff describes a nightly soak workflow the Actions tab does not list" | **STALE VIEW** | `nightly.yml` exists on master, state **active**, with `workflow_dispatch`, 10-way sharding, timeouts, per-shard artifacts, and a `!cancelled()` aggregate gate — and it **ran to success 3×** (runs 37604749664, 37762351811, 37916645286: the 1000-match sweep + dev invariant tier + bench, green). |
| 11 | "CI additionally runs both profiles on Windows and macOS" (declaration, cited as A1 evidence) | **HALF-CONTRADICTED** | The macOS legs do run and pass in both profiles (real A1 evidence since Oct 7 — the golden-pinning tests run there). The Windows legs have **never passed**: the `pandemonium-client` test binary dies with `STATUS_ACCESS_VIOLATION` (0xc0000005) on every run — 41/41 (see §4). The cited cross-OS evidence was therefore never green as a whole; the new cross-OS gate now supplies the Windows-side hash evidence (tools-only). |
| 12 | AI-Handoff §4 verification block, item by item | **MOSTLY VERIFIED** | fmt ✓ (green), clippy ✓ (green), tests dev/release — flaky (claim 1), headless demo ✓, flagship ✓, content-validate ✓, dev soak 4×900 ✓ (0 crashed, invariants clean), release soak ✓ (claim 7). `cargo run -p pandemonium-client` (windowed) **NOT VERIFIED locally — no display/GPU in this environment**; the CI binaries job exercises the client's documented no-display fallback instead (PASS on ubuntu-latest, run 38039227852). The human windowed pass remains DEBT-008/A14, the owner's. |
| 13 | Goldens unchanged through B-001 | **VERIFIED** | No sim/content/test code changed on the branch; the three goldens re-verified bit-identical locally in both profiles and on all three CI OSes. |

---

## 2. The real CI history (what the Actions tab actually holds)

- **41 ci push runs, 41 failures.** Three distinct eras:
  1. **M1-era (Sept 30, runs #1–#2):** invalid YAML ("ci.yml line 75") — fixed later by `08748d9`.
  2. **Oct 3–8:** jobs executed; failures during active development (test failures as features landed), plus two more invalid-file stubs (runs #36–#37, pre-`08748d9`).
  3. **Post-parse-fix master (runs #38–#41):** Linux legs, clippy, replay, and binaries were **green before the audio work** (run #38, sha `08748d9`, Oct 8 12:50 UTC). Then M10.2 Phase 4 landed rodio (`6354f5b`, Oct 8 20:43) and every Linux job that builds the client went red: **ubuntu-latest runners do not ship `libasound2-dev`**, and `rodio → cpal → alsa-sys` probes pkg-config even under `cargo check` (run #41 logs: "The system library `alsa` required by crate `alsa-sys` was not found"). The Windows legs were already red (§4). macOS stayed green throughout.
- **nightly: 3 scheduled successes** (Oct 7–9) — the 1000-match A7 sweep, the dev invariant tier, and the §15 bench are all genuinely green on GitHub runners. That soak evidence in the handoff and declaration is real.
- **audit: 6 failures** (not in this brief's scope; `cargo audit --deny warnings` fails — likely advisories in the dependency tree; the workflow file itself lints clean).

**Consequence for the truth baseline:** A1's cross-OS hash agreement had macOS-only test evidence (strong, but one OS), zero Windows evidence, and broken Linux CI. The new cross-OS gate closes that (§6).

---

## 3. The A11 mirror-fuzz finding (the one red test on master)

**Observed:** `tests/ai.rs::a11_fuzz_mirrored_commands_validate_identically` — "mirror pair diverged (out0=None out1=Some(PlacementBlocked)): Build { worker: EntityId(2), structure: KindId(3)=ore_node, at (47,46) }". Player 0's Build at (47,46) is accepted; player 1's mirror at (15,16) is rejected `PlacementBlocked`.

**Rarity (measured):** hit once in my first full-suite run, then **0 failures in 30 further fresh-seed runs** (256 cases each) locally, and 0 in ~90 historical CI test-leg executions. The failure region is real but narrow — roughly a percent-per-run class event. The persisted seed (`cc c6e1105ce0772f36d68e97d0304186e56032097d950d58dd2dfd434daf120ddd`, in the untracked `tests/ai.proptest-regressions`) reproduces it deterministically in 0.17 s. **The file is deliberately NOT committed** — committing it would pin permanent red.

**Root cause (data-level, nothing touched):** the fuzz's premise — "the Crossroads content is symmetric and the spawns mirror, so every generated pair is structurally identical" — is **half false**.
- Structures and ore nodes *do* 180°-mirror as footprint regions: the map author placed p1's anchor (48,48) exactly at the region-mirror of p0's (12,12) (4×4 CC), and the ore patches mirror origin-for-origin (`crossroads_64.ron`).
- **Units do not mirror — they translate.** The faction's `starting_forces` (`content/factions/legion.ron`) places workers at anchor + offsets (−2,4)…(+4,4): p0's workers stand at (10,16)…(16,16); p1's at (46,52)…(52,52). The point-mirror of p0's worker (16,16) would be (47,47) — no one stands there. p1's actual workers are 36 tiles away.
- The failing pair: p1's ore-node build at (15,16) covers tiles [15..16]×[16..17], which **contains p0's worker at (16,16)** → `PlacementBlocked`. The mirror of p0's accepted (47,46) build is exactly that region. p1's own workers (y=52) never enter [47..48]×[46..47], so p0's build is accepted. The two commands were never structurally identical — the sim's validation is issuer-blind and behaved correctly on both.

**Status:** not fixed — the fix is a PM decision (see §9 proposals). Nothing about it touches frozen decisions or goldens as-is.

---

## 4. The Windows client crash (pre-existing, client lane)

On every CI run ever, both Windows test legs fail the same way: the `pandemonium-client` **unit-test binary** starts ("running 181 tests", first test `audio_wiring::activating_a_menu_row_clicks_the_ui_cue ... ok`) and then the process dies with **`STATUS_ACCESS_VIOLATION` (0xc0000005)** — `cargo test` stops at the first failing binary, so the cross-crate acceptance suites never run on Windows. Evidence: run [38039227852](https://github.com/E-Vex/pandemonium-bd/actions/runs/38039227852) windows dev+release job logs; same signature in run #41 (master `23012bf0`) and run #38 (sha `08748d9`, **before rodio existed**) — so it predates the audio work and is not an ALSA/rodio artifact. macOS (same tests, Metal runners) and Linux pass. This is a client-side defect on headless Windows runners — Agent A's lane, explicitly out of B-001's DO-NOT scope.

---

## 5. CI repair (what changed and why)

All changes on `b/B-001-truth-baseline`; every file lints clean with actionlint 1.7.7 + a strict YAML parser (no duplicate keys, no tabs).

1. `docs/pm/STANDING_ORDERS.md` — the standing orders v1, verbatim as relayed (`8a21e5d`).
2. **Linux system dep** (`3fa2bce`): `sudo apt-get install -y libasound2-dev` on the three ci.yml jobs that build the client (lint/clippy, ubuntu test legs, binaries). Run #41's logs are cited in the workflow comments. The replay job and the goldens job build tools only — no ALSA, no apt (leaner and proven by the green replay job on master).
3. **Timeouts** (`2dc781f`): every ci.yml job carries `timeout-minutes` (30/45/30/30/30/10) — nightly's "a hang must fail loudly" rule applied to the gate.
4. **Cross-OS golden gate** (`898f58a`, fix `96bdbf5`): new `cross-os` matrix (ubuntu/windows/macos, release) running the demo + flagship headless goldens, emitting hashes + per-OS evidence artifacts, and a `cross-os-gate` job that fails unless all three OSes agree with each other **and** the committed goldens. Deliberately `needs:` only the cross-os matrix, not the test matrix — the A1 property gets its own unambiguous signal, unpolluted by the known A11 flake and the Windows client crash. The hex constants are quoted: unquoted `0x…` YAML scalars resolve as YAML 1.1 integers, and the demo value overflows int64, which made GitHub reject the whole workflow file (the stub failure of the first push, run 38039001355 — my miss, fixed same-session).
5. **Nightly small-sweep dispatch** (`d0acec7`): `workflow_dispatch` input `shard_matches` (default 100 = the scheduled full tier) feeding the ten shard matrix entries; seeds stay disjoint for any per-shard count ≤ 100 (the sharding arithmetic comment in nightly.yml).
6. Cargo caching was already present (`Swatinem/rust-cache@v2` on every job); extended to the new goldens job.

**Required-on-push/PR checklist (brief §DO-2):** fmt ✓ · clippy `-D warnings` ✓ · tests 3-OS × dev/release ✓ (all legs execute) · replay round-trip demo + AI-vs-AI ✓ · binaries-start ✓ · nightly confirmed with dispatch/sharding/timeouts/artifacts/aggregate ✓ (all already present; dispatch input added).

---

## 6. Cross-OS gate result (A1)

Run [38039227852](https://github.com/E-Vex/pandemonium-bd/actions/runs/38039227852) (push, sha `96bdbf5`), job `cross-OS golden gate (A1)`:

```
ubuntu-latest   demo=0xb6fff6659cfb7709 flagship=0x6e9a18bd7c5f699f
windows-latest  demo=0xb6fff6659cfb7709 flagship=0x6e9a18bd7c5f699f
macos-latest    demo=0xb6fff6659cfb7709 flagship=0x6e9a18bd7c5f699f
```

All three OSes agree with each other and with the committed goldens. **No divergence; no stop condition.** The same run: fmt+clippy PASS, ubuntu dev+release tests PASS, macos dev+release tests PASS, replay PASS, binaries PASS (client headless fallback on the runner), goldens ×3 PASS; the only failures are the two Windows test legs (§4). The run-level conclusion is therefore `failure` — an honestly-earned red caused by the pre-existing client defect, not by the gate.

## 7. Nightly dispatch (small sweep)

Run [38039572350](https://github.com/E-Vex/pandemonium-bd/actions/runs/38039572350) — `workflow_dispatch` on `b/B-001-truth-baseline` with `shard_matches=4`: **success, 14/14 jobs green** — 10 shards × 4 matches (`--matches 4` verified in the shard logs), smoke-32, soak-dev (A12 invariant sweep), bench (§15), and the aggregate gate: `matches resolved: 88 (player 0: 82, player 1: 6)`, `aggregate: 0 crashed across all tiers`.

## 8. Discrepancies (the complete list)

1. A11 mirror-fuzz premise false for unit spawns (§3) — master's test suite is flaky-red.
2. Windows client test binary crashes (§4) — client lane.
3. The brief's "414/409" test counts are a stale M10.1-era changelog pair (claim 2).
4. The owner's Actions-tab view was stale: 52 runs exist, not 2 (claim 9).
5. The handoff's "everything succeeds; 553 tests pass" is overstated by the A11 flake (claim 1).
6. ci has never been green on any push (41/41 failures — §2's three eras).
7. The declaration's "CI runs both profiles on Windows and macOS" was never true for Windows (claim 11).
8. The audit workflow fails on all 6 of its runs (advisories or tooling — out of scope, flagged).
9. My first branch push was stub-rejected by GitHub's YAML int64 overflow on the unquoted demo hex constant — fixed in `96bdbf5` (documented; the gate now quotes both hex values).

## 9. Proposals (not implemented)

- **P1 (tests lane, my lane — needs a PM brief):** fix the A11 fuzz premise. Options: (a) mirror-compensate the fuzz's Build positions the way spawns actually correspond (translate unit-referenced expectations, region-mirror footprints) or exclude build regions intersecting either side's spawn units, plus a deterministic pin documenting the asymmetry; (b) change the spawner to true 180° mirroring of unit offsets — **moves every golden** (tick-0 positions are hashed) and would need its own authorized golden-move commit; (c) leave as-is and accept flaky red (not recommended). Root cause is in §3; the repro seed is quoted there.
- **P2 (client lane, Agent A):** a diagnostic brief for the Windows `STATUS_ACCESS_VIOLATION` in the client test binary (predates audio; first test passes, crash follows; macOS/Linux green).
- **P3:** a one-line policy note wherever the PM keeps process rules: never commit `*.proptest-regressions` while A11's premise bug is open — it would pin permanent red for everyone.
- **P4 (optional):** add `workflow_dispatch` to ci.yml so the gate can be re-run without a push. Not in the brief; not done.

## 10. Cross-lane requests

- **Agent A:** the Windows client test-binary crash (§4, P2) — it is the only thing keeping a fully green ci run off the table; evidence and reproduction are the cited run logs.
- **PM routing:** the A11 fuzz decision (§9 P1) — test-side fix is my lane, spawn-side fix moves goldens.

## 11. Needs from owner

- Merge decision for `b/B-001-truth-baseline` (6 commits: standing orders, alsa, timeouts, cross-os gate + hex-quoting fix, nightly dispatch input, this report).
- Rotate the GitHub token after the engagement (posted in plaintext; embedded in the clone's git config).
- Nothing else blocked: Actions is enabled, the token carries admin, dispatches and log reads all worked.
