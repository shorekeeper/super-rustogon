//! Inspector panel for the selected section.
//!
//! Renders on the right side of the graph canvas. Shows:
//!
//! * Section header (name, `at` in seconds, move / delete).
//! * Statement list with per-row move up / move down / delete.
//! * Add-statement toolbar with four buttons (emit, wait,
//!   trigger, rule).
//! * Detail editor for the currently selected statement: its
//!   type cycler and one row per numeric or enum parameter.
//! * Meta section: level name (display only), BPM stepper,
//!   sides stepper, music track picker launcher.
//!
//! # Scroll
//!
//! The panel content frequently exceeds the vertical space.
//! The inspector maintains a scroll offset in its own
//! coordinate space (0 at the top, growing downward) and
//! consumes the mouse wheel when the cursor is over the
//! panel. Rows that fall outside the visible rect are culled
//! cheaply by a y-range test.
//!
//! # Mutations
//!
//! Every mutation the inspector applies to the document goes
//! through `doc.touch()` so the dirty bit stays accurate. The
//! inspector does not snapshot history itself; the editor
//! shell is responsible for calling `history.snapshot(...)`
//! before passing the document to `update`.
//!
//! # Action bus
//!
//! A handful of actions cannot be completed inside the
//! inspector alone: picking a music track, importing, opening
//! a picker dialog. The inspector reports these through
//! `InspectorAction` values returned from `update`, which the
//! shell translates into the appropriate picker invocation.

use crate::audio::Audio;
use crate::dsl::ast::{
    Anim, ObstacleSpec, Parity, Section, SpinDir, Stmt, TriggerSpec,
};
use crate::dsl::rules::{
    AbilityKind, RuleCategory, RuleSet, VisionRule, CursorRule,
    AbilityRule, InputRule, ScoreRule, SurvivalRule,
};
use crate::editor::document::Document;
use crate::pipeline::Vertex;
use crate::text_small::{
    push_small, push_small_centered, push_small_right,
    text_height as sh, text_width as sw,
};
use crate::ui::draw::{push_outline, push_quad, push_tri};
use crate::win32::Mouse;

type Rect = (f32, f32, f32, f32);

