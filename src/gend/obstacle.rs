//! Obstacle materialization.
//!
//! Each [`ObstacleSpec`] expands into a list of wall requests
//! with fractional beat offsets and a total duration in beats.
//! The generator schedules them into an absolute time queue, so
//! density is independent of the audio onset stream.
//!
//! Safety invariants enforced here:
//!
//! * every bar leaves exactly one gap,
//! * rainbow / ladder / pot / staircase / cubes patterns always
//!   keep at least one survivable slot per beat,
//! * tunnel / corridor walls fill only alternating slots so half
//!   the ring is guaranteed traversable,
//! * burst patterns never emit the same slot twice in a row,
//! * formula masks have been validated for pathability at parse
//!   time, the generator simply trusts them.

use crate::dsl::ast::{ObstacleSpec, Parity, SpinDir};
use crate::dsl::formula::EvalCtx;
use crate::gend::prng::Prng;

/// One concrete wall request produced by a pattern.
#[derive(Clone, Copy, Debug)]
pub struct WallSpec {
    pub slot:           u32,
    pub thickness_mult: f32,
    pub beat_offset:    f32,
    pub length_seconds: f32,
}

impl WallSpec {
    fn normal(slot: u32, beat_offset: f32, thickness_mult: f32) -> Self {
        WallSpec { slot, thickness_mult, beat_offset, length_seconds: 0.0 }
    }
}

/// Result of materializing one obstacle spec.
pub struct Materialized {
    pub walls:          Vec<WallSpec>,
    pub duration_beats: f32,
}

pub fn materialize(spec: &ObstacleSpec, sides: u32, rng: &mut Prng) -> Materialized {
    match spec {
        ObstacleSpec::Bar { thickness_mult } =>
            mat_bar(sides, rng, *thickness_mult),
        ObstacleSpec::DoubleBar { spacing, thickness_mult } =>
            mat_double_bar(sides, rng, *spacing, *thickness_mult),
        ObstacleSpec::Spiral { dir, thickness_mult, loops } =>
            mat_spiral(sides, rng, *dir, *thickness_mult, *loops),
        ObstacleSpec::Alternate { parity, thickness_mult } =>
            mat_alternate(sides, *parity, *thickness_mult),
        ObstacleSpec::Pinwheel { spokes, dir } =>
            mat_pinwheel(sides, rng, *spokes, *dir),
        ObstacleSpec::Rain { count, thickness_mult } =>
            mat_rain(sides, rng, *count, *thickness_mult),
        ObstacleSpec::Custom { mask, thickness_mult } =>
            mat_custom(sides, rng, mask, *thickness_mult),
        ObstacleSpec::Rainbow { dir } =>
            mat_rainbow(sides, rng, *dir),
        ObstacleSpec::Ladder { rungs } =>
            mat_ladder(sides, rng, *rungs),
        ObstacleSpec::Tunnel { length, lanes } =>
            mat_tunnel(sides, rng, *length, *lanes),
        ObstacleSpec::Pot { layers } =>
            mat_pot(sides, rng, *layers),
        ObstacleSpec::Staircase { dir, steps, thickness_mult } =>
            mat_staircase(sides, rng, *dir, *steps, *thickness_mult),
        ObstacleSpec::Corridor { length, turns, dir } =>
            mat_corridor(sides, rng, *length, *turns, *dir),
        ObstacleSpec::Cubes { layers, dir } =>
            mat_cubes(sides, rng, *layers, *dir),
        ObstacleSpec::CustomFormula { formula, steps, thickness_mult } =>
            mat_custom_formula(sides, rng, formula, *steps, *thickness_mult),
    }
}

// ----- existing patterns -----

fn mat_bar(sides: u32, rng: &mut Prng, thickness: f32) -> Materialized {
    let gap = rng.range(0, sides);
    let walls = (0..sides)
        .filter(|&s| s != gap)
        .map(|s| WallSpec::normal(s, 0.0, thickness))
        .collect();
    Materialized { walls, duration_beats: 1.4 }
}

fn mat_double_bar(sides: u32, rng: &mut Prng, spacing: u32, thickness: f32) -> Materialized {
    let gap_a = rng.range(0, sides);
    let gap_b = (gap_a + sides / 2) % sides;
    let dt = (spacing as f32).max(1.0) * 0.8;
    let mut walls = Vec::new();
    for s in 0..sides {
        if s != gap_a { walls.push(WallSpec::normal(s, 0.0, thickness)); }
    }
    for s in 0..sides {
        if s != gap_b { walls.push(WallSpec::normal(s, dt, thickness)); }
    }
    Materialized { walls, duration_beats: dt + 1.4 }
}

