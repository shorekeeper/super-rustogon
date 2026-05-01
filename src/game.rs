//! Game state, simulation, per frame geometry.
//!
//! Director / pattern table from the previous version is gone.
//! Walls, triggers, camera effects and runtime difficulty are now
//! all driven by a `crate::gend::Generator` built from the level's
//! DSL AST. The visual pipeline below is otherwise unchanged.
//!
//! This revision layers additional visual feedback and mechanics
//! on top of the original director / DSL-driven generator:
//!
//! * **Dash / Shield / Slow-mo abilities** (picked in the options
//!   screen and read from Config).
//! * **Close-call detection** that emits particles and pushes
//!   trauma into the screen shake accumulator.
//! * **Predictive highlight** of the wall the player would hit.
//! * **Pulse echo** rings emitted on every audio onset.
//! * **Shatter death** with a two-phase Dying animation: a short
//!   slow-motion tail followed by a hard freeze.
//! * **Trail afterimages** of the cursor when turning fast.
//! * **Squash and stretch** of the cursor triangle, driven by
//!   angular velocity.
//! * **Replay ghost**: the last two seconds of the player's
//!   trajectory are drawn as a fading arc once the run ends.
//! * **Milestones** pulse time-survived text at 10, 30, 60, 90,
//!   120, 180, 240 seconds.
//! * **Trigger visualizations**: a full-screen white flash on
//!   Flip, subtle background parallax from Tilt, radial speed
//!   lines under SpeedMult, a hue-tinted rim under HueShift.
//! * **Fresh-spawn flash** on every wall during the first
//!   `WALL_SPAWN_FLASH` seconds of its life, to make newly
//!   materialized threats easier to read.
//! * **Thick outlines** (bright inner edge, dark outer edge) on
//!   walls for readability on busy palettes.
//! * **Gradient background wedges** so the playfield reads as
//!   a receding tunnel instead of flat wedges.
//! * **Radial warning** ring when the nearest wall in the
//!   player's lane is very close.
//! * **Dash telegraph** that sketches the destination slot while
//!   the dash is ready.
//! * **Shield ripple** on every absorbed hit.
//! * **Close-call counter** HUD.

use crate::audio::Audio;
use crate::config::{Ability, Config, HighlightMode};
use crate::effects::{ParticleSystem, ScreenShake};
use crate::gend::Generator;
use crate::levels::{Level, Palette, Tier};
use crate::pipeline::Vertex;
use crate::text::{push_text, push_text_alpha, text_height, text_width};
use crate::ui::draw::{push_hex_alpha, push_ring_circle_alpha};
use crate::win32::Input;
use crate::dsl::rules::AbilityKind;
use crate::rule_engine::RuleEngine;

/// Baseline camera pitch always applied during gameplay, in
/// radians. Roughly 6 degrees: enough to give the playfield an
/// Open Hexagon style tunnel-floor look without making text or
/// projectile trajectories hard to read. Menus and the editor
/// do not apply this; they drive the renderer with pitch = 0.
const BASELINE_PITCH: f32 = 0.0;

/// Perspective strength used during gameplay, 0..1. Full
/// perspective divide; the small baseline pitch above keeps the
/// actual visible distortion subtle, so "full" here still reads
/// as SH-style mild 3D rather than an OH fish-eye.
const BASELINE_PERSP: f32 = 1.0;

const TAU: f32 = std::f32::consts::TAU;

/// Number of slots the playfield is divided into. Matches the AST
/// default; levels with a non 6 sides still display on a 6 slot
/// renderer (the renderer is hard coded for hexagons).
//fallback: const SLOTS: u32 = 6;
//fallback: const SLOT_ANGLE: f32 = TAU / SLOTS as f32;

const PLAYER_RADIUS:     f32 = 0.18;
const PLAYER_HEIGHT:     f32 = 0.035;
const PLAYER_HALF_BASE:  f32 = 0.18;

const CENTER_OUTER:      f32 = 0.13;
const CENTER_RING:       f32 = 0.013;
const CENTER_PULSE_AMP:  f32 = 0.018;

const WALL_SPAWN_R:      f32 = 4.0;
const WALL_BASE_THICK:   f32 = 0.05;
const WALL_KILL_R:       f32 = 0.06;
const BG_OUTER_R:        f32 = 5.0;

const WALL_SPEED_BASE:   f32 = 1.25;
const PLAYER_ROT_SPEED:  f32 = 6.5;

const CAMERA_SPEED:      f32 = 1.35;
const CAMERA_EASE_RATE:  f32 = 6.0;
const CAMERA_FLIP_MIN:   f32 = 3.0;
const CAMERA_FLIP_RANGE: f32 = 6.0;

/// Dying phase durations. A short slow-motion tail gives the
/// player a moment to register the hit, then the hard freeze
/// lets the shatter particles play out before transitioning to
/// the game-over prompt.
const DEATH_SLOWMO_SECONDS: f32 = 0.08;
const DEATH_FREEZE_SECONDS: f32 = 0.52;
const COLLISION_ANGLE_EPS:  f32 = 0.002;

/// Angular slack around the player where a passing wall counts
/// as a close call (expressed as a fraction of `SLOT_ANGLE`).
const CLOSE_CALL_ANGLE: f32 = 0.18;
/// Radial band in front of the player that triggers the close
/// call pulse.
const CLOSE_CALL_RADIAL_BAND: f32 = 0.035;

/// Dash parameters.
const DASH_COOLDOWN: f32 = 1.30;
const DASH_AFTERIMAGE: f32 = 0.18;

/// Fresh-spawn flash duration. Walls are painted with a bright
/// leading edge for this many seconds after appearing so new
/// threats pop visually against the already-moving background.
const WALL_SPAWN_FLASH: f32 = 0.20;

/// Maximum real-time span of player positions retained for the
/// post-death replay ghost. Older samples are dropped.
const TRAIL_SECONDS: f32 = 2.0;

/// Sampling period of the replay buffer. One entry every ~16 ms
/// gives a smooth ghost arc without eating memory.
const TRAIL_SAMPLE_DT: f32 = 0.016;

/// Duration of the full-screen flash that visualises a Flip
/// trigger firing.
const FLIP_FLASH_SECONDS: f32 = 0.28;

/// Thresholds at which a milestone text pulse fires, in seconds
/// of survival time.
const MILESTONES: &[f32] = &[10.0, 30.0, 60.0, 90.0, 120.0, 180.0, 240.0];

#[derive(Clone, Copy)]
struct Wall {
    /// Starting angle in radians, in generator coordinate
    /// space (0 at +X axis). Fixed at spawn time. A polygon
    /// morph does not rewrite this, so existing walls keep
    /// their original angular trajectory even when the
    /// playfield changes shape underneath them.
    angle_start: f32,
    /// Angular span. Equal to `TAU / slot_count_at_spawn`,
    /// also frozen at spawn time.
    angle_span: f32,
    distance: f32,
    thickness: f32,
    close_fired: bool,
    spawn_age: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunState { Alive, Dying, GameOver }

/// Dying state splits into two phases: a short slow-motion
/// tail, then a hard freeze. `update` drives each independently
/// so the wall stream keeps moving (slower) during the slow-mo
/// and halts completely during the freeze.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DyingPhase { SlowMo, Freeze }

/// Shot of player-visible feedback. Used for both the dash jump
/// afterimage and the passive turn-speed trail. `strength` scales
/// the alpha at draw time.
#[derive(Clone, Copy)]
struct Afterimage {
    angle:    f32,
    life:     f32,
    max:      f32,
    strength: f32,
}

/// Ring that expands from the center on every onset (and on
/// special events like shield break).
#[derive(Clone, Copy)]
struct PulseEcho {
    age:   f32,
    life:  f32,
    color: [f32; 3],
}

/// One sample in the replay ghost ring buffer.
#[derive(Clone, Copy)]
struct TrailSample {
    age:   f32,
    angle: f32,
}

/// Text pulse fired at a survival-time milestone.
#[derive(Clone)]
struct Milestone {
    text: String,
    age:  f32,
    life: f32,
}

pub struct Game {
    time:             f32,
    run_time:         f32,
    best_time:        f32,

    palette:          Palette,
    hue_shift_speed:  f32,
    hue_offset:       f32,

    tier:              Tier,
    current_music_path: String,

    level_speed_mult:  f32,
    level_density_mult: f32,

    onset_phase: f32,
    prev_onset_counter: u32,

    camera_rot:        f32,
    camera_dir:        f32,
    camera_target_dir: f32,
    camera_flip_timer: f32,

    player_ang:        f32,
    prev_player_ang:   f32,
    /// Smoothed angular velocity of the cursor, in radians per
    /// second. Drives the trail afterimage rate and the squash
    /// / stretch applied to the cursor triangle.
    player_ang_vel:    f32,

    walls:             Vec<Wall>,
    generator:         Generator,

    state:             RunState,
    death_phase:       DyingPhase,
    death_timer:       f32,
    death_slowmo_timer:f32,
    space_was_down:    bool,
    shift_was_down:    bool,

    // Settings snapshot, refreshed once per frame via `apply_config`.
    cfg_ability:       Ability,
    cfg_player_speed:  f32,
    cfg_close_fx:      bool,
    cfg_highlight:     HighlightMode,
    cfg_depth:         f32,
    cfg_beat_flash:    f32,
    cfg_show_hitboxes: bool,
    cfg_reduce_motion: bool,
    cfg_camera_wobble: f32,
    cfg_ui_scale:      f32,
    cfg_parallax:      bool,

    // Ability state.
    dash_cooldown:     f32,
    shield_charge:     f32,
    slowmo_active:     bool,
    afterimages:       Vec<Afterimage>,
    trail_buffer:      Vec<TrailSample>,
    trail_accum:       f32,

