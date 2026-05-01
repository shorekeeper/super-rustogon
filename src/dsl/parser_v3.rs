//! Parser for the Rustogon Level Format v3.
//!
//! v3 is a strict superset of v2. Everything a v2 file can
//! express parses the same way. The new surface consists of:
//!
//! * `#[use_v3]` file directive, selected by the dispatcher
//!   in `mod.rs`.
//! * Top level `fn name(params) -> ret [requires e] [decreases e]
//!   do ... end` meta function definitions.
//! * Top level `pattern :name = obstacle_spec` bindings.
//! * Top level `trigger_stack :name do ... end` blocks.
//! * Section bodies may contain meta level constructs: `@if`,
//!   `match`, `@expand`, `for`, `@on`, plus the previous
//!   runtime statements.
//!
//! The parser reuses the v2 lexer with a few new keywords. All
//! safety limits defined in `safety.rs` apply: file size,
//! token count, formula depth, AST depth, parse deadline. The
//! parser itself never calls into the expander; it builds a
//! `MetaStmt` tree which the expander consumes afterwards.

use std::time::Instant;

use crate::dsl::ast::*;
use crate::dsl::expander::Expander;
use crate::dsl::formula;
use crate::dsl::meta::{
    Attribute, HookEvent, MatchArm, MatchPattern,
    MetaBinOp, MetaExpr, MetaFn, MetaParam, MetaStmt, MetaType,
    MetaUnOp, MetaValue,
};

use crate::dsl::rules::{
    AbilityKind, AbilityRule, CursorRule, InputRule,
    LevelRules, RuleCategory, RuleSet, ScoreRule,
    SurvivalRule, VisionRule,
};

use crate::dsl::safety::{
    self, MAX_FILE_BYTES, MAX_TOKEN_COUNT, MAX_STRING_LENGTH,
    MAX_SECTIONS, MAX_RECURSION_DEPTH,
};
use crate::levels::Palette;
use crate::levels::difficulty::Tier;

/// Error produced by `parse_level` on any syntactic or
/// semantic failure. Carries the offending source position so
/// editor tools can jump straight to the bad token.
///
/// Previously defined in `parser.rs` alongside the v1 parser;
/// moved here when v1 and v2 were retired. The field layout
/// and the `Display` formatting are unchanged so every caller
/// that printed these errors before keeps printing them the
/// same way.
#[derive(Debug)]
pub struct ParseError {
    pub msg:  String,
    pub line: u32,
    pub col:  u32,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}:{}: {}", self.line, self.col, self.msg)
    }
}

/// Entry point for the v3 parser. The caller in `mod.rs` has
/// already confirmed the file carries `#[use_v3]`.
pub fn parse_level(src: &str) -> Result<LevelAst, ParseError> {
    if src.len() > MAX_FILE_BYTES {
        return Err(ParseError {
            msg: format!(
                "file exceeds {} bytes", MAX_FILE_BYTES),
            line: 1, col: 1,
        });
    }
    let tokens = tokenize(src)?;
    if tokens.len() > MAX_TOKEN_COUNT {
        return Err(ParseError {
            msg: format!(
                "file has too many tokens (limit {})",
                MAX_TOKEN_COUNT),
            line: 1, col: 1,
        });
    }
    let start = Instant::now();
    let mut p = ParserV3 {
        toks: tokens,
        i: 0,
        depth: 0,
        start,
        shaders: Vec::new(),
    };
    p.parse_file()
}

// ---------- tokenizer ----------

#[derive(Clone, Debug)]
enum Tok {
    Ident(String),
    Atom(String),
    Str(String),
    Num(f32),
    Sigil(char, String),
    LBrace, RBrace,
    LParen, RParen,
    LBracket, RBracket,
    Eq, ColonColon, Pipe, Chain, DotDot, Arrow, FatArrow,
    Comma, At, Hash, Semi, Dot, Underscore,
    Eof,
}

#[derive(Clone, Debug)]
struct Token { kind: Tok, line: u32, col: u32 }

fn tokenize(src: &str) -> Result<Vec<Token>, ParseError> {
    let bytes: Vec<char> = src.chars().collect();
    let mut toks: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut col = 1u32;

    while i < bytes.len() {
        let c = bytes[i];

        if c == '\n' { line += 1; col = 1; i += 1; continue; }
        if c.is_whitespace() { col += 1; i += 1; continue; }

        // Line comments.
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
            while i < bytes.len() && bytes[i] != '\n' { i += 1; }
            continue;
        }
        if c == '-' && i + 1 < bytes.len() && bytes[i + 1] == '-' {
            while i < bytes.len() && bytes[i] != '\n' { i += 1; }
            continue;
        }
        if c == '#' && (i + 1 >= bytes.len() || bytes[i + 1] != '[') {
            while i < bytes.len() && bytes[i] != '\n' { i += 1; }
            continue;
        }

        let sl = line;
        let sc = col;

        // Multi character punctuation, longest match first.
        if c == ':' && i + 1 < bytes.len() && bytes[i + 1] == ':' {
            toks.push(Token { kind: Tok::ColonColon, line, col });
            i += 2; col += 2; continue;
        }
        if c == '=' && i + 1 < bytes.len() && bytes[i + 1] == '>' {
            toks.push(Token { kind: Tok::FatArrow, line, col });
            i += 2; col += 2; continue;
        }
        if c == '|' && i + 1 < bytes.len() && bytes[i + 1] == '>' {
            toks.push(Token { kind: Tok::Pipe, line, col });
            i += 2; col += 2; continue;
        }
        if c == '>' && i + 1 < bytes.len() && bytes[i + 1] == '>' {
            toks.push(Token { kind: Tok::Chain, line, col });
            i += 2; col += 2; continue;
        }
        if c == '<' && i + 1 < bytes.len() && bytes[i + 1] == '-' {
            toks.push(Token { kind: Tok::Arrow, line, col });
            i += 2; col += 2; continue;
        }
        if c == '.' && i + 1 < bytes.len() && bytes[i + 1] == '.' {
            toks.push(Token { kind: Tok::DotDot, line, col });
            i += 2; col += 2; continue;
        }

        // Single character punctuation.
        let single = match c {
            '{' => Some(Tok::LBrace),
            '}' => Some(Tok::RBrace),
            '(' => Some(Tok::LParen),
            ')' => Some(Tok::RParen),
            '[' => Some(Tok::LBracket),
            ']' => Some(Tok::RBracket),
            '=' => Some(Tok::Eq),
            ',' => Some(Tok::Comma),
            '@' => Some(Tok::At),
            '#' => Some(Tok::Hash),
            ';' => Some(Tok::Semi),
            '.' => Some(Tok::Dot),
            _ => None,
        };
        if let Some(k) = single {
            toks.push(Token { kind: k, line, col });
            i += 1; col += 1;
            continue;
        }

        // Sigil. `~x"..."` with `x` one ASCII letter.
        if c == '~' && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_alphabetic()
            && bytes[i + 2] == '"'
        {
            let letter = bytes[i + 1];
            i += 3; col += 3;
            let mut s = String::new();
            while i < bytes.len() && bytes[i] != '"' {
                if bytes[i] == '\n' {
                    return Err(ParseError {
                        msg: "unterminated sigil body".into(),
                        line: sl, col: sc,
                    });
                }
                s.push(bytes[i]); i += 1; col += 1;
                if s.len() > MAX_STRING_LENGTH {
                    return Err(ParseError {
                        msg: "sigil body too long".into(),
                        line: sl, col: sc,
                    });
                }
            }
            if i >= bytes.len() {
                return Err(ParseError {
                    msg: "unterminated sigil body".into(),
                    line: sl, col: sc,
                });
            }
            i += 1; col += 1;
            toks.push(Token {
                kind: Tok::Sigil(letter, s), line: sl, col: sc,
            });
            continue;
        }

        // Atom `:ident`.
        if c == ':' && i + 1 < bytes.len()
            && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == '_')
        {
            i += 1; col += 1;
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_')
            { i += 1; col += 1; }
            let s: String = bytes[start..i].iter().collect();
            toks.push(Token { kind: Tok::Atom(s), line: sl, col: sc });
            continue;
        }

        // String literal.
        if c == '"' {
            i += 1; col += 1;
            let mut s = String::new();
            while i < bytes.len() && bytes[i] != '"' {
                if bytes[i] == '\n' {
                    return Err(ParseError {
                        msg: "unterminated string".into(),
                        line: sl, col: sc,
                    });
                }
                s.push(bytes[i]); i += 1; col += 1;
                if s.len() > MAX_STRING_LENGTH {
                    return Err(ParseError {
                        msg: "string literal too long".into(),
                        line: sl, col: sc,
                    });
                }
            }
            if i >= bytes.len() {
                return Err(ParseError {
                    msg: "unterminated string".into(),
                    line: sl, col: sc,
                });
            }
            i += 1; col += 1;
            toks.push(Token { kind: Tok::Str(s), line: sl, col: sc });
            continue;
        }

        // Number.
        if c == '-' || c.is_ascii_digit() {
            let mut j = i;
            if bytes[j] == '-' { j += 1; }
            if j + 1 < bytes.len() && bytes[j] == '0'
                && (bytes[j + 1] == 'x' || bytes[j + 1] == 'X')
            {
                let start = j + 2;
                let mut k = start;
                while k < bytes.len() && bytes[k].is_ascii_hexdigit() { k += 1; }
                if k == start {
                    return Err(ParseError {
                        msg: "bad hex literal".into(),
                        line: sl, col: sc,
                    });
                }
                let s: String = bytes[start..k].iter().collect();
                let v = u32::from_str_radix(&s, 16)
                    .map_err(|_| ParseError {
                        msg: "hex overflow".into(),
                        line: sl, col: sc,
                    })?;
                let n = if bytes[i] == '-' { -(v as i64) as f32 }
                        else { v as f32 };
                let n = safety::check_finite(n, "numeric literal")
                    .map_err(|m| ParseError {
                        msg: m, line: sl, col: sc,
                    })?;
                toks.push(Token { kind: Tok::Num(n), line: sl, col: sc });
                col += (k - i) as u32;
                i = k;
                continue;
            }
            while j < bytes.len()
                && (bytes[j].is_ascii_digit() || bytes[j] == '.')
            { j += 1; }
            let s: String = bytes[i..j].iter().collect();
            if s == "-" {
                return Err(ParseError {
                    msg: "stray '-'".into(),
                    line: sl, col: sc,
                });
            }
            let n: f32 = s.parse().map_err(|_| ParseError {
                msg: format!("bad number '{}'", s),
                line: sl, col: sc,
            })?;
            let n = safety::check_finite(n, "numeric literal")
                .map_err(|m| ParseError {
                    msg: m, line: sl, col: sc,
                })?;
            toks.push(Token { kind: Tok::Num(n), line: sl, col: sc });
            col += (j - i) as u32;
            i = j;
            continue;
        }

        // Identifier. The underscore is a distinct token so
        // match patterns can use it as wildcard.
        if c == '_' {
            // Could be a real identifier like `_foo` or the bare
            // wildcard `_`. Disambiguate by lookahead.
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_')
            { j += 1; }
            if j == i + 1 {
                toks.push(Token { kind: Tok::Underscore, line: sl, col: sc });
                i = j; col += 1;
                continue;
            }
            let s: String = bytes[i..j].iter().collect();
            toks.push(Token { kind: Tok::Ident(s), line: sl, col: sc });
            col += (j - i) as u32;
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_')
            { j += 1; }
            let s: String = bytes[i..j].iter().collect();
            toks.push(Token { kind: Tok::Ident(s), line: sl, col: sc });
            col += (j - i) as u32;
            i = j;
            continue;
        }

        return Err(ParseError {
            msg: format!("unexpected character '{}'", c),
            line, col,
        });
    }

    toks.push(Token { kind: Tok::Eof, line, col });
    Ok(toks)
}

