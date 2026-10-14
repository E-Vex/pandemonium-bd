# ADR-0201: Missions, campaigns and cutscenes — the objective model, mission data, and the single-player campaign

- **Date:** 2026-10-14
- **Status:** proposed
- **Frozen decision affected:** **FD-1** (plan §2, plan §6.5 — what a match is and
  what its identity covers): *clarified* — the match-defining data gains the
  **mission** dimension (map + setup + restrictions + objectives + AI settings +
  optional story hooks), and the mission carries its own domain-separated
  identity beside the content hash and map id. **FD-4** (plan §2 — the data/code
  boundary): *extended* — the "versioned data files" side gains the mission
  class (objectives, restrictions and AI settings are data that select and
  parameterize code; their evaluation stays code), the same extension pattern
  ADR-0200 applied for the content classes. **FD-10** (plan §2 — content schema
  moves only forward): *extended* — the mission schema is versioned and
  forward-only like every other content kind. **FD-2, FD-3, FD-5, FD-6, FD-7,
  FD-8, FD-9 are unchanged** and bind everything below: no mutation outside
  `Sim::step`, no floats below the boundary, the sim depends on nothing above
  it, the AI sees only its `PlayerView`, fog never alters the simulation, and
  events flow outward. **plan §17** lists "Campaigns, cosmetics, ranked, mods,
  live service — only after the core proves it needs them" (`plan.md:606`); the
  standing orders' PRODUCT PHASE amendment (`docs/pm/STANDING_ORDERS.md:8`)
  suspends the anti-goal side of that line for product briefs while the
  architecture law still binds — this ADR is the design record that opens the
  campaigns chapter under those rules.
- **Proposed by:** Agent B (brief B-010; recording the owner's rejected-tutorial
  / learn-by-exploration direction — staged story missions, cinematic
  cutscenes, objective-based challenge missions — relayed by the PM)

## Context

The owner has ruled out a tutorial mode. Players are to learn the game through
a **single-player story campaign**: staged missions with a narrative and
cinematic cutscenes, **fully skippable** — an experienced player goes straight
to the advanced modes (vs AI, friends, online), which all stay. Alongside the
campaign the owner wants **objective-based challenge missions against the AI**
instead of only "destroy the opponent". The campaign is a menu peer, never a
gate.

The PM supplied a framework ruling to *evaluate, not rubber-stamp*: a mission =
map + setup + restrictions + objectives + AI settings + optional story hooks; a
campaign = ordered missions + progress + story; a challenge = a mission outside
a campaign; a normal match = a mission with the default objective. §1–§4 of the
Proposal evaluate that ruling against the code as it stands at the pinned head
`d989336e4ec1152c1c7333b91c9e57c71fd9c300` and finds it sound with two named
refinements (where "setup" and "restrictions" land, §3) and one honest gap it
forces into the open (the rules' victory parameters are identity data today but
the evaluator ignores them, §1).

