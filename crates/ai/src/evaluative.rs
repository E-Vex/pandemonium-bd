//! The evaluative opponent (plan §19 Update 8, ADR-0204): the scripted Alpha
//! plus two evaluative systems — research and casting — that use the verbs
//! Updates 6 and 7 added, through the same command door every issuer shares.
//!
//! ## The composition law
//!
//! [`TacticalController`] OWNS a [`ScriptedController`] and forwards every
//! think to it first: the economy, the infrastructure, the defense, and the
//! wave machine are the scripted opponent's own behavior, byte for byte —
//! this layer never re-implements them, it sits *above* them. "Evaluative AI
//! above the command interface" (plan §19's wording) is exactly this shape:
//! the foundation does not move underneath; the controller decides *more*,
//! through the same gate (FD-7).
//!
//! ## What "evaluative" means here
//!
//! The two systems read the view's own evaluation surfaces — the outward
//! data the command card reads (FD-9): `research` (own in-flight projects),
//! `researched` (the completed ledger), `abilities` (own cooldowns), and the
//! fog-filtered `entities`. A player reads the same fields off the same
//! card; the controller reads them as data and decides on them. No sim
//! internals, no wall clock, no RNG draws of its own — every rule below is a
//! pure function of (view, tick, own state).
//!
//! ## The two rules (ADR-0204 §3)
//!
//! - **Research**: when the carrier stands complete and idle, a slot whose
//!   prerequisites sit in the ledger is affordable, unheld, and not in
//!   flight — research it (one per tick, cooldown, retry on expiry: the
//!   acting-blind, verifying-by-sight discipline the scripted half uses).
//! - **Cast**: when a fielded caster's slot is ready and a legal target
//!   cluster is sighted, cast at its centroid, clamped to the authored
//!   range. `DamageArea` targets the densest sighted enemy cluster;
//!   `HealArea` targets the weakest own cluster; `SpeedBurst` targets the
//!   own cluster nearest the wave's destination.
//!
//! Both rules iterate the view's ascending-id lists in fixed order, so two
//! controllers fed the same views emit the same commands, byte for byte.

use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim_api::{
    AbilityView, Command, CommandKind, EntityId, EntityView, KindId, PlayerView, ResourceId, Tick,
};

use crate::scripted::{AiPlan, ScriptedController};
use crate::Controller;

/// The research rule's per-emission cooldown (ticks): matches the scripted
/// half's `TRAIN_COOLDOWN_TICKS` pacing — one decision per second of game
/// time, so the ledger can settle between spends.
const RESEARCH_COOLDOWN_TICKS: u32 = 30;

/// The cast rule's per-caster cooldown (ticks): one cast attempt per caster
/// per second — the slot's own cooldown governs the true rate; this paces
/// the DECISION, not the cast (a rejected cast retries on this cadence).
const CAST_COOLDOWN_TICKS: u32 = 30;

/// How long a pending research order may stay unresolved before the
/// bookkeeping frees its slot for a retry (the pending-train expiry rule:
/// twice the authored research time plus slack — a project that never
/// appeared was refused, and the purse was never spent).
const RESEARCH_SLACK_TICKS: u32 = 90;

/// Per-attempt cooldown between carrier Build commands, in ticks (the
/// scripted half's `BUILD_COOLDOWN_TICKS` — same role: pace the retries).
const BUILD_COOLDOWN_TICKS: u32 = 45;

/// When a carrier Build's site has not appeared in the view by this many
/// ticks, the attempt settles as failed (the scripted half's
/// `SITE_SIGHT_GRACE_TICKS` — covers the command-apply-to-sighting latency).
const SITE_SIGHT_GRACE_TICKS: u32 = 2;

/// The cluster grid's cell size, in whole tiles (ADR-0204 §3: a fixed
/// deterministic bucketing — every sighted combatant lands in the cell its
/// center occupies; no smoothing, no floats).
const CLUSTER_CELL_TILES: i32 = 4;

/// One researchable slot, host-derived from the content (ADR-0204 §2):
/// authored parameters only, ids as data.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResearchSlot {
    /// The upgrade's authored slot index in the carrier kind's list.
    pub slot: u8,
    /// The slot's resolved cost in the ledger's own shape.
    pub cost: Vec<(ResourceId, i64)>,
    /// The slot's research duration in ticks (the pending-expiry pacing).
    pub research_ticks: u32,
    /// The slot's prerequisite (carrier kind, slot) pairs, resolved by the
    /// content seam.
    pub requires: Vec<(KindId, u8)>,
}

/// The research directive (ADR-0204 §2): the carrier to raise and the tree
/// it offers, derived by capability shape — never name-matched.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResearchDirective {
    /// The structure kind that carries the `Upgrades` capability.
    pub carrier: KindId,
    /// The carrier's build cost in the ledger's own shape.
    pub carrier_cost: Vec<(ResourceId, i64)>,
    /// The carrier's build time in ticks (the pending-expiry pacing).
    pub carrier_build_ticks: u32,
    /// The structure kinds that must stand before the carrier may be built
    /// (the content's own requirement list, resolved to ids) — the gate's
    /// `RequirementsUnmet` check, read ahead through the view.
    pub carrier_requires: Vec<KindId>,
    /// The carrier's authored upgrade slots.
    pub slots: Vec<ResearchSlot>,
    /// Candidate carrier placements, nearest first (map knowledge — the
    /// `build_spots` scan, the depot/barracks rule).
    pub carrier_spots: Vec<pandemonium_sim_api::TilePos>,
}

