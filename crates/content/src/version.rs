//! Schema versioning and forward migration (plan §10.2, FD-10: the content
//! schema only moves forward — versioned, validated, old content keeps loading).
//!
//! Every file declares `schema_version`. The loader first reads just that field
//! ([`VersionHeader`]), then dispatches:
//!
//! - a file at the current version parses directly into the current raw type;
//! - an *older* file parses into its historical raw type and migrates forward,
//!   one step at a time, until it reaches the current shape
//!   ([`migrate_map_to_current`] — version 1 maps gain `heightmap: None` per
//!   ADR-0001);
//! - a *newer* file is a precise [`ContentError::Version`] error — content never
//!   loads through a loader older than its schema.

use crate::error::ContentError;
use crate::schema::{RawMapV1, RawMapV2, VersionHeader, MAP_SCHEMA_VERSION};

/// Reads only the `schema_version` field of a file. Lenient by design: all other
/// fields belong to the real schema, which is parsed after the version is known.
pub(crate) fn read_version(file: &str, text: &str) -> Result<u32, ContentError> {
    let header: VersionHeader = ron::from_str(text).map_err(|error| ContentError::Parse {
        file: file.to_string(),
        detail: format!("cannot read schema_version: {error}"),
    })?;
    Ok(header.schema_version)
}

/// Checks a file's declared version against the newest supported version for its
/// kind. Versions older than the current one are accepted (they migrate); only
/// versions above the supported line are refused.
pub(crate) fn check_version(file: &str, found: u32, supported: u32) -> Result<(), ContentError> {
    if found == 0 || found > supported {
        return Err(ContentError::Version {
            file: file.to_string(),
            found,
            supported,
        });
    }
    Ok(())
}

/// The current-version raw map shape, after any forward migration. This is what
/// the rest of the pipeline consumes.
pub(crate) enum MapAtCurrent {
    /// A version-2 map, parsed directly.
    Current(RawMapV2),
    /// A version-1 map, parsed in its historical shape and awaiting migration.
    NeedsMigration(RawMapV1),
}

/// Parses a map file at whatever (supported) version it declares, returning
/// either the current shape or the historical shape plus a pending migration.
/// The declared version is confirmed against the strict parse — the header and
/// the schema read the same field, and a mismatch means the two-phrase parse
/// was refactored incorrectly, which must never pass silently.
pub(crate) fn parse_map_at_declared_version(
    file: &str,
    text: &str,
) -> Result<MapAtCurrent, ContentError> {
    let version = read_version(file, text)?;
    check_version(file, version, MAP_SCHEMA_VERSION)?;
    match version {
        1 => {
            let raw: RawMapV1 = ron::from_str(text).map_err(|error| ContentError::Parse {
                file: file.to_string(),
                detail: error.to_string(),
            })?;
            confirm_declared_version(file, raw.schema_version, version)?;
            Ok(MapAtCurrent::NeedsMigration(raw))
        }
        _ => {
            let raw: RawMapV2 = ron::from_str(text).map_err(|error| ContentError::Parse {
                file: file.to_string(),
                detail: error.to_string(),
            })?;
            confirm_declared_version(file, raw.schema_version, version)?;
            Ok(MapAtCurrent::Current(raw))
        }
    }
}

/// Confirms the strict parse saw the same `schema_version` the header read
/// (they read the same field of the same text; a mismatch is a loader bug).
pub(crate) fn confirm_declared_version(
    file: &str,
    declared: u32,
    header: u32,
) -> Result<(), ContentError> {
    if declared != header {
        return Err(ContentError::Parse {
            file: file.to_string(),
            detail: format!(
                "schema_version disagrees between the version probe ({header}) and the \
                 schema parse ({declared}) — this is a loader bug, please report it"
            ),
        });
    }
    Ok(())
}

/// Migration step 1 -> 2 (ADR-0001): version 2 added the optional display-only
/// heightmap. A version-1 map simply has none.
///
/// Migrations are explicit, one function per step, so the chain stays reviewable
/// and testable; when the schema gains a version 3, this function gains a call to
/// the 2 -> 3 step and a new arm handles version-2 input.
pub(crate) fn migrate_map_v1_to_v2(v1: RawMapV1) -> RawMapV2 {
    RawMapV2 {
        schema_version: 2,
        id: v1.id,
        display_name: v1.display_name,
        width: v1.width,
        height: v1.height,
        terrain: v1.terrain,
        grid: v1.grid,
        starts: v1.starts,
        ore_nodes: v1.ore_nodes,
        symmetric: v1.symmetric,
        heightmap: None,
    }
}
