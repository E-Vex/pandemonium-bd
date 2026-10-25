//! The pure-data GLB loader (M12 Phase 1, ADR-0101): bytes to [`ModelData`],
//! with no GPU and no wgpu — unit-testable headless.
//!
//! This module owns the *data* half of the model pipeline only. The GLB
//! container and the glTF JSON are parsed by the `gltf` crate (parse-only
//! pin, ADR-0101 §7: `default-features = false, features = ["names"]` —
//! `names` is what makes the `team_color` material lookup possible), and the
//! accessor bytes are decoded here by hand: the crate's `utils` readers are
//! deliberately not enabled, so nothing beyond the parser itself rides the
//! dependency.
//!
//! Subset (ART_SPEC §§1, 5, 6, 10 — every rejection is a precise
//! [`ModelLoadError`], never a panic, never a silent skip):
//!
//! * one self-contained `.glb` — a single embedded BIN buffer, embedded
//!   images; any external `uri` (buffer or image) is refused;
//! * static meshes — `animations`, `skins`, and morph targets are counted
//!   and ignored (one load-time note per file carries the count, ADR-0101
//!   §2);
//! * primitive mode `TRIANGLES` only; `POSITION` required (`NORMAL` and
//!   `TEXCOORD_0` optional), `VEC3`/`VEC2` `FLOAT` attributes, `U8`/`U16`/
//!   `U32` indices; sparse accessors are refused;
//! * materials: `OPAQUE` alpha only, one albedo per material — an embedded
//!   `image/png` texture or a plain `baseColorFactor`;
//! * the pre-parse size cap (32 MiB, ART_SPEC §9) is checked before any
//!   byte of the file is parsed.
//!
//! Positions and normals come out in **meters** (1 glTF unit = 1 m), with
//! node-hierarchy transforms baked in. The `METERS_PER_TILE = 3.0` division
//! (ADR-0101 §6) is the Phase 3 renderer pass's business, not the loader's.
//! GPU upload, PNG decode, and the fallback chain are later phases: a bad
//! model is an `Err` here, and *walking* the chain is M12 Phase 4.

use glam::{Mat3, Mat4, Quat, Vec3};
use gltf::json::material::AlphaMode;
use gltf::json::mesh::{Mode, Primitive, Semantic};
use gltf::json::texture::Info as TextureInfo;
use gltf::json::validation::Checked;
use gltf::json::{accessor, Node, Root};

use thiserror::Error;

/// The pre-parse hard refuse: a model file larger than this is never parsed
/// (ART_SPEC §9 — one diagnostic, the fallback renders).
pub const MAX_GLB_BYTES: usize = 32 * 1024 * 1024;

/// The node-hierarchy depth cap: legal static model exports nest a handful
/// of levels; anything past this is malformed input trying to exhaust the
/// stack, and is refused with a precise error instead of recursing.
const MAX_NODE_DEPTH: usize = 64;

/// One loaded model: everything the Phase 3 renderer pass needs, nothing it
/// does not (ADR-0101 §§1-3).
#[derive(Clone, Debug)]
pub struct ModelData {
    /// One entry per mesh primitive in the default scene's node tree, in
    /// walk order, with node transforms already baked into the vertex data.
    pub primitives: Vec<PrimitiveData>,
    /// Every material in the file (primitives reference by index).
    pub materials: Vec<MaterialData>,
    /// The index of the first material named `team_color` (ADR-0101 §3);
    /// `None` when the model carries none.
    pub team_color: Option<usize>,
    /// How many materials are named `team_color` — 0 or 1 is clean; more
    /// than one resolves to the first and is reported by the caller as one
    /// note (the A-146 one-diagnostic discipline).
    pub team_color_materials: usize,
    /// How many ignorable animation data blocks the file carries
    /// (animations + skins + morph-target sets — ADR-0101 §2: ignored, and
    /// exactly one load-time note tells the Designer why).
    pub ignored_animation_data: usize,
}

