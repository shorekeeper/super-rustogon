//! Section node graph rendered on the canvas.
//!
//! Each section in the document becomes one graph node.
//! Connections between consecutive sections (sorted by
//! timestamp) are drawn as bezier arrows, giving the author a
//! visual "flow" of the level even when the nodes are
//! scattered arbitrarily across the canvas.
//!
//! # Interaction
//!
//! * Click on a node: selects it. The selection propagates
//!   back to the editor shell which updates the sidebar and
//!   inspector views accordingly.
//! * Click and drag on a node: repositions it on the canvas.
//!   Node position is purely visual; the section's `at`
//!   timestamp is unaffected. Authors change `at` through the
//!   timeline markers.
//!
//! # Rendering
//!
//! Node content scales with canvas zoom. At very low zoom the
//! nodes still draw a colored rectangle so the layout remains
//! legible; at higher zoom levels the title, the time stamp,
//! and the statement counts appear. Everything clamps to the
//! canvas rect to keep graph content from spilling onto the
//! surrounding panels. Per-widget scissoring is not available
//! in the current renderer, so we emit geometry strictly
//! bounded to `rect`.

use crate::dsl::ast::{Section, Stmt};
use crate::editor::canvas::{Canvas, Rect};
use crate::editor::document::Document;
use crate::pipeline::Vertex;
use crate::text_small::{
    push_small, push_small_right,
    text_height as sh, text_width as sw,
};
use crate::ui::draw::{push_outline, push_quad, push_tri};
use crate::win32::Mouse;

/// World-space half width of a node.
pub const NODE_HALF_W: f32 = 0.36;
/// World-space half height of a node.
pub const NODE_HALF_H: f32 = 0.18;

