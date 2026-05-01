//! Sandbox for user supplied post process shaders.
//!
//! The base game ships one fragment shader for the post pass,
//! authored at `assets/shaders/post.frag`, which bakes every
//! visual effect the DSL can request through the `PostParams`
//! push block. Power users and level authors can write their
//! own post effect on top of the scene, but handing arbitrary
//! code to a graphics driver is risky. A malformed user
//! shader can do three kinds of damage:
//!
//! 1. Drive the GPU into an unbounded loop. On Windows this
//!    triggers Timeout Detection and Recovery after roughly
//!    two seconds, the driver is reset, and every application
//!    using the GPU loses its device. On weaker integrated
//!    parts this can escalate to a blue screen.
//! 2. Touch unbounded memory through Shader Storage Buffer
//!    Objects. SSBOs allow both read and write against an
//!    arbitrary descriptor, with no bounds checking inside
//!    the shader stage.
//! 3. Allocate so many texture samples per pixel that the
//!    frame time collapses to a fraction of a frame per
//!    second, well before the driver considers the shader
//!    stuck.
//!
//! This module mitigates all three in depth. Four layers of
//! defense stack, so an escape from any single layer is
//! still caught by the next:
//!
//! Layer one, constrained DSL. Authors do not write raw
//! GLSL. They write a small expression language that has no
//! loops, no recursion, and no access to mutable state beyond
//! `let` bindings. See `dsl.rs` and `codegen.rs`.
//!
//! Layer two, SPIR-V validation after glslang compiles the
//! generated GLSL. Regardless of how the SPIR-V blob was
//! built the validator walks every instruction and rejects
//! the module whenever it sees: a disallowed capability
//! (anything beyond `Shader` and `Matrix`), an unapproved
//! storage class (most notably `StorageBuffer` for SSBOs),
//! a decoration that marks buffer block layout
//! (`BufferBlock`), any structured loop merge instruction
//! (`OpLoopMerge`), or a module that exceeds the instruction
//! budget. See `spirv.rs`.
//!
//! Layer three, runtime quarantine. If the Vulkan device is
//! lost while a user shader is active, the sandbox writes
//! the shader identifier to a persistent quarantine file.
//! Future loads of that shader refuse to build a pipeline
//! until the user clears the entry. See `quarantine.rs`.
//!
//! Layer four, user facing opt in. The config file has a
//! `custom_shaders` field with three settings: `off` (the
//! default, disables the whole subsystem), `audit` (compile
//! and validate but do not render), `on` (render).
//!
//! Dependencies. Only `ash` plus the Rust standard library.
//! `compile.rs` invokes `glslangValidator` as a subprocess,
//! exactly the same way `build.rs` already does for the
//! engine's own shaders.

pub mod spirv;
pub mod quarantine;
pub mod dsl;
pub mod codegen;
pub mod compile;
pub mod pipeline;

use std::path::{Path, PathBuf};

use ash::{vk, Device, Instance};

use crate::shader_sandbox::pipeline::UserPipeline;
use crate::shader_sandbox::quarantine::Quarantine;

/// What the sandbox is currently allowed to do. Wired from
/// the config file at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    /// Subsystem disabled. No DSL parse, no compile, no
    /// pipeline. Used by default so a fresh install never
    /// runs user shaders until the user opts in.
    Off,
    /// Parse, compile, validate, and benchmark, but do not
    /// swap the active post pipeline. Logs every failure so
    /// an author can iterate on their shader without letting
    /// a rendering regression reach the screen.
    Audit,
    /// Full execution. Compiled and validated shaders can be
    /// installed as the active post pipeline.
    On,
}

