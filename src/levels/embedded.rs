//! Compile time fallback roster, migrated to the v3 dialect.
//!
//! Every stock level here is now valid v3 source. The
//! `#[use_v3]` directive at the top of each one is purely
//! self-documentation (v3 is the default dispatcher target),
//! but including it keeps the files explicit and makes
//! copy-paste migration to on-disk `.rlf` files
//! straightforward.
//!
//! The older v1 and v2 demo tracks were removed alongside
//! their parsers; `V3_DEMO` now covers the feature showcase
//! role that `V2_DEMO` used to fill.

pub const EMBEDDED_LEVELS: &[&str] = &[
    HEXAGON,
    HEXAGONER,
    HEXAGONEST,
    HYPER_HEXAGON,
    HYPER_HEXAGONER,
    HYPER_HEXAGONEST,
    NOSTALGIA,
    V3_DEMO,
    EVERYTHING,
];

const HEXAGON: &str = r#"
#[use_v3]

level "HEXAGON" do
    meta do
        subtitle    = "BEGIN"
        author      = "Chipzel"
        song        = "COURTESY"
        bpm         = 130
        music       = ~p"assets/music/hexagon1.qoa"
        description = "A gentle introduction. Walls come one at a time, gaps are obvious, the tempo is forgiving."
    end
    palette do
        bgA        = rgb(0.10, 0.04, 0.16)
        bgB        = rgb(0.05, 0.02, 0.10)
        centerFill = rgb(0.05, 0.02, 0.08)
        centerRing = rgb(1.00, 0.40, 0.70)
        wall       = rgb(1.00, 0.40, 0.70)
        player     = rgb(1.00, 0.95, 1.00)
        accent     = rgb(1.00, 0.40, 0.70)
    end
    difficulty do range = :Rookie..:Expert, base = :Rookie end
    generation do sides = 6, seed = 0x12345678, speed = 1.0, density = 1.0 end

    global do
        var base_thick :: f32 = 1.0 [pub]
    end

    section "intro" at 0.00 do
        emit bar { thickness = 1.0 }
        emit spiral { dir = :cw }
        emit bar
        emit alternate { parity = :even }
        emit doubleBar { spacing = 2 }
    end
    section "mid" at 0.30 do
        trigger :tilt { angle = 1.2 }
        emit rainbow { dir = :cw }
        emit staircase { dir = :cw, steps = 6 }
        emit doubleBar { spacing = 2 }
        emit spiral { dir = :ccw }
    end
    section "finale" at 0.70 do
        trigger :flip
        trigger :shake { strength = 0.4, duration = 0.5 }
        emit ladder { rungs = 6 }
        emit rainbow { dir = :ccw }
        emit pinwheel { spokes = 3 }
        emit bar
    end
end
"#;

const HEXAGONER: &str = r#"
#[use_v3]

level "HEXAGONER" do
    meta do
        subtitle    = "HARDER"
        author      = "Chipzel"
        song        = "OTIS"
        bpm         = 135
        music       = ~p"assets/music/hexagon2.qoa"
        description = "Spirals and staggered bars. The world spins a little harder now; commit early."
    end
    palette do
        bgA        = rgb(0.04, 0.10, 0.16)
        bgB        = rgb(0.02, 0.05, 0.10)
        centerFill = rgb(0.02, 0.05, 0.08)
        centerRing = rgb(0.40, 0.85, 1.00)
        wall       = rgb(0.40, 0.85, 1.00)
        player     = rgb(0.95, 1.00, 1.00)
        accent     = rgb(0.40, 0.85, 1.00)
    end
    difficulty do range = :Rookie..:Expert, base = :Casual end
    generation do
        sides    = 6
        seed     = 2882400001
        hueSpeed = 0.20
        speed    = 1.10
        density  = 1.05
    end

    section "intro" at 0.00 do
        repeat 2 do
            emit spiral { dir = :cw, loops = 2 }
            emit pinwheel { spokes = 3 }
        end
        emit rainbow { dir = :cw }
        emit doubleBar { spacing = 2 }
    end
    section "storm" at 0.35 do
        trigger :flip
        trigger :speedwarp { walls = 1.2, rotation = 1.1, music = 1.05, duration = 3.5 }
        emit staircase { dir = :cw, steps = 8 }
        emit spiral { dir = :ccw, loops = 2 }
        emit cubes { layers = 4 }
        emit pinwheel { spokes = 4 }
    end
    section "finale" at 0.75 do
        trigger :tilt { angle = -1.8 }
        trigger :glitch { strength = 0.4, duration = 1.0 }
        emit rainbow { dir = :ccw }
        emit tunnel { length = 1.0, lanes = 3 }
        emit ladder { rungs = 8 }
        emit pinwheel { spokes = 4 }
    end
end
"#;

const HEXAGONEST: &str = r#"
#[use_v3]

level "HEXAGONEST" do
    meta do
        subtitle    = "HARDEST"
        author      = "Chipzel"
        song        = "FOCUS"
        bpm         = 174
        music       = ~p"assets/music/hexagon3.qoa"
        description = "Everything you have learned, faster. Pinwheels start appearing between the bars."
    end
    palette do
        bgA        = rgb(0.04, 0.16, 0.06)
        bgB        = rgb(0.02, 0.08, 0.03)
        centerFill = rgb(0.02, 0.08, 0.03)
        centerRing = rgb(0.55, 1.00, 0.40)
        wall       = rgb(0.55, 1.00, 0.40)
        player     = rgb(1.00, 1.00, 0.95)
        accent     = rgb(0.55, 1.00, 0.40)
    end
    difficulty do range = :Casual..:ExpertPlus, base = :Adept end
    generation do
        sides    = 6
        seed     = 1337420
        hueSpeed = 0.35
        speed    = 1.20
        density  = 1.15
    end

    section "intro" at 0.00 do
        emit rainbow { dir = :cw }
        emit pinwheel { spokes = 4 }
        emit staircase { dir = :cw, steps = 8 }
        emit spiral { dir = :cw, loops = 3 }
    end
    section "storm" at 0.35 do
        trigger :flip
        trigger :zoom { target = 1.25, anim = :ease_out, duration = 1.2 }
        emit corridor { length = 1.2, turns = 3, dir = :cw }
        emit rainbow { dir = :ccw }
        emit cubes { layers = 5 }
        emit doubleBar { spacing = 2 }
    end
    section "finale" at 0.75 do
        trigger :pulse
        trigger :tilt { angle = 2.2 }
        trigger :zoom { target = 1.0, anim = :ease_in_out, duration = 1.5 }
        emit ladder { rungs = 10 }
        emit corridor { length = 1.4, turns = 4, dir = :cw }
        emit rainbow { dir = :cw }
        emit pinwheel { spokes = 5 }
    end