// ---------- parser ----------

#[derive(Clone, Copy, Debug)]
enum BlockKind { Brace, Do }

struct ParserV3 {
    toks: Vec<Token>,
    i: usize,
    depth: usize,
    start: Instant,
    /// Shader declarations seen so far. `PostShader`
    /// triggers look up their slot index here during parse.
    shaders: Vec<crate::dsl::ast::ShaderDecl>,
}

impl ParserV3 {
    fn peek(&self) -> &Token { &self.toks[self.i] }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.i].clone();
        self.i += 1;
        t
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, ParseError> {
        let t = self.peek();
        Err(ParseError { msg: msg.into(), line: t.line, col: t.col })
    }
    fn err_owned(&self, msg: impl Into<String>) -> ParseError {
        let t = self.peek();
        ParseError { msg: msg.into(), line: t.line, col: t.col }
    }

    fn check_deadline(&self) -> Result<(), ParseError> {
        let elapsed = self.start.elapsed().as_millis() as u64;
        if elapsed > safety::MAX_PARSE_DURATION_MS {
            return Err(ParseError {
                msg: format!(
                    "parse deadline exceeded ({} ms)",
                    safety::MAX_PARSE_DURATION_MS),
                line: 0, col: 0,
            });
        }
        Ok(())
    }

    fn down(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_RECURSION_DEPTH {
            return Err(self.err_owned(format!(
                "parser recursion depth exceeded {}",
                MAX_RECURSION_DEPTH)));
        }
        Ok(())
    }
    fn up(&mut self) { if self.depth > 0 { self.depth -= 1; } }

    fn expect_kw(&mut self, kw: &str) -> Result<(), ParseError> {
        let t = self.bump();
        match &t.kind {
            Tok::Ident(s) if s == kw => Ok(()),
            _ => Err(ParseError {
                msg: format!("expected '{}'", kw),
                line: t.line, col: t.col,
            }),
        }
    }
    fn read_string(&mut self) -> Result<String, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Str(s) => Ok(s),
            _ => Err(ParseError {
                msg: "expected string literal".into(),
                line: t.line, col: t.col,
            }),
        }
    }
    fn read_ident(&mut self) -> Result<String, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Ident(s) => Ok(s),
            _ => Err(ParseError {
                msg: "expected identifier".into(),
                line: t.line, col: t.col,
            }),
        }
    }
    fn read_atom(&mut self) -> Result<String, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Atom(s) | Tok::Ident(s) => Ok(s),
            _ => Err(ParseError {
                msg: "expected atom".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    fn open_block(&mut self) -> Result<BlockKind, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::LBrace => Ok(BlockKind::Brace),
            Tok::Ident(ref s) if s == "do" => Ok(BlockKind::Do),
            _ => Err(ParseError {
                msg: "expected '{' or 'do'".into(),
                line: t.line, col: t.col,
            }),
        }
    }
    fn is_block_end(&self, kind: BlockKind) -> bool {
        match (kind, &self.peek().kind) {
            (BlockKind::Brace, Tok::RBrace) => true,
            (BlockKind::Do,    Tok::Ident(s)) if s == "end" => true,
            _ => false,
        }
    }
    fn close_block(&mut self, kind: BlockKind) -> Result<(), ParseError> {
        let t = self.bump();
        match (kind, &t.kind) {
            (BlockKind::Brace, Tok::RBrace) => Ok(()),
            (BlockKind::Do,    Tok::Ident(s)) if s == "end" => Ok(()),
            _ => Err(ParseError {
                msg: "expected '}' or 'end'".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    fn skip_opt_eq(&mut self) {
        if matches!(self.peek().kind, Tok::Eq) { self.bump(); }
    }

    // ---------- top level ----------

    fn parse_file(&mut self) -> Result<LevelAst, ParseError> {
        // Skip leading directives. The dispatcher already took
        // care of selecting v3 but we have to consume them so
        // the level keyword is reachable.
        let mut start_from: f32 = 0.0;
        let mut timestamp_format = TimestampFormat::default(); 
        let mut level_rules = LevelRules::default();
        let mut ignore_collisions = false;
        while matches!(self.peek().kind, Tok::Hash) {
            self.bump();
            let t = self.bump();
            if !matches!(t.kind, Tok::LBracket) {
                return Err(ParseError {
                    msg: "expected '[' after '#'".into(),
                    line: t.line, col: t.col,
                });
            }
            let name = self.read_ident()?;
            // Debug directive. Parameter-less, so we only need to
            // confirm the closing ']' and move on. Any level
            // shipping with this on is an authoring build; the
            // runtime still accepts it because stripping the
            // directive from a shipped file is a trivial edit.
            if name == "ignore_collisions" {
                let t = self.bump();
                if !matches!(t.kind, Tok::RBracket) {
                    return Err(ParseError {
                        msg: "expected ']' after ignore_collisions".into(),
                        line: t.line, col: t.col,
                    });
                }
                ignore_collisions = true;
                continue;
            }
            // `#[<category> { ... }]` declarations populate the
            // file level rule baseline. We pattern match the
            // category name first so a typo gets a proper
            // ParseError instead of silently degrading to a
            // no op directive.
            if let Some(cat) = rule_category_from_ident(&name) {
                // Field map uses the same syntax as runtime
                // obstacle / trigger blocks, minus the leading
                // brace since we are already inside `[`.
                let rule = self.parse_rule_fields_inline(cat)?;
                level_rules.merge(rule);
                let t = self.bump();
                if !matches!(t.kind, Tok::RBracket) {
                    return Err(ParseError {
                        msg: "expected ']'".into(),
                        line: t.line, col: t.col,
                    });
                }
                continue;
            }

            // Fallback: the old v2 directive set (timestamps,
            // startfrom). Positional numeric after the name or
            // k=v pairs, same shape as before.
            if matches!(self.peek().kind, Tok::Eq) { self.bump(); }
            let mut positional: Option<f32> = None;
            if matches!(self.peek().kind, Tok::Num(_) | Tok::Sigil('t', _)) {
                positional = Some(self.read_time_or_number()?);
            }
            let mut kv: Vec<(String, f32)> = Vec::new();
            while !matches!(self.peek().kind, Tok::RBracket) {
                if matches!(self.peek().kind, Tok::Comma) {
                    self.bump(); continue;
                }
                let k = self.read_ident()?;
                self.skip_opt_eq();
                let v = self.read_time_or_number()?;
                kv.push((k, v));
            }
            let t = self.bump();
            if !matches!(t.kind, Tok::RBracket) {
                return Err(ParseError {
                    msg: "expected ']'".into(),
                    line: t.line, col: t.col,
                });
            }
            if name == "startfrom" {
                let v = positional.or_else(|| kv.iter()
                    .find(|(k, _)| k == "at" || k == "seconds")
                    .map(|(_, v)| *v));
                if let Some(v) = v { start_from = v.max(0.0); }
            } else if let Some(fmt) = parse_timestamp_directive_v3(&name, &kv) {
                timestamp_format = fmt;
            }
        }

        self.expect_kw("level")?;
        let header_name = self.read_string()?;
        let kind = self.open_block()?;

        let mut meta       = Meta::default();
        let mut palette    = Palette::default();
        let mut difficulty = DifficultySpec::default();
        let mut generation = GenerationSpec::default();
        let mut sections: Vec<(Section, Vec<MetaStmt>)> = Vec::new();

        // Temporary lookup table of meta functions, patterns
        // and trigger stacks, used while parsing section bodies
        // and handed off to the expander afterwards.
        let mut globals: Vec<VarDecl> = Vec::new();
        let mut functions: Vec<MetaFn> = Vec::new();
        let mut patterns:  Vec<(String, ObstacleSpec)> = Vec::new();
        let mut stacks:    Vec<(String, Vec<TriggerSpec>)> = Vec::new();

        while !self.is_block_end(kind) {
            self.check_deadline()?;
            match self.peek().kind.clone() {
                Tok::Comma => { self.bump(); continue; }
                Tok::Ident(kw) => match kw.as_str() {
                    "meta" => {
                        self.bump();
                        meta = self.parse_meta()?;
                    }
                    "palette" => {
                        self.bump();
                        palette = self.parse_palette()?;
                    }
                    "difficulty" => {
                        self.bump();
                        difficulty = self.parse_difficulty()?;
                    }
                    "global" => {
                        self.bump();
                        let decls = self.parse_global_block()?;
                        for d in decls {
                            globals.push(d);
                        }
                    }
                    "generation" => {
                        self.bump();
                        generation = self.parse_generation()?;
                    }
                    "fn" => {
                        self.bump();
                        functions.push(self.parse_fn()?);
                    }
                    "pattern" => {
                        self.bump();
                        let (n, spec) = self.parse_pattern_binding()?;
                        patterns.push((n, spec));
                    }
                    "trigger_stack" => {
                        self.bump();
                        let (n, stack) = self.parse_trigger_stack()?;
                        stacks.push((n, stack));
                    }
                    "shader" => {
                        self.bump();
                        let decl = self.parse_shader_binding()?;
                        if self.shaders.len() >= 256 {
                            return self.err("too many shaders (limit 256)");
                        }
                        self.shaders.push(decl);
                    }
                    "section" => {
                        self.bump();
                        let sec = self.parse_section()?;
                        if sections.len() >= MAX_SECTIONS {
                            return self.err(format!(
                                "too many sections (limit {})",
                                MAX_SECTIONS));
                        }
                        sections.push(sec);
                    }
                    other => return self.err(format!(
                        "unknown top level block '{}'", other)),
                },
                _ => return self.err("expected block keyword"),
            }
        }
        self.close_block(kind)?;

        if !matches!(self.peek().kind, Tok::Eof) {
            return self.err("trailing tokens after level block");
        }

        if meta.name.is_empty() { meta.name = header_name; }
        if sections.is_empty() {
            sections.push((Section {
                name: "default".into(), at: 0.0,
                body: vec![Stmt::Emit(ObstacleSpec::Bar {
                    thickness_mult: 1.0,
                }), Stmt::Wait(2)],
            }, Vec::new()));
        }

        // Build an expander seeded with everything declared at
        // top level, then lower each section's meta body into
        // runtime statements.
        let mut expander = Expander::new(
            difficulty.base_tier, generation.sides, meta.bpm);
        for f in functions { expander.register_fn(f); }
        for (n, s) in patterns { expander.register_pattern(n, s); }
        for (n, s) in stacks { expander.register_trigger_stack(n, s); }
        for g in &globals { expander.register_global(g); }
        let mut final_sections: Vec<Section> = Vec::new();
        for (mut sec, meta_body) in sections {
            self.check_deadline()?;
            let output = expander
                .expand_section(sec.name.clone(), meta_body)
                .map_err(|m| self.err_owned(m))?;
            sec.body = output.stmts;
            final_sections.push(sec);
        }

        final_sections.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap());

        Ok(LevelAst {
            meta, palette, difficulty, generation,
            timestamp_format,
            globals,
            ignore_collisions,
            sections: final_sections,
            start_from_seconds: start_from,
            rules: level_rules,
            shaders: std::mem::take(&mut self.shaders),
        })
    }

    // ---------- meta, palette, difficulty, generation ----------

    fn parse_meta(&mut self) -> Result<Meta, ParseError> {
        let kind = self.open_block()?;
        let mut m = Meta::default();
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            let field = self.read_ident()?;
            self.skip_opt_eq();
            match field.as_str() {
                "name"        => m.name        = self.read_stringish()?,
                "subtitle"    => m.subtitle    = self.read_stringish()?,
                "author"      => m.author      = self.read_stringish()?,
                "song"        => m.song        = self.read_stringish()?,
                "music"       => m.music       = self.read_stringish()?,
                "description" => m.description = self.read_stringish()?,
                "bpm" => {
                    let n = self.read_time_or_number()?;
                    if !(40.0..=300.0).contains(&n) {
                        return self.err(format!("bpm out of range: {}", n));
                    }
                    m.bpm = n as u32;
                }
                other => return self.err(format!(
                    "unknown meta field '{}'", other)),
            }
        }
        self.close_block(kind)?;
        Ok(m)
    }

    fn parse_palette(&mut self) -> Result<Palette, ParseError> {
        let kind = self.open_block()?;
        let mut p = Palette::default();
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            let field = self.read_ident()?;
            self.skip_opt_eq();
            let color = self.read_rgb()?;
            match field.as_str() {
                "bgA" | "bg_a"               => p.bg_a        = color,
                "bgB" | "bg_b"               => p.bg_b        = color,
                "centerFill" | "center_fill" => p.center_fill = color,
                "centerRing" | "center_ring" => p.center_ring = color,
                "wall"                       => p.wall        = color,
                "player"                     => p.player      = color,
                "accent"                     => p.accent      = color,
                other => return self.err(format!(
                    "unknown palette field '{}'", other)),
            }
        }
        self.close_block(kind)?;
        Ok(p)
    }

    fn read_rgb(&mut self) -> Result<[f32; 3], ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Ident(ref s) if s == "rgb" => {
                let paren = matches!(self.peek().kind, Tok::LParen);
                if paren { self.bump(); }
                let r = self.read_time_or_number()?.clamp(0.0, 1.0);
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); }
                let g = self.read_time_or_number()?.clamp(0.0, 1.0);
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); }
                let b = self.read_time_or_number()?.clamp(0.0, 1.0);
                if paren {
                    let t2 = self.bump();
                    if !matches!(t2.kind, Tok::RParen) {
                        return Err(ParseError {
                            msg: "expected ')' after rgb".into(),
                            line: t2.line, col: t2.col,
                        });
                    }
                }
                Ok([r, g, b])
            }
            _ => Err(ParseError {
                msg: "expected 'rgb(r, g, b)'".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    fn parse_difficulty(&mut self) -> Result<DifficultySpec, ParseError> {
        let kind = self.open_block()?;
        let mut d = DifficultySpec::default();
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            let field = self.read_ident()?;
            self.skip_opt_eq();
            if field == "range" {
                let lo = self.read_atom()?;
                let lo_t = Tier::from_keyword(&lo).ok_or_else(|| {
                    self.err_owned(format!("unknown tier '{}'", lo))
                })?;
                let t = self.bump();
                if !matches!(t.kind, Tok::DotDot) {
                    return Err(ParseError {
                        msg: "expected '..' in range".into(),
                        line: t.line, col: t.col,
                    });
                }
                let hi = self.read_atom()?;
                let hi_t = Tier::from_keyword(&hi).ok_or_else(|| {
                    self.err_owned(format!("unknown tier '{}'", hi))
                })?;
                d.min_tier = lo_t;
                d.max_tier = hi_t;
                continue;
            }
            let tname = self.read_atom()?;
            let tier = Tier::from_keyword(&tname).ok_or_else(|| {
                self.err_owned(format!("unknown tier '{}'", tname))
            })?;
            match field.as_str() {
                "base" | "baseTier" => d.base_tier = tier,
                "min"  | "minTier"  => d.min_tier  = tier,
                "max"  | "maxTier"  => d.max_tier  = tier,
                other => return self.err(format!(
                    "unknown difficulty field '{}'", other)),
            }
        }
        self.close_block(kind)?;
        Ok(d)
    }

    fn parse_generation(&mut self) -> Result<GenerationSpec, ParseError> {
        let kind = self.open_block()?;
        let mut g = GenerationSpec::default();
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            let field = self.read_ident()?;
            self.skip_opt_eq();
            match field.as_str() {
                "sides" => {
                    let n = self.read_time_or_number()?;
                    if !(3.0..=12.0).contains(&n) {
                        return self.err(format!("sides out of range: {}", n));
                    }
                    g.sides = n as u32;
                }
                "seed" => g.seed = self.read_time_or_number()? as u32,
                "hueSpeed" | "hue_speed" =>
                    g.hue_speed = self.read_time_or_number()?.clamp(-4.0, 4.0),
                "speedMult" | "speed" =>
                    g.speed_mult = self.read_time_or_number()?.clamp(0.25, 3.0),
                "densityMult" | "density" =>
                    g.density_mult = self.read_time_or_number()?.clamp(0.25, 3.0),
                other => return self.err(format!(
                    "unknown generation field '{}'", other)),
            }
        }
        self.close_block(kind)?;
        Ok(g)
    }

    // ---------- fn, pattern, trigger stack ----------

    /// Parse a `global do ... end` block, collecting
    /// `var name :: type = value [access]` declarations into
    /// a flat vector. The vector is stored on the `LevelAst`
    /// for round tripping through the serializer and also
    /// registered one by one with the expander so meta
    /// expressions in section bodies can read them.
    fn parse_global_block(&mut self) -> Result<Vec<VarDecl>, ParseError> {
        let kind = self.open_block()?;
        let mut decls: Vec<VarDecl> = Vec::new();
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma | Tok::Semi) {
                self.bump();
                continue;
            }
            self.expect_kw("var")?;
            let d = self.parse_ast_var_decl()?;
            decls.push(d);
            if decls.len() > 64 {
                return self.err("too many globals (limit 64)");
            }
        }
        self.close_block(kind)?;
        Ok(decls)
    }

    /// Read one `var` declaration body: identifier, optional
    /// `:: type`, mandatory `=`, value, optional `[access]`
    /// modifiers. Returns a `VarDecl` already type-checked
    /// against its declared type.
    fn parse_ast_var_decl(&mut self) -> Result<VarDecl, ParseError> {
        let name = self.read_ident()?;

        let mut ty = VarType::Float;
        if matches!(self.peek().kind, Tok::ColonColon) {
            self.bump();
            let tn = self.read_ident()?;
            ty = match tn.as_str() {
                "i32" | "int"    => VarType::Int,
                "f32" | "float"  => VarType::Float,
                "str" | "string" => VarType::String,
                "ident"          => VarType::Ident,
                "bool"           => VarType::Bool,
                other => return self.err(format!("unknown type '{}'", other)),
            };
        }

        let t = self.bump();
        if !matches!(t.kind, Tok::Eq) {
            return Err(ParseError {
                msg: "expected '=' in var declaration".into(),
                line: t.line, col: t.col,
            });
        }
        let value = self.read_ast_var_value()?;

        let mut access = VarAccess::Public;
        if matches!(self.peek().kind, Tok::LBracket) {
            self.bump();
            while !matches!(self.peek().kind, Tok::RBracket) {
                if matches!(self.peek().kind, Tok::Comma) {
                    self.bump();
                    continue;
                }
                let a = self.read_ident()?;
                access = match a.as_str() {
                    "pub"  | "public"   => VarAccess::Public,
                    "priv" | "private"  => VarAccess::Private,
                    "read" | "readonly" => VarAccess::ReadOnly,
                    other => return self.err(format!(
                        "unknown access modifier '{}'", other)),
                };
            }
            self.bump();
        }

        let ok = match (&value, ty) {
            (VarValue::Num(_),   VarType::Int   | VarType::Float) => true,
            (VarValue::Str(_),   VarType::String)                 => true,
            (VarValue::Ident(_), VarType::Ident | VarType::String)=> true,
            (VarValue::Bool(_),  VarType::Bool)                   => true,
            _ => false,
        };
        if !ok {
            return self.err(format!(
                "value {:?} does not match declared type {:?}", value, ty));
        }

        Ok(VarDecl { name, ty, value, access })
    }

    /// Read one `var` declaration's value. Separate from
    /// `read_var_value` because that helper targets meta
    /// expressions and returns `MetaValue`; globals live in
    /// the runtime AST and need plain `VarValue` instead.
    fn read_ast_var_value(&mut self) -> Result<VarValue, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n)      => Ok(VarValue::Num(n)),
            Tok::Str(s)      => Ok(VarValue::Str(s)),
            Tok::Sigil(letter, s) => {
                if letter == 't' {
                    parse_time_sigil(&s).map(VarValue::Num)
                        .map_err(|m| ParseError {
                            msg: m, line: t.line, col: t.col,
                        })
                } else {
                    Ok(VarValue::Str(s))
                }
            }
            Tok::Atom(s)     => Ok(VarValue::Ident(s)),
            Tok::Ident(s) => match s.as_str() {
                "true"  => Ok(VarValue::Bool(true)),
                "false" => Ok(VarValue::Bool(false)),
                _       => Ok(VarValue::Ident(s)),
            }
            _ => Err(ParseError {
                msg: "expected value".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    fn parse_fn(&mut self) -> Result<MetaFn, ParseError> {
        let name = self.read_ident()?;
        let t = self.bump();
        if !matches!(t.kind, Tok::LParen) {
            return Err(ParseError {
                msg: "expected '(' after fn name".into(),
                line: t.line, col: t.col,
            });
        }
        let mut params: Vec<MetaParam> = Vec::new();
        while !matches!(self.peek().kind, Tok::RParen) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            let n = self.read_ident()?;
            let mut ty = MetaType::Unknown;
            if matches!(self.peek().kind, Tok::ColonColon) {
                self.bump();
                let tn = self.read_ident()?;
                ty = type_from_ident(&tn);
            }
            params.push(MetaParam { name: n, ty });
            if params.len() > 16 {
                return self.err("too many parameters (limit 16)");
            }
        }
        self.bump();

        let mut return_ty = MetaType::Unknown;
        if matches!(self.peek().kind, Tok::Arrow) {
            self.bump();
            let tn = self.read_ident()?;
            return_ty = type_from_ident(&tn);
        }

        let mut requires = None;
        let mut decreases = None;
        loop {
            match &self.peek().kind {
                Tok::Ident(s) if s == "requires" => {
                    self.bump();
                    requires = Some(self.parse_meta_expr()?);
                }
                Tok::Ident(s) if s == "decreases" => {
                    self.bump();
                    decreases = Some(self.parse_meta_expr()?);
                }
                _ => break,
            }
        }

        let kind = self.open_block()?;
        let body = self.parse_meta_body(kind)?;
        self.close_block(kind)?;

        Ok(MetaFn { name, params, return_ty, requires, decreases, body })
    }

    fn parse_pattern_binding(&mut self) -> Result<(String, ObstacleSpec), ParseError> {
        let name = self.read_atom()?;
        let t = self.bump();
        if !matches!(t.kind, Tok::Eq) {
            return Err(ParseError {
                msg: "expected '=' in pattern binding".into(),
                line: t.line, col: t.col,
            });
        }
        let spec = self.parse_obstacle()?;
        Ok((name, spec))
    }

    fn parse_trigger_stack(&mut self) -> Result<(String, Vec<TriggerSpec>), ParseError> {
        let name = self.read_atom()?;
        let kind = self.open_block()?;
        let mut out: Vec<TriggerSpec> = Vec::new();
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            out.push(self.parse_trigger_stage()?);
            if out.len() > 32 {
                return self.err("too many triggers in stack (limit 32)");
            }
        }
        self.close_block(kind)?;
        Ok((name, out))
    }

    /// Parse a `shader :name = ~p"path"` declaration. The
    /// path is stored verbatim; the main loop resolves it
    /// at level load time through the shader sandbox.
    fn parse_shader_binding(&mut self) -> Result<crate::dsl::ast::ShaderDecl, ParseError> {
        let name = self.read_atom()?;
        let t = self.bump();
        if !matches!(t.kind, Tok::Eq) {
            return Err(ParseError {
                msg: "expected '=' in shader binding".into(),
                line: t.line, col: t.col,
            });
        }
        let t = self.bump();
        let path = match t.kind {
            Tok::Sigil('p', s) => s,
            Tok::Str(s)        => s,
            _ => return Err(ParseError {
                msg: "expected ~p\"...\" or \"...\" path".into(),
                line: t.line, col: t.col,
            }),
        };
        Ok(crate::dsl::ast::ShaderDecl { name, path })
    }

    // ---------- section ----------

    fn parse_section(&mut self) -> Result<(Section, Vec<MetaStmt>), ParseError> {
        let name = self.read_string()?;
        self.expect_kw("at")?;
        let at = self.read_time_or_number()?;
        let kind = self.open_block()?;
        let body = self.parse_meta_body(kind)?;
        self.close_block(kind)?;
        Ok((Section { name, at, body: Vec::new() }, body))
    }

    fn parse_meta_body(&mut self, kind: BlockKind) -> Result<Vec<MetaStmt>, ParseError> {
        let mut out: Vec<MetaStmt> = Vec::new();
        while !self.is_block_end(kind) {
            self.check_deadline()?;
            self.down()?;
            if matches!(self.peek().kind, Tok::Comma | Tok::Semi) {
                self.bump();
                self.up();
                continue;
            }
            out.push(self.parse_meta_stmt()?);
            self.up();
            if out.len() > safety::MAX_STMTS_PER_SECTION {
                return self.err(format!(
                    "section has too many meta statements (limit {})",
                    safety::MAX_STMTS_PER_SECTION));
            }
        }
        Ok(out)
    }

    fn parse_meta_stmt(&mut self) -> Result<MetaStmt, ParseError> {
        // `@` introduced forms.
        if matches!(self.peek().kind, Tok::At) {
            return self.parse_at_form();
        }
        match &self.peek().kind {
            Tok::Ident(s) if s == "let" => {
                self.bump();
                let name = self.read_ident()?;
                let mut ty = MetaType::Unknown;
                if matches!(self.peek().kind, Tok::ColonColon) {
                    self.bump();
                    let tn = self.read_ident()?;
                    ty = type_from_ident(&tn);
                }
                let t = self.bump();
                if !matches!(t.kind, Tok::Eq) {
                    return Err(ParseError {
                        msg: "expected '=' in let".into(),
                        line: t.line, col: t.col,
                    });
                }
                let value = self.parse_meta_expr()?;
                Ok(MetaStmt::Let { name, ty, value })
            }
            Tok::Ident(s) if s == "match" => {
                self.bump();
                let scrutinee = self.parse_meta_expr()?;
                let kind = self.open_block()?;
                let mut arms: Vec<MatchArm> = Vec::new();
                while !self.is_block_end(kind) {
                    if matches!(self.peek().kind, Tok::Comma) {
                        self.bump();
                        continue;
                    }
                    let pat = self.parse_match_pattern()?;
                    let t = self.bump();
                    if !matches!(t.kind, Tok::FatArrow) {
                        return Err(ParseError {
                            msg: "expected '=>' in match arm".into(),
                            line: t.line, col: t.col,
                        });
                    }
                    let body = if matches!(self.peek().kind, Tok::LBrace)
                        || matches!(&self.peek().kind, Tok::Ident(s) if s == "do")
                    {
                        let k = self.open_block()?;
                        let b = self.parse_meta_body(k)?;
                        self.close_block(k)?;
                        b
                    } else {
                        vec![self.parse_meta_stmt()?]
                    };
                    arms.push(MatchArm { pattern: pat, body });
                }
                self.close_block(kind)?;
                Ok(MetaStmt::Match { scrutinee, arms })
            }
            Tok::Ident(s) if s == "for" => {
                self.bump();
                let binder = self.read_ident()?;
                self.expect_kw("in")?;
                let iter = self.parse_meta_expr()?;
                let k = self.open_block()?;
                let body = self.parse_meta_body(k)?;
                self.close_block(k)?;
                Ok(MetaStmt::For { binder, iter, body })
            }
            Tok::Ident(s) if s == "emit_pattern" => {
                self.bump();
                let n = self.read_atom()?;
                Ok(MetaStmt::EmitPattern { name: n })
            }
            Tok::Ident(s) if s == "fire" => {
                self.bump();
                let n = self.read_atom()?;
                Ok(MetaStmt::FireStack { name: n })
            }
            Tok::Ident(s) if s == "emit" => {
                self.bump();
                let spec = self.parse_obstacle()?;
                Ok(MetaStmt::Runtime(Stmt::Emit(spec)))
            }
            Tok::Ident(s) if s == "wait" => {
                self.bump();
                let n = self.read_time_or_number()?;
                if !(1.0..=64.0).contains(&n) {
                    return self.err(format!("wait out of range: {}", n));
                }
                Ok(MetaStmt::Runtime(Stmt::Wait(n as u32)))
            }
            Tok::Ident(s) if s == "trigger" => {
                self.bump();
                // Parse the head trigger, then optionally a
                // chain of `|>` piped trigger stages. Single
                // triggers lower to a plain `Stmt::Trigger`;
                // pipes lower to `MetaStmt::TriggerPipe` which
                // the expander flattens into a contiguous run
                // of `Stmt::Trigger` statements at the same
                // scheduling anchor. Replaces the previous hack
                // that wrapped pipes in `MetaStmt::If { cond:
                // Lit(true), ... }`.
                let first = self.parse_trigger_stage()?;
                if matches!(self.peek().kind, Tok::Pipe) {
                    let mut pipe: Vec<TriggerSpec> = Vec::with_capacity(4);
                    pipe.push(first);
                    while matches!(self.peek().kind, Tok::Pipe) {
                        self.bump();
                        pipe.push(self.parse_trigger_stage()?);
                        if pipe.len() > 32 {
                            return self.err(
                                "too many triggers in pipe (limit 32)");
                        }
                    }
                    Ok(MetaStmt::TriggerPipe(pipe))
                } else {
                    Ok(MetaStmt::Runtime(Stmt::Trigger(first)))
                }
            }
            Tok::Ident(s) if s == "repeat" => {
                self.bump();
                let n = self.read_time_or_number()?;
                if !(1.0..=64.0).contains(&n) {
                    return self.err(format!("repeat count out of range: {}", n));
                }
                let k = self.open_block()?;
                let body = self.parse_meta_body(k)?;
                self.close_block(k)?;
                // Convert to a runtime Repeat of pure-runtime
                // statements if every inner stmt is runtime, or
                // fall back to a meta `For` with range.
                if body.iter().all(|s| matches!(s, MetaStmt::Runtime(_))) {
                    let runtime_body: Vec<Stmt> = body.into_iter()
                        .filter_map(|s| match s {
                            MetaStmt::Runtime(r) => Some(r),
                            _ => None,
                        })
                        .collect();
                    Ok(MetaStmt::Runtime(Stmt::Repeat {
                        count: n as u32,
                        body: runtime_body,
                    }))
                } else {
                    Ok(MetaStmt::For {
                        binder: "_repeat_i".into(),
                        iter: MetaExpr::Range {
                            lo: Box::new(MetaExpr::Lit(MetaValue::Int(0))),
                            hi: Box::new(MetaExpr::Lit(MetaValue::Int(n as i64))),
                            step: None,
                        },
                        body,
                    })
                }
            }
            Tok::Ident(s) if s == "rule" => {
                self.bump();
                let cat_name = self.read_ident()?;
                let cat = rule_category_from_ident(&cat_name)
                    .ok_or_else(|| self.err_owned(format!(
                        "unknown rule category '{}'", cat_name)))?;
                let rule = self.parse_rule_fields_block(cat)?;
                Ok(MetaStmt::Rule(rule))
            }
            Tok::Ident(s) if s == "revert" => {
                self.bump();
                let cat_name = self.read_ident()?;
                let cat = RuleCategory::from_ident(&cat_name)
                    .ok_or_else(|| self.err_owned(format!(
                        "unknown rule category '{}'", cat_name)))?;
                Ok(MetaStmt::Revert(cat))
            }
            Tok::Ident(s) if s == "push" => {
                self.bump();
                let cat_name = self.read_ident()?;
                let cat = RuleCategory::from_ident(&cat_name)
                    .ok_or_else(|| self.err_owned(format!(
                        "unknown rule category '{}'", cat_name)))?;
                if matches!(cat, RuleCategory::All) {
                    return self.err("cannot 'push all'");
                }
                Ok(MetaStmt::Push(cat))
            }
            Tok::Ident(s) if s == "pop" => {
                self.bump();
                let cat_name = self.read_ident()?;
                let cat = RuleCategory::from_ident(&cat_name)
                    .ok_or_else(|| self.err_owned(format!(
                        "unknown rule category '{}'", cat_name)))?;
                if matches!(cat, RuleCategory::All) {
                    return self.err("cannot 'pop all'");
                }
                Ok(MetaStmt::Pop(cat))
            }

            Tok::Ident(_) => {
                // Bare identifier at statement start that did
                // not match any built in keyword. The common
                // case is an implicit user function call used
                // as a statement: `cascade(3)` is sugar for
                // `@expand cascade(3)`. We parse one full meta
                // expression and only accept it when the top
                // level form is a call; anything else (a lone
                // variable, a binary expression) has no effect
                // at statement level and is almost always an
                // author mistake, so we surface it as a parse
                // error with a readable hint.
                let expr = self.parse_meta_expr()?;
                match expr {
                    MetaExpr::Call { .. } => Ok(MetaStmt::Expand(expr)),
                    _ => self.err(
                        "bare expression at statement start; \
                         wrap it in `@expand` or use a keyword \
                         like `emit`, `trigger`, `wait`"),
                }
            }

            other => self.err(format!(
                "unexpected token at statement start: {:?}", other)),
        }
    }

    fn parse_at_form(&mut self) -> Result<MetaStmt, ParseError> {
        self.bump(); // consume '@'
        let kw = self.read_ident()?;
        match kw.as_str() {
            "if" => {
                let cond = self.parse_meta_expr()?;
                self.expect_kw("then")?;
                let then_body = self.parse_meta_then_branch()?;
                let else_body = if matches!(&self.peek().kind,
                    Tok::Ident(s) if s == "else")
                {
                    self.bump();
                    self.parse_meta_then_branch()?
                } else { Vec::new() };
                self.expect_kw("end")?;
                Ok(MetaStmt::If { cond, then_: then_body, else_: else_body })
            }
            "expand" => {
                let expr = self.parse_meta_expr()?;
                Ok(MetaStmt::Expand(expr))
            }
            "on" => {
                let t = self.bump();
                let event_name = match t.kind {
                    Tok::Atom(s) => s,
                    _ => return Err(ParseError {
                        msg: "expected :atom after @on".into(),
                        line: t.line, col: t.col,
                    }),
                };
                let event = self.parse_hook_event(&event_name)?;
                let k = self.open_block()?;
                let mut body: Vec<TriggerSpec> = Vec::new();
                while !self.is_block_end(k) {
                    if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
                    let t = self.bump();
                    match t.kind {
                        Tok::Ident(s) if s == "trigger" => {
                            let spec = self.parse_trigger_stage()?;
                            body.push(spec);
                        }
                        _ => return Err(ParseError {
                            msg: "only trigger statements allowed inside @on"
                                .into(),
                            line: t.line, col: t.col,
                        }),
                    }
                }
                self.close_block(k)?;
                Ok(MetaStmt::Hook { event, body })
            }
            other => self.err(format!("unknown @{} form", other)),
        }
    }

    fn parse_meta_then_branch(&mut self) -> Result<Vec<MetaStmt>, ParseError> {
        let mut out = Vec::new();
        loop {
            match &self.peek().kind {
                Tok::Ident(s) if s == "else" || s == "end" => break,
                Tok::Eof => break,
                _ => {
                    if matches!(self.peek().kind, Tok::Comma | Tok::Semi) {
                        self.bump();
                        continue;
                    }
                    out.push(self.parse_meta_stmt()?);
                }
            }
        }
        Ok(out)
    }

    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Underscore => Ok(MatchPattern::Wildcard),
            Tok::Atom(name) | Tok::Ident(name) => {
                if matches!(self.peek().kind, Tok::DotDot) {
                    self.bump();
                    let hi = self.read_atom()?;
                    Ok(MatchPattern::AtomRange { lo: name, hi })
                } else {
                    Ok(MatchPattern::Atom(name))
                }
            }
            _ => Err(ParseError {
                msg: "expected match pattern".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    fn parse_hook_event(&mut self, name: &str) -> Result<HookEvent, ParseError> {
        match name {
            "onset" => {
                // Forms:
                //   @on :onset every N do ... end
                //   @on :onset between ~t"A"..~t"B" do ... end
                //   @on :onset do ... end  (every 1)
                if matches!(&self.peek().kind, Tok::Ident(s) if s == "every") {
                    self.bump();
                    let n = self.read_time_or_number()? as u32;
                    Ok(HookEvent::OnsetEvery { n: n.max(1) })
                } else if matches!(&self.peek().kind, Tok::Ident(s) if s == "between") {
                    self.bump();
                    let lo = self.read_time_or_number()?;
                    let t = self.bump();
                    if !matches!(t.kind, Tok::DotDot) {
                        return Err(ParseError {
                            msg: "expected '..' in between".into(),
                            line: t.line, col: t.col,
                        });
                    }
                    let hi = self.read_time_or_number()?;
                    Ok(HookEvent::OnsetBetween { lo, hi })
                } else {
                    Ok(HookEvent::OnsetEvery { n: 1 })
                }
            }
            "beat" => {
                // @on :beat { rhythm = :downbeat } do ... end
                if matches!(self.peek().kind, Tok::LBrace) {
                    self.bump();
                    while !matches!(self.peek().kind, Tok::RBrace) {
                        if matches!(self.peek().kind, Tok::Comma) {
                            self.bump();
                            continue;
                        }
                        let _k = self.read_ident()?;
                        self.skip_opt_eq();
                        let t = self.bump();
                        let _ = match t.kind {
                            Tok::Atom(_) | Tok::Ident(_) => {}
                            _ => return Err(ParseError {
                                msg: "expected :atom".into(),
                                line: t.line, col: t.col,
                            }),
                        };
                    }
                    self.bump();
                }
                Ok(HookEvent::BeatDownbeat)
            }
            "section_enter" => {
                let s = self.read_string()?;
                Ok(HookEvent::SectionEnter { name: s })
            }
            "close_call" => {
                // @on :close_call above N do ... end
                if matches!(&self.peek().kind, Tok::Ident(s) if s == "above") {
                    self.bump();
                    let n = self.read_time_or_number()? as u32;
                    Ok(HookEvent::CloseCallCountAbove { n })
                } else {
                    Ok(HookEvent::CloseCallCountAbove { n: 1 })
                }
            }
            other => self.err(format!("unknown hook event :{}", other)),
        }
    }

    // ---------- meta expressions ----------

    fn parse_meta_expr(&mut self) -> Result<MetaExpr, ParseError> {
        self.parse_me_or()
    }

    fn parse_me_or(&mut self) -> Result<MetaExpr, ParseError> {
        self.down()?;
        let mut left = self.parse_me_and()?;
        while matches!(&self.peek().kind, Tok::Ident(s) if s == "or") {
            self.bump();
            let right = self.parse_me_and()?;
            left = MetaExpr::Bin(MetaBinOp::Or, Box::new(left), Box::new(right));
        }
        self.up();
        Ok(left)
    }

    fn parse_me_and(&mut self) -> Result<MetaExpr, ParseError> {
        self.down()?;
        let mut left = self.parse_me_cmp()?;
        while matches!(&self.peek().kind, Tok::Ident(s) if s == "and") {
            self.bump();
            let right = self.parse_me_cmp()?;
            left = MetaExpr::Bin(MetaBinOp::And, Box::new(left), Box::new(right));
        }
        self.up();
        Ok(left)
    }

    fn parse_me_cmp(&mut self) -> Result<MetaExpr, ParseError> {
        self.down()?;
        let left = self.parse_me_add()?;
        // Comparison in meta expressions uses keyword operators
        // (`eq`, `neq`, `lt`, `le`, `gt`, `ge`) rather than the
        // punctuation variants used by the formula language.
        // This keeps the grammar unambiguous without extending
        // the lexer with more multi character tokens, and avoids
        // the Option<_> inference dead end the previous draft
        // fell into with a match that had no typed right hand
        // side.
        let left = self.parse_me_cmp_keyword(left)?;
        self.up();
        Ok(left)
    }

    fn parse_me_cmp_keyword(&mut self, mut left: MetaExpr) -> Result<MetaExpr, ParseError> {
        loop {
            let kind = self.peek().kind.clone();
            let op = match kind {
                Tok::Ident(ref s) => match s.as_str() {
                    "eq" => Some(MetaBinOp::Eq),
                    "neq" => Some(MetaBinOp::Ne),
                    "lt" => Some(MetaBinOp::Lt),
                    "le" => Some(MetaBinOp::Le),
                    "gt" => Some(MetaBinOp::Gt),
                    "ge" => Some(MetaBinOp::Ge),
                    _ => None,
                },
                _ => None,
            };
            if let Some(op) = op {
                self.bump();
                let right = self.parse_me_add()?;
                left = MetaExpr::Bin(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_me_add(&mut self) -> Result<MetaExpr, ParseError> {
        self.down()?;
        let mut left = self.parse_me_mul()?;
        loop {
            let op = match &self.peek().kind {
                Tok::Ident(s) if s == "plus" => Some(MetaBinOp::Add),
                Tok::Ident(s) if s == "minus" => Some(MetaBinOp::Sub),
                _ => None,
            };
            if let Some(op) = op {
                self.bump();
                let right = self.parse_me_mul()?;
                left = MetaExpr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_me_mul(&mut self) -> Result<MetaExpr, ParseError> {
        self.down()?;
        let mut left = self.parse_me_unary()?;
        loop {
            let op = match &self.peek().kind {
                Tok::Ident(s) if s == "mul" => Some(MetaBinOp::Mul),
                Tok::Ident(s) if s == "div" => Some(MetaBinOp::Div),
                Tok::Ident(s) if s == "mod" => Some(MetaBinOp::Mod),
                _ => None,
            };
            if let Some(op) = op {
                self.bump();
                let right = self.parse_me_unary()?;
                left = MetaExpr::Bin(op, Box::new(left), Box::new(right));
            } else { break; }
        }
        self.up();
        Ok(left)
    }

    fn parse_me_unary(&mut self) -> Result<MetaExpr, ParseError> {
        if matches!(&self.peek().kind, Tok::Ident(s) if s == "not") {
            self.bump();
            let inner = self.parse_me_unary()?;
            return Ok(MetaExpr::Un(MetaUnOp::Not, Box::new(inner)));
        }
        self.parse_me_primary()
    }

    fn parse_me_primary(&mut self) -> Result<MetaExpr, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n) => Ok(MetaExpr::Lit(MetaValue::Float(n))),
            Tok::Str(s) => Ok(MetaExpr::Lit(MetaValue::Str(s))),
            Tok::Atom(s) => Ok(MetaExpr::Lit(MetaValue::Atom(s))),
            Tok::Sigil('t', s) => {
                let secs = parse_time_sigil(&s).map_err(|m| ParseError {
                    msg: m, line: t.line, col: t.col,
                })?;
                Ok(MetaExpr::Lit(MetaValue::Float(secs)))
            }
            Tok::Sigil(_, s) => Ok(MetaExpr::Lit(MetaValue::Str(s))),
            Tok::Ident(name) => {
                // Function call?
                if matches!(self.peek().kind, Tok::LParen) {
                    self.bump();
                    let mut args: Vec<MetaExpr> = Vec::new();
                    while !matches!(self.peek().kind, Tok::RParen) {
                        if matches!(self.peek().kind, Tok::Comma) {
                            self.bump();
                            continue;
                        }
                        args.push(self.parse_meta_expr()?);
                        if args.len() > 16 {
                            return self.err("too many args (limit 16)");
                        }
                    }
                    self.bump();
                    Ok(MetaExpr::Call { name, args })
                } else {
                    Ok(MetaExpr::Var(name))
                }
            }
            Tok::LParen => {
                let e = self.parse_meta_expr()?;
                let t2 = self.bump();
                if !matches!(t2.kind, Tok::RParen) {
                    return Err(ParseError {
                        msg: "expected ')'".into(),
                        line: t2.line, col: t2.col,
                    });
                }
                Ok(e)
            }
            Tok::LBracket => {
                let mut items: Vec<MetaExpr> = Vec::new();
                while !matches!(self.peek().kind, Tok::RBracket) {
                    if matches!(self.peek().kind, Tok::Comma) {
                        self.bump();
                        continue;
                    }
                    items.push(self.parse_meta_expr()?);
                    if items.len() > safety::MAX_FOR_ITERATIONS as usize {
                        return self.err("list literal too long");
                    }
                }
                self.bump();
                Ok(MetaExpr::List(items))
            }
            _ => Err(ParseError {
                msg: "expected expression".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    // ---------- obstacles and triggers (stripped v2 style) ----------

    fn parse_obstacle(&mut self) -> Result<ObstacleSpec, ParseError> {
        let t = self.bump();
        let name = match t.kind {
            Tok::Atom(s) | Tok::Ident(s) => s,
            _ => return Err(ParseError {
                msg: "expected obstacle name".into(),
                line: t.line, col: t.col,
            }),
        };
        let fields = if matches!(self.peek().kind, Tok::LBrace) {
            self.parse_field_map()?
        } else {
            Fields::default()
        };
        let spec = build_obstacle(self, &name, &fields)?;
        Ok(spec)
    }

    fn parse_trigger_stage(&mut self) -> Result<TriggerSpec, ParseError> {
        let t = self.bump();
        let name = match t.kind {
            Tok::Atom(s) | Tok::Ident(s) => s,
            _ => return Err(ParseError {
                msg: "expected trigger name".into(),
                line: t.line, col: t.col,
            }),
        };
        let fields = if matches!(self.peek().kind, Tok::LBrace) {
            self.parse_field_map()?
        } else {
            Fields::default()
        };
        build_trigger(self, &name, &fields)
    }

    fn parse_field_map(&mut self) -> Result<Fields, ParseError> {
        let t = self.bump();
        if !matches!(t.kind, Tok::LBrace) {
            return Err(ParseError {
                msg: "expected '{'".into(),
                line: t.line, col: t.col,
            });
        }
        let mut f = Fields::default();
        while !matches!(self.peek().kind, Tok::RBrace) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            let name = self.read_ident()?;
            self.skip_opt_eq();
            let v = self.read_field_value()?;
            f.set(name, v);
            if f.entries.len() > 32 {
                return self.err("too many fields (limit 32)");
            }
        }
        self.bump();
        Ok(f)
    }

    fn read_field_value(&mut self) -> Result<FieldValue, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n) => Ok(FieldValue::Num(n)),
            Tok::Str(s) => Ok(FieldValue::Str(s)),
            Tok::Sigil(letter, s) => {
                if letter == 't' {
                    parse_time_sigil(&s).map(FieldValue::Num)
                        .map_err(|m| ParseError {
                            msg: m, line: t.line, col: t.col,
                        })
                } else {
                    Ok(FieldValue::Str(s))
                }
            }
            Tok::Atom(s) | Tok::Ident(s) => Ok(FieldValue::Ident(s)),
            _ => Err(ParseError {
                msg: "expected value".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    // ---------- numeric and string readers ----------

    fn read_time_or_number(&mut self) -> Result<f32, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n) => Ok(n),
            Tok::Sigil('t', body) => parse_time_sigil(&body)
                .map_err(|m| ParseError {
                    msg: m, line: t.line, col: t.col,
                }),
            _ => Err(ParseError {
                msg: "expected number".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    fn read_stringish(&mut self) -> Result<String, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Str(s) => Ok(s),
            Tok::Sigil(_, s) => Ok(s),
            _ => Err(ParseError {
                msg: "expected string".into(),
                line: t.line, col: t.col,
            }),
        }
    }
    /// Parse a rule field map delimited by `{ ... }` or `do ...
    /// end`. Used by `rule <category> { ... }` inside section
    /// bodies. Returns a `RuleSet` with the appropriate variant
    /// already clamped.
    fn parse_rule_fields_block(
        &mut self, cat: RuleCategory,
    ) -> Result<RuleSet, ParseError> {
        let kind = self.open_block()?;
        let fields = self.collect_fields_until_block_end(kind)?;
        self.close_block(kind)?;
        build_rule(self, cat, &fields)
    }

    /// Parse a rule field list without an opening brace. Used
    /// inside directives `#[<category> { ... }]` where the `[`
    /// and `]` are the delimiters and the inner `{ }` is
    /// optional.
    fn parse_rule_fields_inline(
        &mut self, cat: RuleCategory,
    ) -> Result<RuleSet, ParseError> {
        let fields = if matches!(self.peek().kind, Tok::LBrace) {
            self.bump();
            let f = self.collect_fields_until_rbrace()?;
            let t = self.bump();
            if !matches!(t.kind, Tok::RBrace) {
                return Err(ParseError {
                    msg: "expected '}'".into(),
                    line: t.line, col: t.col,
                });
            }
            f
        } else {
            Fields::default()
        };
        build_rule(self, cat, &fields)
    }

    /// Read key/value pairs until the parser sees the end token
    /// of the enclosing block (`}` for brace, `end` for do).
    fn collect_fields_until_block_end(
        &mut self, kind: BlockKind,
    ) -> Result<Fields, ParseError> {
        let mut f = Fields::default();
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma) {
                self.bump();
                continue;
            }
            let name = self.read_ident()?;
            self.skip_opt_eq();
            let v = self.read_field_value()?;
            f.set(name, v);
            if f.entries.len() > 32 {
                return self.err("too many rule fields (limit 32)");
            }
        }
        Ok(f)
    }

    fn collect_fields_until_rbrace(&mut self) -> Result<Fields, ParseError> {
        let mut f = Fields::default();
        while !matches!(self.peek().kind, Tok::RBrace) {
            if matches!(self.peek().kind, Tok::Comma) {
                self.bump();
                continue;
            }
            let name = self.read_ident()?;
            self.skip_opt_eq();
            let v = self.read_field_value()?;
            f.set(name, v);
            if f.entries.len() > 32 {
                return self.err("too many rule fields (limit 32)");
            }
        }
        Ok(f)
    }
}

/// Convert a `timestamp_format_use_<tag>` directive name + its
/// optional k/v body into a concrete `TimestampFormat`. Returns
/// `None` for anything that does not start with the expected
/// prefix, letting the caller fall through to other directive
/// handlers.
fn parse_timestamp_directive_v3(
    name: &str, kv: &[(String, f32)],
) -> Option<TimestampFormat> {
    let tag = name.strip_prefix("timestamp_format_use_")?;
    Some(match tag {
        "tracklength" => TimestampFormat::TrackLength,
        "relative"    => TimestampFormat::Relative,
        "beats" | "bars" => {
            let total = kv.iter().find(|(k, _)| k == "count")
                .map(|(_, v)| *v as u32).unwrap_or(64).max(1);
            TimestampFormat::Beats { total }
        }
        other => TimestampFormat::Named(other.to_string()),
    })
}

/// Map a token ident to a concrete rule category (no `All`).
/// Used by the file directive parser and the inline `rule`
/// parser to convert category names into the strongly typed
/// enum before reading fields.
fn rule_category_from_ident(s: &str) -> Option<RuleCategory> {
    match s {
        "ability"  => Some(RuleCategory::Ability),
        "vision"   => Some(RuleCategory::Vision),
        "cursor"   => Some(RuleCategory::Cursor),
        "survival" => Some(RuleCategory::Survival),
        "input"    => Some(RuleCategory::Input),
        "score"    => Some(RuleCategory::Score),
        _ => None,
    }
}

/// Materialise a rule set from a field map. Unknown fields are
/// rejected with a descriptive error. Values pass through the
/// category's `clamp` method before they reach the AST.
fn build_rule(
    p: &ParserV3, cat: RuleCategory, fields: &Fields,
) -> Result<RuleSet, ParseError> {
    match cat {
        RuleCategory::Ability => {
            let mut r = AbilityRule::default();
            for (name, v) in &fields.entries {
                match name.as_str() {
                    "kind" => {
                        let k = match v {
                            FieldValue::Ident(s) => AbilityKind::from_atom(s),
                            _ => None,
                        }.ok_or_else(|| p.err_owned(format!(
                            "expected ability kind atom, got {:?}", v)))?;
                        r.kind = Some(k);
                    }
                    "charges" => r.charges = Some(as_u32(v, p, "charges")?),
                    "recharge" => r.recharge = Some(as_f32(v, p, "recharge")?),
                    "invuln" => r.invuln = Some(as_f32(v, p, "invuln")?),
                    "cooldown" => r.cooldown = Some(as_f32(v, p, "cooldown")?),
                    "slots_per_dash" | "slotsPerDash" =>
                        r.slots_per_dash = Some(as_u32(v, p, "slots_per_dash")?),
                    "slowmo_factor" | "slowmoFactor" =>
                        r.slowmo_factor = Some(as_f32(v, p, "slowmo_factor")?),
                    "slowmo_cap" | "slowmoCap" =>
                        r.slowmo_cap = Some(as_f32(v, p, "slowmo_cap")?),
                    "slowmo_recover" | "slowmoRecover" =>
                        r.slowmo_recover = Some(as_f32(v, p, "slowmo_recover")?),
                    other => return Err(p.err_owned(format!(
                        "unknown ability field '{}'", other))),
                }
            }
            r.clamp();
            Ok(RuleSet::Ability(r))
        }
        RuleCategory::Vision => {
            let mut r = VisionRule::default();
            for (name, v) in &fields.entries {
                match name.as_str() {
                    "range" => r.range = Some(as_f32(v, p, "range")?),
                    "fog_near" | "fogNear" =>
                        r.fog_near = Some(as_f32(v, p, "fog_near")?),
                    "fog_far" | "fogFar" =>
                        r.fog_far = Some(as_f32(v, p, "fog_far")?),
                    "strobe" => r.strobe = Some(as_bool(v, p, "strobe")?),
                    "strobe_rate" | "strobeRate" =>
                        r.strobe_rate = Some(as_f32(v, p, "strobe_rate")?),
                    "blind_duration" | "blindDuration" =>
                        r.blind_duration = Some(as_f32(v, p, "blind_duration")?),
                    "blind_frequency" | "blindFrequency" =>
                        r.blind_frequency = Some(as_f32(v, p, "blind_frequency")?),
                    "hide_camera_indicator" | "hideCameraIndicator" =>
                        r.hide_camera_indicator =
                            Some(as_bool(v, p, "hide_camera_indicator")?),
                    other => return Err(p.err_owned(format!(
                        "unknown vision field '{}'", other))),
                }
            }
            r.clamp();
            Ok(RuleSet::Vision(r))
        }
        RuleCategory::Cursor => {
            let mut r = CursorRule::default();
            for (name, v) in &fields.entries {
                match name.as_str() {
                    "speed_mult" | "speedMult" =>
                        r.speed_mult = Some(as_f32(v, p, "speed_mult")?),
                    "width_mult" | "widthMult" =>
                        r.width_mult = Some(as_f32(v, p, "width_mult")?),
                    "count" => r.count = Some(as_u32(v, p, "count")?),
                    "angular_offset" | "angularOffset" =>
                        r.angular_offset =
                            Some(as_f32(v, p, "angular_offset")?),
                    "centripetal_drift" | "centripetalDrift" =>
                        r.centripetal_drift =
                            Some(as_f32(v, p, "centripetal_drift")?),
                    other => return Err(p.err_owned(format!(
                        "unknown cursor field '{}'", other))),
                }
            }
            r.clamp();
            Ok(RuleSet::Cursor(r))
        }
        RuleCategory::Survival => {
            let mut r = SurvivalRule::default();
            for (name, v) in &fields.entries {
                match name.as_str() {
                    "lives" => r.lives = Some(as_u32(v, p, "lives")?),
                    "soft_death" | "softDeath" =>
                        r.soft_death = Some(as_bool(v, p, "soft_death")?),
                    "pushback_seconds" | "pushbackSeconds" =>
                        r.pushback_seconds =
                            Some(as_f32(v, p, "pushback_seconds")?),
                    "invuln_after_hit" | "invulnAfterHit" =>
                        r.invuln_after_hit =
                            Some(as_f32(v, p, "invuln_after_hit")?),
                    "checkpoints_enabled" | "checkpointsEnabled" =>
                        r.checkpoints_enabled =
                            Some(as_bool(v, p, "checkpoints_enabled")?),
                    other => return Err(p.err_owned(format!(
                        "unknown survival field '{}'", other))),
                }
            }
            r.clamp();
            Ok(RuleSet::Survival(r))
        }
        RuleCategory::Input => {
            let mut r = InputRule::default();
            for (name, v) in &fields.entries {
                match name.as_str() {
                    "delay_ms" | "delayMs" =>
                        r.delay_ms = Some(as_u32(v, p, "delay_ms")?),
                    "discrete" => r.discrete = Some(as_bool(v, p, "discrete")?),
                    "noise" => r.noise = Some(as_f32(v, p, "noise")?),
                    "inverted" => r.inverted = Some(as_bool(v, p, "inverted")?),
                    other => return Err(p.err_owned(format!(
                        "unknown input field '{}'", other))),
                }
            }
            r.clamp();
            Ok(RuleSet::Input(r))
        }
        RuleCategory::Score => {
            let mut r = ScoreRule::default();
            for (name, v) in &fields.entries {
                match name.as_str() {
                    "multiplier" =>
                        r.multiplier = Some(as_f32(v, p, "multiplier")?),
                    "close_call_bonus" | "closeCallBonus" =>
                        r.close_call_bonus =
                            Some(as_u32(v, p, "close_call_bonus")?),
                    "survival_per_second" | "survivalPerSecond" =>
                        r.survival_per_second =
                            Some(as_u32(v, p, "survival_per_second")?),
                    other => return Err(p.err_owned(format!(
                        "unknown score field '{}'", other))),
                }
            }
            r.clamp();
            Ok(RuleSet::Score(r))
        }
        RuleCategory::All => {
            Err(p.err_owned(
                "'all' is only valid in 'revert all'"))
        }
    }
}

/// Coerce a `FieldValue` to `f32` with a readable error.
fn as_f32(v: &FieldValue, p: &ParserV3, field: &str) -> Result<f32, ParseError> {
    match v {
        FieldValue::Num(n) => Ok(*n),
        _ => Err(p.err_owned(format!(
            "field '{}' expects a number", field))),
    }
}

/// Coerce a `FieldValue` to `u32` with a readable error.
fn as_u32(v: &FieldValue, p: &ParserV3, field: &str) -> Result<u32, ParseError> {
    match v {
        FieldValue::Num(n) => {
            if !n.is_finite() {
                return Err(p.err_owned(format!(
                    "field '{}' must be finite", field)));
            }
            if *n < 0.0 {
                return Err(p.err_owned(format!(
                    "field '{}' cannot be negative", field)));
            }
            Ok(n.round() as u32)
        }
        _ => Err(p.err_owned(format!(
            "field '{}' expects a number", field))),
    }
}

/// Coerce a `FieldValue` to `bool`. Accepts the atoms `true` /
/// `false` (lexed as idents by the v3 tokenizer) or the numeric
/// literals 0 and 1.
fn as_bool(v: &FieldValue, p: &ParserV3, field: &str) -> Result<bool, ParseError> {
    match v {
        FieldValue::Ident(s) => match s.as_str() {
            "true"  | "on"  | "yes" => Ok(true),
            "false" | "off" | "no"  => Ok(false),
            other => Err(p.err_owned(format!(
                "field '{}' expects true/false, got '{}'", field, other))),
        },
        FieldValue::Num(n) => Ok(*n >= 0.5),
        _ => Err(p.err_owned(format!(
            "field '{}' expects bool", field))),
    }
}

// ---------- helpers shared with v2 style ----------

#[derive(Default, Debug)]
struct Fields { entries: Vec<(String, FieldValue)> }

#[derive(Clone, Debug)]
enum FieldValue { Num(f32), Str(String), Ident(String) }

impl Fields {
    fn set(&mut self, k: String, v: FieldValue) {
        if let Some(e) = self.entries.iter_mut().find(|(n, _)| n == &k) {
            e.1 = v;
        } else {
            self.entries.push((k, v));
        }
    }
    fn get(&self, k: &str) -> Option<&FieldValue> {
        self.entries.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
    fn num(&self, k: &str) -> Option<f32> {
        match self.get(k) { Some(FieldValue::Num(n)) => Some(*n), _ => None }
    }
    fn word(&self, k: &str) -> Option<String> {
        match self.get(k) { Some(FieldValue::Ident(s)) => Some(s.clone()), _ => None }
    }
    fn text(&self, k: &str) -> Option<String> {
        match self.get(k) { Some(FieldValue::Str(s)) => Some(s.clone()), _ => None }
    }
}

fn type_from_ident(s: &str) -> MetaType {
    match s {
        "int"   | "i32"     => MetaType::Int,
        "float" | "f32"     => MetaType::Float,
        "bool"              => MetaType::Bool,
        "str"   | "string"  => MetaType::Str,
        "atom"  | "ident"   => MetaType::Atom,
        "pattern"           => MetaType::Pattern,
        "trigger"           => MetaType::Trigger,
        "stmts"             => MetaType::Stmts,
        _                   => MetaType::Unknown,
    }
}

fn parse_time_sigil(body: &str) -> Result<f32, String> {
    let parts: Vec<&str> = body.split(':').collect();
    let to_f = |s: &str| -> Result<f32, String> {
        s.parse::<f32>().map_err(|_| format!("bad number '{}' in ~t", s))
    };
    let secs = match parts.len() {
        1 => to_f(parts[0])?,
        2 => {
            let m = to_f(parts[0])?;
            let s = to_f(parts[1])?;
            if !(0.0..60.0).contains(&s) {
                return Err(format!("seconds out of 0..60 in ~t: {}", s));
            }
            m * 60.0 + s
        }
        3 => {
            let h = to_f(parts[0])?;
            let m = to_f(parts[1])?;
            let s = to_f(parts[2])?;
            if !(0.0..60.0).contains(&m) {
                return Err(format!("minutes out of 0..60 in ~t: {}", m));
            }
            if !(0.0..60.0).contains(&s) {
                return Err(format!("seconds out of 0..60 in ~t: {}", s));
            }
            h * 3600.0 + m * 60.0 + s
        }
        _ => return Err(format!(
            "~t must have 1, 2 or 3 colon parts, got {}",
            parts.len())),
    };
    safety::check_finite(secs, "~t sigil")
}

fn dir_from(fields: &Fields) -> SpinDir {
    match fields.word("dir").as_deref() {
        Some("ccw") => SpinDir::Ccw,
        _ => SpinDir::Cw,
    }
}

fn build_obstacle(p: &ParserV3, name: &str, fields: &Fields) -> Result<ObstacleSpec, ParseError> {
    match name {
        "bar" => {
            let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.5);
            Ok(ObstacleSpec::Bar { thickness_mult: t })
        }
        "doubleBar" | "double_bar" => {
            let spacing = fields.num("spacing").unwrap_or(2.0).clamp(1.0, 6.0) as u32;
            let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.5);
            Ok(ObstacleSpec::DoubleBar { spacing, thickness_mult: t })
        }
        "spiral" => {
            let dir = dir_from(fields);
            let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
            let loops = fields.num("loops").unwrap_or(2.0).clamp(1.0, 6.0) as u32;
            Ok(ObstacleSpec::Spiral { dir, thickness_mult: t, loops })
        }
        "alternate" => {
            let parity = match fields.word("parity").as_deref() {
                Some("odd") => Parity::Odd,
                _ => Parity::Even,
            };
            let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
            Ok(ObstacleSpec::Alternate { parity, thickness_mult: t })
        }
        "pinwheel" => {
            let spokes = fields.num("spokes").unwrap_or(3.0).clamp(1.0, 6.0) as u32;
            let dir = dir_from(fields);
            Ok(ObstacleSpec::Pinwheel { spokes, dir })
        }
        "rain" => {
            let count = fields.num("count").unwrap_or(5.0).clamp(2.0, 12.0) as u32;
            let t = fields.num("thickness").unwrap_or(0.85).clamp(0.5, 1.5);
            Ok(ObstacleSpec::Rain { count, thickness_mult: t })
        }
        "rainbow" => Ok(ObstacleSpec::Rainbow { dir: dir_from(fields) }),
        "ladder" => {
            let rungs = fields.num("rungs").unwrap_or(6.0).clamp(3.0, 16.0) as u32;
            Ok(ObstacleSpec::Ladder { rungs })
        }
        "tunnel" => {
            let length = fields.num("length").unwrap_or(1.2).clamp(0.4, 2.5);
            let lanes  = fields.num("lanes").unwrap_or(3.0).clamp(1.0, 4.0) as u32;
            Ok(ObstacleSpec::Tunnel { length, lanes })
        }
        "pot" => {
            let layers = fields.num("layers").unwrap_or(4.0).clamp(2.0, 8.0) as u32;
            Ok(ObstacleSpec::Pot { layers })
        }
        "staircase" => {
            let dir = dir_from(fields);
            let steps = fields.num("steps").unwrap_or(6.0).clamp(3.0, 24.0) as u32;
            let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
            Ok(ObstacleSpec::Staircase { dir, steps, thickness_mult: t })
        }
        "corridor" => {
            let dir = dir_from(fields);
            let length = fields.num("length").unwrap_or(1.4).clamp(0.6, 3.0);
            let turns  = fields.num("turns").unwrap_or(3.0).clamp(1.0, 8.0) as u32;
            Ok(ObstacleSpec::Corridor { length, turns, dir })
        }
        "cubes" => {
            let layers = fields.num("layers").unwrap_or(4.0).clamp(2.0, 8.0) as u32;
            let dir = dir_from(fields);
            Ok(ObstacleSpec::Cubes { layers, dir })
        }
        "custom" => {
            let mask_str = fields.text("mask").ok_or_else(|| {
                p.err_owned("custom needs mask = ~m\"...\"")
            })?;
            if mask_str.len() > 128 {
                return Err(p.err_owned("mask longer than 128 slots"));
            }
            let mut mask = Vec::with_capacity(mask_str.len());
            for c in mask_str.chars() {
                match c {
                    '1' => mask.push(true),
                    '0' => mask.push(false),
                    _ => return Err(p.err_owned(format!(
                        "mask contains '{}'", c))),
                }
            }
            if mask.is_empty() || !mask.iter().any(|b| !*b) {
                return Err(p.err_owned("mask must leave at least one gap"));
            }
            let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
            Ok(ObstacleSpec::Custom { mask, thickness_mult: t })
        }
        "formula" | "custom_formula" => {
            let src = fields.text("formula").ok_or_else(|| {
                p.err_owned("formula needs formula = ~f\"...\"")
            })?;
            let steps = fields.num("steps").unwrap_or(6.0)
                .clamp(1.0, safety::MAX_PATTERN_STEPS as f32) as u32;
            let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
            let program = formula::parse(&src)
                .map_err(|m| p.err_owned(m))?;
            let seed = fields.num("seed").unwrap_or(0.0) as u32;
            formula::validate_pathable(&program, steps, 6, seed)
                .map_err(|m| p.err_owned(m))?;
            Ok(ObstacleSpec::CustomFormula {
                formula: Formula { source: src, program },
                steps,
                thickness_mult: t,
            })
        }
        other => Err(p.err_owned(format!("unknown obstacle '{}'", other))),
    }
}

/// Build a `TriggerSpec` from a parsed name and field map
/// in the v3 dialect.
///
/// Most fields pass through straight clamps. The notable
/// exceptions are the `:speedwarp` axes (`walls`, `rotation`,
/// `cursor`, `music` / `musicScale`): these are now truly
/// optional, so an absent field maps to `None` (leave the
/// axis alone) and an explicit `0` maps to `Some(0.0)` (halt
/// the axis for the duration of the trigger). The runtime
/// generator maintains one timer per axis so axes stacked
/// from several piped `:speedwarp` triggers each respect
/// their own duration.
///
/// The historically duration-less triggers (`:tilt`,
/// `:speedMult`, `:hueShift`) now accept an explicit
/// `duration` field with sensible defaults (4 s, 4 s, 6 s
/// respectively). Passing the field from a pipe like
/// `trigger :flip |> :speedwarp { ..., duration = 3.0 }`
/// finally takes effect because the generator no longer
/// ignores the field on stacked triggers.
fn build_trigger(p: &ParserV3, name: &str, fields: &Fields) -> Result<TriggerSpec, ParseError> {
    match name {
        "flip"  => Ok(TriggerSpec::Flip),
        "pulse" => Ok(TriggerSpec::Pulse),
        "tilt" => {
            let deg = fields.num("angle")
                .or_else(|| fields.num("deg"))
                .unwrap_or(0.0);
            let pitch = fields.num("pitch").unwrap_or(0.0);
            let yaw   = fields.num("yaw").unwrap_or(0.0);
            let duration = fields.num("duration")
                .map(|d| d.clamp(0.05, 60.0));
            Ok(TriggerSpec::Tilt {
                angle:    deg.clamp(-30.0, 30.0),
                pitch:    pitch.clamp(-30.0, 30.0),
                yaw:      yaw.clamp(-30.0, 30.0),
                duration,
            })
        }
        "speedMult" => {
            let factor = fields.num("factor").unwrap_or(1.0);
            let dur = fields.num("duration").unwrap_or(4.0);
            Ok(TriggerSpec::SpeedMult {
                factor:   factor.clamp(0.5, 2.0),
                duration: dur.clamp(0.05, 60.0),
            })
        }
        "hueShift" => {
            let rate = fields.num("rate").unwrap_or(0.0);
            let dur = fields.num("duration").unwrap_or(6.0);
            Ok(TriggerSpec::HueShift {
                rate:     rate.clamp(-2.0, 2.0),
                duration: dur.clamp(0.05, 60.0),
            })
        }
        "speedwarp" => {
            // Axis values: omitted = None (leave alone),
            // present = Some(v) clamped into a safe range.
            // The explicit `Some(0.0)` case is legitimate and
            // means "halt this axis".
            let walls    = fields.num("walls");
            let rotation = fields.num("rotation");
            let cursor   = fields.num("cursor");
            let music    = fields.num("musicScale")
                .or_else(|| fields.num("music"));
            let duration = fields.num("duration")
                .unwrap_or(3.0)
                .clamp(0.1, 20.0);
            Ok(TriggerSpec::SpeedWarp {
                walls:       walls.map(|v| v.clamp(0.0, 4.0)),
                rotation:    rotation.map(|v| v.clamp(0.0, 4.0)),
                cursor:      cursor.map(|v| v.clamp(0.0, 4.0)),
                music_scale: music.map(|v| v.clamp(0.25, 4.0)),
                duration,
            })
        }
        "glitch" => {
            let strength = fields.num("strength").unwrap_or(0.5).clamp(0.0, 1.0);
            let duration = fields.num("duration").unwrap_or(0.8).clamp(0.05, 10.0);
            Ok(TriggerSpec::Glitch { strength, duration })
        }
        "shake" => {
            let strength = fields.num("strength").unwrap_or(0.5).clamp(0.0, 1.5);
            let duration = fields.num("duration").unwrap_or(0.6).clamp(0.05, 10.0);
            Ok(TriggerSpec::Shake { strength, duration })
        }
        "zoom" => {
            let target = fields.num("target").unwrap_or(1.0).clamp(0.25, 3.0);
            let anim = anim_from(fields.word("anim").as_deref());
            let duration = fields.num("duration").unwrap_or(1.5).clamp(0.05, 10.0);
            Ok(TriggerSpec::Zoom { target, anim, duration })
        }
        "invert" => {
            let duration = fields.num("duration").unwrap_or(3.0).clamp(0.05, 30.0);
            Ok(TriggerSpec::Invert { duration })
        }
        "strobe" => {
            let rate = fields.num("rate").unwrap_or(6.0).clamp(0.0, 40.0);
            let duration = fields.num("duration").unwrap_or(0.8).clamp(0.05, 10.0);
            Ok(TriggerSpec::Strobe { rate, duration })
        }
        "morph" => {
            // Integer side count, fractional morph duration.
            // The clamp mirrors `generation.sides` in the parser
            // so a runtime morph cannot smuggle the playfield
            // into a shape the rest of the engine rejects.
            let sides = fields.num("sides").unwrap_or(6.0)
                .clamp(3.0, 12.0) as u32;
            let duration = fields.num("duration")
                .unwrap_or(1.5).clamp(0.1, 10.0);
            Ok(TriggerSpec::Morph { sides, duration })
        }
        "spin" => {
            let rate = fields.num("rate").unwrap_or(2.0).clamp(-8.0, 8.0);
            let duration = fields.num("duration").unwrap_or(3.0).clamp(0.1, 20.0);
            Ok(TriggerSpec::Spin { rate, duration })
        }
        "bounce" => {
            let amplitude = fields.num("amplitude")
                .or_else(|| fields.num("strength"))
                .unwrap_or(0.25).clamp(0.0, 1.0);
            let duration = fields.num("duration").unwrap_or(4.0).clamp(0.1, 30.0);
            Ok(TriggerSpec::Bounce { amplitude, duration })
        }
        "freeze" => {
            let duration = fields.num("duration").unwrap_or(0.3).clamp(0.05, 3.0);
            Ok(TriggerSpec::Freeze { duration })
        }
        "zoom_punch" | "zoompunch" => {
            let strength = fields.num("strength").unwrap_or(0.35).clamp(0.0, 2.0);
            let duration = fields.num("duration").unwrap_or(0.5).clamp(0.05, 3.0);
            Ok(TriggerSpec::ZoomPunch { strength, duration })
        }
        "invert_colors" | "invertcolors" => {
            let duration = fields.num("duration").unwrap_or(1.5).clamp(0.05, 30.0);
            Ok(TriggerSpec::InvertColors { duration })
        }
        "grayscale" => {
            let strength = fields.num("strength").unwrap_or(1.0).clamp(0.0, 1.0);
            let duration = fields.num("duration").unwrap_or(2.0).clamp(0.05, 30.0);
            Ok(TriggerSpec::Grayscale { strength, duration })
        }
        "shockwave" => {
            let strength = fields.num("strength").unwrap_or(0.8).clamp(0.0, 2.0);
            let duration = fields.num("duration").unwrap_or(0.9).clamp(0.1, 5.0);
            Ok(TriggerSpec::Shockwave { strength, duration })
        }
        "fog" => {
            let near = fields.num("near").unwrap_or(0.25).clamp(0.0, 1.5);
            let far  = fields.num("far").unwrap_or(0.75).clamp(0.0, 1.5);
            let duration = fields.num("duration").unwrap_or(4.0).clamp(0.1, 60.0);
            Ok(TriggerSpec::Fog { near, far: far.max(near + 0.01), duration })
        }
        "outline" => {
            let thickness = fields.num("thickness")
                .or_else(|| fields.num("strength"))
                .unwrap_or(0.6).clamp(0.0, 2.0);
            let duration = fields.num("duration").unwrap_or(3.0).clamp(0.1, 60.0);
            Ok(TriggerSpec::Outline { thickness, duration })
        }
        "centerburst" => {
            let strength = fields.num("strength").unwrap_or(0.7).clamp(0.0, 2.0);
            let duration = fields.num("duration").unwrap_or(0.6).clamp(0.05, 5.0);
            Ok(TriggerSpec::Centerburst { strength, duration })
        }
        "ringburst" => {
            let count = fields.num("count").unwrap_or(3.0).clamp(1.0, 8.0) as u32;
            let duration = fields.num("duration").unwrap_or(1.2).clamp(0.2, 5.0);
            Ok(TriggerSpec::Ringburst { count, duration })
        }
        "bassdrop" | "bass_drop" => {
            let strength = fields.num("strength").unwrap_or(0.9).clamp(0.0, 2.0);
            let duration = fields.num("duration").unwrap_or(1.0).clamp(0.1, 5.0);
            Ok(TriggerSpec::Bassdrop { strength, duration })
        }
        "post_shader" => {
            let atom = fields.word("shader").ok_or_else(||
                p.err_owned(
                    "post_shader requires 'shader = :name'"))?;
            let slot = p.shaders.iter().position(|s| s.name == atom)
                .ok_or_else(||
                    p.err_owned(format!(
                        "unknown shader ':{}'", atom)))?;
            let p0 = fields.num("p0").unwrap_or(0.0);
            let p1 = fields.num("p1").unwrap_or(0.0);
            let p2 = fields.num("p2").unwrap_or(0.0);
            let p3 = fields.num("p3").unwrap_or(0.0);
            Ok(TriggerSpec::PostShader {
                slot: (slot as u8).min(255),
                p:    [p0, p1, p2, p3],
            })
        }
        "post_shader_off" => Ok(TriggerSpec::PostShaderOff),
        other => Err(p.err_owned(format!("unknown trigger '{}'", other))),
    }
}

fn anim_from(s: Option<&str>) -> Anim {
    match s {
        Some("linear") | None => Anim::Linear,
        Some("ease_in")   | Some("easeIn")    => Anim::EaseIn,
        Some("ease_out")  | Some("easeOut")   => Anim::EaseOut,
        Some("ease_in_out") | Some("easeInOut") => Anim::EaseInOut,
        Some("bounce")                         => Anim::Bounce,
        _ => Anim::Linear,
    }
}