//! The deterministic simulation core (plan §6): world state, entity store, capability
//! stores, the fixed tick pipeline, systems, events, and the canonical state hash.
//!
//! Milestone M1 provides `Sim`, `step`, the command queue, the event buffer, and
//! `state_hash`. The crate exists from M0 so the dependency law is enforced from day
//! zero: `sim` depends only on `fx` and `sim_api` (plan §4) — never on the engine,
//! client, AI, replay, content, or any I/O crate (FD-6).
//!
//! The only way state advances is `Sim::step` (FD-2): every intent enters as a
//! validated `Command` applied at a tick boundary.

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]

/// Simulation frequency in ticks per second (FD-1: fixed-tick, render-decoupled).
pub const TICKS_PER_SECOND: u32 = 30;

#[cfg(test)]
mod tests {
    use super::TICKS_PER_SECOND;

    #[test]
    fn simulation_runs_at_thirty_ticks_per_second() {
        // FD-1 is a frozen decision; this pins the constant against accidental drift.
        assert_eq!(TICKS_PER_SECOND, 30);
    }
}
