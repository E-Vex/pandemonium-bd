# ADR-0203: The upgrade system — researchable permanent upgrades as §19 Update 7

- **Date:** 2026-11-09
- **Status:** proposed (the owner's standing delegation names this work — plan §19's
  sequence lists "7 tech tree / upgrades" immediately after Update 6 — so the ADR and
  the implementation land together; the verdict field below stays open for the owner's
  review, exactly as ADR-0202's did)
- **Frozen decision affected:** **FD-3** (plan §2, plan §7.3 — the unified entity model):
  *extended* — the capability vocabulary gains **`Upgrades`** (an authored spec list,
  parameters only — the `Abilities` pattern minus a per-slot runtime, because the
  runtime is a separate injected block) and the sim-internal **`Research`** runtime
  block (never authored in content — the `Research` command injects it, completion
  removes it; the `Construction`/`SpeedBoost` pattern). **FD-4** (plan §2, plan §10.1 —
  the data/code boundary): *extended, and the extension is the ADR's point* — upgrade
  definitions (which upgrades a kind offers, their numbers, their prerequisite edges,
  their target-kind lists) are versioned data; the effect vocabulary (what an upgrade
  can DO) is a closed set in code; data selects and parameterizes, never scripts.
  **plan §8.1** (the command set): *extended* — one new variant, `Research`, through
  the unchanged gate (plan §8.3's law, the CastAbility precedent). **FD-5** (plan §5):
  binds everything — no floats, no unseeded randomness; multipliers are Q16.16 applied
  with `Fx::mul`'s documented round-toward-zero. **FD-9** (events outward): the start
  and completion cues and the two view lists are outward data, never a poll of sim
  internals. **FD-10** (plan §2 — content schema moves only forward): the new
  capability block is additive at a new tag; existing files parse identically. FD-1,
  FD-2, FD-6, FD-7, FD-8 are unchanged and bind everything below.
- **Proposed by:** the Programmer (brief B-034; recording the owner's standing delegation —
  "lets keep working on upgrading this game… I want to make a game that can beat
  Generals Zero Hour", with §19's sequence naming Update 7 next after B-033)

## Context

Plan §19 sequences the post-Alpha expansion as an audit of the foundation: Updates 1–5
(B-029…B-032) proved the content tree absorbs data-only growth; Update 6 (B-033,
ADR-0202) proved the command architecture absorbs a new verb. **Update 7 asks the next
question: does the architecture absorb a second axis of progression — permanent,
researched modifiers — without bending the production model, the hash, or the gate?**

The pressure is concrete, not hypothetical:

1. **The target game's spine is upgrades.** Generals Zero Hour's economy-of-advantage
   runs through researched upgrades (weapon kits, armor kits, supply speed) — a roster
   that cannot improve in place converges on unit-mix rock-paper-scissors, the same
   flat-stat ceiling A-227/A-230 measured for this roster (ADR-0202's own Context).
   Abilities (Update 6) added the transient fourth dimension; upgrades add the
   *permanent* one.
2. **The production system already has the shape.** Research is production's exact
   model with a different completion surface: cost paid at enqueue, a timed project,
   one advancing head. Plan §9.4's stage 3 was built for exactly this kind of growth;
   nothing else in the eleven-stage pipeline moves.
3. **The seams are proven, not speculative.** B-033 landed the `Abilities` capability
   pattern, the injected-runtime-block pattern, the additive mask slots, the replay
   tag growth, and the client card row — every one of them reusable here with zero
   architectural change (the B-033 report's own Consequences named this ADR's
   consumers).

What does not exist at the pinned head `3121fba`: no upgrade anywhere (verified by
search — no capability, command, event, or content field names one), no permanent
stat-modifier state of any kind (the `SpeedBoost` is the only modifier runtime, and it
is transient and per-entity), and no per-player state beyond the ledger, population,
and resignation.

## Proposal

### 1. The capability: `Upgrades` (authored specs, static)

One capability block carries every upgrade a kind offers — multi-slot by construction:

```ron
// content/entities/forge.ron (the shipped example, §9)
Upgrades(( upgrades: [
    ( id: "weapon_systems", display_name: "Weapon Systems",
      effect: AttackDamage(( multiplier_milli: 1250, targets: [ "rifleman", "raider" ] )),
      research_time_ms: 20000, cost_ore: 250, requires: [] ),
    ( id: "field_engines", display_name: "Field Engines",
      effect: MoveSpeed(( multiplier_milli: 1150, targets: [ "worker", "pathfinder" ] )),
      research_time_ms: 15000, cost_ore: 200, requires: [ "weapon_systems" ] ),
] )),
```

Simulation-side (`world.rs`), the block is `UpgradesDef { specs }` — parameters only.
Slots are addressed by authored index (`u8` in the command); ids are for humans,
tooling, and prerequisite resolution. The validator enforces: a non-empty list, ids
unique across the WHOLE tree (ids are the tech tree's global vocabulary — a
prerequisite resolves to exactly one (carrier kind, slot) pair), positive
multipliers, positive research time, non-negative cost, non-empty target lists that
resolve to loaded entities, `requires` lists that resolve to known upgrade ids, an
acyclic `requires` graph, and a Footprint on the carrier (research happens at
buildings).

### 2. The effect vocabulary: two kinds, closed in code

An upgrade's effect is one of two code-defined kinds, parameterized by data:

- **`AttackDamage { multiplier, targets }`** — a permanent attack-damage multiplier
  for the owner's entities of the targeted kinds, read live where the hit is built:
  combat's `PendingHit.damage` flows through `World::effective_damage` (the authored
  base times every matching multiplier the attacker's owner holds, in researched-store
  order — a sorted store, so the application order is deterministic).
- **`MoveSpeed { multiplier, targets }`** — a permanent movement-speed multiplier,
  read live inside `World::effective_speed`: `speed × upgrade × boost` — permanent
  modifiers first, transient last, one fixed order and one fixed rounding (ADR-0202's
  own `effective_speed` seam, composed).

The designed-but-unimplemented third kind — **`MaxHp`** — is recorded in
§Consequences: it needs a completion-time mutation of existing entities plus a spawn
hook (two new seams), and no shipped content wants it yet; the vocabulary stays
closed at two until a brief asks. Why closed at all: plan §10.1's law, verbatim the
ADR-0202 ruling — effects are behavior; behavior is code.

The integer law is explicit: multipliers are `Fx::from_milli(multiplier_milli)`
applied with `Fx::mul` (round toward zero, 64-bit intermediate, saturating) —
8 × 1.25 is 10 exactly; 8 × 1.2 is 9.6 and floors at 9. Authored numbers ride A14
with this law in force (A-241, A-230's standard).

### 3. The command: `Research` (one researcher, one slot)

```rust
CommandKind::Research { researcher: EntityId, upgrade: u8 }
```

One researcher per command. No ground point — research has no target, so the client's
click submits directly (the Train rule; no armed targeting). The cost is spent at
enqueue (the Train economy rule — paid, never refunded; there is no cancel verb in
Update 7, a recorded debt).

### 4. The validation gate: seven checks, three new reasons

The gate is the sole authority (plan §8.2), shared by every issuer (FD-7). In order:

1. researcher exists and is owned by the issuer (`UnknownEntity` / `NotOwnedByIssuer`);
2. researcher carries `Upgrades` (`MissingCapability`) and is `Active`;
3. the slot index is inside the kind's list (**new reason `UpgradeSlotInvalid`**);
4. the player does not already hold this upgrade — research is once per PLAYER, not
   per building (**new reason `UpgradeAlreadyResearched`**);
5. every prerequisite (carrier kind, slot) pair is in the player's ledger (reused
   `RequirementsUnmet` — the tech tree's edges);
6. the researcher's project slot is idle AND the same upgrade is not already in
   flight at another of the issuer's buildings — a duplicate start is a wasted purse,
   refused at the one door every issuer shares (**new reason `ResearchBusy`**, one
   reason covering both scopes);
7. the issuer can pay `cost_ore` (reused `CannotAfford`).

### 5. The pipeline (stage 3, at the head) and the researched ledger

`upgrades::advance_research` runs at the head of stage 3 (production), before the
queues — research is production's shape (plan §9.4), and the placement rule is
abilities-at-stage-7's: the new pass at the stage's head. Semantics mirror the
production head exactly: a project enqueued at tick T advances for the first time at
the same tick's stage 3 (firing `ResearchStarted`, the `ProductionStarted` rule) and
completes when its elapsed count reaches the spec's `research_ticks` (firing
`ResearchCompleted`, removing the block, and appending to the player's ledger).

**The ledger is the design's center:** `world.researched` — one sorted store of
`{ player, carrier kind, slot, effect snapshot }` entries, one per completed upgrade
per player. The effect is COPIED at completion, so the ledger is self-contained: a
researcher may die later, and the upgrade never does (the death-semantics law — a
dead researcher loses its in-flight project only; `remove`/`remove_batch` drop the
block, never the ledger). Effects apply from completion onward by being READ —
combat and movement consult the ledger live; no entity is mutated at completion, no
spawn hook exists, and future entities of the targeted kinds benefit automatically.

### 6. Hashing and replay: mask slots 14/15, a trailing ledger section, tag 12

The canonical state hash gains capability-mask slots 14 (`Upgrades`, the static specs)
and 15 (`Research`, the in-flight block: slot + elapsed ticks) — the mask's last two
spare bits, encoded only when present. The researched ledger is a **trailing section
written only when the store is non-empty** — the additive-empty rule: a pre-B-034
state appends ZERO bytes to the v4 stream, so the demo and flagship goldens stay
bit-identical and `STATE_ENCODING_VERSION` stays 4 (the mask-slot discipline's
section-level twin; "empty" and "absent" encode identically because they MEAN
identically). The replay codec gains `TAG_RESEARCH = 12`; the encoder and decoder
matches stay exhaustive, so the compiler forces the pair. Recordings made before
B-034 contain no tag 12 and verify unmodified.

### 7. The outward surface: two events, two view lists

- `Event::ResearchStarted { researcher, upgrade }` and `Event::ResearchCompleted
  { researcher, upgrade }` — the cues. Both map to `None` in `cue_for` (the
  ADR-0202 AbilityCast ruling: no authored sound yet; the A14 feel pass decides
  whether research gets a voice at all).
- `PlayerView.research` (in-flight: `{ researcher, slot, progress_milli }`, own
  researchers ascending by id — the production queues' exact fog rule) and
  `PlayerView.researched` (completed: `{ kind, slot }` pairs, the ledger's own
  order) — the command card reads both.

### 8. The client surface: the card's next free row, direct submission

Upgrade buttons render in the command card's first free cell row after the ability
row (a researcher is typically also a producer — the Forge trains AND researches, so
its card shows the train row then this one). Three states per cell: researched (dim,
"[done]" — permanently), in flight (a darkened sweep that shrinks as the project
completes, plus the seconds left — the cooldown sweep's idiom), and actionable
(enabled = affordable and prerequisites met, the client's approximation of the gate's
checks; the gate stays the authority and refusals surface through the existing
rejection feedback — the three new reasons get lines). Clicking submits the Research
command directly — the Train rule (no ground point, no arming). A mixed selection
that fills the card omits the row (DEBT-208, the DEBT-206 rule).

### 9. The shipped content: two upgrades, both on the forge tier

- **Weapon Systems** — `AttackDamage` ×1.25 on `[rifleman, raider]`, 20 s, 250 ore,
  no prerequisites: the rifle line hits a quarter harder.
- **Field Engines** — `MoveSpeed` ×1.15 on `[worker, pathfinder]`, 15 s, 200 ore,
  requires Weapon Systems: the logistics line marches (and gathers) a seventh faster,
  chained on the first — the tree's one shipped edge exercises `RequirementsUnmet`.

The carrier is the **Forge** — forge-tier, like both B-033 carriers: the scripted AI
builds no Forge (pinned by `tests/ai.rs` since B-029), so no pinned run ever spawns
the carrier or issues a Research command, and the demo and flagship goldens are
bit-identical (the A-232 argument, executed again). The numbers are authored, not
playtested (A-241, A-230's standard). The Phalanx is deliberately untouched (A-230's
defer rides on).

## Evidence

- **The sequence's own ruling:** plan §19 names Update 7 (tech tree / upgrades) as
  the next step after Update 6, and B-033's report and the AI-Handoff both point
  here.
- **The seams exist and are proven:** B-033 exercised every one of them once already
  (capability store, injected runtime, mask slots, codec tag, card row, gate checks)
  — this ADR's diff reuses each pattern by name.
- **The golden-stability proof is executable:** after implementation, `headless
  --seed 7 --ticks 300` still prints `0xb6fff6659cfb7709` and the flagship run
  still prints `0x3a58e55ea6250906` — both measured live at the branch tip. The
  content hash and the basin bundle hash move in one dedicated commit with every
  pin (the B-033 procedure, STANDING_ORDERS §7): content
  `0xbe1a3d97ae6780e9` → `0x4d362e1a17b9eefc`, basin `0x3e3d96b500aa3ffb` →
  `0x1f14961cff5a4b0f`; the map id `0xd38136401ab02ff1` does not move.
- **The acceptance suite:** `tests/upgrades.rs` (thirteen laws: the gate matrix, the
  prerequisite edge, the exact spend/timing ticks, both effects through the real
  pipelines, parallel research and the wasted-purse refusal, the death semantics,
  determinism and hashing, the replay round-trip, the view's fog rules, the shipped
  parameters, and the end-to-end real-bundle path) plus four unit tests in
  `crates/sim/src/upgrades.rs`.

## Consequences

- **Cheaper:** the permanent axis of progression opens (Update 8's evaluative AI
  researches through the same door — one more command source); the client gains the
  three-state progress-cell idiom every future timed surface reuses; the content
  tree gains a capability any structure can carry; the authored numbers compose
  (a future upgrade targeting the same kind multiplies in ledger order).
- **More expensive:** every issuer shares three more reject reasons (all consumers
  of `RejectReason` grow arms); `PlayerView` grows two fields (every constructor
  site updates); the state hash has two more encodable blocks and one trailing
  section; the mask's spare bits are now EXHAUSTED — the next capability (slot 16)
  needs a wider mask word, which IS a value shift and a version bump (recorded here
  so the future ADR cites it).
- **Breaks and migrations:** the registered content hash and the basin bundle hash
  move once, in one dedicated commit, through every pin (ci.yml, release.yml, three
  test files, the tools crate, README, AI-Handoff §2) — the documented procedure,
  the B-033 exercise repeated. The demo and flagship goldens do NOT move (measured;
  the additive-empty argument above). Old replays verify unmodified; replays
  containing research commands require a B-034+ build.
- **Honest limits (recorded, not waved through):** no AI research in Update 7 (the
  scripted plan is untouched — its pins must not move; Update 8's territory); no
  research cancel verb (the cost is committed at enqueue — DEBT-209); the `MaxHp`
  effect kind is designed but unimplemented (two new seams: a completion-time
  entity mutation and a spawn hook — the shape is recorded here for the brief that
  wants it); the upgrade card row yields to a full card (DEBT-208); the audio cues
  ride A14 (the AbilityCast ruling). Each is a named, dated seam for a later brief —
  the vocabulary is closed on purpose.

## Alternatives considered

1. **Upgrades as production queue items** (a new `QueueItem` variant riding the
   existing queues). Rejected: the queue item's shape is `producible: KindId` — a
   unit kind — and completion spawns an entity; research completes into a ledger
   entry instead. Forcing both completions through one queue grows the production
   system's branches where Update 7 wants one new module with the same semantics
   (the stage-3 head), and the per-building single project slot is cleaner as an
   injected block than as a queue invariant.
2. **Per-entity researched state** (the completed list living on the researcher).
   Rejected: the researcher can die, and a completed upgrade must not — the ledger
   is the player's. Per-entity state also makes "already researched" a multi-entity
   walk and the hash encoding per-entity; the one sorted world-level ledger is
   simpler, player-scoped by construction, and additive-empty in the hash.
3. **A wider effect DSL** (generic stat overrides authored in content). Rejected on
  plan §10.1's law, the ADR-0202 ruling verbatim — content never carries logic; the
  closed vocabulary keeps every upgrade's cost model reviewable and the determinism
  story trivial.
4. **Do nothing** (upgrades wait for post-1.0). Rejected: the §19 sequence exists to
   audit the foundation by growing it; Update 8 (evaluative AI) presumes a tech tree
   to evaluate, and the target game's identity — the thing the owner wants beaten —
   is upgrades all the way down.

## Decision

<pending the owner's verdict — the implementation lands under the standing delegation
with this ADR in `proposed`, the ADR-0201 flow.>
