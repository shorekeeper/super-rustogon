//! Embedded level editor.
//!
//! Brings up a graph-driven authoring view on top of the same
//! direct-drawing pipeline used by the menu and the game. The
//! editor lets you:
//!
//! * walk and edit the section timeline as a horizontal node
//!   graph, dragging selection through `[S1]-[S2]-[S3]` nodes,
//! * add or remove sections, shift their `at` value with
//!   `[-]/[+]` steppers,
//! * inspect the active section's `Stmt` list as a vertical
//!   stack of cards; select a card to bring up a typed
//!   inspector with field-level editors,
//! * drop in obstacles and triggers through a 4xN palette
//!   popup which hides the underlying enum width but shows
//!   every variant the v2 grammar can emit,
//! * watch the selection play back live in a small hexagon
//!   viewport in the lower right corner; the preview wraps an
//!   actual `Generator` so changes take effect immediately,
//! * save the entire AST as a v2 `.rlf` file under
//!   `assets/customlevels/<stem>.rlf` via `serialize_v2`.
//!
//! The editor never mutates the in-memory level catalogue; new
//! files only appear after the next catalogue refresh (which
//! today happens at process restart).

mod serialize;
pub use serialize::serialize_v2;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::audio::Audio;
use crate::dsl::ast::*;
use crate::gend::generator::Generator;
use crate::levels::Palette;
use crate::levels::difficulty::Tier;
use crate::pipeline::Vertex;
use crate::text::{push_text, push_text_centered, push_text_right,
                  text_height, text_width};
use crate::ui::draw::{push_quad, push_outline, push_quad_alpha,
                      push_outline_alpha, push_tri, push_hex,
                      push_hex_ring, push_hex_alpha, push_hex_ring_alpha};
use crate::win32::{Input, Mouse};

const TAU: f32 = std::f32::consts::TAU;

