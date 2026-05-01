//! Persistent user configuration.
//!
//! The file format is intentionally trivial: one `key = value` pair
//! per line, `#` starts a comment, blank lines are ignored. There
//! are no nested blocks, no arrays, no escape sequences. Everything
//! the game wants to remember fits into that flat key space and the
//! format is obvious enough for users to edit by hand.
//!
//! The file is stored next to the executable as `rustogon.cfg`.
//! Loading is best-effort: any unknown key is skipped with a log
//! message and any invalid value reverts to default, so an old
//! config from a previous build can never break the current one.
//!
//! Saving is also best-effort: if the file cannot be written (read
//! only directory, permission error) the error is logged and the
//! game continues. No user data is ever held in RAM only by this
//! path; every setting has a sensible default so a missing config
//! file just means "first run".

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const CONFIG_FILE_NAME: &str = "rustogon.cfg";

/// Vertical sync policy applied when creating / recreating the
/// swapchain. Maps directly to Vulkan present modes inside the
/// renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VsyncMode {
    /// No sync, lowest latency, may tear. Vulkan IMMEDIATE.
    Off,
    /// Classic hard vsync, no tearing, fixed to refresh rate.
    /// Vulkan FIFO.
    On,
    /// Triple-buffered "fast vsync": newest frame always wins, no
    /// tearing, but the driver may drop in-flight frames. Vulkan
    /// MAILBOX, falls back to FIFO if unavailable.
    Fast,
}

impl VsyncMode {
    pub fn label(self) -> &'static str {
        match self {
            VsyncMode::Off  => "OFF",
            VsyncMode::On   => "ON",
            VsyncMode::Fast => "FAST",
        }
    }
    pub fn next(self) -> Self {
        match self {
            VsyncMode::Off  => VsyncMode::On,
            VsyncMode::On   => VsyncMode::Fast,
            VsyncMode::Fast => VsyncMode::Off,
        }
    }
    pub fn from_str_key(s: &str) -> Self {
        match s {
            "off"  => VsyncMode::Off,
            "on"   => VsyncMode::On,
            "fast" => VsyncMode::Fast,
            _      => VsyncMode::On,
        }
    }
    pub fn to_str_key(self) -> &'static str {
        match self {
            VsyncMode::Off  => "off",
            VsyncMode::On   => "on",
            VsyncMode::Fast => "fast",
        }
    }
}

/// Density bucket for the particle system. Translated at runtime
/// into a per-emitter scaling factor so every effect shrinks
/// together instead of having to guard each emission point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleDensity {
    Off,
    Low,
    Medium,
    High,
}

impl ParticleDensity {
    pub fn label(self) -> &'static str {
        match self {
            ParticleDensity::Off    => "OFF",
            ParticleDensity::Low    => "LOW",
            ParticleDensity::Medium => "MEDIUM",
            ParticleDensity::High   => "HIGH",
        }
    }
    pub fn factor(self) -> f32 {
        match self {
            ParticleDensity::Off    => 0.0,
            ParticleDensity::Low    => 0.35,
            ParticleDensity::Medium => 0.70,
            ParticleDensity::High   => 1.20,
        }
    }
    pub fn next(self) -> Self {
        match self {
            ParticleDensity::Off    => ParticleDensity::Low,
            ParticleDensity::Low    => ParticleDensity::Medium,
            ParticleDensity::Medium => ParticleDensity::High,
            ParticleDensity::High   => ParticleDensity::Off,
        }
    }
    pub fn from_str_key(s: &str) -> Self {
        match s {
            "off"    => ParticleDensity::Off,
            "low"    => ParticleDensity::Low,
            "medium" => ParticleDensity::Medium,
            "high"   => ParticleDensity::High,
            _        => ParticleDensity::Medium,
        }
    }
    pub fn to_str_key(self) -> &'static str {
        match self {
            ParticleDensity::Off    => "off",
            ParticleDensity::Low    => "low",
            ParticleDensity::Medium => "medium",
            ParticleDensity::High   => "high",
        }
    }
}

/// Active player ability. Starts at `None` so the first-run
/// experience matches classic Super Hexagon; the player chooses
/// to opt in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ability {
    None,
    Dash,
    Shield,
    SlowMo,
}

