# ADR-0204: The evaluative AI — a second controller above the command interface as §19 Update 8

- **Date:** 2026-11-12
- **Status:** proposed (the owner's standing delegation names this work — plan §19's
  sequence lists "8 evaluative AI above the command interface" immediately after
  Update 7, and both the AI-Handoff's §4 pointer and ADR-0203's own Consequences name
  it — so the ADR and the implementation land together; the verdict field below stays
  open for the owner's review, exactly as ADR-0202/0203's did)
- **Frozen decision affected:** **FD-7** (plan §2 — parity: every issuer through one
  gate): *proven, extended in population* — a second controller (`TacticalController`)
  emits `Research` and `CastAbility` through the unchanged validation gate, never any
  other door; the A5/A11 parity pins hold untouched because the gate, the checks, and
  the reject reasons are unchanged. **plan §9.6** (the AI contract): *extended within
  its own law* — the plan's Controller trait, the perceive/decide/act seams, the
  seeded-RNG rule, and the knowledge boundary (fog-filtered view + public map/faction
  facts as configuration) are reused verbatim; what is new is only *what the decide
  stage evaluates*: the view's own `research`, `researched`, `abilities`, `fog`, and
  `entities` fields — outward data every issuer already receives (FD-9), now read by
  a controller. **plan §10.1** (the data/code boundary): *reaffirmed* — the
  controller is code parameterized by an `EvalPlan` derived from the content by
  capability shape (the `AiPlan` rule); no content scripts AI. FD-1, FD-2, FD-3
  (untouched — no capability, no store, no hash change), FD-4 (no schema change),
  FD-5 (integer math only), FD-6, FD-8 (the controller sees the fog-filtered view,
  exactly the scripted one), FD-10 are unchanged and bind everything below.
- **Proposed by:** the Programmer (brief B-035; recording the owner's standing
  delegation — "lets keep working on upgrading this game… I want to make a game that
  can beat Generals Zero Hour", with §19's sequence naming Update 8 next after B-034)

## Context

Plan §19 sequences the post-Alpha expansion as an audit of the foundation: Updates 1–5
proved the content tree absorbs data-only growth; Update 6 (ADR-0202) proved the
command architecture absorbs a new verb; Update 7 (ADR-0203) proved it absorbs a
second axis of progression. **Update 8 asks the foundation's own founding question,
inverted: does the AI seam absorb a second *brain* — an opponent that uses the verbs
and the progression Updates 6 and 7 added — without bending the trait, the gate, the
pins, or the determinism law?**

The pressure is concrete, and the project's own registers name it:

1. **The scripted opponent fields none of the second tier.** A-239 records it as this
   brief's territory: "the scripted opponent fields none of the second tier nor any
   upgrade (the AI's adoption is §19 step 8's evaluative-AI territory)". The B-034
   report's Proposals section says the same: "AI research … Update 8's evaluative AI
   is the right home; doing it now would move the AI pins this brief explicitly
   holds." A player who raises a Forge fights an opponent that never will.
2. **The target game's identity is exactly this.** Generals Zero Hour's opponent
   researches upgrades and uses actives on cadence; "an AI that casts and researches
   through the same door" is the AI-Handoff's own one-line description of Update 8.
   Without it, two of the four dimensions of counterplay shipped in B-033/B-034
   (abilities, upgrades) exist only for the human side of the table.
3. **The seams are proven, not speculative.** The `Controller` trait is one method;
   `AiMatchHost` accepts any `Box<dyn Controller>` per slot (sorted, one per slot);
   `PlayerView` already carries `abilities`, `research`, and `researched` (ADR-0202
   §7, ADR-0203 §7 — the command card reads them; a controller can too, they are
   outward data by construction); the gate's Research/CastAbility checks are
   issuer-blind; the replay codec already encodes both commands (tags 11/12).

What does not exist at the pinned head `9ea2c66`: no second controller (the `ai`
  crate exports exactly `ScriptedController`), no controller ever issues `Research`
  or `CastAbility` (verified by search over `crates/ai`), and no host seam derives a
  research/ability plan from the content.

## Proposal

### 1. The controller: `TacticalController` (scripted economy, evaluative tech)

A second controller in the `ai` crate (new module `evaluative.rs`), constructed per
player from an `EvalPlan` and driven by the same host loop (`AiMatchHost` or
`MatchHost::with_controllers` — no host change). It is **the scripted opponent plus
two evaluative systems**, not a rewrite: the economy, infrastructure, defense, and
wave behaviors are the `ScriptedController`'s own (delegated by composition — the
controller owns a `ScriptedController` and forwards every think to it, then runs its
two systems after). This is the §19 reading of "evaluative AI **above** the command
interface": the foundation does not move underneath it; it decides *more*, through
the same door.

