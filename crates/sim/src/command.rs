//! The command gate: validation and application (plan §8.2, §6.3 stage 1).
//!
//! One shared gate serves every issuer — human, AI, replay, and (later) network
//! (FD-2, FD-7): commands arrive at a tick boundary, are sorted by
//! `(issuer slot, seq)`, validated in that order, and applied in that order.
//! Invalid commands emit [`Event::CommandRejected`] and change no state.
//!
//! Validation checks, in order: tick match → issuer exists → sequence not reused
//! this tick → per-kind checks (referenced entities exist, are owned by the
//! issuer, carry the required capabilities; targets exist and are visible to the
//! issuer). The economy checks of plan §8.2 (affordability, population,
//! placement, requirements) become reachable when the economy data and systems
//! arrive in M5 — see docs/ASSUMPTIONS.md.
//!
//! Commands whose required capabilities do not exist in the M1 vocabulary yet
//! (Attack, Gather, Build, Train, …) fail the capability-presence check after the
//! existence and ownership checks run — exactly what a unit without the
//! capability deserves (plan §8.2's own example). When M5/M6 add those
//! capability variants, the checks become store lookups and the commands start
//! working; the gate's structure does not change (plan §8.3).

use pandemonium_sim_api::{
    Command, CommandKind, EntityId, Event, KindId, PlayerId, Reject, RejectReason,
};

use crate::world::{Order, World};

/// Applies one tick's commands to the world (plan §6.3 stage 1).
///
/// Commands are sorted by `(issuer, seq)` — a *stable* sort, so commands that
/// share the key keep their feed order; the first one wins and later duplicates
/// are rejected as [`RejectReason::DuplicateSeq`] (deterministic no matter how the
/// caller ordered them).
pub(crate) fn apply_commands(
    world: &mut World,
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

        match validate(world, cmd) {
            Err(reason) => rejected(events, cmd, reason),
            Ok(()) => apply_valid(world, cmd),
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
/// and carries the required capability; targets exist and are visible.
fn validate(world: &World, cmd: &Command) -> Result<(), RejectReason> {
    match &cmd.kind {
        CommandKind::Move { units, .. } | CommandKind::AttackMove { units, .. } => {
            // AttackMove will additionally require Attack when combat lands (M6);
            // the movement leg it exercises today is Move.
            check_units(world, cmd.issuer, units, |id| {
                if world.has_move(id) {
                    Ok(())
                } else {
                    Err(RejectReason::MissingCapability)
                }
            })?;
            Ok(())
        }
        CommandKind::Stop { units } => {
            // Stopping is always legal for owned entities: an entity with no
            // orders (or no mover) is already stopped — a valid no-op.
            check_units(world, cmd.issuer, units, |_| Ok(()))?;
            Ok(())
        }
        CommandKind::Attack { units, target } => {
            check_units_exist_and_owned(world, cmd.issuer, units)?;
            // The Attack capability variant arrives with combat (M6). Until the
            // vocabulary can carry it, no entity can satisfy an Attack order.
            check_capability_present()?;
            check_target(world, cmd.issuer, *target)?;
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
            worker, structure, ..
        } => {
            check_single_entity(world, cmd.issuer, *worker)?;
            check_capability_present()?;
            check_kind(world, *structure)
        }
        CommandKind::Train { producer, unit } => {
            check_single_entity(world, cmd.issuer, *producer)?;
            check_capability_present()?;
            check_kind(world, *unit)
        }
        CommandKind::CancelQueueItem { producer, .. } | CommandKind::SetRally { producer, .. } => {
            check_single_entity(world, cmd.issuer, *producer)?;
            check_capability_present()?;
            Ok(())
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
/// (plan §5.3).
fn apply_valid(world: &mut World, cmd: &Command) {
    match &cmd.kind {
        CommandKind::Move { units, target } | CommandKind::AttackMove { units, target } => {
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
        CommandKind::Stop { units } => {
            for id in sorted_unique(units) {
                if let Some(entity) = world.entity_mut(id) {
                    entity.orders.clear();
                }
                if let Some(def) = world.move_of_mut(id) {
                    def.reset_runtime();
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
        CommandKind::Resign {} => {
            if let Some(player) = world.player_mut(cmd.issuer) {
                player.resigned = true;
            }
        }
        // Reaching these arms means validation passed a command whose capability
        // does not exist yet — a logic error in the gate, not a state to paper
        // over. The exhaustive match makes the compiler demand an arm when a
        // future capability variant is added (plan §8.3).
        CommandKind::Attack { .. }
        | CommandKind::Build { .. }
        | CommandKind::Train { .. }
        | CommandKind::CancelQueueItem { .. }
        | CommandKind::SetRally { .. } => {
            unreachable!("validated command {cmd:?} reached apply without a handler")
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

/// Placeholder capability check for capabilities that do not exist in the M1
/// vocabulary: no entity can carry them, so the command is refused as missing
/// capability. Replaced by a store lookup when the variant lands (M5/M6).
fn check_capability_present() -> Result<(), RejectReason> {
    Err(RejectReason::MissingCapability)
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
        world.kind_count = 2;
        world
    }

    fn cmd(issuer: u8, tick: u32, seq: u32, kind: CommandKind) -> Command {
        Command::new(PlayerId(issuer), tick, seq, kind)
    }

    fn apply(world: &mut World, tick: u32, commands: &[Command]) -> Vec<Event> {
        let mut events = Vec::new();
        apply_commands(world, tick, commands, &mut events);
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