end
"#;

const HYPER_HEXAGON: &str = r#"
#[use_v3]
#[timestamp_format_use_relative]

level "HYPER HEXAGON" do
    meta do
        subtitle    = "HARDESTEST"
        author      = "Chipzel"
        song        = "COURTESY"
        bpm         = 130
        music       = ~p"assets/music/hexagon1.qoa"
        description = "The hyper tier starts here. Walls keep coming, the camera keeps flipping."
    end
    palette do
        bgA        = rgb(0.18, 0.08, 0.02)
        bgB        = rgb(0.09, 0.04, 0.01)
        centerFill = rgb(0.09, 0.04, 0.01)
        centerRing = rgb(1.00, 0.65, 0.20)
        wall       = rgb(1.00, 0.65, 0.20)
        player     = rgb(1.00, 1.00, 0.95)
        accent     = rgb(1.00, 0.65, 0.20)
    end
    difficulty do range = :Adept..:ExpertPlus2, base = :Skilled end
    generation do
        sides    = 6
        seed     = 112358
        hueSpeed = 0.50
        speed    = 1.30
        density  = 1.20
    end

    global do
        var warp_duration :: f32 = 3.0 [pub]
    end

    section "intro" at 0.00 do
        emit staircase { dir = :cw, steps = 8 }
        emit ladder { rungs = 6 }
        emit spiral { dir = :ccw, loops = 3 }
        emit cubes { layers = 4 }
    end
    section "mid" at 0.30 do
        trigger :flip
        trigger :speedwarp { walls = 1.15, cursor = 1.05, duration = 3.0 }
        emit corridor { length = 1.1, turns = 3, dir = :cw }
        emit rainbow { dir = :ccw }
        emit pinwheel { spokes = 5 }
        emit staircase { dir = :ccw, steps = 8 }
    end
    section "finale" at 0.70 do
        trigger :tilt { angle = -2.5 }
        trigger :shake { strength = 0.7, duration = 1.0 }
        trigger :strobe { rate = 8, duration = 1.2 }
        emit corridor { length = 1.5, turns = 4, dir = :cw }
        emit rainbow { dir = :cw }
        emit ladder { rungs = 10 }
        emit cubes { layers = 6 }
        emit pinwheel { spokes = 5 }
    end
end
"#;

const HYPER_HEXAGONER: &str = r#"
#[use_v3]

level "HYPER HEXAGONER" do
    meta do
        subtitle    = "HARDERESTEST"
        author      = "Chipzel"
        song        = "OTIS"
        bpm         = 135
        music       = ~p"assets/music/hexagon2.qoa"
        description = "Dense patterns, rapid camera flips, a palette that drifts under your feet."
    end
    palette do
        bgA        = rgb(0.16, 0.04, 0.16)
        bgB        = rgb(0.08, 0.02, 0.08)
        centerFill = rgb(0.08, 0.02, 0.08)
        centerRing = rgb(0.95, 0.45, 1.00)
        wall       = rgb(0.95, 0.45, 1.00)
        player     = rgb(1.00, 1.00, 1.00)
        accent     = rgb(0.95, 0.45, 1.00)
    end
    difficulty do range = :Skilled..:ExpertPlus3, base = :Expert end
    generation do
        sides    = 6
        seed     = 2718281
        hueSpeed = 0.70
        speed    = 1.40
        density  = 1.30
    end

    section "intro" at 0.00 do
        emit pinwheel { spokes = 5 }
        emit staircase { dir = :cw, steps = 10 }
        emit ladder { rungs = 8 }
        emit corridor { length = 1.0, turns = 3, dir = :cw }
    end
    section "mid" at 0.25 do
        trigger :flip
        trigger :tilt { angle = 1.5 }
        trigger :zoom { target = 1.35, anim = :bounce, duration = 1.8 }
        emit rainbow { dir = :ccw }
        emit cubes { layers = 6 }
        emit corridor { length = 1.4, turns = 4, dir = :cw }
        emit ladder { rungs = 10 }
    end
    section "finale" at 0.65 do
        trigger :speedwarp { walls = 1.25, rotation = 1.2, music = 1.1, duration = 5.0 }
        trigger :glitch { strength = 0.7, duration = 1.4 }
        emit corridor { length = 1.8, turns = 5, dir = :cw }
        emit rainbow { dir = :cw }
        emit ladder { rungs = 12 }
        emit pinwheel { spokes = 6 }
        emit cubes { layers = 7 }
    end
end
"#;

const HYPER_HEXAGONEST: &str = r#"
#[use_v3]