### 2. The plan: `EvalPlan` (capability-shaped, host-derived, data-never-names)

`EvalPlan { base: AiPlan, research: Option<ResearchDirective>, abilities:
Vec<AbilityDirective> }`, derived by the engine (`eval_plan`, beside `alpha_plan`)
from the loaded content by capability shape — the AiPlan law, verbatim:

- **`ResearchDirective`** — the first structure kind that carries `Upgrades`
  (resolved over the faction's buildable roster, the `build_spots` ground, its cost,
  build time, and the tree's authored slot list with each slot's cost, prerequisite
  edges, and research ticks). One directive: **build the carrier when the bank can
  pay for it and the army is fed, then research every slot the prerequisites allow,
  cheapest-first by authored order**, re-trying on a fixed cooldown (the
  acting-blind, verifying-by-sight rule: the view's `researched` list is the
  completion signal; `research` is the in-flight signal; a slot that never completes
  is retried on the pending-order expiry the scripted half already uses).
- **`AbilityDirective`** — one per (caster kind, slot) the faction's roster can
  field: the caster kind, the slot, the cast range, and the effect kind's own
  decision rule (below). Directives fire on a fixed cadence when their caster is
  **sighted in the controller's own view with a legal target** — the fog law is the
  gate's own (FD-8): the controller never targets a point it cannot see, because the
  view is all it has.

The host derivation resolves everything the command card shows a player: prices,
ranges, cooldowns, prerequisites. Nothing is name-matched; the ids arrive as data.

### 3. The two decision rules (closed, small, integer-only)

- **Research** (the tech tree): when the carrier stands complete and idle, and the
  issuer's ledger covers a slot's cost, and every prerequisite of that slot sits in
  `view.researched`, and neither `view.research` (own in-flight) nor the directive's
  own pending bookkeeping holds it — emit one `Research` command for the **first
  such slot in authored order**. Cooldown after any emission; retries on expiry.
