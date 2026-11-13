//! Host parameters and persisted state.
//!
//! Four automatable parameters exist (ids are frozen): the vinyl
//! calibration `reference_level`, the stereo `lf_crossover` and the input
//! trims `trim_l` / `trim_r` (applied to the pass-through audio before all
//! metering). All GUI choices are persisted but not automatable
//! ([`crate::settings`], [`crate::testrec`]); the generator is neither.

use crate::settings::Settings;
use crate::testrec::TestRecState;
use crate::vinyl::DEFAULT_REFERENCE_DBFS;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::Arc;

pub const REF_MIN_DB: f32 = -60.0;
pub const REF_MAX_DB: f32 = 0.0;
pub const LF_MIN_HZ: f32 = 50.0;
pub const LF_MAX_HZ: f32 = 1000.0;
pub const TRIM_MIN_DB: f32 = -24.0;
pub const TRIM_MAX_DB: f32 = 24.0;

/// Default editor size.
pub fn default_editor_state() -> Arc<EguiState> {
    EguiState::from_size(980, 620)
}

#[derive(Params)]
pub struct NeoMeterParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[persist = "meter-settings"]
    pub settings: Arc<Settings>,

    /// Test-record profile, options and measurements ([`crate::testrec`]).
    #[persist = "test-record"]
    pub testrec: Arc<TestRecState>,

    /// dBFS (after pre-emphasis) that corresponds to a recorded peak lateral
    /// velocity of 5 cm/s at 1 kHz.
    #[id = "reference_level"]
    pub reference_level: FloatParam,

    /// Crossover of the low-frequency vertical/lateral meters.
    #[id = "lf_crossover"]
    pub lf_crossover: FloatParam,

    /// Input trim of the left channel, applied before all metering and to the
    /// pass-through audio (smoothed).
    #[id = "trim_l"]
    pub trim_l: FloatParam,

    /// Input trim of the right channel.
    #[id = "trim_r"]
    pub trim_r: FloatParam,
}

fn trim_param(name: &str) -> FloatParam {
    FloatParam::new(
        name,
        0.0,
        FloatRange::Linear {
            min: TRIM_MIN_DB,
            max: TRIM_MAX_DB,
        },
    )
    .with_unit(" dB")
    .with_step_size(0.01)
    .with_value_to_string(Arc::new(|v| format!("{v:.2}")))
}

impl Default for NeoMeterParams {
    fn default() -> Self {
        Self {
            editor_state: default_editor_state(),
            settings: Arc::new(Settings::default()),
            testrec: Arc::new(TestRecState::default()),
            reference_level: FloatParam::new(
                "Reference Level",
                DEFAULT_REFERENCE_DBFS as f32,
                FloatRange::Linear {
                    min: REF_MIN_DB,
                    max: REF_MAX_DB,
                },
            )
            .with_unit(" dBFS")
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(|v| format!("{v:.1}"))),
            lf_crossover: FloatParam::new(
                "LF Crossover",
                300.0,
                FloatRange::Skewed {
                    min: LF_MIN_HZ,
                    max: LF_MAX_HZ,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" Hz")
            .with_step_size(1.0)
            .with_value_to_string(Arc::new(|v| format!("{v:.0}"))),
            trim_l: trim_param("Trim L"),
            trim_r: trim_param("Trim R"),
        }
    }
}
