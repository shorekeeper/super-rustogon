//! Cheap CPU particle system.
//!
//! Particles are stored in a flat Vec. Each frame `update` ticks
//! them and removes the dead ones with `swap_remove`. `draw`
//! emits a small hex or square per particle into the shared
//! vertex buffer.
//!
//! The system is intentionally small. There is no texture, no
//! instancing, no depth sorting. For the effect budgets this
//! game needs (a few hundred particles max) that is plenty and
//! keeps the code dependency-free.

use crate::pipeline::Vertex;
use crate::ui::draw::{push_hex_alpha, push_quad_alpha};

#[derive(Clone, Copy)]
pub enum ParticleShape {
    Hex,
    Square,
}

#[derive(Clone, Copy)]
pub struct Particle {
    pub pos:       [f32; 2],
    pub vel:       [f32; 2],
    pub color:     [f32; 3],
    pub size:      f32,
    pub life:      f32,
    pub max_life:  f32,
    pub spin:      f32,
    pub angle:     f32,
    pub drag:      f32,
    pub shape:     ParticleShape,
}

pub struct ParticleSystem {
    particles: Vec<Particle>,
    density:   f32,
    budget:    usize,
}

impl ParticleSystem {
    pub fn new() -> Self {
        ParticleSystem {
            particles: Vec::with_capacity(512),
            density:   1.0,
            budget:    2048,
        }
    }

    pub fn set_density(&mut self, d: f32) { self.density = d.clamp(0.0, 2.0); }
    pub fn density(&self) -> f32 { self.density }

    pub fn clear(&mut self) { self.particles.clear(); }

    pub fn count(&self) -> usize { self.particles.len() }

    /// Push a single particle, respecting density and the
    /// per-system budget.
    pub fn emit(&mut self, p: Particle) {
        if self.density <= 0.0 { return; }
        if self.particles.len() >= self.budget { return; }
        self.particles.push(p);
    }

    /// Emit a radial burst of `count` particles from a point.
    pub fn emit_burst(
        &mut self, pos: [f32; 2], count: u32, base_speed: f32,
        color: [f32; 3], life: f32, size: f32,
        seed: &mut u32,
    ) {
        let n = ((count as f32) * self.density).round() as u32;
        let n = n.max(1);
        for i in 0..n {
            *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let r1 = ((*seed >> 8) as f32) / (1u32 << 24) as f32;
            *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let r2 = ((*seed >> 8) as f32) / (1u32 << 24) as f32;
            let angle = (i as f32 / n as f32) * std::f32::consts::TAU
                      + r1 * 0.4;
            let speed = base_speed * (0.6 + r2 * 0.8);
            self.emit(Particle {
                pos,
                vel:      [angle.cos() * speed, angle.sin() * speed],
                color,
                size:     size * (0.7 + r1 * 0.6),
                life,
                max_life: life,
                spin:     (r2 - 0.5) * 6.0,
                angle:    r1 * std::f32::consts::TAU,
                drag:     1.6,
                shape:    ParticleShape::Hex,
            });
        }
    }

    /// Emit shards for the player death shatter animation. Shards
    /// are square (so they read as broken fragments) and have a
    /// strong drag so they fall out of the play area naturally.
    pub fn emit_shatter(
        &mut self, pos: [f32; 2], count: u32,
        color: [f32; 3], seed: &mut u32,
    ) {
        for i in 0..count {
            *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let r1 = ((*seed >> 8) as f32) / (1u32 << 24) as f32;
            *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let r2 = ((*seed >> 8) as f32) / (1u32 << 24) as f32;
            let angle = (i as f32 / count as f32) * std::f32::consts::TAU;
            let speed = 1.4 + r1 * 1.8;
            self.emit(Particle {
                pos,
                vel: [angle.cos() * speed, angle.sin() * speed],
                color,
                size: 0.018 + r2 * 0.020,
                life: 1.2 + r1 * 0.6,
                max_life: 1.4,
                spin:  (r2 - 0.5) * 18.0,
                angle: r1 * std::f32::consts::TAU,
                drag:  2.2,
                shape: ParticleShape::Square,
            });
        }
    }

    pub fn update(&mut self, dt: f32) {
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            let decay = (-p.drag * dt).exp();
            p.vel[0] *= decay;
            p.vel[1] *= decay;
            p.pos[0] += p.vel[0] * dt;
            p.pos[1] += p.vel[1] * dt;
            p.angle  += p.spin * dt;
            p.life   -= dt;
            if p.life <= 0.0 {
                self.particles.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    pub fn draw(&self, out: &mut Vec<Vertex>) {
        for p in &self.particles {
            let t = (p.life / p.max_life).clamp(0.0, 1.0);
            let a = t;
            match p.shape {
                ParticleShape::Hex => {
                    push_hex_alpha(out, p.pos[0], p.pos[1],
                        p.size * (0.4 + 0.6 * t), p.color, a);
                }
                ParticleShape::Square => {
                    let s = p.size * (0.4 + 0.6 * t);
                    let c = p.angle.cos() * s;
                    let n = p.angle.sin() * s;
                    // Rotated square using two triangles.
                    let p0 = [p.pos[0] + c - n, p.pos[1] + n + c];
                    let p1 = [p.pos[0] - c - n, p.pos[1] - n + c];
                    let p2 = [p.pos[0] - c + n, p.pos[1] - n - c];
                    let p3 = [p.pos[0] + c + n, p.pos[1] + n - c];
                    out.push(crate::pipeline::Vertex::rgba(p0, p.color, a));
                    out.push(crate::pipeline::Vertex::rgba(p1, p.color, a));
                    out.push(crate::pipeline::Vertex::rgba(p2, p.color, a));
                    out.push(crate::pipeline::Vertex::rgba(p0, p.color, a));
                    out.push(crate::pipeline::Vertex::rgba(p2, p.color, a));
                    out.push(crate::pipeline::Vertex::rgba(p3, p.color, a));
                    let _ = push_quad_alpha; // satisfies unused import.
                }
            }
        }
    }
}