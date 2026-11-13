//! Stereo page: goniometer, correlation, balance, M/S and the low-frequency
//! vertical/lateral meters.

use super::scale::*;
use super::widgets::*;
use super::{EditorState, Frame};
use crate::settings::{GonioMode, CORR_TAUS};
use crate::shared::reset;
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{self, Align2, Color32, DragValue, Pos2, Sense, Stroke, Ui, Vec2};
use nih_plug_egui::widgets::ParamSlider;

fn goniometer(ui: &mut Ui, pts: &[[f32; 2]], mode: GonioMode, side: f32) {
    let (resp, painter) = ui.allocate_painter(Vec2::splat(side), Sense::hover());
    let r = resp.rect;
    painter.rect_filled(r, 4.0, PLOT_BG);
    let c = r.center();
    let rad = side * 0.45;
    let axis = Stroke::new(1.0_f32, Color32::from_gray(50));
    painter.line_segment([Pos2::new(c.x - rad, c.y), Pos2::new(c.x + rad, c.y)], axis);
    painter.line_segment([Pos2::new(c.x, c.y - rad), Pos2::new(c.x, c.y + rad)], axis);
    painter.circle_stroke(c, rad, axis);
    let d = rad * std::f32::consts::FRAC_1_SQRT_2;
    let diag = Stroke::new(1.0_f32, Color32::from_gray(36));
    painter.line_segment(
        [Pos2::new(c.x - d, c.y - d), Pos2::new(c.x + d, c.y + d)],
        diag,
    );
    painter.line_segment(
        [Pos2::new(c.x - d, c.y + d), Pos2::new(c.x + d, c.y - d)],
        diag,
    );
    match mode {
        GonioMode::MidSide => {
            caption(
                &painter,
                Pos2::new(c.x, r.min.y + 4.0),
                Align2::CENTER_TOP,
                "M",
                GREY,
            );
            caption(
                &painter,
                Pos2::new(r.max.x - 6.0, c.y),
                Align2::RIGHT_CENTER,
                "S",
                GREY,
            );
            caption(
                &painter,
                Pos2::new(c.x - d - 6.0, c.y - d - 6.0),
                Align2::RIGHT_BOTTOM,
                "L",
                GREY,
            );
            caption(
                &painter,
                Pos2::new(c.x + d + 6.0, c.y - d - 6.0),
                Align2::LEFT_BOTTOM,
                "R",
                GREY,
            );
        }
        GonioMode::LeftRight => {
            caption(
                &painter,
                Pos2::new(c.x, r.min.y + 4.0),
                Align2::CENTER_TOP,
                "R",
                GREY,
            );
            caption(
                &painter,
                Pos2::new(r.max.x - 6.0, c.y),
                Align2::RIGHT_CENTER,
                "L",
                GREY,
            );
        }
    }
    let n = pts.len().max(1);
    for (i, p) in pts.iter().enumerate() {
        let [x, y] = gonio_xy(*p, mode);
        let a = 30 + (200 * i / n) as u8;
        painter.circle_filled(
            Pos2::new(c.x + x * rad, c.y - y * rad),
            1.2,
            Color32::from_rgba_unmultiplied(120, 220, 140, a),
        );
    }
}