**ADR-0200 is accepted and binds this ADR**: the content classes (base content
/ gameplay mods / presentation packs, §3 there), the manifest contract's `kind`
taxonomy (§7), "maps are data, never code" (§5), and scripting only ever as a
separate, controlled feature (§8's trigger row). The owner's nine architecture
principles recorded in ADR-0200's Context apply: the brain/body split, versioned
assets/maps/mods/manifests, community content loading without executable
changes, cosmetic data outside the authoritative hash, mods under session
compatibility rules, packs independently manageable, cosmetics never changing
simulation results, and deterministic generated worlds. The hard constraints:
FD-1..FD-10 (`plan.md:62-71`), integer-only determinism (FD-5, `plan.md:140-153`),
and **goldens never move without bundled, explicit owner authorization**
(STANDING_ORDERS §7) — every design choice below is shaped by that last rule.

Nothing mission-shaped exists today: no mission, campaign, cutscene or
tutorial surface anywhere in the crates (verified by search at the pinned
head), no netcode (the workspace is nine crates + tests, `Cargo.toml:3`; the
controller vocabulary is exactly `Human | Ai`, `crates/sim_api/src/lib.rs:76-81`),
and no presentation-data channel beyond the map's display-only heightmap.

## Proposal

### 1. Current state — every claim cited at the pinned head `d989336`

**How a match is set up.** A match is `MatchSetup` — one seed plus the player
slots (`crates/sim_api/src/lib.rs:92-101`) — plus the content. The windowed
client loads the whole content tree at startup (`crates/client/src/main.rs:131-145`),
the New Match screen edits mode/seed/map (`crates/client/src/screens.rs:104-137`),
and Start resolves to `begin_match` (`crates/client/src/app/mod.rs:379-438`):
it bundles the chosen map (`tree.bundle(map_id)`, the `ContentBundle` builder at
`crates/content/src/bundle.rs:133-152`), derives the mode's setup
(`MatchMode::match_setup`, `screens.rs:56-81` — both slots are always present;
"no opponent" is a controller-less P2, never a missing player), and builds a
`MatchHost` with one scripted Alpha controller per `ai_slots()` — the opponent
in Player vs AI, both players in spectate, none in Sandbox
(`app/mod.rs:354-372`, `screens.rs:83-92`). The host constructs the `Sim` from
the world definition (`MatchHost::with_controllers`, `crates/engine/src/host.rs:101-130`;
`Sim::new`, `crates/sim/src/sim.rs:71-134` — players from the setup, initial
spawns in the documented order, initial fog pass, population recompute). The
headless path is the same seam: `tools headless` → `run_ai_match` →
`AiMatchHost::new` with `alpha_controller` per driven slot
(`crates/tools/src/ai_match.rs:75-122`, `crates/engine/src/ai_host.rs:45-147`).
The ruleset data the match runs under is `content/rules/alpha.ron`: one resource
(Ore, starting 200, line 8), base population cap 0 (line 10), and
`victory: ( kind: "elimination", allow_resignation: true )` (line 11).

**How win/loss is decided.** Stage 10 of the fixed eleven-stage tick pipeline
(`crates/sim_api/src/lib.rs:504-528`; the call at `crates/sim/src/sim.rs:515`)
runs `match_rules::evaluate` (`crates/sim/src/match_rules.rs:64-120`): a player
is defeated when they resigned (the `Resign` command sets the flag,
`crates/sim/src/command.rs:359-363`) or own zero structures — Footprint-carrying
entities, the data-defined "structure" marker (`match_rules.rs:84-96`); the
defeat check only fires in a "structure-bearing" match, the A-067 guard
(`match_rules.rs:73-83`); the match ends when at least one player is defeated
and at most one non-defeated player remains — the survivor wins, mutual
destruction is `PlayerId::NEUTRAL` (`match_rules.rs:97-119`). Exactly one
`Event::MatchEnded` fires (`match_rules.rs:118`; the vocabulary at
`crates/sim_api/src/lib.rs:487-491`); the outcome is cached (`Sim::outcome`,
`sim.rs:148-155`) and the host surfaces it (`host.rs:217-224`), where the
client promotes InMatch to the end screen (`app/mod.rs:977-990`) and the
headless tool prints it (`crates/tools/src/main.rs:184-194`).
**The honest gap:** the rules' victory parameters are validated at load
("known kinds: 'elimination'", `crates/content/src/loader.rs:177-185`) and
hashed into the content identity (`bundle.rs:631-634`), but the evaluator never
reads them — elimination semantics are hard-coded in `match_rules.rs`, and
`Resign` is applied unconditionally of `allow_resignation`
(`command.rs:359-363`). The data side of plan §10.1's "match conditions
parameters = Data; victory evaluation is code" (`plan.md:367`) exists for
identity only. The mission objective model (§2) is what makes stage 10 actually
data-driven for the first time.

**The events the client already consumes.** The outward vocabulary is the
`Event` enum (`crates/sim_api/src/lib.rs:405-492`): `Spawned`, `Died`,
`AttackHit`, `ProductionStarted`, `ProductionCompleted`, `ConstructionStarted`,
`ConstructionCompleted`, `ResourceDelivered`, `NodeDepleted`,
`CommandRejected`, `MoveFailed`, `MatchEnded`. Events are outputs and never
part of the hashed state (`lib.rs:397-401`). The emitters exist across the
pipeline (`Spawned` at `sim.rs:558`; `Died` at `sim.rs:653`; `AttackHit` at
`crates/sim/src/combat.rs:300`; `ResourceDelivered` at
`crates/sim/src/economy.rs:156`; `NodeDepleted` at `economy.rs:250`;
`ProductionCompleted` at `crates/sim/src/production.rs:145`;
`ConstructionCompleted` at `production.rs:184`). The client consumes them after
each step, never inside it (FD-9): the audio sink and the feedback state eat
the event list (`app/mod.rs:1011-1016`), the refusal cue matches
`CommandRejected` for the human's own last order (`app/mod.rs:1025-1039`), and
death fades match `Died` against the last-seen map (`app/mod.rs:1040-1047`).
The HUD reads the fog-filtered `PlayerView` through `hud_state`
(`host.rs:248-262`); the command card reads the view plus loaded content
(`crates/client/src/ui.rs:145-162`).

**The AI entry points.** The `Controller` trait is one method — `think(&mut
PlayerView, Tick, &mut Vec<Command>)` (`crates/ai/src/lib.rs:55-58`) — invoked
once per tick boundary on the controller's fog-filtered view, in ascending slot
order, feeding the same step through the same validation gate
(`host.rs:173-200`; `ai_host.rs:82-103`). The scripted Alpha opponent is
`ScriptedController` (`crates/ai/src/scripted.rs:219-243`), constructed from an
`AiPlan` (`scripted.rs:127-179`) that the engine derives from the loaded
content by capability shape — never name-matched kinds (`alpha_plan`,
`ai_host.rs:170-339`; the roster from the faction's production lists at
`ai_host.rs:191-216`). Its behavior is a scripted build order with attack waves
on timers and army-size thresholds: the pressure cadence is
`WAVE_TIMER_TICKS = 2400` — "80 s at 30 Hz" (`scripted.rs:82-86`) — and the
wave trigger is `size >= MIN_WAVE (4) && (size >= WAVE_SIZE (10) || tick >=
next_wave_timer)` (`scripted.rs:78-81`, the trigger at `scripted.rs:537`); a
live wave re-marches on the same cadence (`scripted.rs:561-610`), defense pulls
the army onto intruders inside a base radius on a cooldown
(`scripted.rs:503-527`, `scripted.rs:107-115`). The controller is a pure
function of (view, tick, its own state) with a seeded RNG derived from the
match seed (`scripted.rs:31-37`). The tuning knobs are consts in code —
"The script is code, not data (plan §10.1: content parameterizes systems; it
does not script AI)" (`scripted.rs:50-52`) — the fact §4 builds on.

