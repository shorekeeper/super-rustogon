//! Compile time fallback roster, written in RLFv2.
//!
//! These strings double as reference material for the new
//! syntax. They exercise every feature added in v2: flexible
//! key/value separators, `global`/`local` variable blocks,
//! `repeat` loops, rich triggers with field blocks, the new
//! obstacle patterns (`staircase`, `corridor`, `cubes`),
//! scriptable `formula` patterns, and the timestamp format
//! directive.

pub const EMBEDDED_LEVELS: &[&str] = &[
    HEXAGON,
    HEXAGONER,
    HEXAGONEST,
    HYPER_HEXAGON,
    HYPER_HEXAGONER,
    HYPER_HEXAGONEST,
    NOSTALGIA,
    V2_DEMO,
];

const HEXAGON: &str = r#"
level "HEXAGON" {
    meta {
        subtitle    = "BEGIN"
        author      = "Chipzel"
        song        = "COURTESY"
        bpm         = 130
        music       = "assets/music/hexagon1.qoa"
        description = "A gentle introduction. Walls come one at a time, gaps are obvious, the tempo is forgiving."
    }
    palette {
        bgA        rgb(0.10, 0.04, 0.16)
        bgB        rgb(0.05, 0.02, 0.10)
        centerFill rgb(0.05, 0.02, 0.08)
        centerRing rgb(1.00, 0.40, 0.70)
        wall       rgb(1.00, 0.40, 0.70)
        player     rgb(1.00, 0.95, 1.00)
        accent     rgb(1.00, 0.40, 0.70)
    }
    difficulty { range Rookie..Expert, base Rookie }
    generation { sides: 6, seed: 0x12345678, speed: 1.0, density: 1.0 }

    global {
        var base_thick: f32 = 1.0 [pub]
    }

    section "intro" at 0.00 {
        local { var burst: f32 = 2.0 }

        emit bar { thickness: @base_thick }
        emit spiral { dir cw }
        emit bar
        emit alternate { parity even }
        emit doubleBar { spacing: @burst }
    }
    section "mid" at 0.30 {
        trigger tilt { angle 1.2 }
        emit rainbow { dir cw }
        emit staircase { dir cw, steps 6 }
        emit doubleBar { spacing: 2 }
        emit spiral { dir ccw }
    }
    section "finale" at 0.70 {
        trigger flip
        trigger shake { strength 0.4, duration 0.5 }
        emit ladder { rungs 6 }
        emit rainbow { dir ccw }
        emit pinwheel { spokes 3 }
        emit bar
    }
}
"#;

const HEXAGONER: &str = r#"
level "HEXAGONER" {
    meta {
        subtitle    = "HARDER"
        author      = "Chipzel"
        song        = "OTIS"
        bpm         = 135
        music       = "assets/music/hexagon2.qoa"
        description = "Spirals and staggered bars. The world spins a little harder now; commit early."
    }
    palette {
        bgA        rgb(0.04, 0.10, 0.16)
        bgB        rgb(0.02, 0.05, 0.10)
        centerFill rgb(0.02, 0.05, 0.08)
        centerRing rgb(0.40, 0.85, 1.00)
        wall       rgb(0.40, 0.85, 1.00)
        player     rgb(0.95, 1.00, 1.00)
        accent     rgb(0.40, 0.85, 1.00)
    }
    difficulty { range Rookie..Expert, base Casual }
    generation { sides 6, seed 2882400001, hueSpeed 0.20, speed 1.10, density 1.05 }

    section "intro" at 0.00 {
        repeat 2 {
            emit spiral { dir cw, loops 2 }
            emit pinwheel { spokes 3 }
        }
        emit rainbow { dir cw }
        emit doubleBar { spacing 2 }
    }
    section "storm" at 0.35 {
        trigger flip
        trigger speedwarp { walls 1.2, rotation 1.1, musicScale 1.05, duration 3.5 }
        emit staircase { dir cw, steps 8 }
        emit spiral { dir ccw, loops 2 }
        emit cubes { layers 4 }
        emit pinwheel { spokes 4 }
    }
    section "finale" at 0.75 {
        trigger tilt -1.8
        trigger glitch { strength 0.4, duration 1.0 }
        emit rainbow { dir ccw }
        emit tunnel { length 1.0, lanes 3 }
        emit ladder { rungs 8 }
        emit pinwheel { spokes 4 }
    }
}
"#;

