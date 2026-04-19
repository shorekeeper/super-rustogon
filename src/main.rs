//! HexRS: a from-scratch Super Hexagon style clone skeleton.
//!
//! This entry point intentionally does almost nothing: it spawns a Win32
//! window directly through `user32` / `kernel32` (no winit), attaches a
//! Vulkan surface to it through `ash`, and runs a minimal redraw loop.
//!
//! The visible output at this stage is purely diagnostic: a pulsing
//! centered square (the future playfield) on a darker pulsing background.
//! It exists only to confirm that the swapchain, render pass and
//! present chain are alive before any geometry, shaders, audio or
//! actual gameplay code is wired in.
//!
//! Design notes:
//!
//! * Windows only for now. Anything that would have to vary by OS
//!   (window creation, surface extension, library loading) is hidden
//!   behind `mod win32` and `mod renderer`.
//! * No external windowing or math crates. The only dependency is `ash`,
//!   which is itself a thin wrapper over Vulkan.
//! * Aspect ratio independent. The renderer asks the window for its
//!   client size on every resize and computes a centered square play
//!   region, so 4:3, 16:9, 21:9 and 32:9 all work out of the box.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod win32;
mod renderer;

use std::time::Instant;

fn main() {
    // Open a 1280x720 window at default position. The user can freely
    // resize it; the renderer will follow.
    let window = win32::Window::new("Super Rustogon", 1280, 720);

    // Bring up Vulkan and create a swapchain that matches the current
    // client size of the window.
    let mut renderer = renderer::Renderer::new(window.hinstance, window.hwnd);

    let start = Instant::now();

    while !window.should_close() {
        // Drain pending OS messages first so resize / close take effect
        // before we try to draw into a stale swapchain.
        window.poll_events();

        if window.take_resized() {
            renderer.recreate_swapchain();
        }

        let t = start.elapsed().as_secs_f32();
        renderer.render_frame(t);
    }

    renderer.destroy();
}