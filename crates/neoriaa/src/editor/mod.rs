//! egui editor. Painting code (`view`) can only be verified by running in a
//! host; the pure logic lives in `scale` and is unit-tested.

pub mod scale;
mod view;

use crate::audio::Meters;
use crate::dsp::RiaaFilters;
use crate::params::NeoRiaaParams;
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Default editor window state (520x360).
pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(520, 360)
}

/// What the plugin hands to the editor besides the parameters.
#[derive(Clone, Default)]
pub struct EditorHandles {
    pub meters: Arc<Meters>,
    /// Design for the current sample rate (`None` until `initialize()` ran).
    /// Locked only briefly, never on the audio thread.
    pub filters: Arc<Mutex<Option<Arc<RiaaFilters>>>>,
}

pub(crate) struct EditorState {
    pub params: Arc<NeoRiaaParams>,
    pub handles: EditorHandles,
    pub curves: scale::CurveCache,
}

pub fn create(params: Arc<NeoRiaaParams>, handles: EditorHandles) -> Option<Box<dyn Editor>> {
    let egui_state = params.editor_state.clone();
    let state = EditorState {
        params,
        handles,
        curves: scale::CurveCache::default(),
    };
    create_egui_editor(
        egui_state.clone(),
        state,
        |ctx, _| {
            ctx.set_visuals(egui::Visuals::dark());
        },
        move |ctx, setter, state| {
            ctx.request_repaint_after(Duration::from_millis(33));
            nih_plug_egui::resizable_window::ResizableWindow::new("neoriaa-window")
                .min_size(egui::vec2(480.0, 340.0))
                .show(ctx, egui_state.as_ref(), |ui| view::show(ui, state, setter));
        },
    )
}
