# B-002 Part 3 — Scale-Budget Proof (plan §15)

**Task:** B-002 PART 3 (scale-budget proof) · PRODUCT PHASE
**Branch:** `b/B-002-a11-ci-scale` · **Method owner:** Agent B
**Environment:** Linux x86_64 container, **2 cores / 4 GB RAM**, no GPU/display, Rust 1.98.1 (pinned), release profile unless stated. A slow host — absolute numbers are conservative; the ratios, shares, and verdict structure are the durable evidence.

**The instrument** (all new on this branch, no sim/engine/client changes in the scenario itself):
- `content_scale/` — the data-only proving ground: a 128×128 180°-symmetric map (8-node ore ring per start, 4 contested center nodes, four symmetric rock clusters; `content-validate` PASS, symmetry declared + verified) and the `swarm` faction (1000 starting forces per side: 1 CC + 910 workers + 89 riflemen). Entities and rules are verbatim copies of the real content — a drift-guard test fails if they diverge.
- `tools bench --scale N` — the deterministic scenario: per-side forces truncated to `100 × N` (scale 1 = 220 entities, scale 10 = 2020), scripted command waves at tick 0 (every worker gathers its nearest live node, every rifleman attack-moves to the map center) and tick 150 (re-gather, re-target the enemy base), no AI controllers — the workload is the eleven-stage tick pipeline itself, driven by commands (FD-2).
- `Sim::step_observed` — the opt-in stage observer (golden-neutral; the three goldens re-verified bit-identical in both profiles on this branch) feeding a tools-side timing collector. Clocks live in `tools`, never in the determinism crates (plan §5.4).
- Determinism: same content + scale + seed → identical command stream and final canonical hash, pinned by `scale::tests::the_scale_scenario_is_deterministic` (both tiers, both profiles). Scenario hashes of record: 1× `0x7ee9f2c2cf3daae3`, 10× `0x11bf5135d865e045` (seed 7, 1500 ticks).

## 1. The plan §15 verdict table

| §15 row | Threshold | Evidence | Measured | Verdict |
|---|---|---|---|---|
| Sim tick, Alpha-size match (≤ 200 entities) | ≤ 1 ms avg, ≤ 4 ms p99 | the real AI-driven bench, re-run this branch (`bench --ticks 10000 --seed 7`, 36 end entities) | **0.246 ms avg / 2.21 ms p99 / 3.31 ms max** | **MEETS** |
| — same row, stress shape | — | scale 1 tier (220 entities, 198 simultaneously ordered) | 2.68 ms avg / 0.33 ms p50 / 18.3 ms p99 / 340 ms max | **MISSES under order-density** — recorded finding, see §3 |
| Sim tick at 10× scale (≈ 2000 entities) | ≤ 8 ms avg | scale 10 tier (2020 start entities, 1894 end) | **34.1 ms avg / 21.7 ms p50 / 140.6 ms p99 / 3.41 s max** | **NOT MET** (4.3× over, on this host) |
| Headless throughput | ≥ 1000 ticks/s | real bench / scale 1 / scale 10 | **4050 t/s** / 373 t/s / 29 t/s | **MEETS** (the row targets Alpha matches; scale tiers informational) |
| Render | 60 FPS at 1080p | — | — | **NOT MEASURABLE HEADLESS** — no GPU or display in this environment; the windowed path is the owner's (DEBT-008's Xvfb recipe verifies it boots, not frame rate) |
| Pathfinding request (64×64 map) | ≤ 0.5 ms typical | the real Crossroads map, Movement-stage marginal cost of one far `Move` per tick, 40 requests (`bench --scale` prints it) | **avg 0.313 ms / p50 0.472 ms / max 0.623 ms** (idle baseline 4.5 µs/tick) | **MEETS** — barely; one map-size class up would cross it |
| Memory | < 200 MB for an Alpha match | peak RSS (VmHWM) of the tools process | **4.4 MiB at 1× / 5.5 MiB at 10×** | **MEETS** — with the plain caveat that the renderer is not in this process; a client-side measurement is future work, not measurable headless |

