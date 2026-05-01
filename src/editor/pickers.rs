//! Modal file pickers: import existing levels, pick a music
//! track.
//!
//! A `Picker` is a scrollable list overlay. It intercepts every
//! input event while active, the same way a `Dialog` does, but
//! is laid out differently: bigger panel, scrollable body, click
//! an entry to accept and close. The editor shell holds an
//! `Option<Picker>` slot parallel to the `Option<Dialog>` one;
//! only one of the two may be active at a time.
//!
//! # Entry sources
//!
//! * [`scan_levels`] enumerates `.rlf` files in
//!   `assets/customlevels/` and `assets/levels/`, tagging each
//!   as `[CUSTOM]` or `[STOCK]`.
//! * [`scan_tracks`] enumerates `.qoa` files in
//!   `assets/customlevels/songs/` and `assets/music/`, tagging
//!   each the same way.
//!
//! Both functions probe the directory relative to the current
//! working dir first (what `cargo run` sees) and then relative
//! to the executable's parent (what a packaged launch sees), so
//! authoring works from both locations without reconfiguration.

use std::path::PathBuf;

use crate::pipeline::Vertex;
use crate::text::{push_text_centered, text_height as th, text_width as tw};
use crate::text_small::{
    push_small, push_small_centered, push_small_right,
    text_height as sh, text_width as sw,
};
use crate::ui::draw::{push_outline, push_quad, push_quad_alpha};
use crate::win32::Mouse;

type Rect = (f32, f32, f32, f32);

