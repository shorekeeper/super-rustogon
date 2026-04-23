//! Visual effects engine.
//!
//! Houses the systems that do not belong to a specific game object:
//!
//! * [`particles`] — fire-and-forget particle pool.
//! * [`shake`] — camera / screen shake accumulator.
//!
//! The split means gameplay code can push events (a close call,
//! a wall break, a death shatter) and let this module own the
//! lifetime / rendering of the resulting visuals.

pub mod particles;
pub mod shake;

pub use particles::{Particle, ParticleSystem};
pub use shake::ScreenShake;