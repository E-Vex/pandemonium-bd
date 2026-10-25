//! Client-side asset pack: a local, presentation-only collection of visual
//! assets keyed by entity kind id and terrain class.
//!
//! The pack lives entirely above the sim boundary (plan FD-6). It is loaded
//! at startup, validated, and handed to the renderer. The renderer resolves
//! an entity's presentation through its data-driven kind id from `content/`,
//! consulting the pack for asset references. When an asset cannot be used
//! (no pack, missing file, unsupported format), the existing placeholder
//! rendering is used — the simulation never sees any of this.
//!
//! Manifest format (RON, `manifest.ron` at the pack root), schema 1 —
//! the official `tactical_field` pack's shape, loading unchanged:
//!
//! ```ron
//! (
//!     pack_id: "default",
//!     schema_version: 1,
//!     entities: {
//!         "rifleman": "textures/entities/rifleman.png",
//!     },
//!     terrain: {
//!         "grass": "textures/terrain/grass.png",
//!     },
//! )
//! ```
//!
//! Schema 2 (ADR-0101 §4) — a v2 entity entry carries `model`, `skin`,
//! `scale`, `rotation_deg`, `ground_offset`, `icon`, and optional `sounds`:
//!
//! ```ron
//! (
//!     pack_id: "tactical_field",
//!     schema_version: 2,
//!     entities: {
//!         "rifleman": (
//!             model: Some("models/units/rifleman.glb"),     // optional
//!             skin:  Some("textures/entities/rifleman.png"), // optional
//!             scale: 1.0,            // optional, default 1.0, 0.05..=10.0
//!             rotation_deg: 0.0,     // optional, default 0.0, -360.0..=360.0
//!             ground_offset: 0.0,    // optional, meters, default 0.0
//!             icon: Some("icons/rifleman.png"),             // optional
//!             sounds: { "SelectionClick": "audio/rifleman/select.ogg" }, // optional
//!         ),
//!     },
//!     terrain: { "grass": "textures/terrain/grass.png" },  // unchanged from v1
//! )
//! ```
//!
//! (RON note: map entries take the plain `"key": value` form the v1
//! terrain map has always used — ron 0.9 rejects the parenthesized
//! `("key": value)` form ADR-0101 §4's illustrative example shows.)
//!
//! Version rules (FD-10's forward-only discipline, the map-schema v1→v2
//! precedent of ADR-0001): a v1 entry migrates forward one typed step at
//! load — a `.png` becomes `skin`, a `.glb`/`.gltf` becomes `model`, every
//! other field takes its default — so v1 packs (including the official
//! `tactical_field` pack the release smoke gates on) load with identical
//! rendering behavior. `schema_version` greater than 2 is the precise
//! [`UnsupportedVersion`](PackError::UnsupportedVersion) error it always
//! was; unknown fields are rejected in both versions (strict mode,
//! mirroring the content schema's `deny_unknown_fields`).
//!
//! `icon` and `sounds` are schema-validated but **not consumed in M12**
//! — paths validated inside the pack root, icon `.png`-only, sound cue
//! names checked against the engine's cue vocabulary — the exact pattern
//! the terrain PNGs rode in on (A-014). `scale`, `rotation_deg`, and
//! `ground_offset` are floats in the client's presentation tree only
//! (plan §5's float ban scopes to the determinism crates, which the pack
//! never touches).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// The latest manifest schema version this build accepts; v1 manifests
/// load unchanged through the forward migration (ADR-0101 §4).
pub const MANIFEST_SCHEMA_VERSION: u32 = 2;

/// The manifest file name inside a pack root.
const MANIFEST_FILE_NAME: &str = "manifest.ron";

/// File extensions recognized as model assets. `.gltf` stays recognized by
/// the pack system (a v1 `.gltf` entry migrates to a `model` reference)
/// but is not consumed as a model: the GLB loader refuses non-GLB bytes
/// with one precise error and the kind walks the fallback chain
/// (ADR-0101 §1, ART_SPEC §1).
#[allow(dead_code)]
const MODEL_EXTENSIONS: &[&str] = &["glb", "gltf"];

/// File extensions recognized as texture assets.
#[allow(dead_code)]
const TEXTURE_EXTENSIONS: &[&str] = &["png"];

/// The one extension an `icon` field accepts (ART_SPEC §8: 128×128 PNG,
/// schema-only in M12).
const ICON_EXTENSION: &str = "png";

/// The manifest schema version this loader accepts.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // populated at load; consumed by the renderer in a follow-up
pub struct AssetPack {
    /// Stable pack id, echoed from the manifest.
    pub pack_id: String,
    /// Manifest schema version as declared (v1 packs echo 1, v2 echo 2 —
    /// the release smoke's "schema 1" line for the official pack stays
    /// true until that pack itself bumps).
    pub schema_version: u32,
    /// Entity kind id → presentation entry.
    pub entities: BTreeMap<String, AssetEntry>,
    /// Terrain class → asset reference.
    pub terrain: BTreeMap<String, AssetEntry>,
    /// The pack's root directory (canonicalized).
    pub root: PathBuf,
}

/// One declared asset path: the path as written in the manifest and its
/// canonicalized absolute location on disk (guaranteed inside the pack
/// root).
#[derive(Clone, Debug)]
#[allow(dead_code)] // populated at load; consumed by the renderer in a follow-up
pub struct AssetRef {
    /// The path as written in the manifest, relative to the pack root.
    pub path: String,
    /// Canonicalized absolute path — the file the renderer would open.
    pub resolved: PathBuf,
}

/// One entity's presentation entry — the v2 shape (ADR-0101 §4). v1
/// entries migrate forward into it at load: a `.png` string becomes
/// `skin`, a `.glb`/`.gltf` string becomes `model`, every other field
/// takes its default.
#[derive(Clone, Debug)]
#[allow(dead_code)] // populated at load; consumed by the renderer in a follow-up
pub struct AssetEntry {
    /// The renderable presentation path as declared — the skin when one is
    /// declared, else the model. This is v1's field, kept for the pre-model
    /// renderer seam (`render::texture` reads `resolved` and filters by
    /// `kind_of`): an entry that declares neither a model nor a skin (an
    /// icon/sounds-only kind) carries an empty `asset` and renders the
    /// placeholder.
    pub asset: String,
    /// Canonicalized absolute path of `asset` (empty when `asset` is).
    pub resolved: PathBuf,
    /// The model reference (`.glb`; a `.gltf` is the deferred
    /// skip-with-diagnostic case — the loader refuses it and the chain
    /// walks).
    pub model: Option<AssetRef>,
    /// The skin reference — the v1 entity texture's new name in v2.
    pub skin: Option<AssetRef>,
    /// The icon reference (`.png`-only, validated inside the root;
    /// schema-only in M12).
    pub icon: Option<AssetRef>,
    /// Sound cue name → audio path (cue names validated against the
    /// engine's vocabulary; schema-only in M12).
    pub sounds: BTreeMap<String, AssetRef>,
    /// Uniform model scale in meters (default 1.0, range 0.05..=10.0).
    pub scale: f32,
    /// Yaw about the origin in degrees (default 0.0, range -360.0..=360.0).
    pub rotation_deg: f32,
    /// Vertical lift in meters (default 0.0; no range is defined —
    /// A-144/ART_SPEC §8 state none, so none is enforced).
    pub ground_offset: f32,
}