const ACCENT:     [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:  [f32; 3] = [1.00, 0.80, 0.92];
const DIM:        [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:      [f32; 3] = [1.00, 1.00, 1.00];
const BG_PANEL:   [f32; 3] = [0.10, 0.04, 0.15];
const BG_PANEL_HI:[f32; 3] = [0.22, 0.10, 0.30];
const BG_DEEP:    [f32; 3] = [0.03, 0.01, 0.06];

/// One row in the picker list.
#[derive(Clone)]
pub struct PickerEntry {
    /// Short label shown on the left of the row. Usually the
    /// filename.
    pub display: String,
    /// Full relative path the caller uses after selection.
    pub path: String,
    /// Short bracketed tag shown on the right, e.g. `[STOCK]`
    /// or `[CUSTOM]`.
    pub folder_tag: &'static str,
}

/// Outcome of `Picker::update` for one frame.
pub enum PickerOutcome {
    Stay,
    Selected(String),
    Cancel,
}

pub struct Picker {
    pub title: String,
    pub entries: Vec<PickerEntry>,
    hint: String,
    scroll: f32,
    hovered: Option<usize>,
    prev_left_down: bool,
    prev_escape: bool,
}

impl Picker {
    /// Construct a picker with the given title, entries, and
    /// an optional one line hint rendered above the list.
    pub fn new(title: String, entries: Vec<PickerEntry>, hint: String) -> Self {
        Picker {
            title,
            entries,
            hint,
            scroll: 0.0,
            hovered: None,
            // Start with `prev_left_down = true` so the very
            // first frame after the picker opens absorbs the
            // mouse-button-still-held-from-opening-click event
            // without treating it as a real click on the
            // picker. Otherwise the first frame sees
            // `clicked = true` with pointer on whichever
            // toolbar button triggered the picker, decides
            // that click is "outside panel", and cancels
            // itself immediately.
            prev_left_down: true,
            prev_escape: false,
        }
    }

    pub fn update(
        &mut self, dt: f32,
        pointer: (f32, f32), mouse: Mouse, escape: bool,
        scroll_delta: i32,
    ) -> PickerOutcome {
        let _ = dt;
        let clicked = mouse.left_down && !self.prev_left_down;
        self.prev_left_down = mouse.left_down;
        let esc_edge = escape && !self.prev_escape;
        self.prev_escape = escape;

        if esc_edge { return PickerOutcome::Cancel; }

        let panel = self.panel_rect();
        let list_rect = self.list_rect();

        // Scroll via wheel when pointer is over the list.
        if rect_contains_pt(list_rect, pointer) && scroll_delta != 0 {
            let notches = scroll_delta as f32 / 120.0;
            let row_h = Self::row_height();
            self.scroll -= notches * row_h * 3.0;
            self.scroll = self.scroll.max(0.0);
            let max = self.max_scroll();
            if self.scroll > max { self.scroll = max; }
        }

        // Hover test over visible rows.
        self.hovered = None;
        for (i, r) in self.row_rects(list_rect).into_iter().enumerate() {
            if rect_contains_pt(r, pointer) {
                self.hovered = Some(i);
                break;
            }
        }

        if clicked {
            if let Some(i) = self.hovered {
                if i < self.entries.len() {
                    return PickerOutcome::Selected(
                        self.entries[i].path.clone());
                }
            }
            if !rect_contains_pt(panel, pointer) {
                return PickerOutcome::Cancel;
            }
            // Cancel button.
            if rect_contains_pt(self.cancel_btn_rect(), pointer) {
                return PickerOutcome::Cancel;
            }
        }

        PickerOutcome::Stay
    }

    pub fn draw(&self, out: &mut Vec<Vertex>,
                view_left: f32, view_right: f32,
                view_top: f32, view_bottom: f32)
    {
        push_quad_alpha(out, view_left, view_top,
            view_right, view_bottom,
            [0.0, 0.0, 0.0], 0.70);

        let panel = self.panel_rect();
        push_quad(out, panel.0, panel.1, panel.2, panel.3, BG_PANEL);
        push_outline(out, panel.0, panel.1, panel.2, panel.3,
            0.004, ACCENT);

        // Title bar.
        push_quad(out, panel.0, panel.1, panel.2, panel.1 + 0.050,
            BG_PANEL_HI);
        let cx = (panel.0 + panel.2) * 0.5;
        push_text_centered(out, &self.title, cx,
            panel.1 + 0.012, 0.0080, ACCENT_HI);

        // Hint just below the title.
        if !self.hint.is_empty() {
            push_small_centered(out, &self.hint, cx,
                panel.1 + 0.056, 0.0040, DIM);
        }

        // List area background.
        let list = self.list_rect();
        push_quad(out, list.0, list.1, list.2, list.3, BG_DEEP);

        // List rows.
        if self.entries.is_empty() {
            let mid = (list.1 + list.3) * 0.5;
            push_small_centered(out,
                "NO MATCHING FILES FOUND",
                (list.0 + list.2) * 0.5, mid - 0.010, 0.0050, DIM);
        } else {
            for (i, r) in self.row_rects(list).into_iter().enumerate() {
                // Clip fully offscreen rows.
                if r.3 < list.1 || r.1 > list.3 { continue; }
                if i >= self.entries.len() { break; }

                let entry = &self.entries[i];
                let is_hovered = self.hovered == Some(i);
                let bg = if is_hovered { BG_PANEL_HI } else { BG_PANEL };
                push_quad(out, r.0, r.1, r.2, r.3, bg);
                if is_hovered {
                    push_outline(out, r.0, r.1, r.2, r.3, 0.002, ACCENT_HI);
                }

                let max_name_w = r.2 - r.0 - 0.180;
                let name = fit_small(&entry.display.to_uppercase(),
                    0.0045, max_name_w);
                push_small(out, &name,
                    r.0 + 0.012,
                    (r.1 + r.3) * 0.5 - sh(0.0045) * 0.5,
                    0.0045, WHITE);
                push_small_right(out, entry.folder_tag,
                    r.2 - 0.012,
                    (r.1 + r.3) * 0.5 - sh(0.0040) * 0.5,
                    0.0040, DIM);
            }

            // Scroll indicator if there's more than one page.
            if self.max_scroll() > 0.0 {
                let t = (self.scroll / self.max_scroll()).clamp(0.0, 1.0);
                let track_x = list.2 - 0.008;
                push_quad(out,
                    track_x - 0.002, list.1,
                    track_x + 0.002, list.3, BG_PANEL);
                let h = list.3 - list.1;
                let visible_fraction = (h / (h + self.max_scroll()))
                    .clamp(0.05, 1.0);
                let thumb_h = h * visible_fraction;
                let thumb_y = list.1 + (h - thumb_h) * t;
                push_quad(out,
                    track_x - 0.003, thumb_y,
                    track_x + 0.003, thumb_y + thumb_h, ACCENT);
            }
        }

        // Cancel button.
        let c = self.cancel_btn_rect();
        push_quad(out, c.0, c.1, c.2, c.3, BG_PANEL);
        push_outline(out, c.0, c.1, c.2, c.3, 0.003, DIM);
        let ccx = (c.0 + c.2) * 0.5;
        let ccy = (c.1 + c.3) * 0.5;
        let label = "CANCEL  (ESC)";
        let lw = sw(label, 0.0045);
        push_small(out, label,
            ccx - lw * 0.5, ccy - sh(0.0045) * 0.5, 0.0045, WHITE);

        // Counter on the left side of the footer.
        let count = format!("{} ITEMS", self.entries.len());
        push_small(out, &count,
            panel.0 + 0.018,
            c.1 + (c.3 - c.1) * 0.5 - sh(0.0040) * 0.5,
            0.0040, DIM);
    }

    // ---- layout ----

    fn panel_rect(&self) -> Rect {
        (-1.10, -0.56, 1.10, 0.56)
    }
    fn list_rect(&self) -> Rect {
        let p = self.panel_rect();
        (p.0 + 0.018, p.1 + 0.080, p.2 - 0.018, p.3 - 0.060)
    }
    fn cancel_btn_rect(&self) -> Rect {
        let p = self.panel_rect();
        let y1 = p.3 - 0.014;
        let y0 = y1 - 0.030;
        (p.2 - 0.16, y0, p.2 - 0.018, y1)
    }
    fn row_height() -> f32 { 0.034 }

    fn max_scroll(&self) -> f32 {
        let list = self.list_rect();
        let h = list.3 - list.1;
        let total = self.entries.len() as f32 * Self::row_height();
        (total - h).max(0.0)
    }

    fn row_rects(&self, list: Rect) -> Vec<Rect> {
        let mut out = Vec::with_capacity(self.entries.len());
        let rh = Self::row_height();
        let y0_base = list.1 + 0.004 - self.scroll;
        for i in 0..self.entries.len() {
            let y0 = y0_base + i as f32 * rh;
            out.push((list.0 + 0.004, y0, list.2 - 0.014, y0 + rh - 0.004));
        }
        out
    }
}

fn rect_contains_pt(r: Rect, p: (f32, f32)) -> bool {
    // Small tolerance to account for `push_outline` drawing
    // borders outside the rect. Without it the visual button
    // edge and the clickable area mismatch by the outline
    // thickness, producing the "have to click two pixels
    // inside" feel the editor was reported to exhibit.
    const TOL: f32 = 0.004;
    p.0 >= r.0 - TOL && p.0 <= r.2 + TOL
        && p.1 >= r.1 - TOL && p.1 <= r.3 + TOL
}

fn fit_small(text: &str, px: f32, max_w: f32) -> String {
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

/// Enumerate `.rlf` files across both stock and user folders.
/// Returns a sorted, deduplicated list tagged by folder origin.
pub fn scan_levels() -> Vec<PickerEntry> {
    let mut out: Vec<PickerEntry> = Vec::new();
    let dirs: &[(&str, &'static str)] = &[
        ("assets/customlevels", "[CUSTOM]"),
        ("assets/levels",       "[STOCK]"),
    ];
    for (rel, tag) in dirs {
        let mut roots: Vec<PathBuf> = vec![PathBuf::from(rel)];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() { roots.push(d.join(rel)); }
        }
        for root in &roots {
            let entries = match std::fs::read_dir(root) {
                Ok(e) => e, Err(_) => continue,
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("rlf") {
                    continue;
                }
                let display = p.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?").to_string();
                // Skip the hidden session file so it never
                // shows up as an importable level.
                if display.starts_with('.') { continue; }
                let path = format!("{}/{}", rel, display);
                if out.iter().all(|m| m.path != path) {
                    out.push(PickerEntry {
                        display, path, folder_tag: tag,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.display.to_lowercase()
        .cmp(&b.display.to_lowercase()));
    out
}

/// Enumerate `.qoa` files across both stock and user folders.
pub fn scan_tracks() -> Vec<PickerEntry> {
    let mut out: Vec<PickerEntry> = Vec::new();
    let dirs: &[(&str, &'static str)] = &[
        ("assets/customlevels/songs", "[CUSTOM]"),
        ("assets/music",              "[STOCK]"),
    ];
    for (rel, tag) in dirs {
        let mut roots: Vec<PathBuf> = vec![PathBuf::from(rel)];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() { roots.push(d.join(rel)); }
        }
        for root in &roots {
            let entries = match std::fs::read_dir(root) {
                Ok(e) => e, Err(_) => continue,
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("qoa") {
                    continue;
                }
                let display = p.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?").to_string();
                let path = format!("{}/{}", rel, display);
                if out.iter().all(|m| m.path != path) {
                    out.push(PickerEntry {
                        display, path, folder_tag: tag,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.display.to_lowercase()
        .cmp(&b.display.to_lowercase()));
    out
}

/// Read a level file from disk with the same relative-first,
/// exe-fallback rule used by the rest of the engine.
pub fn read_level_file(rel_path: &str) -> Option<String> {
    if let Ok(s) = std::fs::read_to_string(rel_path) { return Some(s); }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            let p = d.join(rel_path);
            if let Ok(s) = std::fs::read_to_string(p) { return Some(s); }
        }
    }
    None
}

/// Enumerate `.shader` files in the user shader folder plus
/// any folder an author is likely to have placed them in.
/// Matches the convention used by `scan_tracks` and
/// `scan_levels`: relative path first, exe-relative fallback,
/// deduplicated and sorted.
pub fn scan_shaders() -> Vec<PickerEntry> {
    let mut out: Vec<PickerEntry> = Vec::new();
    let dirs: &[(&str, &'static str)] = &[
        ("assets/shaders/user",       "[USER]"),
        ("assets/customlevels/shaders", "[CUSTOM]"),
    ];
    for (rel, tag) in dirs {
        let mut roots: Vec<PathBuf> = vec![PathBuf::from(rel)];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() { roots.push(d.join(rel)); }
        }
        for root in &roots {
            let entries = match std::fs::read_dir(root) {
                Ok(e) => e, Err(_) => continue,
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("shader") {
                    continue;
                }
                let display = p.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?").to_string();
                let path = format!("{}/{}", rel, display);
                if out.iter().all(|m| m.path != path) {
                    out.push(PickerEntry {
                        display, path, folder_tag: tag,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.display.to_lowercase()
        .cmp(&b.display.to_lowercase()));
    out
}