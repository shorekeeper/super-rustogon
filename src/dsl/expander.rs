//! Expansion engine for the v3 meta language.
//!
//! Takes a parsed meta AST and lowers it to the flat runtime
//! `Stmt` sequence the generator consumes. Every step of the
//! expansion touches a shared safety instrument:
//!
//! * Parse deadline. Polled at each statement and at each loop
//!   iteration. If the file has taken more than
//!   `MAX_PARSE_DURATION_MS` milliseconds to load the expander
//!   aborts with a clear error.
//! * Expansion budget. Decremented on every evaluated node,
//!   every runtime statement appended, every loop iteration,
//!   every function call. Starts at `MAX_EXPANSION_BUDGET`.
//!   When it hits zero the expander aborts.
//! * Recursion depth counter. Incremented on every nested
//!   expression and statement expansion. Capped at
//!   `MAX_RECURSION_DEPTH` to prevent native stack overflow.
//! * AST node counter. Counts materialized runtime statements
//!   across the whole load. Capped at `MAX_AST_NODES`.
//!
//! Recursive meta functions require an explicit `decreases`
//! measure. The expander checks that the measure evaluates to
//! a non negative integer on entry to each call. Combined with
//! the budget this gives every meta program the termination
//! property the overall design relies on.
//!
//! Arithmetic routes through `safety::sanitize_float`,
//! `safe_div`, `safe_mod`, and integer saturating operations.
//! NaN and infinity cannot reach the resulting AST, a divisor
//! of zero gives back zero, and integer overflow saturates at
//! the i64 boundary instead of panicking.

use std::collections::HashMap;

use crate::dsl::ast::{
    Stmt, ObstacleSpec, TriggerSpec,
    VarDecl, VarType, VarValue,
};
use crate::dsl::meta::{
    MetaStmt, MetaExpr, MetaValue, MetaFn, MetaBinOp, MetaUnOp,
    MatchArm, MatchPattern, HookEvent,
};

use crate::dsl::rules::{
    LevelRules, RuleCategory, RuleSet, guard_stack_push,
};

use crate::dsl::safety::{
    Budget, Deadline, MAX_AST_NODES, MAX_EXPANSION_BUDGET,
    MAX_RECURSION_DEPTH, MAX_FOR_ITERATIONS,
    MAX_STMTS_PER_SECTION, MAX_PARSE_DURATION_MS,
    sanitize_float, safe_div, safe_mod,
};
use crate::levels::difficulty::Tier;

/// Result of expanding one section body. Returned by
/// `Expander::expand_section` and consumed by the v3 parser
/// before attaching both pieces to the enclosing `Section`.
pub struct ExpandOutput {
    /// Runtime statements in the order they should appear in the
    /// section body.
    pub stmts: Vec<Stmt>,
    /// Hooks declared by `@on` blocks within the section. The
    /// parser stores these on the `Section` for the runtime
    /// hook dispatcher to pick up.
    pub hooks: Vec<(HookEvent, Vec<TriggerSpec>)>,
}

/// Shared state across one expansion pass.
///
/// One `Expander` instance is used per file load. It owns the
/// budget, the deadline, the function registry, the pattern
/// registry, and the trigger stack registry. Each section body
/// is expanded against this state, which is why pattern bindings
/// and function definitions written early in the file are
/// visible to sections later on.
pub struct Expander {
    pub budget: Budget,
    pub deadline: Deadline,
    pub functions: HashMap<String, MetaFn>,
    pub patterns: HashMap<String, ObstacleSpec>,
    pub trigger_stacks: HashMap<String, Vec<TriggerSpec>>,
    /// Level-scoped variable bindings declared in the file's
    /// `global do ... end` block. Resolved by `eval_expr`
    /// after the lexical scope stack and before the engine
    /// builtins, so a local `let` shadows a global of the
    /// same name but a global shadows nothing.
    pub globals: HashMap<String, MetaValue>,
    pub tier: Tier,
    pub sides: u32,
    pub bpm: u32,
    pub section_name: String,
    pub ast_nodes: usize,
    pub rule_stack_depths: [usize; 6],
}

