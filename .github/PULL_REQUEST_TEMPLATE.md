<!--
The green gate (plan §0/AI-Handoff §4) is non-negotiable. Tick every box
before requesting review. A PR that fails the gate cannot land; a PR that
moves a golden hash without a written reason cannot land either.
-->

## Summary

<!-- One or two sentences: what does this change do, and why now? -->

## Type of change

- [ ] Bug fix (no behavior change to the simulation's tick pipeline or
      state hash encoding)
- [ ] Refactor (behavior identical; golden hashes unchanged)
- [ ] Test addition (production code unchanged)
- [ ] Documentation
- [ ] CI / tooling
- [ ] New content (`content/` data files only)
- [ ] Other (explain)

If this change touches a frozen decision (plan §2 FD-1..FD-10), an ADR in
`docs/adr/` **must** accompany it. Frozen-decision changes do not land by PR.

## The green gate

- [ ] `cargo fmt --all -- --check` is green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is green
- [ ] `cargo test --workspace` is green (dev profile)
- [ ] `cargo test --workspace --release` is green (determinism must hold
      in release too — plan §5.6, A1)

## Architecture & determinism law

- [ ] No new internal dependency outside `tests/architecture_law.rs`'s
      `allowed_internal()` map.
- [ ] No new external dependency outside the per-crate allow-list, and
      none from the forbidden-crate list (`bevy`, `macroquad`, `ggez`,
      `fyrox`, godot bindings, ECS crates, physics/pathfinding crates,
      `rand`/`getrandom`).
- [ ] No new `HashMap`, `HashSet`, `Instant`, `SystemTime`, `f32`, or
      `f64` token in `crates/{fx,sim,sim_api,ai,content}/` — even in
      comments. (`tests/architecture_law.rs` scans line by line.)

## Golden hashes

- [ ] No pinned golden hash moved (`tests/determinism.rs`,
      `tests/ai.rs`, `tests/match_rules.rs`, `tests/alpha_loop.rs`,
      `tests/economy.rs`, `tests/combat.rs`, `tests/vision.rs`).

      If a golden moved: cite the test by name and write the reason.
      Goldens move for a *reason* — a behavior change in the tick
      pipeline, the state hash encoding, or the AI's RNG-driven
      decisions. They do not move silently.

- [ ] If the state hash encoding changed: `STATE_ENCODING_VERSION`
      bumped in `crates/sim/src/hash.rs` and the written reason cites
      the encoding delta.

## Hygiene

- [ ] One logical change per commit.
- [ ] Every simplification logged in `docs/DEBT.md` with a repayment
      trigger.
- [ ] Every guessed decision logged in `docs/ASSUMPTIONS.md` (numbered,
      citing the plan section it interprets).
- [ ] No dangerous shortcuts (plan §18).
