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
use crate::dsl::ast::ShaderDecl;

use crate::dsl::rules::{
    AbilityKind, AbilityRule, CursorRule, InputRule, RuleCategory,
    RuleSet, ScoreRule, SurvivalRule, VisionRule,
};

/// Serialize an in-memory `LevelAst` back into .rlf source text.
///
/// Output always uses the v3 dialect (`#[use_v3]`). The v3
/// parser is a strict superset of v2 and v1, so a v3-serialized
/// level can be edited in tools that expect the older syntax
/// for everything except the new `fn`, `pattern`,
/// `trigger_stack`, and meta statements. The editor itself
/// never emits those through this function yet; it serializes
/// the runtime AST which only carries plain emits, triggers,
/// waits, repeats, and rule ops.
///
/// The function name is kept as `serialize_v2` for API
/// stability across the ongoing refactor. A later pass may
/// rename it.
pub fn serialize_v2(ast: &LevelAst) -> String {
    let mut s = String::new();
    s.push_str("#[use_v3]\n");

    // Persist timestamp format as a directive so the file
    // round trips through parse → serialize without drift.
    // TrackLength is especially important: the editor defaults
    // new drafts to it so authors work in real seconds, and
    // losing that on save would silently turn every `at` into
    // a 0..1 fraction on next load.
    match &ast.timestamp_format {
        TimestampFormat::Relative => {}
        TimestampFormat::TrackLength =>
            s.push_str("#[timestamp_format_use_tracklength]\n"),
        TimestampFormat::Beats { total } =>
            writeln!(s, "#[timestamp_format_use_beats count = {}]", total).unwrap(),
        TimestampFormat::Named(name) =>
            writeln!(s, "#[timestamp_format_use_{}]", name).unwrap(),
    }

    // `#[startfrom]` debug helper. Only emitted when non-zero
    // so clean files stay clean on re-save.
    if ast.start_from_seconds > 0.0 {
        writeln!(s, "#[startfrom = {:.3}]", ast.start_from_seconds).unwrap();
    }

    // Debug directive. Only emitted when the author
    // actually set it, so clean levels stay clean.
    if ast.ignore_collisions {
        s.push_str("#[ignore_collisions]\n");
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
    // Seed is serialized in hex on round trip. Two reasons:
    //
    // 1. Hex matches the hand authored style of the stock
    //    levels, which use forms like `seed = 0x5EED1234`.
    //    Losing that on save and reload would break `git diff`
    //    style authoring workflows for no good reason.
    // 2. The parser stores literals as f32 and f32 cannot
    //    exactly represent u32 values above 2^24. Writing the
    //    seed as decimal forces a lossy conversion on every
    //    save. Writing it as hex keeps the textual form stable
    //    even when the underlying numeric value is off by a
    //    few low bits.
    writeln!(s, "        seed    = 0x{:08X}", g.seed).unwrap();
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
        for stmt in &sec.body { write_stmt(&mut s, stmt, 8, &ast.shaders); }
        s.push_str("    end\n\n");
    }

    // ---- shaders ----
    if !ast.shaders.is_empty() {
        for sh in &ast.shaders {
            writeln!(s, "    shader :{} = ~p\"{}\"",
                sh.name, esc(&sh.path)).unwrap();
        }
        s.push('\n');
    }

    s.push_str("end\n");
    s
}

