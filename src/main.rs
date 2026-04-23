//! HexRS entry point.
//!
//! (See original documentation.)
//!
//! This revision threads the loaded `Config` through both
//! `menu.update` and `menu.build_geometry` so the settings screen
//! can read and write it without forcing the menu module to hold
//! a long lived reference.

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
mod config;
mod ui;
mod effects;
mod post;
mod editor;

use std::time::Instant;

use crate::config::Config;
use crate::menu::{AppState, AudioSnapshot};
use crate::pipeline::Vertex;
use crate::renderer::FramePacer;

fn main() {
    let mut config = Config::load();
    config.clamp();

    let window = win32::Window::new("Super Rustogon", 1280, 720);
    let mut renderer = renderer::Renderer::new(
        window.hinstance, window.hwnd, config.vsync);
    let audio = audio::Audio::new();
    audio.set_volume(config.master_volume);

    let _ = levels::num();

    let mut menu = menu::Menu::new();
    let mut game:   Option<game::Game>     = None;
    let mut editor: Option<editor::Editor> = None;
    let mut editor_play_level: Option<crate::levels::Level> = None;

    let mut vertices: Vec<Vertex> = Vec::with_capacity(262_144);

    let mut pacer = FramePacer::new(config.fps_cap);

    let mut last = Instant::now();
    let mut esc_was_down = false;
    let mut game_was_alive = false;

    let mut current_music: Option<String> = None;
    let mut current_bpm:   u32 = 0;

    let mut applied_vsync = config.vsync;
    let mut applied_fps   = config.fps_cap;
    let mut app_time: f32 = 0.0;

    // FPS telemetry for the overlay.
    let mut fps_samples: [f32; 32] = [0.0; 32];
    let mut fps_cursor = 0usize;

    while !window.should_close() {
        pacer.begin();
        window.poll_events();
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
                    // Music rate scaling honors the DSL speedwarp trigger.
                    // MusicTrack already clamps the rate internally.
                    let rate = g.music_rate().clamp(0.25, 4.0);
                    // Convert the rate into a BPM hint so the audio worker
                    // still publishes sensible pulse data during warps.
                    let bpm = (current_bpm as f32 * rate).round() as u32;
                    audio.set_bpm(bpm.clamp(40, 300));
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
                let outcome = e.update(dt, mouse, input, cw, ch, &audio);
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
            }
            AppState::EditorPlay { tier_idx } => {
                // Ordering matters: the ESC branch must take
                // a mutable borrow of `editor_play_level` to
                // drop it, while the normal-play branch holds
                // an immutable borrow of the same variable
                // through `lvl`. Splitting them into separate
                // if/else arms keeps those borrows from ever
                // overlapping, which is what the previous
                // revision got wrong and what made the draft
                // seem to evaporate on exit.
                if esc_edge {
                    menu.return_to_editor();
                    game = None;
                    game_was_alive = false;
                    editor_play_level = None;
                    audio.play_exit();
                    renderer.set_shake([0.0, 0.0]);
                    renderer.set_zoom(1.0);
                } else if editor_play_level.is_none() {
                    // Defensive: some other branch should have
                    // populated the snapshot before we got
                    // here. Rather than panic on an unwrap,
                    // slide back into editing.
                    menu.return_to_editor();
                    renderer.set_shake([0.0, 0.0]);
                    renderer.set_zoom(1.0);
                } else {
                    let lvl = editor_play_level.as_ref().unwrap();
                    if game.is_none() {
                        let tier = lvl.difficulty_tiers
                            .get(tier_idx as usize).copied()
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
                    let g = game.as_mut().unwrap();
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
                menu.build_geometry(&mut vertices, &snap, &config);
            }
        }

        // Apply vsync / fps changes at the end of the frame.
        if config.vsync != applied_vsync {
            renderer.set_vsync(config.vsync);
            applied_vsync = config.vsync;
        }
        if config.fps_cap != applied_fps {
            pacer.set_fps(config.fps_cap);
            applied_fps = config.fps_cap;
        }

        // Optional FPS overlay. We compute the view bounds inline
        // rather than going through the `ui` module so the main
        // loop stays independent of the widget layer.
        if config.show_fps {
            let avg: f32 = fps_samples.iter().sum::<f32>() / fps_samples.len() as f32;
            let txt = format!("{:3.0} FPS", avg);
            let aspect = cw.max(1) as f32 / ch.max(1) as f32;
            let (vleft, vbottom) = if aspect >= 1.0 {
                (-aspect, 1.0_f32)
            } else {
                (-1.0_f32, 1.0 / aspect)
            };
            text::push_text(&mut vertices, &txt,
                vleft + 0.03, vbottom - 0.06,
                0.0045, [1.0, 1.0, 1.0]);
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

        let (glitch, strobe) = match &game {
            Some(g) if matches!(menu.state(), AppState::Playing { .. }) =>
                (g.glitch_amount(), g.strobe_alpha()),
            _ => (0.0, 0.0),
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
            _pad:               [0.0, 0.0],
        };
        renderer.set_post_params(post_params);

        renderer.render_frame(&vertices);

        pacer.wait();
    }

    // Persist any last-minute tweaks.
    config.save();

    drop(audio);
    renderer.destroy();
}