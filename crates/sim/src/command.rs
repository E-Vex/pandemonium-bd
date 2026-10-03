//! The command gate: validation and application (plan §8.2, §6.3 stage 1).
//!
//! One shared gate serves every issuer — human, AI, replay, and (later) network
//! (FD-2, FD-7): commands arrive at a tick boundary, are sorted by
//! `(issuer slot, seq)`, validated in that order, and applied in that order.
//! Invalid commands emit [`Event::CommandRejected`] and change no state.
//!
//! Validation checks, in order: tick match → issuer exists → sequence not reused
//! this tick → per-kind checks (referenced entities exist, are owned by the
//! issuer, carry the required capabilities; targets exist and are legal) → the
//! economy checks of plan §8.2 (requirements, affordability, population,
//! placement) for the M5 commands (A-014).
//!
//! The economy checks read the world *and* the loaded content (kind costs,
//! production lists, requirements) through the fixture handed in by
//! [`crate::Sim::step`]. The placement command (Build) arrives with the
//! construction system (the M5 construction commit); Attack/AttackMove arrive
//! with the combat pipeline (M6 — the gate checks the Attack capability per
//! unit and the target's visibility per issuer).

use pandemonium_sim_api::{
    Command, CommandKind, EntityId, Event, KindId, PlayerId, Reject, RejectReason, TilePos, Vec2Fx,
};

use crate::fixture::TrivialWorld;
use crate::production::{self, kind_economy, producible_by, requirements_met};
use crate::world::{Order, World};

/// Applies one tick's commands to the world (plan §6.3 stage 1).
///
/// Commands are sorted by `(issuer, seq)` — a *stable* sort, so commands that
/// share the key keep their feed order; the first one wins and later duplicates
/// are rejected as [`RejectReason::DuplicateSeq`] (deterministic no matter how the
/// caller ordered them). `content` is the loaded fixture — the economy checks
/// read kind costs, production lists, and requirements from it.
pub(crate) fn apply_commands(
    world: &mut World,
    content: &TrivialWorld,
    nav: &mut crate::nav::NavGrid,
    next_entity_id: &mut u64,
    tick: u32,
    commands: &[Command],
    events: &mut Vec<Event>,
) {
    let mut sorted: Vec<&Command> = commands.iter().collect();
    sorted.sort_by_key(|cmd| cmd.order_key());

    // Sequence numbers already consumed this tick, per issuer — the duplicate gate.
    let mut used_seq: Vec<(PlayerId, u32)> = Vec::new();

    for cmd in sorted {
        // 1. Tick boundary: every command must be for the tick being applied.
        if cmd.tick != tick {
            rejected(events, cmd, RejectReason::TickMismatch);
            continue;
        }
        // 2. Issuer must be a player of this match (the neutral sentinel never is).
        if world.player(cmd.issuer).is_none() {
            rejected(events, cmd, RejectReason::PlayerMissing);
            continue;
        }
        // 3. Sequence reuse within the tick is a client bug; refuse deterministically.
        if used_seq.contains(&(cmd.issuer, cmd.seq)) {
            rejected(events, cmd, RejectReason::DuplicateSeq);
            continue;
        }
        used_seq.push((cmd.issuer, cmd.seq));

        match validate(world, content, nav, cmd) {
            Err(reason) => rejected(events, cmd, reason),
            Ok(()) => apply_valid(world, content, nav, next_entity_id, cmd, events),
        }
    }
}

/// Emits the rejection event — the only effect an invalid command may have.
fn rejected(events: &mut Vec<Event>, cmd: &Command, reason: RejectReason) {
    events.push(Event::CommandRejected {
        issuer: cmd.issuer,
        seq: cmd.seq,
        reject: Reject::new(reason),
    });
}

