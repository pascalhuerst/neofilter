//! NeoFilter DSP: analog prototypes, matched z-transform, biquad cascades.
//!
//! This crate has no plugin or GUI dependencies. See `PLAN.md` for the design.

pub mod analog;
pub mod band;
pub mod biquad;
pub mod conventions;
pub mod engine;
pub mod matched;
pub mod response;
pub mod smoothing;
pub mod types;

pub use engine::EqEngine;
pub use types::{BandParams, Placement, Shape, Slope};
