//! NeoFilter DSP: analog prototypes, matched z-transform, biquad cascades.
//!
//! This crate has no plugin or GUI dependencies. See `PLAN.md` for the design.

// Comparisons are written as `!(x > 0.0)` on purpose where NaN must be rejected
// together with out-of-range values (coefficient design guard rails).
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod analog;
pub mod autofit;
pub mod band;
pub mod biquad;
pub mod conventions;
pub mod engine;
pub mod matched;
pub mod refmatch;
pub mod response;
pub mod smoothing;
pub mod types;

pub use engine::EqEngine;
pub use types::{BandParams, Placement, Shape, Slope};
