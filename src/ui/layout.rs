//! Design-space coordinate system for UI layout.
//!
//! The renderer works in "game space" where the smaller of
//! the two visible axes always spans `[-1, 1]` and the larger
//! extends symmetrically to whatever the current aspect ratio
//! demands. Wide monitors see more horizontal extent, portrait
//! monitors see more vertical extent. This is great for the
//! gameplay but inconvenient for UI layout: a button at game
//! coordinate `(0.5, -0.8)` lands at very different pixel
//! positions on different monitors and overlaps panels on
//! ultrawide aspects that the author never tested against.
//!
//! This module introduces an aspect-aware layer on top of
//! game space:
//!
//! * `Layout` is a per-frame snapshot of the visible viewport
//!   in game space, plus a "design pixel" scale factor. A
//!   design pixel is a fixed fraction (1/1080) of the smaller
//!   visible axis, so 1 design pixel always covers the same
//!   visual area regardless of aspect ratio. Authors can
//!   write layout code as though they were targeting a 1080p
//!   reference monitor, and it scales to any other window
//!   size through this single multiplier.
//!
//! * `Anchor` identifies one of nine reference points in the
//!   viewport: the four corners, the four edge midpoints, and
//!   the center. Each top-level UI element sticks to one of
//!   these and expresses its own offset relative to the
//!   anchor in design pixels. A button anchored to
//!   `BottomRight` always sits the same distance away from
//!   the bottom-right corner of the visible area, regardless
//!   of how wide or tall the window happens to be.
//!
//! * `Frame` is an `(anchor, offset, size)` triple, all
//!   measured in design pixels. `Frame::to_rect` resolves the
//!   frame to a `Rect` in game space using the current
//!   `Layout`. Frames are the recommended top-level entry
//!   point: declare them once at the call site, resolve once
//!   per frame.
//!
//! * `Rect` is the game-space rectangle widgets actually
//!   draw against. It supports common subdivisions (split
//!   into columns, rows, or fixed-height bands) used to
//!   build nested widget hierarchies inside a panel. Rect
//!   operations work in game-space units; use
//!   `Layout::px(N)` whenever a gap or padding needs to be
//!   expressed in design pixels.
//!
//! # Anchor offset convention
//!
//! For edge and corner anchors, offsets are interpreted as
//! "inward" distances from the corresponding edges:
//!
//! * `Frame::at(Anchor::TopLeft, 20.0, 30.0, w, h)` places
//!   the frame 20 design pixels right of the left edge and
//!   30 design pixels below the top edge.
//! * `Frame::at(Anchor::BottomRight, 20.0, 30.0, w, h)`
//!   places the frame 20 design pixels left of the right
//!   edge and 30 design pixels above the bottom edge.
//!
//! For `Center` and the four edge-midpoint anchors, offsets
//! follow the standard screen-space convention (right is
//! positive X, down is positive Y) and shift the frame
//! relative to the anchor point. So
//! `Frame::at(Anchor::Center, 0.0, -50.0, w, h)` places a
//! frame 50 design pixels above the center, the natural
//! position for a header.
//!
//! Negative offsets are accepted and produce frames that
//! hang off the visible viewport. This is occasionally
//! useful for slide-in animations: animate offset from
//! `-200.0` to `0.0` to slide a panel in from outside the
//! left edge.

/// Reference design-pixel resolution along the smaller of
/// the two visible axes. Choosing 1080 means a 16:9
/// landscape monitor sees a 1920x1080 design canvas; an
/// ultrawide monitor sees the same 1080 vertical pixels with
/// extra horizontal extent; a portrait monitor sees 1080
/// horizontal pixels with extra vertical extent.
///
/// Bumping this number does not break any existing layout
/// code: positions and sizes are interpreted relative to
/// the chosen reference, so doubling it would just halve
/// the visual size of every UI element. The current value
/// gives a comfortable balance of resolution headroom and
/// integer-friendly arithmetic for hand-tuned offsets.
pub const DESIGN_REF: f32 = 1080.0;

/// One of nine anchor points in the visible viewport.
///
/// The variants are arranged in the natural keypad order so
/// they read like a 3x3 grid:
///
/// ```text
/// TopLeft     Top         TopRight
/// Left        Center      Right
/// BottomLeft  Bottom      BottomRight
/// ```
///
/// See the module documentation for the offset
/// interpretation of each anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

