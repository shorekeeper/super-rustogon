//! Immediate-mode UI toolkit (revision 2).
//!
//! Major changes from the previous revision:
//!
//! * **Design-space coordinates.** Layout is now expressed
//!   in "design pixels" against a fixed reference resolution
//!   (1080 px tall at 16:9). The new `Layout` struct (see
//!   `layout.rs`) is recomputed each frame from the
//!   framebuffer extent and holds the design-pixel-to-game
//!   scale factor plus the four edges of the visible area.
//!
//! * **Anchor-based positioning.** Top-level widgets accept
//!   a `Frame` (anchor + offset + size in design pixels)
//!   which resolves to a game-space `Rect`. Anchoring to the
//!   nine compass points handles aspect ratio variation
//!   automatically: a widget anchored to `BottomRight` always
//!   sits the same distance from the bottom-right corner of
//!   the visible viewport, whether the window is 4:3, 16:9,
//!   ultrawide, or portrait.
//!
//! * **SDF text rendering.** All widget labels go through
//!   `crate::font` instead of the legacy bitmap font in
//!   `crate::text`. Font sizes in `Theme` are now em heights
//!   in design pixels. Text scales smoothly to any size and
//!   stays anti-aliased on hi-DPI monitors.
//!
//! * **Two output buffers.** `end_frame` extends both a
//!   shape vertex buffer (consumed by the main pipeline) and
//!   a text vertex buffer (consumed by the SDF text stage).
//!   The renderer accepts both buffers separately, so the
//!   caller does not have to merge them.
//!
//! Widget IDs and the immediate-mode call pattern are
//! unchanged: the caller passes a stable WidgetId per widget
//! and the Ui struct tracks hover ease and drag state in
//! HashMaps keyed by ID.
//!
//! # Custom widgets
//!
//! The built in widgets cover the common cases (button,
//! slider, toggle, cycle, tabs, panel) but the engine often
//! needs widgets whose visual identity is too specific to
//! squeeze through a generic API: a level select rail with
//! slanted edges, a music panel with a slide animation, a
//! main menu bar with a chevron and accent stripe. For
//! those, a higher level module owns the drawing code and
//! reaches into the Ui through three escape hatches:
//!
//! * `track_hover(id, hovered)` registers a hover target
//!   for the current frame and returns the eased hover
//!   value the caller uses to drive its own animations.
//! * `dragging_id()` and `set_dragging(id)` cooperate with
//!   the built in drag tracker so a custom widget's drag
//!   state survives across frames the same way a slider's
//!   drag state does, with no extra bookkeeping.
//! * `capture_pointer()` flags the pointer as consumed for
//!   the frame, mirroring what the built in widgets do
//!   internally on click.
//!
//! These three plus direct access to `geom_mut()` and
//! `text_mut()` are enough to build any widget against the
//! same input and animation tracker the built in widgets
//! use, without forcing the higher level module to keep its
//! own hover map.
//!
//! # Two API levels
//!
//! Most widgets come in two flavours:
//!
//! * `widget(id, frame, ...)` - top-level entry point. Takes
//!   a Frame (declarative anchor + offset + size in design
//!   pixels) and resolves it internally to a Rect. Use this
//!   when the widget is anchored to a viewport edge or
//!   centred on screen.
//!
//! * `widget_rect(id, rect, ...)` - same widget, but takes
//!   a pre-resolved Rect. Use this when you have already
//!   subdivided a parent rectangle into cells (via
//!   `Rect::split_x` and friends) and want to drop a widget
//!   into one of the cells.
//!
//! Both paths share their implementation, so they produce
//! identical pixels for equivalent inputs.

pub mod widget;
pub mod draw;
pub mod layout;

pub use widget::*;
pub use draw::*;
pub use layout::{Anchor, DESIGN_REF, Frame, Layout, Rect};

use std::collections::HashMap;

use crate::font::{self, TextVertex};
use crate::pipeline::Vertex;
use crate::win32::{Input, Mouse};

/// Stable per-widget identifier. Callers pick these
/// manually; the framework tracks animation and drag state
/// keyed by ID across frames.
pub type WidgetId = u32;

