# Proptest Regressions File Policy

**Status:** policy of record (B-002 Part 1) · applies to every crate under
`tests/` and every in-crate `proptest!` suite.

## What the file is

When a `proptest!` case fails, proptest persists the failing seed to a
`<test-file>.proptest-regressions` file next to the test source (for the
acceptance suites: `tests/ai.proptest-regressions`, etc.). On the next run
proptest replays the persisted seeds *first*, then draws fresh cases — the
file exists so a rare, seed-dependent failure stays reproducible after the
process that found it is gone.

## The policy

1. **Default: not committed.** The file is a local diagnostic artifact. An
   untracked `*.proptest-regressions` in `git status` is normal after a local
   red and carries no review meaning.
2. **A regressions file may be committed only when every seed it pins is
   green on the committed tree.** That is the state after the bug it recorded
   is fixed: the file then becomes a permanent regression guard — the pinned
   case replays on every clone, every CI leg, both profiles, forever.
3. **A file that pins a red must never be committed.** Proptest replays
   persisted seeds before drawing fresh cases, so a committed red seed fails
   first on all six CI legs (3 OSes × dev/release) — permanently, for
   everyone, behind no feature flag. It converts a local diagnostic into a
   globally broken gate and hides every *new* failure behind the old one.
4. **The honest conversion for a red case is an explicit, named,
   deterministic test with the inputs written out** — a `#[test]` that
   constructs the failing input directly, with a doc comment explaining the
   behavior it pins. A named test is reviewable, greppable, documents the
   premise, and fails loudly with a message a human wrote. Example of record:
   `tests/ai.rs::a11_mirror_positions_are_not_equivalent_states_units_translate`
   (the old A11 mirror fuzz's failing Build case, written out; see B-001
   §3 / B-002 Part 1). Alternatively, an un-reproducible-in-test finding goes
   to the assumption/debt registers with the seed quoted — not into the tree
   as a pinned red.

## Mechanics for a working session

- A failure dropped a `*.proptest-regressions` file next to the test? Triage
  the case, then either fix the bug or convert the case per rule 4.
- Delete the local file once its knowledge is converted (or commit it only
  under rule 2 — all pins green).
- Never `git add` a regressions file as part of an unrelated commit; if it
  appears in `git status`, that is the signal to apply this policy, not to
  sweep it in.

## History of record

- The A11 mirror fuzz (pre-B-002) drew a rare failing case; its persisted
  seed lived only in an untracked `tests/ai.proptest-regressions` —
  deliberately not committed (B-001 §3 / proposal P3).
- B-002 Part 1 converted that case into the named deterministic test above
  and re-founded the fuzz on label-swapped equivalent states, so no seed
  remains to pin and nothing is committed here.

## Rationale in one line

A regressions file is a *diagnostic* while red and a *guard* once green —
committing it in any other state lies about which of the two it is.
