//! Tiny DSL for user authored post process shaders.
//!
//! The grammar is deliberately flat and expression oriented.
//! An author declares a name, a list of uniform parameters
//! with default values, and a body made of `let` bindings
//! plus a single `output` expression. There are no loops,
//! no branches, no user defined functions, no array indexing
//! with a variable, and no state that persists between
//! pixels. Everything the author writes is inlined into a
//! single straight line GLSL fragment by `codegen.rs`.
//!
//! Grammar, informal:
//!
//!     shader "<name>" {
//!         param <id> : <type> = <literal>
//!         ...
//!         body {
//!             let <id> = <expr>
//!             ...
//!             output <expr>
//!         }
//!     }
//!
//! Types: `float`, `vec2`, `vec3`, `vec4`.
//! Literals: numbers and constructor calls like `vec2(0.1, 0.2)`.
//! Expressions: arithmetic (`+ - * /`), comparison produces
//! float 0.0 or 1.0 (no branching), calls to a fixed builtin
//! list: `sample`, `length`, `sin`, `cos`, `abs`, `min`,
//! `max`, `clamp`, `mix`, `smoothstep`, `pow`, `vec2`,
//! `vec3`, `vec4`, `dot`, `fract`, `floor`, `ceil`.
//!
//! The `sample` builtin takes a `vec2` uv and returns the
//! scene color at that texel. The code generator counts
//! `sample` calls statically and rejects bodies that exceed
//! the per pixel sample budget; an infinite number of
//! samples is how a formally well structured shader ends up
//! reading the entire frame into one pixel and tanking the
//! frame rate.

use std::fmt;

/// Limit on the number of `let` bindings per body. Kept
/// small so a compiled shader stays short enough to be
/// readable and well under the SPIR-V instruction budget.
pub const MAX_LET_BINDINGS: usize = 32;

/// Hard upper bound on `sample` calls per pixel.
pub const MAX_SAMPLE_CALLS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderType { Float, Vec2, Vec3, Vec4 }

impl fmt::Display for ShaderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ShaderType::Float => "float",
            ShaderType::Vec2  => "vec2",
            ShaderType::Vec3  => "vec3",
            ShaderType::Vec4  => "vec4",
        })
    }
}

/// Uniform parameter declared by the author. Translated to a
/// field of the user shader's push constant block.
#[derive(Clone, Debug)]
pub struct Param {
    pub name:    String,
    pub ty:      ShaderType,
    pub default: Vec<f32>,
}

/// Parsed shader program.
#[derive(Clone, Debug)]
pub struct Shader {
    pub name:   String,
    pub params: Vec<Param>,
    pub body:   Body,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub lets:   Vec<LetBinding>,
    pub output: Expr,
}

#[derive(Clone, Debug)]
pub struct LetBinding {
    pub name: String,
    pub expr: Expr,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f32),
    Var(String),
    Un(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call { name: String, args: Vec<Expr> },
}

#[derive(Clone, Copy, Debug)]
pub enum UnOp { Neg }

#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Add, Sub, Mul, Div,
    Lt, Le, Gt, Ge, Eq, Ne,
}

/// Parse a complete shader program. The returned AST is
/// ready for `codegen::emit_glsl`.
pub fn parse(src: &str) -> Result<Shader, String> {
    let mut p = Parser::new(src);
    p.skip_ws();
    p.expect_keyword("shader")?;
    p.skip_ws();
    let name = p.read_string()?;
    p.skip_ws();
    p.expect_char('{')?;

    let mut params: Vec<Param> = Vec::new();
    let mut body: Option<Body> = None;

    loop {
        p.skip_ws();
        if p.peek_char('}') { break; }
        if p.peek_keyword("param") {
            p.bump_word();
            params.push(p.parse_param()?);
        } else if p.peek_keyword("body") {
            p.bump_word();
            p.skip_ws();
            p.expect_char('{')?;
            body = Some(p.parse_body()?);
            p.skip_ws();
            p.expect_char('}')?;
        } else {
            return Err(format!(
                "unexpected token at byte {}", p.pos));
        }
    }

    p.skip_ws();
    p.expect_char('}')?;

    let body = body.ok_or_else(||
        "shader has no body block".to_string())?;

    let shader = Shader { name, params, body };
    validate_shader(&shader)?;
    Ok(shader)
}

// ---------- parser ----------