- **Abilities** (the actives): for each directive whose caster is in view and whose
  slot's `AbilityView.cooldown_milli` is zero — pick the target by effect kind:
  `DamageArea` aims at the **centroid of the largest cluster of sighted enemy
  combatants** (a deterministic grid-bucket count over the view's entities: bucket
  the sighted enemies by a fixed tile grid, take the densest bucket's centroid,
  nearest-bucket tie by ascending bucket key; if no enemy is sighted, the directive
  is silent); `HealArea`/`SpeedBurst` aim at the **most damaged friendly cluster**
  (same bucketing over own entities, `hp_fraction_milli` as the weight — heal picks
  the lowest-health cluster, speed picks the cluster nearest the wave's target).
  The cast point is clamped to the caster's authored range (the controller walks
  nothing; the gate's `AbilityOutOfRange` is respected by construction) and must be
  sighted (the fog law above). Cooldown after any emission.

Both rules are pure functions of (view, tick, own state): fixed iteration order
(view lists are ascending by id by construction), no RNG draws at all (the
`ScriptedController`'s seeded RNG continues to serve the wave jitter — the
evaluative systems add zero draws, so the shared RNG stream is untouched by this
layer sitting above it).

### 4. Hosting: `eval_controller` beside `alpha_controller`

The engine gains `eval_controller(bundle, world, player, seed, mission)` and
`eval_plan(...)` (the `alpha_*` twins, same signature, same mission-tuning pass-
through to the base plan). The tools' `headless` gains one CLI spelling: `--p1
tactical` / `--p2 tactical` (the `Slot` enum grows `Tactical`; the replay's recorded
`ControllerKind` stays `Ai` — the kind is metadata about *which side of the parity
law* drives the slot (A-060), not which brain; FD-1's identity is seed + content +
the command log, which differs, and that is the whole point). The client's
`PlayerVsAi` mode is untouched in this brief (the client swap is a presentation
decision for the owner; recorded in §Consequences).

### 5. Pins: the scripted opponent is bit-identical, the tactical run pins fresh

The `ScriptedController`, `AiPlan`, and `alpha_plan` are untouched (one exception:
none). Every existing pin — the flagship golden `0x3a58e55ea6250906`, the 612-command
count, the command-stream digest, the controller RNG streams, the demo golden —
holds by construction: the tactical controller lives in *new* code paths no pinned
run constructs. The tactical run gets **its own** pins (a `tests/ai_evaluative.rs`
suite: determinism run-to-run, replay round-trip through `run_command_log`, gate
rejections exercised through the real bundle, and a fresh golden for a
tactical-vs-scripted match at the flagship shape). No registered golden moves; no
content hash moves (zero content edits).

### 6. The acceptance suite: `tests/ai_evaluative.rs` + unit tests

The A5/A11 parity laws, re-proven for the second brain: the tactical match's ledger
balances to the cent through the *data-defined* prices (research spends included);
every Forge and every upgrade in the ledger traces to an accepted `Research`
command; the tactical run is bit-identical run-to-run; the recorded log replays
without any controller; and the A11 mirror battery needs no extension (the gate is
issuer-blind — its pins already cover both verbs). Unit tests in `evaluative.rs`
cover the two decision rules' laws (the fog law, the range clamp, the cooldown, the
prerequisite rule, the cluster pick's determinism) on synthetic views, the
`ScriptedController` test discipline verbatim.

## Evidence

- **The sequence's own ruling:** plan §19 names Update 8 (evaluative AI above the
  command interface) as the next step after Update 7; the AI-Handoff §4 and ADR-0203
  §Consequences both point here; B-034's report §9 names this brief's shape.
- **The seams exist and are unexercised:** the one-method `Controller` trait, the
  `Box<dyn Controller>` host slot, `PlayerView.abilities/research/researched`
  (outward data by FD-9's construction — the command card's own sources), the
  issuer-blind Research/CastAbility gate checks, the replay codec's tags 11/12.
- **The pin-stability proof is executable:** after implementation, the flagship
  `headless --seed 7 --ticks 7200 --p1 ai --p2 ai` still prints `0x3a58e55ea6250906`
  and the demo still prints `0xb6fff6659cfb7709` (both pinned by existing tests and
  CI; the scripted paths are not compiled away, just not constructed by pinned
  runs); the content hash `0x4d362e1a17b9eefc` and map id `0xd38136401ab02ff1` do
  not move (zero content edits — the first §19 update with none).
- **The acceptance suite:** `tests/ai_evaluative.rs` (the parity laws above, the
  fresh tactical golden, the replay round-trip) plus the unit tests in
  `crates/ai/src/evaluative.rs`.

## Consequences

- **Cheaper:** the fourth and final dimension the sequence audited (an opponent that
  uses it) is populated; every future verb and progression axis inherits a second
  consumer for free (any controller through the same door); the client's opponent
  swap becomes a one-line change when the owner asks; the tools can pit tactical
  against scripted to measure the upgrade's worth in AI-vs-AI evidence.
- **More expensive:** the `ai` crate grows a second module and the engine a second
  plan derivation (both under the dependency law — `ai` still reaches only `fx` and
  `sim_api`); the tools' CLI grows one slot spelling; a new acceptance suite and its
  fresh goldens join the CI budget.
- **Breaks and migrations:** none. Zero content edits, zero sim/sim_api/replay
  changes, zero client changes, zero registered-golden moves. Every existing test,
  pin, and replay verifies unmodified (the tactical run's replays are new files).
- **Honest limits (recorded, not waved through):** the evaluative layer is authored
  heuristics, not search — "evaluative" names its seat (above the command interface,
  reading the view's own evaluation surfaces), not a utility solver; the client still
  fields the scripted opponent in `PlayerVsAi` (the swap is the owner's presentation
  call — recorded here, one line when wanted); ability targeting is cluster-shaped
  (no per-unit focus-fire heuristics); no new RNG draws exist to pin. The feel of an
  opponent that researches under pressure rides A14 (A-230's standard).

## Alternatives considered

1. **Extend `ScriptedController` with research/cast steps** (one brain, more
   branches). Rejected: every scripted pin (the 612-command count, the digest, the
   RNG streams, the flagship golden) would move or need re-proof under new
   conditionals, and the §19 sequence's point — does the seam absorb a *second*
   consumer — would go unasked. The composition keeps the scripted opponent
   bit-identical by construction.
2. **A learned or searched AI** (utility scoring, MCTS over the view). Rejected for
   Update 8's scope: plan §9.6's Alpha behavior is scripted by design; a solver
   needs a fast state clone the sim does not expose (and would tempt FD-7's most
   common violation — reading sim state directly). The heuristic layer is the
   honest increment; the ADR's §Consequences records the upgrade path.
3. **AI research only** (the B-034 report's minimal proposal). Rejected as
   incomplete: Update 6's abilities shipped with the same AI gap (A-236's note), and
   the two systems share every seam (the view's own lists, the cooldown discipline,
   the acting-blind retries) — landing one and re-opening the brief for the other
   doubles the review surface for no saving.
4. **Do nothing** (the scripted opponent suffices). Rejected: the owner's goal is a
   game that beats Generals Zero Hour; that game's opponent researches and casts.
   Two of the four counterplay dimensions shipping human-only is the measured gap
   this brief closes.

## Decision

<pending the owner's verdict — the implementation lands under the standing delegation
with this ADR in `proposed`, the ADR-0201 flow.>