// ---- theme ----
const ACCENT:     [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:  [f32; 3] = [1.00, 0.80, 0.92];
const DIM:        [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:      [f32; 3] = [1.00, 1.00, 1.00];
const BG_PANEL:   [f32; 3] = [0.10, 0.04, 0.15];
const BG_PANEL_HI:[f32; 3] = [0.22, 0.10, 0.30];
const BG_DEEP:    [f32; 3] = [0.03, 0.01, 0.06];
const EMIT_C:     [f32; 3] = [0.40, 0.85, 1.00];
const TRIG_C:     [f32; 3] = [1.00, 0.65, 0.20];
const WAIT_C:     [f32; 3] = [0.55, 1.00, 0.40];
const REPEAT_C:   [f32; 3] = [0.95, 0.45, 1.00];

const TITLE_PX: f32 = 0.010;
const H1_PX:    f32 = 0.0070;
const BODY_PX:  f32 = 0.0050;
const SMALL_PX: f32 = 0.0042;

// ---- widget ids ----
type Id = u32;
const ID_BACK:        Id = 0x0001;
const ID_SAVE:        Id = 0x0002;
const ID_NEW_SECTION: Id = 0x0003;
const ID_DEL_SECTION: Id = 0x0004;
const ID_ADD_EMIT:    Id = 0x0005;
const ID_ADD_TRIG:    Id = 0x0006;
const ID_ADD_WAIT:    Id = 0x0007;
const ID_BPM_DOWN:    Id = 0x0008;
const ID_BPM_UP:      Id = 0x0009;
const ID_SIDES_DOWN:  Id = 0x000A;
const ID_SIDES_UP:    Id = 0x000B;
const ID_AT_DOWN:     Id = 0x000C;
const ID_AT_UP:       Id = 0x000D;
const ID_POPUP_X:     Id = 0x000E;
const ID_TRACK_BTN: Id = 0x000F;
const ID_PLAY_BTN:  Id = 0x0010;
const ID_PAUSE_BTN: Id = 0x0011;
const ID_STOP_BTN:  Id = 0x0012;
const ID_SEEK_BAR:  Id = 0x0013;
const ID_PREPLAY: Id = 0x0014;
const ID_IMPORT_BTN: Id = 0x0015;

fn id_import_item(i: usize) -> Id { 0x6000 + i as Id }
fn id_track_item(i: usize) -> Id { 0x5000 + i as Id }

fn id_section(i: usize)      -> Id { 0x1000 + i as Id }
fn id_stmt   (i: usize)      -> Id { 0x2000 + i as Id }
fn id_stmt_x (i: usize)      -> Id { 0x2800 + i as Id }
fn id_popup  (i: usize)      -> Id { 0x3000 + i as Id }
fn id_field  (i: usize)      -> Id { 0x4000 + i as Id }

/// Result of one editor frame, consumed by the main loop.
pub enum EditorOutcome { Stay, Back, PrePlay }

/// Origin of a level being edited. `New` produces a fresh
/// draft, `Existing` clones a catalogue entry's AST so the
/// user can iterate on a stock level without touching the
/// embedded source.
pub enum EditorSource {
    New,
    Existing { stem: String, ast: LevelAst },
}

#[derive(Clone, Copy)]
enum PopupKind { Obstacle, Trigger, Track, Import }

pub struct Editor {
    file_stem: String,
    ast: LevelAst,

    selected_section: usize,
    selected_stmt:    Option<usize>,

    view_aspect: f32,
    pointer: (f32, f32),
    left_was_down: bool,
    escape_was_down: bool,

    hover: HashMap<Id, f32>,
    hover_target: HashMap<Id, bool>,

    time: f32,
    popup: Option<PopupKind>,
    level_list: Vec<LevelEntry>,
    status: String,
    status_timer: f32,

    track_list: Vec<MusicEntry>,
    seeking: bool,
    dragging_section: Option<(usize, f32)>,
    preview: Preview,
}

struct Preview {
    genz:   Option<Generator>,
    walls: Vec<PreviewWall>,
    time:  f32,
    cycle: f32,
    sides: u32,
}

#[derive(Clone, Copy)]
struct PreviewWall {
    slot: u32,
    distance: f32,
    thickness: f32,
}

impl Editor {
    pub fn new(source: EditorSource) -> Self {
        let (file_stem, ast) = match source {
            EditorSource::New => (
                format!("draft_{}", std::process::id()),
                default_level(),
            ),
            EditorSource::Existing { stem, ast } => (stem, ast),
        };
        let sides = ast.generation.sides;
        let mut e = Editor {
            file_stem,
            ast,
            selected_section: 0,
            selected_stmt: None,
            view_aspect: 16.0 / 9.0,
            pointer: (0.0, 0.0),
            left_was_down: false,
            escape_was_down: false,
            hover: HashMap::new(),
            hover_target: HashMap::new(),
            time: 0.0,
            popup: None,
            status: "READY".into(),
            status_timer: 2.0,
            level_list: Vec::new(),
            track_list: scan_tracks(),
            seeking: false,
            dragging_section: None,
            preview: Preview {
                genz: None, walls: Vec::new(),
                time: 0.0, cycle: 8.0, sides,
            },
        };
        e.rebuild_preview();
        e
    }

    pub fn update(
        &mut self, dt: f32, mouse: Mouse, input: Input,
        cw: u32, ch: u32, audio: &Audio,
    ) -> EditorOutcome {
        self.time += dt;
        self.view_aspect = (cw.max(1) as f32) / (ch.max(1) as f32);
        if self.status_timer > 0.0 { self.status_timer -= dt; }

        let (sx, sy) = crate::renderer::aspect_scale(cw, ch);
        let cwf = cw.max(1) as f32;
        let chf = ch.max(1) as f32;
        let nx = ((mouse.x as f32 / cwf) * 2.0 - 1.0) / sx;
        let ny = ((mouse.y as f32 / chf) * 2.0 - 1.0) / sy;
        self.pointer = (nx, ny);

        let clicked = mouse.left_down && !self.left_was_down;
        self.left_was_down = mouse.left_down;
        let esc_edge = input.escape && !self.escape_was_down;
        self.escape_was_down = input.escape;

        // Hard reset of pointer drag states every time the
        // mouse is up. Done unconditionally (not gated on any
        // `self.*ing`) so a frame where the button was released
        // between polls cannot leave a stale drag alive into
        // the next click.
        if !mouse.left_down {
            self.seeking = false;
            // Finalize any in-flight section drag: re-sort the
            // sections by their new `at` values and track where
            // the dragged one ended up so the selection still
            // points at the same body.
            if let Some((idx, _)) = self.dragging_section.take() {
                if idx < self.ast.sections.len() {
                    let moved = self.ast.sections.remove(idx);
                    let moved_at = moved.at;
                    let new_idx = self.ast.sections.iter()
                        .position(|s| s.at > moved_at)
                        .unwrap_or(self.ast.sections.len());
                    self.ast.sections.insert(new_idx, moved);
                    self.selected_section = new_idx;
                    self.rebuild_preview();
                }
            }
        }

        self.hover_target.clear();
        self.preview.sides = self.ast.generation.sides;
        self.tick_preview(dt);

        let mut outcome = EditorOutcome::Stay;
        if esc_edge {
            if self.popup.is_some() {
                self.popup = None;
                self.seeking = false;
                self.dragging_section = None;
            } else {
                return EditorOutcome::Back;
            }
        }

        if let Some(popup) = self.popup {
            self.seeking = false;
            self.dragging_section = None;
            self.tick_popup(popup, nx, ny, clicked, audio);
        } else {
            outcome = self.tick_main(nx, ny, clicked, mouse, audio);
        }

        let blend = 1.0 - (-14.0 * dt).exp();
        let keys: Vec<Id> = self.hover.keys().copied().collect();
        for id in keys {
            let target = if *self.hover_target.get(&id).unwrap_or(&false) { 1.0 } else { 0.0 };
            let v = self.hover.get_mut(&id).unwrap();
            *v += (target - *v) * blend;
            if *v < 0.001 && target == 0.0 { self.hover.remove(&id); }
        }
        for (&id, &t) in &self.hover_target {
            if t && !self.hover.contains_key(&id) { self.hover.insert(id, 0.0); }
        }

        outcome
    }

    /// Path the editor wants the audio worker to play. The main
    /// loop overrides `menu.desired_music()` with this while in
    /// `AppState::Editor` so picking a track in the dropdown
    /// instantly switches what is streaming.
    pub fn desired_music(&self) -> Option<String> {
        if self.ast.meta.music.is_empty() { None }
        else { Some(self.ast.meta.music.clone()) }
    }

    /// Hand a freshly cloned AST to the main loop so it can
    /// build a playable [`crate::levels::Level`] without
    /// borrowing from the editor's internal state.
    pub fn ast_clone(&self) -> LevelAst { self.ast.clone() }

    // ---------- main UI ----------

    fn tick_main(
        &mut self, nx: f32, ny: f32, clicked: bool,
        mouse: Mouse, audio: &Audio,
    ) -> EditorOutcome {
        // Top bar buttons.
        let r = self.back_btn();
        if self.btn_hit(ID_BACK, r, nx, ny, clicked) {
            audio.play_interact();
            return EditorOutcome::Back;
        }
        let r = self.save_btn();
        if self.btn_hit(ID_SAVE, r, nx, ny, clicked) {
            audio.play_interact();
            self.save();
        }
        let r = self.import_btn();
        if self.btn_hit(ID_IMPORT_BTN, r, nx, ny, clicked) {
            // Refresh the listing every time the dropdown
            // opens so freshly dropped .rlf files show up
            // without having to restart the whole game.
            self.level_list = scan_levels();
            self.popup = Some(PopupKind::Import);
            audio.play_interact();
        }
        let r = self.preplay_btn();
        if self.btn_hit(ID_PREPLAY, r, nx, ny, clicked) {
            // Belt-and-suspenders: auto-save the draft before
            // handing control to the play-test. Even if the
            // author crashes the game or force-quits during
            // the test, the latest edits survive on disk.
            self.save();
            audio.play_enter();
            return EditorOutcome::PrePlay;
        }

        // BPM cluster.
        let bpm_cx = self.bpm_cx();
        let cy_clu = self.cluster_cy();
        if self.stepper(ID_BPM_DOWN, bpm_cx - 0.07, cy_clu, nx, ny, clicked) {
            self.ast.meta.bpm = (self.ast.meta.bpm.saturating_sub(5)).max(40);
            self.rebuild_preview(); audio.play_interact();
        }
        if self.stepper(ID_BPM_UP,   bpm_cx + 0.07, cy_clu, nx, ny, clicked) {
            self.ast.meta.bpm = (self.ast.meta.bpm + 5).min(300);
            self.rebuild_preview(); audio.play_interact();
        }

        // SIDES cluster.
        let sides_cx = self.sides_cx();
        if self.stepper(ID_SIDES_DOWN, sides_cx - 0.07, cy_clu, nx, ny, clicked) {
            self.ast.generation.sides = (self.ast.generation.sides.saturating_sub(1)).max(3);
            self.rebuild_preview(); audio.play_interact();
        }
        if self.stepper(ID_SIDES_UP,   sides_cx + 0.07, cy_clu, nx, ny, clicked) {
            self.ast.generation.sides = (self.ast.generation.sides + 1).min(12);
            self.rebuild_preview(); audio.play_interact();
        }

        // Music row: track picker + transport.
        let r = self.track_btn_rect();
        if self.btn_hit(ID_TRACK_BTN, r, nx, ny, clicked) {
            self.popup = Some(PopupKind::Track);
            audio.play_interact();
        }
        let r = self.play_btn_rect();
        if self.btn_hit(ID_PLAY_BTN, r, nx, ny, clicked) {
            if !self.ast.meta.music.is_empty() {
                if audio.has_music() && audio.is_music_paused() {
                    audio.resume_music();
                } else {
                    audio.restart_music(&self.ast.meta.music);
                }
            }
            audio.play_interact();
        }
        let r = self.pause_btn_rect();
        if self.btn_hit(ID_PAUSE_BTN, r, nx, ny, clicked) {
            audio.pause_music();
            audio.play_interact();
        }
        let r = self.stop_btn_rect();
        if self.btn_hit(ID_STOP_BTN, r, nx, ny, clicked) {
            audio.stop_music();
            audio.play_interact();
        }

// ---- Section nodes and drag handling.
        //
        // Clicking a node selects that section and starts a
        // potential drag. While the button is held, the node's
        // `at` tracks the pointer (with an offset so the grab
        // point stays under the cursor). The drag is finalized
        // on mouse-up in `update`, which also re-sorts the
        // sections by `at`.
        let n = self.ast.sections.len();
        let mut node_was_clicked = false;

        for i in 0..n {
            let r = self.section_node(i);
            let h = self.rect_hit(r, nx, ny);
            self.set_hover(id_section(i), h);
            if h && clicked {
                self.selected_section = i;
                self.selected_stmt = None;
                if audio.has_music() {
                    let dur = audio.music_duration();
                    if dur > 0.1 {
                        audio.seek_music(self.ast.sections[i].at * dur);
                    }
                }
                // Begin drag. Capture the offset between the
                // node's current `at` and the pointer's `at` so
                // a click on the left edge of a wide node does
                // not snap the node center under the cursor.
                let x0 = self.timeline_x0();
                let x1 = self.timeline_x1();
                let pointer_at = ((nx - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let offset = self.ast.sections[i].at - pointer_at;
                self.dragging_section = Some((i, offset));

                self.rebuild_preview();
                audio.play_interact();
                node_was_clicked = true;
            }
        }

        // Update the dragged node's `at` each frame it remains
        // held. Audio is scrubbed in lockstep so the author
        // can hear where the section will land. Sorting is
        // deferred until mouse-up to keep the index stable for
        // the duration of the gesture.
        if let Some((idx, offset)) = self.dragging_section {
            if mouse.left_down && idx < self.ast.sections.len() {
                let x0 = self.timeline_x0();
                let x1 = self.timeline_x1();
                let pointer_at = ((nx - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let new_at = (pointer_at + offset).clamp(0.0, 1.0);
                self.ast.sections[idx].at = new_at;
                if audio.has_music() {
                    let dur = audio.music_duration();
                    if dur > 0.1 { audio.seek_music(new_at * dur); }
                }
                node_was_clicked = true;
            }
        }

        let r = self.new_section_node();
        if self.btn_hit(ID_NEW_SECTION, r, nx, ny, clicked) {
            self.add_section();
            audio.play_interact();
            node_was_clicked = true;
        }

        // ---- Music seek bar.
        //
        // Strict rules:
        //
        //  * The drag is only ever STARTED when the click lands
        //    inside the bar's drawn rectangle and the section
        //    node row did not consume the click first.
        //  * A click landing OUTSIDE the bar terminates any
        //    drag immediately, so a stray earlier scrub cannot
        //    leak into an unrelated click on, say, the
        //    inspector or a stmt row.
        //  * Drag CONTINUES wherever the cursor goes as long
        //    as the button stays pressed, which is the
        //    expected behaviour of every desktop slider.
        //  * Drag ENDS the moment the mouse comes up, handled
        //    unconditionally at the top of `update`.
        let bar = self.timeline_bar_rect();
        let bar_hit = self.rect_hit(bar, nx, ny);
        self.set_hover(ID_SEEK_BAR, bar_hit);

        if clicked && !bar_hit {
            // Click happened on something other than the bar:
            // treat as an explicit cancellation of any prior
            // seek session.
            self.seeking = false;
        }
        if bar_hit && clicked && !node_was_clicked && audio.has_music() {
            self.seeking = true;
        }
        if self.seeking {
            if mouse.left_down && audio.has_music() {
                let dur = audio.music_duration();
                if dur > 0.1 {
                    let t = ((nx - bar.0) / (bar.2 - bar.0)).clamp(0.0, 1.0);
                    audio.seek_music(t * dur);
                }
            } else {
                self.seeking = false;
            }
        }

        // Section header controls.
        if !self.ast.sections.is_empty() {
            let cy = self.section_header_y();
            let at_cx = self.section_at_cx();
            if self.stepper(ID_AT_DOWN, at_cx - 0.07, cy, nx, ny, clicked) {
                let s = &mut self.ast.sections[self.selected_section];
                s.at = (s.at - 0.05).max(0.0);
                audio.play_interact();
            }
            if self.stepper(ID_AT_UP,   at_cx + 0.07, cy, nx, ny, clicked) {
                let s = &mut self.ast.sections[self.selected_section];
                s.at = (s.at + 0.05).min(1.0);
                audio.play_interact();
            }
            let r = self.del_section_btn();
            if self.btn_hit(ID_DEL_SECTION, r, nx, ny, clicked) && n > 1 {
                self.delete_section();
                audio.play_interact();
            }
        }

        // Statements.
        if let Some(sec) = self.ast.sections.get(self.selected_section) {
            for i in 0..sec.body.len() {
                let r = self.stmt_row(i);
                let h = self.rect_hit(r, nx, ny);
                self.set_hover(id_stmt(i), h);
                if h && clicked {
                    self.selected_stmt = Some(i);
                    audio.play_interact();
                }
                let xr = self.stmt_x_btn(i);
                if self.btn_hit(id_stmt_x(i), xr, nx, ny, clicked) {
                    self.delete_stmt(i);
                    audio.play_interact();
                    break;
                }
            }
        }

        // Add buttons.
        let r = self.add_btn(0);
        if self.btn_hit(ID_ADD_EMIT, r, nx, ny, clicked) {
            self.popup = Some(PopupKind::Obstacle);
            audio.play_interact();
        }
        let r = self.add_btn(1);
        if self.btn_hit(ID_ADD_TRIG, r, nx, ny, clicked) {
            self.popup = Some(PopupKind::Trigger);
            audio.play_interact();
        }
        let r = self.add_btn(2);
        if self.btn_hit(ID_ADD_WAIT, r, nx, ny, clicked) {
            self.add_stmt(Stmt::Wait(2));
            audio.play_interact();
        }

        self.tick_inspector(nx, ny, clicked, audio);
        EditorOutcome::Stay
    }

    fn tick_inspector(&mut self, nx: f32, ny: f32, clicked: bool, audio: &Audio) {
        let Some(idx) = self.selected_stmt else { return; };
        let sec_idx = self.selected_section;
        let stmt = match self.ast.sections.get(sec_idx).and_then(|s| s.body.get(idx)) {
            Some(s) => s.clone(),
            None => return,
        };
        let fields = inspector_fields(&stmt);
        for (k, f) in fields.iter().enumerate() {
            let r = self.inspector_field_rect(k);
            let cx = (r.0 + r.2) * 0.5;
            let cy = (r.1 + r.3) * 0.5;
            match f.kind {
                InsKind::Stepper { .. } => {
                    if self.stepper(id_field(k * 2), cx - 0.10, cy, nx, ny, clicked) {
                        self.apply_inspector_delta(idx, k, -1.0);
                        audio.play_interact();
                    }
                    if self.stepper(id_field(k * 2 + 1), cx + 0.10, cy, nx, ny, clicked) {
                        self.apply_inspector_delta(idx, k, 1.0);
                        audio.play_interact();
                    }
                }
                InsKind::Cycle { .. } => {
                    let cyc = (cx - 0.06, cy - 0.025, cx + 0.06, cy + 0.025);
                    let h = self.rect_hit(cyc, nx, ny);
                    self.set_hover(id_field(k * 2), h);
                    if h && clicked {
                        self.apply_inspector_cycle(idx, k);
                        audio.play_interact();
                    }
                }
            }
        }
    }

    fn tick_popup(&mut self, kind: PopupKind, nx: f32, ny: f32, clicked: bool, audio: &Audio) {
        let r = self.popup_close_btn();
        if self.btn_hit(ID_POPUP_X, r, nx, ny, clicked) {
            self.popup = None;
            audio.play_interact();
            return;
        }
        match kind {
            PopupKind::Obstacle | PopupKind::Trigger => {
                let entries = if matches!(kind, PopupKind::Obstacle) {
                    OBSTACLE_TYPES
                } else { TRIGGER_TYPES };
                for (i, label) in entries.iter().enumerate() {
                    let r = self.popup_btn(i);
                    let h = self.rect_hit(r, nx, ny);
                    self.set_hover(id_popup(i), h);
                    if h && clicked {
                        match kind {
                            PopupKind::Obstacle => self.add_stmt(Stmt::Emit(default_obstacle(label))),
                            PopupKind::Trigger  => self.add_stmt(Stmt::Trigger(default_trigger(label))),
                            _ => {}
                        }
                        self.popup = None;
                        audio.play_interact();
                        return;
                    }
                }
            }

            PopupKind::Import => {
                let visible = self.level_list.len().min(12);
                for i in 0..visible {
                    let r = self.track_popup_btn(i);
                    let h = self.rect_hit(r, nx, ny);
                    self.set_hover(id_import_item(i), h);
                    if h && clicked {
                        let entry = &self.level_list[i];
                        let path    = entry.path.clone();
                        let display = entry.display.clone();
                        match read_level_file(&path) {
                            Some(text) => match crate::dsl::parse_level(&text) {
                                Ok(ast) => {
                                    self.ast = ast;
                                    // Strip the .rlf suffix; if
                                    // that leaves an empty stem,
                                    // fall back to a safe
                                    // default so SAVE does not
                                    // write to a hidden file.
                                    let stem = display.strip_suffix(".rlf")
                                        .map(|s| s.to_string())
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or_else(|| "imported".to_string());
                                    self.file_stem = stem;
                                    self.selected_section = 0;
                                    self.selected_stmt = None;
                                    self.dragging_section = None;
                                    self.rebuild_preview();
                                    self.status = format!("IMPORTED: {}", display);
                                    self.status_timer = 4.0;
                                }
                                Err(e) => {
                                    self.status = format!("PARSE ERROR: {}", e);
                                    self.status_timer = 6.0;
                                }
                            },
                            None => {
                                self.status = format!("READ FAILED: {}", display);
                                self.status_timer = 6.0;
                            }
                        }
                        self.popup = None;
                        audio.play_interact();
                        return;
                    }
                }
            }

            PopupKind::Track => {
                // Cap at 12 entries so the popup never grows past
                // its panel. Authors with deeper folders should
                // organise into subfolders or curate the
                // `customlevels/songs/` directory directly.
                let visible = self.track_list.len().min(12);
                for i in 0..visible {
                    let r = self.track_popup_btn(i);
                    let h = self.rect_hit(r, nx, ny);
                    self.set_hover(id_track_item(i), h);
                    if h && clicked {
                        let entry = &self.track_list[i];
                        self.ast.meta.music = entry.path.clone();
                        // The main loop's desired_music diff
                        // will pick this up next frame and
                        // route it to the audio worker.
                        self.popup = None;
                        audio.play_interact();
                        return;
                    }
                }
            }
        }
    }

    // ---------- inspector apply ----------

    fn apply_inspector_delta(&mut self, idx: usize, k: usize, sign: f32) {
        let sec_idx = self.selected_section;
        let Some(sec) = self.ast.sections.get_mut(sec_idx) else { return; };
        let Some(stmt) = sec.body.get_mut(idx) else { return; };
        apply_field_delta(stmt, k, sign);
    }

    fn apply_inspector_cycle(&mut self, idx: usize, k: usize) {
        let sec_idx = self.selected_section;
        let Some(sec) = self.ast.sections.get_mut(sec_idx) else { return; };
        let Some(stmt) = sec.body.get_mut(idx) else { return; };
        apply_field_cycle(stmt, k);
    }

    // ---------- preview ----------

    fn rebuild_preview(&mut self) {
        let sec = match self.ast.sections.get(self.selected_section) {
            Some(s) => s.clone(),
            None    => return,
        };
        let mut single = self.ast.clone();
        single.sections = vec![Section {
            name: sec.name, at: 0.0, body: sec.body,
        }];
        let bs = 60.0 / self.ast.meta.bpm.max(40) as f32;
        self.preview.genz = Some(Generator::new(single, 1.0, bs));
        self.preview.walls.clear();
        self.preview.time = 0.0;
    }

    fn tick_preview(&mut self, dt: f32) {
        if let Some(g) = self.preview.genz.as_mut() {
            g.update(dt, 0.0, 1.0);
            let walls = &mut self.preview.walls;
            g.drain_walls(|slot, thick, _len| {
                walls.push(PreviewWall {
                    slot, distance: 1.0, thickness: 0.04 * thick,
                });
            });
            for w in walls.iter_mut() { w.distance -= 0.30 * dt; }
            walls.retain(|w| w.distance > -0.02);
        }
        self.preview.time += dt;
        if self.preview.time >= self.preview.cycle {
            self.rebuild_preview();
        }
    }

    // ---------- mutations ----------

    fn add_section(&mut self) {
        // New section is placed midway between the currently
        // selected section and its successor (or between the
        // last section and the end of the track when the
        // selection is already at the tail). Falls back to
        // 0.0 when the list is empty. This gives authors a
        // sensible starting point that they can then drag
        // anywhere, rather than stacking every new node on the
        // same rigid fraction.
        let sel = self.selected_section;
        let next_at = if self.ast.sections.is_empty() {
            0.0
        } else if sel + 1 < self.ast.sections.len() {
            let a = self.ast.sections[sel].at;
            let b = self.ast.sections[sel + 1].at;
            (a + b) * 0.5
        } else {
            let a = self.ast.sections.last().map(|s| s.at).unwrap_or(0.0);
            (a + 1.0) * 0.5
        }.clamp(0.0, 1.0);

        let n = self.ast.sections.len();
        let new_section = Section {
            name: format!("sec_{}", n + 1),
            at: next_at,
            body: Vec::new(),
        };

        let insert_pos = self.ast.sections.iter()
            .position(|s| s.at > next_at)
            .unwrap_or(n);
        self.ast.sections.insert(insert_pos, new_section);
        self.selected_section = insert_pos;
        self.selected_stmt = None;
        self.rebuild_preview();
    }

    fn delete_section(&mut self) {
        if self.ast.sections.len() <= 1 { return; }
        self.ast.sections.remove(self.selected_section);
        if self.selected_section >= self.ast.sections.len() {
            self.selected_section = self.ast.sections.len() - 1;
        }
        self.selected_stmt = None;
        self.rebuild_preview();
    }

    fn add_stmt(&mut self, st: Stmt) {
        let i = self.selected_section;
        if let Some(sec) = self.ast.sections.get_mut(i) {
            sec.body.push(st);
            self.selected_stmt = Some(sec.body.len() - 1);
            self.rebuild_preview();
        }
    }

    fn delete_stmt(&mut self, idx: usize) {
        let i = self.selected_section;
        if let Some(sec) = self.ast.sections.get_mut(i) {
            if idx < sec.body.len() {
                sec.body.remove(idx);
                self.selected_stmt = None;
                self.rebuild_preview();
            }
        }
    }

    fn save(&mut self) {
        let text = serialize_v2(&self.ast);
        let path = save_path(&self.file_stem);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::write(&path, text) {
            Ok(_) => {
                self.status = format!("SAVED: {}", path.display());
                self.status_timer = 4.0;
            }
            Err(e) => {
                self.status = format!("SAVE FAILED: {}", e);
                self.status_timer = 6.0;
            }
        }
    }

    // ---------- view bounds ----------

    fn view_left  (&self) -> f32 { if self.view_aspect >= 1.0 { -self.view_aspect } else { -1.0 } }
    fn view_right (&self) -> f32 { -self.view_left() }
    fn view_top   (&self) -> f32 { if self.view_aspect >= 1.0 { -1.0 } else { -1.0 / self.view_aspect } }
    fn view_bottom(&self) -> f32 { -self.view_top() }

    // ---------- rects ----------
    //
    // Layout zones, top to bottom:
    //
    //   0.00 ........ 0.16   top bar (BACK / SAVE / title / BPM / SIDES)
    //   0.21              .  music row (track picker + transport + clock)
    //   0.30              .  section node row (graph nodes)
    //   0.38              .  music timeline bar (progress + scrubber)
    //   0.50              .  section header (label / AT / count / DELETE)
    //   0.57 .. bot-0.32    statement list
    //   bot-0.32 .. bot     add buttons + inspector

    fn back_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.11;
        let cy = self.view_top() + 0.075;
        (cx - 0.075, cy - 0.030, cx + 0.075, cy + 0.030)
    }
    fn save_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.28;
        let cy = self.view_top() + 0.075;
        (cx - 0.075, cy - 0.030, cx + 0.075, cy + 0.030)
    }
    fn import_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.45;
        let cy = self.view_top() + 0.075;
        (cx - 0.075, cy - 0.030, cx + 0.075, cy + 0.030)
    }
    fn preplay_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.64;
        let cy = self.view_top() + 0.075;
        (cx - 0.085, cy - 0.030, cx + 0.085, cy + 0.030)
    }

    fn bpm_cx(&self)     -> f32 { self.view_right() - 0.42 }
    fn sides_cx(&self)   -> f32 { self.view_right() - 0.13 }
    fn cluster_cy(&self) -> f32 { self.view_top() + 0.080 }

    // ---- music row ----

    fn music_row_y(&self) -> f32 { self.view_top() + 0.21 }

    fn track_btn_rect(&self) -> (f32, f32, f32, f32) {
        let cy = self.music_row_y();
        let x0 = self.view_left() + 0.04;
        (x0, cy - 0.030, x0 + 0.50, cy + 0.030)
    }

    fn transport_x0(&self) -> f32 { self.view_left() + 0.58 }
    fn transport_btn(&self, i: usize) -> (f32, f32, f32, f32) {
        let cy = self.music_row_y();
        let cx = self.transport_x0() + i as f32 * 0.075;
        (cx - 0.030, cy - 0.025, cx + 0.030, cy + 0.025)
    }
    fn play_btn_rect (&self) -> (f32, f32, f32, f32) { self.transport_btn(0) }
    fn pause_btn_rect(&self) -> (f32, f32, f32, f32) { self.transport_btn(1) }
    fn stop_btn_rect (&self) -> (f32, f32, f32, f32) { self.transport_btn(2) }

    fn time_label_pos(&self) -> (f32, f32) {
        let cy = self.music_row_y();
        (self.transport_x0() + 0.27, cy - text_height(BODY_PX) * 0.5)
    }

    // ---- combined music + section timeline ----

    fn nodes_y(&self) -> f32 { self.view_top() + 0.30 }
    fn bar_y(&self)   -> f32 { self.view_top() + 0.38 }

    fn timeline_x0(&self) -> f32 { self.view_left()  + 0.06 }
    fn timeline_x1(&self) -> f32 { self.view_right() - 0.66 }

    fn timeline_bar_rect(&self) -> (f32, f32, f32, f32) {
        let y = self.bar_y();
        (self.timeline_x0(), y - 0.014, self.timeline_x1(), y + 0.014)
    }

    fn section_node(&self, i: usize) -> (f32, f32, f32, f32) {
        let s = &self.ast.sections[i];
        let x0 = self.timeline_x0();
        let x1 = self.timeline_x1();
        let cx = x0 + (x1 - x0) * s.at.clamp(0.0, 1.0);
        let cy = self.nodes_y();
        (cx - 0.055, cy - 0.038, cx + 0.055, cy + 0.038)
    }
    fn new_section_node(&self) -> (f32, f32, f32, f32) {
        let cx = self.timeline_x1() + 0.05;
        let cy = self.nodes_y();
        (cx - 0.038, cy - 0.038, cx + 0.038, cy + 0.038)
    }

    // ---- section header ----

    fn section_header_y(&self) -> f32 { self.view_top() + 0.50 }
    fn section_label_x(&self)  -> f32 { self.view_left() + 0.04 }
    fn section_at_cx (&self)   -> f32 { self.view_left() + 0.95 }
    fn section_count_x(&self)  -> f32 { self.view_left() + 1.30 }

    fn del_section_btn(&self) -> (f32, f32, f32, f32) {
        let cy = self.section_header_y();
        let cx = self.view_right() - 0.14;
        (cx - 0.105, cy - 0.028, cx + 0.105, cy + 0.028)
    }

    fn stmt_list_y0(&self) -> f32 { self.view_top() + 0.57 }
    fn stmt_row_h (&self) -> f32 { 0.060 }

    fn stmt_row(&self, i: usize) -> (f32, f32, f32, f32) {
        let y0 = self.stmt_list_y0() + i as f32 * self.stmt_row_h();
        let x0 = self.view_left() + 0.04;
        let x1 = self.view_left() + 1.30;
        (x0, y0, x1, y0 + self.stmt_row_h() - 0.010)
    }
    fn stmt_x_btn(&self, i: usize) -> (f32, f32, f32, f32) {
        let r = self.stmt_row(i);
        (r.2 - 0.045, r.1 + 0.005, r.2 - 0.005, r.3 - 0.005)
    }

    fn add_btn(&self, k: usize) -> (f32, f32, f32, f32) {
        let cy = self.view_bottom() - 0.32;
        let cx0 = self.view_left() + 0.18;
        let cx = cx0 + k as f32 * 0.30;
        (cx - 0.13, cy - 0.030, cx + 0.13, cy + 0.030)
    }

    fn inspector_y0(&self) -> f32 { self.view_bottom() - 0.24 }
    fn inspector_field_rect(&self, k: usize) -> (f32, f32, f32, f32) {
        let row = k / 2;
        let col = k % 2;
        let y0 = self.inspector_y0() + row as f32 * 0.060;
        let x0 = self.view_left() + 0.04 + col as f32 * 0.65;
        (x0, y0, x0 + 0.62, y0 + 0.050)
    }

    fn preview_rect(&self) -> (f32, f32, f32, f32) {
        let x1 = self.view_right() - 0.05;
        let y1 = self.view_bottom() - 0.05;
        (x1 - 0.60, self.view_top() + 0.55, x1, y1)
    }

    fn popup_panel(&self) -> (f32, f32, f32, f32) {
        (-0.95, -0.55, 0.95, 0.55)
    }
    fn popup_btn(&self, i: usize) -> (f32, f32, f32, f32) {
        let cols = 4usize;
        let row = i / cols;
        let col = i % cols;
        let p = self.popup_panel();
        let cell_w = (p.2 - p.0 - 0.08) / cols as f32;
        let cell_h = 0.080;
        let x0 = p.0 + 0.04 + col as f32 * cell_w;
        let y0 = p.1 + 0.16 + row as f32 * (cell_h + 0.012);
        (x0 + 0.012, y0, x0 + cell_w - 0.012, y0 + cell_h)
    }
    fn track_popup_btn(&self, i: usize) -> (f32, f32, f32, f32) {
        let p = self.popup_panel();
        let h = 0.050;
        let y0 = p.1 + 0.16 + i as f32 * (h + 0.008);
        (p.0 + 0.04, y0, p.2 - 0.04, y0 + h)
    }
    fn popup_close_btn(&self) -> (f32, f32, f32, f32) {
        let p = self.popup_panel();
        (p.2 - 0.18, p.1 + 0.04, p.2 - 0.04, p.1 + 0.10)
    }

    // ---------- hit / hover helpers ----------

    fn set_hover(&mut self, id: Id, v: bool) { self.hover_target.insert(id, v); }
    fn hover_eased(&self, id: Id) -> f32 {
        let t = (*self.hover.get(&id).unwrap_or(&0.0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
    fn rect_hit(&self, r: (f32, f32, f32, f32), nx: f32, ny: f32) -> bool {
        nx >= r.0 && nx <= r.2 && ny >= r.1 && ny <= r.3
    }
    fn btn_hit(&mut self, id: Id, r: (f32, f32, f32, f32),
               nx: f32, ny: f32, clicked: bool) -> bool {
        let h = self.rect_hit(r, nx, ny);
        self.set_hover(id, h);
        h && clicked
    }
    fn stepper(&mut self, id: Id, cx: f32, cy: f32,
               nx: f32, ny: f32, clicked: bool) -> bool {
        let r = (cx - 0.025, cy - 0.022, cx + 0.025, cy + 0.022);
        let h = self.rect_hit(r, nx, ny);
        self.set_hover(id, h);
        h && clicked
    }

    // ---------- draw ----------

    pub fn draw(&self, out: &mut Vec<Vertex>, audio: &Audio) {
        out.clear();
        self.draw_bg(out);
        self.draw_top_bar(out);
        self.draw_music_row(out, audio);
        self.draw_timeline(out, audio);
        self.draw_section_header(out);
        self.draw_statement_list(out);
        self.draw_add_buttons(out);
        self.draw_inspector(out);
        self.draw_preview(out);
        if self.status_timer > 0.0 {
            push_text_centered(out, &self.status, 0.0,
                self.view_bottom() - 0.020, BODY_PX, ACCENT_HI);
        }
        if let Some(p) = self.popup { self.draw_popup(out, p); }
    }

    fn draw_bg(&self, out: &mut Vec<Vertex>) {
        push_quad(out, self.view_left(), self.view_top(),
                       self.view_right(), self.view_bottom(),
                  BG_DEEP);
        // Subtle hex grid ornament along the corners.
        for i in 0..6 {
            let x = self.view_left() + 0.08 + i as f32 * 0.22;
            push_hex(out, x, self.view_bottom() - 0.04, 0.008, BG_PANEL);
        }
    }

    fn draw_music_row(&self, out: &mut Vec<Vertex>, audio: &Audio) {
        // Track picker button. Shows the current track's file
        // name, or a hint if nothing has been chosen yet.
        let r = self.track_btn_rect();
        let h = self.hover_eased(ID_TRACK_BTN);
        let bg   = mix3(BG_PANEL, BG_PANEL_HI, h);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, ring);

        let (label, label_col) = if self.ast.meta.music.is_empty() {
            ("SELECT TRACK".to_string(), DIM)
        } else {
            let stem = std::path::Path::new(&self.ast.meta.music)
                .file_name().and_then(|s| s.to_str())
                .unwrap_or(&self.ast.meta.music)
                .to_string();
            (format!("TRACK: {}", stem), WHITE)
        };
        let label_max = (r.2 - r.0) - 0.045;
        let label = fit_text(&label, BODY_PX, label_max);
        push_text(out, &label,
            r.0 + 0.012,
            (r.1 + r.3) * 0.5 - text_height(BODY_PX) * 0.5,
            BODY_PX, label_col);

        // Down arrow on the right edge of the track button hints
        // at the dropdown affordance. Drawn as a triangle so we
        // do not depend on glyph coverage of the bitmap font.
        let ax = r.2 - 0.018;
        let ay = (r.1 + r.3) * 0.5;
        push_tri(out,
            [ax - 0.008, ay - 0.005],
            [ax + 0.008, ay - 0.005],
            [ax,         ay + 0.008],
            ring);

        // Transport.
        self.draw_transport_play (out, self.play_btn_rect());
        self.draw_transport_pause(out, self.pause_btn_rect());
        self.draw_transport_stop (out, self.stop_btn_rect());

        // Time label, anchored to the cluster's right.
        let pos = audio.music_position();
        let dur = audio.music_duration();
        let s   = format!("{} / {}", fmt_time(pos), fmt_time(dur));
        let (tx, ty) = self.time_label_pos();
        let col = if audio.has_music() { ACCENT_HI } else { DIM };
        push_text(out, &s, tx, ty, BODY_PX, col);
    }

    fn draw_transport_play(&self, out: &mut Vec<Vertex>, r: (f32, f32, f32, f32)) {
        self.draw_transport_bg(out, r, ID_PLAY_BTN);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let s  = (r.3 - r.1) * 0.30;
        push_tri(out,
            [cx - s * 0.7, cy - s],
            [cx - s * 0.7, cy + s],
            [cx + s * 0.9, cy],
            WHITE);
    }
    fn draw_transport_pause(&self, out: &mut Vec<Vertex>, r: (f32, f32, f32, f32)) {
        self.draw_transport_bg(out, r, ID_PAUSE_BTN);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let h  = (r.3 - r.1) * 0.30;
        let w  = 0.005;
        let g  = 0.006;
        push_quad(out, cx - g - w, cy - h, cx - g,     cy + h, WHITE);
        push_quad(out, cx + g,     cy - h, cx + g + w, cy + h, WHITE);
    }
    fn draw_transport_stop(&self, out: &mut Vec<Vertex>, r: (f32, f32, f32, f32)) {
        self.draw_transport_bg(out, r, ID_STOP_BTN);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let s  = (r.3 - r.1) * 0.28;
        push_quad(out, cx - s, cy - s, cx + s, cy + s, WHITE);
    }
    fn draw_transport_bg(&self, out: &mut Vec<Vertex>,
                         r: (f32, f32, f32, f32), id: Id) {
        let h = self.hover_eased(id);
        let bg = mix3(BG_PANEL, BG_PANEL_HI, h);
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.002,
            mix3(ACCENT, ACCENT_HI, h));
    }

    fn draw_top_bar(&self, out: &mut Vec<Vertex>) {
        let y0 = self.view_top();
        let y1 = y0 + 0.16;
        push_quad(out, self.view_left(), y0, self.view_right(), y1, BG_PANEL);
        push_quad(out, self.view_left(), y1, self.view_right(), y1 + 0.004, ACCENT);

        self.draw_button(out, self.back_btn(), "BACK", ID_BACK);
        self.draw_button(out, self.save_btn(), "SAVE", ID_SAVE);
        self.draw_import_button(out);
        self.draw_preplay_button(out);

        // Title block. Sits in the empty space between SAVE and
        // the BPM cluster. We compute the rightmost safe x from
        // the BPM cluster's left edge so the title cannot ever
        // overlap a stepper, no matter the aspect ratio. If the
        // computed slot is too narrow we silently drop the
        // filename subtitle.
        let title_x = self.view_left() + 0.80;
        let title_y0 = y0 + 0.040;
        let title_y1 = y0 + 0.094;
        let title_right_safe = self.bpm_cx() - 0.10;
        let editor_w = text_width("EDITOR", H1_PX);
        if title_x + editor_w < title_right_safe {
            push_text(out, "EDITOR", title_x, title_y0, H1_PX, ACCENT_HI);
        }
        let file_str = format!("{}.rlf", self.file_stem);
        let file_w = text_width(&file_str, SMALL_PX);
        if title_x + file_w < title_right_safe {
            push_text(out, &file_str, title_x, title_y1, SMALL_PX, DIM);
        }

        // BPM cluster.
        let bpm_cx = self.bpm_cx();
        let cy     = self.cluster_cy();
        push_text_centered(out, "BPM", bpm_cx,
            y0 + 0.020, SMALL_PX, DIM);
        self.draw_stepper(out, bpm_cx - 0.07, cy, ID_BPM_DOWN, "<");
        self.draw_stepper(out, bpm_cx + 0.07, cy, ID_BPM_UP,   ">");
        let s = format!("{}", self.ast.meta.bpm);
        push_text_centered(out, &s, bpm_cx,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        // SIDES cluster.
        let sides_cx = self.sides_cx();
        push_text_centered(out, "SIDES", sides_cx,
            y0 + 0.020, SMALL_PX, DIM);
        self.draw_stepper(out, sides_cx - 0.07, cy, ID_SIDES_DOWN, "<");
        self.draw_stepper(out, sides_cx + 0.07, cy, ID_SIDES_UP,   ">");
        let s2 = format!("{}", self.ast.generation.sides);
        push_text_centered(out, &s2, sides_cx,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);
    }

    fn draw_timeline(&self, out: &mut Vec<Vertex>, audio: &Audio) {
        let bar = self.timeline_bar_rect();
        let has = audio.has_music();
        let dur = audio.music_duration();
        let pos = audio.music_position();

        // Bar background.
        push_quad(out, bar.0, bar.1, bar.2, bar.3, BG_DEEP);
        push_outline(out, bar.0, bar.1, bar.2, bar.3, 0.002,
            if has { ACCENT } else { DIM });

        // Progress fill.
        if has && dur > 0.1 {
            let prog = (pos / dur).clamp(0.0, 1.0);
            let fx = bar.0 + (bar.2 - bar.0) * prog;
            push_quad(out, bar.0, bar.1, fx, bar.3, ACCENT);
            // Scrubber line, taller than the bar so it remains
            // visible behind the progress fill.
            push_quad(out, fx - 0.002, bar.1 - 0.022,
                          fx + 0.002, bar.3 + 0.022, ACCENT_HI);
        }

        // Vertical guides from each section node down to the
        // bar, drawn before the nodes themselves so the nodes
        // sit on top.
        for (i, _sec) in self.ast.sections.iter().enumerate() {
            let r = self.section_node(i);
            let cx = (r.0 + r.2) * 0.5;
            let col = if i == self.selected_section { ACCENT_HI } else { DIM };
            push_quad(out, cx - 0.001, r.3, cx + 0.001, bar.1, col);
        }

        // Section nodes themselves.
        for (i, sec) in self.ast.sections.iter().enumerate() {
            let r = self.section_node(i);
            let cx = (r.0 + r.2) * 0.5;
            let cy = (r.1 + r.3) * 0.5;
            let h = self.hover_eased(id_section(i));
            let active = i == self.selected_section;
            let being_dragged = matches!(self.dragging_section, Some((di, _)) if di == i);
            // Dragged nodes get a stronger accent and a wider
            // outer ring so the author can track which one they
            // are moving even if it crosses another node.
            let col = if being_dragged { [1.00, 1.00, 0.85] }
                      else if active   { ACCENT_HI }
                      else             { mix3(EMIT_C, ACCENT_HI, h) };
            let ring_r = 0.034 + 0.004 * h
                + if being_dragged { 0.010 } else { 0.0 };
            push_hex(out, cx, cy, ring_r, BG_DEEP);
            push_hex_ring(out, cx, cy, ring_r, ring_r - 0.008, col);
            push_hex(out, cx, cy, 0.012, col);
            // Index inside the node.
            let idx = format!("{}", i + 1);
            let w = text_width(&idx, SMALL_PX);
            push_text(out, &idx, cx - w * 0.5,
                cy - text_height(SMALL_PX) * 0.5, SMALL_PX, WHITE);
            // `at` value above the node.
            let at_str = format!("{:.2}", sec.at);
            let w2 = text_width(&at_str, SMALL_PX);
            push_text(out, &at_str, cx - w2 * 0.5,
                r.1 - 0.018, SMALL_PX, DIM);
        }

        // [+] add-section node.
        let r = self.new_section_node();
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let h = self.hover_eased(ID_NEW_SECTION);
        let col = mix3(DIM, ACCENT, h);
        push_hex(out, cx, cy, 0.028 + 0.004 * h, BG_DEEP);
        push_hex_ring(out, cx, cy, 0.028, 0.022, col);
        push_text_centered(out, "+", cx,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, col);

        // Hint when no music has been picked yet.
        if !has {
            push_text_centered(out, "NO TRACK LOADED -- PICK ONE ABOVE",
                (bar.0 + bar.2) * 0.5, bar.3 + 0.024, SMALL_PX, DIM);
        }
    }

    fn draw_section_header(&self, out: &mut Vec<Vertex>) {
        let Some(sec) = self.ast.sections.get(self.selected_section) else { return; };
        let cy = self.section_header_y();

        // Column 1: section label. Drawn at SECTION_LABEL_X and
        // explicitly bounded by the AT cluster's left edge so a
        // long author chosen name truncates gracefully instead
        // of running into the stepper.
        let label_x      = self.section_label_x();
        let at_cx        = self.section_at_cx();
        let label_max_w  = (at_cx - 0.12) - label_x;
        let raw_label    = format!("SECTION \"{}\"", sec.name);
        let label = fit_text(&raw_label, H1_PX, label_max_w);
        push_text(out, &label, label_x,
            cy - text_height(H1_PX) * 0.5, H1_PX, WHITE);

        // Column 2: AT controls. "AT" small label sits on top
        // of the steppers so the row reads as one unit even on
        // narrow viewports.
        push_text_centered(out, "AT", at_cx,
            cy - 0.040, SMALL_PX, DIM);
        self.draw_stepper(out, at_cx - 0.07, cy, ID_AT_DOWN, "<");
        self.draw_stepper(out, at_cx + 0.07, cy, ID_AT_UP,   ">");
        let at = format!("{:.2}", sec.at);
        push_text_centered(out, &at, at_cx,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        // Column 3: statement count. Pinned to its own anchor
        // so it can never overlap the AT cluster on the left
        // or the DELETE button on the right.
        let n_stmts = sec.body.len();
        let count_x = self.section_count_x();
        let count_max_w = (self.del_section_btn().0 - 0.04) - count_x;
        if count_max_w > 0.10 {
            let count = format!("{} STATEMENT{}",
                n_stmts, if n_stmts == 1 { "" } else { "S" });
            let count = fit_text(&count, SMALL_PX, count_max_w);
            push_text(out, &count, count_x,
                cy - text_height(SMALL_PX) * 0.5, SMALL_PX, DIM);
        }

        // Column 4: DELETE SEC, right-anchored.
        let can_del = self.ast.sections.len() > 1;
        let r = self.del_section_btn();
        let h = self.hover_eased(ID_DEL_SECTION);
        let col = if can_del {
            mix3([0.7, 0.3, 0.4], [1.0, 0.45, 0.55], h)
        } else { DIM };
        push_quad(out, r.0, r.1, r.2, r.3, BG_PANEL);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, col);
        let cxr = (r.0 + r.2) * 0.5;
        let cyr = (r.1 + r.3) * 0.5;
        let w = text_width("DELETE SEC", SMALL_PX);
        push_text(out, "DELETE SEC", cxr - w * 0.5,
            cyr - text_height(SMALL_PX) * 0.5, SMALL_PX, col);
    }

    fn draw_statement_list(&self, out: &mut Vec<Vertex>) {
        let Some(sec) = self.ast.sections.get(self.selected_section) else { return; };
        for (i, st) in sec.body.iter().enumerate() {
            let r = self.stmt_row(i);
            let h = self.hover_eased(id_stmt(i));
            let active = self.selected_stmt == Some(i);
            let bg = if active { BG_PANEL_HI }
                     else { mix3(BG_PANEL, BG_PANEL_HI, h) };
            push_quad(out, r.0, r.1, r.2, r.3, bg);
            // Color stripe by stmt kind.
            let stripe = stmt_color(st);
            push_quad(out, r.0, r.1, r.0 + 0.012, r.3, stripe);
            // Number.
            let idx_s = format!("{:02}", i + 1);
            push_text(out, &idx_s, r.0 + 0.025,
                r.1 + 0.008, BODY_PX, DIM);
            // Label.
            let label = stmt_label(st);
            push_text(out, &label, r.0 + 0.080,
                r.1 + 0.008, BODY_PX, WHITE);
            // X button.
            let xr = self.stmt_x_btn(i);
            let xh = self.hover_eased(id_stmt_x(i));
            let xcol = mix3(DIM, [1.0, 0.4, 0.5], xh);
            push_outline(out, xr.0, xr.1, xr.2, xr.3, 0.002, xcol);
            push_text_centered(out, "X",
                (xr.0 + xr.2) * 0.5,
                (xr.1 + xr.3) * 0.5 - text_height(SMALL_PX) * 0.5,
                SMALL_PX, xcol);
        }
        if sec.body.is_empty() {
            push_text(out, "EMPTY SECTION  USE +EMIT / +TRIG / +WAIT",
                self.view_left() + 0.04, self.stmt_list_y0() + 0.020,
                BODY_PX, DIM);
        }
    }

    fn draw_add_buttons(&self, out: &mut Vec<Vertex>) {
        for (k, (label, c)) in [
            ("+EMIT",   EMIT_C),
            ("+TRIG",   TRIG_C),
            ("+WAIT",   WAIT_C),
        ].iter().enumerate() {
            let r = self.add_btn(k);
            let id = match k { 0 => ID_ADD_EMIT, 1 => ID_ADD_TRIG, _ => ID_ADD_WAIT };
            let h = self.hover_eased(id);
            let bg = mix3(BG_PANEL, BG_PANEL_HI, h);
            push_quad(out, r.0, r.1, r.2, r.3, bg);
            push_outline(out, r.0, r.1, r.2, r.3, 0.003, *c);
            let cx = (r.0 + r.2) * 0.5;
            let cy = (r.1 + r.3) * 0.5;
            let w = text_width(label, BODY_PX);
            push_text(out, label, cx - w * 0.5,
                cy - text_height(BODY_PX) * 0.5, BODY_PX, *c);
        }
    }

    fn draw_import_button(&self, out: &mut Vec<Vertex>) {
        let r = self.import_btn();
        let h = self.hover_eased(ID_IMPORT_BTN);
        // Cool blue accent distinguishes the "pull existing
        // work in" operation from SAVE (which flushes out) and
        // from BACK (which discards).
        let stroke = mix3([0.50, 0.75, 1.00], [0.80, 0.92, 1.00], h);
        let bg     = mix3(BG_PANEL, [0.08, 0.14, 0.24], h);
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, stroke);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let w = text_width("IMPORT", BODY_PX);
        push_text(out, "IMPORT", cx - w * 0.5,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);
    }

    fn draw_inspector(&self, out: &mut Vec<Vertex>) {
        let Some(idx) = self.selected_stmt else {
            push_text(out, "SELECT A STATEMENT TO INSPECT",
                self.view_left() + 0.04, self.inspector_y0() - 0.020,
                SMALL_PX, DIM);
            return;
        };
        let Some(stmt) = self.ast.sections.get(self.selected_section)
            .and_then(|s| s.body.get(idx)) else { return; };

        push_text(out, "INSPECTOR",
            self.view_left() + 0.04, self.inspector_y0() - 0.025,
            SMALL_PX, ACCENT_HI);

        let fields = inspector_fields(stmt);
        for (k, f) in fields.iter().enumerate() {
            let r = self.inspector_field_rect(k);
            let cx = (r.0 + r.2) * 0.5;
            let cy = (r.1 + r.3) * 0.5;
            push_text(out, &f.label, r.0,
                cy - text_height(SMALL_PX) * 0.5, SMALL_PX, DIM);
            match &f.kind {
                InsKind::Stepper { value, .. } => {
                    self.draw_stepper(out, cx - 0.10, cy, id_field(k * 2),     "<");
                    self.draw_stepper(out, cx + 0.10, cy, id_field(k * 2 + 1), ">");
                    push_text_centered(out, value, cx,
                        cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);
                }
                InsKind::Cycle { value, .. } => {
                    let cyc = (cx - 0.06, cy - 0.025, cx + 0.06, cy + 0.025);
                    let h = self.hover_eased(id_field(k * 2));
                    let bg = mix3(BG_PANEL, BG_PANEL_HI, h);
                    push_quad(out, cyc.0, cyc.1, cyc.2, cyc.3, bg);
                    push_outline(out, cyc.0, cyc.1, cyc.2, cyc.3, 0.002, ACCENT);
                    push_text_centered(out, value, cx,
                        cy - text_height(BODY_PX) * 0.5, BODY_PX, ACCENT_HI);
                }
            }
        }
    }

    fn draw_preview(&self, out: &mut Vec<Vertex>) {
        let r = self.preview_rect();
        push_quad(out, r.0, r.1, r.2, r.3, BG_DEEP);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, ACCENT);
        push_text(out, "LIVE PREVIEW",
            r.0 + 0.012, r.1 + 0.008, SMALL_PX, ACCENT_HI);

        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5 + 0.010;
        let radius = ((r.2 - r.0).min(r.3 - r.1)) * 0.40;
        let sides = self.ast.generation.sides.max(3);
        let pal = self.ast.palette;

        // Background wedges.
        for s in 0..sides {
            let a0 = s as f32 * TAU / sides as f32;
            let a1 = a0 + TAU / sides as f32;
            let col = if s % 2 == 0 { pal.bg_a } else { pal.bg_b };
            out.push(Vertex::opaque([cx, cy], col));
            out.push(Vertex::opaque([cx + a0.cos() * radius, cy + a0.sin() * radius], col));
            out.push(Vertex::opaque([cx + a1.cos() * radius, cy + a1.sin() * radius], col));
        }
        // Walls.
        let slot_a = TAU / sides as f32;
        for w in &self.preview.walls {
            let a0 = w.slot as f32 * slot_a;
            let a1 = a0 + slot_a;
            let r0 = (w.distance * radius).max(0.0);
            let r1 = ((w.distance + w.thickness) * radius).max(0.0);
            let p00 = [cx + a0.cos() * r0, cy + a0.sin() * r0];
            let p01 = [cx + a1.cos() * r0, cy + a1.sin() * r0];
            let p10 = [cx + a0.cos() * r1, cy + a0.sin() * r1];
            let p11 = [cx + a1.cos() * r1, cy + a1.sin() * r1];
            out.push(Vertex::opaque(p00, pal.wall));
            out.push(Vertex::opaque(p01, pal.wall));
            out.push(Vertex::opaque(p11, pal.wall));
            out.push(Vertex::opaque(p00, pal.wall));
            out.push(Vertex::opaque(p11, pal.wall));
            out.push(Vertex::opaque(p10, pal.wall));
        }
        // Center hex.
        push_hex(out, cx, cy, radius * 0.16, pal.center_fill);
        push_hex_ring(out, cx, cy, radius * 0.16, radius * 0.135, pal.center_ring);

        // Loop progress bar.
        let bar_y = r.3 - 0.020;
        push_quad(out, r.0 + 0.012, bar_y - 0.004,
                       r.2 - 0.012, bar_y + 0.004, BG_PANEL);
        let prog = (self.preview.time / self.preview.cycle).clamp(0.0, 1.0);
        let fx = r.0 + 0.012 + (r.2 - r.0 - 0.024) * prog;
        push_quad(out, r.0 + 0.012, bar_y - 0.004,
                       fx, bar_y + 0.004, ACCENT);
    }

    fn draw_popup(&self, out: &mut Vec<Vertex>, kind: PopupKind) {
        push_quad_alpha(out, self.view_left(), self.view_top(),
                              self.view_right(), self.view_bottom(),
            [0.0, 0.0, 0.0], 0.55);
        let p = self.popup_panel();
        push_quad(out, p.0, p.1, p.2, p.3, BG_PANEL);
        push_outline(out, p.0, p.1, p.2, p.3, 0.004, ACCENT);

        let title = match kind {
            PopupKind::Obstacle => "ADD OBSTACLE",
            PopupKind::Trigger  => "ADD TRIGGER",
            PopupKind::Track    => "PICK A TRACK",
            PopupKind::Import   => "IMPORT EXISTING LEVEL",
        };
        push_text(out, title, p.0 + 0.04, p.1 + 0.04, H1_PX, ACCENT_HI);

        let r = self.popup_close_btn();
        let h = self.hover_eased(ID_POPUP_X);
        let col = mix3(DIM, [1.0, 0.4, 0.5], h);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, col);
        push_text_centered(out, "CANCEL",
            (r.0 + r.2) * 0.5,
            (r.1 + r.3) * 0.5 - text_height(SMALL_PX) * 0.5,
            SMALL_PX, col);

        match kind {
            PopupKind::Obstacle | PopupKind::Trigger => {
                let entries = if matches!(kind, PopupKind::Obstacle) {
                    OBSTACLE_TYPES
                } else { TRIGGER_TYPES };
                let accent = if matches!(kind, PopupKind::Obstacle) {
                    EMIT_C
                } else { TRIG_C };
                for (i, label) in entries.iter().enumerate() {
                    let r = self.popup_btn(i);
                    let h = self.hover_eased(id_popup(i));
                    let bg = mix3(BG_DEEP, BG_PANEL_HI, h);
                    push_quad(out, r.0, r.1, r.2, r.3, bg);
                    push_outline(out, r.0, r.1, r.2, r.3, 0.003,
                        mix3(accent, ACCENT_HI, h));
                    let cx = (r.0 + r.2) * 0.5;
                    let cy = (r.1 + r.3) * 0.5;
                    let w = text_width(label, BODY_PX);
                    push_text(out, label, cx - w * 0.5,
                        cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);
                }
            }
            PopupKind::Import => {
                if self.level_list.is_empty() {
                    push_text_centered(out,
                        "NO .RLF FILES FOUND IN assets/customlevels/ OR assets/levels/",
                        0.0, p.1 + 0.30, BODY_PX, DIM);
                    return;
                }
                let visible = self.level_list.len().min(12);
                for i in 0..visible {
                    let r = self.track_popup_btn(i);
                    let h = self.hover_eased(id_import_item(i));
                    let entry = &self.level_list[i];
                    let bg = mix3(BG_DEEP, BG_PANEL_HI, h);
                    push_quad(out, r.0, r.1, r.2, r.3, bg);
                    push_outline(out, r.0, r.1, r.2, r.3, 0.003,
                        mix3([0.50, 0.75, 1.00], [0.80, 0.92, 1.00], h));

                    let label = fit_text(&entry.display, BODY_PX,
                        (r.2 - r.0) * 0.55);
                    push_text(out, &label,
                        r.0 + 0.018,
                        (r.1 + r.3) * 0.5 - text_height(BODY_PX) * 0.5,
                        BODY_PX, WHITE);

                    let folder = if entry.path.starts_with("assets/customlevels") {
                        "[CUSTOM]"
                    } else { "[STOCK]" };
                    push_text_right(out, folder,
                        r.2 - 0.018,
                        (r.1 + r.3) * 0.5 - text_height(SMALL_PX) * 0.5,
                        SMALL_PX, DIM);
                }
                if self.level_list.len() > visible {
                    let s = format!("(+{} MORE NOT SHOWN)",
                        self.level_list.len() - visible);
                    push_text_centered(out, &s, 0.0,
                        p.3 - 0.06, SMALL_PX, DIM);
                }
            }

            PopupKind::Track => {
                if self.track_list.is_empty() {
                    push_text_centered(out,
                        "NO QOA FILES FOUND IN assets/customlevels/songs/ OR assets/music/",
                        0.0, p.1 + 0.30, BODY_PX, DIM);
                    return;
                }
                let visible = self.track_list.len().min(12);
                for i in 0..visible {
                    let r = self.track_popup_btn(i);
                    let h = self.hover_eased(id_track_item(i));
                    let entry = &self.track_list[i];
                    let active = self.ast.meta.music == entry.path;
                    let bg = if active { BG_PANEL_HI }
                             else      { mix3(BG_DEEP, BG_PANEL_HI, h) };
                    push_quad(out, r.0, r.1, r.2, r.3, bg);
                    push_outline(out, r.0, r.1, r.2, r.3, 0.003,
                        mix3(if active { ACCENT_HI } else { ACCENT }, ACCENT_HI, h));

                    // File name on the left.
                    let label = fit_text(&entry.display, BODY_PX,
                        (r.2 - r.0) * 0.55);
                    push_text(out, &label,
                        r.0 + 0.018,
                        (r.1 + r.3) * 0.5 - text_height(BODY_PX) * 0.5,
                        BODY_PX, WHITE);

                    // Folder hint on the right (assets/music vs
                    // assets/customlevels/songs).
                    let folder = if entry.path.starts_with("assets/customlevels") {
                        "[CUSTOM]"
                    } else { "[STOCK]" };
                    push_text_right(out, folder,
                        r.2 - 0.018,
                        (r.1 + r.3) * 0.5 - text_height(SMALL_PX) * 0.5,
                        SMALL_PX, DIM);

                    if active {
                        push_text(out, ">",
                            r.0 + 0.004,
                            (r.1 + r.3) * 0.5 - text_height(BODY_PX) * 0.5,
                            BODY_PX, ACCENT_HI);
                    }
                }
                if self.track_list.len() > visible {
                    let s = format!("(+{} MORE NOT SHOWN)",
                        self.track_list.len() - visible);
                    push_text_centered(out, &s, 0.0,
                        p.3 - 0.06, SMALL_PX, DIM);
                }
            }
        }
    }

    // ---------- atomic widgets ----------

    fn draw_button(&self, out: &mut Vec<Vertex>,
                   r: (f32, f32, f32, f32), label: &str, id: Id) {
        let h = self.hover_eased(id);
        let bg   = mix3(BG_PANEL, BG_PANEL_HI, h);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, ring);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let w = text_width(label, BODY_PX);
        push_text(out, label, cx - w * 0.5,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);
    }

    fn draw_preplay_button(&self, out: &mut Vec<Vertex>) {
        let r = self.preplay_btn();
        let h = self.hover_eased(ID_PREPLAY);
        // Greenish accent so the destructive nature of leaving
        // the editor briefly is visually distinct from BACK.
        let stroke = mix3([0.40, 0.85, 0.45], [0.65, 1.00, 0.70], h);
        let bg     = mix3(BG_PANEL, [0.10, 0.20, 0.10], h);
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, stroke);
        // Play triangle on the left.
        let cy = (r.1 + r.3) * 0.5;
        let tx = r.0 + 0.018;
        let s  = (r.3 - r.1) * 0.30;
        push_tri(out,
            [tx,           cy - s],
            [tx,           cy + s],
            [tx + s * 1.4, cy],
            stroke);
        // Label.
        let label = "PRE-PLAY";
        let lw = text_width(label, BODY_PX);
        let cx = (r.0 + r.2) * 0.5 + 0.012;
        push_text(out, label, cx - lw * 0.5,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);
    }

    fn draw_stepper(&self, out: &mut Vec<Vertex>,
                    cx: f32, cy: f32, id: Id, glyph: &str) {
        let h = self.hover_eased(id);
        let r = (cx - 0.025, cy - 0.022, cx + 0.025, cy + 0.022);
        let col = mix3(ACCENT, ACCENT_HI, h);
        push_outline(out, r.0, r.1, r.2, r.3, 0.002, col);
        push_text_centered(out, glyph, cx,
            cy - text_height(BODY_PX) * 0.5, BODY_PX, col);
    }
}

