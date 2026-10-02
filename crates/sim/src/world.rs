//! The entity model and its stores (plan §7): everything that exists is an entity;
//! capabilities are composable data blocks attached per entity; systems query by
//! capability, never by kind name.
//!
//! Storage design (plan §7.3 implementation freedom): every store is a `Vec` kept
//! sorted ascending by [`EntityId`] — an invariant, not an aspiration. Ids are
//! allocated monotonically, so appends preserve order; removals use `Vec::remove`,
//! which shifts survivors down without reordering them. Iteration is therefore
//! ascending id by construction, lookups are a binary search (O(log n)), and the
//! invariant is pinned by unit tests plus the acceptance suite. No unordered
//! containers exist anywhere in this crate (plan §5.2, enforced by the
//! architecture-law scan).

use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim_api::{
    ControllerKind, EntityId, KindId, MoveState, PlayerId, PlayerSetup, ResourceId,
};

/// One capability block in its runtime form (plan §7.3: one variant per capability
/// type; extend by adding a variant plus a system). The M1 vocabulary is exactly
/// what the trivial world and its systems consume: `Health`, `Move`, `Vision`.
/// Economy and combat capabilities arrive with their milestones (M5/M6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CapabilityData {
    /// Hit points and regeneration.
    Health(HealthDef),
    /// Locomotion parameters.
    Move(MoveDef),
    /// Perception radius.
    Vision(VisionDef),
}

/// Health capability: current hp plus its parameters. Regeneration advances every
/// tick (saturating, clamped to the pool) and is the M1 death path that exercises
/// death & cleanup (plan §6.3 stage 8); combat damage becomes the primary health
/// mover in M6.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HealthDef {
    /// Maximum hit points.
    pub max_hp: i32,
    /// Current hit points.
    pub hp: i32,
    /// Hit points gained (lost, when negative) per tick.
    pub regen_per_tick: i32,
}

/// Move capability in runtime units: the parameters plus this capability's
/// runtime state (the same pattern as `hp` living on `HealthDef`). The path
/// is the *remaining* waypoints of the current order — `path[0]` is the next
/// steering target; it empties as waypoints are reached and is rebuilt when
/// orders change or a repath escalates (M4, plan §9.1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MoveDef {
    /// Top speed per tick in fixed-point tile units (already converted from
    /// authoring milli-tiles per second, plan §10.2).
    pub speed_per_tick: Fx,
    /// Collision radius in fixed-point tile units (plan §9.1.3: units have
    /// integer radii from data).
    pub radius: Fx,
    /// Remaining waypoints of the current order, next-first. Empty while no
    /// path is active (a fresh order builds one in stage 6).
    pub path: Vec<Vec2Fx>,
    /// Consecutive ticks the mover was blocked (stuck detection, plan §9.1).
    pub stuck_ticks: u32,
    /// Repath attempts spent on the current order (the escalation counter
    /// that ends in `MoveFailed`).
    pub repaths: u32,
}

impl MoveDef {
    /// A mover with its parameters and a clean runtime state.
    pub fn new(speed_per_tick: Fx, radius: Fx) -> Self {
        Self {
            speed_per_tick,
            radius,
            path: Vec::new(),
            stuck_ticks: 0,
            repaths: 0,
        }
    }

    /// Clears the runtime movement state — a new order starts from scratch.
    pub fn reset_runtime(&mut self) {
        self.path.clear();
        self.stuck_ticks = 0;
        self.repaths = 0;
    }
}

/// Vision capability in runtime units.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VisionDef {
    /// Sight radius in fixed-point tile units.
    pub radius: Fx,
}

/// An order in an entity's queue (plan §6.3 stage 2: the order queue resolves into
/// system intents). M1's order vocabulary is movement-only; combat and economy
/// intents extend this enum with their milestones.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Order {
    /// Advance to a position.
    MoveTo {
        /// Destination in tile units.
        target: Vec2Fx,
    },
}