level "HYPER HEXAGONEST" do
    meta do
        subtitle    = "HARDESTESTEST"
        author      = "Chipzel"
        song        = "FOCUS"
        bpm         = 174
        music       = ~p"assets/music/hexagon3.qoa"
        description = "The hardest stock level. No palette to hide behind: everything is stark white."
    end
    palette do
        bgA        = rgb(0.14, 0.14, 0.16)
        bgB        = rgb(0.06, 0.06, 0.08)
        centerFill = rgb(0.06, 0.06, 0.08)
        centerRing = rgb(1.00, 1.00, 1.00)
        wall       = rgb(1.00, 1.00, 1.00)
        player     = rgb(1.00, 1.00, 1.00)
        accent     = rgb(1.00, 1.00, 1.00)
    end
    difficulty do range = :Expert..:ExpertPlus4, base = :ExpertPlus end
    generation do
        sides    = 6
        seed     = 141421356
        hueSpeed = 0.0
        speed    = 1.55
        density  = 1.45
    end

    section "intro" at 0.00 do
        emit staircase { dir = :cw, steps = 2 }
        emit ladder { rungs = 8 }
        emit pinwheel { spokes = 6 }
    end
    section "mid" at 0.25 do
        trigger :flip
        trigger :speedwarp { walls = 1.3, cursor = 1.1, duration = 4.0 }
        emit corridor { length = 1.5, turns = 4, dir = :cw }
        emit rainbow { dir = :ccw }
        emit cubes { layers = 7 }
        emit ladder { rungs = 12 }
    end
    section "finale" at 0.65 do
        trigger :pulse
        trigger :tilt { angle = 2.5 }
        trigger :shake { strength = 0.9, duration = 1.5 }
        trigger :invert { duration = 2.5 }
        emit corridor { length = 2.0, turns = 5, dir = :cw }
        emit rainbow { dir = :cw }
        emit ladder { rungs = 14 }
        emit rainbow { dir = :ccw }
        emit pinwheel { spokes = 6 }
        emit cubes { layers = 8 }
    end
end
"#;

// NOSTALGIA showcases the `#[startfrom]` directive and a
// tracklength-based timeline. Authored directly in v3; kept
// verbatim through the migration.
const NOSTALGIA: &str = r#"
#[use_v3]
#[timestamp_format_use_tracklength]
#[startfrom ~t"0:38"]
#[ignore_collisions]
// Empirical constraints from playtest:
//
// 1) `corridor.length` multiplies current wall_speed to
//    produce the segment radial thickness. At Skilled tier
//    with level speed > 1.0 and any speedwarp on, segments
//    grow past 3 units (the engine clamp) and consecutive
//    segments overlap into a continuous wall. We use
//    `staircase` everywhere instead: same "walking gap"
//    feel, single-thickness walls.
//
// 2) Keep speedwarp.walls / speedwarp.cursor ratio under
//    1.15 so the cursor reaches the gap in time.
//
// 3) Adept tier at level speed 1.0 puts wall flight time
//    around 2.3 seconds, enough for staircase chains of
//    10+ steps without the playfield filling.

#[ability  { kind = :dash, cooldown = 0.95 }]
#[vision   { range = 5.5, fog_near = 4.0, fog_far = 5.0 }]
#[score    { multiplier = 1.0, close_call_bonus = 75 }]

