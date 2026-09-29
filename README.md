# Pandemonium

A deterministic real-time strategy (RTS) game foundation and alpha plan, written
in Rust with a fully custom engine stack — no Bevy, no ECS crate, no game
framework. Every layer (game loop, entity storage, rendering, fixed-point math,
pathfinding, UI toolkit) is built from scratch.

## Highlights

- **Deterministic simulation** — 30 ticks/s, Q16.16 fixed-point math only (no
  floats in sim or content), lockstep-safe across platforms.
- **Custom math crate (`fx`)** — fixed-point scalars/vectors, integer square
  root, PCG32 RNG with canonical seeding, FNV-1a 64-bit hashing.
- **Architecture enforced by tests** — dependency direction, external crate
  allow-list, and determinism rules (no unordered maps, no wall-clock, no
  floats) are verified automatically on every test run.
- **Replay-first** — everything a player does is an intent-validated Command
  applied at tick boundaries; replays are input logs with golden-hash
  verification.

## Status

Milestone **M0 (Skeleton & Guardrails)** is complete: the 9-crate workspace,
CI, lint bans, architecture-law tests, and the `fx` math crate are done, with
57 tests green. The roadmap (M1–M10) lives in [`plan.md`](plan.md).

## Quick start

Requires Rust 1.98.1 (pinned via `rust-toolchain.toml`).

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Repository layout

```
crates/    fx, sim_api, sim, content, ai, replay, engine, client, tools
tests/     acceptance tests + architecture law tests
content/   data-only game content (entities, factions, maps, rules)
docs/      ARCHITECTURE, DEBT, ASSUMPTIONS, CONTENT_GUIDE, ADRs
plan.md    the full v2.0 foundation & alpha plan
```

## Documentation

- [`plan.md`](plan.md) — the complete plan: frozen decisions, milestones
  M0–M10, acceptance criteria, process contracts.
- [`AI-Handoff.md`](AI-Handoff.md) — status board and resume protocol; start
  here if you are picking up development.
- [`docs/`](docs/) — living architecture and process documentation.
