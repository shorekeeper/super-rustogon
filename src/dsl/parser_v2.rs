//! Hand-written lexer and parser for the Rustogon Level Format
//! v2 flavour.
//!
//! This module is a sibling of `parser.rs`. Both produce the
//! same [`LevelAst`] so the rest of the engine does not care
//! which dialect a given `.rlf` file was written in. Selection
//! is driven by the `#[use_v2]` directive and implemented in
//! the dispatcher in `mod.rs`.
//!
//! # Syntactic fingerprints
//!
//! From Elixir:
//!
//! * Atoms `:name` as first-class enum values
//!   (`dir = :ccw`, `range = :Casual..:ExpertPlus2`).
//! * Sigils `~p"path"`, `~m"mask"`, `~f"formula"`. The letter
//!   selects the parser that should read the body.
//! * Trigger pipe `trigger :flip |> :shake { ... }`; every
//!   stage becomes its own `Stmt::Trigger`, applied at the
//!   same scheduling anchor.
//! * `do ... end` as a complete synonym for `{ ... }`.
//!
//! From Haskell:
//!
//! * `section "name" at 0.5 where k = v, j = v do ... end`
//!   introduces local bindings visible inside the body. The
//!   scope is pushed before the body and popped on exit, which
//!   implements the unwind rule the DSL spec requires.
//! * `::` type annotations on `var` declarations.
//! * Enum ranges via `..`.
//! * Bare identifiers are resolved through the variable scope
//!   before falling back to "string-like enum token", so
//!   `steps = rungs` just works.
//!
//! From Vulkan:
//!
//! * `>> :tag { ... }` attaches a typed modifier to the head
//!   of an emit expression. The chain composes left to right
//!   the way pNext extends a `VkCreateInfo`.
//!
//! # Grammar (informal)
//!
//!     file       := directive* 'level' STRING block
//!     block      := '{' item* '}' | 'do' item* 'end'
//!     item       := meta | palette | difficulty | generation
//!                 | global | section
//!     section    := 'section' STRING 'at' num (where_clause)? block
//!     where_cl.  := 'where' binding (',' binding)*
//!     binding    := IDENT ('::' TYPE)? '=' value
//!     stmt       := 'emit' obstacle
//!                 | 'wait' num
//!                 | 'trigger' trig_stage ('|>' trig_stage)*
//!                 | 'repeat' num block
//!                 | 'local' block
//!     obstacle   := name record? ('>>' ATOM record)*
//!     trig_stage := name record?
//!     name       := IDENT | ATOM
//!     value      := num | STRING | IDENT | ATOM | SIGIL
//!                 | 'rgb' '(' num ',' num ',' num ')' | var_ref
//!     var_ref    := '@' IDENT
//!
//! Whitespace, newlines and commas are all equivalent as field
//! separators. Comments start with `//`, `--` or `#` (but
//! `#[` opens a directive).

use crate::dsl::ast::*;
use crate::dsl::formula;
use crate::dsl::parser::ParseError;
use crate::levels::Palette;
use crate::levels::difficulty::Tier;

/// Entry point. Parse a v2 source buffer into the shared AST.
/// The dispatcher in `mod.rs` only calls this when the file
/// has already been confirmed to carry `#[use_v2]`.
pub fn parse_level(src: &str) -> Result<LevelAst, ParseError> {
    let tokens = tokenize(src)?;
    let mut p = ParserV2 {
        toks: tokens,
        i: 0,
        scope: ScopeStack::new(),
    };
    p.parse_file()
}

// ----- lexer -----

#[derive(Clone, Debug)]
enum Tok {
    Ident(String),
    Atom(String),
    Str(String),
    Num(f32),
    /// `~x"..."`. First field is the sigil letter, second is
    /// the raw body (no parsing done here).
    Sigil(char, String),
    LBrace, RBrace,
    LParen, RParen,
    LBracket, RBracket,
    Eq,          // =
    ColonColon,  // ::
    Pipe,        // |>
    Chain,       // >>
    Arrow,       // <-  (reserved for future list comprehensions)
    DotDot,      // ..
    Comma,
    At,          // @
    Hash,        // #
    Eof,
}

#[derive(Clone, Debug)]
struct Token { kind: Tok, line: u32, col: u32 }