impl Expander {
    /// Construct a fresh expander for one file load. `tier` is
    /// available inside meta programs as the `tier` builtin
    /// atom, `sides` as the integer `sides`, and `bpm` as the
    /// integer `bpm`.
    pub fn new(tier: Tier, sides: u32, bpm: u32) -> Self {
        Expander {
            budget: Budget::new(MAX_EXPANSION_BUDGET),
            deadline: Deadline::new(MAX_PARSE_DURATION_MS),
            functions: HashMap::new(),
            patterns: HashMap::new(),
            trigger_stacks: HashMap::new(),
            globals: HashMap::new(),
            tier, sides, bpm,
            section_name: String::new(),
            ast_nodes: 0,
            rule_stack_depths: [0; 6],
        }
    }

    /// Register a meta function. The parser calls this once per
    /// `fn` definition as it walks top level forms.
    pub fn register_fn(&mut self, f: MetaFn) {
        self.functions.insert(f.name.clone(), f);
    }

    /// Register a pattern binding. The parser calls this once
    /// per `pattern :name = obstacle_spec` form.
    pub fn register_pattern(&mut self, name: String, spec: ObstacleSpec) {
        self.patterns.insert(name, spec);
    }

    /// Record one file level variable declaration in the
    /// global table. The parser calls this once per entry in
    /// the `global` block during top level parsing.
    ///
    /// `VarValue` uses separate variants for numbers, strings,
    /// atoms, and bools, whereas meta expressions speak the
    /// unified `MetaValue` language. Conversion happens here
    /// so the rest of the expander never has to know `VarValue`
    /// exists. Integer-typed numbers go through to
    /// `MetaValue::Int` to make subsequent range iteration and
    /// modulo arithmetic behave as authors expect.
    pub fn register_global(&mut self, decl: &VarDecl) {
        let v = match (&decl.value, decl.ty) {
            (VarValue::Num(n), VarType::Int) =>
                MetaValue::Int(n.round() as i64),
            (VarValue::Num(n), _)   => MetaValue::Float(*n),
            (VarValue::Str(s), _)   => MetaValue::Str(s.clone()),
            (VarValue::Ident(s), _) => MetaValue::Atom(s.clone()),
            (VarValue::Bool(b), _)  => MetaValue::Bool(*b),
        };
        self.globals.insert(decl.name.clone(), v);
    }

    /// Register a trigger stack. The parser calls this once per
    /// `trigger_stack :name do ... end` block.
    pub fn register_trigger_stack(&mut self, name: String, stack: Vec<TriggerSpec>) {
        self.trigger_stacks.insert(name, stack);
    }

    fn count_node(&mut self) -> Result<(), String> {
        self.ast_nodes += 1;
        if self.ast_nodes > MAX_AST_NODES {
            return Err(format!(
                "AST size exceeded {} nodes", MAX_AST_NODES));
        }
        Ok(())
    }

    /// Top level entry point: expand the meta body of one
    /// section into a flat runtime statement list plus any
    /// hooks declared inside.
    pub fn expand_section(
        &mut self,
        section_name: String,
        body: Vec<MetaStmt>,
    ) -> Result<ExpandOutput, String> {
        self.section_name = section_name;
        // Rule stacks are per section by design: an author who
        // pushed a snapshot and forgot to pop should not corrupt
        // the next section's state. The runtime engine enforces
        // the same rule, but resetting here makes expansion
        // errors about stack depth point at the offending
        // section rather than some later one.
        self.rule_stack_depths = [0; 6];
        let mut out = ExpandOutput {
            stmts: Vec::new(),
            hooks: Vec::new(),
        };
        let mut scope = Scope::new();
        self.expand_stmts(&body, &mut scope, &mut out, 0)?;
        if out.stmts.len() > MAX_STMTS_PER_SECTION {
            return Err(format!(
                "section has {} runtime statements (limit {})",
                out.stmts.len(), MAX_STMTS_PER_SECTION));
        }
        Ok(out)
    }

