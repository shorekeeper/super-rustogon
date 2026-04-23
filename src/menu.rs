//! Application shell: intro, main menu, level browser, options.
//!
//! Layout is fully aspect aware. On entry to `update` we snapshot
//! the current framebuffer aspect into `view_aspect`, then all
//! drawing and hit testing routes through `view_left` / `view_right`
//! / `view_top` / `view_bottom` helpers so content can be pinned to
//! the true screen edges.
//!
//! Music panel:
//!
//! * Always visible as a thin peek bar along the very top when a
//!   track is loaded.
//! * Hovering the pointer inside the panel's currently drawn
//!   rectangle opens it (slides down). Moving the pointer below
//!   closes it. Trigger zone tracks the live drawn bottom so
//!   there is no phantom hit box above or below the drawn panel.
//! * Full content (now playing / controls / progress) appears
//!   only once the panel is mostly open and fades in with the
//!   slide to avoid drawing text outside the visible rect.
//!
//! Level browser layout:
//!
//!   +-----------------------------------------------+
//!   | (sliding top panel)                           |
//!   |                                               |
//!   | [BACK]  SELECT A LEVEL        +----------+    |
//!   |         UP / DOWN ...         |   CARD   |    |
//!   |                   \           |          |    |
//!   |    01 HEXAGON      \          |          |    |
//!   |    02 HEXAGONER     \         |          |    |
//!   |    03 HEXAGONEST     \        +----------+    |
//!   |                       \                       |
//!   |-----------------------+-----------------------|
//!   | [RANDOM] [MODIFIER] [MODIFIER] [...]          |
//!   +-----------------------------------------------+
//!
//! The slanted accent line runs from `RAIL_RIGHT_TOP` at
//! `RAIL_TOP` to `RAIL_RIGHT_BOTTOM` at `MOD_ROW_TOP`, passing
//! through the separator that divides the browser from the
//! modifier strip. Row right edges are computed from the same
//! line so they never drift away from it.
//!
//! Settings screen (new in this revision):
//!
//! The three-slider settings screen was replaced by a tabbed
//! layout so the extended configuration (graphics knobs, audio
//! mix, abilities, accessibility) fits without scrolling. The
//! rest of the menu visually matches the previous version to the
//! pixel.

use std::collections::HashMap;

use crate::audio::Audio;
use crate::config::{
    Ability, ColorblindMode, Config, HighlightMode, ParticleDensity, VsyncMode,
};
use crate::levels::Palette;
use crate::pipeline::Vertex;
use crate::text::{
    push_text, push_text_alpha, push_text_centered,
    push_text_centered_alpha, push_text_right,
    push_text_wrapped, text_width, text_height,
};
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

// ----- original (preserved) widget ids -----
const ID_PLAY:        WidgetId = 1;
const ID_OPTIONS:     WidgetId = 2;
const ID_QUIT:        WidgetId = 3;
const ID_BACK:        WidgetId = 10;
const ID_RESET:       WidgetId = 11;
const ID_PLAY_LEVEL:  WidgetId = 12;
const ID_DIFF_LEFT:   WidgetId = 13;
const ID_DIFF_RIGHT:  WidgetId = 14;
const ID_MUSIC_PAUSE: WidgetId = 300;
const ID_MUSIC_STOP:  WidgetId = 301;
const ID_MUSIC_BAR:   WidgetId = 302;
fn id_level_row(i: usize) -> WidgetId { 1000 + i as WidgetId }

// ----- new settings widget ids -----
// Tabs.
const ID_TAB_GFX: WidgetId = 400;
const ID_TAB_AUD: WidgetId = 401;
const ID_TAB_GPL: WidgetId = 402;
const ID_TAB_ACC: WidgetId = 403;
const ID_TAB_CTL: WidgetId = 404;

// Graphics tab widgets. Cycle widgets occupy two ids (left / right
// arrow hit boxes) so hover easing tracks them independently.
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

// Audio tab widgets.
const ID_AUD_MASTER: WidgetId = 440;
const ID_AUD_MUSIC:  WidgetId = 441;
const ID_AUD_SFX:    WidgetId = 442;

// Gameplay tab widgets.
const ID_GPL_PSPEED:    WidgetId = 450;
const ID_GPL_WOBBLE:    WidgetId = 451;
const ID_GPL_ABILITY_L: WidgetId = 452;
const ID_GPL_ABILITY_R: WidgetId = 453;
const ID_GPL_CLOSE_FX:  WidgetId = 454;
const ID_GPL_HL_L:      WidgetId = 455;
const ID_GPL_HL_R:      WidgetId = 456;

// Accessibility tab widgets.
const ID_ACC_CONTRAST: WidgetId = 470;
const ID_ACC_REDUCE:   WidgetId = 471;
const ID_ACC_HITBOX:   WidgetId = 472;
const ID_ACC_CB_L:     WidgetId = 473;
const ID_ACC_CB_R:     WidgetId = 474;

// Typography.
const TITLE_PX:  f32 = 0.013;
const H1_PX:     f32 = 0.010;
const H2_PX:     f32 = 0.007;
const BUTTON_PX: f32 = 0.010;
const BODY_PX:   f32 = 0.005;
const SMALL_PX:  f32 = 0.0045;

// Palette defaults.
const ACCENT:      [f32; 3] = [1.00, 0.45, 0.75];
const ACCENT_HI:   [f32; 3] = [1.00, 0.80, 0.92];
const DIM:         [f32; 3] = [0.55, 0.42, 0.52];
const WHITE:       [f32; 3] = [1.00, 1.00, 1.00];
const BG_PANEL:    [f32; 3] = [0.10, 0.04, 0.15];
const BG_PANEL_HI: [f32; 3] = [0.22, 0.10, 0.30];
const BG_DEEP:     [f32; 3] = [0.03, 0.01, 0.06];

// Main menu bars.
const BAR_HW: f32 = 0.46;
const BAR_HH: f32 = 0.075;

// Music panel slide geometry.
const PANEL_PEEK_HEIGHT:     f32 = 0.028;
const PANEL_EXTENDED_HEIGHT: f32 = 0.26;
const PANEL_SLIDE_EASE:      f32 = 14.0;
const PANEL_CONTENT_LO:      f32 = 0.55;
const PANEL_CONTENT_HI:      f32 = 0.95;
const PANEL_INPUT_THRESHOLD: f32 = 0.60;

// Level browser geometry.
const HEADER_Y:          f32 = -0.90;
const TOP_SEPARATOR_Y:   f32 = -0.74;
const RAIL_TOP:          f32 = TOP_SEPARATOR_Y;
const RAIL_BOT:          f32 =  0.76;
const RAIL_RIGHT_TOP:    f32 = -0.30;
const RAIL_RIGHT_BOTTOM: f32 =  0.55;
const RAIL_EXTEND_LEFT:  f32 =  5.00;
const MOD_SEPARATOR_Y:   f32 =  0.78;
const MOD_ROW_TOP:       f32 =  0.80;
const MOD_ROW_BOT:       f32 =  0.94;

// Row geometry.
const ROW_HH:      f32 = 0.070;
const ROW_SPACING: f32 = 0.155;

// Card geometry.
const CARD_WIDTH: f32 = 1.20;

// Settings screen geometry.
const SETTINGS_TAB_Y:       f32 = -0.76;
const SETTINGS_TAB_H:       f32 =  0.050;
const SETTINGS_CONTENT_TOP: f32 = -0.64;
const SETTINGS_CONTENT_BOT: f32 =  0.82;

const ID_EDITOR: WidgetId = 4;

struct MainItem { label: &'static str, sub: &'static str, id: WidgetId }
const MAIN_ITEMS: &[MainItem] = &[
    MainItem { label: "PLAY",    sub: "START A RUN",      id: ID_PLAY    },
    MainItem { label: "EDITOR",  sub: "AUTHOR A LEVEL",   id: ID_EDITOR  },
    MainItem { label: "OPTIONS", sub: "TUNE THE FEEL",    id: ID_OPTIONS },
    MainItem { label: "QUIT",    sub: "EXIT TO DESKTOP",  id: ID_QUIT    },
];

const HOVER_EASE_RATE:   f32 = 14.0;
const PALETTE_EASE_RATE: f32 = 6.0;

/// Choice handed to the editor on entry. Set by the menu, read
/// once by main.rs when the editor is constructed, then reset.
pub enum EditorBoot {
    New,
    Existing(u32),
}

