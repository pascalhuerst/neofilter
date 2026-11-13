//! Levels & Loudness page.

use super::scale::*;
use super::widgets::*;
use super::{EditorState, Frame};
use crate::settings::TARGET_LUFS;
use crate::shared::reset;
use nih_plug_egui::egui::{self, Color32, DragValue, Ui, Vec2};

pub fn show(ui: &mut Ui, st: &mut EditorState, f: &mut Frame) {
    let lv = f.ro.levels;
    let ld = f.snap.loudness;
    ui.columns(2, |cols| {
        // ---- per-channel levels
        let ui = &mut cols[0];
        for (c, name) in ["L", "R"].iter().enumerate() {
            section(ui, &format!("Channel {name}"), |ui| {
                let zone = rgb(zone_rgb(peak_zone(lv.tp_db[c].max(lv.peak_db[c]))));
                level_bar(
                    ui,
                    Vec2::new(ui.available_width().min(420.0), 16.0),
                    -60.0,
                    3.0,
                    lv.rms_db[c],
                    Color32::from_rgb(80, 130, 200),
                    &[(lv.peak_db[c], zone), (lv.tp_max_db[c], Color32::from_gray(200))],
                );
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        readout(ui, "Peak (hold)", &format!("{} dBFS", fmt_db(lv.peak_db[c])), None);
                        readout(
                            ui,
                            "Peak max",
                            &format!("{} dBFS", fmt_db(lv.peak_max_db[c])),
                            Some(rgb(zone_rgb(peak_zone(lv.peak_max_db[c])))),
                        );
                        readout(ui, "RMS", &format!("{} dB", fmt_db(lv.rms_db[c])), None);
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        readout(ui, "True peak (hold)", &format!("{} dBTP", fmt_db(lv.tp_db[c])), None);
                        readout(
                            ui,
                            "True peak max",
                            &format!("{} dBTP", fmt_db(lv.tp_max_db[c])),
                            Some(rgb(zone_rgb(peak_zone(lv.tp_max_db[c])))),
                        );
                        readout(ui, "Crest", &format!("{} dB", fmt_db(lv.crest_db[c])), None);
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        readout(ui, "DC offset", &fmt_dc(lv.dc[c]), None);
                    });
                });
            });
        }
        ui.label(
            egui::RichText::new(
                "RMS: 300 ms window, sine referenced (a full-scale sine reads 0 dB). Crest and DC \
                 are measured since the last reset. The bar shows RMS; markers: held peak, max true peak.",
            )
            .color(GREY)
            .size(11.0),
        );

        // ---- loudness
        let ui = &mut cols[1];
        section(ui, "Loudness (ITU-R BS.1770-4, LUFS)", |ui| {
            ui.horizontal(|ui| {
                big_value(ui, "Momentary", &fmt_lufs(Some(ld.momentary)), None);
                ui.add_space(18.0);
                big_value(ui, "Short-term", &fmt_lufs(Some(ld.short_term)), None);
                ui.add_space(18.0);
                big_value(ui, "Integrated", &fmt_lufs(ld.integrated), Some(ACCENT));
            });
            ui.add_space(6.0);
            let target = f.set.target_lufs;
            level_bar(
                ui,
                Vec2::new(ui.available_width().min(460.0), 18.0),
                -40.0,
                0.0,
                ld.momentary,
                Color32::from_rgb(80, 130, 200),
                &[
                    (ld.short_term, Color32::from_rgb(230, 190, 60)),
                    (target, Color32::from_rgb(230, 120, 220)),
                ],
            );
            ui.label(
                egui::RichText::new("bar: momentary, yellow: short-term, magenta: target (visual marker only)")
                    .color(GREY)
                    .size(11.0),
            );
            ui.horizontal(|ui| {
                readout(ui, "Loudness range", &ld.range.map_or("--".into(), |r| format!("{:.2} LU", r.lra)), None);
                if let Some(r) = ld.range {
                    ui.label(
                        egui::RichText::new(format!("({:.2} .. {:.2})", r.low, r.high)).color(GREY),
                    );
                }
            });
            ui.horizontal(|ui| {
                readout(ui, "Max momentary", &fmt_lufs(Some(ld.max_momentary)), None);
                ui.add_space(12.0);
                readout(ui, "Max short-term", &fmt_lufs(Some(ld.max_short_term)), None);
            });
            let max_tp = max_db(lv.tp_max_db[0], lv.tp_max_db[1]);
            readout(
                ui,
                "Max true peak",
                &format!("{} dBTP", fmt_db(max_tp)),
                Some(rgb(zone_rgb(peak_zone(max_tp)))),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Target");
                for t in TARGET_LUFS {
                    let sel = !f.set.target_custom && (f.set.target_lufs - t).abs() < 1e-9;
                    if ui.selectable_label(sel, format!("{t:.0}")).clicked() {
                        f.set.target_lufs = t;
                        f.set.target_custom = false;
                    }
                }
                if ui.selectable_label(f.set.target_custom, "Custom").clicked() {
                    f.set.target_custom = true;
                }
                if f.set.target_custom {
                    ui.add(
                        DragValue::new(&mut f.set.target_lufs)
                            .range(-60.0..=0.0)
                            .speed(0.1)
                            .suffix(" LUFS"),
                    );
                }
            });
            if let Some(d) = target_delta(ld.integrated, f.set.target_lufs) {
                readout(ui, "Integrated vs target", &format!("{} LU", fmt_signed(d, 2)), None);
            }
        });
        ui.add_space(6.0);
        if ui.button("Reset all").on_hover_text("Clear peak holds, maxima, DC/crest statistics and the loudness integration").clicked() {
            st.shared.request_reset(reset::LEVELS | reset::LOUDNESS | reset::STEREO | reset::VINYL);
        }
    });
}
