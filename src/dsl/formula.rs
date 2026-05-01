//! Tiny expression language for user authored obstacle masks and
//! pattern formulas.
//!
//! Supported grammar is an extension of the v2 baseline with
//! mandatory fuel limited iteration. Every control flow piece
//! has an upper bound on its work, every arithmetic op is wrapped
//! through the safety module so NaN or infinity never reaches
//! the caller, and an overall per evaluation fuel counter makes
//! even a technically valid program terminate in bounded time.
//!
//! Grammar:
//!
//!     program  := stmt (';' stmt)*
//!     stmt     := assign | while_s | expr
//!     assign   := 'let'? IDENT '=' expr
//!     while_s  := 'while' expr 'with' 'fuel' INT 'do' program 'end'
//!     expr     := or_expr
//!     or_expr  := and_expr ('||' and_expr)*
//!     and_expr := eq_expr ('&&' eq_expr)*
//!     eq_expr  := cmp_expr (('==' | '!=') cmp_expr)*
//!     cmp_expr := bit_expr (('<' | '>' | '<=' | '>=') bit_expr)*
//!     bit_expr := add_expr (('&' | '|' | '<<' | '>>') add_expr)*
//!     add_expr := mul_expr (('+' | '-') mul_expr)*
//!     mul_expr := unary   (('*' | '/' | '%') unary)*
//!     unary    := ('-' | '!') unary | primary
//!     primary  := number | ident | '(' program ')'
//!
//! All intermediate values are f32. Booleans encode as 1.0 for
//! true and 0.0 for false; a result is treated as wall when
//! strictly greater than 0.5.
//!
//! Safety:
//!
//! * Parse produces an expression whose AST depth is capped at
//!   `MAX_FORMULA_AST_DEPTH`.
//! * Every eval call carries a mutable fuel counter. When fuel
//!   drops to zero the evaluation collapses to 0.0 and no further
//!   computation runs. Authors can observe this as a formula
//!   that prematurely opens up all slots, which is safer than
//!   a panic or a freeze.
//! * While loops require an explicit fuel tag at parse time in
//!   the range `1 ..= MAX_FORMULA_FUEL`. The per iteration cost
//!   is deducted from both the loop own allowance and the
//!   evaluation wide counter.
//! * Division, modulo, shift, power are routed through safety
//!   helpers. Bit ops clamp to i32 range. Comparisons treat
//!   non finite values as the neutral element.
//! * Validation ahead of gameplay rejects formulas whose masks
//!   leave even one step without a gap, across a deterministic
//!   grid of (step, slot) pairs.

use std::fmt;

use crate::dsl::safety::{
    self, MAX_FORMULA_AST_DEPTH, MAX_FORMULA_FUEL,
    sanitize_float, safe_div, safe_mod,
    safe_and, safe_or, safe_shl, safe_shr,
};

/// Fully parsed formula ready for evaluation.
#[derive(Clone, Debug)]
pub enum Expr {
    Num(f32),
    Var(String),
    Un(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// Sequence of statements. Allows `let` and `while` to
    /// appear in expression positions by producing the value of
    /// the last statement of the sequence.
    Seq(Vec<Stmt>),
}

/// Statements recognized inside a formula body.
#[derive(Clone, Debug)]
pub enum Stmt {
    /// Assign the value of `expr` to a local variable.
    Assign { name: String, expr: Expr },
    /// Fuel bounded while loop.
    While { cond: Expr, fuel: u32, body: Vec<Stmt> },
    /// Bare expression.
    Expr(Expr),
}

#[derive(Clone, Copy, Debug)]
pub enum UnOp { Neg, Not }

#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    BAnd, BOr, Shl, Shr,
}

/// Read only evaluation context. Holds the builtin variable
/// bindings the generator wants to pass to a formula. The
/// mutable state (fuel, depth, locals) lives inside the
/// evaluator and is not visible to the caller.
#[derive(Clone)]
pub struct EvalCtx<'a> {
    pub slot:  f32,
    pub step:  f32,
    pub sides: f32,
    pub phase: f32,
    pub seed:  f32,
    pub extras: &'a [(String, f32)],
}

impl<'a> EvalCtx<'a> {
    pub fn new(
        slot: f32, step: f32, sides: f32, phase: f32, seed: f32,
        extras: &'a [(String, f32)],
    ) -> Self {
        EvalCtx { slot, step, sides, phase, seed, extras }
    }
}

