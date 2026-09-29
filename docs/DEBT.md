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
