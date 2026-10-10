# RELEASING — how a Pandemonium build becomes a download

The operational counterpart to `.github/workflows/release.yml`. Everything
here is the PM's lever set; the agents' lever set is the workflow file
itself.

## The two channels

**`workflow_dispatch` (dry run, no publishing).** Actions tab → *release* →
*Run workflow* → pick the ref. Builds client + tools in release on the three
pinned runners, packages one archive per OS (binaries, `content/` — kept
external so the bundle hash is untouched — `README.md` with a Linux-only
runtime-deps appendix, `QUICKSTART.md`, `THIRD_PARTY_NOTICES.md`), records
SHA-256 sidecars, then a separate smoke job unpacks each archive into a
clean directory **on that archive's own OS** and runs the packaged client's
`--headless` smoke plus `pandemonium-tools content-validate` against the
packaged content, and verifies the checksum. Artifacts land under the run's
Artifacts section (`release-linux-x86_64`, `release-windows-x86_64`,
`release-macos-aarch64`, `release-notices`). Nothing is published.

**A `v*` tag push (the PM's trigger — no one else pushes tags).** Same
builds, same smokes, plus `gh release create` publishing a **pre-release**
with the three archives and a consolidated `checksums.txt`. The workflow
never pushes a tag itself; the release exists only because the PM did.

## The gates, in order of authority

1. **The licence gate** (`licence-gate`): cargo-deny (pinned 0.20.2) checks
   the dependency graph against `docs/product/deny.toml` — the allow-list
   (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, MPL-2.0, and CC0-1.0 by the
   PM's A-003 ruling). Anything outside fails the run. **It blocks
   publishing** (`publish` needs it) but not artifact production — a red
   gate still yields inspectable archives in the dispatch channel, because
   a licence decision is the PM's, and evidence beats silence.

   **The A-003 ruling (flag closed):** `hexf-parse 0.2.1` (pulled by `naga`
   ← `wgpu` ← `pandemonium-client`) is **CC0-1.0**; the PM approved adding
   CC0-1.0 to `deny.toml`'s `allow` — a permissive public-domain dedication,
   standard in the Rust graphics stack, no copyleft, no attribution
   requirement. With it in place the gate runs green (the A-003 report
   carries the run URL). **Only the PM edits the allow-list or the
   exceptions block** — that is the point of the gate; the next
   out-of-list licence reopens this paragraph.

2. **The build matrix** — release builds on pinned runners, with the
   version stamp (`--version` prints semver + git short SHA + content hash)
   as an always-visible step.

3. **The packaged-archive smoke** — an archive that does not *run from a
   clean directory on its own OS* is not a deliverable, whatever built it.
   On Windows this is also the product-level access-violation canary: the
   smoke runs the real audio device probe (no endpoint on a runner — the
   documented silent fallback, or the crash the pipeline must name).

## Pinned runner images

| Leg | Image | Target | Archive |
|---|---|---|---|
| Linux | `ubuntu-24.04` | `x86_64-unknown-linux-gnu` | `tar.gz` |
| Windows | `windows-2022` | `x86_64-pc-windows-msvc` | `zip` |
| macOS | `macos-15` | `aarch64-apple-darwin` | `tar.gz` |

Pins are deliberate (B-002's rule): a `-latest` label is a moving fleet, and
a release build should be reproducible to the runner image it was built on.
Bumping a pin is a normal review line in the workflow file, not an ambient
surprise. The macOS build is **aarch64 only** (Apple silicon; Intel Macs are
not served by this pipeline today — flag to the PM if a tester needs one).

Toolchain: 1.98.1 exactly, per `rust-toolchain.toml`. Tools in the pipeline
are version-pinned too (cargo-deny 0.20.2, cargo-about 0.9.2) — same rule,
same reason.

## SmartScreen and Gatekeeper (builds are unsigned — for now)

The archives carry no code signature. That means the OS interposes once per
download; the bypasses are documented on the tester's side in
`QUICKSTART.md`, and repeated here for the release operator:

- **Windows (SmartScreen):** the first launch shows "Windows protected your
  PC". Click **More info → Run anyway**. One-time per downloaded file.
  The long-term fix is a code-signing certificate — an owner purchase
  decision, not a workflow change; when it exists, sign in the build job
  before checksumming (the checksums then cover the signed binary).
- **macOS (Gatekeeper):** double-clicking an unsigned binary is blocked.
  Run from Terminal (`./pandemonium-client`), or right-click → **Open**, or
  one-time dequarantine the download:
  `xattr -d com.apple.quarantine pandemonium-client`.
- **Linux:** no signature story; the only runtime dependency is
  `libasound2` (see QUICKSTART.md — the loader-level fact: without it the
  process cannot start at all, so the game's own silent audio fallback
  never gets the chance).

## What the version line means

`pandemonium-client --version` (and the F3 overlay's first line) prints:

```
pandemonium-client 0.1.0 (git 2bcb25a, content 0x9bc18c521107b262)
```

- `0.1.0` — the workspace semver.
- `git 2bcb25a` — the exact commit, stamped at build time by
  `crates/client/build.rs` ("unknown" for tarball builds — honest).
- `content 0x…` — the loaded content tree's bundle hash: the same identity
  `tools content-validate` prints, and the value the A14 bug-report promise
  wants next to every incident. A tester's report that quotes this line
  names the build, the commit, and the content in one breath.

A tag build's archive names carry the tag (`pandemonium-0.1.0-…`); a
dispatch build carries the commit (`pandemonium-git-<sha>-…`).

## The release checklist (PM)

1. Decide any open licence flag (none open today — hexf-parse/CC0-1.0 was
   ruled in by brief A-003; a new out-of-list licence reopens this line).
2. Run a `workflow_dispatch` dry run on the release commit; read the smoke
   jobs' output — all three OSes green, `--version` line correct.
3. Download the three artifacts (or spot-check one) — unpack somewhere
   clean, run it, feel the SmartScreen/Gatekeeper flow once yourself.
4. Push `v<semver>` on master (your trigger, your tag — the workflow
   publishes the pre-release with checksums).
5. Link the pre-release to the playtest channel with the QUICKSTART pointer
   — the archive already carries the card.

## Out of scope by design (the owner's decisions)

- The project's own licence text (the README defers to the owner; the
  archives state nothing about it either).
- Code-signing certificates (Windows/macOS trust story).
- Intel-mac or arm-Linux targets.