/// Mutable runtime used internally by the evaluator. Holds the
/// fuel counter shared across the current evaluation, the depth
/// counter, and the local variable stack populated by `let`
/// bindings inside sequences and while loops.
struct Runtime<'a> {
    slot:  f32,
    step:  f32,
    sides: f32,
    phase: f32,
    seed:  f32,
    extras: &'a [(String, f32)],
    locals: Vec<(String, f32)>,
    fuel:   u32,
    depth:  usize,
}

impl<'a> Runtime<'a> {
    fn from_ctx(ctx: &EvalCtx<'a>) -> Self {
        Runtime {
            slot:  ctx.slot,
            step:  ctx.step,
            sides: ctx.sides,
            phase: ctx.phase,
            seed:  ctx.seed,
            extras: ctx.extras,
            locals: Vec::new(),
            fuel:   MAX_FORMULA_FUEL,
            depth:  0,
        }
    }
    fn spend(&mut self) -> Result<(), ()> {
        if self.fuel == 0 { return Err(()); }
        self.fuel -= 1;
        Ok(())
    }
    fn resolve(&self, name: &str) -> f32 {
        for (k, v) in self.locals.iter().rev() {
            if k == name { return *v; }
        }
        match name {
            "slot"  => self.slot,
            "step"  => self.step,
            "sides" => self.sides,
            "phase" => self.phase,
            "seed"  => self.seed,
            other   => self.extras.iter()
                .find(|(k, _)| k == other)
                .map(|(_, v)| *v)
                .unwrap_or(0.0),
        }
    }
    fn set_local(&mut self, name: &str, value: f32) {
        for (k, v) in self.locals.iter_mut().rev() {
            if k == name { *v = value; return; }
        }
        self.locals.push((name.to_string(), value));
    }
}

impl Expr {
    /// Evaluate the expression against `ctx`. Returns the final
    /// numeric result, clamped into a finite range. On any
    /// runtime failure (fuel exhausted, depth exceeded) returns
    /// 0.0 so callers see a neutral gap rather than a panic.
    pub fn eval(&self, ctx: &EvalCtx) -> f32 {
        let mut rt = Runtime::from_ctx(ctx);
        eval_expr(self, &mut rt).unwrap_or(0.0)
    }
}

fn eval_expr(expr: &Expr, rt: &mut Runtime) -> Result<f32, ()> {
    if rt.spend().is_err() { return Err(()); }
    rt.depth += 1;
    if rt.depth > MAX_FORMULA_AST_DEPTH {
        rt.depth -= 1;
        return Err(());
    }
    let result = match expr {
        Expr::Num(n) => Ok(sanitize_float(*n)),
        Expr::Var(name) => Ok(sanitize_float(rt.resolve(name))),
        Expr::Un(op, inner) => {
            let v = eval_expr(inner, rt)?;
            Ok(match op {
                UnOp::Neg => sanitize_float(-v),
                UnOp::Not => if v > 0.5 { 0.0 } else { 1.0 },
            })
        }
        Expr::Bin(op, a, b) => {
            let x = eval_expr(a, rt)?;
            let y = eval_expr(b, rt)?;
            Ok(match op {
                BinOp::Add => sanitize_float(x + y),
                BinOp::Sub => sanitize_float(x - y),
                BinOp::Mul => sanitize_float(x * y),
                BinOp::Div => safe_div(x, y),
                BinOp::Mod => safe_mod(x, y),
                BinOp::Eq => if (x - y).abs() < 1e-4 { 1.0 } else { 0.0 },
                BinOp::Ne => if (x - y).abs() < 1e-4 { 0.0 } else { 1.0 },
                BinOp::Lt => if x <  y { 1.0 } else { 0.0 },
                BinOp::Le => if x <= y { 1.0 } else { 0.0 },
                BinOp::Gt => if x >  y { 1.0 } else { 0.0 },
                BinOp::Ge => if x >= y { 1.0 } else { 0.0 },
                BinOp::And => if x > 0.5 && y > 0.5 { 1.0 } else { 0.0 },
                BinOp::Or  => if x > 0.5 || y > 0.5 { 1.0 } else { 0.0 },
                BinOp::BAnd => safe_and(x, y),
                BinOp::BOr  => safe_or(x, y),
                BinOp::Shl  => safe_shl(x, y),
                BinOp::Shr  => safe_shr(x, y),
            })
        }
        Expr::Seq(stmts) => eval_seq(stmts, rt),
    };
    rt.depth -= 1;
    result
}