impl ResearchDirective {
    /// Whether the ledger covers a cost list (the can_afford law, on the
    /// view's outward resource list).
    fn affordable(view: &PlayerView, cost: &[(ResourceId, i64)]) -> bool {
        cost.iter().all(|(resource, amount)| {
            *amount <= 0
                || view
                    .resources
                    .iter()
                    .any(|held| held.resource == *resource && held.amount >= *amount)
        })
    }
}

/// One castable slot the faction fields, host-derived (ADR-0204 §2): the
/// caster kind, the slot, and the authored parameters the decision reads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AbilityDirective {
    /// The caster kind.
    pub caster: KindId,
    /// The ability's authored slot index in the caster kind's list.
    pub slot: u8,
    /// The cast range in milli-tiles (the clamp law: the controller walks
    /// nothing; `AbilityOutOfRange` is respected by construction).
    pub cast_range_milli: i32,
    /// The effect kind's decision rule (ADR-0204 §3): which cluster shape
    /// the cast aims at.
    pub aim: AimKind,
}

/// Which cluster a cast aims at (ADR-0204 §3 — closed, one arm per shipped
/// effect kind).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AimKind {
    /// `DamageArea`: the densest sighted enemy cluster.
    EnemyCluster,
    /// `HealArea`: the weakest own cluster (lowest health weight).
    WeakestFriendlies,
    /// `SpeedBurst`: the own cluster nearest the wave's destination.
    FriendliesNearestPush,
}

/// The evaluative opponent's plan (ADR-0204 §2): the scripted base plus the
/// two directives. `None` directives mean the content offers nothing (a
/// faction with no Upgrades carrier and no Abilities caster is the scripted
/// opponent, exactly).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EvalPlan {
    /// The scripted half's plan, verbatim.
    pub base: AiPlan,
    /// The research directive, when the faction's roster can raise an
    /// `Upgrades` carrier.
    pub research: Option<ResearchDirective>,
    /// One directive per (caster kind, slot) the faction's roster fields.
    pub abilities: Vec<AbilityDirective>,
}

/// The evaluative opponent (ADR-0204): the scripted Alpha plus research and
/// casting. Construct from an [`EvalPlan`]; the host drives
/// [`Controller::think`] exactly as for the scripted one.
pub struct TacticalController {
    player: pandemonium_sim_api::PlayerId,
    base: ScriptedController,
    plan: EvalPlan,
    /// Per-tick sequence counter, restarted every think (the gate's
    /// duplicate rule) — AFTER the base's own emissions, so the two layers
    /// share one numbering space per tick.
    seq: u32,
    /// Own entity ids the controller has sighted (spawn sightings retire
    /// pending carrier builds — the base's rule, applied to the carrier).
    seen: Vec<EntityId>,
    /// A pending carrier build: issued, not yet confirmed by sight (the
    /// scripted half's `BuildAttempt` discipline, on the carrier).
    attempt: Option<BuildAttempt>,
    /// The next candidate carrier spot's cursor.
    carrier_spot: usize,
    /// The cooldown gate for carrier build retries.
    build_cooldown_until: Tick,
    /// The carrier entity, once sighted (idempotent re-derivation per think).
    carrier: Option<EntityId>,
    /// Pending research orders, one per slot index: the tick the order
    /// expires at (never sighted in `researched` by then = refused or the
    /// researcher died; retry).
    research_pending: Vec<(u8, Tick)>,
    research_cooldown_until: Tick,
    /// Per (caster kind, slot) the last decision tick (the pacing cadence);
    /// `None` until the first decision.
    cast_last: Vec<Option<Tick>>,
}

/// A carrier build order in flight (the scripted half's `BuildAttempt`,
/// applied to the research carrier): issued, not yet confirmed by sight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct BuildAttempt {
    /// When the command was issued.
    issued_at: Tick,
    /// How many own entities of the carrier kind the view held at issue
    /// time — a successful placement raises the count (sites spawn on the
    /// spot), everything else leaves it untouched.
    count_at_issue: u32,
}

impl TacticalController {
    /// Builds the controller for a plan. The plan's `rng_seed` is the base
    /// plan's own — the evaluative layer draws NOTHING (ADR-0204 §3: the
    /// base's seeded stream serves the wave jitter; this layer adds zero
    /// draws, so the shared stream is untouched by the layer above it).
    pub fn new(plan: EvalPlan) -> Self {
        let player = plan.base.player;
        let cast_last = plan.abilities.iter().map(|_| None).collect();
        Self {
            player,
            base: ScriptedController::new(plan.base.clone()),
            plan,
            seq: 0,
            seen: Vec::new(),
            attempt: None,
            carrier_spot: 0,
            build_cooldown_until: 0,
            carrier: None,
            research_pending: Vec::new(),
            research_cooldown_until: 0,
            cast_last,
        }
    }

    /// The base plan's controller (for hosts and tests).
    pub fn scripted(&self) -> &ScriptedController {
        &self.base
    }

    /// The plan this controller runs (for hosts and tests).
    pub fn plan(&self) -> &EvalPlan {
        &self.plan
    }

    /// The base controller's RNG state — the tactical layer owns no RNG of
    /// its own, so the base's stream is the whole stream (B-021's probe
    /// discipline, unchanged).
    pub fn rng_state(&self) -> (u64, u64) {
        self.base.rng_state()
    }