/// Snapshot of audio state polled once per frame.
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

    pointer: (f32, f32),
    left_was_down: bool,
    up_edge_was: bool,
    down_edge_was: bool,
    enter_edge_was: bool,

    dragging: Option<WidgetId>,

    selected: u32,
    difficulty_idx: Vec<u32>,

    hover:         HashMap<WidgetId, f32>,
    hover_target:  HashMap<WidgetId, bool>,

    theme_bg_a:   [f32; 3],
    theme_bg_b:   [f32; 3],
    theme_accent: [f32; 3],

    view_aspect: f32,
    editor_boot: Option<EditorBoot>,
    panel_t: f32,
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
            pointer: (0.0, 0.0),
            left_was_down: false,
            up_edge_was: false,
            down_edge_was: false,
            enter_edge_was: false,
            dragging: None,
            selected: 0,
            difficulty_idx,
            hover: HashMap::new(),
            hover_target: HashMap::new(),
            theme_bg_a:   BG_PANEL,
            theme_bg_b:   BG_DEEP,
            theme_accent: ACCENT,
            view_aspect: 16.0 / 9.0,
            panel_t: 0.0,
            editor_boot: None,
        }
    }

    pub fn state(&self)          -> AppState { self.state }
    pub fn selected_level(&self) -> u32      { self.selected }

    /// Track the main loop should currently be playing.
    pub fn desired_music(&self) -> Option<String> {
        match self.state {
            AppState::Intro    => Some(MENU_ENTER_MUSIC_PATH.to_string()),
            AppState::MainMenu | AppState::Settings => Some(MENU_MUSIC_PATH.to_string()),
            AppState::LevelSelect => {
                let lvl = crate::levels::get(self.selected);
                if lvl.music_path.is_empty() { None } else { Some(lvl.music_path.clone()) }
            }
            AppState::Editor | AppState::EditorPlay { .. } =>
                Some(MENU_MUSIC_PATH.to_string()),
            AppState::Playing { level, .. } => {
                let lvl = crate::levels::get(level);
                if lvl.music_path.is_empty() { None } else { Some(lvl.music_path.clone()) }
            }
            AppState::Quit => None,
        }
    }

    pub fn desired_bpm(&self) -> u32 {
        match self.state {
            AppState::Intro | AppState::MainMenu
                | AppState::Settings | AppState::LevelSelect => MENU_BPM,
            AppState::Editor | AppState::EditorPlay { .. } => MENU_BPM,
            AppState::Playing { level, .. } => crate::levels::get(level).music_bpm,
            AppState::Quit => MENU_BPM,
        }
    }

    pub fn return_to_main(&mut self) {
        self.state = AppState::MainMenu;
        self.dragging = None;
    }

    // ---------- view bounds ----------

    fn view_left(&self) -> f32 {
        if self.view_aspect >= 1.0 { -self.view_aspect } else { -1.0 }
    }
    fn view_right(&self) -> f32 { -self.view_left() }
    fn view_top(&self) -> f32 {
        if self.view_aspect >= 1.0 { -1.0 } else { -1.0 / self.view_aspect }
    }
    fn view_bottom(&self) -> f32 { -self.view_top() }

    /// Pop the pending editor boot directive. Called once by
    /// main.rs when the editor instance is being created.
    pub fn take_editor_boot(&mut self) -> EditorBoot {
        self.editor_boot.take().unwrap_or(EditorBoot::New)
    }

    /// Switch into the play-test sub-state. The editor frame
    /// stays on the main loop's stack; only the rendering and
    /// input routing change.
    pub fn enter_editor_play(&mut self, tier_idx: u32) {
        self.state = AppState::EditorPlay { tier_idx };
    }

    /// Return from a play-test back to the editor.
    pub fn return_to_editor(&mut self) {
        self.state = AppState::Editor;
        self.dragging = None;
    }

    // ---------- panel geometry ----------

    fn panel_bot(&self) -> f32 {
        let top = self.view_top();
        let peek = top + PANEL_PEEK_HEIGHT;
        let ext  = top + PANEL_EXTENDED_HEIGHT;
        peek + (ext - peek) * self.panel_t
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
        self.view_aspect = (cw.max(1) as f32) / (ch.max(1) as f32);
        audio.set_volume(config.master_volume);

        if snap.onset_counter != self.prev_onset_counter {
            self.last_onset_time = self.time;
        }
        self.prev_onset_counter = snap.onset_counter;

        self.hover_target.clear();

        let (sx, sy) = crate::renderer::aspect_scale(cw, ch);
        let cwf = cw.max(1) as f32;
        let chf = ch.max(1) as f32;
        let nx = ((mouse.x as f32 / cwf) * 2.0 - 1.0) / sx;
        let ny = ((mouse.y as f32 / chf) * 2.0 - 1.0) / sy;
        self.pointer = (nx, ny);

        let clicked  =  mouse.left_down && !self.left_was_down;
        let released = !mouse.left_down &&  self.left_was_down;
        self.left_was_down = mouse.left_down;

        let up_edge    = input.up    && !self.up_edge_was;
        let down_edge  = input.down  && !self.down_edge_was;
        let enter_edge = input.enter && !self.enter_edge_was;
        self.up_edge_was    = input.up;
        self.down_edge_was  = input.down;
        self.enter_edge_was = input.enter;

        if released { self.dragging = None; }

        // Slide the music panel.
        let want_open = snap.has_music && {
            let top = self.view_top();
            ny >= top && ny <= self.panel_bot() + 0.015
        };
        let target_t = if want_open { 1.0 } else { 0.0 };
        let pb = 1.0 - (-PANEL_SLIDE_EASE * dt).exp();
        self.panel_t += (target_t - self.panel_t) * pb;
        if self.panel_t < 0.0005 { self.panel_t = 0.0; }
        if self.panel_t > 0.9995 { self.panel_t = 1.0; }

        if let Some(id) = self.dragging {
            // Dragged sliders: handle drag here.
            self.handle_slider_drag(id, nx, config);
            if id == ID_MUSIC_BAR {
                if snap.duration > 0.0 {
                    let (bx0, bx1) = self.music_panel_bar_range();
                    let t = ((nx - bx0) / (bx1 - bx0)).clamp(0.0, 1.0);
                    audio.seek_music(t * snap.duration);
                }
            }
        } else {
            let panel_hot = snap.has_music
                && self.panel_t >= PANEL_INPUT_THRESHOLD
                && ny >= self.view_top()
                && ny <= self.panel_bot();
            let panel_handled = if panel_hot {
                self.tick_music_panel(nx, ny, clicked, audio, snap)
            } else { false };

            if !panel_handled {
                match self.state {
                    AppState::Intro => {
                        self.intro_time += dt;
                        if self.intro_time >= INTRO_SECONDS || clicked || enter_edge {
                            self.state = AppState::MainMenu;
                        }
                    }
                    AppState::MainMenu  => self.tick_main(nx, ny, clicked, audio),
                    AppState::LevelSelect => self.tick_level_select(
                        nx, ny, clicked, up_edge, down_edge, enter_edge, audio),
                    AppState::Settings  => self.tick_settings(nx, ny, clicked, audio, config),
                    _ => {}
                }
            }
        }

        // Hover easing.
        let blend = 1.0 - (-HOVER_EASE_RATE * dt).exp();
        let keys: Vec<WidgetId> = self.hover.keys().copied().collect();
        for id in keys {
            let target = if *self.hover_target.get(&id).unwrap_or(&false) { 1.0 } else { 0.0 };
            let v = self.hover.get_mut(&id).unwrap();
            *v += (target - *v) * blend;
            if *v < 0.001 && target == 0.0 { self.hover.remove(&id); }
        }
        for (&id, &t) in &self.hover_target {
            if t && !self.hover.contains_key(&id) {
                self.hover.insert(id, 0.0);
            }
        }

        // Palette ease.
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

    fn set_hover(&mut self, id: WidgetId, v: bool) {
        self.hover_target.insert(id, v);
    }

    fn hover_eased(&self, id: WidgetId) -> f32 {
        let t = *self.hover.get(&id).unwrap_or(&0.0);
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    fn beat_flash(&self) -> f32 {
        if self.last_onset_time < 0.0 { return 0.0; }
        let dt = (self.time - self.last_onset_time).max(0.0);
        (-dt * 7.0).exp()
    }

    // ---------- screen ticks ----------

    fn tick_main(&mut self, nx: f32, ny: f32, clicked: bool, audio: &Audio) {
        for (i, item) in MAIN_ITEMS.iter().enumerate() {
            let (cx, cy) = main_item_pos(i);
            let hit = hit_rect(nx, ny, cx, cy, BAR_HW, BAR_HH);
            self.set_hover(item.id, hit);
            if hit && clicked {
                audio.play_interact();
                self.state = match item.id {
                    ID_PLAY    => AppState::LevelSelect,
                    ID_OPTIONS => AppState::Settings,
                    ID_EDITOR  => {
                        self.editor_boot = Some(EditorBoot::New);
                        AppState::Editor
                    }
                    ID_QUIT    => AppState::Quit,
                    _ => self.state,
                };
                return;
            }
        }
    }

    fn tick_level_select(
        &mut self,
        nx: f32, ny: f32, clicked: bool,
        up_edge: bool, down_edge: bool, enter_edge: bool,
        audio: &Audio,
    ) {
        let n = crate::levels::num();

        let (bcx, bcy, bhw, bhh) = self.back_button_rect();
        let back_hit = hit_rect(nx, ny, bcx, bcy, bhw, bhh);
        self.set_hover(ID_BACK, back_hit);
        if back_hit && clicked {
            audio.play_interact();
            self.state = AppState::MainMenu;
            return;
        }

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

        let rail_left = self.view_left();
        for i in 0..n {
            let y = row_y(i, self.selected);
            let id = id_level_row(i);
            let xr = rail_right_at(y);
            let hx = (rail_left + xr) * 0.5;
            let hw = (xr - rail_left) * 0.5;
            let hit = hit_rect(nx, ny, hx, y, hw, ROW_HH);
            self.set_hover(id, hit);
            if hit && clicked {
                if self.selected == i as u32 {
                    audio.play_enter();
                    self.state = AppState::Playing {
                        level: self.selected,
                        difficulty_idx: self.difficulty_idx[self.selected as usize],
                    };
                    return;
                }
                self.selected = i as u32;
                audio.play_interact();
                return;
            }
        }

        // Difficulty arrows inside the card.
        let (cx0, _, cx1, _) = self.card_rect();
        let dcx = (cx0 + cx1) * 0.5;
        let dy  = 0.38;
        let lhit = hit_rect(nx, ny, dcx - 0.30, dy, 0.06, 0.05);
        let rhit = hit_rect(nx, ny, dcx + 0.30, dy, 0.06, 0.05);
        self.set_hover(ID_DIFF_LEFT, lhit);
        self.set_hover(ID_DIFF_RIGHT, rhit);
        if lhit && clicked {
            audio.play_interact();
            let di = &mut self.difficulty_idx[self.selected as usize];
            if *di > 0 { *di -= 1; }
        }
        if rhit && clicked {
            audio.play_interact();
            let lvl = crate::levels::get(self.selected);
            let di = &mut self.difficulty_idx[self.selected as usize];
            if (*di as usize) + 1 < lvl.difficulty_tiers.len() { *di += 1; }
        }

        // PLAY button inside the card.
        let pcx = dcx;
        let pcy = self.card_rect().3 - 0.08;
        let phit = hit_rect(nx, ny, pcx, pcy, 0.22, 0.055);
        self.set_hover(ID_PLAY_LEVEL, phit);
        if phit && clicked {
            audio.play_enter();
            self.state = AppState::Playing {
                level: self.selected,
                difficulty_idx: self.difficulty_idx[self.selected as usize],
            };
        }
    }

    /// Settings screen. Tabbed layout; each tab is a short flat
    /// list of rows. Hit testing and drawing are split between
    /// `tick_settings` (here) and `draw_settings` (later).
    fn tick_settings(
        &mut self, nx: f32, ny: f32, clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        // Back / reset buttons.
        let (bcx, bcy, bhw, bhh) = self.back_button_rect();
        let back_hit = hit_rect(nx, ny, bcx, bcy, bhw, bhh);
        self.set_hover(ID_BACK, back_hit);
        if back_hit && clicked {
            audio.play_interact();
            config.clamp();
            config.save();
            self.state = AppState::MainMenu;
            return;
        }
        let (rcx, rcy, rhw, rhh) = self.reset_button_rect();
        let reset_hit = hit_rect(nx, ny, rcx, rcy, rhw, rhh);
        self.set_hover(ID_RESET, reset_hit);
        if reset_hit && clicked {
            audio.play_interact();
            *config = Config::default();
            return;
        }

        // Tab bar.
        let tabs = [
            ("GRAPHICS",     ID_TAB_GFX),
            ("AUDIO",        ID_TAB_AUD),
            ("GAMEPLAY",     ID_TAB_GPL),
            ("ACCESS",       ID_TAB_ACC),
            ("CONTROLS",     ID_TAB_CTL),
        ];
        for (i, (_, id)) in tabs.iter().enumerate() {
            let (cx, cy, hw, hh) = self.tab_rect(i, tabs.len());
            let hit = hit_rect(nx, ny, cx, cy, hw, hh);
            self.set_hover(*id, hit);
            if hit && clicked && self.settings_tab != i {
                self.settings_tab = i;
                audio.play_interact();
                return;
            }
        }

        // Delegate to the active tab.
        match self.settings_tab {
            0 => self.tick_tab_graphics     (nx, ny, clicked, audio, config),
            1 => self.tick_tab_audio        (nx, ny, clicked, audio, config),
            2 => self.tick_tab_gameplay     (nx, ny, clicked, audio, config),
            3 => self.tick_tab_accessibility(nx, ny, clicked, audio, config),
            _ => {} // controls tab is static text
        }

        config.clamp();
    }

    fn tick_tab_graphics(
        &mut self, nx: f32, ny: f32, clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 11;
        // Row 0: VSync cycle.
        let (cx0, cy0) = self.row_center(0, rows);
        if self.tick_cycle(nx, ny, clicked, audio,
            cx0, cy0, ID_GFX_VSYNC_L, ID_GFX_VSYNC_R)
        {
            // Handled below in draw; we detect click direction here.
        }
        if self.arrow_clicked(nx, ny, clicked, cx0, cy0, false) {
            config.vsync = vsync_prev(config.vsync);
            audio.play_interact();
        }
        if self.arrow_clicked(nx, ny, clicked, cx0, cy0, true) {
            config.vsync = config.vsync.next();
            audio.play_interact();
        }

        // Row 1: FPS cap cycle.
        let (_cx, cy1) = self.row_center(1, rows);
        let cxf = cy1; // unused alias just to keep symmetry; actual x is fixed
        let _ = cxf;
        let (axc, ayc) = self.row_center(1, rows);
        self.set_hover_arrows(nx, ny, axc, ayc, ID_GFX_FPS_L, ID_GFX_FPS_R);
        if self.arrow_clicked(nx, ny, clicked, axc, ayc, false) {
            config.fps_cap = fps_prev(config.fps_cap);
            audio.play_interact();
        }
        if self.arrow_clicked(nx, ny, clicked, axc, ayc, true) {
            config.fps_cap = fps_next(config.fps_cap);
            audio.play_interact();
        }

        // Rows 2..=7: sliders.
        self.tick_slider(nx, ny, clicked, 2, rows,
            ID_GFX_BLOOM, &mut config.bloom_intensity, 0.0, 1.5, audio);
        self.tick_slider(nx, ny, clicked, 3, rows,
            ID_GFX_CHROMATIC, &mut config.chromatic_strength, 0.0, 1.0, audio);
        self.tick_slider(nx, ny, clicked, 4, rows,
            ID_GFX_SHAKE, &mut config.screen_shake, 0.0, 1.5, audio);
        self.tick_slider(nx, ny, clicked, 5, rows,
            ID_GFX_VIGNETTE, &mut config.vignette, 0.0, 1.0, audio);
        self.tick_slider(nx, ny, clicked, 6, rows,
            ID_GFX_BEAT_FLASH, &mut config.beat_flash, 0.0, 1.5, audio);
        self.tick_slider(nx, ny, clicked, 7, rows,
            ID_GFX_DEPTH, &mut config.fake_3d_depth, 0.0, 1.0, audio);

        // Row 8: Particles cycle.
        let (_, py8) = self.row_center(8, rows);
        self.set_hover_arrows(nx, ny, 0.0, py8, ID_GFX_PART_L, ID_GFX_PART_R);
        if self.arrow_clicked(nx, ny, clicked, 0.0, py8, false) {
            config.particle_density = particle_prev(config.particle_density);
            audio.play_interact();
        }
        if self.arrow_clicked(nx, ny, clicked, 0.0, py8, true) {
            config.particle_density = config.particle_density.next();
            audio.play_interact();
        }

        // Rows 9..=10: toggles.
        self.tick_toggle(nx, ny, clicked, 9, rows, ID_GFX_SCANLINES,
            &mut config.scanlines, audio);
        self.tick_toggle(nx, ny, clicked, 10, rows, ID_GFX_SHOW_FPS,
            &mut config.show_fps, audio);
    }

    fn tick_tab_audio(
        &mut self, nx: f32, ny: f32, clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 3;
        self.tick_slider(nx, ny, clicked, 0, rows,
            ID_AUD_MASTER, &mut config.master_volume, 0.0, 1.0, audio);
        self.tick_slider(nx, ny, clicked, 1, rows,
            ID_AUD_MUSIC,  &mut config.music_volume,  0.0, 1.0, audio);
        self.tick_slider(nx, ny, clicked, 2, rows,
            ID_AUD_SFX,    &mut config.sfx_volume,    0.0, 1.0, audio);
    }

    fn tick_tab_gameplay(
        &mut self, nx: f32, ny: f32, clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 5;
        self.tick_slider(nx, ny, clicked, 0, rows,
            ID_GPL_PSPEED, &mut config.player_speed, 0.5, 2.0, audio);
        self.tick_slider(nx, ny, clicked, 1, rows,
            ID_GPL_WOBBLE, &mut config.camera_wobble, 0.0, 1.0, audio);

        let (_, py2) = self.row_center(2, rows);
        self.set_hover_arrows(nx, ny, 0.0, py2, ID_GPL_ABILITY_L, ID_GPL_ABILITY_R);
        if self.arrow_clicked(nx, ny, clicked, 0.0, py2, false) {
            config.ability = ability_prev(config.ability);
            audio.play_interact();
        }
        if self.arrow_clicked(nx, ny, clicked, 0.0, py2, true) {
            config.ability = config.ability.next();
            audio.play_interact();
        }

        self.tick_toggle(nx, ny, clicked, 3, rows, ID_GPL_CLOSE_FX,
            &mut config.close_call_fx, audio);

        let (_, py4) = self.row_center(4, rows);
        self.set_hover_arrows(nx, ny, 0.0, py4, ID_GPL_HL_L, ID_GPL_HL_R);
        if self.arrow_clicked(nx, ny, clicked, 0.0, py4, false) {
            config.predictive_highlight = highlight_prev(config.predictive_highlight);
            audio.play_interact();
        }
        if self.arrow_clicked(nx, ny, clicked, 0.0, py4, true) {
            config.predictive_highlight = config.predictive_highlight.next();
            audio.play_interact();
        }
    }

    fn tick_tab_accessibility(
        &mut self, nx: f32, ny: f32, clicked: bool,
        audio: &Audio, config: &mut Config,
    ) {
        let rows = 4;
        self.tick_toggle(nx, ny, clicked, 0, rows, ID_ACC_CONTRAST,
            &mut config.high_contrast, audio);
        self.tick_toggle(nx, ny, clicked, 1, rows, ID_ACC_REDUCE,
            &mut config.reduce_motion, audio);
        self.tick_toggle(nx, ny, clicked, 2, rows, ID_ACC_HITBOX,
            &mut config.show_hitboxes, audio);

        let (_, py3) = self.row_center(3, rows);
        self.set_hover_arrows(nx, ny, 0.0, py3, ID_ACC_CB_L, ID_ACC_CB_R);
        if self.arrow_clicked(nx, ny, clicked, 0.0, py3, false) {
            config.colorblind = colorblind_prev(config.colorblind);
            audio.play_interact();
        }
        if self.arrow_clicked(nx, ny, clicked, 0.0, py3, true) {
            config.colorblind = config.colorblind.next();
            audio.play_interact();
        }
    }

    /// Unified slider tick for the settings screen. Returns true
    /// when the value changed during this call.
    fn tick_slider(
        &mut self, nx: f32, ny: f32, clicked: bool,
        row: usize, rows: usize, id: WidgetId,
        value: &mut f32, min: f32, max: f32,
        audio: &Audio,
    ) -> bool {
        let (x0, x1) = self.slider_track_range();
        let (_, y) = self.row_center(row, rows);

        let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
        let hx = x0 + (x1 - x0) * t;
        let handle_hit = hit_rect(nx, ny, hx, y, 0.05, 0.04);
        self.set_hover(id, handle_hit);
        let track_hit = nx > x0 - 0.02 && nx < x1 + 0.02 && (ny - y).abs() < 0.04;
        if clicked && (handle_hit || track_hit) {
            audio.play_interact();
            self.dragging = Some(id);
            let nt = ((nx - x0) / (x1 - x0)).clamp(0.0, 1.0);
            *value = min + (max - min) * nt;
            return true;
        }
        false
    }

    /// Dragging a slider after it was grabbed.
    fn handle_slider_drag(&mut self, id: WidgetId, nx: f32, config: &mut Config) {
        // Map id to (value, min, max).
        let (x0, x1) = self.slider_track_range();
        let t = ((nx - x0) / (x1 - x0)).clamp(0.0, 1.0);
        match id {
            ID_GFX_BLOOM        => config.bloom_intensity    = 0.0 + (1.5 - 0.0) * t,
            ID_GFX_CHROMATIC    => config.chromatic_strength = 0.0 + (1.0 - 0.0) * t,
            ID_GFX_SHAKE        => config.screen_shake       = 0.0 + (1.5 - 0.0) * t,
            ID_GFX_VIGNETTE     => config.vignette           = 0.0 + (1.0 - 0.0) * t,
            ID_GFX_BEAT_FLASH   => config.beat_flash         = 0.0 + (1.5 - 0.0) * t,
            ID_GFX_DEPTH        => config.fake_3d_depth      = 0.0 + (1.0 - 0.0) * t,
            ID_AUD_MASTER       => config.master_volume      = t,
            ID_AUD_MUSIC        => config.music_volume       = t,
            ID_AUD_SFX          => config.sfx_volume         = t,
            ID_GPL_PSPEED       => config.player_speed       = 0.5 + (2.0 - 0.5) * t,
            ID_GPL_WOBBLE       => config.camera_wobble      = t,
            _ => {}
        }
    }

    /// Toggle widget: a small rectangular button showing ON / OFF.
    fn tick_toggle(
        &mut self, nx: f32, ny: f32, clicked: bool,
        row: usize, rows: usize, id: WidgetId,
        value: &mut bool, audio: &Audio,
    ) -> bool {
        let (cx, cy, hw, hh) = self.toggle_rect(row, rows);
        let hit = hit_rect(nx, ny, cx, cy, hw, hh);
        self.set_hover(id, hit);
        if hit && clicked {
            *value = !*value;
            audio.play_interact();
            return true;
        }
        false
    }

    /// Register hover targets for both arrows of a cycle widget at
    /// row center `(cx, cy)`.
    fn set_hover_arrows(
        &mut self, nx: f32, ny: f32,
        _cx: f32, cy: f32, id_l: WidgetId, id_r: WidgetId,
    ) {
        let (lx, rx) = self.arrow_positions();
        self.set_hover(id_l, hit_rect(nx, ny, lx, cy, 0.05, 0.04));
        self.set_hover(id_r, hit_rect(nx, ny, rx, cy, 0.05, 0.04));
    }

    /// Returns true if the arrow at the given row was clicked.
    fn arrow_clicked(
        &self, nx: f32, ny: f32, clicked: bool,
        _cx: f32, cy: f32, right: bool,
    ) -> bool {
        if !clicked { return false; }
        let (lx, rx) = self.arrow_positions();
        let ax = if right { rx } else { lx };
        hit_rect(nx, ny, ax, cy, 0.05, 0.04)
    }

    /// Legacy shim used by `tick_tab_graphics` for VSync row
    /// (keeps a consistent signature for the Graphics row 0).
    fn tick_cycle(
        &mut self, nx: f32, ny: f32, _clicked: bool, _audio: &Audio,
        _cx: f32, cy: f32, id_l: WidgetId, id_r: WidgetId,
    ) -> bool {
        self.set_hover_arrows(nx, ny, 0.0, cy, id_l, id_r);
        false
    }

    fn tick_music_panel(
        &mut self, nx: f32, ny: f32, clicked: bool,
        audio: &Audio, snap: &AudioSnapshot,
    ) -> bool {
        let cy = self.view_top() + PANEL_EXTENDED_HEIGHT - 0.06;
        let (bx0, bx1) = self.music_panel_bar_range();

        let ctrl_x0 = bx0 - 0.26;
        let pause_cx = ctrl_x0 + 0.050;
        let stop_cx  = ctrl_x0 + 0.155;
        let btn_hw   = 0.045;
        let btn_hh   = 0.030;

        let pause_hit = hit_rect(nx, ny, pause_cx, cy, btn_hw, btn_hh);
        self.set_hover(ID_MUSIC_PAUSE, pause_hit);
        if pause_hit && clicked {
            audio.play_interact();
            audio.set_music_paused(!snap.paused);
            return true;
        }

        let stop_hit = hit_rect(nx, ny, stop_cx, cy, btn_hw, btn_hh);
        self.set_hover(ID_MUSIC_STOP, stop_hit);
        if stop_hit && clicked {
            audio.play_interact();
            audio.stop_music();
            return true;
        }

        let bar_hit = nx > bx0 - 0.01
                   && nx < bx1 + 0.01
                   && (ny - cy).abs() < 0.022;
        self.set_hover(ID_MUSIC_BAR, bar_hit);
        if bar_hit && clicked && snap.duration > 0.0 {
            let t = ((nx - bx0) / (bx1 - bx0)).clamp(0.0, 1.0);
            audio.seek_music(t * snap.duration);
            self.dragging = Some(ID_MUSIC_BAR);
            return true;
        }

        clicked
    }

    // ---------- layout helpers ----------

    fn back_button_rect(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_left() + 0.13;
        let cy = HEADER_Y;
        (cx, cy, 0.09, 0.040)
    }

    fn reset_button_rect(&self) -> (f32, f32, f32, f32) {
        let cx = self.view_right() - 0.15;
        let cy = HEADER_Y;
        (cx, cy, 0.09, 0.040)
    }

    fn card_rect(&self) -> (f32, f32, f32, f32) {
        let x1 = self.view_right() - 0.04;
        let x0 = x1 - CARD_WIDTH;
        let y0 = self.view_top() + PANEL_PEEK_HEIGHT + 0.03;
        let y1 = MOD_SEPARATOR_Y - 0.02;
        (x0, y0, x1, y1)
    }

    fn music_panel_bar_range(&self) -> (f32, f32) {
        let right = self.view_right() - 0.55;
        let left  = self.view_left() + 0.80;
        (left, right)
    }

    fn tab_rect(&self, i: usize, n: usize) -> (f32, f32, f32, f32) {
        let vl = self.view_left()  + 0.08;
        let vr = self.view_right() - 0.08;
        let total = vr - vl;
        let gap = 0.010;
        let cell = (total - gap * (n as f32 - 1.0)) / n as f32;
        let x0 = vl + (cell + gap) * i as f32;
        let x1 = x0 + cell;
        let cx = (x0 + x1) * 0.5;
        let hw = (x1 - x0) * 0.5;
        (cx, SETTINGS_TAB_Y, hw, SETTINGS_TAB_H)
    }

    fn row_center(&self, row: usize, total: usize) -> (f32, f32) {
        let total = total.max(1);
        let top = SETTINGS_CONTENT_TOP;
        let bot = SETTINGS_CONTENT_BOT;
        let row_h = (bot - top) / total as f32;
        let cy = top + row_h * (row as f32 + 0.5);
        (0.0, cy)
    }

    fn slider_track_range(&self) -> (f32, f32) {
        // Aligned so the track does not collide with the left
        // label column. Values mirror the legacy slider range
        // for familiarity.
        let vl = self.view_left()  + 0.08;
        let x0 = vl + 0.52;
        let x1 = vl + 1.30;
        (x0, x1)
    }

    fn arrow_positions(&self) -> (f32, f32) {
        // Left and right arrow X for cycle widgets on the settings
        // screen. The label sits to the left of `lx` and the name
        // reads is centered between the two arrows.
        let vl = self.view_left() + 0.08;
        let lx = vl + 0.62;
        let rx = vl + 1.22;
        (lx, rx)
    }

    fn toggle_rect(&self, row: usize, rows: usize) -> (f32, f32, f32, f32) {
        let (_, cy) = self.row_center(row, rows);
        let vl = self.view_left() + 0.08;
        let cx = vl + 0.92;
        (cx, cy, 0.12, 0.032)
    }

    // ---------- rendering ----------

    pub fn build_geometry(
        &self, out: &mut Vec<Vertex>, snap: &AudioSnapshot, config: &Config,
    ) {
        out.clear();
        self.draw_background(out);
        match self.state {
            AppState::Intro       => self.draw_intro(out),
            AppState::MainMenu    => self.draw_main(out),
            AppState::LevelSelect => self.draw_level_select(out),
            AppState::Settings    => self.draw_settings(out, config),
            _ => {}
        }
        self.draw_music_panel(out, snap);
    }

    fn draw_background(&self, out: &mut Vec<Vertex>) {
        let flash = self.beat_flash();
        let t = self.time;
        let n = 12;
        let span = TAU / n as f32;
        let rot = t * 0.04;
        let a = mix3(self.theme_bg_a, WHITE, 0.04 * flash);
        let b = self.theme_bg_b;
        for s in 0..n {
            let a0 = s as f32 * span + rot;
            let a1 = a0 + span;
            let color = if s % 2 == 0 { a } else { b };
            push_tri(out,
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
            let c = mix3([0.18, 0.07, 0.16], self.theme_accent, 0.25);
            push_hex(out, px, py, r, c);
        }
    }

    fn draw_intro(&self, out: &mut Vec<Vertex>) {
        let t = self.intro_time;
        let d = INTRO_SECONDS;
        let fade_in  = smoothstep(0.0, 0.8, t);
        let fade_out = 1.0 - smoothstep(d - 0.6, d, t);
        let alpha    = fade_in * fade_out;

        let ring_t = smoothstep(0.0, 1.5, t);
        let ring_r = ring_t * 0.9;
        let count = 24;
        for i in 0..count {
            let ang = i as f32 / count as f32 * TAU;
            let x = ang.cos() * ring_r;
            let y = ang.sin() * ring_r;
            let size = 0.015 * alpha;
            let col = mix3(BG_DEEP, ACCENT, alpha);
            push_hex(out, x, y, size, col);
        }
        let contract = 1.0 - 0.1 * smoothstep(d - 0.6, d, t);
        let r0 = 0.22 * contract;
        let r1 = 0.15 * contract;
        let r2 = 0.07 * contract;
        let tint = |c: [f32; 3]| mix3(BG_DEEP, c, alpha);
        push_hex_rot(out, 0.0, -0.05, r0, tint(BG_PANEL),    t * 0.4);
        push_hex_ring_rot(out, 0.0, -0.05, r0, r0 - 0.012, tint(ACCENT),    t * 0.4);
        push_hex_rot(out, 0.0, -0.05, r1, tint(BG_PANEL_HI), -t * 0.9);
        push_hex_ring_rot(out, 0.0, -0.05, r1, r1 - 0.010, tint(ACCENT_HI), -t * 0.9);
        push_hex_rot(out, 0.0, -0.05, r2, tint(WHITE),       t * 1.6);

        let slide = (1.0 - fade_in) * 0.08;
        push_text_centered_alpha(out, "SUPER RUSTOGON", 0.0, 0.28 + slide, TITLE_PX, ACCENT_HI, alpha);
        push_text_centered_alpha(out, "A HEXAGONAL DESCENT", 0.0, 0.42 + slide, H2_PX, WHITE, alpha * 0.7);
        push_text_centered_alpha(out, "CLICK OR ENTER TO SKIP", 0.0, 0.85, SMALL_PX, DIM, (alpha * 0.8).max(0.0));
    }

    fn draw_main(&self, out: &mut Vec<Vertex>) {
        let title_y = -0.80;
        push_text_centered(out, "SUPER RUSTOGON",  0.006, title_y + 0.010, TITLE_PX, BG_DEEP);
        push_text_centered(out, "SUPER RUSTOGON",  0.000, title_y,         TITLE_PX, ACCENT_HI);
        push_text_centered(out, "A HEXAGONAL DESCENT", 0.0, title_y + 0.12, H2_PX, DIM);

        let lw = 0.36;
        push_quad(out, -0.52 - lw, title_y + 0.14, -0.52, title_y + 0.145, ACCENT);
        push_quad(out,  0.52,      title_y + 0.14,  0.52 + lw, title_y + 0.145, ACCENT);

        for (i, item) in MAIN_ITEMS.iter().enumerate() {
            let (cx, cy) = main_item_pos(i);
            let h = self.hover_eased(item.id);
            self.draw_menu_bar(out, cx, cy, item.label, item.sub, h);
        }

        push_text(out, "V0.2", self.view_left() + 0.03, self.view_bottom() - 0.06, SMALL_PX, DIM);
        push_text_centered(out, "MUSIC FROM OPENGAMEART", 0.0, self.view_bottom() - 0.06, SMALL_PX, DIM);
        push_text_right(out, "ESC TO QUIT", self.view_right() - 0.03, self.view_bottom() - 0.06, SMALL_PX, DIM);
    }

    fn draw_menu_bar(
        &self, out: &mut Vec<Vertex>,
        cx: f32, cy: f32, label: &str, sub: &str, h: f32,
    ) {
        let slide = h * 0.025;
        let bg   = mix3(BG_PANEL, BG_PANEL_HI, h);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        let x0 = cx - BAR_HW + slide;
        let x1 = cx + BAR_HW + slide;
        let y0 = cy - BAR_HH;
        let y1 = cy + BAR_HH;
        push_quad(out, x0, y0, x1, y1, bg);
        push_quad(out, x0, y0, x0 + 0.024, y1, ring);
        if h > 0.01 {
            push_outline(out, x0, y0, x1, y1, 0.004 + 0.002 * h, ring);
            let chev_col = mix3(bg, ring, 0.5 + 0.5 * h);
            push_tri(out,
                [x1 - 0.075, cy - 0.035],
                [x1 - 0.075, cy + 0.035],
                [x1 - 0.030, cy],
                chev_col);
        }
        let label_y = cy - text_height(BUTTON_PX) * 0.5 - 0.010;
        push_text(out, label, x0 + 0.055, label_y, BUTTON_PX, WHITE);
        let sub_y = label_y + text_height(BUTTON_PX) + 0.010;
        push_text(out, sub, x0 + 0.055, sub_y, SMALL_PX, DIM);
    }

    fn draw_level_select(&self, out: &mut Vec<Vertex>) {
        let lvl   = crate::levels::get(self.selected);
        let pal   = &lvl.palette;
        let t     = self.time;
        let flash = self.beat_flash();

        // Header row: BACK button then title + subtitle.
        let (bcx, bcy, bhw, bhh) = self.back_button_rect();
        self.draw_small_button(out, bcx, bcy, bhw, bhh, "BACK", ID_BACK);

        let title_x = bcx + bhw + 0.04;
        let title_y = HEADER_Y - text_height(TITLE_PX) * 0.5;
        push_text(out, "SELECT A LEVEL", title_x, title_y, TITLE_PX, ACCENT_HI);
        let sub_y = title_y + text_height(TITLE_PX) + 0.008;
        push_text(out, "UP / DOWN TO NAVIGATE - ENTER TO PLAY",
            title_x, sub_y, SMALL_PX, DIM);

        self.draw_level_rail(out);
        self.draw_level_card(out, lvl, pal, t, flash);
        self.draw_modifier_row(out);
    }

    fn draw_level_rail(&self, out: &mut Vec<Vertex>) {
        let n = crate::levels::num();
        for i in 0..n {
            let y = row_y(i, self.selected);
            if y < RAIL_TOP - 0.25 || y > MOD_SEPARATOR_Y + 0.25 { continue; }
            let id = id_level_row(i);
            let hover = self.hover_eased(id);
            self.draw_level_row(out, y, i, hover);
        }

        push_quad(out,
            self.view_left(), TOP_SEPARATOR_Y,
            self.view_right(), TOP_SEPARATOR_Y + 0.004,
            ACCENT);

        push_quad(out,
            self.view_left(), MOD_SEPARATOR_Y,
            self.view_right(), MOD_SEPARATOR_Y + 0.004,
            ACCENT);

        push_slanted_edge(out,
            RAIL_RIGHT_TOP, RAIL_RIGHT_BOTTOM,
            TOP_SEPARATOR_Y, MOD_SEPARATOR_Y,
            0.005, ACCENT);
    }

    fn draw_level_row(
        &self, out: &mut Vec<Vertex>, cy: f32, idx: usize, hover: f32,
    ) {
        let lvl = crate::levels::get(idx as u32);
        let pal = &lvl.palette;
        let is_selected = idx as u32 == self.selected;

        let unclipped_top = cy - ROW_HH;
        let unclipped_bot = cy + ROW_HH;
        if unclipped_bot <= TOP_SEPARATOR_Y + 0.004 { return; }
        if unclipped_top >= MOD_SEPARATOR_Y { return; }
        let y_top = unclipped_top.max(TOP_SEPARATOR_Y + 0.004);
        let y_bot = unclipped_bot.min(MOD_SEPARATOR_Y);

        let xr_top = rail_right_at(y_top);
        let xr_bot = rail_right_at(y_bot);

        let view_l = self.view_left();
        let row_left_fill = view_l - RAIL_EXTEND_LEFT;

        if is_selected {
            let bg = mix3(BG_PANEL_HI, pal.bg_a, 0.50);
            push_trapezoid(out, row_left_fill, row_left_fill,
                xr_top, xr_bot, y_top, y_bot, bg);

            let delim = 0.004;
            push_quad(out, row_left_fill, y_top,
                xr_top, y_top + delim, pal.accent);
            push_quad(out, row_left_fill, y_bot - delim,
                xr_bot, y_bot, pal.accent);
        } else if hover > 0.01 {
            let bg = mix3(BG_PANEL, BG_PANEL_HI, hover);
            push_trapezoid(out, row_left_fill, row_left_fill,
                xr_top, xr_bot, y_top, y_bot, bg);
        }

        let content_x = view_l + 0.02;
        let col_stripe_w = 0.022;
        let col_icon_w   = 0.090;
        let col_num_w    = 0.130;
        let col_gap      = 0.030;

        let stripe_top = (cy - ROW_HH + 0.006).max(TOP_SEPARATOR_Y + 0.008);
        let stripe_bot = (cy + ROW_HH - 0.006).min(MOD_SEPARATOR_Y - 0.002);
        if stripe_bot > stripe_top {
            push_quad(out,
                content_x, stripe_top,
                content_x + col_stripe_w, stripe_bot,
                pal.accent);
        }

        if cy < TOP_SEPARATOR_Y + 0.006 { return; }
        if cy > MOD_SEPARATOR_Y - 0.006 { return; }

        let hex_cx = content_x + col_stripe_w + col_icon_w * 0.5;
        push_hex(out, hex_cx, cy, 0.035, pal.bg_a);
        push_hex_ring(out, hex_cx, cy, 0.035, 0.028, pal.accent);
        push_hex(out, hex_cx, cy, 0.014, pal.player);

        let num_col_left = content_x + col_stripe_w + col_icon_w;
        let num_col_right = num_col_left + col_num_w;
        let num = format!("{:02}", idx + 1);
        let num_color = if is_selected { pal.accent } else { DIM };
        let num_y = cy - text_height(H1_PX) * 0.5 - 0.005;
        push_text_right(out, &num, num_col_right, num_y, H1_PX, num_color);

        let text_x = num_col_right + col_gap;
        let name_h = text_height(H1_PX);
        let sub_h  = text_height(SMALL_PX);
        let gap    = 0.005;
        let total_h = name_h + gap + sub_h;
        let name_y = cy - total_h * 0.5;
        let sub_y  = name_y + name_h + gap;

        let slant_limit = rail_right_at(name_y) - 0.04;
        if text_x < slant_limit {
            push_text(out, &lvl.name,     text_x, name_y, H1_PX,    WHITE);
            push_text(out, &lvl.subtitle, text_x, sub_y,  SMALL_PX, DIM);
        }
    }

    fn draw_level_card(
        &self, out: &mut Vec<Vertex>,
        lvl: &crate::levels::Level, pal: &Palette,
        t: f32, flash: f32,
    ) {
        let (x0, y0, x1, y1) = self.card_rect();

        push_quad(out, x0, y0, x1, y1, mix3(BG_DEEP, pal.bg_a, 0.45));
        push_outline(out, x0, y0, x1, y1, 0.004, pal.accent);

        let pad = 0.035;
        let line_h_h1 = text_height(H1_PX) + 0.006;
        let line_h_h2 = text_height(H2_PX) + 0.005;
        let line_h_sm = text_height(SMALL_PX) + 0.004;

        let hex_cx = x0 + 0.14;
        let hex_cy = y0 + 0.18;
        let r0 = 0.11;
        let r1 = 0.075;
        let r2 = 0.042;
        push_hex_rot     (out, hex_cx, hex_cy, r0, pal.bg_a,           t * 0.20);
        push_hex_ring_rot(out, hex_cx, hex_cy, r0, r0 - 0.012, pal.accent,       t * 0.20);
        push_hex_rot     (out, hex_cx, hex_cy, r1, pal.bg_b,          -t * 0.45);
        push_hex_ring_rot(out, hex_cx, hex_cy, r1, r1 - 0.010, pal.center_ring, -t * 0.45);
        push_hex_rot(out, hex_cx, hex_cy, r2 + 0.006 * flash, pal.player, t * 0.85);

        let info_x = x0 + 0.30;
        let mut y = y0 + pad;
        push_text(out, "TRACK", info_x, y, SMALL_PX, DIM);
        y += line_h_sm;
        push_text(out, &lvl.song, info_x, y, H1_PX, pal.accent);
        y += line_h_h1 + 0.006;
        push_text(out, "ARTIST", info_x, y, SMALL_PX, DIM);
        y += line_h_sm;
        push_text(out, &lvl.artist, info_x, y, H2_PX, WHITE);
        y += line_h_h2 + 0.006;
        let bpm = format!("{} BPM", lvl.music_bpm);
        push_text(out, "TEMPO", info_x, y, SMALL_PX, DIM);
        y += line_h_sm;
        push_text(out, &bpm, info_x, y, H2_PX, pal.accent);

        let divider_y = y0 + 0.45;
        push_quad(out, x0 + pad, divider_y, x1 - pad, divider_y + 0.004,
            mix3(BG_DEEP, pal.accent, 0.3));

        let mut y = divider_y + 0.02;
        push_text(out, &lvl.name, x0 + pad, y, H1_PX, WHITE);
        y += line_h_h1;
        push_text(out, &lvl.subtitle, x0 + pad, y, SMALL_PX, DIM);
        y += line_h_sm + 0.006;

        push_text(out, "DESCRIPTION", x0 + pad, y, SMALL_PX, DIM);
        y += line_h_sm;
        push_text_wrapped(out, &lvl.description, x0 + pad, y,
            (x1 - x0) - 2.0 * pad, BODY_PX, WHITE);

        let dcx = (x0 + x1) * 0.5;
        let dy = 0.38;
        let di = self.difficulty_idx[self.selected as usize] as usize;
        let tier_label = lvl.difficulty_names.get(di)
            .cloned()
            .unwrap_or_else(|| "NORMAL".to_string());
        let tier_mult  = lvl.difficulty_tiers.get(di)
            .map(|t| t.wall_speed_mult())
            .unwrap_or(1.0);

        push_text_centered(out, "DIFFICULTY", dcx,
            dy - 0.09 - text_height(SMALL_PX) * 0.5, SMALL_PX, DIM);

        let lh = self.hover_eased(ID_DIFF_LEFT);
        let rh = self.hover_eased(ID_DIFF_RIGHT);
        let dimc: [f32; 3] = [0.25, 0.15, 0.25];
        let lc = if di > 0 { mix3(pal.accent, ACCENT_HI, lh) } else { dimc };
        let rc = if di + 1 < lvl.difficulty_tiers.len() {
            mix3(pal.accent, ACCENT_HI, rh) } else { dimc };
        let lx = dcx - 0.30;
        let rx = dcx + 0.30;
        push_tri(out, [lx - 0.04 - lh * 0.006, dy],
                      [lx + 0.028, dy - 0.040],
                      [lx + 0.028, dy + 0.040], lc);
        push_tri(out, [rx + 0.04 + rh * 0.006, dy],
                      [rx - 0.028, dy - 0.040],
                      [rx - 0.028, dy + 0.040], rc);

        push_text_centered(out, &tier_label, dcx,
            dy - text_height(H1_PX) * 0.5, H1_PX, WHITE);
        let mstr = format!("X{:.2}", tier_mult);
        push_text_centered(out, &mstr, dcx,
            dy + text_height(H1_PX) * 0.5 + 0.006, SMALL_PX, pal.accent);

        let n = lvl.difficulty_tiers.len();
        for i in 0..n {
            let dx = dcx - 0.10 + (i as f32 / (n - 1).max(1) as f32) * 0.20;
            let lit = i <= di;
            let color = if lit { pal.accent } else { DIM };
            let r = if lit { 0.012 } else { 0.009 };
            push_hex(out, dx, dy + 0.10, r, color);
        }

        self.draw_play_button(out, dcx, y1 - 0.08, pal);
    }

    fn draw_play_button(
        &self, out: &mut Vec<Vertex>,
        pcx: f32, pcy: f32, pal: &Palette,
    ) {
        let ph = self.hover_eased(ID_PLAY_LEVEL);
        let bg  = mix3(BG_PANEL, BG_PANEL_HI, ph);
        let ring = mix3(pal.accent, ACCENT_HI, ph);
        let hw = 0.22 + 0.006 * ph;
        let hh = 0.055 + 0.003 * ph;
        let thick = 0.005 + 0.002 * ph;

        push_quad(out, pcx - hw, pcy - hh, pcx + hw, pcy + hh, bg);
        push_outline(out, pcx - hw, pcy - hh, pcx + hw, pcy + hh, thick, ring);

        let chev_w = 0.040;
        let chev_h = 0.028;
        let gap    = 0.018;
        let text_w = text_width("PLAY", BUTTON_PX);
        let content_w = chev_w + gap + text_w;
        let content_left = pcx - content_w * 0.5;
        let chev_x = content_left;
        let text_x = chev_x + chev_w + gap;

        push_tri(out,
            [chev_x,          pcy - chev_h],
            [chev_x,          pcy + chev_h],
            [chev_x + chev_w, pcy],
            ring);
        push_text(out, "PLAY", text_x,
            pcy - text_height(BUTTON_PX) * 0.5, BUTTON_PX, ring);
    }

    fn draw_modifier_row(&self, out: &mut Vec<Vertex>) {
        let y0 = MOD_ROW_TOP;
        let y1 = MOD_ROW_BOT;
        let left  = self.view_left() + 0.04;
        let right = self.view_right() - 0.04;
        let count = 5;
        let total_w = right - left;
        let cell_w = total_w / count as f32 - 0.015;

        for i in 0..count {
            let x0 = left + i as f32 * (cell_w + 0.015);
            let x1 = x0 + cell_w;
            push_quad(out, x0, y0, x1, y1, BG_PANEL);
            push_outline(out, x0, y0, x1, y1, 0.003, DIM);
            let label = if i == 0 { "RANDOM LVL" } else { "MODIFIER" };
            let sub   = if i == 0 { "SHUFFLE" }    else { "PLACEHOLDER" };
            push_text(out, label, x0 + 0.012, y0 + 0.020, SMALL_PX, WHITE);
            push_text(out, sub,   x0 + 0.012, y0 + 0.020 + text_height(SMALL_PX) + 0.005,
                SMALL_PX, DIM);
        }
    }

    // ---------- settings drawing ----------

    fn draw_settings(&self, out: &mut Vec<Vertex>, config: &Config) {
        push_text_centered(out, "OPTIONS",        0.0, -0.92, TITLE_PX, ACCENT_HI);
        push_text_centered(out, "TUNE THE FEEL",  0.0, -0.84, H2_PX,    DIM);

        let (bcx, bcy, bhw, bhh) = self.back_button_rect();
        self.draw_small_button(out, bcx, bcy, bhw, bhh, "BACK", ID_BACK);
        let (rcx, rcy, rhw, rhh) = self.reset_button_rect();
        self.draw_small_button(out, rcx, rcy, rhw, rhh, "RESET", ID_RESET);

        // Tab bar.
        let tabs = [
            ("GRAPHICS", ID_TAB_GFX),
            ("AUDIO",    ID_TAB_AUD),
            ("GAMEPLAY", ID_TAB_GPL),
            ("ACCESS",   ID_TAB_ACC),
            ("CONTROLS", ID_TAB_CTL),
        ];
        for (i, (label, id)) in tabs.iter().enumerate() {
            let (cx, cy, hw, hh) = self.tab_rect(i, tabs.len());
            let active = i == self.settings_tab;
            let h = self.hover_eased(*id);
            let x0 = cx - hw; let x1 = cx + hw;
            let y0 = cy - hh; let y1 = cy + hh;
            let bg = if active {
                BG_PANEL_HI
            } else {
                mix3(BG_PANEL, BG_PANEL_HI, h)
            };
            push_quad(out, x0, y0, x1, y1, bg);
            if active {
                push_quad(out, x0, y1 - 0.006, x1, y1, ACCENT);
            } else {
                push_outline(out, x0, y0, x1, y1, 0.002,
                    mix3(DIM, ACCENT, h));
            }
            let w = text_width(label, BUTTON_PX);
            push_text(out, label, cx - w * 0.5,
                cy - text_height(BUTTON_PX) * 0.5, BUTTON_PX,
                if active { WHITE } else { DIM });
        }

        // Content per tab.
        match self.settings_tab {
            0 => self.draw_tab_graphics(out, config),
            1 => self.draw_tab_audio   (out, config),
            2 => self.draw_tab_gameplay(out, config),
            3 => self.draw_tab_access  (out, config),
            _ => self.draw_tab_controls(out),
        }
    }

    fn draw_tab_graphics(&self, out: &mut Vec<Vertex>, c: &Config) {
        let rows = 11;
        self.draw_cycle_row(out, 0, rows, "V-SYNC",
            c.vsync.label(), ID_GFX_VSYNC_L, ID_GFX_VSYNC_R);
        let fps_label = fps_label(c.fps_cap);
        self.draw_cycle_row(out, 1, rows, "FPS CAP",
            &fps_label, ID_GFX_FPS_L, ID_GFX_FPS_R);

        self.draw_slider_row(out, 2,  rows, "BLOOM",          c.bloom_intensity,    0.0, 1.5, ID_GFX_BLOOM);
        self.draw_slider_row(out, 3,  rows, "CHROMATIC",      c.chromatic_strength, 0.0, 1.0, ID_GFX_CHROMATIC);
        self.draw_slider_row(out, 4,  rows, "SCREEN SHAKE",   c.screen_shake,       0.0, 1.5, ID_GFX_SHAKE);
        self.draw_slider_row(out, 5,  rows, "VIGNETTE",       c.vignette,           0.0, 1.0, ID_GFX_VIGNETTE);
        self.draw_slider_row(out, 6,  rows, "BEAT FLASH",     c.beat_flash,         0.0, 1.5, ID_GFX_BEAT_FLASH);
        self.draw_slider_row(out, 7,  rows, "FAKE 3D DEPTH",  c.fake_3d_depth,      0.0, 1.0, ID_GFX_DEPTH);

        self.draw_cycle_row(out, 8, rows, "PARTICLES",
            c.particle_density.label(), ID_GFX_PART_L, ID_GFX_PART_R);
        self.draw_toggle_row(out, 9,  rows, "SCANLINES", c.scanlines,  ID_GFX_SCANLINES);
        self.draw_toggle_row(out, 10, rows, "SHOW FPS",  c.show_fps,   ID_GFX_SHOW_FPS);
    }

    fn draw_tab_audio(&self, out: &mut Vec<Vertex>, c: &Config) {
        let rows = 3;
        self.draw_slider_row(out, 0, rows, "MASTER VOLUME", c.master_volume, 0.0, 1.0, ID_AUD_MASTER);
        self.draw_slider_row(out, 1, rows, "MUSIC VOLUME",  c.music_volume,  0.0, 1.0, ID_AUD_MUSIC);
        self.draw_slider_row(out, 2, rows, "SFX VOLUME",    c.sfx_volume,    0.0, 1.0, ID_AUD_SFX);
    }

    fn draw_tab_gameplay(&self, out: &mut Vec<Vertex>, c: &Config) {
        let rows = 5;
        self.draw_slider_row(out, 0, rows, "PLAYER SPEED",   c.player_speed,  0.5, 2.0, ID_GPL_PSPEED);
        self.draw_slider_row(out, 1, rows, "CAMERA WOBBLE",  c.camera_wobble, 0.0, 1.0, ID_GPL_WOBBLE);
        self.draw_cycle_row (out, 2, rows, "ABILITY",
            c.ability.label(), ID_GPL_ABILITY_L, ID_GPL_ABILITY_R);
        self.draw_toggle_row(out, 3, rows, "CLOSE-CALL FX", c.close_call_fx, ID_GPL_CLOSE_FX);
        self.draw_cycle_row (out, 4, rows, "PREDICT HIGHLIGHT",
            c.predictive_highlight.label(), ID_GPL_HL_L, ID_GPL_HL_R);

        // Ability description under the widgets.
        let (_, cy) = self.row_center(2, rows);
        let vl = self.view_left() + 0.08;
        push_text(out, c.ability.description(),
            vl + 0.52, cy + 0.035, SMALL_PX, DIM);
    }

    fn draw_tab_access(&self, out: &mut Vec<Vertex>, c: &Config) {
        let rows = 4;
        self.draw_toggle_row(out, 0, rows, "HIGH CONTRAST",  c.high_contrast,  ID_ACC_CONTRAST);
        self.draw_toggle_row(out, 1, rows, "REDUCE MOTION",  c.reduce_motion,  ID_ACC_REDUCE);
        self.draw_toggle_row(out, 2, rows, "SHOW HITBOXES",  c.show_hitboxes,  ID_ACC_HITBOX);
        self.draw_cycle_row (out, 3, rows, "COLOR BLIND MODE",
            c.colorblind.label(), ID_ACC_CB_L, ID_ACC_CB_R);
    }

    fn draw_tab_controls(&self, out: &mut Vec<Vertex>) {
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
        let vl = self.view_left() + 0.16;
        let top = SETTINGS_CONTENT_TOP + 0.06;
        for (i, line) in lines.iter().enumerate() {
            push_text(out, line, vl, top + i as f32 * 0.065, BODY_PX, WHITE);
        }
    }

    /// Draw one slider row on the settings screen.
    fn draw_slider_row(
        &self, out: &mut Vec<Vertex>,
        row: usize, rows: usize, label: &str,
        value: f32, min: f32, max: f32, id: WidgetId,
    ) {
        let (_, y) = self.row_center(row, rows);
        let vl = self.view_left() + 0.08;

        push_text(out, label, vl + 0.04,
            y - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        let (x0, x1) = self.slider_track_range();
        push_quad(out, x0 - 0.006, y - 0.014, x1 + 0.006, y + 0.014, BG_DEEP);
        push_quad(out, x0, y - 0.010, x1, y + 0.010, BG_PANEL);

        let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let fx = x0 + (x1 - x0) * t;
        push_quad(out, x0, y - 0.010, fx, y + 0.010, ACCENT);

        let h = self.hover_eased(id);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        let r = 0.022 + 0.005 * h;
        push_hex(out, fx, y, r, BG_DEEP);
        push_hex_ring(out, fx, y, r, r - 0.006, ring);
        push_hex(out, fx, y, r * 0.40, ring);

        // Value readout on the right.
        let disp = if (max - min - 1.0).abs() < 1e-3 && min == 0.0 {
            format!("{}%", (value * 100.0).round() as i32)
        } else {
            format!("{:.2}", value)
        };
        push_text_right(out, &disp, x1 + 0.16,
            y - text_height(BODY_PX) * 0.5, BODY_PX, ACCENT_HI);
    }

    /// Draw one cycle row (left arrow / value / right arrow).
    fn draw_cycle_row(
        &self, out: &mut Vec<Vertex>,
        row: usize, rows: usize, label: &str, value: &str,
        id_l: WidgetId, id_r: WidgetId,
    ) {
        let (_, y) = self.row_center(row, rows);
        let vl = self.view_left() + 0.08;

        push_text(out, label, vl + 0.04,
            y - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        let (lx, rx) = self.arrow_positions();
        let lh = self.hover_eased(id_l);
        let rh = self.hover_eased(id_r);
        let lc = mix3(ACCENT, ACCENT_HI, lh);
        let rc = mix3(ACCENT, ACCENT_HI, rh);

        push_tri(out,
            [lx - 0.035 - 0.005 * lh, y],
            [lx + 0.020, y - 0.024],
            [lx + 0.020, y + 0.024], lc);
        push_tri(out,
            [rx + 0.035 + 0.005 * rh, y],
            [rx - 0.020, y - 0.024],
            [rx - 0.020, y + 0.024], rc);

        let mid = (lx + rx) * 0.5;
        let w = text_width(value, BODY_PX);
        push_text(out, value, mid - w * 0.5,
            y - text_height(BODY_PX) * 0.5, BODY_PX, ACCENT_HI);
    }

    /// Draw one toggle row (label + ON/OFF button).
    fn draw_toggle_row(
        &self, out: &mut Vec<Vertex>,
        row: usize, rows: usize, label: &str,
        value: bool, id: WidgetId,
    ) {
        let (_, y) = self.row_center(row, rows);
        let vl = self.view_left() + 0.08;

        push_text(out, label, vl + 0.04,
            y - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);

        let (cx, cy, hw, hh) = self.toggle_rect(row, rows);
        let h = self.hover_eased(id);
        let bg = if value {
            mix3(ACCENT, ACCENT_HI, h)
        } else {
            mix3(BG_PANEL, BG_PANEL_HI, h)
        };
        let ring = mix3(ACCENT, ACCENT_HI, h);
        let x0 = cx - hw; let x1 = cx + hw;
        let y0 = cy - hh; let y1 = cy + hh;
        push_quad(out, x0, y0, x1, y1, bg);
        push_outline(out, x0, y0, x1, y1, 0.003, ring);
        let text = if value { "ON" } else { "OFF" };
        let w = text_width(text, BODY_PX);
        push_text(out, text, cx - w * 0.5,
            cy - text_height(BODY_PX) * 0.5, BODY_PX,
            if value { BG_DEEP } else { WHITE });
    }

    fn draw_small_button(
        &self, out: &mut Vec<Vertex>,
        cx: f32, cy: f32, hw: f32, hh: f32,
        label: &str, id: WidgetId,
    ) {
        let h = self.hover_eased(id);
        let bg   = mix3(BG_PANEL, BG_PANEL_HI, h);
        let ring = mix3(ACCENT, ACCENT_HI, h);
        let x0 = cx - hw; let x1 = cx + hw;
        let y0 = cy - hh; let y1 = cy + hh;
        push_quad(out, x0, y0, x1, y1, bg);
        push_outline(out, x0, y0, x1, y1, 0.004 + 0.002 * h, ring);
        let w = text_width(label, BODY_PX);
        push_text(out, label, cx - w * 0.5, cy - text_height(BODY_PX) * 0.5, BODY_PX, WHITE);
    }

    fn draw_music_panel(&self, out: &mut Vec<Vertex>, snap: &AudioSnapshot) {
        if !snap.has_music { return; }

        let left  = self.view_left();
        let right = self.view_right();
        let top   = self.view_top();
        let bot   = self.panel_bot();
        let cy    = top + PANEL_EXTENDED_HEIGHT - 0.06;

        push_quad(out, left, top, right, bot, BG_PANEL);
        push_quad(out, left, bot - 0.004, right, bot, ACCENT);

        if self.panel_t < 0.05 { return; }

        let overlay_a = self.panel_t * 0.55;
        push_quad_alpha(out, left, bot, right, self.view_bottom(),
            [0.0, 0.0, 0.0], overlay_a);

        let alpha = smoothstep(PANEL_CONTENT_LO, PANEL_CONTENT_HI, self.panel_t);
        if alpha < 0.02 { return; }

        let (track_name, artist) = self.current_track_display_names();
        let label_x = left + 0.04;
        let pad_top = 0.030;
        let label_y  = top + pad_top;
        let title_y  = label_y + text_height(SMALL_PX) + 0.008;
        let artist_y = title_y + text_height(H2_PX) + 0.008;

        push_text_alpha(out, "NOW PLAYING", label_x, label_y, SMALL_PX, DIM, alpha);
        push_text_alpha(out, &track_name,   label_x, title_y, H2_PX,    WHITE, alpha);
        push_text_alpha(out, &artist,       label_x, artist_y, SMALL_PX, DIM, alpha);

        let (bx0, bx1) = self.music_panel_bar_range();
        let ctrl_x0 = bx0 - 0.26;
        let pause_cx = ctrl_x0 + 0.050;
        let stop_cx  = ctrl_x0 + 0.155;
        let btn_hw   = 0.045;
        let btn_hh   = 0.030;

        let ph = self.hover_eased(ID_MUSIC_PAUSE);
        let pring = mix3(ACCENT, ACCENT_HI, ph);
        push_quad_alpha(out,
            pause_cx - btn_hw, cy - btn_hh,
            pause_cx + btn_hw, cy + btn_hh,
            BG_DEEP, alpha);
        push_outline_alpha(out,
            pause_cx - btn_hw, cy - btn_hh,
            pause_cx + btn_hw, cy + btn_hh,
            0.003, pring, alpha);
        if snap.paused {
            push_tri_alpha(out,
                [pause_cx - 0.012, cy - 0.018],
                [pause_cx - 0.012, cy + 0.018],
                [pause_cx + 0.020, cy],
                pring, alpha);
        } else {
            push_quad_alpha(out, pause_cx - 0.020, cy - 0.018, pause_cx - 0.006, cy + 0.018, pring, alpha);
            push_quad_alpha(out, pause_cx + 0.006, cy - 0.018, pause_cx + 0.020, cy + 0.018, pring, alpha);
        }

        let sh = self.hover_eased(ID_MUSIC_STOP);
        let sring = mix3(ACCENT, ACCENT_HI, sh);
        push_quad_alpha(out,
            stop_cx - btn_hw, cy - btn_hh,
            stop_cx + btn_hw, cy + btn_hh,
            BG_DEEP, alpha);
        push_outline_alpha(out,
            stop_cx - btn_hw, cy - btn_hh,
            stop_cx + btn_hw, cy + btn_hh,
            0.003, sring, alpha);
        push_quad_alpha(out,
            stop_cx - 0.018, cy - 0.018,
            stop_cx + 0.018, cy + 0.018,
            sring, alpha);

        push_quad_alpha(out, bx0 - 0.004, cy - 0.014, bx1 + 0.004, cy + 0.014, BG_DEEP, alpha);
        push_quad_alpha(out, bx0, cy - 0.010, bx1, cy + 0.010, BG_PANEL_HI, alpha);
        let prog = if snap.duration > 0.0 {
            (snap.position / snap.duration).clamp(0.0, 1.0)
        } else { 0.0 };
        let fx = bx0 + (bx1 - bx0) * prog;
        push_quad_alpha(out, bx0, cy - 0.010, fx, cy + 0.010, ACCENT, alpha);

        let bh = self.hover_eased(ID_MUSIC_BAR);
        let scrub = mix3(ACCENT, ACCENT_HI, bh);
        let sr = 0.018 + 0.004 * bh;
        push_hex_alpha(out, fx, cy, sr, BG_DEEP, alpha);
        push_hex_ring_alpha(out, fx, cy, sr, sr - 0.006, scrub, alpha);

        let remaining = (snap.duration - snap.position).max(0.0);
        let t_str = format!("{} / {}   (-{})",
            fmt_time(snap.position), fmt_time(snap.duration), fmt_time(remaining));
        push_text_right_alpha(out, &t_str,
            right - 0.04, cy - text_height(SMALL_PX) * 0.5,
            SMALL_PX, WHITE, alpha);
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
}

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

fn rail_right_at(y: f32) -> f32 {
    let t = ((y - RAIL_TOP) / (MOD_SEPARATOR_Y - RAIL_TOP)).clamp(0.0, 1.0);
    RAIL_RIGHT_TOP + (RAIL_RIGHT_BOTTOM - RAIL_RIGHT_TOP) * t
}

fn hit_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32) -> bool {
    (px - cx).abs() < hw && (py - cy).abs() < hh
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t,
     a[1] + (b[1] - a[1]) * t,
     a[2] + (b[2] - a[2]) * t]
}

// -------- enum cycling helpers --------

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

// -------- low level geometry --------

fn push_quad(out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3]) {
    out.push(Vertex::opaque([x0, y0], c));
    out.push(Vertex::opaque([x1, y0], c));
    out.push(Vertex::opaque([x1, y1], c));
    out.push(Vertex::opaque([x0, y0], c));
    out.push(Vertex::opaque([x1, y1], c));
    out.push(Vertex::opaque([x0, y1], c));
}

fn push_quad_alpha(
    out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32,
    c: [f32; 3], a: f32,
) {
    out.push(Vertex::rgba([x0, y0], c, a));
    out.push(Vertex::rgba([x1, y0], c, a));
    out.push(Vertex::rgba([x1, y1], c, a));
    out.push(Vertex::rgba([x0, y0], c, a));
    out.push(Vertex::rgba([x1, y1], c, a));
    out.push(Vertex::rgba([x0, y1], c, a));
}

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
    push_trapezoid(out, x_top - thick, x_bot - thick, x_top, x_bot, y_top, y_bot, c);
}

fn push_outline(
    out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, t: f32, c: [f32; 3],
) {
    push_quad(out, x0 - t, y0 - t, x1 + t, y0,     c);
    push_quad(out, x0 - t, y1,     x1 + t, y1 + t, c);
    push_quad(out, x0 - t, y0,     x0,     y1,     c);
    push_quad(out, x1,     y0,     x1 + t, y1,     c);
}

fn push_outline_alpha(
    out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32,
    t: f32, c: [f32; 3], a: f32,
) {
    push_quad_alpha(out, x0 - t, y0 - t, x1 + t, y0,     c, a);
    push_quad_alpha(out, x0 - t, y1,     x1 + t, y1 + t, c, a);
    push_quad_alpha(out, x0 - t, y0,     x0,     y1,     c, a);
    push_quad_alpha(out, x1,     y0,     x1 + t, y1,     c, a);
}

fn push_tri(out: &mut Vec<Vertex>, a: [f32; 2], b: [f32; 2], c: [f32; 2], col: [f32; 3]) {
    out.push(Vertex::opaque(a, col));
    out.push(Vertex::opaque(b, col));
    out.push(Vertex::opaque(c, col));
}

fn push_tri_alpha(
    out: &mut Vec<Vertex>, a: [f32; 2], b: [f32; 2], c: [f32; 2],
    col: [f32; 3], alpha: f32,
) {
    out.push(Vertex::rgba(a, col, alpha));
    out.push(Vertex::rgba(b, col, alpha));
    out.push(Vertex::rgba(c, col, alpha));
}

fn push_hex(out: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, c: [f32; 3]) {
    push_hex_rot(out, cx, cy, r, c, 0.0);
}

fn push_hex_alpha(out: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, c: [f32; 3], a: f32) {
    let mut v = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let ang = i as f32 * TAU / 6.0;
        v[i] = [cx + ang.cos() * r, cy + ang.sin() * r];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::rgba([cx, cy], c, a));
        out.push(Vertex::rgba(v[i],     c, a));
        out.push(Vertex::rgba(v[j],     c, a));
    }
}

