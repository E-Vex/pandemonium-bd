# Pandemonium — Architecture

Status: milestone M0 (skeleton + guardrails). The authoritative specification is
[`plan.md`](../plan.md) — §2 frozen decisions, §4 workspace law, §5 determinism rules.
This file is the working map of how the code is actually laid out; update it when the
shape of the system changes, not for every feature. For current build status and what
exists versus what is pending, see [`AI-Handoff.md`](../AI-Handoff.md).

## Workspace

| Crate | Role (plan §4) | Depends on |
|-------|----------------|------------|
| `crates/fx` | Fixed-point math (`Fx`, `Vec2Fx`), integer sqrt, PCG32 RNG, FNV-1a hasher | nothing |
| `crates/sim_api` | Public vocabulary crossing the sim boundary: ids, commands, events, views | fx |
| `crates/sim` | World state, entity store, capabilities, tick pipeline, systems, state hash | fx, sim_api |
| `crates/content` | RON schema, versioned loaders, validators, content bundle + hash | fx, sim, sim_api |
| `crates/ai` | Controllers: perceive a `PlayerView`, decide, emit commands | fx, sim_api |
| `crates/replay` | Replay record/load/verify format | fx, sim_api |
| `crates/engine` | Game loop, timing, interpolation, input mapping, camera, UI toolkit | fx, sim, sim_api, content, ai |
| `crates/client` | Playable binary: wgpu renderer, winit window, screens, HUD | fx, sim, sim_api, content, ai, replay, engine |
| `crates/tools` | Headless runner, soak, replay-verify, content-validate, bench | fx, sim, sim_api, content, ai, replay |
| `tests` (`pandemonium-tests`) | Cross-crate acceptance suite (plan §13) | any — test host, exempt from the internal law |

The graph is enforced — not merely documented — by `tests/architecture_law.rs`, which
parses every member manifest and fails the build on any violation. The plan asks for
"a test that parses cargo metadata"; the test parses the member manifests directly
instead, which is hermetic (no nested cargo invocation, works offline, identical
enforcement). See [`ASSUMPTIONS.md`](ASSUMPTIONS.md).

## Hard rules encoded in the test

1. Dependencies point downward only, per the table above. Dev-dependencies are exempt
   (test-only code never ships), but the forbidden-crate list applies to them too.
2. External crates are allow-listed per member exactly as plan §3.2's table. The
   allow-lists encode the *whole* table from day zero, so adding wgpu to `client` in
   M3 is not a law change — it was always allowed there.
3. `bevy`, `macroquad`, `ggez`, `fyrox`, godot bindings, every ECS crate, physics and
   pathfinding crates, and `rand`/`getrandom` are forbidden workspace-wide.
4. `sim` may never depend on anything above it (engine, client, ai, replay, content,
   winit, wgpu, or any I/O) — FD-6.
5. `ai` may never depend on `sim` — AI parity is structural, a compile error rather
   than a policy (FD-7).
6. The determinism crates (`fx`, `sim`, `sim_api`, `ai`, `content`) may not contain
   unordered-map types, wall-clock reads, or floating-point types — scanned line by
   line by the same test (plan §5). The engine and client are *not* scanned: rendering
   interpolation and real-time timing legitimately use real clocks and float math
   there (plan §11.1); they never enter the simulation.

## Determinism digest (plan §5)

- All sim math is `fx` Q16.16 fixed-point. Authored content uses integer units
  (milliseconds, milli-tiles) converted at load (plan §10.2).
- `fx` arithmetic uses 64-bit intermediates, rounds toward zero, and saturates instead
  of panicking; the contracts are documented on the API and property-tested.
- No unordered maps anywhere simulation state or state-affecting iteration could
  touch. Iteration is ascending id order; sorts are stable with id tie-breaks.
- All randomness flows from `fx::Rng` (PCG32), seeded from the match seed and owned
  by the simulation state, advanced in a fixed order.
- State hashing is an explicit canonical little-endian byte encoding through
  `fx::Fnv1a64` — never `Debug` output or memory layout (plan §6.4).

## The boundary the whole design hangs on

FD-2/FD-6: `Sim::step(commands)` is the only mutator of simulation state, and the
simulation knows nothing above it. Everything the outside world learns arrives as
snapshots, events, and fog-filtered `PlayerView`s (FD-8/FD-9). Every later milestone
preserves this shape; if a task seems to require breaking it, write an ADR first
(plan §0, `docs/adr/`).
