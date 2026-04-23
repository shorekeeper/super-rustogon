//! Serialise a [`LevelAst`] back to v2 RLF source.
//!
//! Output is canonical and always opens with `#[use_v2]` so a
//! round-trip through `parse_level` lands on the v2 parser. The
//! formatter writes one field per line inside `do ... end`
//! blocks, uses atom syntax for enum-like values and sigils for
//! paths / masks / formulas, matching the dialect demonstrated
//! by the embedded `V2 DEMO` level.

use std::fmt::Write;

use crate::dsl::ast::*;
use crate::levels::difficulty::Tier;

/// Render an AST as a v2 RLF source string.
pub fn serialize_v2(ast: &LevelAst) -> String {
    let mut s = String::new();
    s.push_str("#[use_v2]\n");

    // Optional timestamp directive.
    match &ast.timestamp_format {
        TimestampFormat::Relative => {}
        TimestampFormat::TrackLength =>
            s.push_str("#[timestamp_format_use_tracklength]\n"),
        TimestampFormat::Beats { total } =>
            writeln!(s, "#[timestamp_format_use_beats count={}]", total).unwrap(),
        TimestampFormat::Named(name) =>
            writeln!(s, "#[timestamp_format_use_{}]", name).unwrap(),
    }
    s.push('\n');

    writeln!(s, "level \"{}\" do", esc(&ast.meta.name)).unwrap();

    // ---- meta ----
    s.push_str("    meta do\n");
    let m = &ast.meta;
    if !m.subtitle.is_empty()    { writeln!(s, "        subtitle    = \"{}\"", esc(&m.subtitle)).unwrap(); }
    if !m.author.is_empty()      { writeln!(s, "        author      = \"{}\"", esc(&m.author)).unwrap(); }
    if !m.song.is_empty()        { writeln!(s, "        song        = \"{}\"", esc(&m.song)).unwrap(); }
    if  m.bpm > 0                { writeln!(s, "        bpm         = {}",      m.bpm).unwrap(); }
    if !m.music.is_empty()       { writeln!(s, "        music       = ~p\"{}\"", esc(&m.music)).unwrap(); }
    if !m.description.is_empty() { writeln!(s, "        description = \"{}\"", esc(&m.description)).unwrap(); }
    s.push_str("    end\n\n");

    // ---- palette ----
    s.push_str("    palette do\n");
    let p = &ast.palette;
    writeln!(s, "        bgA        = {}", rgb(p.bg_a)).unwrap();
    writeln!(s, "        bgB        = {}", rgb(p.bg_b)).unwrap();
    writeln!(s, "        centerFill = {}", rgb(p.center_fill)).unwrap();
    writeln!(s, "        centerRing = {}", rgb(p.center_ring)).unwrap();
    writeln!(s, "        wall       = {}", rgb(p.wall)).unwrap();
    writeln!(s, "        player     = {}", rgb(p.player)).unwrap();
    writeln!(s, "        accent     = {}", rgb(p.accent)).unwrap();
    s.push_str("    end\n\n");

    // ---- difficulty ----
    s.push_str("    difficulty do\n");
    writeln!(s, "        range = :{}..:{}",
        tier_kw(ast.difficulty.min_tier), tier_kw(ast.difficulty.max_tier)).unwrap();
    writeln!(s, "        base  = :{}", tier_kw(ast.difficulty.base_tier)).unwrap();
    s.push_str("    end\n\n");

    // ---- generation ----
    s.push_str("    generation do\n");
    let g = &ast.generation;
    writeln!(s, "        sides   = {}",     g.sides).unwrap();
    writeln!(s, "        seed    = {}",     g.seed).unwrap();
    writeln!(s, "        speed   = {:.3}",  g.speed_mult).unwrap();
    writeln!(s, "        density = {:.3}",  g.density_mult).unwrap();
    if g.hue_speed.abs() > 1e-4 {
        writeln!(s, "        hueSpeed = {:.3}", g.hue_speed).unwrap();
    }
    s.push_str("    end\n\n");

    // ---- globals ----
    if !ast.globals.is_empty() {
        s.push_str("    global do\n");
        for v in &ast.globals { write_var(&mut s, v, 8); }
        s.push_str("    end\n\n");
    }

    // ---- sections ----
    for sec in &ast.sections {
        writeln!(s, "    section \"{}\" at {:.3} do", esc(&sec.name), sec.at).unwrap();
        for stmt in &sec.body { write_stmt(&mut s, stmt, 8); }
        s.push_str("    end\n\n");
    }

    s.push_str("end\n");
    s
}

