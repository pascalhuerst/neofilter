//! Main display: grid, analyzer, response curves, band nodes and their
//! interactions. Paint code; not unit-testable (logic lives in `scale`).

use super::params_io::{put, remove_band, write_band};
use super::scale::{self, band_rgb, View, DB_RANGES, FREQ_LABELS};
use super::{Drag, EditorState, MenuTarget};
use crate::analyzer::column_levels;
use crate::params::{format_freq, BandPlace, BandShape, BandSlope};
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, Placement, Shape, Slope};
use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, epaint::Mesh, pos2, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, Ui,
};
use std::sync::atomic::Ordering;

const MARGIN_LEFT: f32 = 36.0;
const MARGIN_BOTTOM: f32 = 18.0;
const NODE_RADIUS: f32 = 7.0;
const HIT_RADIUS: f32 = 11.0;
/// Analyzer display span (dBFS) mapped over the full plot height.
const SPEC_TOP_DB: f32 = 0.0;
const SPEC_BOTTOM_DB: f32 = -90.0;

fn color(i: usize) -> Color32 {
    let [r, g, b] = band_rgb(i);
    Color32::from_rgb(r, g, b)
}

/// Filled area between a polyline and a horizontal line.
fn fill_to(ui: &Ui, pts: &[Pos2], base_y: f32, fill: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = Mesh::default();
    for p in pts {
        mesh.colored_vertex(*p, fill);
        mesh.colored_vertex(pos2(p.x, base_y), fill);
    }
    for i in 0..pts.len() as u32 - 1 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    ui.painter().add(egui::Shape::mesh(mesh));
}

fn spec_y(view: &View, db: f32) -> f32 {
    let t = ((db - SPEC_BOTTOM_DB) / (SPEC_TOP_DB - SPEC_BOTTOM_DB)).clamp(0.0, 1.0);
    view.y0 + view.height * (1.0 - t)
}

fn draw_grid(ui: &Ui, view: &View) {
    let p = ui.painter();
    let minor = Stroke::new(1.0_f32, Color32::from_gray(34));
    let major = Stroke::new(1.0_f32, Color32::from_gray(52));
    let font = FontId::proportional(10.0);
    let bottom = view.y0 + view.height;
    for f in scale::minor_freq_ticks() {
        let x = view.freq_to_x(f);
        p.line_segment([pos2(x, view.y0), pos2(x, bottom)], minor);
    }
    for (f, label) in FREQ_LABELS {
        let x = view.freq_to_x(f);
        p.line_segment([pos2(x, view.y0), pos2(x, bottom)], major);
        p.text(
            pos2(x, bottom + 2.0),
            Align2::CENTER_TOP,
            label,
            font.clone(),
            Color32::from_gray(150),
        );
    }
    for db in scale::db_ticks(view.db_range) {
        let y = view.db_to_y(db);
        let stroke = if db == 0.0 {
            Stroke::new(1.0_f32, Color32::from_gray(90))
        } else {
            major
        };
        p.line_segment([pos2(view.x0, y), pos2(view.x0 + view.width, y)], stroke);
        p.text(
            pos2(view.x0 - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{db:+.0}"),
            font.clone(),
            Color32::from_gray(150),
        );
    }
}

fn draw_analyzer(ui: &Ui, state: &mut EditorState, view: &View, n: usize) {
    let shared = state.handles.analyzer_shared.clone();
    if !shared.enabled() {
        return;
    }
    state.pre_cols.resize(n, scale_floor());
    state.post_cols.resize(n, scale_floor());
    let Ok(mut a) = state.handles.analyzer.try_lock() else {
        return;
    };
    a.update();
    let bin_hz = a.bin_hz();
    column_levels(
        a.pre_db(),
        bin_hz,
        scale::DISPLAY_F_MIN,
        scale::DISPLAY_F_MAX,
        &mut state.pre_cols,
    );
    column_levels(
        a.post_db(),
        bin_hz,
        scale::DISPLAY_F_MIN,
        scale::DISPLAY_F_MAX,
        &mut state.post_cols,
    );
    drop(a);
    let bottom = view.y0 + view.height;
    let to_pts = |cols: &[f32]| -> Vec<Pos2> {
        let d = (n.max(2) - 1) as f32;
        cols.iter()
            .enumerate()
            .map(|(i, &db)| pos2(view.x0 + view.width * i as f32 / d, spec_y(view, db)))
            .collect()
    };
    let pre = to_pts(&state.pre_cols);
    let post = to_pts(&state.post_cols);
    fill_to(
        ui,
        &pre,
        bottom,
        Color32::from_rgba_unmultiplied(150, 150, 150, 28),
    );
    ui.painter().add(egui::Shape::line(
        pre,
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(170, 170, 170, 90)),
    ));
    fill_to(
        ui,
        &post,
        bottom,
        Color32::from_rgba_unmultiplied(70, 150, 230, 40),
    );
    ui.painter().add(egui::Shape::line(
        post,
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(110, 180, 255, 150)),
    ));
}

