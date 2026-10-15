# readme-assets

The SVG artwork referenced by the top-level [`README.md`](../README.md). Four
pieces, each in a dark and a light variant, selected by the README's
`<picture>` blocks via `prefers-color-scheme`:

| file | role | canvas |
|---|---|---|
| `hero-{dark,light}.svg` | the verification console — the banner under the title | 960 × 400 |
| `architecture-{dark,light}.svg` | the crate map as five tier bands | 960 × 784 |
| `timestep-{dark,light}.svg` | the fixed-tick heartbeat | 960 × 232 |
| `seal-{dark,light}.svg` | the footer brand mark | 240 × 240 |

## Design system

One monospace stack (`ui-monospace, Cascadia Mono, SF Mono, Menlo, Consolas,
Liberation Mono`), GitHub-primed palettes, and **amber as the single accent** —
the same `#9E6A03` family the README badges use. Dark rides on `#0d1117` /
`#161b22` panels; light on `#f6f8fa` / `#ffffff`. Red is reserved for the
forbidden edge (FD-7) and the hostile contact; green for verified states;
blue for the render lane, the one place floats are allowed.

Motion is part of the system, under one law: **every animation is decorative
and every one of them stops under `prefers-reduced-motion: reduce`.** The
hero's eleven checkpoint nodes fill in sequence over an 11-second loop (one
second per checkpoint — 30 ticks at a deliberately slowed cadence); the
reduced-motion fallback freezes every node in the amber, verified state. The
timestep's accumulator fills and its sweep cursor scan; both stop on request.
The seal is static, as a stamp should be.

## Content rules

- Every label is fact-grounded in the repository or it does not ship: FD
  numbers cite `plan.md` §2, tick and catch-up figures cite §11.1, the golden
  hashes (`0xb6fff6659cfb7709`, `0x9bc18c521107b262`) are the pinned goldens
  from `AI-Handoff.md`, and the fx crate's hashers are listed as built
  (`Q16.16 · PCG32 · XXH64`, with FNV-1a surviving only as the replay file
  checksum — see `crates/fx/src/hash.rs`).
- The hero's "chaos" strokes are generated from a fixed seed (7, naturally),
  so the artwork itself is deterministic — regenerate the set and you get the
  same picture, byte for byte.
- Alt text in the README describes each piece; when a drawing changes, its
  alt text changes with it in the same commit.