    fn expand_stmts(
        &mut self,
        stmts: &[MetaStmt],
        scope: &mut Scope,
        out: &mut ExpandOutput,
        depth: usize,
    ) -> Result<(), String> {
        for stmt in stmts {
            self.deadline.check()?;
            self.budget.spend(1)?;
            self.expand_stmt(stmt, scope, out, depth)?;
        }
        Ok(())
    }

    fn expand_stmt(
        &mut self,
        stmt: &MetaStmt,
        scope: &mut Scope,
        out: &mut ExpandOutput,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(format!(
                "meta recursion depth exceeded {}",
                MAX_RECURSION_DEPTH));
        }
        self.count_node()?;
        match stmt {
            MetaStmt::Let { name, ty: _, value } => {
                let v = self.eval_expr(value, scope, depth)?;
                scope.set(name.clone(), v);
            }
            MetaStmt::If { cond, then_, else_ } => {
                let c = self.eval_expr(cond, scope, depth)?;
                if truthy(&c) {
                    self.expand_stmts(then_, scope, out, depth + 1)?;
                } else {
                    self.expand_stmts(else_, scope, out, depth + 1)?;
                }
            }
            MetaStmt::Match { scrutinee, arms } => {
                let v = self.eval_expr(scrutinee, scope, depth)?;
                let mut matched = false;
                for arm in arms {
                    if self.match_pattern(&arm.pattern, &v)? {
                        self.expand_stmts(&arm.body, scope, out, depth + 1)?;
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    return Err(format!(
                        "match has no arm for {:?}", v));
                }
            }
            MetaStmt::For { binder, iter, body } => {
                let v = self.eval_expr(iter, scope, depth)?;
                let items = iter_items(&v)?;
                if items.len() as u32 > MAX_FOR_ITERATIONS {
                    return Err(format!(
                        "for loop iterates {} times (limit {})",
                        items.len(), MAX_FOR_ITERATIONS));
                }
                for item in items {
                    self.deadline.check()?;
                    self.budget.spend(1)?;
                    scope.push();
                    scope.set(binder.clone(), item);
                    self.expand_stmts(body, scope, out, depth + 1)?;
                    scope.pop();
                }
            }
            MetaStmt::Expand(call_expr) => {
                let result = self.eval_expr(call_expr, scope, depth)?;
                match result {
                    MetaValue::Stmts(ss) => {
                        for s in ss {
                            out.stmts.push(s);
                            self.count_node()?;
                        }
                    }
                    MetaValue::Unit => {
                        // Legal, produced no statements.
                    }
                    other => {
                        return Err(format!(
                            "@expand target must return statements, got {:?}",
                            other));
                    }
                }
            }
            MetaStmt::EmitPattern { name } => {
                let spec = self.patterns.get(name).cloned()
                    .ok_or_else(|| format!(
                        "unknown pattern ':{}'", name))?;
                out.stmts.push(Stmt::Emit(spec));
                self.count_node()?;
            }
            MetaStmt::FireStack { name } => {
                let stack = self.trigger_stacks.get(name).cloned()
                    .ok_or_else(|| format!(
                        "unknown trigger stack ':{}'", name))?;
                for t in stack {
                    out.stmts.push(Stmt::Trigger(t));
                    self.count_node()?;
                }
            }
            MetaStmt::Runtime(s) => {
                out.stmts.push(s.clone());
                self.count_node()?;
            }
            MetaStmt::TriggerPipe(triggers) => {
                // Every element in the pipe fires at the same
                // scheduling anchor. The runtime generator does
                // not advance its clock on `Stmt::Trigger`, so
                // pushing them one after another is exactly
                // what the author's `|>` operator means.
                for t in triggers {
                    out.stmts.push(Stmt::Trigger(*t));
                    self.count_node()?;
                }
            }
            MetaStmt::Hook { event, body } => {
                out.hooks.push((event.clone(), body.clone()));
            }
            MetaStmt::Track { .. } => {
                return Err(
                    "track { ... } is reserved for a future revision"
                    .into());
            }
            MetaStmt::Rule(rs) => {
                out.stmts.push(Stmt::Rule(rs.clone()));
                self.count_node()?;
            }
            MetaStmt::Revert(cat) => {
                // Reverting resets the per category stack so
                // subsequent pops do nothing rather than pull
                // stale state back in. Matches the runtime
                // engine's semantics.
                let idx = category_index(*cat);
                if let Some(i) = idx {
                    self.rule_stack_depths[i] = 0;
                } else {
                    // `All` variant: clear every stack.
                    self.rule_stack_depths = [0; 6];
                }
                out.stmts.push(Stmt::Revert(*cat));
                self.count_node()?;
            }
            MetaStmt::Push(cat) => {
                let idx = category_index(*cat).ok_or_else(|| {
                    "cannot push :all, use push with a concrete category"
                        .to_string()
                })?;
                guard_stack_push(self.rule_stack_depths[idx])?;
                self.rule_stack_depths[idx] += 1;
                out.stmts.push(Stmt::Push(*cat));
                self.count_node()?;
            }
            MetaStmt::Pop(cat) => {
                let idx = category_index(*cat).ok_or_else(|| {
                    "cannot pop :all, use pop with a concrete category"
                        .to_string()
                })?;
                if self.rule_stack_depths[idx] > 0 {
                    self.rule_stack_depths[idx] -= 1;
                }
                // A pop on an empty stack is a no op at
                // runtime, intentionally permissive so
                // recursive or conditional pushes do not
                // require balanced emission for the level
                // to load.
                out.stmts.push(Stmt::Pop(*cat));
                self.count_node()?;
            }
        }
        Ok(())
    }