fn scale_floor() -> f32 {
    crate::analyzer::FLOOR_DB
}

fn curve_points(view: &View, db: &[f64]) -> Vec<Pos2> {
    let d = (db.len().max(2) - 1) as f32;
    db.iter()
        .enumerate()
        .map(|(i, &v)| {
            pos2(
                view.x0 + view.width * i as f32 / d,
                view.db_to_y(v as f32)
                    .clamp(view.y0 - 2.0, view.y0 + view.height + 2.0),
            )
        })
        .collect()
}

pub fn show(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter, rect: Rect) {
    let params = state.params.clone();
    let snap = params.band_snapshot();
    let gs = params.gain_scale_factor();
    let range_idx = state.handles.settings.db_range_idx.load(Ordering::Relaxed) as usize;
    let db_range = DB_RANGES[range_idx.min(DB_RANGES.len() - 1)];
    let view = View {
        x0: rect.min.x + MARGIN_LEFT,
        y0: rect.min.y + 6.0,
        width: (rect.width() - MARGIN_LEFT - 10.0).max(50.0),
        height: (rect.height() - MARGIN_BOTTOM - 6.0).max(50.0),
        db_range,
    };
    let plot_rect =
        Rect::from_min_size(pos2(view.x0, view.y0), egui::vec2(view.width, view.height));
    let response = ui.interact(plot_rect, ui.id().with("plot"), Sense::click_and_drag());
    let painter = ui.painter().clone();
    painter.rect_filled(plot_rect, 2.0, Color32::from_gray(18));

    draw_grid(ui, &view);

    let n = ((view.width / 2.0) as usize).max(16);
    draw_analyzer(ui, state, &view, n);

    // Curves.
    let sr = f64::from(state.handles.analyzer_shared.sample_rate());
    state.curves.update(&snap, f64::from(gs), sr, n);
    let zero_y = view.db_to_y(0.0);
    for (i, b) in snap.iter().enumerate() {
        if !b.used || !b.enabled {
            continue;
        }
        let c = color(i);
        let pts = curve_points(&view, &state.curves.band_db[i]);
        if i == state.selected {
            let [r, g, bl, _] = c.to_array();
            fill_to(
                ui,
                &pts,
                zero_y,
                Color32::from_rgba_unmultiplied(r, g, bl, 50),
            );
            painter.add(egui::Shape::line(pts, Stroke::new(1.5_f32, c)));
        } else {
            let [r, g, bl, _] = c.to_array();
            painter.add(egui::Shape::line(
                pts,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(r, g, bl, 120)),
            ));
        }
    }
    painter.add(egui::Shape::line(
        curve_points(&view, &state.curves.total_db),
        Stroke::new(2.5_f32, Color32::from_rgb(255, 214, 90)),
    ));

    // Interaction.
    let nodes = scale::nodes(&view, &snap, gs);
    let hover = ui.ctx().pointer_hover_pos();
    let hovered = hover
        .filter(|_| plot_rect.contains(hover.unwrap_or_default()))
        .and_then(|p| scale::hit_test(&nodes, p.x, p.y, HIT_RADIUS));
    if hovered.is_some() || state.drag.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    if response.drag_started() {
        let origin = ui.ctx().input(|i| i.pointer.press_origin());
        if let Some(band) = origin.and_then(|p| scale::hit_test(&nodes, p.x, p.y, HIT_RADIUS)) {
            let b = &snap[band];
            let has_gain = scale::shape_has_gain(b.shape);
            state.selected = band;
            state.drag = Some(Drag {
                band,
                start_freq: b.freq_hz as f32,
                start_gain: b.gain_db as f32,
                has_gain,
                ax: 0.0,
                ay: 0.0,
            });
            setter.begin_set_parameter(&params.bands[band].freq);
            if has_gain {
                setter.begin_set_parameter(&params.bands[band].gain);
            }
        }
    }
    if response.dragged() {
        if let Some(d) = state.drag.as_mut() {
            let (fine, cmd) = ui.ctx().input(|i| (i.modifiers.shift, i.modifiers.command));
            let k = if fine { 0.1 } else { 1.0 };
            let delta = response.drag_delta();
            d.ax += delta.x * k;
            d.ay += delta.y * k;
            let (lock_x, lock_y) = if cmd {
                (d.ay.abs() > d.ax.abs(), d.ax.abs() >= d.ay.abs())
            } else {
                (false, false)
            };
            let shape = Shape::from(params.bands[d.band].shape.value());
            let r = scale::drag_to(
                &view,
                shape,
                d.start_freq,
                d.start_gain,
                gs,
                d.ax,
                d.ay,
                lock_x,
                lock_y,
            );
            setter.set_parameter(&params.bands[d.band].freq, r.freq);
            if let (Some(g), true) = (r.gain, d.has_gain) {
                setter.set_parameter(&params.bands[d.band].gain, g);
            }
        }
    }
    if response.drag_stopped() {
        if let Some(d) = state.drag.take() {
            setter.end_set_parameter(&params.bands[d.band].freq);
            if d.has_gain {
                setter.end_set_parameter(&params.bands[d.band].gain);
            }
        }
    }

    let click_pos = response.interact_pointer_pos();
    if response.clicked() {
        if let Some(b) = click_pos.and_then(|p| scale::hit_test(&nodes, p.x, p.y, HIT_RADIUS)) {
            state.selected = b;
        }
    }
    if response.double_clicked() {
        if let Some(p) = click_pos {
            match scale::hit_test(&nodes, p.x, p.y, HIT_RADIUS) {
                Some(b) => put(setter, &params.bands[b].gain, 0.0),
                None => add_band(state, setter, &snap, &view, p),
            }
        }
    }
    if response.secondary_clicked() {
        if let Some(p) = click_pos {
            state.menu_target = match scale::hit_test(&nodes, p.x, p.y, HIT_RADIUS) {
                Some(b) => {
                    state.selected = b;
                    MenuTarget::Band(b)
                }
                None => MenuTarget::Empty { x: p.x, y: p.y },
            };
        }
    }
    if let Some(b) = hovered {
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let q = scale::q_after_scroll(snap[b].q as f32, scroll);
            put(setter, &params.bands[b].q, q);
            ui.ctx()
                .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            state.selected = b;
        }
    }

    let target = state.menu_target;
    response.context_menu(|ui| context_menu(ui, state, setter, &snap, &view, target));

    // Nodes.
    let font = FontId::proportional(10.0);
    let small_font = FontId::proportional(8.5);
    for nd in &nodes {
        let b = &snap[nd.band];
        let mut c = color(nd.band);
        if !b.enabled {
            c = c.gamma_multiply(0.35);
        }
        let center = pos2(
            nd.x.clamp(view.x0, view.x0 + view.width),
            nd.y.clamp(view.y0, view.y0 + view.height),
        );
        let selected = nd.band == state.selected;
        let r = if selected || Some(nd.band) == hovered {
            NODE_RADIUS + 1.5
        } else {
            NODE_RADIUS
        };
        painter.circle_filled(center, r, c);
        painter.circle_stroke(
            center,
            r,
            Stroke::new(
                if selected { 2.0_f32 } else { 1.0 },
                if selected {
                    Color32::WHITE
                } else {
                    Color32::from_gray(20)
                },
            ),
        );
        painter.text(
            center,
            Align2::CENTER_CENTER,
            format!("{}", nd.band + 1),
            // two-digit numbers need the smaller font to fit the disc
            if nd.band + 1 >= 10 {
                small_font.clone()
            } else {
                font.clone()
            },
            Color32::BLACK,
        );
    }

    // Value read-out for the hovered/dragged node.
    let readout = state.drag.as_ref().map(|d| d.band).or(hovered);
    if let Some(i) = readout {
        if let Some(nd) = nodes.iter().find(|n| n.band == i) {
            let b = &snap[i];
            let mut text = format!("{}  {}", b.shape.name(), format_freq(b.freq_hz as f32));
            if scale::shape_has_gain(b.shape) {
                text += &format!("  {:+.2} dB", b.gain_db);
            }
            text += &format!("  Q {:.2}", b.q);
            let pos = pos2(nd.x, nd.y - NODE_RADIUS - 6.0);
            let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), Color32::WHITE);
            let mut r = Rect::from_center_size(
                pos - egui::vec2(0.0, galley.size().y * 0.5),
                galley.size() + egui::vec2(8.0, 4.0),
            );
            if r.min.x < plot_rect.min.x {
                r = r.translate(egui::vec2(plot_rect.min.x - r.min.x, 0.0));
            }
            if r.max.x > plot_rect.max.x {
                r = r.translate(egui::vec2(plot_rect.max.x - r.max.x, 0.0));
            }
            if r.min.y < plot_rect.min.y {
                r = r.translate(egui::vec2(0.0, nd.y + 2.0 * NODE_RADIUS + 6.0 - r.min.y));
            }
            painter.rect_filled(r, 3.0, Color32::from_black_alpha(210));
            painter.galley(r.min + egui::vec2(4.0, 2.0), galley, Color32::WHITE);
        }
    }
}

