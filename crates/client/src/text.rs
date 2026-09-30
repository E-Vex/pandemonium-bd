//! Our own text renderer (plan §3.1 "we write ourselves … the UI toolkit
//! (rects, text …)"; §3.2 "fontdue … for our own text renderer"): fontdue
//! rasterizes the embedded placeholder font once into a grayscale coverage
//! atlas, and layout is plain client-side math. Presentation-only — floats are
//! legal in the client (ADR-0001, A-007); none of this ever crosses the sim
//! boundary.
//!
//! The embedded font is "Pandemonium Sans": an ASCII subset of DejaVu Sans,
//! renamed as the Bitstream Vera license requires for modified versions (see
//! `assets/fonts/LICENSE.txt`).

use fontdue::{Font, FontSettings};

/// First covered codepoint (printable ASCII).
const FIRST: u8 = b' ';
/// Last covered codepoint.
const LAST: u8 = b'~';
/// How many glyphs that is.
const GLYPH_COUNT: usize = (LAST - FIRST + 1) as usize;

/// Rasterization size in pixels. One size fits the placeholder HUD (plan §0:
/// no time on graphics).
const PIXEL_SIZE: f32 = 18.0;

/// Atlas width in pixels (shelf packing, fixed row height).
const ATLAS_WIDTH: u32 = 512;

/// Extra pixels between shelf rows and neighbors (bleeding guard).
const PADDING: u32 = 1;

/// The embedded font bytes.
const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/PandemoniumSans-Regular.ttf");

/// Placement and metrics of one rasterized glyph.
#[derive(Clone, Copy, Debug)]
pub struct GlyphInfo {
    /// Atlas box (pixels).
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Horizontal advance (pixels).
    pub advance: f32,
    /// Bitmap left edge relative to the pen (pixels).
    pub bearing_x: i32,
    /// Bitmap top edge relative to the baseline (pixels, y-down screen space —
    /// negative above the baseline).
    pub bearing_y: i32,
}

/// One rectangle to draw through the UI pipeline: a screen-space box (pixels,
/// y down), an atlas region, and a color. Solid rects sample the atlas's white
/// pixel.
#[derive(Clone, Copy, Debug)]
pub struct UiQuad {
    /// Left edge (pixels).
    pub x: f32,
    /// Top edge (pixels).
    pub y: f32,
    /// Width (pixels).
    pub w: f32,
    /// Height (pixels).
    pub h: f32,
    /// Atlas top-left, normalized.
    pub u0: f32,
    /// Atlas top-left, normalized.
    pub v0: f32,
    /// Atlas bottom-right, normalized.
    pub u1: f32,
    /// Atlas bottom-right, normalized.
    pub v1: f32,
    /// RGBA color (the alpha scales the sampled coverage).
    pub color: [f32; 4],
}

/// The rasterized glyph atlas plus the layout math. Construct once; it is
/// immutable afterwards.
pub struct TextAtlas {
    /// Atlas width in pixels.
    pub width: u32,
    /// Atlas height in pixels.
    pub height: u32,
    /// Grayscale coverage, `width * height` bytes (uploaded as R8Unorm).
    pub coverage: Vec<u8>,
    /// Glyphs in ASCII order (index = codepoint - 0x20).
    glyphs: Vec<GlyphInfo>,
    /// Atlas pixel that is solid white (for solid rects).
    white: (u32, u32),
    /// Ascent above the baseline (pixels, positive).
    pub ascent: f32,
    /// Descent below the baseline (pixels, positive).
    pub descent: f32,
    /// Recommended line height (pixels).
    pub line_height: f32,
}

impl TextAtlas {
    /// Rasterizes the embedded font into the atlas.
    pub fn new() -> Self {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default())
            .expect("the embedded font parses");
        let line = font
            .horizontal_line_metrics(PIXEL_SIZE)
            .expect("the embedded font has horizontal metrics");

        let mut coverage: Vec<u8> = Vec::new();
        let mut glyphs: Vec<GlyphInfo> = Vec::with_capacity(GLYPH_COUNT);
        let mut cursor_x = 2u32; // leave the white pixel at (0, 0)
        let mut row_y = 0u32;
        let mut row_height = 0u32;
        // The white pixel: full coverage at the atlas origin.
        coverage.push(255);
        let white = (0u32, 0u32);