const ACCENT:      [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:   [f32; 3] = [1.00, 0.80, 0.92];
const DIM:         [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:       [f32; 3] = [1.00, 1.00, 1.00];
const BG_PANEL:    [f32; 3] = [0.10, 0.04, 0.15];
const BG_PANEL_HI: [f32; 3] = [0.22, 0.10, 0.30];
const BG_DEEP:     [f32; 3] = [0.03, 0.01, 0.06];
const STRIPE_EMIT: [f32; 3] = [0.40, 0.85, 1.00];
const STRIPE_TRIG: [f32; 3] = [1.00, 0.65, 0.20];
const STRIPE_WAIT: [f32; 3] = [0.55, 1.00, 0.40];
const STRIPE_RULE: [f32; 3] = [0.70, 0.70, 0.95];
const BAD:         [f32; 3] = [1.00, 0.35, 0.45];

// Row geometry. The earlier draft caused two kinds of visual
// crowding: HDR_FONT text was taller than ROW_H so headers
// bled into the next row, and the statement index column was
// too narrow so the "01" label overlapped the statement body.
// These values fix both by enlarging rows and separating the
// font sizes so every glyph fits cleanly within its row.
const ROW_H: f32 = 0.036;
const GAP:   f32 = 0.008;
const PAD_X: f32 = 0.012;
const FONT:  f32 = 0.0046;
const HDR_FONT: f32 = 0.0050;

/// Request to the shell for something the inspector cannot
/// do on its own.
#[derive(Clone, Debug)]
pub enum InspectorAction {
    None,
    OpenTrackPicker,
    OpenImportPicker,
    OpenShaderPicker,
}

pub struct Inspector {
    scroll: f32,
    /// Index of the statement selected for detailed editing.
    selected_stmt: Option<usize>,
    prev_left_down: bool,
    /// Indices to delete at end of frame, so iteration does
    /// not invalidate by removing mid-loop.
    pending_delete: Option<usize>,
    pending_move: Option<(usize, i32)>,
    pending_shader_delete: Option<usize>,
}

impl Inspector {
    pub fn new() -> Self {
        Inspector {
            scroll: 0.0,
            selected_stmt: None,
            prev_left_down: false,
            pending_delete: None,
            pending_move: None,
            pending_shader_delete: None,
        }
    }

    /// Reset selection when the active section changes.
    pub fn on_section_changed(&mut self) {
        self.selected_stmt = None;
        self.scroll = 0.0;
    }

    /// Run the inspector. Returns whether the inspector
    /// consumed pointer input this frame, plus any action it
    /// wants the shell to perform.
    pub fn update(
        &mut self,
        doc: &mut Document,
        section_idx: Option<usize>,
        pointer: (f32, f32),
        mouse: Mouse,
        scroll_delta: i32,
        rect: Rect,
    ) -> (bool, InspectorAction) {
        let clicked = mouse.left_down && !self.prev_left_down;
        self.prev_left_down = mouse.left_down;

        let pointer_in = rect_contains(rect, pointer);

        if pointer_in && scroll_delta != 0 {
            let notches = scroll_delta as f32 / 120.0;
            self.scroll -= notches * 0.080;
            if self.scroll < 0.0 { self.scroll = 0.0; }
        }

        let mut action = InspectorAction::None;
        let mut consumed = false;

        if !pointer_in && !clicked {
            return (false, action);
        }

        // Walk the layout. Every row/button knows its screen
        // rect and its behavior; we hit test in the same pass
        // we would use for drawing.
        let rows = self.layout(doc, section_idx);
        let origin_y = rect.1 + 0.006 - self.scroll;
        let content_x0 = rect.0 + PAD_X;
        let content_x1 = rect.2 - PAD_X;

        let Some(sec_idx) = section_idx else {
            // No section selected: nothing to interact with.
            // Still consume pointer when inside the rect to
            // prevent click-through to the graph behind.
            return (pointer_in, action);
        };

        for (i, row) in rows.iter().enumerate() {
            let y0 = origin_y + row.offset_y;
            let y1 = y0 + row.height;
            if y1 < rect.1 || y0 > rect.3 {
                continue;
            }
            let row_rect = (content_x0, y0, content_x1, y1);
            let _ = i;
            match &row.kind {
                RowKind::Header(_) => {}
                RowKind::Separator => {}
                RowKind::SectionAt => {
                    if clicked {
                        let (minus, plus) = stepper_button_rects(row_rect);
                        if rect_contains(minus, pointer) {
                            self.section_at_delta(doc, sec_idx, -0.1);
                            consumed = true;
                        } else if rect_contains(plus, pointer) {
                            self.section_at_delta(doc, sec_idx, 0.1);
                            consumed = true;
                        }
                    }
                }
                RowKind::StatementListItem { stmt_idx } => {
                    let idx = *stmt_idx;
                    let (move_up, move_down, del, body) =
                        statement_row_hit_areas(row_rect);
                    if clicked {
                        if rect_contains(move_up, pointer) {
                            self.pending_move = Some((idx, -1));
                            consumed = true;
                        } else if rect_contains(move_down, pointer) {
                            self.pending_move = Some((idx, 1));
                            consumed = true;
                        } else if rect_contains(del, pointer) {
                            self.pending_delete = Some(idx);
                            consumed = true;
                        } else if rect_contains(body, pointer) {
                            self.selected_stmt = Some(idx);
                            consumed = true;
                        }
                    }
                }
                RowKind::AddStmtPair { left, right } => {
                    if clicked {
                        let (lr, rr) = pair_button_rects(row_rect);
                        if rect_contains(lr, pointer) {
                            self.add_statement(doc, sec_idx, *left);
                            consumed = true;
                        } else if let Some(rk) = right {
                            if rect_contains(rr, pointer) {
                                self.add_statement(doc, sec_idx, *rk);
                                consumed = true;
                            }
                        }
                    }
                }
                RowKind::DetailCycle { kind } => {
                    if clicked {
                        let (left, right) = cycle_button_rects(row_rect);
                        if rect_contains(left, pointer) {
                            self.cycle_detail(doc, sec_idx, *kind, -1);
                            consumed = true;
                        } else if rect_contains(right, pointer) {
                            self.cycle_detail(doc, sec_idx, *kind, 1);
                            consumed = true;
                        }
                    }
                }
                RowKind::DetailStepper { kind } => {
                    if clicked {
                        let (minus, plus) = stepper_button_rects(row_rect);
                        if rect_contains(minus, pointer) {
                            self.step_detail(doc, sec_idx, *kind, -1);
                            consumed = true;
                        } else if rect_contains(plus, pointer) {
                            self.step_detail(doc, sec_idx, *kind, 1);
                            consumed = true;
                        }
                    }
                }
                RowKind::MetaBpm => {
                    if clicked {
                        let (minus, plus) = stepper_button_rects(row_rect);
                        if rect_contains(minus, pointer) {
                            let n = doc.ast.meta.bpm.saturating_sub(5).max(40);
                            if n != doc.ast.meta.bpm {
                                doc.ast.meta.bpm = n;
                                doc.touch();
                            }
                            consumed = true;
                        } else if rect_contains(plus, pointer) {
                            let n = (doc.ast.meta.bpm + 5).min(300);
                            if n != doc.ast.meta.bpm {
                                doc.ast.meta.bpm = n;
                                doc.touch();
                            }
                            consumed = true;
                        }
                    }
                }
                RowKind::MetaSides => {
                    if clicked {
                        let (minus, plus) = stepper_button_rects(row_rect);
                        if rect_contains(minus, pointer) {
                            let n = doc.ast.generation.sides
                                .saturating_sub(1).max(3);
                            if n != doc.ast.generation.sides {
                                doc.ast.generation.sides = n;
                                doc.touch();
                            }
                            consumed = true;
                        } else if rect_contains(plus, pointer) {
                            let n = (doc.ast.generation.sides + 1).min(12);
                            if n != doc.ast.generation.sides {
                                doc.ast.generation.sides = n;
                                doc.touch();
                            }
                            consumed = true;
                        }
                    }
                }
                RowKind::MetaMusicPicker => {
                    if clicked && rect_contains(row_rect, pointer) {
                        action = InspectorAction::OpenTrackPicker;
                        consumed = true;
                    }
                }
                RowKind::ShaderListItem { shader_idx } => {
                    let idx = *shader_idx;
                    if clicked {
                        let del_r = shader_row_delete_rect(row_rect);
                        if rect_contains(del_r, pointer) {
                            self.pending_shader_delete = Some(idx);
                            consumed = true;
                        }
                    }
                }
                RowKind::AddShaderButton => {
                    if clicked && rect_contains(row_rect, pointer) {
                        action = InspectorAction::OpenShaderPicker;
                        consumed = true;
                    }
                }
            }
        }

        // Apply pending mutations.
        if let Some(idx) = self.pending_delete.take() {
            if let Some(sec) = doc.ast.sections.get_mut(sec_idx) {
                if idx < sec.body.len() {
                    sec.body.remove(idx);
                    doc.touch();
                    if self.selected_stmt == Some(idx) {
                        self.selected_stmt = None;
                    } else if let Some(s) = self.selected_stmt {
                        if s > idx { self.selected_stmt = Some(s - 1); }
                    }
                }
            }
        }
        if let Some((idx, delta)) = self.pending_move.take() {
            if let Some(sec) = doc.ast.sections.get_mut(sec_idx) {
                let n = sec.body.len();
                let new_idx = (idx as i32 + delta)
                    .clamp(0, n as i32 - 1) as usize;
                if new_idx != idx {
                    let item = sec.body.remove(idx);
                    sec.body.insert(new_idx, item);
                    doc.touch();
                    if self.selected_stmt == Some(idx) {
                        self.selected_stmt = Some(new_idx);
                    }
                }
            }
        }

        if let Some(idx) = self.pending_shader_delete.take() {
            if idx < doc.ast.shaders.len() {
                doc.ast.shaders.remove(idx);
                doc.touch();
            }
        }

        (consumed, action)
    }

    /// Emit geometry for the inspector.
    pub fn draw(
        &self,
        doc: &Document,
        section_idx: Option<usize>,
        pointer: (f32, f32),
        rect: Rect,
        out: &mut Vec<Vertex>,
    ) {
        push_quad(out, rect.0, rect.1, rect.2, rect.3, BG_PANEL);
        push_quad(out, rect.0, rect.1, rect.0 + 0.002, rect.3, ACCENT);

        let title_rect = (rect.0, rect.1, rect.2, rect.1 + 0.028);
        push_quad(out, title_rect.0, title_rect.1,
            title_rect.2, title_rect.3, BG_PANEL_HI);
        push_small(out, "INSPECTOR",
            rect.0 + PAD_X, rect.1 + 0.009, HDR_FONT, ACCENT_HI);

        if section_idx.is_none() {
            push_small_centered(out,
                "SELECT A SECTION",
                (rect.0 + rect.2) * 0.5, rect.1 + 0.100,
                FONT, DIM);
            push_small_centered(out,
                "FROM THE GRAPH OR SIDEBAR",
                (rect.0 + rect.2) * 0.5, rect.1 + 0.120,
                FONT, DIM);
            return;
        }

        let sec_idx = section_idx.unwrap();
        let rows = self.layout(doc, section_idx);
        let origin_y = rect.1 + 0.030 + 0.006 - self.scroll;
        let content_x0 = rect.0 + PAD_X;
        let content_x1 = rect.2 - PAD_X;

        for row in rows.iter() {
            let y0 = origin_y + row.offset_y;
            let y1 = y0 + row.height;
            if y1 < rect.1 + 0.030 || y0 > rect.3 { continue; }
            let row_rect = (content_x0, y0, content_x1, y1);
            self.draw_row(out, doc, sec_idx, &row.kind, row_rect, pointer);
        }

        // Scroll indicator.
        let max = self.max_scroll(&rows, rect);
        if max > 0.0 {
            let t = (self.scroll / max).clamp(0.0, 1.0);
            let track_x = rect.2 - 0.004;
            let track_y0 = rect.1 + 0.034;
            let track_y1 = rect.3 - 0.004;
            push_quad(out, track_x - 0.001, track_y0,
                track_x + 0.001, track_y1, BG_DEEP);
            let h = track_y1 - track_y0;
            let visible_fraction = (h / (h + max)).clamp(0.05, 1.0);
            let thumb_h = h * visible_fraction;
            let thumb_y = track_y0 + (h - thumb_h) * t;
            push_quad(out, track_x - 0.002, thumb_y,
                track_x + 0.002, thumb_y + thumb_h, ACCENT);
        }
    }

    // ---------- layout ----------

    fn layout(
        &self, doc: &Document, section_idx: Option<usize>,
    ) -> Vec<LayoutRow> {
        let mut rows: Vec<LayoutRow> = Vec::new();
        let mut y = 0.0f32;
        let mut push = |rows: &mut Vec<LayoutRow>, y: &mut f32,
                        kind: RowKind, h: f32| {
            rows.push(LayoutRow { offset_y: *y, height: h, kind });
            *y += h + GAP;
        };

        let Some(sec_idx) = section_idx else { return rows; };
        let sec = match doc.ast.sections.get(sec_idx) {
            Some(s) => s, None => return rows,
        };

        push(&mut rows, &mut y, RowKind::Header("SECTION".into()), ROW_H);
        push(&mut rows, &mut y,
            RowKind::SectionAt, ROW_H);
        push(&mut rows, &mut y, RowKind::Separator, 0.010);

        
        let n = sec.body.len();
        push(&mut rows, &mut y,
            RowKind::Header(format!("STATEMENTS ({})", n)), ROW_H);
        for i in 0..n {
            push(&mut rows, &mut y,
                RowKind::StatementListItem { stmt_idx: i }, ROW_H);
        }

        // Add buttons arranged as a 2x2 grid. Previously each
        // of the four kinds occupied its own row, which ate
        // 4 * (ROW_H + GAP) of vertical space just on add
        // controls and pushed the detail editor offscreen on
        // smaller inspector panels.
        push(&mut rows, &mut y,
            RowKind::AddStmtPair {
                left: AddKind::Emit,
                right: Some(AddKind::Wait),
            }, ROW_H);
        push(&mut rows, &mut y,
            RowKind::AddStmtPair {
                left: AddKind::Trigger,
                right: Some(AddKind::Rule),
            }, ROW_H);

        // Detail editor for the selected statement.
        if let Some(i) = self.selected_stmt {
            if let Some(stmt) = sec.body.get(i) {
                push(&mut rows, &mut y, RowKind::Separator, 0.010);
                let title = format!("DETAIL  #{:02}", i + 1);
                push(&mut rows, &mut y,
                    RowKind::Header(title), ROW_H);
                push_detail_rows(&mut rows, &mut y, stmt);
            }
        }

        push(&mut rows, &mut y, RowKind::Separator, 0.010);
        push(&mut rows, &mut y,
            RowKind::Header(format!(
                "SHADERS ({})", doc.ast.shaders.len())),
            ROW_H);
        for i in 0..doc.ast.shaders.len() {
            push(&mut rows, &mut y,
                RowKind::ShaderListItem { shader_idx: i }, ROW_H);
        }
        push(&mut rows, &mut y, RowKind::AddShaderButton, ROW_H);

        // Meta section.
        push(&mut rows, &mut y, RowKind::Separator, 0.010);
        push(&mut rows, &mut y,
            RowKind::Header("META".into()), ROW_H);
        push(&mut rows, &mut y, RowKind::MetaBpm, ROW_H);
        push(&mut rows, &mut y, RowKind::MetaSides, ROW_H);
        push(&mut rows, &mut y, RowKind::MetaMusicPicker, ROW_H);

        rows
    }

    fn max_scroll(&self, rows: &[LayoutRow], rect: Rect) -> f32 {
        let total = rows.last()
            .map(|r| r.offset_y + r.height)
            .unwrap_or(0.0);
        let visible = rect.3 - (rect.1 + 0.030) - 0.012;
        (total - visible).max(0.0)
    }

    // ---------- draw one row ----------

    fn draw_row(
        &self, out: &mut Vec<Vertex>,
        doc: &Document, sec_idx: usize,
        kind: &RowKind, rect: Rect,
        pointer: (f32, f32),
    ) {
        match kind {
            RowKind::Header(label) => {
                push_small(out, label,
                    rect.0, rect.1 + 0.004,
                    HDR_FONT, ACCENT_HI);
            }
            RowKind::Separator => {
                let mid = (rect.1 + rect.3) * 0.5;
                push_quad(out, rect.0, mid - 0.0008,
                    rect.2, mid + 0.0008, DIM);
            }
            RowKind::SectionAt => {
                let Some(sec) = doc.ast.sections.get(sec_idx) else { return; };
                let label = format!("AT  {:.3}S", sec.at);
                self.draw_stepper_row(out, rect, &label, pointer);
            }
            RowKind::StatementListItem { stmt_idx } => {
                let Some(sec) = doc.ast.sections.get(sec_idx) else { return; };
                let Some(stmt) = sec.body.get(*stmt_idx) else { return; };
                let selected = self.selected_stmt == Some(*stmt_idx);
                let bg = if selected { BG_PANEL_HI } else { BG_DEEP };
                push_quad(out, rect.0, rect.1, rect.2, rect.3, bg);
                let stripe = stripe_for(stmt);
                push_quad(out, rect.0, rect.1,
                    rect.0 + 0.004, rect.3, stripe);

                // Index column starts after a visible gap from
                // the colored stripe. Using the advance width
                // of the 4x6 font, a two-digit index is 0.041
                // wide at FONT=0.0046, so the label column
                // starts at 0.012 + 0.041 + 0.008 gap = 0.061.
                let idx_s = format!("{:02}", stmt_idx + 1);
                push_small(out, &idx_s,
                    rect.0 + 0.012,
                    (rect.1 + rect.3) * 0.5 - sh(FONT) * 0.5,
                    FONT, DIM);

                let label = stmt_label(stmt);
                let label_x = rect.0 + 0.062;
                let label_max = rect.2 - label_x - 0.080;
                let label = fit(&label, FONT, label_max);
                push_small(out, &label,
                    label_x,
                    (rect.1 + rect.3) * 0.5 - sh(FONT) * 0.5,
                    FONT, WHITE);

                let (up, down, del, _body) = statement_row_hit_areas(rect);
                draw_mini_arrow(out, up,   true,  pointer, ACCENT);
                draw_mini_arrow(out, down, false, pointer, ACCENT);
                draw_mini(out, del, "X", pointer, BAD);
            }
            RowKind::AddStmtPair { left, right } => {
                let (lr, rr) = pair_button_rects(rect);
                self.draw_add_button(out, lr, *left, pointer);
                if let Some(rk) = right {
                    self.draw_add_button(out, rr, *rk, pointer);
                }
            }
            RowKind::DetailCycle { kind } => {
                let Some(sec) = doc.ast.sections.get(sec_idx) else { return; };
                let Some(stmt) = sec.body.get(
                    self.selected_stmt.unwrap_or(0)) else { return; };
                let (label, value) = detail_cycle_label_value(kind, stmt, doc);
                let text = format!("{}  {}", label, value);
                self.draw_cycle_row(out, rect, &text, pointer);
            }
            RowKind::DetailStepper { kind } => {
                let Some(sec) = doc.ast.sections.get(sec_idx) else { return; };
                let Some(stmt) = sec.body.get(
                    self.selected_stmt.unwrap_or(0)) else { return; };
                let (label, value) = detail_stepper_label_value(kind, stmt);
                let text = format!("{}  {}", label, value);
                self.draw_stepper_row(out, rect, &text, pointer);
            }
            RowKind::MetaBpm => {
                let label = format!("BPM  {}", doc.ast.meta.bpm);
                self.draw_stepper_row(out, rect, &label, pointer);
            }
            RowKind::MetaSides => {
                let label = format!("SIDES  {}", doc.ast.generation.sides);
                self.draw_stepper_row(out, rect, &label, pointer);
            }
            RowKind::MetaMusicPicker => {
                let is_hover = rect_contains(rect, pointer);
                let bg = if is_hover { BG_PANEL_HI } else { BG_DEEP };
                let ring = if is_hover { ACCENT_HI } else { ACCENT };
                push_quad(out, rect.0, rect.1, rect.2, rect.3, bg);
                push_outline(out, rect.0, rect.1, rect.2, rect.3,
                    0.002, ring);
                let track = if doc.ast.meta.music.is_empty() {
                    "PICK TRACK...".to_string()
                } else {
                    std::path::Path::new(&doc.ast.meta.music)
                        .file_name().and_then(|s| s.to_str())
                        .unwrap_or("?").to_uppercase()
                };
                let text = format!("MUSIC  {}", track);
                let max = rect.2 - rect.0 - 0.016;
                let text = fit(&text, FONT, max);
                push_small(out, &text,
                    rect.0 + 0.008,
                    (rect.1 + rect.3) * 0.5 - sh(FONT) * 0.5,
                    FONT, WHITE);
            }
            RowKind::ShaderListItem { shader_idx } => {
                let Some(decl) = doc.ast.shaders.get(*shader_idx) else { return; };
                push_quad(out, rect.0, rect.1, rect.2, rect.3, BG_DEEP);
                push_quad(out, rect.0, rect.1,
                    rect.0 + 0.004, rect.3, STRIPE_RULE);
                let name_x = rect.0 + 0.012;
                push_small(out, &decl.name.to_uppercase(),
                    name_x,
                    (rect.1 + rect.3) * 0.5 - sh(FONT) * 0.5,
                    FONT, WHITE);
                let file = std::path::Path::new(&decl.path)
                    .file_name().and_then(|s| s.to_str())
                    .unwrap_or("?").to_uppercase();
                let max_w = rect.2 - name_x - 0.080 - 0.040;
                let file = fit(&file, FONT, max_w);
                push_small_right(out, &file,
                    rect.2 - 0.040,
                    (rect.1 + rect.3) * 0.5 - sh(FONT) * 0.5,
                    FONT, DIM);
                let del_r = shader_row_delete_rect(rect);
                draw_mini(out, del_r, "X", pointer, BAD);
            }
            RowKind::AddShaderButton => {
                let is_hover = rect_contains(rect, pointer);
                let bg = if is_hover { BG_PANEL_HI } else { BG_DEEP };
                let ring = if is_hover { ACCENT_HI } else { ACCENT };
                push_quad(out, rect.0, rect.1, rect.2, rect.3, bg);
                push_outline(out, rect.0, rect.1, rect.2, rect.3,
                    0.002, ring);
                let cx = (rect.0 + rect.2) * 0.5;
                let cy = (rect.1 + rect.3) * 0.5;
                let label = "+ ADD SHADER";
                let w = sw(label, FONT);
                push_small(out, label,
                    cx - w * 0.5, cy - sh(FONT) * 0.5,
                    FONT, WHITE);
            }
        }
    }

    fn draw_stepper_row(
        &self, out: &mut Vec<Vertex>,
        rect: Rect, label: &str, pointer: (f32, f32),
    ) {
        let (minus, plus) = stepper_button_rects(rect);
        push_quad(out, rect.0, rect.1, rect.2, rect.3, BG_DEEP);
        push_small(out, label,
            rect.0 + 0.006,
            (rect.1 + rect.3) * 0.5 - sh(FONT) * 0.5,
            FONT, WHITE);
        draw_mini(out, minus, "-", pointer, ACCENT);
        draw_mini(out, plus,  "+", pointer, ACCENT);
    }

    fn draw_cycle_row(
        &self, out: &mut Vec<Vertex>,
        rect: Rect, label: &str, pointer: (f32, f32),
    ) {
        let (left, right) = cycle_button_rects(rect);
        push_quad(out, rect.0, rect.1, rect.2, rect.3, BG_DEEP);
        push_small(out, label,
            rect.0 + 0.006,
            (rect.1 + rect.3) * 0.5 - sh(FONT) * 0.5,
            FONT, WHITE);
        draw_mini(out, left,  "<", pointer, ACCENT);
        draw_mini(out, right, ">", pointer, ACCENT);
    }

    // ---------- mutations ----------

    fn section_at_delta(&self, doc: &mut Document, idx: usize, d: f32) {
        let cur = doc.ast.sections.get(idx).map(|s| s.at).unwrap_or(0.0);
        doc.set_section_at(idx, (cur + d).max(0.0));
    }

    fn add_statement(&mut self, doc: &mut Document,
                     sec_idx: usize, kind: AddKind) {
        let stmt = match kind {
            AddKind::Emit => Stmt::Emit(ObstacleSpec::Bar {
                thickness_mult: 1.0,
            }),
            AddKind::Wait => Stmt::Wait(2),
            AddKind::Trigger => Stmt::Trigger(TriggerSpec::Flip),
            AddKind::Rule => Stmt::Rule(RuleSet::Vision(VisionRule::default())),
        };
        if let Some(sec) = doc.ast.sections.get_mut(sec_idx) {
            sec.body.push(stmt);
            self.selected_stmt = Some(sec.body.len() - 1);
            doc.touch();
        }
    }

    fn cycle_detail(
        &self, doc: &mut Document, sec_idx: usize,
        kind: DetailCycleKind, sign: i32,
    ) {
        let Some(idx) = self.selected_stmt else { return; };
        let shader_count = doc.ast.shaders.len();
        let Some(sec) = doc.ast.sections.get_mut(sec_idx) else { return; };
        let Some(stmt) = sec.body.get_mut(idx) else { return; };
        let changed = apply_detail_cycle(stmt, kind, sign, shader_count);
        if changed { doc.touch(); }
    }

    fn step_detail(
        &self, doc: &mut Document, sec_idx: usize,
        kind: DetailStepperKind, sign: i32,
    ) {
        let Some(idx) = self.selected_stmt else { return; };
        let Some(sec) = doc.ast.sections.get_mut(sec_idx) else { return; };
        let Some(stmt) = sec.body.get_mut(idx) else { return; };
        let changed = apply_detail_stepper(stmt, kind, sign);
        if changed { doc.touch(); }
    }

    fn draw_add_button(
        &self, out: &mut Vec<Vertex>, r: Rect,
        kind: AddKind, pointer: (f32, f32),
    ) {
        let is_hover = rect_contains(r, pointer);
        let bg = if is_hover { BG_PANEL_HI } else { BG_DEEP };
        let ring = if is_hover { ACCENT_HI } else { ACCENT };
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.002, ring);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let label = add_kind_label(kind);
        let w = sw(label, FONT);
        push_small(out, label,
            cx - w * 0.5, cy - sh(FONT) * 0.5,
            FONT, WHITE);
    }

}

// ---------- row kinds ----------

struct LayoutRow { offset_y: f32, height: f32, kind: RowKind }

enum RowKind {
    Header(String),
    Separator,
    SectionAt,
    StatementListItem { stmt_idx: usize },
    /// Row containing one or two add-statement buttons
    /// laid out horizontally. The right side is optional so
    /// rows with an odd total count render cleanly.
    AddStmtPair { left: AddKind, right: Option<AddKind> },
    DetailCycle { kind: DetailCycleKind },
    DetailStepper { kind: DetailStepperKind },
    MetaBpm,
    MetaSides,
    MetaMusicPicker,
    ShaderListItem { shader_idx: usize },
    AddShaderButton,
}

#[derive(Clone, Copy)]
enum AddKind { Emit, Wait, Trigger, Rule }

/// Enumeration of cycleable detail fields keyed by position
/// within the currently selected statement.
#[derive(Clone, Copy)]
enum DetailCycleKind {
    ObstacleType,
    TriggerType,
    Dir,
    Parity,
    Anim,
    AbilityKind,
    RuleCategory,
    PostShaderSlot,
}

#[derive(Clone, Copy)]
enum DetailStepperKind {
    Thickness,
    Spacing,
    Loops,
    Spokes,
    Count,
    Rungs,
    LayersOrSteps,
    TunnelLength,
    CorridorLength,
    Lanes,
    Turns,
    WaitBeats,
    RepeatCount,
    TiltAngle,
    TiltPitch,
    TiltYaw,
    TiltDuration,
    SpeedMultFactor,
    HueShiftRate,
    SpeedMultDuration,
    HueShiftDuration,
    ShakeStrength,
    ShakeDuration,
    GlitchStrength,
    GlitchDuration,
    ZoomTarget,
    ZoomDuration,
    InvertDuration,
    StrobeRate,
    StrobeDuration,
    SpeedwarpWalls,
    SpeedwarpRotation,
    SpeedwarpCursor,
    SpeedwarpMusic,
    SpeedwarpDuration,
    SpinRate, SpinDuration,
    BounceAmplitude, BounceDuration,
    FreezeDuration,
    ZoomPunchStrength, ZoomPunchDuration,
    InvertColorsDuration,
    GrayscaleStrength, GrayscaleDuration,
    ShockwaveStrength, ShockwaveDuration,
    FogNear, FogFar, FogDuration,
    OutlineThickness, OutlineDuration,
    CenterburstStrength, CenterburstDuration,
    RingburstCount, RingburstDuration,
    BassdropStrength, BassdropDuration,
    PostShaderP0, PostShaderP1, PostShaderP2, PostShaderP3,
    MorphSides,
    MorphDuration,
}

// ---------- detail rows builder ----------
/// Split a row rect into two equal halves for a 2x2 button
/// grid row. The gap in the middle matches the inspector's
/// standard `GAP` to avoid visual crowding.
fn pair_button_rects(row: Rect) -> (Rect, Rect) {
    let mid = (row.0 + row.2) * 0.5;
    let left  = (row.0, row.1, mid - GAP * 0.5, row.3);
    let right = (mid + GAP * 0.5, row.1, row.2, row.3);
    (left, right)
}

fn add_kind_label(k: AddKind) -> &'static str {
    match k {
        AddKind::Emit    => "+ EMIT",
        AddKind::Wait    => "+ WAIT",
        AddKind::Trigger => "+ TRIGGER",
        AddKind::Rule    => "+ RULE",
    }
}