/// Per-frame view geometry. Built once at the start of each
/// frame from the framebuffer extent and consulted by every
/// frame -> rect resolution call afterwards.
///
/// All fields are public because callers occasionally need
/// to compute custom positions outside the standard
/// anchor-and-offset framework, for example animated
/// background gradients that span the entire visible area.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// Aspect ratio of the framebuffer (width / height).
    pub aspect:      f32,
    /// Game-space coordinate of the visible left edge.
    pub view_left:   f32,
    /// Game-space coordinate of the visible right edge.
    pub view_right:  f32,
    /// Game-space coordinate of the visible top edge.
    pub view_top:    f32,
    /// Game-space coordinate of the visible bottom edge.
    pub view_bottom: f32,
    /// Game-space units per design pixel. Multiply a length
    /// in design pixels by this scalar to obtain a length in
    /// game space.
    pub scale:       f32,
}

impl Layout {
    /// Build a Layout from raw framebuffer pixel dimensions.
    ///
    /// Both inputs are clamped to a minimum of one pixel so
    /// the aspect ratio computation never divides by zero
    /// during the brief moments after a window minimize when
    /// the swapchain may report a degenerate extent.
    pub fn from_framebuffer(cw: u32, ch: u32) -> Self {
        let aspect = (cw.max(1) as f32) / (ch.max(1) as f32);
        let (view_left, view_right, view_top, view_bottom) =
            if aspect >= 1.0 {
                (-aspect, aspect, -1.0, 1.0)
            } else {
                let inv = 1.0 / aspect;
                (-1.0, 1.0, -inv, inv)
            };
        Layout {
            aspect,
            view_left,
            view_right,
            view_top,
            view_bottom,
            scale: 2.0 / DESIGN_REF,
        }
    }

    /// Convert a length expressed in design pixels to game
    /// space units. Use this for gaps, paddings, and inset
    /// distances inside Rect operations.
    pub fn px(&self, design_px: f32) -> f32 {
        design_px * self.scale
    }

    /// Locate one of the nine anchor points in game space.
    ///
    /// Used internally by `Frame::to_rect` and exposed in
    /// case caller code needs to position custom geometry
    /// (background art, animated decorations) at one of the
    /// standard reference points without going through a
    /// full Frame.
    pub fn anchor_point(&self, anchor: Anchor) -> [f32; 2] {
        let cx = (self.view_left + self.view_right) * 0.5;
        let cy = (self.view_top + self.view_bottom) * 0.5;
        match anchor {
            Anchor::TopLeft     => [self.view_left,  self.view_top   ],
            Anchor::Top         => [cx,              self.view_top   ],
            Anchor::TopRight    => [self.view_right, self.view_top   ],
            Anchor::Left        => [self.view_left,  cy              ],
            Anchor::Center      => [cx,              cy              ],
            Anchor::Right       => [self.view_right, cy              ],
            Anchor::BottomLeft  => [self.view_left,  self.view_bottom],
            Anchor::Bottom      => [cx,              self.view_bottom],
            Anchor::BottomRight => [self.view_right, self.view_bottom],
        }
    }

    /// Width of the visible viewport in design pixels.
    /// Convenience helper for centred panels whose width is
    /// expressed as a fraction of the screen.
    pub fn view_width_px(&self) -> f32 {
        (self.view_right - self.view_left) / self.scale
    }

    /// Height of the visible viewport in design pixels.
    pub fn view_height_px(&self) -> f32 {
        (self.view_bottom - self.view_top) / self.scale
    }

    /// Build a `Rect` anchored to one of the nine compass
    /// points, with offset and size expressed directly in
    /// game-space units (the same coordinate system the
    /// renderer's vertex shader consumes after its aspect
    /// correction step).
    ///
    /// This is the escape hatch for layout code that already
    /// has hand-tuned game-space coordinates from a previous
    /// revision and wants to anchor them to an edge without
    /// going through the design-pixel conversion. Behaves
    /// identically to `Frame::to_rect` except that `ox`,
    /// `oy`, `w`, `h` are NOT multiplied by `Layout::scale`
    /// before being applied. New layout code should prefer
    /// `Frame::at` with design-pixel sizes; this helper
    /// exists so an aspect-aware anchor system can be
    /// retrofitted onto existing screens without a wholesale
    /// re-tuning of every magic constant.
    ///
    /// Offset interpretation matches `Frame::to_rect`:
    /// corner / edge anchors treat positive offsets as
    /// "inward" distances; center and edge midpoints follow
    /// the standard right / down sign convention.
    pub fn anchor_rect_ndc(
        &self, anchor: Anchor,
        ox: f32, oy: f32, w: f32, h: f32,
    ) -> Rect {
        let ap = self.anchor_point(anchor);
        let (x0, y0) = match anchor {
            Anchor::TopLeft     => (ap[0] + ox,           ap[1] + oy),
            Anchor::Top         => (ap[0] + ox - w * 0.5, ap[1] + oy),
            Anchor::TopRight    => (ap[0] - ox - w,       ap[1] + oy),
            Anchor::Left        => (ap[0] + ox,           ap[1] + oy - h * 0.5),
            Anchor::Center      => (ap[0] + ox - w * 0.5, ap[1] + oy - h * 0.5),
            Anchor::Right       => (ap[0] - ox - w,       ap[1] + oy - h * 0.5),
            Anchor::BottomLeft  => (ap[0] + ox,           ap[1] - oy - h),
            Anchor::Bottom      => (ap[0] + ox - w * 0.5, ap[1] - oy - h),
            Anchor::BottomRight => (ap[0] - ox - w,       ap[1] - oy - h),
        };
        Rect { x0, y0, x1: x0 + w, y1: y0 + h }
    }
}