// =================================================================
// helpers free functions
// =================================================================

/// Entry in the import dropdown. `path` is relative so the
/// editor can reload portably from any cwd / exe directory.
struct LevelEntry { display: String, path: String }

/// Enumerate `.rlf` files both folders the editor can load
/// from. Same relative-first, exe-dir-fallback policy as
/// [`scan_tracks`], and the same dedup rule so entries never
/// appear twice when the two roots overlap.
fn scan_levels() -> Vec<LevelEntry> {
    let mut out: Vec<LevelEntry> = Vec::new();
    let dirs = ["assets/customlevels", "assets/levels"];
    for rel in &dirs {
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
                let path = format!("{}/{}", rel, display);
                if out.iter().all(|m| m.path != path) {
                    out.push(LevelEntry { display, path });
                }
            }
        }
    }
    out.sort_by(|a, b| a.display.to_lowercase().cmp(&b.display.to_lowercase()));
    out
}

/// Try to read a `.rlf` file using the same relative-first,
/// exe-fallback rule as the music loader. Returns `None` if
/// the file is missing from both locations.
fn read_level_file(rel_path: &str) -> Option<String> {
    if let Ok(s) = std::fs::read_to_string(rel_path) { return Some(s); }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            let p = d.join(rel_path);
            if let Ok(s) = std::fs::read_to_string(p) { return Some(s); }
        }
    }
    None
}