impl Ability {
    pub fn label(self) -> &'static str {
        match self {
            Ability::None   => "NONE",
            Ability::Dash   => "DASH",
            Ability::Shield => "SHIELD",
            Ability::SlowMo => "SLOW-MO",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Ability::None   => "CLASSIC. NO GADGETS.",
            Ability::Dash   => "SHIFT: LEAP ONE SLOT ACROSS.",
            Ability::Shield => "ABSORB ONE HIT. RECHARGES.",
            Ability::SlowMo => "HOLD SHIFT: TIME DILATION.",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Ability::None   => Ability::Dash,
            Ability::Dash   => Ability::Shield,
            Ability::Shield => Ability::SlowMo,
            Ability::SlowMo => Ability::None,
        }
    }
    pub fn from_str_key(s: &str) -> Self {
        match s {
            "none"   => Ability::None,
            "dash"   => Ability::Dash,
            "shield" => Ability::Shield,
            "slowmo" => Ability::SlowMo,
            _        => Ability::None,
        }
    }
    pub fn to_str_key(self) -> &'static str {
        match self {
            Ability::None   => "none",
            Ability::Dash   => "dash",
            Ability::Shield => "shield",
            Ability::SlowMo => "slowmo",
        }
    }
}

/// Accessibility: recolor the palette for color blind players.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorblindMode {
    Off,
    Protanopia,
    Deuteranopia,
    Tritanopia,
}

impl ColorblindMode {
    pub fn label(self) -> &'static str {
        match self {
            ColorblindMode::Off          => "OFF",
            ColorblindMode::Protanopia   => "PROTANOPIA",
            ColorblindMode::Deuteranopia => "DEUTERANOPIA",
            ColorblindMode::Tritanopia   => "TRITANOPIA",
        }
    }
    pub fn next(self) -> Self {
        match self {
            ColorblindMode::Off          => ColorblindMode::Protanopia,
            ColorblindMode::Protanopia   => ColorblindMode::Deuteranopia,
            ColorblindMode::Deuteranopia => ColorblindMode::Tritanopia,
            ColorblindMode::Tritanopia   => ColorblindMode::Off,
        }
    }
    pub fn from_str_key(s: &str) -> Self {
        match s {
            "off"          => ColorblindMode::Off,
            "protanopia"   => ColorblindMode::Protanopia,
            "deuteranopia" => ColorblindMode::Deuteranopia,
            "tritanopia"   => ColorblindMode::Tritanopia,
            _              => ColorblindMode::Off,
        }
    }
    pub fn to_str_key(self) -> &'static str {
        match self {
            ColorblindMode::Off          => "off",
            ColorblindMode::Protanopia   => "protanopia",
            ColorblindMode::Deuteranopia => "deuteranopia",
            ColorblindMode::Tritanopia   => "tritanopia",
        }
    }
}

/// Optional assist: highlight the wall the player would collide
/// with if they stopped turning. Three modes for "never", "always",
/// and "auto" (hidden at Expert+ tiers where it would trivialize
/// the game).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightMode {
    Off,
    On,
    Auto,
}

impl HighlightMode {
    pub fn label(self) -> &'static str {
        match self {
            HighlightMode::Off  => "OFF",
            HighlightMode::On   => "ON",
            HighlightMode::Auto => "AUTO",
        }
    }
    pub fn next(self) -> Self {
        match self {
            HighlightMode::Off  => HighlightMode::On,
            HighlightMode::On   => HighlightMode::Auto,
            HighlightMode::Auto => HighlightMode::Off,
        }
    }
    pub fn from_str_key(s: &str) -> Self {
        match s {
            "off"  => HighlightMode::Off,
            "on"   => HighlightMode::On,
            "auto" => HighlightMode::Auto,
            _      => HighlightMode::Off,
        }
    }
    pub fn to_str_key(self) -> &'static str {
        match self {
            HighlightMode::Off  => "off",
            HighlightMode::On   => "on",
            HighlightMode::Auto => "auto",
        }
    }
}

/// Policy for the user shader sandbox. Starts at `Off` so a
/// fresh install never executes author supplied GPU code until
/// the user opts in. `Audit` compiles and validates but does not
/// render, which is useful for shader authors who want to test
/// their programs without putting the driver at risk. `On` lets
/// validated shaders become the active post pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomShaders {
    Off,
    Audit,
    On,
}