/// One drawable primitive: raw vertex/index data in meters, plus the
/// material it renders with.
#[derive(Clone, Debug)]
pub struct PrimitiveData {
    /// Vertex positions (required — `VEC3` `FLOAT`), node transforms baked.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals, present only when the file carries `NORMAL`
    /// (`VEC3` `FLOAT`); the renderer computes a flat normal otherwise
    /// (ART_SPEC §5).
    pub normals: Option<Vec<[f32; 3]>>,
    /// `TEXCOORD_0` (`VEC2` `FLOAT`), present only when the file carries it;
    /// a textured material then renders as its base-color factor
    /// (ART_SPEC §5).
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Triangles as vertex indices (`U8`/`U16`/`U32`, validated against the
    /// position count); `None` for non-indexed triangle soup.
    pub indices: Option<Vec<u32>>,
    /// Index into [`ModelData::materials`]; `None` renders the glTF default
    /// material.
    pub material: Option<usize>,
}

/// One material's presentation slice: the albedo and whether it is the
/// `team_color` material (exact, case-sensitive name match — ADR-0101 §3).
#[derive(Clone, Debug)]
pub struct MaterialData {
    /// The material's albedo: the embedded PNG's raw bytes (decode and GPU
    /// upload are Phase 3's business, exactly like the skin path) or the
    /// plain `baseColorFactor`.
    pub albedo: Albedo,
    /// This material is named `team_color`.
    pub team_color: bool,
}

/// A material's albedo source (ART_SPEC §6: one albedo per material — a
/// texture *or* a factor).
#[derive(Clone, Debug, PartialEq)]
pub enum Albedo {
    /// Raw bytes of an embedded `image/png` texture (mimeType-validated;
    /// power-of-two and size limits gate delivery, not load).
    Texture(Vec<u8>),
    /// The `baseColorFactor` (glTF's default is white when absent).
    Factor([f32; 4]),
}

/// Every rejection path of the loader (ART_SPEC §10's failure map, each
/// with its precise cause; the caller turns one into the single
/// per-kind-per-run diagnostic — A-146).
#[derive(Debug, Error)]
pub enum ModelLoadError {
    #[error("the file is {size} bytes — over the {limit}-byte pre-parse refuse (ART_SPEC §9)")]
    Oversized { size: usize, limit: usize },
    #[error("not a GLB container (no glTF magic) — model entries are glTF 2.0 binary only (ADR-0101 §1)")]
    NotGlb,
    #[error("the GLB failed to parse or failed glTF validation: {0}")]
    Parse(String),
    #[error("the GLB declares a buffer but carries no BIN chunk")]
    MissingBinaryChunk,
    #[error("the BIN chunk holds {actual} bytes but the buffer declares {declared} — truncated")]
    TruncatedBinaryChunk { declared: u64, actual: u64 },
    #[error("buffer {index} is not the single embedded BIN buffer — pack models are self-contained .glb files (ART_SPEC §1)")]
    UnsupportedBuffer { index: usize },
    #[error("image {index} is not embedded in the GLB — pack models are self-contained .glb files (ART_SPEC §1)")]
    ExternalImage { index: usize },
    #[error("image {index} has neither a bufferView nor a uri — malformed glTF")]
    MalformedImage { index: usize },
    #[error("embedded image {index} carries no mimeType — malformed glTF")]
    MissingImageMimeType { index: usize },
    #[error("the texture of material {material} is unsupported ({detail}) — only embedded PNG on TEXCOORD_0 (ART_SPEC §6)")]
    UnsupportedTexture { material: usize, detail: String },
    #[error("material {index} uses alpha mode {mode} — only OPAQUE is accepted (ART_SPEC §6)")]
    UnsupportedAlphaMode { index: usize, mode: String },
    #[error("primitive {index} uses mode {mode} — only TRIANGLES is accepted (ART_SPEC §5)")]
    UnsupportedPrimitiveMode { index: usize, mode: String },
    #[error(
        "primitive {index} carries no POSITION attribute — positions are required (ART_SPEC §5)"
    )]
    MissingPositions { index: usize },
    #[error("accessor {index} ({semantic}) is outside the M12 subset: {detail}")]
    UnsupportedAccessor {
        index: usize,
        semantic: &'static str,
        detail: String,
    },
    #[error(
        "accessor {index} ({semantic}) reads to byte {needed} but the BIN chunk holds {available}"
    )]
    AccessorOutOfBounds {
        index: usize,
        semantic: &'static str,
        needed: u64,
        available: u64,
    },
    #[error("the index buffer references vertex {index} but the primitive has {count} positions")]
    InvalidIndices { index: u32, count: usize },
    #[error("the node hierarchy is invalid at node {node} ({reason})")]
    InvalidNodeHierarchy { node: usize, reason: &'static str },
    #[error("the GLB has no default scene — nothing to draw")]
    NoDefaultScene,
    #[error("the default scene carries no drawable mesh geometry")]
    NoDrawableGeometry,
}

/// Loads a `.glb`'s bytes into a [`ModelData`]. Never panics on any byte
/// sequence — that property is pinned by the fuzz test below, not merely
/// promised (ADR-0101 §5).
pub fn load(bytes: &[u8]) -> Result<ModelData, ModelLoadError> {
    if bytes.len() > MAX_GLB_BYTES {
        return Err(ModelLoadError::Oversized {
            size: bytes.len(),
            limit: MAX_GLB_BYTES,
        });
    }
    // Policy first: only the binary container form is a model entry. A
    // `.gltf` (JSON + external files) is the pack layer's
    // skip-with-diagnostic case (ADR-0101 §1); here it is simply not a GLB.
    if !bytes.starts_with(b"glTF") {
        return Err(ModelLoadError::NotGlb);
    }
    // Container + JSON in one call (magic, version, chunk lengths). The
    // crate's own validator is deliberately NOT run: its POSITION hook
    // indexes the accessor table unchecked, which the fuzz test proved is
    // a reachable panic on hostile bytes — so this loader validates its
    // own subset instead, every access bounds-checked (the never-crash
    // promise, ADR-0101 §5).
    let gltf = gltf::Gltf::from_slice_without_validation(bytes)
        .map_err(|error| ModelLoadError::Parse(error.to_string()))?;
    let root = gltf.document.as_json();
    let blob: &[u8] = gltf.blob.as_deref().unwrap_or(&[]);

    // One self-contained buffer, embedded as the BIN chunk (ART_SPEC §1).
    for (index, buffer) in root.buffers.iter().enumerate() {
        if index != 0 || buffer.uri.is_some() {
            return Err(ModelLoadError::UnsupportedBuffer { index });
        }
    }
    let declared = root.buffers.first().map(|b| b.byte_length.0).unwrap_or(0);
    if !root.buffers.is_empty() && gltf.blob.is_none() {
        return Err(ModelLoadError::MissingBinaryChunk);
    }
    if (blob.len() as u64) < declared {
        return Err(ModelLoadError::TruncatedBinaryChunk {
            declared,
            actual: blob.len() as u64,
        });
    }

    // Materials first: the whole file must sit inside the M12 material
    // subset (alpha, embedded-PNG-only textures) before any geometry is
    // read (ART_SPEC §10's order).
    let mut materials = Vec::with_capacity(root.materials.len());
    let mut team_color = None;
    let mut team_color_materials = 0usize;
    for (index, material) in root.materials.iter().enumerate() {
        match material.alpha_mode {
            Checked::Valid(AlphaMode::Opaque) => {}
            Checked::Valid(other) => {
                return Err(ModelLoadError::UnsupportedAlphaMode {
                    index,
                    mode: format!("{other:?}"),
                })
            }
            Checked::Invalid => {
                return Err(ModelLoadError::UnsupportedAlphaMode {
                    index,
                    mode: "unknown".to_string(),
                })
            }
        }
        let pbr = &material.pbr_metallic_roughness;
        let albedo = match &pbr.base_color_texture {
            Some(info) => {
                let image_index = texture_image_index(root, info)?;
                Albedo::Texture(embedded_png_bytes(root, blob, index, image_index, info)?)
            }
            None => Albedo::Factor(pbr.base_color_factor.0),
        };
        let is_team_color = material.name.as_deref() == Some("team_color");
        if is_team_color {
            team_color_materials += 1;
            if team_color.is_none() {
                team_color = Some(index);
            }
        }
        materials.push(MaterialData {
            albedo,
            team_color: is_team_color,
        });
    }

    // Static-mesh subset: animations, skins, and morph targets are counted
    // and ignored (ADR-0101 §2) — one load-time note names the count.
    let mut ignored_animation_data = root.animations.len() + root.skins.len();

    // Walk the default scene's node tree, baking node transforms.
    let scene_index = root.scene.ok_or(ModelLoadError::NoDefaultScene)?.value();
    let scene = root
        .scenes
        .get(scene_index)
        .ok_or_else(|| ModelLoadError::Parse("scene index out of range".to_string()))?;
    let mut loader = Loader {
        root,
        blob,
        primitives: Vec::new(),
    };
    let mut path = Vec::new();
    for node in &scene.nodes {
        loader.walk(
            node.value(),
            Mat4::IDENTITY,
            &mut path,
            &mut ignored_animation_data,
        )?;
    }
    if loader.primitives.is_empty() {
        return Err(ModelLoadError::NoDrawableGeometry);
    }

    Ok(ModelData {
        primitives: loader.primitives,
        materials,
        team_color,
        team_color_materials,
        ignored_animation_data,
    })
}

