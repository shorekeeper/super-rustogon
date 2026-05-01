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
use crate::dsl::rules::{LevelRules, RuleCategory, RuleSet};

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
    pub start_from_seconds: f32,
    /// Debug directive: when `true`, the simulation never
    /// kills the player on collision. Close-call detection,
    /// particles, shake and all other "you almost died"
    /// visuals still fire, but the run does not end. Intended
    /// for authoring sessions where the author wants to scrub
    /// through their level without dying on every mistake.
    ///
    /// Toggled by the `#[ignore_collisions]` file directive.
    /// Defaults to `false` so every normal level plays the
    /// same way it always did.
    pub ignore_collisions: bool,
    /// Baseline gameplay rules declared at file level via
    /// `#[ability ...]`, `#[vision ...]`, etc. Section level
    /// `rule` statements stack on top of these; `revert` inside
    /// a section returns to the values stored here.
    pub rules: LevelRules,
    /// User post shader declarations, in source order. A
    /// `PostShader` trigger references one of these entries
    /// by index.
    pub shaders: Vec<ShaderDecl>,
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
            ignore_collisions: false,
            rules:              LevelRules::default(),
            shaders:            Vec::new(),
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
    /// Repeat `body` exactly `count` times in order.
    Repeat  { count: u32, body: Vec<Stmt> },
    /// Push a variable scope that lives until the enclosing
    /// `{ ... }` body ends.
    LocalVars (Vec<VarDecl>),
    /// Apply a rule set in place, overwriting the matching
    /// category's current state with the fields this rule
    /// contains. Fields left at `None` keep their prior value.
    Rule(RuleSet),
    /// Revert one category (or all of them) to the file level
    /// baseline stored in `LevelAst::rules`. Clears any pushed
    /// snapshots along the way.
    Revert(RuleCategory),
    /// Save the current state of one category onto its stack.
    /// A later `Pop(same_category)` restores exactly this
    /// state, regardless of intervening `rule` statements.
    Push(RuleCategory),
    /// Restore the most recently pushed state for one
    /// category. No-op when the stack is empty.
    Pop(RuleCategory),
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

/// One declaration of a user post-process shader in a level.
/// The body source lives in a separate file; the AST carries
/// only the name and the path. The main loop hands the path
/// to the shader sandbox at runtime when a `PostShader`
/// trigger activates this slot.
#[derive(Clone, Debug)]
pub struct ShaderDecl {
    pub name: String,
    pub path: String,
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

/// Trigger family. Every trigger variant now carries a
/// `duration` so the simulation can return to neutral state
/// automatically once the authored time window elapses. The
/// older variants that did not have a duration (`Tilt`,
/// `SpeedMult`, `HueShift`) gain the field too; legacy parser
/// paths that do not specify it fall back to reasonable
/// defaults documented at the parser level.
///
/// `SpeedWarp` axes are now `Option<f32>`: `None` means "do
/// not touch this axis", `Some(0.0)` means "force this axis
/// to zero", and any positive value is the target multiplier.
/// This resolves the long standing ambiguity where `0.0`
/// meant both "neutral" and "halt".
///
/// All variants are still `Copy` because `Option<f32>` is
/// `Copy` whenever its payload is. This preserves the zero
/// cost pass by value semantics the rest of the engine relies
/// on.
#[derive(Clone, Copy, Debug)]
pub enum TriggerSpec {
    /// Instantaneous camera flip. No duration because the flip
    /// itself is a discrete event; the camera simply reverses
    /// direction and keeps spinning.
    Flip,

    /// Short background pulse. The visual duration is hard
    /// coded on the generator side (see `PULSE_DURATION`) so
    /// this variant stays parameter-less.
    Pulse,

    /// Camera tilt on up to three axes.
    ///
    /// `angle` is the Z-axis roll (plain 2D rotation of the
    /// screen). `pitch` tilts the scene forward / backward as
    /// if viewed from above at an angle. `yaw` tilts it
    /// sideways. All three are in degrees and clamped by the
    /// parser to `±30`.
    ///
    /// `duration` is `Option<f32>`:
    ///
    /// * `None` means the tilt holds indefinitely (sticky).
    ///   Another `Tilt` trigger can override it, otherwise it
    ///   persists until the level ends. This matches legacy
    ///   pre-perspective behaviour.
    /// * `Some(seconds)` makes the tilt self-cancel.
    ///
    /// Perspective is applied globally inside the vertex
    /// shader, so a tilted frame skews the entire display,
    /// including HUD overlays and editor UI drawn through the
    /// same pipeline. This is intentional and gives `:tilt` a
    /// visible, unmistakable effect even on a spinning
    /// camera where a pure Z-roll would be swallowed by the
    /// ongoing rotation.
    Tilt {
        angle:    f32,
        pitch:    f32,
        yaw:      f32,
        duration: Option<f32>,
    },

    /// Multiply wall travel speed by `factor` for `duration`
    /// seconds. Historically `duration` was a fixed constant
    /// inside the generator; exposing it here lets authors
    /// decide how long a burst lasts.
    SpeedMult { factor: f32, duration: f32 },

