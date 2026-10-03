//! AI controllers (plan §9.6): perceive a fog-filtered `PlayerView`, decide, and act
//! by emitting `Command` records — never by touching simulation state (FD-7).
//!
//! Parity is structural, not a policy: this crate depends only on `fx` and
//! `sim_api` (plan §4), which makes "AI reaches into game state" a compile error.
//! Controllers run on the same tick boundary as the player, through the same
//! validation gate, and their economy obeys the same costs and times. Any
//! exception must be a named, documented, toggled option recorded in an ADR,
//! defaulting off.
//!
//! The three stages with clean seams (plan §9.6):
//!
//! - **Perceive** — read the [`PlayerView`]; it is the *only* window a controller
//!   gets. Everything about live entities (positions, ownership, health) arrives
//!   through it, fog-filtered (FD-8): own entities always, others only inside
//!   friendly vision radii.
//! - **Decide** — a state machine over the view and the controller's own
//!   bookkeeping. Map and faction facts a player reads off the screen (kind ids,
//!   costs, start positions, candidate build ground) arrive as configuration
//!   ([`AiPlan`]), because they are public knowledge, not fog-protected state.
//! - **Act** — emit `Command` records. The simulation validates them like any
//!   player's; rejections change no state and the controller simply retries on
//!   its own schedule.
//!
//! The scripted Alpha opponent ([`ScriptedController`]) lands with milestone M7:
//! a scripted build order (workers → supply → barracks → mixed army),
//! expansion-free, attack waves on timers and army-size thresholds, and defense
//! when its base is threatened — the exact behavior plan §9.6 names.

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
#![warn(missing_docs)]

pub mod scripted;

pub use scripted::{AiPlan, ScriptedController};

use pandemonium_sim_api::{Command, PlayerView, Tick};

/// A controller occupying one player slot (plan §9.6).
///
/// `think` runs once per tick boundary, before the tick's `Sim::step` applies
/// commands: the `view` is the fog-filtered state as of `tick`, and every
/// command appended to `out` must carry `tick` and the controller's own issuer
/// slot so the shared validation gate accepts them. A controller may emit
/// nothing; it must never mutate anything but its own state and `out`.
///
/// Controllers are deterministic: any randomness they need comes from a seeded
/// RNG constructed from the match seed (see [`ScriptedController`]), never from
/// their own entropy (plan §9.6).
pub trait Controller {
    /// Perceives `view`, decides, and appends this tick's commands to `out`.
    fn think(&mut self, view: &PlayerView, tick: Tick, out: &mut Vec<Command>);
}