level "NOSTALGIA" do
    meta do
        name        = "NOSTALGIA"
        subtitle    = "FOUR FIFTY OF FIRE"
        author      = "ATOMIZED V2.1"
        song        = "NOSTALGIA"
        bpm         = 140
        music       = ~p"assets/music/nostalgia.qoa"
        description = "Two drops, a 27-second climax, an absolute peak at 4:17."
    end

    palette do
        bgA        = rgb(0.05, 0.02, 0.18)
        bgB        = rgb(0.02, 0.01, 0.10)
        centerFill = rgb(0.02, 0.01, 0.08)
        centerRing = rgb(1.00, 0.45, 0.85)
        wall       = rgb(1.00, 0.55, 0.95)
        player     = rgb(1.00, 0.95, 1.00)
        accent     = rgb(1.00, 0.45, 0.85)
    end

    difficulty do range = :Casual..:Expert, base = :Adept end

    generation do
        sides    = 6
        seed     = 0x4A7E1101
        speed    = 1.00
        density  = 1.10
        hueSpeed = 0.10
    end

    // 0:00 - 0:42 INTRO. avg_intensity 0..0.24.
    section "ghost_in" at ~t"0:00" do
        trigger :fog       { near = 0.35, far = 0.90, duration = 14.0 }
            |> :grayscale  { strength = 0.55, duration = 14.0 }
            |> :hueShift   { rate = 0.10, duration = 14.0 }
        wait 8
    end

    section "first_stir" at ~t"0:14" do
        emit bar { thickness = 0.8 }
        wait 3
        emit alternate { parity = :even, thickness = 0.8 }
        wait 3
        emit bar { thickness = 0.85 }
        wait 3
        emit ladder { rungs = 4 }
        wait 3
    end

    section "preheat" at ~t"0:30" do
        trigger :speedwarp { walls = 0.85, cursor = 1.10, duration = 12.0 }
            |> :outline    { thickness = 0.8, duration = 12.0 }
            |> :hueShift   { rate = 0.40, duration = 12.0 }
        emit ladder    { rungs = 6 }
        wait 2
        emit spiral    { dir = :cw, loops = 2 }
        wait 2
        emit pinwheel  { spokes = 3 }
        wait 2
        emit alternate { parity = :odd, thickness = 0.9 }
        wait 2
    end

    // 0:42.5 first drop. Walls/cursor 1.20/1.15.
    section "drop_one" at ~t"0:42.500" do
        trigger :bassdrop  { strength = 1.0, duration = 1.6 }
            |> :flip
            |> :speedwarp  { walls = 1.20, rotation = 1.10, cursor = 1.15, duration = 16.0 }
            |> :ringburst  { count = 4, duration = 1.8 }
            |> :centerburst { strength = 0.9, duration = 0.5 }
            |> :hueShift   { rate = 0.55, duration = 12.0 }
        emit doubleBar { spacing = 2, thickness = 1.1 }
        wait 1
    end

    section "fire_one" at ~t"0:46" do
        emit staircase { dir = :cw, steps = 6, thickness = 1.0 }
        wait 5
        emit alternate { parity = :odd }
        wait 4
    end

    section "fire_two" at ~t"0:58" do
        trigger :pulse
        emit cubes     { layers = 5, dir = :cw }
        wait 4
        emit pinwheel  { spokes = 5 }
        wait 3
        emit alternate { parity = :odd }
        wait 3
    end

    section "hit_111" at ~t"1:11" do
        trigger :flip
            |> :centerburst { strength = 0.9, duration = 0.5 }
            |> :shake       { strength = 0.5, duration = 0.7 }
            |> :spin        { rate = 1.8, duration = 4.0 }
            |> :speedwarp   { cursor = 1.15, duration = 8.0 }
        emit cubes { layers = 6, dir = :ccw }
        wait 3
    end

    section "rolling_115" at ~t"1:14.500" do
        emit staircase { dir = :cw, steps = 6 }
        wait 5
        emit doubleBar { spacing = 2 }
        wait 4
    end

    section "rolling_125" at ~t"1:25" do
        trigger :spin     { rate = -1.5, duration = 8.0 }
            |> :hueShift  { rate = 0.4, duration = 8.0 }
            |> :speedwarp { cursor = 1.15, duration = 12.0 }
        emit pinwheel  { spokes = 5 }
        wait 3
        emit ladder    { rungs = 6 }
        wait 4
        emit cubes     { layers = 5, dir = :ccw }
        wait 4
    end

    // 1:39 groove_b. Was: corridor length=1.5 turns=5.
    section "groove_b1" at ~t"1:39" do
        trigger :flip |> :glitch { strength = 0.45, duration = 0.8 }
        emit staircase { dir = :cw, steps = 10, thickness = 1.0 }
        wait 6
        emit staircase { dir = :ccw, steps = 8 }
        wait 5
    end

    section "groove_b2" at ~t"1:55" do
        emit cubes     { layers = 5, dir = :cw }
        wait 4
        emit pinwheel  { spokes = 6 }
        wait 3
        emit doubleBar { spacing = 3 }
        wait 3
    end

    // 2:06.5 - 2:33 first climax. 27 seconds, peaks 0.99/0.96.
    section "climax_open" at ~t"2:06.500" do
        trigger :bassdrop  { strength = 1.0, duration = 1.8 }
            |> :flip
            |> :speedwarp  { walls = 1.30, rotation = 1.20, cursor = 1.20, duration = 20.0 }
            |> :spin       { rate = 2.2, duration = 10.0 }
            |> :ringburst  { count = 5, duration = 2.0 }
            |> :centerburst { strength = 1.1, duration = 0.6 }
            |> :hueShift   { rate = 0.7, duration = 16.0 }
            |> :tilt       { angle = 8, pitch = 6, duration = 14.0 }
        emit staircase { dir = :cw, steps = 12, thickness = 1.0 }
        wait 6
    end

    section "climax_213" at ~t"2:13" do
        trigger :flip |> :shake { strength = 0.55, duration = 0.7 }
        emit cubes    { layers = 6, dir = :ccw }
        wait 4
        emit pinwheel { spokes = 6 }
        wait 2
    end

    section "climax_215" at ~t"2:15.500" do
        trigger :bassdrop    { strength = 0.95, duration = 1.2 }
            |> :centerburst  { strength = 1.1, duration = 0.6 }
            |> :shockwave    { strength = 0.9, duration = 0.8 }
        emit staircase { dir = :cw, steps = 8 }
        wait 5
    end

    section "climax_219" at ~t"2:19" do
        trigger :flip |> :shockwave { strength = 1.0, duration = 0.9 }
        emit staircase { dir = :ccw, steps = 8 }
        wait 4
    end

    section "climax_222" at ~t"2:22" do
        trigger :ringburst { count = 3, duration = 1.4 }
        emit cubes    { layers = 5, dir = :cw }
        wait 4
        emit pinwheel { spokes = 6 }
        wait 3
    end

    section "climax_burn" at ~t"2:26" do
        trigger :pulse
            |> :tilt      { angle = -8, duration = 6.0 }
            |> :spin      { rate = -2.0, duration = 6.0 }
            |> :speedwarp { cursor = 1.15, duration = 8.0 }
        emit staircase { dir = :ccw, steps = 8 }
        wait 5
        emit doubleBar { spacing = 2 }
        wait 3
    end

    // 2:34 breakdown.
    section "breath_one" at ~t"2:34.500" do
        trigger :speedwarp { walls = 0.40, rotation = 0.50, duration = 8.0 }
            |> :grayscale  { strength = 0.75, duration = 8.0 }
            |> :fog        { near = 0.20, far = 0.80, duration = 12.0 }
            |> :zoom       { target = 0.85, anim = :ease_in_out, duration = 4.0 }
        emit bar { thickness = 0.7 }
        wait 5
    end

    section "breath_two" at ~t"2:42" do
        emit alternate { parity = :even, thickness = 0.7 }
        wait 4
        emit bar       { thickness = 0.7 }
        wait 4
    end

    section "rebuild" at ~t"2:46" do
        trigger :hueShift  { rate = -0.5, duration = 12.0 }
            |> :outline    { thickness = 0.7, duration = 10.0 }
            |> :speedwarp  { walls = 0.85, cursor = 1.05, duration = 8.0 }
        emit ladder { rungs = 5 }
        wait 3
        emit spiral { dir = :cw, loops = 2, thickness = 0.85 }
        wait 3
    end

    section "rising_again" at ~t"2:52" do
        trigger :speedwarp { walls = 1.05, cursor = 1.10, duration = 14.0 }
            |> :hueShift   { rate = 0.6, duration = 14.0 }
            |> :zoom       { target = 1.0, anim = :ease_in_out, duration = 3.0 }
        emit pinwheel  { spokes = 4 }
        wait 3
        emit doubleBar { spacing = 2 }
        wait 3
        emit spiral    { dir = :ccw, loops = 3 }
        wait 4
    end

    section "pre_drop_two" at ~t"3:06" do
        trigger :freeze   { duration = 0.5 }
            |> :grayscale { strength = 0.3, duration = 4.0 }
        emit bar { thickness = 0.85 }
        wait 3
    end

    // 3:10.5 second drop.
    section "drop_two" at ~t"3:10.500" do
        trigger :bassdrop  { strength = 1.0, duration = 1.8 }
            |> :flip
            |> :speedwarp  { walls = 1.30, rotation = 1.20, cursor = 1.20, duration = 16.0 }
            |> :ringburst  { count = 5, duration = 2.2 }
            |> :centerburst { strength = 1.1, duration = 0.6 }
            |> :hueShift   { rate = 0.75, duration = 16.0 }
        emit doubleBar { spacing = 2, thickness = 1.0 }
        wait 1
    end

    section "fire_314" at ~t"3:14" do
        trigger :flip |> :shake { strength = 0.5, duration = 0.6 }
        emit staircase { dir = :ccw, steps = 10 }
        wait 6
    end

    // Was: corridor length=1.5 turns=5. Replaced.
    section "groove_c1" at ~t"3:23" do
        trigger :speedwarp { cursor = 1.15, duration = 18.0 }
        emit staircase { dir = :cw, steps = 8 }
        wait 5
        emit pinwheel  { spokes = 6 }
        wait 3
        emit doubleBar { spacing = 2 }
        wait 3
    end

    section "groove_c2" at ~t"3:38" do
        trigger :spin     { rate = -1.5, duration = 8.0 }
            |> :hueShift  { rate = -0.4, duration = 10.0 }
        emit cubes     { layers = 6, dir = :ccw }
        wait 4
        emit staircase { dir = :cw, steps = 8 }
        wait 4
        emit alternate { parity = :odd }
        wait 3
    end

    // Was: corridor length=1.6 turns=6. Replaced.
    section "ramp_to_peak" at ~t"3:50" do
        trigger :speedwarp { walls = 1.20, rotation = 1.15, cursor = 1.20, duration = 20.0 }
            |> :tilt       { angle = 6, pitch = 5, duration = 16.0 }
            |> :outline    { thickness = 0.7, duration = 18.0 }
            |> :hueShift   { rate = 0.7, duration = 18.0 }
        emit staircase { dir = :ccw, steps = 10 }
        wait 6
        emit pinwheel  { spokes = 6 }
        wait 3
        emit cubes     { layers = 5, dir = :cw }
        wait 4
    end

    section "climax_pre" at ~t"4:05.500" do
        trigger :flip
            |> :speedwarp  { walls = 1.40, rotation = 1.25, cursor = 1.25, duration = 20.0 }
            |> :spin       { rate = 2.5, duration = 20.0 }
            |> :hueShift   { rate = 0.95, duration = 22.0 }
            |> :ringburst  { count = 4, duration = 1.6 }
        emit staircase { dir = :cw, steps = 12, thickness = 1.0 }
        wait 6
    end

    // Was: corridor length=1.5 turns=6. Replaced.
    section "climax_408" at ~t"4:08" do
        trigger :bassdrop   { strength = 0.95, duration = 1.2 }
            |> :centerburst { strength = 0.95, duration = 0.5 }
        emit cubes     { layers = 6, dir = :ccw }
        wait 4
        emit staircase { dir = :cw, steps = 10 }
        wait 5
    end

    section "climax_413" at ~t"4:13" do
        trigger :flip |> :shake { strength = 0.55, duration = 0.7 }
        emit staircase { dir = :ccw, steps = 12 }
        wait 5
    end

    // 4:17.811 absolute peak. Walls 1.50 with cursor 1.30
    // keeps the gap reachable even at maximum effect stack.
    section "ABSOLUTE_PEAK" at ~t"4:17" do
        trigger :bassdrop    { strength = 1.0, duration = 1.6 }
            |> :flip
            |> :speedwarp    { walls = 1.50, rotation = 1.35, cursor = 1.30, duration = 8.0 }
            |> :ringburst    { count = 6, duration = 2.0 }
            |> :centerburst  { strength = 1.2, duration = 0.7 }
            |> :shockwave    { strength = 1.2, duration = 1.0 }
            |> :shake        { strength = 0.75, duration = 1.2 }
            |> :zoom         { target = 1.20, anim = :bounce, duration = 1.5 }
        emit cubes     { layers = 7, dir = :ccw }
        wait 4
        emit staircase { dir = :cw, steps = 10 }
        wait 4
    end

    // Was: corridor length=1.5 turns=5. Replaced.
    section "post_peak_421" at ~t"4:21" do
        emit staircase { dir = :ccw, steps = 8 }
        wait 4
        emit pinwheel  { spokes = 6 }
        wait 3
    end

    section "post_peak_425" at ~t"4:25" do
        trigger :flip |> :glitch { strength = 0.5, duration = 0.8 }
        emit doubleBar { spacing = 2 }
        wait 3
        emit cubes     { layers = 5, dir = :cw }
        wait 4
    end

    section "tail_climax" at ~t"4:28" do
        trigger :centerburst { strength = 0.85, duration = 0.5 }
            |> :tilt         { angle = -7, duration = 5.0 }
            |> :ringburst    { count = 3, duration = 1.2 }
        emit staircase { dir = :ccw, steps = 8 }
        wait 4
    end

    // Was: corridor length=1.4 turns=4. Replaced.
    section "winding" at ~t"4:33" do
        trigger :hueShift { rate = -0.6, duration = 12.0 }
            |> :spin      { rate = 1.2, duration = 8.0 }
        emit staircase { dir = :cw, steps = 6 }
        wait 5
        emit pinwheel  { spokes = 5 }
        wait 3
        emit alternate { parity = :odd }
        wait 3
    end

    section "almost_done" at ~t"4:42" do
        emit ladder    { rungs = 6 }
        wait 3
        emit doubleBar { spacing = 2 }
        wait 3
    end

    section "fade_out" at ~t"4:46.500" do
        trigger :speedwarp { walls = 0.40, rotation = 0.40, duration = 5.0 }
            |> :grayscale  { strength = 0.85, duration = 5.0 }
            |> :fog        { near = 0.10, far = 0.60, duration = 5.0 }
            |> :zoom       { target = 0.65, anim = :ease_in_out, duration = 3.5 }
        emit bar { thickness = 0.7 }
        wait 3
    end

    section "silence" at ~t"4:49.500" do
        trigger :freeze { duration = 1.5 }
    end
