//! Persistent editor session.
//!
//! The session file remembers what the author was working on
//! the last time they opened the editor. On startup the editor
//! checks for a session and restores it so the author lands
//! exactly where they left off: same level, same camera, same
//! selection, same scroll. Exiting the editor writes a fresh
//! session over the old one.
//!
//! # Storage layout
//!
//! A single `editor_session.rlf` file next to the executable
//! (or under `assets/customlevels/` if that path is unreachable).
//! The file is a plain v3 level, with a `#[editor_session]`
//! comment at the top carrying the view state as simple
//! `key = value` lines.
//!
//! This format is intentionally the same as a playable level
//! so a session never accidentally becomes a schema the game
//! cannot read. The worst that happens on a format change is
//! the comment block is ignored and the camera resets.
//!
//! The session never holds partial or invalid data: the
//! serializer runs the AST through `serialize_v2` which
//! enforces v3 validity end to end, and the parser rejects
//! any corrupted blob the way it would reject a user
//! authored level.

use std::fs;
use std::path::PathBuf;

use crate::dsl::parse_level;
use crate::editor::document::{Document, NodePos};
use crate::editor::serialize::serialize_v2;

/// Metadata prepended to the session file as a comment
/// block. Parsed by `load` to reconstruct the camera and the
/// currently selected section. Everything in here is optional;
/// missing keys fall back to the document's defaults.
struct SessionMeta {
    file_stem: String,
    camera_pan: [f32; 2],
    camera_zoom: f32,
    selected_section: Option<String>,
}

impl SessionMeta {
    fn from_document(doc: &Document, selected: Option<&str>) -> Self {
        SessionMeta {
            file_stem: doc.file_stem.clone(),
            camera_pan: doc.camera_pan,
            camera_zoom: doc.camera_zoom,
            selected_section: selected.map(|s| s.to_string()),
        }
    }

    fn serialize(&self) -> String {
        let mut s = String::new();
        s.push_str("// editor_session_v1\n");
        s.push_str(&format!("// file_stem = {}\n", self.file_stem));
        s.push_str(&format!(
            "// camera_pan = {:.4}, {:.4}\n",
            self.camera_pan[0], self.camera_pan[1]));
        s.push_str(&format!(
            "// camera_zoom = {:.4}\n", self.camera_zoom));
        if let Some(ref sel) = self.selected_section {
            s.push_str(&format!("// selected_section = {}\n", sel));
        }
        s.push_str("//\n");
        s
    }

    fn parse(text: &str) -> Self {
        let mut m = SessionMeta {
            file_stem: "draft".into(),
            camera_pan: [0.0, 0.0],
            camera_zoom: 0.0,
            selected_section: None,
        };
        for raw in text.lines() {
            let line = raw.trim_start();
            if !line.starts_with("//") { break; }
            let body = line.trim_start_matches("//").trim();
            let Some(eq) = body.find('=') else { continue; };
            let key = body[..eq].trim();
            let val = body[eq + 1..].trim();
            match key {
                "file_stem" => m.file_stem = val.to_string(),
                "camera_pan" => {
                    let parts: Vec<&str> = val.split(',').collect();
                    if parts.len() == 2 {
                        if let (Ok(x), Ok(y)) = (
                            parts[0].trim().parse::<f32>(),
                            parts[1].trim().parse::<f32>(),
                        ) {
                            m.camera_pan = [x, y];
                        }
                    }
                }
                "camera_zoom" => {
                    if let Ok(z) = val.parse::<f32>() {
                        m.camera_zoom = z;
                    }
                }
                "selected_section" => {
                    m.selected_section = Some(val.to_string());
                }
                _ => {}
            }
        }
        m
    }
}

/// Absolute path of the session file. Matches the save path
/// resolution used by `Document::save_path` so the session
/// lives alongside authored level files.
fn session_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("assets").join("customlevels")
                .join(".editor_session.rlf");
        }
    }
    PathBuf::from("assets/customlevels/.editor_session.rlf")
}

/// Persist the current editor state to disk. Called on every
/// clean exit from the editor (Back button, main menu
/// transition) and on explicit Save, so a crash does not lose
/// more than the last few actions.
pub fn save(doc: &Document, selected_section: Option<&str>) -> Result<(), String> {
    let meta = SessionMeta::from_document(doc, selected_section);
    let mut text = meta.serialize();
    text.push_str(&serialize_v2(&doc.ast));
    let path = session_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&path, text).map_err(|e| e.to_string())
}

/// Restore the last editor session from disk, if present.
/// Returns `None` when the file is missing or unparseable,
/// letting the caller fall back to a fresh draft.
pub fn load() -> Option<RestoredSession> {
    let path = session_path();
    let text = fs::read_to_string(&path).ok()?;
    let meta = SessionMeta::parse(&text);
    let ast = parse_level(&text).ok()?;
    let mut doc = Document::from_ast(meta.file_stem.clone(), ast);
    doc.camera_pan = meta.camera_pan;
    doc.camera_zoom = meta.camera_zoom;
    // Loading a session treats the restored document as clean
    // because the author did not make any new changes beyond
    // what was already saved at the previous exit.
    doc.dirty = false;
    Some(RestoredSession {
        doc,
        selected_section: meta.selected_section,
    })
}

/// Bundle returned from `load`, paired so the editor shell
/// can apply both the document and the selection in one
/// step.
pub struct RestoredSession {
    pub doc: Document,
    pub selected_section: Option<String>,
}

/// Delete the session file. Used when the author explicitly
/// chooses "discard and exit" so the next open starts fresh.
pub fn clear() {
    let path = session_path();
    let _ = fs::remove_file(path);
}