//! Public vocabulary types shared outward from the simulation (plan §4).
//!
//! Everything crossing the sim boundary — commands, events, views — is expressed in
//! types from this crate, so that `ai`, `replay`, `engine`, and `client` never need to
//! depend on simulation internals. M0 ships the identity vocabulary; commands,
//! events, and views arrive with milestone M1 (plan §6.5, §8, §9).

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]

/// Entity identity: 64-bit, monotonic, never reused (plan §7.2).
///
/// IDs are allocated by the simulation only; because they are never reused, replays,
/// networking, and save/load can reference them without ambiguity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EntityId(pub u64);

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
