//! Continuous, time driven obstacle generator.
//!
//! This revision of the generator supports the entire v2/v3
//! DSL trigger set (`speedwarp`, `glitch`, `shake`, `zoom`,
//! `invert`, `strobe`) alongside the historical `flip` /
//! `tilt` / `pulse` / `speedMult` / `hueShift` triggers, plus
//! `repeat N { ... }` blocks and `local` variable scopes.
//! Sections whose `at` values use an exotic timestamp format
//! are still resolved through [`Section::progress_threshold`].
//!
//! # Changes from the previous revision
//!
//! Three architectural fixes landed together:
//!
//! * **Per axis speedwarp state.** Each of the four
//!   `:speedwarp` axes now tracks its own value and timer,
//!   so piped triggers like `trigger :speedwarp { walls = 1.5,
//!   duration = 6.0 } |> :speedwarp { rotation = 1.2, duration
//!   = 2.0 }` apply both axes simultaneously with their own
//!   independent timeouts. The old code kept a single timer
//!   shared by all four axes, which forced the second pipe
//!   element to overwrite the first.
//!
//! * **Consistent "max wins" stacking policy.** Every
//!   trigger with an intensity and a duration (glitch, shake,
//!   strobe, invert, tilt, speedMult, hueShift) updates its
//!   intensity to the newly authored value but extends the
//!   timer to `max(current, new)`. An author can never lose
//!   authored time by issuing a follow up stronger trigger.
//!
//! * **Authored durations on legacy triggers.** `Tilt`,
//!   `SpeedMult` and `HueShift` now accept a `duration` field
//!   (see `dsl/ast.rs`). The generator honours it and eases
//!   the state back to neutral once the timer expires. The
//!   old hardcoded `SPEED_MULT_DURATION` and
//!   `HUE_SHIFT_DURATION` constants are gone.
//!
//! # Safety fixes for unplayable chains
//!
//! Two gameplay level fixes accompany the trigger rework:
//!
//! * **Hard section boundary.** When the active section
//!   changes, every queued wall, trigger, or rule whose
//!   scheduled spawn time is more than
//!   `SECTION_BOUNDARY_EPSILON` seconds in the future is
//!   purged. Items about to fire within that epsilon are
//!   allowed to finish, so the transition does not visibly
//!   glitch. This prevents the "tail of previous section
//!   mixes with head of next section" chain that could close
//!   every slot of the playfield at once.
//!
//! * **Minimum cooldown between emits.** The per tier
//!   density multiplier could crush the gap between patterns
//!   below the time the player needs to reach an adjacent
//!   slot. We now enforce a floor of
//!   `min_pattern_gap_seconds()`, computed from the player's
//!   slot traverse time with a 1.35x safety factor. The
//!   authored soft gap still applies; the floor kicks in only
//!   when the computed soft gap would be smaller.
//!
//! Lookahead is reduced from 2.5 seconds to 0.8 seconds so
//! newly entered sections respond more promptly to section
//! transitions. A wall's flight time from the spawn radius to
//! the center is around 3 seconds, so the smaller lookahead
//! never starves the simulation of walls.

use crate::dsl::ast::{
    Anim, LevelAst, Stmt, TimestampFormat, TriggerSpec,
};
use crate::gend::obstacle::{materialize, Materialized};
use crate::gend::prng::Prng;

/// Maximum authored tilt in radians (converted from the 2.5
/// degree DSL limit). The generator clamps one more time to
/// be defensive against malformed triggers.
const MAX_TILT_DEG: f32 = 30.0;
/// Rate at which the smoothed `tilt` follows `tilt_target`.
/// Measured in 1/s: higher values mean faster convergence.
const TILT_EASE_RATE: f32 = 6.0;

/// Short visual pulse fired by `:pulse`. Duration is fixed so
/// the DSL can keep the variant parameter-less.
const PULSE_DURATION:      f32 = 0.6;

/// How far ahead the generator schedules walls and triggers.
/// Lower values make section transitions feel sharper at the
/// cost of tighter safety margin on stalled frames. 0.8s is
/// chosen because typical beat_seconds at 150 BPM is ~0.4s,
/// so this still covers two beats of pre-scheduling.
const LOOKAHEAD_SECONDS: f32 = 0.8;
/// Hard cap on the number of enqueue loop iterations per frame,
/// as a backstop against pathological states where the enqueue
/// loop would otherwise spin.
const ENQUEUE_BUDGET:    u32 = 64;
/// Default soft gap between back-to-back emits, in beats. This
/// gets divided by `density_mult`, so at very high density the
/// soft gap approaches zero. The `min_pattern_gap_seconds`
/// floor then provides the true minimum.
const BASE_GAP_BEATS:    f32 = 0.4;

/// Tolerance for "still about to fire" queued events when a
/// section boundary is crossed. Items with `spawn_time <=
/// clock + this` are allowed to play out; items farther in the
/// future are discarded. 0.15 seconds is short enough that a
/// queued trigger for the old section never leaks a noticeable
/// amount into the new section, yet long enough that a wall
/// about to spawn next frame still spawns cleanly.
const SECTION_BOUNDARY_EPSILON: f32 = 0.15;

/// Reference player rotation speed used to estimate minimum
/// safe cooldown between patterns. Must track
/// `PLAYER_ROT_SPEED` in `game.rs`; duplicated here so the
/// generator does not have to depend on the game module.
const PLAYER_ROT_SPEED_REF: f32 = 6.5;
/// Multiplicative safety factor applied to the slot traverse
/// time to produce the minimum cooldown. 1.35 gives the player
/// a small margin of error even at the hardest tiers.
const PATTERN_GAP_SAFETY: f32 = 1.35;

#[derive(Clone, Copy)]
struct QueuedWall {
    spawn_time:     f32,
    slot:           u32,
    /// Side count at the time this wall was materialized.
    /// Stored per wall so a polygon morph triggered between
    /// materialization and spawn still projects the wall
    /// onto the geometry it was designed for.
    slot_count:     u32,
    thickness_mult: f32,
    length_seconds: f32,
}

/// Trigger awaiting its scheduled clock time. Triggers used to
/// be applied the instant the enqueue cursor reached them,
/// which made their firing time depend on whatever lookahead
/// the previous section had accumulated; pulling them through
/// the same clock as walls eliminates that drift.
#[derive(Clone)]
struct QueuedTrigger {
    spawn_time: f32,
    spec:       TriggerSpec,
}

