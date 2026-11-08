# ADR-0202: The ability system — active castable abilities as the first designed extension

- **Date:** 2026-11-09
- **Status:** proposed (the owner's standing delegation names this work — "§19 Update 6 (the
  ability system — needs an ADR)" — so the ADR and the implementation land together; the
  verdict field below stays open for the owner's review, exactly as ADR-0201's did)
- **Frozen decision affected:** **FD-3** (plan §2, plan §7.3 — the unified entity model):
  *extended* — the capability vocabulary gains **`Abilities`** (authored specs + per-slot
  cooldown runtime, exactly the `AttackDef` pattern: parameters plus this capability's own
  runtime on the block) and the sim-internal **`SpeedBoost`** runtime block (never authored
  in content — the ability system injects it, the `Construction` pattern). **FD-4** (plan §2,
  plan §10.1 — the data/code boundary): *extended, and the extension is the ADR's point* —
  ability definitions (which abilities a kind has, their numbers) are versioned data; the
  effect vocabulary (what an ability can DO) is a closed set in code; data selects and
  parameterizes, never scripts (plan §10.1's own clause: "if a rule needs logic, add a
  capability or a hook in code; data only selects and parameterizes"). **plan §8.1** (the
  command set): *extended* — one new variant, `CastAbility`, through the unchanged gate
  (plan §8.3: "a new command is a new variant plus payload validation plus a handler —
  never a change to the gate structure or application order"). **FD-5** (plan §5): binds
  everything — no floats, no unseeded randomness (ability effects are flat integer math;
  no variance rolls). **FD-8** (fog): the gate rejects a target point the issuer cannot
  see — fog filters the order, never the simulation. **FD-9** (events outward): the cast
  cue and the cooldown view are outward data (`Event::AbilityCast`, `AbilityView`), never
  a poll of sim internals. **FD-10** (plan §2 — content schema moves only forward): the
  new capability block is additive at a new tag; existing files parse identically. FD-1,
  FD-2, FD-6, FD-7 are unchanged and bind everything below.
- **Proposed by:** the Programmer (brief B-033; recording the owner's standing delegation —
  "lets keep working on upgrading this game… I want to make a game that can beat Generals
  Zero Hour", with Update 6 and the ADR named as the next step)

## Context

Plan §17 names the ability system as the foundation's designed extension test: "Abilities —
command machinery, validation gate, cooldown-ready pipeline." Plan §19 sequences it as
**Update 6, the first extension that is not data-only** — Updates 1–5 (B-029…B-032) proved
the content tree absorbs units, buildings, maps, a rebalance, and a whole second faction
with zero engine hacks; Update 6 asks the harder question: does the *command* architecture
absorb a new verb without bending the gate?

The pressure is real, not hypothetical. Three measured facts:

1. **The battery found lines the roster cannot cross.** A-227/A-230: the pure rifle line
   does NOT beat lancers, and the Phalanx has no equal-ore answer to the Lancer — flat-stat
   rosters converge on rock-paper-scissors; active abilities are the classic fourth
   dimension of counterplay (the whole of the target game's identity — Generals Zero Hour
   is unit actives and general's powers).
2. **The combat pipeline already reserves the insertion point.** Plan §8.3: "Every stage
   is a named function so future armor, projectiles, and abilities insert at a stage." The
   stage exists (`crates/sim/src/combat.rs`, stage 7); nothing occupies it.
3. **The foundation's own seams are unused until now**: the capability store pattern
   (parameters + runtime on one block, `AttackDef` at `crates/sim/src/world.rs:271`), the
   single validation gate every issuer shares (`crates/sim/src/command.rs:88`), the outward
   event stream, and the replay codec's exhaustive tags — all built for exactly this kind
   of growth, never yet exercised by a new verb.

What does not exist at the pinned head `dfdccb5`: no ability anywhere (verified by search —
no capability, command, event, or content field names one), and no timed-modifier state of
any kind (nothing in the sim temporarily changes a parameter; `MoveDef.speed_per_tick` is
immutable after spawn).

## Proposal

### 1. The capability: `Abilities` (authored specs + cooldown runtime)

One capability block carries every ability a kind has — multi-slot by construction:

```ron
// content/entities/pathfinder.ron (the shipped example, §9)
Abilities(( abilities: [
    ( id: "overland_sprint", display_name: "Overland Sprint",
      effect: SpeedBurst(( multiplier_milli: 1500, duration_ms: 4000, radius_milli_tiles: 2500 )),
      cast_range_milli_tiles: 4000, cooldown_ms: 20000, cost_ore: 0 ),
])),
```

Simulation-side (`world.rs`), the block is `AbilitiesDef { specs, cooldowns }` — the specs
are the load-converted parameters (range to fixed point, ms to ticks per plan §10.2, the
same once-at-load conversion every other duration takes); `cooldowns` is the per-slot
remaining-ticks runtime, the exact `AttackDef.cooldown_remaining` pattern. Slots are
addressed by authored index (`u8` in the command); ids are for humans and tooling.

### 2. The effect vocabulary: three kinds, closed in code

An ability's effect is one of — and for Update 6, exactly one of three code-defined kinds,
parameterized by data:

- **`DamageArea { damage, radius }`** — instant integer damage to every *enemy-player*
  entity within `radius` of the target point (neutral nodes are never collateral; there is
  no friendly fire in the vocabulary yet). Hits land through the combat system's damage
  path and surface as `AttackHit` events (the attacker is the caster), so the existing hit
  cues, radar tracking, and stage-8 death cleanup work unmodified.
- **`HealArea { amount, radius }`** — instant integer healing to every friendly entity
  with Health in radius, clamped at max. The health bars rise; the cast flash is the cue
  (no per-target event in Update 6 — recorded as a consequence).
- **`SpeedBurst { multiplier_milli, duration_ms, radius }`** — a timed movement modifier
  applied to every friendly entity with Move in radius: the injected `SpeedBoost` runtime
  block (multiplier as fixed point, remaining ticks), read by the movement system as
  `effective_speed = speed_per_tick × multiplier`. One boost per entity — a later cast
  overwrites (deterministic, newest wins). The block is removed at expiry.

Why a closed vocabulary and not a data DSL: plan §10.1's own law. Effects are behavior;
behavior is code. New effect kinds are new variants plus new stage hooks — the same
review-visible growth every capability has taken since M1.

### 3. The command: `CastAbility` (one caster, one ground point)

```rust
CommandKind::CastAbility { caster: EntityId, ability: u8, target: Vec2Fx }
```

Ground-point targeting covers all three shipped effect kinds (area damage at a point, area
heal at a point, area speed at a point) and matches the client's existing armed-click
family (A/G/placement). Entity-targeted abilities (a stun, a snare, a single-target nuke
that must track) are deliberately out of Update 6 — the payload grows a target-id form when
an ability needs it (§Consequences). One caster per command: multi-caster casts are N
commands (the Move/Attack list form buys nothing here — each caster has its own cooldown).

### 4. The validation gate: seven checks, three new reasons

The gate is the sole authority (plan §8.2), shared by every issuer (FD-7 — the AI and the
human cast through the identical door). In order:

1. caster exists and is owned by the issuer (`UnknownEntity` / `NotOwnedByIssuer`);
2. caster carries `Abilities` (`MissingCapability`) and is `Active` (not a site);
3. the slot index is inside the kind's list (**new reason `AbilitySlotInvalid`**);
4. the slot's cooldown is zero (**new reason `AbilityOnCooldown`**);
5. the issuer can pay `cost_ore` (`CannotAfford`, reused);
6. the caster-to-point distance is within the slot's cast range, squared compare only
   (**new reason `AbilityOutOfRange`**) — the caster casts from where it stands; there is
   no walk-to-range in Update 6 (the order form, §Alternatives);
7. the target point is visible to the issuer (reused `NotVisible`) — a *point* visibility
   law: inside any friendly Vision radius, the ground analog of the entity law at
   `command.rs`'s `visible_to`. This is the spotting seam: a lone Ballista cannot barrage
   beyond its own sight, but a Pathfinder's 12-tile eyes lift the fog for it — the exact
   synergy the Pathfinder's authoring note already names ("spots for the Ballista").

### 5. The cooldown-ready pipeline and the timed modifier (stage 7)

`abilities::advance_abilities` runs at the head of stage 7 (combat), before
`advance_combat`: every slot's remaining cooldown ticks down (saturating at zero), every
`SpeedBoost` block's remaining ticks tick down and expired blocks are removed (the store's
ascending-id order is preserved by `Vec::remove`, the documented store law). The semantics
mirror the attack cooldown exactly: a cast at tick T sets `cooldown_remaining =
cooldown_ticks`; the same tick's stage-7 pass decrements it; the slot is ready again at
T + cooldown_ticks. A boost is live for movement from the cast tick (stage 1 sets it;
stage 6 of the same tick reads it) through its final tick; expiry at stage 7 means the
boost's last movement tick is the one where it hits zero. All integer, all deterministic.

### 6. Hashing and replay: additive slots, no version bump

The canonical state hash gains capability-mask slots 12 (`Abilities`) and 13
(`SpeedBoost`), encoded only when present — the mask was built with four spare bits for
exactly this. **`STATE_ENCODING_VERSION` stays 4**: the encoded bytes of every
pre-B-033 state are bit-identical (no existing entity carries either block; the mask reads
the same; no field order moves), and the proof is executable — the demo golden and the
flagship are pinned and must not move (§Evidence). The version word exists to catch silent
*value* shifts, not additive growth at unused slots; the B-029 precedent is the same shape
(new kinds, unchanged encodings, goldens proven stable). The replay codec gains
`TAG_CAST_ABILITY = 11`; the encoder and decoder matches are exhaustive, so the compiler
forces the pair. Recordings made before B-033 contain no tag 11 and verify unmodified.

### 7. The outward surface: one event, one view

- `Event::AbilityCast { caster, ability, at }` — the cue (where, by whom, which slot);
  presentation draws its own flash. Ability *damage* rides the existing `AttackHit`
  (attacker = caster): an ability hit is outwardly an attack-hit-shaped fact, and every
  consumer (hit cue, attack radar) works unmodified.
- `AbilityView { caster, slot, cooldown_milli }` in `PlayerView.abilities` — own casters
  only, ascending by id, the `production` queues' exact rule (fog hides enemy cooldowns as
  it hides enemy queues). `cooldown_milli` is thousandths of the authored cooldown
  remaining (0 = ready) — the same integer convention as `hp_fraction_milli`.

### 8. The client surface: the card's first free row, armed targeting

Ability buttons render in the command card's first free cell row (row 0 when no
producer/worker shares the selection — the common caster case; the row after Build's
otherwise; omitted when the card is full, a recorded presentation debt for A14 to rule
on). Enabled = ready and affordable; the cooldown is a darkened sweep over the cell plus
the seconds remaining, the health-bar fill idiom inverted. Clicking arms the cast
(`cast_armed: Option<(EntityId, u8)>` — the A/G arm/cancel family: crosshair cursor, HUD
instruction line, left-click fires at the ground point, right-click and Escape cancel).
The client-side enablement is an approximation; the gate is the authority and refusals
surface through the existing refusal feedback (the three new reasons get lines).

