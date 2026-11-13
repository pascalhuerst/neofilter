//! NeoRIAA plugin entry point. The DSP lives in [`dsp`] (no plugin types) and
//! the host-independent audio path in [`audio::AudioPath`].

// Lint fired by nih-plug's vst3 export macro (upstream vst3_com), not our code.
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod audio;
pub mod dsp;
pub mod editor;
pub mod params;

use audio::{AudioParams, AudioPath, Meters};
use dsp::RiaaFilters;
use nih_plug::prelude::*;
use params::NeoRiaaParams;
use std::num::NonZeroU32;
use std::sync::Arc;

pub struct NeoRiaa {
    params: Arc<NeoRiaaParams>,
    /// `None` until `initialize()` has designed the filters.
    audio: Option<AudioPath>,
    meters: Arc<Meters>,
    handles: editor::EditorHandles,
    /// Design of the current sample rate (reused by `initialize()` when the
    /// rate did not change).
    design: Option<Arc<RiaaFilters>>,
}

impl Default for NeoRiaa {
    fn default() -> Self {
        let handles = editor::EditorHandles::default();
        Self {
            params: Arc::new(NeoRiaaParams::default()),
            audio: None,
            meters: handles.meters.clone(),
            handles,
            design: None,
        }
    }
}

impl Plugin for NeoRiaa {
    const NAME: &'static str = "NeoRIAA";
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

    // The audio path ramps parameter changes itself; per-block updates suffice.
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.handles.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = f64::from(buffer_config.sample_rate);
        let design = match &self.design {
            Some(d) if d.sample_rate() == sr => d.clone(),
            _ => Arc::new(RiaaFilters::design(sr)),
        };
        match &mut self.audio {
            Some(a) => a.prepare(&design),
            None => self.audio = Some(AudioPath::new(&design, self.meters.clone())),
        }
        if let Ok(mut g) = self.handles.filters.lock() {
            *g = Some(design.clone());
        }
        self.design = Some(design);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        if let Some(a) = &mut self.audio {
            a.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(audio) = &mut self.audio else {
            return ProcessStatus::Normal;
        };
        let p = AudioParams {
            mode: self.params.mode.value().into(),
            options: self.params.options(),
            trim_db: f64::from(self.params.trim.value()),
            bypass: self.params.bypass.value(),
        };
        match buffer.as_slice() {
            [l, r] => audio.process_stereo(l, r, &p),
            [m] => audio.process_mono(m, &p),
            _ => {}
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for NeoRiaa {
    const CLAP_ID: &'static str = "org.neofilter.neoriaa";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("RIAA pre-emphasis / de-emphasis for vinyl cutting");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Filter,
    ];
}

impl Vst3Plugin for NeoRiaa {
    const VST3_CLASS_ID: [u8; 16] = *b"NeoRIAAPreEmph01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Filter];
}

nih_export_clap!(NeoRiaa);
nih_export_vst3!(NeoRiaa);