**Authoritative vs presentation today.** Authoritative: the ruleset, entity
kinds, factions (roster, production, starting forces), and the map's
simulation fields — validated, hashed into the content identity
(`bundle.rs:255-263`; the encoders `bundle.rs:621-635`, `:637-654`, `:726-750`,
`:752-794`), and mapped through `world()` into the simulation's vocabulary
(`bundle.rs:333-473`). Presentation: the map's display-only heightmap (hashed
today as ADR-0200 §2's recorded interim, exiting at R2), terrain class colors
computed in the mesh builder (`crates/engine/src/mesh.rs:56`), instanced-cube
placeholder entities (`docs/ARCHITECTURE.md:338`), synthesized audio behind the
`AudioSink` seam (ADR-0002), and the rect-and-text UI with its embedded font
(`crates/client/src/text.rs:8-17`). Derived and deliberately unhashed: fog
(A-059, `sim.rs:49-54`) and the match outcome (A-066, `sim.rs:55-61`). The
client renders and hit-tests through per-player fog-filtered views
(`app/mod.rs:178-182`, `:1052-1080`).

### 2. The objective model

**Which objective kinds the current events and state can already express.**
All six the brief names, plus the existing elimination rule, with zero new
simulation *systems* — what is missing is an evaluation home and an outward
progress vocabulary, not expressiveness:

| Objective kind | Already expressible from | New sim support needed |
|---|---|---|
| Destroy (a target set / all enemy structures) | `Died` per entity (`lib.rs:419-423`) + the entity set; the elimination rule itself (`match_rules.rs:84-119`) | none |
| Survive N ticks | the tick every view and step carries (`lib.rs:50-52`, 30 ticks/s) | none |
| Reach an area | own entities' positions in `PlayerView.entities` (`lib.rs:116-133`, `:213`) vs an integer area | an area declared in mission data (rect in tiles / radius in milli-tiles) |
| Protect an entity | `Died` for the protected id; view membership | none |
| Gather N ore | the ledger (`PlayerView.resources`, `lib.rs:207-208`; hashed per player, `crates/sim/src/hash.rs:157-168`) or cumulative `ResourceDelivered` (`economy.rs:156`) | semantics choice: cumulative-delivered vs hold-balance (mission data picks) |
| Build / train N | `ConstructionCompleted` (`production.rs:184`), `ProductionCompleted` (`production.rs:145`), kinds resolved through content | none |

**Where objectives are evaluated — three options, each scored on the brief's
four axes:**

**(a) In the sim — stage 10 generalized over a mission objective set.** The
mission's objectives arrive through the content seam as match-condition
parameters; stage 10 evaluates them as *derived* state exactly as it evaluates
elimination today (the A-066 pattern — a pure function of hashed inputs,
evaluated per tick, cached once resolved). *Determinism:* full — evaluation is
part of the tick, identical on every peer and in every re-simulation.
*Replay-log impact:* none — the command stream, checkpoints and the replay
format are untouched (`ReplayFile` carries commands and checkpoints, not
events — `crates/replay/src/lib.rs:49-69`); new outward progress events do not
enter the codec at all. *State-hash impact:* none — objective data rides the
world definition the way production lists do (`bundle.rs:402-421`), and
`hash_state` encodes world state, never the fixture (`hash.rs:33-171`).
*Golden impact:* none, provably — see §7. *FD fit:* plan §10.1's own boundary
row ("Match conditions parameters | Data | victory evaluation is code",
`plan.md:367`) — data selects and parameterizes code evaluators, one evaluator
per objective kind, never logic in the file (`plan.md:371`).

**(b) In the engine layer — the host evaluates from `FrameOutcome`.** The host
already sees every event and view (`host.rs:150-160`, `:173-200`).
*Determinism:* the code is deterministic, but the evaluation sits outside the
replay's re-simulation — `run_command_log` (`crates/sim/src/runner.rs:43-81`)
drives the `Sim` only, so a replay verifier would have to re-implement
objective evaluation in `tools` to check a recorded outcome: the outcome is not
a property of the record. *Replay-log impact:* the outcome is absent from the
record's semantics. *State-hash/golden impact:* none. *FD fit:* forks the
authority — stage 10 still decides elimination while the host decides mission
objectives, two sources of truth for "the match ended"; and it places
authoritative match logic in `crates/engine`, Lane A's crate
(STANDING_ORDERS §3), for the core of a Lane B feature.

**(c) In the client — presentation-only evaluation.** *Determinism:* the client
is not part of any determinism contract; a mission's result would be
client-authoritative, unverifiable by replay, and forgeable by a modified
client. *Replay-log impact:* same absence as (b). *State-hash/golden impact:*
none. *FD fit:* strains FD-1's letter (a match is seed + content + ordered
command log — its *resolution* should be derivable from that record) and FD-9's
spirit (the client would decide a match-level fact, not merely present one).

**Recommendation: (a) — sim-side evaluation in stage 10, objectives as
match-condition data.** It is the only placement where a mission's outcome is
provably a property of the deterministic record, it is the placement plan §10.1
already sketches, it keeps the authoritative half in Lane B's crates, and it
closes §1's honest gap by making stage 10 read match-condition data for the
first time. The PM ruling's "normal match = a mission with the default
objective" then falls out exactly: the default objective set *is* today's
elimination rule, so a mission-less match is the default mission by
construction — same code path, same bytes, same goldens.