const ACCENT:       [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:    [f32; 3] = [1.00, 0.80, 0.92];
const DIM:          [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:        [f32; 3] = [1.00, 1.00, 1.00];
const NODE_BG:      [f32; 3] = [0.12, 0.06, 0.18];
const NODE_BG_HOV:  [f32; 3] = [0.18, 0.10, 0.26];
const NODE_BG_SEL:  [f32; 3] = [0.25, 0.14, 0.35];
const CONN:         [f32; 3] = [0.30, 0.22, 0.40];
const CONN_HI:      [f32; 3] = [0.70, 0.55, 0.85];
const STRIPE_EMIT:  [f32; 3] = [0.40, 0.85, 1.00];
const STRIPE_TRIG:  [f32; 3] = [1.00, 0.65, 0.20];
const STRIPE_RULE:  [f32; 3] = [0.70, 0.70, 0.95];

pub struct Graph {
    hovered: Option<String>,
    /// Currently dragged node, with the grab offset in world
    /// space so the node tracks the pointer from wherever it
    /// was initially grabbed.
    dragging: Option<(String, [f32; 2])>,
    prev_left_down: bool,
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            hovered: None,
            dragging: None,
            prev_left_down: false,
        }
    }

    /// Returns `true` if the graph consumed this frame's input,
    /// meaning the canvas should not start a pan drag. Updates
    /// `selected` in place when the author clicks a node.
    pub fn update(
        &mut self,
        doc: &mut Document,
        canvas: &Canvas,
        pointer: (f32, f32),
        mouse: Mouse,
        rect: Rect,
        selected: &mut Option<String>,
    ) -> bool {
        let clicked = mouse.left_down && !self.prev_left_down;
        let released = !mouse.left_down && self.prev_left_down;
        self.prev_left_down = mouse.left_down;

        let in_rect = rect_contains_pt(rect, pointer);
        let world = canvas.screen_to_world([pointer.0, pointer.1], rect);

        // Hover test. Iterate in reverse over sections so nodes
        // drawn later (and thus visually on top) win over those
        // drawn earlier.
        self.hovered = None;
        if in_rect {
            for sec in doc.ast.sections.iter().rev() {
                if let Some(pos) = doc.node_positions.get(&sec.name) {
                    if node_contains([pos.x, pos.y], world) {
                        self.hovered = Some(sec.name.clone());
                        break;
                    }
                }
            }
        }

        let mut consumed = false;

        if released && self.dragging.is_some() {
            self.dragging = None;
            consumed = true;
        }

        if clicked && in_rect {
            if let Some(name) = self.hovered.clone() {
                *selected = Some(name.clone());
                if let Some(pos) = doc.node_positions.get(&name) {
                    let offset = [world[0] - pos.x, world[1] - pos.y];
                    self.dragging = Some((name, offset));
                }
                consumed = true;
            }
        }

        if let Some((name, offset)) = self.dragging.clone() {
            if mouse.left_down {
                let new_x = world[0] - offset[0];
                let new_y = world[1] - offset[1];
                doc.move_node(&name, new_x, new_y);
                consumed = true;
            }
        }

        consumed
    }

    /// Emit geometry for every visible node and every
    /// connection between consecutive sections. `selected` is
    /// the section whose name matches the shell's canonical
    /// selection; nodes and connections adjacent to it receive
    /// stronger highlight colors.
    pub fn draw(
        &self,
        doc: &Document,
        canvas: &Canvas,
        rect: Rect,
        selected: Option<&str>,
        out: &mut Vec<Vertex>,
    ) {
        // Connections under nodes. Sort sections chronologically
        // so arrows always point from earlier to later, which
        // makes the visual flow easy to follow regardless of
        // how the author arranged the nodes on the canvas.
        let sorted: Vec<&Section> = {
            let mut v: Vec<&Section> = doc.ast.sections.iter().collect();
            v.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap());
            v
        };

        for win in sorted.windows(2) {
            let a = win[0];
            let b = win[1];
            let pa = match doc.node_positions.get(&a.name) {
                Some(p) => *p,
                None => continue,
            };
            let pb = match doc.node_positions.get(&b.name) {
                Some(p) => *p,
                None => continue,
            };
            let screen_a = canvas.world_to_screen(
                [pa.x + NODE_HALF_W, pa.y], rect);
            let screen_b = canvas.world_to_screen(
                [pb.x - NODE_HALF_W, pb.y], rect);
            let highlighted = selected == Some(a.name.as_str())
                           || selected == Some(b.name.as_str());
            let color = if highlighted { CONN_HI } else { CONN };
            draw_bezier_arrow(out, screen_a, screen_b, color, rect);
        }

        for sec in &doc.ast.sections {
            if let Some(pos) = doc.node_positions.get(&sec.name) {
                let is_selected = selected == Some(sec.name.as_str());
                let is_hovered = self.hovered.as_deref()
                    == Some(sec.name.as_str());
                draw_node(out, sec, [pos.x, pos.y], canvas, rect,
                          is_selected, is_hovered);
            }
        }
    }

    pub fn hovered_name(&self) -> Option<&str> {
        self.hovered.as_deref()
    }
}

fn node_contains(pos: [f32; 2], world: [f32; 2]) -> bool {
    (world[0] - pos[0]).abs() <= NODE_HALF_W
        && (world[1] - pos[1]).abs() <= NODE_HALF_H
}

