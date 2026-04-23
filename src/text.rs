//! Tiny 5x7 bitmap font, rendered as triangle quads through the same
//! pipeline used by the rest of the game. Zero dependencies.
//!
//! All public draw calls take RGB color plus an optional alpha. The
//! default alpha is 1.0 (opaque). Passing a value below 1.0 makes the
//! glyph quads translucent through the pipeline's alpha blending
//! stage; this is used for fade transitions.

use crate::pipeline::Vertex;

pub const GLYPH_COLS: usize = 5;
pub const GLYPH_ROWS: usize = 7;
/// Horizontal advance between character cells, in font pixels
/// (5 lit + 1 gap).
pub const ADVANCE: f32 = 6.0;
/// Vertical advance between lines, in font pixels (7 lit + 1 gap).
pub const LINE_HEIGHT: f32 = 8.0;

struct Glyph { rows: [u8; 7] }

fn glyph(c: char) -> Option<Glyph> {
    let c = c.to_ascii_uppercase();
    Some(Glyph { rows: match c {
        ' ' => [0,0,0,0,0,0,0],
        'A' => [0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],
        'B' => [0b11110,0b10001,0b10001,0b11110,0b10001,0b10001,0b11110],
        'C' => [0b01111,0b10000,0b10000,0b10000,0b10000,0b10000,0b01111],
        'D' => [0b11110,0b10001,0b10001,0b10001,0b10001,0b10001,0b11110],
        'E' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b11111],
        'F' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000],
        'G' => [0b01111,0b10000,0b10000,0b10111,0b10001,0b10001,0b01110],
        'H' => [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],
        'I' => [0b01110,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110],
        'J' => [0b00001,0b00001,0b00001,0b00001,0b10001,0b10001,0b01110],
        'K' => [0b10001,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001],
        'L' => [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111],
        'M' => [0b10001,0b11011,0b10101,0b10001,0b10001,0b10001,0b10001],
        'N' => [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001],
        'O' => [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'P' => [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000],
        'Q' => [0b01110,0b10001,0b10001,0b10001,0b10101,0b10010,0b01101],
        'R' => [0b11110,0b10001,0b10001,0b11110,0b10100,0b10010,0b10001],
        'S' => [0b01111,0b10000,0b10000,0b01110,0b00001,0b00001,0b11110],
        'T' => [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100],
        'U' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'V' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100],
        'W' => [0b10001,0b10001,0b10001,0b10001,0b10101,0b11011,0b10001],
        'X' => [0b10001,0b10001,0b01010,0b00100,0b01010,0b10001,0b10001],
        'Y' => [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100],
        'Z' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b10000,0b11111],
        '0' => [0b01110,0b10001,0b10011,0b10101,0b11001,0b10001,0b01110],
        '1' => [0b00100,0b01100,0b00100,0b00100,0b00100,0b00100,0b01110],
        '2' => [0b01110,0b10001,0b00001,0b00010,0b00100,0b01000,0b11111],
        '3' => [0b11110,0b00001,0b00001,0b01110,0b00001,0b00001,0b11110],
        '4' => [0b00010,0b00110,0b01010,0b10010,0b11111,0b00010,0b00010],
        '5' => [0b11111,0b10000,0b11110,0b00001,0b00001,0b10001,0b01110],
        '6' => [0b00110,0b01000,0b10000,0b11110,0b10001,0b10001,0b01110],
        '7' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000],
        '8' => [0b01110,0b10001,0b10001,0b01110,0b10001,0b10001,0b01110],
        '9' => [0b01110,0b10001,0b10001,0b01111,0b00001,0b00010,0b01100],
        '.' => [0,0,0,0,0,0b00110,0b00110],
        ',' => [0,0,0,0,0b00110,0b00110,0b00100],
        ':' => [0,0b00110,0b00110,0,0b00110,0b00110,0],
        ';' => [0,0b00110,0b00110,0,0b00110,0b00110,0b00100],
        '-' => [0,0,0,0b01110,0,0,0],
        '_' => [0,0,0,0,0,0,0b11111],
        '+' => [0,0b00100,0b00100,0b11111,0b00100,0b00100,0],
        '=' => [0,0,0b11111,0,0b11111,0,0],
        '/' => [0b00001,0b00010,0b00010,0b00100,0b01000,0b01000,0b10000],
        '\\'=> [0b10000,0b01000,0b01000,0b00100,0b00010,0b00010,0b00001],
        '!' => [0b00100,0b00100,0b00100,0b00100,0b00100,0,0b00100],
        '?' => [0b01110,0b10001,0b00001,0b00010,0b00100,0,0b00100],
        '(' => [0b00010,0b00100,0b01000,0b01000,0b01000,0b00100,0b00010],
        ')' => [0b01000,0b00100,0b00010,0b00010,0b00010,0b00100,0b01000],
        '[' => [0b01110,0b01000,0b01000,0b01000,0b01000,0b01000,0b01110],
        ']' => [0b01110,0b00010,0b00010,0b00010,0b00010,0b00010,0b01110],
        '<' => [0b00010,0b00100,0b01000,0b10000,0b01000,0b00100,0b00010],
        '>' => [0b01000,0b00100,0b00010,0b00001,0b00010,0b00100,0b01000],
        '\''=> [0b00100,0b00100,0,0,0,0,0],
        '"' => [0b01010,0b01010,0,0,0,0,0],
        '#' => [0b01010,0b11111,0b01010,0b01010,0b01010,0b11111,0b01010],
        '*' => [0,0b10101,0b01110,0b11111,0b01110,0b10101,0],
        '%' => [0b11001,0b11010,0b00100,0b01000,0b10000,0b01011,0b10011],
        _   => return None,
    }})
}