fn eval_seq(stmts: &[Stmt], rt: &mut Runtime) -> Result<f32, ()> {
    let mut last = 0.0f32;
    for stmt in stmts {
        if rt.spend().is_err() { return Err(()); }
        match stmt {
            Stmt::Assign { name, expr } => {
                let v = eval_expr(expr, rt)?;
                rt.set_local(name, v);
                last = v;
            }
            Stmt::While { cond, fuel, body } => {
                let mut remaining = (*fuel).min(MAX_FORMULA_FUEL);
                while remaining > 0 {
                    if rt.spend().is_err() { return Err(()); }
                    let c = eval_expr(cond, rt)?;
                    if c <= 0.5 { break; }
                    eval_seq(body, rt)?;
                    remaining -= 1;
                }
                last = 0.0;
            }
            Stmt::Expr(e) => {
                last = eval_expr(e, rt)?;
            }
        }
    }
    Ok(last)
}

/// Parse a formula source string into an `Expr` tree. Returns
/// a diagnostic string on syntax failure. Depth and length are
/// bounded by the safety module before any evaluation ever runs.
pub fn parse(src: &str) -> Result<Expr, String> {
    if src.len() > 8192 {
        return Err("formula source exceeds 8 KB".into());
    }
    let mut p = Parser::new(src);
    let program = p.parse_program()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return Err(format!("trailing tokens at byte {}", p.pos));
    }
    Ok(program)
}

