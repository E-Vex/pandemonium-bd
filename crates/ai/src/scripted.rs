//! The scripted Alpha opponent (plan §9.6): a deterministic controller that runs
//! the whole game through commands — gathering, building, training, defending,
//! and attacking — while seeing nothing but its fog-filtered
//! [`PlayerView`](pandemonium_sim_api::PlayerView).
//!
//! Behavior (plan §9.6, "Alpha behavior"): a scripted build order (workers →
//! supply → barracks → mixed army), expansion-free, attack waves on timers and
//! army-size thresholds, defense when its base is attacked.
//!
//! ## The knowledge boundary (FD-7, FD-8)
//!
//! Everything about *live entities* — positions, ownership, health, even their
//! existence — flows through the view, fog-filtered: own entities always,
//! others only inside friendly vision radii. Everything else the plan carries is
//! public knowledge a player reads off the screen before the match starts: the
//! kind ids of its faction's roster, costs and build times from the command
//! card, its own start position, the enemy start, and candidate build ground.
//! Fog hides *entities*, never terrain or prices.
//!
//! ## Acting blind, verifying by sight
//!
//! The controller cannot see rejections (the plan's `Controller` trait receives
//! no events), so every order it issues is followed by a sight check through
//! the next views: a `Build` succeeds exactly when a construction site of that
//! kind appears near the chosen spot (sites spawn on the spot the moment the
//! command applies), a `Train` succeeds when a new unit of that kind appears in
//! its entity list. Failed orders are retried on fixed cooldowns and failed
//! placements advance to the next candidate spot — self-healing without any
//! channel back into simulation state.
//!
//! ## Determinism
//!
//! The controller is a pure function of (view, tick, its own state); the only
//! randomness is the seeded `fx::Rng` constructed from the match seed (plan
//! §9.6: "seeded RNG passed in, never its own entropy"), consumed in a fixed
//! decision order. Two controllers seeded alike and fed the same views emit the
//! same commands, byte for byte.

use pandemonium_fx::{Fx, Rng, Vec2Fx};
use pandemonium_sim_api::{
    Command, CommandKind, EntityId, EntityView, KindId, MoveState, PlayerId, PlayerView,
    ResourceId, Tick, TilePos,
};

use crate::Controller;

/// A cost list in the ledger's own shape: what a player sees on a command card.
pub type Cost = Vec<(ResourceId, i64)>;

/// The scripted build order's tuning. The script is code, not data (plan §10.1:
/// content parameterizes systems; it does not script AI) — these numbers shape
/// the *opponent*, not the rules.
///
/// Pacing: ten workers keep a Legion base fed (~12 Ore per second); supply
/// blocks are prevented by the [`DEPOT_HEADROOM_TRIGGER`] margin; the first
/// wave marches when the barracks has fielded it, with an 80-second
/// timer pressure trigger for smaller attack groups (and, while a wave is
/// out, the same cadence re-issues the march — a live wave never parks).
/// M9's closing tuning: waves of ten (with the round-robin mix that is
/// roughly one Guardian per three riflemen) carry enough sustained damage to
/// level a defended base in one successful press, and the army cap leaves
/// headroom for a decisive second wave — the M8 flagship stalled with waves
/// of six trading forever in low-count attrition cycles that never
/// accumulated killing power.
/// The worker count the script grows the economy to before pausing worker
/// training. Pairs with `BARRACKS_MIN_WORKERS` (the army-commit threshold)
/// so the economy reaches a self-sustaining throughput before the army
/// queue opens. See the block comment above for the pacing rationale.
const WORKER_TARGET: u32 = 10;
/// The hard cap on the army size — when reached, the script pauses training
/// until attrition opens a slot. Sized (M9) so a wave of `WAVE_SIZE` plus a
/// reserve can both fit, leaving headroom for a decisive second wave.
const ARMY_CAP: u32 = 16;
/// The number of army units a wave marches with when the full army is ready.
/// M9's tuning: ten (with the round-robin Guardian/rifleman mix) carries
/// enough sustained damage to level a defended base in one successful press.
const WAVE_SIZE: u32 = 10;
/// The minimum army size that may march — below this, only the
/// `WAVE_TIMER_TICKS` pressure cadence sends a wave, and even then it is a
/// harassment poke rather than a committed attack.
const MIN_WAVE: u32 = 4;
/// The pressure cadence: if a wave has not marched by this many ticks (80 s
/// at 30 Hz), the script sends whatever army it has at or above `MIN_WAVE`.
/// While a live wave is out, the same cadence re-issues the march — a wave
/// never parks.
const WAVE_TIMER_TICKS: u32 = 2400;
/// The number of supply-providing depots the script grows to before pausing
/// depot construction. Pairs with `DEPOT_HEADROOM_TRIGGER` to keep supply
/// blocks from stalling the worker and army queues.
const DEPOT_TARGET: u32 = 3;
/// When the supply headroom (cap − usage) drops to this many slots, the
/// script begins the next depot even if `DEPOT_TARGET` has not been reached
/// — keeps queues flowing instead of stalling on a supply block.
const DEPOT_HEADROOM_TRIGGER: u32 = 3;
/// The worker count at or above which the script will commit to building the
/// first barracks. Below this, the economy cannot sustain both worker
/// training and army production simultaneously.
const BARRACKS_MIN_WORKERS: u32 = 6;
/// Per-producer cooldown between Train commands, in ticks. Prevents the
/// script from queueing every unit in one frame (which would bankrupt the
/// ledger and starve other producers).
const TRAIN_COOLDOWN_TICKS: u32 = 30;
/// Per-builder cooldown between Build commands, in ticks. Same role as
/// `TRAIN_COOLDOWN_TICKS` for construction — keeps one worker from
/// committing to more sites than it can reach in a reasonable window.
const BUILD_COOLDOWN_TICKS: u32 = 45;
/// Per-army cooldown between defense dispatches, in ticks. Throttles the
/// reaction to intrusions so a single raider does not pull the whole army
/// home and abandon the press.
const DEFENSE_COOLDOWN_TICKS: u32 = 45;
/// The radius (in milli-tiles) around the home base that the defense check
/// scans for sighted enemy units. Sized to cover the worker line and the
/// depot cluster without reaching the map center (which would over-react
/// to neutral scouting).
const DEFENSE_RADIUS_MILLI: i32 = 12000;
/// When a pending Train/Build order is older than this many ticks, the
/// script assumes the producer died (or the site was destroyed) and
/// re-issues. Prevents the bookkeeping from waiting forever on a ghost
/// pending slot.
const PENDING_SLACK_TICKS: u32 = 90;
/// When a Build order's site has not yet appeared in the script's fog view,
/// allow this many ticks of grace before treating the build as failed.
/// Covers the latency between the command being accepted and the
/// structure's footprint becoming visible to the controller's own sight.
const SITE_SIGHT_GRACE_TICKS: u32 = 2;

