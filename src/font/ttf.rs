//! TrueType font parser.
//!
//! Reads just enough of the TTF file format to drive an SDF
//! atlas baker: codepoint to glyph index mapping, glyph
//! outlines as point sequences with on or off curve flags, and
//! horizontal metrics.
//!
//! The parser intentionally does not implement TrueType bytecode
//! hinting, variation tables, GPOS or GSUB tables, kerning, or
//! any cmap subtable other than format 4. These omissions cover
//! the bulk of the format spec and keep the implementation under
//! one thousand lines while still working on every modern Latin
//! and Cyrillic capable font we have tested.
//!
//! Format reference: Microsoft's OpenType specification, version
//! 1.9 or later.
//!
//! # Endianness
//!
//! TrueType is big endian. The reader helpers at the bottom of
//! this file convert from big endian on every read. Modern x86
//! and ARM cores have a single instruction byte swap, so the
//! cost is negligible compared to the parser's logic.
//!
//! # Allocation
//!
//! The parser allocates one `Vec<u8>` to own a copy of the input
//! bytes (so the resulting `ParsedFont` is self contained), one
//! `HashMap` for the codepoint to glyph table, and small
//! transient buffers per glyph during outline decoding. There is
//! no recursion that depends on file content beyond a hard
//! capped composite glyph depth of four.

use std::collections::HashMap;

/// Result of parsing a TrueType file.
///
/// Holds an owned copy of the raw bytes plus parsed table
/// offsets so glyph data can be decoded lazily on demand. The
/// codepoint to glyph index table is fully populated up front
/// because building it costs nothing at runtime and downstream
/// code typically needs the full set.
pub struct ParsedFont {
    raw: Vec<u8>,

    /// Font design units per em. Conventional values are 1000
    /// (PostScript style) and 2048 (TrueType style). Used to
    /// normalize all metrics to em units in the SDF baker.
    pub units_per_em: u16,

    /// Distance from baseline to the top of the typical
    /// uppercase glyph, in font design units. Positive value.
    pub ascent: i16,

    /// Distance from baseline to the bottom of the typical
    /// descender, in font design units. Conventionally negative.
    pub descent: i16,

    /// Recommended additional spacing between two consecutive
    /// lines, in font design units. Often zero.
    pub line_gap: i16,

    /// Total number of glyphs in the font. Glyph index 0 is
    /// always the `.notdef` glyph used for missing characters.
    pub num_glyphs: u16,

    /// Codepoint to glyph index table built from the cmap.
    pub cmap: HashMap<u32, u16>,

    glyf_offset: u32,
    loca_offset: u32,
    loca_format: i16,
    hmtx_offset: u32,
    num_h_metrics: u16,
}

/// Outline of one glyph after parsing the glyf table.
///
/// Contours are sequences of points marked as either on the
/// curve (corner or curve endpoint) or off the curve (Bezier
/// control point). Two consecutive off curve points carry an
/// implicit on curve point at their midpoint, which the SDF
/// baker handles when tessellating curves into line segments.
pub struct GlyphOutline {
    /// Bounding box in font design units, in the order
    /// (xmin, ymin, xmax, ymax).
    pub bbox: (i16, i16, i16, i16),
    /// One vector per contour, each holding the contour's
    /// points in order.
    pub contours: Vec<Vec<OutlinePoint>>,
}

/// One point on a glyph contour.
#[derive(Clone, Copy)]
pub struct OutlinePoint {
    pub x: i16,
    pub y: i16,
    /// True for an on curve point (Bezier endpoint or corner).
    /// False for an off curve point (Bezier control).
    pub on_curve: bool,
}

/// Horizontal metrics for one glyph.
pub struct GlyphMetrics {
    /// Horizontal advance width in font design units. The pen
    /// advances by this amount after drawing the glyph,
    /// regardless of where the glyph's outline actually ends.
    pub advance: u16,
    /// Distance from the pen position to the leftmost outline
    /// point at the time of drawing. Often zero or slightly
    /// negative for italics.
    pub lsb: i16,
}