    fn eval_expr(
        &mut self,
        expr: &MetaExpr,
        scope: &mut Scope,
        depth: usize,
    ) -> Result<MetaValue, String> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(format!(
                "meta recursion depth exceeded {}",
                MAX_RECURSION_DEPTH));
        }
        self.budget.spend(1)?;
        match expr {
            MetaExpr::Lit(v) => Ok(v.clone()),
            MetaExpr::Var(name) => {
                // Resolution order, most specific first:
                // 1. Lexical scope (function params, for
                //    loop binders, local `let` bindings).
                // 2. File level globals (`global do ... end`).
                // 3. Engine builtins (tier, sides, bpm,
                //    section_name).
                // A name undefined at all three levels is a
                // parse time error, never a silent zero at
                // runtime.
                scope.get(name)
                    .or_else(|| self.globals.get(name).cloned())
                    .or_else(|| self.builtin_var(name))
                    .ok_or_else(|| format!(
                        "unknown variable '{}'", name))
            }
            MetaExpr::Un(op, a) => {
                let va = self.eval_expr(a, scope, depth + 1)?;
                eval_un(*op, va)
            }
            MetaExpr::Bin(op, a, b) => {
                let va = self.eval_expr(a, scope, depth + 1)?;
                let vb = self.eval_expr(b, scope, depth + 1)?;
                eval_bin(*op, va, vb)
            }
            MetaExpr::Call { name, args } => {
                let mut arg_vals = Vec::new();
                for a in args {
                    arg_vals.push(self.eval_expr(a, scope, depth + 1)?);
                }
                self.call_fn(name, arg_vals, depth)
            }
            MetaExpr::If { cond, then_, else_ } => {
                let c = self.eval_expr(cond, scope, depth + 1)?;
                if truthy(&c) {
                    self.eval_expr(then_, scope, depth + 1)
                } else {
                    self.eval_expr(else_, scope, depth + 1)
                }
            }
            MetaExpr::Range { lo, hi, step } => {
                let lv = self.eval_expr(lo, scope, depth + 1)?;
                let hv = self.eval_expr(hi, scope, depth + 1)?;
                let sv = match step {
                    Some(s) => self.eval_expr(s, scope, depth + 1)?,
                    None => MetaValue::Int(1),
                };
                let li = as_int(&lv)?;
                let hi_i = as_int(&hv)?;
                let si = as_int(&sv)?;
                if si == 0 {
                    return Err("range step cannot be zero".into());
                }
                Ok(MetaValue::Range { lo: li, hi: hi_i, step: si })
            }
            MetaExpr::List(items) => {
                let mut list = Vec::with_capacity(items.len());
                for e in items {
                    list.push(self.eval_expr(e, scope, depth + 1)?);
                }
                Ok(MetaValue::List(list))
            }
        }
    }

    /// Lookup a builtin variable. These are the read only
    /// bindings a meta program receives from the engine: the
    /// active difficulty tier as an atom, the hex ring side
    /// count, and the level BPM. Anything tier or configuration
    /// specific lives here so meta programs cannot invent their
    /// own facts about the running game.
    fn builtin_var(&self, name: &str) -> Option<MetaValue> {
        match name {
            "tier" => Some(MetaValue::Atom(tier_atom(self.tier).into())),
            "sides" => Some(MetaValue::Int(self.sides as i64)),
            "bpm" => Some(MetaValue::Int(self.bpm as i64)),
            "section_name" => Some(MetaValue::Str(self.section_name.clone())),
            _ => None,
        }
    }

    fn call_fn(
        &mut self,
        name: &str,
        args: Vec<MetaValue>,
        depth: usize,
    ) -> Result<MetaValue, String> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(format!(
                "meta call depth exceeded {}",
                MAX_RECURSION_DEPTH));
        }
        let f = self.functions.get(name).cloned()
            .ok_or_else(|| format!(
                "unknown function '{}'", name))?;
        if args.len() != f.params.len() {
            return Err(format!(
                "'{}' expects {} args, got {}",
                name, f.params.len(), args.len()));
        }

        let mut scope = Scope::new();
        for (p, v) in f.params.iter().zip(args.iter()) {
            scope.set(p.name.clone(), v.clone());
        }

        // Precondition.
        if let Some(req) = &f.requires {
            let r = self.eval_expr(req, &mut scope, depth + 1)?;
            if !truthy(&r) {
                return Err(format!(
                    "'{}' requires clause not satisfied",
                    name));
            }
        }
        // Termination measure. The caller side check is simple
        // (non negative integer). Combined with the expansion
        // budget this is enough to guarantee that any recursion
        // either completes or fails to load with a clear error.
        if let Some(dec) = &f.decreases {
            let m = self.eval_expr(dec, &mut scope, depth + 1)?;
            let mi = as_int(&m)?;
            if mi < 0 {
                return Err(format!(
                    "'{}' decreases measure is negative ({})",
                    name, mi));
            }
        }

        // Execute body, collecting any runtime statements the
        // function emits into a dedicated buffer. The function
        // returns either those statements wrapped in
        // `MetaValue::Stmts` (the common case for builders) or
        // the last evaluated expression value if the body
        // contains no emits.
        let mut out = ExpandOutput {
            stmts: Vec::new(),
            hooks: Vec::new(),
        };
        for stmt in &f.body {
            self.deadline.check()?;
            self.budget.spend(1)?;
            self.expand_stmt(stmt, &mut scope, &mut out, depth + 1)?;
        }
        if !out.hooks.is_empty() {
            return Err(format!(
                "'{}' declared hooks inside a function body",
                name));
        }
        if !out.stmts.is_empty() {
            Ok(MetaValue::Stmts(out.stmts))
        } else {
            Ok(MetaValue::Unit)
        }
    }

    fn match_pattern(
        &self,
        pat: &MatchPattern,
        value: &MetaValue,
    ) -> Result<bool, String> {
        match pat {
            MatchPattern::Wildcard => Ok(true),
            MatchPattern::Atom(name) => {
                if let MetaValue::Atom(n) = value {
                    Ok(n == name)
                } else {
                    Ok(false)
                }
            }
            MatchPattern::AtomRange { lo, hi } => {
                if let MetaValue::Atom(n) = value {
                    let ri = atom_rank(n);
                    let rlo = atom_rank(lo);
                    let rhi = atom_rank(hi);
                    if let (Some(ri), Some(rlo), Some(rhi)) = (ri, rlo, rhi) {
                        Ok(ri >= rlo && ri <= rhi)
                    } else {
                        Ok(false)
                    }
                } else {
                    Ok(false)
                }
            }
        }
    }
}

