//! Thin wrapper around `glslangValidator`.
//!
//! The sandbox does not link any SPIR-V producing library
//! against the engine binary. Instead it writes the
//! generated GLSL to a temporary file, invokes the
//! `glslangValidator` program from the same toolchain that
//! `build.rs` already uses for the engine's own shaders,
//! reads the produced SPIR-V, and removes both temporary
//! files. This keeps the runtime's dependency graph at
//! exactly one crate (`ash`) while still offering a real
//! source to binary shader pipeline.
//!
//! The executable name is configurable through the
//! `RUSTOGON_GLSLANG` environment variable. If not set,
//! `glslangValidator` from PATH is used, which matches the
//! behavior of the Vulkan SDK installer on all three desktop
//! operating systems.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Compile GLSL source text to SPIR-V bytes.
pub fn compile_glsl_to_spirv(glsl: &str) -> Result<Vec<u8>, String> {
    let exe = env::var("RUSTOGON_GLSLANG")
        .unwrap_or_else(|_| "glslangValidator".to_string());

    let tag = unique_tag();
    let src_path = temp_path(&format!("rustogon_user_{}.frag", tag));
    let out_path = temp_path(&format!("rustogon_user_{}.spv",  tag));

    {
        let mut f = fs::File::create(&src_path)
            .map_err(|e| format!("create source tmp: {}", e))?;
        f.write_all(glsl.as_bytes())
            .map_err(|e| format!("write source tmp: {}", e))?;
    }

    let result = Command::new(&exe)
        .arg("-V")
        .arg("-S").arg("frag")
        .arg("-o").arg(&out_path)
        .arg(&src_path)
        .output();

    let _ = fs::remove_file(&src_path);

    let output = match result {
        Ok(o) => o,
        Err(e) => {
            let _ = fs::remove_file(&out_path);
            return Err(format!("failed to spawn {}: {}", exe, e));
        }
    };

    if !output.status.success() {
        let _ = fs::remove_file(&out_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "{} exited with {}\nstderr: {}\nstdout: {}",
            exe, output.status, stderr.trim(), stdout.trim()));
    }

    let bytes = fs::read(&out_path)
        .map_err(|e| format!("read spv: {}", e))?;
    let _ = fs::remove_file(&out_path);

    if bytes.len() < 20 {
        return Err("glslang produced a blob too small to be SPIR-V".into());
    }
    Ok(bytes)
}

fn temp_path(name: &str) -> PathBuf {
    let mut base = env::temp_dir();
    base.push(name);
    base
}

fn unique_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}_{}", std::process::id(), nanos)
}