end
"#;

// V3_DEMO exercises every v3-specific post trigger so an
// author can see each effect in isolation. Superset of the
// old V2_DEMO, which was removed alongside the v2 parser.
const V3_DEMO: &str = r#"
#[use_v3]
#[timestamp_format_use_tracklength]

level "EFFECTS DEMO" do
    meta do
        bpm   = 128
        music = ~p"assets/music/nostalgia.qoa"
    end

    palette    do bgA = rgb(0.1, 0.05, 0.15) end
    difficulty do range = :Rookie..:Expert, base = :Casual end
    generation do sides = 4, seed = 0x1 end

    section "warmup"     at ~t"0:00" do emit bar end
    section "fog"        at ~t"0:08" do trigger :fog { near = 0.2, far = 0.7, duration = 6.0 } end
    section "spin"       at ~t"0:16" do trigger :spin { rate = 1.2, duration = 5.0 } end
    section "bounce"     at ~t"0:24" do trigger :bounce { amplitude = 0.4, duration = 8.0 } end
    section "shockwave"  at ~t"0:32" do trigger :shockwave { strength = 1.0 } end
    section "punch"      at ~t"0:40" do trigger :zoom_punch { strength = 0.5 } end
    section "freeze"     at ~t"0:48" do trigger :freeze { duration = 0.5 } end
    section "invert"     at ~t"0:56" do trigger :invert_colors { duration = 2.0 } end
    section "gray"       at ~t"1:04" do trigger :grayscale { strength = 1.0, duration = 3.0 } end
    section "outline"    at ~t"1:12" do trigger :outline { thickness = 0.9, duration = 4.0 } end
    section "bursts"     at ~t"1:20" do
        trigger :centerburst { strength = 1.0 }
            |> :ringburst { count = 4, duration = 1.5 }
    end
    section "bassdrop"   at ~t"1:28" do trigger :bassdrop { strength = 1.0, duration = 1.2 } end