/// Music file picker entry. `path` is what we write into
/// `meta.music`; we always keep it relative to the working
/// directory so saved `.rlf` files stay portable across
/// machines and asset roots.
struct MusicEntry { display: String, path: String }

/// Walk the two folders the editor scans for tracks:
///
/// * `assets/customlevels/songs/` for user-supplied music,
/// * `assets/music/` for the stock Chipzel set,
///
/// and return a sorted, deduplicated list. Each folder is
/// looked up both relative to the current working directory
/// (which is what `cargo run` uses) and relative to the
/// executable directory (which is what double-click launches
/// use). Identical files reachable from both ends collapse to
/// a single entry, with the relative-to-cwd path winning so
/// the eventual save is portable.
fn scan_tracks() -> Vec<MusicEntry> {
    let mut out: Vec<MusicEntry> = Vec::new();
    let dirs = ["assets/customlevels/songs", "assets/music"];

    for rel in &dirs {
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
                    out.push(MusicEntry { display, path });
                }
            }
        }
    }
    out.sort_by(|a, b| a.display.to_lowercase().cmp(&b.display.to_lowercase()));
    out
}

fn fmt_time(s: f32) -> String {
    let s = s.max(0.0) as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t,
     a[1] + (b[1] - a[1]) * t,
     a[2] + (b[2] - a[2]) * t]
}