fn write_stmt(s: &mut String, st: &Stmt, indent: usize,
              shaders: &[ShaderDecl]) {
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
            write_trigger(s, t, shaders);
            s.push('\n');
        }
        Stmt::Repeat { count, body } => {
            writeln!(s, "{}repeat {} do", pad, count).unwrap();
            for inner in body { write_stmt(s, inner, indent + 4, shaders); }
            writeln!(s, "{}end", pad).unwrap();
        }
        Stmt::LocalVars(decls) => {
            writeln!(s, "{}local do", pad).unwrap();
            for v in decls { write_var(s, v, indent + 4); }
            writeln!(s, "{}end", pad).unwrap();
        }
        Stmt::Rule(rs) => {
            write!(s, "{}rule ", pad).unwrap();
            write_rule_set(s, rs);
            s.push('\n');
        }
        Stmt::Revert(cat) => {
            writeln!(s, "{}revert {}", pad, category_keyword(cat)).unwrap();
        }
        Stmt::Push(cat) => {
            writeln!(s, "{}push {}", pad, category_keyword(cat)).unwrap();
        }
        Stmt::Pop(cat) => {
            writeln!(s, "{}pop {}", pad, category_keyword(cat)).unwrap();
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

/// Serialize one trigger to its .rlf textual form.
///
/// Every trigger emits its authored parameters, including the
/// new `duration` field on `Tilt` / `SpeedMult` / `HueShift`.
/// `SpeedWarp` emits only the axes that are `Some`, so a
/// round-trip through parse → edit → serialize never invents
/// axis overrides the author did not write.
fn write_trigger(s: &mut String, t: &TriggerSpec, shaders: &[ShaderDecl]) {
    match t {
        TriggerSpec::Flip  => write!(s, ":flip").unwrap(),
        TriggerSpec::Pulse => write!(s, ":pulse").unwrap(),
        TriggerSpec::Tilt { angle, pitch, yaw, duration } => {
            // Serialize every field the author set, omitting
            // axes left at zero so round-tripped files stay
            // visually close to the source. `angle` is always
            // emitted to keep the trigger block non-empty even
            // when only pitch/yaw were authored, which makes
            // the output unambiguous.
            let mut parts: Vec<String> = Vec::new();
            parts.push(format!(" angle = {:.3}", angle));
            if pitch.abs() > 1e-4 {
                parts.push(format!(" pitch = {:.3}", pitch));
            }
            if yaw.abs() > 1e-4 {
                parts.push(format!(" yaw = {:.3}", yaw));
            }
            if let Some(d) = duration {
                parts.push(format!(" duration = {:.3}", d));
            }
            write!(s, ":tilt {{").unwrap();
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
        TriggerSpec::SpeedMult { factor, duration } =>
            write!(s, ":speedMult {{ factor = {:.3}, duration = {:.3} }}",
                factor, duration).unwrap(),
        TriggerSpec::HueShift { rate, duration } =>
            write!(s, ":hueShift {{ rate = {:.3}, duration = {:.3} }}",
                rate, duration).unwrap(),
        TriggerSpec::SpeedWarp { walls, rotation, cursor, music_scale, duration } => {
            // Omit absent axes so round trip matches source.
            // Always emit `duration` since it has no sensible
            // "absent" form; the parser gives it a default but
            // the editor always carries a concrete value.
            write!(s, ":speedwarp {{").unwrap();
            let mut parts: Vec<String> = Vec::new();
            if let Some(v) = walls {
                parts.push(format!(" walls = {:.3}", v));
            }
            if let Some(v) = rotation {
                parts.push(format!(" rotation = {:.3}", v));
            }
            if let Some(v) = cursor {
                parts.push(format!(" cursor = {:.3}", v));
            }
            if let Some(v) = music_scale {
                parts.push(format!(" music = {:.3}", v));
            }
            parts.push(format!(" duration = {:.3}", duration));
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
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
        TriggerSpec::Spin { rate, duration } =>
            write!(s, ":spin {{ rate = {:.3}, duration = {:.3} }}",
                rate, duration).unwrap(),
        TriggerSpec::Bounce { amplitude, duration } =>
            write!(s, ":bounce {{ amplitude = {:.3}, duration = {:.3} }}",
                amplitude, duration).unwrap(),
        TriggerSpec::Freeze { duration } =>
            write!(s, ":freeze {{ duration = {:.3} }}", duration).unwrap(),
        TriggerSpec::ZoomPunch { strength, duration } =>
            write!(s, ":zoom_punch {{ strength = {:.3}, duration = {:.3} }}",
                strength, duration).unwrap(),
        TriggerSpec::InvertColors { duration } =>
            write!(s, ":invert_colors {{ duration = {:.3} }}", duration).unwrap(),
        TriggerSpec::Grayscale { strength, duration } =>
            write!(s, ":grayscale {{ strength = {:.3}, duration = {:.3} }}",
                strength, duration).unwrap(),
        TriggerSpec::Shockwave { strength, duration } =>
            write!(s, ":shockwave {{ strength = {:.3}, duration = {:.3} }}",
                strength, duration).unwrap(),
        TriggerSpec::Fog { near, far, duration } =>
            write!(s, ":fog {{ near = {:.3}, far = {:.3}, duration = {:.3} }}",
                near, far, duration).unwrap(),
        TriggerSpec::Outline { thickness, duration } =>
            write!(s, ":outline {{ thickness = {:.3}, duration = {:.3} }}",
                thickness, duration).unwrap(),
        TriggerSpec::Centerburst { strength, duration } =>
            write!(s, ":centerburst {{ strength = {:.3}, duration = {:.3} }}",
                strength, duration).unwrap(),
        TriggerSpec::Ringburst { count, duration } =>
            write!(s, ":ringburst {{ count = {}, duration = {:.3} }}",
                count, duration).unwrap(),
        TriggerSpec::Bassdrop { strength, duration } =>
            write!(s, ":bassdrop {{ strength = {:.3}, duration = {:.3} }}",
                strength, duration).unwrap(),
        TriggerSpec::PostShader { slot, p } => {
            let name = shaders.get(*slot as usize)
                .map(|s| s.name.as_str())
                .unwrap_or("UNKNOWN");
            write!(s,
                ":post_shader {{ shader = :{}, \
                 p0 = {:.3}, p1 = {:.3}, p2 = {:.3}, p3 = {:.3} }}",
                name, p[0], p[1], p[2], p[3]).unwrap();
        }
        TriggerSpec::PostShaderOff => {
            write!(s, ":post_shader_off").unwrap();
        }
        TriggerSpec::Morph { sides, duration } =>
            write!(s, ":morph {{ sides = {}, duration = {:.3} }}",
                sides, duration).unwrap(),
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

/// DSL keyword form of a rule category. Matches the identifier
/// the v3 parser accepts after `rule`, `revert`, `push`, `pop`.
fn category_keyword(cat: &RuleCategory) -> &'static str {
    match cat {
        RuleCategory::Ability  => "ability",
        RuleCategory::Vision   => "vision",
        RuleCategory::Cursor   => "cursor",
        RuleCategory::Survival => "survival",
        RuleCategory::Input    => "input",
        RuleCategory::Score    => "score",
        RuleCategory::All      => "all",
    }
}

/// Serialize a `RuleSet` as `<category> { field = value, ... }`.
/// Only fields that are `Some` in the source are emitted, which
/// lets a partial override round trip through the editor without
/// silently filling in defaults an author never asked for.
fn write_rule_set(s: &mut String, rs: &RuleSet) {
    match rs {
        RuleSet::Ability(r) => {
            write!(s, "ability {{").unwrap();
            let mut parts: Vec<String> = Vec::new();
            if let Some(k) = r.kind {
                parts.push(format!(" kind = :{}", k.to_atom()));
            }
            if let Some(v) = r.charges  { parts.push(format!(" charges = {}", v)); }
            if let Some(v) = r.recharge { parts.push(format!(" recharge = {:.3}", v)); }
            if let Some(v) = r.invuln   { parts.push(format!(" invuln = {:.3}", v)); }
            if let Some(v) = r.cooldown { parts.push(format!(" cooldown = {:.3}", v)); }
            if let Some(v) = r.slots_per_dash {
                parts.push(format!(" slots_per_dash = {}", v));
            }
            if let Some(v) = r.slowmo_factor {
                parts.push(format!(" slowmo_factor = {:.3}", v));
            }
            if let Some(v) = r.slowmo_cap {
                parts.push(format!(" slowmo_cap = {:.3}", v));
            }
            if let Some(v) = r.slowmo_recover {
                parts.push(format!(" slowmo_recover = {:.3}", v));
            }
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
        RuleSet::Vision(r) => {
            write!(s, "vision {{").unwrap();
            let mut parts: Vec<String> = Vec::new();
            if let Some(v) = r.range    { parts.push(format!(" range = {:.3}", v)); }
            if let Some(v) = r.fog_near { parts.push(format!(" fog_near = {:.3}", v)); }
            if let Some(v) = r.fog_far  { parts.push(format!(" fog_far = {:.3}", v)); }
            if let Some(v) = r.strobe   { parts.push(format!(" strobe = {}", v)); }
            if let Some(v) = r.strobe_rate {
                parts.push(format!(" strobe_rate = {:.3}", v));
            }
            if let Some(v) = r.blind_duration {
                parts.push(format!(" blind_duration = {:.3}", v));
            }
            if let Some(v) = r.blind_frequency {
                parts.push(format!(" blind_frequency = {:.3}", v));
            }
            if let Some(v) = r.hide_camera_indicator {
                parts.push(format!(" hide_camera_indicator = {}", v));
            }
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
        RuleSet::Cursor(r) => {
            write!(s, "cursor {{").unwrap();
            let mut parts: Vec<String> = Vec::new();
            if let Some(v) = r.speed_mult {
                parts.push(format!(" speed_mult = {:.3}", v));
            }
            if let Some(v) = r.width_mult {
                parts.push(format!(" width_mult = {:.3}", v));
            }
            if let Some(v) = r.count { parts.push(format!(" count = {}", v)); }
            if let Some(v) = r.angular_offset {
                parts.push(format!(" angular_offset = {:.3}", v));
            }
            if let Some(v) = r.centripetal_drift {
                parts.push(format!(" centripetal_drift = {:.3}", v));
            }
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
        RuleSet::Survival(r) => {
            write!(s, "survival {{").unwrap();
            let mut parts: Vec<String> = Vec::new();
            if let Some(v) = r.lives { parts.push(format!(" lives = {}", v)); }
            if let Some(v) = r.soft_death { parts.push(format!(" soft_death = {}", v)); }
            if let Some(v) = r.pushback_seconds {
                parts.push(format!(" pushback_seconds = {:.3}", v));
            }
            if let Some(v) = r.invuln_after_hit {
                parts.push(format!(" invuln_after_hit = {:.3}", v));
            }
            if let Some(v) = r.checkpoints_enabled {
                parts.push(format!(" checkpoints_enabled = {}", v));
            }
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
        RuleSet::Input(r) => {
            write!(s, "input {{").unwrap();
            let mut parts: Vec<String> = Vec::new();
            if let Some(v) = r.delay_ms { parts.push(format!(" delay_ms = {}", v)); }
            if let Some(v) = r.discrete { parts.push(format!(" discrete = {}", v)); }
            if let Some(v) = r.noise    { parts.push(format!(" noise = {:.3}", v)); }
            if let Some(v) = r.inverted { parts.push(format!(" inverted = {}", v)); }
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
        RuleSet::Score(r) => {
            write!(s, "score {{").unwrap();
            let mut parts: Vec<String> = Vec::new();
            if let Some(v) = r.multiplier {
                parts.push(format!(" multiplier = {:.3}", v));
            }
            if let Some(v) = r.close_call_bonus {
                parts.push(format!(" close_call_bonus = {}", v));
            }
            if let Some(v) = r.survival_per_second {
                parts.push(format!(" survival_per_second = {}", v));
            }
            s.push_str(&parts.join(","));
            s.push_str(" }");
        }
    }
}