end
"#;

/// Exhaustive showcase level. Every runtime trigger, every
/// rule category, every rule stack operation (`rule`,
/// `revert`, `push`, `pop`), every file level rule
/// directive, and the user post shader pipeline are
/// exercised at least once across its timeline.
///
/// Emit patterns are deliberately kept to a single
/// placeholder `emit bar` per section. The point of this
/// file is to audit effects, not to demonstrate obstacle
/// variety; the stock stock roster already covers that.
///
/// Section timeline is authored against absolute track
/// seconds (`#[timestamp_format_use_tracklength]`) so an
/// author can seek the audio file to a known offset and
/// watch exactly one effect at a time.
///
/// Shader declarations reference files under
/// `assets/shaders/user/`. If the sandbox is disabled or
/// the files are missing, the corresponding `post_shader`
/// triggers become no ops at runtime (the renderer falls
/// back to the default post pipeline), but the parser and
/// the generator still accept every form below.
pub const EVERYTHING: &str = r#"
#[use_v3]
#[timestamp_format_use_tracklength]
#[startfrom ~t"0:00"]
#[ignore_collisions]

// File level rule baselines. Any later `rule <cat> { ... }`
// statement overrides these in place, and `revert <cat>`
// returns the engine here.
#[ability  { kind = :dash, cooldown = 1.2, slots_per_dash = 1 }]
#[vision   { range = 6.0, fog_near = 4.0, fog_far = 5.0 }]
#[cursor   { speed_mult = 1.0, width_mult = 1.0, count = 1 }]
#[survival { lives = 1, soft_death = false }]
#[input    { delay_ms = 0, discrete = false, noise = 0.0, inverted = false }]
#[score    { multiplier = 1.0, close_call_bonus = 50, survival_per_second = 10 }]