fn push_detail_rows(
    rows: &mut Vec<LayoutRow>, y: &mut f32, stmt: &Stmt,
) {
    let mut push = |rows: &mut Vec<LayoutRow>, y: &mut f32,
                    kind: RowKind| {
        rows.push(LayoutRow { offset_y: *y, height: ROW_H, kind });
        *y += ROW_H + GAP;
    };
    match stmt {
        Stmt::Emit(spec) => {
            push(rows, y, RowKind::DetailCycle {
                kind: DetailCycleKind::ObstacleType });
            match spec {
                ObstacleSpec::Bar { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Thickness });
                }
                ObstacleSpec::DoubleBar { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Spacing });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Thickness });
                }
                ObstacleSpec::Spiral { .. } => {
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Dir });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Loops });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Thickness });
                }
                ObstacleSpec::Alternate { .. } => {
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Parity });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Thickness });
                }
                ObstacleSpec::Pinwheel { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Spokes });
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Dir });
                }
                ObstacleSpec::Rain { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Count });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Thickness });
                }
                ObstacleSpec::Rainbow { .. } => {
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Dir });
                }
                ObstacleSpec::Ladder { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Rungs });
                }
                ObstacleSpec::Tunnel { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::TunnelLength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Lanes });
                }
                ObstacleSpec::Pot { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::LayersOrSteps });
                }
                ObstacleSpec::Staircase { .. } => {
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Dir });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::LayersOrSteps });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Thickness });
                }
                ObstacleSpec::Corridor { .. } => {
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Dir });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::CorridorLength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::Turns });
                }
                ObstacleSpec::Cubes { .. } => {
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Dir });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::LayersOrSteps });
                }
                ObstacleSpec::Custom { .. } => {}
                ObstacleSpec::CustomFormula { .. } => {}
            }
        }
        Stmt::Wait(_) => {
            push(rows, y, RowKind::DetailStepper {
                kind: DetailStepperKind::WaitBeats });
        }
        Stmt::Trigger(t) => {
            push(rows, y, RowKind::DetailCycle {
                kind: DetailCycleKind::TriggerType });
            match t {
                TriggerSpec::Flip | TriggerSpec::Pulse => {}
                TriggerSpec::Tilt { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::TiltAngle });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::TiltPitch });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::TiltYaw });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::TiltDuration });
                }
                TriggerSpec::SpeedMult { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpeedMultFactor });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpeedMultDuration });
                }
                TriggerSpec::HueShift { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::HueShiftRate });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::HueShiftDuration });
                }
                TriggerSpec::Shake { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ShakeStrength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ShakeDuration });
                }
                TriggerSpec::Glitch { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::GlitchStrength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::GlitchDuration });
                }
                TriggerSpec::Zoom { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ZoomTarget });
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::Anim });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ZoomDuration });
                }
                TriggerSpec::Invert { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::InvertDuration });
                }
                TriggerSpec::Strobe { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::StrobeRate });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::StrobeDuration });
                }
                TriggerSpec::SpeedWarp { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpeedwarpWalls });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpeedwarpRotation });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpeedwarpCursor });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpeedwarpMusic });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpeedwarpDuration });
                }
                TriggerSpec::Spin { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpinRate });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::SpinDuration });
                }
                TriggerSpec::Bounce { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::BounceAmplitude });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::BounceDuration });
                }
                TriggerSpec::Freeze { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::FreezeDuration });
                }
                TriggerSpec::ZoomPunch { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ZoomPunchStrength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ZoomPunchDuration });
                }
                TriggerSpec::InvertColors { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::InvertColorsDuration });
                }
                TriggerSpec::Grayscale { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::GrayscaleStrength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::GrayscaleDuration });
                }
                TriggerSpec::Shockwave { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ShockwaveStrength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::ShockwaveDuration });
                }
                TriggerSpec::Fog { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::FogNear });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::FogFar });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::FogDuration });
                }
                TriggerSpec::Outline { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::OutlineThickness });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::OutlineDuration });
                }
                TriggerSpec::Centerburst { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::CenterburstStrength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::CenterburstDuration });
                }
                TriggerSpec::Ringburst { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::RingburstCount });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::RingburstDuration });
                }
                TriggerSpec::Bassdrop { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::BassdropStrength });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::BassdropDuration });
                }
                TriggerSpec::PostShader { .. } => {
                    push(rows, y, RowKind::DetailCycle {
                        kind: DetailCycleKind::PostShaderSlot });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::PostShaderP0 });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::PostShaderP1 });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::PostShaderP2 });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::PostShaderP3 });
                }
                TriggerSpec::PostShaderOff => {}
                TriggerSpec::Morph { .. } => {
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::MorphSides });
                    push(rows, y, RowKind::DetailStepper {
                        kind: DetailStepperKind::MorphDuration });
                }
            }
        }
        Stmt::Repeat { .. } => {
            push(rows, y, RowKind::DetailStepper {
                kind: DetailStepperKind::RepeatCount });
        }
        Stmt::Rule(_) | Stmt::Revert(_) | Stmt::Push(_) | Stmt::Pop(_) => {
            push(rows, y, RowKind::DetailCycle {
                kind: DetailCycleKind::RuleCategory });
        }
        Stmt::LocalVars(_) => {}
    }
}