/// Anchored rectangle described in design pixels.
///
/// A Frame is a declarative position descriptor: it says
/// "this rectangle is anchored to the BottomRight corner,
/// 20 design pixels in from each edge, 200 by 80 design
/// pixels in size". `Frame::to_rect` resolves it to a
/// concrete game-space rectangle using the current
/// `Layout`.
///
/// Frames are cheap value types and Copy, so callers can
/// freely pass them through helper functions or store them
/// in const-friendly tables when their layout is fully
/// static.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub anchor: Anchor,
    pub offset: [f32; 2],
    pub size:   [f32; 2],
}

impl Frame {
    /// Generic constructor with arrays for offset and size.
    pub fn new(anchor: Anchor, offset: [f32; 2], size: [f32; 2]) -> Self {
        Frame { anchor, offset, size }
    }

    /// Convenience constructor with spread-out scalar fields.
    pub fn at(anchor: Anchor, x: f32, y: f32, w: f32, h: f32) -> Self {
        Frame::new(anchor, [x, y], [w, h])
    }

    /// Resolve this frame to a game-space Rect using the
    /// supplied layout. The exact corner of the frame that
    /// pins to the anchor point depends on the anchor; see
    /// the module documentation for the convention.
    pub fn to_rect(&self, layout: &Layout) -> Rect {
        let ap = layout.anchor_point(self.anchor);
        let ox = self.offset[0] * layout.scale;
        let oy = self.offset[1] * layout.scale;
        let sw = self.size[0]   * layout.scale;
        let sh = self.size[1]   * layout.scale;

        let (x0, y0) = match self.anchor {
            Anchor::TopLeft     => (ap[0] + ox,
                                    ap[1] + oy),
            Anchor::Top         => (ap[0] + ox - sw * 0.5,
                                    ap[1] + oy),
            Anchor::TopRight    => (ap[0] - ox - sw,
                                    ap[1] + oy),
            Anchor::Left        => (ap[0] + ox,
                                    ap[1] + oy - sh * 0.5),
            Anchor::Center      => (ap[0] + ox - sw * 0.5,
                                    ap[1] + oy - sh * 0.5),
            Anchor::Right       => (ap[0] - ox - sw,
                                    ap[1] + oy - sh * 0.5),
            Anchor::BottomLeft  => (ap[0] + ox,
                                    ap[1] - oy - sh),
            Anchor::Bottom      => (ap[0] + ox - sw * 0.5,
                                    ap[1] - oy - sh),
            Anchor::BottomRight => (ap[0] - ox - sw,
                                    ap[1] - oy - sh),
        };
        Rect { x0, y0, x1: x0 + sw, y1: y0 + sh }
    }
}