/// Top level error type returned by sandbox entry points.
#[derive(Debug)]
pub enum ShaderError {
    /// Sandbox is off. Caller should present the shader as a
    /// missing feature, not a hard failure.
    Disabled,
    /// The shader file could not be read.
    Io(String),
    /// DSL parse or semantic check failed.
    Dsl(String),
    /// glslang invocation failed or produced a nonzero exit.
    GlslangFailed(String),
    /// SPIR-V validation rejected the module.
    Spirv(String),
    /// The shader is in the persistent quarantine file.
    Quarantined(String),
    /// Vulkan returned an error while building the pipeline.
    Vulkan(vk::Result),
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShaderError::Disabled =>
                write!(f, "shader sandbox is disabled"),
            ShaderError::Io(s) =>
                write!(f, "io: {}", s),
            ShaderError::Dsl(s) =>
                write!(f, "dsl: {}", s),
            ShaderError::GlslangFailed(s) =>
                write!(f, "glslang: {}", s),
            ShaderError::Spirv(s) =>
                write!(f, "spirv: {}", s),
            ShaderError::Quarantined(s) =>
                write!(f, "shader is quarantined: {}", s),
            ShaderError::Vulkan(r) =>
                write!(f, "vulkan: {:?}", r),
        }
    }
}

/// Public entry point. Owns the quarantine state and the
/// current mode. One instance lives for the lifetime of the
/// process. The main loop constructs it once after reading
/// the config.
pub struct ShaderSandbox {
    mode:       SandboxMode,
    quarantine: Quarantine,
    /// Identifier of the user shader most recently bound for
    /// rendering. Used by `report_device_lost` to decide which
    /// shader to blame for a lost device. Cleared whenever the
    /// renderer switches back to the default post pipeline.
    active:     Option<String>,
}

impl ShaderSandbox {
    /// Build a fresh sandbox with `mode` as the starting
    /// policy. The quarantine file is loaded from disk; on
    /// first run it does not exist and the list starts empty.
    pub fn new(mode: SandboxMode) -> Self {
        ShaderSandbox {
            mode,
            quarantine: Quarantine::load(),
            active: None,
        }
    }

    pub fn mode(&self) -> SandboxMode { self.mode }

    /// Change the mode at runtime. The caller is responsible
    /// for reverting any active user pipeline to the default
    /// before switching away from `On`.
    pub fn set_mode(&mut self, mode: SandboxMode) { self.mode = mode; }

    /// Load one shader source file, produce SPIR-V, validate
    /// it, and wrap it in a ready to bind Vulkan pipeline.
    /// This is the one stop entry the renderer calls.
    ///
    /// `render_pass` must be the render pass the main post
    /// pipeline was built against. `set_layout` is the
    /// descriptor set layout the main post pipeline uses, so
    /// a user shader observes the same scene sampler and
    /// push constants.
    ///
    /// The returned `UserPipeline` owns its pipeline object
    /// and is destroyed through `UserPipeline::destroy`. The
    /// sandbox does not keep a reference.
    pub fn load_file<P: AsRef<Path>>(
        &mut self,
        instance:        &Instance,
        device:          &Device,
        physical_device: vk::PhysicalDevice,
        render_pass:     vk::RenderPass,
        set_layout:      vk::DescriptorSetLayout,
        source_path:     P,
    ) -> Result<UserPipeline, ShaderError> {
        let _ = (instance, physical_device);

        if matches!(self.mode, SandboxMode::Off) {
            return Err(ShaderError::Disabled);
        }

        let path: PathBuf = source_path.as_ref().to_path_buf();
        let id = shader_id_for_path(&path);

        if self.quarantine.contains(&id) {
            return Err(ShaderError::Quarantined(id));
        }

        let source = read_shader_file(path.as_path())
            .map_err(ShaderError::Io)?;

        // Layer 1: DSL parse and semantic checks.
        let ast = dsl::parse(&source)
            .map_err(ShaderError::Dsl)?;
        let glsl = codegen::emit_glsl(&ast)
            .map_err(ShaderError::Dsl)?;

        // Layer 2a: hand the GLSL to glslang and read SPIR-V.
        let spirv = compile::compile_glsl_to_spirv(&glsl)
            .map_err(ShaderError::GlslangFailed)?;

        // Layer 2b: walk the SPIR-V and reject anything the
        // whitelist does not explicitly allow.
        spirv::validate(&spirv, &spirv::default_policy())
            .map_err(ShaderError::Spirv)?;

        if matches!(self.mode, SandboxMode::Audit) {
            return Err(ShaderError::Disabled);
        }

        // Build the Vulkan pipeline. Still returns an error
        // for out of memory and the like, no GPU code runs
        // until the renderer binds it.
        let pipe = pipeline::build(
            device, render_pass, set_layout, &spirv, id.clone())
            .map_err(ShaderError::Vulkan)?;

        Ok(pipe)
    }

