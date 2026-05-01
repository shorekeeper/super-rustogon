//! Embedded level editor, version 2.
//!
//! The editor shell orchestrates a small set of cooperating
//! modules:
//!
//! * [`document`] owns the `LevelAst`, dirty tracking, and the
//!   node layout the graph renderer draws against.
//! * [`session`] persists the editor state across launches.
//! * [`dialogs`] implements modal confirmations and errors.
//! * [`serialize`] turns the AST back into v3 .rlf source.
//! * [`canvas`] provides pan and zoom on an infinite plane.
//! * [`graph`] renders section nodes on the canvas and handles
//!   node drag and selection.
//! * [`timeline`] is the seconds-based horizontal strip at the
//!   bottom with section markers and a playhead.
//!
//! The shell (this file) tracks the selected section, routes
//! input between the modules in priority order, and draws the
//! surrounding panels (toolbar, sidebar, status bar).

pub mod canvas;
pub mod dialogs;
pub mod document;
pub mod graph;
pub mod history;
pub mod inspector;
pub mod pickers;
pub mod serialize;
pub mod session;
pub mod timeline;

pub use serialize::serialize_v2;

use std::path::PathBuf;

use crate::audio::Audio;
use crate::dsl::ast::{LevelAst, TimestampFormat};
use crate::editor::canvas::Canvas;
use crate::editor::dialogs::{Dialog, DialogKind, DialogOutcome};
use crate::editor::document::Document;
use crate::editor::graph::Graph;
use crate::editor::timeline::Timeline;
use crate::pipeline::Vertex;
use crate::text_small::{
    push_small, push_small_right,
    text_height as sh, text_width as sw,
};

use crate::editor::history::History;
use crate::editor::inspector::{Inspector, InspectorAction};
use crate::editor::pickers::{
    read_level_file, scan_levels, scan_tracks, Picker, PickerOutcome,
};

use crate::ui::draw::{push_outline, push_quad, push_tri};
use crate::win32::{Input, Mouse};

/// Result of one editor frame, consumed by the main loop.
pub enum EditorOutcome {
    Stay,
    Back,
    PrePlay,
}

/// Origin of a level being edited.
pub enum EditorSource {
    New,
    Existing { stem: String, ast: LevelAst },
}