/// Entity lifecycle (plan §7.2: Spawn → Active → Dead → Cleaned, with events at
/// each step). Entities are removed the same tick they die, so externally visible
/// states are `Active` and "gone".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lifecycle {
    /// Alive and simulated.
    Active,
    /// Health reached zero this tick; cleanup removes it before the tick ends.
    Dead,
}

/// The base entity record (plan §7.3's `Entity` plus the M1 order queue on the
/// spine, since order queues are per-entity state consumed by the mover).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntityRecord {
    /// Identity; monotonic, never reused.
    pub id: EntityId,
    /// Owning slot (the neutral sentinel for unowned entities).
    pub owner: PlayerId,
    /// Which kind template this entity was spawned from.
    pub kind: KindId,
    /// Position in tile units.
    pub pos: Vec2Fx,
    /// Facing direction vector; zero until the entity first moves (plan §5.8).
    pub facing: Vec2Fx,
    /// Lifecycle stage.
    pub lifecycle: Lifecycle,
    /// Pending orders, front first. Replaced or appended by commands (plan §8.1
    /// `queue`).
    pub orders: Vec<Order>,
}

impl EntityRecord {
    /// The movement-system state derived from the order queue — a small explicit
    /// enum per system, never a stored bool that can drift (plan §7.2).
    pub fn move_state(&self) -> MoveState {
        if self.orders.is_empty() {
            MoveState::Idle
        } else {
            MoveState::Moving
        }
    }
}

/// Per-player runtime state: controller kind, resignation flag, resource ledger,
/// population. Ledgers are `Vec`s in ascending [`ResourceId`] order (the fixture
/// defines them sorted; `World::new` enforces it).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayerState {
    /// The slot.
    pub player: PlayerId,
    /// What drives the slot (recorded into replays, plan §6.5).
    pub controller: ControllerKind,
    /// Whether the player resigned.
    pub resigned: bool,
    /// Resource balances, ascending by id.
    pub resources: Vec<(ResourceId, i64)>,
    /// Population usage (zero until M5).
    pub population: u32,
    /// Population cap (zero until M5).
    pub population_cap: u32,
}

impl PlayerState {
    /// Builds a player's state from a setup entry and the fixture's registry.
    pub fn from_setup(setup: &PlayerSetup, resources: &[(ResourceId, i64)]) -> Self {
        let mut ledger = resources.to_vec();
        ledger.sort_by_key(|(id, _)| *id);
        Self {
            player: setup.player,
            controller: setup.controller,
            resigned: false,
            resources: ledger,
            population: 0,
            population_cap: 0,
        }
    }

    /// The player's balance of one resource (zero when unknown).
    pub fn balance(&self, resource: ResourceId) -> i64 {
        self.resources
            .iter()
            .find(|(id, _)| *id == resource)
            .map(|(_, amount)| *amount)
            .unwrap_or(0)
    }

    /// Whether the ledger can pay every entry of the cost (plan §9.3). Entries
    /// for unknown resources or with non-positive amounts are no-ops — costs
    /// are validated non-negative at load, so this is defensive only.
    pub fn can_afford(&self, cost: &[(ResourceId, i64)]) -> bool {
        cost.iter()
            .all(|(resource, amount)| *amount <= 0 || self.balance(*resource) >= *amount)
    }

    /// Deducts a cost already found affordable (plan §9.3 "spend"). Saturating
    /// subtraction keeps extremes defined; callers gate on [`Self::can_afford`]
    /// first, so the balance stays non-negative (the A12 invariant).
    pub fn spend(&mut self, cost: &[(ResourceId, i64)]) {
        for (resource, amount) in cost {
            if *amount <= 0 {
                continue;
            }
            if let Some(entry) = self.resources.iter_mut().find(|(id, _)| id == resource) {
                entry.1 = entry.1.saturating_sub(*amount);
            }
        }
    }

    /// Returns a previously spent cost (plan §9.3 "refund" — queue
    /// cancellation). Saturating addition: a refund never overflows the ledger.
    pub fn refund(&mut self, cost: &[(ResourceId, i64)]) {
        for (resource, amount) in cost {
            if *amount <= 0 {
                continue;
            }
            if let Some(entry) = self.resources.iter_mut().find(|(id, _)| id == resource) {
                entry.1 = entry.1.saturating_add(*amount);
            }
        }
    }
}

