// `texture/` (the PNG half of the client-side asset pack MVP): the CPU-side
// PNG decoder with its size guards, and the GPU-side skin upload that turns a
// decoded RGBA8 buffer into one wgpu texture + bind group. All of it is
// presentation-only (plan FD-6): a kind's texture is resolved through the
// pack's data-driven entry, never through content, and every failure —
// missing file, oversized image, malformed PNG — degrades that one kind to
// the placeholder silhouette instead of touching the simulation.
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// The largest texture dimension the renderer accepts, per side. Generous
/// for entity art (a box face is a handful of tiles on screen) while keeping
/// a decoded image — and its GPU copy — inside 16 MiB RGBA8, well under the
/// WebGPU texture limits every adapter must meet.
pub(super) const MAX_TEXTURE_DIMENSION: u32 = 2048;

/// The largest PNG file read off disk, before any decoding. Compressed
/// images can expand far beyond their file size, so the on-disk bytes are
/// bounded separately from the decoded pixel budget.
pub(super) const MAX_TEXTURE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// One decoded PNG, ready for upload: RGBA8, row-major, `width * height * 4`
/// bytes. The pixel format every skin is normalized to before upload, so
/// the GPU path handles one layout, not one per PNG color type.
#[derive(Debug)]
pub(super) struct DecodedTexture {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

impl DecodedTexture {
    /// The placeholder skin's own pixels: an opaque white 1×1. Sampling it
    /// multiplies by exactly 1.0, so placeholder draws keep the silhouette
    /// colors byte-for-byte while running the same shader and pipeline.
    pub(super) fn white_1x1() -> Self {
        Self {
            width: 1,
            height: 1,
            rgba: vec![255, 255, 255, 255],
        }
    }
}

/// Why a texture could not be turned into a skin. Every variant carries the
/// path so the operator can find the offending file in one read.
#[derive(Debug, thiserror::Error)]
pub(super) enum TextureLoadError {
    #[error("cannot read {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("file is {size} bytes but the limit is {limit} bytes")]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error(
        "image is {width}x{height} but the per-side limit is {limit}x{limit} \
         (limit: {MAX_TEXTURE_DIMENSION})"
    )]
    TooLarge {
        path: PathBuf,
        width: u32,
        height: u32,
        limit: u32,
    },
    #[error("not a valid PNG: {1}")]
    Malformed(PathBuf, String),
}

