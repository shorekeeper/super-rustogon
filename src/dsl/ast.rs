//! Abstract syntax tree produced by the Rustogon Level Format v2
//! parser.
//!
//! This module is the single source of truth for what a `.rlf`
//! level can express. It intentionally carries no knowledge of
//! the renderer, the audio worker or Vulkan. Tests or external
//! tools can parse a level file and walk this AST without
//! linking the game executable.
//!
//! # What changed from v1
//!
//! * Obstacle patterns gained `Staircase`, `Corridor` and `Cubes`
//!   (the Super Hexagon tunnel shapes visible on the reference
//!   screenshots) plus a scriptable `CustomFormula` pattern whose
//!   mask is derived from a tiny expression language; the parser
//!   rejects formulas whose mask has no gap (unsurvivable).
//! * Triggers were split into two groups: the historical single
//!   parameter ones (`Flip`, `Tilt`, `Pulse`, `SpeedMult`,
//!   `HueShift`) and a richer family described by structs with
//!   named fields, accessible via a `{ ... }` block in the DSL:
//!   `SpeedWarp`, `Glitch`, `Shake`, `Zoom`, `Invert`, `Strobe`.
//! * Statements gained `Repeat { count, body }` loops and a
//!   `LocalVars` scope so authors can name numeric constants and
//!   reuse them across a section without re-typing.
//! * Levels may declare a `globals` block, visible to every
//!   section, whose variables have `pub`/`priv`/`read` access
//!   modifiers. `local` variables declared inside a section (or
//!   inside a `repeat` body) shadow the globals for the length
//!   of that scope and are unwound on exit.
//! * File level directives, written `#[timestamp_format_use_*]`
//!   before the `level` form, override the meaning of every
//!   `at` clause: relative 0..1 (default), absolute seconds,
//!   beat buckets or author named sections.

use crate::levels::Palette;
use crate::levels::difficulty::Tier;

/// Top level syntactic unit corresponding to one `.rlf` file.
#[derive(Clone, Debug)]
pub struct LevelAst {
    pub meta:             Meta,
    pub palette:          Palette,
    pub difficulty:       DifficultySpec,
    pub generation:       GenerationSpec,
    /// Meaning of every `at` clause inside the file. Defaults to
    /// [`TimestampFormat::Relative`] so older files keep working.
    pub timestamp_format: TimestampFormat,
    /// File level variable declarations visible everywhere else.
    pub globals:          Vec<VarDecl>,
    /// Every `section "..." at ... { ... }` form, sorted by `at`
    /// once parsing finishes.
    pub sections:         Vec<Section>,
    /// Debug helper populated by the `#[startfrom]` directive.
    /// Main loop seeks the music to this many seconds the
    /// moment the level starts, so authors can iterate on a
    /// specific drop without waiting through the whole song.
    /// `0.0` means "start from the top".
    pub start_from_seconds: f32,
}