/// The whole mutable world: entity store, one store per capability type, players.
/// Every store keeps the ascending-id invariant, which makes all iteration
/// deterministic (plan §5.3) without any ordered-map bookkeeping.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct World {
    /// Every live entity, ascending by id.
    pub entities: Vec<EntityRecord>,
    /// Health capability store, ascending by id.
    pub health: Vec<(EntityId, HealthDef)>,
    /// Move capability store, ascending by id.
    pub movement: Vec<(EntityId, MoveDef)>,
    /// Vision capability store, ascending by id.
    pub vision: Vec<(EntityId, VisionDef)>,
    /// Player states, ascending by slot.
    pub players: Vec<PlayerState>,
    /// How many kind templates the loaded fixture defines — the ceiling for valid
    /// [`pandemonium_sim_api::KindId`] values in the command gate. Set once by
    /// `Sim::new` from the fixture; the loader becomes the source in M2.
    pub kind_count: usize,
}

impl World {
    /// An empty world (used by `Sim::new` before spawning).
    pub fn new() -> Self {
        Self::default()
    }

    /// Index of an entity in the store, by id (binary search on the invariant).
    pub fn entity_index(&self, id: EntityId) -> Option<usize> {
        self.entities.binary_search_by_key(&id, |e| e.id).ok()
    }

    /// Read one entity by id.
    pub fn entity(&self, id: EntityId) -> Option<&EntityRecord> {
        self.entity_index(id).map(|i| &self.entities[i])
    }

    /// Mutable access to one entity by id.
    pub fn entity_mut(&mut self, id: EntityId) -> Option<&mut EntityRecord> {
        match self.entities.binary_search_by_key(&id, |e| e.id) {
            Ok(i) => Some(&mut self.entities[i]),
            Err(_) => None,
        }
    }

    /// Health data for an entity.
    pub fn health_of(&self, id: EntityId) -> Option<&HealthDef> {
        self.health
            .binary_search_by_key(&id, |(eid, _)| *eid)
            .ok()
            .map(|i| &self.health[i].1)
    }

    /// Mutable health data for an entity.
    pub fn health_of_mut(&mut self, id: EntityId) -> Option<&mut HealthDef> {
        match self.health.binary_search_by_key(&id, |(eid, _)| *eid) {
            Ok(i) => Some(&mut self.health[i].1),
            Err(_) => None,
        }
    }

    /// Move data for an entity.
    pub fn move_of(&self, id: EntityId) -> Option<&MoveDef> {
        self.movement
            .binary_search_by_key(&id, |(eid, _)| *eid)
            .ok()
            .map(|i| &self.movement[i].1)
    }

    /// Mutable move data for an entity.
    pub fn move_of_mut(&mut self, id: EntityId) -> Option<&mut MoveDef> {
        match self.movement.binary_search_by_key(&id, |(eid, _)| *eid) {
            Ok(i) => Some(&mut self.movement[i].1),
            Err(_) => None,
        }
    }

    /// Vision data for an entity.
    pub fn vision_of(&self, id: EntityId) -> Option<&VisionDef> {
        self.vision
            .binary_search_by_key(&id, |(eid, _)| *eid)
            .ok()
            .map(|i| &self.vision[i].1)
    }

    /// True when the entity carries the given capability.
    pub fn has_move(&self, id: EntityId) -> bool {
        self.move_of(id).is_some()
    }

    /// Player state by slot.
    pub fn player(&self, player: PlayerId) -> Option<&PlayerState> {
        self.players
            .binary_search_by_key(&player, |p| p.player)
            .ok()
            .map(|i| &self.players[i])
    }

    /// Mutable player state by slot.
    pub fn player_mut(&mut self, player: PlayerId) -> Option<&mut PlayerState> {
        match self.players.binary_search_by_key(&player, |p| p.player) {
            Ok(i) => Some(&mut self.players[i]),
            Err(_) => None,
        }
    }

