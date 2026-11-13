//! Vinyl page: levels after RIAA pre-emphasis, HF band load, velocity and
//! displacement estimates.

use super::scale::*;
use super::widgets::*;
use super::{EditorState, Frame};
use crate::vinyl::{displacement_um, velocity_cm_s, SIGNALS};
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{self, DragValue, Ui};
use nih_plug_egui::widgets::ParamSlider;

/// Tooltip with the calibration formula (also in the README).
pub const FORMULA_TIP: &str = "Estimates, they depend on the calibration value.\n\
Reference level = dBFS (after RIAA pre-emphasis) that corresponds to 5 cm/s peak lateral velocity at 1 kHz.\n\
velocity [cm/s] = 5 * |x_pre| / 10^(reference / 20)\n\
displacement [um] = 5 * |D| / 10^(reference / 20) * 1e4, D = integral of x_pre dt\n\
(leaky integrator, low cut 10 Hz).\n\
Velocity is assumed proportional to the pre-emphasised signal; lateral = (L+R)/2, vertical = (L-R)/2.";

pub fn show(ui: &mut Ui, st: &mut EditorState, f: &mut Frame, setter: &ParamSetter) {
    let reference = f64::from(st.params.reference_level.value());
    ui.horizontal_wrapped(|ui| {
        ui.label("Reference level").on_hover_text(FORMULA_TIP);
        ui.add(ParamSlider::for_param(&st.params.reference_level, setter).with_width(200.0))
            .on_hover_text(FORMULA_TIP);
        ui.label(egui::RichText::new("= 5 cm/s peak lateral at 1 kHz").color(GREY));
        ui.separator();
        ui.label("HF band from");
        ui.add(
            DragValue::new(&mut f.set.hf_corner_hz)
                .range(3000.0..=10_000.0)
                .speed(20.0)
                .suffix(" Hz"),
        );
        ui.separator();
        ui.label("Pre-emphasis");
        ui.checkbox(&mut f.set.riaa_iec, "IEC").on_hover_text(
            "IEC 7950 us low-frequency roll-off (the bass boost of the pre-emphasis is limited by a 5 Hz high-pass). \
             Changing it restarts the pre-emphasis filter.",
        );
        ui.checkbox(&mut f.set.riaa_ultrasonic, "ultrasonic pole")
            .on_hover_text("3.18 us (about 50 kHz) pole in the pre-emphasis, as used by some cutting systems.");
        ui.separator();
        if ui.button("Reset max").clicked() {
            st.shared.request_reset(crate::shared::reset::VINYL);
        }
    });
    ui.label(
        egui::RichText::new(
            "All levels after RIAA pre-emphasis (what the cutter head sees). Velocity and displacement are \
             estimates that depend on the reference level above. Max holds include start-up transients of a \
             test tone: reset after the tone has settled.",
        )
        .color(GREY)
        .size(11.0),
    );
    ui.add_space(4.0);
    let v = f.ro.vinyl;
    section(ui, "Levels after pre-emphasis", |ui| {
        egui::Grid::new("vinyl-levels")
            .striped(true)
            .num_columns(9)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "",
                    "Peak",
                    "Peak max",
                    "True peak",
                    "TP max",
                    "RMS",
                    "HF RMS",
                    "HF peak",
                    "HF peak max",
                ] {
                    ui.label(egui::RichText::new(h).color(GREY));
                }
                ui.end_row();
                for (k, name) in SIGNALS.iter().enumerate() {
                    let r = v[k];
                    ui.label(*name);
                    for (val, zone) in [
                        (r.peak_db, true),
                        (r.peak_max_db, true),
                        (r.tp_db, true),
                        (r.tp_max_db, true),
                        (r.rms_db, false),
                        (r.hf_rms_db, false),
                        (r.hf_peak_db, false),
                        (r.hf_peak_max_db, false),
                    ] {
                        let c = zone.then(|| rgb(zone_rgb(peak_zone(val))));
                        let mut t = egui::RichText::new(fmt_db(val)).monospace();
                        if let Some(c) = c {
                            t = t.color(c);
                        }
                        ui.label(t);
                    }
                    ui.end_row();
                }
            });
        ui.label(egui::RichText::new("dBFS / dBTP; RMS sine referenced (300 ms); HF band = above the corner, 4th-order Butterworth.").color(GREY).size(11.0));
    });
    section(ui, "Recorded velocity and displacement (estimate)", |ui| {
        egui::Grid::new("vinyl-vel")
            .striped(true)
            .num_columns(5)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "",
                    "Velocity now [cm/s]",
                    "Velocity max [cm/s]",
                    "Displacement now [um]",
                    "Displacement max [um]",
                ] {
                    ui.label(egui::RichText::new(h).color(GREY));
                }
                ui.end_row();
                for (k, name) in SIGNALS.iter().enumerate() {
                    let r = v[k];
                    ui.label(*name);
                    for s in [
                        fmt_velocity(velocity_cm_s(r.tp_db, reference)),
                        fmt_velocity(velocity_cm_s(r.tp_max_db, reference)),
                        fmt_displacement(displacement_um(r.disp_now, reference)),
                        fmt_displacement(displacement_um(r.disp_max, reference)),
                    ] {
                        ui.label(egui::RichText::new(s).monospace().size(15.0));
                    }
                    ui.end_row();
                }
            });
        ui.label(
            egui::RichText::new("Velocity from the true peak; displacement from the leaky integral (10 Hz low cut). Hover the reference level for the formulas.")
                .color(GREY)
                .size(11.0),
        )
        .on_hover_text(FORMULA_TIP);
    });
}
