//! Bottom strip: all controls of the selected band.

use super::params_io::{put, remove_band};
use super::scale::band_rgb;
use super::widgets::{enum_combo, float_edit, FloatEdit, Mapping};
use super::EditorState;
use crate::params::{
    format_freq, parse_db, parse_freq, parse_q, FREQ_MAX, FREQ_MIN, GAIN_MAX_DB, Q_MAX, Q_MIN,
};
use neofilter_dsp::types::NUM_BANDS;
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{Color32, RichText, Ui};

pub fn freq_edit() -> FloatEdit {
    FloatEdit {
        min: FREQ_MIN,
        max: FREQ_MAX,
        mapping: Mapping::Log2,
        speed: 0.01,
        format: |hz| format_freq(hz),
        parse: parse_freq,
    }
}

pub fn gain_edit() -> FloatEdit {
    FloatEdit {
        min: -GAIN_MAX_DB,
        max: GAIN_MAX_DB,
        mapping: Mapping::Linear,
        speed: 0.05,
        format: |db| format!("{db:+.2} dB"),
        parse: parse_db,
    }
}

pub fn q_edit() -> FloatEdit {
    FloatEdit {
        min: Q_MIN,
        max: Q_MAX,
        mapping: Mapping::Log2,
        speed: 0.01,
        format: |q| format!("{q:.3}"),
        parse: parse_q,
    }
}

/// Next/previous *used* band index (wrapping); `None` if no band is in use.
pub fn step_band(used: &[bool; NUM_BANDS], from: usize, forward: bool) -> Option<usize> {
    (1..=NUM_BANDS)
        .map(|k| {
            if forward {
                (from + k) % NUM_BANDS
            } else {
                (from + NUM_BANDS - k % NUM_BANDS) % NUM_BANDS
            }
        })
        .find(|&i| used[i])
}

pub fn show(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter) {
    let params = state.params.clone();
    let sel = state.selected.min(NUM_BANDS - 1);
    let snap = params.band_snapshot();
    let mut used = [false; NUM_BANDS];
    for (u, b) in used.iter_mut().zip(snap.iter()) {
        *u = b.used;
    }
    let set = &params.bands[sel];
    let [r, g, b] = band_rgb(sel);
    let col = Color32::from_rgb(r, g, b);

    ui.horizontal(|ui| {
        if ui
            .button("\u{25C0}")
            .on_hover_text("Previous band")
            .clicked()
        {
            if let Some(i) = step_band(&used, sel, false) {
                state.selected = i;
            }
        }
        ui.label(
            RichText::new(format!("\u{25CF} Band {}", sel + 1))
                .color(col)
                .strong(),
        );
        if snap[sel].used {
            let mut on = set.on.value();
            if ui.checkbox(&mut on, "Enabled").changed() {
                put(setter, &set.on, on);
            }
            ui.label("Shape");
            enum_combo(ui, setter, "strip_shape", &set.shape, 90.0);
            if ui.button("Remove").clicked() {
                remove_band(setter, &params, sel);
            }
        } else {
            ui.weak("unused (double-click the display to add a band)");
            if ui.button("Add band").clicked() {
                put(setter, &set.used, true);
            }
        }
        if ui.button("\u{25B6}").on_hover_text("Next band").clicked() {
            if let Some(i) = step_band(&used, sel, true) {
                state.selected = i;
            }
        }
    });

    if !snap[sel].used {
        return;
    }
    ui.horizontal(|ui| {
        ui.label("Freq");
        float_edit(ui, setter, &set.freq, freq_edit());
        ui.label("Gain");
        float_edit(ui, setter, &set.gain, gain_edit());
        ui.label("Q");
        float_edit(ui, setter, &set.q, q_edit());
        ui.label("Slope");
        enum_combo(ui, setter, "strip_slope", &set.slope, 80.0);
        ui.label("Place");
        enum_combo(ui, setter, "strip_place", &set.place, 70.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_wraps_and_skips_unused() {
        let mut u = [false; NUM_BANDS];
        u[2] = true;
        u[7] = true;
        u[NUM_BANDS - 1] = true;
        assert_eq!(step_band(&u, 2, true), Some(7));
        assert_eq!(step_band(&u, NUM_BANDS - 1, true), Some(2));
        assert_eq!(step_band(&u, 2, false), Some(NUM_BANDS - 1));
        assert_eq!(step_band(&u, 7, false), Some(2));
        assert_eq!(step_band(&u, 5, true), Some(7));
        assert_eq!(step_band(&[false; NUM_BANDS], 0, true), None);
        let mut one = [false; NUM_BANDS];
        one[4] = true;
        assert_eq!(step_band(&one, 4, true), Some(4));
    }
}