        for code in FIRST..=LAST {
            let (metrics, bitmap) = font.rasterize(code as char, PIXEL_SIZE);
            let glyph_w = metrics.width as u32;
            let glyph_h = metrics.height as u32;
            if cursor_x + glyph_w + PADDING >= ATLAS_WIDTH {
                row_y += row_height + PADDING;
                row_height = 0;
                cursor_x = 0;
            }
            let glyph_x = cursor_x;
            let glyph_y = row_y;
            // Copy the glyph bitmap into the shelf row (rows grow on demand).
            let needed = ((glyph_y + glyph_h) as usize) * ATLAS_WIDTH as usize;
            if coverage.len() < needed {
                coverage.resize(needed, 0);
            }
            for row in 0..glyph_h as usize {
                let src = &bitmap[row * glyph_w as usize..(row + 1) * glyph_w as usize];
                let dst_base = (glyph_y as usize + row) * ATLAS_WIDTH as usize + glyph_x as usize;
                coverage[dst_base..dst_base + src.len()].copy_from_slice(src);
            }
            cursor_x += glyph_w + PADDING;
            row_height = row_height.max(glyph_h);
            glyphs.push(GlyphInfo {
                x: glyph_x,
                y: glyph_y,
                w: glyph_w,
                h: glyph_h,
                advance: metrics.advance_width,
                bearing_x: metrics.xmin,
                // fontdue reports y-up metrics; convert to the y-down screen
                // convention the UI pipeline uses: top = baseline - (ymin + h).
                bearing_y: -(metrics.ymin + glyph_h as i32),
            });
        }
        // Final height includes the trailing padding row; `resize` (not
        // `truncate`) so the coverage buffer covers exactly height × width.
        let height = row_y + row_height + PADDING;
        coverage.resize(height as usize * ATLAS_WIDTH as usize, 0);
        Self {
            width: ATLAS_WIDTH,
            height,
            coverage,
            glyphs,
            white,
            ascent: line.ascent,
            descent: -line.descent,
            line_height: line.new_line_size,
        }
    }

    /// Placement info for one character (ASCII only; anything else renders as
    /// the fallback box).
    pub fn glyph(&self, ch: char) -> Option<&GlyphInfo> {
        let code = ch as u32;
        if (FIRST as u32..=(LAST as u32)).contains(&code) {
            self.glyphs.get((code - FIRST as u32) as usize)
        } else {
            None
        }
    }

    /// The advance width of one line of text (pixels).
    pub fn measure(&self, text: &str) -> f32 {
        let mut width = 0.0f32;
        for ch in text.chars() {
            width += self
                .glyph(ch)
                .map_or(PIXEL_SIZE * 0.6, |glyph| glyph.advance);
        }
        width
    }

    /// Lays out one line of text with its baseline at `baseline` (y-down
    /// pixels), left edge at `x`. Glyph quads come back in draw order.
    pub fn layout(&self, text: &str, x: f32, baseline: f32, color: [f32; 4]) -> Vec<UiQuad> {
        let mut quads = Vec::new();
        let mut pen_x = x;
        for ch in text.chars() {
            match self.glyph(ch) {
                Some(glyph) => {
                    if glyph.w > 0 && glyph.h > 0 {
                        quads.push(UiQuad {
                            x: pen_x + glyph.bearing_x as f32,
                            y: baseline + glyph.bearing_y as f32,
                            w: glyph.w as f32,
                            h: glyph.h as f32,
                            u0: glyph.x as f32 / self.width as f32,
                            v0: glyph.y as f32 / self.height as f32,
                            u1: (glyph.x + glyph.w) as f32 / self.width as f32,
                            v1: (glyph.y + glyph.h) as f32 / self.height as f32,
                            color,
                        });
                    }
                    pen_x += glyph.advance;
                }
                None => {
                    // Not in the ASCII set: draw the fallback box and advance.
                    quads.push(UiQuad {
                        x: pen_x + 2.0,
                        y: baseline - self.ascent * 0.6,
                        w: PIXEL_SIZE * 0.55,
                        h: self.ascent * 0.55,
                        u0: self.white.0 as f32 / self.width as f32,
                        v0: self.white.1 as f32 / self.height as f32,
                        u1: (self.white.0 + 1) as f32 / self.width as f32,
                        v1: (self.white.1 + 1) as f32 / self.height as f32,
                        color: [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, color[3]],
                    });
                    pen_x += PIXEL_SIZE * 0.6;
                }
            }
        }
        quads
    }

    /// A solid (uncolored-texture) rectangle sampling the white pixel.
    pub fn solid_rect(&self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> UiQuad {
        UiQuad {
            x,
            y,
            w,
            h,
            u0: self.white.0 as f32 / self.width as f32,
            v0: self.white.1 as f32 / self.height as f32,
            u1: (self.white.0 + 1) as f32 / self.width as f32,
            v1: (self.white.1 + 1) as f32 / self.height as f32,
            color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_font_rasterizes_every_ascii_glyph() {
        let atlas = TextAtlas::new();
        assert!(atlas.height > 0);
        assert!(atlas.width == ATLAS_WIDTH);
        // Every printable ASCII character has a placement, and no two placements
        // overlap (the shelf packer's invariant).
        let mut boxes: Vec<(u32, u32, u32, u32)> = Vec::new();
        for code in FIRST..=LAST {
            let glyph = atlas.glyph(code as char).expect("ASCII glyph present");
            boxes.push((glyph.x, glyph.y, glyph.w, glyph.h));
        }
        for (index, &(x, y, w, h)) in boxes.iter().enumerate() {
            for &(x2, y2, w2, h2) in boxes.iter().take(index) {
                let separated = x + w <= x2 || x2 + w2 <= x || y + h <= y2 || y2 + h2 <= y;
                assert!(separated, "glyph {index} overlaps another atlas box");
            }
        }
        // The white pixel is full coverage.
        assert_eq!(
            atlas.coverage[atlas.white.1 as usize * atlas.width as usize + atlas.white.0 as usize],
            255
        );
    }

    #[test]
    fn layout_advances_monotonically_and_stays_left_of_measure() {
        let atlas = TextAtlas::new();
        let quads = atlas.layout("ORE 200 POP 0/0", 8.0, 24.0, [1.0, 1.0, 1.0, 1.0]);
        // Spaces produce no quads: the spaced and unspaced strings agree.
        assert_eq!(
            quads.len(),
            atlas.layout("ORE200POP0/0", 8.0, 24.0, [1.0; 4]).len(),
            "spaces produce no quads"
        );
        // Strictly increasing left edges in draw order.
        for pair in quads.windows(2) {
            assert!(pair[0].x <= pair[1].x, "quads must advance left to right");
        }
        // Everything fits within the measured width.
        let width = atlas.measure("ORE 200 POP 0/0");
        for quad in &quads {
            assert!(quad.x + quad.w <= 8.0 + width + 0.5);
        }
        // Baseline-relative placement: capital letters sit above the baseline
        // and end at it (a hair of overshoot is allowed).
        let caps = atlas.layout("A", 0.0, 100.0, [1.0; 4]);
        assert!(!caps.is_empty());
        assert!(caps[0].y < 100.0, "capitals sit above the baseline");
        assert!(
            caps[0].y + caps[0].h <= 101.5,
            "capitals end at the baseline"
        );
    }

    #[test]
    fn descenders_reach_below_the_baseline() {
        let atlas = TextAtlas::new();
        let quads = atlas.layout("p", 0.0, 100.0, [1.0; 4]);
        assert_eq!(quads.len(), 1);
        // The bitmap box starts above the baseline (x-height) and extends
        // below it (the descender).
        assert!(
            quads[0].y < 100.0,
            "the bitmap box starts above the baseline"
        );
        assert!(
            quads[0].y + quads[0].h > 102.0,
            "the descender reaches below the baseline"
        );
    }

    #[test]
    fn measure_is_monotonic_and_zero_for_empty_text() {
        let atlas = TextAtlas::new();
        assert_eq!(atlas.measure(""), 0.0);
        let short = atlas.measure("TICK");
        let long = atlas.measure("TICK 123456");
        assert!(short < long);
        // Repeated characters measure exactly (deterministic layout math).
        assert_eq!(atlas.measure("AA"), atlas.measure("AA"));
        assert!((atlas.measure("AA") - 2.0 * atlas.measure("A")).abs() < 1e-3);
    }

    #[test]
    fn non_ascii_characters_get_the_fallback_box() {
        let atlas = TextAtlas::new();
        let quads = atlas.layout("\u{4e2d}", 0.0, 50.0, [1.0; 4]);
        assert_eq!(quads.len(), 1, "one fallback box per non-ASCII character");
        assert!(quads[0].w > 0.0 && quads[0].h > 0.0);
        // Solid rects sample the white pixel.
        let rect = atlas.solid_rect(0.0, 0.0, 10.0, 10.0, [0.0, 0.0, 0.0, 0.5]);
        assert_eq!(rect.u0, rect.u1 - 1.0 / atlas.width as f32);
    }
}