/// Per-kind validation: every referenced entity exists, is owned by the issuer,
/// and carries the required capability; targets exist and are legal; the economy
/// checks (requirements, affordability, population) gate the M5 commands.
fn validate(
    world: &World,
    content: &TrivialWorld,
    nav: &crate::nav::NavGrid,
    cmd: &Command,
) -> Result<(), RejectReason> {
    match &cmd.kind {
        CommandKind::Move { units, .. } => {
            // Move requires only the Move capability — pure locomotion.
            check_units(world, cmd.issuer, units, |id| {
                if world.has_move(id) {
                    Ok(())
                } else {
                    Err(RejectReason::MissingCapability)
                }
            })?;
            Ok(())
        }
        CommandKind::AttackMove { units, .. } => {
            // AttackMove requires both Move (for the movement leg) and Attack
            // (for engaging enemies en route — plan §9.2, M6). Units without
            // Attack can still be ordered to Move; AttackMove is the stricter
            // command. A unit with Attack but no Move (e.g. the Turret) is
            // refused here — it cannot move to the destination. The turret
            // can still receive an Attack command on a specific target.
            check_units(world, cmd.issuer, units, |id| {
                if !world.has_move(id) {
                    return Err(RejectReason::MissingCapability);
                }
                if !world.has_attack(id) {
                    return Err(RejectReason::MissingCapability);
                }
                Ok(())
            })?;
            Ok(())
        }
        CommandKind::Stop { units } => {
            // Stopping is always legal for owned entities: an entity with no
            // orders (or no mover) is already stopped — a valid no-op. Stop
            // also clears any Attack target slot (combat.rs's clear_dead_targets
            // drops dead targets; Stop drops live ones the player wants gone).
            check_units(world, cmd.issuer, units, |_| Ok(()))?;
            Ok(())
        }
        CommandKind::Attack { units, target } => {
            // Attack requires the Attack capability per unit (plan §9.2, M6).
            // The target must exist and be visible to the issuer (FD-8: fog
            // never alters the simulation — hidden targets are rejected at
            // the gate, never quietly attacked).
            check_units(world, cmd.issuer, units, |id| {
                if world.has_attack(id) {
                    Ok(())
                } else {
                    Err(RejectReason::MissingCapability)
                }
            })?;
            check_target(world, cmd.issuer, *target)?;
            // Cannot attack your own entity (the gate's NotVisible check would
            // already pass for own entities, since friendlies are always
            // visible — so an explicit ownership check is the right gate here).
            if world.entity(*target).is_some_and(|t| t.owner == cmd.issuer) {
                return Err(RejectReason::InvalidTarget);
            }
            Ok(())
        }
        CommandKind::Gather { units, node } => {
            check_units(world, cmd.issuer, units, |id| {
                if world.gather_of(id).is_some() {
                    Ok(())
                } else {
                    Err(RejectReason::MissingCapability)
                }
            })?;
            // The target must be a live resource node (an entity carrying a
            // Resource body — plan §9.3). Fog filtering of gather targets
            // arrives with the vision work (M6; A-048).
            match world.entity(*node) {
                None => Err(RejectReason::InvalidTarget),
                Some(_) if world.resource_of(*node).is_none() => Err(RejectReason::InvalidTarget),
                Some(_) => Ok(()),
            }
        }
        CommandKind::Build {
            worker,
            structure,
            at,
        } => {
            check_single_entity(world, cmd.issuer, *worker)?;
            // Only builders build (plan §7.4).
            if world.build_of(*worker).is_none() {
                return Err(RejectReason::MissingCapability);
            }
            check_kind(world, *structure)?;
            // Only structures are built (the Train mirror — A-047).
            if !has_footprint_kind(content, *structure) {
                return Err(RejectReason::InvalidTarget);
            }
            check_economy(world, content, cmd.issuer, *structure)?;
            check_placement(world, content, nav, *structure, *at)
        }
        CommandKind::Train { producer, unit } => {
            check_single_entity(world, cmd.issuer, *producer)?;
            check_producer(world, *producer)?;
            check_kind(world, *unit)?;
            // Structures are built, not trained (A-047).
            if world
                .entity(*producer)
                .is_some_and(|_| has_footprint_kind(content, *unit))
            {
                return Err(RejectReason::InvalidTarget);
            }
            // The faction's production list decides what this producer trains.
            let producer_kind = world.entity(*producer).expect("checked").kind;
            if !producible_by(content, producer_kind, *unit) {
                return Err(RejectReason::MissingCapability);
            }
            check_economy(world, content, cmd.issuer, *unit)
        }
        CommandKind::CancelQueueItem { producer, index } => {
            check_single_entity(world, cmd.issuer, *producer)?;
            check_producer(world, *producer)?;
            // The index must land inside the producer's queue.
            let in_range = world
                .produce_of(*producer)
                .is_some_and(|def| (*index as usize) < def.queue.len());
            if in_range {
                Ok(())
            } else {
                Err(RejectReason::QueueIndexInvalid)
            }
        }
        CommandKind::SetRally { producer, .. } => {
            check_single_entity(world, cmd.issuer, *producer)?;
            check_producer(world, *producer)
        }
        CommandKind::Resign {} => {
            // Issuer existence was checked at the gate; resigning twice is a
            // harmless, deterministic no-op.
            Ok(())
        }
    }
}