    /// Drift the palette hue at `rate` radians per second for
    /// `duration` seconds. Same rationale as `SpeedMult` for
    /// the new explicit duration field.
    HueShift { rate: f32, duration: f32 },

    /// Multiply wall / rotation / cursor / music speed
    /// independently for `duration` seconds. Each axis is
    /// `Option`:
    ///
    /// * `None` leaves that channel untouched, so a pipe like
    ///   `:speedwarp { walls = 1.5 } |> :speedwarp { rotation
    ///   = 1.2 }` independently scales walls and rotation.
    /// * `Some(0.0)` is a legitimate "halt this axis" request,
    ///   which the old schema could not express.
    /// * `Some(v)` for any other `v` is the target multiplier.
    ///
    /// The generator maintains a separate timer per axis so
    /// axes stacked from several piped triggers each respect
    /// their own duration rather than having the latest
    /// duration overwrite the earlier one.
    SpeedWarp {
        walls:       Option<f32>,
        rotation:    Option<f32>,
        cursor:      Option<f32>,
        music_scale: Option<f32>,
        duration:    f32,
    },

    /// Engage the post-process glitch effect. `strength` in
    /// 0..1 sets the magnitude; `duration` in seconds sets the
    /// tail.
    Glitch { strength: f32, duration: f32 },

    /// Hard screen shake. Stacks with other shake requests
    /// using a "maximum wins" policy in the generator, so back
    /// to back `:shake` triggers reinforce each other rather
    /// than cancel each other out.
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

    /// Continuous camera spin added to the gameplay camera
    /// rotation. `rate` is radians per second (positive is CW
    /// in game coords). Stops cleanly when the timer elapses.
    Spin { rate: f32, duration: f32 },

    /// Per onset trauma bounce. While active, every detected
    /// audio onset injects `amplitude` of trauma into the screen
    /// shake accumulator. Gives JSAB style beat impact without
    /// requiring a shake trigger on every beat.
    Bounce { amplitude: f32, duration: f32 },

    /// Freeze wall motion for `duration` seconds. Input, audio
    /// and camera keep moving; only the radial wall velocity is
    /// held at zero. Great for breath moments before drops.
    Freeze { duration: f32 },

    /// Animated zoom punch. Ramps zoom up by `strength` over
    /// the first half of `duration` and back to neutral over
    /// the second half. Distinct from `Zoom` because it is a
    /// one shot impulse with a built in ease; authors do not
    /// have to schedule a return zoom.
    ZoomPunch { strength: f32, duration: f32 },

    /// Invert the final framebuffer colors for `duration`
    /// seconds. Separate from `Invert` which swaps input.
    InvertColors { duration: f32 },

    /// Desaturate the final framebuffer. `strength` in 0..1,
    /// 1.0 is full grayscale.
    Grayscale { strength: f32, duration: f32 },

    /// Radial shockwave displacement. A ring of pixel offsets
    /// expands from screen center over the duration, giving a
    /// recognizable punch effect on drops.
    Shockwave { strength: f32, duration: f32 },

    /// Radial fog. `near` and `far` are normalized screen
    /// radii 0..1. Pixels beyond `far` fade toward black while
    /// pixels inside `near` stay untouched.
    Fog { near: f32, far: f32, duration: f32 },

    /// Sobel style edge outlining across the whole image.
    /// `thickness` scales the additive contribution.
    Outline { thickness: f32, duration: f32 },

    /// Particle explosion out of the playfield center.
    /// `strength` scales count and speed. Fires once per
    /// trigger regardless of duration; duration is reserved
    /// for future extensions that might sustain the effect.
    Centerburst { strength: f32, duration: f32 },

    /// Series of expanding pulse rings. Rings are spaced
    /// evenly across `duration`. Reads as a multi wave
    /// emanation from the center.
    Ringburst { count: u32, duration: f32 },

    /// Composite impact trigger. Fires zoom punch, shake,
    /// shockwave and a brief flash at once. Single authored
    /// call per drop, full JSAB bass drop aesthetic.
    Bassdrop { strength: f32, duration: f32 },

    /// Activate a user supplied post process shader declared
    /// in the level's `shader` block. `slot` is the index
    /// into `LevelAst::shaders`; values outside that range
    /// are a no op at runtime. `p` carries up to four scalar
    /// parameters that the shader receives through its push
    /// constant block in declaration order.
    PostShader { slot: u8, p: [f32; 4] },

    /// Revert the post pipeline to the engine's built in
    /// shader. Paired with `PostShader`. Equivalent to
    /// issuing `PostShader` with a slot whose shader file is
    /// the default, but cheaper and explicit.
    PostShaderOff,

    /// Morph the playfield's polygon shape to `sides` over
    /// `duration` seconds. Integer target in 3..=12 clamped
    /// at parse time.
    ///
    /// Walls already in flight keep the angular positions they
    /// were spawned with, so a morph never teleports existing
    /// threats into new lanes. Newly spawned walls use the
    /// target `sides` immediately, so the slot count available
    /// to the generator updates at the instant the trigger
    /// fires, not at the end of the visual ease.
    Morph { sides: u32, duration: f32 },
}