fn draw_node(
    out: &mut Vec<Vertex>,
    sec: &Section,
    world_pos: [f32; 2],
    canvas: &Canvas,
    rect: Rect,
    selected: bool,
    hovered: bool,
) {
    let tl_world = [world_pos[0] - NODE_HALF_W, world_pos[1] - NODE_HALF_H];
    let br_world = [world_pos[0] + NODE_HALF_W, world_pos[1] + NODE_HALF_H];
    let tl = canvas.world_to_screen(tl_world, rect);
    let br = canvas.world_to_screen(br_world, rect);

    // Cheap viewport cull. A node fully outside the rect does
    // not contribute any vertices.
    if br[0] < rect.0 || tl[0] > rect.2
        || br[1] < rect.1 || tl[1] > rect.3 { return; }

    // Clamp to rect so nodes near the edge do not spill over
    // the surrounding panels. The panels draw on top of the
    // graph so visual artifacts would be limited to frames
    // where the graph renders after them, but belt-and-braces.
    let node_rect = (
        tl[0].max(rect.0),
        tl[1].max(rect.1),
        br[0].min(rect.2),
        br[1].min(rect.3),
    );
    if node_rect.2 <= node_rect.0 + 0.001
        || node_rect.3 <= node_rect.1 + 0.001 { return; }

    let bg = if selected { NODE_BG_SEL }
             else if hovered { NODE_BG_HOV }
             else { NODE_BG };
    let ring = if selected { ACCENT_HI }
               else if hovered { ACCENT }
               else { DIM };
    let outline_thick = if selected { 0.004 } else { 0.002 };

    push_quad(out, node_rect.0, node_rect.1, node_rect.2, node_rect.3, bg);
    push_outline(out, node_rect.0, node_rect.1, node_rect.2, node_rect.3,
                 outline_thick, ring);

    // Left stripe colored by what the section mostly contains.
    // Gives an at-a-glance reading of "this section is mostly
    // emits" versus "this section is mostly rule overrides".
    let (emits, triggers, waits, rules) = count_stmts(&sec.body);
    let stripe_color = if rules > emits && rules > triggers { STRIPE_RULE }
                       else if triggers > emits { STRIPE_TRIG }
                       else { STRIPE_EMIT };
    let stripe_w = (node_rect.2 - node_rect.0).min(0.010);
    push_quad(out, node_rect.0, node_rect.1,
              node_rect.0 + stripe_w, node_rect.3, stripe_color);

    // Text visibility gates based on zoom. Below 35% the font
    // renders as noise so we skip text entirely. Between 35%
    // and 70% only the title is shown. Above 70% the time
    // stamp and statement counts appear too.
    let scale = canvas.effective_scale();
    if scale < 0.35 { return; }

    let title_px = (0.0060 * scale).clamp(0.0034, 0.0080);
    let title_x = node_rect.0 + stripe_w + 0.008;
    let title_y = node_rect.1 + 0.008;
    let title_max_w = node_rect.2 - title_x - 0.008;
    if title_max_w > 0.020 {
        let title = fit(&sec.name.to_uppercase(), title_px, title_max_w);
        push_small(out, &title, title_x, title_y, title_px, WHITE);
    }

    if scale < 0.7 { return; }

    let info_px = (0.0045 * scale).clamp(0.0032, 0.0060);
    let time_str = fmt_time(sec.at);
    push_small_right(out, &time_str, node_rect.2 - 0.008,
                     node_rect.1 + 0.007, info_px, ACCENT_HI);

    let stats = format!("E{} T{} W{} R{}",
                        emits, triggers, waits, rules);
    let stats_max_w = node_rect.2 - title_x - 0.008;
    if stats_max_w > 0.020 {
        let stats = fit(&stats, info_px, stats_max_w);
        push_small(out, &stats, title_x,
                   node_rect.3 - sh(info_px) - 0.008, info_px, DIM);
    }
}

fn count_stmts(stmts: &[Stmt]) -> (u32, u32, u32, u32) {
    let mut emits = 0;
    let mut triggers = 0;
    let mut waits = 0;
    let mut rules = 0;
    for s in stmts {
        match s {
            Stmt::Emit(_) => emits += 1,
            Stmt::Trigger(_) => triggers += 1,
            Stmt::Wait(_) => waits += 1,
            Stmt::Rule(_) | Stmt::Revert(_)
            | Stmt::Push(_) | Stmt::Pop(_) => rules += 1,
            Stmt::Repeat { body, .. } => {
                let (e, t, w, r) = count_stmts(body);
                emits += e;
                triggers += t;
                waits += w;
                rules += r;
            }
            Stmt::LocalVars(_) => {}
        }
    }
    (emits, triggers, waits, rules)
}

