//! SPIR-V post compile validator.
//!
//! The validator is the single most important layer in the
//! sandbox. Even if the DSL front end were compromised or
//! the user bypassed it entirely and dropped in a raw SPIR-V
//! blob, this file decides whether a module is accepted by
//! the renderer. It must therefore read every word of the
//! blob and make no assumptions about structure beyond what
//! the SPIR-V specification requires.
//!
//! The parser is intentionally minimal. It reads the header,
//! then walks instructions looking at:
//!
//! * `OpCapability` (opcode 17). Every declared capability
//!   must be in the whitelist. The default policy allows
//!   only `Shader` and `Matrix`, which is enough for a
//!   fragment stage that reads a sampler and does arithmetic.
//! * `OpLoopMerge` (opcode 246). Structured loops require
//!   this instruction at the header of their merge block.
//!   Rejecting it forbids `while`, `for`, and `do`, which
//!   closes the runaway loop attack vector entirely.
//! * `OpTypePointer` (opcode 32) for storage class checks.
//!   If the pointer uses `StorageBuffer` (class 12), an
//!   SSBO has been declared and the module is rejected.
//! * `OpVariable` (opcode 59) for the same reason, to catch
//!   variables declared directly in `StorageBuffer` class.
//! * `OpDecorate` (opcode 71) and `OpMemberDecorate` (72).
//!   Any decoration that introduces buffer block layout
//!   (`BufferBlock`, decoration 3) is rejected so SSBOs
//!   cannot sneak in via the legacy pre SPIR-V 1.3 shape.
//! * `OpTypeRuntimeArray` (opcode 29). Runtime sized arrays
//!   only exist inside storage blocks; a legitimate fragment
//!   shader for post processing never needs them.
//!
//! Instruction count is accumulated as a backstop against a
//! shader that is formally well formed but expensive. The
//! default budget is 4096 instructions, which is roughly ten
//! times what the engine's own `post.frag` compiles to, so
//! legitimate user shaders have plenty of headroom while
//! runaway templates are caught.
//!
//! The validator never executes code and never touches the
//! Vulkan device. It operates purely on the binary blob,
//! which makes it safe to run during audit mode even when
//! the renderer is disabled.

use std::collections::HashSet;

/// SPIR-V magic number as stored in the first word of a
/// little endian module.
const SPIRV_MAGIC: u32 = 0x0723_0203;
/// Upper bound on SPIR-V version (major.minor packed in
/// bytes 2 and 1). 0x00010600 is SPIR-V 1.6.
const SPIRV_MAX_VERSION: u32 = 0x0001_0600;

// ---------- opcodes ----------

const OP_CAPABILITY:        u16 = 17;
const OP_EXTENSION:         u16 = 10;
const OP_EXT_INST_IMPORT:   u16 = 11;
const OP_MEMORY_MODEL:      u16 = 14;
const OP_ENTRY_POINT:       u16 = 15;
const OP_EXECUTION_MODE:    u16 = 16;
const OP_TYPE_RUNTIME_ARR:  u16 = 29;
const OP_TYPE_POINTER:      u16 = 32;
const OP_VARIABLE:          u16 = 59;
const OP_DECORATE:          u16 = 71;
const OP_MEMBER_DECORATE:   u16 = 72;
const OP_LOOP_MERGE:        u16 = 246;

// ---------- capabilities (enum Capability in spec) ----------

const CAPABILITY_MATRIX: u32 = 0;
const CAPABILITY_SHADER: u32 = 1;

// ---------- storage classes (enum StorageClass in spec) --

const STORAGE_CLASS_STORAGE_BUFFER: u32 = 12;

// ---------- decorations (enum Decoration in spec) --------

const DECORATION_BUFFER_BLOCK: u32 = 3;

/// Validation policy. Fields are public so higher layers can
/// tweak limits, for example the renderer could raise the
/// instruction budget for a shader that benchmarks well, or
/// extend the capability whitelist for a future feature.
#[derive(Clone, Debug)]
pub struct Policy {
    pub allowed_capabilities: HashSet<u32>,
    pub max_instructions:     u32,
    pub max_extensions:       u32,
    pub allow_loop_merge:     bool,
    pub allow_runtime_array:  bool,
    pub allow_storage_buffer: bool,
    pub allow_buffer_block:   bool,
}