fn write_stmt(s: &mut String, st: &Stmt, indent: usize) {
    let pad = " ".repeat(indent);
    match st {
        Stmt::Wait(n) => writeln!(s, "{}wait {}", pad, n).unwrap(),
        Stmt::Emit(spec) => {
            write!(s, "{}emit ", pad).unwrap();
            write_obstacle(s, spec);
            s.push('\n');
        }
        Stmt::Trigger(t) => {
            write!(s, "{}trigger ", pad).unwrap();
            write_trigger(s, t);
            s.push('\n');
        }
        Stmt::Repeat { count, body } => {
            writeln!(s, "{}repeat {} do", pad, count).unwrap();
            for inner in body { write_stmt(s, inner, indent + 4); }
            writeln!(s, "{}end", pad).unwrap();
        }
        Stmt::LocalVars(decls) => {
            writeln!(s, "{}local do", pad).unwrap();
            for v in decls { write_var(s, v, indent + 4); }
            writeln!(s, "{}end", pad).unwrap();
        }
    }
}

fn write_var(s: &mut String, v: &VarDecl, indent: usize) {
    let pad = " ".repeat(indent);
    let ty = match v.ty {
        VarType::Int    => "i32",
        VarType::Float  => "f32",
        VarType::String => "string",
        VarType::Ident  => "ident",
        VarType::Bool   => "bool",
    };
    write!(s, "{}var {} :: {} = ", pad, v.name, ty).unwrap();
    write_var_value(s, &v.value);
    let mods = match v.access {
        VarAccess::Public   => " [pub]",
        VarAccess::Private  => " [priv]",
        VarAccess::ReadOnly => " [read]",
    };
    s.push_str(mods);
    s.push('\n');
}

fn write_var_value(s: &mut String, v: &VarValue) {
    match v {
        VarValue::Num(n)   => write!(s, "{}", n).unwrap(),
        VarValue::Str(x)   => write!(s, "\"{}\"", esc(x)).unwrap(),
        VarValue::Ident(x) => write!(s, ":{}", x).unwrap(),
        VarValue::Bool(b)  => write!(s, "{}", b).unwrap(),
    }
}

fn write_obstacle(s: &mut String, spec: &ObstacleSpec) {
    match spec {
        ObstacleSpec::Bar { thickness_mult } =>
            write!(s, ":bar {{ thickness = {:.3} }}", thickness_mult).unwrap(),
        ObstacleSpec::DoubleBar { spacing, thickness_mult } =>
            write!(s, ":doubleBar {{ spacing = {}, thickness = {:.3} }}",
                spacing, thickness_mult).unwrap(),
        ObstacleSpec::Spiral { dir, thickness_mult, loops } =>
            write!(s, ":spiral {{ dir = :{}, loops = {}, thickness = {:.3} }}",
                dir_str(*dir), loops, thickness_mult).unwrap(),
        ObstacleSpec::Alternate { parity, thickness_mult } => {
            let p = match parity { Parity::Even => "even", Parity::Odd => "odd" };
            write!(s, ":alternate {{ parity = :{}, thickness = {:.3} }}",
                p, thickness_mult).unwrap();
        }
        ObstacleSpec::Pinwheel { spokes, dir } =>
            write!(s, ":pinwheel {{ spokes = {}, dir = :{} }}", spokes, dir_str(*dir)).unwrap(),
        ObstacleSpec::Rain { count, thickness_mult } =>
            write!(s, ":rain {{ count = {}, thickness = {:.3} }}",
                count, thickness_mult).unwrap(),
        ObstacleSpec::Custom { mask, thickness_mult } => {
            let bits: String = mask.iter().map(|b| if *b {'1'} else {'0'}).collect();
            write!(s, ":custom {{ mask = ~m\"{}\", thickness = {:.3} }}",
                bits, thickness_mult).unwrap();
        }
        ObstacleSpec::Rainbow { dir } =>
            write!(s, ":rainbow {{ dir = :{} }}", dir_str(*dir)).unwrap(),
        ObstacleSpec::Ladder { rungs } =>
            write!(s, ":ladder {{ rungs = {} }}", rungs).unwrap(),
        ObstacleSpec::Tunnel { length, lanes } =>
            write!(s, ":tunnel {{ length = {:.3}, lanes = {} }}", length, lanes).unwrap(),
        ObstacleSpec::Pot { layers } =>
            write!(s, ":pot {{ layers = {} }}", layers).unwrap(),
        ObstacleSpec::Staircase { dir, steps, thickness_mult } =>
            write!(s, ":staircase {{ dir = :{}, steps = {}, thickness = {:.3} }}",
                dir_str(*dir), steps, thickness_mult).unwrap(),
        ObstacleSpec::Corridor { length, turns, dir } =>
            write!(s, ":corridor {{ length = {:.3}, turns = {}, dir = :{} }}",
                length, turns, dir_str(*dir)).unwrap(),
        ObstacleSpec::Cubes { layers, dir } =>
            write!(s, ":cubes {{ layers = {}, dir = :{} }}", layers, dir_str(*dir)).unwrap(),
        ObstacleSpec::CustomFormula { formula, steps, thickness_mult } =>
            write!(s, ":formula {{ formula = ~f\"{}\", steps = {}, thickness = {:.3} }}",
                esc(&formula.source), steps, thickness_mult).unwrap(),
    }
}

