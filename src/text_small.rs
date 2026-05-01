//! Compact 4x6 bitmap font for dense editor panels.
//!
//! Hand-designed to pair with the existing 5x7 font in
//! `text.rs`. The glyph cell is 4 columns wide, 6 rows tall,
//! advance width 5, line height 7. Visually yields roughly
//! 40% more text per unit area than the 5x7 font, which is
//! what the editor inspector, graph node labels, and
//! statement list rows want so we can fit every authored
//! field on screen at once.
//!
//! Usage mirrors `text.rs`:
//!
//! * `push_small(...)` for the basic case,
//! * `push_small_alpha(...)` for fade in / fade out,
//! * `push_small_centered(...)` for layout.
//!
//! The two font modules coexist: `text.rs` stays canonical
//! for splash titles, menu bars, game HUD; `text_small.rs`
//! is used inside `editor/*`. Any caller is free to pick.

use crate::pipeline::Vertex;

/// Lit pixels per glyph cell: 4 columns, 6 rows.
pub const GLYPH_COLS: usize = 4;
pub const GLYPH_ROWS: usize = 6;

/// Horizontal advance between glyph cells, in font pixels
/// (4 lit + 1 gap).
pub const ADVANCE: f32 = 5.0;

/// Vertical advance between lines, in font pixels
/// (6 lit + 1 gap).
pub const LINE_HEIGHT: f32 = 7.0;

struct Glyph { rows: [u8; 6] }

/// Return the bitmask for one printable character.
///
/// Each row's four bits are stored in the low nibble of a
/// u8, MSB is the leftmost pixel. Rows go top to bottom.
/// Lowercase automatically falls through to uppercase.
fn glyph(c: char) -> Option<Glyph> {
    let c = c.to_ascii_uppercase();
    Some(Glyph { rows: match c {
        ' ' => [0, 0, 0, 0, 0, 0],
        'A' => [0b0110, 0b1001, 0b1001, 0b1111, 0b1001, 0b1001],
        'B' => [0b1110, 0b1001, 0b1110, 0b1001, 0b1001, 0b1110],
        'C' => [0b0111, 0b1000, 0b1000, 0b1000, 0b1000, 0b0111],
        'D' => [0b1110, 0b1001, 0b1001, 0b1001, 0b1001, 0b1110],
        'E' => [0b1111, 0b1000, 0b1110, 0b1000, 0b1000, 0b1111],
        'F' => [0b1111, 0b1000, 0b1110, 0b1000, 0b1000, 0b1000],
        'G' => [0b0111, 0b1000, 0b1000, 0b1011, 0b1001, 0b0111],
        'H' => [0b1001, 0b1001, 0b1111, 0b1001, 0b1001, 0b1001],
        'I' => [0b1110, 0b0100, 0b0100, 0b0100, 0b0100, 0b1110],
        'J' => [0b0011, 0b0001, 0b0001, 0b0001, 0b1001, 0b0110],
        'K' => [0b1001, 0b1010, 0b1100, 0b1100, 0b1010, 0b1001],
        'L' => [0b1000, 0b1000, 0b1000, 0b1000, 0b1000, 0b1111],
        'M' => [0b1001, 0b1111, 0b1111, 0b1001, 0b1001, 0b1001],
        'N' => [0b1001, 0b1101, 0b1111, 0b1011, 0b1001, 0b1001],
        'O' => [0b0110, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110],
        'P' => [0b1110, 0b1001, 0b1001, 0b1110, 0b1000, 0b1000],
        'Q' => [0b0110, 0b1001, 0b1001, 0b1011, 0b1011, 0b0111],
        'R' => [0b1110, 0b1001, 0b1001, 0b1110, 0b1010, 0b1001],
        'S' => [0b0111, 0b1000, 0b0110, 0b0001, 0b0001, 0b1110],
        'T' => [0b1111, 0b0100, 0b0100, 0b0100, 0b0100, 0b0100],
        'U' => [0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110],
        'V' => [0b1001, 0b1001, 0b1001, 0b0110, 0b0110, 0b0100],
        'W' => [0b1001, 0b1001, 0b1001, 0b1111, 0b1111, 0b1001],
        'X' => [0b1001, 0b1001, 0b0110, 0b0110, 0b1001, 0b1001],
        'Y' => [0b1001, 0b1001, 0b0110, 0b0100, 0b0100, 0b0100],
        'Z' => [0b1111, 0b0001, 0b0010, 0b0100, 0b1000, 0b1111],
        '0' => [0b0110, 0b1001, 0b1011, 0b1101, 0b1001, 0b0110],
        '1' => [0b0100, 0b1100, 0b0100, 0b0100, 0b0100, 0b1110],
        '2' => [0b0110, 0b1001, 0b0001, 0b0110, 0b1000, 0b1111],
        '3' => [0b1110, 0b0001, 0b0110, 0b0001, 0b0001, 0b1110],
        '4' => [0b0010, 0b0110, 0b1010, 0b1111, 0b0010, 0b0010],
        '5' => [0b1111, 0b1000, 0b1110, 0b0001, 0b1001, 0b0110],
        '6' => [0b0110, 0b1000, 0b1110, 0b1001, 0b1001, 0b0110],
        '7' => [0b1111, 0b0001, 0b0010, 0b0100, 0b0100, 0b0100],
        '8' => [0b0110, 0b1001, 0b0110, 0b1001, 0b1001, 0b0110],
        '9' => [0b0110, 0b1001, 0b0111, 0b0001, 0b0001, 0b0110],
        '.' => [0, 0, 0, 0, 0, 0b0100],
        ',' => [0, 0, 0, 0, 0b0100, 0b1000],
        ':' => [0, 0b0100, 0, 0, 0b0100, 0],
        ';' => [0, 0b0100, 0, 0, 0b0100, 0b1000],
        '-' => [0, 0, 0, 0b1110, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0b1111],
        '+' => [0, 0b0100, 0b0100, 0b1110, 0b0100, 0],
        '=' => [0, 0, 0b1110, 0, 0b1110, 0],
        '/' => [0b0001, 0b0001, 0b0010, 0b0100, 0b1000, 0b1000],
        '\\'=> [0b1000, 0b1000, 0b0100, 0b0010, 0b0001, 0b0001],
        '!' => [0b0100, 0b0100, 0b0100, 0b0100, 0, 0b0100],
        '?' => [0b0110, 0b1001, 0b0010, 0b0100, 0, 0b0100],
        '(' => [0b0010, 0b0100, 0b1000, 0b1000, 0b0100, 0b0010],
        ')' => [0b1000, 0b0100, 0b0010, 0b0010, 0b0100, 0b1000],
        '[' => [0b1110, 0b1000, 0b1000, 0b1000, 0b1000, 0b1110],
        ']' => [0b1110, 0b0010, 0b0010, 0b0010, 0b0010, 0b1110],
        '<' => [0b0010, 0b0100, 0b1000, 0b0100, 0b0010, 0],
        '>' => [0b1000, 0b0100, 0b0010, 0b0100, 0b1000, 0],
        '\''=> [0b0100, 0b0100, 0, 0, 0, 0],
        '"' => [0b1010, 0b1010, 0, 0, 0, 0],
        '^' => [0b0100, 0b1010, 0, 0, 0, 0],
        '|' => [0b0100, 0b0100, 0b0100, 0b0100, 0b0100, 0b0100],
        '#' => [0b1010, 0b1111, 0b1010, 0b1111, 0b1010, 0],
        '*' => [0, 0b0101, 0b0010, 0b0101, 0, 0],
        '%' => [0b1001, 0b0001, 0b0010, 0b0100, 0b1000, 0b1001],
        '@' => [0b0110, 0b1001, 0b1011, 0b1010, 0b1000, 0b0111],
        '~' => [0, 0b0101, 0b1010, 0, 0, 0],
        '&' => [0b0100, 0b1010, 0b0100, 0b1010, 0b1011, 0b0101],
        _   => return None,
    }})
}

