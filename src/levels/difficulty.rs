//! Strict difficulty tier system.
//!
//! The game exposes six named tiers, from easiest to hardest:
//!
//! | rank | tier      | display            |
//! |------|-----------|--------------------|
//! |  0   | Rookie    | ROOKIE             |
//! |  1   | Casual    | CASUAL             |
//! |  2   | Adept     | ADEPT              |
//! |  3   | Skilled   | SKILLED            |
//! |  4   | Expert    | EXPERT             |
//! |  5   | ExpertPlus{1} | EXPERT+        |
//!
//! The top tier (`ExpertPlus`) is parameterized so it can be
//! stacked up to `ExpertPlus{4}` (displayed as `EXPERT 4+`). This
//! matches the brief ("Expert+ with ability to add pluses, up to
//! Expert 4+"). Ranks beyond 5 use `5 + plus_minus_one` so sorting
//! stays monotonic.
//!
//! Each tier carries three multipliers that together fully
//! determine the feel of the run:
//!
//! * [`wall_speed_mult`]     — how fast walls approach the center.
//! * [`spawn_density_mult`]  — how often the generator may emit.
//! * [`player_speed_mult`]   — how fast the cursor orbits.
//!
//! All three scale up together with rank. BPM from the song
//! interacts multiplicatively with this table at the generator
//! level (walls per beat stays the same; more BPM means more
//! walls per second without rescaling the whole engine).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Rookie,
    Casual,
    Adept,
    Skilled,
    Expert,
    /// `plus` is 1 for "Expert+", 2 for "Expert 2+", up to 4.
    ExpertPlus { plus: u8 },
}

/// Fixed ordering of the six stock tiers, used by the catalogue
/// to build a level's available difficulty picker.
pub const TIER_ORDER: &[Tier] = &[
    Tier::Rookie,
    Tier::Casual,
    Tier::Adept,
    Tier::Skilled,
    Tier::Expert,
    Tier::ExpertPlus { plus: 1 },
];

impl Tier {
    /// Total ordering for the tier. Lower is easier. `ExpertPlus{n}`
    /// maps to `5 + (n - 1)`, so `ExpertPlus{1}` is rank 5 and
    /// `ExpertPlus{4}` is rank 8.
    pub fn rank(&self) -> i32 {
        match self {
            Tier::Rookie          => 0,
            Tier::Casual          => 1,
            Tier::Adept           => 2,
            Tier::Skilled         => 3,
            Tier::Expert          => 4,
            Tier::ExpertPlus { plus } => 5 + (plus.saturating_sub(1)) as i32,
        }
    }

    /// Human readable name used in the menu.
    pub fn display_name(&self) -> String {
        match self {
            Tier::Rookie  => "ROOKIE".into(),
            Tier::Casual  => "CASUAL".into(),
            Tier::Adept   => "ADEPT".into(),
            Tier::Skilled => "SKILLED".into(),
            Tier::Expert  => "EXPERT".into(),
            Tier::ExpertPlus { plus } => {
                if *plus <= 1 { "EXPERT+".into() }
                else          { format!("EXPERT {}+", plus) }
            }
        }
    }

    /// Parse a keyword as it appears in the DSL. Supports
    /// `Rookie`, `Casual`, `Adept`, `Skilled`, `Expert`,
    /// `ExpertPlus`, and `ExpertPlus2` .. `ExpertPlus4`.
    pub fn from_keyword(s: &str) -> Option<Tier> {
        match s {
            "Rookie"  => Some(Tier::Rookie),
            "Casual"  => Some(Tier::Casual),
            "Adept"   => Some(Tier::Adept),
            "Skilled" => Some(Tier::Skilled),
            "Expert"  => Some(Tier::Expert),
            "ExpertPlus"  | "ExpertPlus1" => Some(Tier::ExpertPlus { plus: 1 }),
            "ExpertPlus2" => Some(Tier::ExpertPlus { plus: 2 }),
            "ExpertPlus3" => Some(Tier::ExpertPlus { plus: 3 }),
            "ExpertPlus4" => Some(Tier::ExpertPlus { plus: 4 }),
            _ => None,
        }
    }

    /// Wall travel speed multiplier. The curve is calibrated so
    /// that `WALL_SPEED_BASE * wall_speed_mult * level.speed_mult`
    /// produces roughly:
    ///
    /// * Rookie    : 1.2 units/s (~3.3 s per wall)
    /// * Expert    : 2.6 units/s (~1.5 s per wall)
    /// * Expert+   : 3.1 units/s (~1.3 s per wall)
    /// * Expert 2+ : 3.5 units/s (~1.1 s per wall)
    /// * Expert 3+ : 4.0 units/s (~1.0 s per wall)
    /// * Expert 4+ : 4.4 units/s (~0.9 s per wall, matches
    ///               Super Hexagon's hardest timings)
    pub fn wall_speed_mult(&self) -> f32 {
        match self {
            Tier::Rookie  => 0.95,
            Tier::Casual  => 1.15,
            Tier::Adept   => 1.40,
            Tier::Skilled => 1.70,
            Tier::Expert  => 2.10,
            Tier::ExpertPlus { plus } => 2.10 + 0.45 * (*plus as f32),
        }
    }

    /// Cursor rotation speed multiplier. Scales less steeply than
    /// wall speed but must still keep pace at the top tiers or
    /// the player cannot cover the gap before the wall arrives.
    pub fn player_speed_mult(&self) -> f32 {
        match self {
            Tier::Rookie  => 0.95,
            Tier::Casual  => 1.05,
            Tier::Adept   => 1.15,
            Tier::Skilled => 1.25,
            Tier::Expert  => 1.35,
            Tier::ExpertPlus { plus } => 1.35 + 0.10 * (*plus as f32),
        }
    }

    /// Density multiplier: directly shrinks the cooldown between
    /// obstacles in the generator. >1 means "walls land faster
    /// than the DSL author's default", <1 means "more breathing
    /// room". At Expert 4+ the multiplier is high enough that
    /// most obstacles chain with zero idle beats in between.
    pub fn spawn_density_mult(&self) -> f32 {
        match self {
            Tier::Rookie  => 0.60,
            Tier::Casual  => 0.80,
            Tier::Adept   => 1.00,
            Tier::Skilled => 1.30,
            Tier::Expert  => 1.70,
            Tier::ExpertPlus { plus } => 1.70 + 0.40 * (*plus as f32),
        }
    }
}