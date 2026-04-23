//! Immediate-mode widget toolkit.
//!
//! Design goals:
//!
//! * No retained tree: every frame the caller describes the UI it
//!   wants, widgets return immediately whether they were clicked,
//!   dragged, etc. This matches the existing rendering model
//!   (rebuild the geometry every frame) and avoids the usual
//!   retained-mode bookkeeping.
//!
//! * All visual state (hover ease, drag anchor) lives inside the
//!   [`Ui`] struct keyed by a stable `WidgetId`. Callers pick ids
//!   manually; duplicates silently share state, which is sometimes
//!   what you want (one hover highlight spans two draws) and can
//!   be avoided by splitting the id when it is not.
//!
//! * Drawing is emitted into the same `Vec<Vertex>` the rest of
//!   the game uses, so the UI layer adds zero pipelines.
//!
//! A typical use site looks like this:
//!
//! ```ignore
//! ui.begin_frame(dt, mouse, input, cw, ch);
//! if ui.button(id::PLAY, rect, "PLAY") { /* ... */ }
//! ui.slider_f32(id::VOL, rect, "VOLUME", &mut cfg.master_volume, 0.0..=1.0);
//! ui.end_frame(&mut vertices);
//! ```

pub mod widget;
pub mod draw;

pub use widget::*;
pub use draw::*;

use std::collections::HashMap;

use crate::pipeline::Vertex;
use crate::text::{
    push_text, push_text_centered, push_text_right, text_height, text_width,
};
use crate::win32::{Input, Mouse};

/// Unique identifier of a widget across a frame. Plain u32 so it
/// can be a `const` and fit in tables. Callers typically define
/// their own `mod id { pub const PLAY: WidgetId = 1; ... }` block.
pub type WidgetId = u32;

/// Axis-aligned rectangle in game space. The UI system works
/// purely in game coordinates (same as the rest of the renderer):
/// x grows to the right, y grows downward, a unit circle at the
/// origin stays a circle on screen.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn from_center(cx: f32, cy: f32, hw: f32, hh: f32) -> Self {
        Rect { x0: cx - hw, y0: cy - hh, x1: cx + hw, y1: cy + hh }
    }
    pub fn xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x0: x, y0: y, x1: x + w, y1: y + h }
    }
    pub fn center(self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }
    pub fn width(self)  -> f32 { self.x1 - self.x0 }
    pub fn height(self) -> f32 { self.y1 - self.y0 }
    pub fn contains(self, px: f32, py: f32) -> bool {
        px >= self.x0 && px <= self.x1 && py >= self.y0 && py <= self.y1
    }
    pub fn inset(self, m: f32) -> Self {
        Rect { x0: self.x0 + m, y0: self.y0 + m, x1: self.x1 - m, y1: self.y1 - m }
    }
    /// Split the rect into `n` equal cells along X. Returns the
    /// `i`'th cell with a `gap` pixel gap between cells.
    pub fn split_x(self, n: usize, i: usize, gap: f32) -> Self {
        let total_gap = gap * (n.saturating_sub(1)) as f32;
        let cell = (self.width() - total_gap) / n as f32;
        let x0 = self.x0 + (cell + gap) * i as f32;
        Rect { x0, y0: self.y0, x1: x0 + cell, y1: self.y1 }
    }
    /// Split into rows.
    pub fn split_y(self, n: usize, i: usize, gap: f32) -> Self {
        let total_gap = gap * (n.saturating_sub(1)) as f32;
        let cell = (self.height() - total_gap) / n as f32;
        let y0 = self.y0 + (cell + gap) * i as f32;
        Rect { x0: self.x0, y0, x1: self.x1, y1: y0 + cell }
    }
    /// Take a top slice of given height. Useful for "give me the
    /// header row, the rest is content".
    pub fn split_top(self, h: f32) -> (Rect, Rect) {
        let mid = self.y0 + h;
        (Rect { x0: self.x0, y0: self.y0, x1: self.x1, y1: mid },
         Rect { x0: self.x0, y0: mid,     x1: self.x1, y1: self.y1 })
    }
    pub fn split_left(self, w: f32) -> (Rect, Rect) {
        let mid = self.x0 + w;
        (Rect { x0: self.x0, y0: self.y0, x1: mid,     y1: self.y1 },
         Rect { x0: mid,     y0: self.y0, x1: self.x1, y1: self.y1 })
    }
}

