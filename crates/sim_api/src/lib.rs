//! Public vocabulary types shared outward from the simulation (plan §4, §6, §8, §9.8).
//!
//! Everything crossing the sim boundary — commands, events, views — is expressed in
//! types from this crate, so that `ai`, `replay`, `engine`, and `client` never need
//! to depend on simulation internals. M0 shipped the identity vocabulary; M1 adds
//! the command system (plan §8.1), the outward event stream (§9.8), rejection
//! reasons (§8.2), the match setup carried by replays (§6.5), and the read-only
//! view types (`Snapshot` for presentation, `PlayerView` for AI/UI under fog).
//!
//! Everything here is plain data: no behavior, no construction beyond simple
//! helpers, no I/O. Validation and application of commands live in `sim`.

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
#![warn(missing_docs)]

// The vector type is part of the outward vocabulary (commands, events, and views
// carry positions), so it is re-exported for API consumers alongside the local
// types (docs/ASSUMPTIONS.md A-005 records that sim_api owning fx types is the
// intended bottom of the graph).
pub use pandemonium_fx::Vec2Fx;

/// Entity identity: 64-bit, monotonic, never reused (plan §7.2).
///
/// IDs are allocated by the simulation only; because they are never reused, replays,
/// networking, and save/load can reference them without ambiguity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EntityId(pub u64);

impl EntityId {
    /// A value that is never allocated; useful as a null placeholder in tests and
    /// tools. The simulator allocates ids starting from 1.
    pub const NONE: Self = Self(0);
}

/// Player slot (plan §7.2). The [`PlayerId::NEUTRAL`] sentinel marks unowned
/// entities; it is not a controller.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PlayerId(pub u8);

impl PlayerId {
    /// The neutral (unowned) slot sentinel.
    pub const NEUTRAL: Self = Self(u8::MAX);
}

/// Simulation tick index. The simulation advances at exactly 30 ticks per second
/// (FD-1), decoupled from rendering.
pub type Tick = u32;

/// Compact identifier for an entity kind (e.g. "rifleman"), assigned during content
/// load (plan §7.3). Systems never match on kind names — the litmus test of the
/// entity model.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct KindId(pub u32);

/// Compact identifier for a registered resource (Alpha has one: Ore; plan §9.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ResourceId(pub u32);

/// A whole-tile position, used by placement commands (plan §8.1 `Build`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TilePos {
    /// The tile column.
    pub x: i32,
    /// The tile row.
    pub y: i32,
}

/// Which controller drives a player slot (plan §6.5: replays record the controller
/// kinds; plan §9.6: both controller kinds go through the same command door).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ControllerKind {
    /// A human at this machine.
    Human,
    /// A deterministic AI controller (crate `ai`, milestone M7).
    Ai,
}

/// One player slot in a match setup.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PlayerSetup {
    /// The slot this setup entry configures.
    pub player: PlayerId,
    /// What drives the slot.
    pub controller: ControllerKind,
}

/// How a match begins (plan §6.2 `Sim::new`): the seed that owns all simulation
/// randomness, plus the player slots. A match is fully described by this plus the
/// content and the ordered command log (FD-1).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MatchSetup {
    /// The single seed every simulation RNG stream derives from.
    pub seed: u64,
    /// Player slots, one per controller; the neutral sentinel never appears.
    pub players: Vec<PlayerSetup>,
}

/// Movement system state for an entity (plan §7.2: a small explicit enum per
/// system, no ad-hoc bool flags). Derived from the order queue, not stored twice.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum MoveState {
    /// No order being executed.
    Idle,
    /// The mover is advancing along its current order.
    Moving,
}