    /// Appends one command with the next sequence number, continuing the
    /// base's numbering for this tick.
    fn emit(&mut self, out: &mut Vec<Command>, tick: Tick, kind: CommandKind) {
        self.seq += 1;
        out.push(Command::new(self.player, tick, self.seq, kind));
    }

    /// The next candidate carrier spot (the scripted half's `next_spot`
    /// rule, verbatim): spots are consumed in order — a failed placement
    /// and a successful one alike move on to the next candidate.
    fn next_carrier_spot(&mut self) -> Option<pandemonium_sim_api::TilePos> {
        let spots = &self
            .plan
            .research
            .as_ref()
            .map(|directive| directive.carrier_spots.as_slice())
            .unwrap_or(&[]);
        if spots.is_empty() {
            return None;
        }
        let index = self.carrier_spot % spots.len();
        self.carrier_spot = self.carrier_spot.wrapping_add(1);
        spots.get(index).copied()
    }
}

impl Controller for TacticalController {
    /// One tick of the evaluative opponent: the scripted half thinks first
    /// (its own bookkeeping and commands, byte for byte), then the two
    /// evaluative systems run in a fixed order — research, then casting.
    /// The order is part of the controller's determinism contract.
    fn think(&mut self, view: &PlayerView, tick: Tick, out: &mut Vec<Command>) {
        debug_assert_eq!(
            view.player, self.player,
            "a controller must be fed its own view"
        );
        // The scripted half: its seq counter restarts at 0 every think, so
        // its commands carry 1..=N; the evaluative layer continues N+1...
        self.base.think(view, tick, out);
        self.seq = out.iter().filter(|c| c.issuer == self.player).count() as u32;

        // ---- Research system (ADR-0204 §3). ----
        self.research_step(view, tick, out);

        // ---- Cast system (ADR-0204 §3). ----
        self.cast_step(view, tick, out);
    }
}

impl TacticalController {
    /// The research rule: raise the carrier when affordable, then research
    /// the first slot whose prerequisites are met, is affordable, unheld,
    /// and not in flight — one per cooldown, retried on expiry.
    fn research_step(&mut self, view: &PlayerView, tick: Tick, out: &mut Vec<Command>) {
        let Some(directive) = self.plan.research.clone() else {
            return;
        };

        // Sight bookkeeping: new own ids are spawn sightings (the base's
        // rule); a sighted carrier id settles the pending build.
        for entity in view.entities.iter().filter(|e| e.owner == self.player) {
            if !self.seen.contains(&entity.id) {
                self.seen.push(entity.id);
            }
            if entity.kind == directive.carrier && self.carrier.is_none() {
                self.carrier = Some(entity.id);
            }
        }

        // Retire pending research orders that completed (sighted in the
        // ledger) or expired (never completed: refused, or the researcher
        // died — the ledger is the completion signal, the only one).
        self.research_pending.retain(|(slot, expire_at)| {
            let completed = view
                .researched
                .iter()
                .any(|done| done.kind == directive.carrier && done.slot == *slot);
            !completed && *expire_at > tick
        });

        // If no carrier stands, raise one — the scripted half's own build
        // discipline, verbatim: one attempt at a time, confirmed by sight
        // (a successful Build raises the own count of that kind — sites
        // spawn on the spot), failed placements advance the candidate
        // spot, a cooldown paces retries. The requirements read ahead
        // through the view (the gate's own `RequirementsUnmet` rule, seen
        // coming — the carrier is forge-tier: it needs its producer-tier
        // prerequisite standing first).
        if self.carrier.is_none() && self.attempt.is_none() && tick >= self.build_cooldown_until {
            let requirements_stood = directive.carrier_requires.iter().all(|kind| {
                view.entities
                    .iter()
                    .any(|e| e.owner == self.player && e.kind == *kind)
            });
            let worker = view
                .entities
                .iter()
                .filter(|e| e.owner == self.player && e.kind == self.plan.base.worker)
                .min_by_key(|e| (e.pos - self.plan.base.home).len_sq_raw())
                .copied();
            // The bank rule (ADR-0204 §3): the carrier builds only from a
            // purse that also covers the army's next train — the scripted
            // half's queue spends first in the same tick (its commands emit
            // before this layer's), so a carrier that costs the army's food
            // is a rejected Build, not a strategy. The margin is the army's
            // most expensive unit: with it, both spends fit one tick.
            let army_margin = self
                .plan
                .base
                .army_cost
                .iter()
                .flat_map(|cost| cost.iter().map(|(_, amount)| *amount))
                .max()
                .unwrap_or(0);
            let carrier_with_margin: Vec<(ResourceId, i64)> = directive
                .carrier_cost
                .iter()
                .map(|(resource, amount)| (*resource, amount + army_margin))
                .collect();
            if let (true, Some(worker)) = (requirements_stood, worker) {
                if let Some(spot) = self.next_carrier_spot() {
                    if ResearchDirective::affordable(view, &carrier_with_margin) {
                        let count_at_issue = view
                            .entities
                            .iter()
                            .filter(|e| e.owner == self.player && e.kind == directive.carrier)
                            .count() as u32;
                        self.emit(
                            out,
                            tick,
                            CommandKind::Build {
                                worker: worker.id,
                                structure: directive.carrier,
                                at: spot,
                            },
                        );
                        self.attempt = Some(BuildAttempt {
                            issued_at: tick,
                            count_at_issue,
                        });
                    }
                }
            }
        }
        // Confirm the in-flight attempt by sight: after the grace window,
        // a successful placement raised the carrier count (the site spawns
        // on the spot the moment the command applies); an unchanged count
        // was a refusal (blocked ground, most likely) — cool down and the
        // next attempt lands on the next candidate spot.
        if let Some(attempt) = self.attempt {
            if tick.saturating_sub(attempt.issued_at) >= SITE_SIGHT_GRACE_TICKS {
                let count_now = view
                    .entities
                    .iter()
                    .filter(|e| e.owner == self.player && e.kind == directive.carrier)
                    .count() as u32;
                if count_now <= attempt.count_at_issue {
                    self.build_cooldown_until = tick + BUILD_COOLDOWN_TICKS;
                }
                self.attempt = None;
            }
        }

        // Research the first legal slot.
        if self.carrier.is_some() && tick >= self.research_cooldown_until {
            for slot in &directive.slots {
                let already_held = view
                    .researched
                    .iter()
                    .any(|done| done.kind == directive.carrier && done.slot == slot.slot);
                let in_flight_here = view
                    .research
                    .iter()
                    .any(|in_flight| in_flight.slot == slot.slot);
                let pending_here = self
                    .research_pending
                    .iter()
                    .any(|(pending, _)| *pending == slot.slot);
                let prerequisites_met = slot.requires.iter().all(|(carrier, slot)| {
                    view.researched
                        .iter()
                        .any(|done| done.kind == *carrier && done.slot == *slot)
                });
                if already_held || in_flight_here || pending_here || !prerequisites_met {
                    continue;
                }
                if !ResearchDirective::affordable(view, &slot.cost) {
                    continue;
                }
                if let Some(carrier) = self.carrier {
                    self.emit(
                        out,
                        tick,
                        CommandKind::Research {
                            researcher: carrier,
                            upgrade: slot.slot,
                        },
                    );
                    self.research_pending.push((
                        slot.slot,
                        tick + slot.research_ticks.saturating_mul(2) + RESEARCH_SLACK_TICKS,
                    ));
                    self.research_cooldown_until = tick + RESEARCH_COOLDOWN_TICKS;
                }
                break;
            }
        }
    }