const HEXAGONEST: &str = r#"
level "HEXAGONEST" {
    meta {
        subtitle    = "HARDEST"
        author      = "Chipzel"
        song        = "FOCUS"
        bpm         = 174
        music       = "assets/music/hexagon3.qoa"
        description = "Everything you have learned, faster. Pinwheels start appearing between the bars."
    }
    palette {
        bgA        rgb(0.04, 0.16, 0.06)
        bgB        rgb(0.02, 0.08, 0.03)
        centerFill rgb(0.02, 0.08, 0.03)
        centerRing rgb(0.55, 1.00, 0.40)
        wall       rgb(0.55, 1.00, 0.40)
        player     rgb(1.00, 1.00, 0.95)
        accent     rgb(0.55, 1.00, 0.40)
    }
    difficulty { range Casual..ExpertPlus, base Adept }
    generation { sides 6, seed 1337420, hueSpeed 0.35, speed 1.20, density 1.15 }

    section "intro" at 0.00 {
        emit rainbow { dir cw }
        emit pinwheel { spokes 4 }
        emit staircase { dir cw, steps 8 }
        emit spiral { dir cw, loops 3 }
    }
    section "storm" at 0.35 {
        trigger flip
        trigger zoom { target 1.25, anim ease_out, duration 1.2 }
        emit corridor { length 1.2, turns 3 }
        emit rainbow { dir ccw }
        emit cubes { layers 5 }
        emit doubleBar { spacing 2 }
    }
    section "finale" at 0.75 {
        trigger pulse
        trigger tilt 2.2
        trigger zoom { target 1.0, anim ease_in_out, duration 1.5 }
        emit ladder { rungs 10 }
        emit corridor { length 1.4, turns 4 }
        emit rainbow { dir cw }
        emit pinwheel { spokes 5 }
    }
}
"#;

const HYPER_HEXAGON: &str = r#"
#[timestamp_format_use_relative]

level "HYPER HEXAGON" {
    meta {
        subtitle    = "HARDESTEST"
        author      = "Chipzel"
        song        = "COURTESY"
        bpm         = 130
        music       = "assets/music/hexagon1.qoa"
        description = "The hyper tier starts here. Walls keep coming, the camera keeps flipping."
    }
    palette {
        bgA        rgb(0.18, 0.08, 0.02)
        bgB        rgb(0.09, 0.04, 0.01)
        centerFill rgb(0.09, 0.04, 0.01)
        centerRing rgb(1.00, 0.65, 0.20)
        wall       rgb(1.00, 0.65, 0.20)
        player     rgb(1.00, 1.00, 0.95)
        accent     rgb(1.00, 0.65, 0.20)
    }
    difficulty { range Adept..ExpertPlus2, base Skilled }
    generation { sides 6, seed 112358, hueSpeed 0.50, speed 1.30, density 1.20 }

    global {
        var warp_duration: f32 = 3.0 [pub]
    }

    section "intro" at 0.00 {
        emit staircase { dir cw, steps 8 }
        emit ladder { rungs 6 }
        emit spiral { dir ccw, loops 3 }
        emit cubes { layers 4 }
    }
    section "mid" at 0.30 {
        trigger flip
        trigger speedwarp { walls 1.15, cursor 1.05, duration: @warp_duration }
        emit corridor { length 1.1, turns 3 }
        emit rainbow { dir ccw }
        emit pinwheel { spokes 5 }
        emit staircase { dir ccw, steps 8 }
    }
    section "finale" at 0.70 {
        trigger tilt -2.5
        trigger shake { strength 0.7, duration 1.0 }
        trigger strobe { rate 8, duration 1.2 }
        emit corridor { length 1.5, turns 4 }
        emit rainbow { dir cw }
        emit ladder { rungs 10 }
        emit cubes { layers 6 }
        emit pinwheel { spokes 5 }
    }
}
"#;

