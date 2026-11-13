//! Paint code for the NeoRIAA window (not unit-testable; logic is in `scale`).

use super::scale::{self, View};
use super::EditorState;
use crate::params::{ModeParam, NeoRiaaParams};
use neo_measure::riaa::Mode;
use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, Ui};
use nih_plug_egui::widgets::ParamSlider;

const MARGIN_LEFT: f32 = 40.0;
const MARGIN_BOTTOM: f32 = 16.0;
const ANALOG: Color32 = Color32::from_rgb(120, 120, 130);
const DIGITAL: Color32 = Color32::from_rgb(90, 200, 120);
const ERROR: Color32 = Color32::from_rgb(235, 170, 70);
const LABEL: Color32 = Color32::from_gray(150);

/// One complete begin/set/end gesture.
fn put<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

pub fn show(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter) {
    let params = state.params.clone();
    controls(ui, &params, setter);
    ui.add_space(2.0);

    let filters = state.handles.filters.lock().ok().and_then(|g| g.clone());
    let mode: Mode = params.mode.value().into();
    let opts = params.options();
    let sample_rate = filters.as_ref().map(|f| f.sample_rate());
    if let Some(f) = &filters {
        state.curves.update(f, mode, opts);
    }

    ui.horizontal(|ui| {
        match sample_rate {
            Some(fs) => ui.label(format!("Sample rate: {:.1} kHz", fs / 1000.0)),
            None => ui.label("Waiting for the audio engine..."),
        };
    });

    let rest = ui.available_rect_before_wrap();
    let meter_h = 44.0;
    let readout_h = 18.0;
    let plots_h = (rest.height() - meter_h - readout_h - 8.0).max(60.0);
    let main_h = plots_h * 0.68;
    let err_h = plots_h - main_h;
    let main = Rect::from_min_size(rest.min, vec2(rest.width(), main_h));
    let err = Rect::from_min_size(pos2(rest.min.x, main.max.y), vec2(rest.width(), err_h));
    if !state.curves.is_empty() {
        response_plot(ui, state, main);
        error_plot(ui, state, err);
    }
    let readout = Rect::from_min_size(
        pos2(rest.min.x, err.max.y + 2.0),
        vec2(rest.width(), readout_h),
    );
    readout_row(ui, state, readout);
    let meter = Rect::from_min_size(
        pos2(rest.min.x, readout.max.y + 4.0),
        vec2(rest.width(), meter_h),
    );
    meter_row(ui, state, meter);
}

