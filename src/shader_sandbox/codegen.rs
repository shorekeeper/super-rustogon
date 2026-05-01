//! GLSL code generator for the user shader DSL.
//!
//! The output is a complete GLSL fragment shader source
//! string, ready to feed to `glslangValidator`. The shape of
//! the generated source is fixed, the only author controlled
//! portions are:
//!
//! * The list of fields inside the `Params` push constant
//!   block, one per DSL `param` declaration.
//! * The `let` bindings and the `output` expression,
//!   inlined into `main`.
//!
//! Everything else, including the uniform sampler binding,
//! the fullscreen varyings, the `sample_scene` helper, and
//! the final assignment to `out_color`, is emitted verbatim
//! so the Vulkan side of the pipeline matches the built in
//! post pass layout.

use crate::shader_sandbox::dsl::{
    BinOp, Body, Expr, LetBinding, MAX_SAMPLE_CALLS, Param, Shader,
    ShaderType, UnOp,
};

/// Emit GLSL source for `shader`. Returns a human readable
/// error when the DSL semantic checks fail, for example when
/// the body uses more texture samples than the per pixel
/// budget allows or references an unknown builtin.
pub fn emit_glsl(shader: &Shader) -> Result<String, String> {
    let sample_calls = count_sample_calls_in_body(&shader.body);
    if sample_calls > MAX_SAMPLE_CALLS {
        return Err(format!(
            "shader uses {} sample() calls per pixel (limit {})",
            sample_calls, MAX_SAMPLE_CALLS));
    }

    let mut out = String::new();
    out.push_str("#version 450\n");
    out.push_str("layout(location = 0) in vec2 v_uv;\n");
    out.push_str("layout(location = 0) out vec4 out_color;\n");
    out.push_str(
        "layout(set = 0, binding = 0) uniform sampler2D u_scene;\n");

    // Push constant block. The runtime side of the pipeline
    // is responsible for writing these fields from a Rust
    // struct that matches the declared parameter order.
    out.push_str("layout(push_constant) uniform Params {\n");
    for p in &shader.params {
        out.push_str(&format!("    {} {};\n", p.ty, p.name));
    }
    if shader.params.is_empty() {
        out.push_str("    float _unused;\n");
    }
    out.push_str("} pc;\n");

    out.push_str("vec4 sample_scene(vec2 uv) {\n");
    out.push_str("    return texture(u_scene, uv);\n");
    out.push_str("}\n");

    out.push_str("void main() {\n");
    out.push_str("    vec2 uv = v_uv;\n");
    for b in &shader.body.lets {
        check_expr(&b.expr, shader)?;
        out.push_str(&format!(
            "    float {} = {};\n", b.name, emit_expr(&b.expr, shader)));
    }
    check_expr(&shader.body.output, shader)?;
    let output_str = emit_expr(&shader.body.output, shader);
    if returns_vec4(&shader.body.output) {
        out.push_str(&format!(
            "    out_color = {};\n", output_str));
    } else {
        out.push_str(&format!(
            "    out_color = vec4({});\n", output_str));
    }
    out.push_str("}\n");

    Ok(out)
}

/// True when `e` is a call whose return type is already
/// `vec4`. The output expression is wrapped in `vec4(...)`
/// only when this is false, so the generated code never
/// ends up with a nested `vec4(vec4(...))` that glslang
/// would reject.
fn returns_vec4(e: &Expr) -> bool {
    match e {
        Expr::Call { name, .. } =>
            matches!(name.as_str(), "sample" | "vec4"),
        _ => false,
    }
}

fn count_sample_calls_in_body(body: &Body) -> usize {
    let mut n = 0;
    for b in &body.lets { n += count_sample_calls(&b.expr); }
    n += count_sample_calls(&body.output);
    n
}

fn count_sample_calls(e: &Expr) -> usize {
    match e {
        Expr::Num(_) | Expr::Var(_) => 0,
        Expr::Un(_, a) => count_sample_calls(a),
        Expr::Bin(_, a, b) =>
            count_sample_calls(a) + count_sample_calls(b),
        Expr::Call { name, args } => {
            let here = if name == "sample" { 1 } else { 0 };
            here + args.iter().map(count_sample_calls).sum::<usize>()
        }
    }
}

