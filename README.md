# Super Rustogon

A small, fast Rust clone of Super Hexagon with a rich level scripting language and an embedded editor. Single binary, Vulkan rendering, Win32 windowing, no external runtime dependencies beyond a Vulkan driver and (optionally) `glslangValidator` for user shaders.

## Building and running

```
cargo run --release
```

Levels live in `assets/levels/` (stock) and `assets/customlevels/` (yours). Music tracks are QOA files in `assets/music/` or `assets/customlevels/songs/`. Drop a `.rlf` file in the custom folder and the game picks it up next launch.

## Controls

- Left and Right arrows turn the cursor
- Shift triggers the equipped ability
- Space restarts after death
- Enter confirms in menus
- Escape goes back or saves

## What ships

- Six stock levels plus an effects showcase
- Embedded editor with graph canvas, timeline, inspector, undo/redo
- Audio onset detection driving beat-reactive visuals
- Post process pipeline with bloom, chromatic aberration, vignette, scanlines, film grain, fog, outline, shockwave, color invert, grayscale, glitch, strobe
- User shader sandbox with a tiny safe DSL that compiles to SPIR-V through glslangValidator
- Configurable abilities (dash, shield, slowmo)
- Accessibility options including colorblind palettes, motion reduction, high contrast, hitbox overlay
- Per channel audio mix (master, music, sfx)

---

# Rustogon Level Format (RLF)

Levels are written in a small declarative language saved as `.rlf` files. The format has three layers stacked on top of each other:

1. A flat declaration of meta, palette, difficulty, generation parameters
2. A timed sequence of sections, each containing emit, wait, trigger, and rule statements
3. An optional meta layer with functions, patterns, hooks, and compile time control flow

The current parser is v3. Older v1 and v2 dialects have been retired. A file may opt in explicitly with `#[use_v3]` at the top, but the directive is now only documentation; absence is treated the same way.

## Minimal level

```
#[use_v3]

level "HELLO" do
    meta do bpm = 130 end
    palette do bgA = rgb(0.1, 0.05, 0.15) end
    difficulty do range = :Rookie..:Expert, base = :Casual end
    generation do sides = 6, seed = 0x1 end

    section "intro" at 0.0 do
        emit bar
        wait 2
    end
end
```

Save it as `assets/customlevels/hello.rlf`. Done.

## File structure

A file is zero or more directives followed by exactly one `level` block:

```
#[directive_one]
#[directive_two count = 32]

level "Display Name" do
    meta        do ... end
    palette     do ... end
    difficulty  do ... end
    generation  do ... end
    global      do ... end
    section "intro" at ... do ... end
    section "drop"  at ... do ... end
end
```

Block order is free. Missing blocks fall back to defaults. Both `do ... end` and `{ ... }` work and may be mixed.

## Lexical bits

Whitespace, tabs, newlines, and commas are interchangeable separators. The `=` between a field name and value is optional. Three comment styles, all to end of line:

```
// C style
-- Haskell style
#  Shell style (unless followed by [, which begins a directive)
```

Numbers can be decimal or hex (`0xCAFE`). Strings are double quoted with `\"` and `\\` escapes. Atoms look like `:cw`, `:even`, `:ease_out`. Bare identifiers also fall through to atom or variable lookup, so `dir = cw` works the same as `dir = :cw` until you bind a variable named `cw`.

Sigils carry typed strings:

| Sigil    | Meaning              | Example                                     |
|----------|----------------------|---------------------------------------------|
| `~p"..."` | Asset path           | `music = ~p"assets/music/song.qoa"`         |
| `~m"..."` | Bit mask, 0 and 1   | `mask = ~m"110010"`                         |
| `~f"..."` | Custom formula       | `formula = ~f"step % 2 == 0"`               |
| `~t"..."` | Time, S or M:SS or H:MM:SS | `at = ~t"1:30"`                       |

## Directives

Appear before `level`, write as `#[name]` or `#[name k = v, k = v]`. Unknown directives are ignored.

| Directive | Effect |
|-----------|--------|
| `#[use_v3]` | Self documenting v3 marker |
| `#[timestamp_format_use_relative]` | Default. `at` is a 0..1 fraction of song |
| `#[timestamp_format_use_tracklength]` | `at` is absolute seconds. Pair with `~t` |
| `#[timestamp_format_use_beats count = N]` | Divide song into N buckets, `at` is bucket index |
| `#[startfrom = ~t"M:SS"]` | Debug helper. Seek the music to this offset on level entry |
| `#[ignore_collisions]` | Debug helper. Player cannot die. Visual feedback still fires |
| `#[ability { ... }]` etc. | File level rule baseline. See Rules section |