/// Rule related statement awaiting its scheduled clock time.
/// Every `rule`, `revert`, `push`, or `pop` statement is queued
/// here so gameplay effects tied to a beat land on that beat
/// instead of at lookahead time.
#[derive(Clone)]
struct QueuedRule {
    spawn_time: f32,
    stmt:       Stmt,
}

/// Flattened statement consumed by the enqueue loop. Repeats
/// and local blocks are expanded into a single stream so the
/// core loop stays simple. Rule statements travel here verbatim
/// as `Rule(Stmt)` so the generator can schedule them against
/// the clock the same way it schedules triggers.
#[derive(Clone, Debug)]
enum FlatStmt {
    Emit(crate::dsl::ast::ObstacleSpec),
    Wait(u32),
    Trigger(TriggerSpec),
    Rule(Stmt),
}

pub struct Generator {
    ast: LevelAst,
    rng: Prng,

    clock:        f32,
    beat_seconds: f32,

    active:            usize,
    flat:              Vec<FlatStmt>,
    cursor:            usize,

    queue:             Vec<QueuedWall>,
    trigger_queue:     Vec<QueuedTrigger>,
    rule_queue:        Vec<QueuedRule>,
    last_enqueue_time: f32,

    density_mult: f32,

    flip_request: bool,

    /// Target side count. Walls spawning now use this value.
    /// Updated by the `Morph` trigger and by any future DSL
    /// statement that wants to change the playfield shape.
    target_sides: u32,
    /// Fractional current side count for visual rendering.
    /// Eases toward `target_sides` over the morph window.
    current_sides: f32,
    /// Total duration of the currently active morph, zero
    /// when no morph is in progress.
    morph_duration: f32,
    /// Remaining time of the active morph. Reaches zero at
    /// the end of the ease, at which point `current_sides`
    /// is snapped to `target_sides`.
    morph_timer: f32,

    // --- Tilt state ---
    //
    // The `tilt_target` is the authored radian offset while the
    // timer is positive; once the timer expires it is forced to
    // zero so the camera eases back to level. `tilt` is the
    // smoothed value the renderer actually uses.
    tilt:          f32,
    tilt_target:   f32,
    tilt_timer:    f32,
    tilt_duration: f32,

    /// Pitch component of the current camera tilt. Stored as
    /// smoothed radians; the shader reads it once per frame
    /// from the renderer. Eases toward `tilt_pitch_target` at
    /// `TILT_EASE_RATE`, same as the other two axes.
    tilt_pitch:         f32,
    tilt_pitch_target:  f32,
    /// Yaw component of the current camera tilt.
    tilt_yaw:           f32,
    tilt_yaw_target:    f32,

    // --- SpeedMult state ---
    //
    // Legacy `trigger speedMult` / `:speedMult`. `speed_mult`
    // returns to 1.0 when the timer expires.
    speed_mult:     f32,
    speed_timer:    f32,
    speed_duration: f32,

    // --- HueShift state ---
    hue_extra:     f32,
    hue_timer:     f32,
    hue_duration:  f32,

    // --- Pulse state ---
    pulse_timer:  f32,

    // --- SpeedWarp state ---
    //
    // One timer per axis so piped speedwarps can override
    // individual axes without touching the others. Each
    // `*_value` is the currently effective multiplier, falling
    // back to 1.0 (neutral) when the matching timer hits zero.
    warp_walls_value:    f32,
    warp_walls_timer:    f32,
    warp_rotation_value: f32,
    warp_rotation_timer: f32,
    warp_cursor_value:   f32,
    warp_cursor_timer:   f32,
    warp_music_value:    f32,
    warp_music_timer:    f32,

    // --- Glitch state ---
    glitch_strength: f32,
    glitch_timer:    f32,
    glitch_duration: f32,

    // --- Shake state ---
    shake_strength: f32,
    shake_timer:    f32,
    shake_duration: f32,

    // --- Zoom state ---
    zoom_current:  f32,
    zoom_from:     f32,
    zoom_target:   f32,
    zoom_elapsed:  f32,
    zoom_duration: f32,
    zoom_anim:     Anim,

    // --- Invert state ---
    invert_timer: f32,
    invert_duration: f32,

    // --- Strobe state ---
    strobe_rate:  f32,
    strobe_timer: f32,
    strobe_duration: f32,

    spin_rate:           f32,
    spin_timer:          f32,

    bounce_amplitude:    f32,
    bounce_timer:        f32,

    freeze_timer:        f32,

    punch_active:        bool,
    punch_strength:      f32,
    punch_elapsed:       f32,
    punch_duration:      f32,

    invert_colors_timer:    f32,
    invert_colors_duration: f32,

    grayscale_strength:  f32,
    grayscale_timer:     f32,
    grayscale_duration:  f32,

    shockwave_strength:  f32,
    shockwave_elapsed:   f32,
    shockwave_duration:  f32,
    shockwave_active:    bool,

    fog_near:            f32,
    fog_far:             f32,
    fog_timer:           f32,
    fog_duration:        f32,

    outline_thickness:   f32,
    outline_timer:       f32,
    outline_duration:    f32,

    pending_centerburst: Option<f32>,
    pending_ringburst:   Option<(u32, f32)>,
    pending_flash:       f32,
    flash_timer:         f32,
    section_loop_count: u32,

    /// Currently active user post shader: slot index into
    /// `LevelAst::shaders` plus four float parameters. Set
    /// by `PostShader`, cleared by `PostShaderOff`, reset on
    /// section change because the host reads this every
    /// frame and decides whether to swap the post pipeline.
    current_post_shader: Option<(u8, [f32; 4])>,
}