/// Width of `text` in game space units at the given pixel size.
pub fn text_width(text: &str, pixel_size: f32) -> f32 {
    let n = text.chars().count() as f32;
    if n == 0.0 { 0.0 } else { (n * ADVANCE - 1.0) * pixel_size }
}

/// Height of one line in game space units at the given pixel
/// size.
pub fn text_height(pixel_size: f32) -> f32 {
    (GLYPH_ROWS as f32) * pixel_size
}

/// Emit triangles for `text`. `(x, y)` is the top left of the
/// first glyph cell; y grows downward to match the rest of the
/// engine's coordinate system.
pub fn push_small(
    out: &mut Vec<Vertex>, text: &str,
    x: f32, y: f32, pixel_size: f32, color: [f32; 3],
) {
    push_small_alpha(out, text, x, y, pixel_size, color, 1.0);
}

/// Translucent variant used for faded hints and disabled rows.
pub fn push_small_alpha(
    out: &mut Vec<Vertex>, text: &str,
    x: f32, y: f32, pixel_size: f32, color: [f32; 3], alpha: f32,
) {
    let mut cx = x;
    for ch in text.chars() {
        if let Some(g) = glyph(ch) {
            for (ry, row) in g.rows.iter().enumerate() {
                for bx in 0..GLYPH_COLS {
                    if (row >> (GLYPH_COLS - 1 - bx)) & 1 == 0 { continue; }
                    let px = cx + bx as f32 * pixel_size;
                    let py = y  + ry as f32 * pixel_size;
                    let qx = px + pixel_size;
                    let qy = py + pixel_size;
                    out.push(Vertex::rgba([px, py], color, alpha));
                    out.push(Vertex::rgba([qx, py], color, alpha));
                    out.push(Vertex::rgba([qx, qy], color, alpha));
                    out.push(Vertex::rgba([px, py], color, alpha));
                    out.push(Vertex::rgba([qx, qy], color, alpha));
                    out.push(Vertex::rgba([px, qy], color, alpha));
                }
            }
        }
        cx += ADVANCE * pixel_size;
    }
}

/// Render centered around `(cx, y)`. `y` is still the top of
/// the first glyph row, caller typically subtracts half the
/// line height to align vertically.
pub fn push_small_centered(
    out: &mut Vec<Vertex>, text: &str,
    cx: f32, y: f32, pixel_size: f32, color: [f32; 3],
) {
    let w = text_width(text, pixel_size);
    push_small(out, text, cx - w * 0.5, y, pixel_size, color);
}

/// Right aligned variant. `rx` is the right edge of the text.
pub fn push_small_right(
    out: &mut Vec<Vertex>, text: &str,
    rx: f32, y: f32, pixel_size: f32, color: [f32; 3],
) {
    let w = text_width(text, pixel_size);
    push_small(out, text, rx - w, y, pixel_size, color);
}