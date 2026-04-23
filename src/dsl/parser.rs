//! Hand written lexer plus recursive descent parser for RLFv2.
//!
//! The grammar lives in `dsl/mod.rs` and deliberately blends
//! three familiar looks:
//!
//! * Vulkan descriptor records: named blocks between braces, one
//!   field per line, repetition means "overwrite".
//! * Haskell brevity: `=`, `:` and plain whitespace all work as
//!   "this is the value of this field", commas are decoration.
//! * Elixir readability: `#[...]` directives, `@name` variable
//!   references, colon introduced atoms.
//!
//! Every block can be written inline or spread across as many
//! lines as the author wants; the lexer discards whitespace and
//! commas. Errors carry a `(line, col)` pair so editors can jump
//! straight to the bad token.

use crate::dsl::ast::*;
use crate::dsl::formula;
use crate::levels::Palette;
use crate::levels::difficulty::Tier;

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

/// Entry point. Parse an entire `.rlf` source buffer.
pub fn parse_level(src: &str) -> Result<LevelAst, ParseError> {
    let tokens = tokenize(src)?;
    let mut p = Parser { toks: tokens, i: 0, scope: ScopeStack::new() };
    p.parse_file()
}

// --------------- lexer ---------------

#[derive(Clone, Debug)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f32),
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Eq,
    Colon,
    At,
    Hash,
    Comma,
    DotDot,
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

        // Whitespace / newlines.
        if c == '\n' { line += 1; col = 1; i += 1; continue; }
        if c.is_whitespace() { col += 1; i += 1; continue; }

        // Comments. `//` and `--` go to end of line. `#` only
        // starts a comment when not followed by `[` (in which
        // case it opens a directive).
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

        let sl = line; let sc = col;

        // Single / multi char punctuation.
        match c {
            '{' => { toks.push(Token { kind: Tok::LBrace,   line, col }); i += 1; col += 1; continue; }
            '}' => { toks.push(Token { kind: Tok::RBrace,   line, col }); i += 1; col += 1; continue; }
            '(' => { toks.push(Token { kind: Tok::LParen,   line, col }); i += 1; col += 1; continue; }
            ')' => { toks.push(Token { kind: Tok::RParen,   line, col }); i += 1; col += 1; continue; }
            '[' => { toks.push(Token { kind: Tok::LBracket, line, col }); i += 1; col += 1; continue; }
            ']' => { toks.push(Token { kind: Tok::RBracket, line, col }); i += 1; col += 1; continue; }
            '=' => { toks.push(Token { kind: Tok::Eq,       line, col }); i += 1; col += 1; continue; }
            ':' => { toks.push(Token { kind: Tok::Colon,    line, col }); i += 1; col += 1; continue; }
            '@' => { toks.push(Token { kind: Tok::At,       line, col }); i += 1; col += 1; continue; }
            '#' => { toks.push(Token { kind: Tok::Hash,     line, col }); i += 1; col += 1; continue; }
            ',' => { toks.push(Token { kind: Tok::Comma,    line, col }); i += 1; col += 1; continue; }
            _   => {}
        }
        if c == '.' && i + 1 < bytes.len() && bytes[i + 1] == '.' {
            toks.push(Token { kind: Tok::DotDot, line, col });
            i += 2; col += 2;
            continue;
        }

        // String literal.
        if c == '"' {
            i += 1; col += 1;
            let mut s = String::new();
            while i < bytes.len() && bytes[i] != '"' {
                if bytes[i] == '\n' {
                    return Err(ParseError {
                        msg:  "unterminated string".into(),
                        line: sl, col: sc,
                    });
                }
                s.push(bytes[i]); i += 1; col += 1;
            }
            if i >= bytes.len() {
                return Err(ParseError {
                    msg:  "unterminated string".into(),
                    line: sl, col: sc,
                });
            }
            i += 1; col += 1;
            toks.push(Token { kind: Tok::Str(s), line: sl, col: sc });
            continue;
        }

        // Number (decimal or hex).
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
                        msg: "bad hex literal".into(), line: sl, col: sc,
                    });
                }
                let s: String = bytes[start..k].iter().collect();
                let v = u32::from_str_radix(&s, 16).map_err(|_| ParseError {
                    msg: "hex literal overflow".into(), line: sl, col: sc,
                })?;
                let n = if bytes[i] == '-' { -(v as i64) as f32 } else { v as f32 };
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
                    msg: "stray '-'".into(), line: sl, col: sc,
                });
            }
            let n: f32 = s.parse().map_err(|_| ParseError {
                msg: format!("bad number '{}'", s), line: sl, col: sc,
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

// --------------- variable scope ---------------

/// Simple scope stack used by the parser to resolve `@name`
/// references. The bottom layer is the globals; each `local
/// { ... }` or `section { ... }` or `repeat { ... }` pushes a
/// new layer which is popped on exit, implementing the unwind
/// semantics required by the spec.
struct ScopeStack { layers: Vec<Vec<VarDecl>> }

impl ScopeStack {
    fn new() -> Self { ScopeStack { layers: vec![Vec::new()] } }
    fn push(&mut self)  { self.layers.push(Vec::new()); }
    fn pop (&mut self)  { if self.layers.len() > 1 { self.layers.pop(); } }

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

// --------------- parser core ---------------

struct Parser { toks: Vec<Token>, i: usize, scope: ScopeStack }

impl Parser {
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

    fn expect_ident(&mut self, kw: &str) -> Result<(), ParseError> {
        let t = self.bump();
        match &t.kind {
            Tok::Ident(s) if s == kw => Ok(()),
            other => Err(ParseError {
                msg: format!("expected '{}', got {:?}", kw, other),
                line: t.line, col: t.col,
            }),
        }
    }
    fn expect_lbrace(&mut self) -> Result<(), ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::LBrace => Ok(()),
            _ => Err(ParseError { msg: "expected '{'".into(), line: t.line, col: t.col }),
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

    /// Consume an optional `=` or `:` between a field name and
    /// its value so `name = "X"`, `name: "X"` and `name "X"` all
    /// mean the same thing.
    fn eat_assign(&mut self) {
        match self.peek().kind {
            Tok::Eq | Tok::Colon => { self.bump(); }
            _ => {}
        }
    }

    // --------------- top level ---------------

    fn parse_file(&mut self) -> Result<LevelAst, ParseError> {
        let mut timestamp_format = TimestampFormat::default();

        // Leading directives `#[...]`.
        loop {
            if matches!(self.peek().kind, Tok::Hash) {
                self.bump();
                let t = self.bump();
                if !matches!(t.kind, Tok::LBracket) {
                    return Err(ParseError {
                        msg: "expected '[' after '#'".into(),
                        line: t.line, col: t.col,
                    });
                }
                let name = self.read_ident()?;
                let mut kv: Vec<(String, f32)> = Vec::new();
                while !matches!(self.peek().kind, Tok::RBracket) {
                    if matches!(self.peek().kind, Tok::Comma) { self.bump(); continue; }
                    let k = self.read_ident()?;
                    self.eat_assign();
                    let v = self.read_number_resolved()?;
                    kv.push((k, v));
                }
                let t = self.bump();
                if !matches!(t.kind, Tok::RBracket) {
                    return Err(ParseError {
                        msg: "expected ']'".into(),
                        line: t.line, col: t.col,
                    });
                }
                timestamp_format = parse_timestamp_directive(&name, &kv)
                    .map_err(|m| self.err_owned(m))?;
            } else { break; }
        }

        self.expect_ident("level")?;
        // Keep the header string around so it can serve as a
        // default `meta.name` for files whose `meta` block does
        // not carry an explicit name field. Matches v1 semantics.
        let level_header_name = self.read_string()?;
        self.expect_lbrace()?;

        let mut meta       = Meta::default();
        let mut palette    = Palette::default();
        let mut difficulty = DifficultySpec::default();
        let mut generation = GenerationSpec::default();
        let mut sections   = Vec::new();
        let mut saw_globals = false;

        loop {
            match self.peek().kind.clone() {
                Tok::RBrace => { self.bump(); break; }
                Tok::Ident(kw) => match kw.as_str() {
                    "meta"       => { self.bump(); meta       = self.parse_meta()?; }
                    "palette"    => { self.bump(); palette    = self.parse_palette()?; }
                    "difficulty" => { self.bump(); difficulty = self.parse_difficulty()?; }
                    "generation" => { self.bump(); generation = self.parse_generation()?; }
                    "global"     => {
                        self.bump();
                        self.parse_global_block()?;
                        saw_globals = true;
                    }
                    "section"    => {
                        self.bump();
                        sections.push(self.parse_section()?);
                    }
                    other => return self.err(format!(
                        "unknown top level block '{}'", other)),
                }
                _ => return self.err("expected block keyword"),
            }
        }

        if !matches!(self.peek().kind, Tok::Eof) {
            return self.err("trailing tokens after level block");
        }

        // Fallback: if the author did not provide `meta.name`
        // use the header string from `level "..." { ... }`.
        if meta.name.is_empty() {
            meta.name = level_header_name;
        }

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
        let _ = saw_globals;

        Ok(LevelAst {
            meta, palette, difficulty, generation,
            timestamp_format, globals, sections,
            start_from_seconds: 0.0,
        })
    }

    fn parse_meta(&mut self) -> Result<Meta, ParseError> {
        self.expect_lbrace()?;
        let mut m = Meta::default();
        loop {
            if matches!(self.peek().kind, Tok::RBrace) { self.bump(); break; }
            if matches!(self.peek().kind, Tok::Comma)  { self.bump(); continue; }
            let field = self.read_ident()?;
            self.eat_assign();
            match field.as_str() {
                "name"        => m.name        = self.read_string_resolved()?,
                "subtitle"    => m.subtitle    = self.read_string_resolved()?,
                "author"      => m.author      = self.read_string_resolved()?,
                "song"        => m.song        = self.read_string_resolved()?,
                "music"       => m.music       = self.read_string_resolved()?,
                "description" => m.description = self.read_string_resolved()?,
                "bpm"         => {
                    let n = self.read_number_resolved()?;
                    if !(40.0..=300.0).contains(&n) {
                        return self.err(format!("bpm out of range: {}", n));
                    }
                    m.bpm = n as u32;
                }
                other => return self.err(format!("unknown meta field '{}'", other)),
            }
        }
        Ok(m)
    }

    fn parse_palette(&mut self) -> Result<Palette, ParseError> {
        self.expect_lbrace()?;
        let mut p = Palette::default();
        loop {
            if matches!(self.peek().kind, Tok::RBrace) { self.bump(); break; }
            if matches!(self.peek().kind, Tok::Comma)  { self.bump(); continue; }
            let field = self.read_ident()?;
            self.eat_assign();
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
        Ok(p)
    }

    fn read_rgb(&mut self) -> Result<[f32; 3], ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Ident(ref s) if s == "rgb" => {
                let paren = matches!(self.peek().kind, Tok::LParen);
                if paren { self.bump(); }
                let r = self.read_number_resolved()?.clamp(0.0, 1.0);
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); }
                let g = self.read_number_resolved()?.clamp(0.0, 1.0);
                if matches!(self.peek().kind, Tok::Comma) { self.bump(); }
                let b = self.read_number_resolved()?.clamp(0.0, 1.0);
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
                msg: "expected 'rgb r g b' or 'rgb(r, g, b)'".into(),
                line: t.line, col: t.col,
            }),
        }
    }

    fn parse_difficulty(&mut self) -> Result<DifficultySpec, ParseError> {
        self.expect_lbrace()?;
        let mut d = DifficultySpec::default();
        loop {
            if matches!(self.peek().kind, Tok::RBrace) { self.bump(); break; }
            if matches!(self.peek().kind, Tok::Comma)  { self.bump(); continue; }
            let field = self.read_ident()?;
            self.eat_assign();

            // Accept `Rookie`, `Rookie..Expert`, or `range: Rookie..Expert`.
            if field == "range" {
                let lo = self.read_ident()?;
                let lo_t = Tier::from_keyword(&lo)
                    .ok_or_else(|| self.err_owned(format!("unknown tier '{}'", lo)))?;
                let t = self.bump();
                if !matches!(t.kind, Tok::DotDot) {
                    return Err(ParseError {
                        msg: "expected '..' in range".into(),
                        line: t.line, col: t.col,
                    });
                }
                let hi = self.read_ident()?;
                let hi_t = Tier::from_keyword(&hi)
                    .ok_or_else(|| self.err_owned(format!("unknown tier '{}'", hi)))?;
                d.min_tier = lo_t;
                d.max_tier = hi_t;
                continue;
            }

            let tname = self.read_ident()?;
            let tier = Tier::from_keyword(&tname)
                .ok_or_else(|| self.err_owned(format!("unknown tier '{}'", tname)))?;
            match field.as_str() {
                "base" | "baseTier"    => d.base_tier = tier,
                "min"  | "minTier"     => d.min_tier  = tier,
                "max"  | "maxTier"     => d.max_tier  = tier,
                other => return self.err(format!("unknown difficulty field '{}'", other)),
            }
        }
        if d.min_tier.rank() > d.max_tier.rank() {
            return self.err("minTier is above maxTier");
        }
        Ok(d)
    }

    fn parse_generation(&mut self) -> Result<GenerationSpec, ParseError> {
        self.expect_lbrace()?;
        let mut g = GenerationSpec::default();
        loop {
            if matches!(self.peek().kind, Tok::RBrace) { self.bump(); break; }
            if matches!(self.peek().kind, Tok::Comma)  { self.bump(); continue; }
            let field = self.read_ident()?;
            self.eat_assign();
            match field.as_str() {
                "sides" => {
                    let n = self.read_number_resolved()?;
                    if !(3.0..=12.0).contains(&n) {
                        return self.err(format!("sides out of range: {}", n));
                    }
                    g.sides = n as u32;
                }
                "seed" => {
                    let n = self.read_number_resolved()?;
                    g.seed = n as u32;
                }
                "hueSpeed" | "hue_speed" =>
                    g.hue_speed = self.read_number_resolved()?.clamp(-4.0, 4.0),
                "speedMult" | "speed" =>
                    g.speed_mult = self.read_number_resolved()?.clamp(0.25, 3.0),
                "densityMult" | "density" =>
                    g.density_mult = self.read_number_resolved()?.clamp(0.25, 3.0),
                other => return self.err(format!("unknown generation field '{}'", other)),
            }
        }
        Ok(g)
    }

    // --------------- variables ---------------

    fn parse_global_block(&mut self) -> Result<(), ParseError> {
        self.expect_lbrace()?;
        loop {
            if matches!(self.peek().kind, Tok::RBrace) { self.bump(); break; }
            if matches!(self.peek().kind, Tok::Comma)  { self.bump(); continue; }
            self.expect_ident("var")?;
            let d = self.parse_var_decl_body()?;
            self.scope.define(d);
        }
        Ok(())
    }

    fn parse_local_block(&mut self) -> Result<Vec<VarDecl>, ParseError> {
        self.expect_lbrace()?;
        let mut out = Vec::new();
        loop {
            if matches!(self.peek().kind, Tok::RBrace) { self.bump(); break; }
            if matches!(self.peek().kind, Tok::Comma)  { self.bump(); continue; }
            self.expect_ident("var")?;
            let d = self.parse_var_decl_body()?;
            self.scope.define(d.clone());
            out.push(d);
        }
        Ok(out)
    }

    fn parse_var_decl_body(&mut self) -> Result<VarDecl, ParseError> {
        let name = self.read_ident()?;
        let mut ty = VarType::Float;
        if matches!(self.peek().kind, Tok::Colon) {
            self.bump();
            let tn = self.read_ident()?;
            ty = match tn.as_str() {
                "i32" | "int"       => VarType::Int,
                "f32" | "float"     => VarType::Float,
                "str" | "string"    => VarType::String,
                "ident"             => VarType::Ident,
                "bool"              => VarType::Bool,
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
        let val_tok = self.bump();
        let value = match val_tok.kind {
            Tok::Num(n)     => VarValue::Num(n),
            Tok::Str(s)     => VarValue::Str(s),
            Tok::Ident(s) => {
                match s.as_str() {
                    "true"  => VarValue::Bool(true),
                    "false" => VarValue::Bool(false),
                    _       => VarValue::Ident(s),
                }
            }
            Tok::At => {
                let name = self.read_ident()?;
                match self.scope.resolve(&name) {
                    Some(d) => d.value.clone(),
                    None    => return self.err(format!(
                        "undefined variable '@{}'", name)),
                }
            }
            _ => return Err(ParseError {
                msg: "expected value".into(),
                line: val_tok.line, col: val_tok.col,
            }),
        };

        // Optional access modifiers `[pub]`, `[priv]`, `[read]`.
        let mut access = VarAccess::Public;
        if matches!(self.peek().kind, Tok::LBracket) {
            self.bump();
            loop {
                if matches!(self.peek().kind, Tok::RBracket) { self.bump(); break; }
                if matches!(self.peek().kind, Tok::Comma)    { self.bump(); continue; }
                let a = self.read_ident()?;
                access = match a.as_str() {
                    "pub"  | "public"    => VarAccess::Public,
                    "priv" | "private"   => VarAccess::Private,
                    "read" | "readonly"  => VarAccess::ReadOnly,
                    other => return self.err(format!(
                        "unknown access modifier '{}'", other)),
                };
            }
        }

        // Static type check.
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

    // --------------- sections ---------------

    fn parse_section(&mut self) -> Result<Section, ParseError> {
        let name = self.read_string()?;
        self.expect_ident("at")?;
        let at = self.read_number_resolved()?;
        self.expect_lbrace()?;
        self.scope.push();
        let body = self.parse_stmt_block()?;
        self.scope.pop();
        Ok(Section { name, at, body })
    }

    /// Parse statements up to and including the closing `}`.
    fn parse_stmt_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut body = Vec::new();
        loop {
            match self.peek().kind.clone() {
                Tok::RBrace => { self.bump(); break; }
                Tok::Comma  => { self.bump(); }
                Tok::Ident(ref s) if s == "emit" => {
                    self.bump();
                    body.push(Stmt::Emit(self.parse_obstacle()?));
                }
                Tok::Ident(ref s) if s == "wait" => {
                    self.bump();
                    let n = self.read_number_resolved()?;
                    if !(1.0..=64.0).contains(&n) {
                        return self.err(format!("wait out of range: {}", n));
                    }
                    body.push(Stmt::Wait(n as u32));
                }
                Tok::Ident(ref s) if s == "trigger" => {
                    self.bump();
                    body.push(Stmt::Trigger(self.parse_trigger()?));
                }
                Tok::Ident(ref s) if s == "repeat" => {
                    self.bump();
                    let n = self.read_number_resolved()?;
                    if !(1.0..=64.0).contains(&n) {
                        return self.err(format!("repeat count out of range: {}", n));
                    }
                    self.expect_lbrace()?;
                    self.scope.push();
                    let inner = self.parse_stmt_block()?;
                    self.scope.pop();
                    body.push(Stmt::Repeat { count: n as u32, body: inner });
                }
                Tok::Ident(ref s) if s == "local" => {
                    self.bump();
                    let decls = self.parse_local_block()?;
                    body.push(Stmt::LocalVars(decls));
                }
                other => return self.err(format!(
                    "unexpected {:?} in statement block", other)),
            }
        }
        Ok(body)
    }

    // --------------- obstacles ---------------

    fn parse_obstacle(&mut self) -> Result<ObstacleSpec, ParseError> {
        let name = self.read_ident()?;
        let fields = if matches!(self.peek().kind, Tok::LBrace) {
            self.parse_field_map()?
        } else {
            Fields::default()
        };

        match name.as_str() {
            "bar" => {
                let t = fields.get_number("thickness").unwrap_or(1.0).clamp(0.5, 2.5);
                Ok(ObstacleSpec::Bar { thickness_mult: t })
            }
            "doubleBar" | "double_bar" => {
                let spacing = fields.get_number("spacing").unwrap_or(2.0)
                    .clamp(1.0, 6.0) as u32;
                let t = fields.get_number("thickness").unwrap_or(1.0).clamp(0.5, 2.5);
                Ok(ObstacleSpec::DoubleBar { spacing, thickness_mult: t })
            }
            "spiral" => {
                let dir = dir_from(&fields);
                let t = fields.get_number("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                let loops = fields.get_number("loops").unwrap_or(2.0)
                    .clamp(1.0, 6.0) as u32;
                Ok(ObstacleSpec::Spiral { dir, thickness_mult: t, loops })
            }
            "alternate" => {
                let parity = match fields.get_ident("parity").as_deref() {
                    Some("odd") => Parity::Odd,
                    _           => Parity::Even,
                };
                let t = fields.get_number("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                Ok(ObstacleSpec::Alternate { parity, thickness_mult: t })
            }
            "pinwheel" => {
                let spokes = fields.get_number("spokes").unwrap_or(3.0)
                    .clamp(1.0, 6.0) as u32;
                let dir = dir_from(&fields);
                Ok(ObstacleSpec::Pinwheel { spokes, dir })
            }
            "rain" => {
                let count = fields.get_number("count").unwrap_or(5.0)
                    .clamp(2.0, 12.0) as u32;
                let t = fields.get_number("thickness").unwrap_or(0.85).clamp(0.5, 1.5);
                Ok(ObstacleSpec::Rain { count, thickness_mult: t })
            }
            "rainbow" => {
                let dir = dir_from(&fields);
                Ok(ObstacleSpec::Rainbow { dir })
            }
            "ladder" => {
                let rungs = fields.get_number("rungs").unwrap_or(6.0)
                    .clamp(3.0, 16.0) as u32;
                Ok(ObstacleSpec::Ladder { rungs })
            }
            "tunnel" => {
                let length = fields.get_number("length").unwrap_or(1.2)
                    .clamp(0.4, 2.5);
                let lanes = fields.get_number("lanes").unwrap_or(3.0)
                    .clamp(1.0, 4.0) as u32;
                Ok(ObstacleSpec::Tunnel { length, lanes })
            }
            "pot" => {
                let layers = fields.get_number("layers").unwrap_or(4.0)
                    .clamp(2.0, 8.0) as u32;
                Ok(ObstacleSpec::Pot { layers })
            }
            "staircase" => {
                let dir = dir_from(&fields);
                let steps = fields.get_number("steps").unwrap_or(6.0)
                    .clamp(3.0, 24.0) as u32;
                let t = fields.get_number("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                Ok(ObstacleSpec::Staircase { dir, steps, thickness_mult: t })
            }
            "corridor" => {
                let dir = dir_from(&fields);
                let length = fields.get_number("length").unwrap_or(1.4)
                    .clamp(0.6, 3.0);
                let turns = fields.get_number("turns").unwrap_or(3.0)
                    .clamp(1.0, 8.0) as u32;
                Ok(ObstacleSpec::Corridor { length, turns, dir })
            }
            "cubes" => {
                let layers = fields.get_number("layers").unwrap_or(4.0)
                    .clamp(2.0, 8.0) as u32;
                let dir = dir_from(&fields);
                Ok(ObstacleSpec::Cubes { layers, dir })
            }
            "custom" => {
                let mask_str = fields.get_string("mask").ok_or_else(|| {
                    self.err_owned("custom obstacle requires mask = \"...\"")
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
                let t = fields.get_number("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                Ok(ObstacleSpec::Custom { mask, thickness_mult: t })
            }
            "formula" | "custom_formula" => {
                let src = fields.get_string("formula").ok_or_else(|| {
                    self.err_owned("formula obstacle requires formula = \"...\"")
                })?;
                let steps = fields.get_number("steps").unwrap_or(6.0)
                    .clamp(1.0, 32.0) as u32;
                let t = fields.get_number("thickness").unwrap_or(1.0).clamp(0.5, 2.0);
                let program = formula::parse(&src).map_err(|m| self.err_owned(m))?;
                let default_sides = 6;
                let seed = fields.get_number("seed").unwrap_or(0.0) as u32;
                formula::validate_pathable(&program, steps, default_sides, seed)
                    .map_err(|m| self.err_owned(m))?;
                Ok(ObstacleSpec::CustomFormula {
                    formula: Formula { source: src, program },
                    steps,
                    thickness_mult: t,
                })
            }
            other => self.err(format!("unknown obstacle '{}'", other)),
        }
    }

    // --------------- triggers ---------------

    fn parse_trigger(&mut self) -> Result<TriggerSpec, ParseError> {
        let name = self.read_ident()?;
        let has_body = matches!(self.peek().kind, Tok::LBrace);
        let fields = if has_body { self.parse_field_map()? }
                     else        { Fields::default() };

        match name.as_str() {
            "flip"  => Ok(TriggerSpec::Flip),
            "pulse" => Ok(TriggerSpec::Pulse),
            "tilt"  => {
                let deg = if has_body {
                    fields.get_number("angle").unwrap_or(0.0)
                } else {
                    self.read_number_resolved()?
                };
                Ok(TriggerSpec::Tilt(deg.clamp(-2.5, 2.5)))
            }
            "speedMult" => {
                let m = if has_body {
                    fields.get_number("factor").unwrap_or(1.0)
                } else {
                    self.read_number_resolved()?
                };
                Ok(TriggerSpec::SpeedMult(m.clamp(0.5, 2.0)))
            }
            "hueShift" => {
                let r = if has_body {
                    fields.get_number("rate").unwrap_or(0.0)
                } else {
                    self.read_number_resolved()?
                };
                Ok(TriggerSpec::HueShift(r.clamp(-2.0, 2.0)))
            }
            "speedwarp" => {
                let walls    = fields.get_number("walls")      .unwrap_or(0.0);
                let rotation = fields.get_number("rotation")   .unwrap_or(0.0);
                let cursor   = fields.get_number("cursor")     .unwrap_or(0.0);
                let music    = fields.get_number("musicScale")
                    .or_else(|| fields.get_number("music_scale"))
                    .unwrap_or(0.0);
                let duration = fields.get_number("duration")   .unwrap_or(3.0)
                    .clamp(0.1, 20.0);
                Ok(TriggerSpec::SpeedWarp {
                    walls, rotation, cursor, music_scale: music, duration,
                })
            }
            "glitch" => {
                let strength = fields.get_number("strength").unwrap_or(0.5)
                    .clamp(0.0, 1.0);
                let duration = fields.get_number("duration").unwrap_or(0.8)
                    .clamp(0.05, 10.0);
                Ok(TriggerSpec::Glitch { strength, duration })
            }
            "shake" => {
                let strength = fields.get_number("strength").unwrap_or(0.5)
                    .clamp(0.0, 1.5);
                let duration = fields.get_number("duration").unwrap_or(0.6)
                    .clamp(0.05, 10.0);
                Ok(TriggerSpec::Shake { strength, duration })
            }
            "zoom" => {
                let target = fields.get_number("target").unwrap_or(1.0)
                    .clamp(0.25, 3.0);
                let anim = parse_anim(fields.get_ident("anim").as_deref());
                let duration = fields.get_number("duration").unwrap_or(1.5)
                    .clamp(0.05, 10.0);
                Ok(TriggerSpec::Zoom { target, anim, duration })
            }
            "invert" => {
                let duration = fields.get_number("duration").unwrap_or(3.0)
                    .clamp(0.05, 30.0);
                Ok(TriggerSpec::Invert { duration })
            }
            "strobe" => {
                let rate = fields.get_number("rate").unwrap_or(6.0)
                    .clamp(0.0, 40.0);
                let duration = fields.get_number("duration").unwrap_or(0.8)
                    .clamp(0.05, 10.0);
                Ok(TriggerSpec::Strobe { rate, duration })
            }
            other => self.err(format!("unknown trigger '{}'", other)),
        }
    }

    // --------------- field map ---------------

    fn parse_field_map(&mut self) -> Result<Fields, ParseError> {
        self.expect_lbrace()?;
        let mut f = Fields::default();
        loop {
            match self.peek().kind.clone() {
                Tok::RBrace => { self.bump(); break; }
                Tok::Comma  => { self.bump(); }
                _ => {
                    let name = self.read_ident()?;
                    self.eat_assign();
                    let v = self.read_field_value()?;
                    f.set(name, v);
                }
            }
        }
        Ok(f)
    }

    fn read_field_value(&mut self) -> Result<FieldValue, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n)      => Ok(FieldValue::Num(n)),
            Tok::Str(s)      => Ok(FieldValue::Str(s)),
            Tok::Ident(s)    => Ok(FieldValue::Ident(s)),
            Tok::At => {
                let name = self.read_ident()?;
                let d = self.scope.resolve(&name).ok_or_else(|| {
                    self.err_owned(format!("undefined variable '@{}'", name))
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

    // --------------- number / string with @var resolution ---------------

    fn read_number_resolved(&mut self) -> Result<f32, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Num(n) => Ok(n),
            Tok::At => {
                let name = self.read_ident()?;
                let d = self.scope.resolve(&name).ok_or_else(|| {
                    self.err_owned(format!("undefined variable '@{}'", name))
                })?;
                match &d.value {
                    VarValue::Num(n)  => Ok(*n),
                    VarValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
                    _ => Err(ParseError {
                        msg: format!("variable '@{}' is not numeric", name),
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

    fn read_string_resolved(&mut self) -> Result<String, ParseError> {
        let t = self.bump();
        match t.kind {
            Tok::Str(s) => Ok(s),
            Tok::At => {
                let name = self.read_ident()?;
                let d = self.scope.resolve(&name).ok_or_else(|| {
                    self.err_owned(format!("undefined variable '@{}'", name))
                })?;
                match &d.value {
                    VarValue::Str(s)   => Ok(s.clone()),
                    VarValue::Ident(s) => Ok(s.clone()),
                    _ => Err(ParseError {
                        msg: format!("variable '@{}' is not string-like", name),
                        line: t.line, col: t.col,
                    }),
                }
            }
            _ => Err(ParseError {
                msg: "expected string".into(),
                line: t.line, col: t.col,
            }),
        }
    }
}

// --------------- helpers ---------------

fn parse_timestamp_directive(
    name: &str, kv: &[(String, f32)],
) -> Result<TimestampFormat, String> {
    let prefix = "timestamp_format_use_";
    let tag = name.strip_prefix(prefix).ok_or_else(|| format!(
        "unknown directive '#[{}]'", name))?;
    Ok(match tag {
        "tracklength"       => TimestampFormat::TrackLength,
        "relative"          => TimestampFormat::Relative,
        "beats" | "bars"    => {
            let total = kv.iter().find(|(k, _)| k == "count")
                .map(|(_, v)| *v as u32).unwrap_or(64).max(1);
            TimestampFormat::Beats { total }
        }
        other => TimestampFormat::Named(other.to_string()),
    })
}

fn parse_anim(s: Option<&str>) -> Anim {
    match s {
        Some("linear")     | None             => Anim::Linear,
        Some("ease_in")    | Some("easeIn")   => Anim::EaseIn,
        Some("ease_out")   | Some("easeOut")  => Anim::EaseOut,
        Some("ease_in_out")| Some("easeInOut")=> Anim::EaseInOut,
        Some("bounce")                         => Anim::Bounce,
        Some(_) => Anim::Linear,
    }
}

fn dir_from(fields: &Fields) -> SpinDir {
    match fields.get_ident("dir").as_deref() {
        Some("ccw") => SpinDir::Ccw,
        _           => SpinDir::Cw,
    }
}

#[derive(Default, Debug)]
struct Fields { entries: Vec<(String, FieldValue)> }

#[derive(Clone, Debug)]
enum FieldValue { Num(f32), Str(String), Ident(String) }

impl Fields {
    fn set(&mut self, k: String, v: FieldValue) {
        if let Some(e) = self.entries.iter_mut().find(|(n, _)| n == &k) { e.1 = v; }
        else { self.entries.push((k, v)); }
    }
    fn get(&self, k: &str) -> Option<&FieldValue> {
        self.entries.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
    fn get_number(&self, k: &str) -> Option<f32> {
        match self.get(k) { Some(FieldValue::Num(n)) => Some(*n), _ => None }
    }
    fn get_ident(&self, k: &str) -> Option<String> {
        match self.get(k) { Some(FieldValue::Ident(s)) => Some(s.clone()), _ => None }
    }
    fn get_string(&self, k: &str) -> Option<String> {
        match self.get(k) { Some(FieldValue::Str(s)) => Some(s.clone()), _ => None }
    }
}