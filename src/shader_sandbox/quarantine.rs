//! Persistent list of user shaders that the sandbox refuses
//! to load.
//!
//! The sandbox appends a shader identifier to this list when
//! a Vulkan device loss is attributed to that shader via
//! `ShaderSandbox::report_device_lost`. Future calls to
//! `load_file` check the list before running the compile
//! pipeline and fail fast with `ShaderError::Quarantined` if
//! the shader is present.
//!
//! Storage format is deliberately trivial. The file lives at
//! `rustogon_shader_quarantine.txt` next to the executable,
//! one identifier per line, blank and comment lines skipped.
//! A user who is sure their shader is safe can open the file
//! in a text editor and delete the offending line, which is
//! faster and more predictable than hiding the feature
//! behind a settings menu toggle.

use std::fs;
use std::path::PathBuf;

const FILE_NAME: &str = "rustogon_shader_quarantine.txt";

/// In memory mirror of the on disk quarantine file.
pub struct Quarantine {
    entries: Vec<String>,
}

impl Quarantine {
    /// Read the quarantine file from disk. Missing file, IO
    /// errors, and malformed lines are all treated as an
    /// empty list: no user shader becomes arbitrarily
    /// forbidden because of a transient disk error.
    pub fn load() -> Self {
        let mut q = Quarantine { entries: Vec::new() };
        let path = Self::resolve_path();
        if let Ok(text) = fs::read_to_string(&path) {
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if !q.entries.iter().any(|e| e == line) {
                    q.entries.push(line.to_string());
                }
            }
        }
        q
    }

    /// Is `id` present in the list?
    pub fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e == id)
    }

    /// Add `id` if it is not already recorded and flush the
    /// file immediately so the record survives a crash.
    pub fn insert(&mut self, id: &str) {
        if !self.contains(id) {
            self.entries.push(id.to_string());
            self.persist();
        }
    }

    /// Drop every entry and flush. The user triggers this
    /// from the settings UI after confirming their shader is
    /// safe.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.persist();
    }

    pub fn entries(&self) -> &[String] { &self.entries }

    /// Write the list back to disk. All errors are logged
    /// and swallowed; a read only working directory is
    /// annoying but not a reason to crash the game.
    fn persist(&self) {
        let path = Self::resolve_path();
        let mut text = String::new();
        text.push_str("# Super Rustogon shader quarantine.\n");
        text.push_str("# Remove a line to allow that shader ");
        text.push_str("to load again.\n\n");
        for e in &self.entries {
            text.push_str(e);
            text.push('\n');
        }
        if let Err(e) = fs::write(&path, text) {
            eprintln!("[quarantine] save failed at {}: {}",
                path.display(), e);
        }
    }

    fn resolve_path() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join(FILE_NAME);
            }
        }
        PathBuf::from(FILE_NAME)
    }
}