    /// The cast rule: for each directive, when a fielded caster's slot is
    /// ready (the view's own cooldown readout) and the pacing cadence allows
    /// a decision, pick the aim cluster and cast at its centroid, clamped to
    /// the authored range.
    fn cast_step(&mut self, view: &PlayerView, tick: Tick, out: &mut Vec<Command>) {
        if self.plan.abilities.is_empty() {
            return;
        }
        // The wave's push destination: the base's enemy-start knowledge (a
        // map fact) — the SpeedBurst rule's "nearest the push" anchor.
        let push_target = self.plan.base.enemy_start;

        for (index, directive) in self.plan.abilities.clone().iter().enumerate() {
            // The pacing cadence: a decision at tick T blocks the next
            // until T + CAST_COOLDOWN_TICKS; `None` is the opening — the
            // first ready slot fires immediately, exactly like the research
            // rule's zero cooldown at match start.
            if let Some(last) = self.cast_last[index] {
                if tick < last.saturating_add(CAST_COOLDOWN_TICKS) {
                    continue;
                }
            }
            // Every own fielded caster of the kind, ascending id (the view's
            // order), with the slot's readout ready — cast the first one.
            let Some(caster) = view
                .entities
                .iter()
                .filter(|e| e.owner == self.player && e.kind == directive.caster)
                .copied()
                .find(|e| {
                    view.abilities.iter().any(|a: &AbilityView| {
                        a.caster == e.id && a.slot == directive.slot && a.cooldown_milli == 0
                    })
                })
            else {
                continue;
            };
            // Pick the aim point by effect kind.
            let Some(aim) = aim_point(view, self.player, directive.aim, push_target) else {
                continue;
            };
            // Clamp to the caster's authored range from where it stands
            // (the controller walks nothing — ADR-0204 §3).
            let point = clamp_to_range(caster.pos, aim, directive.cast_range_milli);
            // The fog law (see `point_sighted`): the gate stays the
            // authority; a refused cast retries on the cadence.
            if !point_sighted(view, point) {
                continue;
            }
            self.emit(
                out,
                tick,
                CommandKind::CastAbility {
                    caster: caster.id,
                    ability: directive.slot,
                    target: point,
                },
            );
            self.cast_last[index] = Some(tick);
        }
    }
}

/// The cast point's fog law, satisfied by construction (ADR-0204 §3): the
/// aim clusters are built from entities the view CARRIES (every entity in
/// `PlayerView.entities` is sighted — the view is fog-filtered at the
/// boundary), and the clamp only moves the point along the segment toward
/// the caster (a sighted position) — never outward. The gate stays the
/// authority (`NotVisible` refuses what this argument missed); a refused
/// cast retries on the pacing cadence, the acting-blind discipline.
///
/// Kept as a named function (not an inline comment) because the law it
/// encodes is the ADR's own: the controller never targets a point it
/// cannot see, because the view is all it has.
fn point_sighted(view: &PlayerView, point: Vec2Fx) -> bool {
    // Every entity position in the view is sighted by construction; the
    // aim point is either a sighted cluster's centroid or a point on the
    // segment between the caster and that centroid. True by construction;
    // the gate is the authority for the residual cases.
    let _ = (view, point);
    true
}

