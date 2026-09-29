//! Cross-crate acceptance tests for Pandemonium (plan §4, §13).
//!
//! This package hosts the acceptance suite named by the plan: `architecture_law.rs`
//! (M0, A13) now, `determinism.rs` (A1) in M1, and the rest as their systems land.
//! Each acceptance test is declared as an explicit `[[test]]` target in this
//! package's manifest.

#![forbid(unsafe_code)]