**Semantics.** A mission carries an ordered list of objectives, AND-composed;
each is `{ kind, parameters…, optional: bool }` from a fixed kind vocabulary
(the table above). Mission success resolves exactly like elimination does
today: the mission player's victory condition is met, `MatchEnded` fires with
the winning slot, and failure paths (a protected entity dies, a survive-timer
expires with the defeat condition live) resolve as the mission player defeated
— the existing `MatchEnded { winner }` shape suffices (`lib.rs:487-491`), no
vocabulary break. Progress surfaces through new outward `Event` variants (an
objective-state family — completed / failed), which are additive by the
pipeline's design (events are outputs, unhashed, and absent from the replay
codec) and consumed like today's events, after the step (FD-9). Optional
objectives never gate `MatchEnded`; they feed the end screen and story hooks
(§5).

### 3. Mission restrictions as authoritative mission data

The PM ruling's "restrictions" land on content references, not on a new
restriction engine — the composition with ADR-0200 is that a mission is a
*closed reference set* into the authoritative content tree, validated at load
the way ore-node kinds are validated today (`bundle.rs:100-106`):

- **Buildable / trainable kinds** — the mission references faction and rules
  *variants*: a faction file whose `roster` and `production` are subsets
  (the fields at `content/factions/legion.ron:8-21`). Trainability is already
  gate-enforced from exactly that data — the command gate rejects a `Train`
  whose kind is not in the producer's production list
  (`command.rs:188-205`, the `producible_by` check at `:199-203`), and the
  command card derives its train buttons from the same list
  (`ui.rs:333-352`). Restricting the list restricts card, gate, and the AI's
  roster derivation (`ai_host.rs:191-216`) in one move. **Flagged asymmetry:**
  the `Build` gate checks kind-exists, is-a-structure, economy and placement —
  but not roster membership (`command.rs:170-186`); the build buttons are
  roster-derived in the UI only (`ui.rs:411-421`). A restricted roster *hides*
  the button but does not *forbid* the command. When restrictions land, the
  Build validation arm should gain the same membership check (one
  review-visible gate extension per plan §8.3's rule) so restrictions are
  authoritative rather than hidden — a small, named sim change for the
  restrictions brief, not this one.
- **Starting units and resources** — already data: a faction's
  `starting_forces` (placed relative to map start anchors, hashed at
  `bundle.rs:743-748`, spawned deterministically at `bundle.rs:423-443`) and
  the rules' `resources[].starting` balances (hashed at `bundle.rs:624-629`,
  seeded into every ledger at `sim.rs:108-118`). A mission that wants
  different start conditions references a variant faction/rules file — no new
  surface, the identity follows the referenced bytes automatically.
- **Fog** — a mission field (`default | explored | visible`) selecting the
  *initial* fog state. This is the one restriction needing new sim support: an
  initial-seeding path beside today's all-Hidden init + first vision pass
  (`sim.rs:93`, `:129-132`). It stays inside FD-8's letter — fog filters
  information and never alters the simulation; the preset changes the filter's
  starting width, deterministically, for *every* player's view alike (human and
  AI parity preserved, FD-7), and fog remains derived and unhashed (A-059), so
  no checkpoint can move. The default preset reproduces today's init exactly.

