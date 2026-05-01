//! Signed distance field atlas baker.
//!
//! Takes a parsed TrueType font and produces an R8 pixel buffer
//! plus per glyph metadata suitable for upload to a Vulkan
//! image. The atlas layout is fixed: 16 columns by 12 rows of
//! 32 by 32 cells, total 512 by 384 pixels (192 KiB).
//!
//! # Algorithm
//!
//! For each codepoint:
//!
//! 1. Look up the glyph index in the parsed font's cmap.
//! 2. Decode the glyph outline (a set of contours of points).
//! 3. Convert each contour to a sequence of line segments by
//!    flattening quadratic Bezier curves through recursive
//!    subdivision until the control point is within one font
//!    unit of the chord.
//! 4. For every pixel in the glyph's atlas cell, compute the
//!    signed distance to the nearest segment in font units,
//!    convert to atlas pixels, normalize against the SDF
//!    spread, and encode as an unsigned 8 bit value.
//! 5. The sign comes from a horizontal ray cast even odd test
//!    against the same segments; positive distance means the
//!    pixel is inside the glyph (visible after smoothstep),
//!    negative means outside.
//!
//! All glyphs share the same vertical layout within their cell:
//! the baseline always sits at the same y coordinate, the same
//! pixels per em scale applies across the atlas, and each cell
//! covers the same window in font space relative to the pen
//! position. This is what makes SDF edge softness consistent
//! across glyph sizes; per glyph scale would produce blurrier
//! edges on wide glyphs and crisper on narrow ones.
//!
//! # Performance
//!
//! Naive O(pixels * segments) loops. For 191 codepoints with
//! ~1024 pixels per cell and ~30 segments per glyph after
//! tessellation, the bake takes roughly 100 ms in release mode
//! on a desktop core. Acceptable as a one time startup cost.

use std::collections::HashMap;

use crate::font::ttf::{ParsedFont, GlyphOutline, OutlinePoint};

/// Cell width and height in atlas pixels.
const CELL_SIZE: u32 = 32;

/// Empty pixel border around each glyph. Provides room for the
/// SDF gradient to extend beyond the glyph contour without
/// being clipped at the cell boundary.
const PADDING_PIXELS: u32 = 4;

/// Maximum signed distance encoded into the atlas, in atlas
/// pixels. Larger values give wider but blurrier AA at small
/// rendering sizes; smaller values give crisper but thinner
/// AA at large rendering sizes. Four pixels is a good
/// compromise for a 32 pixel cell.
const SDF_SPREAD: f32 = 4.0;

/// Number of cells per row in the atlas.
const GRID_W: u32 = 16;
/// Number of cell rows in the atlas.
const GRID_H: u32 = 12;

/// Per glyph atlas record. The bake emits one of these for
/// every codepoint that resolves to a non missing glyph.
#[derive(Clone, Copy, Debug)]
pub struct AtlasGlyph {
    /// UV coordinates of the cell's top left corner in 0..1.
    pub uv_min: [f32; 2],
    /// UV coordinates of the cell's bottom right corner in 0..1.
    pub uv_max: [f32; 2],
    /// Plane bounds of the glyph quad in em units, relative to
    /// the pen position. For text rendering, the screen quad
    /// for this glyph is positioned at:
    ///
    ///   left   = pen_x + plane_min.x * pixel_size
    ///   right  = pen_x + plane_max.x * pixel_size
    ///   top    = baseline_y - plane_max.y * pixel_size
    ///   bottom = baseline_y - plane_min.y * pixel_size
    ///
    /// Note that y in font space grows upward whereas y on
    /// screen grows downward, hence the sign flips.
    pub plane_min: [f32; 2],
    pub plane_max: [f32; 2],
    /// Horizontal advance width in em units.
    pub advance_em: f32,
}

/// Complete font atlas ready for GPU upload.
pub struct FontAtlas {
    /// R8 pixel buffer of length `width * height`.
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Per codepoint atlas record. Codepoints not present
    /// either had no glyph in the font or did not fit in the
    /// atlas grid.
    pub glyphs: HashMap<u32, AtlasGlyph>,
    /// Recommended distance between two consecutive baselines
    /// in em units.
    pub line_height_em: f32,
    /// Distance from baseline to the top of the typical
    /// uppercase glyph, in em units.
    pub ascent_em: f32,
    /// Distance from baseline to the bottom of the typical
    /// descender, in em units. Negative.
    pub descent_em: f32,
}

