//! Rustogon Level Format entry point.
//!
//! The engine ships with two parsers. A given `.rlf` file picks
//! one via a file-level directive placed before the `level`
//! keyword:
//!
//! * no directive, or anything else       -> v1 (stable).
//! * `#[use_v2]`                          -> v2 (richer syntax).
//!
//! Both parsers produce the same [`LevelAst`], so the rest of
//! the engine has no idea which dialect was used.
//!
//! # v1 fingerprints
//!
//! Curly-brace records, `key = value` fields, commas cosmetic.
//! Lived here since the start, kept as the default because
//! every existing level file is written in it.
//!
//! # v2 fingerprints
//!
//! * `:atom` literals for enum-valued fields
//!   (`dir = :cw`, `anim = :ease_out`).
//! * Sigils `~p"..."`, `~m"..."`, `~f"..."` for paths, masks,
//!   and formulas respectively.
//! * `do ... end` as a synonym for `{ ... }`.
//! * `|>` pipe chains for triggers
//!   (`trigger :flip |> :shake { ... }`).
//! * `>>` modifier chains on obstacles
//!   (`emit spiral { ... } >> :thickness { mult = 1.3 }`).
//! * `where x = 1, y = 2` bindings introducing local scope
//!   before a section body.
//! * `::` type annotations on `var` declarations.
//! * Enum ranges via `A..B` (also accepted in v1).
//! * Bare identifiers resolve through the variable scope, so
//!   a `where`-bound name can be used directly without `@`.
//!
//! Opt in at the very top of the file:
//!
//!     #[use_v2]
//!
//!     level "My Song" do
//!         meta do
//!             author = "Me"
//!             music  = ~p"assets/music/track.qoa"
//!             bpm    = 128
//!         end
//!
//!         -- rest of the file ...
//!     end
//!
//! # Safety
//!
//! Both parsers enforce the same invariants: thickness clamps,
//! mask gaps, formula pathability, survivable patterns. A bad
//! level fails at load time with a `ParseError` carrying the
//! source line and column, never silently degrades at runtime.

pub mod ast;
pub mod parser;
pub mod parser_v2;
pub mod formula;

pub use ast::*;
pub use parser::ParseError;

/// Parse a `.rlf` source buffer. Selection between v1 and v2
/// happens here based on whether `#[use_v2]` appears in the
/// file-level directive prelude.
pub fn parse_level(src: &str) -> Result<LevelAst, ParseError> {
    if detect_use_v2(src) {
        parser_v2::parse_level(src)
    } else {
        parser::parse_level(src)
    }
}

/// Cheap scan for `#[use_v2]` among the leading `#[...]`
/// directives. Stops at the first non-comment, non-directive
/// line so directives embedded deeper in the file cannot
/// retroactively switch parsers.
fn detect_use_v2(src: &str) -> bool {
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()           { continue; }
        if trimmed.starts_with("//")    { continue; }
        if trimmed.starts_with("--")    { continue; }
        if trimmed.starts_with("#[") {
            let inner = trimmed[2..].trim_start();
            if inner.starts_with("use_v2") { return true; }
            continue;
        }
        // Reached real content without seeing the directive.
        return false;
    }
    false
}