//! Trauma-based screen shake.
//!
//! A single scalar `trauma` in 0..=1 accumulates on events and
//! decays exponentially. The actual per-frame offset is computed
//! as `amplitude * trauma^2 * noise(t)` so big shakes feel meaty
//! and small ones stay subtle.
//!
//! The output is a `(dx, dy)` offset in game-space units which
//! the renderer adds to its aspect-scale push-constant before
//! uploading. That means shake composes with aspect correction
//! for free and never distorts the playfield, only translates it.

pub struct ScreenShake {
    trauma:    f32,
    amplitude: f32,
    time:      f32,
    /// User scale (from settings). 0 disables shake entirely.
    user_scale: f32,
}

impl ScreenShake {
    pub fn new() -> Self {
        ScreenShake { trauma: 0.0, amplitude: 0.05, time: 0.0, user_scale: 1.0 }
    }

    pub fn set_user_scale(&mut self, s: f32) { self.user_scale = s.clamp(0.0, 1.5); }

    pub fn add(&mut self, delta: f32) {
        self.trauma = (self.trauma + delta).clamp(0.0, 1.0);
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        let decay = (-1.6 * dt).exp();
        self.trauma *= decay;
        if self.trauma < 0.001 { self.trauma = 0.0; }
    }

    pub fn offset(&self) -> (f32, f32) {
        if self.trauma <= 0.0 || self.user_scale <= 0.0 { return (0.0, 0.0); }
        let magnitude = self.amplitude * self.trauma * self.trauma * self.user_scale;
        let t = self.time;
        let nx = noise(t * 19.13).sin();
        let ny = noise(t * 23.77).cos();
        (nx * magnitude, ny * magnitude)
    }
}

#[inline]
fn noise(x: f32) -> f32 {
    // Cheap hashed sine; bounded, not periodic enough to read as
    // obvious sinusoidal wobble.
    (x * 127.1 + (x * 311.7).sin() * 13.0).sin()
}