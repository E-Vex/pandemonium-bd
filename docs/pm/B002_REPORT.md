# B-002 — Task Report

**TASK ID:** B-002 (A11 premise fix, CI hygiene, scale proof) · PRODUCT PHASE
**STATUS:** done — the CI-side evidence landed after the token re-supply: audit green, nightly dispatch re-verified 14/14, CI 11/13 with only the two pre-existing Windows client-crash legs red (§3 Part 2, §11)
**Agent:** Agent B (lane per docs/pm/STANDING_ORDERS.md §3)
**Branch:** `b/B-002-a11-ci-scale` (base: master `9d3874a`, not moved — no rebase needed)
**Date:** 2026-10-10

---

## 1. Summary

The A11 fuzz is re-founded on states that are actually equivalent (label swap over one identical world), with the old premise's failure pinned as an explicit deterministic test; the battery and A5 audits are untouched. All three workflows carry pinned runner images and current node-24 action majors — now **run-verified live on the pushed branch** (§3 Part 2) — ci.yml gained workflow_dispatch (dispatch acceptance proven), and the audit red is triaged to two unmaintained-informational advisories with documented dispositions; the audit workflow is green again. The §15 scale proof exists end to end: a data-only 128×128 proving ground, `bench --scale`, the eleven-stage profile, and the memory row — the 10× row is honestly **NOT MET** (34.1 ms vs ≤ 8 ms), with ranked, not-implemented optimization proposals.

## 2. Changes

Branch `b/B-002-a11-ci-scale`, 8 code commits + 4 doc commits (the scale report, this report, the registers, and the CI-evidence update) = 12 commits, one logical change each; 30 files, +2915/−72 against master.

| Commit | One logical change |
|---|---|
| `e79c089` | `tests/ai.rs` — the re-founded label-swap fuzz (4096 cases) + the named deterministic pin of the old failing case |
| `a0c1b59` | `docs/pm/PROPTEST_REGRESSIONS_POLICY.md` — the regressions-file policy |
| `edb912b` | `docs/ALPHA_DECLARATION.md` — the A11 evidence row only |
| `a321993` | `ci.yml` + `nightly.yml` — runner pins, action-major bumps, ci.yml workflow_dispatch |
| `8e4f9a3` | `audit.yml` + `.cargo/audit.toml` — advisory dispositions + pinned auditor |
| `4a5923e` | `sim_api`/`sim` — the opt-in `StageObserver` + `Sim::step_observed` (golden-neutral) |
| `ec6401d` | `content_scale/` — the data-only proving ground (map + swarm faction + verbatim copies) |
| `f42178f` | `tools` — `bench --scale` (scenario, stage profile, memory, pathfinding row, 5 tests) |
| (docs) | `docs/pm/SCALE_REPORT.md`, `docs/pm/B002_REPORT.md`, registers A-200..A-202 / DEBT-200..DEBT-202 |
| (docs) | the CI-evidence update — run URLs for the three pinned workflows, the live runner-label and Node-20-absence verification, DEBT-201 closed |

No sim gameplay code, goldens, client, README, handoff, or plan changes. The sim change is an instrumentation seam only (`step` delegates to `step_observed` with `None`; goldens re-verified bit-identical in both profiles — §5).

## 3. Evidence per acceptance criterion