fn controls(ui: &mut Ui, params: &NeoRiaaParams, setter: &ParamSetter) {
    ui.horizontal_wrapped(|ui| {
        ui.heading("NeoRIAA");
        ui.separator();
        let current = params.mode.value();
        for (m, label) in [
            (ModeParam::PreEmphasis, "Pre-emphasis (cut)"),
            (ModeParam::DeEmphasis, "De-emphasis (playback)"),
        ] {
            if ui.selectable_label(current == m, label).clicked() && current != m {
                put(setter, &params.mode, m);
            }
        }
        ui.separator();
        let mut iec = params.iec.value();
        if ui
            .checkbox(&mut iec, "IEC 7950 \u{b5}s")
            .on_hover_text(
                "IEC low-frequency roll-off. De-emphasis: 20 Hz high-pass. \
                 Pre-emphasis: matching bass boost, limited below 5 Hz.",
            )
            .changed()
        {
            put(setter, &params.iec, iec);
        }
        let mut us = params.ultrasonic.value();
        if ui
            .checkbox(&mut us, "3.18 \u{b5}s pole")
            .on_hover_text("Ultrasonic pole (about 50 kHz) limiting the pre-emphasis rise.")
            .changed()
        {
            put(setter, &params.ultrasonic, us);
        }
        let mut bypass = params.bypass.value();
        if ui.checkbox(&mut bypass, "Bypass").changed() {
            put(setter, &params.bypass, bypass);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Trim");
        ui.add(ParamSlider::for_param(&params.trim, setter).with_width(200.0));
    });
}

fn plot_area(ui: &mut Ui, rect: Rect) -> Rect {
    let inner = Rect::from_min_max(
        pos2(rect.min.x + MARGIN_LEFT, rect.min.y + 2.0),
        pos2(rect.max.x - 6.0, rect.max.y - MARGIN_BOTTOM),
    );
    ui.allocate_rect(rect, Sense::hover());
    ui.painter().rect_filled(inner, 2.0, Color32::from_gray(20));
    inner
}

fn draw_freq_grid(ui: &Ui, view: &View) {
    let p = ui.painter();
    let bottom = view.y0 + view.height;
    let band = Stroke::new(1.0_f32, Color32::from_gray(52));
    for (f, label) in scale::freq_ticks(view.f_max) {
        let x = view.freq_to_x(f);
        p.line_segment([pos2(x, view.y0), pos2(x, bottom)], band);
        p.text(
            pos2(x, bottom + 2.0),
            Align2::CENTER_TOP,
            label,
            FontId::proportional(10.0),
            LABEL,
        );
    }
}

fn draw_db_grid(ui: &Ui, view: &View, step: f64, decimals: usize) {
    let p = ui.painter();
    for db in scale::db_ticks(view.db_min, view.db_max, step) {
        let y = view.db_to_y(db);
        let stroke = if db == 0.0 {
            Stroke::new(1.0_f32, Color32::from_gray(95))
        } else {
            Stroke::new(1.0_f32, Color32::from_gray(40))
        };
        p.line_segment([pos2(view.x0, y), pos2(view.x0 + view.width, y)], stroke);
        p.text(
            pos2(view.x0 - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{db:+.decimals$}"),
            FontId::proportional(10.0),
            LABEL,
        );
    }
}

fn polyline(ui: &Ui, view: &View, freqs: &[f64], values: &[f64], stroke: Stroke) {
    let top = view.y0;
    let bottom = view.y0 + view.height;
    let pts: Vec<_> = freqs
        .iter()
        .zip(values)
        .map(|(&f, &v)| pos2(view.freq_to_x(f), view.db_to_y(v).clamp(top, bottom)))
        .collect();
    ui.painter().add(egui::Shape::line(pts, stroke));
}

fn response_plot(ui: &mut Ui, state: &EditorState, rect: Rect) {
    let inner = plot_area(ui, rect);
    let c = &state.curves;
    let view = View {
        x0: inner.min.x,
        y0: inner.min.y,
        width: inner.width(),
        height: inner.height(),
        f_min: scale::F_MIN,
        f_max: c.f_max,
        db_min: scale::RESPONSE_DB.0,
        db_max: scale::RESPONSE_DB.1,
    };
    draw_freq_grid(ui, &view);
    draw_db_grid(ui, &view, 10.0, 0);
    polyline(ui, &view, &c.freqs, &c.analog, Stroke::new(3.0_f32, ANALOG));
    polyline(
        ui,
        &view,
        &c.freqs,
        &c.digital,
        Stroke::new(1.5_f32, DIGITAL),
    );
    let p = ui.painter();
    p.text(
        pos2(inner.max.x - 6.0, inner.min.y + 4.0),
        Align2::RIGHT_TOP,
        "analog target (grey)  digital (green)",
        FontId::proportional(10.0),
        LABEL,
    );
}

fn error_plot(ui: &mut Ui, state: &EditorState, rect: Rect) {
    let inner = plot_area(ui, rect);
    let c = &state.curves;
    let view = View {
        x0: inner.min.x,
        y0: inner.min.y,
        width: inner.width(),
        height: inner.height(),
        f_min: scale::F_MIN,
        f_max: c.f_max,
        db_min: -scale::ERROR_DB,
        db_max: scale::ERROR_DB,
    };
    draw_freq_grid(ui, &view);
    draw_db_grid(ui, &view, scale::ERROR_DB / 2.0, 3);
    // shade what lies outside the 20 Hz .. 20 kHz accuracy band
    let shade = Color32::from_black_alpha(90);
    let (xl, xh) = (view.freq_to_x(scale::BAND.0), view.freq_to_x(scale::BAND.1));
    ui.painter().rect_filled(
        Rect::from_min_max(inner.min, pos2(xl, inner.max.y)),
        0.0,
        shade,
    );
    if xh < inner.max.x {
        ui.painter().rect_filled(
            Rect::from_min_max(pos2(xh, inner.min.y), inner.max),
            0.0,
            shade,
        );
    }
    polyline(ui, &view, &c.freqs, &c.error, Stroke::new(1.5_f32, ERROR));
    ui.painter().text(
        pos2(inner.max.x - 6.0, inner.min.y + 3.0),
        Align2::RIGHT_TOP,
        "error (digital - analog), magnified \u{b1}0.05 dB",
        FontId::proportional(10.0),
        LABEL,
    );
}

fn readout_row(ui: &mut Ui, state: &EditorState, rect: Rect) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.horizontal(|ui| {
            if state.curves.is_empty() {
                return;
            }
            for (&hz, (dig, ana)) in scale::READOUT_HZ.iter().zip(state.curves.readout) {
                ui.label(
                    egui::RichText::new(format!(
                        "{}: {}",
                        scale::format_freq(hz),
                        scale::format_db(dig)
                    ))
                    .monospace()
                    .size(11.0),
                )
                .on_hover_text(format!(
                    "analog target {}, error {}",
                    scale::format_db(ana),
                    scale::format_error_db(dig - ana)
                ));
            }
        });
    });
}

