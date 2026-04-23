//! Level catalogue.
//!
//! Levels are described in the Rustogon Level Format (`.rlf`,
//! implemented by [`crate::dsl`]). On startup the catalogue scans
//!
//! 1. `assets/levels/*.rlf`        (stock roster)
//! 2. `assets/customlevels/*.rlf`  (user additions)
//!
//! If neither directory exists (running from `cargo run` before the
//! assets are copied next to the executable), a built in fallback
//! roster is parsed from string literals in `embedded.rs`.
//!
//! The public API kept here is intentionally small:
//!
//! * [`all`] returns the current slice of levels;
//! * [`num`] returns how many there are;
//! * [`get`] returns one by index, clamped.
//!
//! All three are backed by a `OnceLock` initialized on first call.
//! The rest of the engine (menu, game) only ever holds a
//! `&'static Level` borrow into that lock, which is safe because
//! the lock outlives the process.

pub mod difficulty;
pub mod embedded;

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

pub use difficulty::{Tier, TIER_ORDER};

use crate::dsl::ast::LevelAst;

/// Visual palette. Kept identical in layout to the old
/// hand written struct so existing renderer code does not care
/// whether the palette came from a static table or a DSL file.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg_a:        [f32; 3],
    pub bg_b:        [f32; 3],
    pub center_fill: [f32; 3],
    pub center_ring: [f32; 3],
    pub wall:        [f32; 3],
    pub player:      [f32; 3],
    pub accent:      [f32; 3],
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            bg_a:        [0.10, 0.04, 0.16],
            bg_b:        [0.05, 0.02, 0.10],
            center_fill: [0.05, 0.02, 0.08],
            center_ring: [1.00, 0.40, 0.70],
            wall:        [1.00, 0.40, 0.70],
            player:      [1.00, 0.95, 1.00],
            accent:      [1.00, 0.40, 0.70],
        }
    }
}

/// A fully resolved, ready to play level.
///
/// Ownership: every string / vector is owned (not `'static`). The
/// catalogue hands out `&'static Level` because its storage lives
/// inside a `OnceLock` that never drops, but an individual `Level`
/// could in principle be passed around by value.
pub struct Level {
    pub name:        String,
    pub subtitle:    String,
    pub description: String,
    pub song:        String,
    pub artist:      String,

    pub music_path: String,
    pub music_bpm:  u32,

    pub palette:         Palette,
    pub hue_shift_speed: f32,

    pub sides: u32,
    pub seed:  u32,

    pub base_tier: Tier,
    pub min_tier:  Tier,
    pub max_tier:  Tier,

    /// Display names for the tier picker shown in the menu. One
    /// entry per tier between `min_tier` and `max_tier` inclusive.
    pub difficulty_names: Vec<String>,

    /// Legacy multiplier table. Each entry corresponds by index to
    /// `difficulty_names` and is derived from the associated tier's
    /// `wall_speed_mult`. Kept so the menu and game code can keep
    /// using a numeric multiplier under the hood.
    pub difficulty_mults: Vec<f32>,

    /// Tiers parallel to the two vectors above. Passed to the game
    /// so the generator can scale its own knobs from the same
    /// source of truth as the menu.
    pub difficulty_tiers: Vec<Tier>,

    /// Raw AST as parsed from the `.rlf` source. The generator
    /// borrows sections from here.
    pub ast: LevelAst,
}