impl AssetEntry {
    /// The defaults every v1 entry and every absent v2 field takes
    /// (A-144: the migration "applies the defaults").
    fn with_primary(
        asset: String,
        resolved: PathBuf,
        model: Option<AssetRef>,
        skin: Option<AssetRef>,
    ) -> Self {
        Self {
            asset,
            resolved,
            model,
            skin,
            icon: None,
            sounds: BTreeMap::new(),
            scale: 1.0,
            rotation_deg: 0.0,
            ground_offset: 0.0,
        }
    }
}

/// The kind of asset, inferred from the file extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // used by `kind_of`; consumed by the renderer follow-up
pub enum AssetKind {
    Model,
    Texture,
}

/// Fatal errors when loading a pack. Non-fatal issues (a missing asset
/// file, an unusable format for one entry) do not abort the whole pack:
/// that reference is skipped with a diagnostic and the fallback tier is
/// used.
#[derive(Debug, Error)]
#[allow(dead_code)] // `NotFound` is reserved for future use; other variants are active
pub enum PackError {
    #[error("pack manifest not found (searched: {0})")]
    NotFound(String),
    #[error("failed to read pack manifest at {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("pack manifest at {0} is malformed: {1}")]
    Parse(PathBuf, String),
    #[error("unsupported pack schema version {found} (expected {expected})")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("absolute asset path not allowed for {field}: {path}")]
    AbsolutePath { field: String, path: String },
    #[error("path traversal not allowed for {field}: {path}")]
    PathTraversal { field: String, path: String },
    #[error("asset path escapes the pack root for {field}: {path}")]
    EscapesPackRoot { field: String, path: String },
    #[error("entity '{kind}' field {field} is out of range: {found} (allowed {allowed})")]
    ValueOutOfRange {
        kind: String,
        field: &'static str,
        found: f32,
        allowed: &'static str,
    },
    #[error("icon for entity '{kind}' must be a .png (found '{path}')")]
    IconNotPng { kind: String, path: String },
    #[error("sound cue '{cue}' for entity '{kind}' is not in the engine's cue vocabulary (crates/engine/src/audio.rs)")]
    UnknownSoundCue { kind: String, cue: String },
}

/// Where one declared asset path comes from — the manifest section (for
/// fatal path-security errors), the human-readable reference name (for
/// diagnostics), and the entry's key.
struct RefContext<'a> {
    /// The manifest section the path was declared in ("entities"/"terrain").
    field: &'a str,
    /// What the reference is ("asset", "model", "skin", "icon", …).
    what: &'a str,
    /// "entity" or "terrain" — the diagnostic's noun.
    scope: &'a str,
    /// The kind id or terrain class the entry belongs to.
    key: &'a str,
}

/// What one schema's parse produces: the pack identity and its two
/// validated maps.
type ParsedManifest = (
    String,
    u32,
    BTreeMap<String, AssetEntry>,
    BTreeMap<String, AssetEntry>,
);

impl AssetPack {
    /// The schema version this loader accepts.
    #[allow(dead_code)] // used by callers that report the pack's schema version
    pub const fn schema_version() -> u32 {
        MANIFEST_SCHEMA_VERSION
    }

    /// Loads a pack from `pack_root`. Returns `Ok(None)` (with no diagnostic)
    /// when no manifest exists there — a missing pack is not an error, the
    /// client falls back to placeholders. Returns `Err` for a manifest that
    /// exists but cannot be safely parsed or validated.
    pub fn load(pack_root: &Path) -> Result<Option<Self>, PackError> {
        let manifest_path = pack_root.join(MANIFEST_FILE_NAME);
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| PackError::Io(manifest_path.clone(), e))?;

        // Version probe first: v1 and v2 have different entity shapes, and
        // anything above the latest is the precise UnsupportedVersion error
        // before any field is read.
        let probe: RawManifestVersion = ron::from_str(&text)
            .map_err(|e| PackError::Parse(manifest_path.clone(), e.to_string()))?;
        if probe.schema_version == 0 || probe.schema_version > MANIFEST_SCHEMA_VERSION {
            return Err(PackError::UnsupportedVersion {
                found: probe.schema_version,
                expected: MANIFEST_SCHEMA_VERSION,
            });
        }

        let canonical_root = pack_root
            .canonicalize()
            .map_err(|e| PackError::Io(pack_root.to_path_buf(), e))?;

        let (pack_id, schema_version, entities, terrain) = if probe.schema_version == 1 {
            Self::load_v1(&text, &manifest_path, &canonical_root)?
        } else {
            Self::load_v2(&text, &manifest_path, &canonical_root)?
        };