impl FontAtlas {
    /// Bake the atlas for the given codepoint set.
    ///
    /// Codepoints beyond the grid capacity are silently
    /// dropped. Codepoints that resolve to glyph index 0
    /// (the `.notdef` placeholder) are skipped as well; the
    /// runtime falls back to a substitute character for them.
    pub fn bake(font: &ParsedFont, codepoints: &[u32]) -> Self {
        let width = GRID_W * CELL_SIZE;
        let height = GRID_H * CELL_SIZE;
        let mut pixels = vec![0u8; (width * height) as usize];
        let mut glyphs = HashMap::new();

        let upem = font.units_per_em as f32;

        // Single uniform scale shared by every glyph. The cell's
        // inner area spans one full em vertically (ascent plus
        // |descent|), so a 1000 upem font renders at 24 atlas
        // pixels per em.
        let inner = (CELL_SIZE - 2 * PADDING_PIXELS) as f32;
        let total_height_units =
            (font.ascent as f32 - font.descent as f32).max(1.0);
        let scale = inner / total_height_units;

        // Pixel y of the baseline within each cell. The descent
        // sits in the lower padding band, so the baseline ends
        // up roughly one descent below the bottom inner edge.
        let baseline_y = (CELL_SIZE - PADDING_PIXELS) as f32
                       + (font.descent as f32) * scale;

        // Plane bounds shared by every cell. Computed by
        // inverting the pixel to font space mapping at the cell
        // edges and dividing by upem.
        let plane_min_x_em = (-(PADDING_PIXELS as f32)) / scale / upem;
        let plane_max_x_em = ((CELL_SIZE - PADDING_PIXELS) as f32) / scale / upem;
        let plane_min_y_em = (baseline_y - CELL_SIZE as f32) / scale / upem;
        let plane_max_y_em = baseline_y / scale / upem;

        for (i, &cp) in codepoints.iter().enumerate() {
            let gx = (i as u32) % GRID_W;
            let gy = (i as u32) / GRID_W;
            if gy >= GRID_H { break; }

            let glyph_idx = match font.glyph_index(cp) {
                Some(g) if g != 0 => g,
                _ => continue,
            };
            let outline = match font.glyph_outline(glyph_idx) {
                Some(o) => o,
                None => continue,
            };
            let metrics = font.glyph_metrics(glyph_idx);

            let cell_origin_x = (gx * CELL_SIZE) as i32;
            let cell_origin_y = (gy * CELL_SIZE) as i32;

            // Always record the atlas entry, even for empty
            // outlines (such as space). The cell pixels stay at
            // zero, which encodes "fully outside" everywhere,
            // and the runtime quad collapses to a 0 alpha
            // rectangle that the shader discards via smoothstep.
            let uv_min = [
                cell_origin_x as f32 / width as f32,
                cell_origin_y as f32 / height as f32,
            ];
            let uv_max = [
                (cell_origin_x as u32 + CELL_SIZE) as f32 / width as f32,
                (cell_origin_y as u32 + CELL_SIZE) as f32 / height as f32,
            ];

            glyphs.insert(cp, AtlasGlyph {
                uv_min, uv_max,
                plane_min: [plane_min_x_em, plane_min_y_em],
                plane_max: [plane_max_x_em, plane_max_y_em],
                advance_em: metrics.advance as f32 / upem,
            });

            if outline.contours.is_empty() {
                continue;
            }

            let segments = tessellate_outline(&outline);
            if segments.is_empty() {
                continue;
            }

            // Render the SDF for this cell. Pixel center
            // coordinates are 0.5 offsets so the geometric
            // center of each pixel is sampled rather than its
            // top left corner.
            for cy in 0..CELL_SIZE {
                for cx in 0..CELL_SIZE {
                    let px = cx as f32 + 0.5;
                    let py = cy as f32 + 0.5;

                    let fx = (px - PADDING_PIXELS as f32) / scale;
                    let fy = (baseline_y - py) / scale;

                    let mut min_sq = f32::INFINITY;
                    for &(a, b) in &segments {
                        let d = dist_sq_to_segment((fx, fy), a, b);
                        if d < min_sq { min_sq = d; }
                    }
                    let dist_units = min_sq.sqrt();
                    let dist_pixels = dist_units * scale;

                    let inside = point_in_polygon((fx, fy), &segments);
                    let signed = if inside { dist_pixels } else { -dist_pixels };

                    let normalized = (signed / SDF_SPREAD).clamp(-1.0, 1.0);
                    let encoded = ((normalized * 0.5 + 0.5) * 255.0)
                        .round()
                        .clamp(0.0, 255.0) as u8;

                    let ax = (cell_origin_x + cx as i32) as usize;
                    let ay = (cell_origin_y + cy as i32) as usize;
                    pixels[ay * width as usize + ax] = encoded;
                }
            }
        }

        let line_height_em =
            (font.ascent as f32 - font.descent as f32 + font.line_gap as f32)
            / upem;
        let ascent_em = font.ascent as f32 / upem;
        let descent_em = font.descent as f32 / upem;

        FontAtlas {
            pixels,
            width, height,
            glyphs,
            line_height_em,
            ascent_em, descent_em,
        }
    }
}

