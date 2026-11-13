//! The "Calibrate" panel and the overlay curves in the main plot. Paint and
//! interaction code only; decisions live in `calib_view` (unit-tested).

use super::calib_view::{self as view, Severity};
use super::params_io::write_band;
use super::scale::View;
use super::topbar::pasted_text;
use super::EditorState;
use crate::calibration::session::Env;
use crate::calibration::settings::{CalCurve, InjectMode, NoiseKind, Routing};
use crate::params::NeoFilterParams;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{self, pos2, Align2, Color32, DragValue, FontId, Stroke, Ui};

/// GUI-local state of the panel.
pub struct CalUi {
    /// Show: measured, coherence, target, fitted, predicted.
    pub show: [bool; 5],
    pub curve_paste_armed: bool,
    pub curve_msg: String,
    pub notice: String,
}

impl Default for CalUi {
    fn default() -> Self {
        Self {
            show: [true; 5],
            curve_paste_armed: false,
            curve_msg: String::new(),
            notice: String::new(),
        }
    }
}

pub const SERIES_NAMES: [&str; 5] = [
    "Measured loop",
    "Coherence",
    "Target correction",
    "Fitted correction",
    "Predicted residual",
];
pub const SERIES_COLORS: [Color32; 5] = [
    Color32::from_rgb(120, 200, 255),
    Color32::from_rgb(120, 160, 120),
    Color32::from_rgb(255, 150, 80),
    Color32::from_rgb(255, 90, 160),
    Color32::from_rgb(190, 255, 120),
];

fn env<'a>(params: &NeoFilterParams, snap: &'a [BandParams; NUM_BANDS]) -> Env<'a> {
    Env {
        bands: snap,
        gain_scale: f64::from(params.gain_scale_factor()),
        bypass: params.bypass.value(),
    }
}

fn write_changed(
    setter: &ParamSetter,
    params: &NeoFilterParams,
    cur: &[BandParams; NUM_BANDS],
    new: &[BandParams; NUM_BANDS],
) {
    for i in view::bands_to_write(cur, new) {
        write_band(setter, params, i, &new[i]);
    }
}

/// Run every frame, whether or not the panel is open: drives the session
/// (auto mode applies fits from here) and writes the bands through the host.
pub fn poll(state: &mut EditorState, setter: &ParamSetter) {
    let params = state.params.clone();
    let shared = state.handles.calibration.clone();
    let snap = params.band_snapshot();
    let (Ok(mut sess), Ok(mut persist)) = (shared.session.lock(), params.calibration.lock()) else {
        return;
    };
    if let Some(new) = sess.poll(&mut persist, &env(&params, &snap)) {
        write_changed(setter, &params, &snap, &new);
    }
}

/// True while noise is audible (or a measurement is being captured).
pub fn noise_on(state: &EditorState) -> bool {
    state.handles.calibration.link.noise_active() || state.handles.calibration.link.is_running()
}

fn enum_combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    cur: &mut T,
    items: &[(T, &str)],
) -> bool {
    let mut changed = false;
    let label = items
        .iter()
        .find(|(v, _)| v == cur)
        .map_or("?", |(_, l)| *l);
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .width(70.0)
        .show_ui(ui, |ui| {
            for (v, l) in items {
                if ui.selectable_label(v == cur, *l).clicked() {
                    *cur = *v;
                    changed = true;
                }
            }
        });
    changed
}

fn num(ui: &mut Ui, v: &mut f64, lo: f64, hi: f64, speed: f64, suffix: &str) -> bool {
    ui.add(DragValue::new(v).range(lo..=hi).speed(speed).suffix(suffix))
        .changed()
}