/// The shape an accessor must have to be read as a given attribute
/// (ART_SPEC §5's subset, made checkable): the component count per element
/// and whether the accessor's component type and dimensions sit inside it.
enum ExpectedAccessor {
    /// `POSITION` / `NORMAL`: `VEC3` of `FLOAT`.
    Vec3F32,
    /// `TEXCOORD_0`: `VEC2` of `FLOAT`.
    Vec2F32,
    /// Indices: `SCALAR` of `U8`/`U16`/`U32`.
    IndexScalar,
}

impl ExpectedAccessor {
    fn check(
        &self,
        component: accessor::ComponentType,
        dimensions: accessor::Type,
    ) -> (usize, bool) {
        match self {
            ExpectedAccessor::Vec3F32 => (
                3,
                component == accessor::ComponentType::F32 && dimensions == accessor::Type::Vec3,
            ),
            ExpectedAccessor::Vec2F32 => (
                2,
                component == accessor::ComponentType::F32 && dimensions == accessor::Type::Vec2,
            ),
            ExpectedAccessor::IndexScalar => (
                1,
                matches!(
                    component,
                    accessor::ComponentType::U8
                        | accessor::ComponentType::U16
                        | accessor::ComponentType::U32
                ) && dimensions == accessor::Type::Scalar,
            ),
        }
    }
}

/// Resolves a base-color texture to its image index (the texture table's
/// source).
fn texture_image_index(root: &Root, info: &TextureInfo) -> Result<usize, ModelLoadError> {
    let texture = root
        .textures
        .get(info.index.value())
        .ok_or_else(|| ModelLoadError::Parse("texture index out of range".to_string()))?;
    let image_index = texture.source.value();
    if root.images.get(image_index).is_none() {
        return Err(ModelLoadError::Parse(
            "image index out of range".to_string(),
        ));
    }
    Ok(image_index)
}

/// Validates one embedded texture of `material` (PNG mimeType, TEXCOORD_0,
/// inside the BIN chunk) and returns its raw bytes.
fn embedded_png_bytes(
    root: &Root,
    blob: &[u8],
    material: usize,
    image_index: usize,
    info: &TextureInfo,
) -> Result<Vec<u8>, ModelLoadError> {
    let image = root
        .images
        .get(image_index)
        .ok_or_else(|| ModelLoadError::Parse("image index out of range".to_string()))?;
    if image.uri.is_some() {
        return Err(ModelLoadError::ExternalImage { index: image_index });
    }
    let view_index = image
        .buffer_view
        .ok_or(ModelLoadError::MalformedImage { index: image_index })?
        .value();
    match &image.mime_type {
        Some(mime) if mime.0 == "image/png" => {}
        Some(mime) => {
            return Err(ModelLoadError::UnsupportedTexture {
                material,
                detail: format!("mimeType '{}'", mime.0),
            })
        }
        None => return Err(ModelLoadError::MissingImageMimeType { index: image_index }),
    }
    if info.tex_coord != 0 {
        return Err(ModelLoadError::UnsupportedTexture {
            material,
            detail: format!("texCoord {}", info.tex_coord),
        });
    }
    let view = root
        .buffer_views
        .get(view_index)
        .ok_or_else(|| ModelLoadError::Parse("image bufferView index out of range".to_string()))?;
    let start = view.byte_offset.map(|offset| offset.0).unwrap_or(0);
    let end = start
        .checked_add(view.byte_length.0)
        .ok_or_else(|| ModelLoadError::Parse("bufferView arithmetic overflow".to_string()))?;
    if end > blob.len() as u64 {
        return Err(ModelLoadError::AccessorOutOfBounds {
            index: view_index,
            semantic: "image data",
            needed: end,
            available: blob.len() as u64,
        });
    }
    Ok(blob[start as usize..end as usize].to_vec())
}

/// The node-walk state: the parsed document, the BIN chunk, and the
/// primitives accumulated so far.
struct Loader<'a> {
    root: &'a Root,
    blob: &'a [u8],
    primitives: Vec<PrimitiveData>,
}

impl<'a> Loader<'a> {
    /// Visits one node (and its subtree) with `parent` as the accumulated
    /// ancestor transform. The `path` stack carries the ancestor node
    /// indices: a node that is its own ancestor is a cyclic hierarchy
    /// (glTF forbids it; the parser does not check it, so this walk does —
    /// the never-crash promise is only as good as its recursion guard).
    fn walk(
        &mut self,
        node_index: usize,
        parent: Mat4,
        path: &mut Vec<usize>,
        ignored_animation_data: &mut usize,
    ) -> Result<(), ModelLoadError> {
        if path.len() >= MAX_NODE_DEPTH {
            return Err(ModelLoadError::InvalidNodeHierarchy {
                node: node_index,
                reason: "nested deeper than 64 levels",
            });
        }
        if path.contains(&node_index) {
            return Err(ModelLoadError::InvalidNodeHierarchy {
                node: node_index,
                reason: "cyclic (a node is its own ancestor)",
            });
        }
        let node = self
            .root
            .nodes
            .get(node_index)
            .ok_or_else(|| ModelLoadError::Parse("node index out of range".to_string()))?;
        let world = parent * node_transform(node);
        if let Some(mesh) = node.mesh {
            let mesh = self
                .root
                .meshes
                .get(mesh.value())
                .ok_or_else(|| ModelLoadError::Parse("mesh index out of range".to_string()))?;
            for primitive in &mesh.primitives {
                if primitive.targets.is_some() {
                    *ignored_animation_data += 1;
                }
                let index = self.primitives.len();
                let data = self.read_primitive(index, primitive, world)?;
                self.primitives.push(data);
            }
        }
        path.push(node_index);
        for child in node.children.iter().flatten() {
            self.walk(child.value(), world, path, ignored_animation_data)?;
        }
        path.pop();
        Ok(())
    }