/// Width of `text` in game-space units at the given pixel size.
pub fn text_width(text: &str, pixel_size: f32) -> f32 {
    let n = text.chars().count() as f32;
    if n == 0.0 { 0.0 } else { (n * ADVANCE - 1.0) * pixel_size }
}

/// Height of a single line at the given pixel size.
pub fn text_height(pixel_size: f32) -> f32 {
    (GLYPH_ROWS as f32) * pixel_size
}

/// Emit triangles for `text`. `(x, y)` is the top-left of the first
/// character cell; y grows downward.
pub fn push_text(
    out: &mut Vec<Vertex>,
    text: &str,
    x: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
) {
    push_text_alpha(out, text, x, y, pixel_size, color, 1.0);
}

/// Same as [`push_text`] but with an explicit alpha. Used by
/// splash fade-ins and the darkening overlay.
pub fn push_text_alpha(
    out: &mut Vec<Vertex>,
    text: &str,
    x: f32, y: f32,
    pixel_size: f32,
    color: [f32; 3],
    alpha: f32,
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

pub fn push_text_centered(
    out: &mut Vec<Vertex>, text: &str,
    cx: f32, y: f32, pixel_size: f32, color: [f32; 3],
) {
    let w = text_width(text, pixel_size);
    push_text(out, text, cx - w * 0.5, y, pixel_size, color);
}

pub fn push_text_centered_alpha(
    out: &mut Vec<Vertex>, text: &str,
    cx: f32, y: f32, pixel_size: f32, color: [f32; 3], alpha: f32,
) {
    let w = text_width(text, pixel_size);
    push_text_alpha(out, text, cx - w * 0.5, y, pixel_size, color, alpha);
}

pub fn push_text_right(
    out: &mut Vec<Vertex>, text: &str,
    rx: f32, y: f32, pixel_size: f32, color: [f32; 3],
) {
    let w = text_width(text, pixel_size);
    push_text(out, text, rx - w, y, pixel_size, color);
}

/// Word-wrap `text` inside a box of `max_w` game-space units wide,
/// starting at top-left `(x, y)`.
pub fn push_text_wrapped(
    out: &mut Vec<Vertex>, text: &str,
    x: f32, y: f32, max_w: f32,
    pixel_size: f32, color: [f32; 3],
) {
    let line_h = LINE_HEIGHT * pixel_size;
    let mut cur = String::new();
    let mut cur_w = 0.0f32;
    let mut cy = 0.0f32;
    let mut first = true;
    for word in text.split_whitespace() {
        let piece = if first { word.to_string() } else { format!(" {}", word) };
        let w = text_width(&piece, pixel_size);
        if cur_w + w > max_w && !first {
            push_text(out, &cur, x, y + cy, pixel_size, color);
            cur.clear();
            cur_w = 0.0;
            cy += line_h;
            let ww = text_width(word, pixel_size);
            cur.push_str(word);
            cur_w += ww;
            first = false;
        } else {
            cur.push_str(&piece);
            cur_w += w;
            first = false;
        }
    }
    if !cur.is_empty() {
        push_text(out, &cur, x, y + cy, pixel_size, color);
    }
}