/// Applies a validated command (plan §8.2: apply valid ones). Units within one
/// command are processed in ascending id order — deduplicated, sorted — so
/// application order never depends on the list order the issuer happened to send
/// (plan §5.3). The economy applications (Train/Cancel/SetRally) read the
/// content through the production system's helpers.
fn apply_valid(
    world: &mut World,
    content: &TrivialWorld,
    nav: &mut crate::nav::NavGrid,
    next_entity_id: &mut u64,
    cmd: &Command,
    events: &mut Vec<Event>,
) {
    match &cmd.kind {
        CommandKind::Move { units, target } => {
            for id in sorted_unique(units) {
                if let Some(entity) = world.entity_mut(id) {
                    if cmd.queue {
                        entity.orders.push(Order::MoveTo { target: *target });
                    } else {
                        // A replacing order restarts movement from scratch:
                        // the old path no longer serves the new destination.
                        entity.orders = vec![Order::MoveTo { target: *target }];
                        if let Some(def) = world.move_of_mut(id) {
                            def.reset_runtime();
                        }
                    }
                }
            }
        }
        CommandKind::AttackMove { units, target } => {
            // AttackMove pushes a MoveTo order (the engage-en-route semantics
            // are M6's combat work — the combat pipeline's auto-acquisition
            // will engage enemies within acquire_range as the unit moves).
            // A replacing order resets the runtime path like Move does.
            for id in sorted_unique(units) {
                if let Some(entity) = world.entity_mut(id) {
                    if cmd.queue {
                        entity.orders.push(Order::MoveTo { target: *target });
                    } else {
                        entity.orders = vec![Order::MoveTo { target: *target }];
                        if let Some(def) = world.move_of_mut(id) {
                            def.reset_runtime();
                        }
                    }
                }
            }
        }
        CommandKind::Stop { units } => {
            for id in sorted_unique(units) {
                if let Some(entity) = world.entity_mut(id) {
                    entity.orders.clear();
                }
                if let Some(def) = world.move_of_mut(id) {
                    def.reset_runtime();
                }
                // Clear the Attack target slot too — Stop means "stand down".
                if let Some(def) = world.attack_of_mut(id) {
                    def.clear_target();
                }
            }
        }
        CommandKind::Attack { units, target } => {
            for id in sorted_unique(units) {
                if let Some(entity) = world.entity_mut(id) {
                    if cmd.queue {
                        entity.orders.push(Order::AttackUnit { target: *target });
                    } else {
                        entity.orders = vec![Order::AttackUnit { target: *target }];
                        if let Some(def) = world.move_of_mut(id) {
                            def.reset_runtime();
                        }
                    }
                }
                // Set the Attack capability's target slot immediately so the
                // combat pipeline (stage 7, this same tick) can hit if in range.
                if let Some(def) = world.attack_of_mut(id) {
                    def.target = Some(*target);
                }
            }
        }
        CommandKind::Gather { units, node } => {
            for id in sorted_unique(units) {
                if let Some(entity) = world.entity_mut(id) {
                    if cmd.queue {
                        entity.orders.push(Order::GatherAt { node: *node });
                    } else {
                        // A replacing order restarts travel from scratch —
                        // the old lanes served the old intent.
                        entity.orders = vec![Order::GatherAt { node: *node }];
                        if let Some(def) = world.move_of_mut(id) {
                            def.reset_runtime();
                        }
                    }
                }
            }
        }
        CommandKind::Build {
            worker,
            structure,
            at,
        } => {
            build_site(
                world,
                content,
                nav,
                next_entity_id,
                cmd,
                *worker,
                *structure,
                *at,
                events,
            );
        }
        CommandKind::Train { producer, unit } => {
            let cost = kind_economy(content, *unit)
                .map(|economy| economy.cost.clone())
                .unwrap_or_default();
            production::enqueue(world, *producer, *unit, cost);
        }
        CommandKind::CancelQueueItem { producer, index } => {
            production::cancel(world, *producer, *index);
        }
        CommandKind::SetRally { producer, target } => {
            if let Some(def) = world.produce_of_mut(*producer) {
                def.rally = Some(*target);
            }
        }
        CommandKind::Resign {} => {
            if let Some(player) = world.player_mut(cmd.issuer) {
                player.resigned = true;
            }
        }
    }
}

/// Deduplicates and sorts a unit list ascending — the canonical processing order.
fn sorted_unique(units: &[EntityId]) -> Vec<EntityId> {
    let mut ids = units.to_vec();
    ids.sort();
    ids.dedup();
    ids
}