fn meter_row(ui: &mut Ui, state: &EditorState, rect: Rect) {
    let meters = state.handles.meters.clone();
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.horizontal(|ui| {
            // clip LED
            let clipped = meters.clipped();
            let (led, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            let color = if clipped {
                Color32::from_rgb(235, 40, 40)
            } else {
                Color32::from_gray(50)
            };
            ui.painter().circle_filled(led.center(), 7.0, color);
            ui.painter().circle_stroke(
                led.center(),
                7.0,
                Stroke::new(1.0_f32, Color32::from_gray(110)),
            );
            ui.label(if clipped {
                "CLIP (over 0 dBFS)"
            } else {
                "no clip"
            });
            if ui
                .button("Reset")
                .on_hover_text("Clear the clip latch and the peak hold")
                .clicked()
            {
                meters.reset();
            }
            ui.label(
                egui::RichText::new(format!("hold {}", scale::format_dbfs(meters.hold())))
                    .monospace()
                    .size(11.0),
            );
        });
        // peak bar
        let (bar, _) = ui.allocate_exact_size(vec2(rect.width() - 8.0, 14.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(bar, 2.0, Color32::from_gray(25));
        let frac = scale::meter_fraction(meters.peak());
        let zero_x = bar.min.x + bar.width() * scale::meter_zero_fraction();
        let fill_to = bar.min.x + bar.width() * frac;
        let green_to = fill_to.min(zero_x);
        p.rect_filled(
            Rect::from_min_max(bar.min, pos2(green_to, bar.max.y)),
            2.0,
            DIGITAL,
        );
        if fill_to > zero_x {
            p.rect_filled(
                Rect::from_min_max(pos2(zero_x, bar.min.y), pos2(fill_to, bar.max.y)),
                0.0,
                Color32::from_rgb(235, 40, 40),
            );
        }
        let hold_x = bar.min.x + bar.width() * scale::meter_fraction(meters.hold());
        p.line_segment(
            [pos2(hold_x, bar.min.y), pos2(hold_x, bar.max.y)],
            Stroke::new(2.0_f32, Color32::WHITE),
        );
        p.line_segment(
            [pos2(zero_x, bar.min.y), pos2(zero_x, bar.max.y)],
            Stroke::new(1.0_f32, Color32::from_gray(160)),
        );
        p.text(
            pos2(bar.min.x + 3.0, bar.center().y),
            Align2::LEFT_CENTER,
            format!("peak {}", scale::format_dbfs(meters.peak())),
            FontId::proportional(10.0),
            Color32::BLACK,
        );
    });
}
