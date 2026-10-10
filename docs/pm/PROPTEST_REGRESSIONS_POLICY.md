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

## The seed policy (B-005)

Three execution contexts, three deliberate seed regimes — one per job of the
merge pipeline, each matching what that job is *for*:

1. **PR CI: one pinned, documented constant.** ci.yml's `test` job sets
   `PROPTEST_RNG_SEED: "20261011"` (one of B-004's five verified-green
   seeds — green at 10,000 cases per property in dev and release, and
   re-verified at the committed 256/4096 counts on master `88ea689` in both
   profiles). Every leg of every PR run draws the identical case stream, so
   **a red PR is a property of the code under the pinned seed — never a
   lottery**; the CI #77 class is closed at the gate. The run step echoes
   the seed in force, so every leg's log is self-describing. The seed
   rotates only by brief, exactly like the runner pins (A-201) — never by
   drift: a frozen constant that changes silently is a coverage regression
   no review can see.
2. **Nightly: random-but-logged.** nightly.yml's `proptest` sweep derives
   the seed from the run id, prints it at the start of the sweep, and
   prints it again — with the exact repro command — in any failure summary.
   The nightly is the exploration tier: fresh input regions at volume
   (fx 100,000 cases; the A11 fuzz 40,000 — two sims constructed and
   stepped per case, so it is the heavy tier), which is exactly what
   per-push deterministic runs cannot give. A green nightly says "no red in
   tonight's input region", not "the properties are safe everywhere".
3. **Local: random (the proptest default).** Nothing in the repo pins a
   local seed; a developer who wants determinism exports `PROPTEST_RNG_SEED`
   per command. Random local runs are free extra exploration, and proptest
   prints the seed of any local red in its failure banner.

**How to reproduce any red:** copy the seed from where the context prints it
(the PR log's "proptest seed in force" line; the nightly's announce or
failure-summary step; a local failure banner) and re-run the same command
with it, e.g.
`PROPTEST_RNG_SEED=38090000250 PROPTEST_CASES=40000 cargo test -p pandemonium-tests --test ai a11_fuzz_label_swapped_commands_validate_identically`.
proptest 1.11.0 applies the env var after any in-file config
(`contextualize_config`), so it overrides even committed `ProptestConfig`
settings. With the seed unset, proptest seeds from OS entropy per process —
which is why local runs differ from each other and from CI.

**A red nightly converts exactly like a red anywhere else: the named
deterministic test comes first** (rule 4 above). Reproduce with the logged
seed, triage the case, and either fix the bug or write the case out as an
explicit `#[test]` with the inputs and expected behaviour documented — never
silence the red with a re-roll, and never commit the seed as a pinned red
regressions file. The seed and the case count are in the log precisely so
this conversion is mechanical.

## Premises that do not imply the claim (the B-004 defect class)

A property test carries two things: a **premise** (the input ranges, plus any
`if` guard in the body) and a **claim** (what is asserted). The premise must
*imply* the claim — every input the premise admits must be one where the
claim is mathematically true — otherwise the property is a lottery ticket:
random draws eventually land in the gap and the gate goes red on code that is
honouring its contracts. The fx CI #77 flake was exactly this: the guard was
`b != 0` (with raws drawn from ±2^20) but the claim was
`checked_div == Some(div)`, which is true only where the quotient
`a·2^16/b` fits in i32 — roughly `|a| ≤ 32768·|b|` — so the guard admitted
inputs the claim could not survive. When re-founding a property, prefer an
**independent oracle** (recompute the expected answer in wider integer math,
e.g. i128, and branch on *that*) over an input-range premise: the oracle
decides every branch, so no untested gap exists. CI #77's red is now the
`checked_div_is_exact_or_none_against_the_i128_quotient_oracle` fuzz plus the
`checked_div_pins_the_flake_and_representability_boundaries` deterministic
test (B-004).

## History of record

- The A11 mirror fuzz (pre-B-002) drew a rare failing case; its persisted
  seed lived only in an untracked `tests/ai.proptest-regressions` —
  deliberately not committed (B-001 §3 / proposal P3).
- B-002 Part 1 converted that case into the named deterministic test above
  and re-founded the fuzz on label-swapped equivalent states, so no seed
  remains to pin and nothing is committed here.
- CI #77 (master c876174, run 38076181010): the fx
  `checked_div_agrees_in_range` property drew a rare failing case
  (a=-393216, b=-1 raw) — a false premise, not an fx bug (the true quotient
  25_769_803_776 raw is unrepresentable, so `None` was the documented
  behaviour). B-004 re-founded the fuzz on the i128 quotient oracle, pinned
  the case and the representability boundaries as the deterministic test
  above, and reverted the seed that the local red appended to the tracked
  `crates/fx/tests/properties.proptest-regressions` — no new seed committed.
- B-005 pinned the PR seed (`20261011`) on ci.yml's test legs, added the
  nightly's run-id-seeded proptest sweep (fx 100,000 / A11 fuzz 40,000
  cases, dev, inside a stated 30-minute cap), and wrote the seed policy
  above. The mechanism — env var lands in `Config::rng_seed` after any
  in-file config and seeds the per-property ChaCha RNG — is verified in the
  shipped proptest 1.11.0 source (config.rs `RNG_SEED`, runner.rs
  `TestRunner::new`, rng.rs `default_rng`); the same-cases-on-two-runs
  proof is a scratch-crate harness recording every drawn case (byte-
  identical logs under one seed, differing logs without one, differing
  logs under a different seed).

## Rationale in one line

A regressions file is a *diagnostic* while red and a *guard* once green —
committing it in any other state lies about which of the two it is.