/// Truncate `text` with a trailing ellipsis until it fits in
/// `max_w` game-space units at the given pixel size. Returns
/// the original string when it already fits, an empty string
/// when even a single character would not fit.
fn fit_text(text: &str, px: f32, max_w: f32) -> String {
    if max_w <= 0.0 { return String::new(); }
    if text_width(text, px) <= max_w { return text.to_string(); }
    let dots = "...";
    let dots_w = text_width(dots, px);
    if dots_w >= max_w { return String::new(); }
    let chars: Vec<char> = text.chars().collect();
    let mut take = chars.len();
    while take > 0 {
        take -= 1;
        let candidate: String = chars.iter().take(take).collect();
        if text_width(&candidate, px) + dots_w <= max_w {
            return format!("{}{}", candidate, dots);
        }
    }
    String::new()
}

fn save_path(stem: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("assets").join("customlevels")
                .join(format!("{}.rlf", stem));
        }
    }
    PathBuf::from("assets/customlevels").join(format!("{}.rlf", stem))
}

fn default_level() -> LevelAst {
    use crate::dsl::ast::*;
    LevelAst {
        meta: Meta {
            name: "DRAFT".into(),
            subtitle: "EDITOR".into(),
            author: "YOU".into(),
            song: "MENU".into(),
            bpm: 130,
            music: String::new(),
            description: "A new level created in the editor.".into(),
        },
        palette: Palette::default(),
        difficulty: DifficultySpec::default(),
        generation: GenerationSpec::default(),
        timestamp_format: TimestampFormat::Relative,
        globals: Vec::new(),
        sections: vec![
            Section {
                name: "intro".into(), at: 0.0,
                body: vec![Stmt::Emit(default_obstacle("BAR"))],
            },
        ],
        start_from_seconds: 0.0,
    }
}

