//! egui editor. Paint and interaction code (`levels`, `stereo`, `spectrum`,
//! `vinyl`, `test`, `testrec`, `widgets`) can only be verified by running in a host; the
//! pure logic (formatting, scales, warning rules, plot mappings) lives in
//! [`scale`] and is unit-tested.

mod levels;
pub mod scale;
mod spectrum;
mod stereo;
mod test;
mod testrec;
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

const PAGES: [(Page, &str); 6] = [
    (Page::Levels, "Levels & Loudness"),
    (Page::Stereo, "Stereo"),
    (Page::Spectrum, "Spectrum"),
    (Page::Vinyl, "Vinyl"),
    (Page::Test, "Test"),
    (Page::TestRecord, "Test record"),
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
                            Page::TestRecord => testrec::show(ui, state, &mut frame, setter),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::SweepLive;
    use crate::testrec::{
        measure_ref_band, measure_track_row, CapturedSweep, Mod, ProfileKind, RefBand,
    };
    use nih_plug::context::gui::GuiContext;
    use nih_plug::prelude::{ParamPtr, PluginApi, PluginState};
    use std::f64::consts::PI;

    /// Host stand-in: accepts and ignores parameter changes.
    struct NullHost;
    impl GuiContext for NullHost {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Standalone
        }
        fn request_resize(&self) -> bool {
            false
        }
        unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
        unsafe fn raw_set_parameter_normalized(&self, _param: ParamPtr, _normalized: f32) {}
        unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
        fn get_state(&self) -> PluginState {
            unimplemented!("not used by the pages")
        }
        fn set_state(&self, _state: PluginState) {}
    }

    fn tone(f: f64, db: f64, ph: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| 10f64.powf(db / 20.0) * (2.0 * PI * f * i as f64 / 48_000.0 + ph).sin())
            .collect()
    }

    /// Every page runs a few headless egui frames without panicking, with
    /// empty and with populated test-record state (layout only: nothing is
    /// rendered or looked at).
    #[test]
    fn all_pages_run_headless() {
        let params = Arc::new(NeoMeterParams::default());
        let shared = Shared::new(params.settings.clone(), None);
        let mut state = EditorState {
            params: params.clone(),
            shared: shared.clone(),
            frozen: None,
        };
        let host = NullHost;
        let setter = ParamSetter::new(&host);
        let ctx = egui::Context::default();
        let run = |state: &mut EditorState, page: Page| {
            let mut frame = Frame {
                ro: state.shared.readouts(),
                snap: state.shared.snapshot(),
                set: state.shared.settings.get(),
            };
            frame.set.page = page;
            for _ in 0..2 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| match page {
                        Page::Levels => levels::show(ui, state, &mut frame),
                        Page::Stereo => stereo::show(ui, state, &mut frame, &setter),
                        Page::Spectrum => spectrum::show(ui, state, &mut frame),
                        Page::Vinyl => vinyl::show(ui, state, &mut frame, &setter),
                        Page::Test => test::show(ui, state, &mut frame),
                        Page::TestRecord => testrec::show(ui, state, &mut frame, &setter),
                    });
                });
            }
        };
        let pages = [
            Page::Levels,
            Page::Stereo,
            Page::Spectrum,
            Page::Vinyl,
            Page::Test,
            Page::TestRecord,
        ];
        for p in pages {
            run(&mut state, p);
        }
        // populated state
        let n = 48_000;
        let mk = |slot: Mod, l: Vec<f64>, r: Vec<f64>| -> RefBand {
            measure_ref_band(&l, &r, 48_000.0, slot, 1.0, [0.0; 2]).unwrap()
        };
        let sw = CapturedSweep {
            cells: std::array::from_fn(|_| {
                (0..100)
                    .map(|i| {
                        let f = 800.0 * 2f64.powf(i as f64 / 24.0);
                        (i as u32 + 24, f, -20.0 - (f / 1000.0).log2())
                    })
                    .collect()
            }),
            trim_db: [0.0; 2],
            detected: Some(Mod::Lateral),
            level_diff_db: Some(0.0),
            phase_deg: Some(0.0),
            pair_blocks: 10,
        };
        let live = SweepLive {
            cells: sw.cells.clone(),
            ..SweepLive::default()
        };
        shared.testrec.update(|d| {
            d.set_ref_band(mk(
                Mod::Left,
                tone(1000.0, -20.0, 0.0, n),
                tone(1000.0, -50.0, 0.1, n),
            ));
            d.set_ref_band(mk(
                Mod::Right,
                tone(1000.0, -50.0, 0.0, n),
                tone(1000.0, -19.0, 0.2, n),
            ));
            for (a, db) in [(50.0, -30.0), (60.0, -28.4)] {
                let t = tone(315.0, db, 0.0, n);
                d.set_track_row(measure_track_row(&t, &t, 48_000.0, a).unwrap());
            }
            d.sweep = Some(sw);
        });
        for kind in [ProfileKind::Ortofon, ProfileKind::Custom] {
            shared.testrec.update(|d| d.profile_kind = kind);
            run(&mut state, Page::TestRecord);
        }
        shared
            .testrec
            .update(|d| d.norm = crate::testrec::NormMode::ToReference);
        run(&mut state, Page::TestRecord);
        // live sweep view and a running capture in the snapshot
        let mut f = Frame {
            ro: shared.readouts(),
            snap: shared.snapshot(),
            set: shared.settings.get(),
        };
        let mut snap = (*f.snap).clone();
        snap.rec.sweep = Some(live);
        snap.rec.busy = Some((
            crate::analysis::RecKindView(crate::analysis::RecKind::WowFlutter),
            0.4,
        ));
        f.snap = Arc::new(snap);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| testrec::show(ui, &mut state, &mut f, &setter));
        });
    }
}