    /// Reads one primitive's data, baking `world` into the vertices.
    fn read_primitive(
        &self,
        index: usize,
        primitive: &Primitive,
        world: Mat4,
    ) -> Result<PrimitiveData, ModelLoadError> {
        match primitive.mode {
            Checked::Valid(Mode::Triangles) => {}
            Checked::Valid(other) => {
                return Err(ModelLoadError::UnsupportedPrimitiveMode {
                    index,
                    mode: format!("{other:?}"),
                })
            }
            Checked::Invalid => {
                return Err(ModelLoadError::UnsupportedPrimitiveMode {
                    index,
                    mode: "unknown".to_string(),
                })
            }
        }
        let mut position_accessor = None;
        let mut normal_accessor = None;
        let mut uv_accessor = None;
        for (semantic, accessor) in &primitive.attributes {
            match semantic.as_ref() {
                Checked::Valid(Semantic::Positions) => position_accessor = Some(accessor.value()),
                Checked::Valid(Semantic::Normals) => normal_accessor = Some(accessor.value()),
                Checked::Valid(Semantic::TexCoords(0)) => uv_accessor = Some(accessor.value()),
                // Everything else (TANGENT, COLOR_*, TEXCOORD_1+, custom
                // semantics, animation joints/weights) is not consumed by
                // the M12 subset and rides along harmlessly.
                _ => {}
            }
        }
        let position_accessor =
            position_accessor.ok_or(ModelLoadError::MissingPositions { index })?;

        let mut positions = self.read_vec3(position_accessor, "POSITION")?;
        let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
        for position in positions.iter_mut() {
            *position = world
                .transform_point3(Vec3::from_array(*position))
                .to_array();
        }
        let normals = match normal_accessor {
            Some(accessor) => {
                let mut normals = self.read_vec3(accessor, "NORMAL")?;
                for normal in normals.iter_mut() {
                    *normal = (normal_matrix * Vec3::from_array(*normal))
                        .normalize_or_zero()
                        .to_array();
                }
                Some(normals)
            }
            None => None,
        };
        let uvs = match uv_accessor {
            Some(accessor) => Some(self.read_vec2(accessor, "TEXCOORD_0")?),
            None => None,
        };
        let indices = match primitive.indices {
            Some(accessor) => {
                let indices = self.read_indices(accessor.value())?;
                for &vertex in &indices {
                    if vertex as usize >= positions.len() {
                        return Err(ModelLoadError::InvalidIndices {
                            index: vertex,
                            count: positions.len(),
                        });
                    }
                }
                Some(indices)
            }
            None => None,
        };
        let material = match primitive.material {
            Some(material) => {
                let material_index = material.value();
                if self.root.materials.get(material_index).is_none() {
                    return Err(ModelLoadError::Parse(format!(
                        "primitive {index} references material {material_index}, \
                         which does not exist"
                    )));
                }
                Some(material_index)
            }
            None => None,
        };
        Ok(PrimitiveData {
            positions,
            normals,
            uvs,
            indices,
            material,
        })
    }

