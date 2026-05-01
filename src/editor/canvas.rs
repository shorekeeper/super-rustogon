//! Pannable and zoomable 2D canvas.
//!
//! Hosts the section node graph. Works in two coordinate
//! spaces: world coordinates, which the graph module uses for
//! node positions, and screen coordinates, which the renderer
//! consumes. `Canvas::world_to_screen` and the inverse bridge
//! them; every graph render call funnels positions through
//! these helpers so zooming in and out stays crisp without any
//! per-node bookkeeping.
//!
//! # Interaction model
//!
//! * Left click on empty canvas area: start pan. Drag moves the
//!   view.
//! * Click the `+` or `-` buttons in the corner: zoom in or
//!   zoom out centered on the canvas midpoint.
//! * Input consumption: `handle_pan` refuses to start a pan
//!   when another widget (a node, a zoom button) has already
//!   consumed the click. The editor shell orchestrates this
//!   priority order each frame.
//!
//! # Grid
//!
//! An infinite grid of minor and major lines stays readable at
//! every zoom level. The step size is picked dynamically from
//! the 1/2/5 series so the grid never becomes either a blur or
//! a single big square.

use crate::pipeline::Vertex;
use crate::text_small::{push_small, text_width as sw, text_height as sh};
use crate::ui::draw::{push_outline, push_quad};
use crate::win32::Mouse;

/// Axis aligned rectangle on the screen, in game coordinates.
/// Tuple layout matches the rest of the editor: (x0, y0, x1, y1).
pub type Rect = (f32, f32, f32, f32);