// ---------- detail mutation ----------

fn apply_detail_cycle(
    stmt: &mut Stmt, kind: DetailCycleKind, sign: i32,
    shader_count: usize,
) -> bool {
    let forward = sign > 0;
    match (stmt, kind) {
        (Stmt::Emit(spec), DetailCycleKind::ObstacleType) => {
            *spec = cycle_obstacle(spec, forward);
            true
        }
        (Stmt::Emit(spec), DetailCycleKind::Dir) => {
            toggle_obstacle_dir(spec);
            true
        }
        (Stmt::Emit(spec), DetailCycleKind::Parity) => {
            toggle_obstacle_parity(spec);
            true
        }
        (Stmt::Trigger(t), DetailCycleKind::TriggerType) => {
            *t = cycle_trigger(t, forward);
            true
        }
        (Stmt::Trigger(TriggerSpec::Zoom { anim, .. }),
                DetailCycleKind::Anim) => {
            *anim = cycle_anim(*anim, forward);
            true
        }
        (Stmt::Rule(rs), DetailCycleKind::RuleCategory) => {
            *rs = cycle_rule(rs, forward);
            true
        }
        (Stmt::Revert(c), DetailCycleKind::RuleCategory)
        | (Stmt::Push(c),   DetailCycleKind::RuleCategory)
        | (Stmt::Pop(c),    DetailCycleKind::RuleCategory) => {
            *c = cycle_rule_category(*c, forward);
            true
        }
        (Stmt::Trigger(TriggerSpec::PostShader { slot, .. }),
                DetailCycleKind::PostShaderSlot) => {
            if shader_count == 0 { return false; }
            let n = shader_count as i32;
            let cur = *slot as i32;
            let next = if sign >= 0 {
                (cur + 1).rem_euclid(n)
            } else {
                (cur - 1).rem_euclid(n)
            };
            *slot = next as u8; true
        }
        _ => false,
    }
}