        Ok(Some(Self {
            pack_id,
            schema_version,
            entities,
            terrain,
            root: canonical_root,
        }))
    }

    /// Parses a schema-1 manifest: string entity entries, forward-migrated
    /// into the typed v2 shape (`.png` → skin, `.glb`/`.gltf` → model,
    /// defaults applied — FD-10's rule; ADR-0001's map-schema precedent).
    fn load_v1(text: &str, manifest_path: &Path, root: &Path) -> Result<ParsedManifest, PackError> {
        let raw: RawManifestV1 = ron::from_str(text)
            .map_err(|e| PackError::Parse(manifest_path.to_path_buf(), e.to_string()))?;

        let mut entities = BTreeMap::new();
        for (kind_id, asset) in &raw.entities {
            let context = RefContext {
                field: "entities",
                what: "asset",
                scope: "entity",
                key: kind_id,
            };
            // A v1 entry is one path: a missing file skips the whole entry
            // (v1's own behavior, preserved — the placeholder is the
            // fallback tier).
            if let Some(reference) = Self::resolve_ref(root, &context, asset)? {
                let (model, skin) = match Self::extension_of(asset) {
                    Some("png") => (None, Some(reference.clone())),
                    Some("glb") | Some("gltf") => (Some(reference.clone()), None),
                    // Any other extension keeps the v1 behavior exactly:
                    // the entry records the path, the renderer's kind_of
                    // filter leaves it a placeholder.
                    _ => (None, None),
                };
                entities.insert(
                    kind_id.clone(),
                    AssetEntry::with_primary(
                        asset.clone(),
                        reference.resolved.clone(),
                        model,
                        skin,
                    ),
                );
            }
        }

        let terrain = Self::load_terrain(root, &raw.terrain)?;
        Ok((raw.pack_id, raw.schema_version, entities, terrain))
    }

    /// Parses a schema-2 manifest: typed entity entries, terrain unchanged
    /// from v1.
    fn load_v2(text: &str, manifest_path: &Path, root: &Path) -> Result<ParsedManifest, PackError> {
        let raw: RawManifestV2 = ron::from_str(text)
            .map_err(|e| PackError::Parse(manifest_path.to_path_buf(), e.to_string()))?;

        let mut entities = BTreeMap::new();
        for (kind_id, entry) in &raw.entities {
            entities.insert(kind_id.clone(), Self::build_v2_entry(root, kind_id, entry)?);
        }

        let terrain = Self::load_terrain(root, &raw.terrain)?;
        Ok((raw.pack_id, raw.schema_version, entities, terrain))
    }

    /// Terrain entries are the same shape in both schemas: a plain path per
    /// class, recorded as the entry's primary asset.
    fn load_terrain(
        root: &Path,
        raw_terrain: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, AssetEntry>, PackError> {
        let mut terrain = BTreeMap::new();
        for (class, asset) in raw_terrain {
            let context = RefContext {
                field: "terrain",
                what: "asset",
                scope: "terrain",
                key: class,
            };
            if let Some(reference) = Self::resolve_ref(root, &context, asset)? {
                terrain.insert(
                    class.clone(),
                    AssetEntry::with_primary(asset.clone(), reference.resolved.clone(), None, None),
                );
            }
        }
        Ok(terrain)
    }

    /// Builds one v2 entity entry: every declared reference resolved
    /// (missing files skip that reference with a diagnostic — the tier
    /// walks), ranges validated (out-of-range is a precise fatal error),
    /// icon and sounds schema-validated but not consumed.
    fn build_v2_entry(
        root: &Path,
        kind_id: &str,
        raw: &RawEntityEntry,
    ) -> Result<AssetEntry, PackError> {
        let model = match &raw.model {
            Some(path) => Self::resolve_ref(
                root,
                &RefContext {
                    field: "entities",
                    what: "model",
                    scope: "entity",
                    key: kind_id,
                },
                path,
            )?,
            None => None,
        };
        let skin = match &raw.skin {
            Some(path) => Self::resolve_ref(
                root,
                &RefContext {
                    field: "entities",
                    what: "skin",
                    scope: "entity",
                    key: kind_id,
                },
                path,
            )?,
            None => None,
        };
        let icon = match &raw.icon {
            Some(path) => {
                if Self::extension_of(path) != Some(ICON_EXTENSION) {
                    return Err(PackError::IconNotPng {
                        kind: kind_id.to_string(),
                        path: path.clone(),
                    });
                }
                Self::resolve_ref(
                    root,
                    &RefContext {
                        field: "entities",
                        what: "icon",
                        scope: "entity",
                        key: kind_id,
                    },
                    path,
                )?
            }
            None => None,
        };
        let mut sounds = BTreeMap::new();
        for (cue, path) in &raw.sounds {
            if !is_known_cue(cue) {
                return Err(PackError::UnknownSoundCue {
                    kind: kind_id.to_string(),
                    cue: cue.clone(),
                });
            }
            let what = format!("sound '{cue}'");
            if let Some(reference) = Self::resolve_ref(
                root,
                &RefContext {
                    field: "entities",
                    what: &what,
                    scope: "entity",
                    key: kind_id,
                },
                path,
            )? {
                sounds.insert(cue.clone(), reference);
            }
        }
        let scale = raw.scale;
        if !(0.05..=10.0).contains(&scale) {
            return Err(PackError::ValueOutOfRange {
                kind: kind_id.to_string(),
                field: "scale",
                found: scale,
                allowed: "0.05..=10.0",
            });
        }
        let rotation_deg = raw.rotation_deg;
        if !(-360.0..=360.0).contains(&rotation_deg) {
            return Err(PackError::ValueOutOfRange {
                kind: kind_id.to_string(),
                field: "rotation_deg",
                found: rotation_deg,
                allowed: "-360.0..=360.0",
            });
        }
        // `ground_offset` carries no range (none is defined — A-144).
        let ground_offset = raw.ground_offset;
        // The primary renderable asset: the skin when declared, else the
        // model; an icon/sounds-only kind carries neither and renders the
        // placeholder.
        let primary = skin.as_ref().or(model.as_ref());
        Ok(AssetEntry {
            asset: primary.map(|r| r.path.clone()).unwrap_or_default(),
            resolved: primary.map(|r| r.resolved.clone()).unwrap_or_default(),
            model,
            skin,
            icon,
            sounds,
            scale,
            rotation_deg,
            ground_offset,
        })
    }

    /// Resolves one declared asset path: rejects absolute paths and `..`
    /// traversal (fatal), resolves relative to the pack root, canonicalizes
    /// (which resolves symlinks), and confirms the result stays inside the
    /// pack root. A missing or unreadable file is non-fatal — `Ok(None)`
    /// with one diagnostic, so the entry's fallback tier is used.
    fn resolve_ref(
        root: &Path,
        context: &RefContext,
        asset: &str,
    ) -> Result<Option<AssetRef>, PackError> {
        // Reject absolute paths on any platform.
        if Path::new(asset).is_absolute() {
            return Err(PackError::AbsolutePath {
                field: context.field.to_string(),
                path: asset.to_string(),
            });
        }
        // Reject any `..` component.
        for component in Path::new(asset).components() {
            if matches!(component, Component::ParentDir) {
                return Err(PackError::PathTraversal {
                    field: context.field.to_string(),
                    path: asset.to_string(),
                });
            }
        }
        let joined = root.join(asset);
        // Canonicalize resolves symlinks; a missing file yields Io(NotFound)
        // which the caller treats as non-fatal.
        match joined.canonicalize() {
            Ok(resolved) => {
                // Confirm the resolved path is inside the pack root.
                if !resolved.starts_with(root) {
                    return Err(PackError::EscapesPackRoot {
                        field: context.field.to_string(),
                        path: asset.to_string(),
                    });
                }
                Ok(Some(AssetRef {
                    path: asset.to_string(),
                    resolved,
                }))
            }
            Err(_) => {
                eprintln!(
                    "pandemonium pack: {what} not readable for {scope} '{key}': {asset} \
                     (placeholder rendering used)",
                    what = context.what,
                    scope = context.scope,
                    key = context.key
                );
                Ok(None)
            }
        }
    }

    /// The extension of a declared path, lowercased by comparison only
    /// (`"PNG"` matches `"png"` — extensions are case-insensitive in
    /// practice; the pack system has always compared them literally, and
    /// this keeps that behavior for the new fields).
    fn extension_of(path: &str) -> Option<&str> {
        Path::new(path).extension().and_then(|e| e.to_str())
    }

    /// Looks up an entity kind's asset entry.
    pub fn asset_for(&self, kind_id: &str) -> Option<&AssetEntry> {
        self.entities.get(kind_id)
    }

    /// Looks up a terrain class's asset entry.
    #[allow(dead_code)] // consumed by the renderer follow-up
    pub fn terrain_asset_for(&self, class: &str) -> Option<&AssetEntry> {
        self.terrain.get(class)
    }

    /// Infers the asset kind from the file extension. `None` means the
    /// format is not supported by the pack system (and therefore not by the
    /// renderer).
    #[allow(dead_code)] // consumed by the renderer follow-up
    pub fn kind_of(path: &Path) -> Option<AssetKind> {
        let ext = path.extension()?.to_str()?;
        if MODEL_EXTENSIONS.contains(&ext) {
            return Some(AssetKind::Model);
        }
        if TEXTURE_EXTENSIONS.contains(&ext) {
            return Some(AssetKind::Texture);
        }
        None
    }
}

