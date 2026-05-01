//! Central safety limits and math guards for the DSL pipeline.
//!
//! Every input path that can affect runtime performance, memory
//! footprint, or numerical stability passes through constants and
//! helpers defined here. The design assumption is that a .rlf
//! file may come from an untrusted source, a well meaning friend,
//! or a buggy tool. No matter where it comes from, loading it
//! must never:
//!
//! * crash the game process with a panic, stack overflow,
//!   division by zero, or NaN propagation into the renderer,
//! * hang the parser with an unbounded loop or exponential blow
//!   up from recursive meta expansion,
//! * consume more than a few megabytes of memory for the
//!   resulting AST regardless of the input size,
//! * expose any arbitrary code execution vector through the
//!   formula evaluator or the meta language.
//!
//! The game has no native extension mechanism, no scripting
//! backend that can touch the filesystem or network, and no
//! runtime compilation of any kind. All the DSL can do is emit
//! walls, pick triggers, and evaluate arithmetic. This module
//! is the enforcement of that invariant.

use std::time::{Duration, Instant};

/// Upper bound on the size of a source file accepted by the
/// parser. Above this the loader refuses to read the file at all
/// instead of streaming it into memory.
pub const MAX_FILE_BYTES: usize = 1_048_576;

/// Maximum number of tokens produced by the lexer. Hit before
/// any parsing decisions are made, so even a tokenizer pathology
/// caps quickly.
pub const MAX_TOKEN_COUNT: usize = 100_000;

/// Maximum number of AST nodes across the whole file after meta
/// expansion. Counted inside the expander; going over aborts
/// expansion with a readable error.
pub const MAX_AST_NODES: usize = 50_000;

/// Maximum sections allowed in a level. Sections are linear in
/// count so this is a high limit.
pub const MAX_SECTIONS: usize = 256;

/// Maximum statements per section after flattening. Bounds what
/// the runtime generator has to walk each frame.
pub const MAX_STMTS_PER_SECTION: usize = 2048;

/// Maximum walls that one obstacle spec may materialize into.
/// Protects against pathological formulas or pattern algebra.
pub const MAX_WALLS_PER_PATTERN: usize = 4096;

/// Maximum repeat count inside a single `repeat` block. The
/// expansion budget multiplies with this, but an additional per
/// block cap makes the most common author mistake impossible.
pub const MAX_REPEAT_COUNT: u32 = 256;

/// Maximum iterations of a meta level `for .. in ..` loop.
pub const MAX_FOR_ITERATIONS: u32 = 2048;

/// Maximum recursion depth for meta functions. Functions above
/// this abort expansion with an error rather than overflowing
/// the real Rust call stack.
pub const MAX_RECURSION_DEPTH: usize = 64;

/// Global expansion budget, shared across one load. Every time
/// the expander produces a new node or evaluates a meta value it
/// decrements the budget. When the budget reaches zero the
/// expansion terminates with an error, regardless of what caused
/// the growth.
pub const MAX_EXPANSION_BUDGET: u32 = 100_000;

/// Maximum character length of any string literal. Long strings
/// are almost always bugs or buffer overflow attempts.
pub const MAX_STRING_LENGTH: usize = 4096;

/// Fuel limit for one formula evaluation. Includes every
/// arithmetic op, every variable read, every iteration of every
/// embedded loop. Hitting this returns 0 (treated as gap).
pub const MAX_FORMULA_FUEL: u32 = 4096;

/// Per formula AST depth cap. Prevents the parser from building
/// a tree that then blows the stack when evaluated.
pub const MAX_FORMULA_AST_DEPTH: usize = 64;

/// Maximum number of elements a `take(stream, N)` expression
/// may produce. Keeps stream materialization bounded.
pub const MAX_STREAM_TAKE: usize = 1024;

/// Maximum number of steps one formula driven pattern may span.
/// Even if the budget allows more, no author will realistically
/// want to play through a thousand step formula.
pub const MAX_PATTERN_STEPS: u32 = 128;

/// Absolute hard stop on parse time. If a file takes longer than
/// this to load we kill the attempt. Defense in depth against
/// any case the fuel counters miss.
pub const MAX_PARSE_DURATION_MS: u64 = 2500;

/// Maximum absolute value accepted in plain AST numeric
/// literals. Covers the full u32 range with headroom, which is
/// what seeds and similar identity-style fields actually
/// require. The concrete limit (`1e10`) stays comfortably below
/// f32's lossless integer range boundary is nonetheless kept
/// as a coarse sanity check; anything larger almost certainly
/// indicates a typo in the source file.
pub const MAX_AST_LITERAL_MAGNITUDE: f32 = 1.0e10;

/// Stricter bound used inside the formula evaluator. Formula
/// arithmetic runs repeatedly (once per slot, per step, per
/// frame) so we clamp intermediate values aggressively to make
/// sure a single outlier cannot cascade into a runaway figure
/// that lands in a push constant and tears the shader state.
pub const MAX_FORMULA_MAGNITUDE: f32 = 1.0e6;