const HYPER_HEXAGONER: &str = r#"
level "HYPER HEXAGONER" {
    meta {
        subtitle    = "HARDERESTEST"
        author      = "Chipzel"
        song        = "OTIS"
        bpm         = 135
        music       = "assets/music/hexagon2.qoa"
        description = "Dense patterns, rapid camera flips, a palette that drifts under your feet."
    }
    palette {
        bgA        rgb(0.16, 0.04, 0.16)
        bgB        rgb(0.08, 0.02, 0.08)
        centerFill rgb(0.08, 0.02, 0.08)
        centerRing rgb(0.95, 0.45, 1.00)
        wall       rgb(0.95, 0.45, 1.00)
        player     rgb(1.00, 1.00, 1.00)
        accent     rgb(0.95, 0.45, 1.00)
    }
    difficulty { range Skilled..ExpertPlus3, base Expert }
    generation { sides 6, seed 2718281, hueSpeed 0.70, speed 1.40, density 1.30 }

    section "intro" at 0.00 {
        emit pinwheel { spokes 5 }
        emit staircase { dir cw, steps 10 }
        emit ladder { rungs 8 }
        emit corridor { length 1.0, turns 3 }
    }
    section "mid" at 0.25 {
        trigger flip
        trigger tilt 1.5
        trigger zoom { target 1.35, anim bounce, duration 1.8 }
        emit rainbow { dir ccw }
        emit cubes { layers 6 }
        emit corridor { length 1.4, turns 4 }
        emit ladder { rungs 10 }
    }
    section "finale" at 0.65 {
        trigger speedwarp { walls 1.25, rotation 1.2, musicScale 1.1, duration 5.0 }
        trigger glitch { strength 0.7, duration 1.4 }
        emit corridor { length 1.8, turns 5 }
        emit rainbow { dir cw }
        emit ladder { rungs 12 }
        emit pinwheel { spokes 6 }
        emit cubes { layers 7 }
    }
}
"#;

const HYPER_HEXAGONEST: &str = r#"
level "HYPER HEXAGONEST" {
    meta {
        subtitle    = "HARDESTESTEST"
        author      = "Chipzel"
        song        = "FOCUS"
        bpm         = 174
        music       = "assets/music/hexagon3.qoa"
        description = "The hardest stock level. No palette to hide behind: everything is stark white."
    }
    palette {
        bgA        rgb(0.14, 0.14, 0.16)
        bgB        rgb(0.06, 0.06, 0.08)
        centerFill rgb(0.06, 0.06, 0.08)
        centerRing rgb(1.00, 1.00, 1.00)
        wall       rgb(1.00, 1.00, 1.00)
        player     rgb(1.00, 1.00, 1.00)
        accent     rgb(1.00, 1.00, 1.00)
    }
    difficulty { range Expert..ExpertPlus4, base ExpertPlus }
    generation { sides 6, seed 141421356, hueSpeed 0.0, speed 1.55, density 1.45 }

    section "intro" at 0.00 {
        emit staircase { dir cw, steps 2 }
        
        emit ladder { rungs 8 }
        emit pinwheel { spokes 6 }
    }
    section "mid" at 0.25 {
        trigger flip
        trigger speedwarp { walls 1.3, cursor 1.1, duration 4.0 }
        emit corridor { length 1.5, turns 4 }
        emit rainbow { dir ccw }
        emit cubes { layers 7 }
        emit ladder { rungs 12 }
    }
    section "finale" at 0.65 {
        trigger pulse
        trigger tilt 2.5
        trigger shake { strength 0.9, duration 1.5 }
        trigger invert { duration 2.5 }
        emit corridor { length 2.0, turns 5 }
        emit rainbow { dir cw }
        emit ladder { rungs 14 }
        emit rainbow { dir ccw }
        emit pinwheel { spokes 6 }
        emit cubes { layers 8 }
    }
}
"#;

// NOSTALGIA shows off the formula pattern. `step % 2 == 0 &&
// slot != (step + phase) % sides` paints a walking staircase
// skipping every other step, which gives the player breathing
// room on alternate beats while keeping the advance readable.
const NOSTALGIA: &str = r#"
#[use_v2]
#[timestamp_format_use_tracklength]
#[startfrom ~t"0:38"]