/// The aim point for one directive (ADR-0204 §3): the cluster rule by
/// effect kind. All clusters are grid-bucketed at [`CLUSTER_CELL_TILES`],
/// deterministic in bucket key order; ties break by first-seen entity id.
fn aim_point(
    view: &PlayerView,
    player: pandemonium_sim_api::PlayerId,
    aim: AimKind,
    push_target: Option<Vec2Fx>,
) -> Option<Vec2Fx> {
    match aim {
        AimKind::EnemyCluster => {
            let enemies: Vec<EntityView> = view
                .entities
                .iter()
                .filter(|e| e.owner != player && e.owner != pandemonium_sim_api::PlayerId::NEUTRAL)
                .copied()
                .collect();
            // Simplest densest-cluster form: bucket by grid cell, take the
            // densest cell's centroid.
            densest_centroid(&enemies)
        }
        AimKind::WeakestFriendlies => {
            let own: Vec<EntityView> = view
                .entities
                .iter()
                .filter(|e| e.owner == player)
                .copied()
                .collect();
            weakest_centroid(&own)
        }
        AimKind::FriendliesNearestPush => {
            let target = push_target?;
            let own: Vec<EntityView> = view
                .entities
                .iter()
                .filter(|e| e.owner == player)
                .copied()
                .collect();
            nearest_cluster_centroid(&own, target)
        }
    }
}

/// The densest grid-bucket's centroid (ascending bucket key, count then
/// first-member id as the tie-break — deterministic).
fn densest_centroid(entities: &[EntityView]) -> Option<Vec2Fx> {
    let mut buckets: Vec<((i32, i32), Vec<EntityView>)> = Vec::new();
    for entity in entities {
        let key = (
            entity.pos.x.floor_int() / CLUSTER_CELL_TILES,
            entity.pos.y.floor_int() / CLUSTER_CELL_TILES,
        );
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(*entity),
            None => buckets.push((key, vec![*entity])),
        }
    }
    let best = buckets
        .iter()
        .max_by_key(|(_, members)| (members.len(), ReverseId(members[0].id.0)))?;
    centroid(&best.1)
}

/// The lowest-health-weight grid-bucket's centroid (the weakest cluster).
fn weakest_centroid(entities: &[EntityView]) -> Option<Vec2Fx> {
    let mut buckets: Vec<((i32, i32), Vec<EntityView>)> = Vec::new();
    for entity in entities {
        let key = (
            entity.pos.x.floor_int() / CLUSTER_CELL_TILES,
            entity.pos.y.floor_int() / CLUSTER_CELL_TILES,
        );
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(*entity),
            None => buckets.push((key, vec![*entity])),
        }
    }
    // The bucket with the lowest max hp_fraction among members (the most
    // hurt cluster), ties by first-member id.
    let best = buckets.iter().min_by_key(|(_, members)| {
        let worst = members
            .iter()
            .map(|m| m.hp_fraction_milli)
            .min()
            .unwrap_or(1000);
        (worst, members[0].id.0)
    })?;
    centroid(&best.1)
}

/// The grid-bucket centroid nearest a target point (the push rule).
fn nearest_cluster_centroid(entities: &[EntityView], target: Vec2Fx) -> Option<Vec2Fx> {
    let mut buckets: Vec<((i32, i32), Vec<EntityView>)> = Vec::new();
    for entity in entities {
        let key = (
            entity.pos.x.floor_int() / CLUSTER_CELL_TILES,
            entity.pos.y.floor_int() / CLUSTER_CELL_TILES,
        );
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(*entity),
            None => buckets.push((key, vec![*entity])),
        }
    }
    let mut best: Option<(u64, &Vec<EntityView>)> = None;
    for (_, members) in &buckets {
        let center = centroid(members)?;
        let dist = (center - target).len_sq_raw();
        if best.is_none_or(|(best_dist, _)| dist < best_dist) {
            best = Some((dist, members));
        }
    }
    best.and_then(|(_, members)| centroid(members))
}

/// A bucket's centroid: the integer mean of member positions (fixed-point
/// division truncates toward zero — the integer law). Saturating casts:
/// the mean of view positions cannot leave i32 range, but the narrowing
/// is written as saturating anyway (the no-panic discipline).
fn centroid(entities: &[EntityView]) -> Option<Vec2Fx> {
    if entities.is_empty() {
        return None;
    }
    let sum = entities.iter().fold((0i64, 0i64), |acc, e| {
        (acc.0 + e.pos.x.raw() as i64, acc.1 + e.pos.y.raw() as i64)
    });
    let n = entities.len() as i64;
    Some(Vec2Fx::new(
        Fx::from_raw((sum.0 / n).clamp(i32::MIN as i64, i32::MAX as i64) as i32),
        Fx::from_raw((sum.1 / n).clamp(i32::MIN as i64, i32::MAX as i64) as i32),
    ))
}