## Blocks

### meta

```
meta do
    name        = "EXAMPLE"
    subtitle    = "DEMO"
    author      = "YOU"
    song        = "TRACK NAME"
    bpm         = 130
    music       = ~p"assets/music/track.qoa"
    description = "Three lines about the level."
end
```

If `name` is omitted, the string from `level "..."` is used. BPM range is 40 to 300.

### palette

Seven colors, each `rgb(r, g, b)` with components in 0..1. Both camelCase and snake_case are accepted.

```
palette do
    bgA        = rgb(0.10, 0.04, 0.16)
    bgB        = rgb(0.05, 0.02, 0.10)
    centerFill = rgb(0.05, 0.02, 0.08)
    centerRing = rgb(1.00, 0.40, 0.70)
    wall       = rgb(1.00, 0.40, 0.70)
    player     = rgb(1.00, 0.95, 1.00)
    accent     = rgb(1.00, 0.40, 0.70)
end
```

### difficulty

```
difficulty do
    range = :Rookie..:ExpertPlus2
    base  = :Adept
end
```

Tiers in rank order:

```
:Rookie  :Casual  :Adept  :Skilled  :Expert
:ExpertPlus  :ExpertPlus1  :ExpertPlus2  :ExpertPlus3  :ExpertPlus4
```

`:ExpertPlus` and `:ExpertPlus1` are the same. You can also use separate `min`, `max`, `base` fields instead of `range`.

### generation

| Field | Default | Range | Meaning |
|-------|---------|-------|---------|
| `sides` | 6 | 3..12 | Slot count of the playfield |
| `seed` | varies | u32 | Generator seed, hex allowed |
| `speed` | 1.0 | 0.25..3.0 | Wall speed multiplier |
| `density` | 1.0 | 0.25..3.0 | Spawn density multiplier |
| `hueSpeed` | 0.0 | -4.0..4.0 | Continuous palette rotation |

### global

File level variable declarations visible everywhere.

```
global do
    var base_thick :: f32 = 1.0       [pub]
    var dash_dir   :: ident = :cw     [read]
end
```

Types: `f32`, `i32`, `string`, `ident`, `bool`. Access modifiers `pub`, `priv`, `read` are documentation only for now.

### section

```
section "drop" at 0.50 do
    trigger :flip
    emit staircase { dir = :cw, steps = 8 }
    wait 4
end
```

Sections are sorted by `at` after parsing, so source order does not matter. The `at` value is interpreted by the active timestamp format.

## Statements

### emit

Spawn one obstacle.

```
emit bar
emit spiral { dir = :cw, loops = 3 }
```

### wait

Pause for `n` beats, where a beat is `60 / bpm` seconds. Range 1 to 64.

```
wait 4
```

### trigger

Apply an effect. Single or piped chain at the same scheduling anchor.

```
trigger :flip
trigger :speedwarp { walls = 1.4, duration = 4.0 }
trigger :flip |> :shake { strength = 0.6, duration = 0.5 }
```

### repeat

Inline a body N times.

```
repeat 4 do
    emit ladder { rungs = 6 }
    wait 2
end
```

### rule, revert, push, pop

Adjust the active rule state. See the Rules section.

```
rule ability { kind = :shield, charges = 3 }
revert ability
push cursor
rule cursor { count = 2 }
pop cursor
```

## Obstacles

Every obstacle is guaranteed to leave at least one survivable gap. Out of range numeric arguments are clamped silently.

| Name | Fields |
|------|--------|
| `bar` | `thickness` |
| `doubleBar` | `spacing`, `thickness` |
| `spiral` | `dir`, `loops`, `thickness` |
| `alternate` | `parity`, `thickness` |
| `pinwheel` | `spokes`, `dir` |
| `rain` | `count`, `thickness` |
| `rainbow` | `dir` |
| `ladder` | `rungs` |
| `tunnel` | `length`, `lanes` |
| `pot` | `layers` |
| `staircase` | `dir`, `steps`, `thickness` |
| `corridor` | `length`, `turns`, `dir` |
| `cubes` | `layers`, `dir` |
| `custom` | `mask`, `thickness` |
| `formula` | `formula`, `steps`, `thickness`, `seed` |

Examples:

```
emit doubleBar { spacing = 2, thickness = 1.4 }
emit staircase { dir = :cw, steps = 12 }
emit tunnel    { length = 1.6, lanes = 3 }
emit custom    { mask = ~m"110010" }
emit formula   { formula = ~f"step % 2 == 0 && slot != phase", steps = 8 }
```

`dir` accepts `:cw` (default) and `:ccw`. `parity` accepts `:even` and `:odd`.

## Triggers

Triggers do not spawn walls. They modify camera, speed, post process, input, or audio state.

### Stateless

```
trigger :flip      // reverse rotation
trigger :pulse     // brief background pulse
```

### Camera and motion

| Trigger | Fields | Notes |
|---------|--------|-------|
| `:tilt` | `angle`, `pitch`, `yaw`, `duration` | Degrees, clamped to 30. Without duration the tilt is sticky |
| `:speedMult` | `factor`, `duration` | Wall speed multiplier |
| `:hueShift` | `rate`, `duration` | Radians per second |
| `:speedwarp` | `walls`, `rotation`, `cursor`, `music`, `duration` | Per axis multipliers. Each axis is optional and stacks independently across piped triggers |
| `:zoom` | `target`, `anim`, `duration` | Animated zoom. `anim` is `:linear`, `:ease_in`, `:ease_out`, `:ease_in_out`, `:bounce` |
| `:zoom_punch` | `strength`, `duration` | One shot impulse zoom |
| `:spin` | `rate`, `duration` | Continuous camera rotation |
| `:bounce` | `amplitude`, `duration` | Per onset trauma injection |
| `:freeze` | `duration` | Freeze wall motion |
| `:morph` | `sides`, `duration` | Morph playfield to a new polygon. Walls in flight keep their angles |

### Post process

| Trigger | Fields | Notes |
|---------|--------|-------|
| `:glitch` | `strength`, `duration` | VHS tear |
| `:shake` | `strength`, `duration` | Camera shake. Stacks with max wins policy |
| `:strobe` | `rate`, `duration` | `rate 0` is a single fade |
| `:invert` | `duration` | Flip cursor input |
| `:invert_colors` | `duration` | Flip framebuffer colors |
| `:grayscale` | `strength`, `duration` | Desaturate framebuffer |
| `:shockwave` | `strength`, `duration` | Radial ripple |
| `:fog` | `near`, `far`, `duration` | Radial fog in normalized screen radii |
| `:outline` | `thickness`, `duration` | Sobel edge highlight |
| `:centerburst` | `strength`, `duration` | Particle burst from center |
| `:ringburst` | `count`, `duration` | Series of expanding rings |
| `:bassdrop` | `strength`, `duration` | Composite zoom punch plus shake plus shockwave plus flash |
| `:post_shader` | `shader`, `p0`..`p3` | Activate a user shader. See User shaders |
| `:post_shader_off` | none | Return to default post pipeline |

### Pipes

Several triggers at the same anchor:

```
trigger :flip
    |> :speedwarp { walls = 1.4, duration = 6.0 }
    |> :zoom      { target = 1.4, anim = :bounce, duration = 1.5 }
    |> :strobe    { rate = 12, duration = 1.0 }
```

Compiles to four trigger events firing on the same beat.

### Stacking policy

Intensity replaces the previous value (last writer wins). Duration uses max stacking, so a follow up trigger never clips an earlier longer tail. Speedwarp axes that are absent on a new trigger are left alone.

## Rules

Six categories let a level rewrite gameplay parameters live: `ability`, `vision`, `cursor`, `survival`, `input`, `score`.

File level baseline:

```
#[ability { kind = :dash, cooldown = 0.95 }]
#[vision  { range = 5.5, fog_near = 4.0, fog_far = 5.0 }]
```

Section level overrides:

```
section "blind" at ~t"1:00" do
    rule vision { range = 1.5, fog_near = 0.8, fog_far = 1.5 }
    emit bar
    wait 8
    revert vision
end
```

Stack ops nest:

```
push ability
rule ability { kind = :slowmo }
emit bar
wait 4
pop ability        // restores the file level baseline
```

`revert all` clears every category back to the file baseline at once. Stack depth is capped at 16 per category.

Available fields per category are documented in `src/dsl/rules.rs`. Every numeric value is clamped to a safe range at parse time.

## Time literals

`~t"..."` parses to seconds:

```
~t"30"            ->   30.0 seconds
~t"30.5"          ->   30.5 seconds
~t"1:30"          ->   90.0 seconds
~t"1:30.250"      ->   90.25 seconds
~t"1:05:00"       -> 3900.0 seconds
```