/// One player's scripted-opponent configuration: the content- and map-derived
/// facts the controller needs, resolved by the host (never name-matched here —
/// the ids arrive as data).
///
/// The host derives this from the loaded content (kind ids by capability
/// shape, costs and build times from the entity definitions, start positions
/// from the map) and hands it to [`ScriptedController::new`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AiPlan {
    /// The slot this controller drives.
    pub player: PlayerId,
    /// The controller's RNG seed — derived from the match seed by the host, so
    /// the controller never owns entropy (plan §9.6).
    pub rng_seed: u64,
    /// The worker kind id (gathers, builds).
    pub worker: KindId,
    /// The economy producer kind id (trains workers).
    pub command_center: KindId,
    /// The population-provider structure kind id, when the content has one.
    pub depot: Option<KindId>,
    /// The army producer structure kind id, when the content has one.
    pub barracks: Option<KindId>,
    /// The gatherable resource node kind id, when the content has one.
    pub node: Option<KindId>,
    /// The army composition: the producer's producible combat kinds, in
    /// production-list order. Trained round-robin.
    pub army: Vec<KindId>,
    /// The worker's cost.
    pub worker_cost: Cost,
    /// The depot structure's cost.
    pub depot_cost: Cost,
    /// The barracks structure's cost.
    pub barracks_cost: Cost,
    /// The worker's build time in ticks (pending-order expiry pacing).
    pub worker_build_ticks: u32,
    /// The army kinds' costs, parallel to [`AiPlan::army`].
    pub army_cost: Vec<Cost>,
    /// The army kinds' build times in ticks, parallel to [`AiPlan::army`].
    pub army_build_ticks: Vec<u32>,
    /// The army kinds' population costs, parallel to [`AiPlan::army`].
    pub army_pop: Vec<u32>,
    /// The worker's population cost.
    pub worker_pop: u32,
    /// This player's start position (map knowledge; the defense anchor).
    pub home: Vec2Fx,
    /// The enemy start position — the attack-wave destination, when the map
    /// declares another player's start.
    pub enemy_start: Option<Vec2Fx>,
    /// Candidate depot placements, nearest first (map knowledge).
    pub depot_spots: Vec<TilePos>,
    /// Candidate barracks placements, nearest first (map knowledge).
    pub barracks_spots: Vec<TilePos>,
}

/// A training order in flight: issued, not yet sighted as a spawned unit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PendingTrain {
    /// What is being trained.
    kind: KindId,
    /// The tick after which the order is considered lost (rejected or the
    /// unit died before sighting) and the slot frees for a retry.
    expire_at: Tick,
}

/// A build order in flight: issued, not yet confirmed by sight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct BuildAttempt {
    /// The structure kind being placed.
    kind: KindId,
    /// When the command was issued.
    issued_at: Tick,
    /// How many own entities of `kind` the view held at issue time — a
    /// successful placement raises the count (sites spawn on the spot),
    /// everything else leaves it untouched.
    count_at_issue: u32,
}

/// The attack-wave state machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WaveState {
    /// Growing the army at home.
    Massing,
    /// The wave has marched; `target` is where it was sent.
    Attacking { target: Vec2Fx },
}

/// The scripted Alpha opponent (plan §9.6).
///
/// Construct it per player from an [`AiPlan`], then drive [`Controller::think`]
/// once per tick boundary on the boundary's fog-filtered view. The controller
/// emits commands only; the shared validation gate judges them exactly like a
/// player's (FD-2, FD-7).
pub struct ScriptedController {
    plan: AiPlan,
    rng: Rng,
    /// Per-tick sequence counter: restarted every think so sequences never
    /// repeat within a tick (the gate's duplicate rule).
    seq: u32,
    /// Own entity ids ever sighted — new ids are spawn sightings.
    seen: Vec<EntityId>,
    /// Training orders in flight.
    pending: Vec<PendingTrain>,
    /// The build order in flight (one at a time: a second placement the same
    /// tick would steal the builder from the first site).
    build: Option<BuildAttempt>,
    build_cooldown_until: Tick,
    /// The next candidate spot per structure kind.
    depot_spot: usize,
    barracks_spot: usize,
    /// The round-robin cursor over the army composition.
    army_cursor: usize,
    worker_train_cooldown_until: Tick,
    army_train_cooldown_until: Tick,
    wave: WaveState,
    next_wave_timer: Tick,
    last_defense: Option<Tick>,
}

impl ScriptedController {
    /// Builds the controller for a plan. The plan's `rng_seed` must derive from
    /// the match seed (the host's job) so the controller stays deterministic.
    pub fn new(plan: AiPlan) -> Self {
        let rng = Rng::seeded(plan.rng_seed);
        Self {
            plan,
            rng,
            seq: 0,
            seen: Vec::new(),
            pending: Vec::new(),
            build: None,
            build_cooldown_until: 0,
            depot_spot: 0,
            barracks_spot: 0,
            army_cursor: 0,
            worker_train_cooldown_until: 0,
            army_train_cooldown_until: 0,
            wave: WaveState::Massing,
            next_wave_timer: WAVE_TIMER_TICKS,
            last_defense: None,
        }
    }

    /// The plan this controller runs (for hosts and tests).
    pub fn plan(&self) -> &AiPlan {
        &self.plan
    }

    /// Appends one command with the next sequence number.
    fn emit(&mut self, out: &mut Vec<Command>, tick: Tick, kind: CommandKind) {
        self.seq += 1;
        out.push(Command::new(self.plan.player, tick, self.seq, kind));
    }
}