/// Centralised colour and font theme. Font fields hold em
/// heights in design pixels. Colour fields are linear RGB
/// triples in 0..1.
///
/// The defaults match the previous bitmap-font theme as
/// closely as is reasonable: same hue family, same general
/// hierarchy of font sizes, but tuned upward in absolute
/// terms because em height is a slightly different metric
/// than the legacy "font pixel size" the bitmap font used.
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
    /// Title text size (em height in design pixels).
    /// Reserved for splash and main-menu headers.
    pub font_title:   f32,
    /// Section heading size.
    pub font_h1:      f32,
    /// Sub-heading size.
    pub font_h2:      f32,
    /// Button-label size.
    pub font_button:  f32,
    /// Body / paragraph size.
    pub font_body:    f32,
    /// Small-print size, used for hints and value readouts.
    pub font_small:   f32,
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
            font_title:   72.0,
            font_h1:      48.0,
            font_h2:      32.0,
            font_button:  28.0,
            font_body:    22.0,
            font_small:   18.0,
        }
    }
}

/// Per-frame input snapshot consumed by widgets. Pointer
/// position is in game-space coordinates; edge flags are
/// true on the single frame the corresponding key
/// transitions from up to down.
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
///
/// One Ui instance owns the layout, theme, hover state, drag
/// tracker, and per-frame scratch geometry buffers for one
/// UI surface. The shell creates exactly one Ui at startup
/// and reuses it across every frame.
///
/// The struct is intentionally not `Send` or `Sync`; UI runs
/// on the main thread and does not need to.
pub struct Ui {
    pub theme:  Theme,
    pub layout: Layout,
    time:       f32,

    hover:        HashMap<WidgetId, f32>,
    hover_target: HashMap<WidgetId, bool>,
    dragging:     Option<WidgetId>,

    prev_left_down: bool,
    prev_up:        bool,
    prev_down:      bool,
    prev_left:      bool,
    prev_right:     bool,
    prev_enter:     bool,
    prev_escape:    bool,

    input: UiInput,
    geom:  Vec<Vertex>,
    text:  Vec<TextVertex>,

    pointer_captured: bool,
}

impl Ui {
    /// Construct an empty Ui with default theme. Layout is
    /// seeded against a 720p reference until the first
    /// `begin_frame` arrives with the real framebuffer size.
    pub fn new() -> Self {
        Ui {
            theme:  Theme::default(),
            layout: Layout::from_framebuffer(1280, 720),
            time:   0.0,

            hover:        HashMap::new(),
            hover_target: HashMap::new(),
            dragging:     None,

            prev_left_down: false,
            prev_up:        false,
            prev_down:      false,
            prev_left:      false,
            prev_right:     false,
            prev_enter:     false,
            prev_escape:    false,

            input: UiInput {
                pointer: (0.0, 0.0),
                left_down: false,
                left_clicked: false,
                left_released: false,
                up_edge:    false,
                down_edge:  false,
                left_edge:  false,
                right_edge: false,
                enter_edge: false,
                escape_edge: false,
            },
            geom: Vec::with_capacity(32_768),
            text: Vec::with_capacity(8_192),
            pointer_captured: false,
        }
    }

