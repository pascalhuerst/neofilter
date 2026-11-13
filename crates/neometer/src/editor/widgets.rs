//! Small paint helpers shared by the pages. Paint code: not unit-testable,
//! needs visual checking in a host.

use super::scale::{bar_fraction, SILENCE_DB};
use nih_plug_egui::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub const GREY: Color32 = Color32::from_gray(150);
pub const WARN: Color32 = Color32::from_rgb(230, 80, 70);
pub const ACCENT: Color32 = Color32::from_rgb(110, 180, 255);
pub const PLOT_BG: Color32 = Color32::from_gray(18);

/// `label  value unit` on one line with a monospace value.
pub fn readout(ui: &mut Ui, label: &str, value: &str, colour: Option<Color32>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(GREY));
        let mut t = RichText::new(value).monospace().size(15.0);
        if let Some(c) = colour {
            t = t.color(c);
        }
        ui.label(t);
    });
}

/// Large number with a caption underneath.
pub fn big_value(ui: &mut Ui, caption: &str, value: &str, colour: Option<Color32>) {
    ui.vertical(|ui| {
        let mut t = RichText::new(value).monospace().size(26.0);
        if let Some(c) = colour {
            t = t.color(c);
        }
        ui.label(t);
        ui.label(RichText::new(caption).color(GREY).size(11.0));
    });
}

/// Group box with a title.
pub fn section<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.add_space(2.0);
            add(ui)
        })
        .inner
}

/// Row of mutually exclusive choices.
pub fn choice<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, items: &[(T, &str)]) -> bool {
    let mut changed = false;
    for (v, name) in items {
        if ui.selectable_label(*value == *v, *name).clicked() {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// Horizontal level bar: `fill_db` as a filled bar in `fill_colour`, `markers`
/// as thin vertical lines (e.g. held peak, target).
pub fn level_bar(
    ui: &mut Ui,
    size: Vec2,
    min_db: f64,
    max_db: f64,
    fill_db: f64,
    fill_colour: Color32,
    markers: &[(f64, Color32)],
) -> Rect {
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let r = resp.rect;
    painter.rect_filled(r, 2.0, PLOT_BG);
    if fill_db > SILENCE_DB {
        let w = r.width() * bar_fraction(fill_db, min_db, max_db);
        painter.rect_filled(
            Rect::from_min_size(r.min, Vec2::new(w, r.height())),
            2.0,
            fill_colour,
        );
    }
    for &(db, c) in markers {
        if db > SILENCE_DB {
            let x = r.min.x + r.width() * bar_fraction(db, min_db, max_db);
            painter.line_segment(
                [Pos2::new(x, r.min.y), Pos2::new(x, r.max.y)],
                Stroke::new(2.0_f32, c),
            );
        }
    }
    painter.rect_stroke(
        r,
        2.0,
        Stroke::new(1.0_f32, Color32::from_gray(60)),
        StrokeKind::Inside,
    );
    r
}

/// Bipolar bar from -1 to +1 with a centre line and a marker at `v`.
pub fn bipolar_bar(ui: &mut Ui, size: Vec2, v: f64, colour: Color32) {
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let r = resp.rect;
    painter.rect_filled(r, 2.0, PLOT_BG);
    let cx = r.center().x;
    let x = cx + 0.5 * r.width() * v.clamp(-1.0, 1.0) as f32;
    let (a, b) = if x < cx { (x, cx) } else { (cx, x) };
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(a, r.min.y + 2.0), Pos2::new(b, r.max.y - 2.0)),
        1.0,
        colour,
    );
    painter.line_segment(
        [Pos2::new(cx, r.min.y), Pos2::new(cx, r.max.y)],
        Stroke::new(1.0_f32, Color32::from_gray(120)),
    );
    painter.rect_stroke(
        r,
        2.0,
        Stroke::new(1.0_f32, Color32::from_gray(60)),
        StrokeKind::Inside,
    );
}

/// Small caption text inside a painter.
pub fn caption(painter: &egui::Painter, pos: Pos2, anchor: Align2, text: &str, colour: Color32) {
    painter.text(pos, anchor, text, FontId::proportional(11.0), colour);
}