    /// Notify the sandbox that the renderer is about to issue
    /// draw commands using `id`. Call this every time the
    /// user shader becomes the active post pipeline. Cleared
    /// with `mark_inactive` when the renderer swaps back to
    /// the default.
    pub fn mark_active(&mut self, id: &str) {
        self.active = Some(id.to_string());
    }

    pub fn mark_inactive(&mut self) { self.active = None; }

    /// Report a Vulkan device loss. If a user shader was
    /// flagged as active through `mark_active` it is added
    /// to the persistent quarantine and no longer loadable
    /// without manual intervention. The caller is still
    /// responsible for recreating the device; this function
    /// only records the attribution.
    pub fn report_device_lost(&mut self) {
        if let Some(id) = self.active.take() {
            self.quarantine.insert(&id);
        }
    }

    /// Inspect the persistent quarantine list. Useful for the
    /// settings UI that may expose a "clear quarantine"
    /// action to the user.
    pub fn quarantined(&self) -> &[String] {
        self.quarantine.entries()
    }

    /// Forget every quarantine entry and persist the change.
    /// The user triggers this explicitly from the settings
    /// screen after an author has confirmed their shader is
    /// safe to load again.
    pub fn clear_quarantine(&mut self) {
        self.quarantine.clear();
    }
}

/// Derive a stable identifier from a shader source path. The
/// quarantine uses the file stem (without extension and
/// without directory) so renaming the directory layout does
/// not invalidate recorded misbehavior.
fn shader_id_for_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Read a shader source file with the same relative-first,
/// exe-relative fallback convention used by the rest of the
/// engine's asset loaders (see `music.rs` and
/// `levels/mod.rs`).
///
/// Why bother: when the game is launched by double-click
/// on Windows, the current working directory is whatever
/// the user's shell happened to be in, which is almost
/// never the game's install folder. Every other asset
/// loader (levels, music, textures) transparently retries
/// against `current_exe().parent()`; shaders must do the
/// same or they fail to load on any launch that is not
/// `cargo run` from the crate root.
///
/// The error returned on a miss includes both attempted
/// paths so an author can tell whether they mistyped the
/// path in the .rlf or forgot to ship the shader next to
/// the executable.
fn read_shader_file(path: &std::path::Path) -> Result<String, String> {
    // Absolute paths are used verbatim.
    if path.is_absolute() {
        return std::fs::read_to_string(path)
            .map_err(|e| format!(
                "cannot read {}: {}", path.display(), e));
    }

    // First attempt: path as given, resolved against CWD.
    // This is what `cargo run` from the project root sees.
    let err_cwd = match std::fs::read_to_string(path) {
        Ok(s) => return Ok(s),
        Err(e) => e.to_string(),
    };

    // Second attempt: same relative path, but rooted at
    // the directory of the running executable. This is
    // what a double-click launch sees on Windows and what
    // a packaged install relies on.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(path);
            match std::fs::read_to_string(&candidate) {
                Ok(s) => return Ok(s),
                Err(e) => {
                    return Err(format!(
                        "cannot read shader: tried '{}' ({}), \
                         and '{}' ({})",
                        path.display(), err_cwd,
                        candidate.display(), e));
                }
            }
        }
    }

    Err(format!(
        "cannot read shader '{}': {}", path.display(), err_cwd))
}