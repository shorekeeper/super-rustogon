//! Meta language for compile time Turing complete level
//! authoring.
//!
//! The meta language layer sits between the user's source text
//! and the runtime generator. Its job is to let authors describe
//! level content with the full expressive power of a small
//! functional programming language while guaranteeing that:
//!
//! 1. Every meta program terminates. Recursive functions must
//!    carry a `decreases` clause and the expander verifies the
//!    expression is non negative before each recursive call, in
//!    combination with the shared expansion budget and the
//!    global parse deadline.
//! 2. Every computation fits in a finite budget. The expander
//!    spends budget on every node it visits and aborts loading
//!    with a readable error when the budget runs out.
//! 3. The resulting runtime AST is the same flat, deterministic,
//!    lookahead friendly structure the generator already knows.
//!    Everything that looks like control flow in the source text
//!    is resolved at load time into a straight line sequence of
//!    the same `Stmt::Emit / Wait / Trigger / Repeat / LocalVars`
//!    the generator has always consumed.
//!
//! The expander in `expander.rs` consumes instances of the types
//! defined here and emits runtime statements ready for
//! `LevelAst`.
//!
//! # Safety posture
//!
//! Meta values can only be numbers, booleans, strings, atoms,
//! ranges, lists, and pre built AST fragments. There is no eval,
//! no filesystem access, no network access, no reflection, no
//! way to invoke host functions. The worst a meta program can
//! do is take a bounded amount of parse time and then either
//! succeed or report an error.

use crate::dsl::ast::{ObstacleSpec, Stmt, TriggerSpec};
use crate::dsl::rules::{RuleCategory, RuleSet};

/// Runtime value of a meta level expression.
///
/// Everything a meta program can observe or produce is one of
/// these. No references, no user defined types, no reflection.
#[derive(Clone, Debug)]
pub enum MetaValue {
    Int(i64),
    Float(f32),
    Bool(bool),
    Str(String),
    Atom(String),
    /// Exclusive range `[lo, hi)` with a non zero integer step.
    Range { lo: i64, hi: i64, step: i64 },
    List(Vec<MetaValue>),
    /// Pre built obstacle specification. Returned by pattern
    /// bindings and consumed by `emit`.
    Pattern(ObstacleSpec),
    /// Pre built trigger specification. Returned by trigger stack
    /// members and consumed by the fire block.
    Trigger(TriggerSpec),
    /// Sequence of runtime statements. This is what meta
    /// functions produce when they are used from `@expand`.
    Stmts(Vec<Stmt>),
    /// Absence of a value, used as the default result for
    /// functions that emit statements rather than values.
    Unit,
}

/// Type annotation optionally attached to meta parameters and
/// variable declarations. Purely informational for now. The
/// expander checks values loosely against these so an author
/// gets a readable error before the runtime ever sees a bad
/// cast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaType {
    Int,
    Float,
    Bool,
    Str,
    Atom,
    Pattern,
    Trigger,
    Stmts,
    /// Type was not annotated. Works like a dynamic value.
    Unknown,
}

/// Meta level expression, evaluated by the expander without
/// side effects other than spending expansion budget.
#[derive(Clone, Debug)]
pub enum MetaExpr {
    Lit(MetaValue),
    Var(String),
    Un(MetaUnOp, Box<MetaExpr>),
    Bin(MetaBinOp, Box<MetaExpr>, Box<MetaExpr>),
    /// Function call. The expander looks `name` up in the
    /// function table registered during parsing.
    Call { name: String, args: Vec<MetaExpr> },
    /// Conditional expression, both branches pre parsed.
    If { cond: Box<MetaExpr>, then_: Box<MetaExpr>, else_: Box<MetaExpr> },
    /// Range literal `a..b` with optional step `a..b step s`.
    Range { lo: Box<MetaExpr>, hi: Box<MetaExpr>, step: Option<Box<MetaExpr>> },
    /// List literal `[a, b, c]`.
    List(Vec<MetaExpr>),
}

#[derive(Clone, Copy, Debug)]
pub enum MetaUnOp { Neg, Not }

#[derive(Clone, Copy, Debug)]
pub enum MetaBinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
}

/// One parameter of a meta level function.
#[derive(Clone, Debug)]
pub struct MetaParam {
    pub name: String,
    pub ty: MetaType,
}

/// Meta level function definition.
///
/// A function may be recursive, but only under two conditions.
/// First, the author must supply a `decreases` expression that
/// evaluates to a non negative integer at every call site.
/// Second, the shared expansion budget must cover every call.
/// Together these mean a recursive meta program either completes
/// in bounded time or fails to load with a clear error, never
/// hangs the editor.
#[derive(Clone, Debug)]
pub struct MetaFn {
    pub name: String,
    pub params: Vec<MetaParam>,
    pub return_ty: MetaType,
    /// Optional precondition, must evaluate to a truthy value
    /// on entry.
    pub requires: Option<MetaExpr>,
    /// Optional termination measure, must evaluate to a non
    /// negative integer on entry.
    pub decreases: Option<MetaExpr>,
    /// Function body, executed against a fresh scope seeded
    /// with the arguments.
    pub body: Vec<MetaStmt>,
}