**Part 1 — A11 mirror fuzz (test-side fix, per the brief's decision on P1):**
1. *Failing case as explicit named test, inputs written out:* `tests/ai.rs::a11_mirror_positions_are_not_equivalent_states_units_translate` — the exact drawn case (Build `ore_node` at (47,46) for p0, footprint-mirrored (15,16) for p1) with literal inputs; asserts p0 accepted / p1 `PlacementBlocked` under all four controller-label arrangements. Run: `cargo test -p pandemonium-tests --test ai a11_mirror_positions` — **ok** (dev and release). No proptest-regressions file is relied on (or committed).
2. *Re-founded fuzz on actually-equivalent states:* `a11_fuzz_label_swapped_commands_validate_identically` — one world, one command set, the Human/Ai labels swapped between the two sims; equivalence proven in-test by snapshot equality (the identity hash deliberately differs — labels are hashed match identity, `controller_tag`; A-202). The premise error is explained in the test docs and in §"why the old premise was wrong" above the test. **4096 cases, dev: ok in 10.07 s; release: ok in 0.58 s.** Battery and A5 audits intact (`git diff` shows no other ai.rs test touched).
3. *High case count:* 4096 committed (`#![proptest_config(ProptestConfig::with_cases(4096))]`), both profiles green (above).
4. *Declaration row:* only the A11 row of `docs/ALPHA_DECLARATION.md` changed (`edb912b`); the claim stays **PASS** and true — the evidence now names the re-founded fuzz and the pin.
5. *Policy:* `docs/pm/PROPTEST_REGRESSIONS_POLICY.md` — never commit a red-pinning file (it replays before fresh cases on all six CI legs, permanently); commit only all-green pins (the guard state); the honest conversion for a red case is a named deterministic test. History of record included.
6. *STOP check:* the divergence is **not** genuine issuer-dependent behavior — proven by the pin test itself (identical outcomes under every label arrangement; the difference is occupancy: p0's translated worker at (16,16) sits inside p1's mirrored footprint region). No sim patch needed or made.

**Part 2 — CI hygiene:**
1. *Runner pins:* every job in all three workflows: `ubuntu-latest`→`ubuntu-24.04`, `windows-latest`→`windows-2025`, `macos-latest`→`macos-26` — exactly the `-latest` resolutions of 2026-10-10 (verified against the actions/runner-images table; the annotation's Ubuntu-26 migration is 2026-10-19). Each workflow carries the why-and-when comment. **Run-verified live on the pushed branch** — every job's actual runner label (read from the run APIs) is its pinned image: ci https://github.com/E-Vex/pandemonium-bd/actions/runs/38049743047 (`ubuntu-24.04` on lint/goldens/replay/builds/its test legs, `windows-2025`, `macos-26` — 13 jobs); audit https://github.com/E-Vex/pandemonium-bd/actions/runs/38049743044 (`ubuntu-24.04`); nightly https://github.com/E-Vex/pandemonium-bd/actions/runs/38049761220 (`ubuntu-24.04` on all 14 jobs).
2. *Action majors:* `checkout@v4`→`@v7` (7.0.1), `upload-artifact@v4`→`@v7` (7.0.2), `download-artifact@v4`→`@v8` (8.0.2) — all node24 (verified in each release's `action.yml`); `rust-cache@v2` stays (v2.9.2 already runs node24). Lint: **actionlint 1.7.12 clean** (1.7.7's `macos-26` complaint is its stale label list — current actionlint knows the label), plus strict YAML parse (no duplicate keys, no tabs). One YAML trap caught and fixed in audit.yml (unquoted colon in a step name — B-001's hex lesson re-applied). **Run-verified live:** checkout@v7 / upload-artifact@v7 / download-artifact@v8 executed on real runners in all three runs above, and the "Node.js 20 is deprecated … actions/checkout@v4" warning that still fires on master's pre-pin runs (present in job 114190333629's log tail) is **absent from every job log of this branch's runs**.
3. *Audit triage (the red 6/6):* **real advisories, but informational — not vulnerabilities.** `cargo audit --deny warnings` (cargo-audit 0.22.2, the workflow's own install path) against the lockfile: `paste` 1.0.15 (RUSTSEC-2024-0436, unmaintained; via wgpu 26.0.1→wgpu-hal→metal 0.32.0, Apple target only; 1.0.15 is the final release — no fix exists) and `ttf-parser` 0.25.1 (RUSTSEC-2026-0192, unmaintained; via fontdue 0.9.4 and winit 0.30.13→sctk-adwaita→ab_glyph→owned_ttf_parser; no fix in current majors). Zero vulnerability advisories. Fix applied: `.cargo/audit.toml` ignores both by ID with reasons (committed, reviewed — not silenced; any new advisory still fails), verified **green locally** with the exact command the workflow runs; the auditor is pinned (`--version 0.22.2`) so the gate cannot drift with cargo-audit releases. **Run-verified green on the pushed branch:** https://github.com/E-Vex/pandemonium-bd/actions/runs/38049743044 — job "cargo audit" on `ubuntu-24.04`, 337 crate dependencies scanned, zero advisories firing (the two dispositions ignored by ID, visible in the committed `.cargo/audit.toml`); the workflow's red streak ends.
4. *workflow_dispatch on ci.yml:* added (B-001 P4) — and the trigger is proven live: the dispatch API accepts the branch (HTTP 204) and created run 38051409659 (event `workflow_dispatch`), which was then deliberately cancelled — the acceptance itself is the evidence; the full gate ran via the push trigger (URLs above).
5. *Nightly dispatch re-verify after pinning:* **DONE — 14/14 jobs green in ~24 min** (11:49:24→12:13:20 UTC; B-001's dispatch precedent: 25:54): dispatched with `shard_matches=4` on the pushed branch — https://github.com/E-Vex/pandemonium-bd/actions/runs/38049761220 — all 14 jobs on pinned `ubuntu-24.04`: ten release shards, smoke-32, the §15 perf-budget bench, the dev-profile A12 invariant sweep (soak-dev), and the aggregate evidence job. Only the release shards shrank (4 matches each); smoke and the invariant tier kept their sizes, per the workflow's own input contract.

**Part 3 — Scale-budget proof (plan §15):** full report with method, tables, and analysis in **`docs/pm/SCALE_REPORT.md`**. Summary of the acceptance items:
1. *Deterministic scale scenario, data-only, no sim changes in the scenario:* `content_scale/` (128×128 symmetric proving ground + swarm faction; `content-validate content_scale` PASS, symmetry declared+verified) + `bench --scale N` (forces truncated to 100·N per side; scale 10 = 2020 entities). ~2000 entities on a data-only large map, as the brief allows. Determinism test: `scale::tests::the_scale_scenario_is_deterministic` — identical command stream, entity count, and final hash run-to-run at both tiers; green in both profiles.
2. *Per-stage profile at 1× and 10× + top 3 super-linear:* the eleven §6.3 stages measured through the new observer seam (tables in SCALE_REPORT §2). Top 3: **Movement** (×11.2 growth, 80–91 % of every tick, 3.41 s tick-0 A* storm, per-tick full-map spatial-hash rebuild, quadratic-in-cell push-apart crowding), **Economy** (×22.4 — crowd-blocked delivery/auto-seek repath churn), **Vision** (entity-linear ×9.1 but a per-tick full-map rescan floor — the map-area scaling axis; 234 µs/tick at 1× already).
3. *Proposed optimizations, ranked by gain/risk, golden status each (NOT implemented):* (1) true incremental vision — low risk, **golden-preserving by construction** (fog is outside the canonical hash, A-059/A10); (2) persistent spatial hash — medium, **golden-preserving only if iteration order stays bit-identical** (must be proven); (3) path-request budgeting — high, **moves goldens** (arrival ticks are hashed state; needs authorized golden-move commit); (4) crowd-aware gather assignment — high, **moves goldens**. §15 verdicts: Alpha-size **MEETS** (0.246 ms avg / 2.21 ms p99, real bench re-run; the 220-entity 198-concurrent-gatherer stress shape misses at 2.68 ms — recorded as the workload-shaped-budget finding); **10× NOT MET** (34.1 ms); throughput **MEETS** (4050 t/s); pathfinding **MEETS** (avg 0.313 ms / p50 0.472 ms, real map, barely); memory **MEETS** (4.4/5.5 MiB peak RSS, sim-side); **render: NOT MEASURABLE HEADLESS — stated plainly** (no GPU/display; the client-side memory and frame-rate halves are future work).
4. *Goldens unchanged* — §5.

## 4. Gate results (both profiles, this branch's head)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **GREEN** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **GREEN** (one lint found and fixed during work: `needless_option_as_deref` on the observer reborrow) |
| `cargo test --workspace` (dev) | **559 passed / 0 failed** |
| `cargo test --workspace --release` | **554 passed / 0 failed** |
| Baseline preserved | 553 dev / 548 release at B-001; the net +6/+6 is the named A11 pin (+1), the re-founded fuzz replacing the old (±0), and five new scale tests (+5) — **all green; the A11 flake is gone** |
| `content-validate content` / `content_scale` | PASS / PASS (symmetry declared + verified on both maps) |
| A11 fuzz @ 4096 cases | dev ok (10.07 s), release ok (0.58 s) |

## 5. Goldens

**Unchanged, bit-identical, both profiles, on this branch's head** (re-verified after the only sim-adjacent change, the observer seam): demo `0xb6fff6659cfb7709`; flagship `0x6e9a18bd7c5f699f`; content `0x9bc18c521107b262` / map `0xd38136401ab02ff1`. The scale scenario has its own recorded hashes (1× `0x7ee9f2c2cf3daae3`, 10× `0x11bf5135d865e045`) — new evidence for a new instrument, additive, not golden moves.

## 6. Deviations and assumptions

- **A-200** — the scale tiers are tools-authored selection from content (like the scripted commands); the proving ground is an instrument, not a product map.
- **A-201** — runner pins freeze the 2026-10-10 `-latest` resolutions; migration only by brief.
- **A-202** — controller labels are hashed match identity, never behavior; the A11 equivalence proof is snapshot+outcome equality, not identity-hash equality.
- Deviation (report-level), since resolved: the brief's Part 2 acceptance ("run URLs", nightly dispatch re-verify) was blocked at report-writing time by the missing push token — the honest blocker, not a silent skip; the token was then re-supplied, the branch pushed, and every run-URL item now carries live evidence (§3 Part 2). Closed, not dropped.
- Note: `cargo-audit` (0.22.2) reads `.cargo/audit.toml` (not `cargo-audit.toml`); the ignore entries are plain IDs with reasons in comments — the file-schema reality of the pinned version, discovered and verified locally.

## 7. New debt

- **DEBT-200** — the two unmaintained-advisory ignores (paste, ttf-parser): documented dispositions, revisit on the next wgpu/winit/fontdue bump.
- **DEBT-201 — CLOSED**: token re-supplied, branch pushed, all three pinned workflows ran live — audit green, nightly dispatch 14/14, CI 11/13 with the two pre-existing Windows client-crash legs (see §10); the register row carries the closure evidence.
- **DEBT-202** — the §15 10× row not met; four ranked optimization proposals await a PM brief (two are golden-preserving, two need authorized golden moves).

## 8. Risks and surprises

- The A11 fuzz's original premise failure was **premised on a half-true symmetry** (structures mirror; units translate) — the same half-truth could bite any future test that assumes positional equivalence between sides. The pin test and A-202 record it.
- The label-arrangement states hash **differently by design** (`controller_tag`) — my first fuzz draft asserted hash equality and failed on exactly that; the discovery is now the documented basis of the equivalence proof.
- The §15 Alpha-size budget is **workload-shaped, not entity-count-shaped**: 198 simultaneous gatherers miss ≤ 1 ms at 220 entities while the real 36-entity match sits at 0.246 ms. Anyone quoting "≤ 200 entities" as a safety margin should quote order density too.
- The pathfinding row passes by 6 % (p50 0.472 vs 0.5 ms) on the real map — one map class up crosses it.
- 2-core host: the 10× miss (34.1 ms vs 8 ms) is 4.3× over on this machine; a faster host narrows but does not obviously close it, and the super-linear structure (not raw speed) is the finding.

## 9. Proposals (not implemented)

- The four scale optimizations of SCALE_REPORT §4 (incremental vision first — golden-preserving).
- Add `tests/*.proptest-regressions` to `.gitignore` as a mechanical backstop for the policy doc (left undone: the brief asked for the policy, and an untracked file in `git status` is itself the triage signal).
- A faction-selection seam (`ContentTree::bundle_for(map, faction)`) would let one scale tree serve multiple tier factions without the tools-side truncation (A-200); post-Alpha per plan §17.

## 10. Cross-lane requests

- **Agent A (client):** unchanged from B-001 P2 — the Windows client test-binary `STATUS_ACCESS_VIOLATION` keeps the full-CI green run off the table, now with evidence on both sides of the lane boundary: master @ `9d3874a` fails the same two legs (run 38044206117), and this branch's run fails them with the **byte-identical test binary** (`pandemonium_client-841d54467a05de16.exe` — same cargo build-id as master's failing run; this branch touches no client code), so the red is inherited from the base, not introduced by B-002. Agent A's `a/A-002-green-windows-and-release` (unmerged at this writing) carries the fix ("unit tests never touch the machine's hardware"). Also, the §15 render and client-memory rows still need a windowed machine.
- None new for engine: the per-stage profile was designed to need zero engine changes (the observer rides `Sim::step_observed` directly from `tools`).

## 11. Needs from owner

1. **Rotate the repo token** — the one used for this push transited the chat; B-001's rotation note applies (rotate after merge). The previous §11's publish-and-verify steps are DONE, executed verbatim: branch pushed (`b/B-002-a11-ci-scale`, 12 commits), ci + audit ran from the push, nightly dispatched with `shard_matches=4` — all run URLs in §3 Part 2; DEBT-201 closed.
2. Merge decision for `b/B-002-a11-ci-scale` (12 commits; one logical change each). Order note: merging this before Agent A's A-002 leaves the two Windows test legs red on master's CI exactly as they are today (the crash is pre-existing at the base sha); merging after — or together — turns them green. The lanes do not conflict: A-002 touches no workflow files, B-002 touches no client files.
3. PM routing for DEBT-202 (the 10× optimization brief — two of the four proposals move goldens and need §7 authorization).