impl ParsedFont {
    /// Parse a TTF file given its raw bytes.
    ///
    /// Returns a typed error message on any structural failure.
    /// The most common rejection is the OpenType `OTTO` scaler,
    /// which indicates a CFF based font that this parser does
    /// not support. CFF requires a separate decoder and is
    /// rarely used by the open source fonts we ship with.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 12 {
            return Err("font file too small for offset table".into());
        }

        let scaler = read_u32(bytes, 0);
        let valid_scaler =
               scaler == 0x0001_0000        // TrueType
            || scaler == 0x7472_7565        // 'true'
            || scaler == 0x7479_7031;       // 'typ1', rare but valid TT
        if scaler == 0x4F54_544F {
            return Err(
                "OpenType CFF fonts are not supported; \
                 use a TrueType outline font."
                .into());
        }
        if !valid_scaler {
            return Err(format!(
                "unrecognized font scaler 0x{:08X}", scaler));
        }

        let num_tables = read_u16(bytes, 4) as usize;
        if bytes.len() < 12 + num_tables * 16 {
            return Err("table directory truncated".into());
        }

        let mut tables: HashMap<[u8; 4], (u32, u32)> = HashMap::new();
        for i in 0..num_tables {
            let entry = 12 + i * 16;
            let tag = [
                bytes[entry],     bytes[entry + 1],
                bytes[entry + 2], bytes[entry + 3],
            ];
            let off = read_u32(bytes, entry + 8);
            let len = read_u32(bytes, entry + 12);
            tables.insert(tag, (off, len));
        }

        let need = |tag: &[u8; 4]| -> Result<u32, String> {
            tables.get(tag).map(|(o, _)| *o).ok_or_else(|| {
                format!("missing required table '{}'",
                        std::str::from_utf8(tag).unwrap_or("?"))
            })
        };

        let head_off = need(b"head")? as usize;
        let maxp_off = need(b"maxp")? as usize;
        let hhea_off = need(b"hhea")? as usize;
        let hmtx_off = need(b"hmtx")?;
        let cmap_off = need(b"cmap")? as usize;
        let loca_off = need(b"loca")?;
        let glyf_off = need(b"glyf")?;

        // head: units per em at offset 18, indexToLocFormat at offset 50.
        let units_per_em = read_u16(bytes, head_off + 18);
        let loca_format = read_i16(bytes, head_off + 50);

        // maxp: numGlyphs at offset 4.
        let num_glyphs = read_u16(bytes, maxp_off + 4);

        // hhea: ascent at 4, descent at 6, line_gap at 8,
        // numberOfHMetrics at 34.
        let ascent = read_i16(bytes, hhea_off + 4);
        let descent = read_i16(bytes, hhea_off + 6);
        let line_gap = read_i16(bytes, hhea_off + 8);
        let num_h_metrics = read_u16(bytes, hhea_off + 34);

        let cmap = parse_cmap(bytes, cmap_off)?;

        Ok(ParsedFont {
            raw: bytes.to_vec(),
            units_per_em,
            ascent, descent, line_gap,
            num_glyphs,
            cmap,
            glyf_offset: glyf_off,
            loca_offset: loca_off,
            loca_format,
            hmtx_offset: hmtx_off,
            num_h_metrics,
        })
    }

    /// Map a Unicode codepoint to a glyph index.
    pub fn glyph_index(&self, codepoint: u32) -> Option<u16> {
        self.cmap.get(&codepoint).copied()
    }

    /// Read the horizontal metrics for a glyph index.
    ///
    /// Glyph indices beyond `num_h_metrics` reuse the last
    /// recorded advance width and pull only the left side
    /// bearing from the trailing portion of the hmtx table.
    /// This is how monospace fonts compress their hmtx data.
    pub fn glyph_metrics(&self, glyph_index: u16) -> GlyphMetrics {
        let i = glyph_index as usize;
        let n = self.num_h_metrics as usize;
        let base = self.hmtx_offset as usize;
        if i < n {
            let off = base + i * 4;
            GlyphMetrics {
                advance: read_u16(&self.raw, off),
                lsb: read_i16(&self.raw, off + 2),
            }
        } else {
            let last_adv = base + (n.saturating_sub(1)) * 4;
            let lsb_off = base + n * 4 + (i - n) * 2;
            GlyphMetrics {
                advance: read_u16(&self.raw, last_adv),
                lsb: read_i16(&self.raw, lsb_off),
            }
        }
    }

    /// Decode a glyph's outline.
    ///
    /// Returns `None` for glyphs outside the valid index range.
    /// Empty glyphs (such as the space character) return a
    /// `GlyphOutline` with an empty `contours` vector.
    pub fn glyph_outline(&self, glyph_index: u16) -> Option<GlyphOutline> {
        self.glyph_outline_internal(glyph_index, 0)
    }

    fn glyph_outline_internal(
        &self,
        glyph_index: u16,
        depth: u32,
    ) -> Option<GlyphOutline> {
        // Hard cap on composite recursion. Real fonts never go
        // beyond two or three levels of nesting; this guards
        // against malformed files attempting to exhaust the
        // host stack.
        if depth > 4 { return None; }
        if glyph_index >= self.num_glyphs { return None; }

        let (start, end) = if self.loca_format == 0 {
            // Short loca: uint16 entries, multiplied by 2.
            let loca = self.loca_offset as usize + (glyph_index as usize) * 2;
            let s = read_u16(&self.raw, loca) as u32 * 2;
            let e = read_u16(&self.raw, loca + 2) as u32 * 2;
            (s, e)
        } else {
            // Long loca: uint32 entries.
            let loca = self.loca_offset as usize + (glyph_index as usize) * 4;
            let s = read_u32(&self.raw, loca);
            let e = read_u32(&self.raw, loca + 4);
            (s, e)
        };

        if start == end {
            return Some(GlyphOutline {
                bbox: (0, 0, 0, 0),
                contours: Vec::new(),
            });
        }

        let g = self.glyf_offset as usize + start as usize;
        let num_contours = read_i16(&self.raw, g);
        let bbox = (
            read_i16(&self.raw, g + 2),
            read_i16(&self.raw, g + 4),
            read_i16(&self.raw, g + 6),
            read_i16(&self.raw, g + 8),
        );
        let body = g + 10;

        if num_contours >= 0 {
            self.parse_simple_glyph(body, num_contours as usize, bbox)
        } else {
            self.parse_composite_glyph(body, depth, bbox)
        }
    }

    fn parse_simple_glyph(
        &self,
        offset: usize,
        num_contours: usize,
        bbox: (i16, i16, i16, i16),
    ) -> Option<GlyphOutline> {
        if num_contours == 0 {
            return Some(GlyphOutline { bbox, contours: Vec::new() });
        }

        // End points of contours, then instructions length and
        // bytes (which we skip), then variable size flags and
        // coordinates. The format is described in chapter
        // "glyf - Glyph Data" of the OpenType spec.
        let mut end_pts = Vec::with_capacity(num_contours);
        for i in 0..num_contours {
            end_pts.push(read_u16(&self.raw, offset + i * 2));
        }
        let num_points = (*end_pts.last().unwrap_or(&0) as usize) + 1;

        let mut p = offset + num_contours * 2;
        let instr_len = read_u16(&self.raw, p) as usize;
        p += 2 + instr_len;

        // Decode flags. Bit 3 (0x08) signals that the next byte
        // is a repeat count for the previous flag value.
        let mut flags: Vec<u8> = Vec::with_capacity(num_points);
        while flags.len() < num_points {
            let f = self.raw[p];
            p += 1;
            flags.push(f);
            if f & 0x08 != 0 {
                let repeat = self.raw[p] as usize;
                p += 1;
                for _ in 0..repeat {
                    if flags.len() >= num_points { break; }
                    flags.push(f);
                }
            }
        }
        flags.truncate(num_points);

        // Decode x coordinates. Each value is a delta from the
        // previous point, with bit 1 (0x02) toggling between
        // one byte unsigned and two byte signed encodings, and
        // bit 4 (0x10) reused as either the sign bit (when bit
        // 1 is set) or a "same as previous" marker.
        let mut xs: Vec<i16> = Vec::with_capacity(num_points);
        let mut x_acc: i32 = 0;
        for &f in &flags {
            let dx: i32 = if f & 0x02 != 0 {
                let v = self.raw[p] as i32;
                p += 1;
                if f & 0x10 != 0 { v } else { -v }
            } else if f & 0x10 != 0 {
                0
            } else {
                let v = read_i16(&self.raw, p) as i32;
                p += 2;
                v
            };
            x_acc = x_acc.wrapping_add(dx);
            xs.push(x_acc as i16);
        }

        // Same shape for y, with bits 2 (0x04) and 5 (0x20).
        let mut ys: Vec<i16> = Vec::with_capacity(num_points);
        let mut y_acc: i32 = 0;
        for &f in &flags {
            let dy: i32 = if f & 0x04 != 0 {
                let v = self.raw[p] as i32;
                p += 1;
                if f & 0x20 != 0 { v } else { -v }
            } else if f & 0x20 != 0 {
                0
            } else {
                let v = read_i16(&self.raw, p) as i32;
                p += 2;
                v
            };
            y_acc = y_acc.wrapping_add(dy);
            ys.push(y_acc as i16);
        }

        // Slice the flat point list into per contour vectors.
        let mut contours = Vec::with_capacity(num_contours);
        let mut start = 0usize;
        for &end in &end_pts {
            let end = end as usize;
            if end >= num_points || end < start {
                return None;
            }
            let mut points = Vec::with_capacity(end - start + 1);
            for i in start..=end {
                points.push(OutlinePoint {
                    x: xs[i], y: ys[i],
                    on_curve: flags[i] & 0x01 != 0,
                });
            }
            contours.push(points);
            start = end + 1;
        }

        Some(GlyphOutline { bbox, contours })
    }

    fn parse_composite_glyph(
        &self,
        offset: usize,
        depth: u32,
        bbox: (i16, i16, i16, i16),
    ) -> Option<GlyphOutline> {
        let mut all_contours = Vec::new();
        let mut p = offset;

        loop {
            let flags = read_u16(&self.raw, p);
            let glyph_idx = read_u16(&self.raw, p + 2);
            p += 4;

            // Argument decoding. Two variants: integer translation
            // (the common case used by most accented glyphs) or
            // point matching (which we cannot reproduce without
            // running the simple glyphs first; we skip it).
            let (dx, dy) = if flags & 0x0001 != 0 {
                // 16 bit args.
                if flags & 0x0002 != 0 {
                    let x = read_i16(&self.raw, p) as i32;
                    let y = read_i16(&self.raw, p + 2) as i32;
                    p += 4;
                    (x, y)
                } else {
                    p += 4;
                    (0, 0)
                }
            } else {
                // 8 bit args.
                if flags & 0x0002 != 0 {
                    let x = (self.raw[p] as i8) as i32;
                    let y = (self.raw[p + 1] as i8) as i32;
                    p += 2;
                    (x, y)
                } else {
                    p += 2;
                    (0, 0)
                }
            };

            // Skip any transform matrix. We deliberately do not
            // apply scale or rotation; non integer transforms are
            // rare for accented Latin and Cyrillic glyphs and
            // never essential for legible text.
            if flags & 0x0008 != 0 {
                p += 2;
            } else if flags & 0x0040 != 0 {
                p += 4;
            } else if flags & 0x0080 != 0 {
                p += 8;
            }

            if let Some(sub) = self.glyph_outline_internal(glyph_idx, depth + 1) {
                for contour in sub.contours {
                    let translated: Vec<OutlinePoint> = contour
                        .into_iter()
                        .map(|pt| OutlinePoint {
                            x: pt.x.wrapping_add(dx as i16),
                            y: pt.y.wrapping_add(dy as i16),
                            on_curve: pt.on_curve,
                        })
                        .collect();
                    all_contours.push(translated);
                }
            }

            // MORE_COMPONENTS bit signals another component
            // record follows. Some fonts add an instruction
            // record at the very end (WE_HAVE_INSTRUCTIONS,
            // bit 0x0100) which we ignore.
            if flags & 0x0020 == 0 { break; }
        }

        Some(GlyphOutline { bbox, contours: all_contours })
    }
}

