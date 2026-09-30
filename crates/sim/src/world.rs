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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

/// Move capability in runtime units.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MoveDef {
    /// Top speed per tick in fixed-point tile units (already converted from
    /// authoring milli-tiles per second, plan §10.2).
    pub speed_per_tick: Fx,
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
                    CapabilityData::Move(MoveDef {
                        speed_per_tick: Fx::from_milli(50),
                    }),
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
                vec![CapabilityData::Move(MoveDef {
                    speed_per_tick: Fx::from_milli(50),
                })],
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
}
