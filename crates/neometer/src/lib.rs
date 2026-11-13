//! NeoMeter: a pass-through metering and measurement plugin for mastering and
//! vinyl cutting. See `README.md` for the meter definitions.
//!
//! * [`audio::AudioPath`]: host-independent audio path (pass-through,
//!   generator, realtime meters, ring hand-off).
//! * [`analysis`]: background analysis (loudness gating, spectrum, tone, wow
//!   & flutter, ...), tested without threads via [`analysis::Engine`].
//! * [`vinyl`]: pre-emphasised cutter-head meters and calibration formulas.
//! * [`editor`]: egui editor.

// Lint fired by nih-plug's vst3 export macro (upstream vst3_com), not our code.
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod analysis;
pub mod audio;
pub mod editor;
pub mod params;
pub mod ring;
pub mod settings;
pub mod shared;
pub mod testgen;
pub mod vinyl;

use analysis::AnalysisThread;
use audio::{AudioParams, AudioPath};
use neo_measure::riaa::{Mode, Options, RiaaFilters};
use nih_plug::prelude::*;
use params::NeoMeterParams;
use shared::Shared;
use std::num::NonZeroU32;
use std::sync::Arc;

pub struct NeoMeter {
    params: Arc<NeoMeterParams>,
    shared: Arc<Shared>,
    audio: AudioPath,
    analysis: Option<AnalysisThread>,
    /// Pre-emphasis design of the last sample rate (design takes up to
    /// seconds, re-used across `initialize` calls at the same rate).
    riaa: Option<(f64, Arc<RiaaFilters>)>,
}

impl Default for NeoMeter {
    fn default() -> Self {
        let params = Arc::new(NeoMeterParams::default());
        let shared = Shared::new(params.settings.clone(), Some(params.editor_state.clone()));
        Self {
            audio: AudioPath::new(shared.clone()),
            params,
            shared,
            analysis: None,
            riaa: None,
        }
    }
}

impl Plugin for NeoMeter {
    const NAME: &'static str = "NeoMeter";
    const VENDOR: &'static str = "NeoFilter";
    const URL: &'static str = "https://github.com/pascalhuerst/neofilter";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let fs = f64::from(buffer_config.sample_rate);
        let channels = audio_io_layout
            .main_input_channels
            .map_or(2, |c| c.get() as usize);
        // Stop a previous instance of the thread (re-initialise).
        self.analysis = None;
        self.audio.release();
        let design = match &self.riaa {
            Some((r, d)) if (*r - fs).abs() < 1e-9 => d.clone(),
            _ => {
                let d = Arc::new(RiaaFilters::design(fs));
                self.riaa = Some((fs, d.clone()));
                d
            }
        };
        let cascade = *design.cascade(Mode::PreEmphasis, Options::default());
        let (thread, producer) = AnalysisThread::spawn(self.shared.clone(), fs, channels);
        self.analysis = Some(thread);
        self.audio.prepare(fs, channels == 1, cascade, producer);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {}

    fn deactivate(&mut self) {
        self.audio.release();
        self.analysis = None;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let p = AudioParams {
            lf_crossover_hz: f64::from(self.params.lf_crossover.value()),
        };
        match buffer.as_slice() {
            [l, r] => self.audio.process_stereo(l, r, &p),
            [m] => self.audio.process_mono(m, &p),
            _ => {}
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for NeoMeter {
    const CLAP_ID: &'static str = "org.neofilter.neometer";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Metering and measurement for mastering and vinyl cutting");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Analyzer,
        ClapFeature::Stereo,
        ClapFeature::Mono,
    ];
}

impl Vst3Plugin for NeoMeter {
    const VST3_CLASS_ID: [u8; 16] = *b"NeoMeterMeas0001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

nih_export_clap!(NeoMeter);
nih_export_vst3!(NeoMeter);