/// Default policy for post process user shaders. Suitable
/// for the current engine and rejects every category of
/// risky construct discussed in the module header.
pub fn default_policy() -> Policy {
    let mut caps = HashSet::new();
    caps.insert(CAPABILITY_SHADER);
    caps.insert(CAPABILITY_MATRIX);
    Policy {
        allowed_capabilities: caps,
        max_instructions:     4096,
        max_extensions:       4,
        allow_loop_merge:     false,
        allow_runtime_array:  false,
        allow_storage_buffer: false,
        allow_buffer_block:   false,
    }
}

/// Walk `blob` and return `Ok(())` when every policy rule
/// is satisfied. On failure the error carries a short
/// human readable reason, intended to surface in the shader
/// load log and the failed shader report.
pub fn validate(blob: &[u8], policy: &Policy) -> Result<(), String> {
    let words = decode_words(blob)?;
    validate_header(&words)?;

    let mut instruction_count: u32 = 0;
    let mut extension_count:   u32 = 0;

    let mut i = 5usize;
    while i < words.len() {
        instruction_count = instruction_count.saturating_add(1);
        if instruction_count > policy.max_instructions {
            return Err(format!(
                "instruction budget exceeded ({})",
                policy.max_instructions));
        }

        let header = words[i];
        let word_count = (header >> 16) as usize;
        let opcode = (header & 0xFFFF) as u16;

        if word_count == 0 {
            return Err(format!(
                "zero word instruction at word offset {}", i));
        }
        if i + word_count > words.len() {
            return Err(format!(
                "truncated instruction at word offset {}", i));
        }

        let operands = &words[i + 1 .. i + word_count];

        match opcode {
            OP_CAPABILITY => {
                let cap = *operands.first().ok_or_else(||
                    "OpCapability without operand".to_string())?;
                if !policy.allowed_capabilities.contains(&cap) {
                    return Err(format!(
                        "capability {} not allowed", cap));
                }
            }

            OP_EXTENSION | OP_EXT_INST_IMPORT => {
                extension_count = extension_count.saturating_add(1);
                if extension_count > policy.max_extensions {
                    return Err(format!(
                        "too many extensions (limit {})",
                        policy.max_extensions));
                }
            }

            OP_LOOP_MERGE => {
                if !policy.allow_loop_merge {
                    return Err(
                        "OpLoopMerge is forbidden, loops are not allowed"
                        .to_string());
                }
            }

            OP_TYPE_RUNTIME_ARR => {
                if !policy.allow_runtime_array {
                    return Err(
                        "runtime sized arrays are not allowed"
                        .to_string());
                }
            }

            OP_TYPE_POINTER => {
                // operands: result_id, storage_class, type_id.
                if let Some(&sc) = operands.get(1) {
                    reject_storage_class(sc, policy)?;
                }
            }

            OP_VARIABLE => {
                // operands: result_type_id, result_id,
                //           storage_class, [initializer].
                if let Some(&sc) = operands.get(2) {
                    reject_storage_class(sc, policy)?;
                }
            }

            OP_DECORATE => {
                // operands: target_id, decoration, [extra].
                if let Some(&dec) = operands.get(1) {
                    reject_decoration(dec, policy)?;
                }
            }

            OP_MEMBER_DECORATE => {
                // operands: structure_type, member, decoration,
                //           [extra].
                if let Some(&dec) = operands.get(2) {
                    reject_decoration(dec, policy)?;
                }
            }

            // Every other opcode is permitted. We trust
            // glslang to emit syntactically correct modules
            // and rely on the Vulkan driver's own validation
            // for semantic wellformedness.
            _ => {}
        }

        i += word_count;
    }

    if i != words.len() {
        return Err(format!(
            "trailing bytes after last instruction at word {}", i));
    }

    Ok(())
}

