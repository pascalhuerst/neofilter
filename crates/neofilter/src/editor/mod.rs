//! egui editor. Placeholder: owned later by the Editor agent.

use crate::params::NeoFilterParams;
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use std::sync::Arc;

/// Default editor window state (900x560).
pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(900, 560)
}

/// Create the editor.
pub fn create(params: Arc<NeoFilterParams>) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        (),
        |_, _| {},
        move |ctx, _setter, _state| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("NeoFilter");
            });
        },
    )
}