    /// Begin a frame: snapshot input state, recompute the
    /// layout from the current framebuffer extent, and clear
    /// scratch buffers. Called once per frame before any
    /// widget call.
    ///
    /// `dt` drives the hover-ease tween. `mouse` and `input`
    /// come straight from the platform layer; `cw` and `ch`
    /// are the framebuffer dimensions in pixels.
    ///
    /// Order of operations is important: hover ease runs
    /// first, against the targets recorded by widgets during
    /// the previous frame. Only then does the target table
    /// clear, so widgets visited during the upcoming frame
    /// can record fresh targets that the next frame's ease
    /// step will pick up. Doing it the other way around
    /// (clear before ease) would zero every target before the
    /// blend ran and pin every hover value at zero forever.
    pub fn begin_frame(
        &mut self,
        dt: f32,
        mouse: Mouse,
        input: Input,
        cw: u32,
        ch: u32,
    ) {
        self.time += dt;
        self.layout = Layout::from_framebuffer(cw, ch);

        // Convert pixel pointer position to game space.
        // Same conversion the renderer uses, kept here so
        // widgets can compare pointer to widget rects without
        // a per-call helper.
        let (sx, sy) = crate::renderer::aspect_scale(cw, ch);
        let cwf = cw.max(1) as f32;
        let chf = ch.max(1) as f32;
        let nx = ((mouse.x as f32 / cwf) * 2.0 - 1.0) / sx;
        let ny = ((mouse.y as f32 / chf) * 2.0 - 1.0) / sy;

        let left_clicked  =  mouse.left_down && !self.prev_left_down;
        let left_released = !mouse.left_down &&  self.prev_left_down;

        self.input = UiInput {
            pointer:       (nx, ny),
            left_down:     mouse.left_down,
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
        self.prev_up    = input.up;
        self.prev_down  = input.down;
        self.prev_left  = input.left;
        self.prev_right = input.right;
        self.prev_enter = input.enter;
        self.prev_escape = input.escape;

        if left_released { self.dragging = None; }
        self.pointer_captured = false;
        self.geom.clear();
        self.text.clear();

        // Step 1: ease every accumulated hover value toward
        // the target the widget recorded during the previous
        // frame. The ease blend follows the standard
        // exponential lag pattern; values that converge to
        // zero are dropped so the table does not grow without
        // bound when widgets disappear.
        let blend = 1.0 - (-14.0_f32 * dt).exp();
        let keys: Vec<WidgetId> = self.hover.keys().copied().collect();
        for id in keys {
            let target = if *self.hover_target.get(&id).unwrap_or(&false) {
                1.0
            } else {
                0.0
            };
            let v = self.hover.get_mut(&id).unwrap();
            *v += (target - *v) * blend;
            if *v < 0.001 && target == 0.0 {
                self.hover.remove(&id);
            }
        }

        // Step 2: clear targets for the new frame. Every
        // widget that wants to keep its hover state alive
        // must call `track_hover` (or one of the built in
        // widget methods) during the upcoming frame to put
        // its target back.
        self.hover_target.clear();
    }

    /// Append accumulated geometry to the caller-provided
    /// shape and text buffers. Both buffers are extended in
    /// place; the Ui's internal scratch is preserved so a
    /// caller could in principle inspect or replay it,
    /// although in normal use it is cleared on the next
    /// `begin_frame`.
    pub fn end_frame(
        &mut self,
        shapes: &mut Vec<Vertex>,
        text: &mut Vec<TextVertex>,
    ) {
        shapes.extend_from_slice(&self.geom);
        text.extend_from_slice(&self.text);
    }

    /// Direct mutable access to the shape geometry buffer.
    /// Use this for custom decorations that no built-in
    /// widget provides (animated rings, pulse effects,
    /// trapezoids etc.).
    pub fn geom_mut(&mut self) -> &mut Vec<Vertex> { &mut self.geom }

    /// Direct mutable access to the text geometry buffer.
    /// Use this when calling `font::push_text_alpha` and
    /// similar helpers that take an explicit vec target.
    pub fn text_mut(&mut self) -> &mut Vec<TextVertex> { &mut self.text }

    /// Read-only view on this frame's input snapshot.
    pub fn input(&self) -> UiInput { self.input }

    /// Continuous time accumulator since UI startup. Useful
    /// for time-driven animations (pulsing accents, slow
    /// rotations) that want a stable phase across frames.
    pub fn time(&self) -> f32 { self.time }

    /// True when the given widget owns the active drag.
    pub fn is_dragging(&self, id: WidgetId) -> bool {
        self.dragging == Some(id)
    }

    /// Set the active drag owner. Cleared automatically on
    /// the frame the left mouse button is released.
    pub fn set_dragging(&mut self, id: WidgetId) {
        self.dragging = Some(id);
    }

    /// Read the active drag owner, if any. Custom widgets
    /// inspect this to decide whether they should branch
    /// into "drag in progress" handling versus "fresh
    /// click" handling.
    pub fn dragging_id(&self) -> Option<WidgetId> { self.dragging }

    /// True if any widget consumed the pointer this frame.
    /// Outer code (custom hit-testers) should respect this
    /// flag and skip their own click handling when set.
    pub fn pointer_captured(&self) -> bool {
        self.pointer_captured
    }

    /// Mark the pointer as consumed for the current frame.
    /// Custom widget code calls this after handling a click
    /// it considers significant, mirroring what the built in
    /// widgets do internally. Has no immediate visible
    /// effect; downstream code that respects
    /// `pointer_captured()` is the consumer.
    pub fn capture_pointer(&mut self) {
        self.pointer_captured = true;
    }

    fn set_hover(&mut self, id: WidgetId, v: bool) {
        if v {
            self.hover_target.insert(id, true);
        }
        if !self.hover.contains_key(&id) && v {
            self.hover.insert(id, 0.0);
        }
    }

    fn hover_eased(&self, id: WidgetId) -> f32 {
        let t = self.hover.get(&id).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        // Smoothstep curve so the visual transition is soft
        // at both endpoints rather than a linear crossfade.
        t * t * (3.0 - 2.0 * t)
    }

    /// Public hover tracker for custom widgets.
    ///
    /// Records `hovered` as the widget's hover target for
    /// the current frame and returns the eased value the
    /// previous frame's blend produced. Callers use the
    /// return value to drive their own animations: ring
    /// brightness, slide offset, scale pop, anything that
    /// looks better with a smoothed transition than with a
    /// hard on/off step.
    pub fn track_hover(&mut self, id: WidgetId, hovered: bool) -> f32 {
        self.set_hover(id, hovered);
        self.hover_eased(id)
    }

    /// Convert a length expressed in design pixels to game
    /// space units. Shorthand for `self.layout.px(v)`.
    pub fn px(&self, v: f32) -> f32 { self.layout.px(v) }

    /// Em height in game-space units for a given
    /// design-pixel font size. Pass the result to
    /// `font::push_text` and friends.
    pub fn em(&self, design_px: f32) -> f32 {
        self.layout.px(design_px)
    }

    /// Width of `s` in game-space units at the given font
    /// size (in design pixels). Useful for centring labels.
    pub fn text_width(&self, s: &str, design_px: f32) -> f32 {
        font::text_width(s, self.layout.px(design_px))
    }

    /// Height of one line in game-space units.
    pub fn text_height(&self, design_px: f32) -> f32 {
        font::text_height(self.layout.px(design_px))
    }

    // ===== widget primitives =====

    /// Filled rectangular button with centred label.
    /// Returns true on the frame the user clicks the button.
    /// Hover state is tracked across frames keyed by `id`.
    pub fn button(
        &mut self,
        id: WidgetId,
        frame: Frame,
        label: &str,
    ) -> bool {
        self.button_rect(id, frame.to_rect(&self.layout), label)
    }

    /// Rect-targeted variant of `button`. Use when the
    /// caller has already resolved the position from a
    /// parent rect via `Rect::split_x` / `split_y`.
    pub fn button_rect(
        &mut self,
        id: WidgetId,
        rect: Rect,
        label: &str,
    ) -> bool {
        let (px, py) = self.input.pointer;
        let hovered = rect.contains(px, py);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);
        let theme = self.theme;

        let bg   = mix3(theme.bg_panel, theme.bg_panel_hi, h);
        let ring = mix3(theme.accent, theme.accent_hi, h);
        let outline_thick = self.layout.px(2.0)
                          + self.layout.px(1.0) * h;

        push_quad(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1, bg);
        push_outline(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1,
            outline_thick, ring);

        let (cx, cy) = rect.center();
        let em = self.em(theme.font_button);
        let w = font::text_width(label, em);
        let h_text = font::text_height(em);
        font::push_text(&mut self.text, label,
            cx - w * 0.5, cy - h_text * 0.5,
            em, theme.white);

        let clicked = hovered && self.input.left_clicked;
        if clicked { self.pointer_captured = true; }
        clicked
    }