/// Human readable `at` display. Used inside nodes and in the
/// quick-info sidebar.
pub fn fmt_time(seconds: f32) -> String {
    let s = seconds.max(0.0);
    if s < 60.0 {
        format!("{:.1}s", s)
    } else {
        let m = (s / 60.0) as u32;
        let rem = s - (m * 60) as f32;
        format!("{}:{:04.1}", m, rem)
    }
}

fn fit(text: &str, px: f32, max_w: f32) -> String {
    if max_w <= 0.0 { return String::new(); }
    if sw(text, px) <= max_w { return text.to_string(); }
    let dots = "...";
    let dots_w = sw(dots, px);
    if dots_w >= max_w { return String::new(); }
    let chars: Vec<char> = text.chars().collect();
    let mut take = chars.len();
    while take > 0 {
        take -= 1;
        let c: String = chars.iter().take(take).collect();
        if sw(&c, px) + dots_w <= max_w {
            return format!("{}{}", c, dots);
        }
    }
    String::new()
}

fn draw_bezier_arrow(
    out: &mut Vec<Vertex>,
    a: [f32; 2], b: [f32; 2],
    color: [f32; 3],
    rect: Rect,
) {
    let dx = (b[0] - a[0]).abs() * 0.5;
    let c1 = [a[0] + dx, a[1]];
    let c2 = [b[0] - dx, b[1]];

    let segments = 16;
    let mut prev = a;
    for i in 1..=segments {
        let t = i as f32 / segments as f32;
        let mt = 1.0 - t;
        let p = [
            mt * mt * mt * a[0]
                + 3.0 * mt * mt * t * c1[0]
                + 3.0 * mt * t * t * c2[0]
                + t * t * t * b[0],
            mt * mt * mt * a[1]
                + 3.0 * mt * mt * t * c1[1]
                + 3.0 * mt * t * t * c2[1]
                + t * t * t * b[1],
        ];
        // Clip each segment against the visible rect.
        if rect_contains_pt(rect, (prev[0], prev[1]))
            || rect_contains_pt(rect, (p[0], p[1]))
        {
            draw_line(out, prev, p, 0.002, color);
        }
        prev = p;
    }

    // Arrow head at the terminal point.
    if rect_contains_pt(rect, (b[0], b[1])) {
        let dir_x = b[0] - prev[0];
        let dir_y = b[1] - prev[1];
        let len = (dir_x * dir_x + dir_y * dir_y).sqrt().max(1e-4);
        let nx = dir_x / len;
        let ny = dir_y / len;
        let size = 0.010;
        let back = [b[0] - nx * size, b[1] - ny * size];
        let left = [back[0] + ny * size * 0.45,
                    back[1] - nx * size * 0.45];
        let right = [back[0] - ny * size * 0.45,
                     back[1] + nx * size * 0.45];
        push_tri(out, b, left, right, color);
    }
}

fn draw_line(
    out: &mut Vec<Vertex>,
    a: [f32; 2], b: [f32; 2],
    thick: f32, color: [f32; 3],
) {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = (dx * dx + dy * dy).sqrt().max(1e-5);
    let nx = -dy / len * thick * 0.5;
    let ny = dx / len * thick * 0.5;
    use crate::pipeline::Vertex;
    out.push(Vertex::opaque([a[0] + nx, a[1] + ny], color));
    out.push(Vertex::opaque([a[0] - nx, a[1] - ny], color));
    out.push(Vertex::opaque([b[0] - nx, b[1] - ny], color));
    out.push(Vertex::opaque([a[0] + nx, a[1] + ny], color));
    out.push(Vertex::opaque([b[0] - nx, b[1] - ny], color));
    out.push(Vertex::opaque([b[0] + nx, b[1] + ny], color));
}

fn rect_contains_pt(r: Rect, p: (f32, f32)) -> bool {
    const TOL: f32 = 0.003;
    p.0 >= r.0 - TOL && p.0 <= r.2 + TOL
        && p.1 >= r.1 - TOL && p.1 <= r.3 + TOL
}