    // Visual state.
    particles:         ParticleSystem,
    shake:             ScreenShake,
    /// Runtime rule state driven by the level's DSL.
    ///
    /// Every `rule`, `revert`, `push`, or `pop` statement
    /// emitted by the generator is fed into this engine; the
    /// simulation then reads the flattened effective state
    /// through helpers like `active_ability()` and
    /// `effective_invert()`. The engine is seeded from the
    /// level's file level baseline in `Game::new` and reset
    /// along with the rest of the run in `Game::restart`.
    rule_engine:       RuleEngine,
    echoes:            Vec<PulseEcho>,
    close_call_count:  u32,
    milestone_bits:    u32,
    milestones:        Vec<Milestone>,
    flip_flash_timer:  f32,
    prng:              u32,
    /// Copied from `level.ast.ignore_collisions` on construction.
    /// When true, collision checks still run (close calls,
    /// shield bookkeeping, particles) but `kill_core` never
    /// fires. A subtle HUD label warns the author the mode is
    /// active.
    ignore_collisions: bool,
}

impl Game {
    /// Build a runtime game state from a level description and a
    /// picked tier. The generator is seeded from the level's own
    /// `seed` field, so reloading the same level at the same tier
    /// plays back identical patterns.
    pub fn new(level: &Level, tier: Tier, config: &Config) -> Self {
        let mut g = Game {
            time:              0.0,
            run_time:          0.0,
            best_time:         0.0,
            palette:           level.palette,
            hue_shift_speed:   level.hue_shift_speed,
            hue_offset:        0.0,
            tier,
            current_music_path: level.music_path.clone(),
            onset_phase:        1.0,
            prev_onset_counter: 0,
            camera_rot:         0.0,
            camera_dir:         1.0,
            camera_target_dir:  1.0,
            camera_flip_timer:  CAMERA_FLIP_MIN,
            // Starting angle places the cursor in the middle of slot
            // 0 of whatever polygon the level declared. Computed from
            // `level.ast.generation.sides` rather than a global
            // constant so a three-sided or twelve-sided level opens
            // with the cursor centered correctly.
            player_ang:      (TAU / level.ast.generation.sides.max(3) as f32) * 0.5,
            prev_player_ang: (TAU / level.ast.generation.sides.max(3) as f32) * 0.5,
            player_ang_vel:     0.0,
            walls:              Vec::with_capacity(128),
            level_speed_mult:   level.ast.generation.speed_mult,
            level_density_mult: level.ast.generation.density_mult,
            generator: Generator::new(
                level.ast.clone(),
                tier.spawn_density_mult()
                    * level.ast.generation.density_mult,
                60.0 / (level.music_bpm.max(40) as f32),
            ),
            state:              RunState::Alive,
            death_phase:        DyingPhase::SlowMo,
            death_timer:        0.0,
            death_slowmo_timer: 0.0,
            space_was_down:     false,
            shift_was_down:     false,

            cfg_ability:       config.ability,
            cfg_player_speed:  config.player_speed,
            cfg_close_fx:      config.close_call_fx,
            cfg_highlight:     config.predictive_highlight,
            cfg_depth:         config.fake_3d_depth,
            cfg_beat_flash:    config.beat_flash,
            cfg_show_hitboxes: config.show_hitboxes,
            cfg_reduce_motion: config.reduce_motion,
            cfg_camera_wobble: config.camera_wobble,
            cfg_ui_scale:      config.ui_scale,
            cfg_parallax:      config.background_parallax,

            dash_cooldown:  0.0,
            shield_charge:  1.0,
            slowmo_active:  false,
            afterimages:    Vec::with_capacity(16),
            trail_buffer:   Vec::with_capacity(256),
            trail_accum:    0.0,

            particles:        ParticleSystem::new(),
            shake:            ScreenShake::new(),
            rule_engine:      RuleEngine::new(level.ast.rules.clone()),
            echoes:           Vec::with_capacity(16),
            close_call_count: 0,
            milestone_bits:   0,
            milestones:       Vec::new(),
            flip_flash_timer: 0.0,
            prng:             level.seed ^ 0xCAFE1234,
            ignore_collisions: level.ast.ignore_collisions,
        };
        g.particles.set_density(config.particle_density.factor());
        g.shake.set_user_scale(config.screen_shake);
        g
    }

    /// Refresh the in-game state with any settings that changed
    /// since last frame. Called every frame from the main loop.
    pub fn apply_config(&mut self, c: &Config) {
        self.cfg_ability       = c.ability;
        self.cfg_player_speed  = c.player_speed;
        self.cfg_close_fx      = c.close_call_fx;
        self.cfg_highlight     = c.predictive_highlight;
        self.cfg_depth         = c.fake_3d_depth;
        self.cfg_beat_flash    = c.beat_flash;
        self.cfg_show_hitboxes = c.show_hitboxes;
        self.cfg_reduce_motion = c.reduce_motion;
        self.cfg_camera_wobble = c.camera_wobble;
        self.cfg_ui_scale      = c.ui_scale;
        self.cfg_parallax      = c.background_parallax;
        self.particles.set_density(c.particle_density.factor());
        self.shake.set_user_scale(c.screen_shake);
    }

    /// Snap the onset watchdog after construction so the first
    /// frame does not fire a stale "beat just fired" event.
    pub fn sync_onset_counter(&mut self, audio: &Audio) {
        self.prev_onset_counter = audio.onset_counter();
    }

    pub fn shake_offset(&self) -> (f32, f32) { self.shake.offset() }

    /// Camera pitch in radians for the vertex shader. Combines
    /// the always-on baseline with whatever the generator reports
    /// for the DSL `:tilt` trigger. This is the axis that tilts
    /// the hexagon plane in 3D, the SH/OH "look down into the
    /// tunnel" effect. Driving tilt through pitch instead of roll
    /// is what makes the hexagon itself appear tilted rather than
    /// the whole screen rotating around the cursor.
    pub fn camera_pitch(&self) -> f32 {
        self.generator.camera_pitch()
    }

    /// Camera roll in radians for the vertex shader. Currently
    /// pinned to zero: SH/OH reserve screen-roll for very rare
    /// hype moments, not the bread-and-butter tilt trigger, and
    /// applying roll to everything including HUD text reads as
    /// "the whole game window is broken" rather than "the camera
    /// moved". If a future DSL trigger wants an epic roll it can
    /// feed a dedicated field here without touching pitch.
    pub fn camera_roll(&self) -> f32 {
        0.0
    }

    /// Perspective strength in 0..1 for the vertex shader. Held
    /// at the baseline so the pitch above actually reads as 3D
    /// rather than a flat shear. Exposed as a method so the
    /// renderer has a single source of truth for the shader
    /// interface.
    pub fn camera_perspective(&self) -> f32 {
        BASELINE_PERSP
    }

    fn restart(&mut self, audio: &Audio, level: &Level) {
        self.run_time          = 0.0;
        self.camera_rot        = 0.0;
        self.camera_dir        = 1.0;
        self.camera_target_dir = 1.0;
        self.camera_flip_timer = CAMERA_FLIP_MIN;
        // Respawn the cursor in slot 0 of the current target
        // polygon. `self.generator.sides()` already reflects the
        // level's `generation.sides` at this point because the
        // generator is about to be rebuilt below.
        let init_slot = TAU / self.generator.sides().max(3) as f32;
        self.player_ang      = init_slot * 0.5;
        self.prev_player_ang = init_slot * 0.5;
        self.player_ang_vel    = 0.0;
        self.walls.clear();
        self.generator = Generator::new(
            level.ast.clone(),
            self.tier.spawn_density_mult() * self.level_density_mult,
            60.0 / (level.music_bpm.max(40) as f32),
        );
        self.hue_offset        = 0.0;
        self.state             = RunState::Alive;
        self.death_phase       = DyingPhase::SlowMo;
        self.death_timer       = 0.0;
        self.death_slowmo_timer= 0.0;
        self.dash_cooldown     = 0.0;
        self.shield_charge     = 1.0;
        self.slowmo_active     = false;
        self.afterimages.clear();
        self.trail_buffer.clear();
        self.trail_accum       = 0.0;
        self.particles.clear();
        self.echoes.clear();
        self.close_call_count  = 0;
        self.milestone_bits    = 0;
        self.milestones.clear();
        self.flip_flash_timer  = 0.0;
        // Rule engine is reset to the file level baseline so
        // a retry plays from the same rule state as the first
        // attempt. Without this, a level that pushed a
        // transient rule override before dying would leave the
        // simulation in that overridden state across the
        // restart.
        self.rule_engine.reset();
        if !self.current_music_path.is_empty() {
        audio.restart_music_at(
            &self.current_music_path,
            level.ast.start_from_seconds.max(0.0),
        );
    }
        self.prev_onset_counter = audio.onset_counter();
    }

