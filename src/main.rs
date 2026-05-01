//! HexRS entry point.
//!
//! This revision threads the loaded `Config` through both
//! `menu.update` and `menu.build_geometry` so the settings screen
//! can read and write it without forcing the menu module to hold
//! a long lived reference. It also hosts the shader sandbox that
//! compiles, validates, and optionally renders user supplied post
//! shaders. The sandbox never touches Vulkan directly; it hands
//! back an opaque `UserPipeline` which the renderer installs
//! through `Renderer::set_user_post_pipeline`. Device loss during
//! rendering is propagated back to the sandbox via
//! `Renderer::take_device_lost` so misbehaving shaders can be
//! quarantined.
//!
//! The `shader :name = ~p"path"` declarations inside a .rlf
//! level flow to runtime through `LevelAst::shaders`. A
//! `PostShader` trigger inside a section sets
//! `Generator::current_post_shader`, which the main loop reads
//! every frame via `Game::active_post_shader` and hands to
//! `apply_active_shader`. That helper owns the compile cache,
//! tracks which shader is currently bound on the renderer, and
//! falls back to the built in post pipeline whenever compilation
//! or validation fails.
//!
//! Font system: the SDF font atlas is baked at startup by
//! `font::init`, before the renderer is constructed. The
//! renderer's text stage reaches into the global atlas during
//! its own initialization, so the order is mandatory. A failure
//! at this stage (corrupted embedded TTF, parser bug) is fatal
//! and exits the process with a readable diagnostic; a missing
//! font file is impossible by construction because the file is
//! embedded with `include_bytes!`.

mod win32;
mod renderer;
mod pipeline;
mod buffer;
mod game;
mod menu;
mod levels;
mod dsl;
mod gend;
mod audio;
mod qoa;
mod music;
mod text;
mod font;
mod config;
mod ui;
mod effects;
mod post;
mod editor;
mod rule_engine;
mod text_small;
mod shader_sandbox;

use std::collections::HashMap;
use std::time::Instant;

use crate::config::Config;
use crate::menu::{AppState, AudioSnapshot};
use crate::pipeline::Vertex;
use crate::renderer::FramePacer;
use crate::shader_sandbox::{SandboxMode, ShaderSandbox};
use crate::shader_sandbox::pipeline::UserPipeline;

/// Result of one attempt to compile a user shader. We cache
/// failures explicitly so a misbehaving or missing shader
/// fails the load exactly once per section activation rather
/// than every frame while the section is active.
enum ShaderSlot {
    /// Compilation succeeded. The pipeline is owned here
    /// and destroyed through `drop_shader_cache` when the
    /// player leaves the level or the sandbox mode is
    /// lowered.
    Pipeline(UserPipeline),
    /// Compilation or validation failed. The cached error
    /// message is emitted once on insertion, then silently
    /// reused for every subsequent frame the same shader is
    /// requested. Prevents the log flood reported on
    /// shaders whose files are missing on disk.
    Failed,
}


/// Destroy every cached user pipeline, forget every cached
/// failure, and clear the renderer's active user pipeline
/// slot. Called when the player leaves a level, when the
/// sandbox is downgraded away from `On`, and during
/// shutdown so user shader Vulkan objects never leak and
/// never outlive the device, and so a fresh attempt to
/// load a shader starts from a clean slate.
fn drop_shader_cache(
    cache:    &mut HashMap<String, ShaderSlot>,
    renderer: &mut renderer::Renderer,
    sandbox:  &mut ShaderSandbox,
) {
    renderer.set_user_post_pipeline(None);
    sandbox.mark_inactive();
    for (_, slot) in cache.drain() {
        if let ShaderSlot::Pipeline(p) = slot {
            p.destroy(renderer.device_ref());
        }
    }
}