fn apply_detail_stepper(stmt: &mut Stmt, kind: DetailStepperKind, sign: i32) -> bool {
    let s = sign as f32;
    match (stmt, kind) {
        (Stmt::Wait(n), DetailStepperKind::WaitBeats) => {
            let nv = (*n as i32 + sign).clamp(1, 64);
            *n = nv as u32;
            true
        }
        (Stmt::Repeat { count, .. }, DetailStepperKind::RepeatCount) => {
            let nv = (*count as i32 + sign).clamp(1, 64);
            *count = nv as u32;
            true
        }
        (Stmt::Emit(spec), DetailStepperKind::Thickness) => {
            step_obstacle_thickness(spec, s * 0.05); true
        }
        (Stmt::Emit(ObstacleSpec::DoubleBar { spacing, .. }),
                DetailStepperKind::Spacing) => {
            let n = (*spacing as i32 + sign).clamp(1, 6); *spacing = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Spiral { loops, .. }),
                DetailStepperKind::Loops) => {
            let n = (*loops as i32 + sign).clamp(1, 6); *loops = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Pinwheel { spokes, .. }),
                DetailStepperKind::Spokes) => {
            let n = (*spokes as i32 + sign).clamp(1, 6); *spokes = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Rain { count, .. }),
                DetailStepperKind::Count) => {
            let n = (*count as i32 + sign).clamp(2, 12); *count = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Ladder { rungs }),
                DetailStepperKind::Rungs) => {
            let n = (*rungs as i32 + sign).clamp(3, 16); *rungs = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Tunnel { length, .. }),
                DetailStepperKind::TunnelLength) => {
            *length = (*length + s * 0.1).clamp(0.4, 2.5); true
        }
        (Stmt::Emit(ObstacleSpec::Tunnel { lanes, .. }),
                DetailStepperKind::Lanes) => {
            let n = (*lanes as i32 + sign).clamp(1, 4); *lanes = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Pot { layers }),
                DetailStepperKind::LayersOrSteps) => {
            let n = (*layers as i32 + sign).clamp(2, 8); *layers = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Staircase { steps, .. }),
                DetailStepperKind::LayersOrSteps) => {
            let n = (*steps as i32 + sign).clamp(3, 24); *steps = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Cubes { layers, .. }),
                DetailStepperKind::LayersOrSteps) => {
            let n = (*layers as i32 + sign).clamp(2, 8); *layers = n as u32; true
        }
        (Stmt::Emit(ObstacleSpec::Corridor { length, .. }),
                DetailStepperKind::CorridorLength) => {
            *length = (*length + s * 0.1).clamp(0.6, 3.0); true
        }
        (Stmt::Emit(ObstacleSpec::Corridor { turns, .. }),
                DetailStepperKind::Turns) => {
            let n = (*turns as i32 + sign).clamp(1, 8); *turns = n as u32; true
        }
        (Stmt::Trigger(TriggerSpec::Tilt { angle, .. }),
                DetailStepperKind::TiltAngle) => {
            // Step in whole degrees. With the 30 degree clamp
            // this gives 60 clicks to sweep the full range,
            // which is precise enough for authoring.
            *angle = (*angle + s * 1.0).clamp(-30.0, 30.0); true
        }
        (Stmt::Trigger(TriggerSpec::Tilt { pitch, .. }),
                DetailStepperKind::TiltPitch) => {
            *pitch = (*pitch + s * 1.0).clamp(-30.0, 30.0); true
        }
        (Stmt::Trigger(TriggerSpec::Tilt { yaw, .. }),
                DetailStepperKind::TiltYaw) => {
            *yaw = (*yaw + s * 1.0).clamp(-30.0, 30.0); true
        }
        (Stmt::Trigger(TriggerSpec::Tilt { duration, .. }),
                DetailStepperKind::TiltDuration) => {
            // Editor steppers treat `None` as "sticky" and
            // materialize it as a sensible 4-second default
            // when the author first nudges the value.
            let current = duration.unwrap_or(4.0);
            *duration = Some((current + s * 0.25).clamp(0.05, 60.0));
            true
        }
        (Stmt::Trigger(TriggerSpec::SpeedMult { factor, .. }),
                DetailStepperKind::SpeedMultFactor) => {
            *factor = (*factor + s * 0.05).clamp(0.5, 2.0); true
        }
        (Stmt::Trigger(TriggerSpec::SpeedMult { duration, .. }),
                DetailStepperKind::SpeedMultDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.05, 60.0); true
        }
        (Stmt::Trigger(TriggerSpec::HueShift { rate, .. }),
                DetailStepperKind::HueShiftRate) => {
            *rate = (*rate + s * 0.1).clamp(-2.0, 2.0); true
        }
        (Stmt::Trigger(TriggerSpec::HueShift { duration, .. }),
                DetailStepperKind::HueShiftDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.05, 60.0); true
        }
        (Stmt::Trigger(TriggerSpec::Shake { strength, .. }),
                DetailStepperKind::ShakeStrength) => {
            *strength = (*strength + s * 0.05).clamp(0.0, 1.5); true
        }
        (Stmt::Trigger(TriggerSpec::Shake { duration, .. }),
                DetailStepperKind::ShakeDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.05, 10.0); true
        }
        (Stmt::Trigger(TriggerSpec::Glitch { strength, .. }),
                DetailStepperKind::GlitchStrength) => {
            *strength = (*strength + s * 0.05).clamp(0.0, 1.0); true
        }
        (Stmt::Trigger(TriggerSpec::Glitch { duration, .. }),
                DetailStepperKind::GlitchDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.05, 10.0); true
        }
        (Stmt::Trigger(TriggerSpec::Zoom { target, .. }),
                DetailStepperKind::ZoomTarget) => {
            *target = (*target + s * 0.05).clamp(0.25, 3.0); true
        }
        (Stmt::Trigger(TriggerSpec::Zoom { duration, .. }),
                DetailStepperKind::ZoomDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.05, 10.0); true
        }
        (Stmt::Trigger(TriggerSpec::Invert { duration }),
                DetailStepperKind::InvertDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.05, 30.0); true
        }
        (Stmt::Trigger(TriggerSpec::Strobe { rate, .. }),
                DetailStepperKind::StrobeRate) => {
            *rate = (*rate + s * 1.0).clamp(0.0, 40.0); true
        }
        (Stmt::Trigger(TriggerSpec::Strobe { duration, .. }),
                DetailStepperKind::StrobeDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.05, 10.0); true
        }
        // SpeedWarp axes are now Option<f32>. Stepping a
        // previously-absent axis materialises it with the
        // current multiplier + delta, clamped into the valid
        // engine range. The inspector cannot go back to
        // "absent" from the stepper; that remains the parser's
        // job (omit the field in source).
        (Stmt::Trigger(TriggerSpec::SpeedWarp { walls, .. }),
                DetailStepperKind::SpeedwarpWalls) => {
            let current = walls.unwrap_or(1.0);
            *walls = Some((current + s * 0.05).clamp(0.0, 4.0));
            true
        }
        (Stmt::Trigger(TriggerSpec::SpeedWarp { rotation, .. }),
                DetailStepperKind::SpeedwarpRotation) => {
            let current = rotation.unwrap_or(1.0);
            *rotation = Some((current + s * 0.05).clamp(0.0, 4.0));
            true
        }
        (Stmt::Trigger(TriggerSpec::SpeedWarp { cursor, .. }),
                DetailStepperKind::SpeedwarpCursor) => {
            let current = cursor.unwrap_or(1.0);
            *cursor = Some((current + s * 0.05).clamp(0.0, 4.0));
            true
        }
        (Stmt::Trigger(TriggerSpec::SpeedWarp { music_scale, .. }),
                DetailStepperKind::SpeedwarpMusic) => {
            let current = music_scale.unwrap_or(1.0);
            *music_scale = Some((current + s * 0.05).clamp(0.25, 4.0));
            true
        }
        (Stmt::Trigger(TriggerSpec::SpeedWarp { duration, .. }),
                DetailStepperKind::SpeedwarpDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.1, 20.0); true
        }
        (Stmt::Trigger(TriggerSpec::Spin { rate, .. }),
                DetailStepperKind::SpinRate) => {
            *rate = (*rate + s * 0.25).clamp(-8.0, 8.0); true
        }
        (Stmt::Trigger(TriggerSpec::Spin { duration, .. }),
                DetailStepperKind::SpinDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.1, 20.0); true
        }
        (Stmt::Trigger(TriggerSpec::Bounce { amplitude, .. }),
                DetailStepperKind::BounceAmplitude) => {
            *amplitude = (*amplitude + s * 0.05).clamp(0.0, 1.0); true
        }
        (Stmt::Trigger(TriggerSpec::Bounce { duration, .. }),
                DetailStepperKind::BounceDuration) => {
            *duration = (*duration + s * 0.5).clamp(0.1, 30.0); true
        }
        (Stmt::Trigger(TriggerSpec::Freeze { duration }),
                DetailStepperKind::FreezeDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.05, 3.0); true
        }
        (Stmt::Trigger(TriggerSpec::ZoomPunch { strength, .. }),
                DetailStepperKind::ZoomPunchStrength) => {
            *strength = (*strength + s * 0.05).clamp(0.0, 2.0); true
        }
        (Stmt::Trigger(TriggerSpec::ZoomPunch { duration, .. }),
                DetailStepperKind::ZoomPunchDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.05, 3.0); true
        }
        (Stmt::Trigger(TriggerSpec::InvertColors { duration }),
                DetailStepperKind::InvertColorsDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.05, 30.0); true
        }
        (Stmt::Trigger(TriggerSpec::Grayscale { strength, .. }),
                DetailStepperKind::GrayscaleStrength) => {
            *strength = (*strength + s * 0.05).clamp(0.0, 1.0); true
        }
        (Stmt::Trigger(TriggerSpec::Grayscale { duration, .. }),
                DetailStepperKind::GrayscaleDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.05, 30.0); true
        }
        (Stmt::Trigger(TriggerSpec::Shockwave { strength, .. }),
                DetailStepperKind::ShockwaveStrength) => {
            *strength = (*strength + s * 0.05).clamp(0.0, 2.0); true
        }
        (Stmt::Trigger(TriggerSpec::Shockwave { duration, .. }),
                DetailStepperKind::ShockwaveDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.1, 5.0); true
        }
        (Stmt::Trigger(TriggerSpec::Fog { near, .. }),
                DetailStepperKind::FogNear) => {
            *near = (*near + s * 0.05).clamp(0.0, 1.5); true
        }
        (Stmt::Trigger(TriggerSpec::Fog { far, near, .. }),
                DetailStepperKind::FogFar) => {
            let new_far = (*far + s * 0.05).clamp(0.0, 1.5);
            *far = new_far.max(*near + 0.01); true
        }
        (Stmt::Trigger(TriggerSpec::Fog { duration, .. }),
                DetailStepperKind::FogDuration) => {
            *duration = (*duration + s * 0.5).clamp(0.1, 60.0); true
        }
        (Stmt::Trigger(TriggerSpec::Outline { thickness, .. }),
                DetailStepperKind::OutlineThickness) => {
            *thickness = (*thickness + s * 0.05).clamp(0.0, 2.0); true
        }
        (Stmt::Trigger(TriggerSpec::Outline { duration, .. }),
                DetailStepperKind::OutlineDuration) => {
            *duration = (*duration + s * 0.5).clamp(0.1, 60.0); true
        }
        (Stmt::Trigger(TriggerSpec::Centerburst { strength, .. }),
                DetailStepperKind::CenterburstStrength) => {
            *strength = (*strength + s * 0.05).clamp(0.0, 2.0); true
        }
        (Stmt::Trigger(TriggerSpec::Centerburst { duration, .. }),
                DetailStepperKind::CenterburstDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.05, 5.0); true
        }
        (Stmt::Trigger(TriggerSpec::Ringburst { count, .. }),
                DetailStepperKind::RingburstCount) => {
            let n = (*count as i32 + sign).clamp(1, 8);
            *count = n as u32; true
        }
        (Stmt::Trigger(TriggerSpec::Ringburst { duration, .. }),
                DetailStepperKind::RingburstDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.2, 5.0); true
        }
        (Stmt::Trigger(TriggerSpec::Bassdrop { strength, .. }),
                DetailStepperKind::BassdropStrength) => {
            *strength = (*strength + s * 0.05).clamp(0.0, 2.0); true
        }
        (Stmt::Trigger(TriggerSpec::Bassdrop { duration, .. }),
                DetailStepperKind::BassdropDuration) => {
            *duration = (*duration + s * 0.1).clamp(0.1, 5.0); true
        }
        (Stmt::Trigger(TriggerSpec::PostShader { p, .. }),
                DetailStepperKind::PostShaderP0) => {
            p[0] = (p[0] + s * 0.05).clamp(-10.0, 10.0); true
        }
        (Stmt::Trigger(TriggerSpec::PostShader { p, .. }),
                DetailStepperKind::PostShaderP1) => {
            p[1] = (p[1] + s * 0.05).clamp(-10.0, 10.0); true
        }
        (Stmt::Trigger(TriggerSpec::PostShader { p, .. }),
                DetailStepperKind::PostShaderP2) => {
            p[2] = (p[2] + s * 0.05).clamp(-10.0, 10.0); true
        }
        (Stmt::Trigger(TriggerSpec::PostShader { p, .. }),
                DetailStepperKind::PostShaderP3) => {
            p[3] = (p[3] + s * 0.05).clamp(-10.0, 10.0); true
        }
        (Stmt::Trigger(TriggerSpec::Morph { sides, .. }),
                DetailStepperKind::MorphSides) => {
            let n = (*sides as i32 + sign).clamp(3, 12);
            *sides = n as u32; true
        }
        (Stmt::Trigger(TriggerSpec::Morph { duration, .. }),
                DetailStepperKind::MorphDuration) => {
            *duration = (*duration + s * 0.25).clamp(0.1, 10.0); true
        }
        _ => false,
    }
}

fn step_obstacle_thickness(spec: &mut ObstacleSpec, delta: f32) {
    match spec {
        ObstacleSpec::Bar { thickness_mult }
        | ObstacleSpec::DoubleBar { thickness_mult, .. }
        | ObstacleSpec::Spiral { thickness_mult, .. }
        | ObstacleSpec::Alternate { thickness_mult, .. }
        | ObstacleSpec::Rain { thickness_mult, .. }
        | ObstacleSpec::Custom { thickness_mult, .. }
        | ObstacleSpec::Staircase { thickness_mult, .. }
        | ObstacleSpec::CustomFormula { thickness_mult, .. } => {
            *thickness_mult = (*thickness_mult + delta).clamp(0.5, 2.5);
        }
        _ => {}
    }
}

fn toggle_obstacle_dir(spec: &mut ObstacleSpec) {
    match spec {
        ObstacleSpec::Spiral { dir, .. }
        | ObstacleSpec::Pinwheel { dir, .. }
        | ObstacleSpec::Rainbow { dir }
        | ObstacleSpec::Staircase { dir, .. }
        | ObstacleSpec::Corridor { dir, .. }
        | ObstacleSpec::Cubes { dir, .. } => {
            *dir = match dir { SpinDir::Cw => SpinDir::Ccw, _ => SpinDir::Cw };
        }
        _ => {}
    }
}

