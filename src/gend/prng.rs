//! Deterministic linear congruential PRNG.
//!
//! Intentionally identical in structure to the historical random
//! used by `game.rs`, so a given seed plays back the same level
//! between runs. Thread safe by construction: the generator owns
//! one and never shares it.

#[derive(Clone, Copy)]
pub struct Prng { state: u32 }

impl Prng {
    pub fn new(seed: u32) -> Self {
        // Avoid the degenerate state == 0 which sticks at 0 through
        // multiplication on some LCG variants; we add a fixed
        // offset to turn 0 into a normal seed.
        let s = if seed == 0 { 0xA5A5_A5A5 } else { seed };
        Prng { state: s }
    }

    pub fn next_u32(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(1664525).wrapping_add(1013904223);
        self.state
    }

    /// Uniform float in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Integer in `[lo, hi)`. `hi` must be greater than `lo`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.next_u32() % (hi - lo)
    }

    pub fn bool(&mut self) -> bool { (self.next_u32() & 1) == 1 }
}