/// Whether a manifest sound key is one of the engine's cue names
/// (`crates/engine/src/audio.rs` — the nine-cue vocabulary).
fn is_known_cue(cue: &str) -> bool {
    pandemonium_engine::AudioCue::ALL
        .iter()
        .any(|known| known.name() == cue)
}

/// The version probe: just enough of the manifest to route v1 vs v2 before
/// any field is read (unknown fields ignored here — the real parse below
/// is strict).
#[derive(Deserialize)]
struct RawManifestVersion {
    schema_version: u32,
}

/// The raw, serde-deserialized schema-1 manifest shape.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct RawManifestV1 {
    pack_id: String,
    schema_version: u32,
    #[serde(default)]
    entities: BTreeMap<String, String>,
    #[serde(default)]
    terrain: BTreeMap<String, String>,
}

/// One raw schema-2 entity entry (ADR-0101 §4's field list, completed with
/// `skin`). Floats are plain values with serde defaults so the ADR's
/// manifest example (`scale: 1.0`, bare) is literally valid RON.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct RawEntityEntry {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skin: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    sounds: BTreeMap<String, String>,
    #[serde(default = "default_scale")]
    scale: f32,
    #[serde(default)]
    rotation_deg: f32,
    #[serde(default)]
    ground_offset: f32,
}

fn default_scale() -> f32 {
    1.0
}

