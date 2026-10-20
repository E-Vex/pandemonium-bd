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
/// the remaining variants light up as their systems land (M4–M8). ADR-0201
/// adds the objective-state family (M-missions Phase 1): [`Event::ObjectiveState`]
/// reports mission-objective progress, consumed after the step like every
/// other event.
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
    /// A worker deposited cargo (M5). B-018: the event carries the
    /// credited player (`owner` — the ledger that received the ore), so
    /// per-player delivery totals are exact AT CONSUMPTION TIME: ids die
    /// (a worker that delivers at stage 4 and falls at stage 8 is gone by
    /// stage 10), and the only authoritative attribution moment is the
    /// deposit itself. The mission evaluator's cumulative gather objective
    /// reads this field (FD-9 — the ADR's §2 expressible basis for the
    /// kind); the event stays outward and unhashed exactly as before.
    ResourceDelivered {
        /// The delivering worker.
        worker: EntityId,
        /// The player whose ledger was credited (the storage owner — the
        /// gather loop selects the worker's own player's storage, so this
        /// is the worker's player at deposit time).
        owner: PlayerId,
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
    /// An objective's state changed (ADR-0201 §2): one mission objective
    /// resolved this tick — completed or failed. `index` is 0-indexed into
    /// the mission's objective list. Unhashed and outward (FD-9): like every
    /// event, it is an output consumed after the step, never part of the
    /// canonical state and never inside the replay codec.
    ObjectiveState {
        /// The objective that resolved, 0-indexed into the mission's
        /// objective list.
        index: u32,
        /// How it resolved.
        state: ObjectiveState,
    },
}

/// How one mission objective resolved (ADR-0201 §2) — the payload of
/// [`Event::ObjectiveState`]. The Phase 1 kinds complete, never fail; the
/// failure arm lights up with Phase 2's protect kind (B-017: a protect
/// objective fails when the last protected entity dies). The state is
/// monotonic per objective: once resolved, it never reverts, and the event
/// fires exactly once per objective.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectiveState {
    /// The objective's success condition is met.
    Completed,
    /// The objective's failure condition is met.
    Failed,
}

/// A mission objective's kind in the simulation's resolved vocabulary
/// (ADR-0201 §2): what the stage-10 evaluator checks. Content authors kinds
/// by id string; the content seam resolves them to `KindId`s. Phase 1
/// carried the two vertical-slice kinds; Phase 2's B-017 added survive and
/// protect, B-018 adds reach and gather — ADR-0201 §9's Phase 2 objective
/// set is complete.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissionObjectiveKind {
    /// Destroy all enemy structures — or every enemy structure of a target
    /// kind (`MissionObjectiveParameters::Destroy`).
    Destroy,
    /// Build/train N entities of a kind (`MissionObjectiveParameters::BuildTrain`).
    BuildTrain,
    /// Survive N ticks from mission start (B-017): pending until the
    /// mission tick reaches the authored count, then completed.
    Survive,
    /// Protect the mission player's entities of a kind (B-017): a fail-only
    /// constraint — pending while at least one living entity of the kind
    /// stands, failed on the first tick none remains. It never completes
    /// by itself; victory comes from the other objectives.
    Protect,
    /// Reach an area with any mission player entity (B-018): pending until
    /// one stands inside [`AreaSpec`], then completed.
    ReachArea,
    /// Gather N ore (B-018): pending until the mission player's ore crosses
    /// the authored amount under the mission's chosen semantics
    /// (`MissionObjectiveParameters::Gather`) — cumulative delivered, or
    /// held balance.
    Gather,
}