/// The panel (about 200 px high).
pub fn show(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter) {
    let params = state.params.clone();
    let shared = state.handles.calibration.clone();
    let snap = params.band_snapshot();
    let (Ok(mut sess), Ok(mut persist)) = (shared.session.lock(), params.calibration.lock()) else {
        return;
    };
    let persist = &mut *persist;
    let noise = noise_on(state);
    let en = view::enabled(sess.phase(), sess.can_fit(), sess.can_apply(), noise);
    let e = env(&params, &snap);

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
            // Buttons and status.
            ui.horizontal_wrapped(|ui| {
                let mut msg: Option<String> = None;
                if ui.add_enabled(en.measure, egui::Button::new("Measure")).on_hover_text("Play noise through the EQ and measure the loop once").clicked() {
                    msg = sess.start_measure(persist, &e).err();
                }
                if ui.add_enabled(en.fit, egui::Button::new("Fit")).on_hover_text("Fit calibration bands to the last measurement").clicked() {
                    msg = sess.fit(persist, &e).err();
                }
                if ui.add_enabled(en.apply, egui::Button::new("Apply")).on_hover_text("Write the fitted bands into the calibration band slots").clicked() {
                    match sess.apply(persist, &e) {
                        Ok(new) => write_changed(setter, &params, &snap, &new),
                        Err(m) => msg = Some(m),
                    }
                }
                if ui.add_enabled(en.auto, egui::Button::new("Auto")).on_hover_text("Measure, fit, apply and repeat until flat").clicked() {
                    msg = sess.start_auto(persist, &e).err();
                }
                if ui.add_enabled(en.stop, egui::Button::new("Stop")).on_hover_text("Stop the noise and abort").clicked() {
                    sess.stop();
                }
                if ui.add_enabled(en.clear, egui::Button::new("Clear calibration")).on_hover_text("Unuse all calibration bands").clicked() {
                    let new = sess.clear_calibration(persist, &e);
                    write_changed(setter, &params, &snap, &new);
                }
                if let Some(m) = msg {
                    state.cal.notice = m;
                }
                if noise {
                    ui.label(egui::RichText::new(" NOISE ON ").strong().color(Color32::WHITE).background_color(Color32::from_rgb(200, 30, 30)));
                }
            });
            let (text, sev) = view::status_line(&sess);
            let color = match sev {
                Severity::Info => Color32::from_gray(190),
                Severity::Busy => Color32::from_rgb(255, 214, 90),
                Severity::Warn => Color32::from_rgb(255, 170, 60),
                Severity::Error => Color32::from_rgb(255, 100, 90),
            };
            ui.horizontal(|ui| {
                ui.colored_label(color, text);
                if !state.cal.notice.is_empty() {
                    ui.colored_label(Color32::from_rgb(255, 170, 60), &state.cal.notice);
                }
            });
            if sess.busy() {
                let (c, t) = sess.progress();
                ui.add(egui::ProgressBar::new(view::progress_fraction(c, t)).desired_height(6.0));
            }

            // Settings.
            let s = &mut persist.settings;
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                ui.label("Noise");
                changed |= enum_combo(ui, "cal_noise", &mut s.noise, &[(NoiseKind::Pink, "Pink"), (NoiseKind::White, "White")]);
                changed |= num(ui, &mut s.level_db, -80.0, -6.0, 0.2, " dBFS");
                ui.label("Channels");
                changed |= enum_combo(ui, "cal_routing", &mut s.routing, &[(Routing::Both, "L+R mono"), (Routing::Left, "Left"), (Routing::Right, "Right")]);
                changed |= enum_combo(ui, "cal_mode", &mut s.mode, &[(InjectMode::Replace, "Replace input"), (InjectMode::Add, "Add to input")]);
                ui.label("Duration");
                changed |= num(ui, &mut s.duration_s, 4.0, 60.0, 0.2, " s");
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Range");
                changed |= num(ui, &mut s.range_lo_hz, 20.0, 1000.0, 1.0, " Hz");
                changed |= num(ui, &mut s.range_hi_hz, 2000.0, 20_000.0, 20.0, " Hz");
                ui.label("Max boost");
                changed |= num(ui, &mut s.max_boost_db, 0.0, 12.0, 0.1, " dB");
                ui.label("Max cut");
                changed |= num(ui, &mut s.max_cut_db, 0.0, 24.0, 0.1, " dB");
                ui.label("Smoothing 1/");
                let mut sm = f64::from(s.smoothing);
                if num(ui, &mut sm, 0.0, 48.0, 0.2, " oct") {
                    s.smoothing = sm as u32;
                    changed = true;
                }
                ui.label("Max bands");
                let mut mb = s.max_bands as f64;
                if num(ui, &mut mb, 1.0, 32.0, 0.1, "") {
                    s.max_bands = mb as usize;
                    changed = true;
                }
                changed |= ui.checkbox(&mut s.replace_all, "Replace all").on_hover_text("Every band becomes a calibration band (the whole EQ is rewritten)").changed();
            });
            if changed {
                s.sanitize();
            }

            // Playback curve.
            ui.horizontal_wrapped(|ui| {
                ui.label("Playback curve:");
                ui.label(view::curve_label(persist.correction.as_ref()));
                let armed = state.cal.curve_paste_armed;
                if ui.selectable_label(armed, "Paste curve").on_hover_text("Click, then press Ctrl+V to load a response curve (JSON)").clicked() {
                    state.cal.curve_paste_armed = !armed;
                    state.cal.curve_msg = if armed { String::new() } else { "Press Ctrl+V".into() };
                }
                if ui.add_enabled(persist.correction.is_some(), egui::Button::new("Clear")).clicked() {
                    persist.correction = None;
                    state.cal.curve_msg.clear();
                }
                if !state.cal.curve_msg.is_empty() {
                    ui.weak(&state.cal.curve_msg);
                }
            });
            if state.cal.curve_paste_armed {
                if let Some(text) = ui.ctx().input(|i| pasted_text(&i.events)) {
                    state.cal.curve_paste_armed = false;
                    match CalCurve::from_json(&text) {
                        Ok(c) => {
                            state.cal.curve_msg = format!("loaded '{}'", c.name);
                            persist.correction = Some(c);
                        }
                        Err(e) => state.cal.curve_msg = format!("paste failed: {e}"),
                    }
                }
            }

            // Readouts.
            if let Some(m) = sess.measurement() {
                for l in view::channel_lines(m) {
                    ui.label(egui::RichText::new(l).monospace().size(11.0));
                }
                ui.label(format!(
                    "Return {:.1} dBFS rms / {:.1} dBFS peak, mean coherence {:.2}",
                    m.return_rms_db, m.return_peak_db, m.mean_coherence
                ));
            }
            if let Some(l) = &persist.last {
                ui.label(format!(
                    "Deviation in range: {:.2} dB rms, {:.2} dB max{}",
                    l.rms_deviation_db,
                    l.max_deviation_db,
                    if l.note.is_empty() { String::new() } else { format!(" ({})", l.note) }
                ));
            }
            for r in sess.history() {
                ui.label(egui::RichText::new(view::history_line(r)).monospace().size(11.0));
            }
            ui.horizontal(|ui| {
                ui.label("Show:");
                for (i, name) in SERIES_NAMES.iter().enumerate() {
                    ui.checkbox(&mut state.cal.show[i], egui::RichText::new(*name).color(SERIES_COLORS[i]));
                }
            });
            ui.weak("The noise goes through the EQ to the cutter chain: keep the cutter's own protection active.");
        });
    });
}