    /// Reads a `VEC3` `FLOAT` accessor.
    fn read_vec3(
        &self,
        accessor_index: usize,
        semantic: &'static str,
    ) -> Result<Vec<[f32; 3]>, ModelLoadError> {
        let (offset, stride, count, _) =
            self.accessor_slice(accessor_index, semantic, ExpectedAccessor::Vec3F32)?;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let base = offset + i * stride;
            out.push([
                read_f32(self.blob, base),
                read_f32(self.blob, base + 4),
                read_f32(self.blob, base + 8),
            ]);
        }
        Ok(out)
    }

    /// Reads a `VEC2` `FLOAT` accessor.
    fn read_vec2(
        &self,
        accessor_index: usize,
        semantic: &'static str,
    ) -> Result<Vec<[f32; 2]>, ModelLoadError> {
        let (offset, stride, count, _) =
            self.accessor_slice(accessor_index, semantic, ExpectedAccessor::Vec2F32)?;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let base = offset + i * stride;
            out.push([read_f32(self.blob, base), read_f32(self.blob, base + 4)]);
        }
        Ok(out)
    }

    /// Reads a `SCALAR` index accessor (`U8`/`U16`/`U32`), widened to `u32`.
    fn read_indices(&self, accessor_index: usize) -> Result<Vec<u32>, ModelLoadError> {
        let (offset, stride, count, width) =
            self.accessor_slice(accessor_index, "indices", ExpectedAccessor::IndexScalar)?;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let base = offset + i * stride;
            let value = match width {
                1 => self.blob[base] as u32,
                2 => u16::from_le_bytes([self.blob[base], self.blob[base + 1]]) as u32,
                _ => u32::from_le_bytes([
                    self.blob[base],
                    self.blob[base + 1],
                    self.blob[base + 2],
                    self.blob[base + 3],
                ]),
            };
            out.push(value);
        }
        Ok(out)
    }

    /// Validates one accessor against the expected shape and returns
    /// `(byte offset, stride, count, element size)` with the data's end
    /// proven inside the BIN chunk. All arithmetic is checked — a hostile
    /// file cannot make this slice panic.
    fn accessor_slice(
        &self,
        accessor_index: usize,
        semantic: &'static str,
        expected: ExpectedAccessor,
    ) -> Result<(usize, usize, usize, usize), ModelLoadError> {
        let accessor = self
            .root
            .accessors
            .get(accessor_index)
            .ok_or_else(|| ModelLoadError::Parse("accessor index out of range".to_string()))?;
        if accessor.sparse.is_some() {
            return Err(ModelLoadError::UnsupportedAccessor {
                index: accessor_index,
                semantic,
                detail: "sparse accessors are outside the M12 subset".to_string(),
            });
        }
        let component = match accessor.component_type.as_ref() {
            Checked::Valid(accessor::GenericComponentType(component)) => *component,
            Checked::Invalid => {
                return Err(ModelLoadError::UnsupportedAccessor {
                    index: accessor_index,
                    semantic,
                    detail: "unknown componentType".to_string(),
                })
            }
        };
        let dimensions = match accessor.type_.as_ref() {
            Checked::Valid(dimensions) => *dimensions,
            Checked::Invalid => {
                return Err(ModelLoadError::UnsupportedAccessor {
                    index: accessor_index,
                    semantic,
                    detail: "unknown accessor type".to_string(),
                })
            }
        };
        let (components, ok) = expected.check(component, dimensions);
        if !ok {
            return Err(ModelLoadError::UnsupportedAccessor {
                index: accessor_index,
                semantic,
                detail: format!("componentType {component:?} with type {dimensions:?}"),
            });
        }
        let component_size = match component {
            accessor::ComponentType::I8 | accessor::ComponentType::U8 => 1,
            accessor::ComponentType::I16 | accessor::ComponentType::U16 => 2,
            _ => 4,
        };
        let element = components * component_size;
        let view_index = accessor
            .buffer_view
            .ok_or(ModelLoadError::UnsupportedAccessor {
                index: accessor_index,
                semantic,
                detail: "no bufferView (and not sparse)".to_string(),
            })?
            .value();
        let view =
            self.root.buffer_views.get(view_index).ok_or_else(|| {
                ModelLoadError::Parse("bufferView index out of range".to_string())
            })?;
        // All accessor data comes from the one embedded BIN buffer — a view
        // naming any other (or a nonexistent) buffer is not self-contained.
        if self.root.buffers.is_empty() || view.buffer.value() != 0 {
            return Err(ModelLoadError::UnsupportedBuffer {
                index: view.buffer.value(),
            });
        }
        let stride = match &view.byte_stride {
            Some(stride) => {
                let stride = stride.0;
                if stride < element {
                    return Err(ModelLoadError::UnsupportedAccessor {
                        index: accessor_index,
                        semantic,
                        detail: format!(
                            "byteStride {stride} is smaller than the element size {element}"
                        ),
                    });
                }
                stride
            }
            None => element,
        };
        let start = view
            .byte_offset
            .map(|offset| offset.0)
            .unwrap_or(0)
            .checked_add(accessor.byte_offset.map(|offset| offset.0).unwrap_or(0))
            .ok_or_else(|| ModelLoadError::Parse("accessor offset overflow".to_string()))?;
        let count = accessor.count.0;
        // The last element needs only `element` bytes — trailing stride
        // padding is not required (the glTF accessor layout rule).
        let needed_end = if count == 0 {
            start
        } else {
            start
                .checked_add((count - 1).saturating_mul(stride as u64))
                .and_then(|end| end.checked_add(element as u64))
                .ok_or_else(|| ModelLoadError::Parse("accessor range overflow".to_string()))?
        };
        if needed_end > self.blob.len() as u64 {
            return Err(ModelLoadError::AccessorOutOfBounds {
                index: accessor_index,
                semantic,
                needed: needed_end,
                available: self.blob.len() as u64,
            });
        }
        Ok((start as usize, stride, count as usize, element))
    }
}

/// A node's local transform: the explicit `matrix` when present, otherwise
/// the translation/rotation/scale triple with glTF's defaults.
fn node_transform(node: &Node) -> Mat4 {
    if let Some(matrix) = node.matrix {
        return Mat4::from_cols_array(&matrix);
    }
    let translation = node.translation.unwrap_or([0.0; 3]);
    let rotation = node
        .rotation
        .unwrap_or(gltf::json::scene::UnitQuaternion([0.0, 0.0, 0.0, 1.0]));
    let scale = node.scale.unwrap_or([1.0; 3]);
    Mat4::from_scale_rotation_translation(
        Vec3::from_array(scale),
        Quat::from_array(rotation.0),
        Vec3::from_array(translation),
    )
}

