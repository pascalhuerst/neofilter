//! egui editor. Paint/interaction code (`display`, `strip`, `topbar`) can only be
//! verified by running in a host; the pure logic lives in `scale`, `curves` and
//! `analyzer` and is unit-tested.

mod calib;
mod calib_view;
mod curves;
mod display;
mod params_io;
pub mod scale;
mod strip;
mod topbar;
mod widgets;

use crate::analyzer::{AnalyzerOutput, AnalyzerShared};
use crate::params::NeoFilterParams;
use curves::CurveCache;
use nih_plug::prelude::*;
use nih_plug_egui::resizable_window::ResizableWindow;
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Default editor window state (900x560).
pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(900, 560)
}

/// GUI settings that live as long as the plugin instance (survive closing and
/// re-opening the editor; not saved with the project).
pub struct EditorSettings {
    /// Index into [`scale::DB_RANGES`].
    pub db_range_idx: AtomicU8,
}

/// Everything the plugin hands to the editor besides the params.
#[derive(Clone)]
pub struct EditorHandles {
    /// Locked only by the GUI thread.
    pub analyzer: Arc<Mutex<AnalyzerOutput>>,
    pub analyzer_shared: Arc<AnalyzerShared>,
    pub settings: Arc<EditorSettings>,
    pub calibration: Arc<crate::calibration::CalShared>,
}

impl EditorHandles {
    pub fn new(analyzer_output: AnalyzerOutput) -> Self {
        Self {
            calibration: Arc::new(crate::calibration::CalShared::default()),
            analyzer_shared: analyzer_output.shared(),
            analyzer: Arc::new(Mutex::new(analyzer_output)),
            settings: Arc::new(EditorSettings {
                db_range_idx: AtomicU8::new(1),
            }),
        }
    }
}

/// An in-progress node drag.
pub(crate) struct Drag {
    pub band: usize,
    pub start_freq: f32,
    pub start_gain: f32,
    pub has_gain: bool,
    pub ax: f32,
    pub ay: f32,
}

/// What the open context menu refers to.
#[derive(Clone, Copy)]
pub(crate) enum MenuTarget {
    Band(usize),
    Empty { x: f32, y: f32 },
}

/// GUI-local state.
pub(crate) struct EditorState {
    pub params: Arc<NeoFilterParams>,
    pub handles: EditorHandles,
    pub selected: usize,
    pub drag: Option<Drag>,
    pub menu_target: MenuTarget,
    pub curves: CurveCache,
    pub pre_cols: Vec<f32>,
    pub post_cols: Vec<f32>,
    pub preset_name: String,
    pub status: String,
    pub paste_armed: bool,
    pub cal_open: bool,
    pub cal: calib::CalUi,
}

/// Create the editor.
///
/// `handles` come from [`EditorHandles::new`] wrapping the `AnalyzerOutput`
/// created together with the audio-side `AnalyzerInput`.
pub fn create(params: Arc<NeoFilterParams>, handles: EditorHandles) -> Option<Box<dyn Editor>> {
    let egui_state = params.editor_state.clone();
    let state = EditorState {
        params: params.clone(),
        handles,
        selected: 0,
        drag: None,
        menu_target: MenuTarget::Empty { x: 0.0, y: 0.0 },
        curves: CurveCache::default(),
        pre_cols: Vec::new(),
        post_cols: Vec::new(),
        preset_name: String::new(),
        status: String::new(),
        paste_armed: false,
        cal_open: false,
        cal: calib::CalUi::default(),
    };
    create_egui_editor(
        egui_state.clone(),
        state,
        |ctx, _| {
            ctx.set_visuals(egui::Visuals::dark());
        },
        move |ctx, setter, state| {
            ctx.request_repaint_after(Duration::from_millis(33));
            ResizableWindow::new("neofilter-window")
                .min_size(egui::vec2(560.0, 380.0))
                .show(ctx, egui_state.as_ref(), |ui| {
                    calib::poll(state, setter);
                    topbar::show(ui, state, setter);
                    ui.add_space(2.0);
                    if state.cal_open {
                        calib::show(ui, state, setter);
                        ui.add_space(2.0);
                    }
                    let rest = ui.available_rect_before_wrap();
                    let strip_h = 62.0;
                    let plot = egui::Rect::from_min_max(
                        rest.min,
                        egui::pos2(
                            rest.max.x,
                            (rest.max.y - strip_h - 6.0).max(rest.min.y + 40.0),
                        ),
                    );
                    let strip = egui::Rect::from_min_max(
                        egui::pos2(rest.min.x, plot.max.y + 6.0),
                        rest.max,
                    );
                    display::show(ui, state, setter, plot);
                    ui.scope_builder(egui::UiBuilder::new().max_rect(strip), |ui| {
                        strip::show(ui, state, setter);
                    });
                });
        },
    )
}