fn toggle_obstacle_parity(spec: &mut ObstacleSpec) {
    if let ObstacleSpec::Alternate { parity, .. } = spec {
        *parity = match parity { Parity::Even => Parity::Odd, _ => Parity::Even };
    }
}

fn cycle_obstacle(spec: &ObstacleSpec, forward: bool) -> ObstacleSpec {
    let types: &[&str] = &[
        "BAR", "DOUBLEBAR", "SPIRAL", "ALTERNATE", "PINWHEEL",
        "RAIN", "RAINBOW", "LADDER", "TUNNEL", "POT",
        "STAIRCASE", "CORRIDOR", "CUBES",
    ];
    let cur = obstacle_kind_name(spec);
    let i = types.iter().position(|s| *s == cur).unwrap_or(0);
    let n = types.len();
    let next = if forward { (i + 1) % n } else { (i + n - 1) % n };
    obstacle_default_for(types[next])
}

fn cycle_trigger(t: &TriggerSpec, forward: bool) -> TriggerSpec {
    let types: &[&str] = &[
        "FLIP", "PULSE", "TILT", "SPEEDMULT", "HUESHIFT", "SPEEDWARP",
        "GLITCH", "SHAKE", "ZOOM", "INVERT", "STROBE",
        "SPIN", "BOUNCE", "FREEZE", "ZOOMPUNCH", "INVERTCOLORS",
        "GRAYSCALE", "SHOCKWAVE", "FOG", "OUTLINE",
        "CENTERBURST", "RINGBURST", "BASSDROP",
        "POSTSHADER", "POSTSHADEROFF", "MORPH",
    ];
    let cur = trigger_kind_name(t);
    let i = types.iter().position(|s| *s == cur).unwrap_or(0);
    let n = types.len();
    let next = if forward { (i + 1) % n } else { (i + n - 1) % n };
    trigger_default_for(types[next])
}

fn cycle_anim(a: Anim, forward: bool) -> Anim {
    let order = [Anim::Linear, Anim::EaseIn, Anim::EaseOut,
                 Anim::EaseInOut, Anim::Bounce];
    let i = order.iter().position(|x| std::mem::discriminant(x)
        == std::mem::discriminant(&a)).unwrap_or(0);
    let n = order.len();
    let next = if forward { (i + 1) % n } else { (i + n - 1) % n };
    order[next]
}

fn cycle_rule(rs: &RuleSet, forward: bool) -> RuleSet {
    let cats: &[RuleCategory] = &[
        RuleCategory::Ability, RuleCategory::Vision, RuleCategory::Cursor,
        RuleCategory::Survival, RuleCategory::Input, RuleCategory::Score,
    ];
    let cur = rs.category();
    let i = cats.iter().position(|c| *c == cur).unwrap_or(0);
    let n = cats.len();
    let next = if forward { (i + 1) % n } else { (i + n - 1) % n };
    match cats[next] {
        RuleCategory::Ability  => RuleSet::Ability(AbilityRule::default()),
        RuleCategory::Vision   => RuleSet::Vision(VisionRule::default()),
        RuleCategory::Cursor   => RuleSet::Cursor(CursorRule::default()),
        RuleCategory::Survival => RuleSet::Survival(SurvivalRule::default()),
        RuleCategory::Input    => RuleSet::Input(InputRule::default()),
        RuleCategory::Score    => RuleSet::Score(ScoreRule::default()),
        RuleCategory::All      => RuleSet::Vision(VisionRule::default()),
    }
}

fn cycle_rule_category(c: RuleCategory, forward: bool) -> RuleCategory {
    let cats = [RuleCategory::Ability, RuleCategory::Vision,
                RuleCategory::Cursor, RuleCategory::Survival,
                RuleCategory::Input, RuleCategory::Score,
                RuleCategory::All];
    let i = cats.iter().position(|x| *x == c).unwrap_or(0);
    let n = cats.len();
    let next = if forward { (i + 1) % n } else { (i + n - 1) % n };
    cats[next]
}

fn obstacle_default_for(name: &str) -> ObstacleSpec {
    match name {
        "BAR"        => ObstacleSpec::Bar { thickness_mult: 1.0 },
        "DOUBLEBAR"  => ObstacleSpec::DoubleBar { spacing: 2, thickness_mult: 1.0 },
        "SPIRAL"     => ObstacleSpec::Spiral { dir: SpinDir::Cw, thickness_mult: 1.0, loops: 2 },
        "RAIN"       => ObstacleSpec::Rain { count: 5, thickness_mult: 0.85 },
        "ALTERNATE"  => ObstacleSpec::Alternate { parity: Parity::Even, thickness_mult: 1.0 },
        "PINWHEEL"   => ObstacleSpec::Pinwheel { spokes: 3, dir: SpinDir::Cw },
        "LADDER"     => ObstacleSpec::Ladder { rungs: 6 },
        "POT"        => ObstacleSpec::Pot { layers: 4 },
        "TUNNEL"     => ObstacleSpec::Tunnel { length: 1.2, lanes: 3 },
        "RAINBOW"    => ObstacleSpec::Rainbow { dir: SpinDir::Cw },
        "STAIRCASE"  => ObstacleSpec::Staircase { dir: SpinDir::Cw, steps: 6, thickness_mult: 1.0 },
        "CORRIDOR"   => ObstacleSpec::Corridor { length: 1.4, turns: 3, dir: SpinDir::Cw },
        "CUBES"      => ObstacleSpec::Cubes { layers: 4, dir: SpinDir::Cw },
        _            => ObstacleSpec::Bar { thickness_mult: 1.0 },
    }
}

fn trigger_default_for(name: &str) -> TriggerSpec {
    match name {
        "FLIP"      => TriggerSpec::Flip,
        "PULSE"     => TriggerSpec::Pulse,
        "TILT"      => TriggerSpec::Tilt {
            angle: 0.0, pitch: 10.0, yaw: 0.0, duration: None
        },
        "SPEEDMULT" => TriggerSpec::SpeedMult { factor: 1.2, duration: 4.0 },
        "HUESHIFT"  => TriggerSpec::HueShift { rate: 0.5, duration: 6.0 },
        "SPEEDWARP" => TriggerSpec::SpeedWarp {
            walls: Some(1.2), rotation: Some(1.0), cursor: Some(1.0),
            music_scale: Some(1.0), duration: 3.0,
        },
        "GLITCH"    => TriggerSpec::Glitch { strength: 0.5, duration: 0.8 },
        "SHAKE"     => TriggerSpec::Shake  { strength: 0.5, duration: 0.6 },
        "ZOOM"      => TriggerSpec::Zoom { target: 1.2, anim: Anim::EaseOut, duration: 1.0 },
        "INVERT"    => TriggerSpec::Invert { duration: 3.0 },
        "STROBE"    => TriggerSpec::Strobe { rate: 6.0, duration: 0.8 },
        "SPIN"         => TriggerSpec::Spin { rate: 2.0, duration: 3.0 },
        "BOUNCE"       => TriggerSpec::Bounce { amplitude: 0.25, duration: 4.0 },
        "FREEZE"       => TriggerSpec::Freeze { duration: 0.3 },
        "ZOOMPUNCH"    => TriggerSpec::ZoomPunch { strength: 0.35, duration: 0.5 },
        "INVERTCOLORS" => TriggerSpec::InvertColors { duration: 1.5 },
        "GRAYSCALE"    => TriggerSpec::Grayscale { strength: 1.0, duration: 2.0 },
        "SHOCKWAVE"    => TriggerSpec::Shockwave { strength: 0.8, duration: 0.9 },
        "FOG"          => TriggerSpec::Fog { near: 0.25, far: 0.75, duration: 4.0 },
        "OUTLINE"      => TriggerSpec::Outline { thickness: 0.6, duration: 3.0 },
        "CENTERBURST"  => TriggerSpec::Centerburst { strength: 0.7, duration: 0.6 },
        "RINGBURST"    => TriggerSpec::Ringburst { count: 3, duration: 1.2 },
        "BASSDROP"     => TriggerSpec::Bassdrop { strength: 0.9, duration: 1.0 },
        "POSTSHADER"    => TriggerSpec::PostShader { slot: 0, p: [0.0; 4] },
        "POSTSHADEROFF" => TriggerSpec::PostShaderOff,
        "MORPH" => TriggerSpec::Morph { sides: 6, duration: 1.5 },
        _           => TriggerSpec::Flip,
    }
}

// ---------- labels ----------

fn detail_cycle_label_value(
    kind: &DetailCycleKind, stmt: &Stmt, doc: &Document,
) -> (String, String) {
    match (kind, stmt) {
        (DetailCycleKind::ObstacleType, Stmt::Emit(s)) =>
            ("TYPE".into(), obstacle_kind_name(s).into()),
        (DetailCycleKind::TriggerType, Stmt::Trigger(t)) =>
            ("TYPE".into(), trigger_kind_name(t).into()),
        (DetailCycleKind::Dir, Stmt::Emit(spec)) =>
            ("DIR".into(), dir_of(spec).into()),
        (DetailCycleKind::Parity, Stmt::Emit(ObstacleSpec::Alternate { parity, .. })) =>
            ("PARITY".into(), match parity { Parity::Even => "EVEN", _ => "ODD" }.into()),
        (DetailCycleKind::Anim, Stmt::Trigger(TriggerSpec::Zoom { anim, .. })) =>
            ("ANIM".into(), anim_name(*anim).into()),
        (DetailCycleKind::RuleCategory, Stmt::Rule(rs)) =>
            ("CATEGORY".into(), rule_category_label(rs.category()).into()),
        (DetailCycleKind::RuleCategory, Stmt::Revert(c)) =>
            ("CATEGORY".into(), rule_category_label(*c).into()),
        (DetailCycleKind::RuleCategory, Stmt::Push(c)) =>
            ("CATEGORY".into(), rule_category_label(*c).into()),
        (DetailCycleKind::RuleCategory, Stmt::Pop(c)) =>
            ("CATEGORY".into(), rule_category_label(*c).into()),
        (DetailCycleKind::PostShaderSlot,
                        Stmt::Trigger(TriggerSpec::PostShader { slot, .. })) => {
                    let name = doc.ast.shaders.get(*slot as usize)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "NONE".to_string());
                    ("SHADER".into(), name.to_uppercase())
                }
        _ => ("?".into(), "?".into()),
    }
}

