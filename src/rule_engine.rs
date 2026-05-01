//! Runtime rule engine.
//!
//! Takes the `LevelRules` baseline declared at file level and
//! mutates its state over time as the current section emits
//! `Stmt::Rule`, `Stmt::Revert`, `Stmt::Push`, and `Stmt::Pop`
//! instructions. Exposes a read only view of the currently
//! effective values through `effective_*` methods, which the
//! game simulation reads every frame.
//!
//! The engine is deliberately dumb: it knows how to merge rule
//! fields and maintain per category stacks, nothing more. All
//! the interesting semantics (what does a shield with 3 charges
//! actually do? what does vision.range affect?) live in the
//! game simulation and the renderer, which query this engine
//! through the `Effective` struct.
//!
//! # Safety
//!
//! * Stack depth on every category is capped at
//!   `MAX_RULE_STACK_DEPTH` so a runaway level cannot allocate
//!   arbitrary memory.
//! * A `Pop` on an empty stack is a no op, never a panic.
//! * `Revert(All)` resets every category and clears every
//!   stack, regardless of prior history.
//! * All numeric effective values pass through the same
//!   clamping logic used by the parser, so a bug in any of the
//!   above paths still cannot hand NaN or out of range numbers
//!   to the renderer.

use crate::dsl::ast::Stmt;
use crate::dsl::rules::{
    AbilityKind, AbilityRule, CursorRule, InputRule, LevelRules,
    MAX_RULE_STACK_DEPTH, RuleCategory, RuleSet, ScoreRule,
    SurvivalRule, VisionRule,
};

/// Mutable per category state. Each field is initialised from
/// the corresponding baseline at construction and updated in
/// place as rule statements fire.
#[derive(Clone, Debug)]
pub struct RuleEngine {
    baseline: LevelRules,

    ability:  AbilityRule,
    vision:   VisionRule,
    cursor:   CursorRule,
    survival: SurvivalRule,
    input:    InputRule,
    score:    ScoreRule,

    stack_ability:  Vec<AbilityRule>,
    stack_vision:   Vec<VisionRule>,
    stack_cursor:   Vec<CursorRule>,
    stack_survival: Vec<SurvivalRule>,
    stack_input:    Vec<InputRule>,
    stack_score:    Vec<ScoreRule>,
}

impl RuleEngine {
    /// Build an engine seeded with the level's file level rule
    /// baseline. `game::Game::new` calls this once per run.
    pub fn new(baseline: LevelRules) -> Self {
        RuleEngine {
            ability:  baseline.ability.clone(),
            vision:   baseline.vision.clone(),
            cursor:   baseline.cursor.clone(),
            survival: baseline.survival.clone(),
            input:    baseline.input.clone(),
            score:    baseline.score.clone(),
            baseline,
            stack_ability:  Vec::new(),
            stack_vision:   Vec::new(),
            stack_cursor:   Vec::new(),
            stack_survival: Vec::new(),
            stack_input:    Vec::new(),
            stack_score:    Vec::new(),
        }
    }

    /// Return the engine to a fresh copy of the baseline,
    /// throwing away every push and every in place merge.
    /// Called by `game::Game::restart` so a retry plays the
    /// level from the same starting state as the first attempt.
    pub fn reset(&mut self) {
        self.ability  = self.baseline.ability.clone();
        self.vision   = self.baseline.vision.clone();
        self.cursor   = self.baseline.cursor.clone();
        self.survival = self.baseline.survival.clone();
        self.input    = self.baseline.input.clone();
        self.score    = self.baseline.score.clone();
        self.stack_ability.clear();
        self.stack_vision.clear();
        self.stack_cursor.clear();
        self.stack_survival.clear();
        self.stack_input.clear();
        self.stack_score.clear();
    }

