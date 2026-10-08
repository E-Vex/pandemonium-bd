# ADR-0002: rodio as the client's audio backend

- **Date:** 2026-10-08
- **Status:** accepted
- **Frozen decision affected:** plan §4 dependency law (the client crate's
  external allow-list). No FD-1..FD-10 decision changes: the `AudioSink` seam
  (plan §11.5, FD-9) stays exactly as M9 built it — the sink is fed after the
  step from the presentation layer and feeds nothing back.
- **Proposed by:** E-Vex (project owner) — the backend was pre-chosen as
  A-100 during the M10.2 planning pass; this ADR records the choice, the
  resolved version, and the feature trim.

## Context

DEBT-011 tracked the M9 state: the `AudioSink` trait, the pure event → cue
mapping, and the null counter sink exist, the client feeds them every frame,
but no audible backend does anything with the cues. PLAN-M10.2 §4.1 (Phase 4)
requires one audio backend in the client: "Choose the smallest dependable
option (for example rodio or cpal-based) and justify it in an ADR; update the
architecture-law allow-list for the client crate only." The owner's re-test
of Phase 3 unblocked Phase 4 (A-107's chain, DEBT-015 narrowed to "Phase 4
only"), and A-100 records the owner's decision made in advance: rodio.

## Proposal

Add `rodio` to `pandemonium-client` only, with default features disabled and
exactly the `playback` feature enabled:

```toml
rodio = { version = "0.22.2", default-features = false, features = ["playback"] }
```

and add `"rodio"` to the client's entry in `allowed_external()` in
`tests/architecture_law.rs` (the same amendment the plan's global rules
require for every new external crate). The resolved version is **0.22.2**
(as resolved by `cargo add` on 2026-10-08, locked in `Cargo.lock`).

The client's sink keeps the rodio device stream (`rodio::stream::MixerDeviceSink`
— 0.22's stream-owning type, the successor of the older `OutputStream`) stored
in the sink struct for the App's entire lifetime: dropping it ends playback,
which is exactly the "store the stream or everything goes silent" rule. Sounds
are synthesized PCM (PLAN §4.2 — no decoder formats needed), which is why the
decoder features stay off: rodio is used only for its device-probe, mixing,
and resampling path.

## Evidence

- **The dependency tree stays small.** With `default-features = false,
  features = ["playback"]`, `cargo tree` for the client adds exactly: `cpal`
  → `alsa`/`alsa-sys`/`libc`, `dasp_sample`, `num-rational` (num chain), and
  `thiserror` (already in the tree). The default feature set would have added
  `symphonia` decoders (flac/vorbis/mp3/mp4) plus `rand` for the noise
  generators — none of which a synthesized-PCM pass needs; `rand` additionally
  collides with the plan §3.2 forbidden-crate posture.
- **The API fits the seam.** `DeviceSinkBuilder::from_default_device()` +
  `open_sink_or_fallback()` probes the device once and returns
  `Result<MixerDeviceSink, DeviceSinkError>` — the Err arm is the null
  fallback (PLAN §4.3: CI, Xvfb, headless). `Mixer::add(source)` appends a
  source to the mixer *infallibly* (a channel send), mixes any number of
  concurrent sources, and resamples whatever the source declares to the
  device's rate — no per-cue blocking, no per-cue fallible device work in the
  render loop (PLAN §4.3's "never crash or block the render loop").
- **The error callback is ownable.** The builder accepts a custom stream
  error callback, so a device lost mid-session is observable (a flag the sink
  polls) without the default `eprintln!` path — keeping PLAN §4.3's silent
  fallback and the pass's no-stderr-spam rule.

## Consequences

- The client binary links ALSA on Linux (`libasound`) — a runtime-only system
  dependency the windowed build already tolerated (`cpal` sat on the plan's
  dependency table since day zero; machines without a sound stack take the
  null arm).
- The allow-list change touches `tests/architecture_law.rs` only; every other
  member's law is unchanged, and the sim/sim_api/fx/ai/content crates gain no
  audio knowledge whatsoever (FD-9 intact).
- No golden hash moves: the backend lives entirely above the sim boundary
  (the phase's global rule).
- If rodio's API drifts again (0.20 → 0.22 renamed `OutputStream` to
  `MixerDeviceSink`), the seam absorbs it: only `crates/client/src/sound.rs`
  sees rodio types.

## Alternatives considered

- **Raw `cpal`** (DEBT-011's original sketch): the same device probe plus a
  hand-written mixer thread, sample-format conversion, and resampling —
  exactly the machinery rodio's mixer already is, maintained and tested
  elsewhere. Rejected as duplicated systems code for zero Alpha value.
- **`kira`** (game-audio crate): stronger spatial/parameter model, but a
  heavier dependency tree for sounds this pass deliberately keeps to short
  synthesized mono buffers. Rejected for weight.
- **Do nothing** (keep DEBT-011 open): the plan's Phase 4 exists because the
  playtest found the game silent; the owner already directed the backend
  (A-100). Rejected by the owner.

## Decision

Accepted 2026-10-08: rodio 0.22.2 (playback feature only) is the client's
audio backend behind the existing `AudioSink` seam — the owner's pre-choice
(A-100) recorded with its resolved version and the null fallback as the
constructor's other arm.
