# Debt Register

Every deliberate simplification gets a row (plan §0 and §18). Dangerous shortcuts
(plan §18) are forbidden outright and never belong here — if you took one, undo it.
Reviewed at every milestone exit.

Row format (plan §18): `| id | what | category | why now | replacement plan |
repay-when trigger | status |`

| id | what | category | why now | replacement plan | repay-when trigger | status |
|----|------|----------|---------|------------------|--------------------|--------|
| DEBT-001 | Canonical hasher is FNV-1a 64-bit, not xxHash64 | simplification | trivial, obviously-correct implementation fits M0; at Alpha state sizes a 64-bit collision is a non-issue | swap the algorithm behind the same `fx::hash` API surface (state hash call sites are unaffected) | M10 hash baselines, or any suspected hash collision | open |
| DEBT-002 | `Vec2Fx::normalized` loses precision for very short vectors (integer-sqrt floor error dominates below ~2^12 raw length) | simplification | real facing/direction vectors are thousands of raw units long, where the error is under 0.1% | precision-boosted path (sqrt of a shifted sum of squares) or a wider intermediate inside `normalized` | M4 steering work shows directional drift | open |
| DEBT-003 | M1 movement is a placeholder straight-line mover: no nav grid, no pathfinding, no collision push-apart, no stuck detection; map passability is advisory only | simplification | M1's exit tests prove the spine (ids, ordering, hashing, commands), and movement quality is M4's entire job (plan §14 P1) | replace stage 6 with the three-layer movement contract (plan §9.1): A* with deterministic tie-breaks, path execution, spatial-hash collision, stuck detection with `MoveFailed` | M4 exit tests | open |
| DEBT-004 | Vision/visibility (`visible_to`, `player_view`) recomputes the full radius scan on every call | simplification | correct and cheap at M1 entity counts; incremental maintenance is M6's design work (plan §9.5) | incremental per-player visibility update in stage 9 with the three-state tile model | M6 vision work, or visible profile cost | open |
| DEBT-005 | The replay re-simulation driver exists twice (tools `verify.rs` and `tests/determinism.rs`), ~40 lines each | simplification | the `replay` crate must not depend on `sim` (plan §4), so one shared home needs a new crate both may depend on | extract a tiny `replay-runner` (or move verification into `sim` as a pure function over a command stream) when a third consumer appears or the driver grows beyond re-simulation | a third consumer of the driver, or soak tooling (M7/M8) | open |