/// Calibration curves on top of the main plot (only while the panel is open).
pub fn draw_overlay(ui: &Ui, state: &EditorState, view_: &View) {
    let shared = &state.handles.calibration;
    let Ok(sess) = shared.session.try_lock() else {
        return;
    };
    let Ok(persist) = state.params.calibration.try_lock() else {
        return;
    };
    let Some(s) = view::series(
        sess.measurement(),
        sess.plan(),
        &persist.settings,
        persist.correction.as_ref(),
    ) else {
        return;
    };
    let range = f64::from(view_.db_range);
    let painter = ui.painter();
    let line = |ys: &[f64], map: &dyn Fn(f64) -> f64, color: Color32, width: f32| {
        let pts: Vec<_> = s
            .freqs
            .iter()
            .zip(ys)
            .filter(|(f, _)| **f >= 10.0)
            .map(|(&f, &v)| {
                pos2(
                    view_.freq_to_x(f as f32),
                    view_
                        .db_to_y(map(v) as f32)
                        .clamp(view_.y0, view_.y0 + view_.height),
                )
            })
            .collect();
        if pts.len() > 1 {
            painter.add(egui::Shape::line(pts, Stroke::new(width, color)));
        }
    };
    let id = |v: f64| v;
    let coh = |c: f64| view::coherence_to_db(c, range);
    if state.cal.show[1] {
        line(&s.coherence, &coh, SERIES_COLORS[1], 1.0);
    }
    if state.cal.show[0] {
        line(&s.measured, &id, SERIES_COLORS[0], 1.5);
    }
    if state.cal.show[2] && !s.target.is_empty() {
        line(&s.target, &id, SERIES_COLORS[2], 1.2);
    }
    if state.cal.show[3] && !s.fitted.is_empty() {
        line(&s.fitted, &id, SERIES_COLORS[3], 1.8);
    }
    if state.cal.show[4] && !s.predicted.is_empty() {
        line(&s.predicted, &id, SERIES_COLORS[4], 1.2);
    }
    painter.text(
        pos2(view_.x0 + 6.0, view_.y0 + view_.height - 4.0),
        Align2::LEFT_BOTTOM,
        "coherence 0..1 (lowest fifth)",
        FontId::proportional(9.0),
        SERIES_COLORS[1],
    );
}