impl Default for LevelAst {
    fn default() -> Self {
        LevelAst {
            meta:               Meta::default(),
            palette:            Palette::default(),
            difficulty:         DifficultySpec::default(),
            generation:         GenerationSpec::default(),
            timestamp_format:   TimestampFormat::default(),
            globals:            Vec::new(),
            sections:           Vec::new(),
            start_from_seconds: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub name:        String,
    pub subtitle:    String,
    pub author:      String,
    pub song:        String,
    pub bpm:         u32,
    pub music:       String,
    pub description: String,
}

#[derive(Clone, Copy, Debug)]
pub struct DifficultySpec {
    pub base_tier: Tier,
    pub min_tier:  Tier,
    pub max_tier:  Tier,
}

impl Default for DifficultySpec {
    fn default() -> Self {
        DifficultySpec {
            base_tier: Tier::Rookie,
            min_tier:  Tier::Rookie,
            max_tier:  Tier::Expert,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GenerationSpec {
    pub sides:        u32,
    pub seed:         u32,
    pub hue_speed:    f32,
    pub speed_mult:   f32,
    pub density_mult: f32,
}

impl Default for GenerationSpec {
    fn default() -> Self {
        GenerationSpec {
            sides:        6,
            seed:         0x5EED1234,
            hue_speed:    0.0,
            speed_mult:   1.0,
            density_mult: 1.0,
        }
    }
}

/// How `at` clauses should be interpreted when choosing the
/// active section.
///
/// * `Relative`       — `at` is a fraction of the track (0..1).
/// * `TrackLength`    — `at` is absolute seconds, compared to
///                       the live `audio.music_position()`.
/// * `Beats { total }`— the track is divided into `total` beat
///                       buckets; `at` is the bucket index.
/// * `Named(name)`    — author named format. Acts like
///                       `Relative` for scheduling but tools
///                       may distinguish them.
#[derive(Clone, Debug)]
pub enum TimestampFormat {
    Relative,
    TrackLength,
    Beats  { total: u32 },
    Named  (String),
}

impl Default for TimestampFormat {
    fn default() -> Self { TimestampFormat::Relative }
}

#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    /// Raw value written after `at`. Meaning depends on the
    /// file's [`TimestampFormat`]; normalized to a 0..1 fraction
    /// by [`Section::progress_threshold`] whenever the generator
    /// needs it.
    pub at:   f32,
    pub body: Vec<Stmt>,
}

impl Section {
    /// Convert the raw `at` value to a 0..1 progress threshold
    /// given the file's timestamp format and the live track
    /// duration in seconds. A zero or unknown duration falls
    /// back to "treat as relative".
    pub fn progress_threshold(&self,
        fmt: &TimestampFormat, duration_s: f32,
    ) -> f32 {
        match fmt {
            TimestampFormat::Relative | TimestampFormat::Named(_) =>
                self.at.clamp(0.0, 1.0),
            TimestampFormat::TrackLength => {
                if duration_s > 0.1 { (self.at / duration_s).clamp(0.0, 1.0) }
                else { self.at.clamp(0.0, 1.0) }
            }
            TimestampFormat::Beats { total } => {
                let t = (*total).max(1) as f32;
                (self.at / t).clamp(0.0, 1.0)
            }
        }
    }
}

/// Static type of a variable. The parser rejects values that do
/// not fit the declared type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarType { Int, Float, String, Ident, Bool }

/// Visibility / access of a variable inside the level. All three
/// modifiers are cosmetic in the current engine (the parser does
/// not see multiple files) but are preserved verbatim so tooling
/// and documentation stay truthful to the author's intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarAccess { Public, Private, ReadOnly }

/// Concrete value a variable is bound to. Mirrors the field
/// value enum used by the parser.
#[derive(Clone, Debug)]
pub enum VarValue {
    Num(f32),
    Str(String),
    Ident(String),
    Bool(bool),
}

/// Single `var name : type = value [modifiers]` declaration.
#[derive(Clone, Debug)]
pub struct VarDecl {
    pub name:   String,
    pub ty:     VarType,
    pub value:  VarValue,
    pub access: VarAccess,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Emit    (ObstacleSpec),
    Wait    (u32),
    Trigger (TriggerSpec),
    /// Repeat `body` exactly `count` times in order. Any
    /// `LocalVars` declared inside are re-initialized for each
    /// iteration, matching the usual meaning of a "for i in
    /// 1..=count" loop.
    Repeat  { count: u32, body: Vec<Stmt> },
    /// Push a variable scope that lives until the enclosing
    /// `{ ... }` body ends, then unwinds automatically. Bindings
    /// shadow any global with the same name for the duration of
    /// the scope.
    LocalVars (Vec<VarDecl>),
}

/// Spinning direction for patterns that walk around the ring.
#[derive(Clone, Copy, Debug)]
pub enum SpinDir { Cw, Ccw }

/// Parity selector for `alternate`.
#[derive(Clone, Copy, Debug)]
pub enum Parity { Even, Odd }

/// Easing function for animated triggers (currently `zoom` and
/// the ramp portion of `speedwarp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anim {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bounce,
}

impl Anim {
    /// Apply the easing to a normalized progress value in 0..1.
    pub fn eval(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Anim::Linear    => t,
            Anim::EaseIn    => t * t,
            Anim::EaseOut   => 1.0 - (1.0 - t) * (1.0 - t),
            Anim::EaseInOut => {
                if t < 0.5 { 2.0 * t * t }
                else { 1.0 - (-2.0 * t + 2.0).powi(2) * 0.5 }
            }
            Anim::Bounce => {
                let u = 1.0 - t;
                1.0 - bounce_out(u)
            }
        }
    }
}

fn bounce_out(x: f32) -> f32 {
    let n1 = 7.5625f32;
    let d1 = 2.75f32;
    if x < 1.0 / d1 { n1 * x * x }
    else if x < 2.0 / d1 {
        let x = x - 1.5 / d1;
        n1 * x * x + 0.75
    } else if x < 2.5 / d1 {
        let x = x - 2.25 / d1;
        n1 * x * x + 0.9375
    } else {
        let x = x - 2.625 / d1;
        n1 * x * x + 0.984375
    }
}

/// Parsed formula describing a `CustomFormula` obstacle mask.
///
/// The body is pre-compiled into an expression tree by
/// [`crate::dsl::formula`] at parse time so the generator can
/// evaluate thousands of (step, slot) combinations cheaply.
#[derive(Clone, Debug)]
pub struct Formula {
    /// Raw source kept for diagnostics and `{:?}` display.
    pub source: String,
    /// Precompiled AST of the body expression.
    pub program: crate::dsl::formula::Expr,
}

/// Every obstacle pattern the DSL can spawn. Each variant is
/// guaranteed by the generator to leave at least one reachable
/// gap per step; structural invariants are checked once at
/// parse time so runtime code never has to.
#[derive(Clone, Debug)]
pub enum ObstacleSpec {
    Bar        { thickness_mult: f32 },
    DoubleBar  { spacing: u32, thickness_mult: f32 },
    Spiral     { dir: SpinDir, thickness_mult: f32, loops: u32 },
    Alternate  { parity: Parity, thickness_mult: f32 },
    Pinwheel   { spokes: u32, dir: SpinDir },
    Rain       { count: u32, thickness_mult: f32 },
    Custom     { mask: Vec<bool>, thickness_mult: f32 },
    Rainbow    { dir: SpinDir },
    Ladder     { rungs: u32 },
    Tunnel     { length: f32, lanes: u32 },
    Pot        { layers: u32 },

    /// Classic Super Hexagon staircase: `steps` back to back C
    /// walls whose gap rotates one slot per beat. Produces the
    /// "stepping down a spiral" look visible on the reference
    /// screenshots (Point, Hyper Mode).
    Staircase  { dir: SpinDir, steps: u32, thickness_mult: f32 },
    /// Long radial corridor with periodic lateral shifts
    /// ("turns"). The player spends `length` seconds committed
    /// to a lane before the gap swaps sides.
    Corridor   { length: f32, turns: u32, dir: SpinDir },
    /// Cubes made of four back to back bars rotated by one slot
    /// per layer. Reads on screen as a rotating rubik face.
    Cubes      { layers: u32, dir: SpinDir },
    /// Author defined pattern whose mask is computed by
    /// evaluating `formula` for every (step, slot) pair. Parser
    /// has already verified that every step has at least one
    /// gap; the generator can spawn freely.
    CustomFormula {
        formula:        Formula,
        steps:          u32,
        thickness_mult: f32,
    },
}

/// Trigger family. "Simple" triggers (Flip, Tilt, etc.) keep
/// their v1 single-argument form for brevity; the richer
/// triggers take a struct style field block in the DSL.
#[derive(Clone, Copy, Debug)]
pub enum TriggerSpec {
    Flip,
    Tilt(f32),
    Pulse,
    SpeedMult(f32),
    HueShift(f32),

    /// Multiply wall / rotation / cursor / music speed
    /// independently for `duration` seconds. A zero entry means
    /// "do not touch this axis".
    SpeedWarp {
        walls:       f32,
        rotation:    f32,
        cursor:      f32,
        music_scale: f32,
        duration:    f32,
    },
    /// Engage the post-process glitch effect. `strength` in
    /// 0..1 sets the magnitude; `duration` in seconds sets the
    /// tail.
    Glitch { strength: f32, duration: f32 },
    /// Hard screen shake. Stacks additively with `Glitch` and
    /// with the ambient close-call trauma.
    Shake  { strength: f32, duration: f32 },
    /// Animated camera zoom. `target` is the final scale
    /// (1.0 is neutral). Eases over `duration` seconds using
    /// the named easing.
    Zoom   { target: f32, anim: Anim, duration: f32 },
    /// Invert the left / right cursor input for `duration`
    /// seconds. The screen tint hints at the active state so
    /// the player is not just confused.
    Invert { duration: f32 },
    /// Periodic bright flash at `rate` Hz for `duration`
    /// seconds. Useful for BPM synchronized bridges; `rate 0`
    /// means "pulse once".
    Strobe { rate: f32, duration: f32 },
}