/// Existence + ownership for every unit, plus a per-unit capability requirement.
fn check_units(
    world: &World,
    issuer: PlayerId,
    units: &[EntityId],
    capability: impl Fn(EntityId) -> Result<(), RejectReason>,
) -> Result<(), RejectReason> {
    check_units_exist_and_owned(world, issuer, units)?;
    for id in sorted_unique(units) {
        capability(id)?;
    }
    Ok(())
}

/// Existence + ownership for every unit in the list.
fn check_units_exist_and_owned(
    world: &World,
    issuer: PlayerId,
    units: &[EntityId],
) -> Result<(), RejectReason> {
    for id in sorted_unique(units) {
        check_single_entity(world, issuer, id)?;
    }
    Ok(())
}

/// Existence + ownership for one entity.
fn check_single_entity(world: &World, issuer: PlayerId, id: EntityId) -> Result<(), RejectReason> {
    match world.entity(id) {
        None => Err(RejectReason::UnknownEntity),
        Some(entity) if entity.owner != issuer => Err(RejectReason::NotOwnedByIssuer),
        Some(_) => Ok(()),
    }
}

/// A producer must carry the Produce capability and be a completed structure
/// (sites are not open for business — A-046).
fn check_producer(world: &World, producer: EntityId) -> Result<(), RejectReason> {
    let Some(entity) = world.entity(producer) else {
        return Err(RejectReason::UnknownEntity);
    };
    if world.produce_of(producer).is_none() {
        return Err(RejectReason::MissingCapability);
    }
    if !matches!(entity.lifecycle, crate::world::Lifecycle::Active) {
        return Err(RejectReason::MissingCapability);
    }
    Ok(())
}

/// Whether a kind carries a Footprint (a structure) in the loaded content.
fn has_footprint_kind(content: &TrivialWorld, kind: KindId) -> bool {
    content.kinds.get(kind.0 as usize).is_some_and(|template| {
        template
            .caps
            .iter()
            .any(|cap| matches!(cap, crate::CapTemplate::Footprint { .. }))
    })
}

/// The shared economy checks for producing a kind (plan §8.2's affordability
/// and population, plus §9.4's requirement list): requirements met, cost
/// payable, population headroom at enqueue.
fn check_economy(
    world: &World,
    content: &TrivialWorld,
    issuer: PlayerId,
    kind: KindId,
) -> Result<(), RejectReason> {
    let Some(economy) = kind_economy(content, kind) else {
        return Err(RejectReason::UnknownKind);
    };
    // Requirements: one shared checker (plan §9.4).
    if !requirements_met(
        world,
        issuer,
        &economy.requires.iter().map(|r| r.0).collect::<Vec<_>>(),
    ) {
        return Err(RejectReason::RequirementsUnmet);
    }
    // Affordability.
    let Some(player) = world.player(issuer) else {
        return Err(RejectReason::PlayerMissing);
    };
    if !player.can_afford(&economy.cost) {
        return Err(RejectReason::CannotAfford);
    }
    // Population headroom at enqueue (A-042: current live usage; the spawn
    // re-checks and holds when the world changed in between).
    let pop = economy.population.max(0) as u32;
    if pop > 0 {
        let usage = production::population_usage(world, &content.kinds, issuer);
        let cap = production::population_cap(world, issuer, content.base_population_cap);
        if usage.saturating_add(pop) > cap {
            return Err(RejectReason::PopulationFull);
        }
    }
    Ok(())
}

/// Placement legality (plan §8.2): every footprint tile must be inside the
/// map, on buildable terrain, unclaimed by another static body, and free of
/// movers standing on it — classic "cannot place a building on units".
fn check_placement(
    world: &World,
    content: &TrivialWorld,
    nav: &crate::nav::NavGrid,
    structure: KindId,
    at: TilePos,
) -> Result<(), RejectReason> {
    let Some(footprint) = content.kinds.get(structure.0 as usize).and_then(|kind| {
        kind.caps.iter().find_map(|cap| match cap {
            crate::CapTemplate::Footprint { w, h } => Some((*w, *h)),
            _ => None,
        })
    }) else {
        return Err(RejectReason::InvalidTarget);
    };
    for dy in 0..footprint.1 as i32 {
        for dx in 0..footprint.0 as i32 {
            let x = at.x + dx;
            let y = at.y + dy;
            // Bounds and buildability come from the fixture's grid.
            let in_bounds = x >= 0
                && y >= 0
                && (x as u64) < content.width_tiles as u64
                && (y as u64) < content.height_tiles as u64;
            if !in_bounds {
                return Err(RejectReason::PlacementBlocked);
            }
            let index = y as usize * content.width_tiles as usize + x as usize;
            if content.buildability.get(index).copied().unwrap_or(0) == 0 {
                return Err(RejectReason::PlacementBlocked);
            }
            // Static bodies (nodes, structures, other sites) claim tiles.
            if nav.is_occupied(x, y) {
                return Err(RejectReason::PlacementBlocked);
            }
            // No mover may stand where the site lands.
            let mover_on_tile = world.entities.iter().any(|entity| {
                world.move_of(entity.id).is_some()
                    && entity.pos.x.floor_int() == x
                    && entity.pos.y.floor_int() == y
            });
            if mover_on_tile {
                return Err(RejectReason::PlacementBlocked);
            }
        }
    }
    Ok(())
}