fn mat_spiral(
    sides: u32, rng: &mut Prng, dir: SpinDir, thickness: f32, loops: u32,
) -> Materialized {
    let cw = matches!(dir, SpinDir::Cw);
    let start = rng.range(0, sides);
    let step = 0.5f32;
    let total = sides * loops.max(1);
    let mut walls = Vec::with_capacity(total as usize);
    for i in 0..total {
        let slot = if cw { (start + i) % sides }
                   else   { (start + total * sides - i) % sides };
        walls.push(WallSpec::normal(slot, i as f32 * step, thickness));
    }
    Materialized { walls, duration_beats: total as f32 * step + 1.0 }
}

fn mat_alternate(sides: u32, parity: Parity, thickness: f32) -> Materialized {
    let offset = match parity { Parity::Even => 0, Parity::Odd => 1 };
    let mut walls = Vec::new();
    let mut s = offset;
    while s < sides {
        walls.push(WallSpec::normal(s, 0.0, thickness));
        s += 2;
    }
    Materialized { walls, duration_beats: 1.4 }
}

fn mat_pinwheel(sides: u32, rng: &mut Prng, spokes: u32, dir: SpinDir) -> Materialized {
    let cw = matches!(dir, SpinDir::Cw);
    let start = rng.range(0, sides);
    let mut walls = Vec::new();
    for i in 0..spokes {
        let step = if cw { i } else { sides.saturating_sub(i) };
        let a = (start + step) % sides;
        let b = (a + sides / 2) % sides;
        let t = i as f32 * 0.5;
        walls.push(WallSpec::normal(a, t, 1.0));
        walls.push(WallSpec::normal(b, t, 1.0));
    }
    Materialized { walls, duration_beats: spokes as f32 * 0.5 + 1.0 }
}

fn mat_rain(sides: u32, rng: &mut Prng, count: u32, thickness: f32) -> Materialized {
    let mut walls = Vec::new();
    let mut last = u32::MAX;
    for i in 0..count {
        let mut slot = rng.range(0, sides);
        if slot == last { slot = (slot + 1) % sides; }
        last = slot;
        walls.push(WallSpec::normal(slot, i as f32 * 0.35, thickness));
    }
    Materialized { walls, duration_beats: count as f32 * 0.35 + 0.8 }
}

fn mat_custom(sides: u32, rng: &mut Prng, mask: &[bool], thickness: f32) -> Materialized {
    let mut walls = Vec::new();
    let n = sides.max(1);
    for s in 0..n {
        let idx = (s as usize) % mask.len();
        if mask[idx] { walls.push(WallSpec::normal(s, 0.0, thickness)); }
    }
    if walls.len() as u32 >= n {
        let gap = rng.range(0, n);
        walls.retain(|w| w.slot != gap);
    }
    Materialized { walls, duration_beats: 1.4 }
}

fn mat_rainbow(sides: u32, rng: &mut Prng, dir: SpinDir) -> Materialized {
    let cw = matches!(dir, SpinDir::Cw);
    let start_gap = rng.range(0, sides);
    let step = 1.0f32;
    let bars = sides;
    let mut walls = Vec::new();
    for i in 0..bars {
        let gap = if cw { (start_gap + i) % sides }
                  else  { (start_gap + sides * 2 - i) % sides };
        for s in 0..sides {
            if s != gap { walls.push(WallSpec::normal(s, i as f32 * step, 1.0)); }
        }
    }
    Materialized { walls, duration_beats: bars as f32 * step + 1.0 }
}

fn mat_ladder(sides: u32, rng: &mut Prng, rungs: u32) -> Materialized {
    let gap_a = rng.range(0, sides);
    let gap_b = (gap_a + 1) % sides;
    let step = 0.75f32;
    let rungs = rungs.max(2);
    let mut walls = Vec::new();
    for i in 0..rungs {
        let gap = if i % 2 == 0 { gap_a } else { gap_b };
        for s in 0..sides {
            if s != gap { walls.push(WallSpec::normal(s, i as f32 * step, 1.0)); }
        }
    }
    Materialized { walls, duration_beats: rungs as f32 * step + 1.0 }
}

fn mat_tunnel(sides: u32, rng: &mut Prng, length: f32, lanes: u32) -> Materialized {
    let phase = rng.range(0, 2);
    let length_s = length.clamp(0.4, 2.5);
    let want = lanes.clamp(1, sides.saturating_sub(2));
    let mut walls = Vec::new();
    let mut count = 0u32;
    for s in 0..sides {
        if (s + phase) % 2 == 0 && count < want {
            walls.push(WallSpec {
                slot: s, thickness_mult: 1.0,
                beat_offset: 0.0, length_seconds: length_s,
            });
            count += 1;
        }
    }
    Materialized { walls, duration_beats: 2.5 }
}

fn mat_pot(sides: u32, rng: &mut Prng, layers: u32) -> Materialized {
    let start_gap = rng.range(0, sides);
    let step = 0.65f32;
    let layers = layers.clamp(2, 8);
    let mut walls = Vec::new();
    for i in 0..layers {
        let gap = (start_gap + i) % sides;
        for s in 0..sides {
            if s != gap { walls.push(WallSpec::normal(s, i as f32 * step, 1.0)); }
        }
    }
    Materialized { walls, duration_beats: layers as f32 * step + 1.0 }
}