/// Walk the cmap table and build the codepoint to glyph index
/// table from the first usable subtable.
///
/// Preference order: Windows Unicode BMP (platform 3, encoding
/// 1), then any Unicode platform subtable (platform 0). Format
/// 4 is mandated, which is universal for Unicode coverage up to
/// U+FFFF. Format 12 (full Unicode) is not supported, but every
/// font we have shipped covers ASCII and Cyrillic in format 4.
fn parse_cmap(bytes: &[u8], cmap_off: usize) -> Result<HashMap<u32, u16>, String> {
    let _version = read_u16(bytes, cmap_off);
    let num_subtables = read_u16(bytes, cmap_off + 2) as usize;

    let mut chosen: Option<u32> = None;
    let mut fallback: Option<u32> = None;

    for i in 0..num_subtables {
        let off = cmap_off + 4 + i * 8;
        let plat = read_u16(bytes, off);
        let enc = read_u16(bytes, off + 2);
        let sub_off = read_u32(bytes, off + 4);

        if plat == 3 && enc == 1 {
            chosen = Some(sub_off);
            break;
        }
        if plat == 0 && fallback.is_none() {
            fallback = Some(sub_off);
        }
    }

    let sub_off = chosen.or(fallback)
        .ok_or_else(|| "no usable cmap subtable".to_string())?;
    let sub = cmap_off + sub_off as usize;
    let format = read_u16(bytes, sub);
    if format != 4 {
        return Err(format!(
            "cmap format {} is not supported, only format 4",
            format));
    }

    let length = read_u16(bytes, sub + 2) as usize;
    let _language = read_u16(bytes, sub + 4);
    let seg_count_x2 = read_u16(bytes, sub + 6) as usize;
    let seg_count = seg_count_x2 / 2;

    let end_codes = sub + 14;
    let start_codes = end_codes + seg_count_x2 + 2;
    let id_deltas = start_codes + seg_count_x2;
    let id_range_offsets = id_deltas + seg_count_x2;

    let mut map = HashMap::new();
    for i in 0..seg_count {
        let end = read_u16(bytes, end_codes + i * 2);
        let start = read_u16(bytes, start_codes + i * 2);
        let delta = read_i16(bytes, id_deltas + i * 2);
        let range_off = read_u16(bytes, id_range_offsets + i * 2);

        // Sentinel last segment ends at 0xFFFF; nothing useful
        // beyond it for our codepoint range.
        if end == 0xFFFF && start == 0xFFFF { continue; }

        for cp in start..=end {
            let glyph_id = if range_off == 0 {
                ((cp as i32).wrapping_add(delta as i32) & 0xFFFF) as u16
            } else {
                let offset = id_range_offsets + i * 2
                           + range_off as usize
                           + ((cp - start) as usize) * 2;
                if offset + 2 > sub + length { continue; }
                let raw_id = read_u16(bytes, offset);
                if raw_id == 0 { continue; }
                ((raw_id as i32).wrapping_add(delta as i32) & 0xFFFF) as u16
            };
            if glyph_id != 0 {
                map.insert(cp as u32, glyph_id);
            }
        }
    }

    Ok(map)
}

// Big endian integer readers. Each performs bounds checking
// implicitly through Rust's slice indexing.

fn read_u16(bytes: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([bytes[off], bytes[off + 1]])
}

fn read_i16(bytes: &[u8], off: usize) -> i16 {
    i16::from_be_bytes([bytes[off], bytes[off + 1]])
}

fn read_u32(bytes: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([
        bytes[off], bytes[off + 1],
        bytes[off + 2], bytes[off + 3],
    ])
}