/// Spawns a construction site (the Build command's application): pays the
/// cost, allocates the id, spawns the structure-to-be in
/// `UnderConstruction` with its full capability set plus the `Construction`
/// runtime block, claims the footprint tiles, orders the builder, and emits
/// `ConstructionStarted`. Sites take exact placement — no spawn jitter (a
/// shifted site would misalign its footprint).
#[allow(clippy::too_many_arguments)]
fn build_site(
    world: &mut World,
    content: &TrivialWorld,
    nav: &mut crate::nav::NavGrid,
    next_entity_id: &mut u64,
    cmd: &Command,
    worker: EntityId,
    structure: KindId,
    at: TilePos,
    events: &mut Vec<Event>,
) {
    let economy = kind_economy(content, structure).map(|economy| economy.cost.clone());
    // Pay first (the gate validated affordability).
    if let Some(player) = world.player_mut(cmd.issuer) {
        player.spend(&economy.unwrap_or_default());
    }
    let total_ticks = kind_economy(content, structure)
        .map(|economy| economy.build_time_ticks)
        .unwrap_or(0);
    let (w, h) = content
        .kinds
        .get(structure.0 as usize)
        .and_then(|kind| {
            kind.caps.iter().find_map(|cap| match cap {
                crate::CapTemplate::Footprint { w, h } => Some((*w, *h)),
                _ => None,
            })
        })
        .unwrap_or((1, 1));
    let id = EntityId(*next_entity_id);
    *next_entity_id += 1;
    let pos = Vec2Fx::new(
        pandemonium_fx::Fx::from_milli((at.x * 2 + w as i32) * 500),
        pandemonium_fx::Fx::from_milli((at.y * 2 + h as i32) * 500),
    );
    let caps: Vec<_> = content
        .kinds
        .get(structure.0 as usize)
        .map(|template| {
            template
                .caps
                .iter()
                .map(|cap| cap.to_runtime(crate::TICKS_PER_SECOND))
                .collect()
        })
        .unwrap_or_default();
    let caps_with_site = {
        let mut caps = caps;
        caps.push(crate::world::CapabilityData::Construction(
            crate::world::ConstructionDef {
                builder: worker,
                progress_ticks: 0,
                total_ticks,
            },
        ));
        caps
    };
    world.spawn(id, cmd.issuer, structure, pos, caps_with_site);
    if let Some(entity) = world.entity_mut(id) {
        entity.lifecycle = crate::world::Lifecycle::UnderConstruction;
    }
    // Claim the ground immediately (sites block tiles from their first tick).
    if let Some(footprint) = world.footprint_of(id) {
        for (x, y) in footprint.tiles(pos) {
            nav.occupy(x, y);
        }
    }
    // Order the builder.
    if let Some(entity) = world.entity_mut(worker) {
        if cmd.queue {
            entity.orders.push(Order::BuildAt { site: id });
        } else {
            entity.orders = vec![Order::BuildAt { site: id }];
            if let Some(def) = world.move_of_mut(worker) {
                def.reset_runtime();
            }
        }
    }
    events.push(Event::ConstructionStarted {
        builder: worker,
        site: id,
    });
}

/// Target legality (plan §8.2): exists and is visible to the issuer (plan §9.5).
fn check_target(world: &World, issuer: PlayerId, target: EntityId) -> Result<(), RejectReason> {
    if world.entity(target).is_none() {
        return Err(RejectReason::InvalidTarget);
    }
    if !visible_to(world, issuer, target) {
        return Err(RejectReason::NotVisible);
    }
    Ok(())
}