impl Level {
    /// Materialize a playable level from a parsed AST.
    pub fn from_ast(ast: LevelAst) -> Self {
        let (min_tier, max_tier, base_tier) = (
            ast.difficulty.min_tier,
            ast.difficulty.max_tier,
            ast.difficulty.base_tier,
        );
        let mut tiers = Vec::new();
        for t in TIER_ORDER.iter().copied() {
            if t.rank() >= min_tier.rank() && t.rank() <= max_tier.rank() {
                tiers.push(t);
            }
        }
        let difficulty_names = tiers.iter().map(|t| t.display_name()).collect();
        let difficulty_mults = tiers.iter().map(|t| t.wall_speed_mult()).collect();

        Level {
            name:            ast.meta.name.clone(),
            subtitle:        ast.meta.subtitle.clone(),
            description:     ast.meta.description.clone(),
            song:            ast.meta.song.clone(),
            artist:          ast.meta.author.clone(),
            music_path:      ast.meta.music.clone(),
            music_bpm:       ast.meta.bpm,
            palette:         ast.palette,
            hue_shift_speed: ast.generation.hue_speed,
            sides:           ast.generation.sides,
            seed:            ast.generation.seed,
            base_tier,
            min_tier,
            max_tier,
            difficulty_names,
            difficulty_mults,
            difficulty_tiers: tiers,
            ast,
        }
    }

    /// Index of `base_tier` inside `difficulty_tiers`, or a middle
    /// value if the base is out of range (should not happen but we
    /// clamp defensively).
    pub fn default_difficulty_index(&self) -> usize {
        self.difficulty_tiers
            .iter()
            .position(|t| *t == self.base_tier)
            .unwrap_or(self.difficulty_tiers.len() / 2)
    }
}

static CATALOGUE: OnceLock<Vec<Level>> = OnceLock::new();

/// Return a borrow of the whole catalogue, initializing it on the
/// first call. Subsequent calls are cheap.
pub fn all() -> &'static [Level] {
    CATALOGUE.get_or_init(load_catalogue).as_slice()
}

/// Number of levels currently in the catalogue. Always at least one
/// because the loader falls back to the embedded roster.
pub fn num() -> usize { all().len() }

/// Clamped getter by index.
pub fn get(idx: u32) -> &'static Level {
    let xs = all();
    let i = (idx as usize).min(xs.len().saturating_sub(1));
    &xs[i]
}

fn load_catalogue() -> Vec<Level> {
    let mut levels: Vec<Level> = Vec::new();

    // Stock roster on disk, if present.
    read_folder("assets/levels", &mut levels);
    read_folder_from_exe("assets/levels", &mut levels);

    // User roster on disk, if present.
    read_folder("assets/customlevels", &mut levels);
    read_folder_from_exe("assets/customlevels", &mut levels);

    // Final fallback: compile time embedded levels. We always try
    // these because on a fresh checkout assets/levels/ may not
    // exist next to the binary yet.
    if levels.is_empty() {
        for src in embedded::EMBEDDED_LEVELS {
            match crate::dsl::parse_level(src) {
                Ok(ast)  => levels.push(Level::from_ast(ast)),
                Err(e)   => eprintln!("[levels] embedded parse error: {}", e),
            }
        }
    }

    if levels.is_empty() {
        eprintln!("[levels] no playable levels loaded, game will crash on entry");
    }
    levels
}

/// Read `*.rlf` from a path relative to CWD.
fn read_folder(rel: &str, out: &mut Vec<Level>) {
    let path = Path::new(rel);
    ingest_folder(path, out);
}

/// Read `*.rlf` from a path relative to the executable directory.
fn read_folder_from_exe(rel: &str, out: &mut Vec<Level>) {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join(rel);
            ingest_folder(&path, out);
        }
    }
}

fn ingest_folder(path: &Path, out: &mut Vec<Level>) {
    let entries = match fs::read_dir(path) {
        Ok(e)  => e,
        Err(_) => return,
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("rlf") { continue; }
        let text = match fs::read_to_string(&p) {
            Ok(t)  => t,
            Err(err) => {
                eprintln!("[levels] read {} failed: {}", p.display(), err);
                continue;
            }
        };
        match crate::dsl::parse_level(&text) {
            Ok(ast) => {
                // De duplicate by music path + level name so reloading
                // a file from both cwd and exe directory does not
                // double up.
                let name = ast.meta.name.clone();
                let music = ast.meta.music.clone();
                if out.iter().any(|l| l.name == name && l.music_path == music) {
                    continue;
                }
                out.push(Level::from_ast(ast));
            }
            Err(err) => {
                eprintln!("[levels] parse {} failed: {}", p.display(), err);
            }
        }
    }
}