/// Decodes one PNG file into [`DecodedTexture`], refusing anything the
/// renderer would not safely upload. The guards run in bomb-proof order:
/// the file's size on disk is checked before it is read, and the header's
/// dimensions are checked before a single pixel is decoded — a tiny file
/// claiming a huge bitmap is rejected by the header pass, never by an
/// allocation.
pub(super) fn decode_png(path: &Path) -> Result<DecodedTexture, TextureLoadError> {
    let metadata =
        std::fs::metadata(path).map_err(|e| TextureLoadError::Io(path.to_path_buf(), e))?;
    if metadata.len() > MAX_TEXTURE_FILE_BYTES {
        return Err(TextureLoadError::FileTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            limit: MAX_TEXTURE_FILE_BYTES,
        });
    }
    let bytes = std::fs::read(path).map_err(|e| TextureLoadError::Io(path.to_path_buf(), e))?;

    // Header-only dimension pass: `into_dimensions` reads the PNG IHDR and
    // stops — the decode bomb never fires before the limit check below.
    let (width, height) =
        image::ImageReader::with_format(Cursor::new(&bytes[..]), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(|e| TextureLoadError::Malformed(path.to_path_buf(), e.to_string()))?;
    if width == 0 || height == 0 || width > MAX_TEXTURE_DIMENSION || height > MAX_TEXTURE_DIMENSION
    {
        return Err(TextureLoadError::TooLarge {
            path: path.to_path_buf(),
            width,
            height,
            limit: MAX_TEXTURE_DIMENSION,
        });
    }

    let decoded = image::ImageReader::with_format(Cursor::new(&bytes[..]), image::ImageFormat::Png)
        .decode()
        .map_err(|e| TextureLoadError::Malformed(path.to_path_buf(), e.to_string()))?;
    let rgba = decoded.into_rgba8();
    Ok(DecodedTexture {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

/// One uploaded entity skin: the GPU texture (kept alive explicitly — the
/// bind group references it, but the renderer owns its lifetime in the
/// open), its view, and the bind group the entity pass binds at group(1)
/// for every draw that uses this skin.
pub(super) struct EntitySkin {
    /// The skin's label (the kind id or "placeholder") for GPU debugging.
    #[allow(dead_code)] // labels are for GPU debugging; not read by the CPU
    pub(super) label: String,
    /// Held to keep the texture alive — the bind group samples it, nothing
    /// on the CPU reads it again after upload.
    #[allow(dead_code)]
    pub(super) texture: wgpu::Texture,
    /// Held to keep the view's texture alive through the bind group.
    #[allow(dead_code)]
    pub(super) view: wgpu::TextureView,
    pub(super) bind_group: wgpu::BindGroup,
}

/// Uploads a decoded texture as the entity pass's sample source and binds
/// it against the shared skin layout (sampler at 0, view at 1). sRGB
/// storage matches how PNG pixels are authored: sampling linearizes them
/// for the shader's lighting math, exactly like the minimap texture.
pub(super) fn upload_skin(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    decoded: &DecodedTexture,
    label: &str,
) -> EntitySkin {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: decoded.width.max(1),
            height: decoded.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &decoded.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(decoded.width.max(1) * 4),
            rows_per_image: Some(decoded.height.max(1)),
        },
        wgpu::Extent3d {
            width: decoded.width.max(1),
            height: decoded.height.max(1),
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
        ],
    });
    EntitySkin {
        label: label.to_string(),
        texture,
        view,
        bind_group,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes a real PNG (via the `image` encoder, the round-trip partner
    /// of the decoder under test) into a fresh temp dir.
    fn write_png(name: &str, width: u32, height: u32, pixels: &[[u8; 4]]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pandemonium-texture-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("creating temp dir");
        let path = dir.join(name);
        let mut image = image::RgbaImage::new(width, height);
        for (index, pixel) in pixels.iter().enumerate() {
            let x = (index as u32) % width;
            let y = (index as u32) / width;
            image.put_pixel(x, y, image::Rgba(*pixel));
        }
        image
            .write_to(
                &mut std::fs::File::create(&path).expect("creating png file"),
                image::ImageFormat::Png,
            )
            .expect("encoding png");
        path
    }

    fn write_bytes(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pandemonium-texture-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("creating temp dir");
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).expect("creating file");
        file.write_all(bytes).expect("writing bytes");
        path
    }

    #[test]
    fn decodes_a_valid_png_into_rgba8_pixels() {
        // A 2x2 image with four distinct pixels: red, green, half-alpha
        // blue, and opaque white. Decoding must reproduce all four bytes of
        // each pixel in reading order — the exact buffer the GPU upload
        // expects, not just the right dimensions.
        let path = write_png(
            "valid.png",
            2,
            2,
            &[
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 128],
                [255, 255, 255, 255],
            ],
        );
        let decoded = decode_png(&path).expect("a valid png decodes");
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(
            decoded.rgba,
            vec![
                255, 0, 0, 255, //
                0, 255, 0, 255, //
                0, 0, 255, 128, //
                255, 255, 255, 255,
            ]
        );
    }

    #[test]
    fn decodes_a_grayscale_png_into_the_same_rgba8_layout() {
        // A single-channel PNG exercises the color-type normalization:
        // whatever the PNG's own color type, the decoder must hand the
        // upload one RGBA8 buffer.
        let dir = std::env::temp_dir().join(format!(
            "pandemonium-texture-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("creating temp dir");
        let path = dir.join("gray.png");
        let mut gray = image::GrayImage::new(1, 1);
        gray.put_pixel(0, 0, image::Luma([7]));
        gray.write_to(
            &mut std::fs::File::create(&path).expect("creating png file"),
            image::ImageFormat::Png,
        )
        .expect("encoding png");
        let decoded = decode_png(&path).expect("a grayscale png decodes");
        assert_eq!(decoded.rgba, vec![7, 7, 7, 255]);
    }

    #[test]
    fn garbage_bytes_are_rejected_not_decoded() {
        let path = write_bytes("garbage.png", b"this is not a png at all");
        let err = decode_png(&path).expect_err("garbage must not decode");
        assert!(
            matches!(err, TextureLoadError::Malformed(..)),
            "got {err:?}"
        );
    }

    #[test]
    fn an_empty_file_is_rejected() {
        let path = write_bytes("empty.png", b"");
        let err = decode_png(&path).expect_err("an empty file must not decode");
        assert!(
            matches!(err, TextureLoadError::Malformed(..)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_missing_file_reports_an_io_error() {
        let err = decode_png(Path::new("/nonexistent/nowhere.png"))
            .expect_err("a missing file must not decode");
        assert!(matches!(err, TextureLoadError::Io(..)), "got {err:?}");
    }

    #[test]
    fn an_oversized_header_is_rejected_before_decoding() {
        // 1 pixel wide but one past the per-side limit: the header pass must
        // refuse it — the guard reads dimensions, never decoded pixels.
        let path = write_png(
            "wide.png",
            MAX_TEXTURE_DIMENSION + 1,
            1,
            &[[255, 255, 255, 255]],
        );
        let err = decode_png(&path).expect_err("an oversized image must be refused");
        assert!(
            matches!(
                &err,
                TextureLoadError::TooLarge {
                    width,
                    height: 1,
                    ..
                } if *width == MAX_TEXTURE_DIMENSION + 1
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn the_limit_boundary_is_accepted() {
        // Exactly at the limit is fine: the guard is inclusive, so a
        // 2048x1 strip uploads instead of surprising the operator.
        let path = write_png("edge.png", MAX_TEXTURE_DIMENSION, 1, &[[255, 0, 0, 255]]);
        let decoded = decode_png(&path).expect("the boundary itself decodes");
        assert_eq!(decoded.width, MAX_TEXTURE_DIMENSION);
        assert_eq!(decoded.height, 1);
    }

    #[test]
    fn the_placeholder_skin_is_one_opaque_white_pixel() {
        let white = DecodedTexture::white_1x1();
        assert_eq!(white.width, 1);
        assert_eq!(white.height, 1);
        assert_eq!(white.rgba, vec![255, 255, 255, 255]);
    }
}
// (The resolve_kind_textures plan tests that lived here moved out with the
// function: M12 phase 4's chain resolution — pack/mod.rs's
// resolve_presentation suite and render/mod.rs's model pass — owns the
// declared-vs-renderable distinction now, reading the typed model/skin
// fields per A-155 instead of planning off the v1 primary.)