    /// Spawns an entity: allocates nothing (the caller supplies the id, keeping
    /// the monotonic allocator in `Sim` the single source of identity), inserts
    /// the base record and every capability block. Because ids arrive in
    /// ascending order, each append preserves the store invariant.
    pub fn spawn(
        &mut self,
        id: EntityId,
        owner: PlayerId,
        kind: KindId,
        pos: Vec2Fx,
        caps: Vec<CapabilityData>,
    ) {
        debug_assert!(self.entity(id).is_none(), "id {id:?} already present");
        debug_assert!(
            self.entities.last().is_none_or(|last| last.id < id),
            "spawn out of ascending order would break the store invariant"
        );
        self.entities.push(EntityRecord {
            id,
            owner,
            kind,
            pos,
            facing: Vec2Fx::ZERO,
            lifecycle: Lifecycle::Active,
            orders: Vec::new(),
        });
        // Capability blocks are inserted in the fixture's fixed order; each store
        // stays ascending because ids are monotonic.
        for cap in caps {
            match cap {
                CapabilityData::Health(def) => self.health.push((id, def)),
                CapabilityData::Move(def) => self.movement.push((id, def)),
                CapabilityData::Vision(def) => self.vision.push((id, def)),
            }
        }
    }

    /// Removes an entity from the base record and every capability store.
    /// `Vec::remove` shifts survivors down without reordering them, so the
    /// ascending-id invariant — and therefore deterministic iteration — survives
    /// removal (plan §7.3 requirement b).
    pub fn remove(&mut self, id: EntityId) {
        if let Ok(i) = self.entities.binary_search_by_key(&id, |e| e.id) {
            self.entities.remove(i);
        }
        if let Ok(i) = self.health.binary_search_by_key(&id, |(eid, _)| *eid) {
            self.health.remove(i);
        }
        if let Ok(i) = self.movement.binary_search_by_key(&id, |(eid, _)| *eid) {
            self.movement.remove(i);
        }
        if let Ok(i) = self.vision.binary_search_by_key(&id, |(eid, _)| *eid) {
            self.vision.remove(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_helper(world: &mut World, id: u64, caps: Vec<CapabilityData>) {
        world.spawn(EntityId(id), PlayerId(0), KindId(0), Vec2Fx::ZERO, caps);
    }

    #[test]
    fn stores_iterate_ascending_by_construction() {
        let mut world = World::new();
        for id in [1u64, 2, 3, 5, 8] {
            spawn_helper(
                &mut world,
                id,
                vec![
                    CapabilityData::Health(HealthDef {
                        max_hp: 10,
                        hp: 10,
                        regen_per_tick: 0,
                    }),
                    CapabilityData::Move(MoveDef::new(Fx::from_milli(50), Fx::from_milli(350))),
                ],
            );
        }
        let ids: Vec<EntityId> = world.entities.iter().map(|e| e.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        let health_ids: Vec<EntityId> = world.health.iter().map(|(id, _)| *id).collect();
        assert!(health_ids.windows(2).all(|w| w[0] < w[1]));
        let move_ids: Vec<EntityId> = world.movement.iter().map(|(id, _)| *id).collect();
        assert!(move_ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn removal_never_reorders_survivors() {
        let mut world = World::new();
        for id in [1u64, 2, 3, 4, 5] {
            spawn_helper(
                &mut world,
                id,
                vec![CapabilityData::Move(MoveDef::new(
                    Fx::from_milli(50),
                    Fx::from_milli(350),
                ))],
            );
        }
        let before: Vec<EntityId> = world.movement.iter().map(|(id, _)| *id).collect();
        world.remove(EntityId(3));
        let after: Vec<EntityId> = world.movement.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            after,
            vec![EntityId(1), EntityId(2), EntityId(4), EntityId(5)]
        );
        // Survivors keep their relative order exactly.
        assert_eq!(
            before
                .iter()
                .filter(|id| **id != EntityId(3))
                .copied()
                .collect::<Vec<_>>(),
            after
        );
        assert!(world.entity(EntityId(3)).is_none());
        assert!(world.move_of(EntityId(3)).is_none());
    }

    #[test]
    fn lookups_are_by_id_only() {
        let mut world = World::new();
        spawn_helper(
            &mut world,
            7,
            vec![CapabilityData::Vision(VisionDef {
                radius: Fx::from_milli(7000),
            })],
        );
        assert!(world.entity(EntityId(7)).is_some());
        assert!(world.entity(EntityId(8)).is_none());
        assert!(world.vision_of(EntityId(7)).is_some());
        assert!(world.vision_of(EntityId(9)).is_none());
        assert!(!world.has_move(EntityId(7)));
    }

    #[test]
    fn player_state_ledger_is_sorted_and_queryable() {
        let setup = PlayerSetup {
            player: PlayerId(1),
            controller: ControllerKind::Ai,
        };
        let registry = vec![(ResourceId(3), 10), (ResourceId(1), 20)];
        let state = PlayerState::from_setup(&setup, &registry);
        assert_eq!(state.balance(ResourceId(1)), 20);
        assert_eq!(state.balance(ResourceId(3)), 10);
        assert_eq!(state.balance(ResourceId(9)), 0);
        assert!(!state.resigned);
    }

    #[test]
    fn ledger_can_afford_spend_and_refund_round_trip() {
        let setup = PlayerSetup {
            player: PlayerId(0),
            controller: ControllerKind::Human,
        };
        let mut state = PlayerState::from_setup(&setup, &[(ResourceId(0), 200)]);
        let worker_cost = [(ResourceId(0), 50)];
        assert!(state.can_afford(&worker_cost));
        state.spend(&worker_cost);
        assert_eq!(state.balance(ResourceId(0)), 150);
        state.refund(&worker_cost);
        assert_eq!(state.balance(ResourceId(0)), 200);
        // Beyond the balance: not affordable, and spending is the caller's
        // gate — the refund path is the only way money comes back.
        let huge = [(ResourceId(0), 201)];
        assert!(!state.can_afford(&huge));
    }

    #[test]
    fn ledger_costs_are_per_resource_and_empty_costs_are_free() {
        let setup = PlayerSetup {
            player: PlayerId(0),
            controller: ControllerKind::Human,
        };
        // Two resources: a second resource is data-only (plan §9.3).
        let mut state = PlayerState::from_setup(&setup, &[(ResourceId(0), 30), (ResourceId(1), 5)]);
        let mixed = [(ResourceId(0), 30), (ResourceId(1), 5)];
        assert!(state.can_afford(&mixed));
        let short = [(ResourceId(0), 30), (ResourceId(1), 6)];
        assert!(!state.can_afford(&short), "every entry must be payable");
        state.spend(&mixed);
        assert_eq!(state.balance(ResourceId(0)), 0);
        assert_eq!(state.balance(ResourceId(1)), 0);
        // Unknown resources carry a zero balance, so a positive cost on one
        // is unpayable — the loader only emits registry-mapped costs, this
        // pins the defensive fail-closed behavior.
        assert!(!state.can_afford(&[(ResourceId(9), 999)]));
        assert!(state.can_afford(&[]));
        state.spend(&[(ResourceId(9), 999), (ResourceId(0), -5)]);
        assert_eq!(state.balance(ResourceId(0)), 0);
    }

    #[test]
    fn ledger_extremes_saturate_instead_of_panicking() {
        let setup = PlayerSetup {
            player: PlayerId(0),
            controller: ControllerKind::Human,
        };
        let mut state = PlayerState::from_setup(&setup, &[(ResourceId(0), i64::MAX)]);
        state.refund(&[(ResourceId(0), 1)]);
        assert_eq!(state.balance(ResourceId(0)), i64::MAX);
        let mut poor = PlayerState::from_setup(&setup, &[(ResourceId(0), 0)]);
        poor.spend(&[(ResourceId(0), i64::MAX)]);
        assert_eq!(
            poor.balance(ResourceId(0)),
            -i64::MAX,
            "saturates, never wraps"
        );
        // Saturating spend floors at the minimum; the gate keeps real ledgers
        // non-negative, this pins the defined behavior at the extreme.
        assert!(poor.balance(ResourceId(0)) <= 0);
    }
}