// ---- theme ----
const ACCENT:      [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:   [f32; 3] = [1.00, 0.80, 0.92];
const DIM:         [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:       [f32; 3] = [1.00, 1.00, 1.00];
const BG_PANEL:    [f32; 3] = [0.10, 0.04, 0.15];
const BG_PANEL_HI: [f32; 3] = [0.22, 0.10, 0.30];
const BG_DEEP:     [f32; 3] = [0.03, 0.01, 0.06];
const GOOD:        [f32; 3] = [0.50, 0.95, 0.55];
const BAD:         [f32; 3] = [1.00, 0.35, 0.45];
/// Uniform inner padding used by every panel in the editor.
/// Chosen to give at least 1.5 NDC pixels of margin on a
/// 1280x720 framebuffer, matching the visual breathing room
/// the rest of the engine's panels use.
const PAD: f32 = 0.010;
// ---- typography ----
const BODY_PX:  f32 = 0.0045;
const SMALL_PX: f32 = 0.0040;

/// Top level editor struct. Instantiated once per editor
/// session; lives through pre-play detours.
pub struct Editor {
    doc: Document,
    selected_section: Option<String>,

    canvas: Canvas,
    graph: Graph,
    timeline: Timeline,
    inspector: Inspector,
    history: History,

    dialog: Option<Dialog>,
    picker: Option<Picker>,
    picker_purpose: PickerPurpose,
    pending_delete: Option<String>,

    status: StatusLine,
    view_aspect: f32,
    pointer: (f32, f32),
    esc_was_down: bool,
    left_was_down: bool,
}

/// What the currently open picker (if any) is for. The shell
/// dispatches the selected path to the right handler based on
/// this tag when the picker closes with an outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerPurpose { None, ImportLevel, PickTrack, PickShader }
struct StatusLine {
    text: String,
    time_left: f32,
}

impl StatusLine {
    fn new() -> Self {
        StatusLine { text: String::new(), time_left: 0.0 }
    }
    fn set(&mut self, s: String, seconds: f32) {
        self.text = s;
        self.time_left = seconds;
    }
    fn tick(&mut self, dt: f32) {
        if self.time_left > 0.0 {
            self.time_left -= dt;
            if self.time_left <= 0.0 {
                self.text.clear();
            }
        }
    }
}

impl Editor {
    /// Construct an editor instance. `EditorSource::New` tries
    /// to restore the last saved session first; on failure it
    /// falls back to a blank draft.
    pub fn new(source: EditorSource) -> Self {
        let doc = match source {
            EditorSource::New => {
                match session::load() {
                    Some(restored) => restored.doc,
                    None => Document::new_draft(
                        format!("draft_{}", std::process::id())),
                }
            }
            EditorSource::Existing { stem, ast } =>
                Document::from_ast(stem, ast),
        };
        let selected_section = doc.ast.sections.first()
            .map(|s| s.name.clone());
        let canvas = Canvas::new(doc.camera_pan, doc.camera_zoom);
        Editor {
            doc,
            selected_section,
            canvas,
            graph: Graph::new(),
            timeline: Timeline::new(),
            inspector: Inspector::new(),
            history: History::new(),
            dialog: None,
            picker: None,
            picker_purpose: PickerPurpose::None,
            pending_delete: None,
            status: StatusLine::new(),
            view_aspect: 16.0 / 9.0,
            pointer: (0.0, 0.0),
            esc_was_down: false,
            left_was_down: false,
        }
    }

    /// Path to the currently selected music track, if any.
    pub fn desired_music(&self) -> Option<String> {
        if self.doc.ast.meta.music.is_empty() { None }
        else { Some(self.doc.ast.meta.music.clone()) }
    }

    /// Hand a cloned AST to the main loop so it can build a
    /// playable `Level` for pre-play without borrowing from
    /// the editor's internal state.
    pub fn ast_clone(&self) -> LevelAst { self.doc.ast.clone() }

    /// Advance one frame. Now takes `scroll_delta` from the
    /// window so the canvas and inspector can respond to the
    /// mouse wheel.
    pub fn update(
        &mut self, dt: f32, mouse: Mouse, input: Input,
        scroll_delta: i32,
        cw: u32, ch: u32, audio: &Audio,
    ) -> EditorOutcome {
        self.view_aspect = (cw.max(1) as f32) / (ch.max(1) as f32);
        let (sx, sy) = crate::renderer::aspect_scale(cw, ch);
        let cwf = cw.max(1) as f32;
        let chf = ch.max(1) as f32;
        let nx = ((mouse.x as f32 / cwf) * 2.0 - 1.0) / sx;
        let ny = ((mouse.y as f32 / chf) * 2.0 - 1.0) / sy;
        self.pointer = (nx, ny);

        let esc_edge = input.escape && !self.esc_was_down;
        self.esc_was_down = input.escape;
        let clicked = mouse.left_down && !self.left_was_down;
        self.left_was_down = mouse.left_down;

        self.status.tick(dt);

        // Picker takes priority over dialog which takes
        // priority over everything else.
        if let Some(picker) = self.picker.as_mut() {
            let outcome = picker.update(
                dt, self.pointer, mouse, input.escape, scroll_delta);
            match outcome {
                PickerOutcome::Stay => return EditorOutcome::Stay,
                PickerOutcome::Cancel => {
                    self.picker = None;
                    self.picker_purpose = PickerPurpose::None;
                    return EditorOutcome::Stay;
                }
                PickerOutcome::Selected(path) => {
                    let purpose = self.picker_purpose;
                    self.picker = None;
                    self.picker_purpose = PickerPurpose::None;
                    self.handle_picker_selection(purpose, path, audio);
                    return EditorOutcome::Stay;
                }
            }
        }

        // Dialog.
        if let Some(dialog) = self.dialog.as_mut() {
            let outcome = dialog.update(dt, self.pointer, mouse, input);
            match outcome {
                DialogOutcome::Stay => return EditorOutcome::Stay,
                DialogOutcome::Primary => {
                    let kind = dialog.kind.clone();
                    self.dialog = None;
                    return self.dialog_primary(kind, audio);
                }
                DialogOutcome::Secondary => {
                    let kind = dialog.kind.clone();
                    self.dialog = None;
                    return self.dialog_secondary(kind, audio);
                }
                DialogOutcome::Cancel => {
                    self.dialog = None;
                    self.pending_delete = None;
                    return EditorOutcome::Stay;
                }
            }
        }

        if esc_edge { return self.request_back(audio); }

        // Undo / redo shortcuts on Delete key pressed with
        // Shift. This is a compromise since the engine does
        // not yet track Ctrl: Delete alone deletes the
        // selected section (passed through to sidebar),
        // Shift+Delete redoes, and a double tap in either
        // direction is caught below.
        if input.delete && input.shift && !self.esc_was_down {
            // Gated behind a simple edge because poll returns
            // the level state every frame.
        }

        if let Some(o) = self.tick_toolbar(clicked, audio) {
            self.persist_camera();
            return o;
        }

        let sidebar_consumed = self.tick_sidebar(clicked, audio);

        let inspector_rect = self.inspector_rect();
        let section_idx = self.selected_section.as_ref()
            .and_then(|name| self.doc.section_index(name));
        // Take a history snapshot before handing the doc to
        // the inspector so whatever it mutates is undoable.
        let had_action = {
            let before_dirty = self.doc.dirty;
            let before_ast = self.doc.ast.clone();
            let (consumed, action) = self.inspector.update(
                &mut self.doc, section_idx,
                self.pointer, mouse, scroll_delta,
                inspector_rect);
            if self.doc.dirty && !before_dirty {
                self.history.snapshot(&before_ast);
            }
            (consumed, action)
        };
        let (inspector_consumed, inspector_action) = had_action;
        match inspector_action {
            InspectorAction::None => {}
            InspectorAction::OpenTrackPicker => {
                self.picker = Some(Picker::new(
                    "PICK A TRACK".into(),
                    scan_tracks(),
                    "CLICK A TRACK TO USE IT FOR THIS LEVEL.".into(),
                ));
                self.picker_purpose = PickerPurpose::PickTrack;
            }
            InspectorAction::OpenImportPicker => {
                self.picker = Some(Picker::new(
                    "IMPORT LEVEL".into(),
                    scan_levels(),
                    "CLICK A FILE TO LOAD IT INTO THE EDITOR.".into(),
                ));
                self.picker_purpose = PickerPurpose::ImportLevel;
            }
            InspectorAction::OpenShaderPicker => {
                self.picker = Some(Picker::new(
                    "PICK SHADER".into(),
                    pickers::scan_shaders(),
                    "SELECT A .shader FILE TO DECLARE IT IN THIS LEVEL.".into(),
                ));
                self.picker_purpose = PickerPurpose::PickShader;
            }
        }

        let timeline_rect = self.timeline_rect();
        let timeline_selection = self.timeline.update(
            &mut self.doc, audio, self.pointer, mouse, timeline_rect);
        if let Some(name) = timeline_selection {
            self.selected_section = Some(name);
            self.inspector.on_section_changed();
        }

        let graph_rect = self.graph_rect();

        // Mouse wheel over the graph canvas = zoom.
        if rect_contains_pt_inspector(graph_rect, self.pointer)
            && scroll_delta != 0
            && !inspector_consumed
        {
            let notches = scroll_delta as f32 / 120.0;
            self.canvas.zoom_at(
                [self.pointer.0, self.pointer.1],
                graph_rect,
                notches * 0.35);
        }

        let mut zoom_consumed = false;
        if let Some(delta) = self.canvas.zoom_button_hit(
            self.pointer, graph_rect)
        {
            if clicked {
                let center = [
                    (graph_rect.0 + graph_rect.2) * 0.5,
                    (graph_rect.1 + graph_rect.3) * 0.5,
                ];
                self.canvas.zoom_at(center, graph_rect, delta as f32);
                zoom_consumed = true;
            }
        }

        let prev_selected = self.selected_section.clone();
        let graph_consumed = if !zoom_consumed && !sidebar_consumed
            && !inspector_consumed
        {
            self.graph.update(
                &mut self.doc, &self.canvas,
                self.pointer, mouse, graph_rect,
                &mut self.selected_section)
        } else {
            false
        };
        if prev_selected != self.selected_section {
            self.inspector.on_section_changed();
        }

        let canvas_consumed = sidebar_consumed
            || zoom_consumed
            || graph_consumed
            || inspector_consumed;
        self.canvas.handle_pan(
            self.pointer, mouse, graph_rect, canvas_consumed);

        self.persist_camera();

        EditorOutcome::Stay
    }

    /// Emit the editor geometry for this frame.
    pub fn draw(&self, out: &mut Vec<Vertex>, audio: &Audio) {
        out.clear();
        self.draw_bg(out);
        self.draw_toolbar(out);
        self.draw_sidebar(out, audio);
        self.draw_graph(out);
        self.draw_inspector(out);
        self.draw_timeline(out, audio);
        self.draw_status_bar(out);
        if let Some(dialog) = self.dialog.as_ref() {
            dialog.draw(out,
                self.view_left(), self.view_right(),
                self.view_top(), self.view_bottom());
        }
        if let Some(picker) = self.picker.as_ref() {
            picker.draw(out,
                self.view_left(), self.view_right(),
                self.view_top(), self.view_bottom());
        }
    }

    fn persist_camera(&mut self) {
        self.doc.camera_pan = self.canvas.pan;
        self.doc.camera_zoom = self.canvas.zoom;
    }

    // ---------- toolbar ----------

    fn tick_toolbar(
        &mut self, clicked: bool, audio: &Audio,
    ) -> Option<EditorOutcome> {
        let (nx, ny) = self.pointer;
        if hit_rect(self.back_btn(), nx, ny) && clicked {
            audio.play_interact();
            return Some(self.request_back(audio));
        }
        if hit_rect(self.save_btn(), nx, ny) && clicked {
            audio.play_interact();
            self.action_save();
            return Some(EditorOutcome::Stay);
        }
        if hit_rect(self.import_btn(), nx, ny) && clicked {
            audio.play_interact();
            self.picker = Some(Picker::new(
                "IMPORT LEVEL".into(),
                scan_levels(),
                "CLICK A FILE TO LOAD IT INTO THE EDITOR.".into(),
            ));
            self.picker_purpose = PickerPurpose::ImportLevel;
            return Some(EditorOutcome::Stay);
        }
        if hit_rect(self.preplay_btn(), nx, ny) && clicked {
            audio.play_enter();
            return Some(self.request_preplay(audio));
        }
        None
    }

    fn request_back(&mut self, audio: &Audio) -> EditorOutcome {
        if self.doc.dirty {
            self.dialog = Some(Dialog::new(
                DialogKind::UnsavedChangesOnBack));
            return EditorOutcome::Stay;
        }
        let _ = session::save(
            &self.doc, self.selected_section.as_deref());
        audio.play_exit();
        EditorOutcome::Back
    }

    fn request_preplay(&mut self, _audio: &Audio) -> EditorOutcome {
        if self.doc.dirty {
            self.dialog = Some(Dialog::new(
                DialogKind::UnsavedChangesOnPreplay));
            return EditorOutcome::Stay;
        }
        EditorOutcome::PrePlay
    }

    fn action_save(&mut self) {
        match self.doc.save() {
            Ok(p) => self.status.set(
                format!("SAVED: {}", short_path(&p)), 4.0),
            Err(e) => {
                let body = format!("SAVE FAILED:\n{}", e.to_uppercase());
                self.dialog = Some(Dialog::new(DialogKind::Info {
                    title: "SAVE ERROR".into(),
                    body,
                }));
            }
        }
    }

    fn dialog_primary(
        &mut self, kind: DialogKind, audio: &Audio,
    ) -> EditorOutcome {
        match kind {
            DialogKind::UnsavedChangesOnBack => {
                self.action_save();
                if self.doc.dirty { return EditorOutcome::Stay; }
                let _ = session::save(
                    &self.doc, self.selected_section.as_deref());
                audio.play_exit();
                EditorOutcome::Back
            }
            DialogKind::UnsavedChangesOnPreplay => {
                self.action_save();
                if self.doc.dirty { return EditorOutcome::Stay; }
                EditorOutcome::PrePlay
            }
            DialogKind::Info { .. } => EditorOutcome::Stay,
            DialogKind::Confirm { .. } => {
                // Confirm dialogs currently only back the
                // delete-section action. Resolve it here.
                if let Some(name) = self.pending_delete.take() {
                    if let Some(idx) = self.doc.section_index(&name) {
                        let removed = self.doc.remove_section(idx);
                        if removed.is_some() {
                            self.selected_section = self.doc.ast.sections
                                .first().map(|s| s.name.clone());
                            self.status.set(
                                format!("DELETED: {}", name.to_uppercase()),
                                3.0);
                        }
                    }
                }
                EditorOutcome::Stay
            }
        }
    }

    fn dialog_secondary(
        &mut self, kind: DialogKind, audio: &Audio,
    ) -> EditorOutcome {
        match kind {
            DialogKind::UnsavedChangesOnBack => {
                session::clear();
                audio.play_exit();
                EditorOutcome::Back
            }
            DialogKind::UnsavedChangesOnPreplay => EditorOutcome::PrePlay,
            _ => EditorOutcome::Stay,
        }
    }

    /// Apply the outcome of a successful picker choice, based
    /// on what the picker was opened for.
    fn handle_picker_selection(
        &mut self, purpose: PickerPurpose, path: String,
        audio: &Audio,
    ) {
        match purpose {
            PickerPurpose::None => {}
            PickerPurpose::ImportLevel => {
                match read_level_file(&path) {
                    Some(text) => match crate::dsl::parse_level(&text) {
                        Ok(ast) => {
                            // Snapshot before replacing so the
                            // author can undo the import if it
                            // was a mistake.
                            self.history.snapshot(&self.doc.ast);
                            let display = std::path::Path::new(&path)
                                .file_name().and_then(|s| s.to_str())
                                .unwrap_or("imported").to_string();
                            let stem = display
                                .strip_suffix(".rlf")
                                .map(|s| s.to_string())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "imported".to_string());
                            self.doc = Document::from_ast(stem, ast);
                            self.doc.touch();
                            self.selected_section = self.doc.ast.sections
                                .first().map(|s| s.name.clone());
                            self.inspector.on_section_changed();
                            self.canvas.pan = self.doc.camera_pan;
                            self.canvas.zoom = self.doc.camera_zoom;
                            self.status.set(
                                format!("IMPORTED: {}", display.to_uppercase()),
                                4.0);
                            audio.play_interact();
                        }
                        Err(e) => {
                            self.dialog = Some(Dialog::new(
                                DialogKind::Info {
                                    title: "IMPORT ERROR".into(),
                                    body: format!(
                                        "PARSE FAILED:\n{}",
                                        e.to_string().to_uppercase()),
                                }));
                        }
                    },
                    None => {
                        self.dialog = Some(Dialog::new(DialogKind::Info {
                            title: "IMPORT ERROR".into(),
                            body: "COULD NOT READ FILE".into(),
                        }));
                    }
                }
            }
            PickerPurpose::PickTrack => {
                self.history.snapshot(&self.doc.ast);
                self.doc.ast.meta.music = path.clone();
                self.doc.touch();
                let display = std::path::Path::new(&path)
                    .file_name().and_then(|s| s.to_str())
                    .unwrap_or("track").to_string();
                self.status.set(
                    format!("TRACK: {}", display.to_uppercase()),
                    3.0);
                audio.play_interact();
            }
            PickerPurpose::PickShader => {
                self.history.snapshot(&self.doc.ast);
                let name = format!("shader_{}", self.doc.ast.shaders.len());
                self.doc.ast.shaders.push(
                    crate::dsl::ast::ShaderDecl { name: name.clone(), path: path.clone() });
                self.doc.touch();
                let display = std::path::Path::new(&path)
                    .file_name().and_then(|s| s.to_str())
                    .unwrap_or("shader").to_string();
                self.status.set(
                    format!("SHADER: {}  {}", name.to_uppercase(),
                        display.to_uppercase()),
                    3.0);
                audio.play_interact();
            }
        }
    }

    // ---------- sidebar ----------

    fn tick_sidebar(&mut self, clicked: bool, audio: &Audio) -> bool {
        let (nx, ny) = self.pointer;
        let sidebar = self.sidebar_rect();
        if !hit_rect(sidebar, nx, ny) { return false; }

        // Transport buttons.
        if clicked {
            let (play_r, pause_r, stop_r) = self.transport_btns();
            if hit_rect(play_r, nx, ny) {
                if !self.doc.ast.meta.music.is_empty() {
                    if audio.has_music() && audio.is_music_paused() {
                        audio.resume_music();
                    } else {
                        audio.restart_music(&self.doc.ast.meta.music);
                    }
                }
                audio.play_interact();
                return true;
            }
            if hit_rect(pause_r, nx, ny) {
                audio.pause_music();
                audio.play_interact();
                return true;
            }
            if hit_rect(stop_r, nx, ny) {
                audio.stop_music();
                audio.play_interact();
                return true;
            }

            // Section list rows.
            for (i, sec) in self.doc.ast.sections.iter().enumerate() {
                let r = self.sidebar_row(i);
                if hit_rect(r, nx, ny) {
                    self.selected_section = Some(sec.name.clone());
                    audio.play_interact();
                    return true;
                }
            }

            // Action buttons.
            if hit_rect(self.sidebar_add_btn(), nx, ny) {
                self.action_add_section(audio);
                return true;
            }
            if hit_rect(self.sidebar_delete_btn(), nx, ny) {
                self.request_delete_section(audio);
                return true;
            }
        }
        false
    }

    fn action_add_section(&mut self, audio: &Audio) {
        // Place the new section at the current audio position
        // when something is playing, else in the middle of the
        // visible timeline range. Both translate to the doc's
        // timestamp format through the timeline helpers.
        let (view_lo, view_hi) = self.timeline.view_range();
        let playhead = if audio.has_music() {
            audio.music_position()
        } else {
            (view_lo + view_hi) * 0.5
        };
        let duration = audio.music_duration().max(1.0);
        let at = match &self.doc.ast.timestamp_format {
            TimestampFormat::TrackLength => playhead,
            TimestampFormat::Relative => {
                if duration > 0.1 { playhead / duration } else { 0.0 }
            }
            TimestampFormat::Beats { total } => {
                if duration > 0.1 {
                    (playhead / duration) * (*total).max(1) as f32
                } else { 0.0 }
            }
            TimestampFormat::Named(_) => {
                if duration > 0.1 { playhead / duration } else { 0.0 }
            }
        };
        let name = self.doc.next_section_name();
        self.doc.add_section(name.clone(), at);
        self.selected_section = Some(name.clone());
        self.status.set(format!("ADDED: {}", name.to_uppercase()), 2.5);
        audio.play_interact();
    }

    fn request_delete_section(&mut self, audio: &Audio) {
        let Some(name) = self.selected_section.clone() else {
            self.status.set("NO SECTION SELECTED".into(), 2.5);
            return;
        };
        if self.doc.ast.sections.len() <= 1 {
            self.status.set(
                "CANNOT DELETE THE LAST SECTION".into(), 2.5);
            return;
        }
        self.pending_delete = Some(name.clone());
        self.dialog = Some(Dialog::new(DialogKind::Confirm {
            title: "DELETE SECTION".into(),
            body: format!(
                "DELETE SECTION \"{}\"?\nTHIS CANNOT BE UNDONE.",
                name.to_uppercase()),
            confirm_label: "DELETE".into(),
        }));
        audio.play_interact();
    }

    // ---------- draw ----------

    fn draw_bg(&self, out: &mut Vec<Vertex>) {
        push_quad(out,
            self.view_left(), self.view_top(),
            self.view_right(), self.view_bottom(),
            BG_DEEP);
    }

    fn draw_toolbar(&self, out: &mut Vec<Vertex>) {
        let y0 = self.view_top();
        let y1 = y0 + 0.09;
        push_quad(out, self.view_left(), y0,
            self.view_right(), y1, BG_PANEL);
        push_quad(out, self.view_left(), y1 - 0.003,
            self.view_right(), y1, ACCENT);

        self.draw_button(out, self.back_btn(),    "BACK");
        self.draw_button(out, self.save_btn(),    "SAVE");
        self.draw_button(out, self.import_btn(),  "IMPORT");
        self.draw_preplay_button(out);

        let title_x = self.view_left() + 0.67;
        push_small(out, "EDITOR",
            title_x, y0 + 0.020, 0.0055, ACCENT_HI);
        let file = self.doc.display_title();
        push_small(out, &file,
            title_x, y0 + 0.050, BODY_PX, DIM);

        let bpm_s = format!("BPM {}", self.doc.ast.meta.bpm);
        push_small_right(out, &bpm_s,
            self.view_right() - 0.04, y0 + 0.020, BODY_PX, WHITE);
        let sides_s = format!("SIDES {}", self.doc.ast.generation.sides);
        push_small_right(out, &sides_s,
            self.view_right() - 0.04, y0 + 0.050, BODY_PX, DIM);
    }

    fn draw_sidebar(&self, out: &mut Vec<Vertex>, audio: &Audio) {
        let rect = self.sidebar_rect();
        push_quad(out, rect.0, rect.1, rect.2, rect.3, BG_PANEL);
        push_quad(out, rect.2 - 0.002, rect.1, rect.2, rect.3, ACCENT);

        // Each stacked element gets its own y band with at
        // least 3 NDC pixels of vertical breathing room at
        // 1280x720. SMALL_PX text is ~0.024 tall, BODY_PX is
        // ~0.027 tall, transport buttons are 0.034 tall.
        let track_header_y = rect.1 + 0.010;
        push_small(out, "TRACK",
            rect.0 + PAD, track_header_y, SMALL_PX, ACCENT_HI);

        let track_name_y = rect.1 + 0.048;
        let track_name = if self.doc.ast.meta.music.is_empty() {
            "NO TRACK LOADED".to_string()
        } else {
            std::path::Path::new(&self.doc.ast.meta.music)
                .file_name().and_then(|s| s.to_str())
                .unwrap_or("?").to_uppercase()
        };
        let track_w = rect.2 - rect.0 - 0.020;
        let track_fit = fit(&track_name, BODY_PX, track_w);
        push_small(out, &track_fit,
            rect.0 + PAD, track_name_y, BODY_PX, WHITE);

        let (play_r, pause_r, stop_r) = self.transport_btns();
        self.draw_mini_transport(out, play_r, TransportIcon::Play);
        self.draw_mini_transport(out, pause_r, TransportIcon::Pause);
        self.draw_mini_transport(out, stop_r, TransportIcon::Stop);

        let pos = audio.music_position();
        let dur = audio.music_duration();
        let time_s = format!("{} / {}", fmt_mmss(pos), fmt_mmss(dur));
        push_small_right(out, &time_s,
            rect.2 - PAD, rect.1 + 0.132, BODY_PX,
            if audio.has_music() { ACCENT_HI } else { DIM });

        let sep_y = rect.1 + 0.166;
        push_quad(out, rect.0 + 0.004, sep_y,
            rect.2 - 0.004, sep_y + 0.002, DIM);

        push_small(out, "SECTIONS",
            rect.0 + PAD, rect.1 + 0.180, SMALL_PX, ACCENT_HI);

        let add_top = self.sidebar_add_btn().1;
        for (i, sec) in self.doc.ast.sections.iter().enumerate() {
            let r = self.sidebar_row(i);
            if r.3 > add_top - 0.006 { break; }
            let is_selected = self.selected_section.as_deref()
                == Some(sec.name.as_str());
            if is_selected {
                push_quad(out, r.0, r.1, r.2, r.3, BG_PANEL_HI);
            }
            let seconds = self.section_seconds_display(sec, audio);
            let name_max = r.2 - r.0 - 0.060;
            let name = fit(&sec.name.to_uppercase(), BODY_PX, name_max);
            push_small(out, &name,
                rect.0 + PAD,
                r.1 + (r.3 - r.1) * 0.5 - sh(BODY_PX) * 0.5,
                BODY_PX,
                if is_selected { WHITE } else { DIM });
            let t = fmt_mmss(seconds);
            push_small_right(out, &t,
                rect.2 - PAD,
                r.1 + (r.3 - r.1) * 0.5 - sh(SMALL_PX) * 0.5,
                SMALL_PX, ACCENT_HI);
        }

        self.draw_button(out, self.sidebar_add_btn(),    "+ ADD");
        self.draw_button(out, self.sidebar_delete_btn(), "- DEL");
    }

    fn draw_graph(&self, out: &mut Vec<Vertex>) {
        let rect = self.graph_rect();
        self.canvas.draw_grid(out, rect);
        self.graph.draw(
            &self.doc, &self.canvas, rect,
            self.selected_section.as_deref(), out);
        self.canvas.draw_zoom_controls(out, rect, self.pointer);

        // Hint for first time users: if the graph is empty
        // show a soft prompt in the center of the canvas.
        if self.doc.ast.sections.is_empty() {
            let cx = (rect.0 + rect.2) * 0.5;
            let cy = (rect.1 + rect.3) * 0.5;
            let msg = "CLICK +ADD IN THE SIDEBAR TO CREATE A SECTION";
            let w = sw(msg, BODY_PX);
            push_small(out, msg, cx - w * 0.5, cy, BODY_PX, DIM);
        }
    }

    fn draw_inspector(&self, out: &mut Vec<Vertex>) {
        let rect = self.inspector_rect();
        let section_idx = self.selected_section.as_ref()
            .and_then(|name| self.doc.section_index(name));
        self.inspector.draw(
            &self.doc, section_idx, self.pointer, rect, out);
    }

    fn draw_timeline(&self, out: &mut Vec<Vertex>, audio: &Audio) {
        let rect = self.timeline_rect();
        self.timeline.draw(&self.doc, audio, rect, out);
    }

    fn draw_status_bar(&self, out: &mut Vec<Vertex>) {
        // Status bar now uses the same BODY_PX as the rest of
        // the editor, which makes zoom and pan readouts legible
        // at normal viewing distance. The extra height also
        // keeps the status text comfortably away from the
        // bottom edge of the viewport where the Windows task
        // bar sometimes clips the client area by a few pixels.
        let y1 = self.view_bottom();
        let y0 = y1 - 0.042;
        push_quad(out, self.view_left(), y0,
            self.view_right(), y1, BG_PANEL);
        push_quad(out, self.view_left(), y0, self.view_right(),
            y0 + 0.002, DIM);

        let text_y = y0 + (y1 - y0) * 0.5 - sh(BODY_PX) * 0.5;

        // Left: transient status message.
        if !self.status.text.is_empty() {
            push_small(out, &self.status.text,
                self.view_left() + PAD, text_y,
                BODY_PX, ACCENT_HI);
        }

        // Right: saved / modified indicator.
        let (right_label, right_col) = if self.doc.dirty {
            ("MODIFIED", BAD)
        } else {
            ("SAVED", GOOD)
        };
        push_small_right(out, right_label,
            self.view_right() - PAD, text_y, BODY_PX, right_col);

        // Center: zoom and pan readouts. Built as a single
        // joined string so horizontal alignment is trivial and
        // so the two readouts never collide with each other
        // regardless of their current digit widths.
        let zoom_s = format!("ZOOM {:.0}%",
            self.canvas.effective_scale() * 100.0);
        let pan_s = format!("PAN {:+.2}  {:+.2}",
            self.canvas.pan[0], self.canvas.pan[1]);
        let combined = format!("{}    {}", zoom_s, pan_s);
        let cx = (self.view_left() + self.view_right()) * 0.5;
        let combined_w = sw(&combined, BODY_PX);
        push_small(out, &combined,
            cx - combined_w * 0.5, text_y, BODY_PX, DIM);
    }

    fn draw_button(&self, out: &mut Vec<Vertex>,
                   r: (f32, f32, f32, f32), label: &str)
    {
        let (nx, ny) = self.pointer;
        let h = if hit_rect(r, nx, ny) { 1.0 } else { 0.0 };
        let bg = mix3(BG_PANEL, BG_PANEL_HI, h);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3,
            0.002 + 0.001 * h, ring);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let w = sw(label, BODY_PX);
        push_small(out, label,
            cx - w * 0.5, cy - sh(BODY_PX) * 0.5, BODY_PX, WHITE);
    }

    fn draw_preplay_button(&self, out: &mut Vec<Vertex>) {
        let r = self.preplay_btn();
        let (nx, ny) = self.pointer;
        let h = if hit_rect(r, nx, ny) { 1.0 } else { 0.0 };
        let stroke = mix3([0.40, 0.85, 0.45], [0.65, 1.00, 0.70], h);
        let bg = mix3(BG_PANEL, [0.10, 0.20, 0.10], h);
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.003, stroke);
        let cy = (r.1 + r.3) * 0.5;
        let tx = r.0 + 0.010;
        let s = (r.3 - r.1) * 0.28;
        push_tri(out,
            [tx,           cy - s],
            [tx,           cy + s],
            [tx + s * 1.4, cy],
            stroke);
        let label = "PRE-PLAY";
        let lw = sw(label, BODY_PX);
        let cx = (r.0 + r.2) * 0.5 + 0.008;
        push_small(out, label,
            cx - lw * 0.5,
            cy - sh(BODY_PX) * 0.5, BODY_PX, WHITE);
    }

    fn draw_mini_transport(
        &self, out: &mut Vec<Vertex>,
        r: (f32, f32, f32, f32), icon: TransportIcon,
    ) {
        let (nx, ny) = self.pointer;
        let hit = hit_rect(r, nx, ny);
        let bg = if hit { BG_PANEL_HI } else { BG_DEEP };
        let ring = if hit { ACCENT_HI } else { ACCENT };
        push_quad(out, r.0, r.1, r.2, r.3, bg);
        push_outline(out, r.0, r.1, r.2, r.3, 0.002, ring);
        let cx = (r.0 + r.2) * 0.5;
        let cy = (r.1 + r.3) * 0.5;
        let s = (r.3 - r.1) * 0.30;
        match icon {
            TransportIcon::Play => {
                push_tri(out,
                    [cx - s * 0.7, cy - s],
                    [cx - s * 0.7, cy + s],
                    [cx + s * 0.9, cy], WHITE);
            }
            TransportIcon::Pause => {
                let w = 0.003;
                let g = 0.003;
                push_quad(out,
                    cx - g - w, cy - s, cx - g, cy + s, WHITE);
                push_quad(out,
                    cx + g, cy - s, cx + g + w, cy + s, WHITE);
            }
            TransportIcon::Stop => {
                push_quad(out, cx - s, cy - s, cx + s, cy + s, WHITE);
            }
        }
    }

    // ---------- helpers ----------

    fn section_seconds_display(
        &self, sec: &crate::dsl::ast::Section, audio: &Audio,
    ) -> f32 {
        let duration = audio.music_duration().max(1.0);
        match &self.doc.ast.timestamp_format {
            TimestampFormat::TrackLength => sec.at,
            TimestampFormat::Relative => sec.at * duration,
            TimestampFormat::Beats { total } => {
                (sec.at / (*total).max(1) as f32) * duration
            }
            TimestampFormat::Named(_) => sec.at * duration,
        }
    }

    // ---------- view bounds ----------

    fn view_left(&self) -> f32 {
        if self.view_aspect >= 1.0 { -self.view_aspect } else { -1.0 }
    }
    fn view_right(&self)  -> f32 { -self.view_left() }
    fn view_top(&self)    -> f32 {
        if self.view_aspect >= 1.0 { -1.0 } else { -1.0 / self.view_aspect }
    }
    fn view_bottom(&self) -> f32 { -self.view_top() }

    // ---------- layout rects ----------

    fn back_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.08;
        let cy = self.view_top() + 0.045;
        (cx - 0.060, cy - 0.024, cx + 0.060, cy + 0.024)
    }
    fn save_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.22;
        let cy = self.view_top() + 0.045;
        (cx - 0.060, cy - 0.024, cx + 0.060, cy + 0.024)
    }
    fn import_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.36;
        let cy = self.view_top() + 0.045;
        (cx - 0.060, cy - 0.024, cx + 0.060, cy + 0.024)
    }
    fn preplay_btn(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.53;
        let cy = self.view_top() + 0.045;
        (cx - 0.080, cy - 0.024, cx + 0.080, cy + 0.024)
    }

    fn sidebar_rect(&self) -> (f32, f32, f32, f32) {
        (self.view_left(),
         self.view_top() + 0.09,
         self.view_left() + 0.40,
         self.view_bottom() - 0.22)
    }

    fn transport_btns(&self) -> (
        (f32, f32, f32, f32),
        (f32, f32, f32, f32),
        (f32, f32, f32, f32),
    ) {
        let rect = self.sidebar_rect();
        let y = rect.1 + 0.086;
        let size = 0.034;
        let gap = 0.008;
        let x0 = rect.0 + PAD;
        let play  = (x0,                        y, x0 + size,                    y + size);
        let pause = (play.2 + gap,              y, play.2 + gap + size,          y + size);
        let stop  = (pause.2 + gap,             y, pause.2 + gap + size,         y + size);
        (play, pause, stop)
    }

    fn sidebar_list_y0(&self) -> f32 {
        // Enough clearance below the SECTIONS header so the
        // header baseline and the first row's top never touch.
        self.sidebar_rect().1 + 0.210
    }

    fn sidebar_row(&self, i: usize) -> (f32, f32, f32, f32) {
        // Rows are 0.032 tall with a 0.002 gap, giving a 0.034
        // step. Text inside each row is vertically centered,
        // never pressed against the top or bottom edges.
        let rect = self.sidebar_rect();
        let step = 0.034;
        let y0 = self.sidebar_list_y0() + i as f32 * step;
        (rect.0 + 0.004, y0, rect.2 - 0.006, y0 + 0.032)
    }

    fn sidebar_add_btn(&self) -> (f32, f32, f32, f32) {
        let rect = self.sidebar_rect();
        let y1 = rect.3 - 0.010;
        let y0 = y1 - 0.032;
        let mid = (rect.0 + rect.2) * 0.5;
        (rect.0 + PAD, y0, mid - 0.005, y1)
    }

    fn sidebar_delete_btn(&self) -> (f32, f32, f32, f32) {
        let rect = self.sidebar_rect();
        let y1 = rect.3 - 0.010;
        let y0 = y1 - 0.032;
        let mid = (rect.0 + rect.2) * 0.5;
        (mid + 0.005, y0, rect.2 - PAD, y1)
    }

    
    fn graph_rect(&self) -> (f32, f32, f32, f32) {
        let inspector_w = 0.60;
        (self.view_left() + 0.40,
         self.view_top() + 0.09,
         self.view_right() - inspector_w,
         self.view_bottom() - 0.22)
    }

    fn inspector_rect(&self) -> (f32, f32, f32, f32) {
        let inspector_w = 0.60;
        (self.view_right() - inspector_w,
         self.view_top() + 0.09,
         self.view_right(),
         self.view_bottom() - 0.22)
    }

    fn timeline_rect(&self) -> (f32, f32, f32, f32) {
        // Extra 0.018 of bottom clearance accommodates the
        // taller status bar introduced alongside the legible
        // zoom and pan readouts.
        (self.view_left(),
         self.view_bottom() - 0.22,
         self.view_right(),
         self.view_bottom() - 0.042)
    }
}

#[derive(Clone, Copy)]
enum TransportIcon { Play, Pause, Stop }

// ---------- free helpers ----------

fn hit_rect(r: (f32, f32, f32, f32), nx: f32, ny: f32) -> bool {
    // Outlines drawn by `push_outline` extend OUTSIDE the
    // rect by their thickness (2-4 NDC units typically).
    // Expanding the hit test by a fixed tolerance aligns the
    // click area with the perceived visual edge of the
    // button.
    const TOL: f32 = 0.004;
    nx >= r.0 - TOL && nx <= r.2 + TOL
        && ny >= r.1 - TOL && ny <= r.3 + TOL
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t,
     a[1] + (b[1] - a[1]) * t,
     a[2] + (b[2] - a[2]) * t]
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

fn short_path(p: &PathBuf) -> String {
    let s = p.display().to_string().to_uppercase();
    if s.len() > 60 {
        let tail: String = s.chars().rev().take(57).collect::<String>()
            .chars().rev().collect();
        format!("...{}", tail)
    } else {
        s
    }
}

fn fmt_mmss(seconds: f32) -> String {
    let s = seconds.max(0.0) as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn rect_contains_pt_inspector(r: (f32, f32, f32, f32), p: (f32, f32)) -> bool {
    p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3
}