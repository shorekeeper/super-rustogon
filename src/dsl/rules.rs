//! Authored gameplay rules: abilities, vision, cursor, survival,
//! input, and score. Each category is a set of optional fields.
//! Unset fields inherit from the enclosing scope (the file level
//! directive, the engine default, or the user's config in the
//! case of abilities).
//!
//! The parser fills a `RuleSet` when it sees a `rule` statement
//! or a file level `#[category ...]` directive. The expander
//! pushes these onto a per category stack. The runtime engine
//! (`RuleEngine`) exposes helpers that flatten the stack into
//! effective values every frame.
//!
//! # Safety
//!
//! Every numeric field passes through a per field clamp with
//! deliberately conservative bounds. Unknown categories or
//! unknown fields are parse errors, not silent misses. Every
//! category has a built in default that makes sense as the
//! "rule set does nothing" state, so partially set fields never
//! produce degenerate engine behavior.

use crate::dsl::safety;

/// Known gameplay ability kinds. Mirrors `crate::config::Ability`
/// but belongs to the DSL layer so a level author can force a
/// specific ability regardless of the user's config. Conversion
/// is trivial and happens when the rule is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityKind { None, Dash, Shield, SlowMo }

impl AbilityKind {
    pub fn from_atom(atom: &str) -> Option<Self> {
        match atom {
            "none"   => Some(AbilityKind::None),
            "dash"   => Some(AbilityKind::Dash),
            "shield" => Some(AbilityKind::Shield),
            "slowmo" | "slow_mo" => Some(AbilityKind::SlowMo),
            _ => None,
        }
    }
    pub fn to_atom(self) -> &'static str {
        match self {
            AbilityKind::None   => "none",
            AbilityKind::Dash   => "dash",
            AbilityKind::Shield => "shield",
            AbilityKind::SlowMo => "slowmo",
        }
    }
}

/// Ability category parameters. All fields optional so partial
/// updates keep the rest of the category as it was.
#[derive(Clone, Debug, Default)]
pub struct AbilityRule {
    /// When `Some`, force this ability regardless of config.
    pub kind: Option<AbilityKind>,
    /// Shield: how many hits can be absorbed before it runs out.
    pub charges: Option<u32>,
    /// Shield: seconds between charge regenerations.
    pub recharge: Option<f32>,
    /// Shield / Dash: seconds of invulnerability after use.
    pub invuln: Option<f32>,
    /// Dash: cooldown between consecutive dashes in seconds.
    pub cooldown: Option<f32>,
    /// Dash: how many slots the player jumps in one dash.
    pub slots_per_dash: Option<u32>,
    /// Slowmo: time scale applied while ability is active.
    pub slowmo_factor: Option<f32>,
    /// Slowmo: maximum continuous seconds of use.
    pub slowmo_cap: Option<f32>,
    /// Slowmo: seconds to recharge to full after depletion.
    pub slowmo_recover: Option<f32>,
}

impl AbilityRule {
    /// Apply numeric clamps. Called by the parser after reading
    /// a rule so the AST never carries out of range values.
    pub fn clamp(&mut self) {
        if let Some(v) = self.charges.as_mut() { *v = (*v).min(10); }
        if let Some(v) = self.recharge.as_mut() { *v = v.clamp(0.1, 60.0); }
        if let Some(v) = self.invuln.as_mut() { *v = v.clamp(0.0, 3.0); }
        if let Some(v) = self.cooldown.as_mut() { *v = v.clamp(0.0, 30.0); }
        if let Some(v) = self.slots_per_dash.as_mut() {
            *v = (*v).clamp(1, 3);
        }
        if let Some(v) = self.slowmo_factor.as_mut() {
            *v = v.clamp(0.1, 1.0);
        }
        if let Some(v) = self.slowmo_cap.as_mut() {
            *v = v.clamp(0.1, 30.0);
        }
        if let Some(v) = self.slowmo_recover.as_mut() {
            *v = v.clamp(0.1, 30.0);
        }
    }
}

