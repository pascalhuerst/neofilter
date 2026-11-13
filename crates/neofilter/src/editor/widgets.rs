//! Small parameter widgets: drag-or-type values and enum combos, all using
//! `ParamSetter` gestures.

use super::params_io::put;
use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, DragValue, Ui};

/// How a float parameter maps onto the drag widget's value domain.
#[derive(Clone, Copy)]
pub enum Mapping {
    Linear,
    /// Drag in log2 domain (frequency, Q).
    Log2,
}

impl Mapping {
    fn to(self, v: f64) -> f64 {
        match self {
            Self::Linear => v,
            Self::Log2 => v.max(1e-9).log2(),
        }
    }
    fn from(self, d: f64) -> f64 {
        match self {
            Self::Linear => d,
            Self::Log2 => d.exp2(),
        }
    }
}

/// Describes how to show/parse a float parameter.
#[derive(Clone, Copy)]
pub struct FloatEdit {
    pub min: f32,
    pub max: f32,
    pub mapping: Mapping,
    /// Drag speed per pixel in the mapped domain.
    pub speed: f64,
    pub format: fn(f32) -> String,
    pub parse: fn(&str) -> Option<f32>,
}

/// Drag value that also accepts typed text. Returns true when the value changed.
pub fn float_edit(ui: &mut Ui, setter: &ParamSetter, param: &FloatParam, e: FloatEdit) -> bool {
    let m = e.mapping;
    let mut d = m.to(f64::from(param.value()));
    let (fmt, parse) = (e.format, e.parse);
    let resp = ui.add(
        DragValue::new(&mut d)
            .speed(e.speed)
            .range(m.to(f64::from(e.min))..=m.to(f64::from(e.max)))
            .custom_formatter(move |n, _| fmt(m.from(n) as f32))
            .custom_parser(move |s| parse(s).map(|v| m.to(f64::from(v)))),
    );
    let dragging = resp.dragged() || resp.drag_started();
    if resp.drag_started() {
        setter.begin_set_parameter(param);
    }
    let mut changed = false;
    if resp.changed() {
        let v = (m.from(d) as f32).clamp(e.min, e.max);
        if dragging || resp.drag_stopped() {
            setter.set_parameter(param, v);
        } else {
            put(setter, param, v);
        }
        changed = true;
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(param);
    }
    changed
}

/// Combo box for an enum parameter.
pub fn enum_combo<E: Enum + PartialEq + 'static>(
    ui: &mut Ui,
    setter: &ParamSetter,
    id: impl std::hash::Hash,
    param: &EnumParam<E>,
    width: f32,
) {
    let current = param.value().to_index();
    let names = E::variants();
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(names[current])
        .show_ui(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                if ui.selectable_label(i == current, *name).clicked() {
                    put(setter, param, E::from_index(i));
                }
            }
        });
}