    /// Entry point used by the generator when it encounters a
    /// rule related statement. Returns quickly for every other
    /// variant so callers can forward the entire `Stmt`
    /// stream without a prior filter.
    pub fn apply(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Rule(rs) => self.apply_rule(rs.clone()),
            Stmt::Revert(cat) => self.apply_revert(*cat),
            Stmt::Push(cat) => self.apply_push(*cat),
            Stmt::Pop(cat)  => self.apply_pop(*cat),
            _ => {}
        }
    }

    fn apply_rule(&mut self, rs: RuleSet) {
        match rs {
            RuleSet::Ability(r)  => merge_ability(&mut self.ability, r),
            RuleSet::Vision(r)   => merge_vision(&mut self.vision, r),
            RuleSet::Cursor(r)   => merge_cursor(&mut self.cursor, r),
            RuleSet::Survival(r) => merge_survival(&mut self.survival, r),
            RuleSet::Input(r)    => merge_input(&mut self.input, r),
            RuleSet::Score(r)    => merge_score(&mut self.score, r),
        }
    }

    fn apply_revert(&mut self, cat: RuleCategory) {
        match cat {
            RuleCategory::Ability => {
                self.ability = self.baseline.ability.clone();
                self.stack_ability.clear();
            }
            RuleCategory::Vision => {
                self.vision = self.baseline.vision.clone();
                self.stack_vision.clear();
            }
            RuleCategory::Cursor => {
                self.cursor = self.baseline.cursor.clone();
                self.stack_cursor.clear();
            }
            RuleCategory::Survival => {
                self.survival = self.baseline.survival.clone();
                self.stack_survival.clear();
            }
            RuleCategory::Input => {
                self.input = self.baseline.input.clone();
                self.stack_input.clear();
            }
            RuleCategory::Score => {
                self.score = self.baseline.score.clone();
                self.stack_score.clear();
            }
            RuleCategory::All => {
                self.ability  = self.baseline.ability.clone();
                self.vision   = self.baseline.vision.clone();
                self.cursor   = self.baseline.cursor.clone();
                self.survival = self.baseline.survival.clone();
                self.input    = self.baseline.input.clone();
                self.score    = self.baseline.score.clone();
                self.stack_ability.clear();
                self.stack_vision.clear();
                self.stack_cursor.clear();
                self.stack_survival.clear();
                self.stack_input.clear();
                self.stack_score.clear();
            }
        }
    }

    fn apply_push(&mut self, cat: RuleCategory) {
        match cat {
            RuleCategory::Ability => {
                if self.stack_ability.len() < MAX_RULE_STACK_DEPTH {
                    self.stack_ability.push(self.ability.clone());
                }
            }
            RuleCategory::Vision => {
                if self.stack_vision.len() < MAX_RULE_STACK_DEPTH {
                    self.stack_vision.push(self.vision.clone());
                }
            }
            RuleCategory::Cursor => {
                if self.stack_cursor.len() < MAX_RULE_STACK_DEPTH {
                    self.stack_cursor.push(self.cursor.clone());
                }
            }
            RuleCategory::Survival => {
                if self.stack_survival.len() < MAX_RULE_STACK_DEPTH {
                    self.stack_survival.push(self.survival.clone());
                }
            }
            RuleCategory::Input => {
                if self.stack_input.len() < MAX_RULE_STACK_DEPTH {
                    self.stack_input.push(self.input.clone());
                }
            }
            RuleCategory::Score => {
                if self.stack_score.len() < MAX_RULE_STACK_DEPTH {
                    self.stack_score.push(self.score.clone());
                }
            }
            RuleCategory::All => {
                // Parser already rejects `push all`, but if it
                // somehow reaches us we silently do nothing
                // rather than risk inconsistent stacks.
            }
        }
    }

    fn apply_pop(&mut self, cat: RuleCategory) {
        match cat {
            RuleCategory::Ability => {
                if let Some(v) = self.stack_ability.pop() { self.ability = v; }
            }
            RuleCategory::Vision => {
                if let Some(v) = self.stack_vision.pop() { self.vision = v; }
            }
            RuleCategory::Cursor => {
                if let Some(v) = self.stack_cursor.pop() { self.cursor = v; }
            }
            RuleCategory::Survival => {
                if let Some(v) = self.stack_survival.pop() { self.survival = v; }
            }
            RuleCategory::Input => {
                if let Some(v) = self.stack_input.pop() { self.input = v; }
            }
            RuleCategory::Score => {
                if let Some(v) = self.stack_score.pop() { self.score = v; }
            }
            RuleCategory::All => {}
        }
    }

    /// Flatten the current rule state into concrete numbers
    /// ready for the simulation. Any field the level never set
    /// falls back to the engine's own default so the caller
    /// never has to handle `None`.
    pub fn effective(&self) -> Effective {
        Effective {
            ability_kind: self.ability.kind.unwrap_or(AbilityKind::None),
            shield_charges: self.ability.charges.unwrap_or(1),
            shield_recharge: self.ability.recharge.unwrap_or(12.5),
            shield_invuln: self.ability.invuln.unwrap_or(0.0),
            dash_cooldown: self.ability.cooldown.unwrap_or(1.3),
            dash_slots: self.ability.slots_per_dash.unwrap_or(1),
            slowmo_factor: self.ability.slowmo_factor.unwrap_or(0.45),
            slowmo_cap: self.ability.slowmo_cap.unwrap_or(3.0),
            slowmo_recover: self.ability.slowmo_recover.unwrap_or(6.0),

            vision_range: self.vision.range.unwrap_or(6.0),
            vision_fog_near: self.vision.fog_near.unwrap_or(4.0),
            vision_fog_far:  self.vision.fog_far.unwrap_or(5.0),
            vision_strobe: self.vision.strobe.unwrap_or(false),
            vision_strobe_rate: self.vision.strobe_rate.unwrap_or(4.0),
            vision_blind_duration: self.vision.blind_duration.unwrap_or(0.0),
            vision_blind_frequency: self.vision.blind_frequency.unwrap_or(0.0),
            vision_hide_camera_indicator:
                self.vision.hide_camera_indicator.unwrap_or(false),

            cursor_speed_mult: self.cursor.speed_mult.unwrap_or(1.0),
            cursor_width_mult: self.cursor.width_mult.unwrap_or(1.0),
            cursor_count: self.cursor.count.unwrap_or(1),
            cursor_angular_offset:
                self.cursor.angular_offset.unwrap_or(std::f32::consts::PI),
            cursor_centripetal_drift:
                self.cursor.centripetal_drift.unwrap_or(0.0),

            survival_lives: self.survival.lives.unwrap_or(1),
            survival_soft_death: self.survival.soft_death.unwrap_or(false),
            survival_pushback: self.survival.pushback_seconds.unwrap_or(0.5),
            survival_invuln: self.survival.invuln_after_hit.unwrap_or(0.0),
            survival_checkpoints:
                self.survival.checkpoints_enabled.unwrap_or(false),

            input_delay_ms: self.input.delay_ms.unwrap_or(0),
            input_discrete: self.input.discrete.unwrap_or(false),
            input_noise: self.input.noise.unwrap_or(0.0),
            input_inverted: self.input.inverted.unwrap_or(false),

            score_multiplier: self.score.multiplier.unwrap_or(1.0),
            score_close_call_bonus: self.score.close_call_bonus.unwrap_or(50),
            score_survival_per_second:
                self.score.survival_per_second.unwrap_or(10),
        }
    }

    /// Ability override declared by the currently active rule
    /// state, if any.
    ///
    /// Returns `Some(kind)` when the level (either its file
    /// level directive or a later `rule ability { kind = ... }`
    /// statement) has explicitly forced an ability, including
    /// the legitimate "force no ability" case of
    /// `Some(AbilityKind::None)`. Returns `None` when no
    /// override is in effect, meaning the player's configured
    /// choice from the options screen should be used.
    ///
    /// Kept as a separate method instead of reading
    /// `effective().ability_kind` directly because
    /// `effective()` collapses "no override" and "explicit
    /// none" into the same `AbilityKind::None` value, which
    /// the game cannot distinguish from each other without
    /// this helper.
    pub fn ability_override_kind(&self) -> Option<AbilityKind> {
        self.ability.kind
    }
}

