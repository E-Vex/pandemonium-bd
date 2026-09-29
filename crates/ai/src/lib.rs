//! AI controllers (plan §9.6): perceive a fog-filtered `PlayerView`, decide, and act
//! by emitting `Command` records — never by touching simulation state (FD-7).
//!
//! Parity is structural, not a policy: this crate depends only on `sim_api` (plan
//! §4), which makes "AI reaches into game state" a compile error. Controllers run on
//! the same tick boundary as the player, through the same validation gate, and their
//! economy obeys the same costs and times. Any exception must be a named, documented,
//! toggled option recorded in an ADR, defaulting off.
//!
//! The `Controller` trait and the scripted Alpha opponent arrive with milestone M7.

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
