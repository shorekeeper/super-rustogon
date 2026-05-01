//! Build script.
//!
//! Compiles every shader source file in `src/shaders/` into a
//! SPIR-V binary that the engine then embeds with
//! `include_bytes!(concat!(env!("OUT_DIR"), "/<name>.spv"))`.
//!
//! Source naming convention: `<name>.vert` for vertex stages,
//! `<name>.frag` for fragment stages. Output naming convention
//! mirrors the source, so `main.vert` becomes `main.vert.spv`.
//!
//! The tool used to compile is `glslangValidator`, expected on
//! PATH. The override environment variable `RUSTOGON_GLSLANG`
//! lets a non default install path be selected without editing
//! this script. A failure to spawn the tool, or a non zero
//! exit, aborts the build with the captured stderr so the
//! diagnostic flows back to cargo.

use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/shaders");
    println!("cargo:rerun-if-env-changed=RUSTOGON_GLSLANG");

    let out_dir = env::var("OUT_DIR")
        .expect("OUT_DIR must be set by cargo");
    let exe = env::var("RUSTOGON_GLSLANG")
        .unwrap_or_else(|_| "glslangValidator".to_string());

    // Explicit shader list. Adding a new shader requires one
    // line here and one corresponding `include_bytes!` in the
    // module that uses it. The explicit form is preferred over
    // a directory walk so unused or stale source files do not
    // silently inflate the build.
    let shaders: &[(&str, &str)] = &[
        ("src/shaders/main.vert", "vert"),
        ("src/shaders/main.frag", "frag"),
        ("src/shaders/post.vert", "vert"),
        ("src/shaders/post.frag", "frag"),
        ("src/shaders/text.vert", "vert"),
        ("src/shaders/text.frag", "frag"),
    ];

    for (src, stage) in shaders {
        let src_path = Path::new(src);
        let file_name = src_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_else(|| panic!("bad shader path: {}", src));
        let dst_path = format!("{}/{}.spv", out_dir, file_name);

        println!("cargo:rerun-if-changed={}", src);

        let output = Command::new(&exe)
            .arg("-V")
            .arg("-S").arg(stage)
            .arg("-o").arg(&dst_path)
            .arg(src)
            .output()
            .unwrap_or_else(|e| {
                panic!(
                    "failed to spawn {}: {}; \
                     install the Vulkan SDK or set \
                     RUSTOGON_GLSLANG to the executable path",
                    exe, e);
            });

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            panic!(
                "{} failed for {}\nstderr: {}\nstdout: {}",
                exe, src, stderr.trim(), stdout.trim());
        }
    }
}