/// One entity as seen through a boundary type — the same shape for the full
/// presentation [`Snapshot`] and the fog-filtered [`PlayerView`] (plan §6.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EntityView {
    /// Identity of the entity.
    pub id: EntityId,
    /// Owning slot.
    pub owner: PlayerId,
    /// Which kind template the entity was spawned from.
    pub kind: KindId,
    /// Position in tile units.
    pub pos: Vec2Fx,
    /// Facing direction (a direction vector, not an angle; plan §5.8). Zero until
    /// the entity first moves.
    pub facing: Vec2Fx,
    /// Current health as thousandths of maximum (integer math; presentation draws a
    /// bar from this). Entities without a Health capability report 0.
    pub hp_fraction_milli: u32,
    /// What the movement system is doing with this entity.
    pub move_state: MoveState,
}

/// A read-only copy of what presentation needs (plan §6.4): entities in ascending
/// id order, every capability field presentation renders. The client keeps the
/// previous and current snapshot and interpolates between them (plan §11.1 —
/// interpolation lives above the sim boundary).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Snapshot {
    /// The tick this snapshot was taken at.
    pub tick: Tick,
    /// Every live entity, ascending by [`EntityId`].
    pub entities: Vec<EntityView>,
}

/// One resource in a player's ledger as seen in a [`PlayerView`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ViewResource {
    /// Which resource.
    pub resource: ResourceId,
    /// The player's current balance.
    pub amount: i64,
}

/// One tile's fog state for the viewing player (plan §9.5's three-state
/// model). Presentation reads this to draw the fog of war (the map overlay
/// and the minimap); the AI receives the same field it always had access to
/// through its own view — parity holds because every issuer sees the same
/// shape (FD-7, FD-8: fog filters information, it never alters the sim).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileFog {
    /// Never seen.
    Hidden,
    /// Seen before, not currently visible.
    Explored,
    /// Currently inside a friendly vision radius.
    Visible,
}

/// One queued production item as the owning player sees it (plan §11.4's
/// production queue display): which kind, and how far along (thousandths of
/// the kind's build time — the same integer convention as
/// [`EntityView::hp_fraction_milli`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QueueItemView {
    /// The kind being produced.
    pub kind: KindId,
    /// Work completed as thousandths of the kind's build time (0..=1000).
    pub progress_milli: u32,
}

/// One producer's queue as the owning player sees it (plan §11.4): the
/// ordered items (front first) and the rally point. Empty queues are
/// included — the command card needs to know an entity *is* a producer even
/// while idle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QueueView {
    /// The producing entity.
    pub producer: EntityId,
    /// The queue, front first (only the head progresses).
    pub items: Vec<QueueItemView>,
    /// The rally point newly produced units receive, if one is set.
    pub rally: Option<Vec2Fx>,
}

/// The fog-filtered view of the match for one player (plan §9.6): the *only* window
/// an AI controller gets. It contains what that player may know — own ledger and
/// entities plus entities inside friendly vision radii — never raw simulation
/// internals (FD-7, FD-8).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayerView {
    /// The tick this view was taken at.
    pub tick: Tick,
    /// The player the view belongs to.
    pub player: PlayerId,
    /// The player's resource ledger, ascending by [`ResourceId`].
    pub resources: Vec<ViewResource>,
    /// Current population usage (economy systems arrive in M5; zero until then).
    pub population: u32,
    /// Current population cap (from structures; zero until M5).
    pub population_cap: u32,
    /// Entities this player can see, ascending by [`EntityId`].
    pub entities: Vec<EntityView>,
    /// This player's per-tile fog state, row-major in the map's own tile
    /// order (`index = y * map_width + x`, exactly [`TileFog`] per tile).
    /// Empty when the player is not in the match. Derived state mirroring
    /// the fog cache (A-059 — never part of the canonical hash).
    pub fog: Vec<TileFog>,
    /// This player's production queues, producers in ascending id order
    /// (empty when the player is not in the match). Own queues only — fog
    /// hides enemy production exactly as it hides enemy entities.
    pub production: Vec<QueueView>,
}

