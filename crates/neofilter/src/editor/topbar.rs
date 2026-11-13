//! Top bar: preset menu, clipboard presets, global gain controls, analyzer and
//! dB range switches.

use super::params_io::put;
use super::scale::DB_RANGES;
use super::widgets::{float_edit, FloatEdit, Mapping};
use super::EditorState;
use crate::params::{parse_db, parse_percent, GAIN_SCALE_MAX_PCT, OUT_GAIN_MAX_DB};
use crate::presets::{self, PresetFile};
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{self, Event, Ui};
use std::sync::atomic::Ordering;

/// Text pasted into the window this frame. egui-baseview turns Ctrl+V into
/// `Event::Text` carrying the clipboard contents (it never emits
/// `Event::Paste`), while other backends emit `Event::Paste`. Typed characters
/// also arrive as `Event::Text`, so those only count when they look like a
/// preset (JSON object).
fn pasted_text(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::Paste(s) => Some(s.clone()),
        Event::Text(s) if s.trim_start().starts_with('{') => Some(s.clone()),
        _ => None,
    })
}

pub fn show(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter) {
    let params = state.params.clone();
    ui.horizontal_wrapped(|ui| {
        ui.heading("NeoFilter");
        ui.separator();

        egui::ComboBox::from_id_salt("preset_menu")
            .selected_text(if state.preset_name.is_empty() {
                "Presets"
            } else {
                state.preset_name.as_str()
            })
            .show_ui(ui, |ui| {
                for p in presets::factory_presets() {
                    if ui
                        .selectable_label(state.preset_name == p.name, p.name)
                        .clicked()
                    {
                        presets::apply(p, &params, setter);
                        state.preset_name = p.name.to_string();
                        state.status.clear();
                    }
                }
            });

        if ui
            .button("Copy preset")
            .on_hover_text("Copy the current settings as JSON")
            .clicked()
        {
            let snap = params.band_snapshot();
            let file = PresetFile::from_band_params(
                "NeoFilter preset",
                params.gain_scale.value(),
                params.out_gain.value(),
                &snap,
            );
            ui.ctx().copy_text(file.to_json());
            state.status = "Preset copied to clipboard".into();
        }
        let armed = state.paste_armed;
        if ui
            .selectable_label(armed, "Paste preset")
            .on_hover_text("Click, then press Ctrl+V to apply a copied preset")
            .clicked()
        {
            state.paste_armed = !armed;
            state.status = if state.paste_armed {
                "Press Ctrl+V to paste a preset".into()
            } else {
                String::new()
            };
        }
        if state.paste_armed {
            let pasted = ui.ctx().input(|i| pasted_text(&i.events));
            if let Some(text) = pasted {
                state.paste_armed = false;
                match PresetFile::from_json(&text) {
                    Ok(file) => {
                        presets::apply_file(&file, &params, setter);
                        state.preset_name = file.name.clone();
                        state.status = "Preset applied".into();
                    }
                    Err(e) => state.status = format!("Paste failed: {e}"),
                }
            }
        }

        ui.separator();
        ui.label("Gain scale");
        float_edit(
            ui,
            setter,
            &params.gain_scale,
            FloatEdit {
                min: 0.0,
                max: GAIN_SCALE_MAX_PCT,
                mapping: Mapping::Linear,
                speed: 0.5,
                format: |p| format!("{p:.0} %"),
                parse: parse_percent,
            },
        );
        ui.label("Output");
        float_edit(
            ui,
            setter,
            &params.out_gain,
            FloatEdit {
                min: -OUT_GAIN_MAX_DB,
                max: OUT_GAIN_MAX_DB,
                mapping: Mapping::Linear,
                speed: 0.05,
                format: |db| format!("{db:+.2} dB"),
                parse: parse_db,
            },
        );
        let mut bypass = params.bypass.value();
        if ui.checkbox(&mut bypass, "Bypass").changed() {
            put(setter, &params.bypass, bypass);
        }

        ui.separator();
        let shared = &state.handles.analyzer_shared;
        let mut on = shared.enabled();
        if ui.checkbox(&mut on, "Analyzer").changed() {
            shared.set_enabled(on);
            if on {
                if let Ok(mut a) = state.handles.analyzer.try_lock() {
                    a.clear();
                }
            }
        }
        let cur = state.handles.settings.db_range_idx.load(Ordering::Relaxed) as usize;
        for (i, r) in DB_RANGES.iter().enumerate() {
            if ui
                .selectable_label(i == cur, format!("\u{00B1}{r:.0} dB"))
                .clicked()
            {
                state
                    .handles
                    .settings
                    .db_range_idx
                    .store(i as u8, Ordering::Relaxed);
            }
        }
        if !state.status.is_empty() {
            ui.separator();
            ui.weak(&state.status);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_arrives_as_text_with_egui_baseview() {
        let json = "{ \"version\": 1 }";
        assert_eq!(
            pasted_text(&[Event::Text(json.into())]),
            Some(json.to_string())
        );
        assert_eq!(
            pasted_text(&[Event::Paste(json.into())]),
            Some(json.to_string())
        );
        // typed characters are not a paste
        assert_eq!(pasted_text(&[Event::Text("a".into())]), None);
        assert_eq!(pasted_text(&[]), None);
    }
}