impl CustomShaders {
    pub fn label(self) -> &'static str {
        match self {
            CustomShaders::Off   => "OFF",
            CustomShaders::Audit => "AUDIT",
            CustomShaders::On    => "ON",
        }
    }
    pub fn next(self) -> Self {
        match self {
            CustomShaders::Off   => CustomShaders::Audit,
            CustomShaders::Audit => CustomShaders::On,
            CustomShaders::On    => CustomShaders::Off,
        }
    }
    pub fn from_str_key(s: &str) -> Self {
        match s {
            "off"   => CustomShaders::Off,
            "audit" => CustomShaders::Audit,
            "on"    => CustomShaders::On,
            _       => CustomShaders::Off,
        }
    }
    pub fn to_str_key(self) -> &'static str {
        match self {
            CustomShaders::Off   => "off",
            CustomShaders::Audit => "audit",
            CustomShaders::On    => "on",
        }
    }
}

/// Everything the user can customize, flat for easy persistence.
///
/// Every field has both a reasonable default (see [`Config::default`])
/// and a parse/save entry in [`Config::load_from_string`] /
/// [`Config::write_to_string`]. A new field needs three changes:
///
/// 1. Add the field with a default.
/// 2. Wire a case in the parser for its key.
/// 3. Wire a write line at the matching spot in the serializer.
#[derive(Clone, Debug)]
pub struct Config {
    // ---- graphics ----
    pub vsync:               VsyncMode,
    pub fps_cap:             u32,
    pub bloom_intensity:     f32,
    pub chromatic_strength:  f32,
    pub screen_shake:        f32,
    pub motion_blur:         bool,
    pub vignette:            f32,
    pub scanlines:           bool,
    pub film_grain:          bool,
    pub beat_flash:          f32,
    pub particle_density:    ParticleDensity,
    pub background_parallax: bool,
    pub fake_3d_depth:       f32,
    pub show_fps:            bool,
    pub ui_scale:            f32,

    // ---- audio ----
    pub master_volume: f32,
    pub music_volume:  f32,
    pub sfx_volume:    f32,

    // ---- gameplay ----
    pub player_speed:         f32,
    pub camera_wobble:        f32,
    pub ability:              Ability,
    pub close_call_fx:        bool,
    pub predictive_highlight: HighlightMode,

    // ---- accessibility ----
    pub high_contrast:  bool,
    pub reduce_motion:  bool,
    pub show_hitboxes:  bool,
    pub colorblind:     ColorblindMode,

    // ---- power user ----
    pub custom_shaders: CustomShaders,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vsync:               VsyncMode::On,
            fps_cap:             0,     // 0 = unlimited
            bloom_intensity:     0.55,
            chromatic_strength:  0.30,
            screen_shake:        0.70,
            motion_blur:         false,
            vignette:            0.55,
            scanlines:           false,
            film_grain:          false,
            beat_flash:          0.70,
            particle_density:    ParticleDensity::Medium,
            background_parallax: true,
            fake_3d_depth:       0.45,
            show_fps:            false,
            ui_scale:            1.00,

            master_volume: 0.70,
            music_volume:  1.00,
            sfx_volume:    1.00,

            player_speed:         1.00,
            camera_wobble:        1.00,
            ability:              Ability::None,
            close_call_fx:        true,
            predictive_highlight: HighlightMode::Off,

            high_contrast:  false,
            reduce_motion:  false,
            show_hitboxes:  false,
            colorblind:     ColorblindMode::Off,

            custom_shaders: CustomShaders::Off,
        }
    }
}

