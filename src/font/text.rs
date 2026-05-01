//! High level text drawing API.
//!
//! Mirrors the public surface of `crate::text` but writes to a
//! `Vec<TextVertex>` instead of `Vec<Vertex>`, so the renderer
//! can route text geometry through the SDF pipeline.
//!
//! All sizes are expressed as `pixel_size`, which is the height
//! of one em in game space units. A size of 0.04 means each em
//! takes up four percent of the half height of the viewport.
//! Concretely, on a 1080p screen 0.04 maps to roughly 22
//! screen pixels of em height, which is comfortable for HUD
//! labels and status text.
//!
//! Codepoints absent from the atlas (very rare for ASCII or
//! Cyrillic text) are silently replaced with the question mark
//! glyph; if even the question mark is missing they advance by
//! the font's average advance and produce no quad.

use crate::font::atlas;
use crate::font::stage::TextVertex;

/// Append vertex quads for `text` starting at `(x, y)`.
///
/// Coordinate convention matches the rest of the renderer:
/// `(x, y)` is the top left of the first glyph cell in game
/// space, with y growing downward. The baseline is computed
/// internally from the atlas ascent metric so callers do not
/// have to track it manually.
pub fn push_text(
    out: &mut Vec<TextVertex>,
    text: &str,
    x: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
) {
    push_text_alpha(out, text, x, y, pixel_size, color, 1.0);
}

/// Same as `push_text` but with explicit alpha.
pub fn push_text_alpha(
    out: &mut Vec<TextVertex>,
    text: &str,
    x: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
    alpha: f32,
) {
    let atlas = atlas();
    let baseline_y = y + atlas.ascent_em * pixel_size;
    let mut pen_x = x;
    let rgba = [color[0], color[1], color[2], alpha];

    for ch in text.chars() {
        let cp = ch as u32;
        let glyph = match atlas.glyphs.get(&cp) {
            Some(g) => g,
            None => match atlas.glyphs.get(&('?' as u32)) {
                Some(fallback) => fallback,
                None => continue,
            },
        };

        // Quad rectangle in screen space.
        let left = pen_x + glyph.plane_min[0] * pixel_size;
        let right = pen_x + glyph.plane_max[0] * pixel_size;
        let top = baseline_y - glyph.plane_max[1] * pixel_size;
        let bottom = baseline_y - glyph.plane_min[1] * pixel_size;

        let u0 = glyph.uv_min[0];
        let v0 = glyph.uv_min[1];
        let u1 = glyph.uv_max[0];
        let v1 = glyph.uv_max[1];

        // Two triangles forming the glyph quad. UV order is
        // chosen so the atlas's top edge maps to the screen
        // top edge; both grow downward in their respective
        // coordinate systems so the mapping is direct.
        out.push(TextVertex::new([left,  top   ], [u0, v0], rgba));
        out.push(TextVertex::new([right, top   ], [u1, v0], rgba));
        out.push(TextVertex::new([right, bottom], [u1, v1], rgba));

        out.push(TextVertex::new([left,  top   ], [u0, v0], rgba));
        out.push(TextVertex::new([right, bottom], [u1, v1], rgba));
        out.push(TextVertex::new([left,  bottom], [u0, v1], rgba));

        pen_x += glyph.advance_em * pixel_size;
    }
}

/// Centered helper. `cx` is the desired horizontal center of
/// the entire string.
pub fn push_text_centered(
    out: &mut Vec<TextVertex>,
    text: &str,
    cx: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
) {
    let w = text_width(text, pixel_size);
    push_text(out, text, cx - w * 0.5, y, pixel_size, color);
}

/// Centered helper with explicit alpha.
pub fn push_text_centered_alpha(
    out: &mut Vec<TextVertex>,
    text: &str,
    cx: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
    alpha: f32,
) {
    let w = text_width(text, pixel_size);
    push_text_alpha(out, text, cx - w * 0.5, y, pixel_size, color, alpha);
}

/// Right anchored helper. `rx` is the desired right edge.
pub fn push_text_right(
    out: &mut Vec<TextVertex>,
    text: &str,
    rx: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
) {
    let w = text_width(text, pixel_size);
    push_text(out, text, rx - w, y, pixel_size, color);
}

/// Right anchored helper with explicit alpha.
pub fn push_text_right_alpha(
    out: &mut Vec<TextVertex>,
    text: &str,
    rx: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
    alpha: f32,
) {
    let w = text_width(text, pixel_size);
    push_text_alpha(out, text, rx - w, y, pixel_size, color, alpha);
}

/// Width of `text` in game space units. Sums the horizontal
/// advance of every codepoint, multiplied by `pixel_size`.
pub fn text_width(text: &str, pixel_size: f32) -> f32 {
    let atlas = atlas();
    let mut total = 0.0f32;
    for ch in text.chars() {
        let cp = ch as u32;
        let advance = atlas.glyphs.get(&cp)
            .or_else(|| atlas.glyphs.get(&('?' as u32)))
            .map(|g| g.advance_em)
            .unwrap_or(0.0);
        total += advance;
    }
    total * pixel_size
}

/// Visual height of one line in game space units. Includes the
/// font's ascent, |descent|, and recommended line gap.
pub fn text_height(pixel_size: f32) -> f32 {
    atlas().line_height_em * pixel_size
}

/// Recommended distance between two consecutive baselines.
pub fn line_height(pixel_size: f32) -> f32 {
    atlas().line_height_em * pixel_size
}