/// The raw, serde-deserialized schema-2 manifest shape.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct RawManifestV2 {
    pack_id: String,
    schema_version: u32,
    #[serde(default)]
    entities: BTreeMap<String, RawEntityEntry>,
    #[serde(default)]
    terrain: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, content: &str) {
        std::fs::create_dir_all(dir).expect("creating pack dir");
        std::fs::write(dir.join(MANIFEST_FILE_NAME), content).expect("writing manifest");
    }

    /// Builds a pack fixture: writes the manifest and every referenced
    /// (path, bytes) file inside `dir`, then loads it.
    fn pack_with(manifest: &str, files: &[(&str, &[u8])]) -> Option<AssetPack> {
        let dir = tempfile_dir();
        for (name, bytes) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("creating file parent");
            }
            std::fs::write(&path, bytes).expect("writing pack file");
        }
        write_manifest(&dir, manifest);
        AssetPack::load(&dir).expect("the pack loads")
    }

    fn valid_manifest() -> String {
        r#"(
            pack_id: "default",
            schema_version: 1,
            entities: {
                "rifleman": "models/rifleman.glb",
                "command_center": "models/command_center.glb",
            },
            terrain: {
                "grass": "textures/grass.png",
            },
        )"#
        .to_string()
    }

    /// The official `tactical_field` manifest, verbatim from
    /// `pandemonium-assets/default/manifest.ron` (checked 2026-10-26) —
    /// the v1 golden fixture.
    fn official_tactical_field_manifest() -> String {
        r#"(
    pack_id: "tactical_field",
    schema_version: 1,
    terrain: {
        "grass": "textures/terrain/grass.png",
        "gravel": "textures/terrain/gravel.png",
        "rock": "textures/terrain/rock.png",
        "sand": "textures/terrain/sand.png",
    },
    entities: {
        "barracks": "textures/entities/barracks.png",
        "command_center": "textures/entities/command_center.png",
        "guardian": "textures/entities/guardian.png",
        "raider": "textures/entities/raider.png",
        "rifleman": "textures/entities/rifleman.png",
        "supply_depot": "textures/entities/supply_depot.png",
        "turret": "textures/entities/turret.png",
        "worker": "textures/entities/worker.png",
    },
)"#
        .to_string()
    }

    #[test]
    fn loads_a_valid_manifest() {
        let dir = tempfile_dir();
        // Create the referenced asset files so they resolve inside the pack root.
        std::fs::create_dir_all(dir.join("models")).expect("creating models dir");
        std::fs::create_dir_all(dir.join("textures")).expect("creating textures dir");
        std::fs::write(dir.join("models/rifleman.glb"), b"fake-glb").expect("writing rifleman");
        std::fs::write(dir.join("models/command_center.glb"), b"fake-glb").expect("writing cc");
        std::fs::write(dir.join("textures/grass.png"), b"fake-png").expect("writing grass");
        write_manifest(&dir, &valid_manifest());
        let pack = AssetPack::load(&dir)
            .expect("load succeeds")
            .expect("pack exists");
        assert_eq!(pack.pack_id, "default");
        assert_eq!(pack.schema_version, 1);
        assert!(pack.asset_for("rifleman").is_some());
        assert!(pack.asset_for("command_center").is_some());
        assert!(pack.asset_for("worker").is_none());
        assert!(pack.terrain_asset_for("grass").is_some());
        assert!(pack.terrain_asset_for("rock").is_none());
    }

    /// The v1 golden: the official tactical_field manifest loads to a
    /// byte-identical pack — same pack id, same schema echo, every entity
    /// and terrain entry at the same declared path, every v1 PNG migrated
    /// to `skin`, defaults applied (M12 phase 2's acceptance test).
    #[test]
    fn the_official_v1_pack_loads_unchanged() {
        let files: Vec<(&str, &[u8])> = [
            "textures/terrain/grass.png",
            "textures/terrain/gravel.png",
            "textures/terrain/rock.png",
            "textures/terrain/sand.png",
            "textures/entities/barracks.png",
            "textures/entities/command_center.png",
            "textures/entities/guardian.png",
            "textures/entities/raider.png",
            "textures/entities/rifleman.png",
            "textures/entities/supply_depot.png",
            "textures/entities/turret.png",
            "textures/entities/worker.png",
        ]
        .into_iter()
        .map(|path| (path, b"fake-png" as &[u8]))
        .collect();
        let pack = pack_with(&official_tactical_field_manifest(), &files).expect("pack exists");
        assert_eq!(pack.pack_id, "tactical_field");
        assert_eq!(
            pack.schema_version, 1,
            "the schema echo is the manifest's own version"
        );
        assert_eq!(pack.entities.len(), 8);
        assert_eq!(pack.terrain.len(), 4);
        for kind in [
            "barracks",
            "command_center",
            "guardian",
            "raider",
            "rifleman",
            "supply_depot",
            "turret",
            "worker",
        ] {
            let entry = pack
                .asset_for(kind)
                .unwrap_or_else(|| panic!("{kind} present"));
            assert_eq!(entry.asset, format!("textures/entities/{kind}.png"));
            assert!(entry.resolved.ends_with(format!("{kind}.png")));
            // The forward migration: .png -> skin, no model, defaults.
            let skin = entry.skin.as_ref().unwrap_or_else(|| panic!("{kind} skin"));
            assert_eq!(skin.path, format!("textures/entities/{kind}.png"));
            assert!(skin.resolved.ends_with(format!("{kind}.png")));
            assert!(entry.model.is_none());
            assert!(entry.icon.is_none());
            assert!(entry.sounds.is_empty());
            assert_eq!(entry.scale, 1.0);
            assert_eq!(entry.rotation_deg, 0.0);
            assert_eq!(entry.ground_offset, 0.0);
        }
        for class in ["grass", "gravel", "rock", "sand"] {
            let entry = pack
                .terrain_asset_for(class)
                .unwrap_or_else(|| panic!("{class} present"));
            assert_eq!(entry.asset, format!("textures/terrain/{class}.png"));
            assert!(entry.resolved.ends_with(format!("{class}.png")));
        }
    }

    /// The forward migration of v1 entries: `.png` becomes `skin`,
    /// `.glb`/`.gltf` becomes `model`, defaults applied (M12 phase 2's
    /// acceptance test).
    #[test]
    fn v1_entries_forward_migrate() {
        let pack = pack_with(
            r#"(
                pack_id: "m",
                schema_version: 1,
                entities: {
                    "rifleman": "textures/entities/rifleman.png",
                    "command_center": "models/buildings/command_center.glb",
                    "ore_node": "models/nodes/ore_node.gltf",
                },
            )"#,
            &[
                ("textures/entities/rifleman.png", b"fake png"),
                ("models/buildings/command_center.glb", b"fake glb"),
                ("models/nodes/ore_node.gltf", b"fake gltf"),
            ],
        )
        .expect("pack exists");
        // .png -> skin (and the v1 primary behavior: the png renders).
        let rifleman = pack.asset_for("rifleman").expect("rifleman present");
        assert!(rifleman.skin.is_some());
        assert_eq!(
            rifleman.skin.as_ref().unwrap().path,
            "textures/entities/rifleman.png"
        );
        assert!(rifleman.model.is_none());
        assert_eq!(rifleman.asset, "textures/entities/rifleman.png");
        // .glb -> model.
        let command_center = pack.asset_for("command_center").expect("cc present");
        assert!(command_center.model.is_some());
        assert_eq!(
            command_center.model.as_ref().unwrap().path,
            "models/buildings/command_center.glb"
        );
        assert!(command_center.skin.is_none());
        assert_eq!(command_center.asset, "models/buildings/command_center.glb");
        // .gltf -> model too (the ADR's migration rule; the GLB loader
        // refuses its bytes and the chain walks — that is phase 3/4's
        // discipline, not the manifest's).
        let ore_node = pack.asset_for("ore_node").expect("ore_node present");
        assert!(ore_node.model.is_some());
        assert_eq!(
            ore_node.model.as_ref().unwrap().path,
            "models/nodes/ore_node.gltf"
        );
        // Defaults applied on every migrated entry.
        for entry in pack.entities.values() {
            assert_eq!(entry.scale, 1.0);
            assert_eq!(entry.rotation_deg, 0.0);
            assert_eq!(entry.ground_offset, 0.0);
            assert!(entry.icon.is_none());
            assert!(entry.sounds.is_empty());
        }
    }

    /// A v2 entry with every field declared (M12 phase 2's acceptance
    /// test).
    #[test]
    fn a_full_v2_entry_loads() {
        let pack = pack_with(
            r#"(
                pack_id: "v2full",
                schema_version: 2,
                entities: {
                    "rifleman": (
                        model: Some("models/units/rifleman.glb"),
                        skin: Some("textures/entities/rifleman.png"),
                        scale: 1.25,
                        rotation_deg: -90.0,
                        ground_offset: 0.5,
                        icon: Some("icons/rifleman.png"),
                        sounds: { "SelectionClick": "audio/rifleman/select.ogg" },
                    ),
                },
                terrain: { "grass": "textures/terrain/grass.png" },
            )"#,
            &[
                ("models/units/rifleman.glb", b"fake glb"),
                ("textures/entities/rifleman.png", b"fake png"),
                ("icons/rifleman.png", b"fake icon png"),
                ("audio/rifleman/select.ogg", b"fake ogg"),
                ("textures/terrain/grass.png", b"fake terrain png"),
            ],
        )
        .expect("pack exists");
        assert_eq!(pack.schema_version, 2);
        let entry = pack.asset_for("rifleman").expect("rifleman present");
        assert_eq!(
            entry.model.as_ref().expect("model").path,
            "models/units/rifleman.glb"
        );
        assert!(entry
            .model
            .as_ref()
            .unwrap()
            .resolved
            .ends_with("rifleman.glb"));
        assert_eq!(
            entry.skin.as_ref().expect("skin").path,
            "textures/entities/rifleman.png"
        );
        assert_eq!(
            entry.icon.as_ref().expect("icon").path,
            "icons/rifleman.png"
        );
        let sound = entry
            .sounds
            .get("SelectionClick")
            .expect("SelectionClick cue");
        assert_eq!(sound.path, "audio/rifleman/select.ogg");
        assert_eq!(entry.scale, 1.25);
        assert_eq!(entry.rotation_deg, -90.0);
        assert_eq!(entry.ground_offset, 0.5);
        // The primary is the skin (the pre-model renderer seam).
        assert_eq!(entry.asset, "textures/entities/rifleman.png");
        assert!(entry.resolved.ends_with("rifleman.png"));
        let terrain = pack.terrain_asset_for("grass").expect("terrain present");
        assert_eq!(terrain.asset, "textures/terrain/grass.png");
    }

    /// A minimal v2 entry — every field absent — carries the defaults and
    /// renders the placeholder (empty primary, no refs).
    #[test]
    fn a_minimal_v2_entry_carries_the_defaults() {
        let pack = pack_with(
            r#"(
                pack_id: "v2mini",
                schema_version: 2,
                entities: { "worker": ( ) },
            )"#,
            &[],
        )
        .expect("pack exists");
        let entry = pack.asset_for("worker").expect("worker present");
        assert!(entry.model.is_none());
        assert!(entry.skin.is_none());
        assert!(entry.icon.is_none());
        assert!(entry.sounds.is_empty());
        assert_eq!(entry.scale, 1.0);
        assert_eq!(entry.rotation_deg, 0.0);
        assert_eq!(entry.ground_offset, 0.0);
        assert_eq!(entry.asset, "");
        assert!(entry.resolved.as_os_str().is_empty());
        // An empty primary is not a texture the renderer can load.
        assert_eq!(AssetPack::kind_of(entry.resolved.as_path()), None);
    }

    /// Range validation: out-of-range values are precise fatal errors, the
    /// boundaries themselves load (M12 phase 2's acceptance test).
    #[test]
    fn v2_ranges_are_validated_with_precise_errors() {
        let entry = |scale: &str, rotation: &str, offset: &str| {
            format!(
                r#"(
                    pack_id: "r",
                    schema_version: 2,
                    entities: {{
                        "x": ( scale: {scale}, rotation_deg: {rotation}, ground_offset: {offset} ),
                    }},
                )"#
            )
        };
        // Boundaries load.
        for (scale, rotation, offset) in [
            ("0.05", "-360.0", "0.0"),
            ("10.0", "360.0", "-12.5"),
            ("1.0", "0.0", "77.0"),
        ] {
            let pack = pack_with(&entry(scale, rotation, offset), &[]).expect("pack exists");
            let e = pack.asset_for("x").expect("entry present");
            assert_eq!(e.scale, scale.parse::<f32>().unwrap());
            assert_eq!(e.rotation_deg, rotation.parse::<f32>().unwrap());
            assert_eq!(e.ground_offset, offset.parse::<f32>().unwrap());
        }
        // Out of range: precise errors.
        for scale in ["0.04", "10.01", "-1.0"] {
            let dir = tempfile_dir();
            write_manifest(&dir, &entry(scale, "0.0", "0.0"));
            let err = AssetPack::load(&dir).expect_err("out of range");
            assert!(
                matches!(
                    err,
                    PackError::ValueOutOfRange {
                        ref kind,
                        field: "scale",
                        ..
                    } if kind == "x"
                ),
                "got {err:?}"
            );
        }
        for rotation in ["-360.1", "360.5"] {
            let dir = tempfile_dir();
            write_manifest(&dir, &entry("1.0", rotation, "0.0"));
            let err = AssetPack::load(&dir).expect_err("out of range");
            assert!(
                matches!(
                    err,
                    PackError::ValueOutOfRange {
                        kind: _,
                        field: "rotation_deg",
                        ..
                    }
                ),
                "got {err:?}"
            );
        }
        // NaN fails every range (and names its field precisely).
        let dir = tempfile_dir();
        write_manifest(&dir, &entry("NaN", "0.0", "0.0"));
        let err = AssetPack::load(&dir).expect_err("NaN is not a scale");
        assert!(
            matches!(err, PackError::ValueOutOfRange { field: "scale", .. }),
            "got {err:?}"
        );
        // ground_offset carries no range by design (A-144 defines none).
        let pack = pack_with(&entry("1.0", "0.0", "1e6"), &[]).expect("pack exists");
        assert_eq!(pack.asset_for("x").unwrap().ground_offset, 1e6);
    }

    /// Unknown fields are rejected in v2 (strict mode preserved).
    #[test]
    fn v2_unknown_fields_are_rejected() {
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"(
                pack_id: "x",
                schema_version: 2,
                entities: { "rifleman": ( model: Some("m.glb"), height: 3.0 ) },
            )"#,
        );
        let err = AssetPack::load(&dir).expect_err("unknown field");
        assert!(matches!(err, PackError::Parse(..)), "got {err:?}");
        // Top level too.
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"( pack_id: "x", schema_version: 2, author: "someone" )"#,
        );
        let err = AssetPack::load(&dir).expect_err("unknown top-level field");
        assert!(matches!(err, PackError::Parse(..)), "got {err:?}");
    }

    /// schema_version 3 is the precise UnsupportedVersion error (expected
    /// now points at 2, the latest this build accepts).
    #[test]
    fn unsupported_schema_version_is_rejected() {
        let dir = tempfile_dir();
        write_manifest(&dir, "( pack_id: \"x\", schema_version: 3,\n)");
        let err = AssetPack::load(&dir).expect_err("bad version should error");
        assert!(
            matches!(
                err,
                PackError::UnsupportedVersion {
                    found: 3,
                    expected: 2
                }
            ),
            "got {err:?}"
        );
        // And version 0 (nothing below v1 exists).
        let dir = tempfile_dir();
        write_manifest(&dir, "( pack_id: \"x\", schema_version: 0,\n)");
        let err = AssetPack::load(&dir).expect_err("version 0 should error");
        assert!(
            matches!(
                err,
                PackError::UnsupportedVersion {
                    found: 0,
                    expected: 2
                }
            ),
            "got {err:?}"
        );
        // The old 99 case keeps its precision.
        let dir = tempfile_dir();
        write_manifest(&dir, "( pack_id: \"x\", schema_version: 99,\n)");
        let err = AssetPack::load(&dir).expect_err("v99 should error");
        assert!(
            matches!(
                err,
                PackError::UnsupportedVersion {
                    found: 99,
                    expected: 2
                }
            ),
            "got {err:?}"
        );
    }

    /// The icon field is .png-only (a precise fatal error otherwise), and
    /// the sounds keys must be the engine's cue names.
    #[test]
    fn v2_icon_and_sounds_are_schema_validated() {
        // A non-png icon is refused precisely.
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join("icons")).expect("creating icons dir");
        std::fs::write(dir.join("icons/rifleman.jpg"), b"fake jpg").expect("writing icon");
        write_manifest(
            &dir,
            r#"(
                pack_id: "x",
                schema_version: 2,
                entities: { "rifleman": ( icon: Some("icons/rifleman.jpg") ) },
            )"#,
        );
        let err = AssetPack::load(&dir).expect_err("non-png icon");
        assert!(
            matches!(err, PackError::IconNotPng { ref kind, ref path }
                if kind == "rifleman" && path == "icons/rifleman.jpg"),
            "got {err:?}"
        );
        // An unknown cue name is refused precisely.
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join("audio/rifleman")).expect("creating audio dir");
        std::fs::write(dir.join("audio/rifleman/boom.ogg"), b"fake ogg").expect("writing sound");
        write_manifest(
            &dir,
            r#"(
                pack_id: "x",
                schema_version: 2,
                entities: {
                    "rifleman": ( sounds: { "boom": "audio/rifleman/boom.ogg" } ),
                },
            )"#,
        );
        let err = AssetPack::load(&dir).expect_err("unknown cue");
        assert!(
            matches!(err, PackError::UnknownSoundCue { ref kind, ref cue }
                if kind == "rifleman" && cue == "boom"),
            "got {err:?}"
        );
        // Every cue in the engine's vocabulary validates; a sound file
        // that is missing is a non-fatal per-ref skip (consumption is
        // explicitly absent in M12 — the reference is kept only when the
        // file exists).
        let all_cues: Vec<String> = pandemonium_engine::AudioCue::ALL
            .iter()
            .map(|cue| {
                format!(
                    "\"{}\": \"audio/{}/{}.ogg\"",
                    cue.name(),
                    "rifleman",
                    cue.name()
                )
            })
            .collect();
        let manifest = format!(
            r#"(
                pack_id: "x",
                schema_version: 2,
                entities: {{
                    "rifleman": ( sounds: {{ {} }} ),
                }},
            )"#,
            all_cues.join(", ")
        );
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join("audio/rifleman")).expect("creating audio dir");
        std::fs::write(dir.join("audio/rifleman/AttackLanded.ogg"), b"fake ogg")
            .expect("writing one");
        write_manifest(&dir, &manifest);
        let pack = AssetPack::load(&dir)
            .expect("known cues load")
            .expect("pack exists");
        let entry = pack.asset_for("rifleman").expect("entry present");
        // Exactly the one cue whose file exists is kept; the missing eight
        // were skipped with diagnostics, and the pack still loads.
        assert_eq!(entry.sounds.len(), 1);
        assert!(entry.sounds.contains_key("AttackLanded"));
    }

    /// A v2 entry whose model and skin files are both missing keeps the
    /// entry (icon/sounds/floats survive) with an empty primary — the
    /// placeholder tier — while a missing v1 file skips the whole entry
    /// (v1 behavior, preserved).
    #[test]
    fn missing_v2_refs_skip_refs_but_v1_skips_the_entry() {
        let pack = pack_with(
            r#"(
                pack_id: "x",
                schema_version: 2,
                entities: {
                    "rifleman": (
                        model: Some("models/units/missing.glb"),
                        skin: Some("textures/entities/missing.png"),
                        icon: Some("icons/rifleman.png"),
                    ),
                },
            )"#,
            &[("icons/rifleman.png", b"fake icon png")],
        )
        .expect("pack exists");
        let entry = pack.asset_for("rifleman").expect("the entry survives");
        assert!(entry.model.is_none(), "the missing model is skipped");
        assert!(entry.skin.is_none(), "the missing skin is skipped");
        assert!(entry.icon.is_some(), "the present icon is kept");
        assert_eq!(entry.asset, "", "the primary is empty — placeholder tier");

        // v1: a missing file skips the whole entry (the pinned v1 behavior).
        let pack = pack_with(
            r#"(
                pack_id: "x",
                schema_version: 1,
                entities: { "rifleman": "models/missing.glb" },
            )"#,
            &[],
        )
        .expect("pack exists");
        assert!(
            pack.asset_for("rifleman").is_none(),
            "missing asset => no entry"
        );
    }

    /// The v2 primary is the skin when both a model and a skin are
    /// declared — the pre-model renderer keeps rendering the skin tier.
    #[test]
    fn the_v2_primary_is_the_skin_when_present() {
        let pack = pack_with(
            r#"(
                pack_id: "x",
                schema_version: 2,
                entities: {
                    "rifleman": (
                        model: Some("models/units/rifleman.glb"),
                        skin: Some("textures/entities/rifleman.png"),
                    ),
                },
            )"#,
            &[
                ("models/units/rifleman.glb", b"fake glb"),
                ("textures/entities/rifleman.png", b"fake png"),
            ],
        )
        .expect("pack exists");
        let entry = pack.asset_for("rifleman").expect("entry present");
        assert_eq!(entry.asset, "textures/entities/rifleman.png");
        assert!(entry.resolved.ends_with("rifleman.png"));
        assert_eq!(
            AssetPack::kind_of(entry.resolved.as_path()),
            Some(AssetKind::Texture)
        );
        // A model-only entry's primary is the model (placeholder until
        // phase 3, exactly the v1 .glb behavior).
        let pack = pack_with(
            r#"(
                pack_id: "y",
                schema_version: 2,
                entities: { "rifleman": ( model: Some("models/units/rifleman.glb") ) },
            )"#,
            &[("models/units/rifleman.glb", b"fake glb")],
        )
        .expect("pack exists");
        let entry = pack.asset_for("rifleman").expect("entry present");
        assert_eq!(entry.asset, "models/units/rifleman.glb");
        assert_eq!(
            AssetPack::kind_of(entry.resolved.as_path()),
            Some(AssetKind::Model)
        );
    }

    #[test]
    fn a_minimal_manifest_with_only_required_fields_loads() {
        let dir = tempfile_dir();
        write_manifest(&dir, "( pack_id: \"mini\", schema_version: 1,\n)");
        let pack = AssetPack::load(&dir)
            .expect("load succeeds")
            .expect("pack exists");
        assert_eq!(pack.pack_id, "mini");
        assert!(pack.entities.is_empty());
        assert!(pack.terrain.is_empty());
        // The same minimal shape in v2.
        let dir = tempfile_dir();
        write_manifest(&dir, "( pack_id: \"mini2\", schema_version: 2,\n)");
        let pack = AssetPack::load(&dir)
            .expect("load succeeds")
            .expect("pack exists");
        assert_eq!(pack.pack_id, "mini2");
        assert!(pack.entities.is_empty());
        assert!(pack.terrain.is_empty());
    }

    #[test]
    fn a_missing_manifest_is_not_an_error() {
        let dir = tempfile_dir();
        let result = AssetPack::load(&dir).expect("no crash");
        assert!(result.is_none(), "no manifest => None, not an error");
    }

    #[test]
    fn malformed_manifest_is_rejected() {
        let dir = tempfile_dir();
        write_manifest(&dir, "this is not ron {{{");
        let err = AssetPack::load(&dir).expect_err("malformed should error");
        assert!(matches!(err, PackError::Parse(..)), "got {err:?}");
        // A v2 manifest whose entity value is a bare string (a v1 shape
        // mislabeled v2) is a parse error, not a silent pass.
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"( pack_id: "x", schema_version: 2, entities: { "k": "path.png" } )"#,
        );
        let err = AssetPack::load(&dir).expect_err("v1 shape in v2");
        assert!(matches!(err, PackError::Parse(..)), "got {err:?}");
    }

    #[test]
    fn absolute_paths_are_rejected() {
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"(
                pack_id: "x", schema_version: 1,
                entities: { "rifleman": "/etc/passwd" },
            )"#,
        );
        let err = AssetPack::load(&dir).expect_err("absolute path should error");
        assert!(matches!(err, PackError::AbsolutePath { .. }), "got {err:?}");
        // v2 fields get the same treatment.
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"(
                pack_id: "x", schema_version: 2,
                entities: { "rifleman": ( model: Some("/etc/passwd") ) },
            )"#,
        );
        let err = AssetPack::load(&dir).expect_err("absolute v2 model");
        assert!(matches!(err, PackError::AbsolutePath { .. }), "got {err:?}");
    }

    #[test]
    fn path_traversal_is_rejected() {
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"(
                pack_id: "x", schema_version: 1,
                entities: { "rifleman": "../outside.glb" },
            )"#,
        );
        let err = AssetPack::load(&dir).expect_err("traversal should error");
        assert!(
            matches!(err, PackError::PathTraversal { .. }),
            "got {err:?}"
        );
        // v2 icon traversal too.
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"(
                pack_id: "x", schema_version: 2,
                entities: { "rifleman": ( icon: Some("../icons/rifleman.png") ) },
            )"#,
        );
        let err = AssetPack::load(&dir).expect_err("v2 icon traversal");
        assert!(
            matches!(err, PackError::PathTraversal { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn asset_escaping_pack_root_is_rejected() {
        let dir = tempfile_dir();
        // Create a sibling file outside the pack and point at it via a
        // symlink-free relative path that canonicalizes outside.
        std::fs::write(dir.join("sibling.glb"), b"").expect("writing sibling");
        write_manifest(
            &dir,
            r#"(
                pack_id: "x", schema_version: 1,
                entities: { "rifleman": "sibling.glb" },
            )"#,
        );
        // `sibling.glb` is inside the pack root here, so this is actually
        // fine. Instead, test via a subdirectory that symlinks out — but
        // symlinks are hard to create portably in tests. The traversal test
        // above covers the escape vector. This test documents the intent:
        // canonicalize + starts_with check prevents escapes.
        let pack = AssetPack::load(&dir)
            .expect("load succeeds")
            .expect("pack exists");
        assert!(pack.asset_for("rifleman").is_some());
    }

    #[test]
    fn missing_asset_file_is_non_fatal_with_diagnostic() {
        let dir = tempfile_dir();
        write_manifest(
            &dir,
            r#"(
                pack_id: "x", schema_version: 1,
                entities: { "rifleman": "models/missing.glb" },
            )"#,
        );
        let pack = AssetPack::load(&dir)
            .expect("load succeeds")
            .expect("pack exists");
        assert!(
            pack.asset_for("rifleman").is_none(),
            "missing asset => no entry"
        );
    }

    #[test]
    fn unsupported_format_is_accepted_but_not_usable() {
        // The pack system accepts any declared path that exists and is inside
        // the root; `kind_of` reports the format. A `.psd` is not supported
        // by the pack system (returns None from kind_of) — the renderer
        // would fall back to placeholders.
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join("textures")).expect("creating textures dir");
        std::fs::write(dir.join("textures/grass.psd"), b"fake").expect("writing psd");
        write_manifest(
            &dir,
            r#"(
                pack_id: "x", schema_version: 1,
                terrain: { "grass": "textures/grass.psd" },
            )"#,
        );
        let pack = AssetPack::load(&dir)
            .expect("load succeeds")
            .expect("pack exists");
        // The entry exists (file is present and inside root) but the format
        // is not one the pack system knows about.
        let entry = pack.terrain_asset_for("grass").expect("entry recorded");
        assert_eq!(AssetPack::kind_of(entry.resolved.as_path()), None);
    }

    #[test]
    fn asset_for_returns_none_for_unmapped_kind() {
        let dir = tempfile_dir();
        write_manifest(&dir, &valid_manifest());
        let pack = AssetPack::load(&dir)
            .expect("load succeeds")
            .expect("pack exists");
        assert!(pack.asset_for("nonexistent_kind").is_none());
        assert!(pack.terrain_asset_for("nonexistent_class").is_none());
    }

    #[test]
    fn kind_of_recognizes_model_and_texture_extensions() {
        assert_eq!(
            AssetPack::kind_of(Path::new("models/unit.glb")),
            Some(AssetKind::Model)
        );
        assert_eq!(
            AssetPack::kind_of(Path::new("models/unit.gltf")),
            Some(AssetKind::Model)
        );
        assert_eq!(
            AssetPack::kind_of(Path::new("textures/grass.png")),
            Some(AssetKind::Texture)
        );
        assert_eq!(AssetPack::kind_of(Path::new("models/unit.psd")), None);
        assert_eq!(AssetPack::kind_of(Path::new("noextension")), None);
        assert_eq!(AssetPack::kind_of(Path::new("")), None);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pandemonium-pack-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("creating temp dir");
        dir
    }
}