/// Meta level statement. Executed during expansion. May append
/// runtime statements to the enclosing section's output buffer,
/// register hooks, or mutate the local scope.
#[derive(Clone, Debug)]
pub enum MetaStmt {
    /// Local binding `let x = expr`. Introduces a name visible
    /// to subsequent statements in the same scope.
    Let { name: String, ty: MetaType, value: MetaExpr },
    /// Compile time conditional. Exactly one arm runs.
    If { cond: MetaExpr, then_: Vec<MetaStmt>, else_: Vec<MetaStmt> },
    /// Compile time match. Exactly one arm runs.
    Match { scrutinee: MetaExpr, arms: Vec<MatchArm> },
    /// Bounded loop over a range or list. The iteration count
    /// is capped by `MAX_FOR_ITERATIONS` at expansion time.
    For { binder: String, iter: MetaExpr, body: Vec<MetaStmt> },
    /// Inline expansion of a meta function call. The call must
    /// produce a `Stmts` value; the statements are appended to
    /// the enclosing section in order.
    Expand(MetaExpr),
    /// Inline expansion of a named pattern. Equivalent to an
    /// `emit` of the pattern's bound obstacle spec.
    EmitPattern { name: String },
    /// Fire a named trigger stack in order at the current
    /// scheduling anchor.
    FireStack { name: String },
    /// Plain runtime statement (emit, wait, trigger, repeat,
    /// local vars). These pass through the expander unchanged,
    /// aside from any nested meta constructs they contain.
    Runtime(Stmt),
    /// Pipe chain of triggers that all fire at the same
    /// scheduling anchor. Parsed from `trigger :a |> :b ...`
    /// syntax in the v3 dialect. The expander emits one
    /// `Stmt::Trigger` per element in order so the runtime
    /// generator dispatches them onto the same beat.
    ///
    /// Replaces the earlier hack that wrapped piped triggers
    /// inside a `MetaStmt::If { cond: Lit(true), ... }` block
    /// (which polluted the AST with a meaningless conditional
    /// and made downstream tools treat the pipe as a branch).
    TriggerPipe(Vec<TriggerSpec>),
    /// `@on :event do ... end` hook. The expander lowers these
    /// to a side channel of runtime triggers attached to the
    /// enclosing section.
    Hook { event: HookEvent, body: Vec<TriggerSpec> },
    /// Track declaration. Declares a parallel sub timeline.
    /// Currently reserved; the expander rejects these so the
    /// feature can land in a future revision without breaking
    /// parse time semantics in the meantime.
    Track { name: String, body: Vec<MetaStmt> },
    /// Apply a rule set. Lowers to `Stmt::Rule` at expansion
    /// time, with the rule clamped through the category's
    /// `clamp` method.
    Rule(RuleSet),
    /// Revert a category. Lowers to `Stmt::Revert`.
    Revert(RuleCategory),
    /// Push a category snapshot. Lowers to `Stmt::Push`.
    Push(RuleCategory),
    /// Pop a category snapshot. Lowers to `Stmt::Pop`.
    Pop(RuleCategory),
}

/// One arm of a `match` statement.
#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Vec<MetaStmt>,
}

/// Pattern syntax for `match`. Kept simple: only atoms, atom
/// ranges, and wildcards. No destructuring. Extending this in
/// the future requires adding cases here and to the matcher
/// in `expander.rs` without touching the parser for existing
/// constructs.
#[derive(Clone, Debug)]
pub enum MatchPattern {
    Atom(String),
    AtomRange { lo: String, hi: String },
    Wildcard,
}

/// Hook event kind recognized by `@on`. The enum is closed on
/// purpose: every new hook kind requires a matching entry here
/// and a recognized state in the generator, which forces an
/// end to end audit before a hook can fire.
#[derive(Clone, Debug)]
pub enum HookEvent {
    /// Fires every `n` detected kick onsets while the section
    /// is active.
    OnsetEvery { n: u32 },
    /// Fires once when the named section becomes active.
    SectionEnter { name: String },
    /// Fires when the cumulative close call count reaches `n`.
    CloseCallCountAbove { n: u32 },
    /// Fires on every downbeat (onset index divisible by 4).
    BeatDownbeat,
    /// Fires on every onset inside a half open time window,
    /// both sides in seconds since track start.
    OnsetBetween { lo: f32, hi: f32 },
}

/// Attribute applied to a runtime statement via the `@[...]`
/// syntax. Stored verbatim so tools and future engine revisions
/// can extend the vocabulary without parser churn.
#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: String,
    pub args: Vec<MetaExpr>,
}