/// Vision category parameters. Control how much of the
/// playfield the player can see at a given moment. None of
/// these change collision or physics, only rendering.
#[derive(Clone, Debug, Default)]
pub struct VisionRule {
    /// Distance beyond which walls are fully hidden. In the
    /// same units as `WALL_SPAWN_R`, so 5.0 is the full ring
    /// and 0.0 is "only things touching the player".
    pub range: Option<f32>,
    /// Start of the fog gradient. Walls closer than this stay
    /// fully lit.
    pub fog_near: Option<f32>,
    /// End of the fog gradient. Walls further than this are
    /// fully hidden regardless of `range`.
    pub fog_far: Option<f32>,
    /// Strobe mode: walls are visible only while the strobe
    /// pulse is bright, invisible during dark phases.
    pub strobe: Option<bool>,
    /// Strobe pulse rate in Hz. Only meaningful when
    /// `strobe` is `Some(true)`.
    pub strobe_rate: Option<f32>,
    /// Duration of periodic full blackouts in seconds.
    pub blind_duration: Option<f32>,
    /// Frequency of periodic blackouts in Hz. Zero means off.
    pub blind_frequency: Option<f32>,
    /// Hide the subtle camera spin direction indicator. Useful
    /// for "blind" challenges where the player has to predict
    /// rotation.
    pub hide_camera_indicator: Option<bool>,
}

impl VisionRule {
    pub fn clamp(&mut self) {
        if let Some(v) = self.range.as_mut() { *v = v.clamp(0.0, 6.0); }
        if let Some(v) = self.fog_near.as_mut() { *v = v.clamp(0.0, 6.0); }
        if let Some(v) = self.fog_far.as_mut() { *v = v.clamp(0.0, 6.0); }
        if let Some(v) = self.strobe_rate.as_mut() {
            *v = v.clamp(0.5, 30.0);
        }
        if let Some(v) = self.blind_duration.as_mut() {
            *v = v.clamp(0.0, 2.0);
        }
        if let Some(v) = self.blind_frequency.as_mut() {
            *v = v.clamp(0.0, 10.0);
        }
    }
}

/// Cursor category parameters. Override geometry and behavior
/// of the player cursor. The most impactful field is `count`:
/// setting it to 2 spawns a second cursor locked at a fixed
/// angular offset, radically changing how a section plays.
#[derive(Clone, Debug, Default)]
pub struct CursorRule {
    /// Multiplier on base cursor rotation speed.
    pub speed_mult: Option<f32>,
    /// Multiplier on cursor hitbox width. Smaller values make
    /// the player thinner, larger ones make it fatter.
    pub width_mult: Option<f32>,
    /// Number of cursors. 1 is the default single cursor. 2
    /// adds a second cursor locked at `angular_offset` radians
    /// from the first. Higher counts are rejected by the
    /// clamp.
    pub count: Option<u32>,
    /// Angular separation between multi cursors, in radians.
    /// Ignored when `count` is 1.
    pub angular_offset: Option<f32>,
    /// Constant angular velocity applied to the cursor as if
    /// the player were weakly turning. Positive values pull
    /// clockwise, negative counter clockwise. Zero leaves
    /// control uncontested.
    pub centripetal_drift: Option<f32>,
}

impl CursorRule {
    pub fn clamp(&mut self) {
        if let Some(v) = self.speed_mult.as_mut() {
            *v = v.clamp(0.1, 5.0);
        }
        if let Some(v) = self.width_mult.as_mut() {
            *v = v.clamp(0.25, 3.0);
        }
        if let Some(v) = self.count.as_mut() { *v = (*v).clamp(1, 2); }
        if let Some(v) = self.angular_offset.as_mut() {
            *v = v.clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
        }
        if let Some(v) = self.centripetal_drift.as_mut() {
            *v = v.clamp(-5.0, 5.0);
        }
    }
}

/// Survival category parameters. Control what happens on
/// collision and how forgiving the level is overall.
#[derive(Clone, Debug, Default)]
pub struct SurvivalRule {
    /// Number of lives in the run. 1 is classic. 0 is a valid
    /// "practice / no death" mode where collision is ignored.
    pub lives: Option<u32>,
    /// When true, hitting a wall respawns the player `pushback`
    /// seconds earlier in the timeline instead of ending the
    /// run.
    pub soft_death: Option<bool>,
    /// Seconds of rewind on a soft death.
    pub pushback_seconds: Option<f32>,
    /// Seconds of invulnerability after a survived hit.
    pub invuln_after_hit: Option<f32>,
    /// Enable section checkpoints: respawning restarts at the
    /// latest section the player entered, not at zero.
    pub checkpoints_enabled: Option<bool>,
}

