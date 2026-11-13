//! NeoFilter plugin entry point. Pass-through for now; owned later by the Integration agent.

// Lint fired by nih-plug's vst3 export macro (upstream vst3_com), not our code.
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod analyzer;
pub mod editor;
pub mod params;
pub mod presets;

use nih_plug::prelude::*;
use params::NeoFilterParams;
use std::num::NonZeroU32;
use std::sync::Arc;

pub struct NeoFilter {
    params: Arc<NeoFilterParams>,
    /// Audio-side analyzer tap; the `process()` author feeds it (see `analyzer.rs`).
    #[allow(dead_code)]
    analyzer_in: analyzer::AnalyzerInput,
    editor_handles: editor::EditorHandles,
}

impl Default for NeoFilter {
    fn default() -> Self {
        let params = Arc::new(NeoFilterParams::default());
        let (analyzer_in, analyzer_out) = analyzer::analyzer(params.editor_state.clone());
        Self {
            params,
            analyzer_in,
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

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

impl ClapPlugin for NeoFilter {
    const CLAP_ID: &'static str = "org.neofilter.neofilter";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("24-band parametric equalizer");
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