/// Reads one little-endian `f32` (bounds proven by the caller's slice math).
fn read_f32(data: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assembles a GLB container around one JSON chunk and an optional BIN
    /// chunk (the hand-built fixtures every test below parses).
    fn assemble_glb(json: &str, bin: Option<&[u8]>) -> Vec<u8> {
        let mut json_chunk = json.as_bytes().to_vec();
        while !json_chunk.len().is_multiple_of(4) {
            json_chunk.push(b' ');
        }
        let bin_padded = bin.map(|b| b.len().div_ceil(4) * 4).unwrap_or(0);
        let total = 12 + 8 + json_chunk.len() + if bin.is_some() { 8 + bin_padded } else { 0 };
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_chunk);
        if let Some(bin) = bin {
            out.extend_from_slice(&(bin_padded as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(bin);
            out.extend(std::iter::repeat_n(0u8, bin_padded - bin.len()));
        }
        out
    }

    fn push_f32s(out: &mut Vec<u8>, values: &[f32]) {
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn push_u16s(out: &mut Vec<u8>, values: &[u16]) {
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// The BIN payload of the valid box: 8 positions, 8 normals, 8 uvs,
    /// 36 u16 indices (328 bytes), laid out exactly as the JSON's four
    /// bufferViews declare.
    fn box_bin() -> Vec<u8> {
        let mut bin = Vec::new();
        push_f32s(
            &mut bin,
            &[
                -0.5, -0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5, -0.5, 0.5, 0.5, -0.5, -0.5, -0.5,
                0.5, 0.5, -0.5, 0.5, -0.5, 0.5, 0.5, 0.5, 0.5, 0.5,
            ],
        );
        for _ in 0..8 {
            push_f32s(&mut bin, &[0.0, 1.0, 0.0]);
        }
        for uv in [
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
        ] {
            push_f32s(&mut bin, &uv);
        }
        push_u16s(
            &mut bin,
            &[
                0, 1, 2, 1, 3, 2, 4, 5, 6, 5, 7, 6, 0, 1, 4, 1, 5, 4, 2, 3, 6, 3, 7, 6, 0, 2, 4, 2,
                6, 4, 1, 3, 5, 3, 7, 5,
            ],
        );
        assert_eq!(bin.len(), 328);
        bin
    }

    /// The minimal valid box document, written compact so every fixture
    /// mutation below is a plain substring replace: one scene, one node,
    /// one mesh, one TRIANGLES primitive (POSITION + NORMAL + TEXCOORD_0,
    /// u16 indices, one white `team_color` material). Accessor 0 carries
    /// the min/max the glTF spec demands of POSITION.
    fn box_json() -> String {
        r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0}]}],"materials":[{"name":"team_color","pbrMetallicRoughness":{"baseColorFactor":[1.0,1.0,1.0,1.0]}}],"accessors":[{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]},{"bufferView":1,"componentType":5126,"count":8,"type":"VEC3"},{"bufferView":2,"componentType":5126,"count":8,"type":"VEC2"},{"bufferView":3,"componentType":5123,"count":36,"type":"SCALAR"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":96},{"buffer":0,"byteOffset":96,"byteLength":96},{"buffer":0,"byteOffset":192,"byteLength":64},{"buffer":0,"byteOffset":256,"byteLength":72}],"buffers":[{"byteLength":328}]}"#
            .to_string()
    }

    fn valid_box() -> Vec<u8> {
        assemble_glb(&box_json(), Some(&box_bin()))
    }

    #[test]
    fn a_valid_box_parses() {
        let model = load(&valid_box()).expect("the valid box loads");
        assert_eq!(model.primitives.len(), 1);
        let primitive = &model.primitives[0];
        assert_eq!(primitive.positions.len(), 8);
        assert_eq!(primitive.positions[0], [-0.5, -0.5, -0.5]);
        assert_eq!(primitive.positions[7], [0.5, 0.5, 0.5]);
        let normals = primitive.normals.as_ref().expect("normals present");
        assert_eq!(normals.len(), 8);
        assert_eq!(normals[0], [0.0, 1.0, 0.0]);
        let uvs = primitive.uvs.as_ref().expect("uvs present");
        assert_eq!(uvs.len(), 8);
        assert_eq!(uvs[3], [1.0, 1.0]);
        let indices = primitive.indices.as_ref().expect("indices present");
        assert_eq!(indices.len(), 36);
        assert_eq!(indices[0], 0);
        assert_eq!(primitive.material, Some(0));
        // team_color: present, exactly one, white factor.
        assert_eq!(model.team_color, Some(0));
        assert_eq!(model.team_color_materials, 1);
        assert_eq!(model.materials.len(), 1);
        assert!(model.materials[0].team_color);
        assert_eq!(
            model.materials[0].albedo,
            Albedo::Factor([1.0, 1.0, 1.0, 1.0])
        );
        assert_eq!(model.ignored_animation_data, 0);
    }

    #[test]
    fn oversized_files_are_refused_before_parsing() {
        // One byte over the cap, with garbage behind a valid magic: the
        // size check fires before any parsing could choke.
        let mut bytes = Vec::with_capacity(MAX_GLB_BYTES + 1);
        bytes.extend_from_slice(b"glTF");
        bytes.resize(MAX_GLB_BYTES + 1, 0xAB);
        let error = load(&bytes).expect_err("over the pre-parse cap");
        assert!(
            matches!(
                error,
                ModelLoadError::Oversized {
                    size,
                    limit: MAX_GLB_BYTES
                } if size == MAX_GLB_BYTES + 1
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn non_glb_input_is_refused() {
        // Plain .gltf JSON (or any other bytes) is not a model entry.
        let error = load(b"{\"asset\": {\"version\": \"2.0\"}}").expect_err("not a GLB");
        assert!(matches!(error, ModelLoadError::NotGlb), "got {error:?}");
    }

    #[test]
    fn malformed_json_is_refused() {
        let bytes = assemble_glb("{ this is not json ]]", Some(&box_bin()));
        let error = load(&bytes).expect_err("malformed JSON chunk");
        assert!(matches!(error, ModelLoadError::Parse(_)), "got {error:?}");
    }

    #[test]
    fn truncated_bin_chunk_is_refused() {
        let bin = box_bin();
        let bytes = assemble_glb(&box_json(), Some(&bin[..100]));
        let error = load(&bytes).expect_err("BIN shorter than declared");
        assert!(
            matches!(
                error,
                ModelLoadError::TruncatedBinaryChunk {
                    declared: 328,
                    actual: 100
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn missing_bin_chunk_is_refused() {
        let bytes = assemble_glb(&box_json(), None);
        let error = load(&bytes).expect_err("buffers declared, no BIN chunk");
        assert!(
            matches!(error, ModelLoadError::MissingBinaryChunk),
            "got {error:?}"
        );
    }

    #[test]
    fn external_buffer_uri_is_refused() {
        let json = box_json().replace(
            r#""buffers":[{"byteLength":328}]"#,
            r#""buffers":[{"byteLength":328,"uri":"parts.bin"}]"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("external buffer uri");
        assert!(
            matches!(error, ModelLoadError::UnsupportedBuffer { index: 0 }),
            "got {error:?}"
        );
    }

    #[test]
    fn points_mode_is_refused() {
        let json = box_json().replace(
            r#"{"attributes":{"POSITION":0"#,
            r#"{"mode":0,"attributes":{"POSITION":0"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("POINTS mode");
        assert!(
            matches!(
                error,
                ModelLoadError::UnsupportedPrimitiveMode {
                    index: 0,
                    ref mode
                } if mode == "Points"
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn alpha_blend_is_refused() {
        let json = box_json().replace(
            r#""name":"team_color""#,
            r#""name":"team_color","alphaMode":"BLEND""#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("BLEND alpha");
        assert!(
            matches!(
                error,
                ModelLoadError::UnsupportedAlphaMode {
                    index: 0,
                    ref mode
                } if mode == "Blend"
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn alpha_mask_is_refused() {
        let json = box_json().replace(
            r#""name":"team_color""#,
            r#""name":"team_color","alphaMode":"MASK""#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("MASK alpha");
        assert!(
            matches!(
                error,
                ModelLoadError::UnsupportedAlphaMode {
                    index: 0,
                    ref mode
                } if mode == "Mask"
            ),
            "got {error:?}"
        );
    }

    /// A textured box: the material's albedo becomes a baseColorTexture and
    /// the document gains a texture, an image (the caller's object), and a
    /// fifth bufferView of 16 bytes at offset 328 for the embedded data.
    fn textured_box_json(image: &str) -> String {
        box_json()
            .replace(
                r#""pbrMetallicRoughness":{"baseColorFactor":[1.0,1.0,1.0,1.0]}}"#,
                r#""pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
            )
            .replace(
                r#""bufferViews":["#,
                &format!(r#""textures":[{{"source":0}}],"images":[{image}],"bufferViews":["#),
            )
            .replace(
                r#"{"buffer":0,"byteOffset":256,"byteLength":72}"#,
                r#"{"buffer":0,"byteOffset":256,"byteLength":72},{"buffer":0,"byteOffset":328,"byteLength":16}"#,
            )
            .replace(r#""buffers":[{"byteLength":328}]"#, r#""buffers":[{"byteLength":344}]"#)
    }

    fn textured_box_bin() -> Vec<u8> {
        let mut bin = box_bin();
        bin.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bin.len(), 344);
        bin
    }

    #[test]
    fn embedded_png_texture_parses() {
        let bytes = assemble_glb(
            &textured_box_json(r#"{"bufferView":4,"mimeType":"image/png"}"#),
            Some(&textured_box_bin()),
        );
        let model = load(&bytes).expect("embedded PNG texture loads");
        assert_eq!(
            model.materials[0].albedo,
            Albedo::Texture(vec![
                0x89, 0x50, 0x4E, 0x47, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ])
        );
    }

    #[test]
    fn jpeg_texture_is_refused() {
        let bytes = assemble_glb(
            &textured_box_json(r#"{"bufferView":4,"mimeType":"image/jpeg"}"#),
            Some(&textured_box_bin()),
        );
        let error = load(&bytes).expect_err("JPEG texture");
        assert!(
            matches!(
                error,
                ModelLoadError::UnsupportedTexture {
                    material: 0,
                    ref detail
                } if detail.contains("image/jpeg")
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn webp_texture_is_refused() {
        let bytes = assemble_glb(
            &textured_box_json(r#"{"bufferView":4,"mimeType":"image/webp"}"#),
            Some(&textured_box_bin()),
        );
        let error = load(&bytes).expect_err("WebP texture");
        assert!(
            matches!(
                error,
                ModelLoadError::UnsupportedTexture {
                    material: 0,
                    ref detail
                } if detail.contains("image/webp")
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn external_image_uri_is_refused() {
        // An image that points outside the file is not self-contained.
        let json = box_json()
            .replace(
                r#""pbrMetallicRoughness":{"baseColorFactor":[1.0,1.0,1.0,1.0]}}"#,
                r#""pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
            )
            .replace(
                r#""bufferViews":["#,
                r#""textures":[{"source":0}],"images":[{"uri":"skin.png","mimeType":"image/png"}],"bufferViews":["#,
            );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("external image uri");
        assert!(
            matches!(error, ModelLoadError::ExternalImage { index: 0 }),
            "got {error:?}"
        );
    }

    #[test]
    fn image_without_mime_type_is_refused() {
        // mimeType is required for embedded images (the glTF spec says so;
        // the parser's high-level API would panic on this input — this
        // loader refuses it precisely instead).
        let bytes = assemble_glb(
            &textured_box_json(r#"{"bufferView":4}"#),
            Some(&textured_box_bin()),
        );
        let error = load(&bytes).expect_err("no mimeType");
        assert!(
            matches!(error, ModelLoadError::MissingImageMimeType { index: 0 }),
            "got {error:?}"
        );
    }

    #[test]
    fn missing_position_is_refused() {
        let json = box_json().replace(
            r#""attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}"#,
            r#""attributes":{"NORMAL":1,"TEXCOORD_0":2}"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("no POSITION attribute");
        // The loader's own subset validation fires (the crate's validator
        // is deliberately not run — see `load`).
        assert!(
            matches!(error, ModelLoadError::MissingPositions { index: 0 }),
            "got {error:?}"
        );
    }

    #[test]
    fn sparse_position_accessor_is_refused() {
        let mut bin = box_bin();
        push_u16s(&mut bin, &[0]);
        push_f32s(&mut bin, &[0.0, 0.0, 0.0]);
        let json = box_json()
            .replace(
                r#"{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]}"#,
                r#"{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5],"sparse":{"count":1,"indices":{"bufferView":4,"componentType":5123},"values":{"bufferView":5}}}"#,
            )
            .replace(
                r#"{"buffer":0,"byteOffset":256,"byteLength":72}"#,
                r#"{"buffer":0,"byteOffset":256,"byteLength":72},{"buffer":0,"byteOffset":328,"byteLength":4},{"buffer":0,"byteOffset":332,"byteLength":12}"#,
            )
            .replace(r#""buffers":[{"byteLength":328}]"#, r#""buffers":[{"byteLength":344}]"#);
        let bytes = assemble_glb(&json, Some(&bin));
        let error = load(&bytes).expect_err("sparse accessor");
        assert!(
            matches!(
                error,
                ModelLoadError::UnsupportedAccessor {
                    index: 0,
                    semantic: "POSITION",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn wrong_component_type_is_refused() {
        // POSITION as VEC3 of UNSIGNED_SHORT: outside the subset.
        let json = box_json().replace(
            r#"{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]}"#,
            r#"{"bufferView":0,"componentType":5123,"count":8,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]}"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("U16 positions");
        assert!(
            matches!(
                error,
                ModelLoadError::UnsupportedAccessor {
                    index: 0,
                    semantic: "POSITION",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn indices_beyond_positions_are_refused() {
        // The valid box with every index replaced by vertex 99.
        let mut bin = box_bin();
        for at in (256..328).step_by(2) {
            bin[at..at + 2].copy_from_slice(&99u16.to_le_bytes());
        }
        let bytes = assemble_glb(&box_json(), Some(&bin));
        let error = load(&bytes).expect_err("index past the vertex count");
        assert!(
            matches!(
                error,
                ModelLoadError::InvalidIndices {
                    index: 99,
                    count: 8
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn no_default_scene_is_refused() {
        let json = box_json().replace(r#""scene":0,"#, "");
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("no scene");
        assert!(
            matches!(error, ModelLoadError::NoDefaultScene),
            "got {error:?}"
        );
    }

    #[test]
    fn a_scene_without_meshes_is_refused() {
        let json = box_json().replace(r#""nodes":[{"mesh":0}]"#, r#""nodes":[{}]"#);
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("nothing drawable");
        assert!(
            matches!(error, ModelLoadError::NoDrawableGeometry),
            "got {error:?}"
        );
    }

    #[test]
    fn cyclic_node_hierarchy_is_refused() {
        let json = box_json().replace(
            r#""nodes":[{"mesh":0}]"#,
            r#""nodes":[{"mesh":0,"children":[1]},{"children":[0]}]"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let error = load(&bytes).expect_err("cyclic hierarchy");
        assert!(
            matches!(error, ModelLoadError::InvalidNodeHierarchy { .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn absent_team_color_resolves_to_none() {
        let json = box_json().replace(r#""name":"team_color""#, r#""name":"body""#);
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let model = load(&bytes).expect("a model without team_color still loads");
        assert_eq!(model.team_color, None);
        assert_eq!(model.team_color_materials, 0);
        assert!(!model.materials[0].team_color);
    }

    #[test]
    fn multiple_team_color_materials_resolve_to_the_first() {
        let json = box_json().replace(
            r#""materials":[{"name":"team_color","pbrMetallicRoughness":{"baseColorFactor":[1.0,1.0,1.0,1.0]}}]"#,
            r#""materials":[{"name":"team_color","pbrMetallicRoughness":{"baseColorFactor":[1.0,1.0,1.0,1.0]}},{"name":"team_color","pbrMetallicRoughness":{"baseColorFactor":[0.8,0.8,0.8,1.0]}}]"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let model = load(&bytes).expect("multiple team_color materials still load");
        assert_eq!(model.team_color, Some(0), "the first one wins");
        assert_eq!(
            model.team_color_materials, 2,
            "the count tells the caller to log one note"
        );
        assert!(model.materials[0].team_color);
        assert!(model.materials[1].team_color);
    }

    #[test]
    fn node_translation_is_baked_into_positions() {
        let json = box_json().replace(
            r#""nodes":[{"mesh":0}]"#,
            r#""nodes":[{"translation":[1.0,0.0,0.0],"children":[1]},{"mesh":0}]"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let model = load(&bytes).expect("the translated hierarchy loads");
        let positions = &model.primitives[0].positions;
        assert_eq!(positions.len(), 8);
        assert!((positions[0][0] - 0.5).abs() < 1e-6, "x shifted by +1");
        assert!((positions[0][1] - -0.5).abs() < 1e-6, "y unchanged");
        assert!((positions[0][2] - -0.5).abs() < 1e-6, "z unchanged");
    }

    #[test]
    fn node_scale_is_baked_into_positions_and_normals_stay_unit() {
        let json = box_json().replace(
            r#""nodes":[{"mesh":0}]"#,
            r#""nodes":[{"scale":[2.0,2.0,2.0],"children":[1]},{"mesh":0}]"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let model = load(&bytes).expect("the scaled hierarchy loads");
        let primitive = &model.primitives[0];
        assert!((primitive.positions[0][0] - -1.0).abs() < 1e-6, "x doubled");
        assert!((primitive.positions[7][2] - 1.0).abs() < 1e-6, "z doubled");
        let normals = primitive.normals.as_ref().expect("normals present");
        assert!(
            (normals[0][1] - 1.0).abs() < 1e-6,
            "normals stay unit-length after scale"
        );
    }

    #[test]
    fn node_matrix_is_baked_into_positions() {
        // A column-major translation-by-x matrix on a parent (the `matrix`
        // form must behave exactly like the TRS form).
        let json = box_json().replace(
            r#""nodes":[{"mesh":0}]"#,
            r#""nodes":[{"matrix":[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0,0.0,0.0,1.0,0.0,1.0,0.0,0.0,1.0],"children":[1]},{"mesh":0}]"#,
        );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let model = load(&bytes).expect("the matrix hierarchy loads");
        let positions = &model.primitives[0].positions;
        assert!((positions[0][0] - 0.5).abs() < 1e-6, "x shifted by +1");
    }

    #[test]
    fn animations_skins_and_morph_targets_are_counted_as_ignored() {
        let json = box_json()
            .replace(r#""nodes":[{"mesh":0}]"#, r#""nodes":[{"mesh":0,"skin":0}]"#)
            .replace(
                r#""meshes":["#,
                r#""animations":[{"channels":[],"samplers":[]}],"skins":[{"joints":[0]}],"meshes":["#,
            )
            .replace(
                r#""material":0}]}]"#,
                r#""material":0,"targets":[{"POSITION":0}]}]}]"#,
            );
        let bytes = assemble_glb(&json, Some(&box_bin()));
        let model = load(&bytes).expect("rigged files load as static geometry");
        assert_eq!(
            model.ignored_animation_data, 3,
            "one animation + one skin + one morph-target set"
        );
        assert_eq!(model.primitives.len(), 1, "the geometry is still there");
    }

    /// The never-panics proof, part one: 10,000 pseudo-random byte slices
    /// through the loader. Deterministic (fx's PCG32, fixed seed).
    #[test]
    fn fuzz_random_slices_never_panic() {
        let mut rng = pandemonium_fx::Rng::seeded(0xA017);
        let fixture = valid_box();
        for _ in 0..10_000 {
            let len = rng.bounded((fixture.len() * 2) as u32) as usize;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                bytes.push(rng.next_u32() as u8);
            }
            // Any outcome is fine; a panic is the only failure.
            let _ = load(&bytes);
        }
    }

    /// The never-panics proof, part two: targeted mutations of the valid
    /// box — every truncation, every single-byte overwrite, and mangled
    /// magics. ADR-0101 §5's crash-freedom under any byte sequence.
    #[test]
    fn fuzz_mutations_never_panic() {
        let fixture = valid_box();
        // Every prefix.
        for end in 0..fixture.len() {
            let _ = load(&fixture[..end]);
        }
        // Every single-byte overwrite with a few hostile values.
        for at in 0..fixture.len() {
            for value in [0x00u8, 0xFF, 0x20, 0x67] {
                let mut mutated = fixture.clone();
                mutated[at] = value;
                let _ = load(&mutated);
            }
        }
        // Mangled magics and degenerate short inputs.
        for magic in [b"glTX" as &[u8], b"GLTF", b"\0\0\0\0", b"GLtf"] {
            let mut mutated = fixture.clone();
            mutated[..4].copy_from_slice(magic);
            let _ = load(&mutated);
        }
        let empty: [u8; 0] = [];
        let _ = load(&empty);
        let _ = load(b"g");
        let _ = load(b"glT");
    }
}
