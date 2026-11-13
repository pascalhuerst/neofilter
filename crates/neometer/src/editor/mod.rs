//! egui editor. Paint and interaction code (`levels`, `stereo`, `spectrum`,
//! `vinyl`, `test`, `widgets`) can only be verified by running in a host; the
//! pure logic (formatting, scales, warning rules, plot mappings) lives in
//! [`scale`] and is unit-tested.

mod levels;
pub mod scale;
mod spectrum;
mod stereo;
mod test;
mod vinyl;
mod widgets;

use crate::analysis::{Snapshot, SpectrumData};
use crate::params::NeoMeterParams;
use crate::settings::{Page, SettingsData};
use crate::shared::{reset, Readouts, Shared};
use nih_plug::prelude::*;
use nih_plug_egui::resizable_window::ResizableWindow;
use nih_plug_egui::{create_egui_editor, egui};
use std::sync::Arc;
use std::time::Duration;

/// GUI-local state (lives as long as the editor window).
pub(crate) struct EditorState {
    pub params: Arc<NeoMeterParams>,
    pub shared: Arc<Shared>,
    /// Frozen spectrum, if the freeze button is on.
    pub frozen: Option<Arc<SpectrumData>>,
}

/// Everything a page needs for one frame: the readouts, the analysis snapshot
/// and a mutable copy of the persisted settings (written back if changed).
pub(crate) struct Frame {
    pub ro: Readouts,
    pub snap: Arc<Snapshot>,
    pub set: SettingsData,
}

const PAGES: [(Page, &str); 5] = [
    (Page::Levels, "Levels & Loudness"),
    (Page::Stereo, "Stereo"),
    (Page::Spectrum, "Spectrum"),
    (Page::Vinyl, "Vinyl"),
    (Page::Test, "Test"),
];

pub fn create(params: Arc<NeoMeterParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let egui_state = params.editor_state.clone();
    let state = EditorState {
        params,
        shared,
        frozen: None,
    };
    create_egui_editor(
        egui_state.clone(),
        state,
        |ctx, _| {
            ctx.set_visuals(egui::Visuals::dark());
        },
        move |ctx, setter, state| {
            ctx.request_repaint_after(Duration::from_millis(33));
            ResizableWindow::new("neometer-window")
                .min_size(egui::vec2(760.0, 480.0))
                .show(ctx, egui_state.as_ref(), |ui| {
                    let mut frame = Frame {
                        ro: state.shared.readouts(),
                        snap: state.shared.snapshot(),
                        set: state.shared.settings.get(),
                    };
                    let before = frame.set;
                    top_bar(ui, state, &mut frame);
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match frame.set.page {
                            Page::Levels => levels::show(ui, state, &mut frame),
                            Page::Stereo => stereo::show(ui, state, &mut frame, setter),
                            Page::Spectrum => spectrum::show(ui, state, &mut frame),
                            Page::Vinyl => vinyl::show(ui, state, &mut frame, setter),
                            Page::Test => test::show(ui, state, &mut frame),
                        });
                    if frame.set != before {
                        state.shared.settings.replace(frame.set);
                    }
                });
        },
    )
}

fn top_bar(ui: &mut egui::Ui, state: &mut EditorState, f: &mut Frame) {
    ui.horizontal_wrapped(|ui| {
        ui.heading("NeoMeter");
        ui.separator();
        for (page, name) in PAGES {
            if ui.selectable_label(f.set.page == page, name).clicked() {
                f.set.page = page;
            }
        }
        ui.separator();
        if f.ro.generator_active || state.shared.gen.is_on() {
            ui.label(
                egui::RichText::new(" GENERATOR ON ")
                    .strong()
                    .color(egui::Color32::WHITE)
                    .background_color(egui::Color32::from_rgb(200, 50, 40)),
            );
            if ui.button("Off").clicked() {
                state.shared.gen.force_off();
            }
        }
        let dropped = f.snap.overruns;
        if dropped > 0 {
            ui.label(
                egui::RichText::new(format!("dropped {dropped} frames"))
                    .color(egui::Color32::from_rgb(230, 190, 60)),
            )
            .on_hover_text(
                "The analysis thread fell behind; the audio was not affected, but loudness and \
                 spectrum readouts may have missed samples. Peak, RMS and vinyl meters run on the \
                 audio thread and are exact.",
            );
        }
        if ui.button("Reset all").clicked() {
            state.shared.request_reset(reset::ALL);
        }
    });
}