/// Walk the mask for every step and every slot inside a step.
/// Returns `Ok(())` if every step has at least one gap, else an
/// error message pointing at the first unreachable step.
pub fn validate_pathable(
    expr: &Expr, steps: u32, sides: u32, seed: u32,
) -> Result<(), String> {
    let steps = steps.min(safety::MAX_PATTERN_STEPS);
    for step in 0..steps {
        let mut any_gap = false;
        for slot in 0..sides {
            let ctx = EvalCtx::new(
                slot as f32, step as f32, sides as f32,
                0.0, seed as f32, &[],
            );
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

/// Hand written recursive descent parser with depth tracking.
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser { bytes: src.as_bytes(), pos: 0, depth: 0 }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && self.bytes[self.pos].is_ascii_whitespace()
        {
            self.pos += 1;
        }
    }

    fn peek(&self, s: &str) -> bool {
        let n = s.len();
        if self.pos + n > self.bytes.len() { return false; }
        &self.bytes[self.pos..self.pos + n] == s.as_bytes()
    }

    fn peek_kw(&mut self, kw: &str) -> bool {
        self.skip_ws();
        let n = kw.len();
        if self.pos + n > self.bytes.len() { return false; }
        if &self.bytes[self.pos..self.pos + n] != kw.as_bytes() { return false; }
        if self.pos + n < self.bytes.len() {
            let c = self.bytes[self.pos + n];
            if c.is_ascii_alphanumeric() || c == b'_' { return false; }
        }
        true
    }

    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        if self.peek(s) { self.pos += s.len(); true } else { false }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.peek_kw(kw) { self.pos += kw.len(); true } else { false }
    }

    fn down(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_FORMULA_AST_DEPTH {
            return Err(format!(
                "formula AST depth exceeded {}",
                MAX_FORMULA_AST_DEPTH));
        }
        Ok(())
    }

    fn up(&mut self) {
        if self.depth > 0 { self.depth -= 1; }
    }

    fn parse_program(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut stmts = Vec::new();
        loop {
            self.skip_ws();
            if self.pos >= self.bytes.len() { break; }
            if self.peek(")") || self.peek_kw("end") { break; }
            let s = self.parse_stmt()?;
            stmts.push(s);
            self.skip_ws();
            if !self.eat(";") { break; }
        }
        self.up();
        if stmts.len() == 1 {
            match stmts.into_iter().next().unwrap() {
                Stmt::Expr(e) => Ok(e),
                other => Ok(Expr::Seq(vec![other])),
            }
        } else {
            Ok(Expr::Seq(stmts))
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.down()?;
        let result = if self.eat_kw("let") {
            let name = self.parse_ident()?;
            self.skip_ws();
            if !self.eat("=") {
                return Err(format!("expected '=' at byte {}", self.pos));
            }
            let expr = self.parse_expr()?;
            Ok(Stmt::Assign { name, expr })
        } else if self.eat_kw("while") {
            let cond = self.parse_expr()?;
            if !self.eat_kw("with") {
                return Err(format!("expected 'with fuel N' at byte {}", self.pos));
            }
            if !self.eat_kw("fuel") {
                return Err(format!("expected 'fuel N' at byte {}", self.pos));
            }
            let fuel = self.parse_integer()?;
            if fuel < 1 || fuel > MAX_FORMULA_FUEL {
                return Err(format!(
                    "while fuel must be in 1..={}", MAX_FORMULA_FUEL));
            }
            if !self.eat_kw("do") {
                return Err(format!("expected 'do' at byte {}", self.pos));
            }
            let body = self.parse_stmt_list()?;
            if !self.eat_kw("end") {
                return Err(format!("expected 'end' at byte {}", self.pos));
            }
            Ok(Stmt::While { cond, fuel, body })
        } else {
            let e = self.parse_expr()?;
            self.skip_ws();
            if let Expr::Var(name) = &e {
                if self.peek("=") && !self.peek("==") {
                    self.bump_one();
                    let rhs = self.parse_expr()?;
                    return Ok(Stmt::Assign { name: name.clone(), expr: rhs });
                }
            }
            Ok(Stmt::Expr(e))
        };
        self.up();
        result
    }

    fn bump_one(&mut self) {
        if self.pos < self.bytes.len() { self.pos += 1; }
    }

    fn parse_stmt_list(&mut self) -> Result<Vec<Stmt>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            if self.peek_kw("end") { break; }
            if self.pos >= self.bytes.len() { break; }
            out.push(self.parse_stmt()?);
            self.skip_ws();
            if !self.eat(";") { break; }
        }
        Ok(out)
    }

    fn parse_ident(&mut self) -> Result<String, String> {
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
        {
            self.pos += 1;
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.pos]).to_string())
    }

    fn parse_integer(&mut self) -> Result<u32, String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.bytes.len()
            && self.bytes[self.pos].is_ascii_digit()
        {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(format!("expected integer at byte {}", start));
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|e| e.to_string())?;
        s.parse::<u32>().map_err(|_| format!("bad integer '{}'", s))
    }

    fn parse_expr(&mut self) -> Result<Expr, String> { self.parse_or() }

    fn parse_or(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.eat("||") {
                let right = self.parse_and()?;
                left = Expr::Bin(BinOp::Or, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut left = self.parse_eq()?;
        loop {
            self.skip_ws();
            if self.eat("&&") {
                let right = self.parse_eq()?;
                left = Expr::Bin(BinOp::And, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_eq(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut left = self.parse_cmp()?;
        loop {
            self.skip_ws();
            let op = if self.eat("==") { Some(BinOp::Eq) }
                     else if self.eat("!=") { Some(BinOp::Ne) }
                     else { None };
            if let Some(op) = op {
                let right = self.parse_cmp()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_cmp(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut left = self.parse_bit()?;
        loop {
            self.skip_ws();
            let op = if self.eat("<=") { Some(BinOp::Le) }
                     else if self.eat(">=") { Some(BinOp::Ge) }
                     else if self.peek("<<") || self.peek(">>") { None }
                     else if self.eat("<") { Some(BinOp::Lt) }
                     else if self.eat(">") { Some(BinOp::Gt) }
                     else { None };
            if let Some(op) = op {
                let right = self.parse_bit()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_bit(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut left = self.parse_add()?;
        loop {
            self.skip_ws();
            let op = if self.eat("<<") { Some(BinOp::Shl) }
                     else if self.eat(">>") { Some(BinOp::Shr) }
                     else if self.peek("&&") || self.peek("||") { None }
                     else if self.eat("&") { Some(BinOp::BAnd) }
                     else if self.eat("|") { Some(BinOp::BOr) }
                     else { None };
            if let Some(op) = op {
                let right = self.parse_add()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut left = self.parse_mul()?;
        loop {
            self.skip_ws();
            let op = if self.eat("+") { Some(BinOp::Add) }
                     else if self.eat("-") { Some(BinOp::Sub) }
                     else { None };
            if let Some(op) = op {
                let right = self.parse_mul()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, String> {
        self.down()?;
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op = if self.eat("*") { Some(BinOp::Mul) }
                     else if self.eat("/") { Some(BinOp::Div) }
                     else if self.eat("%") { Some(BinOp::Mod) }
                     else { None };
            if let Some(op) = op {
                let right = self.parse_unary()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
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
            let e = self.parse_program()?;
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
            safety::check_finite_formula(n, "formula literal")?;
            return Ok(Expr::Num(n));
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let name = self.parse_ident()?;
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
                    BinOp::BAnd => "&", BinOp::BOr => "|",
                    BinOp::Shl => "<<", BinOp::Shr => ">>",
                };
                write!(f, "({} {} {})", a, sym, b)
            }
            Expr::Seq(stmts) => write!(f, "(seq of {} stmts)", stmts.len()),
        }
    }
}