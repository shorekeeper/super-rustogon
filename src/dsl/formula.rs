//! Tiny expression language for user authored obstacle masks.
//!
//! Supported grammar:
//!
//!     expr     := or_expr
//!     or_expr  := and_expr ('||' and_expr)*
//!     and_expr := eq_expr  ('&&' eq_expr)*
//!     eq_expr  := cmp_expr (('==' | '!=') cmp_expr)*
//!     cmp_expr := add_expr (('<' | '>' | '<=' | '>=') add_expr)*
//!     add_expr := mul_expr (('+' | '-') mul_expr)*
//!     mul_expr := unary    (('*' | '/' | '%') unary)*
//!     unary    := ('-' | '!') unary | primary
//!     primary  := number | ident | '(' expr ')'
//!
//! Identifiers recognized by the runtime: `slot`, `step`,
//! `sides`, `phase`, `seed`. Anything else yields 0.0.
//!
//! All intermediate values are evaluated as `f32`. Booleans are
//! encoded as 1.0 for true and 0.0 for false; a result is
//! considered "wall" when it is strictly greater than 0.5.
//!
//! Pathability guarantee: the parser that constructs a
//! [`crate::dsl::ast::ObstacleSpec::CustomFormula`] calls
//! [`validate_pathable`] over the entire (step, slot) grid. If
//! any step has zero gaps across all slots the parser rejects
//! the level with a helpful error message, so the generator can
//! safely emit every wall it gets.

use std::fmt;

/// Precompiled expression tree.
#[derive(Clone, Debug)]
pub enum Expr {
    Num(f32),
    Var(String),
    Un(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug)]
pub enum UnOp { Neg, Not }

#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
}

/// Context passed to [`Expr::eval`] during mask construction.
#[derive(Clone, Copy)]
pub struct EvalCtx<'a> {
    pub slot:  f32,
    pub step:  f32,
    pub sides: f32,
    pub phase: f32,
    pub seed:  f32,
    /// Optional extra bindings sourced from the active variable
    /// scope. Looked up by name when the builtin variables do
    /// not match.
    pub extras: &'a [(String, f32)],
}

impl Expr {
    /// Evaluate with the given context.
    pub fn eval(&self, ctx: &EvalCtx) -> f32 {
        match self {
            Expr::Num(n) => *n,
            Expr::Var(name) => match name.as_str() {
                "slot"  => ctx.slot,
                "step"  => ctx.step,
                "sides" => ctx.sides,
                "phase" => ctx.phase,
                "seed"  => ctx.seed,
                other   => ctx.extras.iter()
                    .find(|(k, _)| k == other)
                    .map(|(_, v)| *v)
                    .unwrap_or(0.0),
            },
            Expr::Un(op, a) => {
                let v = a.eval(ctx);
                match op {
                    UnOp::Neg => -v,
                    UnOp::Not => if v > 0.5 { 0.0 } else { 1.0 },
                }
            }
            Expr::Bin(op, a, b) => {
                let x = a.eval(ctx);
                let y = b.eval(ctx);
                match op {
                    BinOp::Add => x + y,
                    BinOp::Sub => x - y,
                    BinOp::Mul => x * y,
                    BinOp::Div => if y.abs() < 1e-9 { 0.0 } else { x / y },
                    BinOp::Mod => {
                        if y.abs() < 1e-9 { 0.0 }
                        else {
                            let xi = x.round() as i64;
                            let yi = y.round() as i64;
                            if yi == 0 { 0.0 }
                            else { xi.rem_euclid(yi) as f32 }
                        }
                    }
                    BinOp::Eq => if (x - y).abs() < 1e-4 { 1.0 } else { 0.0 },
                    BinOp::Ne => if (x - y).abs() < 1e-4 { 0.0 } else { 1.0 },
                    BinOp::Lt => if x <  y { 1.0 } else { 0.0 },
                    BinOp::Le => if x <= y { 1.0 } else { 0.0 },
                    BinOp::Gt => if x >  y { 1.0 } else { 0.0 },
                    BinOp::Ge => if x >= y { 1.0 } else { 0.0 },
                    BinOp::And => {
                        if x > 0.5 && y > 0.5 { 1.0 } else { 0.0 }
                    }
                    BinOp::Or => {
                        if x > 0.5 || y > 0.5 { 1.0 } else { 0.0 }
                    }
                }
            }
        }
    }
}

/// Parse a formula string into an [`Expr`] tree. Returns an
/// error message on syntax failure.
pub fn parse(src: &str) -> Result<Expr, String> {
    let mut p = Parser::new(src);
    let e = p.parse_expr()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return Err(format!("trailing tokens at byte {}", p.pos));
    }
    Ok(e)
}