impl Generator {
    pub fn new(ast: LevelAst, density_mult: f32, beat_seconds: f32) -> Self {
        let seed = ast.generation.seed;
        let initial_sides = ast.generation.sides.clamp(3, 12);
        let mut g = Generator {
            ast,
            rng: Prng::new(seed),
            clock: 0.0,
            beat_seconds: beat_seconds.max(0.05),
            active: 0,
            flat: Vec::new(),
            cursor: 0,
            queue: Vec::with_capacity(128),
            last_enqueue_time: 0.0,
            trigger_queue: Vec::with_capacity(32),
            density_mult: density_mult.max(0.1),
            flip_request: false,
            target_sides:   initial_sides,
            current_sides:  initial_sides as f32,
            morph_duration: 0.0,
            morph_timer:    0.0,
            tilt: 0.0,
            tilt_target: 0.0,
            tilt_timer: 0.0,
            tilt_duration: 1.0,

            tilt_pitch:        0.0,
            tilt_pitch_target: 0.0,
            tilt_yaw:          0.0,
            tilt_yaw_target:   0.0,

            speed_mult: 1.0,
            speed_timer: 0.0,
            speed_duration: 1.0,

            hue_extra: 0.0,
            hue_timer: 0.0,
            hue_duration: 1.0,

            pulse_timer: 0.0,

            warp_walls_value: 1.0,    warp_walls_timer: 0.0,
            warp_rotation_value: 1.0, warp_rotation_timer: 0.0,
            warp_cursor_value: 1.0,   warp_cursor_timer: 0.0,
            warp_music_value: 1.0,    warp_music_timer: 0.0,

            glitch_strength: 0.0,
            glitch_timer: 0.0,
            glitch_duration: 1.0,

            shake_strength: 0.0,
            shake_timer:    0.0,
            shake_duration: 1.0,

            zoom_current: 1.0,
            zoom_from: 1.0,
            zoom_target: 1.0,
            zoom_elapsed: 0.0,
            zoom_duration: 0.0,
            zoom_anim: Anim::Linear,

            invert_timer: 0.0,
            invert_duration: 1.0,

            strobe_rate: 0.0,
            strobe_timer: 0.0,
            strobe_duration: 1.0,
            spin_rate: 0.0, spin_timer: 0.0,
            bounce_amplitude: 0.0, bounce_timer: 0.0,
            freeze_timer: 0.0,
            punch_active: false, punch_strength: 0.0,
            punch_elapsed: 0.0, punch_duration: 1.0,
            invert_colors_timer: 0.0, invert_colors_duration: 1.0,
            grayscale_strength: 0.0, grayscale_timer: 0.0,
            grayscale_duration: 1.0,
            shockwave_strength: 0.0, shockwave_elapsed: 0.0,
            shockwave_duration: 1.0, shockwave_active: false,
            fog_near: 0.0, fog_far: 0.0,
            fog_timer: 0.0, fog_duration: 1.0,
            outline_thickness: 0.0, outline_timer: 0.0,
            outline_duration: 1.0,
            pending_centerburst: None,
            pending_ringburst: None,
            pending_flash: 0.0, flash_timer: 0.0,
            rule_queue: Vec::with_capacity(16),
            section_loop_count: 0,
            current_post_shader: None,
        };
        g.rebuild_flat();
        g
    }
    /// Minimum gap between two consecutive emits, in seconds.
    /// Derived from the slot traverse time at the reference
    /// player rotation speed, with a safety factor so the
    /// player never has to pixel-perfect their timing to
    /// transition between patterns. Independent of tier: at
    /// Rookie the slower player enjoys extra breathing room,
    /// at Expert+ the faster player is not punished but the
    /// cushion still prevents impossible chains.
    fn min_pattern_gap_seconds(&self) -> f32 {
        let sides = self.target_sides.max(3) as f32;
        let slot_angle = std::f32::consts::TAU / sides;
        (slot_angle / PLAYER_ROT_SPEED_REF) * PATTERN_GAP_SAFETY
    }

    /// Drop every queued wall, trigger, and rule statement
    /// scheduled for a time strictly after
    /// `clock + SECTION_BOUNDARY_EPSILON`. Called on section
    /// changes so events queued by the previous section do not
    /// leak into the new section's airspace.
    ///
    /// Items scheduled within the epsilon window are allowed
    /// through because they are visually about to happen
    /// anyway; dropping them would create a one frame jerk in
    /// the wall stream.
    fn clear_future_events(&mut self) {
        let cutoff = self.clock + SECTION_BOUNDARY_EPSILON;
        self.queue.retain(|w| w.spawn_time <= cutoff);
        self.trigger_queue.retain(|t| t.spawn_time <= cutoff);
        self.rule_queue.retain(|r| r.spawn_time <= cutoff);
    }