struct Parser<'a> {
    bytes: &'a [u8],
    pos:   usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser { bytes: src.as_bytes(), pos: 0 }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else if c == b'/' && self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos + 1] == b'/'
            {
                while self.pos < self.bytes.len()
                    && self.bytes[self.pos] != b'\n'
                { self.pos += 1; }
            } else {
                break;
            }
        }
    }

    fn peek_char(&self, c: char) -> bool {
        self.pos < self.bytes.len() && self.bytes[self.pos] == c as u8
    }

    fn expect_char(&mut self, c: char) -> Result<(), String> {
        if self.peek_char(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c, self.pos))
        }
    }

    fn peek_keyword(&self, kw: &str) -> bool {
        let bytes = kw.as_bytes();
        if self.pos + bytes.len() > self.bytes.len() {
            return false;
        }
        if &self.bytes[self.pos .. self.pos + bytes.len()] != bytes {
            return false;
        }
        let after = self.pos + bytes.len();
        if after < self.bytes.len() {
            let c = self.bytes[after];
            if c.is_ascii_alphanumeric() || c == b'_' {
                return false;
            }
        }
        true
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), String> {
        if self.peek_keyword(kw) {
            self.pos += kw.len();
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", kw, self.pos))
        }
    }

    fn bump_word(&mut self) {
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_alphanumeric()
                || self.bytes[self.pos] == b'_')
        { self.pos += 1; }
    }

    fn read_ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        if self.pos >= self.bytes.len()
            || !(self.bytes[self.pos].is_ascii_alphabetic()
                 || self.bytes[self.pos] == b'_')
        {
            return Err(format!("expected identifier at byte {}", self.pos));
        }
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_alphanumeric()
                || self.bytes[self.pos] == b'_')
        { self.pos += 1; }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.pos]).into())
    }

    fn read_string(&mut self) -> Result<String, String> {
        self.skip_ws();
        if !self.peek_char('"') {
            return Err(format!("expected string at byte {}", self.pos));
        }
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            return Err("unterminated string".into());
        }
        let s = String::from_utf8_lossy(
            &self.bytes[start..self.pos]).to_string();
        self.pos += 1;
        Ok(s)
    }

    fn read_number(&mut self) -> Result<f32, String> {
        self.skip_ws();
        let start = self.pos;
        if self.peek_char('-') { self.pos += 1; }
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_digit()
                || self.bytes[self.pos] == b'.')
        { self.pos += 1; }
        let slice = &self.bytes[start..self.pos];
        if slice.is_empty() || slice == b"-" {
            return Err(format!("expected number at byte {}", start));
        }
        let s = std::str::from_utf8(slice).map_err(|e| e.to_string())?;
        s.parse::<f32>().map_err(|_|
            format!("bad number '{}' at byte {}", s, start))
    }

    fn parse_param(&mut self) -> Result<Param, String> {
        let name = self.read_ident()?;
        self.skip_ws();
        self.expect_char(':')?;
        let ty_s = self.read_ident()?;
        let ty = match ty_s.as_str() {
            "float" => ShaderType::Float,
            "vec2"  => ShaderType::Vec2,
            "vec3"  => ShaderType::Vec3,
            "vec4"  => ShaderType::Vec4,
            _ => return Err(format!("unknown type '{}'", ty_s)),
        };
        self.skip_ws();
        self.expect_char('=')?;
        let default = self.parse_literal_list(ty)?;
        Ok(Param { name, ty, default })
    }

    fn parse_literal_list(&mut self, ty: ShaderType) -> Result<Vec<f32>, String> {
        self.skip_ws();
        // Either one literal (for float) or constructor call.
        if self.peek_keyword("vec2") || self.peek_keyword("vec3")
            || self.peek_keyword("vec4")
        {
            let ctor = self.read_ident()?;
            self.skip_ws();
            self.expect_char('(')?;
            let mut out = Vec::new();
            loop {
                out.push(self.read_number()?);
                self.skip_ws();
                if self.peek_char(',') { self.pos += 1; continue; }
                if self.peek_char(')') { self.pos += 1; break; }
                return Err(format!(
                    "expected ',' or ')' at byte {}", self.pos));
            }
            let expected = match ctor.as_str() {
                "vec2" => 2, "vec3" => 3, "vec4" => 4, _ => 0,
            };
            if out.len() != expected {
                return Err(format!(
                    "{} expects {} args, got {}", ctor, expected, out.len()));
            }
            let _ = ty;
            Ok(out)
        } else {
            Ok(vec![self.read_number()?])
        }
    }

    fn parse_body(&mut self) -> Result<Body, String> {
        let mut lets = Vec::new();
        let mut output = None;
        loop {
            self.skip_ws();
            if self.peek_char('}') { break; }
            if self.peek_keyword("let") {
                self.bump_word();
                let name = self.read_ident()?;
                self.skip_ws();
                self.expect_char('=')?;
                let expr = self.parse_expr()?;
                lets.push(LetBinding { name, expr });
                if lets.len() > MAX_LET_BINDINGS {
                    return Err(format!(
                        "too many let bindings (limit {})",
                        MAX_LET_BINDINGS));
                }
            } else if self.peek_keyword("output") {
                self.bump_word();
                output = Some(self.parse_expr()?);
            } else {
                return Err(format!(
                    "unexpected token at byte {}", self.pos));
            }
        }
        let output = output.ok_or_else(||
            "body has no output expression".to_string())?;
        Ok(Body { lets, output })
    }

    // Pratt style expression parser. Precedence levels:
    //  1: or (not supported here, reserved)
    //  2: and (reserved)
    //  3: comparison (< <= > >= == !=)
    //  4: additive (+ -)
    //  5: multiplicative (* /)
    //  6: unary (-)
    //  7: primary (literal, var, call, parenthesized)

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add()?;
        loop {
            self.skip_ws();
            let op =
                if self.peek_str("<=") { self.pos += 2; Some(BinOp::Le) }
                else if self.peek_str(">=") { self.pos += 2; Some(BinOp::Ge) }
                else if self.peek_str("==") { self.pos += 2; Some(BinOp::Eq) }
                else if self.peek_str("!=") { self.pos += 2; Some(BinOp::Ne) }
                else if self.peek_char('<') { self.pos += 1; Some(BinOp::Lt) }
                else if self.peek_char('>') { self.pos += 1; Some(BinOp::Gt) }
                else { None };
            if let Some(op) = op {
                let right = self.parse_add()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn peek_str(&self, s: &str) -> bool {
        let b = s.as_bytes();
        self.pos + b.len() <= self.bytes.len()
            && &self.bytes[self.pos..self.pos + b.len()] == b
    }

    fn parse_add(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul()?;
        loop {
            self.skip_ws();
            let op =
                if self.peek_char('+') { self.pos += 1; Some(BinOp::Add) }
                else if self.peek_char('-') { self.pos += 1; Some(BinOp::Sub) }
                else { None };
            if let Some(op) = op {
                let right = self.parse_mul()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op =
                if self.peek_char('*') { self.pos += 1; Some(BinOp::Mul) }
                else if self.peek_char('/') { self.pos += 1; Some(BinOp::Div) }
                else { None };
            if let Some(op) = op {
                let right = self.parse_unary()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        if self.peek_char('-') {
            self.pos += 1;
            let inner = self.parse_unary()?;
            Ok(Expr::Un(UnOp::Neg, Box::new(inner)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        if self.peek_char('(') {
            self.pos += 1;
            let e = self.parse_expr()?;
            self.skip_ws();
            self.expect_char(')')?;
            return Ok(e);
        }
        if self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            if c.is_ascii_digit() || c == b'.' {
                return Ok(Expr::Num(self.read_number()?));
            }
            if c.is_ascii_alphabetic() || c == b'_' {
                let name = self.read_ident()?;
                self.skip_ws();
                if self.peek_char('(') {
                    self.pos += 1;
                    let mut args = Vec::new();
                    self.skip_ws();
                    if !self.peek_char(')') {
                        loop {
                            args.push(self.parse_expr()?);
                            self.skip_ws();
                            if self.peek_char(',') { self.pos += 1; continue; }
                            break;
                        }
                    }
                    self.expect_char(')')?;
                    return Ok(Expr::Call { name, args });
                }
                return Ok(Expr::Var(name));
            }
        }
        Err(format!("unexpected character at byte {}", self.pos))
    }
}

/// Semantic check run after the parser has built a full
/// `Shader`. Enforces invariants that are awkward to express
/// in the recursive descent grammar:
///
/// * `sample` must only appear inside the output expression.
///   Using it in a let binding would widen the value to a
///   vec4, but let bindings are always scalar in the
///   generated GLSL. Restricting sample to the output leaves
///   the grammar consistent and the codegen trivial.
fn validate_shader(shader: &Shader) -> Result<(), String> {
    for b in &shader.body.lets {
        if contains_call(&b.expr, "sample") {
            return Err(format!(
                "'sample' is not allowed inside the let binding \
                 '{}'; call it from the output expression instead",
                b.name));
        }
    }
    Ok(())
}

fn contains_call(e: &Expr, target: &str) -> bool {
    match e {
        Expr::Num(_) | Expr::Var(_) => false,
        Expr::Un(_, a) => contains_call(a, target),
        Expr::Bin(_, a, b) =>
            contains_call(a, target) || contains_call(b, target),
        Expr::Call { name, args } => {
            if name == target { return true; }
            args.iter().any(|a| contains_call(a, target))
        }
    }
}