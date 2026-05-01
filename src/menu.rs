//! Application shell: intro, main menu, level browser, options.
//!
//! Anchor-driven layout. Every widget rectangle is built via
//! `Layout::anchor_rect_ndc` (or composed from the cached
//! viewport edges in `self.ui.layout`) so the screen adapts
//! to any aspect ratio without the menu code re-deriving the
//! aspect math itself. Magic Y constants from earlier
//! revisions (HEADER_Y, MOD_SEPARATOR_Y, ...) became inward
//! offsets from the corresponding viewport edge: 0.10 from
//! the top, 0.22 from the bottom, etc. The visible game-space
//! coordinates of the legacy renderer are reproduced bit for
//! bit at 16:9, while ultrawide and portrait monitors now see
//! the same panels positioned correctly relative to their own
//! visible edges instead of clipped or floating in space.
//!
//! The screen state machine and the public API are unchanged
//! from earlier revisions: `AppState` drives screen routing,
//! `EditorBoot` carries the editor handoff, `AudioSnapshot`
//! brings audio state in once per frame.
//!
//! Widget lifecycle: `update()` snapshots input through
//! `Ui::begin_frame`, runs the active screen which writes
//! geometry into the Ui's per-frame buffers, and updates the
//! music panel. `build_geometry()` drains those buffers into
//! the caller's shape and text vertex vectors. The split is
//! preserved so main.rs does not have to change.
//!
//! Music panel: always visible as a thin peek bar along the
//! viewport top when a track is loaded; opens on hover into
//! the panel rectangle, closes when the pointer leaves.
//! Trigger zone tracks the live drawn bottom edge so there is
//! no phantom hit area.
//!
//! Level browser: BACK button on the left, header text next
//! to it, slanted accent rail filling the centre band, level
//! card on the right of the rail, modifier strip pinned to
//! the bottom. The slanted accent line runs from
//! `RAIL_RIGHT_TOP` at the top separator to
//! `RAIL_RIGHT_BOTTOM` at the bottom separator; rail row
//! right edges are computed from the same line so they never
//! drift away from it.
//!
//! Settings: tabbed layout (graphics, audio, gameplay,
//! accessibility, controls). Each tab is a flat list of rows
//! drawn by menu-local helpers so the visual tuning of the
//! previous revision (track padding, arrow lean, hex handle
//! size) is preserved exactly.

use crate::audio::Audio;
use crate::config::{
    Ability, ColorblindMode, Config, CustomShaders, HighlightMode,
    ParticleDensity, VsyncMode,
};
use crate::font::{self, TextVertex};
use crate::levels::Palette;
use crate::pipeline::Vertex;
use crate::ui::draw::{
    push_hex, push_hex_alpha, push_hex_ring, push_hex_ring_alpha,
    push_hex_ring_rot, push_hex_rot, push_outline, push_outline_alpha,
    push_quad, push_quad_alpha, push_tri, push_tri_alpha,
};
use crate::ui::{mix3, smoothstep, Anchor, Rect, Ui};
use crate::win32::{Input, Mouse};

const TAU: f32 = std::f32::consts::TAU;
const BG_OUTER_R: f32 = 5.0;

pub const MENU_ENTER_MUSIC_PATH: &str = "assets/music/menu_enter.qoa";
pub const MENU_MUSIC_PATH:       &str = "assets/music/menu.qoa";
pub const MENU_BPM: u32 = 128;
pub const INTRO_SECONDS: f32 = 2.6;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Intro,
    MainMenu,
    LevelSelect,
    Settings,
    Editor,
    EditorPlay { tier_idx: u32 },
    Playing { level: u32, difficulty_idx: u32 },
    Quit,
}

type WidgetId = u32;

// ----- widget ids, preserved from the original revision -----
const ID_PLAY:        WidgetId = 1;
const ID_OPTIONS:     WidgetId = 2;
const ID_QUIT:        WidgetId = 3;
const ID_EDITOR:      WidgetId = 4;
const ID_BACK:        WidgetId = 10;
const ID_RESET:       WidgetId = 11;
const ID_PLAY_LEVEL:  WidgetId = 12;
const ID_DIFF_LEFT:   WidgetId = 13;
const ID_DIFF_RIGHT:  WidgetId = 14;
const ID_MUSIC_PAUSE: WidgetId = 300;
const ID_MUSIC_STOP:  WidgetId = 301;
const ID_MUSIC_BAR:   WidgetId = 302;
fn id_level_row(i: usize) -> WidgetId { 1000 + i as WidgetId }

const ID_TAB_GFX: WidgetId = 400;
const ID_TAB_AUD: WidgetId = 401;
const ID_TAB_GPL: WidgetId = 402;
const ID_TAB_ACC: WidgetId = 403;
const ID_TAB_CTL: WidgetId = 404;

const ID_GFX_VSYNC_L:    WidgetId = 410;
const ID_GFX_VSYNC_R:    WidgetId = 411;
const ID_GFX_FPS_L:      WidgetId = 412;
const ID_GFX_FPS_R:      WidgetId = 413;
const ID_GFX_BLOOM:      WidgetId = 414;
const ID_GFX_CHROMATIC:  WidgetId = 415;
const ID_GFX_SHAKE:      WidgetId = 416;
const ID_GFX_VIGNETTE:   WidgetId = 417;
const ID_GFX_BEAT_FLASH: WidgetId = 418;
const ID_GFX_DEPTH:      WidgetId = 419;
const ID_GFX_PART_L:     WidgetId = 420;
const ID_GFX_PART_R:     WidgetId = 421;
const ID_GFX_SCANLINES:  WidgetId = 422;
const ID_GFX_SHOW_FPS:   WidgetId = 423;
const ID_GFX_SHADERS_L:  WidgetId = 424;
const ID_GFX_SHADERS_R:  WidgetId = 425;

const ID_AUD_MASTER: WidgetId = 440;
const ID_AUD_MUSIC:  WidgetId = 441;
const ID_AUD_SFX:    WidgetId = 442;

const ID_GPL_PSPEED:    WidgetId = 450;
const ID_GPL_WOBBLE:    WidgetId = 451;
const ID_GPL_ABILITY_L: WidgetId = 452;
const ID_GPL_ABILITY_R: WidgetId = 453;
const ID_GPL_CLOSE_FX:  WidgetId = 454;
const ID_GPL_HL_L:      WidgetId = 455;
const ID_GPL_HL_R:      WidgetId = 456;

const ID_ACC_CONTRAST: WidgetId = 470;
const ID_ACC_REDUCE:   WidgetId = 471;
const ID_ACC_HITBOX:   WidgetId = 472;
const ID_ACC_CB_L:     WidgetId = 473;
const ID_ACC_CB_R:     WidgetId = 474;

// ----- typography -----
//
// The legacy renderer used a 5x7 bitmap font and expressed
// every text size as `pixel_size`, the side length of one
// glyph pixel. The visible glyph height was always
// `7 * pixel_size`. The SDF font's `pixel_size` argument is
// instead an em height. A multiplier of 8 takes a legacy
// pixel_size to an em that produces the same visual height.

const FONT_SCALE: f32 = 8.0;
const TITLE_PX:  f32 = 0.013  * FONT_SCALE;
const H1_PX:     f32 = 0.010  * FONT_SCALE;
const H2_PX:     f32 = 0.007  * FONT_SCALE;
const BUTTON_PX: f32 = 0.010  * FONT_SCALE;
const BODY_PX:   f32 = 0.005  * FONT_SCALE;
const SMALL_PX:  f32 = 0.0045 * FONT_SCALE;

