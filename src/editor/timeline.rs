//! Horizontal timeline with a real seconds axis.
//!
//! Bottom strip of the editor. Authors read it top-down:
//!
//!   [  HEADER  : TIMELINE  10s  20s  30s ...    0:00 - 1:00  ]
//!   [ RULER    : tick marks, pannable                         ]
//!   [ BODY     : section markers, playhead, scrub strip       ]
//!
//! # Units
//!
//! The timeline works in seconds everywhere. Level files
//! stored with `timestamp_format = TrackLength` have `at`
//! values already in seconds. Files stored with `Relative`,
//! `Beats`, or `Named` are converted to seconds for display
//! by multiplying by the live music duration (or the fallback
//! `DEFAULT_DURATION_SECONDS` when no track is loaded) so the
//! author sees the same ruler regardless of the storage
//! format. On drag, the selected section's raw `at` is
//! updated back through the inverse of that conversion so the
//! saved format is preserved.
//!
//! # Interaction
//!
//! * Click empty timeline body: seek audio to that time.
//! * Click section marker: select that section and begin a
//!   marker drag; dragging moves the marker and updates the
//!   section's `at`.
//! * Click ruler (top band): begin a view pan. The ruler is
//!   purely visual navigation, independent of audio position.

use crate::audio::Audio;
use crate::dsl::ast::{LevelAst, Section, TimestampFormat};
use crate::editor::document::Document;
use crate::pipeline::Vertex;
use crate::text_small::{
    push_small, push_small_centered, push_small_right,
    text_height as sh, text_width as sw,
};
use crate::ui::draw::{push_outline, push_quad, push_tri};
use crate::win32::Mouse;

type Rect = (f32, f32, f32, f32);