### 9. The shipped content: two abilities, both on the forge tier

- **Pathfinder — Overland Sprint** (`SpeedBurst` 1.5× for 4 s in a 2.5-tile radius,
  20 s cooldown): the scout's dive-and-escape tool; the fastest thing on the field gets
  faster on demand.
- **Ballista — Shatter Volley** (`DamageArea` 45 in a 1.8-tile radius, cast range 13
  tiles, 25 s cooldown): the artillery barrage beyond its own 12-tile sight — spotted by
  the Pathfinder, the pairing the content's own notes promise.

Both carriers are **forge-tier**: the Alpha scripted AI builds no Forge (its plan takes
the barracks — pinned by `tests/ai.rs` since B-029) and the plain demo run spawns only
command centers and workers, so no pinned AI match, mirror budget, or demo golden can
shift — the design's constraint, not an accident. The numbers are authored, not playtested
(A-230's own standard); whether either ability FEELS right is the A14 playtest's call.
The Phalanx is deliberately untouched (A-230: its feel rides A14 wholesale).

## Evidence

- **The sequence's own ruling:** plan §19 Update 6 is the named next step after B-032
  (the B-032 report's Summary and the owner's delegation both point here).
- **The seams exist and are unexercised:** the stage-7 insertion point (plan §8.3's own
  reservation), the `AttackDef` cooldown pattern (`world.rs:271–345`), the gate's shared
  checker structure (`command.rs:88–237`), the mask's spare bits (`hash.rs:210–213`), the
  codec's exhaustive tags (`replay/src/lib.rs:316–327`).
- **The counterplay pressure is measured, not asserted:** A-227 (the line does not beat
  lancers), A-230 (the Phalanx has no equal-ore lancer answer; both logged as authored
  asymmetries whose resolution is either new numbers or new verbs — this ADR is the verb).
- **The golden-stability proof is executable:** after implementation, `headless --seed 7
  --ticks 300` must still print `0xb6fff6659cfb7709` and the flagship run
  `--ticks 7200 --p1 ai --p2 ai` must still print `0x3a58e55ea6250906` — the B-033 test
  suite pins both. The content hash and the basin bundle hash move in one dedicated
  commit with every pin (the B-029/B-032 procedure, STANDING_ORDERS §7).

## Consequences

- **Cheaper:** the fourth dimension of counterplay opens (Update 7's tech tree and
  Update 8's evaluative AI both consume it — an AI that casts is one more command source
  through the same door); the client gains the armed-targeting + cooldown-sweep idioms
  every future ability reuses; the content tree gains a capability every future kind can
  carry.
- **More expensive:** every issuer now shares three more reject reasons (all consumers of
  `RejectReason` grow arms); the state hash has two more encodable blocks; `PlayerView`
  grows a field (every constructor site updates — fixtures and tests).
- **Breaks and migrations:** the registered content hash `0x8edfdfc3a814eb37` and the
  basin bundle hash `0xb5c143a09f45a402` move once, in one dedicated commit, through
  every pin (ci.yml, release.yml, three test files, the tools crate, README, AI-Handoff)
  — the documented procedure. The demo and flagship goldens do NOT move (pinned; the
  additive-encoding argument above). Old replays verify unmodified; replays containing
  casts require a B-033+ build.
- **Honest limits (recorded, not waved through):** no AI casting in Update 6 (the
  scripted plan is untouched — its pins must not move); no entity-targeted abilities; no
  friendly fire, no per-target heal events, no buff stacking, no cast-while-moving
  mechanics; the card's ability row yields to a full card (mixed selections containing a
  caster plus a producer and a worker omit the row). Each is a named, dated seam for a
  later brief — the vocabulary is closed on purpose.

## Alternatives considered

1. **Abilities as orders** (walk into range, wind up, then resolve — the attack-order
   form). Rejected for Update 6: it couples the new verb to the order-queue and pathing
   machinery before the simple form is proven; instant-cast is the exact analog of the
   Alpha's instant-hit combat stance (DEBT-logged as such, like instant-hit itself), and
   the order form composes with a future wind-up stage (plan §8.3's named stages) without
   rework.
2. **A data-driven effect DSL** (scripts in content). Rejected on plan §10.1's own law —
   content never carries logic. The closed vocabulary keeps every ability's cost model
   reviewable and the determinism story trivial (flat integers, no interpreter).
3. **Do nothing** (abilities wait for post-1.0). Rejected: the §19 sequence exists to
   audit the foundation by growing it; the battery's own measured asymmetries (A-227,
   A-230) show flat-stat rosters converging, and both of Update 6's consumers (the tech
   tree, the evaluative AI) sit immediately behind it in the sequence.

## Decision

<pending the owner's verdict — the implementation lands under the standing delegation
with this ADR in `proposed`, the ADR-0201 flow.>