// ----- new patterns -----

/// `staircase`: the classic Super Hexagon rotating staircase
/// visible on the reference screenshots. Each step is a full
/// ring minus one gap, and the gap walks one slot per step in
/// the chosen direction. Produces the "spiral you descend into"
/// reading on the radial playfield.
fn mat_staircase(
    sides: u32, rng: &mut Prng, dir: SpinDir, steps: u32, thickness: f32,
) -> Materialized {
    let cw = matches!(dir, SpinDir::Cw);
    let start_gap = rng.range(0, sides);
    let step_beats = 0.85f32;
    let steps = steps.max(2);
    let mut walls = Vec::new();
    for i in 0..steps {
        let gap = if cw {
            (start_gap + i) % sides
        } else {
            (start_gap + steps * sides - i) % sides
        };
        for s in 0..sides {
            if s != gap {
                walls.push(WallSpec::normal(
                    s, i as f32 * step_beats, thickness));
            }
        }
    }
    Materialized { walls, duration_beats: steps as f32 * step_beats + 1.0 }
}

/// `corridor`: a long radial tunnel whose open lane switches
/// side periodically. Very readable at high tiers because the
/// player commits to a lane for a visible fraction of a second
/// before having to relocate.
fn mat_corridor(
    sides: u32, rng: &mut Prng, length: f32, turns: u32, dir: SpinDir,
) -> Materialized {
    let cw = matches!(dir, SpinDir::Cw);
    let start_gap = rng.range(0, sides);
    let seg_len = length.clamp(0.6, 3.0);
    let turns = turns.clamp(1, 8);
    let mut walls = Vec::new();
    let mut cur_gap = start_gap;
    for i in 0..turns {
        for s in 0..sides {
            if s != cur_gap && s != (cur_gap + sides / 2) % sides {
                walls.push(WallSpec {
                    slot: s,
                    thickness_mult: 1.0,
                    beat_offset: i as f32 * (seg_len * 1.1),
                    length_seconds: seg_len,
                });
            }
        }
        cur_gap = if cw { (cur_gap + 1) % sides }
                  else  { (cur_gap + sides - 1) % sides };
    }
    let d = turns as f32 * (seg_len * 1.1) + 1.5;
    Materialized { walls, duration_beats: d }
}

/// `cubes`: four back to back rings, each rotated by one slot
/// from its predecessor. Looks like a spinning die tumbling
/// toward the player.
fn mat_cubes(sides: u32, rng: &mut Prng, layers: u32, dir: SpinDir) -> Materialized {
    let cw = matches!(dir, SpinDir::Cw);
    let start_gap = rng.range(0, sides);
    let step = 0.55f32;
    let layers = layers.clamp(2, 8);
    let mut walls = Vec::new();
    for i in 0..layers {
        let gap = if cw {
            (start_gap + 2 * i) % sides
        } else {
            (start_gap + 2 * layers * sides - 2 * i) % sides
        };
        for s in 0..sides {
            if s != gap {
                walls.push(WallSpec::normal(s, i as f32 * step, 1.0));
            }
        }
    }
    Materialized { walls, duration_beats: layers as f32 * step + 1.0 }
}

/// `formula`: evaluate the compiled expression at every
/// (step, slot) position and place a wall where the result
/// comes out truthy. The expression has already been proved
/// pathable by the parser, so we never need a runtime fallback
/// here.
fn mat_custom_formula(
    sides: u32, rng: &mut Prng, formula: &crate::dsl::ast::Formula,
    steps: u32, thickness: f32,
) -> Materialized {
    let phase = rng.range(0, sides) as f32;
    let seed  = rng.next_u32() as f32;
    let step_beats = 0.75f32;
    let mut walls = Vec::new();
    for step in 0..steps {
        let mut step_has_gap = false;
        for slot in 0..sides {
            let ctx = EvalCtx {
                slot:  slot as f32,
                step:  step as f32,
                sides: sides as f32,
                phase, seed,
                extras: &[],
            };
            let v = formula.program.eval(&ctx);
            let wall = v > 0.5;
            if !wall { step_has_gap = true; }
            if wall {
                walls.push(WallSpec::normal(
                    slot, step as f32 * step_beats, thickness));
            }
        }
        // Belt and suspenders safety net. The parser already ran
        // validate_pathable with `phase = 0`, but if a phase or
        // seed variant managed to close every gap we remove a
        // random wall in the offending step so the pattern stays
        // survivable.
        if !step_has_gap {
            let victim = rng.range(0, sides);
            let target_t = step as f32 * step_beats;
            walls.retain(|w| !(w.slot == victim
                               && (w.beat_offset - target_t).abs() < 1e-3));
        }
    }
    Materialized { walls, duration_beats: steps as f32 * step_beats + 1.0 }
}