level "NOSTALGIA" do
    meta do
        bpm   = 128
        music = ~p"assets/music/nostalgia.qoa"
    end

    palette    do bgA = rgb(0.1, 0.05, 0.15) end
    difficulty do range = :Rookie..:Expert, base = :Casual end
    generation do sides = 6, seed = 0x1 end

    section "intro"  at ~t"0:00"     do 
        // emit bar
    end

    section "going"  at ~t"0:13"     do
        trigger :shake { strength = 0.6, duration = 0.5 }
        // emit staircase { steps = 8 }
        // wait 7
        // emit ladder { rungs = 6 }
    end

    section "build"  at ~t"0:40"     do 
        trigger :zoom { duration = 2.1, target 2.0 }
        trigger :flip |> :speedwarp { walls = 0.8, music = 1.00, duration = 3.0 }
        trigger :shake |> :glitch { strength = 1.0, duration = 0.3 }
        emit ladder { rungs = 1 } 
    end

    section "run"  at ~t"0:43.800"     do 
        trigger :zoom { duration = 0.1, target 1.0, anim = :ease_out }
        trigger :shake |> :glitch { strength = 1.0, duration = 0.5 }
        trigger :speedwarp { walls = 1.6, rotation = 1.6, cursor = 1.6, music = 1.0, duration = 4.0 }
        wait 4
        emit ladder { rungs = 2 } 
        emit staircase { dir = :cw, steps = 8 }
    end

    section "drop"   at ~t"1:30"     do
        trigger :flip |> :shake { strength = 0.6, duration = 0.5 }
        emit staircase { dir = :cw, steps = 8 }
    end
    section "bridge" at ~t"3:15.500" do emit corridor { length = 1.5, turns = 4 } end
    section "outro"  at ~t"5:00"     do emit pinwheel { spokes = 6 } end
end
"#;

// Demonstration of every v2 syntax fingerprint in one file. Kept
// embedded so anyone testing the game can see the new dialect
// actually working without authoring a new .rlf first. The
// gameplay itself is a short remix of HEXAGONER.
const V2_DEMO: &str = r#"
#[use_v2]

level "V2 DEMO" do
    meta do
        subtitle    = "SYNTAX SHOWCASE"
        author      = "Rustogon"
        song        = "OTIS"
        bpm         = 135
        music       = ~p"assets/music/hexagon2.qoa"
        description = "Atoms, sigils, pipes, chains, where bindings. Everything the v2 parser understands, on one ring."
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

    difficulty do
        range = :Casual..:ExpertPlus2
        base  = :Adept
    end

    generation do
        sides   = 6
        seed    = 0x2718281
        speed   = 1.2
        density = 1.15
    end

    global do
        var base_thick :: f32 = 1.0 [pub]
        var opener     :: i32 = 6   [read]
    end

    section "intro" at 0.00 where lead = 8 do
        emit staircase { dir = :cw, steps = lead, thickness = @base_thick }
        emit spiral    { dir = :ccw, loops = 2 } >> :thickness { mult = 1.2 }
        emit ladder    { rungs = opener }
    end

    section "storm" at 0.35 do
        trigger :flip |> :speedwarp { walls = 1.2, music = 1.05, duration = 3.0 }
                      |> :zoom      { target = 1.2, anim = :ease_out, duration = 1.0 }

        repeat 2 do
            emit corridor { length = 1.2, turns = 3, dir = :cw }
            emit cubes    { layers = 5, dir = :ccw }
        end

        emit formula {
            formula = ~f"step % 2 == 0 && slot != (step + phase) % sides"
            steps   = 8
        } >> :thickness { mult = 0.95 }
    end

    section "finale" at 0.70 where rungs = 12 do
        trigger :tilt { angle = 2.2 } |> :shake  { strength = 0.6, duration = 0.8 }
                                      |> :strobe { rate = 8, duration = 1.0 }

        emit rainbow  { dir = :ccw }
        emit ladder   { rungs = rungs }
        emit pinwheel { spokes = 6 }
    end
end
"#;