impl Controller for ScriptedController {
    /// One tick of the opponent: perceive the view, update bookkeeping, then
    /// decide and act in a fixed order — gather management, build management,
    /// worker training, army training, defense, waves. The order is part of
    /// the controller's determinism contract.
    fn think(&mut self, view: &PlayerView, tick: Tick, out: &mut Vec<Command>) {
        debug_assert_eq!(
            view.player, self.plan.player,
            "a controller must be fed its own view"
        );
        self.seq = 0;
        let player = self.plan.player;

        // ---- Perceive: partition the (fog-filtered) entities. ----
        let own: Vec<EntityView> = view
            .entities
            .iter()
            .filter(|entity| entity.owner == player)
            .copied()
            .collect();
        let workers: Vec<EntityView> = own
            .iter()
            .filter(|entity| entity.kind == self.plan.worker)
            .copied()
            .collect();
        let army: Vec<EntityView> = own
            .iter()
            .filter(|entity| self.plan.army.contains(&entity.kind))
            .copied()
            .collect();
        let cc = own
            .iter()
            .find(|entity| entity.kind == self.plan.command_center)
            .copied();
        let barracks = self
            .plan
            .barracks
            .and_then(|kind| own.iter().find(|entity| entity.kind == kind).copied());
        let depot_count = self
            .plan
            .depot
            .map(|kind| own.iter().filter(|entity| entity.kind == kind).count() as u32)
            .unwrap_or(0);
        let barracks_count = self
            .plan
            .barracks
            .map(|kind| own.iter().filter(|entity| entity.kind == kind).count() as u32)
            .unwrap_or(0);
        let nodes: Vec<EntityView> = self
            .plan
            .node
            .map(|kind| {
                view.entities
                    .iter()
                    .filter(|entity| entity.kind == kind)
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        let enemies: Vec<EntityView> = view
            .entities
            .iter()
            .filter(|entity| entity.owner != player && entity.owner != PlayerId::NEUTRAL)
            .copied()
            .collect();

        // ---- Bookkeeping: spawn sightings retire pending trains; time
        //      expires the ones that never materialized (a rejection or a
        //      spawn-and-death between thinks). ----
        for entity in &own {
            if !self.seen.contains(&entity.id) {
                self.seen.push(entity.id);
                if let Some(index) = self.pending.iter().position(|p| p.kind == entity.kind) {
                    self.pending.remove(index);
                }
            }
        }
        self.pending.retain(|p| p.expire_at > tick);

        // ---- Gather management: an idle worker (no orders at all —
        //      `MoveState::Idle` means an empty order queue) is sent to the
        //      nearest nodes to home, round-robin. Workers mid-gather or
        //      mid-build carry orders and are never touched, so a re-issue can
        //      never cancel work in progress. ----
        if !nodes.is_empty() {
            let mut sorted_nodes = nodes;
            sorted_nodes.sort_by_key(|node| (node.pos - self.plan.home).len_sq_raw());
            for (index, worker) in workers.iter().enumerate() {
                if worker.move_state != MoveState::Idle {
                    continue;
                }
                let node = sorted_nodes[index % sorted_nodes.len()];
                self.emit(
                    out,
                    tick,
                    CommandKind::Gather {
                        units: vec![worker.id],
                        node: node.id,
                    },
                );
            }
        }

        // ---- Build management: confirm the in-flight attempt by sight — a
        //      successful `Build` raises the count of own entities of that
        //      kind (the site spawns on the spot the moment the command
        //      applies); a failed one leaves the count untouched — then start
        //      the next structure when its trigger fires. One attempt at a
        //      time: a second placement the same tick would steal the
        //      builder from the first site. ----
        if let Some(attempt) = self.build {
            if tick.saturating_sub(attempt.issued_at) >= SITE_SIGHT_GRACE_TICKS {
                let count_now = own
                    .iter()
                    .filter(|entity| entity.kind == attempt.kind)
                    .count() as u32;
                if count_now <= attempt.count_at_issue {
                    // The placement failed (blocked ground the controller
                    // could not see): cool down before the next attempt.
                    // The candidate list itself was already consumed by the
                    // `next_spot` call, so the retry lands on the next spot.
                    self.build_cooldown_until = tick + BUILD_COOLDOWN_TICKS;
                }
                // Either way the attempt is settled: the site exists (the
                // builder is committed through its own BuildAt order, freed by
                // the idle check when construction pops it) or it never will.
                self.build = None;
            }
        }
        if self.build.is_none() && tick >= self.build_cooldown_until {
            let headroom = view.population_cap.saturating_sub(view.population);
            let mut started = false;
            if let Some(kind) = self.plan.depot {
                if depot_count < DEPOT_TARGET
                    && headroom < DEPOT_HEADROOM_TRIGGER
                    && can_afford(view, &self.plan.depot_cost)
                {
                    started = self.start_build(kind, SpotKind::Depot, &own, &workers, tick, out);
                }
            }
            if !started {
                if let Some(kind) = self.plan.barracks {
                    if barracks_count == 0
                        && (depot_count >= 1 || workers.len() as u32 >= BARRACKS_MIN_WORKERS)
                        && can_afford(view, &self.plan.barracks_cost)
                    {
                        self.start_build(kind, SpotKind::Barracks, &own, &workers, tick, out);
                    }
                }
            }
        }

        // ---- Worker training: keep the economy growing while the ledger and
        //      population headroom allow. ----
        if tick >= self.worker_train_cooldown_until {
            let pending_workers = self
                .pending
                .iter()
                .filter(|p| p.kind == self.plan.worker)
                .count() as u32;
            if let Some(cc) = cc {
                if workers.len() as u32 + pending_workers < WORKER_TARGET
                    && can_afford(view, &self.plan.worker_cost)
                    && view.population_cap.saturating_sub(view.population) >= self.plan.worker_pop
                {
                    self.emit(
                        out,
                        tick,
                        CommandKind::Train {
                            producer: cc.id,
                            unit: self.plan.worker,
                        },
                    );
                    self.pending.push(PendingTrain {
                        kind: self.plan.worker,
                        expire_at: tick
                            + self.plan.worker_build_ticks.saturating_mul(2)
                            + PENDING_SLACK_TICKS,
                    });
                    self.worker_train_cooldown_until = tick + TRAIN_COOLDOWN_TICKS;
                }
            }
        }

        // ---- Army training: round-robin the composition from the barracks
        //      once it exists (an under-construction barracks rejects the
        //      order silently; the pending expiry retries it). ----
        if tick >= self.army_train_cooldown_until {
            if let Some(barracks) = barracks {
                let pending_army = self
                    .pending
                    .iter()
                    .filter(|p| self.plan.army.contains(&p.kind))
                    .count() as u32;
                if army.len() as u32 + pending_army < ARMY_CAP && !self.plan.army.is_empty() {
                    let index = self.army_cursor % self.plan.army.len();
                    let kind = self.plan.army[index];
                    let headroom = view.population_cap.saturating_sub(view.population);
                    let pop = self.plan.army_pop.get(index).copied().unwrap_or(1);
                    let cost = self.plan.army_cost.get(index).cloned().unwrap_or_default();
                    if can_afford(view, &cost) && headroom >= pop {
                        self.emit(
                            out,
                            tick,
                            CommandKind::Train {
                                producer: barracks.id,
                                unit: kind,
                            },
                        );
                        let build_ticks =
                            self.plan.army_build_ticks.get(index).copied().unwrap_or(0);
                        self.pending.push(PendingTrain {
                            kind,
                            expire_at: tick + build_ticks.saturating_mul(2) + PENDING_SLACK_TICKS,
                        });
                        self.army_cursor = self.army_cursor.wrapping_add(1);
                        self.army_train_cooldown_until = tick + TRAIN_COOLDOWN_TICKS;
                    }
                }
            }
        }

        // ---- Defense: any enemy sighted inside the base radius pulls the
        //      whole army onto the nearest intruder, on a fixed cooldown.
        //      (The intruder is in the view, so the Attack gate's visibility
        //      check passes by construction.) ----
        if let Some(intruder) = enemies
            .iter()
            .filter(|enemy| within_milli(enemy.pos, self.plan.home, DEFENSE_RADIUS_MILLI))
            .min_by_key(|enemy| (enemy.pos - self.plan.home).len_sq_raw())
        {
            if !army.is_empty()
                && self
                    .last_defense
                    .is_none_or(|last| tick >= last + DEFENSE_COOLDOWN_TICKS)
            {
                self.emit(
                    out,
                    tick,
                    CommandKind::Attack {
                        units: army.iter().map(|unit| unit.id).collect(),
                        target: intruder.id,
                    },
                );
                self.last_defense = Some(tick);
            }
        }

        // ---- Attack waves: fire on army size or the pressure timer, press on
        //      the pressure cadence while attacking, and regroup when the
        //      wave is spent. ----
        let army_ids: Vec<EntityId> = army.iter().map(|unit| unit.id).collect();
        match self.wave {
            WaveState::Massing => {
                let size = army.len() as u32;
                if let Some(enemy_start) = self.plan.enemy_start {
                    if size >= MIN_WAVE && (size >= WAVE_SIZE || tick >= self.next_wave_timer) {
                        let target = self.wave_target(self.hunt_focus(&enemies, enemy_start));
                        self.emit(
                            out,
                            tick,
                            CommandKind::AttackMove {
                                units: army_ids,
                                target,
                            },
                        );
                        if let Some(barracks) = barracks {
                            self.emit(
                                out,
                                tick,
                                CommandKind::SetRally {
                                    producer: barracks.id,
                                    target,
                                },
                            );
                        }
                        self.wave = WaveState::Attacking { target };
                    }
                }
            }
            WaveState::Attacking { .. } => {
                if let Some(enemy_start) = self.plan.enemy_start {
                    if army.len() < MIN_WAVE as usize {
                        // Spent: fewer than three survivors anywhere. Regroup
                        // at home and rebuild.
                        self.wave = WaveState::Massing;
                        self.next_wave_timer = tick + WAVE_TIMER_TICKS;
                        if let Some(barracks) = barracks {
                            self.emit(
                                out,
                                tick,
                                CommandKind::SetRally {
                                    producer: barracks.id,
                                    target: self.plan.home,
                                },
                            );
                        }
                    } else if tick >= self.next_wave_timer {
                        // Still pressing, but the wave has gone quiet: its
                        // march orders drained (failed at a choke, or
                        // completed after the target area was cleared) or
                        // defense pulled the army home after intruders. A
                        // wave that merely *exists* does not press — the
                        // timer re-issues the march at the pressure cadence,
                        // so a live wave never parks. The kill focus is the
                        // enemy's command center when it is in sight (the
                        // elimination target), else the enemy start.
                        let target = self.wave_target(self.hunt_focus(&enemies, enemy_start));
                        if !army_ids.is_empty() {
                            self.emit(
                                out,
                                tick,
                                CommandKind::AttackMove {
                                    units: army_ids,
                                    target,
                                },
                            );
                        }
                        if let Some(barracks) = barracks {
                            self.emit(
                                out,
                                tick,
                                CommandKind::SetRally {
                                    producer: barracks.id,
                                    target,
                                },
                            );
                        }
                        self.next_wave_timer = tick + WAVE_TIMER_TICKS;
                    }
                }
            }
        }
    }
}

/// Which structure kind a candidate spot list belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SpotKind {
    Depot,
    Barracks,
}

impl ScriptedController {
    /// Starts one build: picks the next candidate spot and the nearest worker
    /// to it, emits the `Build` command, and records the attempt against the
    /// view's current count of that structure kind. Returns whether an order
    /// was issued.
    fn start_build(
        &mut self,
        kind: KindId,
        spot_kind: SpotKind,
        own: &[EntityView],
        workers: &[EntityView],
        tick: Tick,
        out: &mut Vec<Command>,
    ) -> bool {
        let Some(spot) = self.next_spot(spot_kind) else {
            return false;
        };
        // Nearest worker to the spot, ties to the lower id (the list is
        // id-ascending, so a strict minimum keeps the first).
        let spot_center = Vec2Fx::from_ints(spot.x, spot.y);
        let Some(worker) = workers
            .iter()
            .min_by_key(|worker| (worker.pos - spot_center).len_sq_raw())
            .copied()
        else {
            return false;
        };
        self.emit(
            out,
            tick,
            CommandKind::Build {
                worker: worker.id,
                structure: kind,
                at: spot,
            },
        );
        let count_at_issue = own.iter().filter(|entity| entity.kind == kind).count() as u32;
        self.build = Some(BuildAttempt {
            kind,
            issued_at: tick,
            count_at_issue,
        });
        true
    }