/// Centralized color / metric theme so every widget shares the
/// same look. Values mirror the hand-picked constants from the
/// old menu so there is no visual regression.
#[derive(Clone, Copy)]
pub struct Theme {
    pub accent:       [f32; 3],
    pub accent_hi:    [f32; 3],
    pub dim:          [f32; 3],
    pub white:        [f32; 3],
    pub bg_panel:     [f32; 3],
    pub bg_panel_hi:  [f32; 3],
    pub bg_deep:      [f32; 3],
    pub danger:       [f32; 3],
    pub pixel_title:  f32,
    pub pixel_h1:     f32,
    pub pixel_h2:     f32,
    pub pixel_button: f32,
    pub pixel_body:   f32,
    pub pixel_small:  f32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent:       [1.00, 0.45, 0.75],
            accent_hi:    [1.00, 0.80, 0.92],
            dim:          [0.55, 0.42, 0.52],
            white:        [1.00, 1.00, 1.00],
            bg_panel:     [0.10, 0.04, 0.15],
            bg_panel_hi:  [0.22, 0.10, 0.30],
            bg_deep:      [0.03, 0.01, 0.06],
            danger:       [1.00, 0.30, 0.30],
            pixel_title:  0.013,
            pixel_h1:     0.010,
            pixel_h2:     0.007,
            pixel_button: 0.010,
            pixel_body:   0.005,
            pixel_small:  0.0045,
        }
    }
}

/// Current view bounds, computed from framebuffer aspect. The UI
/// module stores this for layout helpers that want to anchor to
/// screen edges without knowing the raw size.
#[derive(Clone, Copy, Debug)]
pub struct ViewBounds {
    pub aspect: f32,
    pub left:   f32,
    pub right:  f32,
    pub top:    f32,
    pub bottom: f32,
}

impl ViewBounds {
    pub fn from_pixels(cw: u32, ch: u32) -> Self {
        let aspect = (cw.max(1) as f32) / (ch.max(1) as f32);
        if aspect >= 1.0 {
            ViewBounds { aspect, left: -aspect, right: aspect, top: -1.0, bottom: 1.0 }
        } else {
            let inv = 1.0 / aspect;
            ViewBounds { aspect, left: -1.0, right: 1.0, top: -inv, bottom: inv }
        }
    }
}

/// Per-frame UI input state.
#[derive(Clone, Copy, Debug)]
pub struct UiInput {
    pub pointer:       (f32, f32),
    pub left_down:     bool,
    pub left_clicked:  bool,
    pub left_released: bool,
    pub up_edge:       bool,
    pub down_edge:     bool,
    pub left_edge:     bool,
    pub right_edge:    bool,
    pub enter_edge:    bool,
    pub escape_edge:   bool,
}

/// Immediate-mode UI context.
pub struct Ui {
    pub theme: Theme,
    pub view:  ViewBounds,

    time: f32,
    hover:        HashMap<WidgetId, f32>,
    hover_target: HashMap<WidgetId, bool>,
    dragging:     Option<WidgetId>,

    prev_left_down: bool,
    prev_up: bool, prev_down: bool, prev_left: bool, prev_right: bool,
    prev_enter: bool, prev_escape: bool,

    input: UiInput,

    /// Geometry accumulated by widget calls during this frame.
    /// Flushed into the user-supplied vertex buffer on end_frame.
    geom: Vec<Vertex>,