const ACCENT:     [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:  [f32; 3] = [1.00, 0.80, 0.92];
const DIM:        [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:      [f32; 3] = [1.00, 1.00, 1.00];
const BG_PANEL:   [f32; 3] = [0.10, 0.04, 0.15];
const BG_DEEP:    [f32; 3] = [0.03, 0.01, 0.06];
const PLAYHEAD:   [f32; 3] = [0.95, 1.00, 0.50];
const MARKER_HOV: [f32; 3] = [1.00, 1.00, 0.85];

const DEFAULT_DURATION_SECONDS: f32 = 60.0;

pub struct Timeline {
    /// Leftmost visible time in seconds.
    view_start: f32,
    /// Rightmost visible time in seconds.
    view_end: f32,

    dragging_marker: Option<String>,
    dragging_playhead: bool,
    panning: bool,
    pan_start_screen_x: f32,
    pan_start_view_start: f32,

    hovered_marker: Option<String>,
    prev_left_down: bool,
}

impl Timeline {
    pub fn new() -> Self {
        Timeline {
            view_start: 0.0,
            view_end: DEFAULT_DURATION_SECONDS,
            dragging_marker: None,
            dragging_playhead: false,
            panning: false,
            pan_start_screen_x: 0.0,
            pan_start_view_start: 0.0,
            hovered_marker: None,
            prev_left_down: false,
        }
    }

    /// Current visible time window, in seconds.
    pub fn view_range(&self) -> (f32, f32) {
        (self.view_start, self.view_end)
    }

    /// Update hover and interaction state. Returns the section
    /// name that was clicked this frame, if any, so the editor
    /// shell can propagate selection.
    pub fn update(
        &mut self,
        doc: &mut Document,
        audio: &Audio,
        pointer: (f32, f32),
        mouse: Mouse,
        rect: Rect,
    ) -> Option<String> {
        let clicked = mouse.left_down && !self.prev_left_down;
        let released = !mouse.left_down && self.prev_left_down;
        self.prev_left_down = mouse.left_down;

        // Keep the visible window within sensible bounds,
        // expanding to at least cover the loaded track.
        let track_duration = audio.music_duration().max(1.0);
        let max_end = track_duration.max(DEFAULT_DURATION_SECONDS);
        let current_view_w = (self.view_end - self.view_start).max(1.0);
        if self.view_end > max_end + 1.0 {
            self.view_end = max_end;
            self.view_start = (self.view_end - current_view_w).max(0.0);
        }
        if self.view_start < 0.0 {
            self.view_start = 0.0;
            self.view_end = self.view_start + current_view_w;
        }

        let in_rect = rect_contains_pt(rect, pointer);

        if released {
            self.dragging_marker = None;
            self.dragging_playhead = false;
            self.panning = false;
        }

        // Hover test against section markers.
        self.hovered_marker = None;
        if in_rect {
            let body_y0 = rect.1 + 0.024;
            if pointer.1 > body_y0 {
                for sec in &doc.ast.sections {
                    let seconds = self.section_seconds(
                        sec, &doc.ast, track_duration);
                    let marker_x = self.time_to_screen(seconds, rect);
                    if (pointer.0 - marker_x).abs() < 0.010 {
                        self.hovered_marker = Some(sec.name.clone());
                        break;
                    }
                }
            }
        }

        let mut newly_selected = None;

        if clicked && in_rect {
            if let Some(name) = self.hovered_marker.clone() {
                self.dragging_marker = Some(name.clone());
                newly_selected = Some(name);
            } else if pointer.1 < rect.1 + 0.022 {
                // Click on the ruler strip: begin view pan.
                self.panning = true;
                self.pan_start_screen_x = pointer.0;
                self.pan_start_view_start = self.view_start;
            } else {
                // Click on the timeline body: seek audio.
                self.dragging_playhead = true;
                let t = self.screen_to_time(pointer.0, rect).max(0.0);
                if audio.has_music() {
                    audio.seek_music(t);
                }
            }
        }

        if mouse.left_down {
            if let Some(name) = self.dragging_marker.clone() {
                let t = self.screen_to_time(pointer.0, rect).max(0.0);
                if let Some(i) = doc.ast.sections.iter()
                    .position(|s| s.name == name)
                {
                    let new_at = self.seconds_to_at(
                        t, &doc.ast, track_duration);
                    doc.set_section_at(i, new_at);
                }
            } else if self.dragging_playhead {
                let t = self.screen_to_time(pointer.0, rect).max(0.0);
                if audio.has_music() {
                    audio.seek_music(t);
                }
            } else if self.panning {
                let dx = pointer.0 - self.pan_start_screen_x;
                let view_w = self.view_end - self.view_start;
                let pixels_w = (rect.2 - rect.0 - 0.04).max(0.01);
                let time_per_pixel = view_w / pixels_w;
                let shift = -dx * time_per_pixel;
                let new_start = (self.pan_start_view_start + shift).max(0.0);
                let capped_start = new_start.min(max_end - view_w).max(0.0);
                self.view_start = capped_start;
                self.view_end = capped_start + view_w;
            }
        }

        newly_selected
    }

    pub fn draw(
        &self, doc: &Document, audio: &Audio,
        rect: Rect, out: &mut Vec<Vertex>,
    ) {
        push_quad(out, rect.0, rect.1, rect.2, rect.3, BG_PANEL);
        push_quad(out, rect.0, rect.1, rect.2, rect.1 + 0.002, ACCENT);

        push_small(out, "TIMELINE",
            rect.0 + 0.008, rect.1 + 0.004, 0.0045, ACCENT_HI);

        let range_str = format!("{} TO {}",
            fmt_time(self.view_start), fmt_time(self.view_end));
        push_small_right(out, &range_str,
            rect.2 - 0.008, rect.1 + 0.004, 0.0040, DIM);

        let ruler_y0 = rect.1 + 0.018;
        let ruler_y1 = rect.1 + 0.034;
        push_quad(out, rect.0, ruler_y0, rect.2, ruler_y1, BG_DEEP);

        let view_w = self.view_end - self.view_start;
        let step = pick_time_step(view_w);
        let first_tick = (self.view_start / step).ceil() * step;
        let mut t = first_tick;
        let mut n = 0;
        while t <= self.view_end && n < 200 {
            let x = self.time_to_screen(t, rect);
            if x >= rect.0 + 0.020 && x <= rect.2 - 0.020 {
                push_quad(out,
                    x - 0.0004, ruler_y0,
                    x + 0.0004, ruler_y1, DIM);
                let label = fmt_time(t);
                push_small_centered(out, &label,
                    x, ruler_y0 + 0.002, 0.0035, DIM);
            }
            t += step;
            n += 1;
        }

        let body_y0 = ruler_y1 + 0.002;

        // Markers for every section.
        let track_duration = audio.music_duration().max(1.0);
        for sec in &doc.ast.sections {
            let seconds = self.section_seconds(
                sec, &doc.ast, track_duration);
            let x = self.time_to_screen(seconds, rect);
            if x < rect.0 - 0.02 || x > rect.2 + 0.02 { continue; }

            let highlighted = self.hovered_marker.as_deref()
                == Some(sec.name.as_str());
            let col = if highlighted { MARKER_HOV } else { ACCENT };
            push_quad(out,
                x - 0.0015, body_y0,
                x + 0.0015, rect.3 - 0.004, col);
            // Diamond cap so the marker reads as grabbable.
            push_tri(out,
                [x, body_y0 - 0.010],
                [x - 0.006, body_y0],
                [x + 0.006, body_y0], col);

            if rect.3 - body_y0 > 0.028 {
                let label = fit(&sec.name.to_uppercase(), 0.0040, 0.080);
                push_small_centered(out, &label,
                    x, body_y0 + 0.006, 0.0040, WHITE);
            }
        }

        // Playhead.
        if audio.has_music() {
            let pos = audio.music_position();
            if pos >= self.view_start && pos <= self.view_end {
                let x = self.time_to_screen(pos, rect);
                push_quad(out,
                    x - 0.001, ruler_y0,
                    x + 0.001, rect.3, PLAYHEAD);
                push_tri(out,
                    [x, ruler_y0 + 0.002],
                    [x - 0.006, ruler_y0 - 0.006],
                    [x + 0.006, ruler_y0 - 0.006], PLAYHEAD);
            }
        }
    }

    fn time_to_screen(&self, time: f32, rect: Rect) -> f32 {
        let margin = 0.020;
        let x0 = rect.0 + margin;
        let x1 = rect.2 - margin;
        let w = (x1 - x0).max(0.001);
        let view_w = (self.view_end - self.view_start).max(1e-3);
        x0 + ((time - self.view_start) / view_w) * w
    }

    fn screen_to_time(&self, x: f32, rect: Rect) -> f32 {
        let margin = 0.020;
        let x0 = rect.0 + margin;
        let x1 = rect.2 - margin;
        let w = (x1 - x0).max(0.001);
        let view_w = self.view_end - self.view_start;
        self.view_start + (x - x0) / w * view_w
    }

    fn section_seconds(
        &self, sec: &Section, ast: &LevelAst, duration: f32,
    ) -> f32 {
        match &ast.timestamp_format {
            TimestampFormat::TrackLength => sec.at,
            TimestampFormat::Relative => sec.at * duration,
            TimestampFormat::Beats { total } => {
                (sec.at / (*total).max(1) as f32) * duration
            }
            TimestampFormat::Named(_) => sec.at * duration,
        }
    }

    fn seconds_to_at(
        &self, seconds: f32, ast: &LevelAst, duration: f32,
    ) -> f32 {
        match &ast.timestamp_format {
            TimestampFormat::TrackLength => seconds,
            TimestampFormat::Relative => {
                if duration > 0.1 { seconds / duration } else { 0.0 }
            }
            TimestampFormat::Beats { total } => {
                if duration > 0.1 {
                    (seconds / duration) * (*total).max(1) as f32
                } else { 0.0 }
            }
            TimestampFormat::Named(_) => {
                if duration > 0.1 { seconds / duration } else { 0.0 }
            }
        }
    }
}

fn fmt_time(seconds: f32) -> String {
    let s = seconds.max(0.0);
    if s < 10.0 {
        format!("{:.1}s", s)
    } else if s < 60.0 {
        format!("{:.0}s", s)
    } else {
        let m = (s / 60.0) as u32;
        let rem = (s - (m * 60) as f32).round() as u32;
        format!("{}:{:02}", m, rem)
    }
}

/// Pick a tick step that puts between 5 and 15 ticks on the
/// ruler at the current zoom, choosing from a fixed list so
/// the spacing always looks clean.
fn pick_time_step(view_w: f32) -> f32 {
    let steps = &[
        0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0,
        30.0, 60.0, 120.0, 300.0, 600.0,
    ];
    let target = 10.0;
    for &s in steps {
        if view_w / s <= target * 1.5 {
            return s;
        }
    }
    600.0
}

fn fit(text: &str, px: f32, max_w: f32) -> String {
    if sw(text, px) <= max_w { return text.to_string(); }
    let chars: Vec<char> = text.chars().collect();
    let mut take = chars.len();
    while take > 1 {
        take -= 1;
        let c: String = chars.iter().take(take).collect();
        if sw(&c, px) <= max_w { return c; }
    }
    String::new()
}

fn rect_contains_pt(r: Rect, p: (f32, f32)) -> bool {
    p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3
}