fn push_hex_rot(out: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, col: [f32; 3], ang: f32) {
    let mut v = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let a = ang + i as f32 * TAU / 6.0;
        v[i] = [cx + a.cos() * r, cy + a.sin() * r];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::opaque([cx, cy], col));
        out.push(Vertex::opaque(v[i],     col));
        out.push(Vertex::opaque(v[j],     col));
    }
}

fn push_hex_ring(out: &mut Vec<Vertex>, cx: f32, cy: f32, ro: f32, ri: f32, c: [f32; 3]) {
    push_hex_ring_rot(out, cx, cy, ro, ri, c, 0.0);
}

fn push_hex_ring_alpha(
    out: &mut Vec<Vertex>, cx: f32, cy: f32, ro: f32, ri: f32, c: [f32; 3], a: f32,
) {
    let mut o = [[0.0f32; 2]; 6];
    let mut i2 = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let ang = i as f32 * TAU / 6.0;
        o [i] = [cx + ang.cos() * ro, cy + ang.sin() * ro];
        i2[i] = [cx + ang.cos() * ri, cy + ang.sin() * ri];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::rgba(o [i], c, a));
        out.push(Vertex::rgba(o [j], c, a));
        out.push(Vertex::rgba(i2[j], c, a));
        out.push(Vertex::rgba(o [i], c, a));
        out.push(Vertex::rgba(i2[j], c, a));
        out.push(Vertex::rgba(i2[i], c, a));
    }
}

fn push_hex_ring_rot(
    out: &mut Vec<Vertex>, cx: f32, cy: f32,
    ro: f32, ri: f32, col: [f32; 3], ang: f32,
) {
    let mut o = [[0.0f32; 2]; 6];
    let mut i2 = [[0.0f32; 2]; 6];
    for i in 0..6 {
        let a = ang + i as f32 * TAU / 6.0;
        o [i] = [cx + a.cos() * ro, cy + a.sin() * ro];
        i2[i] = [cx + a.cos() * ri, cy + a.sin() * ri];
    }
    for i in 0..6 {
        let j = (i + 1) % 6;
        out.push(Vertex::opaque(o [i], col));
        out.push(Vertex::opaque(o [j], col));
        out.push(Vertex::opaque(i2[j], col));
        out.push(Vertex::opaque(o [i], col));
        out.push(Vertex::opaque(i2[j], col));
        out.push(Vertex::opaque(i2[i], col));
    }
}

fn push_text_right_alpha(
    out: &mut Vec<Vertex>, text: &str,
    rx: f32, y: f32, pixel_size: f32, color: [f32; 3], alpha: f32,
) {
    let w = text_width(text, pixel_size);
    push_text_alpha(out, text, rx - w, y, pixel_size, color, alpha);
}