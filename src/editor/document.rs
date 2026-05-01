//! Document model for the editor.
//!
//! Wraps a `LevelAst` with all the state the editor needs to
//! render, mutate, persist, and track changes against.
//!
//! # Responsibilities
//!
//! * Authoritative storage for the `LevelAst` being edited.
//! * Per section graph layout (node positions on the canvas).
//!   Keyed by section name so adding / removing / reordering
//!   sections preserves the visual layout the author built.
//! * Dirty tracking: every mutator goes through a method that
//!   flips `dirty` to true. Saving clears it. The shell uses
//!   this to drive the "unsaved changes" dialog on exit.
//! * File stem and derived save path. Save writes as v3 via
//!   `editor::serialize::serialize_v2`; v3 is the default
//!   dialect for new drafts.
//!
//! The document does not know about rendering, input, or
//! Vulkan. It is pure data plus a small API surface used by
//! the rest of the editor modules.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::dsl::ast::*;
use crate::dsl::rules::LevelRules;
use crate::editor::serialize::serialize_v2;
use crate::levels::Palette;

/// Anchor position for one graph node, in canvas coordinates.
/// The canvas is an infinite plane; `(0, 0)` is the center of
/// the view on first open. Nodes are positioned relative to
/// this and transformed to screen space by the camera.
#[derive(Clone, Copy, Debug)]
pub struct NodePos {
    pub x: f32,
    pub y: f32,
}

impl Default for NodePos {
    fn default() -> Self { NodePos { x: 0.0, y: 0.0 } }
}

/// Complete editor document. One `Document` instance lives per
/// editor session and travels through pre-play / return / save
/// cycles without being rebuilt.
pub struct Document {
    /// Name of the file on disk, no extension. Save writes to
    /// `assets/customlevels/<file_stem>.rlf`.
    pub file_stem: String,

    /// The level itself. Every field on this AST is directly
    /// editable; the rest of the editor reads and writes it
    /// through the typed accessors below.
    pub ast: LevelAst,

    /// Node positions keyed by section name. Carries visual
    /// layout across section reorders (which happen every
    /// time an `at` value changes and the list re-sorts).
    pub node_positions: HashMap<String, NodePos>,

    /// Pan offset of the graph camera on the canvas. Restored
    /// from the session file on open.
    pub camera_pan: [f32; 2],

    /// Logarithmic zoom of the graph camera. Values above 0
    /// zoom in, below 0 zoom out. Clamped inside the canvas
    /// module when applied.
    pub camera_zoom: f32,

    /// Whether the document has unsaved changes since the last
    /// successful save or load.
    pub dirty: bool,
}

impl Document {
    /// Create a fresh draft with the v3 default rules, one
    /// empty section, and the `TrackLength` timestamp format
    /// so every `at` value the author enters is treated as
    /// absolute seconds. This matches author intuition once a
    /// music track is loaded: "this section starts at 42s".
    pub fn new_draft(file_stem: String) -> Self {
        let mut ast = LevelAst::default();
        ast.meta.name = "DRAFT".into();
        ast.meta.subtitle = "EDITOR".into();
        ast.meta.author = "YOU".into();
        ast.meta.song = "MENU".into();
        ast.meta.bpm = 130;
        ast.meta.description =
            "A new level created in the editor.".into();
        ast.palette = Palette::default();
        ast.rules = LevelRules::default();
        ast.timestamp_format = TimestampFormat::TrackLength;
        ast.sections.push(Section {
            name: "intro".into(),
            at: 0.0,
            body: vec![Stmt::Emit(ObstacleSpec::Bar {
                thickness_mult: 1.0,
            })],
        });
        let mut doc = Document {
            file_stem,
            ast,
            node_positions: HashMap::new(),
            camera_pan: [0.0, 0.0],
            camera_zoom: 0.0,
            dirty: false,
        };
        doc.autoplace_nodes();
        doc
    }

    /// Import an existing AST (from disk or from the catalogue)
    /// and derive a fresh editing document from it. Nodes are
    /// auto-placed along the x axis in proportion to their `at`
    /// values so first open does not look like all sections
    /// collapsed onto the origin.
    pub fn from_ast(file_stem: String, ast: LevelAst) -> Self {
        let mut doc = Document {
            file_stem,
            ast,
            node_positions: HashMap::new(),
            camera_pan: [0.0, 0.0],
            camera_zoom: 0.0,
            dirty: false,
        };
        doc.autoplace_nodes();
        doc
    }