impl SurvivalRule {
    pub fn clamp(&mut self) {
        if let Some(v) = self.lives.as_mut() { *v = (*v).clamp(0, 10); }
        if let Some(v) = self.pushback_seconds.as_mut() {
            *v = v.clamp(0.0, 5.0);
        }
        if let Some(v) = self.invuln_after_hit.as_mut() {
            *v = v.clamp(0.0, 3.0);
        }
    }
}

/// Input category parameters. Distort how player input reaches
/// the simulation. Useful for themed challenges.
#[derive(Clone, Debug, Default)]
pub struct InputRule {
    /// Added artificial input delay in milliseconds.
    pub delay_ms: Option<u32>,
    /// When true, cursor snaps to slot boundaries instead of
    /// moving smoothly.
    pub discrete: Option<bool>,
    /// Per frame random angular jitter. 0 disables.
    pub noise: Option<f32>,
    /// Force left/right inversion regardless of DSL triggers.
    pub inverted: Option<bool>,
}

impl InputRule {
    pub fn clamp(&mut self) {
        if let Some(v) = self.delay_ms.as_mut() { *v = (*v).min(500); }
        if let Some(v) = self.noise.as_mut() { *v = v.clamp(0.0, 1.0); }
    }
}

/// Score category parameters. The scoring system is planned
/// but not yet implemented; these values are stored so levels
/// can already annotate their "intended" scoring regions.
#[derive(Clone, Debug, Default)]
pub struct ScoreRule {
    pub multiplier: Option<f32>,
    pub close_call_bonus: Option<u32>,
    pub survival_per_second: Option<u32>,
}

impl ScoreRule {
    pub fn clamp(&mut self) {
        if let Some(v) = self.multiplier.as_mut() {
            *v = v.clamp(0.0, 10.0);
        }
        if let Some(v) = self.close_call_bonus.as_mut() {
            *v = (*v).min(10_000);
        }
        if let Some(v) = self.survival_per_second.as_mut() {
            *v = (*v).min(10_000);
        }
    }
}

/// One rule set, tagging which category the payload belongs to.
/// Stored in `Stmt::Rule` and inside meta statements.
#[derive(Clone, Debug)]
pub enum RuleSet {
    Ability(AbilityRule),
    Vision(VisionRule),
    Cursor(CursorRule),
    Survival(SurvivalRule),
    Input(InputRule),
    Score(ScoreRule),
}

impl RuleSet {
    pub fn category(&self) -> RuleCategory {
        match self {
            RuleSet::Ability(_)  => RuleCategory::Ability,
            RuleSet::Vision(_)   => RuleCategory::Vision,
            RuleSet::Cursor(_)   => RuleCategory::Cursor,
            RuleSet::Survival(_) => RuleCategory::Survival,
            RuleSet::Input(_)    => RuleCategory::Input,
            RuleSet::Score(_)    => RuleCategory::Score,
        }
    }
}

/// Category selector for `revert`, `push`, and `pop`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleCategory {
    Ability, Vision, Cursor, Survival, Input, Score, All,
}

impl RuleCategory {
    pub fn from_ident(s: &str) -> Option<Self> {
        match s {
            "ability"  => Some(RuleCategory::Ability),
            "vision"   => Some(RuleCategory::Vision),
            "cursor"   => Some(RuleCategory::Cursor),
            "survival" => Some(RuleCategory::Survival),
            "input"    => Some(RuleCategory::Input),
            "score"    => Some(RuleCategory::Score),
            "all"      => Some(RuleCategory::All),
            _ => None,
        }
    }
}

/// Collection of file level rule declarations attached to the
/// `LevelAst`. These are the defaults against which all
/// section level `rule` statements apply; `revert` in a
/// section returns to this baseline.
#[derive(Clone, Debug, Default)]
pub struct LevelRules {
    pub ability:  AbilityRule,
    pub vision:   VisionRule,
    pub cursor:   CursorRule,
    pub survival: SurvivalRule,
    pub input:    InputRule,
    pub score:    ScoreRule,
}

impl LevelRules {
    /// Merge a rule set into the baseline by overwriting any
    /// `Some` field. `None` fields keep their previous value.
    /// Used when several `#[ability ...]` directives appear in
    /// one file; later ones refine earlier ones.
    pub fn merge(&mut self, rs: RuleSet) {
        match rs {
            RuleSet::Ability(r)  => merge_ability(&mut self.ability, r),
            RuleSet::Vision(r)   => merge_vision(&mut self.vision, r),
            RuleSet::Cursor(r)   => merge_cursor(&mut self.cursor, r),
            RuleSet::Survival(r) => merge_survival(&mut self.survival, r),
            RuleSet::Input(r)    => merge_input(&mut self.input, r),
            RuleSet::Score(r)    => merge_score(&mut self.score, r),
        }
    }
}