Works in any numeric position: `at`, `duration`, `length`, even inside `var` and field blocks.

## Custom formulas

`emit formula { formula = ~f"...", steps = N }` evaluates an expression for every (step, slot) pair and places a wall when the result is greater than 0.5.

Variables in scope:

| Name | Value |
|------|-------|
| `slot` | Current slot index, 0 to sides |
| `step` | Current step index, 0 to steps |
| `sides` | Level's `generation.sides` |
| `phase` | Random phase chosen at materialization |
| `seed` | Random seed mixed in |

Operators: `+ - * / %`, `== != < <= > >=`, `&& ||`, `! -`, `( )`. Comparisons produce 1.0 or 0.0.

Pathability proof: the parser sweeps the formula across all steps at parse time and rejects the level if any step has zero gaps.

```
emit formula {
    formula = ~f"slot != step % sides"
    steps   = 12
}
```

## Variables

```
var name              = value           // type inferred (f32 default)
var name :: type      = value
var name :: type      = value [pub]
```

Reference variables with `@name` or just `name`:

```
emit ladder { rungs = @rungs }
emit ladder { rungs = rungs }
```

Three scope sources, inner shadows outer:

1. `global do ... end` at file level
2. `section ... where k = v do ... end` for one section
3. `local do ... end` block local

## Hooks

`@on :event do ... end` attaches a handler that fires reactively. Only `trigger` statements may appear inside.

```
@on :onset every 4 do
    trigger :pulse
end

@on :section_enter "drop" do
    trigger :bassdrop { strength = 1.0, duration = 1.2 }
end

@on :close_call above 5 do
    trigger :glitch { strength = 0.5, duration = 0.5 }
end

@on :onset between ~t"0:30"..~t"0:45" do
    trigger :shake { strength = 0.3, duration = 0.2 }
end
```

Events: `:onset` (with optional `every N` or `between A..B`), `:beat`, `:section_enter "name"`, `:close_call above N`.

## Meta language

Top level forms inside the `level` block can declare reusable building blocks.

### Pattern bindings

Name an obstacle so several sections can emit the same shape.

```
pattern :wave = spiral { dir = :cw, loops = 3 }

section "drop" at ~t"0:30" do
    emit_pattern :wave
    wait 4
end
```

### Trigger stacks

Bundle a sequence of triggers under one name. Firing the stack expands every entry at the current anchor.

```
trigger_stack :impact do
    trigger :flip
    trigger :shake { strength = 0.7, duration = 0.5 }
    trigger :ringburst { count = 3, duration = 1.0 }
end

section "hit" at ~t"1:00" do
    fire :impact
end
```

### Functions

Compile time only. Receive parameters, return a sequence of statements via `@expand`. Recursion requires a `decreases` measure.

```
fn cascade(n :: int) -> stmts
    decreases n
do
    if n eq 0 then end
    else
        emit bar
        wait 1
        @expand cascade(n minus 1)
    end
end

section "ramp" at 0.5 do
    @expand cascade(8)
end
```

Comparison operators in meta code use keywords (`eq`, `neq`, `lt`, `le`, `gt`, `ge`) to keep the grammar unambiguous. Arithmetic uses `plus`, `minus`, `mul`, `div`, `mod`. `and`, `or`, `not` are the boolean ops.

### Compile time control flow

`@if`, `@expand`, `match`, and `for` work on meta values. Iteration is bounded; a runaway expansion is rejected at parse time.

```
section "build" at 0.3 do
    for i in 0..6 do
        emit ladder { rungs = i plus 4 }
        wait 1
    end
end
```

### Builtins inside meta code

| Name | Value |
|------|-------|
| `tier` | Atom of the active difficulty tier |
| `sides` | Integer side count |
| `bpm` | Integer BPM |
| `section_name` | String name of the enclosing section |

## User shaders

Levels can declare custom post process shaders in a tiny safe DSL stored as `.shader` files. The runtime sandbox compiles them through `glslangValidator` and validates the resulting SPIR-V before binding.

In the level:

```
shader :wave_glow = ~p"assets/shaders/user/wave_glow.shader"

section "trippy" at ~t"1:30" do
    trigger :post_shader {
        shader = :wave_glow,
        p0 = 1.0, p1 = 0.5, p2 = 0.25, p3 = 0.0
    }
end
```

In the shader file:

```
shader "wave_glow" {
    param strength : float = 1.0
    param speed    : float = 1.0

    body {
        let dx = sin(uv.x * 30.0 + speed) * 0.01
        output sample(uv + vec2(dx, 0.0)) * strength
    }
}
```

