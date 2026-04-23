//! Build time shader compilation.
//!
//! Invokes `glslangValidator` (shipped with the Vulkan SDK) once per shader
//! source file under `src/shaders/` and writes SPIR-V binaries into OUT_DIR.
//! The renderer pulls them in via `include_bytes!`. No Cargo crates are
//! required: the only external tool is the one that already lives on every
//! Vulkan developer's PATH.

use std::env;
use std::path::PathBuf;
use std::process::Command;

const SHADERS: &[&str] = &["main.vert", "main.frag", "post.frag", "post.vert"];

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let shader_dir = PathBuf::from("src/shaders");

    for s in SHADERS {
        let src = shader_dir.join(s);
        let dst = out_dir.join(format!("{}.spv", s));

        let status = Command::new("glslangValidator")
            .arg("-V")
            .arg("--target-env").arg("vulkan1.0")
            .arg(&src)
            .arg("-o").arg(&dst)
            .status()
            .expect("glslangValidator not found on PATH. Install the Vulkan SDK.");

        assert!(status.success(), "Shader compile failed: {}", s);
        println!("cargo:rerun-if-changed={}", src.display());
    }
    println!("cargo:rerun-if-changed=build.rs");
    // Copy the assets/ tree next to the produced executable so
    // the game finds its music regardless of how it is launched.
    // This runs on every build but is cheap because we only copy
    // files whose modification time changed.
    copy_assets_tree();
}

/// Recursively copy the crate-root `assets/` directory into the
/// target build output directory. Silent on missing source: a
/// fresh checkout without any audio assets still builds.
fn copy_assets_tree() {
    let src = std::path::PathBuf::from("assets");
    if !src.exists() {
        return;
    }
    // OUT_DIR points deep inside target/.../build/...; walk up
    // four levels to reach target/{debug,release}/.
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut dst_root = out.clone();
    for _ in 0..3 {
        if let Some(p) = dst_root.parent() {
            dst_root = p.to_path_buf();
        }
    }
    let dst = dst_root.join("assets");
    copy_dir_recursive(&src, &dst);
    println!("cargo:rerun-if-changed=assets");
}

/// Copy `src` into `dst` recursively, creating directories as
/// needed. Errors are intentionally swallowed: a failed asset
/// copy must not abort the build.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    if !src.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(dst);
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name() {
            Some(n) => n,
            None => continue,
        };
        let dst_path = dst.join(name);
        if path.is_dir() {
            copy_dir_recursive(&path, &dst_path);
        } else {
            let _ = std::fs::copy(&path, &dst_path);
        }
    }
}