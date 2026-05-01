//! Rustogon Level Format entry point.
//!
//! As of this revision only the v3 parser is shipped. The
//! historical v1 and v2 dialects have been retired; every
//! `.rlf` file the engine loads is parsed by `parser_v3`.
//!
//! # File directives
//!
//! A file may still opt in to v3 explicitly with the
//! `#[use_v3]` directive at the top. This is purely
//! self-documenting; absence of any dialect directive is
//! treated the same as presence of `#[use_v3]`. Legacy
//! `#[use_v2]` and `#[use_v1]` directives are silently
//! ignored, which means those old files fail at the first
//! non-v3 syntax they contain rather than silently degrading
//! to an older parser.
//!
//! # Safety
//!
//! The v3 parser enforces every invariant the runtime relies
//! on: thickness clamps, mask gaps, formula pathability,
//! survivable patterns. A bad level fails at load time with a
//! `ParseError` carrying the source line and column, never
//! silently degrades at runtime.

pub mod ast;
pub mod parser_v3;
pub mod formula;
pub mod safety;
pub mod meta;
pub mod expander;
pub mod rules;

pub use ast::*;
pub use parser_v3::ParseError;

/// Parse a `.rlf` source buffer. The v3 parser is always
/// used; older dialect directives (`#[use_v2]`, `#[use_v1]`)
/// are tolerated but do not change the parsing path.
pub fn parse_level(src: &str) -> Result<LevelAst, ParseError> {
    parser_v3::parse_level(src)
}