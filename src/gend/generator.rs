//! Continuous, time driven obstacle generator.
//!
//! This revision of the generator supports the entire v2 DSL
//! trigger set (`speedwarp`, `glitch`, `shake`, `zoom`, `invert`,
//! `strobe`) alongside the historical `flip` / `tilt` / `pulse`
//! / `speedMult` / `hueShift` triggers, plus `repeat N { ... }`
//! blocks and `local` variable scopes. Sections whose `at`
//! values use an exotic timestamp format are still resolved
//! through [`Section::progress_threshold`].

use crate::dsl::ast::{
    Anim, LevelAst, Stmt, TimestampFormat, TriggerSpec,
};
use crate::gend::obstacle::{materialize, Materialized};
use crate::gend::prng::Prng;

const MAX_TILT_DEG: f32 = 2.5;
const TILT_EASE_RATE: f32 = 6.0;

const SPEED_MULT_DURATION: f32 = 4.0;
const HUE_SHIFT_DURATION:  f32 = 6.0;
const PULSE_DURATION:      f32 = 0.6;

const LOOKAHEAD_SECONDS: f32 = 2.5;
const ENQUEUE_BUDGET:    u32 = 64;
const BASE_GAP_BEATS:    f32 = 0.4;

#[derive(Clone, Copy)]
struct QueuedWall {
    spawn_time:     f32,
    slot:           u32,
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

/// Flattened statement consumed by the enqueue loop. Repeats
/// and local blocks are expanded into a single stream so the
/// core loop stays simple.
#[derive(Clone, Debug)]
enum FlatStmt {
    Emit(crate::dsl::ast::ObstacleSpec),
    Wait(u32),
    Trigger(TriggerSpec),
}

pub struct Generator {
    ast: LevelAst,
    rng: Prng,

    clock:        f32,
    beat_seconds: f32,

    active:            usize,
    /// Flattened statement list for the active section, rebuilt
    /// on section change. `repeat` blocks have already been
    /// unrolled and `local` variable scopes are gone.
    flat:              Vec<FlatStmt>,
    cursor:            usize,

    queue:             Vec<QueuedWall>,
    trigger_queue:     Vec<QueuedTrigger>,
    last_enqueue_time: f32,

    density_mult: f32,

    // Legacy triggers.
    flip_request: bool,
    tilt:         f32,
    tilt_target:  f32,
    speed_mult:   f32,
    speed_timer:  f32,
    hue_extra:    f32,
    hue_timer:    f32,
    pulse_timer:  f32,

    // v2 triggers.
    speedwarp_walls:    f32,
    speedwarp_rotation: f32,
    speedwarp_cursor:   f32,
    speedwarp_music:    f32,
    speedwarp_timer:    f32,
    speedwarp_duration: f32,

    glitch_strength: f32,
    glitch_timer:    f32,
    glitch_duration: f32,

    shake_strength: f32,
    shake_timer:    f32,
    shake_duration: f32,

    zoom_current:  f32,
    zoom_from:     f32,
    zoom_target:   f32,
    zoom_elapsed:  f32,
    zoom_duration: f32,
    zoom_anim:     Anim,

    invert_timer: f32,
    invert_duration: f32,