Available builtins: `sample(uv)`, `length`, `sin`, `cos`, `abs`, `min`, `max`, `clamp`, `mix`, `smoothstep`, `pow`, `vec2`, `vec3`, `vec4`, `dot`, `fract`, `floor`, `ceil`. No loops, no recursion, no SSBOs. Texture samples are budgeted at 16 per pixel.

The user shader feature is off by default. Enable it in the options screen under Graphics, Shaders. Off, Audit, On are the three modes. A shader that crashes the GPU is added to a persistent quarantine list and refused on future loads.

## Safety rules enforced at parse time

| Rule | Where |
|------|-------|
| BPM in 40..300 | meta |
| Sides in 3..12 | generation |
| Speed in 0.25..3.0 | generation |
| Density in 0.25..3.0 | generation |
| Hue speed in -4..4 | generation |
| min_tier <= max_tier | difficulty |
| Wait in 1..64 beats | wait |
| Repeat count in 1..64 | repeat |
| Custom mask must contain at least one zero | emit custom |
| Formula must leave at least one gap per step | emit formula |
| Variable type must match declared type | var |
| Hooks may only contain trigger statements | @on |
| File size at most 1 MB | parser |
| AST node count, recursion depth, parse time all bounded | parser |

A bad level fails to load with a clear line and column. It cannot crash the running game.

## Recipes

Ramp into a chorus on the beat:

```
section "ramp" at ~t"0:48" do
    trigger :speedwarp { walls = 1.3, music = 1.05, duration = 4.0 }
    repeat 4 do
        emit spiral { dir = :cw, loops = 1 }
    end
end
```

Test pattern with a guaranteed safe path:

```
emit formula { formula = ~f"slot != step % sides", steps = 12 }
```

Strobed flip and shake hit:

```
trigger :flip
    |> :shake { strength = 0.7, duration = 0.4 }
    |> :strobe { rate = 0, duration = 0.3 }
```

Tier locked bonus stage:

```
difficulty do
    range = :ExpertPlus..:ExpertPlus4
    base  = :ExpertPlus2
end
```

Drop to zero quiet bridge:

```
trigger :speedwarp {
    walls    = 0.5,
    rotation = 0.5,
    music    = 0.85,
    duration = ~t"0:08"
}
```

## Reserved tokens

The following are reserved for future use: `take`, `from`, `case`, `of`. v3 already reserves: `let`, `if`, `then`, `else`, `match`, `for`, `in`, `where`, `var`, `do`, `end`, `local`, `global`, `repeat`, `section`, `meta`, `palette`, `difficulty`, `generation`, `level`, `at`, `emit`, `wait`, `trigger`, `rule`, `revert`, `push`, `pop`, `pattern`, `trigger_stack`, `fn`, `requires`, `decreases`, `shader`, `fire`, `emit_pattern`.

## Editor

The embedded editor (Editor button on the main menu) is a graphical tool that produces `.rlf` files in the v3 dialect. It autosaves a session so closing and reopening the game returns you to the same project.

Workflow:

1. Open the editor from the main menu
2. Use the sidebar to add sections, set their start time, pick a music track
3. Click a section to edit it; use the inspector on the right to add and configure statements
4. Press the Pre-Play button to test the level inside the editor without exiting
5. Save when ready; the file lands in `assets/customlevels/` and shows up in the level select on next entry

The editor only emits constructs the runtime AST supports (emits, waits, triggers, repeats, rule operations, shader declarations). The full meta language is parser side; if you want functions or hooks, write them by hand in a text editor and import the file.

## Project layout

```
src/
  audio.rs              waveOut backend, music streaming, onset detection
  dsl/                  parser, AST, expander, rules
  editor/               graph canvas, timeline, inspector, dialogs
  effects/              particles, screen shake
  gend/                 wall and trigger generator
  levels/               catalogue, embedded fallback levels, tiers
  pipeline.rs           main vertex pipeline
  post.rs               offscreen + post pass with PostParams
  qoa.rs                QOA decoder
  renderer.rs           Vulkan setup, swapchain, present
  rule_engine.rs        runtime rule stack engine
  shader_sandbox/       user shader DSL, compile, SPIR-V validation
  text.rs / text_small  bitmap fonts
  ui/                   widget toolkit and draw helpers
  win32.rs              window, input, mouse
  main.rs               glue
```

## License and credits

Music in stock levels is from OpenGameArt and Acid-Notation.