    /// Cheap xorshift-like RNG for visual jitter. Not used for any
    /// gameplay decisions.
    fn rng(&mut self) -> f32 {
        self.prng = self.prng.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.prng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Ability currently in effect for gameplay.
    ///
    /// When the level's rule state carries an ability override
    /// (either from its file level directive or from a
    /// `rule ability { kind = ... }` statement), that override
    /// wins. Otherwise the player's configured ability from
    /// the options screen applies. `Some(AbilityKind::None)`
    /// and "no override" are deliberately distinct: the first
    /// forces the player to play without any gadget, the
    /// second defers to user config.
    fn active_ability(&self) -> Ability {
        match self.rule_engine.ability_override_kind() {
            Some(AbilityKind::None)   => Ability::None,
            Some(AbilityKind::Dash)   => Ability::Dash,
            Some(AbilityKind::Shield) => Ability::Shield,
            Some(AbilityKind::SlowMo) => Ability::SlowMo,
            None                      => self.cfg_ability,
        }
    }

    /// Combined input inversion state: either the DSL
    /// `:invert` trigger or the rule engine's `input.inverted`
    /// flag makes the player's left / right controls swap. If
    /// both are active the controls un-swap back to normal, in
    /// keeping with boolean XOR semantics (which matches what
    /// an author would intuitively expect: authored invert
    /// modifies the authored state, player-side invert
    /// modifies what the player experiences, and combining
    /// two inversions cancels).
    fn effective_invert(&self) -> bool {
        self.generator.is_inverted() ^ self.rule_engine.effective().input_inverted
    }

    /// Combined cursor rotation multiplier: the
    /// `speedwarp.cursor` trigger axis stacked multiplicatively
    /// with the rule engine's `cursor.speed_mult`.
    fn effective_cursor_mult(&self) -> f32 {
        self.generator.cursor_mult() * self.rule_engine.effective().cursor_speed_mult
    }

    pub fn update(&mut self, dt: f32, input: Input, audio: &Audio, level: &Level) {
        self.time += dt;
        self.onset_phase = audio.onset_phase();
        let counter = audio.onset_counter();
        let onset_fired = counter != self.prev_onset_counter;
        self.prev_onset_counter = counter;

        let space_edge = input.space && !self.space_was_down;
        self.space_was_down = input.space;
        let shift_edge = input.shift && !self.shift_was_down;
        self.shift_was_down = input.shift;

        let hue_rate = self.hue_shift_speed + self.generator.extra_hue_shift();
        self.hue_offset = (self.hue_offset + hue_rate * dt) % TAU;

        // Visual timers always tick so the flip flash, particles
        // and echoes settle even during Dying / GameOver.
        if self.flip_flash_timer > 0.0 {
            self.flip_flash_timer = (self.flip_flash_timer - dt).max(0.0);
        }
        self.particles.update(dt);
        self.shake.update(dt);

        let mut i = 0;
        while i < self.echoes.len() {
            self.echoes[i].age += dt;
            if self.echoes[i].age >= self.echoes[i].life {
                self.echoes.swap_remove(i);
            } else { i += 1; }
        }
        if onset_fired && !self.cfg_reduce_motion {
            let c = self.palette.accent;
            self.echoes.push(PulseEcho { age: 0.0, life: 0.55, color: c });
        }

        // Afterimages tick always (so a dash-then-die still plays).
        let mut j = 0;
        while j < self.afterimages.len() {
            self.afterimages[j].life -= dt;
            if self.afterimages[j].life <= 0.0 {
                self.afterimages.swap_remove(j);
            } else { j += 1; }
        }

        // Trail aging.
        for s in &mut self.trail_buffer { s.age += dt; }
        self.trail_buffer.retain(|s| s.age < TRAIL_SECONDS);

        // Milestones aging.
        let mut k = 0;
        while k < self.milestones.len() {
            self.milestones[k].age += dt;
            if self.milestones[k].age >= self.milestones[k].life {
                self.milestones.swap_remove(k);
            } else { k += 1; }
        }

        match self.state {
            RunState::Alive => {
                self.update_alive(dt, input, audio, onset_fired, shift_edge);
                // Collision detection always runs so shield / close-call
                // bookkeeping keeps working. Only the terminal
                // `kill_core` is gated on the debug flag.
                if self.collides(audio) && !self.ignore_collisions {
                    self.kill_core();
                }
                self.sample_trail(dt);
                self.check_milestones();
            }
            RunState::Dying => {
                match self.death_phase {
                    DyingPhase::SlowMo => {
                        self.death_slowmo_timer -= dt;
                        // Walls keep creeping at 20% speed so the
                        // cause of death stays legible.
                        let wspd = self.current_wall_speed() * 0.20;
                        for w in self.walls.iter_mut() {
                            w.distance -= wspd * dt;
                            w.spawn_age += dt;
                        }
                        if self.death_slowmo_timer <= 0.0 {
                            self.death_phase = DyingPhase::Freeze;
                            self.death_timer = DEATH_FREEZE_SECONDS;
                        }
                    }
                    DyingPhase::Freeze => {
                        self.death_timer -= dt;
                        if self.death_timer <= 0.0 { self.state = RunState::GameOver; }
                    }
                }
            }
            RunState::GameOver => {
                if space_edge {
                    audio.play_enter();
                    self.restart(audio, level);
                }
            }
        }
    }

    fn update_alive(
        &mut self, dt: f32, input: Input, audio: &Audio,
        _onset_fired: bool, shift_edge: bool,
    ) {
        // Slow-mo ability halves the effective time step while the
        // player holds Shift. Every downstream integrator uses
        // `eff_dt`, so the entire simulation (walls, camera, timers,
        // particles) slows together rather than only a subset.
        self.slowmo_active = matches!(self.active_ability(), Ability::SlowMo)
            && input.shift;
        let eff_dt = if self.slowmo_active { dt * 0.45 } else { dt };

        // Advance the generator first. Doing it before any state is
        // read means triggers fired this frame are already reflected
        // in `shake_envelope`, `spin_rate`, `freeze_factor`, and the
        // rest of the accessor methods below. Rule statements that
        // came due on the same tick are drained right after so the
        // rule engine lines up with whatever the generator just
        // pushed into the world.
        self.generator.update(
            eff_dt, audio.music_position(), audio.music_duration());
        let engine = &mut self.rule_engine;
        self.generator.drain_rules(|stmt| engine.apply(stmt));

        // Screen shake from the `:shake` trigger. Trauma is topped
        // up every frame against ScreenShake's own decay so the
        // perceived amplitude matches the authored strength for the
        // full authored duration, with a linear tail coming from the
        // `life` factor of the envelope.
        let (shake_strength, shake_life) = self.generator.shake_envelope();
        if shake_strength > 0.0 && shake_life > 0.0 && !self.cfg_reduce_motion {
            self.shake.add(shake_strength * shake_life * 1.6 * eff_dt);
        }

        // Per-onset trauma bounce. While `:bounce` is active, every
        // detected audio onset injects trauma proportional to the
        // authored amplitude. Unaffected by `reduce_motion` because
        // the setting already gates the shake accumulator.
        let bounce = self.generator.bounce_amplitude();
        if bounce > 0.0 && _onset_fired && !self.cfg_reduce_motion {
            self.shake.add(bounce * 0.6);
        }

        self.run_time += eff_dt;

        // Camera wobble scales every rotational effect. At 0 the
        // camera is still (accessibility preset); at 1 it matches
        // the original game's feel.
        let wobble = self.cfg_camera_wobble.clamp(0.0, 1.0);

        // Scheduled spontaneous flip. Tick the timer, and if it
        // expires, reverse target direction and reset using a
        // pseudo-random jitter derived from the current speed
        // multiplier so the cadence never locks into a pattern.
        self.camera_flip_timer -= eff_dt;
        if self.camera_flip_timer <= 0.0 {
            self.camera_target_dir = -self.camera_target_dir;
            let r = (self.generator.speed_mult().fract().abs() + 0.3).fract();
            self.camera_flip_timer = CAMERA_FLIP_MIN + r * CAMERA_FLIP_RANGE;
        }

        // Authored `:flip` trigger. Takes precedence over the
        // scheduled flip above by restarting its timer, plays a
        // feedback click, injects a small trauma impulse, and arms
        // the full-screen white flash used as visual confirmation.
        if self.generator.take_flip_request() {
            self.camera_target_dir = -self.camera_target_dir;
            self.camera_flip_timer = CAMERA_FLIP_MIN;
            audio.play_interact();
            if !self.cfg_reduce_motion { self.shake.add(0.12); }
            self.flip_flash_timer = FLIP_FLASH_SECONDS;
        }

        // Ease current direction toward target so flips read as a
        // smooth reversal instead of a hard sign change.
        let blend = 1.0 - (-CAMERA_EASE_RATE * eff_dt).exp();
        self.camera_dir += (self.camera_target_dir - self.camera_dir) * blend;

        // Camera rotation. Combines tier and level multipliers with
        // the generator's own rotation axis from `:speedwarp` and
        // the continuous authored spin from `:spin`.
        let cam_mult = (self.tier.wall_speed_mult()
                        * self.level_speed_mult).min(2.5);
        self.camera_rot += eff_dt * CAMERA_SPEED * self.camera_dir
            * cam_mult * wobble * self.generator.rotation_mult();
        self.camera_rot += self.generator.spin_rate() * eff_dt;

        // Player turn input. Honors the generator's invert state and
        // the `:speedwarp` cursor axis. Raw angle is kept unwrapped
        // so the velocity filter below sees a clean delta even when
        // the player crosses the TAU boundary.
        self.prev_player_ang = self.player_ang;
        let mut p = 0.0;
        if input.left  { p -= 1.0; }
        if input.right { p += 1.0; }
        if self.generator.is_inverted() { p = -p; }
        let cursor_mult = self.generator.cursor_mult();
        self.player_ang +=
            p * PLAYER_ROT_SPEED * self.tier.player_speed_mult()
            * self.cfg_player_speed * cursor_mult * eff_dt;
        if self.player_ang >  TAU { self.player_ang -= TAU; }
        if self.player_ang < -TAU { self.player_ang += TAU; }

        // Filtered angular velocity. The raw delta can spike when
        // the angle wraps, so wrap-aware correction happens before
        // the low-pass filter that feeds the trail and cursor
        // squash visuals.
        let mut raw_vel = (self.player_ang - self.prev_player_ang)
            / eff_dt.max(1e-4);
        if raw_vel >  PLAYER_ROT_SPEED * 3.0 {
            raw_vel -= TAU / eff_dt.max(1e-4);
        }
        if raw_vel < -PLAYER_ROT_SPEED * 3.0 {
            raw_vel += TAU / eff_dt.max(1e-4);
        }
        let sm = 1.0 - (-18.0 * eff_dt).exp();
        self.player_ang_vel += (raw_vel - self.player_ang_vel) * sm;

        // Passive trail afterimages when the cursor turns fast.
        // Gated by the accessibility reduce-motion setting so the
        // playfield stays clean for users who requested it.
        if !self.cfg_reduce_motion {
            let speed_norm = (self.player_ang_vel.abs()
                            / (PLAYER_ROT_SPEED * 1.2)).clamp(0.0, 1.0);
            if speed_norm > 0.25 {
                self.afterimages.push(Afterimage {
                    angle:    self.player_ang,
                    life:     0.10,
                    max:      0.10,
                    strength: speed_norm * 0.35,
                });
            }
        }

        // Dash ability. Cooldown ticks every frame regardless of
        // slow-mo so the cooldown reads in wall-clock seconds.
        // Dashing snaps the player one slot in the current turn
        // direction and pushes a strong afterimage.
        if self.dash_cooldown > 0.0 { self.dash_cooldown -= dt; }
        if matches!(self.active_ability(), Ability::Dash)
            && shift_edge && self.dash_cooldown <= 0.0
        {
            let dir = if p >= 0.0 { 1.0 } else { -1.0 };
            let old = self.player_ang;
            // Dash jumps exactly one slot of the current polygon.
            // Using `self.generator.sides()` means the jump width
            // adapts automatically if the level morphed mid run.
            let slot_angle = TAU / self.generator.sides().max(3) as f32;
            self.player_ang += slot_angle * dir;
            self.dash_cooldown = DASH_COOLDOWN;
            audio.play_interact();
            self.afterimages.push(Afterimage {
                angle: old,
                life:  DASH_AFTERIMAGE,
                max:   DASH_AFTERIMAGE,
                strength: 1.0,
            });
            if !self.cfg_reduce_motion { self.shake.add(0.05); }
        }

        // Shield ability regenerates slowly while held.
        if matches!(self.active_ability(), Ability::Shield) {
            self.shield_charge = (self.shield_charge + eff_dt * 0.08).min(1.0);
        }

        // Compute the effective wall speed after every multiplier
        // (tier, level, speedwarp, freeze) is known to the
        // generator. Integrate wall positions and spawn age against
        // this single value so a freeze trigger truly halts motion.
        let wall_speed = self.current_wall_speed();
        for w in self.walls.iter_mut() {
            w.distance -= wall_speed * eff_dt;
            w.spawn_age += eff_dt;
        }

        // Close-call detection. A wall that sweeps past the player
        // in an adjacent slot, within a narrow radial band just
        // outside the cursor, is a close call. Fires particles,
        // trauma, and a touch sound, but only once per wall.
        //
        // "Adjacent slot" is resolved purely by angular distance so
        // the logic works for any polygon shape, not just hex.
        if self.cfg_close_fx {
            let sides = self.generator.sides().max(3);
            let slot_angle = TAU / sides as f32;
            let player_ang_norm = self.player_ang.rem_euclid(TAU);
            let p_inner = PLAYER_RADIUS;

            let mut close_events: Vec<(f32, f32)> = Vec::new();
            for w in self.walls.iter_mut() {
                if w.close_fired { continue; }
                let outer = w.distance + w.thickness;
                if outer < p_inner + CLOSE_CALL_RADIAL_BAND
                && outer > p_inner - CLOSE_CALL_RADIAL_BAND * 0.5
                {
                    let wall_center = (w.angle_start
                        + w.angle_span * 0.5).rem_euclid(TAU);
                    let mut diff = (wall_center - player_ang_norm)
                        .rem_euclid(TAU);
                    if diff > TAU * 0.5 { diff = TAU - diff; }
                    // Adjacent means roughly one slot away in
                    // either direction. The >0.5 lower bound rules
                    // out "same slot" (that's a collision), the
                    // <1.5 upper bound rules out "two slots away".
                    if diff > slot_angle * 0.5 && diff < slot_angle * 1.5 {
                        close_events.push((wall_center + self.camera_rot, outer));
                        w.close_fired = true;
                    }
                }
            }
            for (ang, r) in close_events {
                self.close_call_count += 1;
                if !self.cfg_reduce_motion { self.shake.add(0.07); }
                let pos = [ang.cos() * r, ang.sin() * r];
                let mut seed = self.prng;
                self.particles.emit_burst(
                    pos, 14, 1.6, self.palette.accent,
                    0.55, 0.025, &mut seed,
                );
                self.prng = seed;
                audio.play_touch();
            }
        }

        // Drop walls that passed the inner kill radius so they do
        // not keep consuming collision tests or draw vertices.
        self.walls.retain(|w| w.distance + w.thickness > WALL_KILL_R);

        // Spawn any walls whose scheduled time arrived this frame.
        // Thickness is a base value plus a radial contribution
        // proportional to the authored length in seconds, so longer
        // obstacles genuinely occupy more radial space.
        //
        // The callback now receives `slot_count` alongside `slot`
        // so we can project each wall onto the angular geometry it
        // was designed for, regardless of any morph in progress on
        // the visual playfield.
        let walls_ref = &mut self.walls;
        let wspd = wall_speed;
        self.generator.drain_walls(|slot, slot_count, thickness_mult, length_seconds| {
            let base  = WALL_BASE_THICK * thickness_mult;
            let extra = (wspd * length_seconds).max(0.0);
            let thickness = (base + extra).min(3.0);
            let slot_angle = TAU / slot_count.max(3) as f32;
            let angle_start = slot as f32 * slot_angle;
            walls_ref.push(Wall {
                angle_start,
                angle_span: slot_angle,
                distance: WALL_SPAWN_R,
                thickness,
                close_fired: false,
                spawn_age: 0.0,
            });
        });

        // One-shot burst events. The generator hands them over as
        // Options so a pending burst cannot fire twice, and the
        // visuals for each (count, speed, color, trauma) scale with
        // the authored strength.
        if let Some(strength) = self.generator.take_centerburst() {
            let count = (14.0 + strength * 24.0) as u32;
            let speed = 1.6 + strength * 1.4;
            let color = self.palette.accent;
            let mut seed = self.prng;
            self.particles.emit_burst(
                [0.0, 0.0], count, speed, color, 0.7, 0.035, &mut seed);
            self.prng = seed;
            if !self.cfg_reduce_motion {
                self.shake.add(0.1 * strength);
            }
        }
        if let Some((count, duration)) = self.generator.take_ringburst() {
            let color = self.palette.accent;
            for i in 0..count {
                let delay = duration * (i as f32) / (count as f32);
                self.echoes.push(PulseEcho {
                    age:   -delay,
                    life:  0.6 + duration,
                    color,
                });
            }
        }

        // Retained for future tuning of close-call sensitivity.
        let _ = CLOSE_CALL_ANGLE;
    }
    /// Advance the replay buffer. Sampled at fixed intervals so
    /// the ghost arc looks uniform regardless of frame time.
    fn sample_trail(&mut self, dt: f32) {
        self.trail_accum += dt;
        while self.trail_accum >= TRAIL_SAMPLE_DT {
            self.trail_accum -= TRAIL_SAMPLE_DT;
            self.trail_buffer.push(TrailSample {
                age:   0.0,
                angle: self.player_ang,
            });
            // Trim hard upper bound so the vec never grows unbounded
            // even if delta time spikes.
            if self.trail_buffer.len() > 256 {
                self.trail_buffer.remove(0);
            }
        }
    }

    fn check_milestones(&mut self) {
        for (i, t) in MILESTONES.iter().enumerate() {
            let bit = 1u32 << i;
            if (self.milestone_bits & bit) == 0 && self.run_time >= *t {
                self.milestone_bits |= bit;
                let text = if *t >= 60.0 {
                    let mins = (*t / 60.0).floor() as u32;
                    let secs = (*t - mins as f32 * 60.0) as u32;
                    if secs == 0 {
                        format!("{} MINUTE{}", mins, if mins > 1 { "S" } else { "" })
                    } else {
                        format!("{}:{:02}", mins, secs)
                    }
                } else {
                    format!("{} SECONDS", *t as u32)
                };
                self.milestones.push(Milestone {
                    text, age: 0.0, life: 1.5,
                });
                if !self.cfg_reduce_motion { self.shake.add(0.10); }
            }
        }
    }
    
    /// Current camera zoom for the renderer push constants.
    pub fn zoom(&self) -> f32 {
        let base = self.generator.zoom();
        let punch = self.generator.zoom_punch_offset();
        (base + punch).clamp(0.1, 5.0)
    }

    /// Current camera tilt vector ready for `Renderer::set_tilt`.
    /// Components are `[angle (Z-roll), pitch (X), yaw (Y)]` in
    /// radians.
    ///
    /// The Z-roll component is deliberately zeroed here because
    /// the playfield's own `cam_fg` already includes the tilt
    /// angle on the CPU side. Letting the shader rotate a
    /// second time would both double the effect on gameplay
    /// geometry and, worse, rotate HUD text that has no
    /// business tilting with the camera.
    ///
    /// Pitch and yaw stay in the shader. They drive the fake
    /// perspective divide which is naturally per-vertex work,
    /// and at the 30 degree authored limit their influence on
    /// HUD legibility is small enough to accept.
    pub fn tilt(&self) -> [f32; 3] {
        let limit = 0.55;
        [
            0.0,
            self.generator.camera_pitch().clamp(-limit, limit),
            self.generator.camera_yaw().clamp(-limit, limit),
        ]
    }

    /// Post-process glitch intensity for this frame.
    pub fn glitch_amount(&self) -> f32 { self.generator.glitch_amount() }

    /// Strobe flash alpha for this frame.
    pub fn strobe_alpha(&self) -> f32 { self.generator.strobe_alpha(self.time) }

    pub fn invert_colors_amount(&self) -> f32 {
        self.generator.invert_colors_amount()
    }
    pub fn grayscale_amount(&self) -> f32 {
        self.generator.grayscale_amount()
    }
    pub fn shockwave_progress(&self) -> f32 {
        self.generator.shockwave_progress()
    }
    pub fn shockwave_strength(&self) -> f32 {
        self.generator.shockwave_strength()
    }
    pub fn fog_bounds(&self) -> (f32, f32) {
        self.generator.fog_bounds()
    }
    pub fn outline_amount(&self) -> f32 {
        self.generator.outline_amount()
    }
    pub fn flash_alpha_bassdrop(&self) -> f32 {
        self.generator.flash_alpha()
    }

    /// Slot index and parameters of the user post shader
    /// the active section currently requests, or `None`
    /// when the default post pipeline should be used. The
    /// main loop consults this once per frame to keep the
    /// renderer in sync without the game having to know
    /// about Vulkan.
    pub fn active_post_shader(&self) -> Option<(u8, [f32; 4])> {
        self.generator.active_post_shader()
    }

    /// Music playback rate requested by the generator's
    /// `speedwarp` trigger. 1.0 is neutral.
    pub fn music_rate(&self) -> f32 { self.generator.music_rate() }

    fn collides(&mut self, audio: &Audio) -> bool {
        let p_inner = PLAYER_RADIUS;
        let p_outer = PLAYER_RADIUS + PLAYER_HEIGHT;

        let mut a = self.player_ang % TAU;
        if a < 0.0 { a += TAU; }

        for w in self.walls.iter() {
            let w_inner = w.distance;
            let w_outer = w.distance + w.thickness;
            if p_outer < w_inner || p_inner > w_outer { continue; }

            // Angular containment. Wall spans
            // [angle_start, angle_start + angle_span). Handle
            // the wrap-around case so a wall that straddles
            // the 0/TAU seam still collides correctly.
            let wa_start = w.angle_start.rem_euclid(TAU);
            let wa_end   = wa_start + w.angle_span;
            let in_slot = if wa_end <= TAU {
                a >= wa_start + COLLISION_ANGLE_EPS
                    && a < wa_end - COLLISION_ANGLE_EPS
            } else {
                let wrapped = wa_end - TAU;
                (a >= wa_start + COLLISION_ANGLE_EPS)
                    || (a < wrapped - COLLISION_ANGLE_EPS)
            };
            if !in_slot { continue; }

            // Shield absorbs first hit when fully charged.
            if matches!(self.active_ability(), Ability::Shield)
                && self.shield_charge >= 1.0
            {
                self.shield_charge = 0.0;
                let ang = self.player_ang + self.camera_rot;
                let r = PLAYER_RADIUS + PLAYER_HEIGHT * 0.5;
                let pos = [ang.cos() * r, ang.sin() * r];
                let mut seed = self.prng;
                self.particles.emit_burst(
                    pos, 22, 2.2, [0.55, 0.85, 1.0], 0.8, 0.035, &mut seed);
                self.prng = seed;
                self.echoes.push(PulseEcho {
                    age: 0.0, life: 0.8, color: [0.55, 0.85, 1.0],
                });
                if !self.cfg_reduce_motion { self.shake.add(0.18); }
                audio.play_interact();
                return false;
            }
            return true;
        }
        false
    }

    /// Effective wall travel speed with all multipliers folded
    /// in. Shared by `update_alive` (for integrating wall motion)
    /// and the wall spawn callback (for converting `length_seconds`
    /// into a radial thickness contribution).
    fn current_wall_speed(&self) -> f32 {
        WALL_SPEED_BASE
            * self.tier.wall_speed_mult()
            * self.level_speed_mult
            * self.generator.speed_mult()
            * self.generator.freeze_factor()
    }

    fn kill_core(&mut self) {
        if self.run_time > self.best_time { self.best_time = self.run_time; }
        self.state = RunState::Dying;
        self.death_phase = DyingPhase::SlowMo;
        self.death_slowmo_timer = DEATH_SLOWMO_SECONDS;
        self.death_timer = DEATH_FREEZE_SECONDS;
        // Shatter shards and a red burst.
        let a = self.player_ang + self.camera_rot;
        let r = PLAYER_RADIUS + PLAYER_HEIGHT * 0.5;
        let pos = [a.cos() * r, a.sin() * r];
        let color = self.palette.player;
        let mut seed = self.prng;
        self.particles.emit_shatter(pos, 28, color, &mut seed);
        self.particles.emit_burst(
            pos, 24, 2.4, [1.0, 0.2, 0.2], 0.9, 0.03, &mut seed);
        self.prng = seed;
        self.shake.add(0.9);
    }

    pub fn kill_external(&mut self, audio: &Audio) {
        self.kill_core();
        audio.play_touch();
        audio.stop_music();
    }

    pub fn run_time(&self)  -> f32 { self.run_time }
    pub fn best_time(&self) -> f32 { self.best_time }
    pub fn is_dead(&self) -> bool {
        matches!(self.state, RunState::Dying | RunState::GameOver)
    }
    pub fn is_alive(&self) -> bool { matches!(self.state, RunState::Alive) }
    pub fn close_call_count(&self) -> u32 { self.close_call_count }
    pub fn dash_cooldown_ratio(&self) -> f32 {
        (self.dash_cooldown / DASH_COOLDOWN).clamp(0.0, 1.0)
    }
    pub fn shield_charge(&self) -> f32 { self.shield_charge.clamp(0.0, 1.0) }

    pub fn build_geometry(&self, out: &mut Vec<Vertex>) {
        out.clear();
        // Apply the Z-roll component of the camera tilt on the CPU
        // so HUD overlays (FPS, editor banners, menu text) drawn
        // through the same pipeline stay upright. Pitch and yaw
        // still ride the vertex shader's perspective path because
        // they need the per-vertex divide to look correct, and
        // those axes do not affect 2D text legibility noticeably
        // at the parser-enforced 30 degree clamp.
        //
        // The playfield now rotates visibly when a `:tilt { angle
        // = X }` trigger fires: the shift lands directly on top of
        // the always-on camera rotation, and because `tilt` eases
        // toward its target over TILT_EASE_RATE the transition
        // reads as a smooth lean rather than a single-frame jerk.
        let cam_fg = self.camera_rot + self.generator.camera_tilt();
        let cam_bg = cam_fg;
        let _ = self.cfg_parallax;
        let pulse  = decaying_beat_pulse(self.onset_phase)
                      .max(self.generator.pulse_boost())
                      * self.cfg_beat_flash;
        let dead_t = self.dead_intensity();
        let hue    = self.hue_offset;

        // 1. Background wedges with a radial gradient. We draw
        //    `BG_WEDGE_COUNT` thin wedges and color each one based
        //    on which angular sector of the current N-gon it falls
        //    in. This keeps the visual transition continuous when
        //    `sides_fract()` is morphing between two integer values:
        //    sector boundaries slide across the thin wedges rather
        //    than snapping.
        const BG_WEDGE_COUNT: u32 = 120;
        let bg_a_lit = brighten(rotate_hue(self.palette.bg_a, hue), 0.10 * pulse);
        let bg_b_lit = rotate_hue(self.palette.bg_b, hue);
        let bg_a_ctr = mul_color(bg_a_lit, 0.35);
        let bg_b_ctr = mul_color(bg_b_lit, 0.35);
        let sides_f = self.generator.sides_fract();
        let bg_step = TAU / BG_WEDGE_COUNT as f32;
        for s in 0..BG_WEDGE_COUNT {
            let a0 = s as f32 * bg_step + cam_bg;
            let angle_mid = (s as f32 + 0.5) * bg_step;
            let sector = (angle_mid * sides_f / TAU).floor() as i32;
            let (oc, cc) = if sector.rem_euclid(2) == 0 {
                (bg_a_lit, bg_a_ctr)
            } else {
                (bg_b_lit, bg_b_ctr)
            };
            push_wedge_gradient(out, a0, bg_step, BG_OUTER_R, cc, oc);
        }

        // 2. Pulse echoes (audio onsets and shield breaks).
        for e in &self.echoes {
            let t = (e.age / e.life).clamp(0.0, 1.0);
            let r = 0.25 + t * 2.2;
            let thick = 0.02 * (1.0 - t);
            let a = (1.0 - t) * 0.35;
            push_ring_circle_alpha(out, 0.0, 0.0, r + thick, r, e.color, a, 48);
        }

        // 3. Walls. Each wall gets a soft outer shadow, the main
        //    body, a bright inner edge (toward the player), and a
        //    fresh-spawn flash for its first WALL_SPAWN_FLASH
        //    seconds of life.
        let wall_base = rotate_hue(self.palette.wall, hue);
        let wall_color = mix_color(wall_base, [1.0, 0.10, 0.10], dead_t);

        // Predictive highlight: which wall is the player aimed at?
        // Uses angular containment so the highlight tracks
        // correctly on any polygon count.
        let mut highlight_idx: Option<usize> = None;
        if self.highlight_enabled() {
            let player_ang_norm = self.player_ang.rem_euclid(TAU);
            let mut nearest: Option<(usize, f32)> = None;
            for (i, w) in self.walls.iter().enumerate() {
                let wa_start = w.angle_start.rem_euclid(TAU);
                let wa_end   = wa_start + w.angle_span;
                let in_slot = if wa_end <= TAU {
                    player_ang_norm >= wa_start && player_ang_norm < wa_end
                } else {
                    player_ang_norm >= wa_start
                        || player_ang_norm < (wa_end - TAU)
                };
                if !in_slot { continue; }
                let d = w.distance - PLAYER_RADIUS;
                if d <= 0.0 { continue; }
                if nearest.map_or(true, |(_, bd)| d < bd) {
                    nearest = Some((i, d));
                }
            }
            if let Some((i, _)) = nearest { highlight_idx = Some(i); }
        }

        for (i, w) in self.walls.iter().enumerate() {
            let a0 = w.angle_start + cam_fg;
            let a1 = w.angle_start + w.angle_span + cam_fg;
            let r0 = self.radial_depth(w.distance);
            let r1 = self.radial_depth(w.distance + w.thickness);

            // Base wall color, shaded by depth so distant walls
            // visually recede. Lerps toward near-black at the
            // spawn radius when fake-3D depth is engaged.
            let mut color = wall_color;
            let depth_t = ((w.distance - 0.3) / (WALL_SPAWN_R - 0.3)).clamp(0.0, 1.0);
            color = mix_color(color, mul_color(color, 0.4), depth_t * self.cfg_depth);
            if Some(i) == highlight_idx {
                color = mix_color(color, [1.0, 0.3, 0.3], 0.65);
            }

            // Soft outer shadow: slightly darker quad sitting
            // further from the center, giving the wall a bit of
            // depth against the background gradient.
            let shadow = mul_color(color, 0.25);
            push_quad_polar(out, a0, a1, r1, r1 + 0.012, shadow);

            // Main body.
            push_quad_polar(out, a0, a1, r0, r1, color);

            // Bright inner edge (facing the player). Helps the
            // gap read clearly on busy palettes.
            let edge_bright = brighten(color, 0.22);
            let edge_thick = (r1 - r0).min(0.006);
            if edge_thick > 0.0 {
                push_quad_polar(out, a0, a1, r0, r0 + edge_thick, edge_bright);
            }

            // Fresh-spawn flash: a bright leading strip that fades
            // over WALL_SPAWN_FLASH seconds.
            if w.spawn_age < WALL_SPAWN_FLASH {
                let t = 1.0 - w.spawn_age / WALL_SPAWN_FLASH;
                let flash_color = mix_color(color, [1.0, 1.0, 1.0], 0.6);
                let flash_thick = (r1 - r0).min(0.012);
                push_quad_polar_alpha(
                    out, a0, a1, r1 - flash_thick, r1,
                    flash_color, t * 0.7,
                );
            }
        }

        // 4. Particles (close-call / shatter / shield ripple).
        self.particles.draw(out);

        // 5. Afterimages of cursor. Trail and dash share one pass.
        for ai in &self.afterimages {
            let t = (ai.life / ai.max).clamp(0.0, 1.0);
            let pa = ai.angle + cam_fg;
            let pc = self.palette.player;
            let r_tip  = PLAYER_RADIUS + PLAYER_HEIGHT;
            let r_base = PLAYER_RADIUS;
            let tip = [pa.cos() * r_tip, pa.sin() * r_tip];
            let bl  = pa - PLAYER_HALF_BASE;
            let br  = pa + PLAYER_HALF_BASE;
            let lp  = [bl.cos() * r_base, bl.sin() * r_base];
            let rp  = [br.cos() * r_base, br.sin() * r_base];
            let alpha = t * 0.5 * ai.strength;
            out.push(Vertex::rgba(tip, pc, alpha));
            out.push(Vertex::rgba(lp,  pc, alpha));
            out.push(Vertex::rgba(rp,  pc, alpha));
        }

        // 6. Replay ghost (only after the run ended). Fading arc
        //    of the last TRAIL_SECONDS of player positions.
        if matches!(self.state, RunState::Dying | RunState::GameOver) {
            let r = PLAYER_RADIUS + PLAYER_HEIGHT * 0.5;
            for s in &self.trail_buffer {
                let t = 1.0 - (s.age / TRAIL_SECONDS).clamp(0.0, 1.0);
                let a = s.angle + cam_fg;
                let x = a.cos() * r;
                let y = a.sin() * r;
                push_hex_alpha(out, x, y, 0.012, [1.0, 1.0, 1.0], t * 0.35);
            }
        }

        // 7. Center disk + ring. Both are rendered as a triangle
        //    fan of `CENTER_FAN` segments whose outer radius at
        //    each angle is computed from the current fractional
        //    side count. This makes the center shape morph
        //    smoothly between any two polygons.
        const CENTER_FAN: u32 = 72;
        let center_step = TAU / CENTER_FAN as f32;
        let center_outer = CENTER_OUTER + CENTER_PULSE_AMP * pulse;
        let fill_color = rotate_hue(self.palette.center_fill, hue);
        for s in 0..CENTER_FAN {
            let a_local_0 = s as f32 * center_step;
            let a_local_1 = (s + 1) as f32 * center_step;
            let r0 = polygon_radius_at(a_local_0, sides_f, center_outer);
            let r1 = polygon_radius_at(a_local_1, sides_f, center_outer);
            let a0 = a_local_0 + cam_fg;
            let a1 = a_local_1 + cam_fg;
            push_wedge_varying(out, a0, a1, r0, r1, fill_color);
        }

        let ring_base  = rotate_hue(self.palette.center_ring, hue);
        let ring_color = mix_color(ring_base, [1.0, 0.20, 0.20], dead_t);
        for s in 0..CENTER_FAN {
            let a_local_0 = s as f32 * center_step;
            let a_local_1 = (s + 1) as f32 * center_step;
            let r_inner_0 = polygon_radius_at(a_local_0, sides_f, center_outer);
            let r_inner_1 = polygon_radius_at(a_local_1, sides_f, center_outer);
            let r_outer_0 = r_inner_0 + CENTER_RING;
            let r_outer_1 = r_inner_1 + CENTER_RING;
            let a0 = a_local_0 + cam_fg;
            let a1 = a_local_1 + cam_fg;
            push_quad_polar_varying(out, a0, a1,
                r_inner_0, r_inner_1, r_outer_0, r_outer_1, ring_color);
        }

        // 8. Ability indicators.
        if matches!(self.active_ability(), Ability::Shield) {
            let r = center_outer + CENTER_RING + 0.01;
            let a = 0.20 + 0.60 * self.shield_charge;
            push_ring_circle_alpha(
                out, 0.0, 0.0, r + 0.006, r, [0.55, 0.85, 1.0], a, 48);
        }
        if matches!(self.active_ability(), Ability::Dash) {
            let r = center_outer + CENTER_RING + 0.01;
            let ready = 1.0 - self.dash_cooldown_ratio();
            let col = [0.6 + 0.4 * ready, 0.9, 0.6];
            push_ring_circle_alpha(out, 0.0, 0.0, r + 0.006, r,
                col, 0.3 + 0.5 * ready, 48);
        }

        // 9. Radial warning. When the nearest wall in the player's
        //    current angular slot is dangerously close, pulse a red
        //    ring just outside the center disk. Works regardless of
        //    how many sides the current polygon has.
        if self.is_alive() {
            let sides = self.generator.sides().max(3);
            let slot_angle = TAU / sides as f32;
            let player_ang_norm = self.player_ang.rem_euclid(TAU);
            let ps = (player_ang_norm / slot_angle) as u32 % sides;
            let player_slot_center = (ps as f32 + 0.5) * slot_angle;
            let mut min_d = f32::MAX;
            for w in &self.walls {
                let wa_center = (w.angle_start + w.angle_span * 0.5)
                    .rem_euclid(TAU);
                let mut diff = (wa_center - player_slot_center).abs();
                if diff > TAU * 0.5 { diff = TAU - diff; }
                if diff < slot_angle * 0.5 {
                    let d = w.distance - (PLAYER_RADIUS + PLAYER_HEIGHT);
                    if d > 0.0 && d < min_d { min_d = d; }
                }
            }
            if min_d < 0.30 {
                let intensity = 1.0 - (min_d / 0.30).clamp(0.0, 1.0);
                let pulse = 0.6 + 0.4 * (self.time * 14.0).sin();
                let r0 = CENTER_OUTER + CENTER_RING + 0.022;
                let r1 = r0 + 0.008 + 0.006 * intensity;
                push_ring_circle_alpha(out, 0.0, 0.0, r1, r0,
                    [1.0, 0.25, 0.30], intensity * pulse * 0.55, 48);
            }
        }

        // 10. Dash telegraph. When the dash ability is equipped
        //     and ready, show a faint ghost triangle one slot over
        //     in the direction of the player's current turn. Helps
        //     the player commit to the jump.
        if self.is_alive()
            && matches!(self.active_ability(), Ability::Dash)
            && self.dash_cooldown <= 0.0
        {
            let sides = self.generator.sides().max(3);
            let slot_angle = TAU / sides as f32;
            let dir = if self.player_ang_vel >= 0.0 { 1.0 } else { -1.0 };
            let target_ang = self.player_ang + slot_angle * dir + cam_fg;
            let pc = self.palette.player;
            let r_tip  = PLAYER_RADIUS + PLAYER_HEIGHT;
            let r_base = PLAYER_RADIUS;
            let tip = [target_ang.cos() * r_tip, target_ang.sin() * r_tip];
            let hb = PLAYER_HALF_BASE * 0.65;
            let bl = target_ang - hb;
            let br = target_ang + hb;
            let lp = [bl.cos() * r_base, bl.sin() * r_base];
            let rp = [br.cos() * r_base, br.sin() * r_base];
            out.push(Vertex::rgba(tip, pc, 0.18));
            out.push(Vertex::rgba(lp,  pc, 0.18));
            out.push(Vertex::rgba(rp,  pc, 0.18));
        }

        // 11. Cursor (unless inside the Dying animation, where the
        //     shatter particles stand in for it).
        if !matches!(self.state, RunState::Dying) {
            self.draw_cursor(out, cam_fg, dead_t);
        }

        // 12. Optional hitbox overlay.
        if self.cfg_show_hitboxes {
            let pa = self.player_ang + cam_fg;
            let r_tip  = PLAYER_RADIUS + PLAYER_HEIGHT;
            let r_base = PLAYER_RADIUS;
            let tip = [pa.cos() * r_tip, pa.sin() * r_tip];
            let base = [pa.cos() * r_base, pa.sin() * r_base];
            let hb = [0.2, 1.0, 0.2];
            // Thin line from base to tip.
            let perp_x = -pa.sin() * 0.003;
            let perp_y =  pa.cos() * 0.003;
            out.push(Vertex::rgba([base[0] + perp_x, base[1] + perp_y], hb, 0.7));
            out.push(Vertex::rgba([base[0] - perp_x, base[1] - perp_y], hb, 0.7));
            out.push(Vertex::rgba([tip[0],  tip[1] ], hb, 0.7));
        }

        // 13. Speed lines: fired when the generator's SpeedMult
        //     trigger is active. Random radial streaks, warm tint
        //     if accelerating, cool tint if decelerating.
        let sm = self.generator.speed_mult();
        if (sm - 1.0).abs() > 0.08 && !self.cfg_reduce_motion && self.is_alive() {
            let intensity = ((sm - 1.0).abs() * 1.4).clamp(0.0, 1.0);
            let color = if sm > 1.0 { [1.0, 1.0, 0.92] } else { [0.62, 0.82, 1.0] };
            let mut seed = self.prng ^ ((self.time * 60.0) as u32);
            for _ in 0..24 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let r1 = (seed >> 8) as f32 / (1u32 << 24) as f32;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let r2 = (seed >> 8) as f32 / (1u32 << 24) as f32;
                let ang = r1 * TAU;
                let r_start = 1.4 + r2 * 2.1;
                let r_end   = r_start + 0.18 + 0.14 * intensity;
                push_radial_streak(out, ang, r_start, r_end,
                    0.004, color, intensity * 0.55);
            }
        }

        // 14. HueShift rim: a soft colored ring along the
        //     outer edge of the playfield while the hue drift
        //     is active.
        let hx = self.generator.extra_hue_shift();
        if hx.abs() > 0.05 {
            let t = (hx.abs() / 2.0).clamp(0.0, 1.0);
            let rim = rotate_hue([1.0, 0.55, 0.55],
                self.hue_offset + hx * 2.0);
            let ri = BG_OUTER_R * 0.82;
            let ro = BG_OUTER_R * 0.96;
            push_ring_circle_alpha(out, 0.0, 0.0, ro, ri, rim, t * 0.4, 64);
        }

        // 15. Flip flash. Full-screen white overlay that fades
        //     over FLIP_FLASH_SECONDS.
        if self.flip_flash_timer > 0.0 && !self.cfg_reduce_motion {
            let t = self.flip_flash_timer / FLIP_FLASH_SECONDS;
            let alpha = t * 0.45;
            let r = BG_OUTER_R;
            out.push(Vertex::rgba([-r, -r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([ r, -r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([ r,  r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([-r, -r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([ r,  r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([-r,  r], [1.0, 1.0, 1.0], alpha));
        }

        // 16. Milestone flashes.
        for m in &self.milestones {
            let t = (m.age / m.life).clamp(0.0, 1.0);
            let alpha = (1.0 - t).powf(1.8);
            let px = 0.0090 * (1.0 + t * 0.35) * self.cfg_ui_scale;
            let w = text_width(&m.text, px);
            let y = -0.40 - t * 0.18;
            push_text_alpha(out, &m.text, -w * 0.5, y, px,
                [1.0, 1.0, 1.0], alpha);
        }

        // Bassdrop flash. Composite trigger fades a white full
        // screen overlay proportional to its current strength.
        let flash_bd = self.generator.flash_alpha();
        if flash_bd > 0.001 && !self.cfg_reduce_motion {
            let r = BG_OUTER_R;
            let alpha = flash_bd * 0.55;
            out.push(Vertex::rgba([-r, -r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([ r, -r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([ r,  r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([-r, -r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([ r,  r], [1.0, 1.0, 1.0], alpha));
            out.push(Vertex::rgba([-r,  r], [1.0, 1.0, 1.0], alpha));
        }

        // 17. Close-call counter HUD. Pinned near the top-left of
        //     the default 16:9 visible area. Ultra-wide displays
        //     render it slightly inboard which is fine because
        //     the rest of the playfield is far smaller than the
        //     visible extent on those aspect ratios.
        if self.close_call_count > 0 {
            let text = format!("CLOSE: {}", self.close_call_count);
            let px = 0.0050 * self.cfg_ui_scale;
            push_text(out, &text, -1.55, -0.95, px, self.palette.accent);
        }
        // Authoring reminder: `#[ignore_collisions]` silently
        // neuters death. Drawn as a small warm-red chip in the
        // bottom-left so an author never loses track of it while
        // iterating.
        if self.ignore_collisions {
            let label = "NO CLIP";
            let px = 0.0060 * self.cfg_ui_scale;
            let w = text_width(label, px);
            let margin = 0.035;
            let x = -1.55 + margin;
            let y = 0.88 + margin;
            // Soft dim background so the label stays legible on
            // bright palettes without needing to outline every
            // glyph.
            let pad = 0.010;
            push_quad_polar_alpha_block(
                out,
                x - pad, y - pad * 0.5,
                x + w + pad, y + text_height(px) + pad * 0.5,
                [0.18, 0.05, 0.08], 0.55);
            push_text(out, label, x, y, px, [1.0, 0.40, 0.45]);
        }
    }

    /// Draw the cursor triangle with squash and stretch driven
    /// by angular velocity. Fast turns widen the base and lean
    /// it forward slightly, while the radial height is shortened
    /// so the cursor reads as "leaning into the turn".
    fn draw_cursor(&self, out: &mut Vec<Vertex>, cam: f32, dead_t: f32) {
        let pa = self.player_ang + cam;
        let pc = mix_color(self.palette.player, [0.6, 0.6, 0.6], dead_t);

        let speed = (self.player_ang_vel / PLAYER_ROT_SPEED).clamp(-1.5, 1.5);
        let abs_s = speed.abs().min(1.0);
        let stretch = 1.0 + 0.22 * abs_s;
        let squish  = 1.0 - 0.16 * abs_s;
        let lean    = 0.10 * speed;

        let r_tip  = PLAYER_RADIUS + PLAYER_HEIGHT * squish;
        let r_base = PLAYER_RADIUS;
        let hb = PLAYER_HALF_BASE * stretch;

        let tip_ang = pa + lean * 0.10;
        let bl_ang  = pa - hb + lean * 0.30;
        let br_ang  = pa + hb + lean * 0.30;

        let tip = [tip_ang.cos() * r_tip, tip_ang.sin() * r_tip];
        let lp  = [bl_ang.cos()  * r_base, bl_ang.sin()  * r_base];
        let rp  = [br_ang.cos()  * r_base, br_ang.sin()  * r_base];
        out.push(Vertex::opaque(tip, pc));
        out.push(Vertex::opaque(lp,  pc));
        out.push(Vertex::opaque(rp,  pc));

        // Soft glow halo just behind the cursor.
        push_hex_alpha(out,
            pa.cos() * (r_base + 0.01),
            pa.sin() * (r_base + 0.01),
            0.04, pc, 0.18);
    }

    fn highlight_enabled(&self) -> bool {
        match self.cfg_highlight {
            HighlightMode::Off => false,
            HighlightMode::On  => true,
            HighlightMode::Auto => {
                matches!(self.tier, Tier::Rookie | Tier::Casual | Tier::Adept)
            }
        }
    }

    /// Fake 3D: map a radial distance `r` through a mild
    /// perspective curve so distant walls look pulled in.
    fn radial_depth(&self, r: f32) -> f32 {
        if self.cfg_depth <= 0.0 { return r; }
        let k = self.cfg_depth.clamp(0.0, 1.0);
        let norm = (r / WALL_SPAWN_R).clamp(0.0, 1.0);
        let squash = 1.0 - k * 0.35 * norm * norm;
        r * squash
    }

    fn dead_intensity(&self) -> f32 {
        match self.state {
            RunState::Alive    => 0.0,
            RunState::Dying    => {
                let total = DEATH_SLOWMO_SECONDS + DEATH_FREEZE_SECONDS;
                let elapsed = match self.death_phase {
                    DyingPhase::SlowMo => DEATH_SLOWMO_SECONDS - self.death_slowmo_timer,
                    DyingPhase::Freeze => DEATH_SLOWMO_SECONDS
                                        + (DEATH_FREEZE_SECONDS - self.death_timer),
                };
                (elapsed / total).clamp(0.0, 1.0)
            }
            RunState::GameOver => 1.0,
        }
    }
}

/// Compute normalized song progress 0..1. Uses audio position
/// directly when a track is playing; otherwise falls back to a
/// conservative estimate based on run time and a 120 second
/// reference duration.
fn beat_progress(audio: &Audio, run_time: f32) -> f32 {
    let dur = audio.music_duration();
    if audio.has_music() && dur > 0.1 {
        (audio.music_position() / dur).clamp(0.0, 1.0)
    } else {
        (run_time / 120.0).clamp(0.0, 1.0)
    }
}

fn decaying_beat_pulse(phase: f32) -> f32 {
    let p = phase.clamp(0.0, 1.0);
    let k = 1.0 - p;
    k * k * k
}

fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

fn mix_color(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn mul_color(a: [f32; 3], k: f32) -> [f32; 3] {
    [(a[0] * k).clamp(0.0, 1.0),
     (a[1] * k).clamp(0.0, 1.0),
     (a[2] * k).clamp(0.0, 1.0)]
}

fn brighten(a: [f32; 3], add: f32) -> [f32; 3] {
    [(a[0] + add).clamp(0.0, 1.0),
     (a[1] + add).clamp(0.0, 1.0),
     (a[2] + add).clamp(0.0, 1.0)]
}

fn rotate_hue(rgb: [f32; 3], angle: f32) -> [f32; 3] {
    if angle == 0.0 { return rgb; }
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let nr = r * (0.299 + 0.701 * cos_a + 0.168 * sin_a)
           + g * (0.587 - 0.587 * cos_a + 0.330 * sin_a)
           + b * (0.114 - 0.114 * cos_a - 0.497 * sin_a);
    let ng = r * (0.299 - 0.299 * cos_a - 0.328 * sin_a)
           + g * (0.587 + 0.413 * cos_a + 0.035 * sin_a)
           + b * (0.114 - 0.114 * cos_a + 0.292 * sin_a);
    let nb = r * (0.299 - 0.300 * cos_a + 1.250 * sin_a)
           + g * (0.587 - 0.588 * cos_a - 1.050 * sin_a)
           + b * (0.114 + 0.886 * cos_a - 0.203 * sin_a);
    [nr.clamp(0.0, 1.0), ng.clamp(0.0, 1.0), nb.clamp(0.0, 1.0)]
}

fn push_wedge(out: &mut Vec<Vertex>, a0: f32, span: f32, outer: f32, color: [f32; 3]) {
    let a1 = a0 + span;
    out.push(Vertex::opaque([0.0, 0.0],                         color));
    out.push(Vertex::opaque([a0.cos() * outer, a0.sin() * outer], color));
    out.push(Vertex::opaque([a1.cos() * outer, a1.sin() * outer], color));
}

/// Wedge with a radial color gradient: center vertex gets
/// `center_color`, the two outer vertices get `outer_color`. The
/// rasterizer interpolates between them across the triangle so
/// the playfield reads as a tunnel receding into the depths
/// rather than a flat fan.
fn push_wedge_gradient(
    out: &mut Vec<Vertex>, a0: f32, span: f32, outer: f32,
    center_color: [f32; 3], outer_color: [f32; 3],
) {
    let a1 = a0 + span;
    out.push(Vertex::opaque([0.0, 0.0], center_color));
    out.push(Vertex::opaque([a0.cos() * outer, a0.sin() * outer], outer_color));
    out.push(Vertex::opaque([a1.cos() * outer, a1.sin() * outer], outer_color));
}

fn push_quad_polar(out: &mut Vec<Vertex>, a0: f32, a1: f32, r0: f32, r1: f32, color: [f32; 3]) {
    let p00 = [a0.cos() * r0, a0.sin() * r0];
    let p01 = [a1.cos() * r0, a1.sin() * r0];
    let p10 = [a0.cos() * r1, a0.sin() * r1];
    let p11 = [a1.cos() * r1, a1.sin() * r1];
    out.push(Vertex::opaque(p00, color));
    out.push(Vertex::opaque(p01, color));
    out.push(Vertex::opaque(p11, color));
    out.push(Vertex::opaque(p00, color));
    out.push(Vertex::opaque(p11, color));
    out.push(Vertex::opaque(p10, color));
}

/// Translucent version of `push_quad_polar`. Used for the
/// fresh-spawn flash strip so it blends into the wall color
/// as it fades out.
fn push_quad_polar_alpha(
    out: &mut Vec<Vertex>, a0: f32, a1: f32, r0: f32, r1: f32,
    color: [f32; 3], alpha: f32,
) {
    let p00 = [a0.cos() * r0, a0.sin() * r0];
    let p01 = [a1.cos() * r0, a1.sin() * r0];
    let p10 = [a0.cos() * r1, a0.sin() * r1];
    let p11 = [a1.cos() * r1, a1.sin() * r1];
    out.push(Vertex::rgba(p00, color, alpha));
    out.push(Vertex::rgba(p01, color, alpha));
    out.push(Vertex::rgba(p11, color, alpha));
    out.push(Vertex::rgba(p00, color, alpha));
    out.push(Vertex::rgba(p11, color, alpha));
    out.push(Vertex::rgba(p10, color, alpha));
}

/// Thin radial streak used for the speed-lines overlay. Emitted
/// as a capsule-like quad; the far end fades to zero alpha so the
/// streak has a visible tail without an abrupt edge.
fn push_radial_streak(
    out: &mut Vec<Vertex>, ang: f32, r_near: f32, r_far: f32,
    thick: f32, color: [f32; 3], alpha: f32,
) {
    let c = ang.cos(); let s = ang.sin();
    let px0 = c * r_near; let py0 = s * r_near;
    let px1 = c * r_far;  let py1 = s * r_far;
    let perp_x = -s * thick;
    let perp_y =  c * thick;
    out.push(Vertex::rgba([px0 + perp_x, py0 + perp_y], color, alpha));
    out.push(Vertex::rgba([px0 - perp_x, py0 - perp_y], color, alpha));
    out.push(Vertex::rgba([px1 + perp_x, py1 + perp_y], color, 0.0));
    out.push(Vertex::rgba([px0 - perp_x, py0 - perp_y], color, alpha));
    out.push(Vertex::rgba([px1 + perp_x, py1 + perp_y], color, 0.0));
    out.push(Vertex::rgba([px1 - perp_x, py1 - perp_y], color, 0.0));
}

/// Triangle fan segment with varying outer radius at each
/// end. Used by the center polygon renderer so the polygon
/// shape traces out correctly for any fractional side count.
fn push_wedge_varying(
    out: &mut Vec<Vertex>, a0: f32, a1: f32,
    r0: f32, r1: f32, color: [f32; 3],
) {
    out.push(Vertex::opaque([0.0, 0.0], color));
    out.push(Vertex::opaque([a0.cos() * r0, a0.sin() * r0], color));
    out.push(Vertex::opaque([a1.cos() * r1, a1.sin() * r1], color));
}

/// Annular quad where inner and outer radii differ between
/// the two angular bounds. Used for the center ring when
/// the underlying polygon has a non-uniform radius.
fn push_quad_polar_varying(
    out: &mut Vec<Vertex>, a0: f32, a1: f32,
    r_inner_0: f32, r_inner_1: f32,
    r_outer_0: f32, r_outer_1: f32,
    color: [f32; 3],
) {
    let p00 = [a0.cos() * r_inner_0, a0.sin() * r_inner_0];
    let p01 = [a1.cos() * r_inner_1, a1.sin() * r_inner_1];
    let p10 = [a0.cos() * r_outer_0, a0.sin() * r_outer_0];
    let p11 = [a1.cos() * r_outer_1, a1.sin() * r_outer_1];
    out.push(Vertex::opaque(p00, color));
    out.push(Vertex::opaque(p01, color));
    out.push(Vertex::opaque(p11, color));
    out.push(Vertex::opaque(p00, color));
    out.push(Vertex::opaque(p11, color));
    out.push(Vertex::opaque(p10, color));
}

/// Distance from origin to the edge of a regular polygon
/// with `n` sides inscribed in a circle of radius `r`,
/// evaluated at angle `theta` (in radians, measured from
/// +X axis). Integer `n` gives a crisp polygon outline;
/// fractional `n` linearly interpolates the radii of the
/// two nearest integer shapes so a runtime morph reads as
/// a smooth transition.
///
/// Minimum is clamped to `n >= 3` because anything lower
/// does not describe a closed polygon.
fn polygon_radius_at(theta: f32, n: f32, r: f32) -> f32 {
    let n_clamped = n.max(3.0);
    let n_lo = n_clamped.floor();
    let n_hi = n_lo + 1.0;
    let t = n_clamped - n_lo;
    let r_lo = poly_r_integer(theta, n_lo, r);
    let r_hi = poly_r_integer(theta, n_hi, r);
    r_lo * (1.0 - t) + r_hi * t
}

/// Closed form radius for an integer regular polygon.
/// Algorithm: figure out which edge the direction `theta`
/// crosses, measure the angle between that direction and
/// the edge midpoint, then use `R * cos(pi/n) / cos(phi)`
/// where `phi` is that relative angle.
fn poly_r_integer(theta: f32, n: f32, r: f32) -> f32 {
    let two_pi = TAU;
    let t = theta.rem_euclid(two_pi);
    let k = (t * n / two_pi).floor();
    let theta_local = t - two_pi * (k + 0.5) / n;
    // Defensive floor on the denominator. For valid theta
    // the cos is already bounded away from zero, but a
    // clamp stops a bad float from producing infinity.
    let denom = theta_local.cos().max(0.05);
    r * (std::f32::consts::PI / n).cos() / denom
}

/// Axis-aligned translucent block. The existing
/// `push_quad_polar*` helpers all operate in polar space;
/// this variant takes four screen-space coordinates so
/// HUD chips can draw on top of the playfield without
/// fighting the radial geometry.
fn push_quad_polar_alpha_block(
    out: &mut Vec<Vertex>,
    x0: f32, y0: f32, x1: f32, y1: f32,
    color: [f32; 3], alpha: f32,
) {
    out.push(Vertex::rgba([x0, y0], color, alpha));
    out.push(Vertex::rgba([x1, y0], color, alpha));
    out.push(Vertex::rgba([x1, y1], color, alpha));
    out.push(Vertex::rgba([x0, y0], color, alpha));
    out.push(Vertex::rgba([x1, y1], color, alpha));
    out.push(Vertex::rgba([x0, y1], color, alpha));
}