/// A mission objective's parameters in the simulation's resolved vocabulary
/// (ADR-0201 §2) — the `KindId`-resolved form of content's
/// `ObjectiveParameters` (string ids). The content seam
/// (`ContentTree::mission_data`) performs the resolution; the
/// simulation never sees kind-name strings (FD-6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissionObjectiveParameters {
    /// `None` = all enemy structures (today's elimination rule); `Some(kind)`
    /// = every enemy structure of that kind.
    Destroy {
        /// The target kind, if the objective names one.
        target_kind: Option<KindId>,
    },
    /// The kind to build/train, and how many.
    BuildTrain {
        /// The kind to build or train.
        kind: KindId,
        /// How many must exist.
        count: u32,
    },
    /// How many ticks the mission player must survive
    /// (`MissionObjectiveKind::Survive`, B-017). The count is from mission
    /// start — the tick the match began at, 0 for every mission loaded at
    /// construction. Integer ticks (FD-5); the loader enforces a floor of
    /// 300 so the window always demands real play.
    Survive {
        /// The tick the objective completes at.
        ticks: u32,
    },
    /// The entity kind the mission player must keep alive
    /// (`MissionObjectiveKind::Protect`, B-017). The loader rejects a kind
    /// the mission player's starting forces do not carry — a protect
    /// objective with nothing to protect fails on the first tick.
    Protect {
        /// The protected kind.
        kind: KindId,
    },
    /// The area the mission player must reach with any entity
    /// (`MissionObjectiveKind::ReachArea`, B-018): a rectangle in whole
    /// tiles or a circle in milli-tiles — plain integer geometry (FD-5).
    /// The content seam converts the authored [`AreaSpec`] mirror into
    /// this form verbatim (area data is unit-tagged, never converted);
    /// the loader rejects areas that leave the map, cover it, or already
    /// contain a mission player starting entity at tick 0 (AUD-003).
    ReachArea {
        /// The area to reach.
        area: AreaSpec,
    },
    /// The ore amount the mission player must gather
    /// (`MissionObjectiveKind::Gather`, B-018). The resource is ore by
    /// the objective's definition (the Alpha's single registered
    /// resource); the seam resolves the resource id through the rules
    /// registry — the exact id the world's ledger carries — and the
    /// semantics chooses what the evaluator counts. The loader rejects an
    /// amount at or below the ruleset's effective starting ore for both
    /// semantics (AUD-003: the objective must demand play).
    Gather {
        /// The ore resource, resolved through the rules registry order.
        resource: ResourceId,
        /// How much ore the objective demands.
        amount: i64,
        /// Which ore total the objective counts.
        semantics: GatherSemantics,
    },
}

/// A gather objective's counting semantics (B-018, ADR-0201 §2's table:
/// "cumulative-delivered vs hold-balance (mission data picks)") — the
/// sim-side mirror of content's `GatherSemantics`, converted at the seam
/// the way `FogPreset` is (FD-6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GatherSemantics {
    /// The mission player's cumulative delivered ore: the running total of
    /// their `ResourceDelivered` amounts, accumulated by the evaluator
    /// from the delivery events (FD-9 — the ADR's expressible basis for
    /// the kind). Spending never reduces it.
    Cumulative,
    /// The mission player's current ore balance — the hashed ledger the
    /// economy credits. Spending sets it back below the target; the
    /// objective completes only when the balance actually holds the
    /// amount (the resolution itself fires once, like every objective).
    Balance,
}

impl GatherSemantics {
    /// The schema spelling of this semantics (what appears in the mission
    /// file — the same spellings the content mirror parses).
    pub fn as_str(self) -> &'static str {
        match self {
            GatherSemantics::Cumulative => "Cumulative",
            GatherSemantics::Balance => "Balance",
        }
    }

    /// Parses a schema spelling. Unknown spellings are `None` so callers
    /// can reject them loudly — never silently mapped.
    pub fn from_schema(semantics: &str) -> Option<Self> {
        match semantics {
            "Cumulative" => Some(GatherSemantics::Cumulative),
            "Balance" => Some(GatherSemantics::Balance),
            _ => None,
        }
    }
}

