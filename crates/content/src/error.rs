//! Typed, precise content errors (plan §14 M2 exit: "malformed files produce
//! precise errors").
//!
//! Every variant carries the file it came from (in `category/name.ron` form) plus
//! the specifics a designer needs to fix the problem: ids, coordinates, indices,
//! expected-vs-found values. Parse errors surface the underlying decoder message,
//! which includes line/column positions where the decoder knows them.

use thiserror::Error;

/// Anything that can go wrong while loading or validating content.
///
/// The [`std::error::Error`] trait makes these compose with `anyhow` in the tools
/// binary; the structured fields make them assertable in tests (each validator has
/// a test pinning its exact variant).
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum ContentError {
    /// A file could not be read from disk.
    #[error("reading {file}: {detail}")]
    Io {
        /// The file path that failed.
        file: String,
        /// The I/O failure detail.
        detail: String,
    },

    /// A required content category directory is missing.
    #[error("missing content directory {path} (expected category '{category}')")]
    MissingDirectory {
        /// The directory path that was absent.
        path: String,
        /// The category name (`entities`, `factions`, `maps`, `rules`).
        category: String,
    },

    /// A file failed RON parsing or strict deserialization (unknown fields are
    /// errors — plan §10.2).
    #[error("{file}: {detail}")]
    Parse {
        /// The offending file, in `category/name.ron` form.
        file: String,
        /// The decoder message (includes line/column when known).
        detail: String,
    },

    /// The file's `schema_version` is not loadable by this build (too new, or
    /// invalid). Old versions migrate forward (FD-10); newer ones cannot.
    #[error("{file}: schema_version {found} is not loadable (this loader supports {supported}; content only moves forward — plan §2 FD-10)")]
    Version {
        /// The offending file.
        file: String,
        /// The version the file declares.
        found: u32,
        /// The newest version this loader supports for that file kind.
        supported: u32,
    },

    /// The file's declared `id` does not match its file name (stem).
    #[error("{file}: declared id '{declared}' does not match file name '{stem}' (keep them identical so files are findable)")]
    IdMismatch {
        /// The offending file.
        file: String,
        /// The id declared inside the file.
        declared: String,
        /// The file name stem.
        stem: String,
    },

    /// Two files declare the same id within a category.
    #[error("duplicate id '{id}' in category '{category}' ({first} and {second})")]
    DuplicateId {
        /// The duplicated id.
        id: String,
        /// The category (`entities`, `factions`, `maps`).
        category: String,
        /// The first file declaring it.
        first: String,
        /// The second file declaring it.
        second: String,
    },

    /// A reference to an entity that does not exist (roster, production lists,
    /// requirements, starting forces, map ore nodes).
    #[error("{file}: {what} references unknown entity '{reference}'")]
    UnknownEntityRef {
        /// The offending file.
        file: String,
        /// What the reference was for ("roster entry", "production list of …").
        what: String,
        /// The missing entity id.
        reference: String,
    },

    /// A reference to a resource that is not in the rules registry.
    #[error("{file}: {what} references unknown resource '{reference}' (registered: {registered})")]
    UnknownResourceRef {
        /// The offending file.
        file: String,
        /// What the reference was for.
        what: String,
        /// The missing resource id.
        reference: String,
        /// The ids that are registered, comma-joined.
        registered: String,
    },

    /// An entity carries the same capability twice.
    #[error("{file}: entity '{entity}' carries capability '{capability}' more than once")]
    DuplicateCapability {
        /// The offending file.
        file: String,
        /// The entity id.
        entity: String,
        /// The duplicated capability name.
        capability: String,
    },

    /// A stat value is out of its valid range (plan §10.2 authoring units).
    #[error("{file}: entity '{entity}' has invalid {field} = {value} ({why})")]
    InvalidStat {
        /// The offending file.
        file: String,
        /// The entity id.
        entity: String,
        /// The field name, in schema spelling.
        field: String,
        /// The authored value.
        value: i64,
        /// Why it is invalid.
        why: String,
    },

    /// The rules file is malformed at the semantic level.
    #[error("{file}: {detail}")]
    InvalidRules {
        /// The offending file.
        file: String,
        /// What is wrong.
        detail: String,
    },

    /// A faction file is malformed at the semantic level.
    #[error("{file}: faction '{faction}': {detail}")]
    InvalidFaction {
        /// The offending file.
        file: String,
        /// The faction id.
        faction: String,
        /// What is wrong.
        detail: String,
    },

    /// A map file is malformed at the semantic level (dimensions, grid, legend,
    /// starts, heightmap shape).
    #[error("{file}: map '{map}': {detail}")]
    InvalidMap {
        /// The offending file.
        file: String,
        /// The map id.
        map: String,
        /// What is wrong.
        detail: String,
    },

    /// A start position is illegal (anchor out of bounds, blocked footprint,
    /// overlapping occupancy, walled in).
    #[error("{file}: map '{map}': start for player {player} is illegal: {detail}")]
    IllegalStart {
        /// The offending file.
        file: String,
        /// The map id.
        map: String,
        /// The player slot of the start.
        player: u8,
        /// What is wrong.
        detail: String,
    },

    /// An ore node cannot be reached from a start position (plan §10.5).
    #[error("{file}: map '{map}': ore node '{kind}' at ({x},{y}) is unreachable from player {player}'s start")]
    OreUnreachable {
        /// The offending file.
        file: String,
        /// The map id.
        map: String,
        /// The ore node's entity kind.
        kind: String,
        /// The node's tile x.
        x: i32,
        /// The node's tile y.
        y: i32,
        /// The player whose start cannot reach it.
        player: u8,
    },

    /// The map declares `symmetric: true` but violates the symmetry check
    /// (plan §10.5: the symmetry check is an optional validator, run when
    /// declared).
    #[error("{file}: map '{map}' declares symmetry but {detail}")]
    SymmetryBroken {
        /// The offending file.
        file: String,
        /// The map id.
        map: String,
        /// What part of the map is asymmetric.
        detail: String,
    },

    /// The tree cannot produce a single-match bundle (wrong count of rules or
    /// maps).
    #[error("cannot bundle: {detail}")]
    Bundle {
        /// What is wrong (counts, unknown map id).
        detail: String,
    },
}
