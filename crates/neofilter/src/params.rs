//! Plugin parameters. Stub: owned by the Params agent.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::Arc;

#[derive(Params)]
pub struct NeoFilterParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[id = "bypass"]
    pub bypass: BoolParam,
}

impl Default for NeoFilterParams {
    fn default() -> Self {
        Self {
            editor_state: crate::editor::default_state(),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
        }
    }
}