/// Why the validation gate refused a command (plan §8.2). Every reason is emitted
/// through [`Event::CommandRejected`] and changes no state.
///
/// M1 checks the structural reasons (tick, sequence, ownership, existence,
/// capability presence, target legality). The economy reasons (`CannotAfford`,
/// `PopulationFull`, `PlacementBlocked`, `RequirementsUnmet`) become reachable when
/// the economy systems and their data arrive in M5.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RejectReason {
    /// The command's target tick is not the tick being applied (plan §6.2: all
    /// commands in a step must be for `self.tick`).
    TickMismatch,
    /// The issuer reused a sequence number within one tick; the earlier command
    /// wins and this one is refused deterministically.
    DuplicateSeq,
    /// The issuer is not a player in this match (including the neutral sentinel).
    PlayerMissing,
    /// A referenced entity does not exist (dead or never spawned; ids are never
    /// reused, so an absent id stays absent).
    UnknownEntity,
    /// A referenced entity exists but is not owned by the issuer.
    NotOwnedByIssuer,
    /// A referenced entity lacks the capability the command requires (plan §8.2 —
    /// a unit without Attack cannot be ordered to attack).
    MissingCapability,
    /// A referenced entity kind does not exist in the loaded content.
    UnknownKind,
    /// The command's target is not a legal target (missing, or not targetable).
    InvalidTarget,
    /// The target is legal but not visible to the issuer (plan §9.5 — targeting
    /// rejects entities not visible).
    NotVisible,
    /// The issuer cannot pay the cost (M5).
    CannotAfford,
    /// Population headroom exhausted (M5).
    PopulationFull,
    /// The placement tile is blocked or unbuildable (M5).
    PlacementBlocked,
    /// A requirement list is unmet (M5).
    RequirementsUnmet,
    /// A queue index is out of range (M5).
    QueueIndexInvalid,
}

/// A validation rejection (plan §8.2: "invalid commands produce `Reject { reason }`
/// events and change no state").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reject {
    /// Machine-readable cause.
    pub reason: RejectReason,
}

impl Reject {
    /// Builds a rejection for a reason.
    pub fn new(reason: RejectReason) -> Self {
        Self { reason }
    }
}

/// A single unit of intent entering the simulation (plan §8.1). Every mutation of
/// simulation state begins life as one of these — from a human, an AI controller,
/// or a replay (FD-2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Command {
    /// The player slot issuing the command.
    pub issuer: PlayerId,
    /// The tick at which the command is applied (plan §6.5: the field lockstep
    /// will use for input delay).
    pub tick: Tick,
    /// Per-issuer sequence for stable ordering within a tick.
    pub seq: u32,
    /// Append to the affected entities' order queues instead of replacing them.
    pub queue: bool,
    /// What to do.
    pub kind: CommandKind,
}

impl Command {
    /// Builds a command that replaces order queues (`queue: false`).
    pub fn new(issuer: PlayerId, tick: Tick, seq: u32, kind: CommandKind) -> Self {
        Self {
            issuer,
            tick,
            seq,
            queue: false,
            kind,
        }
    }

    /// The stable ordering key for application within one tick: issuer slot, then
    /// sequence (plan §6.3 stage 1).
    pub fn order_key(&self) -> (PlayerId, u32) {
        (self.issuer, self.seq)
    }
}