/// Kind legality: the kind must exist in the fixture's template list.
fn check_kind(world: &World, kind: KindId) -> Result<(), RejectReason> {
    if (kind.0 as usize) < world.kind_count {
        Ok(())
    } else {
        Err(RejectReason::UnknownKind)
    }
}

/// Fog-filtered visibility (plan §9.5, FD-8): an entity is visible to a player
/// when it is that player's own, or when it stands within the vision radius of
/// any of that player's entities that carry Vision. Squared-distance comparison
/// only — no roots (plan §5.8).
///
/// M1 recomputes this on demand; incremental vision maintenance arrives in M6
/// (see docs/DEBT.md).
pub(crate) fn visible_to(world: &World, viewer: PlayerId, target: EntityId) -> bool {
    let Some(target_entity) = world.entity(target) else {
        return false;
    };
    if target_entity.owner == viewer {
        return true;
    }
    for (eid, def) in &world.vision {
        let Some(eye) = world.entity(*eid) else {
            continue;
        };
        if eye.owner != viewer {
            continue;
        }
        let radius_raw = def.radius.raw().max(0) as u64;
        let r_sq = radius_raw * radius_raw;
        let d_sq = (eye.pos - target_entity.pos).len_sq_raw();
        if d_sq <= r_sq {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{CapTemplate, KindEconomy, KindTemplate};
    use crate::world::{CapabilityData, HealthDef, MoveDef, PlayerState, VisionDef};
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::{ControllerKind, PlayerSetup, ResourceId, Vec2Fx};

    fn target(x: i32, y: i32) -> Vec2Fx {
        Vec2Fx::from_ints(x, y)
    }

    fn demo_world() -> World {
        let mut world = World::new();
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        // id 1: mover with vision; id 2: watcher (vision only, cannot move);
        // id 3: enemy mover belonging to player 1 (absent from players on purpose:
        // a player slot and ownership are independent).
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(0, 0),
            vec![
                CapabilityData::Health(HealthDef {
                    max_hp: 10,
                    hp: 10,
                    regen_per_tick: 0,
                }),
                CapabilityData::Move(MoveDef::new(Fx::from_milli(100), Fx::from_milli(350))),
                CapabilityData::Vision(VisionDef {
                    radius: Fx::from_milli(7000),
                }),
            ],
        );
        world.spawn(
            EntityId(2),
            PlayerId(0),
            KindId(1),
            Vec2Fx::from_ints(20, 0),
            vec![CapabilityData::Vision(VisionDef {
                radius: Fx::from_milli(9000),
            })],
        );
        world.spawn(
            EntityId(3),
            PlayerId(1),
            KindId(0),
            Vec2Fx::from_ints(5, 0),
            vec![
                CapabilityData::Move(MoveDef::new(Fx::from_milli(100), Fx::from_milli(350))),
                CapabilityData::Vision(VisionDef {
                    radius: Fx::from_milli(7000),
                }),
            ],
        );
        world.kind_count = 5;
        world
    }

    /// A fixture matching the demo world's kinds (0 mover, 1 watcher) plus an
    /// economy surface: kind 2 produces from kind 0's list, kind 2 costs 150,
    /// kind 3 costs 50 and pop 1, kind 4 costs 200 and requires kind 2.
    fn demo_fixture() -> TrivialWorld {
        TrivialWorld {
            map_id: 0x0000_C0DE,
            width_tiles: 16,
            height_tiles: 16,
            passability: TrivialWorld::open_passability(16, 16),
            buildability: TrivialWorld::open_buildability(16, 16),
            kinds: vec![
                KindTemplate::from_caps(vec![
                    CapTemplate::Health {
                        max_hp: 10,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 100,
                        radius_milli_tiles: 350,
                    },
                ]),
                KindTemplate::from_caps(vec![CapTemplate::Vision {
                    radius_milli_tiles: 9000,
                }]),
                // Kind 2: a producer structure.
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 300,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Footprint { w: 2, h: 2 },
                        CapTemplate::Produce {},
                    ],
                    economy: KindEconomy {
                        cost: vec![(ResourceId(0), 150)],
                        build_time_ticks: 30,
                        population: 0,
                        requires: vec![],
                    },
                },
                // Kind 3: a producible unit costing 50, pop 1, requires kind 2.
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 40,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Move {
                            speed_milli_tiles_per_s: 100,
                            radius_milli_tiles: 300,
                        },
                    ],
                    economy: KindEconomy {
                        cost: vec![(ResourceId(0), 50)],
                        build_time_ticks: 10,
                        population: 1,
                        requires: vec![KindId(2)],
                    },
                },
                // Kind 4: unproducible here (cost 200 — beyond the 200 balance
                // after any spend), pop 0.
                KindTemplate {
                    economy: KindEconomy {
                        cost: vec![(ResourceId(0), 200)],
                        build_time_ticks: 10,
                        population: 0,
                        requires: vec![],
                    },
                    ..KindTemplate::from_caps(vec![])
                },
            ],
            resources: vec![],
            production: vec![(KindId(2), vec![KindId(3), KindId(4)])],
            base_population_cap: 10,
            initial_spawns: vec![],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    fn cmd(issuer: u8, tick: u32, seq: u32, kind: CommandKind) -> Command {
        Command::new(PlayerId(issuer), tick, seq, kind)
    }

    fn apply(world: &mut World, tick: u32, commands: &[Command]) -> Vec<Event> {
        let fixture = demo_fixture();
        let mut nav = crate::nav::NavGrid::new(
            fixture.width_tiles,
            fixture.height_tiles,
            &fixture.passability,
        );
        let mut next_id = 100u64;
        let mut events = Vec::new();
        apply_commands(
            world,
            &fixture,
            &mut nav,
            &mut next_id,
            tick,
            commands,
            &mut events,
        );
        events
    }

    fn reason_of(events: &[Event]) -> Option<RejectReason> {
        events.first().and_then(|event| match event {
            Event::CommandRejected { reject, .. } => Some(reject.reason),
            _ => None,
        })
    }

    #[test]
    fn valid_move_replaces_order_queue() {
        let mut world = demo_world();
        apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                1,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: target(3, 3),
                },
            )],
        );
        assert_eq!(
            world.entity(EntityId(1)).unwrap().orders,
            vec![Order::MoveTo {
                target: target(3, 3)
            }]
        );
    }

    #[test]
    fn queued_move_appends_instead_of_replacing() {
        let mut world = demo_world();
        let first = cmd(
            0,
            0,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: target(3, 3),
            },
        );
        let mut second = cmd(
            0,
            0,
            2,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: target(6, 6),
            },
        );
        second.queue = true;
        apply(&mut world, 0, &[first, second]);
        assert_eq!(world.entity(EntityId(1)).unwrap().orders.len(), 2);
    }

    #[test]
    fn stop_clears_orders() {
        let mut world = demo_world();
        apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                1,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: target(3, 3),
                },
            )],
        );
        apply(
            &mut world,
            1,
            &[cmd(
                0,
                1,
                2,
                CommandKind::Stop {
                    units: vec![EntityId(1)],
                },
            )],
        );
        assert!(world.entity(EntityId(1)).unwrap().orders.is_empty());
    }

    #[test]
    fn stale_and_future_ticks_are_refused() {
        let mut world = demo_world();
        let events = apply(
            &mut world,
            7,
            &[cmd(
                0,
                6,
                1,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: target(1, 1),
                },
            )],
        );
        assert_eq!(reason_of(&events), Some(RejectReason::TickMismatch));
        let events = apply(
            &mut world,
            7,
            &[cmd(
                0,
                8,
                2,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: target(1, 1),
                },
            )],
        );
        assert_eq!(reason_of(&events), Some(RejectReason::TickMismatch));
    }

    #[test]
    fn unknown_player_and_entity_and_ownership_are_refused() {
        let mut world = demo_world();
        let events = apply(&mut world, 0, &[cmd(9, 0, 1, CommandKind::Resign {})]);
        assert_eq!(reason_of(&events), Some(RejectReason::PlayerMissing));

        let events = apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                2,
                CommandKind::Move {
                    units: vec![EntityId(99)],
                    target: target(1, 1),
                },
            )],
        );
        assert_eq!(reason_of(&events), Some(RejectReason::UnknownEntity));

        let events = apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                3,
                CommandKind::Move {
                    units: vec![EntityId(3)],
                    target: target(1, 1),
                },
            )],
        );
        assert_eq!(reason_of(&events), Some(RejectReason::NotOwnedByIssuer));
    }

    #[test]
    fn capability_presence_is_enforced() {
        let mut world = demo_world();
        // The watcher has no Move capability.
        let events = apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                1,
                CommandKind::Move {
                    units: vec![EntityId(2)],
                    target: target(1, 1),
                },
            )],
        );
        assert_eq!(reason_of(&events), Some(RejectReason::MissingCapability));

        // No entity can attack in M1 (the Attack capability variant is M6).
        let events = apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                2,
                CommandKind::Attack {
                    units: vec![EntityId(1)],
                    target: EntityId(3),
                },
            )],
        );
        assert_eq!(reason_of(&events), Some(RejectReason::MissingCapability));
    }

    #[test]
    fn duplicate_sequence_is_refused_exactly_once() {
        let mut world = demo_world();
        let a = cmd(
            0,
            0,
            5,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: target(1, 1),
            },
        );
        let b = cmd(
            0,
            0,
            5,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: target(2, 2),
            },
        );
        let events = apply(&mut world, 0, &[a.clone(), b.clone()]);
        // First applied, second refused — regardless of feed order.
        assert_eq!(events.len(), 1);
        assert_eq!(reason_of(&events), Some(RejectReason::DuplicateSeq));
        assert_eq!(
            world.entity(EntityId(1)).unwrap().orders,
            vec![Order::MoveTo {
                target: target(1, 1)
            }]
        );
        let events = apply(&mut world, 1, &[b, a]);
        // Both carry tick 0 while the gate applies tick 1: the tick check fires
        // before the duplicate check — order of gate checks is itself pinned here.
        assert_eq!(reason_of(&events), Some(RejectReason::TickMismatch));
        let tick_one_dupes = [
            Command::new(
                PlayerId(0),
                1,
                5,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: target(2, 2),
                },
            ),
            Command::new(
                PlayerId(0),
                1,
                5,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: target(4, 4),
                },
            ),
        ];
        let events = apply(&mut world, 1, &tick_one_dupes);
        assert_eq!(reason_of(&events), Some(RejectReason::DuplicateSeq));
    }

    #[test]
    fn unknown_kinds_are_refused_for_train_and_build() {
        let mut world = demo_world();
        let events = apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                1,
                CommandKind::Train {
                    producer: EntityId(1),
                    unit: KindId(99),
                },
            )],
        );
        // Producer lacks the (future) Produce capability — refused before kind check.
        assert_eq!(reason_of(&events), Some(RejectReason::MissingCapability));
        let events = apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                2,
                CommandKind::Build {
                    worker: EntityId(1),
                    structure: KindId(99),
                    at: Default::default(),
                },
            )],
        );
        assert_eq!(reason_of(&events), Some(RejectReason::MissingCapability));
    }

    #[test]
    fn resign_marks_the_player() {
        let mut world = demo_world();
        apply(&mut world, 0, &[cmd(0, 0, 1, CommandKind::Resign {})]);
        assert!(world.player(PlayerId(0)).unwrap().resigned);
    }

    #[test]
    fn units_are_processed_in_ascending_id_order() {
        let mut world = demo_world();
        // Entity 4 joins as a second mover for player 0.
        world.spawn(
            EntityId(4),
            PlayerId(0),
            KindId(0),
            Vec2Fx::from_ints(9, 9),
            vec![CapabilityData::Move(MoveDef::new(
                Fx::from_milli(100),
                Fx::from_milli(350),
            ))],
        );
        apply(
            &mut world,
            0,
            &[cmd(
                0,
                0,
                1,
                CommandKind::Move {
                    units: vec![EntityId(4), EntityId(1), EntityId(4)],
                    target: target(1, 1),
                },
            )],
        );
        // Duplicates collapse; both movers carry the order.
        assert_eq!(
            world.entity(EntityId(1)).unwrap().orders,
            vec![Order::MoveTo {
                target: target(1, 1)
            }]
        );
        assert_eq!(
            world.entity(EntityId(4)).unwrap().orders,
            vec![Order::MoveTo {
                target: target(1, 1)
            }]
        );
    }

    #[test]
    fn visibility_follows_friendly_vision_radii() {
        let world = demo_world();
        // Entity 3 (enemy) at (5,0) is inside player 0's mover vision (7 tiles).
        assert!(visible_to(&world, PlayerId(0), EntityId(3)));
        // Move the enemy far away (35,0): 15 tiles from the watcher at (20,0)
        // (vision 9) and 35 from the mover (vision 7) — beyond every radius.
        let mut far = world.clone();
        far.entity_mut(EntityId(3)).unwrap().pos = Vec2Fx::from_ints(35, 0);
        assert!(!visible_to(&far, PlayerId(0), EntityId(3)));
        // Own entities are always visible.
        assert!(visible_to(&far, PlayerId(0), EntityId(2)));
        // Unknown targets are not visible.
        assert!(!visible_to(&far, PlayerId(0), EntityId(99)));
    }
}