/// Convert all of a glyph's contours into a flat list of line
/// segments. Quadratic Bezier curves are flattened recursively;
/// implicit on curve points between two consecutive off curve
/// points are inserted as a preprocessing step.
fn tessellate_outline(
    outline: &GlyphOutline,
) -> Vec<((f32, f32), (f32, f32))> {
    let mut segments = Vec::new();

    for contour in &outline.contours {
        if contour.len() < 2 { continue; }
        tessellate_contour(contour, &mut segments);
    }

    segments
}

fn tessellate_contour(
    contour: &[OutlinePoint],
    segments: &mut Vec<((f32, f32), (f32, f32))>,
) {
    let n = contour.len();

    // Preprocess: insert implicit on curve midpoints between
    // any two consecutive off curve points. After this pass
    // the contour alternates on curve and off curve points
    // wherever curves were intended.
    let mut expanded: Vec<OutlinePoint> = Vec::with_capacity(n * 2);
    for i in 0..n {
        let prev = contour[(i + n - 1) % n];
        let curr = contour[i];
        if !prev.on_curve && !curr.on_curve {
            expanded.push(OutlinePoint {
                x: ((prev.x as i32 + curr.x as i32) / 2) as i16,
                y: ((prev.y as i32 + curr.y as i32) / 2) as i16,
                on_curve: true,
            });
        }
        expanded.push(curr);
    }

    let m = expanded.len();
    if m < 2 { return; }

    // Locate the first on curve point. If none exists the
    // contour is degenerate and we skip it.
    let start = match expanded.iter().position(|p| p.on_curve) {
        Some(i) => i,
        None => return,
    };

    // Walk the contour starting from the first on curve point.
    // Each step emits either a single line segment or a
    // quadratic Bezier (one off curve control plus the next
    // on curve endpoint).
    let mut i = 0usize;
    while i < m {
        let cur = expanded[(start + i) % m];
        let next = expanded[(start + i + 1) % m];
        if next.on_curve {
            segments.push((
                (cur.x as f32, cur.y as f32),
                (next.x as f32, next.y as f32),
            ));
            i += 1;
        } else {
            let after = expanded[(start + i + 2) % m];
            tessellate_quadratic(
                (cur.x as f32, cur.y as f32),
                (next.x as f32, next.y as f32),
                (after.x as f32, after.y as f32),
                segments,
                0,
            );
            i += 2;
        }
    }
}

/// Recursive de Casteljau subdivision until the control point
/// is within tolerance of the chord.
fn tessellate_quadratic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    segments: &mut Vec<((f32, f32), (f32, f32))>,
    depth: u32,
) {
    if depth >= 8 || flat_enough(p0, p1, p2, 1.0) {
        segments.push((p0, p2));
        return;
    }
    let m01 = midpoint(p0, p1);
    let m12 = midpoint(p1, p2);
    let m = midpoint(m01, m12);
    tessellate_quadratic(p0, m01, m, segments, depth + 1);
    tessellate_quadratic(m, m12, p2, segments, depth + 1);
}

fn flat_enough(
    p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), tol: f32,
) -> bool {
    let dx = p2.0 - p0.0;
    let dy = p2.1 - p0.1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-6 {
        let d = (p1.0 - p0.0).powi(2) + (p1.1 - p0.1).powi(2);
        return d < tol * tol;
    }
    // Perpendicular distance from p1 to the chord (p0, p2).
    let cross = dx * (p1.1 - p0.1) - dy * (p1.0 - p0.0);
    cross * cross / len_sq < tol * tol
}

fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

fn dist_sq_to_segment(
    p: (f32, f32), a: (f32, f32), b: (f32, f32),
) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-6 {
        let pdx = p.0 - a.0;
        let pdy = p.1 - a.1;
        return pdx * pdx + pdy * pdy;
    }
    let t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let cx = a.0 + t * dx;
    let cy = a.1 + t * dy;
    let pdx = p.0 - cx;
    let pdy = p.1 - cy;
    pdx * pdx + pdy * pdy
}

/// Even odd point in polygon test by horizontal ray casting.
/// Convention: a segment contributes one crossing when the ray
/// at y = p.y passes through the half open y range [lo, hi),
/// where lo and hi are the segment's lower and upper y bounds.
/// Horizontal segments contribute nothing; segments touching
/// the ray at their endpoints are counted at most once.
fn point_in_polygon(
    p: (f32, f32),
    segments: &[((f32, f32), (f32, f32))],
) -> bool {
    let mut inside = false;
    for &(a, b) in segments {
        let (lo, hi) = if a.1 <= b.1 { (a, b) } else { (b, a) };
        if p.1 < lo.1 || p.1 >= hi.1 { continue; }
        let t = (p.1 - lo.1) / (hi.1 - lo.1);
        let x_at = lo.0 + t * (hi.0 - lo.0);
        if x_at > p.0 { inside = !inside; }
    }
    inside
}