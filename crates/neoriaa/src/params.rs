//! Plugin parameters. The ids are frozen: `mode`, `iec`, `ultrasonic`, `trim`,
//! `bypass`.

use neo_measure::riaa::{Mode, Options};
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::Arc;

/// Output trim range in dB.
pub const TRIM_MIN_DB: f32 = -36.0;
pub const TRIM_MAX_DB: f32 = 12.0;

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeParam {
    #[id = "pre"]
    #[name = "Pre-emphasis (cut)"]
    PreEmphasis,
    #[id = "de"]
    #[name = "De-emphasis (playback)"]
    DeEmphasis,
}

impl From<ModeParam> for Mode {
    fn from(m: ModeParam) -> Mode {
        match m {
            ModeParam::PreEmphasis => Mode::PreEmphasis,
            ModeParam::DeEmphasis => Mode::DeEmphasis,
        }
    }
}

impl From<Mode> for ModeParam {
    fn from(m: Mode) -> ModeParam {
        match m {
            Mode::PreEmphasis => ModeParam::PreEmphasis,
            Mode::DeEmphasis => ModeParam::DeEmphasis,
        }
    }
}

#[derive(Params)]
pub struct NeoRiaaParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    /// Pre-emphasis (recording / cutting, default) or de-emphasis (playback).
    #[id = "mode"]
    pub mode: EnumParam<ModeParam>,

    /// IEC 7950 us low-frequency roll-off (bass boost, limited below 5 Hz, in
    /// pre-emphasis mode).
    #[id = "iec"]
    pub iec: BoolParam,

    /// 3.18 us ultrasonic pole (about 50 kHz).
    #[id = "ultrasonic"]
    pub ultrasonic: BoolParam,

    /// Output trim in dB (smoothed linearly in dB).
    #[id = "trim"]
    pub trim: FloatParam,

    #[id = "bypass"]
    pub bypass: BoolParam,
}

impl Default for NeoRiaaParams {
    fn default() -> Self {
        Self {
            editor_state: crate::editor::default_state(),
            mode: EnumParam::new("Mode", ModeParam::PreEmphasis),
            iec: BoolParam::new("IEC 7950 us roll-off", false),
            ultrasonic: BoolParam::new("3.18 us pole", false),
            trim: FloatParam::new(
                "Trim",
                0.0,
                FloatRange::Linear {
                    min: TRIM_MIN_DB,
                    max: TRIM_MAX_DB,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.1)
            .with_smoother(SmoothingStyle::Linear(20.0)),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
        }
    }
}

impl NeoRiaaParams {
    pub fn options(&self) -> Options {
        Options {
            iec: self.iec.value(),
            ultrasonic: self.ultrasonic.value(),
        }
    }
}