    /// The next candidate spot for a structure kind. Spots are consumed in
    /// order — a failed placement and a successful one alike move on to the
    /// next candidate (a successful placement occupies its ground, a failed
    /// one was blocked ground).
    fn next_spot(&mut self, spot_kind: SpotKind) -> Option<TilePos> {
        let (spots, cursor) = match spot_kind {
            SpotKind::Depot => (&self.plan.depot_spots, &mut self.depot_spot),
            SpotKind::Barracks => (&self.plan.barracks_spots, &mut self.barracks_spot),
        };
        let index = *cursor % spots.len();
        *cursor = cursor.wrapping_add(1);
        spots.get(index).copied()
    }

    /// The wave's preferred destination: the enemy's command center when it
    /// is in sight (the elimination target — plan §9.7's defeat rule is zero
    /// owned structures, and the CC is the one structure every player starts
    /// with), else the enemy start position (map knowledge). Sight comes from
    /// the fog-filtered view — a hidden CC is not focused.
    fn hunt_focus(&self, enemies: &[EntityView], enemy_start: Vec2Fx) -> Vec2Fx {
        enemies
            .iter()
            .find(|enemy| enemy.kind == self.plan.command_center)
            .map(|cc| cc.pos)
            .unwrap_or(enemy_start)
    }

