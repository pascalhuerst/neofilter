//! NeoFilter plugin entry point. The audio path lives in [`audio::AudioPath`].

// Lint fired by nih-plug's vst3 export macro (upstream vst3_com), not our code.
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod analyzer;
pub mod audio;
pub mod editor;
pub mod params;
pub mod presets;

use nih_plug::prelude::*;
use params::NeoFilterParams;
use std::num::NonZeroU32;
use std::sync::Arc;

pub struct NeoFilter {
    params: Arc<NeoFilterParams>,
    /// Audio-side analyzer tap (see `analyzer.rs`).
    analyzer_in: analyzer::AnalyzerInput,
    audio: audio::AudioPath,
    editor_handles: editor::EditorHandles,
}

impl Default for NeoFilter {
    fn default() -> Self {
        let params = Arc::new(NeoFilterParams::default());
        let (analyzer_in, analyzer_out) = analyzer::analyzer(params.editor_state.clone());
        Self {
            params,
            analyzer_in,
            audio: audio::AudioPath::default(),
            editor_handles: editor::EditorHandles::new(analyzer_out),
        }
    }
}

impl Plugin for NeoFilter {
    const NAME: &'static str = "NeoFilter";
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

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.editor_handles.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = f64::from(buffer_config.sample_rate);
        self.audio
            .prepare(sr, buffer_config.max_buffer_size as usize);
        self.analyzer_in.set_sample_rate(buffer_config.sample_rate);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.audio.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // The wrappers split the buffer at parameter events (sample-accurate
        // automation), so reading the parameters once per call is exact.
        let bands = self.params.band_snapshot();
        let p = audio::AudioParams {
            bands: &bands,
            gain_scale: f64::from(self.params.gain_scale_factor()),
            out_gain_db: f64::from(self.params.out_gain.value()),
            bypass: self.params.bypass.value(),
        };
        match buffer.as_slice() {
            [l, r] => {
                self.analyzer_in.push_pre(l, Some(r));
                self.audio.process_stereo(l, r, &p);
                self.analyzer_in.push_post(l, Some(r));
            }
            [m] => {
                self.analyzer_in.push_pre(m, None);
                self.audio.process_mono(m, &p);
                self.analyzer_in.push_post(m, None);
            }
            _ => {}
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for NeoFilter {
    const CLAP_ID: &'static str = "org.neofilter.neofilter";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("32-band parametric equalizer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Equalizer,
    ];
}

impl Vst3Plugin for NeoFilter {
    const VST3_CLASS_ID: [u8; 16] = *b"NeoFilterEQ00001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Eq];
}

nih_export_clap!(NeoFilter);
nih_export_vst3!(NeoFilter);
