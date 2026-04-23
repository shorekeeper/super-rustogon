# Rustogon Level Format v2

A reference for the second-generation `.rlf` dialect understood by Super Rustogon.

This document describes everything the v2 parser accepts: the syntactic shapes it recognizes, every block, every obstacle, every trigger, the variable system, the timestamp directives, the formula language, and the safety rules the parser enforces before a level is allowed to ship to the generator.

If you have an existing v1 `.rlf` file, it keeps working. v2 is opt-in: a file becomes v2 the moment it starts with `#[use_v2]`.

---

## Table of contents

1. [Quick start](#quick-start)
2. [File layout](#file-layout)
3. [Lexical elements](#lexical-elements)
4. [Directives](#directives)
5. [Blocks](#blocks)
   - [meta](#meta)
   - [palette](#palette)
   - [difficulty](#difficulty)
   - [generation](#generation)
   - [global](#global)
   - [section](#section)
6. [Statements](#statements)
   - [emit](#emit)
   - [wait](#wait)
   - [trigger](#trigger)
   - [repeat](#repeat)
   - [local](#local)
7. [Obstacles](#obstacles)
8. [Triggers](#triggers)
9. [Modifier chains](#modifier-chains-)
10. [Variables](#variables)
11. [Timestamp formats](#timestamp-formats)
12. [Time literals](#time-literals)
13. [Custom formulas](#custom-formulas)
14. [Safety rules](#safety-rules)
15. [Worked examples](#worked-examples)
16. [Migration from v1](#migration-from-v1)

---

## Quick start

The smallest valid v2 file:

```
#[use_v2]

level "HELLO" do
    meta        do bpm = 130 end
    palette     do bgA = rgb(0.1, 0.05, 0.15) end
    difficulty  do range = :Rookie..:Expert, base = :Casual end
    generation  do sides = 6, seed = 0x1 end

    section "intro" at 0.0 do
        emit bar
        wait 2
    end
end
```

Save it as `assets/customlevels/hello.rlf`. The catalogue picks it up at the next launch.

---

## File layout

A v2 file is a sequence of zero or more **directives** followed by exactly one **level form**:

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

Block order inside the level form is free. You can omit any block except the level wrapper itself; missing blocks fall back to defaults. `global` and `section`s are optional too, but with no sections at all the engine inserts a placeholder so the level remains playable.

Both `do ... end` and `{ ... }` open a block. They are interchangeable. The same file can mix them.

---

## Lexical elements

### Whitespace and separators

Spaces, tabs, newlines and commas are all treated as separators. You can write fields one per line, all on one line, or anywhere in between:

```
meta { name = "X" bpm = 130 }
meta { name = "X", bpm = 130 }
meta do
    name = "X"
    bpm  = 130
end
```

The `=` between a field name and its value is **optional**:

```
emit spiral { dir = :cw loops = 3 }
emit spiral { dir   :cw, loops   3 }
```

### Comments

Three styles, all to end of line:

```
// C-style
-- Haskell-style
#  Shell-style (but only when not followed by '['; that opens a directive)
```

### Strings

Plain double-quoted: `"hello"`. Supports escaped quotes and backslashes via the standard `\"` and `\\` (no other escape sequences).

### Numbers

Decimal floats and integers, optionally negative: `0`, `-1.5`, `0.30`. Hexadecimal integers use the `0x` prefix: `0xCAFE`.

### Atoms

Identifiers prefixed with `:` represent enum-like values:

```
dir   = :cw
parity = :even
anim   = :ease_out
```

The colon is required only when the value is in a position where a bare identifier could be confused with a variable reference. In practice you can almost always write `dir = cw` and v2 figures out it is an enum, but `:cw` is clearer at a glance.

### Sigils

Sigils are a typed string with a one-letter parser tag:

| Sigil | Meaning | Example |
|-------|---------|---------|
| `~p"..."` | Asset path | `music = ~p"assets/music/song.qoa"` |
| `~m"..."` | Bit mask (only `0` and `1`) | `mask = ~m"110010"` |
| `~f"..."` | Custom formula expression | `formula = ~f"step % 2 == 0"` |
| `~t"..."` | Time in `S` / `M:SS` / `H:MM:SS` | `at = ~t"1:30"` |

Sigils are interchangeable with plain strings everywhere except `~t`, which always converts to a number of seconds.

### Variable references

`@name` looks up `name` in the lexical scope (sections inherit globals; local blocks shadow). Bare identifiers also fall through to scope lookup before being treated as enum tokens, so `@thick` and `thick` mean the same thing once a `var thick` is in scope.

### Operators

| Token | Used for |
|-------|----------|
| `=` | Field assignment, `var` declaration |
| `::` | Type annotation in `var` |
| `..` | Range in `difficulty` |
| `\|>` | Trigger pipe |
| `>>` | Obstacle modifier chain (Vulkan-style) |
| `<-` | Reserved for future list comprehensions |

---

## Directives

Directives appear before `level` and tweak file-wide behaviour. Their syntax is `#[name]` or `#[name key = value, key = value]`. Unknown directives are silently ignored.

| Directive | Effect |
|-----------|--------|
| `#[use_v2]` | Selects the v2 parser. Required at the very top. |
| `#[timestamp_format_use_relative]` | (default) `at` values are 0..1 fractions of the song. |
| `#[timestamp_format_use_tracklength]` | `at` values are absolute seconds. Pair with `~t`. |
| `#[timestamp_format_use_beats count = N]` | Divide the song into `N` beat buckets; `at` is a bucket index. |
| `#[timestamp_format_use_<name>]` | Author-named tag, behaves like relative but tools can branch on it. |

See [Timestamp formats](#timestamp-formats) for details.

---

## Blocks

### meta

Cosmetic and audio metadata.

```
meta do
    name        = "EXAMPLE"      // optional, falls back to level header
    subtitle    = "DEMO"
    author      = "YOU"
    song        = "TRACK NAME"
    bpm         = 130            // 40..300
    music       = ~p"assets/music/track.qoa"
    description = "Three lines about the level."
end
```

If `name` is omitted, the string from `level "..." do` is used.

### palette

All seven colors used by the renderer. Values are `rgb(r, g, b)` or `rgb r g b`, each component in 0..1. Both camelCase and snake_case keys are accepted.

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

The tier system.

```
difficulty do
    range = :Rookie..:ExpertPlus2     // closed interval
    base  = :Adept                    // pre-selected in the menu
end
```

Available tiers, in rank order:

```
:Rookie  :Casual  :Adept  :Skilled  :Expert
:ExpertPlus  :ExpertPlus1  :ExpertPlus2  :ExpertPlus3  :ExpertPlus4
```

`:ExpertPlus` and `:ExpertPlus1` are the same.

You can also write `min`, `max`, `base` separately instead of `range`:

```
difficulty do
    min  = :Casual
    max  = :Expert
    base = :Adept
end
```

### generation

Static parameters of the playfield and generator.

| Field | Default | Range | Meaning |
|-------|---------|-------|---------|
| `sides` | `6` | `3..=12` | Slot count of the ring |
| `seed` | varies | `u32` | Generator seed, hex allowed |
| `speed` (or `speedMult`) | `1.0` | `0.25..=3.0` | Wall speed multiplier baked in |
| `density` (or `densityMult`) | `1.0` | `0.25..=3.0` | Spawn density multiplier |
| `hueSpeed` (or `hue_speed`) | `0.0` | `-4.0..=4.0` | Continuous palette rotation |

```
generation do
    sides   = 6
    seed    = 0x12345678
    speed   = 1.20
    density = 1.15
end
```

The renderer is currently locked to 6 slots visually, but the generator respects whatever you set; values other than 6 still affect spawn math.

### global

File-level variable declarations visible everywhere below.

```
global do
    var base_thick :: f32 = 1.0       [pub]
    var dash_dir   :: ident = :cw     [read]
end
```

See [Variables](#variables) for the full grammar.

### section

A timed body of statements.

```
section "drop" at 0.50 do
    trigger :flip
    emit staircase { dir = :cw, steps = 8 }
    wait 4
end
```

The `at` clause is interpreted by the file's [timestamp format](#timestamp-formats). Sections are sorted by `at` after parsing, so source order does not matter.

`section`s can declare local bindings via `where`:

```
section "drop" at 0.5 where
    rungs  = 12
    sweeps = 3
do
    repeat sweeps do
        emit ladder { rungs = rungs }
    end
end
```

`where` bindings live in a fresh scope that shadows globals for the body's duration and is unwound automatically when the section ends.

---

## Statements

A section body is a sequence of statements. Statements run in order; the generator schedules each one onto an absolute clock derived from BPM, `wait` delays and the inherent length of obstacles.

### emit

```
emit <name>
emit <name> { field = value, ... }
emit <name> { ... } >> :modifier { ... } >> :modifier { ... }
```

Spawns one obstacle. The braces are optional when there are no fields. Modifier chains are described in [Modifier chains](#modifier-chains-).

### wait

```
wait <beats>
```

Pause for `beats` beats. Range `1..=64`. Beats are real beats (`60 / bpm` seconds each), not arbitrary ticks.

### trigger

```
trigger :name
trigger :name { args }
trigger :name |> :name { args } |> :name { args }
```

A trigger pipes through `|>` to chain several at the same scheduling anchor. Each stage fires in order. See [Triggers](#triggers) for the full vocabulary.

### repeat

```
repeat <count> do
    ... statements ...
end
```

Inlines its body `count` times. Range `1..=64`. Each iteration runs in a fresh scope, so `local` declarations inside the body do not leak between iterations.

### local

```
local do
    var x :: f32 = 1.5
end
```

Pushes a scope visible only within the enclosing block. Equivalent to `section ... where` but usable anywhere statements appear, including inside `repeat`.

---

## Obstacles

Every obstacle is guaranteed by the generator to leave at least one survivable gap. Out-of-range numeric arguments are clamped, never errors.

### Stable patterns

| Name | Fields | Notes |
|------|--------|-------|
| `bar` | `thickness` | One C-shaped wall. |
| `doubleBar` | `spacing`, `thickness` | Two opposite C-walls staggered in time. |
| `spiral` | `dir`, `loops`, `thickness` | Walking wall, `dir = :cw \| :ccw`. |
| `alternate` | `parity`, `thickness` | Every other slot, `parity = :even \| :odd`. |
| `pinwheel` | `spokes`, `dir` | Pairs of opposite walls rotating. |
| `rain` | `count`, `thickness` | Loose burst of independent walls. |
| `rainbow` | `dir` | Staircase of bars whose gap walks one slot per beat. |
| `ladder` | `rungs` | Zigzag between two adjacent gaps. |
| `tunnel` | `length`, `lanes` | Long radial walls in alternating slots. |
| `pot` | `layers` | Stacked bars rotated one slot per layer. |
| `staircase` | `dir`, `steps`, `thickness` | Super Hexagon descending stair. |
| `corridor` | `length`, `turns`, `dir` | Long corridor with periodic gap shifts. |
| `cubes` | `layers`, `dir` | Spinning rubik-face stack. |

### Authored patterns

| Name | Fields | Notes |
|------|--------|-------|
| `custom` | `mask` (sigil `~m"01..."`), `thickness` | Bit mask. Must contain at least one `0`. |
| `formula` | `formula` (sigil `~f"..."`), `steps`, `thickness`, `seed` | See [Custom formulas](#custom-formulas). |

### Field reference

`thickness` is a multiplier on the engine's base wall thickness, typically clamped to `0.5..=2.5`. `dir` accepts `:cw` (default) and `:ccw`. `parity` accepts `:even` (default) and `:odd`.

Numeric ranges per pattern (after clamping):

```
bar.thickness        0.5..=2.5
doubleBar.spacing    1..=6
doubleBar.thickness  0.5..=2.5
spiral.loops         1..=6
spiral.thickness     0.5..=2.0
pinwheel.spokes      1..=6
rain.count           2..=12
rain.thickness       0.5..=1.5
ladder.rungs         3..=16
tunnel.length        0.4..=2.5     (seconds of traversal)
tunnel.lanes         1..=4
pot.layers           2..=8
staircase.steps      3..=24
staircase.thickness  0.5..=2.0
corridor.length      0.6..=3.0
corridor.turns       1..=8
cubes.layers         2..=8
formula.steps        1..=32
formula.thickness    0.5..=2.0
```

Examples:

```
emit bar
emit doubleBar { spacing = 2, thickness = 1.4 }
emit spiral    { dir = :ccw, loops = 3 }
emit rain      { count = 8 }
emit staircase { dir = :cw, steps = 12, thickness = 1.1 }
emit tunnel    { length = 1.6, lanes = 3 }
emit custom    { mask = ~m"110010", thickness = 1.0 }
emit formula   { formula = ~f"step % 2 == 0 && slot != phase", steps = 8 }
```

---

## Triggers

Triggers do not spawn walls. They modify state: camera, speed, post-process, input, audio.

### Stateless triggers

```
trigger :flip      // reverse rotation
trigger :pulse     // brief background pulse
```

### Single-parameter triggers

The classic `tilt`, `speedMult` and `hueShift` accept either a positional number (legacy v1 form) or a field block:

```
trigger :tilt 1.5
trigger :tilt { angle = 1.5 }

trigger :speedMult { factor = 1.3 }
trigger :hueShift  { rate   = 0.4 }
```

Ranges:

```
:tilt       angle    -2.5..=2.5  (degrees)
:speedMult  factor    0.5..=2.0
:hueShift   rate     -2.0..=2.0  (radians per second)
```

### Field-block triggers

| Trigger | Fields | Defaults | What it does |
|---------|--------|----------|--------------|
| `:speedwarp` | `walls`, `rotation`, `cursor`, `music` (alias `musicScale`), `duration` | `0, 0, 0, 0, 3.0` | Independent multipliers for each axis. A zero on any axis means "leave this alone". |
| `:glitch` | `strength`, `duration` | `0.5, 0.8` | Post-process VHS-style horizontal tear. |
| `:shake` | `strength`, `duration` | `0.5, 0.6` | Camera shake added to the trauma accumulator. |
| `:zoom` | `target`, `anim`, `duration` | `1.0, :linear, 1.5` | Animated camera zoom. `target` is a scale factor. |
| `:invert` | `duration` | `3.0` | Flip left/right cursor input. |
| `:strobe` | `rate`, `duration` | `6.0, 0.8` | Periodic flash at `rate` Hz. `rate = 0` means a single fade. |

Animation names for `:zoom.anim`: `:linear`, `:ease_in`, `:ease_out`, `:ease_in_out`, `:bounce`.

Examples:

```
trigger :speedwarp { walls = 1.4, music = 1.1, duration = 4.0 }
trigger :glitch    { strength = 0.7, duration = 1.2 }
trigger :shake     { strength = 0.5, duration = 0.4 }
trigger :zoom      { target = 1.3, anim = :ease_out, duration = 1.0 }
trigger :invert    { duration = 5.0 }
trigger :strobe    { rate = 8, duration = 1.0 }
```

### Pipes

Several triggers at the same anchor:

```
trigger :flip |> :speedwarp { walls = 1.3, music = 1.05, duration = 3.0 }
              |> :zoom      { target = 1.2, anim = :ease_out, duration = 1.0 }
```

This compiles to three separate `Stmt::Trigger` entries, applied in order on the same beat.

---

## Modifier chains (`>>`)

Vulkan-style chains let you tweak an obstacle without nesting fields. Each link starts with `>> :tag` and may carry its own field block.

```
emit spiral  { dir = :cw, loops = 2 } >> :thickness { mult = 1.3 }
emit ladder  { rungs = 8 }            >> :thickness { mult = 0.8 }
                                      >> :telegraph { lead = 0.3 }
```

Recognized tags:

| Tag | Effect |
|-----|--------|
| `:thickness` | Multiplies the obstacle's thickness by `mult` (or `value`). Clamps to `0.25..=3.0`. No-op on patterns with no adjustable thickness (you may still chain it; it just does nothing). |
| `:telegraph` | Reserved. Accepted for forward compatibility; engine currently ignores it. |

Unknown tags are a parse error so typos do not silently disappear.

---

## Variables

### Declaration

```
var name              = value                 // type inferred (defaults to f32)
var name :: type      = value                 // explicit type
var name :: type      = value [pub]           // with access modifier
var name :: type      = value [priv, read]    // multiple modifiers
```

Types: `f32` / `float`, `i32` / `int`, `string` / `str`, `ident`, `bool`.

Access modifiers: `pub` / `public` (default), `priv` / `private`, `read` / `readonly`. The current engine treats them as documentation; tooling may enforce them later.

Values:

```
var a :: f32     = 1.5
var b :: int     = 42
var c :: bool    = true
var d :: string  = "hi"
var e :: ident   = :cw
var f :: f32     = ~t"1:30"
var g :: f32     = @a            // reference another variable
```

Type mismatches are errors:

```
var x :: int = "hello"   // ParseError: value does not match type i32
```

### Scope and unwind

Three scope sources:

1. `global do ... end` — file-wide.
2. `section ... where k = v, k = v do ... end` — section-wide.
3. `local do ... end` — block-local. Can appear anywhere statements appear, including inside `repeat`.

Inner scopes shadow outer ones. When a scope ends (closing `}` / `end`), all bindings declared in it disappear. Outer values become visible again unchanged.

### Reference

Two ways to reference a variable in any value position:

```
emit ladder { rungs = @rungs }    // explicit
emit ladder { rungs = rungs }     // bare ident; falls back to scope lookup
```

Bare identifiers first try scope lookup, then fall back to "enum token". So if `cw` is not declared, `dir = cw` means `:cw`; if `cw = ...` is declared, it means that variable's value.

---

## Timestamp formats

Every `at` clause in a section is interpreted by the active timestamp format. Switch via a directive at the top of the file.

### `relative` (default)

```
section "intro"  at 0.00 do ... end
section "drop"   at 0.50 do ... end
section "outro"  at 0.85 do ... end
```

Values are 0..1 fractions of the song. Out-of-range values are clamped at runtime.

### `tracklength`

```
#[timestamp_format_use_tracklength]

section "intro" at  0.0  do ... end
section "drop"  at 90.0  do ... end       // 90 seconds in
section "drop"  at ~t"1:30" do ... end    // same thing, more readable
```

Values are absolute seconds, compared to the live track position. Pair with the `~t` time literal for human-readable times.

### `beats`

```
#[timestamp_format_use_beats count = 64]

section "intro" at  0 do ... end
section "drop"  at 32 do ... end
```

The song is divided into `count` equal buckets. `at` is the bucket index. If `count` is omitted, it defaults to 64.

### Named

```
#[timestamp_format_use_phase4]
```

Behaves like `relative` but the AST records the tag verbatim so external tooling can branch on it.

---

## Time literals

`~t"..."` parses to a number of seconds. Five accepted shapes:

```
~t"30"             ->  30 seconds
~t"30.5"           ->  30.5 seconds
~t"1:30"           ->  90 seconds
~t"1:30.250"       ->  90.25 seconds
~t"1:05:00"        ->  3900 seconds
~t"1:05:00.500"    ->  3900.5 seconds
```

Constraints: in `M:SS` form, `SS` must be in `0..60`. In `H:MM:SS` form, `MM` must be in `0..60` and `SS` must be in `0..60`. The leading component is unbounded so `~t"180:00"` is a valid way to write 3 hours on a long mix.

`~t` works in any numeric position: `at`, `duration`, `length`, even inside `var` and field blocks.

```
section "drop" at ~t"1:30" do
    trigger :speedwarp { duration = ~t"0:08" }
    emit corridor      { length   = ~t"2.0" }
end
```

Without `#[timestamp_format_use_tracklength]` the `at` value will still be interpreted as a 0..1 fraction, so 90 seconds gets clamped to 1.0. Use `~t` together with the directive.

---

## Custom formulas

`emit formula { formula = ~f"...", steps = N }` evaluates an expression for every `(step, slot)` pair and places a wall when the result is truthy (`> 0.5`).

### Variables in scope inside the formula

| Name | Value |
|------|-------|
| `slot` | Current slot index, `0..sides` |
| `step` | Current step index, `0..steps` |
| `sides` | The level's `generation.sides` |
| `phase` | Random phase chosen at materialization |
| `seed` | Random seed mixed in at materialization |

### Operators

```
+   -   *   /   %                     arithmetic
==  !=  <   <=  >   >=                comparison
&&  ||                                logical
!  -                                  unary not, negate
( )                                   grouping
```

Booleans encode as `1.0` / `0.0`. The result is treated as "wall" when strictly greater than `0.5`.

### Examples

```
// Bar with a gap walking around the ring.
emit formula { formula = ~f"slot != step % sides", steps = 12 }

// Every other step is empty (breathing room).
emit formula {
    formula = ~f"step % 2 == 0 && slot != (step + phase) % sides"
    steps   = 8
}

// Two parallel gaps that converge.
emit formula {
    formula = ~f"slot != step && slot != sides - 1 - step"
    steps   = 6
}
```

### Pathability proof

The parser sweeps the formula across all `(step, slot)` pairs at parse time and rejects the level if any step has zero gaps. This is a conservative check: if it passes, the pattern is provably survivable for at least one slot at every step.

The rare case where `phase` or `seed` would close every gap on some step is caught at runtime by removing one wall in that step, so a formula that proves pathable but accidentally hits a closed phase still does not kill the player unfairly.

---

## Safety rules

The parser enforces every invariant the runtime relies on. Violations are `ParseError`s with line/column, never silent runtime misbehaviour.

| Rule | Where |
|------|-------|
| BPM in 40..300 | `meta.bpm` |
| Sides in 3..12 | `generation.sides` |
| Speed in 0.25..3.0 | `generation.speed` |
| Density in 0.25..3.0 | `generation.density` |
| Hue speed in -4.0..4.0 | `generation.hueSpeed` |
| min_tier <= max_tier | `difficulty` |
| Wait in 1..64 beats | `wait` statement |
| Repeat count in 1..64 | `repeat` statement |
| Section `at` 0..1 (after timestamp normalization) | `section ... at` |
| Custom mask must contain at least one `0` | `emit custom` |
| Formula must leave at least one gap per step | `emit formula` |
| Modifier tag must be known | `>> :tag` |
| Variable type must match its value | `var ... :: type = value` |
| `@name` must resolve | `@reference` |

Numeric arguments are clamped (not rejected) to their pattern-specific ranges; you do not need to memorize them, you just cannot go outside the playable envelope.

---

## Worked examples

### Long mix with absolute times

```
#[use_v2]
#[timestamp_format_use_tracklength]

level "TWELVE MINUTE EPIC" do
    meta do
        author = "ME"
        bpm    = 128
        music  = ~p"assets/music/long_mix.qoa"
    end
    palette    do bgA = rgb(0.10, 0.05, 0.18) end
    difficulty do range = :Casual..:Expert, base = :Adept end
    generation do sides = 6, seed = 0xC0FFEE end

    section "intro"  at ~t"0:00"     do emit bar end
    section "build"  at ~t"0:30"     do emit ladder { rungs = 6 } end
    section "drop"   at ~t"1:30"     do
        trigger :flip |> :shake { strength = 0.6, duration = 0.5 }
        emit staircase { dir = :cw, steps = 8 }
    end
    section "bridge" at ~t"3:15.500" do emit corridor { length = 1.5, turns = 4 } end
    section "second" at ~t"5:00"     do
        trigger :strobe { rate = 8, duration = 1.0 }
        emit cubes { layers = 6 }
    end
    section "outro"  at ~t"11:00"    do emit pinwheel { spokes = 6 } end
end
```

### Variables and repeat

```
#[use_v2]

level "LADDER WORKOUT" do
    meta        do bpm = 140 end
    palette     do bgA = rgb(0.04, 0.10, 0.16) end
    difficulty  do range = :Adept..:ExpertPlus, base = :Skilled end
    generation  do sides = 6, seed = 0x42 end

    global do
        var rungs   :: int = 6  [pub]
        var sweeps  :: int = 4  [pub]
    end

    section "warmup" at 0.00 do
        repeat 2 do
            emit ladder { rungs = rungs }
            wait 2
        end
    end

    section "drop" at 0.40 where
        rungs = 12
    do
        trigger :flip |> :glitch { strength = 0.6, duration = 1.0 }
        repeat sweeps do
            emit staircase { dir = :cw, steps = rungs }
        end
    end
end
```

### Trigger pipe stack

```
section "climax" at 0.75 do
    trigger :flip
        |> :speedwarp { walls = 1.4, rotation = 1.2, cursor = 1.1, music = 1.1, duration = 6.0 }
        |> :zoom      { target = 1.4, anim = :bounce, duration = 1.5 }
        |> :strobe    { rate = 12, duration = 1.0 }
    emit corridor { length = 2.0, turns = 5 }
end
```

### Modifier chain on every emit in a repeat

```
section "wall_of_walls" at 0.5 where
    bump = 1.3
do
    repeat 4 do
        emit spiral { dir = :cw, loops = 2 } >> :thickness { mult = bump }
    end
end
```

### Custom formula

```
section "puzzle" at 0.6 do
    emit formula {
        formula = ~f"(step % 3 != 0) && slot != (step * 2 + phase) % sides"
        steps   = 9
        thickness = 1.0
    }
end
```

---

## Migration from v1

If you have a v1 file you want to upgrade, the mechanical steps are:

1. Add `#[use_v2]` as the first line.
2. Optional: replace `{` / `}` with `do` / `end` where readable.
3. Optional: replace `dir = ccw` with `dir = :ccw` for visual clarity (both work).
4. Optional: replace `rgb 0.1 0.2 0.3` with `rgb(0.1, 0.2, 0.3)` (both still work).
5. Optional: switch from positional triggers to field blocks: `trigger tilt 1.5` becomes `trigger :tilt { angle = 1.5 }`.
6. Optional: lift repeated sequences into `repeat N do ... end`.

Nothing in step 2 onward is required. v1 syntax remains valid inside a v2 file. Mixing the two during a transition is fine.

A few things v1 did not have that you only get on v2:

- `~t` time literals
- `#[timestamp_format_use_tracklength]` and `beats`
- `global` / `local` / `where` variable scopes
- `repeat`
- Trigger pipes (`|>`)
- Modifier chains (`>>`)
- Custom `formula` patterns
- `staircase`, `corridor`, `cubes` obstacles
- Atom syntax (`:cw`)
- Sigils (`~p`, `~m`, `~f`, `~t`)
- `do ... end` blocks

---

## Recipe index

Common shapes you might want to copy.

**Ramp into a chorus on the beat:**
```
section "ramp" at ~t"0:48" do
    trigger :speedwarp { walls = 1.3, music = 1.05, duration = 4.0 }
    repeat 4 do
        emit spiral { dir = :cw, loops = 1 }
    end
end
```

**Test pattern with a guaranteed safe path:**
```
emit formula { formula = ~f"slot != step % sides", steps = 12 }
```

**Strobed flip-and-shake hit:**
```
trigger :flip |> :shake { strength = 0.7, duration = 0.4 }
              |> :strobe { rate = 0, duration = 0.3 }
```

**Tier-locked bonus stage:**
```
difficulty do
    range = :ExpertPlus..:ExpertPlus4
    base  = :ExpertPlus2
end
```

**Drop-to-zero quiet bridge:**
```
trigger :speedwarp { walls = 0.5, rotation = 0.5, music = 0.85, duration = ~t"0:08" }
```

---

## Errors you will see

| Message | What is wrong |
|---------|---------------|
| `expected ']' after '#'` | Malformed directive header. |
| `unknown obstacle '<name>'` | Typo or obstacle not in this version. |
| `unknown trigger '<name>'` | Same, for triggers. |
| `unknown chain modifier '>> :<tag>'` | Unknown `>>` tag. |
| `custom mask must leave at least one gap` | `~m"111111"` rejected. |
| `formula leaves no gap at step N` | The pathability check failed. |
| `value <X> does not match declared type <T>` | `var x :: int = "hi"`. |
| `undefined '@<name>'` | The variable does not exist in the current scope. |
| `bpm out of range: <N>` | Outside 40..300. |
| `sides out of range: <N>` | Outside 3..12. |
| `~t must have 1, 2 or 3 colon-separated parts` | `~t"1:2:3:4"` is too long. |
| `seconds out of 0..60 in ~t` | `~t"1:90"` is illegal. |
| `bad number 'X' in ~t` | Non-numeric component. |

All errors carry the source line and column so editors can jump straight to the offending token.

---

## Reserved tokens

The following are reserved for future use and should not be relied on as obstacle / trigger / variable names: `<-`, `take`, `from`, `where`, `let`, `in`, `if`, `then`, `else`, `case`, `of`. The current parser does not all use them, but a future revision likely will. v2 already reserves `where`, `var`, `do`, `end`, `local`, `global`, `repeat`, `section`, `meta`, `palette`, `difficulty`, `generation`, `level`, `at`, `emit`, `wait`, `trigger`.

---