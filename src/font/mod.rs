//! TrueType font loading and SDF text rendering.
//!
//! This module is fully self contained. It depends only on the
//! Rust standard library and `ash` for Vulkan bindings, matching
//! the engine's no third party crate policy.
//!
//! # Pipeline
//!
//! 1. The TTF file is embedded at compile time via `include_bytes!`
//!    from `assets/fonts/Exo2-Regular.ttf`.
//! 2. `init()` parses the file and bakes a signed distance field
//!    atlas covering ASCII plus the Cyrillic basic block. The
//!    atlas lives in a `OnceLock` so any module can reach it
//!    without explicit plumbing.
//! 3. The Vulkan side (`TextStage`) uploads the atlas to a GPU
//!    image, builds a sampler, descriptor set, and graphics
//!    pipeline. It is owned by the renderer and runs alongside
//!    the existing shape pipeline inside the offscreen pass.
//! 4. Higher level code calls `font::push_text` to append vertex
//!    quads to a `Vec<TextVertex>`, which the renderer feeds into
//!    `TextStage::render` once per frame.
//!
//! # Why SDF
//!
//! The previous bitmap font (`text.rs`) draws fixed size glyph
//! quads. At any size other than the baked one the result is
//! either pixelated (smaller) or visibly stretched (larger). SDF
//! sampling gives a single atlas that scales smoothly from a few
//! pixels per em up to several hundred. Anti aliasing is implicit
//! through the fragment shader's derivative based smoothstep, so
//! we do not need MSAA to get clean edges.
//!
//! # Limitations
//!
//! * Single font, single style. Supporting bold and italic would
//!   require additional atlases or a multi channel encoding.
//! * No kerning or shaping. Each codepoint advances by its
//!   horizontal advance width with no contextual adjustment.
//! * No bidirectional text. Strings render left to right.
//! * The codepoint set is fixed at startup. Adding glyphs at
//!   runtime would require a dynamic atlas.

use std::sync::OnceLock;

mod ttf;
mod bake;
mod stage;
mod text;

pub use bake::{AtlasGlyph, FontAtlas};
pub use stage::{TextStage, TextVertex};
pub use text::{
    push_text, push_text_alpha, push_text_centered,
    push_text_centered_alpha, push_text_right, push_text_right_alpha,
    text_width, text_height, line_height,
};

/// Embedded font file. The build fails at compile time if the
/// file is absent, which is the desired behaviour: a missing
/// font is a configuration error, not something to recover from
/// at runtime.
const FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/Exo2-Regular.ttf");

/// Process wide font atlas. Initialized exactly once by `init()`
/// and accessed by `atlas()` thereafter.
static ATLAS: OnceLock<FontAtlas> = OnceLock::new();

/// Parse the embedded TTF file and bake the SDF atlas.
///
/// Returns an error if the font file is malformed in a way the
/// minimal parser cannot handle. The most common cause is the
/// font using CFF outlines (`OTTO` scaler) rather than TrueType.
///
/// On success the global atlas is populated and `atlas()` is
/// safe to call from any thread. Calling `init()` more than once
/// is an error: the second call returns immediately without
/// re-baking, since the `OnceLock::set` rejects duplicates.
pub fn init() -> Result<(), String> {
    if ATLAS.get().is_some() {
        return Ok(());
    }
    let parsed = ttf::ParsedFont::parse(FONT_BYTES)?;
    let codepoints = default_codepoints();
    let baked = bake::FontAtlas::bake(&parsed, &codepoints);
    let _ = ATLAS.set(baked);
    Ok(())
}

/// Borrow the global font atlas. Panics if `init()` was not
/// called first; this is an authoring error and panicking gives
/// the clearest stack trace at the call site.
pub fn atlas() -> &'static FontAtlas {
    ATLAS.get().expect(
        "font::atlas() called before font::init(); \
         initialize the font system at startup before any \
         renderer or text drawing call.")
}

/// Codepoint set baked into the default atlas. Two contiguous
/// ranges totaling 191 codepoints, which fits in the 16 by 12
/// cell grid with one empty row of headroom for future
/// extensions.
fn default_codepoints() -> Vec<u32> {
    let mut cps = Vec::with_capacity(192);
    for cp in 0x0020..=0x007Eu32 { cps.push(cp); }
    for cp in 0x0400..=0x045Fu32 { cps.push(cp); }
    cps
}