impl Config {
    /// Resolve the absolute path of the config file. We keep it
    /// next to the executable so the game is fully portable.
    fn path() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join(CONFIG_FILE_NAME);
            }
        }
        PathBuf::from(CONFIG_FILE_NAME)
    }

    /// Load from disk, falling back to defaults on any error.
    pub fn load() -> Self {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(text) => {
                let cfg = Self::load_from_string(&text);
                eprintln!("[config] loaded from {}", path.display());
                cfg
            }
            Err(_) => {
                eprintln!(
                    "[config] no config at {}, using defaults",
                    path.display()
                );
                Config::default()
            }
        }
    }

    /// Flush the current settings to disk. All errors are swallowed
    /// with a log: a read-only working directory is annoying but
    /// not a reason to crash.
    pub fn save(&self) {
        let path = Self::path();
        let text = self.write_to_string();
        if let Err(e) = fs::write(&path, text) {
            eprintln!("[config] save failed at {}: {}", path.display(), e);
        }
    }

    /// Parse a config from its textual form.
    pub fn load_from_string(text: &str) -> Self {
        let mut cfg = Config::default();
        let mut map: BTreeMap<String, String> = BTreeMap::new();

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() { continue; }
            if line.starts_with('#') || line.starts_with("//") { continue; }
            let Some(eq) = line.find('=') else { continue; };
            let key = line[..eq].trim().to_string();
            let val = line[eq + 1..].trim().to_string();
            map.insert(key, val);
        }

        for (k, v) in map {
            match k.as_str() {
                "vsync"               => cfg.vsync = VsyncMode::from_str_key(&v),
                "fps_cap"             => cfg.fps_cap = parse_u32(&v, 0),
                "bloom_intensity"     => cfg.bloom_intensity = parse_f32(&v, 0.55),
                "chromatic_strength"  => cfg.chromatic_strength = parse_f32(&v, 0.30),
                "screen_shake"        => cfg.screen_shake = parse_f32(&v, 0.70),
                "motion_blur"         => cfg.motion_blur = parse_bool(&v, false),
                "vignette"            => cfg.vignette = parse_f32(&v, 0.55),
                "scanlines"           => cfg.scanlines = parse_bool(&v, false),
                "film_grain"          => cfg.film_grain = parse_bool(&v, false),
                "beat_flash"          => cfg.beat_flash = parse_f32(&v, 0.70),
                "particle_density"    => cfg.particle_density = ParticleDensity::from_str_key(&v),
                "background_parallax" => cfg.background_parallax = parse_bool(&v, true),
                "fake_3d_depth"       => cfg.fake_3d_depth = parse_f32(&v, 0.45),
                "show_fps"            => cfg.show_fps = parse_bool(&v, false),
                "ui_scale"            => cfg.ui_scale = parse_f32(&v, 1.0),

                "master_volume" => cfg.master_volume = parse_f32(&v, 0.70),
                "music_volume"  => cfg.music_volume  = parse_f32(&v, 1.00),
                "sfx_volume"    => cfg.sfx_volume    = parse_f32(&v, 1.00),

                "player_speed"         => cfg.player_speed = parse_f32(&v, 1.0),
                "camera_wobble"        => cfg.camera_wobble = parse_f32(&v, 1.0),
                "ability"              => cfg.ability = Ability::from_str_key(&v),
                "close_call_fx"        => cfg.close_call_fx = parse_bool(&v, true),
                "predictive_highlight" => cfg.predictive_highlight = HighlightMode::from_str_key(&v),

                "high_contrast" => cfg.high_contrast = parse_bool(&v, false),
                "reduce_motion" => cfg.reduce_motion = parse_bool(&v, false),
                "show_hitboxes" => cfg.show_hitboxes = parse_bool(&v, false),
                "colorblind"    => cfg.colorblind = ColorblindMode::from_str_key(&v),

                "custom_shaders" => cfg.custom_shaders = CustomShaders::from_str_key(&v),

                other => eprintln!("[config] unknown key '{}' ignored", other),
            }
        }

        cfg.clamp();
        cfg
    }

    /// Serialize to a human readable text form, grouped by section.
    pub fn write_to_string(&self) -> String {
        let mut s = String::new();
        s.push_str("# Super Rustogon configuration file.\n");
        s.push_str("# Delete this file to reset to defaults.\n\n");

        s.push_str("# ---- graphics ----\n");
        kv(&mut s, "vsync",               self.vsync.to_str_key());
        kv(&mut s, "fps_cap",             &self.fps_cap.to_string());
        kv(&mut s, "bloom_intensity",     &fmt_f(self.bloom_intensity));
        kv(&mut s, "chromatic_strength",  &fmt_f(self.chromatic_strength));
        kv(&mut s, "screen_shake",        &fmt_f(self.screen_shake));
        kv(&mut s, "motion_blur",         if self.motion_blur { "true" } else { "false" });
        kv(&mut s, "vignette",            &fmt_f(self.vignette));
        kv(&mut s, "scanlines",           if self.scanlines { "true" } else { "false" });
        kv(&mut s, "film_grain",          if self.film_grain { "true" } else { "false" });
        kv(&mut s, "beat_flash",          &fmt_f(self.beat_flash));
        kv(&mut s, "particle_density",    self.particle_density.to_str_key());
        kv(&mut s, "background_parallax", if self.background_parallax { "true" } else { "false" });
        kv(&mut s, "fake_3d_depth",       &fmt_f(self.fake_3d_depth));
        kv(&mut s, "show_fps",            if self.show_fps { "true" } else { "false" });
        kv(&mut s, "ui_scale",            &fmt_f(self.ui_scale));
        s.push('\n');

        s.push_str("# ---- audio ----\n");
        kv(&mut s, "master_volume", &fmt_f(self.master_volume));
        kv(&mut s, "music_volume",  &fmt_f(self.music_volume));
        kv(&mut s, "sfx_volume",    &fmt_f(self.sfx_volume));
        s.push('\n');

        s.push_str("# ---- gameplay ----\n");
        kv(&mut s, "player_speed",         &fmt_f(self.player_speed));
        kv(&mut s, "camera_wobble",        &fmt_f(self.camera_wobble));
        kv(&mut s, "ability",              self.ability.to_str_key());
        kv(&mut s, "close_call_fx",        if self.close_call_fx { "true" } else { "false" });
        kv(&mut s, "predictive_highlight", self.predictive_highlight.to_str_key());
        s.push('\n');

        s.push_str("# ---- accessibility ----\n");
        kv(&mut s, "high_contrast", if self.high_contrast { "true" } else { "false" });
        kv(&mut s, "reduce_motion", if self.reduce_motion { "true" } else { "false" });
        kv(&mut s, "show_hitboxes", if self.show_hitboxes { "true" } else { "false" });
        kv(&mut s, "colorblind",    self.colorblind.to_str_key());
        s.push('\n');

        s.push_str("# ---- power user ----\n");
        kv(&mut s, "custom_shaders", self.custom_shaders.to_str_key());

        s
    }

    /// Clamp all numeric fields into the range the runtime can
    /// safely consume. Exposed so the menu can call it after each
    /// slider interaction without duplicating limits.
    pub fn clamp(&mut self) {
        self.bloom_intensity    = self.bloom_intensity.clamp(0.0, 1.5);
        self.chromatic_strength = self.chromatic_strength.clamp(0.0, 1.0);
        self.screen_shake       = self.screen_shake.clamp(0.0, 1.5);
        self.vignette           = self.vignette.clamp(0.0, 1.0);
        self.beat_flash         = self.beat_flash.clamp(0.0, 1.5);
        self.fake_3d_depth      = self.fake_3d_depth.clamp(0.0, 1.0);
        self.ui_scale           = self.ui_scale.clamp(0.75, 1.5);

        self.master_volume = self.master_volume.clamp(0.0, 1.0);
        self.music_volume  = self.music_volume.clamp(0.0, 1.0);
        self.sfx_volume    = self.sfx_volume.clamp(0.0, 1.0);

        self.player_speed  = self.player_speed.clamp(0.5, 2.0);
        self.camera_wobble = self.camera_wobble.clamp(0.0, 1.0);

        self.fps_cap = if self.fps_cap == 0 { 0 } else { self.fps_cap.clamp(15, 480) };
    }
}

fn kv(dst: &mut String, k: &str, v: &str) {
    dst.push_str(k);
    dst.push_str(" = ");
    dst.push_str(v);
    dst.push('\n');
}

fn fmt_f(v: f32) -> String {
    format!("{:.4}", v)
}

fn parse_f32(s: &str, def: f32) -> f32 {
    s.parse().unwrap_or(def)
}

fn parse_u32(s: &str, def: u32) -> u32 {
    s.parse().unwrap_or(def)
}

fn parse_bool(s: &str, def: bool) -> bool {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on"  => true,
        "false" | "0" | "no" | "off" => false,
        _ => def,
    }
}