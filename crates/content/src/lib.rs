//! Content pipeline (plan §10): the RON schema, versioned loaders, validators, and the
//! content bundle + content hash, produced as plain structs the simulation receives.
//!
//! The data/code boundary is frozen (FD-4): stats, costs, requirements, maps, and
//! factions are versioned data files; tick order, command semantics, and capability
//! behavior are code. The loader converts authored integer units (milliseconds,
//! milli-tiles — plan §10.2) into the simulation's fixed-point representation at load
//! time, and content structs carry integer-only fields (plan §5).
//!
//! Milestone M2 implements this crate; M0 provides the skeleton so the dependency law
//! is pinned from day zero (`content` depends on `fx`, `sim`, and `sim_api`, and
//! nothing above them).

#![forbid(unsafe_code)]