// ---------- type tables ----------

const OBSTACLE_TYPES: &[&str] = &[
    "BAR", "DOUBLEBAR", "SPIRAL", "RAIN",
    "ALTERNATE", "PINWHEEL", "LADDER", "POT",
    "TUNNEL", "RAINBOW", "STAIRCASE", "CORRIDOR",
    "CUBES",
];

const TRIGGER_TYPES: &[&str] = &[
    "FLIP", "PULSE", "TILT", "SPEEDMULT",
    "HUESHIFT", "SPEEDWARP", "GLITCH", "SHAKE",
    "ZOOM", "INVERT", "STROBE",
];

fn default_obstacle(label: &str) -> ObstacleSpec {
    match label {
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

fn default_trigger(label: &str) -> TriggerSpec {
    match label {
        "FLIP"      => TriggerSpec::Flip,
        "PULSE"     => TriggerSpec::Pulse,
        "TILT"      => TriggerSpec::Tilt(1.0),
        "SPEEDMULT" => TriggerSpec::SpeedMult(1.2),
        "HUESHIFT"  => TriggerSpec::HueShift(0.5),
        "SPEEDWARP" => TriggerSpec::SpeedWarp {
            walls: 1.2, rotation: 1.0, cursor: 1.0,
            music_scale: 1.0, duration: 3.0,
        },
        "GLITCH"    => TriggerSpec::Glitch { strength: 0.5, duration: 0.8 },
        "SHAKE"     => TriggerSpec::Shake  { strength: 0.5, duration: 0.6 },
        "ZOOM"      => TriggerSpec::Zoom   { target: 1.2, anim: Anim::EaseOut, duration: 1.0 },
        "INVERT"    => TriggerSpec::Invert { duration: 3.0 },
        "STROBE"    => TriggerSpec::Strobe { rate: 6.0, duration: 0.8 },
        _           => TriggerSpec::Flip,
    }
}

// ---------- inspector field model ----------

struct InsField { label: String, kind: InsKind }
enum InsKind {
    Stepper { value: String, kind_id: u8 },
    Cycle   { value: String, kind_id: u8 },
}

/// Build the inspector's editable field list for the given
/// statement. The `kind_id` is a per-statement-class index
/// mapped back to the actual mutation in `apply_field_*`.
fn inspector_fields(stmt: &Stmt) -> Vec<InsField> {
    let mut v = Vec::new();
    match stmt {
        Stmt::Wait(n) => {
            v.push(InsField {
                label: "WAIT BEATS".into(),
                kind:  InsKind::Stepper { value: format!("{}", n), kind_id: 0 },
            });
        }
        Stmt::Emit(spec) => {
            v.push(InsField {
                label: "TYPE".into(),
                kind:  InsKind::Cycle {
                    value: obstacle_kind_name(spec).into(), kind_id: 0,
                },
            });
            ins_obstacle_fields(spec, &mut v);
        }
        Stmt::Trigger(t) => {
            v.push(InsField {
                label: "TYPE".into(),
                kind:  InsKind::Cycle {
                    value: trigger_kind_name(t).into(), kind_id: 0,
                },
            });
            ins_trigger_fields(t, &mut v);
        }
        Stmt::Repeat { count, .. } => {
            v.push(InsField {
                label: "REPEAT".into(),
                kind:  InsKind::Stepper { value: format!("{}", count), kind_id: 0 },
            });
        }
        Stmt::LocalVars(_) => {}
    }
    v
}

fn ins_obstacle_fields(spec: &ObstacleSpec, v: &mut Vec<InsField>) {
    match spec {
        ObstacleSpec::Bar { thickness_mult } => {
            v.push(stepper_field("THICKNESS", thickness_mult, 1));
        }
        ObstacleSpec::DoubleBar { spacing, thickness_mult } => {
            v.push(int_stepper_field("SPACING", *spacing, 1));
            v.push(stepper_field("THICKNESS", thickness_mult, 2));
        }
        ObstacleSpec::Spiral { dir, thickness_mult, loops } => {
            v.push(dir_field("DIR", *dir, 1));
            v.push(int_stepper_field("LOOPS", *loops, 2));
            v.push(stepper_field("THICKNESS", thickness_mult, 3));
        }
        ObstacleSpec::Alternate { parity, thickness_mult } => {
            v.push(parity_field("PARITY", *parity, 1));
            v.push(stepper_field("THICKNESS", thickness_mult, 2));
        }
        ObstacleSpec::Pinwheel { spokes, dir } => {
            v.push(int_stepper_field("SPOKES", *spokes, 1));
            v.push(dir_field("DIR", *dir, 2));
        }
        ObstacleSpec::Rain { count, thickness_mult } => {
            v.push(int_stepper_field("COUNT", *count, 1));
            v.push(stepper_field("THICKNESS", thickness_mult, 2));
        }
        ObstacleSpec::Custom { thickness_mult, .. } => {
            v.push(stepper_field("THICKNESS", thickness_mult, 1));
        }
        ObstacleSpec::Rainbow { dir } => {
            v.push(dir_field("DIR", *dir, 1));
        }
        ObstacleSpec::Ladder { rungs } => {
            v.push(int_stepper_field("RUNGS", *rungs, 1));
        }
        ObstacleSpec::Tunnel { length, lanes } => {
            v.push(stepper_field("LENGTH", length, 1));
            v.push(int_stepper_field("LANES", *lanes, 2));
        }
        ObstacleSpec::Pot { layers } => {
            v.push(int_stepper_field("LAYERS", *layers, 1));
        }
        ObstacleSpec::Staircase { dir, steps, thickness_mult } => {
            v.push(dir_field("DIR", *dir, 1));
            v.push(int_stepper_field("STEPS", *steps, 2));
            v.push(stepper_field("THICKNESS", thickness_mult, 3));
        }
        ObstacleSpec::Corridor { length, turns, dir } => {
            v.push(stepper_field("LENGTH", length, 1));
            v.push(int_stepper_field("TURNS", *turns, 2));
            v.push(dir_field("DIR", *dir, 3));
        }
        ObstacleSpec::Cubes { layers, dir } => {
            v.push(int_stepper_field("LAYERS", *layers, 1));
            v.push(dir_field("DIR", *dir, 2));
        }
        ObstacleSpec::CustomFormula { steps, thickness_mult, .. } => {
            v.push(int_stepper_field("STEPS", *steps, 1));
            v.push(stepper_field("THICKNESS", thickness_mult, 2));
        }
    }
}

fn ins_trigger_fields(t: &TriggerSpec, v: &mut Vec<InsField>) {
    match t {
        TriggerSpec::Flip | TriggerSpec::Pulse => {}
        TriggerSpec::Tilt(d)        => v.push(stepper_field("ANGLE", d, 1)),
        TriggerSpec::SpeedMult(m)   => v.push(stepper_field("FACTOR", m, 1)),
        TriggerSpec::HueShift(r)    => v.push(stepper_field("RATE", r, 1)),
        TriggerSpec::SpeedWarp { walls, rotation, cursor, music_scale, duration } => {
            v.push(stepper_field("WALLS",    walls,       1));
            v.push(stepper_field("ROTATION", rotation,    2));
            v.push(stepper_field("CURSOR",   cursor,      3));
            v.push(stepper_field("MUSIC",    music_scale, 4));
            v.push(stepper_field("DURATION", duration,    5));
        }
        TriggerSpec::Glitch { strength, duration }
        | TriggerSpec::Shake { strength, duration } => {
            v.push(stepper_field("STRENGTH", strength, 1));
            v.push(stepper_field("DURATION", duration, 2));
        }
        TriggerSpec::Zoom { target, anim, duration } => {
            v.push(stepper_field("TARGET", target, 1));
            v.push(anim_field("ANIM", *anim, 2));
            v.push(stepper_field("DURATION", duration, 3));
        }
        TriggerSpec::Invert { duration } => {
            v.push(stepper_field("DURATION", duration, 1));
        }
        TriggerSpec::Strobe { rate, duration } => {
            v.push(stepper_field("RATE",     rate,     1));
            v.push(stepper_field("DURATION", duration, 2));
        }
    }
}

fn stepper_field(label: &str, v: &f32, id: u8) -> InsField {
    InsField {
        label: label.into(),
        kind: InsKind::Stepper { value: format!("{:.2}", v), kind_id: id },
    }
}
fn int_stepper_field(label: &str, v: u32, id: u8) -> InsField {
    InsField {
        label: label.into(),
        kind: InsKind::Stepper { value: format!("{}", v), kind_id: id },
    }
}
fn dir_field(label: &str, d: SpinDir, id: u8) -> InsField {
    InsField {
        label: label.into(),
        kind: InsKind::Cycle {
            value: match d { SpinDir::Cw => "CW", SpinDir::Ccw => "CCW" }.into(),
            kind_id: id,
        },
    }
}
fn parity_field(label: &str, p: Parity, id: u8) -> InsField {
    InsField {
        label: label.into(),
        kind: InsKind::Cycle {
            value: match p { Parity::Even => "EVEN", Parity::Odd => "ODD" }.into(),
            kind_id: id,
        },
    }
}
fn anim_field(label: &str, a: Anim, id: u8) -> InsField {
    InsField {
        label: label.into(),
        kind: InsKind::Cycle {
            value: match a {
                Anim::Linear      => "LINEAR",
                Anim::EaseIn      => "EASE_IN",
                Anim::EaseOut     => "EASE_OUT",
                Anim::EaseInOut   => "EASE_INOUT",
                Anim::Bounce      => "BOUNCE",
            }.into(),
            kind_id: id,
        },
    }
}

/// Apply a +/- step to the inspector field at index `k` of the
/// statement. `sign` is `-1.0` for the left stepper, `+1.0` for
/// the right one.
fn apply_field_delta(stmt: &mut Stmt, k: usize, sign: f32) {
    match stmt {
        Stmt::Wait(n) => {
            if k == 0 {
                let nv = (*n as i32 + sign as i32).clamp(1, 64);
                *n = nv as u32;
            }
        }
        Stmt::Repeat { count, .. } => {
            if k == 0 {
                let nv = (*count as i32 + sign as i32).clamp(1, 64);
                *count = nv as u32;
            }
        }
        Stmt::Emit(spec) => apply_obstacle_delta(spec, k, sign),
        Stmt::Trigger(t) => apply_trigger_delta(t, k, sign),
        Stmt::LocalVars(_) => {}
    }
}

fn apply_obstacle_delta(spec: &mut ObstacleSpec, k: usize, sign: f32) {
    // k == 0 is the type cycler (handled by apply_field_cycle).
    if k == 0 { return; }
    macro_rules! step_f {
        ($v:expr, $lo:expr, $hi:expr, $by:expr) => {
            { *$v = (*$v + sign * $by).clamp($lo, $hi); }
        };
    }
    macro_rules! step_u {
        ($v:expr, $lo:expr, $hi:expr) => {
            { let nv = (*$v as i32 + sign as i32).clamp($lo, $hi); *$v = nv as u32; }
        };
    }
    match spec {
        ObstacleSpec::Bar { thickness_mult } if k == 1 =>
            step_f!(thickness_mult, 0.5, 2.5, 0.05),
        ObstacleSpec::DoubleBar { spacing, thickness_mult } => {
            if k == 1 { step_u!(spacing, 1, 6); }
            else if k == 2 { step_f!(thickness_mult, 0.5, 2.5, 0.05); }
        }
        ObstacleSpec::Spiral { thickness_mult, loops, .. } => {
            if k == 2 { step_u!(loops, 1, 6); }
            else if k == 3 { step_f!(thickness_mult, 0.5, 2.0, 0.05); }
        }
        ObstacleSpec::Alternate { thickness_mult, .. } if k == 2 =>
            step_f!(thickness_mult, 0.5, 2.0, 0.05),
        ObstacleSpec::Pinwheel { spokes, .. } if k == 1 =>
            step_u!(spokes, 1, 6),
        ObstacleSpec::Rain { count, thickness_mult } => {
            if k == 1 { step_u!(count, 2, 12); }
            else if k == 2 { step_f!(thickness_mult, 0.5, 1.5, 0.05); }
        }
        ObstacleSpec::Custom { thickness_mult, .. } if k == 1 =>
            step_f!(thickness_mult, 0.5, 2.0, 0.05),
        ObstacleSpec::Ladder { rungs } if k == 1 =>
            step_u!(rungs, 3, 16),
        ObstacleSpec::Tunnel { length, lanes } => {
            if k == 1 { step_f!(length, 0.4, 2.5, 0.10); }
            else if k == 2 { step_u!(lanes, 1, 4); }
        }
        ObstacleSpec::Pot { layers } if k == 1 =>
            step_u!(layers, 2, 8),
        ObstacleSpec::Staircase { steps, thickness_mult, .. } => {
            if k == 2 { step_u!(steps, 3, 24); }
            else if k == 3 { step_f!(thickness_mult, 0.5, 2.0, 0.05); }
        }
        ObstacleSpec::Corridor { length, turns, .. } => {
            if k == 1 { step_f!(length, 0.6, 3.0, 0.10); }
            else if k == 2 { step_u!(turns, 1, 8); }
        }
        ObstacleSpec::Cubes { layers, .. } if k == 1 =>
            step_u!(layers, 2, 8),
        ObstacleSpec::CustomFormula { steps, thickness_mult, .. } => {
            if k == 1 { step_u!(steps, 1, 32); }
            else if k == 2 { step_f!(thickness_mult, 0.5, 2.0, 0.05); }
        }
        _ => {}
    }
}

fn apply_trigger_delta(t: &mut TriggerSpec, k: usize, sign: f32) {
    if k == 0 { return; }
    macro_rules! step {
        ($v:expr, $lo:expr, $hi:expr, $by:expr) => {
            { *$v = (*$v + sign * $by).clamp($lo, $hi); }
        };
    }
    match t {
        TriggerSpec::Tilt(d) if k == 1      => step!(d, -2.5, 2.5, 0.10),
        TriggerSpec::SpeedMult(m) if k == 1 => step!(m,  0.5, 2.0, 0.05),
        TriggerSpec::HueShift(r) if k == 1  => step!(r, -2.0, 2.0, 0.10),
        TriggerSpec::SpeedWarp { walls, rotation, cursor, music_scale, duration } => {
            match k {
                1 => step!(walls,       0.25, 4.0, 0.05),
                2 => step!(rotation,    0.25, 4.0, 0.05),
                3 => step!(cursor,      0.25, 4.0, 0.05),
                4 => step!(music_scale, 0.25, 4.0, 0.05),
                5 => step!(duration,    0.10, 20.0, 0.20),
                _ => {}
            }
        }
        TriggerSpec::Glitch { strength, duration }
        | TriggerSpec::Shake { strength, duration } => {
            if k == 1 { step!(strength, 0.0, 1.5, 0.05); }
            else if k == 2 { step!(duration, 0.05, 10.0, 0.10); }
        }
        TriggerSpec::Zoom { target, duration, .. } => {
            if k == 1 { step!(target, 0.25, 3.0, 0.05); }
            else if k == 3 { step!(duration, 0.05, 10.0, 0.10); }
        }
        TriggerSpec::Invert { duration } if k == 1 =>
            step!(duration, 0.05, 30.0, 0.20),
        TriggerSpec::Strobe { rate, duration } => {
            if k == 1 { step!(rate, 0.0, 40.0, 1.0); }
            else if k == 2 { step!(duration, 0.05, 10.0, 0.10); }
        }
        _ => {}
    }
}

fn apply_field_cycle(stmt: &mut Stmt, k: usize) {
    match stmt {
        Stmt::Emit(spec) => {
            if k == 0 {
                let cur = obstacle_kind_name(spec);
                let next = next_in(OBSTACLE_TYPES, cur);
                *spec = default_obstacle(next);
            } else {
                cycle_obstacle_field(spec, k);
            }
        }
        Stmt::Trigger(t) => {
            if k == 0 {
                let cur = trigger_kind_name(t);
                let next = next_in(TRIGGER_TYPES, cur);
                *t = default_trigger(next);
            } else {
                cycle_trigger_field(t, k);
            }
        }
        _ => {}
    }
}

fn cycle_obstacle_field(spec: &mut ObstacleSpec, k: usize) {
    match spec {
        ObstacleSpec::Spiral { dir, .. }
        | ObstacleSpec::Pinwheel { dir, .. }
        | ObstacleSpec::Rainbow { dir }
        | ObstacleSpec::Cubes { dir, .. } => {
            if k == 1 || k == 2 || k == 3 { *dir = flip_dir(*dir); }
        }
        ObstacleSpec::Staircase { dir, .. } if k == 1 => *dir = flip_dir(*dir),
        ObstacleSpec::Corridor { dir, .. } if k == 3 => *dir = flip_dir(*dir),
        ObstacleSpec::Alternate { parity, .. } if k == 1 =>
            *parity = match parity { Parity::Even => Parity::Odd, _ => Parity::Even },
        _ => {}
    }
}

fn cycle_trigger_field(t: &mut TriggerSpec, k: usize) {
    if let TriggerSpec::Zoom { anim, .. } = t {
        if k == 2 {
            *anim = match anim {
                Anim::Linear     => Anim::EaseIn,
                Anim::EaseIn     => Anim::EaseOut,
                Anim::EaseOut    => Anim::EaseInOut,
                Anim::EaseInOut  => Anim::Bounce,
                Anim::Bounce     => Anim::Linear,
            };
        }
    }
}

fn flip_dir(d: SpinDir) -> SpinDir {
    match d { SpinDir::Cw => SpinDir::Ccw, _ => SpinDir::Cw }
}

fn next_in(arr: &[&'static str], cur: &str) -> &'static str {
    let i = arr.iter().position(|s| *s == cur).unwrap_or(0);
    arr[(i + 1) % arr.len()]
}

fn obstacle_kind_name(s: &ObstacleSpec) -> &'static str {
    match s {
        ObstacleSpec::Bar { .. }           => "BAR",
        ObstacleSpec::DoubleBar { .. }     => "DOUBLEBAR",
        ObstacleSpec::Spiral { .. }        => "SPIRAL",
        ObstacleSpec::Alternate { .. }     => "ALTERNATE",
        ObstacleSpec::Pinwheel { .. }      => "PINWHEEL",
        ObstacleSpec::Rain { .. }          => "RAIN",
        ObstacleSpec::Custom { .. }        => "BAR",
        ObstacleSpec::Rainbow { .. }       => "RAINBOW",
        ObstacleSpec::Ladder { .. }        => "LADDER",
        ObstacleSpec::Tunnel { .. }        => "TUNNEL",
        ObstacleSpec::Pot { .. }           => "POT",
        ObstacleSpec::Staircase { .. }     => "STAIRCASE",
        ObstacleSpec::Corridor { .. }      => "CORRIDOR",
        ObstacleSpec::Cubes { .. }         => "CUBES",
        ObstacleSpec::CustomFormula { .. } => "BAR",
    }
}

fn trigger_kind_name(t: &TriggerSpec) -> &'static str {
    match t {
        TriggerSpec::Flip            => "FLIP",
        TriggerSpec::Pulse           => "PULSE",
        TriggerSpec::Tilt(_)         => "TILT",
        TriggerSpec::SpeedMult(_)    => "SPEEDMULT",
        TriggerSpec::HueShift(_)     => "HUESHIFT",
        TriggerSpec::SpeedWarp { .. }=> "SPEEDWARP",
        TriggerSpec::Glitch { .. }   => "GLITCH",
        TriggerSpec::Shake { .. }    => "SHAKE",
        TriggerSpec::Zoom { .. }     => "ZOOM",
        TriggerSpec::Invert { .. }   => "INVERT",
        TriggerSpec::Strobe { .. }   => "STROBE",
    }
}

fn stmt_label(s: &Stmt) -> String {
    match s {
        Stmt::Wait(n) => format!("WAIT {}", n),
        Stmt::Emit(spec) => format!("EMIT :{}", obstacle_kind_name(spec).to_lowercase()),
        Stmt::Trigger(t) => format!("TRIGGER :{}", trigger_kind_name(t).to_lowercase()),
        Stmt::Repeat { count, .. } => format!("REPEAT {}", count),
        Stmt::LocalVars(_) => "LOCAL VARS".into(),
    }
}

fn stmt_color(s: &Stmt) -> [f32; 3] {
    match s {
        Stmt::Emit(_)        => EMIT_C,
        Stmt::Trigger(_)     => TRIG_C,
        Stmt::Wait(_)        => WAIT_C,
        Stmt::Repeat { .. }  => REPEAT_C,
        Stmt::LocalVars(_)   => DIM,
    }
}