    /// Ensure every section has a node position, laying out
    /// any missing ones horizontally by their `at` value and
    /// vertically along a single row for now. Called on
    /// construction and after section insertions.
    pub fn autoplace_nodes(&mut self) {
        let spread = 2.4f32;
        for sec in &self.ast.sections {
            if self.node_positions.contains_key(&sec.name) {
                continue;
            }
            let x = (sec.at - 0.5) * spread * 2.0;
            self.node_positions.insert(
                sec.name.clone(),
                NodePos { x, y: 0.0 },
            );
        }
    }

    /// Mark the document as modified. All mutators should
    /// funnel through this so the dirty bit is always accurate.
    pub fn touch(&mut self) { self.dirty = true; }

    /// Update a section's `at` timestamp by its index. Returns
    /// the new index after the sort so callers who track the
    /// currently selected section can follow it.
    pub fn set_section_at(&mut self, idx: usize, new_at: f32) -> usize {
        if idx >= self.ast.sections.len() { return idx; }
        let clamped = new_at.clamp(0.0, 1.0e6);
        self.ast.sections[idx].at = clamped;
        self.touch();
        let moved_name = self.ast.sections[idx].name.clone();
        self.ast.sections.sort_by(|a, b|
            a.at.partial_cmp(&b.at).unwrap());
        self.ast.sections.iter()
            .position(|s| s.name == moved_name)
            .unwrap_or(idx)
    }

    /// Insert a blank section and auto-place its node near the
    /// caret position (the average of existing node positions
    /// plus a small downward offset).
    pub fn add_section(&mut self, name: String, at: f32) {
        let section = Section {
            name: name.clone(),
            at: at.clamp(0.0, 1.0e6),
            body: Vec::new(),
        };
        let pos = self.node_positions.values()
            .fold([0.0f32; 2], |a, n| [a[0] + n.x, a[1] + n.y]);
        let n = self.node_positions.len().max(1) as f32;
        let center = NodePos { x: pos[0] / n, y: pos[1] / n + 1.2 };
        self.node_positions.insert(name, center);
        self.ast.sections.push(section);
        self.ast.sections.sort_by(|a, b|
            a.at.partial_cmp(&b.at).unwrap());
        self.touch();
    }

    /// Remove the section at `idx`. Returns the name that was
    /// removed so the caller can clean up references (selection
    /// indices, inspector state) accordingly.
    pub fn remove_section(&mut self, idx: usize) -> Option<String> {
        if idx >= self.ast.sections.len() { return None; }
        if self.ast.sections.len() == 1 { return None; }
        let removed = self.ast.sections.remove(idx);
        self.node_positions.remove(&removed.name);
        self.touch();
        Some(removed.name)
    }

    /// Move a section's graph node on the canvas. No AST
    /// mutation, pure layout change, but still marks the
    /// document dirty so the layout persists across saves.
    pub fn move_node(&mut self, section_name: &str, x: f32, y: f32) {
        if let Some(p) = self.node_positions.get_mut(section_name) {
            p.x = x;
            p.y = y;
            self.touch();
        }
    }

    /// Resolve the save path for this document. Portable
    /// against `cargo run` from the repo root as well as a
    /// packaged binary launched by double click.
    pub fn save_path(&self) -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join("assets").join("customlevels")
                    .join(format!("{}.rlf", self.file_stem));
            }
        }
        PathBuf::from("assets/customlevels")
            .join(format!("{}.rlf", self.file_stem))
    }

    /// Serialize and write the document to disk. Returns the
    /// path written to on success, or an error string. Clears
    /// the dirty bit on success so subsequent close attempts do
    /// not prompt for confirmation again.
    pub fn save(&mut self) -> Result<PathBuf, String> {
        let path = self.save_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let text = serialize_v2(&self.ast);
        fs::write(&path, text).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(path)
    }

    /// Cheap identifier for the currently edited file used by
    /// the status bar. Prefixed with `*` when the document has
    /// unsaved changes.
    pub fn display_title(&self) -> String {
        if self.dirty {
            format!("*{}.rlf", self.file_stem)
        } else {
            format!("{}.rlf", self.file_stem)
        }
    }

    /// Index of the section with the given name. Used by the
    /// editor shell when a sub module (graph drag, timeline
    /// drag) reports a selection change by name and the shell
    /// needs to mutate that section's body through an index
    /// into `ast.sections`.
    pub fn section_index(&self, name: &str) -> Option<usize> {
        self.ast.sections.iter().position(|s| s.name == name)
    }

    /// Pick the next unused `section_N` identifier. Called by
    /// `add_section_at` so newly added sections get unique
    /// names without bothering the author.
    pub fn next_section_name(&self) -> String {
        let mut n = self.ast.sections.len() + 1;
        loop {
            let candidate = format!("section_{}", n);
            if !self.ast.sections.iter().any(|s| s.name == candidate) {
                return candidate;
            }
            n += 1;
        }
    }
    
}