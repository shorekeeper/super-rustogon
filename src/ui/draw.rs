//! Primitive emitter functions.
//!
//! All of these are re-exports of the previous menu-local push_*
//! helpers, unified and documented in one place. Widgets reach for
//! these rather than duplicating geometry building code.

use crate::pipeline::Vertex;

const TAU: f32 = std::f32::consts::TAU;

pub fn push_quad(out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3]) {
    out.push(Vertex::opaque([x0, y0], c));
    out.push(Vertex::opaque([x1, y0], c));
    out.push(Vertex::opaque([x1, y1], c));
    out.push(Vertex::opaque([x0, y0], c));
    out.push(Vertex::opaque([x1, y1], c));
    out.push(Vertex::opaque([x0, y1], c));
}

pub fn push_quad_alpha(
    out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32,
    c: [f32; 3], a: f32,
) {
    out.push(Vertex::rgba([x0, y0], c, a));
    out.push(Vertex::rgba([x1, y0], c, a));
    out.push(Vertex::rgba([x1, y1], c, a));
    out.push(Vertex::rgba([x0, y0], c, a));
    out.push(Vertex::rgba([x1, y1], c, a));
    out.push(Vertex::rgba([x0, y1], c, a));
}

pub fn push_outline(
    out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, t: f32, c: [f32; 3],
) {
    push_quad(out, x0 - t, y0 - t, x1 + t, y0,     c);
    push_quad(out, x0 - t, y1,     x1 + t, y1 + t, c);
    push_quad(out, x0 - t, y0,     x0,     y1,     c);
    push_quad(out, x1,     y0,     x1 + t, y1,     c);
}

pub fn push_outline_alpha(
    out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32,
    t: f32, c: [f32; 3], a: f32,
) {
    push_quad_alpha(out, x0 - t, y0 - t, x1 + t, y0,     c, a);
    push_quad_alpha(out, x0 - t, y1,     x1 + t, y1 + t, c, a);
    push_quad_alpha(out, x0 - t, y0,     x0,     y1,     c, a);
    push_quad_alpha(out, x1,     y0,     x1 + t, y1,     c, a);
}

pub fn push_tri(out: &mut Vec<Vertex>, a: [f32; 2], b: [f32; 2], c: [f32; 2], col: [f32; 3]) {
    out.push(Vertex::opaque(a, col));
    out.push(Vertex::opaque(b, col));
    out.push(Vertex::opaque(c, col));
}

pub fn push_tri_alpha(
    out: &mut Vec<Vertex>, a: [f32; 2], b: [f32; 2], c: [f32; 2],
    col: [f32; 3], alpha: f32,
) {
    out.push(Vertex::rgba(a, col, alpha));
    out.push(Vertex::rgba(b, col, alpha));
    out.push(Vertex::rgba(c, col, alpha));
}

pub fn push_hex(out: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, c: [f32; 3]) {
    push_hex_rot(out, cx, cy, r, c, 0.0);
}

pub fn push_hex_rot(
    out: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, col: [f32; 3], ang: f32,
) {
    let mut v = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let a = ang + i as f32 * TAU / 6.0;
        v[i] = [cx + a.cos() * r, cy + a.sin() * r];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::opaque([cx, cy], col));
        out.push(Vertex::opaque(v[i],     col));
        out.push(Vertex::opaque(v[j],     col));
    }
}

pub fn push_hex_alpha(out: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, c: [f32; 3], a: f32) {
    let mut v = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let ang = i as f32 * TAU / 6.0;
        v[i] = [cx + ang.cos() * r, cy + ang.sin() * r];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::rgba([cx, cy], c, a));
        out.push(Vertex::rgba(v[i],     c, a));
        out.push(Vertex::rgba(v[j],     c, a));
    }
}

pub fn push_hex_ring(out: &mut Vec<Vertex>, cx: f32, cy: f32, ro: f32, ri: f32, c: [f32; 3]) {
    push_hex_ring_rot(out, cx, cy, ro, ri, c, 0.0);
}

pub fn push_hex_ring_rot(
    out: &mut Vec<Vertex>, cx: f32, cy: f32,
    ro: f32, ri: f32, col: [f32; 3], ang: f32,
) {
    let mut o = [[0.0f32; 2]; 6];
    let mut i2 = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let a = ang + i as f32 * TAU / 6.0;
        o [i] = [cx + a.cos() * ro, cy + a.sin() * ro];
        i2[i] = [cx + a.cos() * ri, cy + a.sin() * ri];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::opaque(o [i], col));
        out.push(Vertex::opaque(o [j], col));
        out.push(Vertex::opaque(i2[j], col));
        out.push(Vertex::opaque(o [i], col));
        out.push(Vertex::opaque(i2[j], col));
        out.push(Vertex::opaque(i2[i], col));
    }
}

pub fn push_hex_ring_alpha(
    out: &mut Vec<Vertex>, cx: f32, cy: f32, ro: f32, ri: f32, c: [f32; 3], a: f32,
) {
    let mut o = [[0.0f32; 2]; 6];
    let mut i2 = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let ang = i as f32 * TAU / 6.0;
        o [i] = [cx + ang.cos() * ro, cy + ang.sin() * ro];
        i2[i] = [cx + ang.cos() * ri, cy + ang.sin() * ri];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::rgba(o [i], c, a));
        out.push(Vertex::rgba(o [j], c, a));
        out.push(Vertex::rgba(i2[j], c, a));
        out.push(Vertex::rgba(o [i], c, a));
        out.push(Vertex::rgba(i2[j], c, a));
        out.push(Vertex::rgba(i2[i], c, a));
    }
}

/// Soft ring, used for glow and bloom-like effects. Emits several
/// concentric ring bands with fading alpha.
pub fn push_glow(
    out: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, thick: f32,
    c: [f32; 3], strength: f32,
) {
    let steps = 3;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let ro = r + thick * (1.0 + t * 3.0);
        let ri = r + thick * t * 3.0;
        let a = strength * (1.0 - t) * 0.35;
        push_hex_ring_alpha(out, cx, cy, ro, ri, c, a);
    }
}

/// Circle approximation with N segments, used for radial pulse
/// echoes that should not look hex-faceted.
pub fn push_ring_circle_alpha(
    out: &mut Vec<Vertex>, cx: f32, cy: f32, ro: f32, ri: f32,
    c: [f32; 3], a: f32, segments: u32,
) {
    let n = segments.max(6);
    for i in 0..n {
        let a0 = i as f32 / n as f32 * TAU;
        let a1 = (i + 1) as f32 / n as f32 * TAU;
        let p0o = [cx + a0.cos() * ro, cy + a0.sin() * ro];
        let p1o = [cx + a1.cos() * ro, cy + a1.sin() * ro];
        let p0i = [cx + a0.cos() * ri, cy + a0.sin() * ri];
        let p1i = [cx + a1.cos() * ri, cy + a1.sin() * ri];
        out.push(Vertex::rgba(p0o, c, a));
        out.push(Vertex::rgba(p1o, c, a));
        out.push(Vertex::rgba(p1i, c, a));
        out.push(Vertex::rgba(p0o, c, a));
        out.push(Vertex::rgba(p1i, c, a));
        out.push(Vertex::rgba(p0i, c, a));
    }
}