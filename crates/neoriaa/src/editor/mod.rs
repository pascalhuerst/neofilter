//! Editor placeholder (the egui editor follows in the next commit).

use crate::audio::Meters;
use crate::dsp::RiaaFilters;
use crate::params::NeoRiaaParams;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::{Arc, Mutex};

/// Default editor window state (520x360).
pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(520, 360)
}

/// What the plugin hands to the editor besides the parameters.
#[derive(Clone, Default)]
pub struct EditorHandles {
    pub meters: Arc<Meters>,
    /// Design for the current sample rate (`None` until `initialize()` ran).
    pub filters: Arc<Mutex<Option<Arc<RiaaFilters>>>>,
}

pub fn create(_params: Arc<NeoRiaaParams>, _handles: EditorHandles) -> Option<Box<dyn Editor>> {
    None
}