pub fn show(ui: &mut Ui, st: &mut EditorState, f: &mut Frame, setter: &ParamSetter) {
    let s = f.ro.stereo;
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        ui.horizontal(|ui| {
            choice(
                ui,
                &mut f.set.gonio_mode,
                &[(GonioMode::MidSide, "M/S (rotated)"), (GonioMode::LeftRight, "L/R")],
            );
        });
        let side = ui.available_width().min(ui.available_height() - 10.0).clamp(160.0, 520.0);
        goniometer(ui, &f.snap.gonio, f.set.gonio_mode, side);

        let ui = &mut cols[1];
        section(ui, "Correlation", |ui| {
            bipolar_bar(
                ui,
                Vec2::new(ui.available_width().min(420.0), 18.0),
                s.correlation,
                rgb(correlation_rgb(s.correlation)),
            );
            ui.horizontal(|ui| {
                readout(ui, "Correlation", &fmt_corr(s.correlation), Some(rgb(correlation_rgb(s.correlation))));
                ui.add_space(12.0);
                ui.label("Integration");
                for t in CORR_TAUS {
                    if ui
                        .selectable_label((f.set.corr_tau_s - t).abs() < 1e-6, format!("{t} s"))
                        .clicked()
                    {
                        f.set.corr_tau_s = t;
                    }
                }
            });
        });
        section(ui, "Balance and mid / side", |ui| {
            ui.horizontal(|ui| {
                readout(ui, "Balance L-R", &format!("{} dB", fmt_signed(s.balance_db, 2)), None);
                ui.add_space(12.0);
                readout(ui, "Mid", &format!("{} dB", fmt_db(s.mid_db)), None);
                ui.add_space(12.0);
                readout(ui, "Side", &format!("{} dB", fmt_db(s.side_db)), None);
            });
            ui.label(
                egui::RichText::new("M = (L+R)/2, S = (L-R)/2; RMS, sine referenced, integrated with the time constant above.")
                    .color(GREY)
                    .size(11.0),
            );
        });
        let w = lf_warning(
            s.lf_vertical_db,
            s.lf_lateral_db,
            s.lf_correlation,
            f64::from(f.set.vert_warn_db),
            f64::from(f.set.lf_corr_warn),
        );
        section(ui, "Low-frequency vertical / lateral (disc cutting)", |ui| {
            ui.horizontal(|ui| {
                ui.label("Crossover");
                ui.add(ParamSlider::for_param(&st.params.lf_crossover, setter).with_width(180.0));
            });
            let warn_v = if w.vertical { Some(WARN) } else { None };
            let warn_c = if w.correlation { Some(WARN) } else { None };
            ui.horizontal(|ui| {
                readout(ui, "Lateral", &format!("{} dB", fmt_db(s.lf_lateral_db)), None);
                ui.add_space(12.0);
                readout(ui, "Vertical", &format!("{} dB", fmt_db(s.lf_vertical_db)), warn_v);
                ui.add_space(12.0);
                readout(
                    ui,
                    "V - L",
                    &format!("{} dB", fmt_signed(s.lf_vertical_db - s.lf_lateral_db, 2)),
                    warn_v,
                );
            });
            readout(ui, "LF correlation", &fmt_corr(s.lf_correlation), warn_c);
            bipolar_bar(
                ui,
                Vec2::new(ui.available_width().min(420.0), 14.0),
                s.lf_correlation,
                if w.correlation { WARN } else { rgb(correlation_rgb(s.lf_correlation)) },
            );
            if w.any() {
                ui.colored_label(
                    WARN,
                    if w.vertical && w.correlation {
                        "Warning: vertical bass exceeds lateral and LF phase is negative"
                    } else if w.vertical {
                        "Warning: vertical bass exceeds lateral (stylus lift / cut-through risk)"
                    } else {
                        "Warning: low-frequency correlation is negative"
                    },
                );
            }
            ui.horizontal(|ui| {
                ui.label("Warn when V - L >");
                ui.add(DragValue::new(&mut f.set.vert_warn_db).range(-30.0..=30.0).speed(0.2).suffix(" dB"));
                ui.label("or corr <");
                ui.add(DragValue::new(&mut f.set.lf_corr_warn).range(-1.0..=1.0).speed(0.01));
            });
            ui.label(
                egui::RichText::new("Band below the crossover: 4th-order Linkwitz-Riley low-pass on L and R, then M/S levels and correlation.")
                    .color(GREY)
                    .size(11.0),
            );
        });
        if ui.button("Reset").clicked() {
            st.shared.request_reset(reset::STEREO);
        }
    });
}