    pub fn update(&mut self, dt: f32, progress_seconds: f32, duration_seconds: f32) {
        self.clock += dt;

        // Polygon morph ease. The timer counts down so we can tell
        // when the morph is done; `current_sides` chases the target
        // with an exponential blend calibrated so ~95% of the change
        // lands within `morph_duration` seconds.
        if self.morph_timer > 0.0 {
            self.morph_timer -= dt;
            if self.morph_timer < 0.0 { self.morph_timer = 0.0; }
        }
        let target = self.target_sides as f32;
        if (self.current_sides - target).abs() > 0.005 {
            let rate = if self.morph_duration > 0.01 {
                3.0 / self.morph_duration
            } else {
                20.0
            };
            let blend = 1.0 - (-rate * dt).exp();
            self.current_sides += (target - self.current_sides) * blend;
        } else {
            self.current_sides = target;
        }

        // --- Legacy timers ---
        if self.speed_timer > 0.0 {
            self.speed_timer -= dt;
            if self.speed_timer <= 0.0 {
                self.speed_timer = 0.0;
                self.speed_mult = 1.0;
            }
        }
        if self.hue_timer > 0.0 {
            self.hue_timer -= dt;
            if self.hue_timer <= 0.0 {
                self.hue_timer = 0.0;
                self.hue_extra = 0.0;
            }
        }
        if self.pulse_timer > 0.0 {
            self.pulse_timer -= dt;
            if self.pulse_timer < 0.0 { self.pulse_timer = 0.0; }
        }

        // Tilt timer and smoothing. When a finite duration
        // expires, all three axes return to zero together.
        // Sticky tilts (timer 0) just hold their current
        // targets until another Tilt trigger overrides them.
        if self.tilt_timer > 0.0 {
            self.tilt_timer -= dt;
            if self.tilt_timer <= 0.0 {
                self.tilt_timer = 0.0;
                self.tilt_target       = 0.0;
                self.tilt_pitch_target = 0.0;
                self.tilt_yaw_target   = 0.0;
            }
        }
        let blend = 1.0 - (-TILT_EASE_RATE * dt).exp();
        self.tilt       += (self.tilt_target       - self.tilt)       * blend;
        self.tilt_pitch += (self.tilt_pitch_target - self.tilt_pitch) * blend;
        self.tilt_yaw   += (self.tilt_yaw_target   - self.tilt_yaw)   * blend;

        // --- Per axis speedwarp timers ---
        //
        // Each axis independently falls back to 1.0 when its
        // timer hits zero, so piped speedwarps with different
        // axis coverage correctly release their respective
        // axes at different times.
        if self.warp_walls_timer > 0.0 {
            self.warp_walls_timer -= dt;
            if self.warp_walls_timer <= 0.0 {
                self.warp_walls_timer = 0.0;
                self.warp_walls_value = 1.0;
            }
        }
        if self.warp_rotation_timer > 0.0 {
            self.warp_rotation_timer -= dt;
            if self.warp_rotation_timer <= 0.0 {
                self.warp_rotation_timer = 0.0;
                self.warp_rotation_value = 1.0;
            }
        }
        if self.warp_cursor_timer > 0.0 {
            self.warp_cursor_timer -= dt;
            if self.warp_cursor_timer <= 0.0 {
                self.warp_cursor_timer = 0.0;
                self.warp_cursor_value = 1.0;
            }
        }
        if self.warp_music_timer > 0.0 {
            self.warp_music_timer -= dt;
            if self.warp_music_timer <= 0.0 {
                self.warp_music_timer = 0.0;
                self.warp_music_value = 1.0;
            }
        }

        // --- Glitch / Invert / Strobe / Shake timers ---
        if self.glitch_timer > 0.0 {
            self.glitch_timer -= dt;
            if self.glitch_timer <= 0.0 {
                self.glitch_timer = 0.0;
                self.glitch_strength = 0.0;
            }
        }
        if self.invert_timer > 0.0 {
            self.invert_timer -= dt;
            if self.invert_timer < 0.0 { self.invert_timer = 0.0; }
        }
        if self.strobe_timer > 0.0 {
            self.strobe_timer -= dt;
            if self.strobe_timer < 0.0 { self.strobe_timer = 0.0; }
        }

        if self.spin_timer > 0.0 {
            self.spin_timer -= dt;
            if self.spin_timer <= 0.0 {
                self.spin_timer = 0.0;
                self.spin_rate = 0.0;
            }
        }
        if self.bounce_timer > 0.0 {
            self.bounce_timer -= dt;
            if self.bounce_timer <= 0.0 {
                self.bounce_timer = 0.0;
                self.bounce_amplitude = 0.0;
            }
        }
        if self.freeze_timer > 0.0 {
            self.freeze_timer -= dt;
            if self.freeze_timer < 0.0 { self.freeze_timer = 0.0; }
        }
        if self.punch_active {
            self.punch_elapsed += dt;
            if self.punch_elapsed >= self.punch_duration {
                self.punch_active = false;
            }
        }
        if self.invert_colors_timer > 0.0 {
            self.invert_colors_timer -= dt;
            if self.invert_colors_timer < 0.0 { self.invert_colors_timer = 0.0; }
        }
        if self.grayscale_timer > 0.0 {
            self.grayscale_timer -= dt;
            if self.grayscale_timer <= 0.0 {
                self.grayscale_timer = 0.0;
                self.grayscale_strength = 0.0;
            }
        }
        if self.shockwave_active {
            self.shockwave_elapsed += dt;
            if self.shockwave_elapsed >= self.shockwave_duration {
                self.shockwave_active = false;
                self.shockwave_strength = 0.0;
            }
        }
        if self.fog_timer > 0.0 {
            self.fog_timer -= dt;
            if self.fog_timer <= 0.0 {
                self.fog_timer = 0.0;
                self.fog_near = 0.0;
                self.fog_far = 0.0;
            }
        }
        if self.outline_timer > 0.0 {
            self.outline_timer -= dt;
            if self.outline_timer <= 0.0 {
                self.outline_timer = 0.0;
                self.outline_thickness = 0.0;
            }
        }
        if self.flash_timer > 0.0 {
            self.flash_timer -= dt;
            if self.flash_timer < 0.0 { self.flash_timer = 0.0; }
        }

        if self.shake_timer > 0.0 {
            self.shake_timer -= dt;
            if self.shake_timer <= 0.0 {
                self.shake_timer = 0.0;
                self.shake_strength = 0.0;
                self.shake_duration = 1.0;
            }
        }

        // --- Zoom ---
        if self.zoom_duration > 0.0 {
            self.zoom_elapsed += dt;
            let t = (self.zoom_elapsed / self.zoom_duration).clamp(0.0, 1.0);
            let eased = self.zoom_anim.eval(t);
            self.zoom_current = self.zoom_from
                + (self.zoom_target - self.zoom_from) * eased;
            if t >= 1.0 {
                self.zoom_current = self.zoom_target;
                self.zoom_duration = 0.0;
            }
        }

        let progress = self.progress_normalized(progress_seconds, duration_seconds);
        self.update_active_section(progress);

        // Enqueue loop with stall detection.
        //
        // Triggers deliberately do not move `last_enqueue_time`:
        // they fire at whatever clock moment the preceding emit
        // / wait parked them on, and back-to-back triggers at
        // the same anchor are exactly what the `|>` pipe
        // expresses. The downside is that a section whose body
        // contains ONLY triggers (or wraps around before
        // hitting any emit) would spin forever here, scheduling
        // `ENQUEUE_BUDGET` copies of the same trigger per frame
        // and drowning the game in shake / flip / zoom.
        //
        // Fix: count iterations that made no temporal progress.
        // If we make it through a full lap of the flat without
        // advancing the clock, synthesise a beat of silence so
        // the section ticks forward at the BPM rate instead of
        // at the framerate.
        let mut budget = ENQUEUE_BUDGET;
        let mut stall = 0usize;
        let lap = self.flat.len().max(1);
        while self.last_enqueue_time - self.clock < LOOKAHEAD_SECONDS && budget > 0 {
            let before = self.last_enqueue_time;
            self.enqueue_next();
            if self.last_enqueue_time <= before + 1e-4 {
                stall += 1;
                if stall > lap {
                    self.last_enqueue_time = self.clock.max(before) + self.beat_seconds;
                    stall = 0;
                }
            } else {
                stall = 0;
            }
            budget -= 1;
        }

        // Pop every trigger whose scheduled clock time has
        // arrived. Done after the enqueue loop so a trigger
        // queued this very frame at `spawn_time = clock` still
        // fires this frame.
        let t = self.clock;
        let mut i = 0;
        while i < self.trigger_queue.len() {
            if self.trigger_queue[i].spawn_time <= t {
                let qt = self.trigger_queue.swap_remove(i);
                self.apply_trigger(qt.spec);
            } else {
                i += 1;
            }
        }
    }