fn tokenize(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut toks = Vec::new();
    let bytes: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut col  = 1u32;

    while i < bytes.len() {
        let c = bytes[i];

        if c == '\n' { line += 1; col = 1; i += 1; continue; }
        if c.is_whitespace() { col += 1; i += 1; continue; }

        // Comments.
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

        // Multi-character punctuation. These come before the
        // single-character matches so `::` does not get tokenized
        // as an orphan colon followed by an atom.
        if c == ':' && i + 1 < bytes.len() && bytes[i + 1] == ':' {
            toks.push(Token { kind: Tok::ColonColon, line, col });
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

        // Single-character punctuation.
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
            _   => None,
        };
        if let Some(k) = single {
            toks.push(Token { kind: k, line, col });
            i += 1; col += 1;
            continue;
        }

        // Sigil `~x"..."` where x is any alphabetic letter. The
        // letter is captured so the parser can dispatch based
        // on intent (`p` path, `m` mask, `f` formula, future
        // extensions are forward-compatible here).
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
            }
            if i >= bytes.len() {
                return Err(ParseError {
                    msg: "unterminated sigil body".into(),
                    line: sl, col: sc,
                });
            }
            i += 1; col += 1;
            toks.push(Token { kind: Tok::Sigil(letter, s), line: sl, col: sc });
            continue;
        }

        // Atom `:name`. A colon followed by a letter or underscore
        // starts an atom literal; a colon followed by anything
        // else is rejected here so typos do not silently become
        // empty atoms.
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

        // Number. Decimal, with optional leading minus, or hex
        // with a 0x prefix.
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
                let v = u32::from_str_radix(&s, 16).map_err(|_| ParseError {
                    msg: "hex literal overflow".into(),
                    line: sl, col: sc,
                })?;
                let n = if bytes[i] == '-' { -(v as i64) as f32 }
                        else               { v as f32 };
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
            toks.push(Token { kind: Tok::Num(n), line: sl, col: sc });
            col += (j - i) as u32;
            i = j;
            continue;
        }

        // Identifier.
        if c.is_ascii_alphabetic() || c == '_' {
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

// ----- scope stack -----

/// Lexical variable scope. The bottom layer is the file's
/// `global` block. Each `section where ...`, `repeat`, and
/// `local` pushes a fresh layer that is popped on exit,
/// matching the unwind rule from the spec.
struct ScopeStack { layers: Vec<Vec<VarDecl>> }

impl ScopeStack {
    fn new() -> Self { ScopeStack { layers: vec![Vec::new()] } }
    fn push(&mut self) { self.layers.push(Vec::new()); }
    fn pop (&mut self) { if self.layers.len() > 1 { self.layers.pop(); } }

    fn define(&mut self, v: VarDecl) {
        if let Some(top) = self.layers.last_mut() {
            top.retain(|d| d.name != v.name);
            top.push(v);
        }
    }
    fn resolve(&self, name: &str) -> Option<&VarDecl> {
        for layer in self.layers.iter().rev() {
            if let Some(v) = layer.iter().find(|d| d.name == name) {
                return Some(v);
            }
        }
        None
    }
    fn globals_take(&mut self) -> Vec<VarDecl> {
        std::mem::take(&mut self.layers[0])
    }
}

// ----- parser -----

#[derive(Clone, Copy, Debug)]
enum BlockKind { Brace, Do }

struct ParserV2 {
    toks: Vec<Token>,
    i: usize,
    scope: ScopeStack,
}

impl ParserV2 {
    fn peek(&self) -> &Token { &self.toks[self.i] }
    fn bump(&mut self) -> Token { let t = self.toks[self.i].clone(); self.i += 1; t }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, ParseError> {
        let t = self.peek();
        Err(ParseError { msg: msg.into(), line: t.line, col: t.col })
    }
    fn err_owned(&self, msg: impl Into<String>) -> ParseError {
        let t = self.peek();
        ParseError { msg: msg.into(), line: t.line, col: t.col }
    }

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

    /// Open a block with either `{` or `do`. The kind is
    /// returned so `close_block` can require the matching
    /// terminator later.
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

    /// `=` between key and value is optional. A colon is NOT
    /// accepted here to keep `:atom` unambiguous in value
    /// positions.
    fn skip_opt_eq(&mut self) {
        if matches!(self.peek().kind, Tok::Eq) { self.bump(); }
    }

    // ---------- top level ----------

    fn parse_file(&mut self) -> Result<LevelAst, ParseError> {
        let mut timestamp_format = TimestampFormat::default();

        // Directives. `#[use_v2]` was already consumed by the
        // dispatcher in `mod.rs`, but we accept it here too so
        // the file remains self-describing when tools re-parse
        // it in isolation.
        let mut start_from: f32 = 0.0;
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

            // A directive can carry an unnamed positional
            // value straight after its name (`#[startfrom 30]`
            // or `#[startfrom = ~t"1:30"]`), plus any number of
            // named `k = v` pairs. Both forms are resolved
            // below based on the directive's name.
            if matches!(self.peek().kind, Tok::Eq) { self.bump(); }
            let mut positional: Option<f32> = None;
            if matches!(self.peek().kind, Tok::Num(_) | Tok::Sigil('t', _)) {
                positional = Some(self.read_number()?);
            }

            let mut kv: Vec<(String, f32)> = Vec::new();
            while !matches!(self.peek().kind, Tok::RBracket) {
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
                let k = self.read_ident()?;
                self.skip_opt_eq();
                let v = self.read_number()?;
                kv.push((k, v));
            }
            let t = self.bump();
            if !matches!(t.kind, Tok::RBracket) {
                return Err(ParseError {
                    msg: "expected ']'".into(),
                    line: t.line, col: t.col,
                });
            }

            match name.as_str() {
                "use_v2" => {}
                "startfrom" => {
                    // Accept any of:
                    //   #[startfrom = 30]
                    //   #[startfrom 30]
                    //   #[startfrom ~t"0:30"]
                    //   #[startfrom at = 30]
                    //   #[startfrom seconds = 30]
                    let v = positional
                        .or_else(|| kv.iter()
                            .find(|(k, _)| k == "at" || k == "seconds")
                            .map(|(_, v)| *v));
                    if let Some(v) = v {
                        start_from = v.max(0.0);
                    }
                }
                _ => {
                    if let Some(fmt) = parse_timestamp_directive(&name, &kv) {
                        timestamp_format = fmt;
                    }
                }
            }
        }

        self.expect_kw("level")?;
        let header_name = self.read_string()?;
        let kind = self.open_block()?;

        let mut meta       = Meta::default();
        let mut palette    = Palette::default();
        let mut difficulty = DifficultySpec::default();
        let mut generation = GenerationSpec::default();
        let mut sections   = Vec::new();

        while !self.is_block_end(kind) {
            match self.peek().kind.clone() {
                Tok::Comma => { self.bump(); continue; }
                Tok::Ident(kw) => match kw.as_str() {
                    "meta"       => { self.bump(); meta       = self.parse_meta()?; }
                    "palette"    => { self.bump(); palette    = self.parse_palette()?; }
                    "difficulty" => { self.bump(); difficulty = self.parse_difficulty()?; }
                    "generation" => { self.bump(); generation = self.parse_generation()?; }
                    "global"     => { self.bump(); self.parse_global_block()?; }
                    "section"    => { self.bump(); sections.push(self.parse_section()?); }
                    other => return self.err(format!(
                        "unknown top-level block '{}'", other)),
                }
                _ => return self.err("expected block keyword"),
            }
        }
        self.close_block(kind)?;

        if !matches!(self.peek().kind, Tok::Eof) {
            return self.err("trailing tokens after level block");
        }

        // Fall back to the header name if `meta.name` was left
        // empty, matching v1 semantics.
        if meta.name.is_empty() { meta.name = header_name; }

        if sections.is_empty() {
            sections.push(Section {
                name: "default".into(),
                at:   0.0,
                body: vec![
                    Stmt::Emit(ObstacleSpec::Bar { thickness_mult: 1.0 }),
                    Stmt::Wait(2),
                ],
            });
        }
        sections.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap());

        let globals = self.scope.globals_take();

        Ok(LevelAst {
            meta, palette, difficulty, generation,
            timestamp_format, globals, sections,
            start_from_seconds: start_from,
        })
    }

    // ---------- block parsers ----------

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
                    let n = self.read_number()?;
                    if !(40.0..=300.0).contains(&n) {
                        return self.err(format!("bpm out of range: {}", n));
                    }
                    m.bpm = n as u32;
                }
                other => return self.err(format!("unknown meta field '{}'", other)),
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
                other => return self.err(format!("unknown palette field '{}'", other)),
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
                let r = self.read_number()?.clamp(0.0, 1.0);
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); }
                let g = self.read_number()?.clamp(0.0, 1.0);
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); }
                let b = self.read_number()?.clamp(0.0, 1.0);
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
                let lo = self.read_tier_name()?;
                let lo_t = Tier::from_keyword(&lo).ok_or_else(|| {
                    self.err_owned(format!("unknown tier '{}'", lo))
                })?;
                let t = self.bump();
                if !matches!(t.kind, Tok::DotDot) {
                    return Err(ParseError {
                        msg: "expected '..' in difficulty range".into(),
                        line: t.line, col: t.col,
                    });
                }
                let hi = self.read_tier_name()?;
                let hi_t = Tier::from_keyword(&hi).ok_or_else(|| {
                    self.err_owned(format!("unknown tier '{}'", hi))
                })?;
                d.min_tier = lo_t;
                d.max_tier = hi_t;
                continue;
            }

            let tname = self.read_tier_name()?;
            let tier = Tier::from_keyword(&tname).ok_or_else(|| {
                self.err_owned(format!("unknown tier '{}'", tname))
            })?;
            match field.as_str() {
                "base" | "baseTier" => d.base_tier = tier,
                "min"  | "minTier"  => d.min_tier  = tier,
                "max"  | "maxTier"  => d.max_tier  = tier,
                other => return self.err(format!("unknown difficulty field '{}'", other)),
            }
        }
        self.close_block(kind)?;
        if d.min_tier.rank() > d.max_tier.rank() {
            return self.err("minTier is above maxTier");
        }
        Ok(d)
    }

    /// Tiers can be written either as a plain identifier
    /// (`Expert`) or as an atom (`:Expert`). Both read the same
    /// into the AST.
    fn read_tier_name(&mut self) -> Result<String, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Ident(s) | Tok::Atom(s) => Ok(s),
            _ => Err(ParseError {
                msg: "expected tier name".into(),
                line: t.line, col: t.col,
            }),
        }
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
                    let n = self.read_number()?;
                    if !(3.0..=12.0).contains(&n) {
                        return self.err(format!("sides out of range: {}", n));
                    }
                    g.sides = n as u32;
                }
                "seed" => g.seed = self.read_number()? as u32,
                "hueSpeed" | "hue_speed" =>
                    g.hue_speed = self.read_number()?.clamp(-4.0, 4.0),
                "speedMult"   | "speed"   =>
                    g.speed_mult = self.read_number()?.clamp(0.25, 3.0),
                "densityMult" | "density" =>
                    g.density_mult = self.read_number()?.clamp(0.25, 3.0),
                other => return self.err(format!("unknown generation field '{}'", other)),
            }
        }
        self.close_block(kind)?;
        Ok(g)
    }

    fn parse_global_block(&mut self) -> Result<(), ParseError> {
        let kind = self.open_block()?;
        while !self.is_block_end(kind) {
            if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
            self.expect_kw("var")?;
            let d = self.parse_var_decl_body()?;
            self.scope.define(d);
        }
        self.close_block(kind)?;
        Ok(())
    }

    fn parse_var_decl_body(&mut self) -> Result<VarDecl, ParseError> {
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
        let value = self.read_var_value()?;

        // Optional modifiers `[pub]`, `[priv]`, `[read]`.
        let mut access = VarAccess::Public;
        if matches!(self.peek().kind, Tok::LBracket) {
            self.bump();
            while !matches!(self.peek().kind, Tok::RBracket) {
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
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

    fn read_var_value(&mut self) -> Result<VarValue, ParseError> {
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
            Tok::At => {
                let name = self.read_ident()?;
                self.scope.resolve(&name).map(|d| d.value.clone())
                    .ok_or_else(|| self.err_owned(
                        format!("undefined variable '@{}'", name)))
            }
            _ => Err(ParseError {
                msg: "expected value".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    // ---------- section ----------

    fn parse_section(&mut self) -> Result<Section, ParseError> {
        let name = self.read_string()?;
        self.expect_kw("at")?;
        let at = self.read_number()?;

        // Push a fresh scope for the section body AND its
        // optional `where` clause. Popped at the end.
        self.scope.push();

        if matches!(&self.peek().kind, Tok::Ident(s) if s == "where") {
            self.bump();
            loop {
                // Stop at the block opener.
                if matches!(self.peek().kind, Tok::LBrace) { break; }
                if matches!(&self.peek().kind, Tok::Ident(s) if s == "do") { break; }
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }

                let vname = self.read_ident()?;
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
                        _ => ty,
                    };
                }
                let tok = self.bump();
                if !matches!(tok.kind, Tok::Eq) {
                    return Err(ParseError {
                        msg: "expected '=' in where binding".into(),
                        line: tok.line, col: tok.col,
                    });
                }
                let value = self.read_var_value()?;
                self.scope.define(VarDecl {
                    name: vname, ty, value, access: VarAccess::Private,
                });
            }
        }

        let kind = self.open_block()?;
        let body = self.parse_stmt_block(kind)?;
        self.scope.pop();
        Ok(Section { name, at, body })
    }

    fn parse_stmt_block(&mut self, kind: BlockKind) -> Result<Vec<Stmt>, ParseError> {
        let mut body = Vec::new();
        while !self.is_block_end(kind) {
            match self.peek().kind.clone() {
                Tok::Comma => { self.bump(); continue; }
                Tok::Ident(ref s) if s == "emit" => {
                    self.bump();
                    body.push(Stmt::Emit(self.parse_obstacle()?));
                }
                Tok::Ident(ref s) if s == "wait" => {
                    self.bump();
                    let n = self.read_number()?;
                    if !(1.0..=64.0).contains(&n) {
                        return self.err(format!("wait out of range: {}", n));
                    }
                    body.push(Stmt::Wait(n as u32));
                }
                Tok::Ident(ref s) if s == "trigger" => {
                    self.bump();
                    // Head of the pipe chain.
                    let first = self.parse_trigger_stage()?;
                    body.push(Stmt::Trigger(first));
                    // Subsequent stages, if any. They fire
                    // sequentially at the same scheduling anchor
                    // since `apply_trigger` does not advance time.
                    while matches!(self.peek().kind, Tok::Pipe) {
                        self.bump();
                        let next = self.parse_trigger_stage()?;
                        body.push(Stmt::Trigger(next));
                    }
                }
                Tok::Ident(ref s) if s == "repeat" => {
                    self.bump();
                    let n = self.read_number()?;
                    if !(1.0..=64.0).contains(&n) {
                        return self.err(format!("repeat count out of range: {}", n));
                    }
                    let sub = self.open_block()?;
                    self.scope.push();
                    let inner = self.parse_stmt_block(sub)?;
                    self.scope.pop();
                    body.push(Stmt::Repeat { count: n as u32, body: inner });
                }
                Tok::Ident(ref s) if s == "local" => {
                    self.bump();
                    let sub = self.open_block()?;
                    let mut decls = Vec::new();
                    while !self.is_block_end(sub) {
                        if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
                        self.expect_kw("var")?;
                        let d = self.parse_var_decl_body()?;
                        self.scope.define(d.clone());
                        decls.push(d);
                    }
                    self.close_block(sub)?;
                    body.push(Stmt::LocalVars(decls));
                }
                other => return self.err(format!("unexpected {:?} in body", other)),
            }
        }
        self.close_block(kind)?;
        Ok(body)
    }

    // ---------- obstacles ----------

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
        let mut spec = self.build_obstacle(&name, &fields)?;

        // `>> :tag { ... }` modifier chain, left-to-right. A
        // chain link is allowed to be a bare atom with no body.
        while matches!(self.peek().kind, Tok::Chain) {
            self.bump();
            let t = self.bump();
            let tag = match t.kind {
                Tok::Atom(s) | Tok::Ident(s) => s,
                _ => return Err(ParseError {
                    msg: "expected :tag after '>>'".into(),
                    line: t.line, col: t.col,
                }),
            };
            let fs = if matches!(self.peek().kind, Tok::LBrace) {
                self.parse_field_map()?
            } else {
                Fields::default()
            };
            apply_chain_modifier(&mut spec, &tag, &fs)
                .map_err(|m| self.err_owned(m))?;
        }

        Ok(spec)
    }

    fn build_obstacle(
        &self, name: &str, fields: &Fields,
    ) -> Result<ObstacleSpec, ParseError> {
        match name {
            "bar" => {
                let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.5);
                Ok(ObstacleSpec::Bar { thickness_mult: t })
            }
            "doubleBar" | "double_bar" => {
                let spacing = fields.num("spacing").unwrap_or(2.0)
                    .clamp(1.0, 6.0) as u32;
                let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.5);
                Ok(ObstacleSpec::DoubleBar { spacing, thickness_mult: t })
            }
            "spiral" => {
                let dir = dir_from(fields);
                let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                let loops = fields.num("loops").unwrap_or(2.0)
                    .clamp(1.0, 6.0) as u32;
                Ok(ObstacleSpec::Spiral { dir, thickness_mult: t, loops })
            }
            "alternate" => {
                let parity = match fields.word("parity").as_deref() {
                    Some("odd") => Parity::Odd,
                    _           => Parity::Even,
                };
                let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                Ok(ObstacleSpec::Alternate { parity, thickness_mult: t })
            }
            "pinwheel" => {
                let spokes = fields.num("spokes").unwrap_or(3.0)
                    .clamp(1.0, 6.0) as u32;
                let dir = dir_from(fields);
                Ok(ObstacleSpec::Pinwheel { spokes, dir })
            }
            "rain" => {
                let count = fields.num("count").unwrap_or(5.0)
                    .clamp(2.0, 12.0) as u32;
                let t = fields.num("thickness").unwrap_or(0.85).clamp(0.5, 1.5);
                Ok(ObstacleSpec::Rain { count, thickness_mult: t })
            }
            "rainbow" => Ok(ObstacleSpec::Rainbow { dir: dir_from(fields) }),
            "ladder" => {
                let rungs = fields.num("rungs").unwrap_or(6.0)
                    .clamp(3.0, 16.0) as u32;
                Ok(ObstacleSpec::Ladder { rungs })
            }
            "tunnel" => {
                let length = fields.num("length").unwrap_or(1.2).clamp(0.4, 2.5);
                let lanes  = fields.num("lanes").unwrap_or(3.0).clamp(1.0, 4.0) as u32;
                Ok(ObstacleSpec::Tunnel { length, lanes })
            }
            "pot" => {
                let layers = fields.num("layers").unwrap_or(4.0)
                    .clamp(2.0, 8.0) as u32;
                Ok(ObstacleSpec::Pot { layers })
            }
            "staircase" => {
                let dir = dir_from(fields);
                let steps = fields.num("steps").unwrap_or(6.0)
                    .clamp(3.0, 24.0) as u32;
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
                let layers = fields.num("layers").unwrap_or(4.0)
                    .clamp(2.0, 8.0) as u32;
                let dir = dir_from(fields);
                Ok(ObstacleSpec::Cubes { layers, dir })
            }
            "custom" => {
                let mask_str = fields.text("mask").ok_or_else(|| {
                    self.err_owned("custom obstacle requires mask = ~m\"...\"")
                })?;
                let mut mask = Vec::with_capacity(mask_str.len());
                for c in mask_str.chars() {
                    match c {
                        '1' => mask.push(true),
                        '0' => mask.push(false),
                        _ => return self.err(format!(
                            "mask must contain only 0 and 1, found '{}'", c)),
                    }
                }
                if mask.is_empty() || !mask.iter().any(|b| !*b) {
                    return self.err("custom mask must leave at least one gap");
                }
                let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                Ok(ObstacleSpec::Custom { mask, thickness_mult: t })
            }
            "formula" | "custom_formula" => {
                let src = fields.text("formula").ok_or_else(|| {
                    self.err_owned("formula obstacle requires formula = ~f\"...\"")
                })?;
                let steps = fields.num("steps").unwrap_or(6.0)
                    .clamp(1.0, 32.0) as u32;
                let t = fields.num("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                let program = formula::parse(&src).map_err(|m| self.err_owned(m))?;
                let seed = fields.num("seed").unwrap_or(0.0) as u32;
                formula::validate_pathable(&program, steps, 6, seed)
                    .map_err(|m| self.err_owned(m))?;
                Ok(ObstacleSpec::CustomFormula {
                    formula: Formula { source: src, program },
                    steps, thickness_mult: t,
                })
            }
            other => self.err(format!("unknown obstacle '{}'", other)),
        }
    }

    // ---------- triggers ----------

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

        match name.as_str() {
            "flip"  => Ok(TriggerSpec::Flip),
            "pulse" => Ok(TriggerSpec::Pulse),
            "tilt" => {
                let deg = fields.num("angle")
                    .or_else(|| fields.num("deg"))
                    .unwrap_or(0.0);
                Ok(TriggerSpec::Tilt(deg.clamp(-2.5, 2.5)))
            }
            "speedMult" => {
                let m = fields.num("factor").unwrap_or(1.0);
                Ok(TriggerSpec::SpeedMult(m.clamp(0.5, 2.0)))
            }
            "hueShift" => {
                let r = fields.num("rate").unwrap_or(0.0);
                Ok(TriggerSpec::HueShift(r.clamp(-2.0, 2.0)))
            }
            "speedwarp" => {
                let walls    = fields.num("walls").unwrap_or(0.0);
                let rotation = fields.num("rotation").unwrap_or(0.0);
                let cursor   = fields.num("cursor").unwrap_or(0.0);
                let music    = fields.num("musicScale")
                    .or_else(|| fields.num("music"))
                    .unwrap_or(0.0);
                let duration = fields.num("duration").unwrap_or(3.0).clamp(0.1, 20.0);
                Ok(TriggerSpec::SpeedWarp {
                    walls, rotation, cursor, music_scale: music, duration,
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
                let anim = parse_anim(fields.word("anim").as_deref());
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
            other => self.err(format!("unknown trigger '{}'", other)),
        }
    }

    // ---------- fields ----------

    fn parse_field_map(&mut self) -> Result<Fields, ParseError> {
        let t = self.bump();
        if !matches!(t.kind, Tok::LBrace) {
            return Err(ParseError {
                msg: "expected '{' for field map".into(),
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
        }
        self.bump();
        Ok(f)
    }

    /// Field values accept the full variety: numbers, strings,
    /// sigils (stored as raw strings, the parser above
    /// interprets them per-field), atoms, identifiers (possibly
    /// resolved from scope), and explicit `@refs`.
    fn read_field_value(&mut self) -> Result<FieldValue, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n)      => Ok(FieldValue::Num(n)),
            Tok::Str(s)      => Ok(FieldValue::Str(s)),
            Tok::Sigil(letter, s) => {
                // `~t"..."` is the only sigil that resolves to
                // a number; the others (`~p`, `~m`, `~f`) keep
                // their raw string body so the obstacle
                // builder can interpret them.
                if letter == 't' {
                    parse_time_sigil(&s).map(FieldValue::Num)
                        .map_err(|m| ParseError {
                            msg: m, line: t.line, col: t.col,
                        })
                } else {
                    Ok(FieldValue::Str(s))
                }
            }
            Tok::Atom(s)     => Ok(FieldValue::Ident(s)),
            Tok::Ident(s) => {
                // Bare identifier: resolve via scope first, so
                // `steps = rungs` picks up the `where`-bound
                // value. If nothing matches, treat as an enum
                // token (like `cw`, `even`).
                if let Some(d) = self.scope.resolve(&s) {
                    Ok(match &d.value {
                        VarValue::Num(n)   => FieldValue::Num(*n),
                        VarValue::Str(s)   => FieldValue::Str(s.clone()),
                        VarValue::Ident(s) => FieldValue::Ident(s.clone()),
                        VarValue::Bool(b)  => FieldValue::Num(if *b { 1.0 } else { 0.0 }),
                    })
                } else {
                    Ok(FieldValue::Ident(s))
                }
            }
            Tok::At => {
                let name = self.read_ident()?;
                let d = self.scope.resolve(&name).ok_or_else(|| {
                    self.err_owned(format!("undefined '@{}'", name))
                })?;
                Ok(match &d.value {
                    VarValue::Num(n)   => FieldValue::Num(*n),
                    VarValue::Str(s)   => FieldValue::Str(s.clone()),
                    VarValue::Ident(s) => FieldValue::Ident(s.clone()),
                    VarValue::Bool(b)  => FieldValue::Num(if *b { 1.0 } else { 0.0 }),
                })
            }
            _ => Err(ParseError {
                msg: "expected value".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    /// Standalone numeric reader. Accepts a literal, a bare
    /// ident resolved via scope, or `@name`.
    fn read_number(&mut self) -> Result<f32, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n) => Ok(n),
            Tok::Sigil('t', body) => parse_time_sigil(&body)
                .map_err(|m| ParseError { msg: m, line: t.line, col: t.col }),
            Tok::Ident(s) => {
                let d = self.scope.resolve(&s).ok_or_else(|| ParseError {
                    msg: format!("unknown identifier '{}' (not a number)", s),
                    line: t.line, col: t.col,
                })?;
                match &d.value {
                    VarValue::Num(n)  => Ok(*n),
                    VarValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
                    _ => Err(ParseError {
                        msg: format!("'{}' is not numeric", s),
                        line: t.line, col: t.col,
                    }),
                }
            }
            Tok::At => {
                let name = self.read_ident()?;
                let d = self.scope.resolve(&name).ok_or_else(|| {
                    self.err_owned(format!("undefined '@{}'", name))
                })?;
                match &d.value {
                    VarValue::Num(n)  => Ok(*n),
                    VarValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
                    _ => Err(ParseError {
                        msg: format!("'@{}' is not numeric", name),
                        line: t.line, col: t.col,
                    }),
                }
            }
            _ => Err(ParseError {
                msg: "expected number".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    /// Standalone string-ish reader: literal string, any sigil
    /// body, or `@name` resolving to a string/ident.
    fn read_stringish(&mut self) -> Result<String, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Str(s)      => Ok(s),
            Tok::Sigil(_, s) => Ok(s),
            Tok::At => {
                let name = self.read_ident()?;
                let d = self.scope.resolve(&name).ok_or_else(|| {
                    self.err_owned(format!("undefined '@{}'", name))
                })?;
                match &d.value {
                    VarValue::Str(s)   => Ok(s.clone()),
                    VarValue::Ident(s) => Ok(s.clone()),
                    _ => Err(ParseError {
                        msg: format!("'@{}' is not a string", name),
                        line: t.line, col: t.col,
                    }),
                }
            }
            _ => Err(ParseError {
                msg: "expected string or sigil".into(),
                line: t.line, col: t.col,
            }),
        }
    }
}

// ----- helpers -----

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
    /// Ident or atom (both stored as `Ident`).
    fn word(&self, k: &str) -> Option<String> {
        match self.get(k) { Some(FieldValue::Ident(s)) => Some(s.clone()), _ => None }
    }
    fn text(&self, k: &str) -> Option<String> {
        match self.get(k) { Some(FieldValue::Str(s)) => Some(s.clone()), _ => None }
    }
}

fn dir_from(fields: &Fields) -> SpinDir {
    match fields.word("dir").as_deref() {
        Some("ccw") => SpinDir::Ccw,
        _           => SpinDir::Cw,
    }
}

/// Parse the body of a `~t"..."` sigil into a number of
/// seconds. Accepted shapes:
///
///     ~t"S"            -> S seconds (plain float)
///     ~t"M:SS"         -> M minutes + SS seconds
///     ~t"M:SS.fff"     -> ditto plus fractional seconds
///     ~t"H:MM:SS"      -> H hours + MM minutes + SS seconds
///     ~t"H:MM:SS.fff"  -> ditto plus fractional seconds
///
/// Each colon-separated component is required to fit its
/// natural domain (`SS`, `MM` < 60), but the leading
/// component is unbounded so `~t"180:00"` is a valid way to
/// say "180 minutes from track start" on a long mix. Returns
/// an error message suitable for a `ParseError::msg` when
/// anything fails to parse.
fn parse_time_sigil(body: &str) -> Result<f32, String> {
    let parts: Vec<&str> = body.split(':').collect();
    let to_f = |s: &str| -> Result<f32, String> {
        s.parse::<f32>().map_err(|_| format!("bad number '{}' in ~t", s))
    };
    match parts.len() {
        1 => to_f(parts[0]),
        2 => {
            let m = to_f(parts[0])?;
            let s = to_f(parts[1])?;
            if !(0.0..60.0).contains(&s) {
                return Err(format!("seconds out of 0..60 in ~t: {}", s));
            }
            Ok(m * 60.0 + s)
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
            Ok(h * 3600.0 + m * 60.0 + s)
        }
        _ => Err(format!("~t must have 1, 2 or 3 colon-separated parts, got {}",
                         parts.len())),
    }
}

fn parse_anim(s: Option<&str>) -> Anim {
    match s {
        Some("linear") | None                   => Anim::Linear,
        Some("ease_in")   | Some("easeIn")      => Anim::EaseIn,
        Some("ease_out")  | Some("easeOut")     => Anim::EaseOut,
        Some("ease_in_out") | Some("easeInOut") => Anim::EaseInOut,
        Some("bounce")                           => Anim::Bounce,
        _ => Anim::Linear,
    }
}

fn parse_timestamp_directive(
    name: &str, kv: &[(String, f32)],
) -> Option<TimestampFormat> {
    let prefix = "timestamp_format_use_";
    let tag = name.strip_prefix(prefix)?;
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

/// Apply a `>> :tag { ... }` modifier to an already built
/// obstacle. Unknown tags are rejected with a helpful error so
/// typos do not silently degrade.
fn apply_chain_modifier(
    spec: &mut ObstacleSpec, tag: &str, fields: &Fields,
) -> Result<(), String> {
    match tag {
        "thickness" => {
            let mult = fields.num("mult")
                .or_else(|| fields.num("value"))
                .unwrap_or(1.0).clamp(0.25, 3.0);
            apply_thickness(spec, mult);
            Ok(())
        }
        "telegraph" => {
            // Visual hint. No engine-side handling yet; accepted
            // so forward-compatible files keep parsing.
            Ok(())
        }
        other => Err(format!("unknown chain modifier '>> :{}'", other)),
    }
}

fn apply_thickness(spec: &mut ObstacleSpec, mult: f32) {
    match spec {
        ObstacleSpec::Bar { thickness_mult }
        | ObstacleSpec::DoubleBar { thickness_mult, .. }
        | ObstacleSpec::Spiral { thickness_mult, .. }
        | ObstacleSpec::Alternate { thickness_mult, .. }
        | ObstacleSpec::Rain { thickness_mult, .. }
        | ObstacleSpec::Custom { thickness_mult, .. }
        | ObstacleSpec::Staircase { thickness_mult, .. }
        | ObstacleSpec::CustomFormula { thickness_mult, .. } => {
            *thickness_mult = (*thickness_mult * mult).clamp(0.25, 3.0);
        }
        // Other patterns do not carry an adjustable thickness.
        // The chain link is a no-op on them (documented, not an
        // error) so authors can reuse modifier lists across
        // mixed lists of emits.
        _ => {}
    }
}