/// Flat view over the currently effective rule values. Returned
/// by `RuleEngine::effective` and consumed by the simulation
/// and the renderer.
#[derive(Clone, Debug)]
pub struct Effective {
    pub ability_kind: AbilityKind,
    pub shield_charges: u32,
    pub shield_recharge: f32,
    pub shield_invuln: f32,
    pub dash_cooldown: f32,
    pub dash_slots: u32,
    pub slowmo_factor: f32,
    pub slowmo_cap: f32,
    pub slowmo_recover: f32,

    pub vision_range: f32,
    pub vision_fog_near: f32,
    pub vision_fog_far: f32,
    pub vision_strobe: bool,
    pub vision_strobe_rate: f32,
    pub vision_blind_duration: f32,
    pub vision_blind_frequency: f32,
    pub vision_hide_camera_indicator: bool,

    pub cursor_speed_mult: f32,
    pub cursor_width_mult: f32,
    pub cursor_count: u32,
    pub cursor_angular_offset: f32,
    pub cursor_centripetal_drift: f32,

    pub survival_lives: u32,
    pub survival_soft_death: bool,
    pub survival_pushback: f32,
    pub survival_invuln: f32,
    pub survival_checkpoints: bool,

    pub input_delay_ms: u32,
    pub input_discrete: bool,
    pub input_noise: f32,
    pub input_inverted: bool,

    pub score_multiplier: f32,
    pub score_close_call_bonus: u32,
    pub score_survival_per_second: u32,
}

// ---- merge helpers (copies of the ones in dsl/rules.rs kept
//      local so the runtime engine does not take a dependency
//      on the parser's merge API beyond the `RuleSet` enum) ----

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
    if src.blind_duration.is_some() {
        dst.blind_duration = src.blind_duration;
    }
    if src.blind_frequency.is_some() {
        dst.blind_frequency = src.blind_frequency;
    }
    if src.hide_camera_indicator.is_some() {
        dst.hide_camera_indicator = src.hide_camera_indicator;
    }
}
fn merge_cursor(dst: &mut CursorRule, src: CursorRule) {
    if src.speed_mult.is_some() { dst.speed_mult = src.speed_mult; }
    if src.width_mult.is_some() { dst.width_mult = src.width_mult; }
    if src.count.is_some() { dst.count = src.count; }
    if src.angular_offset.is_some() {
        dst.angular_offset = src.angular_offset;
    }
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