/// Variable scope stack used during expansion. Each `For` loop
/// and function call pushes a fresh layer; closing the construct
/// pops the layer so bindings never leak into the enclosing
/// scope.
pub struct Scope {
    layers: Vec<HashMap<String, MetaValue>>,
}

impl Scope {
    pub fn new() -> Self {
        Scope { layers: vec![HashMap::new()] }
    }
    pub fn push(&mut self) {
        self.layers.push(HashMap::new());
    }
    pub fn pop(&mut self) {
        if self.layers.len() > 1 {
            self.layers.pop();
        }
    }
    pub fn set(&mut self, name: String, value: MetaValue) {
        if let Some(top) = self.layers.last_mut() {
            top.insert(name, value);
        }
    }
    pub fn get(&self, name: &str) -> Option<MetaValue> {
        for layer in self.layers.iter().rev() {
            if let Some(v) = layer.get(name) {
                return Some(v.clone());
            }
        }
        None
    }
}

fn truthy(v: &MetaValue) -> bool {
    match v {
        MetaValue::Bool(b) => *b,
        MetaValue::Int(n) => *n != 0,
        MetaValue::Float(f) => *f > 0.5,
        MetaValue::Atom(_) | MetaValue::Str(_) => true,
        MetaValue::Range { .. } | MetaValue::List(_) => true,
        MetaValue::Pattern(_) | MetaValue::Trigger(_)
        | MetaValue::Stmts(_) => true,
        MetaValue::Unit => false,
    }
}

