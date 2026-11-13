//! Spectrum page: hi-res spectrum / 1/3-octave RTA, controls, cursor readout.

use super::scale::*;
use super::widgets::*;
use super::{EditorState, Frame};
use crate::analysis::{Chan, SpectrumData, SPECTRUM_POINTS};
use crate::settings::{AvgMode, SpecChannel, WindowKind, FFT_SIZES, SMOOTHING};
use crate::shared::reset;
use nih_plug_egui::egui::{
    self, Align2, Color32, DragValue, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2,
};
use std::sync::Arc;

fn chan_colour(c: Chan) -> Color32 {
    match c {
        Chan::Left => Color32::from_rgb(90, 170, 255),
        Chan::Right => Color32::from_rgb(255, 150, 80),
        Chan::Mid => Color32::from_rgb(120, 220, 140),
        Chan::Side => Color32::from_rgb(230, 120, 220),
    }
}

fn smoothing_name(n: u32) -> String {
    if n == 0 {
        "Off".into()
    } else {
        format!("1/{n} oct")
    }
}

pub fn show(ui: &mut Ui, st: &mut EditorState, f: &mut Frame) {
    // ---- controls
    ui.horizontal_wrapped(|ui| {
        ui.label("FFT");
        egui::ComboBox::from_id_salt("fft")
            .selected_text(f.set.fft_size.to_string())
            .show_ui(ui, |ui| {
                for n in FFT_SIZES {
                    ui.selectable_value(&mut f.set.fft_size, n, n.to_string());
                }
            });
        ui.label("Window");
        egui::ComboBox::from_id_salt("win")
            .selected_text(f.set.window.name())
            .show_ui(ui, |ui| {
                for w in WindowKind::ALL {
                    ui.selectable_value(&mut f.set.window, w, w.name());
                }
            });
        ui.label("Averaging");
        choice(
            ui,
            &mut f.set.averaging,
            &[
                (AvgMode::Off, "Off"),
                (AvgMode::Exponential, "Exp"),
                (AvgMode::Infinite, "Inf"),
            ],
        );
        if f.set.averaging == AvgMode::Exponential {
            ui.add(
                DragValue::new(&mut f.set.avg_tau_s)
                    .range(0.05..=30.0)
                    .speed(0.02)
                    .suffix(" s"),
            );
        }
        ui.label("Smoothing");
        egui::ComboBox::from_id_salt("smooth")
            .selected_text(smoothing_name(f.set.smoothing))
            .show_ui(ui, |ui| {
                for n in SMOOTHING {
                    ui.selectable_value(&mut f.set.smoothing, n, smoothing_name(n));
                }
            });
        ui.label("Channel");
        egui::ComboBox::from_id_salt("chan")
            .selected_text(f.set.spec_channel.name())
            .show_ui(ui, |ui| {
                for c in SpecChannel::ALL {
                    ui.selectable_value(&mut f.set.spec_channel, c, c.name());
                }
            });
    });
    ui.horizontal_wrapped(|ui| {
        ui.toggle_value(&mut f.set.rta, "1/3-oct RTA");
        ui.toggle_value(&mut f.set.peak_hold, "Peak hold");
        let frozen = st.frozen.is_some();
        if ui.selectable_label(frozen, "Freeze").clicked() {
            st.frozen = if frozen {
                None
            } else {
                f.snap.spectrum.clone()
            };
        }
        if ui.button("Reset avg/peak").clicked() {
            st.shared.request_reset(reset::SPECTRUM);
        }
        if ui.button("Reset peak").clicked() {
            st.shared.request_reset(reset::SPEC_PEAK);
        }
        ui.separator();
        ui.label("dB");
        ui.add(
            DragValue::new(&mut f.set.db_min)
                .range(-200.0..=-10.0)
                .speed(1.0),
        );
        ui.label("..");
        ui.add(
            DragValue::new(&mut f.set.db_max)
                .range(-100.0..=24.0)
                .speed(1.0),
        );
        ui.label("Hz");
        ui.add(
            DragValue::new(&mut f.set.f_lo)
                .range(5.0..=2000.0)
                .speed(1.0),
        );
        ui.label("..");
        ui.add(
            DragValue::new(&mut f.set.f_hi)
                .range(100.0..=96_000.0)
                .speed(20.0),
        );
        if ui.button("20-20k").clicked() {
            f.set.f_lo = 20.0;
            f.set.f_hi = 20_000.0;
        }
    });

    let sp: Option<Arc<SpectrumData>> = st.frozen.clone().or_else(|| f.snap.spectrum.clone());
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        Vec2::new(avail.x, (avail.y - 4.0).max(120.0)),
        Sense::hover(),
    );
    let full = resp.rect;
    painter.rect_filled(full, 2.0, PLOT_BG);
    let plot = Rect::from_min_max(
        Pos2::new(full.min.x + 40.0, full.min.y + 8.0),
        Pos2::new(full.max.x - 10.0, full.max.y - 20.0),
    );
    let (f_lo, f_hi) = match &sp {
        Some(s) if !s.rta => (s.f_lo as f32, s.f_hi as f32),
        _ => (
            f.set.f_lo,
            f.set
                .f_hi
                .min(f.snap.sample_rate as f32 * 0.5 * 0.999)
                .max(f.set.f_lo * 1.5),
        ),
    };
    let view = SpecView {
        x0: plot.min.x,
        y0: plot.min.y,
        w: plot.width(),
        h: plot.height(),
        f_lo,
        f_hi,
        db_min: f.set.db_min,
        db_max: f.set.db_max,
    };
    // grid
    for (fr, label) in freq_ticks(f64::from(f_lo), f64::from(f_hi)) {
        let x = view.freq_to_x(fr as f32);
        painter.line_segment(
            [Pos2::new(x, plot.min.y), Pos2::new(x, plot.max.y)],
            Stroke::new(1.0_f32, Color32::from_gray(if label { 52 } else { 32 })),
        );
        if label {
            caption(
                &painter,
                Pos2::new(x, plot.max.y + 3.0),
                Align2::CENTER_TOP,
                &freq_label(fr),
                GREY,
            );
        }
    }
    for db in db_ticks(f64::from(f.set.db_min), f64::from(f.set.db_max)) {
        let y = view.db_to_y(db as f32);
        painter.line_segment(
            [Pos2::new(plot.min.x, y), Pos2::new(plot.max.x, y)],
            Stroke::new(1.0_f32, Color32::from_gray(40)),
        );
        caption(
            &painter,
            Pos2::new(plot.min.x - 4.0, y),
            Align2::RIGHT_CENTER,
            &format!("{db:.0}"),
            GREY,
        );
    }

    let Some(sp) = sp else {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "waiting for audio ...",
            egui::FontId::proportional(14.0),
            GREY,
        );
        return;
    };
    // curves
    let clip = painter.with_clip_rect(plot);
    for curve in &sp.curves {
        let col = chan_colour(curve.chan);
        if sp.rta {
            let w = |fc: f32| {
                (view.freq_to_x(fc * 2f32.powf(1.0 / 6.0))
                    - view.freq_to_x(fc / 2f32.powf(1.0 / 6.0)))
                .max(2.0)
            };
            for &(fc, db) in &curve.bands {
                let x = view.freq_to_x(fc);
                let bw = w(fc) * if sp.curves.len() > 1 { 0.42 } else { 0.8 };
                let off = match (sp.curves.len(), curve.chan) {
                    (2, Chan::Right) => bw * 0.5,
                    (2, _) => -bw * 0.5,
                    _ => 0.0,
                };
                let y = view.db_to_y(db);
                clip.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(x + off - bw * 0.5, y),
                        Pos2::new(x + off + bw * 0.5, plot.max.y),
                    ),
                    1.0,
                    Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 170),
                );
            }
        } else {
            let line = |db: &[f32], c: Color32, width: f32| {
                let pts: Vec<Pos2> = db
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        Pos2::new(
                            view.freq_to_x(sp.grid_hz(i) as f32),
                            view.db_to_y(v.max(-300.0)),
                        )
                    })
                    .collect();
                if pts.len() == SPECTRUM_POINTS {
                    clip.add(Shape::line(pts, Stroke::new(width, c)));
                }
            };
            if !curve.peak_db.is_empty() {
                line(
                    &curve.peak_db,
                    Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 110),
                    1.0,
                );
            }
            line(&curve.db, col, 1.5);
        }
    }
    // cursor readout
    if let Some(p) = resp.hover_pos().filter(|p| plot.contains(*p)) {
        let fq = f64::from(view.x_to_freq(p.x));
        clip.line_segment(
            [Pos2::new(p.x, plot.min.y), Pos2::new(p.x, plot.max.y)],
            Stroke::new(1.0_f32, Color32::from_gray(140)),
        );
        let mut lines = vec![format!("{}  ({:.1} dB)", fmt_freq(fq), view.y_to_db(p.y))];
        for curve in &sp.curves {
            let r = if sp.rta {
                band_readout(&curve.bands, fq)
            } else {
                spectrum_readout(&sp, &curve.db, fq)
            };
            if let Some((fr, db)) = r {
                lines.push(format!(
                    "{} {}: {} dB",
                    curve.chan.name(),
                    fmt_freq(fr),
                    fmt_db(db)
                ));
            }
        }
        let mut y = plot.min.y + 6.0;
        for l in lines {
            painter.text(
                Pos2::new(plot.max.x - 8.0, y),
                Align2::RIGHT_TOP,
                l,
                egui::FontId::monospace(12.0),
                Color32::WHITE,
            );
            y += 15.0;
        }
    }
    let info = format!(
        "{} pt, {:.2} Hz/bin, {} frames{}",
        sp.fft_size,
        sp.bin_hz,
        sp.frames,
        if st.frozen.is_some() { "  FROZEN" } else { "" }
    );
    caption(
        &painter,
        Pos2::new(plot.min.x + 6.0, plot.min.y + 4.0),
        Align2::LEFT_TOP,
        &info,
        GREY,
    );
}