/// Validate a floating point value before storing it in the
/// AST. Returns an error when the value is non finite or
/// exceeds the AST magnitude limit.
///
/// The magnitude limit exists to catch obvious typos (a seed
/// written as `1e30` for example) without getting in the way
/// of legitimately large integer fields like random number
/// generator seeds.
pub fn check_finite(value: f32, where_from: &str) -> Result<f32, String> {
    if value.is_nan() {
        return Err(format!("NaN value in {}", where_from));
    }
    if value.is_infinite() {
        return Err(format!("infinite value in {}", where_from));
    }
    if value.abs() > MAX_AST_LITERAL_MAGNITUDE {
        return Err(format!(
            "value out of range in {} ({} exceeds {:.0e})",
            where_from, value, MAX_AST_LITERAL_MAGNITUDE));
    }
    Ok(value)
}

/// Tighter version of `check_finite` used by the formula
/// parser. Rejects NaN, infinities, and anything above 1e6.
/// Callers that need plain AST limits use `check_finite`.
pub fn check_finite_formula(value: f32, where_from: &str) -> Result<f32, String> {
    if value.is_nan() {
        return Err(format!("NaN value in {}", where_from));
    }
    if value.is_infinite() {
        return Err(format!("infinite value in {}", where_from));
    }
    if value.abs() > MAX_FORMULA_MAGNITUDE {
        return Err(format!(
            "formula value out of range in {} ({} exceeds {:.0e})",
            where_from, value, MAX_FORMULA_MAGNITUDE));
    }
    Ok(value)
}
/// Clamp a floating point value to a finite representation. Used
/// inside the formula evaluator on every intermediate result so
/// a single bad operation cannot poison the rest of the
/// computation.
pub fn sanitize_float(v: f32) -> f32 {
    if v.is_nan() { return 0.0; }
    if v.is_infinite() {
        if v > 0.0 { return 1.0e6; } else { return -1.0e6; }
    }
    v.clamp(-1.0e6, 1.0e6)
}

/// Safe division. Returns zero on division by zero or on any
/// intermediate non finite result, instead of producing a NaN.
pub fn safe_div(a: f32, b: f32) -> f32 {
    if b.abs() < 1.0e-12 { return 0.0; }
    sanitize_float(a / b)
}

/// Safe integer modulo. Takes f32 inputs to match the formula
/// language, rounds to integers, and returns zero when the
/// divisor is zero. Never panics.
pub fn safe_mod(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() { return 0.0; }
    let bi = (b.round() as i64).unsigned_abs() as i64;
    if bi == 0 { return 0.0; }
    let ai = a.round() as i64;
    (ai.rem_euclid(bi)) as f32
}

/// Safe bitwise AND. Operates on the integer representation of
/// the floats. Out of range inputs clamp to i32 range.
pub fn safe_and(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() { return 0.0; }
    let ai = a.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    let bi = b.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    (ai & bi) as f32
}

/// Safe bitwise OR.
pub fn safe_or(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() { return 0.0; }
    let ai = a.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    let bi = b.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    (ai | bi) as f32
}

/// Safe right shift. Shift amount clamped to 0..=31.
pub fn safe_shr(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() { return 0.0; }
    let ai = a.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    let bi = (b.round() as i32).clamp(0, 31);
    (ai >> bi) as f32
}

/// Safe left shift. Shift amount clamped to 0..=31 and the
/// multiplication performed via wrapping_shl so an overflow
/// produces a deterministic wrap rather than a runtime panic in
/// debug builds.
pub fn safe_shl(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() { return 0.0; }
    let ai = a.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    let bi = (b.round() as i32).clamp(0, 31);
    (ai.wrapping_shl(bi as u32)) as f32
}

/// Tracks a shared expansion budget across the meta expander.
/// Shared by reference so every recursive helper decrements the
/// same pool.
pub struct Budget {
    remaining: u32,
}

impl Budget {
    pub fn new(initial: u32) -> Self {
        Budget { remaining: initial }
    }
    pub fn spend(&mut self, cost: u32) -> Result<(), String> {
        if cost > self.remaining {
            self.remaining = 0;
            return Err(format!(
                "expansion budget exhausted (limit {})",
                MAX_EXPANSION_BUDGET));
        }
        self.remaining -= cost;
        Ok(())
    }
    pub fn remaining(&self) -> u32 { self.remaining }
}

/// Deadline helper for parse time limiting. Polled at every
/// statement boundary, so a file whose lexer or expander happens
/// to loop inside a single statement still takes at most a few
/// milliseconds longer than the bound.
pub struct Deadline {
    end: Instant,
}

impl Deadline {
    pub fn new(ms: u64) -> Self {
        Deadline {
            end: Instant::now() + Duration::from_millis(ms),
        }
    }
    pub fn expired(&self) -> bool {
        Instant::now() > self.end
    }
    pub fn check(&self) -> Result<(), String> {
        if self.expired() {
            Err(format!(
                "parse deadline exceeded (limit {} ms)",
                MAX_PARSE_DURATION_MS))
        } else {
            Ok(())
        }
    }
}