/// Validate that an expression only references names the
/// DSL knows how to emit: either a declared parameter, a
/// previously bound let, or one of the builtin vocabulary
/// entries. Keeping this check here rather than in the
/// parser lets us produce an error that explains which name
/// was wrong and why.
fn check_expr(e: &Expr, shader: &Shader) -> Result<(), String> {
    match e {
        Expr::Num(_) => Ok(()),
        Expr::Var(name) => {
            if is_shader_local(shader, name) {
                Ok(())
            } else {
                Err(format!("unknown variable '{}'", name))
            }
        }
        Expr::Un(_, a) => check_expr(a, shader),
        Expr::Bin(_, a, b) => {
            check_expr(a, shader)?;
            check_expr(b, shader)
        }
        Expr::Call { name, args } => {
            if !is_builtin(name) {
                return Err(format!("unknown function '{}'", name));
            }
            for a in args { check_expr(a, shader)?; }
            Ok(())
        }
    }
}

fn is_shader_local(shader: &Shader, name: &str) -> bool {
    name == "uv"
        || shader.params.iter().any(|p| p.name == name)
        || shader.body.lets.iter().any(|b| b.name == name)
}

fn is_builtin(name: &str) -> bool {
    matches!(name,
        "sample" | "length" | "sin" | "cos" | "abs"
        | "min" | "max" | "clamp" | "mix" | "smoothstep"
        | "pow" | "vec2" | "vec3" | "vec4"
        | "dot" | "fract" | "floor" | "ceil")
}

/// Emit a scalar valued GLSL expression for `e`. Operations
/// on vectors in the DSL still produce a scalar at the call
/// site, which constrains the language but keeps the
/// generated code uniform and short. Vector component
/// extraction is expressed by passing the full vector to a
/// scalar returning builtin like `length` or `dot`.
fn emit_expr(e: &Expr, shader: &Shader) -> String {
    match e {
        Expr::Num(n)   => format!("{}", n),
        Expr::Var(v)   => {
            if shader.params.iter().any(|p| p.name == *v) {
                format!("pc.{}", v)
            } else {
                v.clone()
            }
        }
        Expr::Un(op, a) => {
            let s = emit_expr(a, shader);
            match op { UnOp::Neg => format!("(-{})", s) }
        }
        Expr::Bin(op, a, b) => {
            let sa = emit_expr(a, shader);
            let sb = emit_expr(b, shader);
            let sym = match op {
                BinOp::Add => "+", BinOp::Sub => "-",
                BinOp::Mul => "*", BinOp::Div => "/",
                BinOp::Lt  => "<", BinOp::Le  => "<=",
                BinOp::Gt  => ">", BinOp::Ge  => ">=",
                BinOp::Eq  => "==", BinOp::Ne => "!=",
            };
            format!("({} {} {})", sa, sym, sb)
        }
        Expr::Call { name, args } => {
            // `sample` is the author facing name for the scene
            // texture read. In GLSL 4.5+ the bare identifier
            // `sample` is reserved as an interpolation qualifier
            // and glslangValidator rejects it in any other
            // context, so we rewrite the call to the helper
            // `sample_scene` that the fixed prelude declares at
            // the top of every generated shader. The AST keeps
            // the name `sample` verbatim, which lets the static
            // budget checks (see `count_sample_calls`) and the
            // `returns_vec4` heuristic continue to match on the
            // unchanged DSL spelling.
            let emitted_name = if name == "sample" {
                "sample_scene"
            } else {
                name.as_str()
            };
            let parts: Vec<String> =
                args.iter().map(|a| emit_expr(a, shader)).collect();
            format!("{}({})", emitted_name, parts.join(", "))
        }
    }
}


/// Helper for the runtime push constant writer. Returns the
/// byte offsets of each uniform parameter inside the push
/// block, assuming the std430 layout rules glslang uses for
/// `push_constant` buffers. The runtime pipeline writes a
/// byte buffer of the right shape before every draw.
pub fn param_offsets(params: &[Param]) -> Vec<(String, usize, ShaderType)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    for p in params {
        let size = match p.ty {
            ShaderType::Float => 4,
            ShaderType::Vec2  => 8,
            ShaderType::Vec3  => 16,
            ShaderType::Vec4  => 16,
        };
        let align = match p.ty {
            ShaderType::Float => 4,
            ShaderType::Vec2  => 8,
            ShaderType::Vec3  => 16,
            ShaderType::Vec4  => 16,
        };
        off = (off + align - 1) & !(align - 1);
        out.push((p.name.clone(), off, p.ty));
        off += size;
    }
    out
}