**Where the mission file lives:** `content/missions/<id>.ron` — a new category
in the content tree under the same discovery rules (sorted `.ron` scan,
subdirectories refused loudly — `crates/content/src/loader.rs:63-75`,
`:107-149`), strict `deny_unknown_fields` parse, `schema_version` with the
forward-only gate and one-step migrations (`crates/content/src/version.rs:31-40`,
`:101-121` — the map v1→v2 chain is the pattern). The mission declares: `id`,
`display_name`, `schema_version`, the map reference, the faction/rules variant
references, the fog preset, the objective list (§2), the AI settings (§4), and
the optional story hooks (§5). Add-a-mission is then add-a-map's exact promise
(FD-4, `plan.md:65`, the A4 test's discipline at `plan.md:431-433`): a new
`.ron` in `content/missions/` appears in the mission list with zero engine
changes — the mission picker mirrors the map picker's tree-order list
(`app/mod.rs:960-968`).

### 4. AI settings per mission

Mission AI settings parameterize the *existing* scripted machines — the wave
trigger, the pressure cadence, the army targets, the defense posture
(`scripted.rs:69-125`) — as optional overrides carried on the `AiPlan`
(`scripted.rs:127-179`) and derived by the host from the mission data beside
today's content-derived facts (`ai_host.rs:170-339`). The mission may also
declare a **scripted wave schedule**: a fixed-vocabulary list of
`(first_attack_delay, wave_timer_ticks, wave_size, min_wave, army_cap,
worker_target)`-shaped entries driving the existing wave state machine — data
selecting and parameterizing code, never logic in the file (`plan.md:371`;
`scripted.rs:50-52`'s own line). Attack timing, aggression and scripted waves
are all expressible in that vocabulary; a scripting language is ruled out
(§10, and ADR-0200 §8's trigger row).

**Defaults must reproduce today's behavior bit-for-bit — proven by
construction, with the proof obligations named:**

1. The default mission (and every mission-less match) constructs the controller
   through the unchanged path — `tools headless` → `run_ai_match` →
   `alpha_controller` → `alpha_plan` with no mission input
   (`ai_match.rs:115`, `ai_host.rs:153-175`) — reading the same consts with
   the same values, so the emitted command stream is byte-identical and the
   demo and flagship goldens cannot move.
2. The override fields are `Option`-shaped with `None` meaning "today's const";
   the plan-resolution code must read the consts exactly as before when no
   override is present. A refactor that moves the consts into plan data must
   keep the default values byte-equal and the RNG consumption order unchanged
   (the controller's determinism contract, `scripted.rs:31-37`) — flagged as a
   review obligation on the implementing brief.
3. The mission file category adds zero bytes to every existing canonical
   encoding (§7) — the existing bundles' content hash is computed by the
   unchanged byte path.
4. The standing warning applies to any *default* touch: "The wave-mass numbers
   (WAVE_SIZE 10, ARMY_CAP 16) are load-bearing: re-run the 32-seed resolution
   sweep after any content timing/HP change" (`AI-Handoff.md:148`) — plus the
   determinism and AI test walls and the cross-OS golden gate
   (`.github/workflows/ci.yml:204-243`).

**Prove or flag:** the above is the proof *plan*; the live proof belongs to the
implementing brief (this ADR moves no code). Flagged: per-mission AI settings
are single-player mission data only — they are not a difficulty knob for
multiplayer sessions, where every peer must agree on the mission/AI record
(ADR-0200 §3's session rules; the handshake comparison set gains the mission
id).

### 5. Cutscenes and narrative

**In-engine, presentation-only, skippable at any moment, never touching sim
state or hash.** A cutscene runs while the host is paused — pause freezes the
clock and discards elapsed time (`host.rs:150-153`, `set_paused` at
`host.rs:202-210`), so no tick passes, no command applies, and the pause flag
itself is host state, not sim state (`host.rs:66-83`). Skipping is an input
event that ends the scene and unpauses; it sits in the existing Esc-ladder
discipline. Commands submitted after resume are stamped with the sim's current
tick at submit (`host.rs:137-142`), so the command log's semantics are
untouched by the interruption — the state hash and every golden are provably
out of reach. The cutscene layer is client code above the seam: it never
submits, never reads sim internals, and consumes only views, events and
content-adjacent presentation data (FD-9; the same discipline as the audio and
feedback layers, `app/mod.rs:1011-1050`).

**Scene model.** Two shapes, one near-term: (i) **live-state scenes** — pause
the match, move the camera over the frozen live world, show captions,
highlight entities (the presentation stack already interpolates and draws the
world through the fog-filtered view, `app/mod.rs:1052-1080`); zero new sim
surface, zero determinism surface. (ii) **staged scenes** — a separately
authored tableau (its own throwaway `Sim` instance or a static scene). In-match
campaign moments use (i); menu/intro cinematic moments are (ii)-shaped but
deferred until the pack layer exists, because staging them well is an art
question, not an architecture one. Recommendation: (i) now, (ii) after the pack
layer, explicitly not designed further here.

**Where cutscene/story files live, and which ADR-0200 class they are.** They
are **presentation data** — never validated by content rules, never hashed
into any identity, never exchanged between peers (ADR-0200 §1's definition and
§3's class rules). Near-term they live in a client-owned presentation
directory (the exact path is Lane A's call — `crates/client`'s existing
`assets/` tree is the precedent, `text.rs:32`); the authoritative content tree
stays authoritative-only, so nothing presentation ever rides `content/`'s
scan. As distributable items they are the `resource-pack` class of ADR-0200
§7's manifest taxonomy once the pack channel exists. The mission's **story
hooks** — cutscene ids keyed to trigger points (mission start, objective
completed, mission end) — are references *from* authoritative mission data
*into* that presentation namespace, treated exactly like `display_name`
strings in ADR-0200 §1's table: authoritative for identity (a mission with
different hooks is a different mission; the ids hash with the mission record),
presentation in consumption (the cutscene content itself is local, swappable,
and invisible to the protocol). A cutscene re-edit therefore changes no
authoritative byte — the cosmetic-differences rule holds.

**Text needs.** The embedded font is an ASCII subset — printable ASCII only
(`text.rs:8-17`; `AI-Handoff.md:165`) — so near-term captions and dialogue are
ASCII, full stop. Extended character sets are a presentation-asset decision
(a wider font is an asset with a licence), which waits on the pack-layer ADR
(0202, Lane A — reserved) and the owner's assets licence decision. **What
waits on those two:** real scene art (models, textures, animations), voice,
and non-ASCII fonts. Until then: the current block art (instanced-cube
entities, `docs/ARCHITECTURE.md:338`; terrain class colors, `mesh.rs:56`),
ASCII captions through the existing text pipeline, and the existing
synthesized cue bank (ADR-0002) for cutscene audio.

### 6. Campaign progression and persistence

**Campaign = ordered missions + progress + story, as data.** A campaign file
lists mission ids in play order (a content reference set, validated like the
mission's own references). Progress is **per-user state, not authoritative
content**: it lives in the per-user config folder beside `settings.cfg`, on
the same per-OS resolution (`config.rs:234-288`) and the same best-effort
write discipline (`config.rs:293-313`) — never hashed, never synced, never
exchangeable between peers.

**Unlock rules.** Sequential by default: mission N+1 unlocks when N is
completed or skipped; challenge missions unlock per their campaign entry's
completion of the mission they extend. Nothing else locks — the advanced modes
stay directly reachable from the main menu exactly as they are today
(`screens.rs:25-35`), because the campaign is a menu peer, never a gate. That
is the owner's skippability requirement made structural.

**"Skip" semantics — three distinct skips, all mandatory.** (i) The campaign
is skippable wholesale: a player who never opens it loses nothing. (ii) Every
mission is individually skippable: a Skip affordance on the mission screen and
the pause menu records the mission as *skipped* (distinct from completed — the
end screen and the progress file distinguish them) and unlocks the next.
(iii) Every cutscene is skippable at any moment (§5). Unlocks are per-user
convenience, not locks — a skipped mission stays replayable.

**Persistence format.** A small hand-rolled file in the `key=value` spirit of
the settings format (ADR-0100 part (b) Option 1, `docs/adr/0100-client-app-shell.md:89-115`)
— one line per mission (`<mission-id>=completed|skipped`), per-key salvage,
unknown ids ignored. ADR-0100's Option 2 trigger ("a settings schema past ~20
keys with structure", pre-approved by the PM at `0100:225-228`) would fire if
campaign progress grows structured beyond flat lines; the format decision is
Lane A's (the client owns the config surface) and is flagged, not made, here.

**Mid-mission save — assessed only, not designed.** Feasible *via the replay
discipline*, with no new frozen-decision surface: FD-1's record (seed +
content + the ordered command log) already reproduces any match exactly
(`runner.rs:43-81`; the round-trip gate A2, `ci.yml:105-128`). A save would
persist `(mission id, seed, the fed command log, current tick)` in the user
folder; resume re-simulates from tick 0 through `run_command_log` and
re-hosts from the saved tick. The cost is bounded by plan §15's headless
throughput budget (≥1000 ticks/s, `plan.md:571`) — minutes of campaign are
seconds of re-simulation. Honest caveats: (i) **controller state is not
serialized** — on resume the AI controllers restart with fresh bookkeeping, so
the continuation diverges from the never-interrupted timeline (deterministic
for a given resume point, and still replay-recordable, but not
byte-continuation; the A15 restart discipline — drop + reconstruct — is the
accepted precedent, not an in-place state restore); (ii) a true in-place state
serializer does not exist (`hash_state` is a digest, not a serialization,
`hash.rs:33-171`) and would be a new sim surface — not recommended now;
(iii) checkpoint-hash equality across an interrupt is *not* expected and must
not be asserted by any test. Verdict: feasible, valuable for long missions,
sized M, deliberately last in the phasing (§9).

### 7. Identity: what is hashed, what is not, and the R2 interaction

**Hashed today (unchanged by this ADR):** the content identity covers rules —
including the victory kind and `allow_resignation` (`bundle.rs:621-635`) —
entities (`bundle.rs:637-654`), factions including roster, production lists
and starting forces (`bundle.rs:726-750`), and the whole map including the
display-only heightmap (`bundle.rs:752-794`, the ADR-0200 §2 interim); the map
id is domain-separated over the map section (`bundle.rs:264-269`). The state
hash encodes only the encoding word, tick, RNG state, id allocator, entities
with capability data, and players (`hash.rs:33-171`) — no identity value, no
fog (A-059), no outcome (A-066) — which is why the demo and flagship goldens
are structurally immune to everything this ADR proposes.

**The mission identity (new): authoritative and domain-separated.** The
mission record — map reference, faction/rules variant references, fog preset,
objectives, AI settings, story hook ids — changes bytes the simulation can
observe, so it must be identity-bearing (ADR-0200 §3's rule for authoritative
content). Recommendation: a **`mission id`** — the `MAP_ID_DOMAIN` pattern
(`bundle.rs:34-36`) with its own domain word, computed over the mission's
canonical encoding — echoed into the world definition (as the map id is,
`bundle.rs:459-460`) and the replay header, compared by `verify_replay`
beside the existing two checks (`crates/tools/src/verify.rs:37-54`). A
mission-less match computes its content hash through the exact unchanged byte
path: the mission category appends nothing to the existing encoders, so the
registered content hash `0x9bc18c521107b262` and map id `0xd38136401ab02ff1`
cannot move.

**Not hashed, ever:** cutscene and story presentation files (local, swappable,
pack-class), campaign progress (per-user state), and the *resolution* of story
hooks (the ids hash with the mission; the cutscenes they name do not).

**Interaction with the R2 re-baseline.** ADR-0200 §2(b) already schedules the
heightmap's exit from the content hash and map id at R2, in one bundled,
owner-authorized golden move. Folding the mission record into the single
content hash would append bytes to *every* bundle's encoding — an unauthorized
golden move today, so this ADR does not do it. At R2 the owner may choose to
fold mission identity into the content hash as part of that one already-
authorized move; until then the mission id rides alongside exactly as the map
id does. **Nothing in this ADR forces an earlier, separately authorized golden
move** — that is a design constraint, not an accident: it is why the mission
id is domain-separated rather than folded, why AI defaults are byte-exact
(§4), and why objectives are derived state (§2). The replay header gaining a
mission field when a mission match is recorded is a `FORMAT_VERSION` bump
(`replay/lib.rs:38`) — not a golden: no replay file is committed to the repo
(the A2 job records and verifies within one CI run, `ci.yml:105-128`).

### 8. FD-1..FD-10 check

| FD | Verdict | Why |
|---|---|---|
| FD-1 deterministic fixed-tick, match = seed + content + command log | **clarified** | A mission match is seed + content(+mission id) + command log — the identity gains a dimension, the record shape is unchanged; objectives are evaluated inside the tick (§2a); a mission's outcome is verifiable by re-simulation |
| FD-2 all intent through validated commands | **unchanged** | Objectives aren't intent (they are match rules, stage 10's job today); cutscenes never submit; mission-driven AI behavior still flows `think` → commands through the same gate |
| FD-3 unified entity model, ids never reused | **unchanged** | No new entity types; objectives reference entities by id and kinds by id exactly as the elimination rule does |
| FD-4 strict data/code boundary; add-a-map = zero engine changes | **extended** (ADR-0200's pattern) | Mission = versioned data file; objective evaluation, gate checks and AI machines = code; add-a-mission is the add-a-map promise verbatim (§3); the one sanctioned code change (the Build-gate membership check) is a review-visible gate extension per plan §8.3, data still selecting the allowed set |
| FD-5 no floats in sim or content | **unchanged** | All mission numbers are integers: ticks, counts, tiles, milli-tiles, milliseconds — the §10.2 authoring units (`plan.md:373-379`) |
| FD-6 sim depends on nothing above it | **unchanged** | Mission data reaches the sim only through the content seam (`world()`); cutscenes, progress and packs live above; no client/engine type crosses down |
| FD-7 AI only through commands + fog-filtered view | **unchanged** | The `Controller` trait is untouched; mission AI settings are plan data; fog presets apply to all views alike (§3) |
| FD-8 fog filters, never alters | **clarified** | A fog preset changes the filter's *initial state* (derived, unhashed) — never simulation state; declared here so the first implementation brief inherits it as a named rule |
| FD-9 events flow outward | **unchanged** | Objective progress is outward events consumed after the step; the client never polls sim internals; cutscenes read views/events only |
| FD-10 content schema forward-only | **extended** | The mission schema is versioned with the same one-step-migration discipline as the map chain (`version.rs:101-121`) |

### 9. Phasing — the smallest vertical slice first

**Phase 1 — the vertical slice (size M).** One mission on `crossroads_64` with
the unchanged Legion content; objective set = two kinds — *destroy the enemy
command center* (destroy) and *train four workers* (build/train N, optional) —
both immediately expressible (§2's table); one skippable live-state cutscene
at mission start (pause, camera pan over the frozen map, ASCII caption,
any-key/Esc skip) on the current block art.
Needs: **sim** (Lane B) — the objective-set data through the world seam,
stage 10's generalized evaluation, the objective-state event family;
**content** (Lane B) — mission schema v1, loader, reference validation, the
domain-separated mission id; **replay/tools** (Lane B) — the optional mission
field in the header and `verify_replay`'s third comparison;
**client** (Lane A) — the mission list entry, the objective HUD line, the
cutscene player, end-screen mission text. **Engine: nothing** — pause, the
camera and the render path already exist. **AI: nothing** — the default plan
is the slice's opponent. No golden moves; the gate count grows with new tests.

**Phase 2 — restrictions and AI settings (M).** Faction/rules variant
references (buildable/trainable, starting forces, starting ore), the
Build-gate membership check (§3's flagged asymmetry), fog presets, the
remaining objective kinds (survive, reach, protect, gather — with the
cumulative-vs-balance semantics choice), and the AI plan tuning overrides with
byte-exact defaults (§4's four proof obligations).

**Phase 3 — the campaign (S/M).** The campaign file, the progress file in the
config folder, unlock and skip semantics (§6's three skips), challenge
missions surfaced beside the campaign, story hooks wired to the
objective-state events (cutscene triggers beyond mission-start).

**Phase 4 — mid-mission save (M, optional).** The command-log checkpoint of
§6, assessed and deliberately last.

**Which steps move goldens: none, in any phase, before R2.** That is the
binding constraint every phase above is shaped around (§7). The only
golden-adjacent decision — folding the mission id into the content hash —
rides the already-authorized R2 re-baseline window, and even there it is the
owner's option, not this ADR's requirement. Lane assignments: Phase 1 splits
Lane B (sim, content, replay, tools, ai-default) / Lane A (client); the split
is clean because the boundary is the existing event/view seam.

### 10. Deferred and out of scope; open owner decisions

**Deferred and out of scope:**

- **Online and friends play — report: no netcode exists today.** The workspace
  is nine crates plus tests, none networking (`Cargo.toml:3`); the controller
  vocabulary is exactly `Human | Ai` (`lib.rs:76-81`); multiplayer is plan
  §17's postponed row whose hook is already present ("Deterministic
  command-log sim, Command.tick, hashed checkpoints", `plan.md:602`); ADR-0200
  §3 names the lockstep handshake future work and §8 defers the wire protocol.
  The mission record is designed to join that future handshake's comparison
  set (mission id beside content hash and map id) so missions never have to be
  re-designed for it.
- **A scripting language for missions or waves — ruled out.** Data selects and
  parameterizes code (`plan.md:360-371`, the standing refusal; ADR-0200 §8's
  trigger row requires a concrete, ADR'd need that capabilities and hooks
  cannot express — missions do not create one).
- **An in-game editor — out.** The World Builder is ADR-0200 §8's deferred
  table's row, triggered by map authors outgrowing hand-authored RON.
- **Voice — out.** Audio today is the synthesized cue bank behind the
  `AudioSink` seam (ADR-0002); voice is a pack-era asset.
- **Real art — out until the pack layer and the licence decision.** The
  campaign ships on placeholder block art (§5).

**Open owner decisions (recorded for the PM to route):**

1. **Premise** — what the campaign's story is about; nothing here depends on
   it, but every cutscene and mission name does.
2. **Tone** — Serious/grim vs pulpy/adventurous; drives text volume and
   cutscene pacing.
3. **Factions** — whether the campaign stays Legion-only (one faction exists
   today, `bundle.rs:305-310`; a second faction is plan §17's hook) or
   introduces an antagonist faction as campaign content.
4. **Mission identity at R2** — fold into the content hash during the
   authorized re-baseline, or keep the domain-separated mission id
   permanently (§7).
5. **Assets licence** — the manifest's `licence` field (ADR-0200 §7) needs the
   owner's choice before any pack-era asset (fonts included) ships.

## Evidence

Every current-behavior claim above cites `file:line` read directly at the
pinned head `d989336e4ec1152c1c7333b91c9e57c71fd9c300` — the branch rides
master exactly (zero commits when the research ran), so the working tree *is*
the pinned head and every citation is valid by construction, with no
blob-identity reconciliation needed (the B-008 D-1 pattern, unnecessary here).

Verification performed in this session (B-010):

- **Session start:** fresh clone + `git fetch --all --prune`; master head
  `d989336e4ec1152c1c7333b91c9e57c71fd9c300`, `v0.1.0^{commit}`
  `4477d83d303c2f9e8bbaebedc457580128da86d3`, `v0.1.1^{commit}`
  `2c5e2905225311860ff6f9a96796c305a09a556f` — all three match the brief
  exactly; no STOP.
- **The gate on untouched master** (the branch base, tree identical to
  master): `cargo fmt --all -- --check` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo test --workspace` **576 passed /
  0 failed / 2 ignored**; `cargo test --workspace --release` **571 passed /
  0 failed / 2 ignored** — matching the brief's expectation exactly.
- **The goldens, re-derived live, not trusted:** `tools content-validate
  content` prints `content hash: 0x9bc18c521107b262, map id:
  0xd38136401ab02ff1` (the registered values, `AI-Handoff.md:32`);
  `tools headless --seed 7 --ticks 300` ends `0xb6fff6659cfb7709`;
  `tools headless --seed 7 --ticks 7200 --p1 ai --p2 ai` ends
  `0x6e9a18bd7c5f699f` — both bit-identical to the registered goldens.
- **No mission-shaped surface exists**: a case-insensitive search over
  `crates/` for mission/campaign/cutscene/tutorial returns only incidental
  word fragments ("sub*mission*", "per*mission*"); the workspace members are
  `crates/*` + `tests` (`Cargo.toml:3`).
- **This ADR moves no code, no test, no content and no workflow**: the
  branch's diff is this file and the report
  (`git diff --name-only master...HEAD`) — the acceptance check, satisfied by
  construction.

## Consequences

- **Cheaper / possible if accepted:** the owner's campaign direction lands on
  named seams instead of an invented framework — objectives reuse stage 10's
  evaluation slot and the A-066 derived-state discipline; restrictions reuse
  the faction/rules data path and its encoders; cutscenes reuse pause, camera
  and the event seam; progression reuses the config-folder discipline; the
  mid-mission save reuses the replay record itself. Missions inherit FD-4's
  add-a-file promise. The challenge-mission and campaign features become
  content + client work, not engine work.
- **More expensive:** stage 10 grows a data-driven objective evaluator (with
  the review obligation that the default objective set is byte-equivalent to
  today's elimination rule); the mission schema, loader and validators are a
  new content kind to maintain forward-only forever (FD-10); the replay
  header grows an optional field; the client grows a mission/campaign surface
  (Lane A briefs); the Build-gate membership check is a small but real gate
  change; the gate count grows with each phase's tests.
- **What breaks if accepted: nothing, now.** No golden moves (§7 proves each
  one unreachable); no test, replay or workflow changes in this brief — the
  diff is two Markdown files and the gate counts are unchanged. The first
  implementation brief (Phase 1) is where code moves, and it moves none of
  the goldens either, by the same construction.
- **The interim rules stay honest:** the heightmap's hash participation
  (ADR-0200 §2's interim) is untouched; the victory-parameters gap (§1) is
  recorded, not silently fixed; the R2 fold option (§7) is the owner's, not a
  default.

## Alternatives considered

- **Do nothing — campaigns stay postponed (plan §17's row).** Rejected: the
  owner's direction is explicit and the standing orders' PRODUCT PHASE
  amendment suspends the anti-goal; deciding the objective model after
  mission-shaped code exists would mean deciding it under pressure, with the
  authority question (who ends a match) already forked in practice.
- **Client-side objective evaluation (option c).** Rejected in §2: mission
  outcomes would be client-authoritative — unverifiable by replay, forgeable,
  and at odds with FD-1's record-is-the-match principle.
- **Host-side objective evaluation (option b).** Rejected in §2: forks the
  end-of-match authority between stage 10 and the host, leaves the outcome out
  of the replay record's semantics, and places authoritative match logic in
  Lane A's engine crate.
- **Inline restrictions in the mission file instead of content references.**
  Rejected (§3): inlining buildable/trainable lists would duplicate the
  faction schema and need new encoders; referencing variant faction/rules
  files reuses the existing validators, encoders and identity semantics — the
  mission stays a thin, closed reference set.
- **Fold the mission into the content hash now.** Rejected (§7): it appends
  bytes to every bundle's encoding and moves the content-hash golden — an
  unauthorized move. The domain-separated mission id achieves identity
  semantics with zero movement; the fold is an R2-window option.
- **A mission scripting language (objectives-as-logic, waves-as-scripts).**
  Refused outright (plan §10.1's standing refusal, `plan.md:360-371`; ADR-0200
  §8's trigger row): mission data selects and parameterizes code evaluators;
  no expressions, no conditionals, no triggers beyond the fixed vocabulary.
- **A second, staged-scene engine for cutscenes now.** Rejected (§5): live-
  state scenes cover the campaign's in-match moments with zero new surface;
  staged scenes are an art question that waits on the pack layer.
- **serde-backed campaign progress now.** Deferred (§6): the hand-rolled
  format covers flat per-mission lines; ADR-0100's Option 2 trigger is the
  pre-approved escape hatch if progress grows structured — Lane A's decision
  at that time.

## Decision

<Only filled in once accepted or rejected, with a one-line rationale.>
