//! Procedural level generation.
//!
//! Reads a parsed `LevelAst` and turns it into a stream of walls
//! and effects (camera flips, tilts, pulses, speed bursts, hue
//! shifts) driven by the audio onset clock from `crate::audio`.
//!
//! The generator is the only place allowed to spawn walls. All of
//! its outputs are validated for survivability before reaching the
//! simulation: `bar` always leaves exactly one gap, `custom` masks
//! are rejected at parse time if they do not, inter obstacle
//! cooldown keeps the player's angular reachability window from
//! closing. This replaces the ad hoc `Director` that used to live
//! in `game.rs`.

pub mod prng;
pub mod obstacle;
pub mod generator;

pub use generator::Generator;
pub use obstacle::WallSpec;