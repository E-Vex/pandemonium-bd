//! Deterministic math foundation for Pandemonium (plan §4, §6.1).
//!
//! This crate is the bottom of the dependency graph: it depends on nothing outside
//! `core`/`std`, contains no floating-point types, no wall-clock reads, no
//! unordered-map types, and no `unsafe` (plan §5). Every operation must produce
//! bit-identical results on every OS, CPU, and compiler version (FD-5).
//!
//! Contents:
//! - `Fx`: Q16.16 fixed-point scalar.
//! - `Vec2Fx`: 2D fixed-point vector (tile units).
//! - `isqrt`: exact integer square root.
//! - `Rng`: PCG32 deterministic random generator.
//! - `XxHash64`: xxHash64 — the canonical hasher (since M10.1, DEBT-001).
//! - `Fnv1a64`: FNV-1a 64-bit — the replay file checksum.

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
#![warn(missing_docs)]

mod fixed;
mod hash;
mod rng;
mod sqrt;
mod vec;

pub use fixed::{Fx, FRAC_BITS, FRAC_SCALE};
pub use hash::{fnv1a64, xxhash64, xxhash64_seeded, Fnv1a64, XxHash64};
pub use rng::Rng;
pub use sqrt::isqrt;
pub use vec::Vec2Fx;