/// What a command asks the simulation to do (plan §8.1, including the v2 queue
/// commands). Payloads reference entities by id and positions in fixed-point tile
/// units. A new command is a new variant plus payload validation plus a handler —
/// never a change to the gate structure or application order (plan §8.3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CommandKind {
    /// Move the units to a position.
    Move {
        /// The ordered units; the simulation processes them in ascending id order
        /// regardless of list order (plan §5.3).
        units: Vec<EntityId>,
        /// The destination in tile units.
        target: Vec2Fx,
    },
    /// Attack a specific entity.
    Attack {
        /// The ordered units.
        units: Vec<EntityId>,
        /// The entity to attack.
        target: EntityId,
    },
    /// Move to a position, engaging enemies encountered en route (combat semantics
    /// arrive with M6; until then the movement leg behaves like [`CommandKind::Move`]).
    AttackMove {
        /// The ordered units.
        units: Vec<EntityId>,
        /// The destination in tile units.
        target: Vec2Fx,
    },
    /// Cancel current orders and clear the order queue.
    Stop {
        /// The ordered units.
        units: Vec<EntityId>,
    },
    /// Gather from a resource node (economy systems arrive in M5).
    Gather {
        /// The ordered workers.
        units: Vec<EntityId>,
        /// The resource node entity.
        node: EntityId,
    },
    /// Start constructing a structure at a tile (construction arrives in M5).
    Build {
        /// The worker that builds.
        worker: EntityId,
        /// The structure kind to build.
        structure: KindId,
        /// The placement tile.
        at: TilePos,
    },
    /// Enqueue a producible in a producer's queue (production arrives in M5).
    Train {
        /// The producing entity.
        producer: EntityId,
        /// The kind to produce.
        unit: KindId,
    },
    /// Cancel one queued item by index (production arrives in M5).
    CancelQueueItem {
        /// The producing entity.
        producer: EntityId,
        /// Zero-based index into the producer's queue.
        index: u16,
    },
    /// Set the producer's rally point (production arrives in M5).
    SetRally {
        /// The producing entity.
        producer: EntityId,
        /// The rally destination in tile units.
        target: Vec2Fx,
    },
    /// Concede the match for the issuer.
    Resign {},
}

/// An outward notification from the simulation (plan §9.8): the only channel
/// through which presentation, audio, telemetry, and replay tooling observe what
/// happened. Nothing polls sim internals (FD-9). Events are outputs — they are not
/// part of the hashed state.
///
/// M1 emits [`Event::Spawned`], [`Event::Died`], and [`Event::CommandRejected`];
/// the remaining variants are the planned vocabulary and light up as their systems
/// land (M4–M8).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    /// An entity entered the world (initial or scheduled spawn now; production and
    /// construction in M5).
    Spawned {
        /// The new entity.
        entity: EntityId,
        /// Its owner.
        owner: PlayerId,
        /// Its kind.
        kind: KindId,
        /// Its spawn position.
        pos: Vec2Fx,
    },
    /// An entity's health reached zero and it was removed (ids are never reused).
    Died {
        /// The removed entity.
        entity: EntityId,
    },
    /// A projectile-less attack landed (combat pipeline arrives in M6).
    AttackHit {
        /// The attacker.
        attacker: EntityId,
        /// The victim.
        target: EntityId,
        /// Integer damage applied.
        damage: i32,
    },
    /// A production queue item started (M5).
    ProductionStarted {
        /// The producing entity.
        producer: EntityId,
        /// The kind being produced.
        producible: KindId,
    },
    /// A production queue item completed and spawned (M5).
    ProductionCompleted {
        /// The producing entity.
        producer: EntityId,
        /// The kind that was produced.
        producible: KindId,
    },
    /// A construction site opened (M5).
    ConstructionStarted {
        /// The committed builder.
        builder: EntityId,
        /// The site entity.
        site: EntityId,
    },
    /// A construction site finished (M5).
    ConstructionCompleted {
        /// The site entity.
        site: EntityId,
    },
    /// A worker deposited cargo (M5).
    ResourceDelivered {
        /// The delivering worker.
        worker: EntityId,
        /// Which resource.
        resource: ResourceId,
        /// How much was deposited.
        amount: i32,
    },
    /// A resource node ran out (M5).
    NodeDepleted {
        /// The exhausted node.
        node: EntityId,
    },
    /// The validation gate refused a command; no state changed.
    CommandRejected {
        /// Who issued the refused command.
        issuer: PlayerId,
        /// The command's sequence number, to identify it in the issuer's stream.
        seq: u32,
        /// The rejection.
        reject: Reject,
    },
    /// A mover gave up on reaching its target (stuck detection arrives in M4).
    MoveFailed {
        /// The entity that could not proceed.
        entity: EntityId,
    },
    /// The match resolved (match rules arrive in M8).
    MatchEnded {
        /// The winning slot.
        winner: PlayerId,
    },
}