    fn progress_normalized(&self, pos: f32, dur: f32) -> f32 {
        match self.ast.timestamp_format {
            TimestampFormat::Relative | TimestampFormat::Named(_) => {
                if dur > 0.1 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 }
            }
            TimestampFormat::TrackLength => {
                // `at` values live in absolute seconds; expose
                // progress directly for section matching.
                pos.max(0.0)
            }
            TimestampFormat::Beats { total } => {
                if dur > 0.1 {
                    let t = (pos / dur).clamp(0.0, 1.0);
                    t * total as f32
                } else { 0.0 }
            }
        }
    }

    fn update_active_section(&mut self, progress: f32) {
        let mut new_active = 0usize;
        for (i, s) in self.ast.sections.iter().enumerate() {
            if progress >= s.at { new_active = i; }
        }
        if new_active != self.active {
            self.active = new_active;
            self.rebuild_flat();
            self.cursor = 0;
            // Section boundaries are sharp. Without this reset
            // the new section's first statement would be
            // scheduled at `clock + leftover_lookahead`, where
            // `leftover_lookahead` is whatever buffer the old
            // section happened to push ahead. Walls already in
            // the wall queue still play out at their original
            // spawn times, so this only affects the anchor for
            // newly-enqueued content.
            self.last_enqueue_time = self.clock;

            // Drop every event that was queued by the previous
            // section but would land outside the small grace
            // window. This prevents gameplay-breaking "two
            // patterns on top of each other" chains at section
            // boundaries.
            self.clear_future_events();

            // Reset loop counter so triggers and rule ops
            // declared in the new section fire exactly once on
            // entry. Without this, a section entered twice in
            // the same run (which the current scheduler does
            // not do, but could in a future revision) would
            // swallow its own triggers.
            self.section_loop_count = 0;

            // Drop any user post shader when the section
            // changes. An author who wants the shader to
            // span multiple sections can re-issue the
            // trigger at the start of the next section;
            // forgetting to do so is the common case and a
            // clean reset is the safer default.
            self.current_post_shader = None;
        }
    }

    /// Flatten `repeat` and `local` blocks. Locals are discarded
    /// here because the parser already resolved every `@var`
    /// reference into a concrete value, so nothing downstream
    /// needs the variable list any more.
    fn rebuild_flat(&mut self) {
        self.flat.clear();
        let body = self.ast.sections[self.active].body.clone();
        flatten_stmts(&body, &mut self.flat);
    }

   /// Pull the next flat statement and schedule whatever it
    /// represents. Four kinds of statement exist:
    ///
    /// * `Emit` materialises an obstacle pattern into queued
    ///   walls and advances the enqueue clock by the pattern's
    ///   beat length plus the minimum cooldown.
    ///
    /// * `Wait` advances the enqueue clock by a number of
    ///   beats.
    ///
    /// * `Trigger` parks a `TriggerSpec` onto `trigger_queue`
    ///   to fire when the scheduled clock time arrives. Does
    ///   not advance the enqueue clock because triggers share
    ///   an anchor with their preceding emit / wait.
    ///
    /// * `Rule` parks a rule related statement onto
    ///   `rule_queue`, same semantics as triggers.
    ///
    /// Loop semantics: when the cursor wraps past the end of
    /// the flat stream, the loop counter increments. On any
    /// iteration after the first (counter > 0), `Trigger` and
    /// `Rule` statements are skipped rather than re-fired.
    /// Their slots still pass through so the enqueue clock
    /// advances exactly as it did on the first pass; this
    /// keeps the wall pattern rhythmically stable across
    /// iterations.
    fn enqueue_next(&mut self) {
        if self.flat.is_empty() {
            self.last_enqueue_time = self.last_enqueue_time.max(self.clock)
                + self.beat_seconds;
            return;
        }
        if self.cursor >= self.flat.len() {
            self.cursor = 0;
            self.section_loop_count = self.section_loop_count.saturating_add(1);
        }
        let stmt = self.flat[self.cursor].clone();
        self.cursor += 1;

        let first_pass = self.section_loop_count == 0;

        match stmt {
            FlatStmt::Emit(spec) => {
                let sides_now = self.target_sides;
                let Materialized { walls, duration_beats } =
                    materialize(&spec, sides_now, &mut self.rng);
                let anchor = self.last_enqueue_time.max(self.clock);
                for w in walls {
                    self.queue.push(QueuedWall {
                        spawn_time:     anchor + w.beat_offset * self.beat_seconds,
                        slot:           w.slot,
                        slot_count:     sides_now,
                        thickness_mult: w.thickness_mult,
                        length_seconds: w.length_seconds,
                    });
                }
                let soft_gap_s = (BASE_GAP_BEATS / self.density_mult).max(0.0)
                    * self.beat_seconds;
                let floor_gap_s = self.min_pattern_gap_seconds();
                let gap_s = soft_gap_s.max(floor_gap_s);
                self.last_enqueue_time = anchor
                    + duration_beats * self.beat_seconds
                    + gap_s;
            }
            FlatStmt::Wait(n) => {
                let scaled = (n as f32 / self.density_mult).max(0.0);
                let anchor = self.last_enqueue_time.max(self.clock);
                self.last_enqueue_time = anchor + scaled * self.beat_seconds;
            }
            FlatStmt::Trigger(t) => {
                // One shot on section entry only. Subsequent
                // passes through the flat stream skip the
                // trigger so authored effects do not re-fire
                // every time the wall pattern cycles. The
                // enqueue clock is not advanced here either
                // way, matching the "triggers share an anchor
                // with the preceding emit" rule.
                if first_pass {
                    let anchor = self.last_enqueue_time.max(self.clock);
                    self.trigger_queue.push(QueuedTrigger {
                        spawn_time: anchor,
                        spec:       t,
                    });
                }
            }
            FlatStmt::Rule(s) => {
                // Rule related statements also fire once only.
                //
                // This is important for `push` and `pop`:
                // re-firing a push on every loop iteration
                // would grow the rule stack without bound, and
                // re-firing a pop would unbalance it. Plain
                // `rule { ... }` is idempotent so one-shot is
                // still correct for it.
                //
                // Authors who want a rule to reapply every
                // iteration can place it inside an explicit
                // `repeat` block, where the intent is clear.
                if first_pass {
                    let anchor = self.last_enqueue_time.max(self.clock);
                    self.rule_queue.push(QueuedRule {
                        spawn_time: anchor,
                        stmt:       s,
                    });
                }
            }
        }
    }

    /// Pull every rule related statement whose scheduled time
    /// has arrived. The caller (game simulation) feeds each one
    /// into its `RuleEngine` to update the effective gameplay
    /// rules. Statements that are not yet due stay in the queue
    /// for a later frame.
    pub fn drain_rules<F: FnMut(&Stmt)>(&mut self, mut f: F) {
        let t = self.clock;
        let mut i = 0;
        while i < self.rule_queue.len() {
            if self.rule_queue[i].spawn_time <= t {
                let qr = self.rule_queue.swap_remove(i);
                f(&qr.stmt);
            } else {
                i += 1;
            }
        }
    }

    /// Apply one trigger to the generator's internal state.
    ///
    /// Stacking policy:
    ///
    /// * The intensity field (angle, factor, rate, strength)
    ///   is replaced by the newly authored value. Last writer
    ///   wins, matching the author's most recent intent.
    /// * The duration field extends the currently running
    ///   timer via `max(current, new_duration)`. An earlier
    ///   trigger's longer tail is never clipped by a shorter
    ///   follow up.
    ///
    /// `SpeedWarp` is the exception: each axis has its own
    /// value and timer, and an axis with `None` on the incoming
    /// trigger is left untouched.
    fn apply_trigger(&mut self, t: TriggerSpec) {
        eprintln!("[trig] apply {:?}", t);
        match t {
            TriggerSpec::Flip => { self.flip_request = true; }

            TriggerSpec::Tilt { angle, pitch, yaw, duration } => {
                let a = angle.clamp(-MAX_TILT_DEG, MAX_TILT_DEG);
                let p = pitch.clamp(-MAX_TILT_DEG, MAX_TILT_DEG);
                let y = yaw  .clamp(-MAX_TILT_DEG, MAX_TILT_DEG);
                self.tilt_target       = a.to_radians();
                self.tilt_pitch_target = p.to_radians();
                self.tilt_yaw_target   = y.to_radians();
                match duration {
                    Some(dur) => {
                        // Finite duration: extend the shared
                        // timer. Max-stacking preserves the
                        // longest window just like shake and
                        // glitch do.
                        let dur = dur.clamp(0.05, 60.0);
                        self.tilt_timer = self.tilt_timer.max(dur);
                        self.tilt_duration = self.tilt_duration.max(dur);
                    }
                    None => {
                        // Sticky tilt: force the timer to zero
                        // so `update` never drives the targets
                        // back to level automatically.
                        self.tilt_timer = 0.0;
                        self.tilt_duration = 1.0;
                    }
                }
            }

            TriggerSpec::Pulse => { self.pulse_timer = PULSE_DURATION; }

            TriggerSpec::SpeedMult { factor, duration } => {
                self.speed_mult = factor.clamp(0.5, 2.0);
                let dur = duration.clamp(0.05, 60.0);
                self.speed_timer = self.speed_timer.max(dur);
                self.speed_duration = self.speed_duration.max(dur);
            }

            TriggerSpec::HueShift { rate, duration } => {
                self.hue_extra = rate.clamp(-2.0, 2.0);
                let dur = duration.clamp(0.05, 60.0);
                self.hue_timer = self.hue_timer.max(dur);
                self.hue_duration = self.hue_duration.max(dur);
            }

            TriggerSpec::SpeedWarp { walls, rotation, cursor, music_scale, duration } => {
                let dur = duration.clamp(0.1, 20.0);
                if let Some(v) = walls {
                    self.warp_walls_value = v.clamp(0.0, 4.0);
                    self.warp_walls_timer = self.warp_walls_timer.max(dur);
                }
                if let Some(v) = rotation {
                    self.warp_rotation_value = v.clamp(0.0, 4.0);
                    self.warp_rotation_timer = self.warp_rotation_timer.max(dur);
                }
                if let Some(v) = cursor {
                    self.warp_cursor_value = v.clamp(0.0, 4.0);
                    self.warp_cursor_timer = self.warp_cursor_timer.max(dur);
                }
                if let Some(v) = music_scale {
                    self.warp_music_value = v.clamp(0.25, 4.0);
                    self.warp_music_timer = self.warp_music_timer.max(dur);
                }
            }

            TriggerSpec::Glitch { strength, duration } => {
                // Intensity takes the stronger of current and
                // new so two back-to-back glitches cannot be
                // less intense than either alone. Duration is
                // max-stacked.
                self.glitch_strength = self.glitch_strength.max(strength);
                let dur = duration.clamp(0.05, 10.0);
                self.glitch_timer = self.glitch_timer.max(dur);
                self.glitch_duration = self.glitch_duration.max(dur);
            }

            TriggerSpec::Shake { strength, duration } => {
                // Same "max intensity, max duration" rule as
                // glitch. This matches what pipes visually
                // produce and never loses authored time.
                self.shake_strength = strength.max(self.shake_strength);
                let d = duration.max(0.05);
                if d > self.shake_timer    { self.shake_timer    = d; }
                if d > self.shake_duration { self.shake_duration = d; }
            }

            TriggerSpec::Zoom { target, anim, duration } => {
                // Zoom intentionally restarts animation on every
                // trigger so a piped sequence of zooms reads as
                // a multi-step camera choreography rather than
                // a single blended motion.
                self.zoom_from     = self.zoom_current;
                self.zoom_target   = target;
                self.zoom_anim     = anim;
                self.zoom_elapsed  = 0.0;
                self.zoom_duration = duration;
            }

            TriggerSpec::Invert { duration } => {
                let dur = duration.clamp(0.05, 30.0);
                self.invert_timer    = self.invert_timer.max(dur);
                self.invert_duration = self.invert_duration.max(dur);
            }

            TriggerSpec::Strobe { rate, duration } => {
                // Rate is a display parameter and reads most
                // naturally as "last writer wins". Duration
                // still obeys the max policy.
                self.strobe_rate = rate;
                let dur = duration.clamp(0.05, 10.0);
                self.strobe_timer    = self.strobe_timer.max(dur);
                self.strobe_duration = self.strobe_duration.max(dur);
            }
            TriggerSpec::Spin { rate, duration } => {
                self.spin_rate = rate.clamp(-8.0, 8.0);
                let dur = duration.clamp(0.1, 20.0);
                self.spin_timer = self.spin_timer.max(dur);
            }

            TriggerSpec::Bounce { amplitude, duration } => {
                self.bounce_amplitude = amplitude.max(self.bounce_amplitude)
                    .clamp(0.0, 1.0);
                let dur = duration.clamp(0.1, 30.0);
                self.bounce_timer = self.bounce_timer.max(dur);
            }

            TriggerSpec::Freeze { duration } => {
                let dur = duration.clamp(0.05, 3.0);
                self.freeze_timer = self.freeze_timer.max(dur);
            }

            TriggerSpec::ZoomPunch { strength, duration } => {
                self.punch_active = true;
                self.punch_strength = strength.clamp(0.0, 2.0);
                self.punch_elapsed = 0.0;
                self.punch_duration = duration.clamp(0.05, 3.0);
            }

            TriggerSpec::InvertColors { duration } => {
                let dur = duration.clamp(0.05, 30.0);
                self.invert_colors_timer = self.invert_colors_timer.max(dur);
                self.invert_colors_duration = self.invert_colors_duration.max(dur);
            }

            TriggerSpec::Grayscale { strength, duration } => {
                self.grayscale_strength = strength.max(self.grayscale_strength)
                    .clamp(0.0, 1.0);
                let dur = duration.clamp(0.05, 30.0);
                self.grayscale_timer = self.grayscale_timer.max(dur);
                self.grayscale_duration = self.grayscale_duration.max(dur);
            }

            TriggerSpec::Shockwave { strength, duration } => {
                self.shockwave_active = true;
                self.shockwave_strength = strength.clamp(0.0, 2.0);
                self.shockwave_elapsed = 0.0;
                self.shockwave_duration = duration.clamp(0.1, 5.0);
            }

            TriggerSpec::Fog { near, far, duration } => {
                self.fog_near = near.clamp(0.0, 1.5);
                self.fog_far  = far.clamp(self.fog_near + 0.01, 1.5);
                let dur = duration.clamp(0.1, 60.0);
                self.fog_timer = self.fog_timer.max(dur);
                self.fog_duration = self.fog_duration.max(dur);
            }

            TriggerSpec::Outline { thickness, duration } => {
                self.outline_thickness = thickness.max(self.outline_thickness)
                    .clamp(0.0, 2.0);
                let dur = duration.clamp(0.1, 60.0);
                self.outline_timer = self.outline_timer.max(dur);
                self.outline_duration = self.outline_duration.max(dur);
            }

            TriggerSpec::Centerburst { strength, duration: _ } => {
                self.pending_centerburst = Some(strength.clamp(0.0, 2.0));
            }

            TriggerSpec::Ringburst { count, duration } => {
                let n = count.clamp(1, 8);
                let d = duration.clamp(0.2, 5.0);
                self.pending_ringburst = Some((n, d));
            }

            TriggerSpec::Bassdrop { strength, duration } => {
                // Composite: dispatch sub triggers through the
                // same path so each one stacks like it would
                // individually.
                let s = strength.clamp(0.0, 2.0);
                let d = duration.clamp(0.1, 5.0);
                self.apply_trigger(TriggerSpec::ZoomPunch {
                    strength: s * 0.4, duration: d * 0.5 });
                self.apply_trigger(TriggerSpec::Shake {
                    strength: (s * 0.8).clamp(0.0, 1.5), duration: d });
                self.apply_trigger(TriggerSpec::Shockwave {
                    strength: s, duration: d * 0.8 });
                self.pending_flash = s.clamp(0.0, 1.0);
                self.flash_timer = (d * 0.35).max(0.1);
            }
            TriggerSpec::PostShader { slot, p } => {
                self.current_post_shader = Some((slot, p));
            }

            TriggerSpec::PostShaderOff => {
                self.current_post_shader = None;
            }
            TriggerSpec::Morph { sides, duration } => {
                // Integer target, fractional duration. Both clamped
                // defensively even though the parser already did so;
                // keeps this function robust to trigger sources that
                // might bypass the parser in the future (debug tools,
                // tests).
                self.target_sides = sides.clamp(3, 12);
                self.morph_duration = duration.clamp(0.1, 10.0);
                self.morph_timer = self.morph_duration;
            }
        }
    }

    /// Drain every wall whose scheduled spawn time has arrived.
    /// The callback receives the slot index, the side count the
    /// wall was materialized against, the thickness multiplier
    /// and the radial length in seconds. The extra `slot_count`
    /// argument lets the caller convert the slot index into an
    /// angle using the geometry that was active when the wall
    /// was authored, not the geometry that is active now, so a
    /// runtime polygon morph never teleports in flight walls.
    pub fn drain_walls<F: FnMut(u32, u32, f32, f32)>(&mut self, mut spawn: F) {
        let t = self.clock;
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].spawn_time <= t {
                let w = self.queue.swap_remove(i);
                spawn(w.slot, w.slot_count, w.thickness_mult, w.length_seconds);
            } else { i += 1; }
        }
    }

    pub fn take_flip_request(&mut self) -> bool {
        let r = self.flip_request; self.flip_request = false; r
    }

    /// Current shake envelope as `(strength, life)`, where
    /// `life` is the fraction of the active trigger's duration
    /// still remaining. The consumer keeps trauma topped up
    /// against its own decay so the shake stays at the authored
    /// `strength` for the full `duration` and then fades.
    pub fn shake_envelope(&self) -> (f32, f32) {
        if self.shake_timer <= 0.0 || self.shake_duration <= 0.0 {
            return (0.0, 0.0);
        }
        let life = (self.shake_timer / self.shake_duration).clamp(0.0, 1.0);
        (self.shake_strength, life)
    }

    /// Currently active user post shader, if any. The
    /// returned tuple is `(slot, params)`. The host reads
    /// this each frame and is responsible for loading,
    /// caching, and swapping the actual Vulkan pipeline;
    /// the generator itself knows nothing about rendering.
    pub fn active_post_shader(&self) -> Option<(u8, [f32; 4])> {
        self.current_post_shader
    }

    pub fn camera_tilt(&self) -> f32 { self.tilt }

    /// Pitch component of current tilt in radians. Positive
    /// values tilt the top of the screen away from the viewer.
    pub fn camera_pitch(&self) -> f32 { self.tilt_pitch }
    /// Yaw component of current tilt in radians. Positive
    /// values tilt the right side of the screen away.
    pub fn camera_yaw(&self) -> f32 { self.tilt_yaw }

    /// Combined wall speed multiplier: legacy `speedMult`
    /// trigger stacked with the new `speedwarp.walls` axis.
    pub fn speed_mult(&self) -> f32 { self.speed_mult * self.warp_walls_value }

    /// Axis specific multipliers exposed separately so the game
    /// can scale rotation and cursor without affecting walls.
    pub fn rotation_mult(&self) -> f32 { self.warp_rotation_value }
    pub fn cursor_mult  (&self) -> f32 { self.warp_cursor_value }
    pub fn music_rate   (&self) -> f32 { self.warp_music_value }

    pub fn extra_hue_shift(&self) -> f32 { self.hue_extra }
    pub fn pulse_boost(&self) -> f32 {
        if self.pulse_timer <= 0.0 { 0.0 }
        else { (self.pulse_timer / PULSE_DURATION).clamp(0.0, 1.0) }
    }

    /// Glitch intensity in 0..1, fading linearly over the
    /// trigger's lifetime so the post-process shader eases out
    /// instead of cutting.
    pub fn glitch_amount(&self) -> f32 {
        if self.glitch_timer <= 0.0 || self.glitch_duration <= 0.0 { 0.0 }
        else {
            let t = (self.glitch_timer / self.glitch_duration).clamp(0.0, 1.0);
            self.glitch_strength * t
        }
    }

    /// Extra camera rotation contributed by the `:spin` trigger,
    /// in radians per second. Game scales by dt and adds to its
    /// own camera_rot.
    pub fn spin_rate(&self) -> f32 { self.spin_rate }

    /// Current `:bounce` amplitude. Game multiplies this by its
    /// own onset detection to decide how much trauma to inject
    /// on each detected audio beat.
    pub fn bounce_amplitude(&self) -> f32 {
        if self.bounce_timer > 0.0 { self.bounce_amplitude } else { 0.0 }
    }

    /// Wall motion scaling factor. 0.0 during freeze, 1.0
    /// otherwise. Game multiplies wall speed by this value.
    pub fn freeze_factor(&self) -> f32 {
        if self.freeze_timer > 0.0 { 0.0 } else { 1.0 }
    }

    /// Extra zoom offset from the `:zoom_punch` animation. Game
    /// adds this to the base zoom before passing to renderer.
    pub fn zoom_punch_offset(&self) -> f32 {
        if !self.punch_active || self.punch_duration <= 0.0 { return 0.0; }
        let t = (self.punch_elapsed / self.punch_duration).clamp(0.0, 1.0);
        let tri = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
        self.punch_strength * tri
    }

    /// Normalized 0..1 color invert amount for the post pass.
    pub fn invert_colors_amount(&self) -> f32 {
        if self.invert_colors_duration <= 0.0 || self.invert_colors_timer <= 0.0 {
            return 0.0;
        }
        (self.invert_colors_timer / self.invert_colors_duration).clamp(0.0, 1.0)
    }

    /// Grayscale mix factor 0..1 for the post pass.
    pub fn grayscale_amount(&self) -> f32 {
        if self.grayscale_duration <= 0.0 || self.grayscale_timer <= 0.0 {
            return 0.0;
        }
        let life = (self.grayscale_timer / self.grayscale_duration).clamp(0.0, 1.0);
        self.grayscale_strength * life
    }

    /// Shockwave progress 0..1 over its lifetime. 0 means no
    /// shockwave active (post pass skips the effect).
    pub fn shockwave_progress(&self) -> f32 {
        if !self.shockwave_active || self.shockwave_duration <= 0.0 {
            return 0.0;
        }
        (self.shockwave_elapsed / self.shockwave_duration).clamp(0.0, 1.0)
    }

    pub fn shockwave_strength(&self) -> f32 {
        if self.shockwave_active { self.shockwave_strength } else { 0.0 }
    }

    /// Fog near/far normalized screen radii, or (0, 0) when
    /// inactive. Game forwards to the post pass.
    pub fn fog_bounds(&self) -> (f32, f32) {
        if self.fog_timer > 0.0 {
            let life = (self.fog_timer / self.fog_duration).clamp(0.0, 1.0);
            (self.fog_near, self.fog_far * life + self.fog_near * (1.0 - life))
        } else {
            (0.0, 0.0)
        }
    }

    pub fn outline_amount(&self) -> f32 {
        if self.outline_duration <= 0.0 || self.outline_timer <= 0.0 {
            return 0.0;
        }
        let life = (self.outline_timer / self.outline_duration).clamp(0.0, 1.0);
        self.outline_thickness * life
    }

    /// Consume a pending centerburst request. Game reads this
    /// once per frame and emits particles when Some.
    pub fn take_centerburst(&mut self) -> Option<f32> {
        self.pending_centerburst.take()
    }

    /// Consume a pending ringburst. Returns (count, duration).
    pub fn take_ringburst(&mut self) -> Option<(u32, f32)> {
        self.pending_ringburst.take()
    }

    /// Active screen flash alpha contribution for bassdrop.
    /// Linearly decays over its own timer.
    pub fn flash_alpha(&self) -> f32 {
        if self.flash_timer <= 0.0 { return 0.0; }
        let t = (self.flash_timer / 0.35).clamp(0.0, 1.0);
        self.pending_flash * t
    }
    
    pub fn zoom(&self) -> f32 { self.zoom_current.clamp(0.25, 3.0) }

    pub fn is_inverted(&self) -> bool { self.invert_timer > 0.0 }

    /// 0..1 strobe intensity for the current frame. `rate 0`
    /// produces a single half-cosine decay instead of a
    /// repeating square wave so a lone strobe feels like a
    /// photo flash.
    pub fn strobe_alpha(&self, time: f32) -> f32 {
        if self.strobe_timer <= 0.0 { return 0.0; }
        let env = if self.strobe_duration > 0.0 {
            (self.strobe_timer / self.strobe_duration).clamp(0.0, 1.0)
        } else { 0.0 };
        if self.strobe_rate <= 0.001 {
            env
        } else {
            let wave = (time * self.strobe_rate * std::f32::consts::TAU).sin();
            env * (0.5 + 0.5 * wave)
        }
    }

    /// Integer side count used by newly materialized patterns
    /// and by wall spawn bookkeeping. This is the target of a
    /// morph, not the fractional render state.
    pub fn sides(&self) -> u32 { self.target_sides }

    /// Fractional side count for rendering. Equal to `sides()`
    /// when no morph is in progress, otherwise eased smoothly
    /// between the old and new integer values.
    pub fn sides_fract(&self) -> f32 { self.current_sides.max(3.0) }
}

fn flatten_stmts(stmts: &[Stmt], out: &mut Vec<FlatStmt>) {
    for s in stmts {
        match s {
            Stmt::Emit(spec)         => out.push(FlatStmt::Emit(spec.clone())),
            Stmt::Wait(n)            => out.push(FlatStmt::Wait(*n)),
            Stmt::Trigger(t)         => out.push(FlatStmt::Trigger(*t)),
            Stmt::Repeat { count, body } => {
                for _ in 0..*count { flatten_stmts(body, out); }
            }
            Stmt::LocalVars(_) => { /* already resolved at parse time */ }
            Stmt::Rule(_) | Stmt::Revert(_)
            | Stmt::Push(_) | Stmt::Pop(_) => {
                // Rule related statements pass through as is and
                // are scheduled onto the rule queue by the
                // enqueue loop so their effects land on the beat
                // the author placed them on, not earlier.
                out.push(FlatStmt::Rule(s.clone()));
            }
        }
    }
}