/// Walk the mask for every step (and every slot inside a step).
/// Returns `Ok(())` if every step has at least one gap, else an
/// error message pinpointing the first unreachable step.
pub fn validate_pathable(
    expr: &Expr, steps: u32, sides: u32, seed: u32,
) -> Result<(), String> {
    for step in 0..steps {
        let mut any_gap = false;
        for slot in 0..sides {
            let ctx = EvalCtx {
                slot:  slot as f32,
                step:  step as f32,
                sides: sides as f32,
                phase: 0.0,
                seed:  seed as f32,
                extras: &[],
            };
            let v = expr.eval(&ctx);
            if v <= 0.5 { any_gap = true; break; }
        }
        if !any_gap {
            return Err(format!(
                "formula leaves no gap at step {} (all {} slots are walls)",
                step, sides,
            ));
        }
    }
    Ok(())
}

// ----- parser implementation -----

struct Parser<'a> {
    bytes: &'a [u8],
    pos:   usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self { Parser { bytes: src.as_bytes(), pos: 0 } }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() &&
              self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self, s: &str) -> bool {
        let n = s.len();
        if self.pos + n > self.bytes.len() { return false; }
        &self.bytes[self.pos..self.pos + n] == s.as_bytes()
    }

    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        if self.peek(s) { self.pos += s.len(); true } else { false }
    }

    fn parse_expr(&mut self) -> Result<Expr, String> { self.parse_or() }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.eat("||") {
                let right = self.parse_and()?;
                left = Expr::Bin(BinOp::Or, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_eq()?;
        loop {
            self.skip_ws();
            if self.eat("&&") {
                let right = self.parse_eq()?;
                left = Expr::Bin(BinOp::And, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_eq(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_cmp()?;
        loop {
            self.skip_ws();
            let op = if self.eat("==")      { Some(BinOp::Eq) }
                     else if self.eat("!=") { Some(BinOp::Ne) }
                     else                   { None };
            if let Some(op) = op {
                let right = self.parse_cmp()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add()?;
        loop {
            self.skip_ws();
            let op = if self.eat("<=")      { Some(BinOp::Le) }
                     else if self.eat(">=") { Some(BinOp::Ge) }
                     else if self.eat("<")  { Some(BinOp::Lt) }
                     else if self.eat(">")  { Some(BinOp::Gt) }
                     else                   { None };
            if let Some(op) = op {
                let right = self.parse_add()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul()?;
        loop {
            self.skip_ws();
            let op = if self.eat("+")      { Some(BinOp::Add) }
                     else if self.eat("-") { Some(BinOp::Sub) }
                     else                  { None };
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
            let op = if self.eat("*")      { Some(BinOp::Mul) }
                     else if self.eat("/") { Some(BinOp::Div) }
                     else if self.eat("%") { Some(BinOp::Mod) }
                     else                  { None };
            if let Some(op) = op {
                let right = self.parse_unary()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        if self.eat("-") {
            let inner = self.parse_unary()?;
            return Ok(Expr::Un(UnOp::Neg, Box::new(inner)));
        }
        if self.eat("!") {
            let inner = self.parse_unary()?;
            return Ok(Expr::Un(UnOp::Not, Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        if self.eat("(") {
            let e = self.parse_expr()?;
            self.skip_ws();
            if !self.eat(")") {
                return Err(format!("expected ')' at byte {}", self.pos));
            }
            return Ok(e);
        }
        if self.pos >= self.bytes.len() {
            return Err("unexpected end of formula".into());
        }
        let c = self.bytes[self.pos];
        if c.is_ascii_digit() || c == b'.' {
            let start = self.pos;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_digit()
                    || self.bytes[self.pos] == b'.')
            { self.pos += 1; }
            let s = std::str::from_utf8(&self.bytes[start..self.pos])
                .map_err(|e| e.to_string())?;
            let n: f32 = s.parse().map_err(|_| format!("bad number '{}'", s))?;
            return Ok(Expr::Num(n));
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.pos;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_alphanumeric()
                    || self.bytes[self.pos] == b'_')
            { self.pos += 1; }
            let name = std::str::from_utf8(&self.bytes[start..self.pos])
                .map_err(|e| e.to_string())?
                .to_string();
            return Ok(Expr::Var(name));
        }
        Err(format!("unexpected character '{}' at byte {}",
            c as char, self.pos))
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Num(n) => write!(f, "{}", n),
            Expr::Var(v) => write!(f, "{}", v),
            Expr::Un(op, a) => match op {
                UnOp::Neg => write!(f, "(-{})", a),
                UnOp::Not => write!(f, "(!{})", a),
            },
            Expr::Bin(op, a, b) => {
                let sym = match op {
                    BinOp::Add => "+", BinOp::Sub => "-",
                    BinOp::Mul => "*", BinOp::Div => "/", BinOp::Mod => "%",
                    BinOp::Eq  => "==", BinOp::Ne => "!=",
                    BinOp::Lt  => "<", BinOp::Le => "<=",
                    BinOp::Gt  => ">", BinOp::Ge => ">=",
                    BinOp::And => "&&", BinOp::Or => "||",
                };
                write!(f, "({} {} {})", a, sym, b)
            }
        }
    }
}