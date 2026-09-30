//! Content pipeline (plan §10): the RON schema, versioned loaders, validators,
//! and the content bundle + content hash, produced as plain structs the
//! simulation receives.
//!
//! The data/code boundary is frozen (FD-4): stats, costs, requirements, maps, and
//! factions are versioned data files; tick order, command semantics, and
//! capability behavior are code. The loader converts authored integer units
//! (milliseconds, milli-tiles — plan §10.2) at load time, and content structs
//! carry integer-only fields (plan §5, FD-5).
//!
//! The dependency direction is the whole design (plan §4, A-004): this crate
//! depends on `sim` and converts RON into the simulation's own plain structs —
//! serde/ron live here and nowhere in the sim's tree. A match built from content
//! is:
//!
//! ```ignore
//! let bundle = ContentBundle::load_dir(content_root)?;
//! let sim = Sim::new(&bundle.world(), setup);
//! ```
//!
//! and `bundle.content_hash()` / `bundle.map_id()` are the content identity a
//! replay records (plan §6.5).
//!
//! Layout:
//! - [`error`] — typed, precise errors (file + entity/map + coordinates + rule).
//! - [`schema`] — the RON-facing raw types (strict: unknown fields are errors).
//! - [`version`] — schema version gates and forward migrations (FD-10).
//! - [`loader`] — named sources, the sorted directory reader, per-file parsing.
//! - [`defs`] — the canonical validated types everything downstream consumes.
//! - [`validate`] — file-level and placement-level validators (plan §10.5).
//! - [`bundle`] — [`ContentTree`], [`ContentBundle`], the canonical content
//!   hash, and the world definition handed to `Sim::new`.

#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::disallowed_types,
    clippy::disallowed_methods
)]
#![warn(missing_docs)]

mod bundle;
mod defs;
mod error;
mod loader;
mod schema;
mod validate;
mod version;

pub use bundle::{ContentBundle, ContentTree};
pub use defs::{
    CapabilityDef, EntityDef, FactionDef, Heightmap, MapDef, OreNodeDef, ResourceDef, RulesDef,
    StartDef, StartingForce, TerrainClass, VictoryKind,
};
pub use error::ContentError;
pub use loader::{ms_to_ticks, Source, SourceMap};

#[cfg(test)]
mod tests;