/// Add a band at the pointer using the first free slot.
fn add_band(
    state: &mut EditorState,
    setter: &ParamSetter,
    snap: &[BandParams; NUM_BANDS],
    view: &View,
    p: Pos2,
) {
    let Some(slot) = scale::first_unused(snap) else {
        state.status = "All 32 bands are in use".into();
        return;
    };
    let shape = scale::shape_for_position((p.x - view.x0) / view.width);
    let gs = state.params.gain_scale_factor();
    let band = BandParams {
        used: true,
        enabled: true,
        shape,
        freq_hz: f64::from(scale::clamp_freq(view.x_to_freq(p.x))),
        gain_db: if scale::shape_has_gain(shape) {
            scale::gain_from_node_db(view.y_to_db(p.y), gs).unwrap_or(0.0) as f64
        } else {
            0.0
        },
        q: 1.0,
        slope: Slope::Db12,
        placement: Placement::Stereo,
    };
    write_band(setter, &state.params, slot, &band);
    state.selected = slot;
}

fn context_menu(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter,
    snap: &[BandParams; NUM_BANDS],
    view: &View,
    target: MenuTarget,
) {
    let params = state.params.clone();
    match target {
        MenuTarget::Empty { x, y } => {
            if ui.button("Add band here").clicked() {
                add_band(state, setter, snap, view, pos2(x, y));
                ui.close_menu();
            }
        }
        MenuTarget::Band(i) => {
            let set = &params.bands[i];
            ui.label(format!("Band {}", i + 1));
            let mut on = set.on.value();
            if ui.checkbox(&mut on, "Enabled").changed() {
                put(setter, &set.on, on);
            }
            ui.menu_button("Shape", |ui| {
                for s in Shape::ALL {
                    if ui.selectable_label(snap[i].shape == s, s.name()).clicked() {
                        put(setter, &set.shape, BandShape::from(s));
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button("Slope", |ui| {
                for s in Slope::ALL {
                    if ui.selectable_label(snap[i].slope == s, s.name()).clicked() {
                        put(setter, &set.slope, BandSlope::from(s));
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button("Placement", |ui| {
                for pl in Placement::ALL {
                    if ui
                        .selectable_label(snap[i].placement == pl, pl.name())
                        .clicked()
                    {
                        put(setter, &set.place, BandPlace::from(pl));
                        ui.close_menu();
                    }
                }
            });
            ui.separator();
            if ui.button("Remove band").clicked() {
                remove_band(setter, &params, i);
                ui.close_menu();
            }
        }
    }
}