fn as_int(v: &MetaValue) -> Result<i64, String> {
    match v {
        MetaValue::Int(n) => Ok(*n),
        MetaValue::Float(f) => Ok(f.round() as i64),
        MetaValue::Bool(b) => Ok(if *b { 1 } else { 0 }),
        _ => Err(format!("expected integer, got {:?}", v)),
    }
}

fn as_float(v: &MetaValue) -> Result<f32, String> {
    match v {
        MetaValue::Int(n) => Ok(*n as f32),
        MetaValue::Float(f) => Ok(*f),
        MetaValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(format!("expected number, got {:?}", v)),
    }
}

fn iter_items(v: &MetaValue) -> Result<Vec<MetaValue>, String> {
    match v {
        MetaValue::List(xs) => Ok(xs.clone()),
        MetaValue::Range { lo, hi, step } => {
            let mut out = Vec::new();
            let mut i = *lo;
            let mut guard: u64 = 0;
            if *step > 0 {
                while i < *hi {
                    out.push(MetaValue::Int(i));
                    i = i.saturating_add(*step);
                    guard += 1;
                    if guard as u32 > MAX_FOR_ITERATIONS {
                        return Err(format!(
                            "range exceeds {} items", MAX_FOR_ITERATIONS));
                    }
                }
            } else if *step < 0 {
                while i > *hi {
                    out.push(MetaValue::Int(i));
                    i = i.saturating_add(*step);
                    guard += 1;
                    if guard as u32 > MAX_FOR_ITERATIONS {
                        return Err(format!(
                            "range exceeds {} items", MAX_FOR_ITERATIONS));
                    }
                }
            }
            Ok(out)
        }
        _ => Err(format!("cannot iterate over {:?}", v)),
    }
}

fn eval_un(op: MetaUnOp, v: MetaValue) -> Result<MetaValue, String> {
    match op {
        MetaUnOp::Neg => match v {
            MetaValue::Int(n) => Ok(MetaValue::Int(n.saturating_neg())),
            MetaValue::Float(f) => Ok(MetaValue::Float(sanitize_float(-f))),
            _ => Err("neg on non number".into()),
        },
        MetaUnOp::Not => Ok(MetaValue::Bool(!truthy(&v))),
    }
}