/// An area a reach objective names (B-018, ADR-0201 §2's table): a
/// rectangle in whole tiles or a circle in milli-tiles. Plain integer
/// data (FD-5 — no floats anywhere in authored geometry); the sim-side
/// mirror of content's `defs::AreaSpec`, converted at the mission seam
/// the way `FogPreset` is (FD-6: the content type never crosses down).
///
/// The containment law lives here — one predicate the stage-10 evaluator
/// and the content loader's tick-0 gate both call, so the gate can never
/// drift from the evaluation (FD-4: the geometry is code, the area is
/// data that selects it):
///
/// - **`Rect`** uses the half-open tile convention: `[x, x+w) x [y, y+h)`
///   — the left and top edges are **inclusive**, the right and bottom
///   edges **exclusive**. This is exactly the tile-cell law everywhere
///   else in the codebase (`FootprintDef::top_left` and vision derive a
///   tile from a position by `floor_int`: an entity standing at
///   `pos.x == x + w` stands *in tile `x + w`*, outside the rect, while
///   `pos.x == x + w - 1/65536` is still inside).
/// - **`Circle`** includes its boundary: a point is inside when its
///   squared distance from the center is `<= radius^2` — the same
///   comparison `advance_vision` uses to mark a tile inside a vision
///   radius. Authored milli-tile values enter the fixed-point space
///   through `Fx::from_milli`, the same truncating conversion every
///   authored milli value takes (vision radii, movement speeds); a
///   radius that is not an exact multiple of `1/65536` tile loses its
///   sub-raw fraction, deterministically, before the comparison —
///   sub-milli-tile precision, pinned by the boundary tests below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AreaSpec {
    /// An axis-aligned rectangle in whole tiles.
    Rect {
        /// The left edge's tile column (inclusive).
        x: i32,
        /// The top edge's tile row (inclusive).
        y: i32,
        /// The width in tiles (the right edge `x + w` is exclusive).
        w: u32,
        /// The height in tiles (the bottom edge `y + h` is exclusive).
        h: u32,
    },
    /// A circle in milli-tiles.
    Circle {
        /// The center's x, in milli-tiles.
        cx: i32,
        /// The center's y, in milli-tiles.
        cy: i32,
        /// The radius, in milli-tiles (boundary inclusive).
        radius: i32,
    },
}

impl AreaSpec {
    /// Whether a position stands inside the area — the objective's
    /// containment law, documented on the type and pinned by unit tests.
    /// Exact integer comparisons in raw Q16.16 units (FD-5: no floats, no
    /// roots — squared-distance, the plan §5.8 discipline).
    pub fn contains(&self, pos: Vec2Fx) -> bool {
        use pandemonium_fx::Fx;
        match *self {
            // Half-open [x, x+w) x [y, y+h): the edges convert exactly
            // (whole tiles are exact in Q16.16), the position compares in
            // raw units, and the right/bottom edge raw value is excluded —
            // an entity exactly on it stands in the next tile. The
            // saturating add is defensive only: the loader's bounds gate
            // rejects areas that leave the map long before an edge could
            // approach the representable ceiling.
            AreaSpec::Rect { x, y, w, h } => {
                let x0 = Fx::from_int(x).raw() as i64;
                let x1 = Fx::from_int(x.saturating_add(w as i32)).raw() as i64;
                let y0 = Fx::from_int(y).raw() as i64;
                let y1 = Fx::from_int(y.saturating_add(h as i32)).raw() as i64;
                let (px, py) = (pos.x.raw() as i64, pos.y.raw() as i64);
                px >= x0 && px < x1 && py >= y0 && py < y1
            }
            // Boundary-inclusive: dist^2 <= r^2 in raw Q16.16 units, the
            // `advance_vision` comparison. The center and radius take
            // `Fx::from_milli`'s truncating conversion — the documented
            // law for authored milli-tiles; the deltas then subtract
            // exactly, and the squared compare stays in i64 (tile-unit
            // magnitudes squared are far inside the range).
            AreaSpec::Circle { cx, cy, radius } => {
                let center = Vec2Fx::new(Fx::from_milli(cx), Fx::from_milli(cy));
                let r = Fx::from_milli(radius).raw() as i64;
                let dx = pos.x.raw() as i64 - center.x.raw() as i64;
                let dy = pos.y.raw() as i64 - center.y.raw() as i64;
                dx * dx + dy * dy <= r * r
            }
        }
    }
}

/// One mission objective as the simulation receives it (ADR-0201 §2): the
/// kind, its resolved parameters, and whether the objective is optional.
/// Optional objectives never gate `MatchEnded` — they feed the end screen
/// and story hooks; a failed *non-optional* objective ends the match with the
/// opponent as winner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MissionObjective {
    /// What the evaluator checks.
    pub kind: MissionObjectiveKind,
    /// The kind's parameters.
    pub parameters: MissionObjectiveParameters,
    /// Whether the objective is optional.
    pub optional: bool,
}