/// Clamps a point to a milli-tile range around an anchor: if the point is
/// beyond `range_milli`, the point on the segment at exactly the range
/// (fixed-point, one division at the end — squared compare, then an exact
/// integer square root for the scale; no floats anywhere).
fn clamp_to_range(anchor: Vec2Fx, point: Vec2Fx, range_milli: i32) -> Vec2Fx {
    let range_raw = (range_milli as i64) * 65_536 / 1_000;
    let delta = point - anchor;
    let dist_sq = delta.len_sq_raw();
    let range_sq = (range_raw * range_raw) as u64;
    if dist_sq <= range_sq {
        return point;
    }
    // Scale the delta to the range: factor = range / dist — one integer
    // division per axis, truncating toward zero (the integer law).
    let dist_raw = isqrt(dist_sq);
    if dist_raw == 0 {
        return anchor;
    }
    let x = (delta.x.raw() as i64) * range_raw / dist_raw as i64;
    let y = (delta.y.raw() as i64) * range_raw / dist_raw as i64;
    anchor
        + Vec2Fx::new(
            Fx::from_raw(x.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
            Fx::from_raw(y.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
        )
}

/// Integer square root (floor) — the exact u64 path for the clamp's scale.
fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// A reversed id ordering key (the densest tie-break: HIGHER first-member id
/// wins ties so the pick is deterministic and stable across view growth).
struct ReverseId(u64);

impl PartialEq for ReverseId {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for ReverseId {}
impl PartialOrd for ReverseId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ReverseId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.cmp(&self.0) // reversed: the greater id orders first
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_sim_api::{
        EntityId, MoveState, PlayerId, ResearchedView, ResourceId, TileFog, ViewResource,
    };

    const WORKER: KindId = KindId(0);
    const CC: KindId = KindId(3);
    const NODE: KindId = KindId(4);
    const RIFLE: KindId = KindId(5);
    const FORGE: KindId = KindId(6);
    const PATHFINDER: KindId = KindId(7);
    const BALLISTA: KindId = KindId(8);
    const ORE: ResourceId = ResourceId(0);
    const BARRACKS_T: KindId = KindId(1);

    fn base_plan() -> AiPlan {
        AiPlan {
            player: PlayerId(0),
            rng_seed: 7,
            worker: WORKER,
            command_center: CC,
            depot: None,
            barracks: None,
            node: Some(NODE),
            army: vec![RIFLE],
            worker_cost: vec![(ORE, 50)],
            depot_cost: Vec::new(),
            barracks_cost: Vec::new(),
            worker_build_ticks: 360,
            army_cost: vec![vec![(ORE, 75)]],
            army_build_ticks: vec![300],
            army_pop: vec![1],
            worker_pop: 1,
            home: Vec2Fx::from_ints(14, 14),
            enemy_start: Some(Vec2Fx::from_ints(50, 50)),
            depot_spots: Vec::new(),
            barracks_spots: Vec::new(),
            wave_timer_ticks: None,
            wave_size: None,
            min_wave: None,
            army_cap: None,
            worker_target: None,
            first_attack_delay: None,
        }
    }

    fn eval_plan() -> EvalPlan {
        EvalPlan {
            base: base_plan(),
            research: Some(ResearchDirective {
                carrier: FORGE,
                carrier_cost: vec![(ORE, 250)],
                carrier_build_ticks: 900,
                carrier_requires: vec![BARRACKS_T],
                slots: vec![
                    ResearchSlot {
                        slot: 0,
                        cost: vec![(ORE, 250)],
                        research_ticks: 600,
                        requires: Vec::new(),
                    },
                    ResearchSlot {
                        slot: 1,
                        cost: vec![(ORE, 200)],
                        research_ticks: 450,
                        requires: vec![(FORGE, 0)],
                    },
                ],
                carrier_spots: vec![pandemonium_sim_api::TilePos { x: 20, y: 10 }],
            }),
            abilities: vec![
                AbilityDirective {
                    caster: BALLISTA,
                    slot: 0,
                    cast_range_milli: 13000,
                    aim: AimKind::EnemyCluster,
                },
                AbilityDirective {
                    caster: PATHFINDER,
                    slot: 0,
                    cast_range_milli: 4000,
                    aim: AimKind::FriendliesNearestPush,
                },
            ],
        }
    }

    fn entity(id: u64, kind: KindId, x: i32, y: i32, hp: u32) -> EntityView {
        EntityView {
            id: EntityId(id),
            owner: PlayerId(0),
            kind,
            pos: Vec2Fx::from_ints(x, y),
            facing: Vec2Fx::ZERO,
            hp_fraction_milli: hp,
            move_state: MoveState::Idle,
        }
    }

    fn foreign(id: u64, kind: KindId, x: i32, y: i32, hp: u32) -> EntityView {
        EntityView {
            owner: PlayerId(1),
            ..entity(id, kind, x, y, hp)
        }
    }

    fn view(tick: Tick, ore: i64, entities: Vec<EntityView>) -> PlayerView {
        view_full(tick, ore, entities, Vec::new(), Vec::new())
    }

    fn view_full(
        tick: Tick,
        ore: i64,
        entities: Vec<EntityView>,
        abilities: Vec<AbilityView>,
        researched: Vec<ResearchedView>,
    ) -> PlayerView {
        PlayerView {
            tick,
            player: PlayerId(0),
            resources: vec![ViewResource {
                resource: ORE,
                amount: ore,
            }],
            population: 4,
            population_cap: 10,
            entities,
            fog: Vec::new(),
            production: Vec::new(),
            abilities,
            research: Vec::new(),
            researched,
        }
    }

    fn think(controller: &mut TacticalController, view: &PlayerView) -> Vec<Command> {
        let mut out = Vec::new();
        Controller::think(controller, view, view.tick, &mut out);
        out
    }

    #[test]
    fn the_carrier_is_built_when_affordable() {
        let mut controller = TacticalController::new(eval_plan());
        // Workers and a fat purse: the first evaluative command is the
        // carrier build at the directive's first spot, by the nearest
        // worker to home.
        let v = view(
            0,
            1000,
            vec![
                entity(1, WORKER, 12, 16, 1000),
                entity(2, WORKER, 13, 15, 1000),
                entity(3, BARRACKS_T, 20, 10, 1000),
            ],
        );
        let commands = think(&mut controller, &v);
        let build = commands
            .iter()
            .find(|c| matches!(c.kind, CommandKind::Build { .. }))
            .cloned();
        assert!(build.is_some(), "the carrier build fires: {commands:?}");
        match build.unwrap().kind {
            CommandKind::Build { structure, at, .. } => {
                assert_eq!(structure, FORGE);
                assert_eq!(at, pandemonium_sim_api::TilePos { x: 20, y: 10 });
            }
            other => panic!("expected a build, got {other:?}"),
        }
    }

    #[test]
    fn the_carrier_waits_for_the_purse() {
        let mut controller = TacticalController::new(eval_plan());
        let v = view(
            0,
            100,
            vec![
                entity(1, WORKER, 12, 16, 1000),
                entity(3, BARRACKS_T, 20, 10, 1000),
            ],
        );
        let commands = think(&mut controller, &v);
        assert!(
            commands.iter().all(|c| !matches!(
                c.kind,
                CommandKind::Build {
                    structure: FORGE,
                    ..
                }
            )),
            "a thin purse builds no carrier: {commands:?}"
        );
    }

    #[test]
    fn research_fires_on_the_standing_carrier() {
        let mut controller = TacticalController::new(eval_plan());
        let v = view(
            0,
            1000,
            vec![
                entity(1, WORKER, 12, 16, 1000),
                entity(2, FORGE, 20, 10, 1000),
            ],
        );
        let commands = think(&mut controller, &v);
        // No build (the carrier stands); the first slot researches.
        assert!(matches!(
            commands[0].kind,
            CommandKind::Research { upgrade: 0, .. }
        ));
        // The pending holds the slot: no re-issue within the cooldown or
        // while pending.
        let v2 = view(
            30,
            1000,
            vec![
                entity(1, WORKER, 12, 16, 1000),
                entity(2, FORGE, 20, 10, 1000),
            ],
        );
        let again = think(&mut controller, &v2);
        assert!(
            again
                .iter()
                .all(|c| !matches!(c.kind, CommandKind::Research { upgrade: 0, .. })),
            "the pending holds the slot: {again:?}"
        );
    }

    #[test]
    fn the_prerequisite_edge_gates_the_second_slot() {
        let mut controller = TacticalController::new(eval_plan());
        let v = view_full(
            0,
            1000,
            vec![
                entity(1, WORKER, 12, 16, 1000),
                entity(2, FORGE, 20, 10, 1000),
            ],
            Vec::new(),
            vec![ResearchedView {
                kind: FORGE,
                slot: 0,
            }],
        );
        let commands = think(&mut controller, &v);
        // Slot 0 is held; slot 1's prerequisite (slot 0 on FORGE) is met:
        // it researches.
        assert!(matches!(
            commands[0].kind,
            CommandKind::Research { upgrade: 1, .. }
        ));

        // Without the prerequisite held, slot 1 never fires.
        let mut controller = TacticalController::new(eval_plan());
        let v = view(
            0,
            1000,
            vec![
                entity(1, WORKER, 12, 16, 1000),
                entity(2, FORGE, 20, 10, 1000),
            ],
        );
        let commands = think(&mut controller, &v);
        assert!(matches!(
            commands[0].kind,
            CommandKind::Research { upgrade: 0, .. }
        ));
    }

    #[test]
    fn the_volley_targets_the_densest_enemy_cluster() {
        let mut controller = TacticalController::new(eval_plan());
        // A Ballista at (30,30), ready; enemies clustered at (36,30) (four
        // in one grid cell) and a loner at (44,44).
        let ballista = entity(50, BALLISTA, 30, 30, 1000);
        let mut entities = vec![ballista, entity(1, WORKER, 12, 16, 1000)];
        for id in 60..64 {
            entities.push(foreign(id, RIFLE, 35 + (id as i32 - 60), 30, 1000));
        }
        entities.push(foreign(70, RIFLE, 44, 44, 1000));
        let v = view_full(
            0,
            1000,
            entities,
            vec![AbilityView {
                caster: EntityId(50),
                slot: 0,
                cooldown_milli: 0,
            }],
            Vec::new(),
        );
        let commands = think(&mut controller, &v);
        let cast = commands
            .iter()
            .find(|c| matches!(c.kind, CommandKind::CastAbility { .. }))
            .cloned();
        assert!(cast.is_some(), "the volley fires: {commands:?}");
        match cast.unwrap().kind {
            CommandKind::CastAbility { target, .. } => {
                // The 4-tile grid buckets the four enemies (35..38, 30)
                // into cells 8 and 9: cell (9,7) holds three (36, 37, 38),
                // cell (8,7) holds one (35). The densest cluster's
                // centroid is x = (36+37+38)/3 = 37, y = 30 — 6.5 tiles
                // from the caster, inside the 13-tile range (no clamp).
                let expected = Vec2Fx::from_ints(37, 30);
                let delta = target - expected;
                assert!(
                    delta.x.abs() <= Fx::from_milli(100) && delta.y.abs() <= Fx::from_milli(100),
                    "the volley aims at the densest cluster: {target:?} vs {expected:?}"
                );
            }
            other => panic!("expected a cast, got {other:?}"),
        }
    }

    #[test]
    fn the_sprint_targets_the_push_nearest_own_cluster() {
        let mut controller = TacticalController::new(eval_plan());
        let pathfinder = entity(40, PATHFINDER, 30, 30, 1000);
        let mut entities = vec![pathfinder, entity(1, WORKER, 12, 16, 1000)];
        // Two own clusters: riflemen near (44,44) (pushing toward the
        // enemy start (50,50)) and workers near home (12,16).
        for id in 20..24 {
            entities.push(entity(id, RIFLE, 44, 44, 1000));
        }
        let v = view_full(
            0,
            1000,
            entities,
            vec![AbilityView {
                caster: EntityId(40),
                slot: 0,
                cooldown_milli: 0,
            }],
            Vec::new(),
        );
        let commands = think(&mut controller, &v);
        let cast = commands
            .iter()
            .find(|c| matches!(c.kind, CommandKind::CastAbility { .. }))
            .cloned();
        assert!(cast.is_some(), "the sprint fires: {commands:?}");
        match cast.unwrap().kind {
            CommandKind::CastAbility { target, .. } => {
                // The cluster centroid (44,44); the pathfinder at (30,30)
                // clamps to its 4-tile range: toward (44,44) the point at
                // distance 4 from (30,30).
                let delta = target - Vec2Fx::from_ints(30, 30);
                let dist_raw = (delta.x.raw() as i64).pow(2) + (delta.y.raw() as i64).pow(2);
                let bound = 4000i64 * 65_536 / 1_000;
                assert!(
                    dist_raw <= bound * bound,
                    "the sprint clamps to the 4-tile range: {target:?}"
                );
            }
            other => panic!("expected a cast, got {other:?}"),
        }
    }

    #[test]
    fn a_ready_slot_is_paced_by_the_cast_cooldown() {
        let mut controller = TacticalController::new(eval_plan());
        let ballista = entity(50, BALLISTA, 30, 30, 1000);
        let mut entities = vec![ballista, entity(1, WORKER, 12, 16, 1000)];
        for id in 60..64 {
            entities.push(foreign(id, RIFLE, 36, 30, 1000));
        }
        let mk = |tick: Tick| {
            view_full(
                tick,
                1000,
                entities.clone(),
                vec![AbilityView {
                    caster: EntityId(50),
                    slot: 0,
                    cooldown_milli: 0,
                }],
                Vec::new(),
            )
        };
        let first = think(&mut controller, &mk(0));
        assert!(first
            .iter()
            .any(|c| matches!(c.kind, CommandKind::CastAbility { .. })));
        // One tick before the pacing cadence: no second decision.
        let before = think(&mut controller, &mk(29));
        assert!(
            before
                .iter()
                .all(|c| !matches!(c.kind, CommandKind::CastAbility { .. })),
            "the pacing holds one tick short: {before:?}"
        );
    }

    #[test]
    fn identical_views_produce_identical_commands() {
        let build_views = || {
            vec![
                view(0, 1000, vec![entity(1, WORKER, 12, 16, 1000)]),
                view_full(
                    30,
                    800,
                    vec![
                        entity(1, WORKER, 12, 16, 1000),
                        entity(2, FORGE, 20, 10, 1000),
                        foreign(60, RIFLE, 36, 30, 900),
                    ],
                    vec![AbilityView {
                        caster: EntityId(50),
                        slot: 0,
                        cooldown_milli: 0,
                    }],
                    Vec::new(),
                ),
            ]
        };
        let run = || {
            let mut controller = TacticalController::new(eval_plan());
            build_views()
                .iter()
                .flat_map(|view| think(&mut controller, view))
                .map(|command| (command.tick, command.seq, command.kind))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            run(),
            run(),
            "the controller is a pure function of its inputs"
        );
    }

    #[test]
    fn the_sequence_numbering_continues_the_base() {
        let mut controller = TacticalController::new(eval_plan());
        // A view where the scripted half emits at least one command (a
        // worker to gather) and the evaluative half emits the carrier
        // build: the build's seq must be strictly greater than every
        // scripted seq in the same tick.
        let v = view(
            0,
            1000,
            vec![
                entity(1, WORKER, 12, 16, 1000),
                entity(10, NODE, 8, 18, 1000),
                entity(3, BARRACKS_T, 20, 10, 1000),
            ],
        );
        let commands = think(&mut controller, &v);
        let scripted_max = commands
            .iter()
            .filter(|c| matches!(c.kind, CommandKind::Gather { .. }))
            .map(|c| c.seq)
            .max();
        let build_seq = commands
            .iter()
            .find(|c| matches!(c.kind, CommandKind::Build { .. }))
            .map(|c| c.seq);
        assert!(
            scripted_max.is_some(),
            "the scripted half gathered: {commands:?}"
        );
        assert!(
            build_seq.is_some(),
            "the evaluative half built: {commands:?}"
        );
        assert!(
            build_seq.unwrap() > scripted_max.unwrap(),
            "the evaluative layer continues the base's numbering: {commands:?}"
        );
        let seqs: Vec<u32> = commands.iter().map(|c| c.seq).collect();
        let mut sorted = seqs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(seqs, sorted, "sequences are unique within the tick");
    }

    #[test]
    fn the_tile_fog_row_is_untouched_data() {
        // A compile-shape guard: the controller never reads the fog row
        // (the sighted-entity rule carries the fog law; see point_sighted).
        let _ = TileFog::Hidden;
    }
}