level "EVERYTHING" do
    meta do
        subtitle    = "FULL EFFECT SHOWCASE"
        author      = "eeeeee"
        song        = "DEMO"
        bpm         = 128
        music       = ~p"assets/music/hexagon2.qoa"
        description = "One section per trigger plus rule ops, stack ops, shaders, and hooks."
    end

    palette do
        bgA        = rgb(0.10, 0.04, 0.16)
        bgB        = rgb(0.05, 0.02, 0.10)
        centerFill = rgb(0.05, 0.02, 0.08)
        centerRing = rgb(1.00, 0.45, 0.75)
        wall       = rgb(1.00, 0.45, 0.75)
        player     = rgb(1.00, 0.95, 1.00)
        accent     = rgb(1.00, 0.80, 0.92)
    end

    difficulty do range = :Rookie..:ExpertPlus, base = :Casual end

    generation do
        sides    = 6
        seed     = 0xDE401234
        hueSpeed = 0.10
        speed    = 1.00
        density  = 1.00
    end

    global do
        var base_thick :: f32 = 1.0 [pub]
        var spin_rate  :: f32 = 1.8 [pub]
    end

    // User shader declarations. Each one is bound to an
    // atom name that `post_shader` triggers reference.
    // Paths are read by the sandbox when the trigger fires.
    shader :wave_glow    = ~p"assets/shaders/user/wave_glow.shader"
    shader :scanline_pro = ~p"assets/shaders/user/scanline_pro.shader"
    shader :pixelate     = ~p"assets/shaders/user/pixelate.shader"
    shader :chroma_split = ~p"assets/shaders/user/chroma_split.shader"

    // Classic triggers: flip, pulse, tilt variants,
    // legacy speedMult and hueShift.

    section "flip" at ~t"0:00" do
        trigger :flip
        emit bar
        wait 4
    end

    section "pulse" at ~t"0:04" do
        trigger :pulse
        emit bar
        wait 4
    end

    section "tilt_timed" at ~t"0:08" do
        // Finite duration: camera eases back to level.
        trigger :tilt { angle = 15, pitch = 10, yaw = 5, duration = 3.0 }
        emit bar
        wait 4
    end

    section "tilt_sticky" at ~t"0:12" do
        // No duration field: tilt holds until overridden.
        trigger :tilt { angle = -10, pitch = 0, yaw = 0 }
        emit bar
        wait 4
    end

    section "speed_mult_burst" at ~t"0:16" do
        trigger :speedMult { factor = 1.5, duration = 3.0 }
        emit bar
        wait 4
    end

    section "speed_mult_slow" at ~t"0:20" do
        trigger :speedMult { factor = 0.7, duration = 2.5 }
        emit bar
        wait 4
    end

    section "hue_shift" at ~t"0:24" do
        trigger :hueShift { rate = 1.2, duration = 4.0 }
        emit bar
        wait 4
    end

    // Modern triggers: all four axes of speedwarp,
    // glitch, shake, every zoom easing, invert, strobe.

    section "speedwarp_all_axes" at ~t"0:28" do
        trigger :speedwarp {
            walls    = 1.4,
            rotation = 1.2,
            cursor   = 1.1,
            music    = 1.05,
            duration = 3.5
        }
        emit bar
        wait 4
    end

    section "speedwarp_walls_only" at ~t"0:32" do
        // Only the walls axis is overridden. Other axes
        // keep their current values, which is how piped
        // speedwarps layer independently.
        trigger :speedwarp { walls = 0.6, duration = 2.0 }
        emit bar
        wait 4
    end

    section "speedwarp_halt" at ~t"0:36" do
        // Explicit zero halts the axis for the duration.
        trigger :speedwarp { walls = 0.0, duration = 1.5 }
        emit bar
        wait 4
    end

    section "glitch" at ~t"0:40" do
        trigger :glitch { strength = 0.8, duration = 1.2 }
        trigger :morph { sides = 4, duration = 2.0 }
        emit bar
        wait 4
    end

    section "shake" at ~t"0:44" do
        trigger :shake { strength = 1.0, duration = 1.0 }
        trigger :morph { sides = 12, duration = 2.0 }
        emit bar
        wait 4
    end

    section "zoom_linear" at ~t"0:48" do
        trigger :zoom { target = 1.4, anim = :linear, duration = 1.0 }
        emit bar
        wait 4
    end

    section "zoom_ease_in" at ~t"0:52" do
        trigger :morph { sides = 6, duration = 2.0 }
        trigger :zoom { target = 0.8, anim = :ease_in, duration = 1.5 }
        emit bar
        wait 4
    end

    section "zoom_ease_out" at ~t"0:56" do
        trigger :zoom { target = 1.2, anim = :ease_out, duration = 1.5 }
        emit bar
        wait 4
    end

    section "zoom_ease_in_out" at ~t"1:00" do
        trigger :zoom { target = 1.0, anim = :ease_in_out, duration = 1.5 }
        emit bar
        wait 4
    end

    section "zoom_bounce" at ~t"1:04" do
        trigger :zoom { target = 1.3, anim = :bounce, duration = 2.0 }
        emit bar
        wait 4
    end

    section "invert_input" at ~t"1:08" do
        trigger :invert { duration = 3.0 }
        emit bar
        wait 4
    end

    section "strobe_repeating" at ~t"1:12" do
        trigger :strobe { rate = 8.0, duration = 1.2 }
        emit bar
        wait 4
    end

    section "strobe_one_shot" at ~t"1:16" do
        // Rate 0 collapses to a single cosine fade.
        trigger :strobe { rate = 0.0, duration = 0.6 }
        emit bar
        wait 4
    end

    // v3 camera and motion triggers: spin, bounce,
    // freeze, zoom_punch.

    section "spin" at ~t"1:20" do
        trigger :spin { rate = 2.0, duration = 3.0 }
        emit bar
        wait 4
    end

    section "bounce" at ~t"1:24" do
        // Injects trauma on every detected onset while
        // active. No shake strength, no duration of its
        // own: the effect rides the audio beat.
        trigger :bounce { amplitude = 0.5, duration = 4.0 }
        emit bar
        wait 4
    end

    section "freeze" at ~t"1:28" do
        trigger :freeze { duration = 0.5 }
        emit bar
        wait 4
    end

    section "zoom_punch" at ~t"1:32" do
        trigger :zoom_punch { strength = 0.6, duration = 0.5 }
        emit bar
        wait 4
    end

    // v3 post process triggers: invert_colors,
    // grayscale, shockwave, fog, outline.

    section "invert_colors" at ~t"1:36" do
        trigger :invert_colors { duration = 2.0 }
        emit bar
        wait 4
    end

    section "grayscale" at ~t"1:40" do
        trigger :grayscale { strength = 1.0, duration = 3.0 }
        emit bar
        wait 4
    end

    section "shockwave" at ~t"1:44" do
        trigger :shockwave { strength = 1.2, duration = 1.0 }
        emit bar
        wait 4
    end

    section "fog" at ~t"1:48" do
        trigger :fog { near = 0.2, far = 0.8, duration = 5.0 }
        emit bar
        wait 4
    end

    section "outline" at ~t"1:52" do
        trigger :outline { thickness = 1.0, duration = 4.0 }
        emit bar
        wait 4
    end

    // v3 burst triggers: one shot particle spawns plus
    // the composite bassdrop.

    section "centerburst" at ~t"1:56" do
        trigger :centerburst { strength = 1.2, duration = 0.6 }
        emit bar
        wait 4
    end

    section "ringburst" at ~t"2:00" do
        trigger :ringburst { count = 5, duration = 2.0 }
        emit bar
        wait 4
    end

    section "bassdrop" at ~t"2:04" do
        // Composite: zoom_punch + shake + shockwave + flash
        // all at once, scaled by strength.
        trigger :bassdrop { strength = 1.0, duration = 1.2 }
        emit bar
        wait 4
    end

    // Trigger pipes. Every element fires at the same
    // scheduling anchor, so the whole chain lands on one
    // beat. Useful for layered drops.

    section "pipe_short" at ~t"2:08" do
        trigger :flip |> :shake { strength = 0.7, duration = 0.6 }
        emit bar
        wait 4
    end

    section "pipe_long" at ~t"2:12" do
        trigger :flip
            |> :speedwarp { walls = 1.6, rotation = 1.4, duration = 3.0 }
            |> :zoom      { target = 1.3, anim = :ease_out, duration = 1.0 }
            |> :shake     { strength = 0.5, duration = 0.8 }
            |> :glitch    { strength = 0.4, duration = 1.0 }
        emit bar
        wait 6
    end

    // User post shaders. Each trigger hands a slot and
    // four scalar parameters to the sandbox. p0..p3 go
    // into the shader's push constant block in
    // declaration order.

    section "shader_wave" at ~t"2:20" do
        trigger :post_shader {
            shader = :wave_glow,
            p0 = 1.00, p1 = 0.50, p2 = 0.25, p3 = 0.00
        }
        emit bar
        wait 4
    end

    section "shader_scanline" at ~t"2:24" do
        trigger :post_shader {
            shader = :scanline_pro,
            p0 = 0.80, p1 = 240.0, p2 = 0.15, p3 = 0.00
        }
        emit bar
        wait 4
    end

    section "shader_pixelate" at ~t"2:28" do
        trigger :post_shader {
            shader = :pixelate,
            p0 = 8.00, p1 = 0.00, p2 = 0.00, p3 = 0.00
        }
        emit bar
        wait 4
    end

    section "shader_chroma" at ~t"2:32" do
        trigger :post_shader {
            shader = :chroma_split,
            p0 = 0.60, p1 = 0.50, p2 = 1.00, p3 = 0.00
        }
        emit bar
        wait 4
    end

    section "shader_off" at ~t"2:36" do
        // Returns the post pipeline to the engine default.
        trigger :post_shader_off
        emit bar
        wait 4
    end

    // Rule operations: one section per category to show
    // how `rule <cat> { ... }` and `revert <cat>` pair.

    section "rule_ability" at ~t"2:40" do
        rule ability {
            kind           = :shield,
            charges        = 3,
            recharge       = 8.0,
            invuln         = 0.6,
            cooldown       = 1.0,
            slots_per_dash = 1,
            slowmo_factor  = 0.45,
            slowmo_cap     = 3.0,
            slowmo_recover = 6.0
        }
        emit bar
        wait 4
        revert ability
    end

    section "rule_vision" at ~t"2:44" do
        rule vision {
            range                 = 2.0,
            fog_near              = 0.8,
            fog_far               = 1.8,
            strobe                = false,
            strobe_rate           = 4.0,
            blind_duration        = 0.2,
            blind_frequency       = 0.5,
            hide_camera_indicator = true
        }
        emit bar
        wait 4
        revert vision
    end

    section "rule_cursor" at ~t"2:48" do
        rule cursor {
            speed_mult        = 1.5,
            width_mult        = 0.75,
            count             = 2,
            angular_offset    = 3.14159,
            centripetal_drift = 0.2
        }
        emit bar
        wait 4
        revert cursor
    end

    section "rule_survival" at ~t"2:52" do
        rule survival {
            lives               = 3,
            soft_death          = true,
            pushback_seconds    = 1.2,
            invuln_after_hit    = 0.6,
            checkpoints_enabled = true
        }
        emit bar
        wait 4
        revert survival
    end

    section "rule_input" at ~t"2:56" do
        rule input {
            delay_ms = 80,
            discrete = false,
            noise    = 0.15,
            inverted = true
        }
        emit bar
        wait 4
        revert input
    end

    section "rule_score" at ~t"3:00" do
        rule score {
            multiplier          = 2.0,
            close_call_bonus    = 100,
            survival_per_second = 25
        }
        emit bar
        wait 4
        revert score
    end

    // Rule stack operations. `push` saves the current
    // state of a category, `pop` restores it. Multiple
    // pushes on the same category nest.

    section "rule_stack_nested" at ~t"3:04" do
        push ability
        rule ability { kind = :slowmo, slowmo_factor = 0.35 }
        emit bar
        wait 2

        push ability
        rule ability { kind = :dash, cooldown = 0.6 }
        emit bar
        wait 2

        // First pop restores the slowmo override, because
        // the inner push captured exactly that state.
        pop ability
        emit bar
        wait 2

        // Second pop restores the file level baseline.
        pop ability
        emit bar
        wait 2
    end

    section "revert_all_at_once" at ~t"3:12" do
        // Multiple categories overridden together, then
        // reset in one go. Clears every rule stack along
        // the way.
        rule ability { kind = :shield }
        rule vision  { range = 1.5 }
        rule cursor  { count = 2 }
        emit bar
        wait 2
        revert all
        emit bar
        wait 2
    end

    // Event hooks. Only `trigger :x` statements are
    // allowed inside an `@on` body; the generator wires
    // them to the hook's event source at section entry.

    section "hook_onset_every" at ~t"3:18" do
        @on :onset every 2 do
            trigger :shake { strength = 0.3, duration = 0.2 }
        end
        emit bar
        wait 8
    end

    section "hook_onset_every_one" at ~t"3:26" do
        // Bare `@on :onset` without `every` defaults to
        // firing on every detected onset.
        @on :onset do
            trigger :pulse
        end
        emit bar
        wait 6
    end

    section "hook_onset_window" at ~t"3:32" do
        @on :onset between ~t"3:32"..~t"3:38" do
            trigger :centerburst { strength = 0.6, duration = 0.3 }
        end
        emit bar
        wait 6
    end

    section "hook_downbeat" at ~t"3:38" do
        // Optional field block is accepted for forward
        // compatibility but currently ignored beyond
        // documenting the rhythm.
        @on :beat { rhythm = :downbeat } do
            trigger :strobe { rate = 0.0, duration = 0.2 }
        end
        emit bar
        wait 8
    end

    section "hook_section_enter" at ~t"3:46" do
        @on :section_enter "hook_section_enter" do
            trigger :flip
            trigger :bassdrop { strength = 0.7, duration = 0.8 }
        end
        emit bar
        wait 6
    end

    section "hook_close_call_count" at ~t"3:52" do
        @on :close_call above 3 do
            trigger :glitch { strength = 0.6, duration = 0.5 }
        end
        emit bar
        wait 6
    end

    // Clean outro: drop the shader, reset every rule,
    // ease zoom back to neutral.

    section "outro" at ~t"4:00" do
        trigger :post_shader_off
        revert all
        trigger :zoom { target = 1.0, anim = :ease_in_out, duration = 2.0 }
        emit bar
        wait 8
    end
end
"#;