/// Read the active user post shader from the running game
/// and install, swap, or clear the corresponding user
/// pipeline on the renderer. Compiles the shader on first
/// use and caches it in `cache`. Compile or validation
/// failures are logged to stderr and the renderer silently
/// falls back to the default post pipeline for the frame,
/// so a level with a bad shader file still plays.
/// Read the active user post shader from the running game
/// and install, swap, or clear the corresponding user
/// pipeline on the renderer. Compiles the shader on first
/// use and caches the result in `cache`.
///
/// Both successful compilations and outright failures are
/// cached so a missing shader file, a syntax error, a
/// rejected SPIR-V module, or a quarantined id all
/// produce exactly one diagnostic on the console rather
/// than one per frame. The cache is cleared whenever the
/// player leaves the level (see `drop_shader_cache`), so
/// authors editing shaders on disk can retry by re-entering
/// the level instead of restarting the game.
///
/// On any failure the renderer silently falls back to the
/// default post pipeline for the frame, so a level with a
/// bad shader file still plays.
fn apply_active_shader(
    g:        &game::Game,
    lvl:      &crate::levels::Level,
    renderer: &mut renderer::Renderer,
    sandbox:  &mut ShaderSandbox,
    cache:    &mut HashMap<String, ShaderSlot>,
) {
    match g.active_post_shader() {
        Some((slot, params)) => {
            let decl = match lvl.ast.shaders.get(slot as usize) {
                Some(d) => d,
                None => {
                    sandbox.mark_inactive();
                    renderer.set_user_post_pipeline(None);
                    return;
                }
            };

            // First time we see this shader in the current
            // level: compile it and record the outcome,
            // whatever it is.
            if !cache.contains_key(&decl.name) {
                let res = sandbox.load_file(
                    renderer.instance_ref(),
                    renderer.device_ref(),
                    renderer.physical_device(),
                    renderer.post_render_pass(),
                    renderer.post_descriptor_set_layout(),
                    &decl.path,
                );
                match res {
                    Ok(pipe) => {
                        cache.insert(decl.name.clone(),
                            ShaderSlot::Pipeline(pipe));
                    }
                    Err(e) => {
                        eprintln!(
                            "[sandbox] load '{}' failed: {}",
                            decl.name, e);
                        cache.insert(decl.name.clone(),
                            ShaderSlot::Failed);
                    }
                }
            }

            match cache.get(&decl.name) {
                Some(ShaderSlot::Pipeline(pipe)) => {
                    let mut push = Vec::with_capacity(16);
                    for f in params.iter() {
                        push.extend_from_slice(&f.to_le_bytes());
                    }
                    sandbox.mark_active(&pipe.id);
                    renderer.set_user_post_pipeline(Some((
                        pipe.pipeline, pipe.layout, push,
                    )));
                }
                _ => {
                    // Known bad shader. Stay on the default
                    // post pipeline, no log spam.
                    sandbox.mark_inactive();
                    renderer.set_user_post_pipeline(None);
                }
            }
        }
        None => {
            sandbox.mark_inactive();
            renderer.set_user_post_pipeline(None);
        }
    }
}