fn eval_bin(op: MetaBinOp, a: MetaValue, b: MetaValue) -> Result<MetaValue, String> {
    let is_arith = matches!(op,
        MetaBinOp::Add | MetaBinOp::Sub | MetaBinOp::Mul
        | MetaBinOp::Div | MetaBinOp::Mod);
    let is_cmp = matches!(op,
        MetaBinOp::Eq | MetaBinOp::Ne | MetaBinOp::Lt
        | MetaBinOp::Le | MetaBinOp::Gt | MetaBinOp::Ge);

    if is_arith {
        if let (Ok(ai), Ok(bi)) = (as_int(&a), as_int(&b)) {
            let result = match op {
                MetaBinOp::Add => ai.saturating_add(bi),
                MetaBinOp::Sub => ai.saturating_sub(bi),
                MetaBinOp::Mul => ai.saturating_mul(bi),
                MetaBinOp::Div => {
                    if bi == 0 { 0 } else { ai.saturating_div(bi) }
                }
                MetaBinOp::Mod => {
                    if bi == 0 { 0 } else { ai.rem_euclid(bi.abs().max(1)) }
                }
                _ => unreachable!(),
            };
            return Ok(MetaValue::Int(result));
        }
        let af = as_float(&a)?;
        let bf = as_float(&b)?;
        return Ok(MetaValue::Float(sanitize_float(match op {
            MetaBinOp::Add => af + bf,
            MetaBinOp::Sub => af - bf,
            MetaBinOp::Mul => af * bf,
            MetaBinOp::Div => safe_div(af, bf),
            MetaBinOp::Mod => safe_mod(af, bf),
            _ => unreachable!(),
        })));
    }

    if is_cmp {
        if let (Ok(ai), Ok(bi)) = (as_int(&a), as_int(&b)) {
            return Ok(MetaValue::Bool(match op {
                MetaBinOp::Eq => ai == bi,
                MetaBinOp::Ne => ai != bi,
                MetaBinOp::Lt => ai <  bi,
                MetaBinOp::Le => ai <= bi,
                MetaBinOp::Gt => ai >  bi,
                MetaBinOp::Ge => ai >= bi,
                _ => unreachable!(),
            }));
        }
        let af = as_float(&a)?;
        let bf = as_float(&b)?;
        return Ok(MetaValue::Bool(match op {
            MetaBinOp::Eq => (af - bf).abs() < 1e-4,
            MetaBinOp::Ne => (af - bf).abs() >= 1e-4,
            MetaBinOp::Lt => af <  bf,
            MetaBinOp::Le => af <= bf,
            MetaBinOp::Gt => af >  bf,
            MetaBinOp::Ge => af >= bf,
            _ => unreachable!(),
        }));
    }

    match op {
        MetaBinOp::And => Ok(MetaValue::Bool(truthy(&a) && truthy(&b))),
        MetaBinOp::Or  => Ok(MetaValue::Bool(truthy(&a) || truthy(&b))),
        _ => unreachable!(),
    }
}

fn tier_atom(t: Tier) -> &'static str {
    match t {
        Tier::Rookie  => "Rookie",
        Tier::Casual  => "Casual",
        Tier::Adept   => "Adept",
        Tier::Skilled => "Skilled",
        Tier::Expert  => "Expert",
        Tier::ExpertPlus { plus: 1 } => "ExpertPlus",
        Tier::ExpertPlus { plus: 2 } => "ExpertPlus2",
        Tier::ExpertPlus { plus: 3 } => "ExpertPlus3",
        Tier::ExpertPlus { plus: _ } => "ExpertPlus4",
    }
}

fn atom_rank(s: &str) -> Option<i32> {
    match s {
        "Rookie" => Some(0),
        "Casual" => Some(1),
        "Adept"  => Some(2),
        "Skilled" => Some(3),
        "Expert" => Some(4),
        "ExpertPlus" | "ExpertPlus1" => Some(5),
        "ExpertPlus2" => Some(6),
        "ExpertPlus3" => Some(7),
        "ExpertPlus4" => Some(8),
        _ => None,
    }
}

/// Map a `RuleCategory` to the index used by the expander's per
/// category stack depth counters. Returns `None` for the `All`
/// variant since it has no single counter.
fn category_index(cat: RuleCategory) -> Option<usize> {
    match cat {
        RuleCategory::Ability  => Some(0),
        RuleCategory::Vision   => Some(1),
        RuleCategory::Cursor   => Some(2),
        RuleCategory::Survival => Some(3),
        RuleCategory::Input    => Some(4),
        RuleCategory::Score    => Some(5),
        RuleCategory::All      => None,
    }
}