/// Split the byte blob into u32 words. SPIR-V is always a
/// whole number of 32 bit words and the reference tooling
/// always emits little endian. A file that is big endian or
/// whose length is not a multiple of four is rejected.
fn decode_words(blob: &[u8]) -> Result<Vec<u32>, String> {
    if blob.len() < 20 {
        return Err("blob too small to be a SPIR-V module".into());
    }
    if blob.len() % 4 != 0 {
        return Err("blob length is not a multiple of four".into());
    }
    let mut out = Vec::with_capacity(blob.len() / 4);
    for chunk in blob.chunks_exact(4) {
        out.push(u32::from_le_bytes(
            [chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

/// Verify the five word SPIR-V header: magic, version,
/// generator, bound, schema. Only magic and version carry
/// meaning for validation, the rest are informational.
fn validate_header(words: &[u32]) -> Result<(), String> {
    if words[0] != SPIRV_MAGIC {
        return Err(format!(
            "bad magic 0x{:08X}, expected 0x{:08X}",
            words[0], SPIRV_MAGIC));
    }
    if words[1] > SPIRV_MAX_VERSION {
        return Err(format!(
            "unsupported SPIR-V version 0x{:08X}", words[1]));
    }
    Ok(())
}

fn reject_storage_class(sc: u32, policy: &Policy) -> Result<(), String> {
    if sc == STORAGE_CLASS_STORAGE_BUFFER && !policy.allow_storage_buffer {
        return Err("StorageBuffer storage class is not allowed".into());
    }
    Ok(())
}

fn reject_decoration(dec: u32, policy: &Policy) -> Result<(), String> {
    if dec == DECORATION_BUFFER_BLOCK && !policy.allow_buffer_block {
        return Err("BufferBlock decoration is not allowed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal blob that holds only the SPIR-V
    /// header, so we can test header validation without
    /// pulling in a real shader.
    fn make_header(version: u32) -> Vec<u8> {
        let words: [u32; 5] = [SPIRV_MAGIC, version, 0, 1, 0];
        let mut out = Vec::with_capacity(20);
        for w in words { out.extend_from_slice(&w.to_le_bytes()); }
        out
    }

    #[test]
    fn header_is_accepted() {
        let b = make_header(0x00010000);
        assert!(validate(&b, &default_policy()).is_ok());
    }

    #[test]
    fn header_bad_magic_rejected() {
        let mut b = make_header(0x00010000);
        b[0] = 0;
        assert!(validate(&b, &default_policy()).is_err());
    }

    #[test]
    fn header_wrong_length_rejected() {
        let mut b = make_header(0x00010000);
        b.pop();
        assert!(validate(&b, &default_policy()).is_err());
    }

    /// Synthesize one instruction with the given opcode and
    /// operands and append it to a header only blob.
    fn with_instruction(opcode: u16, operands: &[u32]) -> Vec<u8> {
        let mut b = make_header(0x00010000);
        let word_count = 1 + operands.len() as u32;
        let header = (word_count << 16) | opcode as u32;
        b.extend_from_slice(&header.to_le_bytes());
        for &o in operands {
            b.extend_from_slice(&o.to_le_bytes());
        }
        b
    }

    #[test]
    fn loop_merge_rejected() {
        let b = with_instruction(OP_LOOP_MERGE, &[1, 2, 0]);
        assert!(validate(&b, &default_policy()).is_err());
    }

    #[test]
    fn storage_buffer_pointer_rejected() {
        let b = with_instruction(
            OP_TYPE_POINTER,
            &[1, STORAGE_CLASS_STORAGE_BUFFER, 2]);
        assert!(validate(&b, &default_policy()).is_err());
    }

    #[test]
    fn buffer_block_decoration_rejected() {
        let b = with_instruction(
            OP_DECORATE, &[1, DECORATION_BUFFER_BLOCK]);
        assert!(validate(&b, &default_policy()).is_err());
    }

    #[test]
    fn unknown_capability_rejected() {
        let b = with_instruction(OP_CAPABILITY, &[9999]);
        assert!(validate(&b, &default_policy()).is_err());
    }

    #[test]
    fn allowed_capability_accepted() {
        let b = with_instruction(OP_CAPABILITY, &[CAPABILITY_SHADER]);
        assert!(validate(&b, &default_policy()).is_ok());
    }
}