/// One of the eleven fixed pipeline stages of the simulation tick (plan
/// §6.3's update order, verbatim). The stages exist as an enum so an
/// instrumentation seam ([`StageObserver`]) can name them without depending
/// on simulation internals.
///
/// The enum is vocabulary, not behavior: `Sim::step` runs the stages in this
/// order whether or not anyone is watching, and stages whose milestone slice
/// currently lives inside another system's function (Orders, Acquisition)
/// are reported as the boundaries they are — their own row in a profile is
/// the honest measure of "no direct code yet".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    /// Stage 1 — apply commands: sorted, validated, applied (plan §6.3.1).
    Commands,
    /// Stage 2 — orders: resolve each entity's current order into intents.
    Orders,
    /// Stage 3 — production & construction: queues, sites, scheduled spawns.
    Production,
    /// Stage 4 — economy: gather timers, cargo, deliveries, depletion.
    Economy,
    /// Stage 5 — target acquisition (plan §9.2's acquire step).
    Acquisition,
    /// Stage 6 — movement: path requests, following, steering, push-apart.
    Movement,
    /// Stage 7 — combat: attacks, damage, `AttackHit` (plan §9.2).
    Combat,
    /// Stage 8 — death & cleanup: health, lifecycle events, removal.
    Cleanup,
    /// Stage 9 — vision: incremental per-player visibility update (§9.5).
    Vision,
    /// Stage 10 — match rules: defeat/victory evaluation (§9.7).
    MatchRules,
    /// Stage 11 — finalize: tick increment, event flush, periodic hash.
    Finalize,
}

/// An optional read-only instrumentation seam over the tick pipeline
/// (B-002's per-stage profiling): `Sim::step_observed` calls [`stage`]
/// (Self::stage) once per pipeline stage, in pipeline order, immediately
/// before that stage's work.
///
/// The observer is presentation-only telemetry by contract: it must not
/// mutate anything it can reach, and it can never change simulation state —
/// the same rule `tools bench` already obeys for its per-tick samples
/// (FD-6). Timing collectors live outside the determinism crates (tools),
/// which is why this trait carries no clock of its own.
///
/// [`stage`]: StageObserver::stage
pub trait StageObserver {
    /// A pipeline stage is about to run.
    fn stage(&mut self, stage: Stage);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_order_key_sorts_by_issuer_then_sequence() {
        let a = Command::new(PlayerId(0), 5, 2, CommandKind::Resign {});
        let b = Command::new(PlayerId(0), 5, 10, CommandKind::Resign {});
        let c = Command::new(PlayerId(1), 5, 1, CommandKind::Resign {});
        assert!(a.order_key() < b.order_key());
        assert!(b.order_key() < c.order_key());
    }

    #[test]
    fn command_new_defaults_to_queue_replace() {
        let cmd = Command::new(PlayerId(0), 9, 1, CommandKind::Resign {});
        assert!(!cmd.queue);
    }

    #[test]
    fn neutral_is_not_a_controller_slot() {
        assert_eq!(PlayerId::NEUTRAL, PlayerId(u8::MAX));
        assert!(PlayerId(0) < PlayerId::NEUTRAL);
    }

    #[test]
    fn entity_id_none_is_outside_the_allocated_range() {
        // The simulator allocates from 1 upward, so zero stays a safe null value.
        assert_eq!(EntityId::NONE, EntityId(0));
        assert!(EntityId::NONE < EntityId(1));
    }
}