    strobe_rate:  f32,
    strobe_timer: f32,
    strobe_duration: f32,
}

impl Generator {
    pub fn new(ast: LevelAst, density_mult: f32, beat_seconds: f32) -> Self {
        let seed = ast.generation.seed;
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
            tilt: 0.0, tilt_target: 0.0,
            speed_mult: 1.0, speed_timer: 0.0,
            hue_extra: 0.0, hue_timer: 0.0,
            pulse_timer: 0.0,

            speedwarp_walls: 1.0,
            speedwarp_rotation: 1.0,
            speedwarp_cursor: 1.0,
            speedwarp_music: 1.0,
            speedwarp_timer: 0.0,
            speedwarp_duration: 1.0,

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
        };
        g.rebuild_flat();
        g
    }

pub fn update(&mut self, dt: f32, progress_seconds: f32, duration_seconds: f32) {
        self.clock += dt;

        // Legacy timers.
        if self.speed_timer > 0.0 {
            self.speed_timer -= dt;
            if self.speed_timer <= 0.0 { self.speed_timer = 0.0; self.speed_mult = 1.0; }
        }
        if self.hue_timer > 0.0 {
            self.hue_timer -= dt;
            if self.hue_timer <= 0.0 { self.hue_timer = 0.0; self.hue_extra = 0.0; }
        }
        if self.pulse_timer > 0.0 {
            self.pulse_timer -= dt;
            if self.pulse_timer < 0.0 { self.pulse_timer = 0.0; }
        }
        let blend = 1.0 - (-TILT_EASE_RATE * dt).exp();
        self.tilt += (self.tilt_target - self.tilt) * blend;

        // v2 timers.
        if self.speedwarp_timer > 0.0 {
            self.speedwarp_timer -= dt;
            if self.speedwarp_timer <= 0.0 {
                self.speedwarp_timer = 0.0;
                self.speedwarp_walls    = 1.0;
                self.speedwarp_rotation = 1.0;
                self.speedwarp_cursor   = 1.0;
                self.speedwarp_music    = 1.0;
            }
        }
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
        if self.shake_timer > 0.0 {
            self.shake_timer -= dt;
            if self.shake_timer <= 0.0 {
                self.shake_timer = 0.0;
                self.shake_strength = 0.0;
                self.shake_duration = 1.0;
            }
        }
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

    fn enqueue_next(&mut self) {
        if self.flat.is_empty() {
            self.last_enqueue_time = self.last_enqueue_time.max(self.clock)
                + self.beat_seconds;
            return;
        }
        if self.cursor >= self.flat.len() { self.cursor = 0; }
        let stmt = self.flat[self.cursor].clone();
        self.cursor += 1;

        match stmt {
            FlatStmt::Emit(spec) => {
                let Materialized { walls, duration_beats } =
                    materialize(&spec, self.ast.generation.sides, &mut self.rng);
                let anchor = self.last_enqueue_time.max(self.clock);
                for w in walls {
                    self.queue.push(QueuedWall {
                        spawn_time:     anchor + w.beat_offset * self.beat_seconds,
                        slot:           w.slot,
                        thickness_mult: w.thickness_mult,
                        length_seconds: w.length_seconds,
                    });
                }
                let gap = (BASE_GAP_BEATS / self.density_mult).max(0.0);
                self.last_enqueue_time = anchor
                    + (duration_beats + gap) * self.beat_seconds;
            }
            FlatStmt::Wait(n) => {
                let scaled = (n as f32 / self.density_mult).max(0.0);
                let anchor = self.last_enqueue_time.max(self.clock);
                self.last_enqueue_time = anchor + scaled * self.beat_seconds;
            }
            FlatStmt::Trigger(t) => {
                // Schedule, do not apply. The trigger fires when
                // the clock reaches its anchor, the same way
                // walls are scheduled. This puts a trigger
                // placed right after an emit at the visual
                // moment that emit's body finishes, instead of
                // up to `LOOKAHEAD_SECONDS` ahead of it.
                let anchor = self.last_enqueue_time.max(self.clock);
                self.trigger_queue.push(QueuedTrigger {
                    spawn_time: anchor,
                    spec:       t,
                });
            }
        }
    }

    fn apply_trigger(&mut self, t: TriggerSpec) {
        match t {
            TriggerSpec::Flip => { self.flip_request = true; }
            TriggerSpec::Tilt(deg) => {
                let d = deg.clamp(-MAX_TILT_DEG, MAX_TILT_DEG);
                self.tilt_target = d.to_radians();
            }
            TriggerSpec::Pulse => { self.pulse_timer = PULSE_DURATION; }
            TriggerSpec::SpeedMult(m) => {
                self.speed_mult  = m.clamp(0.5, 2.0);
                self.speed_timer = SPEED_MULT_DURATION;
            }
            TriggerSpec::HueShift(r) => {
                self.hue_extra = r.clamp(-2.0, 2.0);
                self.hue_timer = HUE_SHIFT_DURATION;
            }
            TriggerSpec::SpeedWarp { walls, rotation, cursor, music_scale, duration } => {
                // Zero or empty entries mean "do not touch this axis".
                self.speedwarp_walls    = if walls    == 0.0 { 1.0 } else { walls.clamp(0.1, 4.0) };
                self.speedwarp_rotation = if rotation == 0.0 { 1.0 } else { rotation.clamp(0.1, 4.0) };
                self.speedwarp_cursor   = if cursor   == 0.0 { 1.0 } else { cursor.clamp(0.1, 4.0) };
                self.speedwarp_music    = if music_scale == 0.0 { 1.0 } else { music_scale.clamp(0.25, 4.0) };
                self.speedwarp_duration = duration;
                self.speedwarp_timer    = duration;
            }
            TriggerSpec::Glitch { strength, duration } => {
                self.glitch_strength = strength;
                self.glitch_duration = duration;
                self.glitch_timer    = duration;
            }
            TriggerSpec::Shake { strength, duration } => {
                // Stacking rule: whichever hit is stronger or
                // longer wins. `|>` pipes produce multiple
                // applications at the same anchor; this keeps
                // them from cancelling each other. `duration`
                // is finally honored via a dedicated timer in
                // `update()`.
                self.shake_strength = strength.max(self.shake_strength);
                let d = duration.max(0.05);
                if d > self.shake_timer    { self.shake_timer    = d; }
                if d > self.shake_duration { self.shake_duration = d; }
            }
            TriggerSpec::Zoom { target, anim, duration } => {
                self.zoom_from     = self.zoom_current;
                self.zoom_target   = target;
                self.zoom_anim     = anim;
                self.zoom_elapsed  = 0.0;
                self.zoom_duration = duration;
            }
            TriggerSpec::Invert { duration } => {
                self.invert_timer    = duration;
                self.invert_duration = duration;
            }
            TriggerSpec::Strobe { rate, duration } => {
                self.strobe_rate     = rate;
                self.strobe_timer    = duration;
                self.strobe_duration = duration;
            }
        }
    }

    pub fn drain_walls<F: FnMut(u32, f32, f32)>(&mut self, mut spawn: F) {
        let t = self.clock;
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].spawn_time <= t {
                let w = self.queue.swap_remove(i);
                spawn(w.slot, w.thickness_mult, w.length_seconds);
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

    pub fn camera_tilt(&self) -> f32 { self.tilt }

    /// Combined wall speed multiplier: legacy `speedMult`
    /// trigger stacked with the new `speedwarp.walls` axis.
    pub fn speed_mult(&self) -> f32 { self.speed_mult * self.speedwarp_walls }

    /// Axis specific multipliers exposed separately so the game
    /// can scale rotation and cursor without affecting walls.
    pub fn rotation_mult(&self) -> f32 { self.speedwarp_rotation }
    pub fn cursor_mult  (&self) -> f32 { self.speedwarp_cursor }
    pub fn music_rate   (&self) -> f32 { self.speedwarp_music }

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

    pub fn sides(&self) -> u32 { self.ast.generation.sides }
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
        }
    }
}