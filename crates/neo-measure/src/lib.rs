//! Audio metering and measurement DSP for mastering and vinyl cutting.
//!
//! Pure DSP: no GUI, no plugin dependencies, no global state. Internal math is
//! `f64`; sample slices may be `f32` or `f64` where a function is generic over
//! [`Sample`].
//!
//! # Realtime classification
//!
//! Every module documents which of its types may be used on the audio thread
//! (allocation-free after construction, panic-free `process`) and which are
//! analysis types for a background/GUI thread that may allocate.
//!
//! | module | audio thread | off-thread analysis |
//! |---|---|---|
//! | [`levels`] | all meters | - |
//! | [`loudness`] | [`loudness::KWeighting`], [`loudness::LoudnessMeter`] (allocates only when the gating history outgrows its reserved capacity) | gating / LRA statistics |
//! | [`stereo`] | all | - |
//! | [`generator`] | all | - |
//! | [`spectrum`] | - (feed from a ring buffer) | all |
//! | [`delay`], [`transfer`], [`tone`], [`wow_flutter`] | - | all |
//! | [`weighting`] | filters | noise-level helper |

pub mod delay;
pub mod filter;
pub mod generator;
//pub mod levels;
//pub mod loudness;
//pub mod spectrum;
//pub mod stereo;
//pub mod tone;
//pub mod transfer;
pub mod util;
//pub mod weighting;
//pub mod wow_flutter;

/// Sample types accepted by the generic processing functions (`f32`, `f64`).
pub trait Sample: Copy + Into<f64> {}
impl<T: Copy + Into<f64>> Sample for T {}