/// Game-space axis-aligned rectangle.
///
/// Working unit for widget layout once a Frame has been
/// resolved. All operations on Rect work in game-space
/// units; convert design pixels with `Layout::px(N)` when
/// passing gaps or insets to these methods.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// Construct from a top-left corner plus width and height.
    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x0: x, y0: y, x1: x + w, y1: y + h }
    }

    /// Construct from a center point plus half-extents.
    pub fn from_center(cx: f32, cy: f32, hw: f32, hh: f32) -> Self {
        Rect { x0: cx - hw, y0: cy - hh, x1: cx + hw, y1: cy + hh }
    }

    /// Geometric center of the rectangle.
    pub fn center(self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    pub fn width(self) -> f32 { self.x1 - self.x0 }
    pub fn height(self) -> f32 { self.y1 - self.y0 }

    /// Point-in-rect test. Inclusive on every edge so a
    /// click that lands exactly on the border still
    /// registers.
    pub fn contains(self, px: f32, py: f32) -> bool {
        px >= self.x0 && px <= self.x1
            && py >= self.y0 && py <= self.y1
    }

    /// Shrink by `m` game units on every side. Negative `m`
    /// expands the rect.
    pub fn inset(self, m: f32) -> Self {
        Rect {
            x0: self.x0 + m, y0: self.y0 + m,
            x1: self.x1 - m, y1: self.y1 - m,
        }
    }

    /// Shrink by separate horizontal and vertical insets.
    /// Useful when the visual outer padding of a panel
    /// differs along the two axes.
    pub fn inset_xy(self, mx: f32, my: f32) -> Self {
        Rect {
            x0: self.x0 + mx, y0: self.y0 + my,
            x1: self.x1 - mx, y1: self.y1 - my,
        }
    }

    /// Split into `n` equal columns separated by `gap` game
    /// units. Returns `n` rects ordered left to right. An
    /// empty input (n == 0) returns an empty vector.
    pub fn split_x(self, n: usize, gap: f32) -> Vec<Rect> {
        if n == 0 { return Vec::new(); }
        let total_gap = gap * (n.saturating_sub(1)) as f32;
        let cell = (self.width() - total_gap) / n as f32;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let x0 = self.x0 + (cell + gap) * i as f32;
            out.push(Rect {
                x0, y0: self.y0,
                x1: x0 + cell, y1: self.y1,
            });
        }
        out
    }

    /// Split into `n` equal rows separated by `gap` game
    /// units. Returns `n` rects ordered top to bottom.
    pub fn split_y(self, n: usize, gap: f32) -> Vec<Rect> {
        if n == 0 { return Vec::new(); }
        let total_gap = gap * (n.saturating_sub(1)) as f32;
        let cell = (self.height() - total_gap) / n as f32;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let y0 = self.y0 + (cell + gap) * i as f32;
            out.push(Rect {
                x0: self.x0, y0,
                x1: self.x1, y1: y0 + cell,
            });
        }
        out
    }

    /// Take a top slice of fixed height `h`. Returns
    /// `(top, rest)` where `top` has height `h` (clamped to
    /// the original height) and `rest` covers the remainder
    /// below it.
    pub fn split_top(self, h: f32) -> (Rect, Rect) {
        let mid = (self.y0 + h).min(self.y1);
        (Rect { x0: self.x0, y0: self.y0, x1: self.x1, y1: mid     },
         Rect { x0: self.x0, y0: mid,     x1: self.x1, y1: self.y1 })
    }

    /// Take a bottom slice of fixed height `h`. Returns
    /// `(rest, bottom)`.
    pub fn split_bottom(self, h: f32) -> (Rect, Rect) {
        let mid = (self.y1 - h).max(self.y0);
        (Rect { x0: self.x0, y0: self.y0, x1: self.x1, y1: mid     },
         Rect { x0: self.x0, y0: mid,     x1: self.x1, y1: self.y1 })
    }

    /// Take a left slice of fixed width `w`. Returns
    /// `(left, rest)`.
    pub fn split_left(self, w: f32) -> (Rect, Rect) {
        let mid = (self.x0 + w).min(self.x1);
        (Rect { x0: self.x0, y0: self.y0, x1: mid,     y1: self.y1 },
         Rect { x0: mid,     y0: self.y0, x1: self.x1, y1: self.y1 })
    }

    /// Take a right slice of fixed width `w`. Returns
    /// `(rest, right)`.
    pub fn split_right(self, w: f32) -> (Rect, Rect) {
        let mid = (self.x1 - w).max(self.x0);
        (Rect { x0: self.x0, y0: self.y0, x1: mid,     y1: self.y1 },
         Rect { x0: mid,     y0: self.y0, x1: self.x1, y1: self.y1 })
    }

    /// Linearly interpolate two rects component-wise. `t`
    /// is clamped to 0..1. Useful for animating between
    /// two layout states (sliding panels, folding accordions).
    pub fn lerp(a: Rect, b: Rect, t: f32) -> Rect {
        let t = t.clamp(0.0, 1.0);
        Rect {
            x0: a.x0 + (b.x0 - a.x0) * t,
            y0: a.y0 + (b.y0 - a.y0) * t,
            x1: a.x1 + (b.x1 - a.x1) * t,
            y1: a.y1 + (b.y1 - a.y1) * t,
        }
    }
}