fn merge_ability(dst: &mut AbilityRule, src: AbilityRule) {
    if src.kind.is_some() { dst.kind = src.kind; }
    if src.charges.is_some() { dst.charges = src.charges; }
    if src.recharge.is_some() { dst.recharge = src.recharge; }
    if src.invuln.is_some() { dst.invuln = src.invuln; }
    if src.cooldown.is_some() { dst.cooldown = src.cooldown; }
    if src.slots_per_dash.is_some() { dst.slots_per_dash = src.slots_per_dash; }
    if src.slowmo_factor.is_some() { dst.slowmo_factor = src.slowmo_factor; }
    if src.slowmo_cap.is_some() { dst.slowmo_cap = src.slowmo_cap; }
    if src.slowmo_recover.is_some() { dst.slowmo_recover = src.slowmo_recover; }
}
fn merge_vision(dst: &mut VisionRule, src: VisionRule) {
    if src.range.is_some() { dst.range = src.range; }
    if src.fog_near.is_some() { dst.fog_near = src.fog_near; }
    if src.fog_far.is_some() { dst.fog_far = src.fog_far; }
    if src.strobe.is_some() { dst.strobe = src.strobe; }
    if src.strobe_rate.is_some() { dst.strobe_rate = src.strobe_rate; }
    if src.blind_duration.is_some() { dst.blind_duration = src.blind_duration; }
    if src.blind_frequency.is_some() { dst.blind_frequency = src.blind_frequency; }
    if src.hide_camera_indicator.is_some() {
        dst.hide_camera_indicator = src.hide_camera_indicator;
    }
}
fn merge_cursor(dst: &mut CursorRule, src: CursorRule) {
    if src.speed_mult.is_some() { dst.speed_mult = src.speed_mult; }
    if src.width_mult.is_some() { dst.width_mult = src.width_mult; }
    if src.count.is_some() { dst.count = src.count; }
    if src.angular_offset.is_some() { dst.angular_offset = src.angular_offset; }
    if src.centripetal_drift.is_some() {
        dst.centripetal_drift = src.centripetal_drift;
    }
}
fn merge_survival(dst: &mut SurvivalRule, src: SurvivalRule) {
    if src.lives.is_some() { dst.lives = src.lives; }
    if src.soft_death.is_some() { dst.soft_death = src.soft_death; }
    if src.pushback_seconds.is_some() {
        dst.pushback_seconds = src.pushback_seconds;
    }
    if src.invuln_after_hit.is_some() {
        dst.invuln_after_hit = src.invuln_after_hit;
    }
    if src.checkpoints_enabled.is_some() {
        dst.checkpoints_enabled = src.checkpoints_enabled;
    }
}
fn merge_input(dst: &mut InputRule, src: InputRule) {
    if src.delay_ms.is_some() { dst.delay_ms = src.delay_ms; }
    if src.discrete.is_some() { dst.discrete = src.discrete; }
    if src.noise.is_some() { dst.noise = src.noise; }
    if src.inverted.is_some() { dst.inverted = src.inverted; }
}
fn merge_score(dst: &mut ScoreRule, src: ScoreRule) {
    if src.multiplier.is_some() { dst.multiplier = src.multiplier; }
    if src.close_call_bonus.is_some() {
        dst.close_call_bonus = src.close_call_bonus;
    }
    if src.survival_per_second.is_some() {
        dst.survival_per_second = src.survival_per_second;
    }
}

/// Maximum depth of the per category rule stack. Prevents a
/// pathological level from pushing thousands of snapshots and
/// running the engine out of memory.
pub const MAX_RULE_STACK_DEPTH: usize = 16;

/// Guard called by the parser and the expander when growing
/// a rule stack. Enforces `MAX_RULE_STACK_DEPTH` and converts
/// overflow into a readable error.
pub fn guard_stack_push(current_depth: usize) -> Result<(), String> {
    if current_depth + 1 > MAX_RULE_STACK_DEPTH {
        return Err(format!(
            "rule stack depth exceeded {} (too many push without pop)",
            MAX_RULE_STACK_DEPTH));
    }
    let _ = safety::MAX_PARSE_DURATION_MS;
    Ok(())
}