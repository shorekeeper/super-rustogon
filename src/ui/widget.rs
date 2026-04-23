//! Shared widget helper types.
//!
//! Keeps the `Ui` module compact by hosting constants and small
//! utility types widgets share.

/// Stable id pool helpers. The UI system itself only cares about
/// `u32` values; these constants give call sites a readable name.
pub mod ids {
    use super::super::WidgetId;

    // Main menu.
    pub const MAIN_PLAY:    WidgetId = 0x0001;
    pub const MAIN_OPTIONS: WidgetId = 0x0002;
    pub const MAIN_QUIT:    WidgetId = 0x0003;

    // Common.
    pub const BACK:         WidgetId = 0x0010;
    pub const RESET:        WidgetId = 0x0011;
    pub const APPLY:        WidgetId = 0x0012;

    // Settings tabs.
    pub const TAB_BAR:      WidgetId = 0x0100;

    // Settings: graphics.
    pub const GFX_VSYNC:        WidgetId = 0x0200;
    pub const GFX_FPS_CAP:      WidgetId = 0x0201;
    pub const GFX_BLOOM:        WidgetId = 0x0202;
    pub const GFX_CHROMATIC:    WidgetId = 0x0203;
    pub const GFX_SHAKE:        WidgetId = 0x0204;
    pub const GFX_MOTION_BLUR:  WidgetId = 0x0205;
    pub const GFX_VIGNETTE:     WidgetId = 0x0206;
    pub const GFX_SCANLINES:    WidgetId = 0x0207;
    pub const GFX_GRAIN:        WidgetId = 0x0208;
    pub const GFX_BEAT_FLASH:   WidgetId = 0x0209;
    pub const GFX_PARTICLES:    WidgetId = 0x020A;
    pub const GFX_PARALLAX:     WidgetId = 0x020B;
    pub const GFX_DEPTH:        WidgetId = 0x020C;
    pub const GFX_SHOW_FPS:     WidgetId = 0x020D;
    pub const GFX_UI_SCALE:     WidgetId = 0x020E;

    // Settings: audio.
    pub const AUD_MASTER: WidgetId = 0x0300;
    pub const AUD_MUSIC:  WidgetId = 0x0301;
    pub const AUD_SFX:    WidgetId = 0x0302;

    // Settings: gameplay.
    pub const GPL_PSPEED:   WidgetId = 0x0400;
    pub const GPL_WOBBLE:   WidgetId = 0x0401;
    pub const GPL_ABILITY:  WidgetId = 0x0402;
    pub const GPL_CLOSE:    WidgetId = 0x0403;
    pub const GPL_HIGHLIGHT:WidgetId = 0x0404;

    // Settings: accessibility.
    pub const ACC_CONTRAST: WidgetId = 0x0500;
    pub const ACC_MOTION:   WidgetId = 0x0501;
    pub const ACC_HITBOX:   WidgetId = 0x0502;
    pub const ACC_COLOR:    WidgetId = 0x0503;

    // Level select.
    pub const LS_DIFF_LEFT:  WidgetId = 0x0600;
    pub const LS_DIFF_RIGHT: WidgetId = 0x0601;
    pub const LS_PLAY:       WidgetId = 0x0602;

    // Music panel.
    pub const MUSIC_PAUSE: WidgetId = 0x0700;
    pub const MUSIC_STOP:  WidgetId = 0x0701;
    pub const MUSIC_BAR:   WidgetId = 0x0702;

    // Level rows (shifted to a safe range).
    pub fn level_row(i: usize) -> WidgetId { 0x8000u32 + i as WidgetId }
}