The 10× miss is the headline, and the plan anticipated it verbatim: "may require
spatial partitioning inside systems" (§15). The profile below shows exactly
where.

## 2. The per-stage profile (plan §6.3's eleven stages)

`bench --scale {1,10} --ticks 1500` (release, seed 7). Costs are the observer's
stage-boundary timings; avg/p99/max per tick, share of the summed stage cost.

| Stage | 1× avg | 1× p99 | 1× max | 1× share | 10× avg | 10× p99 | 10× max | 10× share | growth (×9.18 entities) |
|---|---|---|---|---|---|---|---|---|---|
| 1 Commands | 0.1 µs | 0 µs | 45 µs | 0.0% | 1.5 µs | 0 µs | 1141 µs | 0.0% | ~linear, tiny |
| 2 Orders | 0 µs | 0 µs | 3 µs | 0.0% | 0 µs | 0 µs | 5 µs | 0.0% | — (no direct code) |
| 3 Production | 0 µs | 1 µs | 5 µs | 0.0% | 4.8 µs | 9 µs | 12 µs | 0.0% | small |
| 4 Economy | 9.3 µs | 31 µs | 81 µs | 0.3% | 208 µs | 348 µs | 584 µs | 0.6% | **×22.4** |
| 5 Acquisition | 0 µs | 0 µs | 0 µs | 0.0% | 0 µs | 0 µs | 3 µs | 0.0% | — (runs inside combat) |
| 6 Movement | **2429 µs** | 18058 µs | 340010 µs | **90.8%** | **27296 µs** | 132620 µs | 3405507 µs | **80.0%** | **×11.2** |
| 7 Combat | 0 µs | 0 µs | 3 µs | 0.0% | 4450 µs | 5816 µs | 10489 µs | 13.0% | engagement-driven (126 deaths at 10×; the 1× tier's 16 riflemen never traded hits in 1500 ticks) |
| 8 Cleanup | 0.3 µs | 2 µs | 18 µs | 0.0% | 10.5 µs | 31 µs | 43 µs | 0.0% | ×35 at 10 µs absolute (noise-amplified) |
| 9 Vision | 233.7 µs | 342 µs | 497 µs | 8.7% | 2126 µs | 2336 µs | 4064 µs | 6.2% | ×9.1 — entity-linear, map-area floor |
| 10 MatchRules | 0 µs | 0 µs | 1 µs | 0.0% | 0.1 µs | 2 µs | 6 µs | 0.0% | — |
| 11 Finalize | 2.5 µs | 75 µs | 141 µs | 0.1% | 32.7 µs | 1071 µs | 1423 µs | 0.1% | ×13 at ≤33 µs absolute |

Cross-checks: the summed stage averages reproduce the per-tick aggregates
(movement is 90.8%/80.0% of the tick at 1×/10×); the max row at both tiers is
the tick-0 path-request storm (198 and 1820 simultaneous A\* requests — 340 ms
and **3.41 s** respectively).

## 3. The top three super-linear costs

1. **Movement — ×11.2 growth, 80–91 % of every tick, and the request storm.**
   Three code-anchored drivers:
   - *The tick-0 storm*: every scripted worker requests an A\* path in the same
     tick. A lone request on the real 64×64 map costs 0.31–0.47 ms (the
     pathfinding row); 1820 concurrent requests on the 128×128 grid cost
     ~1.9 ms each (4× the nodes, and concurrent searches thrash allocator and
     cache) — 3.41 s in one tick, 100× the tier average.
   - *Per-tick spatial-hash rebuild*: `push_apart` allocates and zero-fills the
     full one-tile cell grid every tick — O(map area) per tick before any pair
     is examined.
   - *Crowding quadratic*: push-apart examines all pairs within each 1×1 cell;
     900 workers per side converging on 12 ore nodes packs dozens of units per
     cell, so pair counts grow super-linearly with force size. The 1× tier
     already shows the shape: 198 simultaneous gatherers on the same 12 nodes
     produce an 18.3 ms p99 and a 2.68 ms average *below 220 entities* — the
     §15 Alpha row survives real match shapes (0.246 ms) but not this
     order-density; the budget is workload-shaped, not entity-count-shaped.
2. **Economy — ×22.4 growth (0.3 % → 0.6 % share, 9.3 → 208 µs).** The gather
   loop's per-worker cost is not constant under crowding: crowd-blocked
   deliveries and node depletion trigger auto-seek and travel-target repathing,
   and the tick-150 re-issue wave doubles order churn. Smallest absolute of the
   three, but the fastest-growing non-movement stage — the same crowding
   pressure as (1) surfacing through a different system.
3. **Vision — entity-linear (×9.1) but with a structural map-area floor.**
   `advance_vision` clears and re-marks *every tile for both players every
   tick* (a full recompute, despite the "incremental" name), then walks every
   vision-carrying entity's disk. The disk walk is linear in entities; the
   clear/rescan floor is linear in map area — 234 µs/tick at 1× on 128×128,
   ~9 % of the whole tick spent regardless of how few entities move. It is not
   super-linear in entities; it is the second scaling axis (map size), already
   material at this map class.

Not ranked (honest exclusions): Combat's 0 → 4450 µs is engagement-driven
(the 10× tier's 356 riflemen fight; the 1× tier's 16 never engage within 1500
ticks — end-entity count unchanged), so its "growth" reflects workload shape,
not entity scaling. Cleanup ×35 and Finalize ×13 are real but ≤33 µs absolute
— noise-amplified ratios.

## 4. Proposed optimizations — ranked by gain/risk (NOT implemented)

1. **True incremental vision** — re-mark only moved entities' disks; per-tile
   expiry timestamps instead of clear-all/re-mark. Gain: removes the O(map)
   floor and most disk marking (~2.1 ms/tick at 10×, 6 % of the tier; 9 % of
   the 1× tick). Risk: **low — preserves golden hashes by construction**: fog
   is derived state outside the canonical hash (A-059), and A10's fog on/off
   test already pins hash-equality under fog changes; the standard gate re-run
   proves it.
2. **Persistent spatial hash for push-apart** — keep the cell grid across
   ticks, updating entries as units move, preserving ascending-id cell lists
   and the fixed cell scan order. Gain: removes the per-tick O(map) rebuild
   and trims push-apart cost (the 80 % stage). Risk: **medium — golden hashes
   only if the iteration order and arithmetic stay bit-identical**, which is
   achievable by construction but must be proven by the full two-profile gate
   + goldens; a single ordering slip moves them.
3. **Path-request budgeting** — cap A\* requests per tick, defer overflow to
   later ticks in ascending-id order. Gain: the single biggest p99/max win
   (3.41 s storm → ~ms; tier p99 from 141 ms toward single-request costs).
   Risk: **high — moves golden hashes**: deferred paths change arrival ticks,
   and orders/paths are canonical hashed state; requires an authorized
   golden-move commit (standing orders §7) plus a semantics review of the
   deferral order.
4. **Crowd-aware gather assignment** — spread workers across nodes at issue
   time (the tools script already could) and/or relax delivery adjacency.
   Gain: trims the ×22 economy churn and the crowding pressure feeding (1).
   Risk: **high — moves golden hashes** (economy order phases are hashed
   state, arrival timing shifts); same authorization requirement as (3).

All four are proposals per the brief; none is implemented on this branch.

## 5. Re-runnable evidence

```bash
cargo run --release -p pandemonium-tools -- bench --ticks 10000 --seed 7    # the real Alpha rows
cargo run --release -p pandemonium-tools -- bench --scale 1 --ticks 1500    # 1x tier + pathfinding row
cargo run --release -p pandemonium-tools -- bench --scale 10 --ticks 1500   # 10x tier + pathfinding row
cargo test -p pandemonium-tools scale                                       # determinism + guards (both profiles)
cargo run -p pandemonium-tools -- content-validate content_scale            # the proving ground validates
```

Goldens unchanged throughout: demo `0xb6fff6659cfb7709`, flagship
`0x6e9a18bd7c5f699f`, content `0x9bc18c521107b262` / map
`0xd38136401ab02ff1` — bit-identical in dev and release on this branch.