fn detail_stepper_label_value(kind: &DetailStepperKind, stmt: &Stmt) -> (String, String) {
    match (kind, stmt) {
        (DetailStepperKind::Thickness, Stmt::Emit(s)) =>
            ("THICKNESS".into(), format!("{:.2}", thickness_of(s))),
        (DetailStepperKind::Spacing, Stmt::Emit(ObstacleSpec::DoubleBar { spacing, .. })) =>
            ("SPACING".into(), format!("{}", spacing)),
        (DetailStepperKind::Loops, Stmt::Emit(ObstacleSpec::Spiral { loops, .. })) =>
            ("LOOPS".into(), format!("{}", loops)),
        (DetailStepperKind::Spokes, Stmt::Emit(ObstacleSpec::Pinwheel { spokes, .. })) =>
            ("SPOKES".into(), format!("{}", spokes)),
        (DetailStepperKind::Count, Stmt::Emit(ObstacleSpec::Rain { count, .. })) =>
            ("COUNT".into(), format!("{}", count)),
        (DetailStepperKind::Rungs, Stmt::Emit(ObstacleSpec::Ladder { rungs })) =>
            ("RUNGS".into(), format!("{}", rungs)),
        (DetailStepperKind::Lanes, Stmt::Emit(ObstacleSpec::Tunnel { lanes, .. })) =>
            ("LANES".into(), format!("{}", lanes)),
        (DetailStepperKind::TunnelLength, Stmt::Emit(ObstacleSpec::Tunnel { length, .. })) =>
            ("LENGTH".into(), format!("{:.2}", length)),
        (DetailStepperKind::CorridorLength, Stmt::Emit(ObstacleSpec::Corridor { length, .. })) =>
            ("LENGTH".into(), format!("{:.2}", length)),
        (DetailStepperKind::Turns, Stmt::Emit(ObstacleSpec::Corridor { turns, .. })) =>
            ("TURNS".into(), format!("{}", turns)),
        (DetailStepperKind::LayersOrSteps, Stmt::Emit(s)) =>
            ("LAYERS/STEPS".into(), format!("{}", layers_of(s))),
        (DetailStepperKind::WaitBeats, Stmt::Wait(n)) =>
            ("BEATS".into(), format!("{}", n)),
        (DetailStepperKind::RepeatCount, Stmt::Repeat { count, .. }) =>
            ("COUNT".into(), format!("{}", count)),
        (DetailStepperKind::TiltAngle,
                Stmt::Trigger(TriggerSpec::Tilt { angle, .. })) =>
            ("ANGLE".into(), format!("{:.1}", angle)),
        (DetailStepperKind::TiltPitch,
                Stmt::Trigger(TriggerSpec::Tilt { pitch, .. })) =>
            ("PITCH".into(), format!("{:.1}", pitch)),
        (DetailStepperKind::TiltYaw,
                Stmt::Trigger(TriggerSpec::Tilt { yaw, .. })) =>
            ("YAW".into(), format!("{:.1}", yaw)),
        (DetailStepperKind::TiltDuration,
                Stmt::Trigger(TriggerSpec::Tilt { duration, .. })) =>
            ("DURATION".into(), match duration {
                Some(d) => format!("{:.2}", d),
                None    => "STICKY".into(),
            }),
        (DetailStepperKind::SpeedMultFactor,
                Stmt::Trigger(TriggerSpec::SpeedMult { factor, .. })) =>
            ("FACTOR".into(), format!("{:.2}", factor)),
        (DetailStepperKind::SpeedMultDuration,
                Stmt::Trigger(TriggerSpec::SpeedMult { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::HueShiftRate,
                Stmt::Trigger(TriggerSpec::HueShift { rate, .. })) =>
            ("RATE".into(), format!("{:.2}", rate)),
        (DetailStepperKind::HueShiftDuration,
                Stmt::Trigger(TriggerSpec::HueShift { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::ShakeStrength,
                Stmt::Trigger(TriggerSpec::Shake { strength, .. })) =>
            ("STRENGTH".into(), format!("{:.2}", strength)),
        (DetailStepperKind::ShakeDuration,
                Stmt::Trigger(TriggerSpec::Shake { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::GlitchStrength,
                Stmt::Trigger(TriggerSpec::Glitch { strength, .. })) =>
            ("STRENGTH".into(), format!("{:.2}", strength)),
        (DetailStepperKind::GlitchDuration,
                Stmt::Trigger(TriggerSpec::Glitch { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::ZoomTarget,
                Stmt::Trigger(TriggerSpec::Zoom { target, .. })) =>
            ("TARGET".into(), format!("{:.2}", target)),
        (DetailStepperKind::ZoomDuration,
                Stmt::Trigger(TriggerSpec::Zoom { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::InvertDuration,
                Stmt::Trigger(TriggerSpec::Invert { duration })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::StrobeRate,
                Stmt::Trigger(TriggerSpec::Strobe { rate, .. })) =>
            ("RATE".into(), format!("{:.1}", rate)),
        (DetailStepperKind::StrobeDuration,
                Stmt::Trigger(TriggerSpec::Strobe { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        // SpeedWarp axes: show the stored multiplier when the
        // axis is enabled, a dash otherwise. The dash reminds
        // the author that the axis is not currently touching
        // the simulation.
        (DetailStepperKind::SpeedwarpWalls,
                Stmt::Trigger(TriggerSpec::SpeedWarp { walls, .. })) =>
            ("WALLS".into(), match walls {
                Some(v) => format!("{:.2}", v),
                None    => "-".into(),
            }),
        (DetailStepperKind::SpeedwarpRotation,
                Stmt::Trigger(TriggerSpec::SpeedWarp { rotation, .. })) =>
            ("ROTATION".into(), match rotation {
                Some(v) => format!("{:.2}", v),
                None    => "-".into(),
            }),
        (DetailStepperKind::SpeedwarpCursor,
                Stmt::Trigger(TriggerSpec::SpeedWarp { cursor, .. })) =>
            ("CURSOR".into(), match cursor {
                Some(v) => format!("{:.2}", v),
                None    => "-".into(),
            }),
        (DetailStepperKind::SpeedwarpMusic,
                Stmt::Trigger(TriggerSpec::SpeedWarp { music_scale, .. })) =>
            ("MUSIC".into(), match music_scale {
                Some(v) => format!("{:.2}", v),
                None    => "-".into(),
            }),
        (DetailStepperKind::SpeedwarpDuration,
                Stmt::Trigger(TriggerSpec::SpeedWarp { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::SpinRate,
                Stmt::Trigger(TriggerSpec::Spin { rate, .. })) =>
            ("RATE".into(), format!("{:.2}", rate)),
        (DetailStepperKind::SpinDuration,
                Stmt::Trigger(TriggerSpec::Spin { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::BounceAmplitude,
                Stmt::Trigger(TriggerSpec::Bounce { amplitude, .. })) =>
            ("AMPLITUDE".into(), format!("{:.2}", amplitude)),
        (DetailStepperKind::BounceDuration,
                Stmt::Trigger(TriggerSpec::Bounce { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::FreezeDuration,
                Stmt::Trigger(TriggerSpec::Freeze { duration })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::ZoomPunchStrength,
                Stmt::Trigger(TriggerSpec::ZoomPunch { strength, .. })) =>
            ("STRENGTH".into(), format!("{:.2}", strength)),
        (DetailStepperKind::ZoomPunchDuration,
                Stmt::Trigger(TriggerSpec::ZoomPunch { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::InvertColorsDuration,
                Stmt::Trigger(TriggerSpec::InvertColors { duration })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::GrayscaleStrength,
                Stmt::Trigger(TriggerSpec::Grayscale { strength, .. })) =>
            ("STRENGTH".into(), format!("{:.2}", strength)),
        (DetailStepperKind::GrayscaleDuration,
                Stmt::Trigger(TriggerSpec::Grayscale { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::ShockwaveStrength,
                Stmt::Trigger(TriggerSpec::Shockwave { strength, .. })) =>
            ("STRENGTH".into(), format!("{:.2}", strength)),
        (DetailStepperKind::ShockwaveDuration,
                Stmt::Trigger(TriggerSpec::Shockwave { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::FogNear,
                Stmt::Trigger(TriggerSpec::Fog { near, .. })) =>
            ("NEAR".into(), format!("{:.2}", near)),
        (DetailStepperKind::FogFar,
                Stmt::Trigger(TriggerSpec::Fog { far, .. })) =>
            ("FAR".into(), format!("{:.2}", far)),
        (DetailStepperKind::FogDuration,
                Stmt::Trigger(TriggerSpec::Fog { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::OutlineThickness,
                Stmt::Trigger(TriggerSpec::Outline { thickness, .. })) =>
            ("THICKNESS".into(), format!("{:.2}", thickness)),
        (DetailStepperKind::OutlineDuration,
                Stmt::Trigger(TriggerSpec::Outline { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::CenterburstStrength,
                Stmt::Trigger(TriggerSpec::Centerburst { strength, .. })) =>
            ("STRENGTH".into(), format!("{:.2}", strength)),
        (DetailStepperKind::CenterburstDuration,
                Stmt::Trigger(TriggerSpec::Centerburst { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::RingburstCount,
                Stmt::Trigger(TriggerSpec::Ringburst { count, .. })) =>
            ("COUNT".into(), format!("{}", count)),
        (DetailStepperKind::RingburstDuration,
                Stmt::Trigger(TriggerSpec::Ringburst { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::BassdropStrength,
                Stmt::Trigger(TriggerSpec::Bassdrop { strength, .. })) =>
            ("STRENGTH".into(), format!("{:.2}", strength)),
        (DetailStepperKind::BassdropDuration,
                Stmt::Trigger(TriggerSpec::Bassdrop { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        (DetailStepperKind::PostShaderP0,
                Stmt::Trigger(TriggerSpec::PostShader { p, .. })) =>
            ("P0".into(), format!("{:.2}", p[0])),
        (DetailStepperKind::PostShaderP1,
                Stmt::Trigger(TriggerSpec::PostShader { p, .. })) =>
            ("P1".into(), format!("{:.2}", p[1])),
        (DetailStepperKind::PostShaderP2,
                Stmt::Trigger(TriggerSpec::PostShader { p, .. })) =>
            ("P2".into(), format!("{:.2}", p[2])),
        (DetailStepperKind::PostShaderP3,
                Stmt::Trigger(TriggerSpec::PostShader { p, .. })) =>
            ("P3".into(), format!("{:.2}", p[3])),
        (DetailStepperKind::MorphSides,
                Stmt::Trigger(TriggerSpec::Morph { sides, .. })) =>
            ("SIDES".into(), format!("{}", sides)),
        (DetailStepperKind::MorphDuration,
                Stmt::Trigger(TriggerSpec::Morph { duration, .. })) =>
            ("DURATION".into(), format!("{:.2}", duration)),
        _ => ("?".into(), "?".into()),
    }
}

fn obstacle_kind_name(s: &ObstacleSpec) -> &'static str {
    match s {
        ObstacleSpec::Bar { .. }           => "BAR",
        ObstacleSpec::DoubleBar { .. }     => "DOUBLEBAR",
        ObstacleSpec::Spiral { .. }        => "SPIRAL",
        ObstacleSpec::Alternate { .. }     => "ALTERNATE",
        ObstacleSpec::Pinwheel { .. }      => "PINWHEEL",
        ObstacleSpec::Rain { .. }          => "RAIN",
        ObstacleSpec::Custom { .. }        => "CUSTOM",
        ObstacleSpec::Rainbow { .. }       => "RAINBOW",
        ObstacleSpec::Ladder { .. }        => "LADDER",
        ObstacleSpec::Tunnel { .. }        => "TUNNEL",
        ObstacleSpec::Pot { .. }           => "POT",
        ObstacleSpec::Staircase { .. }     => "STAIRCASE",
        ObstacleSpec::Corridor { .. }      => "CORRIDOR",
        ObstacleSpec::Cubes { .. }         => "CUBES",
        ObstacleSpec::CustomFormula { .. } => "FORMULA",
    }
}

fn trigger_kind_name(t: &TriggerSpec) -> &'static str {
    match t {
        TriggerSpec::Flip            => "FLIP",
        TriggerSpec::Pulse           => "PULSE",
        TriggerSpec::Tilt { .. }      => "TILT",
        TriggerSpec::SpeedMult { .. } => "SPEEDMULT",
        TriggerSpec::HueShift { .. }  => "HUESHIFT",
        TriggerSpec::SpeedWarp { .. }=> "SPEEDWARP",
        TriggerSpec::Glitch { .. }   => "GLITCH",
        TriggerSpec::Shake { .. }    => "SHAKE",
        TriggerSpec::Zoom { .. }     => "ZOOM",
        TriggerSpec::Invert { .. }   => "INVERT",
        TriggerSpec::Strobe { .. }   => "STROBE",
        TriggerSpec::Spin { .. }         => "SPIN",
        TriggerSpec::Bounce { .. }       => "BOUNCE",
        TriggerSpec::Freeze { .. }       => "FREEZE",
        TriggerSpec::ZoomPunch { .. }    => "ZOOMPUNCH",
        TriggerSpec::InvertColors { .. } => "INVERTCOLORS",
        TriggerSpec::Grayscale { .. }    => "GRAYSCALE",
        TriggerSpec::Shockwave { .. }    => "SHOCKWAVE",
        TriggerSpec::Fog { .. }          => "FOG",
        TriggerSpec::Outline { .. }      => "OUTLINE",
        TriggerSpec::Centerburst { .. }  => "CENTERBURST",
        TriggerSpec::Ringburst { .. }    => "RINGBURST",
        TriggerSpec::Bassdrop { .. }     => "BASSDROP",
        TriggerSpec::PostShader { .. }  => "POSTSHADER",
        TriggerSpec::PostShaderOff      => "POSTSHADEROFF",
        TriggerSpec::Morph { .. }       => "MORPH",
    }
}

fn stmt_label(s: &Stmt) -> String {
    match s {
        Stmt::Wait(n) => format!("WAIT {}", n),
        Stmt::Emit(spec) => format!("EMIT :{}",
            obstacle_kind_name(spec).to_lowercase()),
        Stmt::Trigger(t) => format!("TRIGGER :{}",
            trigger_kind_name(t).to_lowercase()),
        Stmt::Repeat { count, .. } => format!("REPEAT {}", count),
        Stmt::LocalVars(_) => "LOCAL VARS".into(),
        Stmt::Rule(rs) => format!("RULE {}",
            rule_category_label(rs.category())),
        Stmt::Revert(c) => format!("REVERT {}", rule_category_label(*c)),
        Stmt::Push(c) => format!("PUSH {}", rule_category_label(*c)),
        Stmt::Pop(c) => format!("POP {}", rule_category_label(*c)),
    }
}

fn stripe_for(s: &Stmt) -> [f32; 3] {
    match s {
        Stmt::Emit(_) => STRIPE_EMIT,
        Stmt::Trigger(_) => STRIPE_TRIG,
        Stmt::Wait(_) => STRIPE_WAIT,
        Stmt::Repeat { .. } => [0.95, 0.45, 1.00],
        Stmt::Rule(_) | Stmt::Revert(_)
        | Stmt::Push(_) | Stmt::Pop(_) => STRIPE_RULE,
        Stmt::LocalVars(_) => DIM,
    }
}

fn thickness_of(s: &ObstacleSpec) -> f32 {
    match s {
        ObstacleSpec::Bar { thickness_mult }
        | ObstacleSpec::DoubleBar { thickness_mult, .. }
        | ObstacleSpec::Spiral { thickness_mult, .. }
        | ObstacleSpec::Alternate { thickness_mult, .. }
        | ObstacleSpec::Rain { thickness_mult, .. }
        | ObstacleSpec::Custom { thickness_mult, .. }
        | ObstacleSpec::Staircase { thickness_mult, .. }
        | ObstacleSpec::CustomFormula { thickness_mult, .. } => *thickness_mult,
        _ => 1.0,
    }
}

fn layers_of(s: &ObstacleSpec) -> u32 {
    match s {
        ObstacleSpec::Pot { layers } => *layers,
        ObstacleSpec::Staircase { steps, .. } => *steps,
        ObstacleSpec::Cubes { layers, .. } => *layers,
        _ => 0,
    }
}

fn dir_of(s: &ObstacleSpec) -> &'static str {
    let d = match s {
        ObstacleSpec::Spiral { dir, .. }
        | ObstacleSpec::Pinwheel { dir, .. }
        | ObstacleSpec::Rainbow { dir }
        | ObstacleSpec::Staircase { dir, .. }
        | ObstacleSpec::Corridor { dir, .. }
        | ObstacleSpec::Cubes { dir, .. } => Some(*dir),
        _ => None,
    };
    match d {
        Some(SpinDir::Cw) => "CW",
        Some(SpinDir::Ccw) => "CCW",
        None => "-",
    }
}

fn anim_name(a: Anim) -> &'static str {
    match a {
        Anim::Linear => "LINEAR",
        Anim::EaseIn => "EASE_IN",
        Anim::EaseOut => "EASE_OUT",
        Anim::EaseInOut => "EASE_INOUT",
        Anim::Bounce => "BOUNCE",
    }
}

fn rule_category_label(c: RuleCategory) -> &'static str {
    match c {
        RuleCategory::Ability => "ABILITY",
        RuleCategory::Vision => "VISION",
        RuleCategory::Cursor => "CURSOR",
        RuleCategory::Survival => "SURVIVAL",
        RuleCategory::Input => "INPUT",
        RuleCategory::Score => "SCORE",
        RuleCategory::All => "ALL",
    }
}

// ---------- layout helpers ----------

fn shader_row_delete_rect(row: Rect) -> Rect {
    let size = 0.020;
    let margin = 0.003;
    let y0 = row.1 + 0.003;
    let y1 = row.3 - 0.003;
    (row.2 - size - margin, y0, row.2 - margin, y1)
}

fn stepper_button_rects(row: Rect) -> (Rect, Rect) {
    // Bumped margins: kept 0.003 between the two buttons and
    // another 0.003 between the rightmost button and the row
    // edge so the buttons never touch the border of the
    // inspector panel, which previously made the right
    // stepper look half-cut-off at certain zoom levels.
    let size = 0.022;
    let margin = 0.003;
    let inner_gap = 0.003;
    let y0 = row.1 + 0.003;
    let y1 = row.3 - 0.003;
    let plus  = (row.2 - size - margin,
                 y0,
                 row.2 - margin,
                 y1);
    let minus = (plus.0 - size - inner_gap,
                 y0,
                 plus.0 - inner_gap,
                 y1);
    (minus, plus)
}

fn cycle_button_rects(row: Rect) -> (Rect, Rect) {
    stepper_button_rects(row)
}

fn statement_row_hit_areas(row: Rect) -> (Rect, Rect, Rect, Rect) {
    // up, down, del, body. Each button gets a 0.003 gap from
    // its neighbour so hover rings never touch. The body
    // rect is what carries the statement label and acts as
    // the selection hit target.
    let size = 0.020;
    let margin = 0.003;
    let gap = 0.003;
    let y0 = row.1 + 0.003;
    let y1 = row.3 - 0.003;
    let del  = (row.2 - size - margin,
                y0, row.2 - margin, y1);
    let down = (del.0 - size - gap,
                y0, del.0 - gap, y1);
    let up   = (down.0 - size - gap,
                y0, down.0 - gap, y1);
    let body = (row.0, row.1, up.0 - gap, row.3);
    (up, down, del, body)
}

/// Mini button with a drawn arrow glyph instead of a font
/// character. Used for move-up and move-down controls in the
/// statement list because the 4x6 font has no caret glyph
/// and a centered triangle reads more clearly anyway.
fn draw_mini_arrow(
    out: &mut Vec<Vertex>, r: Rect, up: bool,
    pointer: (f32, f32), active_color: [f32; 3],
) {
    let hit = rect_contains(r, pointer);
    let bg = if hit { BG_PANEL_HI } else { BG_PANEL };
    let ring = if hit { active_color } else { DIM };
    push_quad(out, r.0, r.1, r.2, r.3, bg);
    push_outline(out, r.0, r.1, r.2, r.3, 0.001, ring);

    let cx = (r.0 + r.2) * 0.5;
    let cy = (r.1 + r.3) * 0.5;
    let half = (r.3 - r.1) * 0.22;
    if up {
        // Apex pointing up, base at the bottom.
        push_tri(out,
            [cx, cy - half],
            [cx - half, cy + half],
            [cx + half, cy + half],
            WHITE);
    } else {
        // Apex pointing down, base at the top.
        push_tri(out,
            [cx - half, cy - half],
            [cx + half, cy - half],
            [cx, cy + half],
            WHITE);
    }
}

fn draw_mini(
    out: &mut Vec<Vertex>, r: Rect, label: &str,
    pointer: (f32, f32), active_color: [f32; 3],
) {
    let hit = rect_contains(r, pointer);
    let bg = if hit { BG_PANEL_HI } else { BG_PANEL };
    let ring = if hit { active_color } else { DIM };
    push_quad(out, r.0, r.1, r.2, r.3, bg);
    push_outline(out, r.0, r.1, r.2, r.3, 0.001, ring);
    let cx = (r.0 + r.2) * 0.5;
    let cy = (r.1 + r.3) * 0.5;
    let w = sw(label, 0.0040);
    push_small(out, label, cx - w * 0.5,
        cy - sh(0.0040) * 0.5, 0.0040, WHITE);
}

fn rect_contains(r: Rect, p: (f32, f32)) -> bool {
    // See the note in editor::mod::hit_rect: small tolerance
    // to account for outlines drawn outside the rect.
    const TOL: f32 = 0.003;
    p.0 >= r.0 - TOL && p.0 <= r.2 + TOL
        && p.1 >= r.1 - TOL && p.1 <= r.3 + TOL
}

fn fit(text: &str, px: f32, max_w: f32) -> String {
    if max_w <= 0.0 { return String::new(); }
    if sw(text, px) <= max_w { return text.to_string(); }
    let dots = "...";
    let dw = sw(dots, px);
    if dw >= max_w { return String::new(); }
    let chars: Vec<char> = text.chars().collect();
    let mut take = chars.len();
    while take > 0 {
        take -= 1;
        let c: String = chars.iter().take(take).collect();
        if sw(&c, px) + dw <= max_w {
            return format!("{}{}", c, dots);
        }
    }
    String::new()
}