/// A mission's data set as the simulation receives it (ADR-0201 §2a): the
/// mission player the objectives belong to, the ordered objective list —
/// the indices are the [`Event::ObjectiveState`] indices — and the fog
/// preset (B-015). Match-defining input data, exactly the class
/// [`MatchSetup`] belongs to: it crosses the sim boundary at construction
/// (`Sim::with_mission`) and is evaluated per tick by stage 10 as derived
/// state (the A-066 pattern), so it never enters the canonical state hash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MissionData {
    /// The player the objectives belong to — the mission player. The
    /// objective evaluator reads the world from this player's side: "enemy"
    /// means every non-neutral owner that is not this player.
    pub player: PlayerId,
    /// The ordered objective list, AND-composed (ADR-0201 §2): the mission
    /// player wins when every non-optional objective completes.
    pub objectives: Vec<MissionObjective>,
    /// The fog preset (B-015): how the fog cache initializes for every
    /// player, applied once at construction. The `Default` preset is
    /// byte-identical to the mission-less initialization, so a mission that
    /// does not use the field changes nothing observable in the fog.
    pub fog_preset: FogPreset,
    /// The AI tuning overrides (B-021): carried beside the objectives and
    /// the fog preset, never read by the simulation — the engine's plan
    /// builder reads them when it derives the scripted opponent's plan
    /// (the controller is engine-side state). All-`None` is the byte-exact
    /// default: the mission-less path and a mission without the block
    /// construct the same all-`None` settings, so nothing observable in
    /// the simulation differs.
    pub ai_settings: AiSettings,
}

/// A mission's fog preset (ADR-0201 Phase 2, B-015): how the fog cache
/// initializes for every player when the match carries this mission.
/// Behaviour is data selecting code (FD-4) — the preset is mission data
/// riding [`MissionData`] into the sim (FD-6), and it only ever
/// selects the fog filter's initial view; it never touches simulation
/// state (FD-8: fog filters, never alters).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum FogPreset {
    /// Today's initialization, byte-identical: every tile starts `Hidden`
    /// and vision radii light up from there. The default when the mission
    /// file omits the field.
    #[default]
    Default,
    /// Every tile starts `Explored` (seen before, not currently visible);
    /// the per-tick vision pass still marks what units actually see
    /// `Visible` on top.
    Explored,
    /// The whole map is visible for the entire match, not just tick 0: the
    /// fog filter reports every tile `Visible` persistently, so the
    /// per-tick demotion of unrefreshed tiles can never darken them.
    Visible,
}

impl FogPreset {
    /// The schema spelling of this preset (what appears in the mission file).
    pub fn as_str(self) -> &'static str {
        match self {
            FogPreset::Default => "Default",
            FogPreset::Explored => "Explored",
            FogPreset::Visible => "Visible",
        }
    }

    /// Parses a schema spelling. Unknown spellings are `None` so the loader
    /// can reject them loudly — never silently mapped.
    pub fn from_schema(preset: &str) -> Option<Self> {
        match preset {
            "Default" => Some(FogPreset::Default),
            "Explored" => Some(FogPreset::Explored),
            "Visible" => Some(FogPreset::Visible),
            _ => None,
        }
    }
}