    pointer_captured: bool,
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            theme: Theme::default(),
            view:  ViewBounds::from_pixels(1280, 720),
            time:  0.0,
            hover: HashMap::new(),
            hover_target: HashMap::new(),
            dragging: None,
            prev_left_down: false,
            prev_up: false, prev_down: false, prev_left: false, prev_right: false,
            prev_enter: false, prev_escape: false,
            input: UiInput {
                pointer: (0.0, 0.0),
                left_down: false, left_clicked: false, left_released: false,
                up_edge: false, down_edge: false,
                left_edge: false, right_edge: false,
                enter_edge: false, escape_edge: false,
            },
            geom: Vec::with_capacity(32_768),
            pointer_captured: false,
        }
    }

    /// Establish input state for the new frame. Pointer position
    /// is converted from window pixels into game coordinates the
    /// same way the renderer converts geometry.
    pub fn begin_frame(
        &mut self, dt: f32, mouse: Mouse, input: Input, cw: u32, ch: u32,
    ) {
        self.time += dt;
        self.view = ViewBounds::from_pixels(cw, ch);

        let (sx, sy) = crate::renderer::aspect_scale(cw, ch);
        let cwf = cw.max(1) as f32;
        let chf = ch.max(1) as f32;
        let nx = ((mouse.x as f32 / cwf) * 2.0 - 1.0) / sx;
        let ny = ((mouse.y as f32 / chf) * 2.0 - 1.0) / sy;

        let left_clicked  =  mouse.left_down && !self.prev_left_down;
        let left_released = !mouse.left_down &&  self.prev_left_down;

        self.input = UiInput {
            pointer: (nx, ny),
            left_down: mouse.left_down,
            left_clicked,
            left_released,
            up_edge:     input.up     && !self.prev_up,
            down_edge:   input.down   && !self.prev_down,
            left_edge:   input.left   && !self.prev_left,
            right_edge:  input.right  && !self.prev_right,
            enter_edge:  input.enter  && !self.prev_enter,
            escape_edge: input.escape && !self.prev_escape,
        };

        self.prev_left_down = mouse.left_down;
        self.prev_up = input.up; self.prev_down = input.down;
        self.prev_left = input.left; self.prev_right = input.right;
        self.prev_enter = input.enter; self.prev_escape = input.escape;

        if left_released { self.dragging = None; }
        self.hover_target.clear();
        self.pointer_captured = false;
        self.geom.clear();

        // Ease existing hover values toward their targets. Widgets
        // set the target during the frame; the ease amount from
        // last frame is already what they see on draw.
        let blend = 1.0 - (-14.0_f32 * dt).exp();
        let keys: Vec<WidgetId> = self.hover.keys().copied().collect();
        for id in keys {
            let target = if *self.hover_target.get(&id).unwrap_or(&false) { 1.0 } else { 0.0 };
            let v = self.hover.get_mut(&id).unwrap();
            *v += (target - *v) * blend;
            if *v < 0.001 && target == 0.0 { self.hover.remove(&id); }
        }
    }

    /// Copy accumulated widget geometry into the caller's vertex
    /// list. The user is expected to call this after every widget
    /// for the frame has been emitted.
    pub fn end_frame(&mut self, out: &mut Vec<Vertex>) {
        out.extend_from_slice(&self.geom);
    }

    pub fn geom_mut(&mut self) -> &mut Vec<Vertex> { &mut self.geom }

    pub fn input(&self) -> UiInput { self.input }

    pub fn is_dragging(&self, id: WidgetId) -> bool { self.dragging == Some(id) }
    pub fn set_dragging(&mut self, id: WidgetId) { self.dragging = Some(id); }

    fn set_hover(&mut self, id: WidgetId, v: bool) {
        if v { self.hover_target.insert(id, true); }
        if !self.hover.contains_key(&id) && v {
            self.hover.insert(id, 0.0);
        }
    }

    fn hover_eased(&self, id: WidgetId) -> f32 {
        let t = self.hover.get(&id).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    // ----- primitives (widgets) -----

    /// Simple clickable button with label. Returns true on the
    /// frame the click happens.
    pub fn button(&mut self, id: WidgetId, rect: Rect, label: &str) -> bool {
        let (px, py) = self.input.pointer;
        let hovered = rect.contains(px, py);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);
        let theme = self.theme;

        let bg   = mix3(theme.bg_panel, theme.bg_panel_hi, h);
        let ring = mix3(theme.accent, theme.accent_hi, h);

        draw::push_quad(&mut self.geom, rect.x0, rect.y0, rect.x1, rect.y1, bg);
        draw::push_outline(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1, 0.004 + 0.002 * h, ring);

        let (cx, cy) = rect.center();
        let w = text_width(label, theme.pixel_button);
        push_text(&mut self.geom, label,
            cx - w * 0.5, cy - text_height(theme.pixel_button) * 0.5,
            theme.pixel_button, theme.white);

        let clicked = hovered && self.input.left_clicked;
        if clicked { self.pointer_captured = true; }
        clicked
    }

    /// Small compact button (BACK / RESET etc.).
    pub fn button_small(
        &mut self, id: WidgetId, rect: Rect, label: &str,
    ) -> bool {
        let (px, py) = self.input.pointer;
        let hovered = rect.contains(px, py);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);
        let theme = self.theme;

        let bg   = mix3(theme.bg_panel, theme.bg_panel_hi, h);
        let ring = mix3(theme.accent, theme.accent_hi, h);

        draw::push_quad(&mut self.geom, rect.x0, rect.y0, rect.x1, rect.y1, bg);
        draw::push_outline(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1, 0.003 + 0.001 * h, ring);

        let (cx, cy) = rect.center();
        let w = text_width(label, theme.pixel_body);
        push_text(&mut self.geom, label,
            cx - w * 0.5, cy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.white);

        let clicked = hovered && self.input.left_clicked;
        if clicked { self.pointer_captured = true; }
        clicked
    }

    /// Numeric slider. `range` is `min..=max`. Returns true if the
    /// value changed this frame.
    pub fn slider_f32(
        &mut self, id: WidgetId, rect: Rect,
        label: &str, value: &mut f32, min: f32, max: f32,
    ) -> bool {
        let theme = self.theme;
        let (px, py) = self.input.pointer;

        // Layout inside the rect: label on the left, track in the
        // middle, value string on the right.
        let label_w = 0.35 * rect.width();
        let value_w = 0.15 * rect.width();
        let (cx, cy) = rect.center();
        let label_rect = Rect::xywh(rect.x0, rect.y0, label_w, rect.height());
        let track_rect = Rect::xywh(rect.x0 + label_w, cy - 0.010,
                                    rect.width() - label_w - value_w, 0.020);
        let value_rect = Rect::xywh(rect.x1 - value_w, rect.y0,
                                    value_w, rect.height());

        push_text(&mut self.geom, label,
            label_rect.x0, cy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.white);

        // Track.
        draw::push_quad(&mut self.geom,
            track_rect.x0 - 0.004, track_rect.y0 - 0.004,
            track_rect.x1 + 0.004, track_rect.y1 + 0.004,
            theme.bg_deep);
        draw::push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0, track_rect.x1, track_rect.y1,
            theme.bg_panel);

        let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
        let fx = track_rect.x0 + track_rect.width() * t;
        draw::push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0, fx, track_rect.y1, theme.accent);

        // Handle.
        let handle_hit = (px - fx).abs() < 0.04
                      && (py - cy).abs() < rect.height() * 0.5;
        let track_hit = track_rect.inset(-0.01).contains(px, py);
        let hovered = handle_hit || self.dragging == Some(id);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);

        let ring = mix3(theme.accent, theme.accent_hi, h);
        let r = 0.024 + 0.005 * h;
        draw::push_hex(&mut self.geom, fx, cy, r, theme.bg_deep);
        draw::push_hex_ring(&mut self.geom, fx, cy, r, r - 0.006, ring);
        draw::push_hex(&mut self.geom, fx, cy, r * 0.40, ring);

        let mut changed = false;
        if self.input.left_clicked && (handle_hit || track_hit) {
            self.dragging = Some(id);
            self.pointer_captured = true;
        }
        if self.dragging == Some(id) && self.input.left_down {
            let nt = ((px - track_rect.x0) / track_rect.width()).clamp(0.0, 1.0);
            let newv = min + (max - min) * nt;
            if (newv - *value).abs() > 1e-6 {
                *value = newv;
                changed = true;
            }
            self.pointer_captured = true;
        }

        // Value readout on the right.
        let vstr = format!("{:.2}", *value);
        let vw = text_width(&vstr, theme.pixel_body);
        push_text(&mut self.geom, &vstr,
            value_rect.x1 - vw, cy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.accent_hi);

        changed
    }

    /// Percentage slider (0..=1, shown as 0%..100%).
    pub fn slider_pct(
        &mut self, id: WidgetId, rect: Rect, label: &str, value: &mut f32,
    ) -> bool {
        let theme = self.theme;
        let (px, py) = self.input.pointer;

        let label_w = 0.40 * rect.width();
        let value_w = 0.12 * rect.width();
        let (_, cy) = rect.center();
        let track_rect = Rect::xywh(rect.x0 + label_w, cy - 0.010,
                                    rect.width() - label_w - value_w, 0.020);

        push_text(&mut self.geom, label,
            rect.x0, cy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.white);

        draw::push_quad(&mut self.geom,
            track_rect.x0 - 0.004, track_rect.y0 - 0.004,
            track_rect.x1 + 0.004, track_rect.y1 + 0.004,
            theme.bg_deep);
        draw::push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0, track_rect.x1, track_rect.y1,
            theme.bg_panel);

        let t = value.clamp(0.0, 1.0);
        let fx = track_rect.x0 + track_rect.width() * t;
        draw::push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0, fx, track_rect.y1, theme.accent);

        let handle_hit = (px - fx).abs() < 0.04
                      && (py - cy).abs() < rect.height() * 0.5;
        let track_hit = track_rect.inset(-0.01).contains(px, py);
        let hovered = handle_hit || self.dragging == Some(id);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);

        let ring = mix3(theme.accent, theme.accent_hi, h);
        let r = 0.024 + 0.005 * h;
        draw::push_hex(&mut self.geom, fx, cy, r, theme.bg_deep);
        draw::push_hex_ring(&mut self.geom, fx, cy, r, r - 0.006, ring);
        draw::push_hex(&mut self.geom, fx, cy, r * 0.40, ring);

        let mut changed = false;
        if self.input.left_clicked && (handle_hit || track_hit) {
            self.dragging = Some(id);
            self.pointer_captured = true;
        }
        if self.dragging == Some(id) && self.input.left_down {
            let nt = ((px - track_rect.x0) / track_rect.width()).clamp(0.0, 1.0);
            if (nt - *value).abs() > 1e-6 {
                *value = nt;
                changed = true;
            }
            self.pointer_captured = true;
        }

        let vstr = format!("{}%", (value.clamp(0.0, 1.0) * 100.0).round() as i32);
        let vw = text_width(&vstr, theme.pixel_body);
        push_text(&mut self.geom, &vstr,
            rect.x1 - vw, cy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.accent_hi);

        changed
    }

    /// Toggle (ON / OFF button). Returns true if value changed.
    pub fn toggle(
        &mut self, id: WidgetId, rect: Rect, label: &str, value: &mut bool,
    ) -> bool {
        let theme = self.theme;
        let (_, cy) = rect.center();
        push_text(&mut self.geom, label,
            rect.x0, cy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.white);

        let btn_w = 0.14;
        let btn_rect = Rect::xywh(rect.x1 - btn_w, rect.y0 + 0.008,
                                  btn_w, rect.height() - 0.016);

        let (px, py) = self.input.pointer;
        let hovered = btn_rect.contains(px, py);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);

        let active_col = mix3(theme.bg_panel_hi, theme.accent, 1.0);
        let idle_col   = theme.bg_panel;
        let bg   = if *value { active_col } else { mix3(idle_col, theme.bg_panel_hi, h) };
        let ring = mix3(theme.accent, theme.accent_hi, h);

        draw::push_quad(&mut self.geom, btn_rect.x0, btn_rect.y0,
            btn_rect.x1, btn_rect.y1, bg);
        draw::push_outline(&mut self.geom,
            btn_rect.x0, btn_rect.y0, btn_rect.x1, btn_rect.y1, 0.003, ring);

        let text = if *value { "ON" } else { "OFF" };
        let tw = text_width(text, theme.pixel_body);
        let (bcx, bcy) = btn_rect.center();
        push_text(&mut self.geom, text,
            bcx - tw * 0.5, bcy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, if *value { theme.white } else { theme.dim });

        let mut changed = false;
        if hovered && self.input.left_clicked {
            *value = !*value;
            changed = true;
            self.pointer_captured = true;
        }
        changed
    }

    /// Enum cycler: shows `<  LABEL : CURRENT  >` with left/right
    /// arrows. Returns true if selection changed. `next` is called
    /// to advance to the next variant when the right arrow is
    /// clicked; cyclers are symmetric, so we also offer a way to
    /// go backward by cycling all the way around.
    pub fn cycle<T: Copy + PartialEq>(
        &mut self, id: WidgetId, rect: Rect, label: &str,
        value: &mut T, variants: &[(T, &str)],
    ) -> bool {
        let theme = self.theme;
        let (_, cy) = rect.center();
        push_text(&mut self.geom, label,
            rect.x0, cy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.white);

        let right_btn_w = 0.05;
        let left_btn_w  = 0.05;
        let name_box = Rect::xywh(rect.x0 + 0.40 * rect.width(),
            rect.y0 + 0.008, rect.width() * 0.55, rect.height() - 0.016);
        let left_rect  = Rect::xywh(name_box.x0, name_box.y0, left_btn_w, name_box.height());
        let right_rect = Rect::xywh(name_box.x1 - right_btn_w, name_box.y0,
                                    right_btn_w, name_box.height());
        let center_rect = Rect::xywh(left_rect.x1, name_box.y0,
            name_box.width() - left_btn_w - right_btn_w, name_box.height());

        let (px, py) = self.input.pointer;
        let left_hit  = left_rect.contains(px, py);
        let right_hit = right_rect.contains(px, py);
        let id_left  = id.wrapping_add(1);
        let id_right = id.wrapping_add(2);
        self.set_hover(id_left, left_hit);
        self.set_hover(id_right, right_hit);

        let lh = self.hover_eased(id_left);
        let rh = self.hover_eased(id_right);

        let lc = mix3(theme.accent, theme.accent_hi, lh);
        let rc = mix3(theme.accent, theme.accent_hi, rh);

        let (lcx, lcy) = left_rect.center();
        let (rcx, rcy) = right_rect.center();
        draw::push_tri(&mut self.geom,
            [lcx - 0.018 - 0.006 * lh, lcy],
            [lcx + 0.014, lcy - 0.020],
            [lcx + 0.014, lcy + 0.020], lc);
        draw::push_tri(&mut self.geom,
            [rcx + 0.018 + 0.006 * rh, rcy],
            [rcx - 0.014, rcy - 0.020],
            [rcx - 0.014, rcy + 0.020], rc);

        let cur_idx = variants.iter().position(|(v, _)| *v == *value).unwrap_or(0);
        let name = variants.get(cur_idx).map(|(_, n)| *n).unwrap_or("");
        let (ccx, ccy) = center_rect.center();
        let w = text_width(name, theme.pixel_body);
        push_text(&mut self.geom, name,
            ccx - w * 0.5, ccy - text_height(theme.pixel_body) * 0.5,
            theme.pixel_body, theme.accent_hi);

        let mut changed = false;
        if right_hit && self.input.left_clicked {
            let n = variants.len();
            if n > 0 {
                let ni = (cur_idx + 1) % n;
                *value = variants[ni].0;
                changed = true;
                self.pointer_captured = true;
            }
        }
        if left_hit && self.input.left_clicked {
            let n = variants.len();
            if n > 0 {
                let ni = (cur_idx + n - 1) % n;
                *value = variants[ni].0;
                changed = true;
                self.pointer_captured = true;
            }
        }
        changed
    }

    /// Horizontal tab bar. `current` is clamped into range. Returns
    /// true when the active tab changes.
    pub fn tabs(
        &mut self, id: WidgetId, rect: Rect,
        tabs: &[&str], current: &mut usize,
    ) -> bool {
        let theme = self.theme;
        let n = tabs.len().max(1);
        let gap = 0.006;
        let mut changed = false;
        for (i, label) in tabs.iter().enumerate() {
            let cell = rect.split_x(n, i, gap);
            let (px, py) = self.input.pointer;
            let hovered = cell.contains(px, py);
            let tab_id = id.wrapping_add(i as u32 + 1);
            self.set_hover(tab_id, hovered);
            let h = self.hover_eased(tab_id);
            let active = i == *current;

            let bg = if active {
                theme.bg_panel_hi
            } else {
                mix3(theme.bg_panel, theme.bg_panel_hi, h)
            };
            draw::push_quad(&mut self.geom,
                cell.x0, cell.y0, cell.x1, cell.y1, bg);
            if active {
                let bar_h = 0.006;
                draw::push_quad(&mut self.geom,
                    cell.x0, cell.y1 - bar_h, cell.x1, cell.y1, theme.accent);
            } else {
                draw::push_outline(&mut self.geom,
                    cell.x0, cell.y0, cell.x1, cell.y1,
                    0.002, mix3(theme.dim, theme.accent, h));
            }

            let (cx, cy) = cell.center();
            let w = text_width(label, theme.pixel_button);
            push_text(&mut self.geom, label,
                cx - w * 0.5, cy - text_height(theme.pixel_button) * 0.5,
                theme.pixel_button,
                if active { theme.white } else { theme.dim });

            if hovered && self.input.left_clicked && !active {
                *current = i;
                changed = true;
                self.pointer_captured = true;
            }
        }
        if *current >= n { *current = 0; }
        changed
    }

    /// Static text label. Convenience for menus that want a header
    /// row without building a widget.
    pub fn label(&mut self, rect: Rect, text: &str, pixel_size: f32,
                 color: [f32; 3]) {
        let (_, cy) = rect.center();
        push_text(&mut self.geom, text,
            rect.x0, cy - text_height(pixel_size) * 0.5, pixel_size, color);
    }

    pub fn label_centered(&mut self, rect: Rect, text: &str, pixel_size: f32,
                          color: [f32; 3]) {
        let (cx, cy) = rect.center();
        push_text_centered(&mut self.geom, text, cx,
            cy - text_height(pixel_size) * 0.5, pixel_size, color);
    }

    pub fn label_right(&mut self, rect: Rect, text: &str, pixel_size: f32,
                       color: [f32; 3]) {
        let (_, cy) = rect.center();
        push_text_right(&mut self.geom, text, rect.x1,
            cy - text_height(pixel_size) * 0.5, pixel_size, color);
    }

    pub fn panel(&mut self, rect: Rect) {
        let theme = self.theme;
        draw::push_quad(&mut self.geom, rect.x0, rect.y0, rect.x1, rect.y1,
            mix3(theme.bg_deep, theme.bg_panel, 0.40));
        draw::push_outline(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1, 0.003, theme.accent);
    }

    pub fn horizontal_rule(&mut self, y: f32, x0: f32, x1: f32, color: [f32; 3]) {
        draw::push_quad(&mut self.geom, x0, y - 0.002, x1, y + 0.002, color);
    }

    pub fn pointer_captured(&self) -> bool { self.pointer_captured }
}

// ----- small helpers re-exported -----

pub fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t,
     a[1] + (b[1] - a[1]) * t,
     a[2] + (b[2] - a[2]) * t]
}

pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}