    /// Compact button for toolbar-style action rows. Smaller
    /// outline, body-size label, otherwise identical to
    /// `button`.
    pub fn button_small(
        &mut self,
        id: WidgetId,
        frame: Frame,
        label: &str,
    ) -> bool {
        self.button_small_rect(id, frame.to_rect(&self.layout), label)
    }

    pub fn button_small_rect(
        &mut self,
        id: WidgetId,
        rect: Rect,
        label: &str,
    ) -> bool {
        let (px, py) = self.input.pointer;
        let hovered = rect.contains(px, py);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);
        let theme = self.theme;

        let bg   = mix3(theme.bg_panel, theme.bg_panel_hi, h);
        let ring = mix3(theme.accent, theme.accent_hi, h);
        let outline_thick = self.layout.px(1.5)
                          + self.layout.px(0.5) * h;

        push_quad(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1, bg);
        push_outline(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1,
            outline_thick, ring);

        let (cx, cy) = rect.center();
        let em = self.em(theme.font_body);
        let w = font::text_width(label, em);
        let h_text = font::text_height(em);
        font::push_text(&mut self.text, label,
            cx - w * 0.5, cy - h_text * 0.5,
            em, theme.white);

        let clicked = hovered && self.input.left_clicked;
        if clicked { self.pointer_captured = true; }
        clicked
    }

    /// Numeric slider over `[min, max]`. Returns true if
    /// the value changed this frame. The label is rendered
    /// on the left, the track in the middle, and the
    /// numeric value readout on the right.
    pub fn slider_f32(
        &mut self,
        id: WidgetId,
        frame: Frame,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
    ) -> bool {
        self.slider_f32_rect(
            id, frame.to_rect(&self.layout),
            label, value, min, max)
    }

    pub fn slider_f32_rect(
        &mut self,
        id: WidgetId,
        rect: Rect,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
    ) -> bool {
        let theme = self.theme;
        let (px, py) = self.input.pointer;

        // Layout: label on the left (35% of width), track in
        // the middle (50%), value readout on the right
        // (15%). Numbers are tuned so a typical slider with
        // a 4-character readout has comfortable spacing.
        let label_w = 0.35 * rect.width();
        let value_w = 0.15 * rect.width();
        let (_, cy) = rect.center();
        let track_h = self.layout.px(20.0);
        let track_rect = Rect::from_xywh(
            rect.x0 + label_w,
            cy - track_h * 0.5,
            rect.width() - label_w - value_w,
            track_h,
        );

        // Label.
        let em_body = self.em(theme.font_body);
        let h_text = font::text_height(em_body);
        font::push_text(&mut self.text, label,
            rect.x0, cy - h_text * 0.5,
            em_body, theme.white);

        // Track background and fill.
        let track_pad = self.layout.px(4.0);
        push_quad(&mut self.geom,
            track_rect.x0 - track_pad, track_rect.y0 - track_pad,
            track_rect.x1 + track_pad, track_rect.y1 + track_pad,
            theme.bg_deep);
        push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0,
            track_rect.x1, track_rect.y1,
            theme.bg_panel);

        let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
        let fx = track_rect.x0 + track_rect.width() * t;
        push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0,
            fx, track_rect.y1,
            theme.accent);

        // Hex handle.
        let hover_radius = self.layout.px(40.0);
        let handle_hit = (px - fx).abs() < hover_radius * 0.5
                      && (py - cy).abs() < rect.height() * 0.5;
        let track_hit = track_rect
            .inset_xy(-self.layout.px(10.0), 0.0)
            .contains(px, py);
        let hovered = handle_hit || self.dragging == Some(id);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);

        let ring = mix3(theme.accent, theme.accent_hi, h);
        let r = self.layout.px(24.0) + self.layout.px(5.0) * h;
        push_hex(&mut self.geom, fx, cy, r, theme.bg_deep);
        push_hex_ring(&mut self.geom,
            fx, cy, r, r - self.layout.px(6.0), ring);
        push_hex(&mut self.geom, fx, cy, r * 0.40, ring);

        let mut changed = false;
        if self.input.left_clicked && (handle_hit || track_hit) {
            self.dragging = Some(id);
            self.pointer_captured = true;
            let nt = ((px - track_rect.x0)
                   / track_rect.width()).clamp(0.0, 1.0);
            let newv = min + (max - min) * nt;
            if (newv - *value).abs() > 1e-6 {
                *value = newv;
                changed = true;
            }
        }
        if self.dragging == Some(id) && self.input.left_down {
            let nt = ((px - track_rect.x0)
                   / track_rect.width()).clamp(0.0, 1.0);
            let newv = min + (max - min) * nt;
            if (newv - *value).abs() > 1e-6 {
                *value = newv;
                changed = true;
            }
            self.pointer_captured = true;
        }

        // Value readout on the right.
        let vstr = format!("{:.2}", *value);
        let vw = font::text_width(&vstr, em_body);
        font::push_text(&mut self.text, &vstr,
            rect.x1 - vw, cy - h_text * 0.5,
            em_body, theme.accent_hi);

        changed
    }

    /// Percent slider over `[0, 1]`, displayed as integer
    /// percentage. Returns true if the value changed.
    pub fn slider_pct(
        &mut self,
        id: WidgetId,
        frame: Frame,
        label: &str,
        value: &mut f32,
    ) -> bool {
        self.slider_pct_rect(
            id, frame.to_rect(&self.layout), label, value)
    }

    pub fn slider_pct_rect(
        &mut self,
        id: WidgetId,
        rect: Rect,
        label: &str,
        value: &mut f32,
    ) -> bool {
        let theme = self.theme;
        let (px, py) = self.input.pointer;

        // Slightly different proportions than slider_f32:
        // a percentage readout is shorter, so we give the
        // label more room.
        let label_w = 0.40 * rect.width();
        let value_w = 0.12 * rect.width();
        let (_, cy) = rect.center();
        let track_h = self.layout.px(20.0);
        let track_rect = Rect::from_xywh(
            rect.x0 + label_w,
            cy - track_h * 0.5,
            rect.width() - label_w - value_w,
            track_h,
        );

        let em_body = self.em(theme.font_body);
        let h_text = font::text_height(em_body);
        font::push_text(&mut self.text, label,
            rect.x0, cy - h_text * 0.5,
            em_body, theme.white);

        let track_pad = self.layout.px(4.0);
        push_quad(&mut self.geom,
            track_rect.x0 - track_pad, track_rect.y0 - track_pad,
            track_rect.x1 + track_pad, track_rect.y1 + track_pad,
            theme.bg_deep);
        push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0,
            track_rect.x1, track_rect.y1,
            theme.bg_panel);

        let t = value.clamp(0.0, 1.0);
        let fx = track_rect.x0 + track_rect.width() * t;
        push_quad(&mut self.geom,
            track_rect.x0, track_rect.y0,
            fx, track_rect.y1, theme.accent);

        let hover_radius = self.layout.px(40.0);
        let handle_hit = (px - fx).abs() < hover_radius * 0.5
                      && (py - cy).abs() < rect.height() * 0.5;
        let track_hit = track_rect
            .inset_xy(-self.layout.px(10.0), 0.0)
            .contains(px, py);
        let hovered = handle_hit || self.dragging == Some(id);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);

        let ring = mix3(theme.accent, theme.accent_hi, h);
        let r = self.layout.px(24.0) + self.layout.px(5.0) * h;
        push_hex(&mut self.geom, fx, cy, r, theme.bg_deep);
        push_hex_ring(&mut self.geom,
            fx, cy, r, r - self.layout.px(6.0), ring);
        push_hex(&mut self.geom, fx, cy, r * 0.40, ring);

        let mut changed = false;
        if self.input.left_clicked && (handle_hit || track_hit) {
            self.dragging = Some(id);
            self.pointer_captured = true;
            let nt = ((px - track_rect.x0)
                   / track_rect.width()).clamp(0.0, 1.0);
            if (nt - *value).abs() > 1e-6 {
                *value = nt;
                changed = true;
            }
        }
        if self.dragging == Some(id) && self.input.left_down {
            let nt = ((px - track_rect.x0)
                   / track_rect.width()).clamp(0.0, 1.0);
            if (nt - *value).abs() > 1e-6 {
                *value = nt;
                changed = true;
            }
            self.pointer_captured = true;
        }

        let vstr = format!("{}%",
            (value.clamp(0.0, 1.0) * 100.0).round() as i32);
        let vw = font::text_width(&vstr, em_body);
        font::push_text(&mut self.text, &vstr,
            rect.x1 - vw, cy - h_text * 0.5,
            em_body, theme.accent_hi);

        changed
    }

    /// Boolean toggle widget. Label on the left, ON/OFF
    /// chip on the right. Returns true on the frame the
    /// state flips.
    pub fn toggle(
        &mut self,
        id: WidgetId,
        frame: Frame,
        label: &str,
        value: &mut bool,
    ) -> bool {
        self.toggle_rect(id, frame.to_rect(&self.layout), label, value)
    }

    pub fn toggle_rect(
        &mut self,
        id: WidgetId,
        rect: Rect,
        label: &str,
        value: &mut bool,
    ) -> bool {
        let theme = self.theme;
        let (_, cy) = rect.center();
        let em_body = self.em(theme.font_body);
        let h_text = font::text_height(em_body);
        font::push_text(&mut self.text, label,
            rect.x0, cy - h_text * 0.5,
            em_body, theme.white);

        let btn_w = self.layout.px(140.0);
        let btn_pad = self.layout.px(8.0);
        let btn_rect = Rect::from_xywh(
            rect.x1 - btn_w,
            rect.y0 + btn_pad,
            btn_w,
            rect.height() - 2.0 * btn_pad,
        );

        let (px, py) = self.input.pointer;
        let hovered = btn_rect.contains(px, py);
        self.set_hover(id, hovered);
        let h = self.hover_eased(id);

        let bg = if *value {
            mix3(theme.accent, theme.accent_hi, h)
        } else {
            mix3(theme.bg_panel, theme.bg_panel_hi, h)
        };
        let ring = mix3(theme.accent, theme.accent_hi, h);

        push_quad(&mut self.geom,
            btn_rect.x0, btn_rect.y0,
            btn_rect.x1, btn_rect.y1, bg);
        push_outline(&mut self.geom,
            btn_rect.x0, btn_rect.y0,
            btn_rect.x1, btn_rect.y1,
            self.layout.px(1.5), ring);

        let text = if *value { "ON" } else { "OFF" };
        let tw = font::text_width(text, em_body);
        let (bcx, bcy) = btn_rect.center();
        let text_color = if *value { theme.bg_deep } else { theme.dim };
        font::push_text(&mut self.text, text,
            bcx - tw * 0.5, bcy - h_text * 0.5,
            em_body, text_color);

        let mut changed = false;
        if hovered && self.input.left_clicked {
            *value = !*value;
            changed = true;
            self.pointer_captured = true;
        }
        changed
    }

    /// Enum cycler with left and right arrow buttons.
    /// `variants` is a slice of `(value, display_name)`
    /// pairs; the cycler advances through them in order.
    /// Returns true if the selection changed this frame.
    pub fn cycle<T: Copy + PartialEq>(
        &mut self,
        id: WidgetId,
        frame: Frame,
        label: &str,
        value: &mut T,
        variants: &[(T, &str)],
    ) -> bool {
        self.cycle_rect(
            id, frame.to_rect(&self.layout),
            label, value, variants)
    }

    pub fn cycle_rect<T: Copy + PartialEq>(
        &mut self,
        id: WidgetId,
        rect: Rect,
        label: &str,
        value: &mut T,
        variants: &[(T, &str)],
    ) -> bool {
        let theme = self.theme;
        let (_, cy) = rect.center();
        let em_body = self.em(theme.font_body);
        let h_text = font::text_height(em_body);
        font::push_text(&mut self.text, label,
            rect.x0, cy - h_text * 0.5,
            em_body, theme.white);

        // Right side of the row holds the cycler. Layout:
        // [arrow] [value (centred)] [arrow]
        let arrow_w = self.layout.px(50.0);
        let row_x0 = rect.x0 + 0.40 * rect.width();
        let row_pad = self.layout.px(8.0);
        let inner = Rect {
            x0: row_x0,
            y0: rect.y0 + row_pad,
            x1: rect.x1,
            y1: rect.y1 - row_pad,
        };
        let left_rect = Rect::from_xywh(
            inner.x0, inner.y0, arrow_w, inner.height());
        let right_rect = Rect::from_xywh(
            inner.x1 - arrow_w, inner.y0, arrow_w, inner.height());
        let center_rect = Rect {
            x0: left_rect.x1, y0: inner.y0,
            x1: right_rect.x0, y1: inner.y1,
        };

        let (px, py) = self.input.pointer;
        let left_hit = left_rect.contains(px, py);
        let right_hit = right_rect.contains(px, py);
        let id_left = id.wrapping_add(1);
        let id_right = id.wrapping_add(2);
        self.set_hover(id_left, left_hit);
        self.set_hover(id_right, right_hit);

        let lh = self.hover_eased(id_left);
        let rh = self.hover_eased(id_right);
        let lc = mix3(theme.accent, theme.accent_hi, lh);
        let rc = mix3(theme.accent, theme.accent_hi, rh);

        // Arrow triangles. The hover ease widens the apex
        // slightly so the arrow visibly "leans" toward the
        // cursor when hovered.
        let arrow_size = self.layout.px(20.0);
        let arrow_lean = self.layout.px(6.0);
        let (lcx, lcy) = left_rect.center();
        let (rcx, rcy) = right_rect.center();
        push_tri(&mut self.geom,
            [lcx - arrow_size - arrow_lean * lh, lcy],
            [lcx + self.layout.px(14.0), lcy - arrow_size],
            [lcx + self.layout.px(14.0), lcy + arrow_size],
            lc);
        push_tri(&mut self.geom,
            [rcx + arrow_size + arrow_lean * rh, rcy],
            [rcx - self.layout.px(14.0), rcy - arrow_size],
            [rcx - self.layout.px(14.0), rcy + arrow_size],
            rc);

        let cur_idx = variants.iter()
            .position(|(v, _)| *v == *value)
            .unwrap_or(0);
        let name = variants.get(cur_idx)
            .map(|(_, n)| *n)
            .unwrap_or("");
        let (ccx, ccy) = center_rect.center();
        let w = font::text_width(name, em_body);
        font::push_text(&mut self.text, name,
            ccx - w * 0.5, ccy - h_text * 0.5,
            em_body, theme.accent_hi);

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

    /// Horizontal tab bar. `tabs` is a list of labels;
    /// `current` is updated when the user clicks a tab.
    /// Returns true when the active tab changes.
    pub fn tabs(
        &mut self,
        id: WidgetId,
        frame: Frame,
        tabs: &[&str],
        current: &mut usize,
    ) -> bool {
        self.tabs_rect(
            id, frame.to_rect(&self.layout), tabs, current)
    }

    pub fn tabs_rect(
        &mut self,
        id: WidgetId,
        rect: Rect,
        tabs: &[&str],
        current: &mut usize,
    ) -> bool {
        let theme = self.theme;
        let n = tabs.len().max(1);
        let gap = self.layout.px(6.0);
        let mut changed = false;
        let cells = rect.split_x(n, gap);

        for (i, label) in tabs.iter().enumerate() {
            let cell = cells[i];
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
            push_quad(&mut self.geom,
                cell.x0, cell.y0, cell.x1, cell.y1, bg);

            // Active tab gets an accent strip along the
            // bottom; inactive tabs get a thin outline that
            // brightens on hover.
            if active {
                let bar_h = self.layout.px(6.0);
                push_quad(&mut self.geom,
                    cell.x0, cell.y1 - bar_h,
                    cell.x1, cell.y1, theme.accent);
            } else {
                push_outline(&mut self.geom,
                    cell.x0, cell.y0, cell.x1, cell.y1,
                    self.layout.px(1.5),
                    mix3(theme.dim, theme.accent, h));
            }

            let (cx, cy) = cell.center();
            let em_btn = self.em(theme.font_button);
            let w = font::text_width(label, em_btn);
            let h_text = font::text_height(em_btn);
            let label_color = if active { theme.white } else { theme.dim };
            font::push_text(&mut self.text, label,
                cx - w * 0.5, cy - h_text * 0.5,
                em_btn, label_color);

            if hovered && self.input.left_clicked && !active {
                *current = i;
                changed = true;
                self.pointer_captured = true;
            }
        }
        if *current >= n { *current = 0; }
        changed
    }

    /// Static text label. `font_size` is in design pixels.
    /// The label is left-aligned within the frame and
    /// vertically centred.
    pub fn label(
        &mut self,
        frame: Frame,
        text: &str,
        font_size: f32,
        color: [f32; 3],
    ) {
        self.label_rect(frame.to_rect(&self.layout),
            text, font_size, color);
    }

    pub fn label_rect(
        &mut self,
        rect: Rect,
        text: &str,
        font_size: f32,
        color: [f32; 3],
    ) {
        let (_, cy) = rect.center();
        let em = self.em(font_size);
        let h_text = font::text_height(em);
        font::push_text(&mut self.text, text,
            rect.x0, cy - h_text * 0.5, em, color);
    }

    /// Centred static text label.
    pub fn label_centered(
        &mut self,
        frame: Frame,
        text: &str,
        font_size: f32,
        color: [f32; 3],
    ) {
        self.label_centered_rect(
            frame.to_rect(&self.layout),
            text, font_size, color);
    }

    pub fn label_centered_rect(
        &mut self,
        rect: Rect,
        text: &str,
        font_size: f32,
        color: [f32; 3],
    ) {
        let (cx, cy) = rect.center();
        let em = self.em(font_size);
        let w = font::text_width(text, em);
        let h_text = font::text_height(em);
        font::push_text(&mut self.text, text,
            cx - w * 0.5, cy - h_text * 0.5, em, color);
    }

    /// Right-aligned static text label.
    pub fn label_right(
        &mut self,
        frame: Frame,
        text: &str,
        font_size: f32,
        color: [f32; 3],
    ) {
        self.label_right_rect(
            frame.to_rect(&self.layout),
            text, font_size, color);
    }

    pub fn label_right_rect(
        &mut self,
        rect: Rect,
        text: &str,
        font_size: f32,
        color: [f32; 3],
    ) {
        let (_, cy) = rect.center();
        let em = self.em(font_size);
        let w = font::text_width(text, em);
        let h_text = font::text_height(em);
        font::push_text(&mut self.text, text,
            rect.x1 - w, cy - h_text * 0.5, em, color);
    }

    /// Filled panel with accent outline. Use as a container
    /// for grouped widgets; layout sub-widgets inside by
    /// resolving the same Frame to a Rect and splitting it.
    pub fn panel(&mut self, frame: Frame) {
        self.panel_rect(frame.to_rect(&self.layout));
    }

    pub fn panel_rect(&mut self, rect: Rect) {
        let theme = self.theme;
        push_quad(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1,
            mix3(theme.bg_deep, theme.bg_panel, 0.40));
        push_outline(&mut self.geom,
            rect.x0, rect.y0, rect.x1, rect.y1,
            self.layout.px(2.0), theme.accent);
    }

    /// Thin horizontal divider. `y` is the centre line in
    /// game-space units; `x0` and `x1` define the horizontal
    /// span. Use for separating sections inside a panel.
    pub fn horizontal_rule(
        &mut self,
        y: f32,
        x0: f32,
        x1: f32,
        color: [f32; 3],
    ) {
        let h = self.layout.px(2.0);
        push_quad(&mut self.geom,
            x0, y - h * 0.5, x1, y + h * 0.5, color);
    }
}

// ===== shared helpers =====

/// Linear interpolation between two RGB colours. `t` is
/// clamped to 0..1.
pub fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Smooth Hermite interpolation. Returns a value in 0..1
/// that eases at both endpoints.
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}