/// Mission-controlled AI tuning (B-021, ADR-0201 Phase 2): the overrides a
/// mission may apply to the scripted Alpha opponent's pacing. Every field is
/// an `Option<u32>`; `None` means the script's own const — the byte-exact
/// default law — so a mission that carries no overrides (or an all-`None`
/// block) tunes nothing, exactly like the mission-less path.
///
/// The settings ride [`MissionData`] across the sim boundary at
/// construction (FD-6: mission data reaches the engine only through the
/// content seam), but the simulation never reads them — they parameterize
/// the *controller*, which is engine-side state: `ai_host::alpha_plan`
/// copies them into the `AiPlan` the scripted controller reads as
/// `override.unwrap_or(CONST)`. A match's identity stays seed + content +
/// the command log (FD-1); a tuned controller's commands are the log's
/// bytes, and the mission id (AUD-002) explains the tuning on recordings
/// that carry one.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct AiSettings {
    /// The pressure cadence: how many ticks a marching wave waits before
    /// its march is re-issued (a live wave never parks). `None` = the
    /// script's `WAVE_TIMER_TICKS` (2400 — 80 s at 30 Hz).
    pub wave_timer_ticks: Option<u32>,
    /// The army size a full wave marches at. `None` = the script's
    /// `WAVE_SIZE` (10).
    pub wave_size: Option<u32>,
    /// The minimum army size that may march — below it only a full
    /// `wave_size` wave (or nothing) marches. `None` = the script's
    /// `MIN_WAVE` (4).
    pub min_wave: Option<u32>,
    /// The hard cap on the army size — training pauses at the cap until
    /// attrition opens a slot. `None` = the script's `ARMY_CAP` (16).
    pub army_cap: Option<u32>,
    /// The worker count the script grows the economy to before pausing
    /// worker training. `None` = the script's `WORKER_TARGET` (10).
    pub worker_target: Option<u32>,
    /// The initial pressure timer — how long the script waits before its
    /// first timer-driven march (an army at `wave_size` marches whenever
    /// it is ready, whatever this says). `None` = the script's
    /// `WAVE_TIMER_TICKS` (2400): today the first delay and the pressure
    /// cadence share one const, and the split exists so a mission can
    /// stagger them.
    pub first_attack_delay: Option<u32>,
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

    #[test]
    fn objective_state_events_carry_the_index_and_resolution() {
        // ADR-0201 §2: the outward objective-progress vocabulary is plain data
        // — index into the mission's objective list, and the resolution.
        let event = Event::ObjectiveState {
            index: 1,
            state: ObjectiveState::Completed,
        };
        assert_eq!(
            event,
            Event::ObjectiveState {
                index: 1,
                state: ObjectiveState::Completed,
            }
        );
        assert_ne!(ObjectiveState::Completed, ObjectiveState::Failed);
    }

    #[test]
    fn rect_areas_use_the_half_open_tile_convention() {
        // B-018: the rect containment law, pinned at every edge. Left and
        // top edges are inclusive; right and bottom edges are exclusive —
        // an entity standing exactly on the right edge stands in the next
        // tile (the `floor_int` tile-cell law footprints and vision use).
        // Positions are exact Q16.16 values: whole tiles, and points one
        // raw unit shy of an edge (1/65536 tile — the finest position the
        // sim can express).
        use pandemonium_fx::Fx;
        const RAW_TILE: i32 = 65_536; // Fx::from_int(1).raw()
        let area = AreaSpec::Rect {
            x: 2,
            y: 3,
            w: 4,
            h: 5,
        }; // [2,6) x [3,8)
        let at = |x: i32, y: i32| Vec2Fx::new(Fx::from_int(x), Fx::from_int(y));
        let at_raw = |x: i32, y: i32| Vec2Fx::new(Fx::from_raw(x), Fx::from_raw(y));
        assert!(
            area.contains(at(2, 3)),
            "the top-left corner is inside (edges inclusive)"
        );
        assert!(area.contains(at(5, 7)), "an interior point is inside");
        assert!(
            area.contains(at_raw(6 * RAW_TILE - 1, 3 * RAW_TILE)),
            "one raw unit shy of the right edge is still inside"
        );
        assert!(
            !area.contains(at(6, 3)),
            "exactly on the right edge (x == x+w) is OUTSIDE — that is tile 6"
        );
        assert!(
            area.contains(at_raw(5 * RAW_TILE, 8 * RAW_TILE - 1)),
            "one raw unit shy of the bottom edge is still inside"
        );
        assert!(
            !area.contains(at(5, 8)),
            "exactly on the bottom edge (y == y+h) is OUTSIDE"
        );
        assert!(
            !area.contains(at_raw(6 * RAW_TILE - 1, 8 * RAW_TILE)),
            "x inside but y exactly on the bottom edge: OUTSIDE overall"
        );
        assert!(!area.contains(at(1, 3)), "left of the left edge is outside");
        assert!(
            !area.contains(at_raw(2 * RAW_TILE - 1, 3 * RAW_TILE)),
            "one raw unit shy of the left edge is outside (x >= x is inclusive)"
        );
        assert!(!area.contains(at(2, 2)), "above the top edge is outside");
    }

    #[test]
    fn circle_areas_include_their_boundary() {
        // B-018: the circle containment law — dist^2 <= r^2 in raw Q16.16
        // units, the `advance_vision` comparison. The boundary itself is
        // INSIDE; one milli-tile beyond it is outside. All values are
        // multiples of 500 milli (exact in Q16.16), so the pins test the
        // law, not the conversion.
        use pandemonium_fx::Fx;
        let area = AreaSpec::Circle {
            cx: 1000,
            cy: 2000,
            radius: 500,
        };
        let at_milli = |x: i32, y: i32| Vec2Fx::new(Fx::from_milli(x), Fx::from_milli(y));
        assert!(area.contains(at_milli(1000, 2000)), "the center is inside");
        assert!(
            area.contains(at_milli(1500, 2000)),
            "exactly on the boundary (dist == radius) is INSIDE"
        );
        assert!(
            area.contains(at_milli(1000, 1500)),
            "the boundary in -y is inside too"
        );
        assert!(
            area.contains(at_milli(1350, 2350)),
            "a diagonal boundary point (3-4-5 triangle) is inside"
        );
        assert!(
            !area.contains(at_milli(1501, 2000)),
            "one milli-tile past the boundary is outside"
        );
        assert!(
            area.contains(at_milli(500, 2000)),
            "the boundary in -x (dist 500 == r) is inside"
        );
        assert!(
            !area.contains(at_milli(499, 2000)),
            "one milli-tile past the -x boundary is outside"
        );
    }

    #[test]
    fn fog_presets_round_trip_their_schema_spellings() {
        // B-015: the fog-preset vocabulary is schema-facing — every preset
        // parses from its own spelling and only its own spelling, and the
        // default (an absent mission field) is `Default`.
        for preset in [FogPreset::Default, FogPreset::Explored, FogPreset::Visible] {
            assert_eq!(FogPreset::from_schema(preset.as_str()), Some(preset));
        }
        assert_eq!(FogPreset::default(), FogPreset::Default);
        assert_eq!(FogPreset::from_schema("Hidden"), None);
        assert_eq!(FogPreset::from_schema("default"), None);
        assert_eq!(FogPreset::from_schema(""), None);
    }

    #[test]
    fn mission_data_sets_carry_their_player_ordered_objectives_and_fog_preset() {
        // ADR-0201 §2a: the mission input vocabulary — resolved KindIds, the
        // mission player, the ordered list whose indices the events echo,
        // and (B-015) the fog preset the fog cache initializes from.
        let objectives = MissionData {
            player: PlayerId(0),
            objectives: vec![
                MissionObjective {
                    kind: MissionObjectiveKind::Destroy,
                    parameters: MissionObjectiveParameters::Destroy { target_kind: None },
                    optional: false,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::BuildTrain,
                    parameters: MissionObjectiveParameters::BuildTrain {
                        kind: KindId(3),
                        count: 4,
                    },
                    optional: true,
                },
                // B-017: survive is plain data — an integer tick count from
                // mission start, carried beside the resolved kinds.
                MissionObjective {
                    kind: MissionObjectiveKind::Survive,
                    parameters: MissionObjectiveParameters::Survive { ticks: 300 },
                    optional: false,
                },
                // B-017: protect carries its protected kind as a resolved
                // KindId — the fail-only twin of BuildTrain's kind field.
                MissionObjective {
                    kind: MissionObjectiveKind::Protect,
                    parameters: MissionObjectiveParameters::Protect { kind: KindId(3) },
                    optional: false,
                },
            ],
            fog_preset: FogPreset::Explored,
            ai_settings: AiSettings::default(),
        };
        assert_eq!(objectives.player, PlayerId(0));
        assert_eq!(objectives.objectives.len(), 4);
        assert!(objectives.objectives[0].kind == MissionObjectiveKind::Destroy);
        assert!(!objectives.objectives[0].optional);
        assert!(objectives.objectives[1].optional);
        assert_eq!(
            objectives.objectives[2],
            MissionObjective {
                kind: MissionObjectiveKind::Survive,
                parameters: MissionObjectiveParameters::Survive { ticks: 300 },
                optional: false,
            }
        );
        assert_eq!(
            objectives.objectives[3],
            MissionObjective {
                kind: MissionObjectiveKind::Protect,
                parameters: MissionObjectiveParameters::Protect { kind: KindId(3) },
                optional: false,
            }
        );
        assert_eq!(objectives.fog_preset, FogPreset::Explored);
        // B-021: the AI tuning rides the same set — all-`None` is the
        // byte-exact default, and every field is independently overridable
        // plain integer data (FD-5).
        assert_eq!(objectives.ai_settings, AiSettings::default());
        let tuned = AiSettings {
            wave_timer_ticks: Some(1200),
            worker_target: Some(14),
            ..Default::default()
        };
        assert_eq!(tuned.wave_timer_ticks, Some(1200));
        assert_eq!(tuned.worker_target, Some(14));
        assert_eq!(tuned.wave_size, None);
    }
}