const BG_CANVAS:   [f32; 3] = [0.05, 0.03, 0.08];
const GRID_MINOR:  [f32; 3] = [0.08, 0.05, 0.12];
const GRID_MAJOR:  [f32; 3] = [0.12, 0.08, 0.18];
const AXIS_COLOR:  [f32; 3] = [0.18, 0.12, 0.25];
const BG_PANEL:    [f32; 3] = [0.10, 0.04, 0.15];
const BG_PANEL_HI: [f32; 3] = [0.22, 0.10, 0.30];
const ACCENT:      [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:   [f32; 3] = [1.00, 0.80, 0.92];
const DIM:         [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:       [f32; 3] = [1.00, 1.00, 1.00];

/// Minimum and maximum log-zoom the camera permits. zoom of 0
/// means 1x; each step of 1.0 doubles or halves the scale.
pub const ZOOM_MIN: f32 = -3.0;
pub const ZOOM_MAX: f32 =  3.0;

pub struct Canvas {
    /// Pan position in world coordinates. This is the world
    /// point rendered at the center of the canvas rect.
    pub pan: [f32; 2],
    /// Logarithmic zoom level. `effective_scale` returns the
    /// linear multiplier.
    pub zoom: f32,

    dragging: bool,
    drag_start_screen: [f32; 2],
    drag_start_pan: [f32; 2],
    prev_left_down: bool,
}

impl Canvas {
    /// Construct with the given pan and zoom. The editor shell
    /// restores these from the session file so an author
    /// returns to the same view they left.
    pub fn new(pan: [f32; 2], zoom: f32) -> Self {
        Canvas {
            pan,
            zoom: zoom.clamp(ZOOM_MIN, ZOOM_MAX),
            dragging: false,
            drag_start_screen: [0.0, 0.0],
            drag_start_pan: [0.0, 0.0],
            prev_left_down: false,
        }
    }

    /// Linear zoom multiplier. 1.0 is neutral, 2.0 zooms in by
    /// a factor of two, 0.5 zooms out by a factor of two.
    pub fn effective_scale(&self) -> f32 {
        2.0f32.powf(self.zoom)
    }

    /// Convert a world position to a screen position inside
    /// `rect`. The center of `rect` corresponds to `self.pan`
    /// in world space.
    pub fn world_to_screen(&self, world: [f32; 2], rect: Rect) -> [f32; 2] {
        let scale = self.effective_scale();
        let cx = (rect.0 + rect.2) * 0.5;
        let cy = (rect.1 + rect.3) * 0.5;
        [
            cx + (world[0] - self.pan[0]) * scale,
            cy + (world[1] - self.pan[1]) * scale,
        ]
    }

    /// Invert of `world_to_screen`. Used on every pointer event
    /// to know which world point the author is interacting with.
    pub fn screen_to_world(&self, screen: [f32; 2], rect: Rect) -> [f32; 2] {
        let scale = self.effective_scale();
        let cx = (rect.0 + rect.2) * 0.5;
        let cy = (rect.1 + rect.3) * 0.5;
        [
            self.pan[0] + (screen[0] - cx) / scale,
            self.pan[1] + (screen[1] - cy) / scale,
        ]
    }

    /// Zoom by `delta` log units keeping `anchor_screen` locked
    /// in place. The world point currently under the anchor
    /// stays exactly under the anchor, which is the behavior
    /// every graphical editor on earth uses and the only one
    /// that feels natural.
    pub fn zoom_at(&mut self, anchor_screen: [f32; 2], rect: Rect, delta: f32) {
        let before = self.screen_to_world(anchor_screen, rect);
        self.zoom = (self.zoom + delta).clamp(ZOOM_MIN, ZOOM_MAX);
        let after = self.screen_to_world(anchor_screen, rect);
        self.pan[0] += before[0] - after[0];
        self.pan[1] += before[1] - after[1];
    }

    /// Handle a pan drag. Call only when the click did not
    /// land on any other interactive element in the canvas
    /// (`consumed = true` short-circuits the start of a new
    /// drag). Returns true while dragging is active so the
    /// caller can suppress other hover states.
    pub fn handle_pan(
        &mut self, pointer: (f32, f32), mouse: Mouse,
        rect: Rect, consumed: bool,
    ) -> bool {
        let pointer_arr = [pointer.0, pointer.1];
        let clicked = mouse.left_down && !self.prev_left_down;
        let released = !mouse.left_down && self.prev_left_down;
        self.prev_left_down = mouse.left_down;

        let in_rect = pointer.0 >= rect.0 && pointer.0 <= rect.2
                   && pointer.1 >= rect.1 && pointer.1 <= rect.3;

        if released {
            let was = self.dragging;
            self.dragging = false;
            if was { return true; }
        }

        if clicked && in_rect && !consumed {
            self.dragging = true;
            self.drag_start_screen = pointer_arr;
            self.drag_start_pan = self.pan;
        }

        if self.dragging && mouse.left_down {
            let scale = self.effective_scale();
            let dx = pointer_arr[0] - self.drag_start_screen[0];
            let dy = pointer_arr[1] - self.drag_start_screen[1];
            self.pan[0] = self.drag_start_pan[0] - dx / scale;
            self.pan[1] = self.drag_start_pan[1] - dy / scale;
            return true;
        }
        false
    }

    /// Returns `Some(+1)` or `Some(-1)` if the pointer is over
    /// a zoom button. Caller applies the zoom change via
    /// `zoom_at`.
    pub fn zoom_button_hit(&self, pointer: (f32, f32), rect: Rect) -> Option<i32> {
        let (plus_r, minus_r) = zoom_button_rects(rect);
        if rect_contains_pt(plus_r, pointer) { return Some(1); }
        if rect_contains_pt(minus_r, pointer) { return Some(-1); }
        None
    }

    /// Draw the background fill and the grid. Called first
    /// before any graph content so nodes overlay the grid.
    pub fn draw_grid(&self, out: &mut Vec<Vertex>, rect: Rect) {
        push_quad(out, rect.0, rect.1, rect.2, rect.3, BG_CANVAS);

        let step_world = self.pick_grid_step();
        let world_tl = self.screen_to_world([rect.0, rect.1], rect);
        let world_br = self.screen_to_world([rect.2, rect.3], rect);

        let first_x = (world_tl[0] / step_world).floor() * step_world;
        let first_y = (world_tl[1] / step_world).floor() * step_world;

        let major_every = 5.0;

        let mut x = first_x;
        let mut n = 0;
        while x <= world_br[0] && n < 400 {
            let sx = self.world_to_screen([x, 0.0], rect)[0];
            if sx >= rect.0 && sx <= rect.2 {
                let grid_index = (x / step_world).round();
                let is_major = (grid_index % major_every).abs() < 0.5;
                let is_origin = grid_index.abs() < 0.5;
                let col = if is_origin { AXIS_COLOR }
                          else if is_major { GRID_MAJOR }
                          else { GRID_MINOR };
                push_quad(out, sx - 0.0005, rect.1,
                               sx + 0.0005, rect.3, col);
            }
            x += step_world;
            n += 1;
        }

        let mut y = first_y;
        n = 0;
        while y <= world_br[1] && n < 400 {
            let sy = self.world_to_screen([0.0, y], rect)[1];
            if sy >= rect.1 && sy <= rect.3 {
                let grid_index = (y / step_world).round();
                let is_major = (grid_index % major_every).abs() < 0.5;
                let is_origin = grid_index.abs() < 0.5;
                let col = if is_origin { AXIS_COLOR }
                          else if is_major { GRID_MAJOR }
                          else { GRID_MINOR };
                push_quad(out, rect.0, sy - 0.0005,
                               rect.2, sy + 0.0005, col);
            }
            y += step_world;
            n += 1;
        }
    }

    /// Draw the zoom controls in the corner of the canvas rect.
    /// Called after the grid and before (or after) graph nodes;
    /// since the buttons sit in a fixed corner they do not
    /// interfere with node interaction.
    pub fn draw_zoom_controls(
        &self, out: &mut Vec<Vertex>, rect: Rect,
        pointer: (f32, f32),
    ) {
        let (plus_r, minus_r) = zoom_button_rects(rect);
        draw_mini_btn(out, plus_r, "+", pointer);
        draw_mini_btn(out, minus_r, "-", pointer);

        let label = format!("{:.0}%", self.effective_scale() * 100.0);
        let label_w = sw(&label, 0.0040);
        let x = plus_r.0 - 0.008 - label_w;
        let y = plus_r.1 + (plus_r.3 - plus_r.1) * 0.5 - sh(0.0040) * 0.5;
        push_small(out, &label, x, y, 0.0040, DIM);
    }

    /// Pick a world-space grid step that renders at roughly
    /// `target_screen_step` game units on screen, rounded to
    /// the 1 / 2 / 5 series so the spacing looks deliberate
    /// rather than arbitrary.
    fn pick_grid_step(&self) -> f32 {
        let scale = self.effective_scale();
        let target_screen_step = 0.08;
        let target_world = target_screen_step / scale;
        let log = target_world.log10();
        let floor_log = log.floor();
        let mantissa = 10f32.powf(log - floor_log);
        let quantised = if mantissa < 1.5 { 1.0 }
                        else if mantissa < 3.5 { 2.0 }
                        else if mantissa < 7.5 { 5.0 }
                        else { 10.0 };
        quantised * 10f32.powf(floor_log)
    }
}

fn zoom_button_rects(rect: Rect) -> (Rect, Rect) {
    let size = 0.028;
    let gap = 0.005;
    let margin = 0.010;
    let x1 = rect.2 - margin;
    let y1 = rect.3 - margin;
    let minus = (x1 - size, y1 - size, x1, y1);
    let plus  = (x1 - size, minus.1 - gap - size, x1, minus.1 - gap);
    (plus, minus)
}

fn draw_mini_btn(
    out: &mut Vec<Vertex>, r: Rect, label: &str,
    pointer: (f32, f32),
) {
    let hit = rect_contains_pt(r, pointer);
    let bg = if hit { BG_PANEL_HI } else { BG_PANEL };
    let ring = if hit { ACCENT_HI } else { ACCENT };
    push_quad(out, r.0, r.1, r.2, r.3, bg);
    push_outline(out, r.0, r.1, r.2, r.3, 0.002, ring);
    let cx = (r.0 + r.2) * 0.5;
    let cy = (r.1 + r.3) * 0.5;
    let px = 0.0060;
    let w = sw(label, px);
    push_small(out, label,
        cx - w * 0.5, cy - sh(px) * 0.5, px, WHITE);
}

fn rect_contains_pt(r: Rect, p: (f32, f32)) -> bool {
    const TOL: f32 = 0.003;
    p.0 >= r.0 - TOL && p.0 <= r.2 + TOL
        && p.1 >= r.1 - TOL && p.1 <= r.3 + TOL
}