    /// The wave's destination: the focus position plus a seeded per-wave
    /// jitter, so marching blobs do not funnel onto one exact tile (the M4
    /// crowd lesson). The only randomness the controller uses.
    fn wave_target(&mut self, focus: Vec2Fx) -> Vec2Fx {
        let jx = self.rng.bounded(1201) as i32 - 600;
        let jy = self.rng.bounded(1201) as i32 - 600;
        focus + Vec2Fx::new(Fx::from_milli(jx), Fx::from_milli(jy))
    }
}

/// Whether two positions sit within `radius_milli` milli-tiles of each other
/// (squared-distance comparison — plan §5.8).
fn within_milli(a: Vec2Fx, b: Vec2Fx, radius_milli: i32) -> bool {
    // Milli-tiles to the Q16.16 raw unit: * 65,536 / 1,000.
    let radius_raw = (radius_milli as i64) * 65_536 / 1_000;
    let radius_sq = (radius_raw * radius_raw) as u64;
    (a - b).len_sq_raw() <= radius_sq
}

/// Whether the player's ledger covers a cost list.
fn can_afford(view: &PlayerView, cost: &[(ResourceId, i64)]) -> bool {
    cost.iter().all(|(resource, amount)| {
        *amount <= 0
            || view
                .resources
                .iter()
                .any(|held| held.resource == *resource && held.amount >= *amount)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_sim_api::ViewResource;

    const WORKER: KindId = KindId(0);
    const DEPOT: KindId = KindId(1);
    const BARRACKS: KindId = KindId(2);
    const CC: KindId = KindId(3);
    const NODE: KindId = KindId(4);
    const RIFLE: KindId = KindId(5);
    const RAIDER: KindId = KindId(6);

    fn plan() -> AiPlan {
        AiPlan {
            player: PlayerId(0),
            rng_seed: 7,
            worker: WORKER,
            command_center: CC,
            depot: Some(DEPOT),
            barracks: Some(BARRACKS),
            node: Some(NODE),
            army: vec![RIFLE, RAIDER],
            worker_cost: vec![(ResourceId(0), 50)],
            depot_cost: vec![(ResourceId(0), 100)],
            barracks_cost: vec![(ResourceId(0), 150)],
            army_cost: vec![vec![(ResourceId(0), 75)], vec![(ResourceId(0), 60)]],
            worker_build_ticks: 360,
            army_build_ticks: vec![300, 240],
            army_pop: vec![1, 1],
            worker_pop: 1,
            home: Vec2Fx::from_ints(14, 14),
            enemy_start: Some(Vec2Fx::from_ints(50, 50)),
            depot_spots: vec![TilePos { x: 17, y: 11 }, TilePos { x: 16, y: 16 }],
            barracks_spots: vec![TilePos { x: 20, y: 10 }, TilePos { x: 10, y: 20 }],
        }
    }

    fn entity(id: u64, kind: KindId, x: i32, y: i32, state: MoveState) -> EntityView {
        EntityView {
            id: EntityId(id),
            owner: PlayerId(0),
            kind,
            pos: Vec2Fx::from_ints(x, y),
            facing: Vec2Fx::ZERO,
            hp_fraction_milli: 1000,
            move_state: state,
        }
    }

    fn foreign(id: u64, kind: KindId, x: i32, y: i32) -> EntityView {
        EntityView {
            owner: PlayerId(1),
            ..entity(id, kind, x, y, MoveState::Idle)
        }
    }

    fn view(tick: Tick, ore: i64, pop: u32, cap: u32, entities: Vec<EntityView>) -> PlayerView {
        PlayerView {
            tick,
            player: PlayerId(0),
            resources: vec![ViewResource {
                resource: ResourceId(0),
                amount: ore,
            }],
            population: pop,
            population_cap: cap,
            entities,
        }
    }

    fn think(controller: &mut ScriptedController, view: &PlayerView) -> Vec<Command> {
        let mut out = Vec::new();
        Controller::think(controller, view, view.tick, &mut out);
        out
    }

    /// Ten busy workers (the script's target): a base that trains nothing,
    /// so the behavior under test is the only behavior that can fire.
    fn full_staff(extra: Vec<EntityView>) -> Vec<EntityView> {
        let mut entities = vec![entity(90, CC, 14, 14, MoveState::Idle)];
        for id in 1..=10 {
            entities.push(entity(id, WORKER, 12, 16, MoveState::Moving));
        }
        entities.extend(extra);
        entities
    }

    #[test]
    fn idle_workers_are_sent_gathering_nearest_first() {
        let mut controller = ScriptedController::new(plan());
        // Three nodes around home (14,14): (8,18) is nearest (d^2 = 52),
        // then (18,7) (65), then (7,7) (98).
        let opening = view(
            0,
            0,
            0,
            10,
            vec![
                entity(1, WORKER, 12, 16, MoveState::Idle),
                entity(2, WORKER, 13, 15, MoveState::Idle),
                entity(10, NODE, 7, 7, MoveState::Idle),
                entity(11, NODE, 8, 18, MoveState::Idle),
                entity(12, NODE, 18, 7, MoveState::Idle),
            ],
        );
        let commands = think(&mut controller, &opening);
        let targets: Vec<EntityId> = commands
            .iter()
            .filter_map(|command| match &command.kind {
                CommandKind::Gather { node, .. } => Some(*node),
                _ => None,
            })
            .collect();
        assert_eq!(targets, vec![EntityId(11), EntityId(12)], "{commands:?}");
        // The next view shows both workers carrying orders (the sim applied
        // the commands): nothing re-tasks them.
        let busy = view(
            1,
            0,
            0,
            10,
            vec![
                entity(1, WORKER, 12, 16, MoveState::Moving),
                entity(2, WORKER, 13, 15, MoveState::Moving),
                entity(10, NODE, 7, 7, MoveState::Idle),
                entity(11, NODE, 8, 18, MoveState::Idle),
                entity(12, NODE, 18, 7, MoveState::Idle),
            ],
        );
        let second = think(&mut controller, &busy);
        assert!(
            second.is_empty(),
            "busy workers are never re-tasked: {second:?}"
        );
    }

    #[test]
    fn workers_with_orders_are_left_alone() {
        let mut controller = ScriptedController::new(plan());
        let view = view(
            0,
            0,
            0,
            10,
            vec![
                entity(1, WORKER, 12, 16, MoveState::Moving),
                entity(2, WORKER, 13, 15, MoveState::Idle),
                entity(10, NODE, 7, 7, MoveState::Idle),
            ],
        );
        let commands = think(&mut controller, &view);
        let targets: Vec<EntityId> = commands
            .iter()
            .filter_map(|command| match &command.kind {
                CommandKind::Gather { units, node } if *units == vec![EntityId(2)] => Some(*node),
                _ => None,
            })
            .collect();
        assert_eq!(
            targets,
            vec![EntityId(10)],
            "only the idle one: {commands:?}"
        );
    }

    #[test]
    fn worker_training_respects_target_pending_cooldown_and_affordability() {
        let mut controller = ScriptedController::new(plan());
        // Two busy workers, headroom healthy, no nodes, no barracks: the only
        // behavior that can fire is worker training.
        let world = |tick: Tick, ore: i64, workers: u32| {
            let mut entities = vec![entity(90, CC, 14, 14, MoveState::Idle)];
            for id in 0..workers {
                entities.push(entity(1 + u64::from(id), WORKER, 12, 16, MoveState::Moving));
            }
            view(tick, ore, 4, 10, entities)
        };

        // Affordable and below target: exactly one train per cooldown window.
        let commands = think(&mut controller, &world(0, 200, 2));
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            commands[0].kind,
            CommandKind::Train {
                producer: EntityId(90),
                unit: WORKER
            }
        ));
        assert!(
            think(&mut controller, &world(29, 200, 2)).is_empty(),
            "the cooldown still holds one tick short"
        );
        assert_eq!(
            think(&mut controller, &world(30, 200, 2)).len(),
            1,
            "the cooldown expires at 30 ticks"
        );
        assert_eq!(think(&mut controller, &world(60, 200, 2)).len(), 1);

        // Pending orders occupy the target's remaining slots.
        let mut controller = ScriptedController::new(plan());
        assert_eq!(think(&mut controller, &world(0, 100, 9)).len(), 1);
        assert!(
            think(&mut controller, &world(30, 100, 9)).is_empty(),
            "9 workers + 1 pending = the target: no further orders"
        );

        // Population headroom gates the same order (a thin ledger keeps
        // every other behavior quiet).
        let mut controller = ScriptedController::new(plan());
        let pop_full = view(
            0,
            60,
            10,
            10,
            vec![
                entity(90, CC, 14, 14, MoveState::Idle),
                entity(1, WORKER, 12, 16, MoveState::Moving),
                entity(2, WORKER, 13, 15, MoveState::Moving),
            ],
        );
        assert!(
            think(&mut controller, &pop_full).is_empty(),
            "no headroom means no train"
        );

        // A thin ledger means no order.
        let mut controller = ScriptedController::new(plan());
        assert!(think(&mut controller, &world(0, 10, 2)).is_empty());
    }

    #[test]
    fn pending_train_retires_when_the_unit_appears() {
        let mut controller = ScriptedController::new(plan());
        let world = |tick: Tick, workers: Vec<EntityView>| {
            let mut entities = vec![entity(90, CC, 14, 14, MoveState::Idle)];
            entities.extend(workers);
            view(tick, 200, 4, 10, entities)
        };
        let two = vec![
            entity(1, WORKER, 12, 16, MoveState::Moving),
            entity(2, WORKER, 13, 15, MoveState::Moving),
        ];
        assert_eq!(think(&mut controller, &world(0, two.clone())).len(), 1);
        // The trained worker arrives in view (a new id of the worker kind):
        // the pending retires and the slot reopens.
        let three = vec![
            entity(1, WORKER, 12, 16, MoveState::Moving),
            entity(2, WORKER, 13, 15, MoveState::Moving),
            entity(3, WORKER, 14, 17, MoveState::Moving),
        ];
        let commands = think(&mut controller, &world(30, three));
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0].kind, CommandKind::Train { .. }));
    }

    #[test]
    fn pending_train_expires_and_retries() {
        // Nine workers, one slot from the target: a pending train holds the
        // last slot until it expires (2x360 + 90 = 810).
        let mut controller = ScriptedController::new(plan());
        let world = |tick: Tick| {
            let mut entities = vec![entity(90, CC, 14, 14, MoveState::Idle)];
            for id in 1..=9 {
                entities.push(entity(id, WORKER, 12, 16, MoveState::Moving));
            }
            view(tick, 100, 4, 10, entities)
        };
        assert_eq!(think(&mut controller, &world(0)).len(), 1);
        assert!(think(&mut controller, &world(29)).is_empty(), "cooldown");
        assert!(
            think(&mut controller, &world(809)).is_empty(),
            "the pending holds the last slot"
        );
        assert_eq!(think(&mut controller, &world(810)).len(), 1);
    }

    #[test]
    fn depot_is_built_when_headroom_runs_low() {
        let mut controller = ScriptedController::new(plan());
        // Ten workers (nothing to train), headroom 2 of 10: supply fires.
        let low = view(0, 200, 8, 10, full_staff(Vec::new()));
        let commands = think(&mut controller, &low);
        assert_eq!(commands.len(), 1);
        match &commands[0].kind {
            CommandKind::Build {
                worker,
                structure,
                at,
            } => {
                assert_eq!(*worker, EntityId(1), "the lowest-id worker is nearest");
                assert_eq!(*structure, DEPOT);
                assert_eq!(*at, TilePos { x: 17, y: 11 }, "first candidate spot");
            }
            other => panic!("expected a depot build, got {other:?}"),
        }
        // Healthy headroom: no build.
        let mut controller = ScriptedController::new(plan());
        let rich = view(0, 40, 2, 10, full_staff(Vec::new()));
        assert!(think(&mut controller, &rich).is_empty());
    }

    #[test]
    fn failed_builds_advance_the_candidate_spot() {
        let mut controller = ScriptedController::new(plan());
        let world = |tick: Tick, entities: Vec<EntityView>| view(tick, 200, 8, 10, entities);
        let start = world(0, full_staff(Vec::new()));
        let commands = think(&mut controller, &start);
        assert!(matches!(commands[0].kind, CommandKind::Build { .. }));

        // Two ticks later no site appeared: the attempt settles as failed
        // and a cooldown runs before the retry at the next candidate.
        let sight = world(2, full_staff(Vec::new()));
        assert!(think(&mut controller, &sight).is_empty(), "attempt settles");
        let cooling = world(46, full_staff(Vec::new()));
        assert!(think(&mut controller, &cooling).is_empty(), "cooldown 2+45");
        let retry = world(47, full_staff(Vec::new()));
        let commands = think(&mut controller, &retry);
        assert_eq!(commands.len(), 1);
        match &commands[0].kind {
            CommandKind::Build { at, .. } => assert_eq!(*at, TilePos { x: 16, y: 16 }),
            other => panic!("expected the second spot, got {other:?}"),
        }
    }

    #[test]
    fn successful_builds_retire_the_attempt() {
        let mut controller = ScriptedController::new(plan());
        let world = |tick: Tick, extra: Vec<EntityView>| view(tick, 200, 8, 10, full_staff(extra));
        let commands = think(&mut controller, &world(0, Vec::new()));
        assert!(matches!(commands[0].kind, CommandKind::Build { .. }));

        // The site appears near the spot: the attempt retires with no
        // cooldown, so the same think already starts the next depot (the
        // target allows three) at the consumed second spot.
        let site = world(2, vec![entity(91, DEPOT, 17, 11, MoveState::Idle)]);
        let commands = think(&mut controller, &site);
        assert_eq!(
            commands.len(),
            1,
            "retire + immediate restart: {commands:?}"
        );
        match &commands[0].kind {
            CommandKind::Build { at, .. } => assert_eq!(*at, TilePos { x: 16, y: 16 }),
            other => panic!("expected the consumed next spot, got {other:?}"),
        }
        // The new attempt is in flight: one tick of silence.
        let next = world(3, vec![entity(91, DEPOT, 17, 11, MoveState::Idle)]);
        assert!(think(&mut controller, &next).is_empty());
    }

    #[test]
    fn barracks_follow_the_script_order() {
        // Workers below the script's minimum and no depot: no barracks yet,
        // even with plenty of Ore. The only command that can fire is worker
        // training (5 of 10, affordable, headroom healthy).
        let mut controller = ScriptedController::new(plan());
        let mut five = vec![entity(90, CC, 14, 14, MoveState::Idle)];
        for id in 1..=5 {
            five.push(entity(id, WORKER, 12, 16, MoveState::Moving));
        }
        let early = view(0, 200, 4, 10, five);
        let commands = think(&mut controller, &early);
        assert!(
            commands
                .iter()
                .all(|command| !matches!(command.kind, CommandKind::Build { .. })),
            "no barracks before the script order: {commands:?}"
        );

        // At full worker strength the barracks goes up at its first spot.
        let commands = think(
            &mut controller,
            &view(0, 200, 4, 10, full_staff(Vec::new())),
        );
        assert_eq!(commands.len(), 1);
        match &commands[0].kind {
            CommandKind::Build { structure, at, .. } => {
                assert_eq!(*structure, BARRACKS);
                assert_eq!(*at, TilePos { x: 20, y: 10 });
            }
            other => panic!("expected a barracks build, got {other:?}"),
        }

        // A depot substitutes for the worker count.
        let mut controller = ScriptedController::new(plan());
        let mut five = vec![
            entity(90, CC, 14, 14, MoveState::Idle),
            entity(91, DEPOT, 17, 11, MoveState::Idle),
        ];
        for id in 1..=5 {
            five.push(entity(id, WORKER, 12, 16, MoveState::Moving));
        }
        let supplied = view(0, 200, 8, 20, five);
        let commands = think(&mut controller, &supplied);
        assert!(
            commands.iter().any(|command| matches!(
                &command.kind,
                CommandKind::Build {
                    structure: BARRACKS,
                    ..
                }
            )),
            "a standing depot unlocks the barracks: {commands:?}"
        );
    }

    #[test]
    fn army_trains_round_robin_from_the_barracks() {
        let mut controller = ScriptedController::new(plan());
        let world = |tick: Tick, army: u32| {
            let mut extra = vec![entity(91, BARRACKS, 20, 10, MoveState::Idle)];
            for id in 0..army {
                extra.push(entity(20 + u64::from(id), RIFLE, 15, 15, MoveState::Moving));
            }
            view(tick, 1000, 4, 10, full_staff(extra))
        };
        // The composition cycles rifle, raider, rifle, ...
        let first = think(&mut controller, &world(0, 0));
        assert_eq!(first.len(), 1);
        assert!(matches!(
            first[0].kind,
            CommandKind::Train {
                producer: EntityId(91),
                unit: RIFLE
            }
        ));
        let second = think(&mut controller, &world(30, 0));
        assert_eq!(second.len(), 1);
        assert!(matches!(
            second[0].kind,
            CommandKind::Train {
                producer: EntityId(91),
                unit: RAIDER
            }
        ));
        // Sixteen live army at the cap: no further training — and the full
        // wave marches (which is what an at-cap army exists to do).
        let mut fresh = ScriptedController::new(plan());
        let commands = think(&mut fresh, &world(0, 16));
        assert!(
            commands
                .iter()
                .all(|command| !matches!(command.kind, CommandKind::Train { .. })),
            "army at the cap trains nothing: {commands:?}"
        );
        assert!(commands
            .iter()
            .any(|command| matches!(command.kind, CommandKind::AttackMove { .. })));
    }

    #[test]
    fn defense_attacks_the_nearest_intruder_with_a_cooldown() {
        let mut controller = ScriptedController::new(plan());
        // Two intruders inside the 12-tile radius of home (14,14): (16,15)
        // is nearer than (20,8).
        let under_attack = view(
            0,
            0,
            4,
            10,
            vec![
                entity(30, RIFLE, 15, 15, MoveState::Idle),
                foreign(80, RIFLE, 16, 15),
                foreign(81, RIFLE, 20, 8),
            ],
        );
        let commands = think(&mut controller, &under_attack);
        assert_eq!(commands.len(), 1);
        match &commands[0].kind {
            CommandKind::Attack { units, target } => {
                assert_eq!(*target, EntityId(80), "the nearest intruder");
                assert_eq!(*units, vec![EntityId(30)]);
            }
            other => panic!("expected a defense order, got {other:?}"),
        }
        // Inside the cooldown: no re-issue even as ticks pass.
        let mut again = under_attack.clone();
        again.tick = 44;
        assert!(think(&mut controller, &again).is_empty());
        // After the cooldown: the order re-fires.
        let mut later = under_attack.clone();
        later.tick = 45;
        assert_eq!(think(&mut controller, &later).len(), 1);

        // An intruder outside the radius is ignored.
        let mut passive = ScriptedController::new(plan());
        let far = view(
            0,
            0,
            4,
            10,
            vec![
                entity(30, RIFLE, 15, 15, MoveState::Idle),
                foreign(82, RIFLE, 40, 40),
            ],
        );
        assert!(think(&mut passive, &far).is_empty());
    }

    #[test]
    fn waves_fire_on_size_and_regroup_when_spent() {
        let world = |tick: Tick, army: Vec<EntityView>| view(tick, 0, 4, 10, army);
        // Nine units (below WAVE_SIZE) before the timer: nothing marches.
        let mut controller = ScriptedController::new(plan());
        let mut five = vec![entity(90, CC, 14, 14, MoveState::Idle)];
        for id in 0..9 {
            five.push(entity(20 + id, RIFLE, 15, 15, MoveState::Idle));
        }
        assert!(think(&mut controller, &world(0, five)).is_empty());

        // The full wave size: the army attacks toward the enemy start, and
        // the barracks rally follows it.
        let mut six = vec![
            entity(90, CC, 14, 14, MoveState::Idle),
            entity(91, BARRACKS, 20, 10, MoveState::Idle),
        ];
        for id in 0..10 {
            six.push(entity(20 + id, RIFLE, 15, 15, MoveState::Idle));
        }
        let attacking = world(0, six.clone());
        let commands = think(&mut controller, &attacking);
        assert_eq!(commands.len(), 2, "AttackMove + SetRally: {commands:?}");
        match &commands[0].kind {
            CommandKind::AttackMove { units, target } => {
                assert_eq!(units.len(), 10);
                let delta = *target - Vec2Fx::from_ints(50, 50);
                let bound = Fx::from_milli(600);
                assert!(
                    delta.x.abs() <= bound && delta.y.abs() <= bound,
                    "the wave jitter stays within +-600 milli-tiles: {delta:?}"
                );
            }
            other => panic!("expected an attack move, got {other:?}"),
        }
        assert!(matches!(
            commands[1].kind,
            CommandKind::SetRally {
                producer: EntityId(91),
                ..
            }
        ));

        // The wave is spent (fewer than MIN_WAVE survivors — two is below
        // four): regroup, rally home, and the timer restarts.
        let survivors = world(
            60,
            vec![
                entity(90, CC, 14, 14, MoveState::Idle),
                entity(91, BARRACKS, 20, 10, MoveState::Idle),
                entity(20, RIFLE, 40, 40, MoveState::Idle),
                entity(21, RIFLE, 41, 41, MoveState::Idle),
            ],
        );
        let commands = think(&mut controller, &survivors);
        assert_eq!(commands.len(), 1, "only the rally home: {commands:?}");
        assert!(matches!(
            commands[0].kind,
            CommandKind::SetRally { ref target, .. } if *target == Vec2Fx::from_ints(14, 14)
        ));
    }

    /// M9's stall fix: a wave that is still alive but has gone quiet (its
    /// march orders drained — failed at a choke, or completed after the
    /// target area was cleared — or defense pulled the army home) re-issues
    /// the march on the pressure cadence. A wave that merely exists does not
    /// press; the timer makes it press again. The march aims at the enemy
    /// command center when it is in sight (the elimination target).
    #[test]
    fn a_live_wave_re_marches_when_its_timer_expires() {
        let world = |tick: Tick, entities: Vec<EntityView>| view(tick, 0, 4, 40, entities);
        let mut controller = ScriptedController::new(plan());
        // Fire the wave: ten riflemen at home, tick 0.
        let mut marching = vec![
            entity(90, CC, 14, 14, MoveState::Idle),
            entity(91, BARRACKS, 20, 10, MoveState::Idle),
        ];
        for id in 0..10 {
            marching.push(entity(20 + id, RIFLE, 15, 15, MoveState::Idle));
        }
        let fired = think(&mut controller, &world(0, marching.clone()));
        assert_eq!(fired.len(), 2, "AttackMove + SetRally: {fired:?}");

        // Mid-press (before the cadence): the quiet wave — every unit idle,
        // its march orders long gone — is NOT re-tasked.
        let quiet = world(1000, marching.clone());
        assert!(think(&mut controller, &quiet).is_empty());

        // The cadence expires with the wave alive: the march re-issues for
        // the whole army, and the rally follows it. The enemy CC is in
        // sight at (48, 47), so the march aims there (hunt focus), not at
        // the enemy start (50, 50).
        let mut stalled = vec![
            entity(90, CC, 14, 14, MoveState::Idle),
            entity(91, BARRACKS, 20, 10, MoveState::Idle),
            foreign(95, CC, 48, 47),
        ];
        for id in 0..10 {
            stalled.push(entity(20 + id, RIFLE, 30, 30, MoveState::Idle));
        }
        let commands = think(&mut controller, &world(2400, stalled));
        assert_eq!(commands.len(), 2, "AttackMove + SetRally: {commands:?}");
        match &commands[0].kind {
            CommandKind::AttackMove { units, target } => {
                assert_eq!(units.len(), 10, "the whole living army re-marches");
                let delta = *target - Vec2Fx::from_ints(48, 47);
                let bound = Fx::from_milli(600);
                assert!(
                    delta.x.abs() <= bound && delta.y.abs() <= bound,
                    "the re-march aims at the sighted enemy CC: {delta:?}"
                );
            }
            other => panic!("expected an attack move, got {other:?}"),
        }
        assert!(matches!(
            commands[1].kind,
            CommandKind::SetRally {
                producer: EntityId(91),
                ..
            }
        ));
    }

    #[test]
    fn waves_fire_on_the_timer_with_a_minimum_force() {
        let mut controller = ScriptedController::new(plan());
        // Two units before the timer: no wave.
        let early = view(
            0,
            0,
            4,
            10,
            vec![
                entity(90, CC, 14, 14, MoveState::Idle),
                entity(20, RIFLE, 15, 15, MoveState::Idle),
                entity(21, RIFLE, 16, 15, MoveState::Idle),
            ],
        );
        assert!(think(&mut controller, &early).is_empty());
        // At the timer, four units (MIN_WAVE) march.
        let on_time = view(
            2400,
            0,
            4,
            10,
            vec![
                entity(90, CC, 14, 14, MoveState::Idle),
                entity(20, RIFLE, 15, 15, MoveState::Idle),
                entity(21, RIFLE, 16, 15, MoveState::Idle),
                entity(22, RIFLE, 15, 16, MoveState::Idle),
                entity(23, RIFLE, 16, 16, MoveState::Idle),
            ],
        );
        let commands = think(&mut controller, &on_time);
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0].kind, CommandKind::AttackMove { .. }));
    }

    #[test]
    fn sequences_restart_each_tick_and_stay_unique_within_one() {
        let mut controller = ScriptedController::new(plan());
        let view = PlayerView {
            tick: 0,
            player: PlayerId(0),
            resources: Vec::new(),
            population: 4,
            population_cap: 10,
            entities: vec![
                entity(1, WORKER, 12, 16, MoveState::Idle),
                entity(2, WORKER, 13, 15, MoveState::Idle),
                entity(10, NODE, 7, 7, MoveState::Idle),
            ],
        };
        let commands = think(&mut controller, &view);
        assert!(!commands.is_empty());
        let seqs: Vec<u32> = commands.iter().map(|command| command.seq).collect();
        let mut sorted = seqs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(seqs, sorted, "sequences are unique within the tick");
    }

    #[test]
    fn identical_views_produce_identical_commands() {
        let build_views = || {
            vec![
                view(
                    0,
                    200,
                    8,
                    10,
                    full_staff(vec![entity(10, NODE, 7, 7, MoveState::Idle)]),
                ),
                view(
                    1,
                    150,
                    8,
                    10,
                    full_staff(vec![
                        entity(10, NODE, 7, 7, MoveState::Idle),
                        foreign(80, RIFLE, 16, 15),
                    ]),
                ),
                view(
                    45,
                    300,
                    8,
                    12,
                    full_staff(vec![
                        entity(91, BARRACKS, 20, 10, MoveState::Idle),
                        entity(20, RIFLE, 15, 15, MoveState::Idle),
                        entity(21, RIFLE, 16, 15, MoveState::Idle),
                        entity(22, RIFLE, 15, 16, MoveState::Idle),
                        entity(23, RIFLE, 16, 16, MoveState::Idle),
                        entity(24, RIFLE, 15, 17, MoveState::Idle),
                        entity(25, RIFLE, 16, 17, MoveState::Idle),
                    ]),
                ),
            ]
        };
        let run = || {
            let mut controller = ScriptedController::new(plan());
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
}
