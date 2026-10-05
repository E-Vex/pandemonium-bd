---
name: Bug report
about: A reproducible incident (a deterministic engine's bug reports are
      unusually actionable when seed + tick are present)
title: "[bug] "
labels: bug
---

## Summary

<!-- One or two sentences: what did you observe, and what did you expect? -->

## Reproducible command

A report that includes the seed and the tick is a reproducible incident.
Anything else is a war story. Fill in the exact command:

```bash
cargo run -p pandemonium-tools -- headless \
  --seed <SEED> \
  --ticks <TICKS> \
  [--p1 ai --p2 ai]
```

- **Seed**: <!-- e.g. 7 -->
- **Tick budget**: <!-- e.g. 300, 27000 -->
- **Slots**: <!-- idle|ai for p1, idle|ai for p2 -->

## Expected vs. observed

- **Expected**: <!-- what should have happened -->
- **Observed**: <!-- what actually happened -->

## Final hash(es)

Run the command twice. If the final hashes differ between runs, that is
a determinism bug (plan §5) — flag it explicitly:

- Run 1 final hash: `0x…`
- Run 2 final hash: `0x…` (if different from Run 1, this is a desync)

If only one run is needed (e.g. a logic bug, not a determinism bug),
leave Run 2 blank.

## Tick at which the bug occurs

<!-- If you can pin the tick (e.g. by running with smaller --ticks
budgets), do — it makes the report dramatically more actionable. -->

## Environment

- **Platform**: <!-- Linux / Windows / macOS -->
- **Rust toolchain**: <!-- `rustc --version`; the project pins 1.98.1 -->
- **Profile**: <!-- dev or release -->
- **WGPU backend** (windowed client only): <!-- Vulkan / Metal / GL / DX12 -->
- **Display** (windowed client only): <!-- X11 / Wayland / native macOS / Xvfb -->

## Output

<!-- Paste the relevant stdout/stderr below. For the headless runner,
include the final-hash line and any `match ended:` line. For the windowed
client, include the F3 debug overlay's tick + hash if reproducible. -->

```
<paste here>
```

## Scope check

- [ ] This is a behavior bug (the simulation produces wrong state) or a
      determinism bug (two runs of the same seed + content + command
      log diverge).
- [ ] This is **not** a balance issue, a missing-feature request, or an
      art/UX nit. Those belong in a regular issue, not a bug report.
- [ ] I have read `plan.md` §0 (operating contract) and §5 (determinism
      rules) and I am not reporting something the plan explicitly says
      is out of scope for the Alpha.