fn main() {
    let mut config = Config::load();
    config.clamp();

    // Bake the SDF font atlas before any Vulkan object is
    // created. The renderer's text stage reads through
    // `font::atlas()` while building its image and descriptor
    // set, so the atlas must exist by then. A failure here is
    // unrecoverable: it means either the embedded TTF blob is
    // corrupt or the parser cannot handle this font's tables,
    // both of which are development time issues that should
    // surface immediately rather than as broken text on screen.
    if let Err(e) = font::init() {
        eprintln!("[font] failed to load Exo 2: {}", e);
        eprintln!("[font] make sure assets/fonts/Exo2-Regular.ttf exists");
        std::process::exit(1);
    }

    let window = win32::Window::new("Super Rustogon", 1280, 720);
    let mut renderer = renderer::Renderer::new(
        window.hinstance, window.hwnd, config.vsync);
    let audio = audio::Audio::new();
    audio.set_volume(config.master_volume);

    let _ = levels::num();

    // Shader sandbox. Picks up the `custom_shaders` policy
    // from the config file and begins its life with no
    // active user shader. The sandbox also holds a persistent
    // quarantine list that survives across launches.
    let mut shader_sandbox = ShaderSandbox::new(match config.custom_shaders {
        config::CustomShaders::Off   => SandboxMode::Off,
        config::CustomShaders::Audit => SandboxMode::Audit,
        config::CustomShaders::On    => SandboxMode::On,
    });

    let mut shader_cache: HashMap<String, ShaderSlot> =
        HashMap::new();

    let mut menu = menu::Menu::new();
    let mut game:   Option<game::Game>     = None;
    let mut editor: Option<editor::Editor> = None;
    let mut editor_play_level: Option<crate::levels::Level> = None;

    // Per frame geometry buffers. `vertices` carries gameplay
    // and main menu shapes drawn through the main pipeline.
    // `hud_vertices` carries overlays that should not respond
    // to camera shake / zoom / tilt. `text_vertices` carries
    // SDF text geometry consumed by the renderer's text stage,
    // which has its own pipeline and atlas binding.
    let mut vertices: Vec<Vertex> = Vec::with_capacity(262_144);
    let mut hud_vertices: Vec<Vertex> = Vec::with_capacity(4096);
    let mut text_vertices: Vec<font::TextVertex> = Vec::with_capacity(4096);

    let mut pacer = FramePacer::new(config.fps_cap);

    let mut last = Instant::now();
    let mut esc_was_down = false;
    let mut game_was_alive = false;

    let mut current_music: Option<String> = None;
    let mut current_bpm:   u32 = 0;

    let mut applied_vsync = config.vsync;
    let mut applied_fps   = config.fps_cap;
    let mut applied_sandbox_mode = config.custom_shaders;
    let mut app_time: f32 = 0.0;

    // Tracks the most recently applied music rate so we only
    // push a MusicCommand::SetRate when the value actually
    // changes. The audio worker's mutex is cheap to acquire
    // but still a lock, so throttling to real changes keeps
    // the hot path clean.
    let mut applied_music_rate: f32 = 1.0;

    // FPS telemetry for the overlay.
    let mut fps_samples: [f32; 32] = [0.0; 32];
    let mut fps_cursor = 0usize;

    while !window.should_close() {
        pacer.begin();
        window.poll_events();
        hud_vertices.clear();
        text_vertices.clear();
        // Per-channel audio gains are pushed every frame so any
        // edit in the options screen takes effect on the very
        // next buffer. Master volume stays wired through the
        // menu's own update path.
        audio.set_music_volume(config.music_volume);
        audio.set_sfx_volume(config.sfx_volume);
        if window.take_resized() { renderer.recreate_swapchain(); }

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;
        app_time += dt;

        fps_samples[fps_cursor] = 1.0 / dt.max(1e-4);
        fps_cursor = (fps_cursor + 1) % fps_samples.len();

        let input = window.input();
        let esc_edge = input.escape && !esc_was_down;
        esc_was_down = input.escape;

        let (cw, ch) = window.client_size();
        let mouse = window.mouse();

        let snap = AudioSnapshot {
            position:      audio.music_position(),
            duration:      audio.music_duration(),
            paused:        audio.is_music_paused(),
            has_music:     audio.has_music(),
            onset_phase:   audio.onset_phase(),
            onset_counter: audio.onset_counter(),
        };

        match menu.state() {
            AppState::Playing { level, difficulty_idx } => {
                if game.is_none() {
                    let lvl = crate::levels::get(level);
                    let tier = lvl.difficulty_tiers
                        .get(difficulty_idx as usize)
                        .copied()
                        .unwrap_or(lvl.base_tier);
                    let mut g = game::Game::new(lvl, tier, &config);
                    if !lvl.music_path.is_empty() {
                        // Atomic restart + seek so the worker
                        // installs the new track AT the
                        // `#[startfrom]` offset rather than
                        // racing a follow-up Seek command
                        // against the async decoder.
                        audio.restart_music_at(
                            &lvl.music_path,
                            lvl.ast.start_from_seconds.max(0.0),
                        );
                        current_music = Some(lvl.music_path.clone());
                    }
                    g.sync_onset_counter(&audio);
                    game = Some(g);
                    game_was_alive = true;
                }

                if esc_edge {
                    menu.return_to_main();
                    game = None;
                    game_was_alive = false;
                    audio.play_exit();
                    // Drop cached user pipelines so they do
                    // not outlive the level and do not leak
                    // into the menu theme if the user later
                    // starts a different level.
                    drop_shader_cache(
                        &mut shader_cache, &mut renderer,
                        &mut shader_sandbox);
                    // Reset the music rate so the menu theme
                    // does not inherit a residual speedwarp
                    // from the run that just ended.
                    if (applied_music_rate - 1.0).abs() > 1e-3 {
                        audio.set_music_rate(1.0);
                        applied_music_rate = 1.0;
                    }
                } else {
                    let g   = game.as_mut().unwrap();
                    let lvl = crate::levels::get(level);
                    g.apply_config(&config);
                    g.update(dt, input, &audio, lvl);
                    if game_was_alive && g.is_dead() {
                        audio.play_touch();
                        audio.stop_music();
                    }
                    game_was_alive = g.is_alive();
                    g.build_geometry(&mut vertices);

                    let (sx, sy) = g.shake_offset();
                    renderer.set_shake([sx, sy]);
                    renderer.set_zoom(g.zoom());
                    renderer.set_tilt(g.tilt());
                    // Music rate scaling honors the DSL
                    // speedwarp trigger. Three things happen
                    // each frame:
                    //
                    // 1. Push the rate through the audio
                    //    worker so the track actually plays
                    //    at the new speed / pitch. Throttled
                    //    against `applied_music_rate` to
                    //    avoid posting a command every frame
                    //    when the rate has not moved.
                    // 2. Scale the published BPM so the
                    //    menu / HUD pulse visuals stay in
                    //    lockstep with the warped track.
                    let rate = g.music_rate().clamp(0.25, 4.0);
                    if (rate - applied_music_rate).abs() > 1e-3 {
                        audio.set_music_rate(rate);
                        applied_music_rate = rate;
                    }
                    let bpm = (current_bpm as f32 * rate).round() as u32;
                    audio.set_bpm(bpm.clamp(40, 300));

                    // Sync the user post shader for this
                    // frame. The active shader may have
                    // changed inside `g.update` via a
                    // PostShader trigger; this call handles
                    // compilation, caching, pipeline swap,
                    // and the fallback to the default post
                    // when anything goes wrong.
                    apply_active_shader(
                        g, lvl, &mut renderer,
                        &mut shader_sandbox,
                        &mut shader_cache);
                }
            }
            AppState::Quit => break,
            AppState::Editor => {
                if editor.is_none() {
                    let boot = menu.take_editor_boot();
                    let source = match boot {
                        crate::menu::EditorBoot::New => editor::EditorSource::New,
                        crate::menu::EditorBoot::Existing(idx) => {
                            let lvl = crate::levels::get(idx);
                            let stem = lvl.name
                                .to_lowercase()
                                .chars()
                                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                                .collect::<String>();
                            editor::EditorSource::Existing {
                                stem, ast: lvl.ast.clone(),
                            }
                        }
                    };
                    editor = Some(editor::Editor::new(source));
                }
                let e = editor.as_mut().unwrap();
                let outcome = e.update(
                    dt, mouse, input,
                    window.take_scroll_delta(),
                    cw, ch, &audio);
                e.draw(&mut vertices, &audio);
                match outcome {
                    editor::EditorOutcome::Stay => {}
                    editor::EditorOutcome::Back => {
                        menu.return_to_main();
                        editor = None;
                        editor_play_level = None;
                        audio.play_exit();
                    }
                    editor::EditorOutcome::PrePlay => {
                        // Materialize the editor's draft into a
                        // playable Level, owned by the main
                        // loop. Editor instance is kept around
                        // so the play-test can be exited back
                        // into the editor with state intact.
                        let ast = e.ast_clone();
                        let lvl = crate::levels::Level::from_ast(ast);
                        let tier_idx = lvl.default_difficulty_index() as u32;
                        editor_play_level = Some(lvl);
                        menu.enter_editor_play(tier_idx);
                        audio.stop_music();
                    }
                }
                renderer.set_shake([0.0, 0.0]);
                renderer.set_zoom(1.0);
                renderer.set_tilt([0.0, 0.0, 0.0]);
            }
            AppState::EditorPlay { tier_idx } => {
                if esc_edge {
                    menu.return_to_editor();
                    game = None;
                    game_was_alive = false;
                    editor_play_level = None;
                    audio.play_exit();
                    drop_shader_cache(
                        &mut shader_cache, &mut renderer,
                        &mut shader_sandbox);
                    renderer.set_shake([0.0, 0.0]);
                    renderer.set_zoom(1.0);
                    if (applied_music_rate - 1.0).abs() > 1e-3 {
                        audio.set_music_rate(1.0);
                        applied_music_rate = 1.0;
                    }
                    renderer.set_tilt([0.0, 0.0, 0.0]);
                } else if editor_play_level.is_none() {
                    menu.return_to_editor();
                    renderer.set_shake([0.0, 0.0]);
                    renderer.set_zoom(1.0);
                    renderer.set_tilt([0.0, 0.0, 0.0]);
                } else {
                    let lvl = editor_play_level.as_ref().unwrap();
                    if game.is_none() {
                        let tier = lvl.difficulty_tiers
                            .get(tier_idx as usize).copied()
                            .unwrap_or(lvl.base_tier);
                        let mut g = game::Game::new(lvl, tier, &config);
                        if !lvl.music_path.is_empty() {
                            audio.restart_music_at(
                                &lvl.music_path,
                                lvl.ast.start_from_seconds.max(0.0),
                            );
                            current_music = Some(lvl.music_path.clone());
                        }
                        g.sync_onset_counter(&audio);
                        game = Some(g);
                        game_was_alive = true;
                    }
                    let g = game.as_mut().unwrap();
                    g.apply_config(&config);
                    g.update(dt, input, &audio, lvl);
                    if game_was_alive && g.is_dead() {
                        audio.play_touch();
                        audio.stop_music();
                    }
                    game_was_alive = g.is_alive();
                    g.build_geometry(&mut vertices);

                    // Pre-play banner. Tiny overlay at the top
                    // that reminds the author this is a test
                    // run and ESC returns to the editor.
                    let aspect = cw.max(1) as f32 / ch.max(1) as f32;
                    let (vleft, vright, vtop) = if aspect >= 1.0 {
                        (-aspect, aspect, -1.0_f32)
                    } else {
                        (-1.0_f32, 1.0_f32, -1.0 / aspect)
                    };
                    let banner_y0 = vtop;
                    let banner_y1 = vtop + 0.06;
                    hud_vertices.push(
                        crate::pipeline::Vertex::rgba(
                            [vleft,  banner_y0],
                            [1.0, 0.45, 0.75], 0.70));
                    hud_vertices.push(
                        crate::pipeline::Vertex::rgba(
                            [vright, banner_y0],
                            [1.0, 0.45, 0.75], 0.70));
                    hud_vertices.push(
                        crate::pipeline::Vertex::rgba(
                            [vright, banner_y1],
                            [1.0, 0.45, 0.75], 0.70));
                    hud_vertices.push(
                        crate::pipeline::Vertex::rgba(
                            [vleft,  banner_y0],
                            [1.0, 0.45, 0.75], 0.70));
                    hud_vertices.push(
                        crate::pipeline::Vertex::rgba(
                            [vright, banner_y1],
                            [1.0, 0.45, 0.75], 0.70));
                    hud_vertices.push(
                        crate::pipeline::Vertex::rgba(
                            [vleft,  banner_y1],
                            [1.0, 0.45, 0.75], 0.70));
                    let msg = "PRE-PLAY   ESC: BACK TO EDITOR   SPACE: RETRY ON DEATH";
                    let cx = (vleft + vright) * 0.5;
                    text_small::push_small_centered(
                        &mut hud_vertices, msg,
                        cx, banner_y0 + 0.018, 0.0045,
                        [1.0, 1.0, 1.0]);

                    let (sx, sy) = g.shake_offset();
                    renderer.set_shake([sx, sy]);
                    renderer.set_zoom(g.zoom());
                    renderer.set_tilt(g.tilt());
                    let rate = g.music_rate().clamp(0.25, 4.0);
                    if (rate - applied_music_rate).abs() > 1e-3 {
                        audio.set_music_rate(rate);
                        applied_music_rate = rate;
                    }

                    apply_active_shader(
                        g, lvl, &mut renderer,
                        &mut shader_sandbox,
                        &mut shader_cache);
                }
            }
            _ => {
                game = None;
                game_was_alive = false;
                // Editor instance must survive PrePlay round
                // trips. Drop it only when neither editor nor
                // its play-test is currently the active state.
                let in_editor_flow = matches!(menu.state(),
                    AppState::Editor | AppState::EditorPlay { .. });
                if !in_editor_flow {
                    editor = None;
                    editor_play_level = None;
                }
                renderer.set_shake([0.0, 0.0]);
                renderer.set_zoom(1.0);
                renderer.set_tilt([0.0, 0.0, 0.0]);

                if esc_edge {
                    match menu.state() {
                        AppState::MainMenu => break,
                        AppState::Intro    => {}
                        AppState::Settings => {
                            config.save();
                            menu.return_to_main();
                            audio.play_interact();
                        }
                        _ => {
                            menu.return_to_main();
                            audio.play_interact();
                        }
                    }
                }
                menu.update(dt, mouse, input, cw, ch, &audio, &snap, &mut config);
                menu.build_geometry(&mut vertices, &mut text_vertices, &snap, &config);
            }
        }

        // Apply vsync / fps / sandbox mode changes at the end
        // of the frame. Sandbox mode downgrade (On -> Audit /
        // Off) tears the active user pipeline down so the
        // next frame reverts to the default post pipeline.
        if config.vsync != applied_vsync {
            renderer.set_vsync(config.vsync);
            applied_vsync = config.vsync;
        }
        if config.fps_cap != applied_fps {
            pacer.set_fps(config.fps_cap);
            applied_fps = config.fps_cap;
        }
        if config.custom_shaders != applied_sandbox_mode {
            shader_sandbox.set_mode(match config.custom_shaders {
                config::CustomShaders::Off   => SandboxMode::Off,
                config::CustomShaders::Audit => SandboxMode::Audit,
                config::CustomShaders::On    => SandboxMode::On,
            });
            if !matches!(config.custom_shaders, config::CustomShaders::On) {
                drop_shader_cache(
                    &mut shader_cache, &mut renderer,
                    &mut shader_sandbox);
            }
            applied_sandbox_mode = config.custom_shaders;
        }

        // Optional FPS overlay. Routed through the SDF font
        // stage so the readout stays sharp at any window size
        // and DPI; the legacy bitmap font shipped with the
        // engine produced a visibly fuzzy readout on hi-DPI
        // displays. Pixel size 0.045 here is em height in
        // game space, which renders at roughly the same
        // visual size as the previous bitmap call did at
        // pixel_size 0.0045.
        if config.show_fps {
            let avg: f32 = fps_samples.iter().sum::<f32>()
                / fps_samples.len() as f32;
            let txt = format!("{:3.0} FPS", avg);
            let aspect = cw.max(1) as f32 / ch.max(1) as f32;
            let (vleft, vbottom) = if aspect >= 1.0 {
                (-aspect, 1.0_f32)
            } else {
                (-1.0_f32, 1.0 / aspect)
            };
            font::push_text(
                &mut text_vertices, &txt,
                vleft + 0.03, vbottom - 0.10,
                0.045, [1.0, 1.0, 1.0]);
        }

        // Music override: while in the editor the chosen track
        // comes from the level's `meta.music`, not the menu
        // theme. Falls back to whatever the menu wants when no
        // editor is active or it has no track picked yet.
        let desired_music = match menu.state() {
            crate::menu::AppState::Editor => editor.as_ref()
                .and_then(|e| e.desired_music())
                .or_else(|| menu.desired_music()),
            crate::menu::AppState::EditorPlay { .. } => editor_play_level
                .as_ref()
                .and_then(|l| if l.music_path.is_empty() { None }
                              else { Some(l.music_path.clone()) }),
            _ => menu.desired_music(),
        };
        if desired_music.as_deref() != current_music.as_deref() {
            let is_playing = matches!(menu.state(), AppState::Playing { .. });
            if is_playing {
                current_music = desired_music;
            } else {
                match desired_music.as_deref() {
                    Some(p) => audio.play_music(p),
                    None    => audio.stop_music(),
                }
                current_music = desired_music;
            }
        }

        let desired_bpm = menu.desired_bpm();
        if desired_bpm != current_bpm {
            audio.set_bpm(desired_bpm);
            current_bpm = desired_bpm;
        }

        // Pull every per trigger post parameter from the live
        // game instance. When we are not in a play state the
        // triggers do not exist, so defaults collapse to the
        // neutral (effect disabled) values.
        let (glitch, strobe, invert_colors, grayscale,
             shockwave_progress, shockwave_strength,
             fog_near, fog_far, outline_amount) = match &game {
            Some(g) if matches!(menu.state(),
                AppState::Playing { .. }
                | AppState::EditorPlay { .. }) =>
            {
                let (fn0, ff0) = g.fog_bounds();
                (
                    g.glitch_amount(),
                    g.strobe_alpha(),
                    g.invert_colors_amount(),
                    g.grayscale_amount(),
                    g.shockwave_progress(),
                    g.shockwave_strength(),
                    fn0, ff0,
                    g.outline_amount(),
                )
            }
            _ => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        };
        let post_params = post::PostParams {
            time:               app_time,
            bloom_intensity:    config.bloom_intensity,
            chromatic_strength: config.chromatic_strength,
            vignette:           config.vignette,
            scanlines:          if config.scanlines  { 1.0 } else { 0.0 },
            film_grain:         if config.film_grain { 1.0 } else { 0.0 },
            colorblind_mode:    match config.colorblind {
                config::ColorblindMode::Off          => 0.0,
                config::ColorblindMode::Protanopia   => 1.0,
                config::ColorblindMode::Deuteranopia => 2.0,
                config::ColorblindMode::Tritanopia   => 3.0,
            },
            high_contrast:      if config.high_contrast { 1.0 } else { 0.0 },
            resolution:         [1.0, 1.0],
            glitch, strobe,
            invert_colors, grayscale,
            shockwave_progress, shockwave_strength,
            fog_near, fog_far,
            outline_amount,
            _pad: 0.0,
        };
        renderer.set_post_params(post_params);

        renderer.render_frame(&vertices, &hud_vertices, &text_vertices);

        // After every frame, check whether the device was
        // reported lost. If so and a user shader was active,
        // hand the blame to the sandbox (the shader name has
        // been kept alive through `mark_active`) and drop
        // the cached pipelines. Recovery of the Vulkan
        // objects happens on the following
        // `recreate_swapchain` triggered by the next resize
        // or present failure.
        if renderer.take_device_lost() {
            shader_sandbox.report_device_lost();
            drop_shader_cache(
                &mut shader_cache, &mut renderer,
                &mut shader_sandbox);
            eprintln!("[sandbox] device lost reported to shader sandbox");
        }

        pacer.wait();
    }

    // Persist any last-minute tweaks.
    config.save();

    // Tear down the user shader pipelines, if any, before
    // dropping the renderer so Vulkan objects are destroyed
    // while the device is still alive.
    drop_shader_cache(
        &mut shader_cache, &mut renderer,
        &mut shader_sandbox);

    drop(audio);
    renderer.destroy();
}