fn write_trigger(s: &mut String, t: &TriggerSpec) {
    match t {
        TriggerSpec::Flip  => write!(s, ":flip").unwrap(),
        TriggerSpec::Pulse => write!(s, ":pulse").unwrap(),
        TriggerSpec::Tilt(d) => write!(s, ":tilt {{ angle = {:.3} }}", d).unwrap(),
        TriggerSpec::SpeedMult(m) => write!(s, ":speedMult {{ factor = {:.3} }}", m).unwrap(),
        TriggerSpec::HueShift(r)  => write!(s, ":hueShift {{ rate = {:.3} }}", r).unwrap(),
        TriggerSpec::SpeedWarp { walls, rotation, cursor, music_scale, duration } =>
            write!(s, ":speedwarp {{ walls = {:.3}, rotation = {:.3}, cursor = {:.3}, music = {:.3}, duration = {:.3} }}",
                walls, rotation, cursor, music_scale, duration).unwrap(),
        TriggerSpec::Glitch { strength, duration } =>
            write!(s, ":glitch {{ strength = {:.3}, duration = {:.3} }}",
                strength, duration).unwrap(),
        TriggerSpec::Shake { strength, duration } =>
            write!(s, ":shake {{ strength = {:.3}, duration = {:.3} }}",
                strength, duration).unwrap(),
        TriggerSpec::Zoom { target, anim, duration } =>
            write!(s, ":zoom {{ target = {:.3}, anim = :{}, duration = {:.3} }}",
                target, anim_str(*anim), duration).unwrap(),
        TriggerSpec::Invert { duration } =>
            write!(s, ":invert {{ duration = {:.3} }}", duration).unwrap(),
        TriggerSpec::Strobe { rate, duration } =>
            write!(s, ":strobe {{ rate = {:.3}, duration = {:.3} }}",
                rate, duration).unwrap(),
    }
}

fn rgb(c: [f32; 3]) -> String {
    format!("rgb({:.3}, {:.3}, {:.3})", c[0], c[1], c[2])
}

fn dir_str(d: SpinDir) -> &'static str {
    match d { SpinDir::Cw => "cw", SpinDir::Ccw => "ccw" }
}

fn anim_str(a: Anim) -> &'static str {
    match a {
        Anim::Linear     => "linear",
        Anim::EaseIn     => "ease_in",
        Anim::EaseOut    => "ease_out",
        Anim::EaseInOut  => "ease_in_out",
        Anim::Bounce     => "bounce",
    }
}

fn tier_kw(t: Tier) -> String {
    match t {
        Tier::Rookie  => "Rookie".into(),
        Tier::Casual  => "Casual".into(),
        Tier::Adept   => "Adept".into(),
        Tier::Skilled => "Skilled".into(),
        Tier::Expert  => "Expert".into(),
        Tier::ExpertPlus { plus } => {
            if plus <= 1 { "ExpertPlus".into() }
            else         { format!("ExpertPlus{}", plus) }
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}