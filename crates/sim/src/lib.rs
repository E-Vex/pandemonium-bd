//! The deterministic simulation core (plan §6): world state, entity store, capability
//! stores, the fixed tick pipeline, systems, events, and the canonical state hash.
//!
//! Milestone M1 provides `Sim`, `step`, the tick counter, the entity store with
//! monotonic never-reused ids, capability stores iterated in ascending id order, the
//! command queue with `(issuer, seq)` ordered application, the event buffer drained
//! each tick, `state_hash`, `snapshot`, and `player_view`. The crate keeps the
//! dependency law enforced from day zero: `sim` depends only on `fx` and `sim_api`
//! (plan §4) — never on the engine, client, AI, replay, content, or any I/O crate
//! (FD-6).
//!
//! The only way state advances is [`Sim::step`] (FD-2): every intent enters as a
//! validated [`Command`] applied at a tick boundary. Invalid commands produce
//! [`Event::CommandRejected`] and change no state (plan §8.2).
//!
//! Milestone M1 simulates a *trivial world* described in code (see [`fixture`]);
//! the RON content pipeline that replaces it for real matches is milestone M2.

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
#![warn(missing_docs)]

mod combat;
mod command;
mod economy;
mod fixture;
mod hash;
mod invariants;
mod match_rules;
mod movement;
mod nav;
mod production;
mod runner;
mod sim;
mod vision;
mod world;

use pandemonium_sim_api::Tick;

pub use fixture::{
    CapTemplate, KindEconomy, KindTemplate, ResourceDef, ScheduledSpawnDef, SpawnDef, TrivialWorld,
};
pub use match_rules::MatchOutcome;
pub use runner::{run_command_log, CommandLogRun};
pub use sim::{Sim, StepOutput};
pub use world::{
    AttackDef, BuildDef, CapabilityData, ConstructionDef, FootprintDef, GatherDef, HealthDef,
    Lifecycle, MoveDef, Order, PopulationDef, ProduceDef, QueueItem, ResourceBodyDef, StorageDef,
    VisionDef, World,
};

/// Simulation frequency in ticks per second (FD-1: fixed-tick, render-decoupled).
pub const TICKS_PER_SECOND: u32 = 30;

/// The tick interval at which the pipeline computes and reports a checkpoint hash
/// (plan §6.3 stage 11: "compute periodic hash (every 30 ticks and on demand)").
pub const CHECKPOINT_INTERVAL: Tick = TICKS_PER_SECOND;

#[cfg(test)]
mod tests {
    use super::TICKS_PER_SECOND;

    #[test]
    fn simulation_runs_at_thirty_ticks_per_second() {
        // FD-1 is a frozen decision; this pins the constant against accidental drift.
        assert_eq!(TICKS_PER_SECOND, 30);
    }
}