// ----- palette -----
const ACCENT:      [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:   [f32; 3] = [1.00, 0.80, 0.92];
const DIM:         [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:       [f32; 3] = [1.00, 1.00, 1.00];
const BG_PANEL:    [f32; 3] = [0.10, 0.04, 0.15];
const BG_PANEL_HI: [f32; 3] = [0.22, 0.10, 0.30];
const BG_DEEP:     [f32; 3] = [0.03, 0.01, 0.06];

// ----- main menu bars (centred on screen) -----
const BAR_HW: f32 = 0.46;
const BAR_HH: f32 = 0.075;

// ----- music panel slide -----
const PANEL_PEEK_HEIGHT:     f32 = 0.028;
const PANEL_EXTENDED_HEIGHT: f32 = 0.26;
const PANEL_SLIDE_EASE:      f32 = 14.0;
const PANEL_CONTENT_LO:      f32 = 0.55;
const PANEL_CONTENT_HI:      f32 = 0.95;
const PANEL_INPUT_THRESHOLD: f32 = 0.60;

// ----- screen-band offsets from the viewport edges -----
//
// The legacy revision baked these as absolute Y values
// against a 16:9 viewport (HEADER_Y = -0.90, MOD_SEPARATOR_Y
// = 0.78, etc). They were broken on portrait or ultrawide
// displays. Re-expressed as inward distances from the
// nearest edge they collapse to identity on 16:9 and stay
// proportionally correct everywhere else.

/// Distance from `view_top` to the centre of the BACK / RESET
/// button row and the screen titles.
const HEADER_BAND_Y: f32 = 0.10;
/// Distance from `view_top` to the top separator line of the
/// level select rail.
const TOP_SEPARATOR_Y_INSET: f32 = 0.26;
/// Distance from `view_bottom` to the separator line that
/// divides the rail from the modifier strip.
const MOD_SEPARATOR_Y_INSET: f32 = 0.22;
/// Distance from `view_bottom` to the top edge of the
/// modifier row.
const MOD_ROW_TOP_INSET: f32 = 0.20;
/// Distance from `view_bottom` to the bottom edge of the
/// modifier row.
const MOD_ROW_BOT_INSET: f32 = 0.06;
/// Distance from `view_top` to the centre of the settings
/// tab bar.
const TAB_BAR_Y_INSET: f32 = 0.24;
/// Distance from `view_top` to the top edge of the settings
/// content band.
const SETTINGS_TOP_INSET: f32 = 0.36;
/// Distance from `view_bottom` to the bottom edge of the
/// settings content band.
const SETTINGS_BOT_INSET: f32 = 0.18;
/// Distance from `view_top` to the centre of the settings
/// "OPTIONS" title.
const SETTINGS_TITLE_INSET: f32 = 0.08;
/// Distance from `view_top` to the centre of the settings
/// subtitle line "TUNE THE FEEL".
const SETTINGS_SUBTITLE_INSET: f32 = 0.16;
/// Distance from `view_top` to the main menu title.
const MAIN_TITLE_Y_INSET: f32 = 0.20;
/// Distance from `view_bottom` to the main menu footer line.
const MAIN_FOOTER_INSET: f32 = 0.06;
/// Distance from `view_bottom` to the intro "click to skip"
/// hint.
const INTRO_HINT_INSET: f32 = 0.15;

// ----- rail diagonal -----
//
// Slope endpoints of the slanted accent edge that runs
// through the level select panel. Stored relative to the
// viewport centre because the diagonal is what gives the
// screen its visual identity; pulling these to the edges
// would lose the slant.

const RAIL_RIGHT_TOP:    f32 = -0.30;
const RAIL_RIGHT_BOTTOM: f32 =  0.55;
const RAIL_EXTEND_LEFT:  f32 =  5.00;

// ----- level row geometry -----
const ROW_HH:      f32 = 0.070;
const ROW_SPACING: f32 = 0.155;

// ----- card geometry -----
const CARD_WIDTH: f32 = 1.20;

// ----- palette tween -----
const PALETTE_EASE_RATE: f32 = 6.0;

struct MainItem { label: &'static str, sub: &'static str, id: WidgetId }
const MAIN_ITEMS: &[MainItem] = &[
    MainItem { label: "PLAY",    sub: "START A RUN",      id: ID_PLAY    },
    MainItem { label: "EDITOR",  sub: "AUTHOR A LEVEL",   id: ID_EDITOR  },
    MainItem { label: "OPTIONS", sub: "TUNE THE FEEL",    id: ID_OPTIONS },
    MainItem { label: "QUIT",    sub: "EXIT TO DESKTOP",  id: ID_QUIT    },
];

/// Boot directive handed from the menu to the editor when
/// the editor screen is entered. New means start with a
/// blank draft; Existing carries an index into the level
/// catalogue so the editor opens with that level's AST.
pub enum EditorBoot {
    New,
    Existing(u32),
}

/// Per-frame audio snapshot. Built once at the top of the
/// frame in main.rs and passed in to every consumer that
/// needs to read audio state without triggering atomics
/// multiple times itself.
pub struct AudioSnapshot {
    pub position:      f32,
    pub duration:      f32,
    pub paused:        bool,
    pub has_music:     bool,
    pub onset_phase:   f32,
    pub onset_counter: u32,
}

pub struct Menu {
    state: AppState,
    settings_tab: usize,

    time: f32,
    intro_time: f32,
    last_onset_time: f32,
    prev_onset_counter: u32,

    selected: u32,
    difficulty_idx: Vec<u32>,

    theme_bg_a:   [f32; 3],
    theme_bg_b:   [f32; 3],
    theme_accent: [f32; 3],

    panel_t: f32,
    editor_boot: Option<EditorBoot>,

    /// Immediate-mode UI context. Owns the per-widget hover
    /// map, drag tracker, input snapshot, layout, and the
    /// per-frame shape and text geometry buffers.
    ui: Ui,
}

impl Menu {
    pub fn new() -> Self {
        let n = crate::levels::num();
        let mut difficulty_idx = vec![0u32; n];
        for i in 0..n {
            let lvl = crate::levels::get(i as u32);
            difficulty_idx[i] = lvl.default_difficulty_index() as u32;
        }
        Menu {
            state: AppState::Intro,
            settings_tab: 0,
            time: 0.0,
            intro_time: 0.0,
            last_onset_time: -1.0,
            prev_onset_counter: 0,
            selected: 0,
            difficulty_idx,
            theme_bg_a:   BG_PANEL,
            theme_bg_b:   BG_DEEP,
            theme_accent: ACCENT,
            panel_t: 0.0,
            editor_boot: None,
            ui: Ui::new(),
        }
    }

    pub fn state(&self)          -> AppState { self.state }
    pub fn selected_level(&self) -> u32      { self.selected }

    pub fn desired_music(&self) -> Option<String> {
        match self.state {
            AppState::Intro    => Some(MENU_ENTER_MUSIC_PATH.to_string()),
            AppState::MainMenu | AppState::Settings =>
                Some(MENU_MUSIC_PATH.to_string()),
            AppState::LevelSelect => {
                let lvl = crate::levels::get(self.selected);
                if lvl.music_path.is_empty() {
                    None
                } else {
                    Some(lvl.music_path.clone())
                }
            }
            AppState::Editor | AppState::EditorPlay { .. } =>
                Some(MENU_MUSIC_PATH.to_string()),
            AppState::Playing { level, .. } => {
                let lvl = crate::levels::get(level);
                if lvl.music_path.is_empty() {
                    None
                } else {
                    Some(lvl.music_path.clone())
                }
            }
            AppState::Quit => None,
        }
    }

    pub fn desired_bpm(&self) -> u32 {
        match self.state {
            AppState::Intro
            | AppState::MainMenu
            | AppState::Settings
            | AppState::LevelSelect => MENU_BPM,
            AppState::Editor | AppState::EditorPlay { .. } => MENU_BPM,
            AppState::Playing { level, .. } =>
                crate::levels::get(level).music_bpm,
            AppState::Quit => MENU_BPM,
        }
    }

    pub fn return_to_main(&mut self) {
        self.state = AppState::MainMenu;
    }

    pub fn take_editor_boot(&mut self) -> EditorBoot {
        self.editor_boot.take().unwrap_or(EditorBoot::New)
    }

    pub fn enter_editor_play(&mut self, tier_idx: u32) {
        self.state = AppState::EditorPlay { tier_idx };
    }

    pub fn return_to_editor(&mut self) {
        self.state = AppState::Editor;
    }

    // ---------- view bound shortcuts ----------

    fn view_left(&self)   -> f32 { self.ui.layout.view_left   }
    fn view_right(&self)  -> f32 { self.ui.layout.view_right  }
    fn view_top(&self)    -> f32 { self.ui.layout.view_top    }
    fn view_bottom(&self) -> f32 { self.ui.layout.view_bottom }

    // ---------- screen-band Y coordinates -----------

    fn header_y(&self) -> f32 {
        self.view_top() + HEADER_BAND_Y
    }
    fn top_separator_y(&self) -> f32 {
        self.view_top() + TOP_SEPARATOR_Y_INSET
    }
    fn mod_separator_y(&self) -> f32 {
        self.view_bottom() - MOD_SEPARATOR_Y_INSET
    }
    fn mod_row_top_y(&self) -> f32 {
        self.view_bottom() - MOD_ROW_TOP_INSET
    }
    fn mod_row_bot_y(&self) -> f32 {
        self.view_bottom() - MOD_ROW_BOT_INSET
    }
    fn tab_bar_y(&self) -> f32 {
        self.view_top() + TAB_BAR_Y_INSET
    }
    fn settings_band_top(&self) -> f32 {
        self.view_top() + SETTINGS_TOP_INSET
    }
    fn settings_band_bot(&self) -> f32 {
        self.view_bottom() - SETTINGS_BOT_INSET
    }

    // ---------- panel geometry ----------

    fn panel_bot(&self) -> f32 {
        let top = self.view_top();
        let peek = top + PANEL_PEEK_HEIGHT;
        let ext  = top + PANEL_EXTENDED_HEIGHT;
        peek + (ext - peek) * self.panel_t
    }

    /// Sample point on the slanted rail accent line at game-
    /// space Y `y`. Maps from the top separator (t=0) to the
    /// modifier separator (t=1) along the line interpolated
    /// between `RAIL_RIGHT_TOP` and `RAIL_RIGHT_BOTTOM`.
    fn rail_right_at(&self, y: f32) -> f32 {
        let top = self.top_separator_y();
        let bot = self.mod_separator_y();
        let t = ((y - top) / (bot - top)).clamp(0.0, 1.0);
        RAIL_RIGHT_TOP + (RAIL_RIGHT_BOTTOM - RAIL_RIGHT_TOP) * t
    }

    // ---------- hit-rect helpers (anchored, return Rect) ----------

    fn back_button_rect(&self) -> Rect {
        // BACK button anchored to the top-left corner. The
        // historical (cx=view_left+0.13, cy=view_top+0.10)
        // position is now an inward offset from the top-left
        // anchor combined with the centre-relative button
        // half-extents.
        self.ui.layout.anchor_rect_ndc(
            Anchor::TopLeft,
            0.13 - 0.09, HEADER_BAND_Y - 0.040,
            0.18, 0.080,
        )
    }

    fn reset_button_rect(&self) -> Rect {
        self.ui.layout.anchor_rect_ndc(
            Anchor::TopRight,
            0.15 - 0.09, HEADER_BAND_Y - 0.040,
            0.18, 0.080,
        )
    }

    fn card_rect(&self) -> Rect {
        // Card pinned to the right edge, height stretches
        // from just below the music panel peek to just above
        // the modifier separator.
        let x1 = self.view_right() - 0.04;
        let x0 = x1 - CARD_WIDTH;
        let y0 = self.view_top()  + PANEL_PEEK_HEIGHT + 0.03;
        let y1 = self.mod_separator_y() - 0.02;
        Rect { x0, y0, x1, y1 }
    }

    fn music_panel_bar_range(&self) -> (f32, f32) {
        // Progress bar inside the music panel. Stays anchored
        // to the right edge with a generous left margin.
        let right = self.view_right() - 0.55;
        let left  = self.view_left()  + 0.80;
        (left, right)
    }

    fn tab_rect(&self, i: usize, n: usize) -> Rect {
        let vl = self.view_left()  + 0.08;
        let vr = self.view_right() - 0.08;
        let total = vr - vl;
        let gap = 0.010;
        let cell = (total - gap * (n as f32 - 1.0)) / n as f32;
        let x0 = vl + (cell + gap) * i as f32;
        let x1 = x0 + cell;
        let cy = self.tab_bar_y();
        let hh = 0.050;
        Rect { x0, y0: cy - hh, x1, y1: cy + hh }
    }

    fn row_center(&self, row: usize, total: usize) -> (f32, f32) {
        // Settings-screen content band. Rows are equally
        // spaced from the top of the band to the bottom; the
        // first row's centre sits half a row-height below the
        // top edge.
        let total = total.max(1);
        let top = self.settings_band_top();
        let bot = self.settings_band_bot();
        let row_h = (bot - top) / total as f32;
        let cy = top + row_h * (row as f32 + 0.5);
        (0.0, cy)
    }

    fn slider_track_range(&self) -> (f32, f32) {
        let vl = self.view_left() + 0.08;
        let x0 = vl + 0.52;
        let x1 = vl + 1.30;
        (x0, x1)
    }

    fn arrow_positions(&self) -> (f32, f32) {
        let vl = self.view_left() + 0.08;
        let lx = vl + 0.62;
        let rx = vl + 1.22;
        (lx, rx)
    }

    fn toggle_rect(&self, row: usize, rows: usize) -> Rect {
        let (_, cy) = self.row_center(row, rows);
        let vl = self.view_left() + 0.08;
        let cx = vl + 0.92;
        Rect::from_center(cx, cy, 0.12, 0.032)
    }

    fn modifier_strip_rect(&self) -> Rect {
        Rect {
            x0: self.view_left()  + 0.04,
            y0: self.mod_row_top_y(),
            x1: self.view_right() - 0.04,
            y1: self.mod_row_bot_y(),
        }
    }

    fn main_item_rect(&self, i: usize) -> Rect {
        let (cx, cy) = main_item_pos(i);
        Rect::from_center(cx, cy, BAR_HW, BAR_HH)
    }

    fn level_row_rect(&self, i: usize) -> Rect {
        // Row spans from the left edge to the diagonal accent
        // line, vertically centred on `row_y(i, selected)`.
        let y = row_y(i, self.selected);
        let xr = self.rail_right_at(y);
        let xl = self.view_left();
        Rect {
            x0: xl,
            y0: y - ROW_HH,
            x1: xr,
            y1: y + ROW_HH,
        }
    }

    // ---------- update ----------

    pub fn update(
        &mut self,
        dt: f32,
        mouse: Mouse,
        input: Input,
        cw: u32, ch: u32,
        audio: &Audio,
        snap: &AudioSnapshot,
        config: &mut Config,
    ) {
        self.time += dt;

        // Begin UI frame: snapshot input, recompute layout,
        // ease last frame's hover targets, clear targets and
        // geometry buffers.
        self.ui.begin_frame(dt, mouse, input, cw, ch);

        audio.set_volume(config.master_volume);

        if snap.onset_counter != self.prev_onset_counter {
            self.last_onset_time = self.time;
        }
        self.prev_onset_counter = snap.onset_counter;

        // Slide the music panel.
        let want_open = snap.has_music && {
            let py = self.ui.input().pointer.1;
            py >= self.view_top() && py <= self.panel_bot() + 0.015
        };
        let target_t = if want_open { 1.0 } else { 0.0 };
        let pb = 1.0 - (-PANEL_SLIDE_EASE * dt).exp();
        self.panel_t += (target_t - self.panel_t) * pb;
        if self.panel_t < 0.0005 { self.panel_t = 0.0; }
        if self.panel_t > 0.9995 { self.panel_t = 1.0; }

        // Continuous music bar drag (handled before screen
        // input so a dragging seek is not interrupted).
        if self.ui.is_dragging(ID_MUSIC_BAR) && snap.duration > 0.0 {
            let pointer = self.ui.input().pointer;
            let (bx0, bx1) = self.music_panel_bar_range();
            let t = ((pointer.0 - bx0) / (bx1 - bx0)).clamp(0.0, 1.0);
            audio.seek_music(t * snap.duration);
        }

        // Decide who owns pointer input this frame.
        let panel_hot = snap.has_music
            && self.panel_t >= PANEL_INPUT_THRESHOLD
            && self.ui.input().pointer.1 >= self.view_top()
            && self.ui.input().pointer.1 <= self.panel_bot();
        let dragging_bar = self.ui.is_dragging(ID_MUSIC_BAR);
        let screen_input = !panel_hot && !dragging_bar;

        // Background tunnel + orbiting hexes. Always drawn.
        self.render_background();

        // Active screen.
        match self.state {
            AppState::Intro       =>
                self.render_intro(dt, screen_input),
            AppState::MainMenu    =>
                self.render_main(audio, screen_input),
            AppState::LevelSelect =>
                self.render_level_select(audio, screen_input),
            AppState::Settings    =>
                self.render_settings(audio, config, screen_input),
            _ => {}
        }

        // Music panel renders on top so the slide stays
        // visible no matter what is below.
        self.render_music_panel(audio, snap);

        // Palette tween toward target. Affects next frame's
        // background colour.
        let (tga, tgb, tgac) = self.target_palette();
        let pblend = 1.0 - (-PALETTE_EASE_RATE * dt).exp();
        for i in 0..3 {
            self.theme_bg_a  [i] += (tga [i] - self.theme_bg_a  [i]) * pblend;
            self.theme_bg_b  [i] += (tgb [i] - self.theme_bg_b  [i]) * pblend;
            self.theme_accent[i] += (tgac[i] - self.theme_accent[i]) * pblend;
        }
    }

    fn target_palette(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        match self.state {
            AppState::LevelSelect => {
                let p = crate::levels::get(self.selected).palette;
                (p.bg_a, p.bg_b, p.accent)
            }
            _ => (BG_PANEL, BG_DEEP, ACCENT),
        }
    }

    fn beat_flash(&self) -> f32 {
        if self.last_onset_time < 0.0 { return 0.0; }
        let dt = (self.time - self.last_onset_time).max(0.0);
        (-dt * 7.0).exp()
    }

    // ---------- background ----------

    fn render_background(&mut self) {
        let flash = self.beat_flash();
        let t = self.time;
        let theme_bg_a = self.theme_bg_a;
        let theme_bg_b = self.theme_bg_b;
        let theme_accent = self.theme_accent;

        let geom = self.ui.geom_mut();
        let n = 12;
        let span = TAU / n as f32;
        let rot = t * 0.04;
        let a = mix3(theme_bg_a, WHITE, 0.04 * flash);
        let b = theme_bg_b;
        for s in 0..n {
            let a0 = s as f32 * span + rot;
            let a1 = a0 + span;
            let color = if s % 2 == 0 { a } else { b };
            push_tri(geom,
                [0.0, 0.0],
                [a0.cos() * BG_OUTER_R, a0.sin() * BG_OUTER_R],
                [a1.cos() * BG_OUTER_R, a1.sin() * BG_OUTER_R],
                color);
        }
        for i in 0..14 {
            let f = i as f32 / 14.0;
            let ang = f * TAU + t * 0.08;
            let orbit = 1.25 + ((t * 0.20 + f * 6.3).sin() * 0.35);
            let px = ang.cos() * orbit;
            let py = ang.sin() * orbit;
            let r = 0.011 + 0.004 * flash;
            let c = mix3([0.18, 0.07, 0.16], theme_accent, 0.25);
            push_hex(geom, px, py, r, c);
        }
    }

    // ---------- intro ----------

    fn render_intro(&mut self, dt: f32, consume_input: bool) {
        if consume_input {
            let input = self.ui.input();
            self.intro_time += dt;
            if self.intro_time >= INTRO_SECONDS
                || input.left_clicked
                || input.enter_edge
            {
                self.state = AppState::MainMenu;
            }
        }

        let t = self.intro_time;
        let d = INTRO_SECONDS;
        let fade_in  = smoothstep(0.0, 0.8, t);
        let fade_out = 1.0 - smoothstep(d - 0.6, d, t);
        let alpha    = fade_in * fade_out;

        // Geometry block: ring + central hex stack.
        {
            let geom = self.ui.geom_mut();
            let ring_t = smoothstep(0.0, 1.5, t);
            let ring_r = ring_t * 0.9;
            let count = 24;
            for i in 0..count {
                let ang = i as f32 / count as f32 * TAU;
                let x = ang.cos() * ring_r;
                let y = ang.sin() * ring_r;
                let size = 0.015 * alpha;
                let col = mix3(BG_DEEP, ACCENT, alpha);
                push_hex(geom, x, y, size, col);
            }
            let contract = 1.0 - 0.1 * smoothstep(d - 0.6, d, t);
            let r0 = 0.22 * contract;
            let r1 = 0.15 * contract;
            let r2 = 0.07 * contract;
            let tint = |c: [f32; 3]| mix3(BG_DEEP, c, alpha);
            push_hex_rot     (geom, 0.0, -0.05, r0, tint(BG_PANEL),     t * 0.4);
            push_hex_ring_rot(geom, 0.0, -0.05, r0, r0 - 0.012, tint(ACCENT),    t * 0.4);
            push_hex_rot     (geom, 0.0, -0.05, r1, tint(BG_PANEL_HI), -t * 0.9);
            push_hex_ring_rot(geom, 0.0, -0.05, r1, r1 - 0.010, tint(ACCENT_HI), -t * 0.9);
            push_hex_rot     (geom, 0.0, -0.05, r2, tint(WHITE),        t * 1.6);
        }

        let slide = (1.0 - fade_in) * 0.08;
        let hint_y = self.view_bottom() - INTRO_HINT_INSET;
        let text = self.ui.text_mut();
        font::push_text_centered_alpha(text, "SUPER RUSTOGON",
            0.0, 0.28 + slide, TITLE_PX, ACCENT_HI, alpha);
        font::push_text_centered_alpha(text, "A HEXAGONAL DESCENT",
            0.0, 0.42 + slide, H2_PX, WHITE, alpha * 0.7);
        font::push_text_centered_alpha(text, "CLICK OR ENTER TO SKIP",
            0.0, hint_y, SMALL_PX, DIM, (alpha * 0.8).max(0.0));
    }

    // ---------- main menu ----------

    fn render_main(&mut self, audio: &Audio, consume_input: bool) {
        let pointer = self.ui.input().pointer;
        let clicked = self.ui.input().left_clicked && consume_input;

        let view_l = self.view_left();
        let view_r = self.view_right();
        let view_b = self.view_bottom();
        let title_y = self.view_top() + MAIN_TITLE_Y_INSET;

        // Title block (drop shadow + main + subtitle).
        {
            let text = self.ui.text_mut();
            font::push_text_centered(text, "SUPER RUSTOGON",
                0.006, title_y + 0.010, TITLE_PX, BG_DEEP);
            font::push_text_centered(text, "SUPER RUSTOGON",
                0.000, title_y, TITLE_PX, ACCENT_HI);
            font::push_text_centered(text, "A HEXAGONAL DESCENT",
                0.0, title_y + 0.12, H2_PX, DIM);
        }

        // Decorative accent bars on either side of the title.
        let lw = 0.36;
        {
            let geom = self.ui.geom_mut();
            push_quad(geom, -0.52 - lw, title_y + 0.14,
                -0.52,      title_y + 0.145, ACCENT);
            push_quad(geom,  0.52,      title_y + 0.14,
                 0.52 + lw, title_y + 0.145, ACCENT);
        }

        // Item bars.
        let mut next_state: Option<AppState> = None;
        for (i, item) in MAIN_ITEMS.iter().enumerate() {
            let rect = self.main_item_rect(i);
            let hit = rect.contains(pointer.0, pointer.1);
            let h = self.ui.track_hover(item.id, hit);
            self.draw_menu_bar(rect, item.label, item.sub, h);
            if hit && clicked {
                audio.play_interact();
                next_state = Some(match item.id {
                    ID_PLAY    => AppState::LevelSelect,
                    ID_OPTIONS => AppState::Settings,
                    ID_EDITOR  => {
                        self.editor_boot = Some(EditorBoot::New);
                        AppState::Editor
                    }
                    ID_QUIT    => AppState::Quit,
                    _ => self.state,
                });
                self.ui.capture_pointer();
                break;
            }
        }
        if let Some(s) = next_state { self.state = s; return; }

        // Footer.
        let foot_y = view_b - MAIN_FOOTER_INSET;
        let text = self.ui.text_mut();
        font::push_text(text, "V0.2",
            view_l + 0.03, foot_y, SMALL_PX, DIM);
        font::push_text_centered(text, "MUSIC FROM OPENGAMEART",
            0.0, foot_y, SMALL_PX, DIM);
        font::push_text_right(text, "ESC TO QUIT",
            view_r - 0.03, foot_y, SMALL_PX, DIM);
    }

    fn draw_menu_bar(
        &mut self,
        rect: Rect,
        label: &str, sub: &str,
        h: f32,
    ) {
        let (cx, cy) = rect.center();
        let slide = h * 0.025;
        let bg   = mix3(BG_PANEL, BG_PANEL_HI, h);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        let x0 = rect.x0 + slide;
        let x1 = rect.x1 + slide;
        let y0 = rect.y0;
        let y1 = rect.y1;
        let _ = cx;

        {
            let geom = self.ui.geom_mut();
            push_quad(geom, x0, y0, x1, y1, bg);
            push_quad(geom, x0, y0, x0 + 0.024, y1, ring);
            if h > 0.01 {
                push_outline(geom, x0, y0, x1, y1, 0.004 + 0.002 * h, ring);
                let chev_col = mix3(bg, ring, 0.5 + 0.5 * h);
                push_tri(geom,
                    [x1 - 0.075, cy - 0.035],
                    [x1 - 0.075, cy + 0.035],
                    [x1 - 0.030, cy],
                    chev_col);
            }
        }

        let label_y = cy - font::text_height(BUTTON_PX) * 0.5 - 0.010;
        let sub_y   = label_y + font::text_height(BUTTON_PX) + 0.010;
        let text = self.ui.text_mut();
        font::push_text(text, label, x0 + 0.055, label_y, BUTTON_PX, WHITE);
        font::push_text(text, sub,   x0 + 0.055, sub_y,   SMALL_PX,  DIM);
    }

    // ---------- level select ----------

    fn render_level_select(&mut self, audio: &Audio, consume_input: bool) {
        let n = crate::levels::num();
        let input = self.ui.input();
        let pointer = input.pointer;
        let clicked    = input.left_clicked && consume_input;
        let up_edge    = input.up_edge      && consume_input;
        let down_edge  = input.down_edge    && consume_input;
        let enter_edge = input.enter_edge   && consume_input;

        // Keyboard navigation.
        if up_edge && self.selected > 0 {
            self.selected -= 1;
            audio.play_interact();
        }
        if down_edge && (self.selected as usize) + 1 < n {
            self.selected += 1;
            audio.play_interact();
        }
        if enter_edge {
            audio.play_enter();
            self.state = AppState::Playing {
                level: self.selected,
                difficulty_idx: self.difficulty_idx[self.selected as usize],
            };
            return;
        }

        // BACK button.
        let back_rect = self.back_button_rect();
        let back_hit = back_rect.contains(pointer.0, pointer.1);
        let back_h = self.ui.track_hover(ID_BACK, back_hit);
        self.draw_small_button(back_rect, "BACK", back_h);
        if back_hit && clicked {
            audio.play_interact();
            self.state = AppState::MainMenu;
            return;
        }

        // Title + nav hint.
        let title_x = back_rect.x1 + 0.04;
        let title_y = self.header_y() - font::text_height(TITLE_PX) * 0.5;
        let sub_y = title_y + font::text_height(TITLE_PX) + 0.008;
        {
            let text = self.ui.text_mut();
            font::push_text(text, "SELECT A LEVEL",
                title_x, title_y, TITLE_PX, ACCENT_HI);
            font::push_text(text, "UP / DOWN TO NAVIGATE - ENTER TO PLAY",
                title_x, sub_y, SMALL_PX, DIM);
        }

        // Level rail (rows + separators + slanted edge).
        if self.render_level_rail(pointer, clicked, audio) {
            return;
        }

        // Level card on the right of the slanted edge.
        let lvl = crate::levels::get(self.selected);
        let pal = lvl.palette;
        let t = self.time;
        let flash = self.beat_flash();
        self.render_level_card(lvl, &pal, t, flash, pointer, clicked, audio);

        // Modifier strip pinned to the bottom.
        self.render_modifier_row();
    }

    fn render_level_rail(
        &mut self,
        pointer: (f32, f32), clicked: bool,
        audio: &Audio,
    ) -> bool {
        let n = crate::levels::num();
        let view_l = self.view_left();
        let view_r = self.view_right();
        let top_sep = self.top_separator_y();
        let mod_sep = self.mod_separator_y();

        // Rows. Drawn from earliest to latest so adjacent
        // rows visually overlap correctly when the row stripe
        // crosses the cell boundary.
        let mut row_clicked: Option<u32> = None;
        for i in 0..n {
            let y = row_y(i, self.selected);
            if y < top_sep - 0.25 || y > mod_sep + 0.25 { continue; }
            let id = id_level_row(i);
            let rect = self.level_row_rect(i);
            let hit = rect.contains(pointer.0, pointer.1);
            let hover = self.ui.track_hover(id, hit);
            self.draw_level_row(y, i, hover);
            if hit && clicked {
                row_clicked = Some(i as u32);
            }
        }
        if let Some(idx) = row_clicked {
            if self.selected == idx {
                audio.play_enter();
                self.state = AppState::Playing {
                    level: idx,
                    difficulty_idx: self.difficulty_idx[idx as usize],
                };
                return true;
            }
            self.selected = idx;
            audio.play_interact();
            self.ui.capture_pointer();
            return true;
        }

        // Separators.
        {
            let geom = self.ui.geom_mut();
            push_quad(geom,
                view_l, top_sep, view_r, top_sep + 0.004, ACCENT);
            push_quad(geom,
                view_l, mod_sep, view_r, mod_sep + 0.004, ACCENT);
        }

        // Slanted accent edge running between separators.
        push_slanted_edge(self.ui.geom_mut(),
            RAIL_RIGHT_TOP, RAIL_RIGHT_BOTTOM,
            top_sep, mod_sep, 0.005, ACCENT);

        false
    }

    fn draw_level_row(
        &mut self, cy: f32, idx: usize, hover: f32,
    ) {
        let lvl = crate::levels::get(idx as u32);
        let pal = lvl.palette;
        let is_selected = idx as u32 == self.selected;
        let top_sep = self.top_separator_y();
        let mod_sep = self.mod_separator_y();

        let unclipped_top = cy - ROW_HH;
        let unclipped_bot = cy + ROW_HH;
        if unclipped_bot <= top_sep + 0.004 { return; }
        if unclipped_top >= mod_sep { return; }
        let y_top = unclipped_top.max(top_sep + 0.004);
        let y_bot = unclipped_bot.min(mod_sep);

        let xr_top = self.rail_right_at(y_top);
        let xr_bot = self.rail_right_at(y_bot);

        let view_l = self.view_left();
        let row_left_fill = view_l - RAIL_EXTEND_LEFT;

        if is_selected {
            let bg = mix3(BG_PANEL_HI, pal.bg_a, 0.50);
            push_trapezoid(self.ui.geom_mut(),
                row_left_fill, row_left_fill,
                xr_top, xr_bot, y_top, y_bot, bg);
            let delim = 0.004;
            let geom = self.ui.geom_mut();
            push_quad(geom, row_left_fill, y_top,
                xr_top, y_top + delim, pal.accent);
            push_quad(geom, row_left_fill, y_bot - delim,
                xr_bot, y_bot, pal.accent);
        } else if hover > 0.01 {
            let bg = mix3(BG_PANEL, BG_PANEL_HI, hover);
            push_trapezoid(self.ui.geom_mut(),
                row_left_fill, row_left_fill,
                xr_top, xr_bot, y_top, y_bot, bg);
        }

        let content_x = view_l + 0.02;
        let col_stripe_w = 0.022;
        let col_icon_w   = 0.090;
        let col_num_w    = 0.130;
        let col_gap      = 0.030;

        let stripe_top = (cy - ROW_HH + 0.006).max(top_sep + 0.008);
        let stripe_bot = (cy + ROW_HH - 0.006).min(mod_sep - 0.002);
        if stripe_bot > stripe_top {
            push_quad(self.ui.geom_mut(),
                content_x, stripe_top,
                content_x + col_stripe_w, stripe_bot,
                pal.accent);
        }

        if cy < top_sep + 0.006 { return; }
        if cy > mod_sep - 0.006 { return; }

        // Hex avatar.
        let hex_cx = content_x + col_stripe_w + col_icon_w * 0.5;
        {
            let geom = self.ui.geom_mut();
            push_hex(geom, hex_cx, cy, 0.035, pal.bg_a);
            push_hex_ring(geom, hex_cx, cy, 0.035, 0.028, pal.accent);
            push_hex(geom, hex_cx, cy, 0.014, pal.player);
        }

        // Number, name, subtitle.
        let num_col_left = content_x + col_stripe_w + col_icon_w;
        let num_col_right = num_col_left + col_num_w;
        let num = format!("{:02}", idx + 1);
        let num_color = if is_selected { pal.accent } else { DIM };
        let num_y = cy - font::text_height(H1_PX) * 0.5 - 0.005;

        let text_x = num_col_right + col_gap;
        let name_h = font::text_height(H1_PX);
        let sub_h  = font::text_height(SMALL_PX);
        let gap    = 0.005;
        let total_h = name_h + gap + sub_h;
        let name_y = cy - total_h * 0.5;
        let sub_y  = name_y + name_h + gap;

        let slant_limit = self.rail_right_at(name_y) - 0.04;

        let text = self.ui.text_mut();
        font::push_text_right(text, &num, num_col_right, num_y, H1_PX, num_color);
        if text_x < slant_limit {
            font::push_text(text, &lvl.name,     text_x, name_y, H1_PX,    WHITE);
            font::push_text(text, &lvl.subtitle, text_x, sub_y,  SMALL_PX, DIM);
        }
    }

    fn render_level_card(
        &mut self,
        lvl: &crate::levels::Level,
        pal: &Palette,
        t: f32, flash: f32,
        pointer: (f32, f32), clicked: bool,
        audio: &Audio,
    ) {
        let card = self.card_rect();
        let (x0, y0, x1, y1) = (card.x0, card.y0, card.x1, card.y1);

        // Card background + accent outline.
        {
            let geom = self.ui.geom_mut();
            push_quad(geom, x0, y0, x1, y1, mix3(BG_DEEP, pal.bg_a, 0.45));
            push_outline(geom, x0, y0, x1, y1, 0.004, pal.accent);
        }

        let pad = 0.035;
        let line_h_h1 = font::text_height(H1_PX) + 0.006;
        let line_h_h2 = font::text_height(H2_PX) + 0.005;
        let line_h_sm = font::text_height(SMALL_PX) + 0.004;

        // Rotating hex stack on the upper-left of the card.
        let hex_cx = x0 + 0.14;
        let hex_cy = y0 + 0.18;
        let r0 = 0.11;
        let r1 = 0.075;
        let r2 = 0.042;
        {
            let geom = self.ui.geom_mut();
            push_hex_rot     (geom, hex_cx, hex_cy, r0, pal.bg_a,           t * 0.20);
            push_hex_ring_rot(geom, hex_cx, hex_cy, r0, r0 - 0.012, pal.accent,       t * 0.20);
            push_hex_rot     (geom, hex_cx, hex_cy, r1, pal.bg_b,          -t * 0.45);
            push_hex_ring_rot(geom, hex_cx, hex_cy, r1, r1 - 0.010, pal.center_ring, -t * 0.45);
            push_hex_rot(geom, hex_cx, hex_cy, r2 + 0.006 * flash, pal.player, t * 0.85);
        }

        // Track / artist / tempo info on the upper right.
        let info_x = x0 + 0.30;
        let mut y = y0 + pad;
        {
            let text = self.ui.text_mut();
            font::push_text(text, "TRACK", info_x, y, SMALL_PX, DIM);
            y += line_h_sm;
            font::push_text(text, &lvl.song, info_x, y, H1_PX, pal.accent);
            y += line_h_h1 + 0.006;
            font::push_text(text, "ARTIST", info_x, y, SMALL_PX, DIM);
            y += line_h_sm;
            font::push_text(text, &lvl.artist, info_x, y, H2_PX, WHITE);
            y += line_h_h2 + 0.006;
            let bpm = format!("{} BPM", lvl.music_bpm);
            font::push_text(text, "TEMPO", info_x, y, SMALL_PX, DIM);
            y += line_h_sm;
            font::push_text(text, &bpm, info_x, y, H2_PX, pal.accent);
        }

        // Divider separating header from body.
        let divider_y = y0 + 0.45;
        push_quad(self.ui.geom_mut(),
            x0 + pad, divider_y, x1 - pad, divider_y + 0.004,
            mix3(BG_DEEP, pal.accent, 0.3));

        // Body: name, subtitle, description.
        let mut y = divider_y + 0.02;
        {
            let text = self.ui.text_mut();
            font::push_text(text, &lvl.name, x0 + pad, y, H1_PX, WHITE);
            y += line_h_h1;
            font::push_text(text, &lvl.subtitle, x0 + pad, y, SMALL_PX, DIM);
            y += line_h_sm + 0.006;
            font::push_text(text, "DESCRIPTION", x0 + pad, y, SMALL_PX, DIM);
            y += line_h_sm;
        }

        let max_w = (x1 - x0) - 2.0 * pad;
        push_text_wrapped(self.ui.text_mut(),
            &lvl.description, x0 + pad, y, max_w, BODY_PX, WHITE);

        // Difficulty selector.
        let dcx = (x0 + x1) * 0.5;
        let dy  = 0.38;

        let larrow = Rect::from_center(dcx - 0.30, dy, 0.06, 0.05);
        let rarrow = Rect::from_center(dcx + 0.30, dy, 0.06, 0.05);
        let lhit = larrow.contains(pointer.0, pointer.1);
        let rhit = rarrow.contains(pointer.0, pointer.1);
        let lh = self.ui.track_hover(ID_DIFF_LEFT, lhit);
        let rh = self.ui.track_hover(ID_DIFF_RIGHT, rhit);

        if lhit && clicked {
            audio.play_interact();
            let di = &mut self.difficulty_idx[self.selected as usize];
            if *di > 0 { *di -= 1; }
            self.ui.capture_pointer();
        }
        if rhit && clicked {
            audio.play_interact();
            let len = lvl.difficulty_tiers.len();
            let di = &mut self.difficulty_idx[self.selected as usize];
            if (*di as usize) + 1 < len { *di += 1; }
            self.ui.capture_pointer();
        }

        let di = self.difficulty_idx[self.selected as usize] as usize;
        let tier_label = lvl.difficulty_names.get(di).cloned()
            .unwrap_or_else(|| "NORMAL".to_string());
        let tier_mult  = lvl.difficulty_tiers.get(di)
            .map(|t| t.wall_speed_mult())
            .unwrap_or(1.0);

        {
            let text = self.ui.text_mut();
            font::push_text_centered(text, "DIFFICULTY", dcx,
                dy - 0.09 - font::text_height(SMALL_PX) * 0.5,
                SMALL_PX, DIM);
        }

        let dimc: [f32; 3] = [0.25, 0.15, 0.25];
        let lc = if di > 0 { mix3(pal.accent, ACCENT_HI, lh) } else { dimc };
        let rc = if di + 1 < lvl.difficulty_tiers.len() {
            mix3(pal.accent, ACCENT_HI, rh) } else { dimc };
        let lx = dcx - 0.30;
        let rx = dcx + 0.30;
        {
            let geom = self.ui.geom_mut();
            push_tri(geom,
                [lx - 0.04 - lh * 0.006, dy],
                [lx + 0.028, dy - 0.040],
                [lx + 0.028, dy + 0.040], lc);
            push_tri(geom,
                [rx + 0.04 + rh * 0.006, dy],
                [rx - 0.028, dy - 0.040],
                [rx - 0.028, dy + 0.040], rc);
        }

        {
            let text = self.ui.text_mut();
            font::push_text_centered(text, &tier_label, dcx,
                dy - font::text_height(H1_PX) * 0.5, H1_PX, WHITE);
            let mstr = format!("X{:.2}", tier_mult);
            font::push_text_centered(text, &mstr, dcx,
                dy + font::text_height(H1_PX) * 0.5 + 0.006, SMALL_PX, pal.accent);
        }

        // Tier dots.
        let n_tiers = lvl.difficulty_tiers.len();
        for i in 0..n_tiers {
            let dx = dcx - 0.10 + (i as f32 / (n_tiers - 1).max(1) as f32) * 0.20;
            let lit = i <= di;
            let color = if lit { pal.accent } else { DIM };
            let r = if lit { 0.012 } else { 0.009 };
            push_hex(self.ui.geom_mut(), dx, dy + 0.10, r, color);
        }

        // PLAY button at the bottom of the card.
        let play_rect = Rect::from_center(dcx, y1 - 0.08, 0.22, 0.055);
        let phit = play_rect.contains(pointer.0, pointer.1);
        let ph = self.ui.track_hover(ID_PLAY_LEVEL, phit);
        self.draw_play_button(play_rect.center(), pal, ph);
        if phit && clicked {
            audio.play_enter();
            self.state = AppState::Playing {
                level: self.selected,
                difficulty_idx: self.difficulty_idx[self.selected as usize],
            };
        }
    }

    fn draw_play_button(
        &mut self,
        center: (f32, f32),
        pal: &Palette, ph: f32,
    ) {
        let (pcx, pcy) = center;
        let bg   = mix3(BG_PANEL, BG_PANEL_HI, ph);
        let ring = mix3(pal.accent, ACCENT_HI, ph);
        let hw = 0.22 + 0.006 * ph;
        let hh = 0.055 + 0.003 * ph;
        let thick = 0.005 + 0.002 * ph;

        {
            let geom = self.ui.geom_mut();
            push_quad(geom, pcx - hw, pcy - hh, pcx + hw, pcy + hh, bg);
            push_outline(geom, pcx - hw, pcy - hh, pcx + hw, pcy + hh, thick, ring);
        }

        let chev_w = 0.040;
        let chev_h = 0.028;
        let gap    = 0.018;
        let text_w = font::text_width("PLAY", BUTTON_PX);
        let content_w = chev_w + gap + text_w;
        let content_left = pcx - content_w * 0.5;
        let chev_x = content_left;
        let text_x = chev_x + chev_w + gap;

        push_tri(self.ui.geom_mut(),
            [chev_x,          pcy - chev_h],
            [chev_x,          pcy + chev_h],
            [chev_x + chev_w, pcy],
            ring);

        font::push_text(self.ui.text_mut(), "PLAY", text_x,
            pcy - font::text_height(BUTTON_PX) * 0.5, BUTTON_PX, ring);
    }

    fn render_modifier_row(&mut self) {
        let strip = self.modifier_strip_rect();
        let cells = strip.split_x(5, 0.015);
        for (i, cell) in cells.iter().enumerate() {
            {
                let geom = self.ui.geom_mut();
                push_quad(geom, cell.x0, cell.y0, cell.x1, cell.y1, BG_PANEL);
                push_outline(geom, cell.x0, cell.y0, cell.x1, cell.y1, 0.003, DIM);
            }
            let label = if i == 0 { "RANDOM LVL" } else { "MODIFIER" };
            let sub   = if i == 0 { "SHUFFLE" }    else { "PLACEHOLDER" };
            let text = self.ui.text_mut();
            font::push_text(text, label,
                cell.x0 + 0.012, cell.y0 + 0.020, SMALL_PX, WHITE);
            font::push_text(text, sub,
                cell.x0 + 0.012,
                cell.y0 + 0.020 + font::text_height(SMALL_PX) + 0.005,
                SMALL_PX, DIM);
        }
    }

    // ---------- settings ----------

    fn render_settings(
        &mut self, audio: &Audio, config: &mut Config,
        consume_input: bool,
    ) {
        let pointer = self.ui.input().pointer;
        let clicked = self.ui.input().left_clicked && consume_input;

        // Title + subtitle.
        let title_y = self.view_top() + SETTINGS_TITLE_INSET;
        let sub_y   = self.view_top() + SETTINGS_SUBTITLE_INSET;
        {
            let text = self.ui.text_mut();
            font::push_text_centered(text, "OPTIONS",
                0.0, title_y, TITLE_PX, ACCENT_HI);
            font::push_text_centered(text, "TUNE THE FEEL",
                0.0, sub_y, H2_PX, DIM);
        }

        // BACK + RESET buttons.
        let back_rect = self.back_button_rect();
        let back_hit = back_rect.contains(pointer.0, pointer.1);
        let back_h = self.ui.track_hover(ID_BACK, back_hit);
        self.draw_small_button(back_rect, "BACK", back_h);
        if back_hit && clicked {
            audio.play_interact();
            config.clamp();
            config.save();
            self.state = AppState::MainMenu;
            return;
        }
        let reset_rect = self.reset_button_rect();
        let reset_hit = reset_rect.contains(pointer.0, pointer.1);
        let reset_h = self.ui.track_hover(ID_RESET, reset_hit);
        self.draw_small_button(reset_rect, "RESET", reset_h);
        if reset_hit && clicked {
            audio.play_interact();
            *config = Config::default();
            return;
        }

        // Tab bar.
        let tabs: [(&str, WidgetId); 5] = [
            ("GRAPHICS", ID_TAB_GFX),
            ("AUDIO",    ID_TAB_AUD),
            ("GAMEPLAY", ID_TAB_GPL),
            ("ACCESS",   ID_TAB_ACC),
            ("CONTROLS", ID_TAB_CTL),
        ];
        let mut next_tab: Option<usize> = None;
        for (i, (label, id)) in tabs.iter().enumerate() {
            let rect = self.tab_rect(i, tabs.len());
            let hit = rect.contains(pointer.0, pointer.1);
            let h = self.ui.track_hover(*id, hit);
            let active = i == self.settings_tab;
            let bg = if active {
                BG_PANEL_HI
            } else {
                mix3(BG_PANEL, BG_PANEL_HI, h)
            };
            push_quad(self.ui.geom_mut(),
                rect.x0, rect.y0, rect.x1, rect.y1, bg);
            if active {
                push_quad(self.ui.geom_mut(),
                    rect.x0, rect.y1 - 0.006, rect.x1, rect.y1, ACCENT);
            } else {
                push_outline(self.ui.geom_mut(),
                    rect.x0, rect.y0, rect.x1, rect.y1, 0.002,
                    mix3(DIM, ACCENT, h));
            }
            let (cx, cy) = rect.center();
            let w = font::text_width(label, BUTTON_PX);
            let label_color = if active { WHITE } else { DIM };
            font::push_text(self.ui.text_mut(), label,
                cx - w * 0.5,
                cy - font::text_height(BUTTON_PX) * 0.5,
                BUTTON_PX, label_color);
            if hit && clicked && !active {
                next_tab = Some(i);
            }
        }
        if let Some(i) = next_tab {
            self.settings_tab = i;
            audio.play_interact();
            return;
        }

        // Active tab content.
        match self.settings_tab {
            0 => self.render_tab_graphics(pointer, clicked, audio, config),
            1 => self.render_tab_audio   (pointer, clicked, audio, config),
            2 => self.render_tab_gameplay(pointer, clicked, audio, config),
            3 => self.render_tab_access  (pointer, clicked, audio, config),
            _ => self.render_tab_controls(),
        }

        config.clamp();
    }

    fn render_tab_graphics(
        &mut self, pointer: (f32, f32), clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 12;

        // Row 0: V-Sync cycle.
        {
            let (_, cy) = self.row_center(0, rows);
            self.set_hover_arrows(pointer, cy, ID_GFX_VSYNC_L, ID_GFX_VSYNC_R);
            if self.arrow_clicked(pointer, clicked, cy, false) {
                config.vsync = vsync_prev(config.vsync);
                audio.play_interact();
            }
            if self.arrow_clicked(pointer, clicked, cy, true) {
                config.vsync = config.vsync.next();
                audio.play_interact();
            }
            self.draw_cycle_row(0, rows, "V-SYNC", config.vsync.label());
        }

        // Row 1: FPS cap cycle.
        {
            let (_, cy) = self.row_center(1, rows);
            self.set_hover_arrows(pointer, cy, ID_GFX_FPS_L, ID_GFX_FPS_R);
            if self.arrow_clicked(pointer, clicked, cy, false) {
                config.fps_cap = fps_prev(config.fps_cap);
                audio.play_interact();
            }
            if self.arrow_clicked(pointer, clicked, cy, true) {
                config.fps_cap = fps_next(config.fps_cap);
                audio.play_interact();
            }
            let fps_label = fps_label(config.fps_cap);
            self.draw_cycle_row(1, rows, "FPS CAP", &fps_label);
        }

        // Rows 2..7: sliders.
        self.tick_slider(pointer, clicked, 2, rows,
            ID_GFX_BLOOM, &mut config.bloom_intensity, 0.0, 1.5, audio);
        self.tick_slider(pointer, clicked, 3, rows,
            ID_GFX_CHROMATIC, &mut config.chromatic_strength, 0.0, 1.0, audio);
        self.tick_slider(pointer, clicked, 4, rows,
            ID_GFX_SHAKE, &mut config.screen_shake, 0.0, 1.5, audio);
        self.tick_slider(pointer, clicked, 5, rows,
            ID_GFX_VIGNETTE, &mut config.vignette, 0.0, 1.0, audio);
        self.tick_slider(pointer, clicked, 6, rows,
            ID_GFX_BEAT_FLASH, &mut config.beat_flash, 0.0, 1.5, audio);
        self.tick_slider(pointer, clicked, 7, rows,
            ID_GFX_DEPTH, &mut config.fake_3d_depth, 0.0, 1.0, audio);

        self.draw_slider_row(2, rows, "BLOOM",          config.bloom_intensity,    0.0, 1.5, ID_GFX_BLOOM);
        self.draw_slider_row(3, rows, "CHROMATIC",      config.chromatic_strength, 0.0, 1.0, ID_GFX_CHROMATIC);
        self.draw_slider_row(4, rows, "SCREEN SHAKE",   config.screen_shake,       0.0, 1.5, ID_GFX_SHAKE);
        self.draw_slider_row(5, rows, "VIGNETTE",       config.vignette,           0.0, 1.0, ID_GFX_VIGNETTE);
        self.draw_slider_row(6, rows, "BEAT FLASH",     config.beat_flash,         0.0, 1.5, ID_GFX_BEAT_FLASH);
        self.draw_slider_row(7, rows, "FAKE 3D DEPTH",  config.fake_3d_depth,      0.0, 1.0, ID_GFX_DEPTH);

        // Row 8: Particles cycle.
        {
            let (_, cy) = self.row_center(8, rows);
            self.set_hover_arrows(pointer, cy, ID_GFX_PART_L, ID_GFX_PART_R);
            if self.arrow_clicked(pointer, clicked, cy, false) {
                config.particle_density = particle_prev(config.particle_density);
                audio.play_interact();
            }
            if self.arrow_clicked(pointer, clicked, cy, true) {
                config.particle_density = config.particle_density.next();
                audio.play_interact();
            }
            self.draw_cycle_row(8, rows, "PARTICLES",
                config.particle_density.label());
        }

        // Rows 9..10: toggles.
        self.tick_toggle(pointer, clicked, 9, rows, ID_GFX_SCANLINES,
            &mut config.scanlines, audio);
        self.tick_toggle(pointer, clicked, 10, rows, ID_GFX_SHOW_FPS,
            &mut config.show_fps, audio);
        self.draw_toggle_row(9,  rows, "SCANLINES", config.scanlines, ID_GFX_SCANLINES);
        self.draw_toggle_row(10, rows, "SHOW FPS",  config.show_fps,  ID_GFX_SHOW_FPS);

        // Row 11: custom shaders cycle.
        {
            let (_, cy) = self.row_center(11, rows);
            self.set_hover_arrows(pointer, cy, ID_GFX_SHADERS_L, ID_GFX_SHADERS_R);
            if self.arrow_clicked(pointer, clicked, cy, false) {
                config.custom_shaders = custom_shaders_prev(config.custom_shaders);
                audio.play_interact();
            }
            if self.arrow_clicked(pointer, clicked, cy, true) {
                config.custom_shaders = config.custom_shaders.next();
                audio.play_interact();
            }
            self.draw_cycle_row(11, rows, "SHADERS",
                config.custom_shaders.label());
            let hint = match config.custom_shaders {
                CustomShaders::Off =>
                    "USER SHADERS DISABLED. SAFE DEFAULT.",
                CustomShaders::Audit =>
                    "COMPILE AND VALIDATE. NO RENDERING.",
                CustomShaders::On =>
                    "RENDER USER SHADERS. CAN STRESS THE GPU.",
            };
            let view_l = self.view_left();
            font::push_text(self.ui.text_mut(), hint,
                view_l + 0.52, cy + 0.035, SMALL_PX, DIM);
        }
    }

    fn render_tab_audio(
        &mut self, pointer: (f32, f32), clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 3;
        self.tick_slider(pointer, clicked, 0, rows,
            ID_AUD_MASTER, &mut config.master_volume, 0.0, 1.0, audio);
        self.tick_slider(pointer, clicked, 1, rows,
            ID_AUD_MUSIC,  &mut config.music_volume,  0.0, 1.0, audio);
        self.tick_slider(pointer, clicked, 2, rows,
            ID_AUD_SFX,    &mut config.sfx_volume,    0.0, 1.0, audio);
        self.draw_slider_row(0, rows, "MASTER VOLUME", config.master_volume, 0.0, 1.0, ID_AUD_MASTER);
        self.draw_slider_row(1, rows, "MUSIC VOLUME",  config.music_volume,  0.0, 1.0, ID_AUD_MUSIC);
        self.draw_slider_row(2, rows, "SFX VOLUME",    config.sfx_volume,    0.0, 1.0, ID_AUD_SFX);
    }

    fn render_tab_gameplay(
        &mut self, pointer: (f32, f32), clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 5;
        self.tick_slider(pointer, clicked, 0, rows,
            ID_GPL_PSPEED, &mut config.player_speed, 0.5, 2.0, audio);
        self.tick_slider(pointer, clicked, 1, rows,
            ID_GPL_WOBBLE, &mut config.camera_wobble, 0.0, 1.0, audio);

        {
            let (_, cy) = self.row_center(2, rows);
            self.set_hover_arrows(pointer, cy, ID_GPL_ABILITY_L, ID_GPL_ABILITY_R);
            if self.arrow_clicked(pointer, clicked, cy, false) {
                config.ability = ability_prev(config.ability);
                audio.play_interact();
            }
            if self.arrow_clicked(pointer, clicked, cy, true) {
                config.ability = config.ability.next();
                audio.play_interact();
            }
        }

        self.tick_toggle(pointer, clicked, 3, rows, ID_GPL_CLOSE_FX,
            &mut config.close_call_fx, audio);

        {
            let (_, cy) = self.row_center(4, rows);
            self.set_hover_arrows(pointer, cy, ID_GPL_HL_L, ID_GPL_HL_R);
            if self.arrow_clicked(pointer, clicked, cy, false) {
                config.predictive_highlight =
                    highlight_prev(config.predictive_highlight);
                audio.play_interact();
            }
            if self.arrow_clicked(pointer, clicked, cy, true) {
                config.predictive_highlight =
                    config.predictive_highlight.next();
                audio.play_interact();
            }
        }

        self.draw_slider_row(0, rows, "PLAYER SPEED",   config.player_speed,  0.5, 2.0, ID_GPL_PSPEED);
        self.draw_slider_row(1, rows, "CAMERA WOBBLE",  config.camera_wobble, 0.0, 1.0, ID_GPL_WOBBLE);
        self.draw_cycle_row (2, rows, "ABILITY", config.ability.label());
        self.draw_toggle_row(3, rows, "CLOSE-CALL FX", config.close_call_fx, ID_GPL_CLOSE_FX);
        self.draw_cycle_row (4, rows, "PREDICT HIGHLIGHT",
            config.predictive_highlight.label());

        // Ability description under row 2.
        let (_, cy) = self.row_center(2, rows);
        let view_l = self.view_left();
        font::push_text(self.ui.text_mut(), config.ability.description(),
            view_l + 0.52, cy + 0.035, SMALL_PX, DIM);
    }

    fn render_tab_access(
        &mut self, pointer: (f32, f32), clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 4;
        self.tick_toggle(pointer, clicked, 0, rows, ID_ACC_CONTRAST,
            &mut config.high_contrast, audio);
        self.tick_toggle(pointer, clicked, 1, rows, ID_ACC_REDUCE,
            &mut config.reduce_motion, audio);
        self.tick_toggle(pointer, clicked, 2, rows, ID_ACC_HITBOX,
            &mut config.show_hitboxes, audio);

        {
            let (_, cy) = self.row_center(3, rows);
            self.set_hover_arrows(pointer, cy, ID_ACC_CB_L, ID_ACC_CB_R);
            if self.arrow_clicked(pointer, clicked, cy, false) {
                config.colorblind = colorblind_prev(config.colorblind);
                audio.play_interact();
            }
            if self.arrow_clicked(pointer, clicked, cy, true) {
                config.colorblind = config.colorblind.next();
                audio.play_interact();
            }
        }

        self.draw_toggle_row(0, rows, "HIGH CONTRAST", config.high_contrast,  ID_ACC_CONTRAST);
        self.draw_toggle_row(1, rows, "REDUCE MOTION", config.reduce_motion,  ID_ACC_REDUCE);
        self.draw_toggle_row(2, rows, "SHOW HITBOXES", config.show_hitboxes,  ID_ACC_HITBOX);
        self.draw_cycle_row (3, rows, "COLOR BLIND MODE", config.colorblind.label());
    }

    fn render_tab_controls(&mut self) {
        let lines = [
            "LEFT / RIGHT .... TURN CURSOR",
            "SHIFT ........... ABILITY (WHEN EQUIPPED)",
            "SPACE ........... RESTART AFTER DEATH",
            "ENTER ........... CONFIRM / PLAY",
            "ESCAPE .......... BACK / SAVE",
            "UP / DOWN ....... MENU NAVIGATION",
            "",
            "MOUSE IS USABLE IN MENUS.",
        ];
        let view_l = self.view_left();
        let top = self.settings_band_top() + 0.06;
        let text = self.ui.text_mut();
        for (i, line) in lines.iter().enumerate() {
            font::push_text(text, line, view_l + 0.16, top + i as f32 * 0.065,
                BODY_PX, WHITE);
        }
    }

    /// Slider hit test + drag tracking, register hover via
    /// `track_hover`. Drawing performed by `draw_slider_row`.
    fn tick_slider(
        &mut self, pointer: (f32, f32), clicked: bool,
        row: usize, rows: usize, id: WidgetId,
        value: &mut f32, min: f32, max: f32,
        audio: &Audio,
    ) {
        let (x0, x1) = self.slider_track_range();
        let (_, y) = self.row_center(row, rows);

        let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
        let hx = x0 + (x1 - x0) * t;
        let handle_rect = Rect::from_center(hx, y, 0.05, 0.04);
        let handle_hit = handle_rect.contains(pointer.0, pointer.1);
        let hovered = handle_hit || self.ui.is_dragging(id);
        self.ui.track_hover(id, hovered);

        let track_hit = pointer.0 > x0 - 0.02 && pointer.0 < x1 + 0.02
                     && (pointer.1 - y).abs() < 0.04;
        if clicked && (handle_hit || track_hit) {
            audio.play_interact();
            self.ui.set_dragging(id);
            let nt = ((pointer.0 - x0) / (x1 - x0)).clamp(0.0, 1.0);
            *value = min + (max - min) * nt;
            self.ui.capture_pointer();
            return;
        }
        if self.ui.is_dragging(id) && self.ui.input().left_down {
            let nt = ((pointer.0 - x0) / (x1 - x0)).clamp(0.0, 1.0);
            *value = min + (max - min) * nt;
            self.ui.capture_pointer();
        }
    }

    fn tick_toggle(
        &mut self, pointer: (f32, f32), clicked: bool,
        row: usize, rows: usize, id: WidgetId,
        value: &mut bool, audio: &Audio,
    ) {
        let rect = self.toggle_rect(row, rows);
        let hit = rect.contains(pointer.0, pointer.1);
        self.ui.track_hover(id, hit);
        if hit && clicked {
            *value = !*value;
            audio.play_interact();
            self.ui.capture_pointer();
        }
    }

    fn set_hover_arrows(
        &mut self, pointer: (f32, f32),
        cy: f32, id_l: WidgetId, id_r: WidgetId,
    ) {
        let (lx, rx) = self.arrow_positions();
        let lr = Rect::from_center(lx, cy, 0.05, 0.04);
        let rr = Rect::from_center(rx, cy, 0.05, 0.04);
        self.ui.track_hover(id_l, lr.contains(pointer.0, pointer.1));
        self.ui.track_hover(id_r, rr.contains(pointer.0, pointer.1));
    }

    fn arrow_clicked(
        &self, pointer: (f32, f32), clicked: bool,
        cy: f32, right: bool,
    ) -> bool {
        if !clicked { return false; }
        let (lx, rx) = self.arrow_positions();
        let ax = if right { rx } else { lx };
        let r = Rect::from_center(ax, cy, 0.05, 0.04);
        r.contains(pointer.0, pointer.1)
    }

    fn draw_slider_row(
        &mut self,
        row: usize, rows: usize, label: &str,
        value: f32, min: f32, max: f32, id: WidgetId,
    ) {
        let (_, y) = self.row_center(row, rows);
        let view_l = self.view_left();

        font::push_text(self.ui.text_mut(), label,
            view_l + 0.04,
            y - font::text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        let (x0, x1) = self.slider_track_range();
        {
            let geom = self.ui.geom_mut();
            push_quad(geom, x0 - 0.006, y - 0.014, x1 + 0.006, y + 0.014, BG_DEEP);
            push_quad(geom, x0, y - 0.010, x1, y + 0.010, BG_PANEL);
        }

        let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let fx = x0 + (x1 - x0) * t;
        push_quad(self.ui.geom_mut(),
            x0, y - 0.010, fx, y + 0.010, ACCENT);

        let pointer = self.ui.input().pointer;
        let handle_rect = Rect::from_center(fx, y, 0.05, 0.04);
        let handle_hit = handle_rect.contains(pointer.0, pointer.1);
        let hovered = handle_hit || self.ui.is_dragging(id);
        let h = self.ui.track_hover(id, hovered);

        let ring = mix3(ACCENT, ACCENT_HI, h);
        let r = 0.022 + 0.005 * h;
        {
            let geom = self.ui.geom_mut();
            push_hex(geom, fx, y, r, BG_DEEP);
            push_hex_ring(geom, fx, y, r, r - 0.006, ring);
            push_hex(geom, fx, y, r * 0.40, ring);
        }

        let disp = if (max - min - 1.0).abs() < 1e-3 && min == 0.0 {
            format!("{}%", (value * 100.0).round() as i32)
        } else {
            format!("{:.2}", value)
        };
        font::push_text_right(self.ui.text_mut(), &disp,
            x1 + 0.16,
            y - font::text_height(BODY_PX) * 0.5, BODY_PX, ACCENT_HI);
    }

    fn draw_cycle_row(
        &mut self,
        row: usize, rows: usize, label: &str, value: &str,
    ) {
        let (_, y) = self.row_center(row, rows);
        let view_l = self.view_left();

        font::push_text(self.ui.text_mut(), label,
            view_l + 0.04,
            y - font::text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        let (lx, rx) = self.arrow_positions();

        // Hover values for cycle arrows are tracked by the
        // per-tab tick code via set_hover_arrows; the visual
        // animation of the arrows themselves does not need
        // the eased value (the colour change is gated on
        // hit-test in the per-tab code), so we draw arrows
        // with neutral lean and just let the colour wash
        // come from the current hover via the ui state.
        let lh = 0.0_f32;
        let rh = 0.0_f32;

        let lc = mix3(ACCENT, ACCENT_HI, lh);
        let rc = mix3(ACCENT, ACCENT_HI, rh);
        {
            let geom = self.ui.geom_mut();
            push_tri(geom,
                [lx - 0.035 - 0.005 * lh, y],
                [lx + 0.020, y - 0.024],
                [lx + 0.020, y + 0.024], lc);
            push_tri(geom,
                [rx + 0.035 + 0.005 * rh, y],
                [rx - 0.020, y - 0.024],
                [rx - 0.020, y + 0.024], rc);
        }

        let mid = (lx + rx) * 0.5;
        let w = font::text_width(value, BODY_PX);
        font::push_text(self.ui.text_mut(), value,
            mid - w * 0.5,
            y - font::text_height(BODY_PX) * 0.5, BODY_PX, ACCENT_HI);
    }

    fn draw_toggle_row(
        &mut self,
        row: usize, rows: usize, label: &str,
        value: bool, id: WidgetId,
    ) {
        let (_, y) = self.row_center(row, rows);
        let view_l = self.view_left();

        font::push_text(self.ui.text_mut(), label,
            view_l + 0.04,
            y - font::text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        let rect = self.toggle_rect(row, rows);
        let pointer = self.ui.input().pointer;
        let hovered = rect.contains(pointer.0, pointer.1);
        let h = self.ui.track_hover(id, hovered);

        let bg = if value {
            mix3(ACCENT, ACCENT_HI, h)
        } else {
            mix3(BG_PANEL, BG_PANEL_HI, h)
        };
        let ring = mix3(ACCENT, ACCENT_HI, h);
        {
            let geom = self.ui.geom_mut();
            push_quad(geom, rect.x0, rect.y0, rect.x1, rect.y1, bg);
            push_outline(geom, rect.x0, rect.y0, rect.x1, rect.y1, 0.003, ring);
        }
        let text = if value { "ON" } else { "OFF" };
        let w = font::text_width(text, BODY_PX);
        let (cx, cy) = rect.center();
        let text_color = if value { BG_DEEP } else { WHITE };
        font::push_text(self.ui.text_mut(), text,
            cx - w * 0.5,
            cy - font::text_height(BODY_PX) * 0.5,
            BODY_PX, text_color);
    }

    fn draw_small_button(
        &mut self,
        rect: Rect,
        label: &str, h: f32,
    ) {
        let (cx, cy) = rect.center();
        let bg   = mix3(BG_PANEL, BG_PANEL_HI, h);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        {
            let geom = self.ui.geom_mut();
            push_quad(geom, rect.x0, rect.y0, rect.x1, rect.y1, bg);
            push_outline(geom, rect.x0, rect.y0, rect.x1, rect.y1,
                0.004 + 0.002 * h, ring);
        }
        let w = font::text_width(label, BODY_PX);
        font::push_text(self.ui.text_mut(), label,
            cx - w * 0.5,
            cy - font::text_height(BODY_PX) * 0.5,
            BODY_PX, WHITE);
    }

    // ---------- music panel ----------

    fn render_music_panel(
        &mut self, audio: &Audio, snap: &AudioSnapshot,
    ) {
        if !snap.has_music { return; }

        let view_l = self.view_left();
        let view_r = self.view_right();
        let view_b = self.view_bottom();
        let top = self.view_top();
        let bot = self.panel_bot();
        let cy = top + PANEL_EXTENDED_HEIGHT - 0.06;

        // Panel + accent strip.
        {
            let geom = self.ui.geom_mut();
            push_quad(geom, view_l, top, view_r, bot, BG_PANEL);
            push_quad(geom, view_l, bot - 0.004, view_r, bot, ACCENT);
        }

        if self.panel_t < 0.05 { return; }

        // Backdrop dim under the panel.
        let overlay_a = self.panel_t * 0.55;
        push_quad_alpha(self.ui.geom_mut(),
            view_l, bot, view_r, view_b,
            [0.0, 0.0, 0.0], overlay_a);

        let alpha = smoothstep(PANEL_CONTENT_LO, PANEL_CONTENT_HI, self.panel_t);
        if alpha < 0.02 { return; }

        // Now playing labels.
        let (track_name, artist) = self.current_track_display_names();
        let label_x = view_l + 0.04;
        let pad_top = 0.030;
        let label_y  = top + pad_top;
        let title_y  = label_y + font::text_height(SMALL_PX) + 0.008;
        let artist_y = title_y + font::text_height(H2_PX) + 0.008;
        {
            let text = self.ui.text_mut();
            font::push_text_alpha(text, "NOW PLAYING", label_x, label_y, SMALL_PX, DIM, alpha);
            font::push_text_alpha(text, &track_name,   label_x, title_y, H2_PX,    WHITE, alpha);
            font::push_text_alpha(text, &artist,       label_x, artist_y, SMALL_PX, DIM, alpha);
        }

        // Transport buttons.
        let (bx0, bx1) = self.music_panel_bar_range();
        let ctrl_x0 = bx0 - 0.26;
        let pause_cx = ctrl_x0 + 0.050;
        let stop_cx  = ctrl_x0 + 0.155;
        let btn_hw   = 0.045;
        let btn_hh   = 0.030;

        let pointer = self.ui.input().pointer;
        let pause_rect = Rect::from_center(pause_cx, cy, btn_hw, btn_hh);
        let stop_rect  = Rect::from_center(stop_cx,  cy, btn_hw, btn_hh);
        let pause_hit = pause_rect.contains(pointer.0, pointer.1);
        let stop_hit  = stop_rect.contains(pointer.0, pointer.1);

        let ph = self.ui.track_hover(ID_MUSIC_PAUSE, pause_hit);
        let sh = self.ui.track_hover(ID_MUSIC_STOP,  stop_hit);

        let pring = mix3(ACCENT, ACCENT_HI, ph);
        push_quad_alpha(self.ui.geom_mut(),
            pause_rect.x0, pause_rect.y0,
            pause_rect.x1, pause_rect.y1,
            BG_DEEP, alpha);
        push_outline_alpha(self.ui.geom_mut(),
            pause_rect.x0, pause_rect.y0,
            pause_rect.x1, pause_rect.y1,
            0.003, pring, alpha);
        if snap.paused {
            push_tri_alpha(self.ui.geom_mut(),
                [pause_cx - 0.012, cy - 0.018],
                [pause_cx - 0.012, cy + 0.018],
                [pause_cx + 0.020, cy],
                pring, alpha);
        } else {
            push_quad_alpha(self.ui.geom_mut(),
                pause_cx - 0.020, cy - 0.018,
                pause_cx - 0.006, cy + 0.018, pring, alpha);
            push_quad_alpha(self.ui.geom_mut(),
                pause_cx + 0.006, cy - 0.018,
                pause_cx + 0.020, cy + 0.018, pring, alpha);
        }

        let sring = mix3(ACCENT, ACCENT_HI, sh);
        push_quad_alpha(self.ui.geom_mut(),
            stop_rect.x0, stop_rect.y0,
            stop_rect.x1, stop_rect.y1,
            BG_DEEP, alpha);
        push_outline_alpha(self.ui.geom_mut(),
            stop_rect.x0, stop_rect.y0,
            stop_rect.x1, stop_rect.y1,
            0.003, sring, alpha);
        push_quad_alpha(self.ui.geom_mut(),
            stop_cx - 0.018, cy - 0.018,
            stop_cx + 0.018, cy + 0.018,
            sring, alpha);

        // Progress bar + scrub handle.
        push_quad_alpha(self.ui.geom_mut(),
            bx0 - 0.004, cy - 0.014, bx1 + 0.004, cy + 0.014, BG_DEEP, alpha);
        push_quad_alpha(self.ui.geom_mut(),
            bx0, cy - 0.010, bx1, cy + 0.010, BG_PANEL_HI, alpha);
        let prog = if snap.duration > 0.0 {
            (snap.position / snap.duration).clamp(0.0, 1.0)
        } else { 0.0 };
        let fx = bx0 + (bx1 - bx0) * prog;
        push_quad_alpha(self.ui.geom_mut(),
            bx0, cy - 0.010, fx, cy + 0.010, ACCENT, alpha);

        let bar_hit = pointer.0 > bx0 - 0.01
                   && pointer.0 < bx1 + 0.01
                   && (pointer.1 - cy).abs() < 0.022;
        let bh = self.ui.track_hover(ID_MUSIC_BAR, bar_hit);
        let scrub = mix3(ACCENT, ACCENT_HI, bh);
        let sr = 0.018 + 0.004 * bh;
        push_hex_alpha(self.ui.geom_mut(),
            fx, cy, sr, BG_DEEP, alpha);
        push_hex_ring_alpha(self.ui.geom_mut(),
            fx, cy, sr, sr - 0.006, scrub, alpha);

        // Time readout.
        let remaining = (snap.duration - snap.position).max(0.0);
        let t_str = format!("{} / {}   (-{})",
            fmt_time(snap.position), fmt_time(snap.duration),
            fmt_time(remaining));
        font::push_text_right_alpha(self.ui.text_mut(), &t_str,
            view_r - 0.04,
            cy - font::text_height(SMALL_PX) * 0.5,
            SMALL_PX, WHITE, alpha);

        // Click handling.
        if self.panel_t < PANEL_INPUT_THRESHOLD {
            return;
        }
        let input = self.ui.input();
        if input.left_clicked {
            if pause_hit {
                audio.play_interact();
                audio.set_music_paused(!snap.paused);
                self.ui.capture_pointer();
            } else if stop_hit {
                audio.play_interact();
                audio.stop_music();
                self.ui.capture_pointer();
            } else if bar_hit && snap.duration > 0.0 {
                let t = ((pointer.0 - bx0) / (bx1 - bx0)).clamp(0.0, 1.0);
                audio.seek_music(t * snap.duration);
                self.ui.set_dragging(ID_MUSIC_BAR);
                self.ui.capture_pointer();
            }
        }
    }

    fn current_track_display_names(&self) -> (String, String) {
        match self.state {
            AppState::Playing { level, .. } => {
                let lvl = crate::levels::get(level);
                (lvl.song.clone(), lvl.artist.clone())
            }
            AppState::LevelSelect => {
                let lvl = crate::levels::get(self.selected);
                (lvl.song.clone(), lvl.artist.clone())
            }
            _ => ("MENU THEME".to_string(), "OPENGAMEART".to_string()),
        }
    }

    // ---------- output ----------

    pub fn build_geometry(
        &mut self,
        out: &mut Vec<Vertex>,
        text_out: &mut Vec<TextVertex>,
        _snap: &AudioSnapshot,
        _config: &Config,
    ) {
        out.clear();
        self.ui.end_frame(out, text_out);
    }
}

// ----- helpers -----

fn fmt_time(seconds: f32) -> String {
    let s = seconds.max(0.0) as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn main_item_pos(i: usize) -> (f32, f32) {
    let n = MAIN_ITEMS.len() as f32;
    let spacing = 0.23;
    let total = (n - 1.0) * spacing;
    let y0 = 0.10 - total * 0.5 + 0.15;
    (0.0, y0 + i as f32 * spacing)
}

fn row_y(i: usize, selected: u32) -> f32 {
    let base = 0.10;
    let delta = i as f32 - selected as f32;
    base + delta * ROW_SPACING
}

// ----- enum cycling helpers -----

fn vsync_prev(v: VsyncMode) -> VsyncMode {
    match v {
        VsyncMode::Off  => VsyncMode::Fast,
        VsyncMode::On   => VsyncMode::Off,
        VsyncMode::Fast => VsyncMode::On,
    }
}

fn particle_prev(p: ParticleDensity) -> ParticleDensity {
    match p {
        ParticleDensity::Off    => ParticleDensity::High,
        ParticleDensity::Low    => ParticleDensity::Off,
        ParticleDensity::Medium => ParticleDensity::Low,
        ParticleDensity::High   => ParticleDensity::Medium,
    }
}

fn ability_prev(a: Ability) -> Ability {
    match a {
        Ability::None   => Ability::SlowMo,
        Ability::Dash   => Ability::None,
        Ability::Shield => Ability::Dash,
        Ability::SlowMo => Ability::Shield,
    }
}

fn highlight_prev(h: HighlightMode) -> HighlightMode {
    match h {
        HighlightMode::Off  => HighlightMode::Auto,
        HighlightMode::On   => HighlightMode::Off,
        HighlightMode::Auto => HighlightMode::On,
    }
}

fn colorblind_prev(m: ColorblindMode) -> ColorblindMode {
    match m {
        ColorblindMode::Off          => ColorblindMode::Tritanopia,
        ColorblindMode::Protanopia   => ColorblindMode::Off,
        ColorblindMode::Deuteranopia => ColorblindMode::Protanopia,
        ColorblindMode::Tritanopia   => ColorblindMode::Deuteranopia,
    }
}

fn custom_shaders_prev(s: CustomShaders) -> CustomShaders {
    match s {
        CustomShaders::Off   => CustomShaders::On,
        CustomShaders::Audit => CustomShaders::Off,
        CustomShaders::On    => CustomShaders::Audit,
    }
}

const FPS_PRESETS: &[u32] = &[0, 30, 60, 75, 120, 144, 165, 240];

fn fps_next(v: u32) -> u32 {
    let idx = FPS_PRESETS.iter().position(|&x| x == v).unwrap_or(0);
    FPS_PRESETS[(idx + 1) % FPS_PRESETS.len()]
}

fn fps_prev(v: u32) -> u32 {
    let idx = FPS_PRESETS.iter().position(|&x| x == v).unwrap_or(0);
    FPS_PRESETS[(idx + FPS_PRESETS.len() - 1) % FPS_PRESETS.len()]
}

fn fps_label(v: u32) -> String {
    if v == 0 { "UNLIMITED".to_string() } else { format!("{}", v) }
}

// ----- low level geometry helpers (those not in ui::draw) -----

fn push_trapezoid(
    out: &mut Vec<Vertex>,
    xl_top: f32, xl_bot: f32, xr_top: f32, xr_bot: f32,
    y_top: f32, y_bot: f32, c: [f32; 3],
) {
    out.push(Vertex::opaque([xl_top, y_top], c));
    out.push(Vertex::opaque([xr_top, y_top], c));
    out.push(Vertex::opaque([xr_bot, y_bot], c));
    out.push(Vertex::opaque([xl_top, y_top], c));
    out.push(Vertex::opaque([xr_bot, y_bot], c));
    out.push(Vertex::opaque([xl_bot, y_bot], c));
}

fn push_slanted_edge(
    out: &mut Vec<Vertex>,
    x_top: f32, x_bot: f32,
    y_top: f32, y_bot: f32,
    thick: f32, c: [f32; 3],
) {
    push_trapezoid(out,
        x_top - thick, x_bot - thick,
        x_top, x_bot,
        y_top, y_bot, c);
}

/// Word-wrap helper for SDF text. Splits `text` on whitespace
/// and stacks lines that fit inside `max_w` game-space units.
fn push_text_wrapped(
    out: &mut Vec<TextVertex>,
    text: &str,
    x: f32, y: f32, max_w: f32,
    pixel_size: f32, color: [f32; 3],
) {
    let line_h = font::line_height(pixel_size);
    let mut cur = String::new();
    let mut cur_w = 0.0f32;
    let mut cy = 0.0f32;
    let mut first = true;
    for word in text.split_whitespace() {
        let piece = if first { word.to_string() } else { format!(" {}", word) };
        let w = font::text_width(&piece, pixel_size);
        if cur_w + w > max_w && !first {
            font::push_text(out, &cur, x, y + cy, pixel_size, color);
            cur.clear();
            cur_w = 0.0;
            cy += line_h;
            let ww = font::text_width(word, pixel_size);
            cur.push_str(word);
            cur_w += ww;
            first = false;
        } else {
            cur.push_str(&piece);
            cur_w += w;
            first = false;
        }
    }
    if !cur.is_empty() {
        font::push_text(out, &cur, x, y + cy, pixel_size, color);
    }
}