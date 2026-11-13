//! Test record page: input calibration with a test record (reference level,
//! balance and separation, playback frequency response, speed and wow &
//! flutter, tracking ability, summary). Paint and interaction code; the
//! numbers come from [`crate::testrec`] (unit-tested).

use super::scale::*;
use super::widgets::*;
use super::{EditorState, Frame};
use crate::analysis::{Command, RecCommand, RecKind, SweepLive};
use crate::params::{REF_MAX_DB, REF_MIN_DB};
use crate::testrec::{
    curve_json, evaluate_tracking, CapturedSweep, ExportChannel, Mod, NominalKind, NormMode,
    ProfileKind, RefBand, RefLayout, TestRecData, CURVE_SMOOTHING, REF_SECONDS,
};
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{self, Align2, Color32, DragValue, Pos2, Sense, Shape, Stroke, Ui};
use nih_plug_egui::widgets::ParamSlider;

/// Tooltip with the velocity calibration formula (also in the README).
pub const CAL_TIP: &str = "Velocity calibration (reference level = dBFS of a mono L=R signal at 5 cm/s peak lateral, 1 kHz)\n\
v_peak = sqrt(2) * v_rms\n\
45/45 geometry: one channel alone carries channel velocity v_ch; the same in-phase signal in both channels gives\n\
v_lateral = sqrt(2) * v_ch (anti-phase: v_vertical = sqrt(2) * v_ch).\n\
5 cm/s lateral therefore means v_ch = 5 / sqrt(2) = 3.536 cm/s peak per channel.\n\
reference_level = level_ch - 20 log10(v_ch(record) / 3.536)\n\
5 cm/s RMS on one channel = 7.071 cm/s peak: reference_level = level_ch - 6.02 dB.\n\
Check the quoted value against your sleeve: some records quote lateral instead of channel velocity.";

const WF_NOTE: &str = "Indicative only: standards specify a 3150 Hz carrier. Results from a 1 kHz carrier and the fitted \
(not standard-verified) weighting curve are not comparable with published W&F figures.";

fn send(st: &EditorState, c: RecCommand) {
    st.shared.send(Command::Rec(c));
}

fn copy_button(ui: &mut Ui, label: &str, text: impl FnOnce() -> String) {
    if ui.button(label).clicked() {
        ui.ctx().copy_text(text());
    }
}

fn opt(v: Option<f64>, decimals: usize, unit: &str) -> String {
    v.map_or("--".into(), |v| format!("{v:.decimals$}{unit}"))
}

fn grey(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(GREY).size(11.0));
}

pub fn show(ui: &mut Ui, st: &mut EditorState, f: &mut Frame, setter: &ParamSetter) {
    let before = st.shared.testrec.get();
    let mut d = before.clone();
    let trims = [
        f64::from(st.params.trim_l.value()),
        f64::from(st.params.trim_r.value()),
    ];
    header(ui, st, &mut d, setter);
    ui.add_space(4.0);
    step_reference(ui, st, f, &mut d, trims, setter);
    step_response(ui, st, f, &mut d);
    step_wf(ui, st, f, &mut d);
    step_tracking(ui, st, f, &mut d);
    step_summary(ui, &d, trims);
    if !f.snap.rec.message.is_empty() {
        ui.colored_label(WARN, &f.snap.rec.message);
    }
    if d != before {
        // settings only: the analysis thread may have stored results meanwhile
        st.shared.testrec.update(|t| t.copy_settings_from(&d));
    }
}

fn header(ui: &mut Ui, st: &EditorState, d: &mut TestRecData, setter: &ParamSetter) {
    section(ui, "Test record and input trims", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Profile");
            choice(
                ui,
                &mut d.profile_kind,
                &[
                    (ProfileKind::Ortofon, "Ortofon Test Record"),
                    (ProfileKind::Custom, "Custom"),
                ],
            );
        });
        if d.profile_kind == ProfileKind::Ortofon {
            ui.label(
                egui::RichText::new(
                    "Values from public listings - check your sleeve. Every value can be edited below.",
                )
                .color(Color32::from_rgb(230, 190, 60)),
            );
        }
        egui::CollapsingHeader::new("Profile values (editable)")
            .default_open(false)
            .show(ui, |ui| profile_editor(ui, d));
        ui.horizontal(|ui| {
            ui.label("Input trim L");
            ui.add(ParamSlider::for_param(&st.params.trim_l, setter).with_width(150.0));
            ui.label("R");
            ui.add(ParamSlider::for_param(&st.params.trim_r, setter).with_width(150.0));
            if ui.button("0 dB").clicked() {
                setter.set_parameter(&st.params.trim_l, 0.0);
                setter.set_parameter(&st.params.trim_r, 0.0);
            }
        });
        grey(
            ui,
            "Trims are applied to the pass-through audio before all metering, so every page sees the levelled signal. \
             Measurements remember the trims that were active while they were taken.",
        );
    });
}

fn profile_editor(ui: &mut Ui, d: &mut TestRecData) {
    let kind = d.profile_kind;
    let p = d.profile_mut();
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut p.name);
        if ui.button("Restore defaults").clicked() {
            *p = match kind {
                ProfileKind::Ortofon => crate::testrec::Profile::ortofon(),
                ProfileKind::Custom => crate::testrec::Profile::custom(),
            };
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Reference band");
        ui.add(
            DragValue::new(&mut p.ref_freq_hz)
                .range(20.0..=20_000.0)
                .suffix(" Hz"),
        );
        ui.add(
            DragValue::new(&mut p.ref_velocity_cm_s)
                .range(0.1..=100.0)
                .speed(0.05)
                .suffix(" cm/s"),
        );
        choice(ui, &mut p.ref_is_rms, &[(true, "RMS"), (false, "peak")]);
        ui.label("modulation");
        choice(
            ui,
            &mut p.ref_layout,
            &[
                (RefLayout::LeftRight, "L-only + R-only bands"),
                (RefLayout::Lateral, "lateral band"),
                (RefLayout::Vertical, "vertical band"),
            ],
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Sweep range");
        ui.add(
            DragValue::new(&mut p.sweep_lo_hz)
                .range(1.0..=50_000.0)
                .suffix(" Hz"),
        );
        ui.label("to");
        ui.add(
            DragValue::new(&mut p.sweep_hi_hz)
                .range(100.0..=200_000.0)
                .suffix(" Hz"),
        );
        ui.label("nominal");
        choice(
            ui,
            &mut p.nominal,
            &[
                (NominalKind::ConstantVelocity, "constant velocity"),
                (NominalKind::FlatAfterRiaa, "flat after RIAA"),
                (NominalKind::Turnover, "const. amplitude below turnover"),
            ],
        );
        if p.nominal == NominalKind::Turnover {
            ui.add(
                DragValue::new(&mut p.turnover_hz)
                    .range(20.0..=20_000.0)
                    .suffix(" Hz"),
            );
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Tracking bands");
        ui.add(
            DragValue::new(&mut p.track_freq_hz)
                .range(20.0..=5000.0)
                .suffix(" Hz"),
        );
        let mut remove = None;
        for (i, a) in p.track_amps_um.iter_mut().enumerate() {
            ui.add(DragValue::new(a).range(1.0..=500.0).suffix(" um"));
            if ui.small_button("x").clicked() {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            if p.track_amps_um.len() > 1 {
                p.track_amps_um.remove(i);
            }
        }
        if ui.small_button("+").clicked() && p.track_amps_um.len() < 16 {
            let last = p.track_amps_um.last().copied().unwrap_or(50.0);
            p.track_amps_um.push(last + 10.0);
        }
    });
    grey(
        ui,
        "Ortofon listing: tracks 1-4 sweeps from 800 Hz (constant velocity), 5-8 1 kHz 5 cm/s RMS alternating R/L, \
         9-14 315 Hz lateral 50..100 um, 15 square wave (oscilloscope). Which sweep track is L/R/lateral/vertical is not assumed.",
    );
}

fn progress(ui: &mut Ui, f: &Frame, what: impl Fn(&RecKind) -> bool) -> bool {
    if let Some((k, p)) = f.snap.rec.busy {
        if what(&k.0) {
            ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(160.0)
                    .show_percentage(),
            );
            return true;
        }
    }
    false
}

fn band_row(ui: &mut Ui, b: &RefBand) {
    let det = b.detected.map_or("?", |m| m.name());
    ui.label(b.slot.name());
    let mismatch = b.detected != Some(b.slot);
    let mut t = egui::RichText::new(det).monospace();
    if mismatch {
        t = t.color(WARN);
    }
    ui.label(t).on_hover_text(format!(
        "Detected from the channel level difference ({:.1} dB) and the phase ({}).",
        b.ch[0].level_dbfs - b.ch[1].level_dbfs,
        opt(b.phase_deg, 0, " deg")
    ));
    for c in &b.ch {
        let mut t = egui::RichText::new(format!("{:.2} Hz  {:.2} dBFS", c.freq_hz, c.level_dbfs))
            .monospace();
        if !c.driven {
            t = t.color(GREY);
        }
        ui.label(t);
        ui.label(
            egui::RichText::new(format!(
                "{} / {}  {}",
                opt(c.thd_percent, 3, " %"),
                opt(c.thd_n_percent, 3, " %"),
                opt(c.noise_dbfs, 1, " dB")
            ))
            .monospace(),
        );
    }
    ui.label(opt(b.phase_deg, 1, " deg"));
    ui.label(opt(b.separation_db, 1, " dB"));
    ui.end_row();
}

fn step_reference(
    ui: &mut Ui,
    st: &EditorState,
    f: &Frame,
    d: &mut TestRecData,
    trims: [f64; 2],
    setter: &ParamSetter,
) {
    section(
        ui,
        "1. Reference level, balance, separation (1 kHz band)",
        |ui| {
            let busy = f.snap.rec.busy.is_some();
            ui.horizontal_wrapped(|ui| {
                ui.label("Average over");
                for s in REF_SECONDS {
                    if ui
                        .selectable_label((d.ref_seconds - s).abs() < 1e-9, format!("{s:.0} s"))
                        .clicked()
                    {
                        d.ref_seconds = s;
                    }
                }
                ui.separator();
                for &slot in d.profile().ref_layout.slots() {
                    let label = match slot {
                        Mod::Left => "Measure left band",
                        Mod::Right => "Measure right band",
                        Mod::Lateral => "Measure lateral band",
                        Mod::Vertical => "Measure vertical band",
                    };
                    if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                        send(st, RecCommand::Ref(slot));
                    }
                }
                if !progress(ui, f, |k| matches!(k, RecKind::Ref(_))) {
                    ui.label("");
                } else if ui.button("Cancel").clicked() {
                    send(st, RecCommand::Cancel);
                }
                if ui.button("Clear").clicked() {
                    send(st, RecCommand::ClearRef);
                }
            });
            grey(
                ui,
                "Play the band, then press the button for that band. Levels are peak dBFS from the tone analysis; the \
                 modulation is detected from the inter-channel level difference and phase (L only / R only / lateral ~0 deg / \
                 vertical ~180 deg).",
            );
            if !d.ref_bands.is_empty() {
                egui::Grid::new("rec-ref")
                    .striped(true)
                    .num_columns(8)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        for h in [
                            "band",
                            "detected",
                            "L tone",
                            "L THD / THD+N  noise",
                            "R tone",
                            "R THD / THD+N  noise",
                            "phase L-R",
                            "separation",
                        ] {
                            ui.label(egui::RichText::new(h).color(GREY));
                        }
                        ui.end_row();
                        for b in &d.ref_bands {
                            band_row(ui, b);
                        }
                    });
            }
            let Some(c) = d.ref_calibration(trims) else {
                grey(
                    ui,
                    "Measure both channels' bands (left + right, or one lateral/vertical band) for the results.",
                );
                return;
            };
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                big_value(
                    ui,
                    "L level (dBFS)",
                    &format!("{:.2}", c.level_dbfs[0]),
                    None,
                );
                big_value(
                    ui,
                    "R level (dBFS)",
                    &format!("{:.2}", c.level_dbfs[1]),
                    None,
                );
                big_value(ui, "balance L-R (dB)", &fmt_signed(c.balance_db, 2), None);
                big_value(
                    ui,
                    "crosstalk L->R (dB)",
                    &opt(c.xtalk_l_to_r_db, 1, ""),
                    None,
                );
                big_value(
                    ui,
                    "crosstalk R->L (dB)",
                    &opt(c.xtalk_r_to_l_db, 1, ""),
                    None,
                );
                big_value(ui, "tone (Hz)", &format!("{:.2}", c.tone_hz), None);
                big_value(ui, "speed deviation (%)", &fmt_signed(c.speed_pct, 2), None);
            });
            for w in &c.warnings {
                ui.colored_label(WARN, w);
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Target level");
                ui.add(
                    DragValue::new(&mut d.target_dbfs)
                        .range(-60.0..=-1.0)
                        .speed(0.1)
                        .suffix(" dBFS"),
                );
                ui.label(format!(
                    "suggested trims: L {:+.2} dB, R {:+.2} dB{}",
                    c.suggested_trim_db[0],
                    c.suggested_trim_db[1],
                    if c.trims_clamped {
                        " (limited to +-24 dB)"
                    } else {
                        ""
                    }
                ));
                if ui.button("Apply trims").clicked() {
                    setter.set_parameter(&st.params.trim_l, c.suggested_trim_db[0] as f32);
                    setter.set_parameter(&st.params.trim_r, c.suggested_trim_db[1] as f32);
                }
                if ui
                    .button(format!(
                        "Set velocity calibration ({:.2} dBFS)",
                        c.reference_level_dbfs
                    ))
                    .on_hover_text(CAL_TIP)
                    .clicked()
                {
                    setter.set_parameter(
                        &st.params.reference_level,
                        (c.reference_level_dbfs as f32).clamp(REF_MIN_DB, REF_MAX_DB),
                    );
                }
            });
            ui.label(
                egui::RichText::new(format!(
                    "Reference level {:.2} dBFS = 5 cm/s peak lateral; band channel velocity {:.3} cm/s peak (hover the button for the formula)",
                    c.reference_level_dbfs, c.channel_velocity_peak_cm_s
                ))
                .color(GREY)
                .size(11.0),
            )
            .on_hover_text(CAL_TIP);
            grey(
                ui,
                "Apply trims, then measure the bands again to verify the levels; the velocity calibration uses the levels \
                 as they read with the trims that are applied now.",
            );
            copy_button(ui, "Copy results", || d.ref_text(trims));
        },
    );
}

fn live_to_captured(live: &SweepLive, trims: [f64; 2]) -> CapturedSweep {
    CapturedSweep {
        cells: live.cells.clone(),
        trim_db: trims,
        detected: live
            .pair
            .filter(|p| p.phase_coherence > 0.5)
            .and_then(|p| neo_measure::velocity::detect_modulation(p.level_diff_db, p.phase_deg))
            .map(Mod::from_m),
        level_diff_db: live.pair.map(|p| p.level_diff_db),
        phase_deg: live.pair.map(|p| p.phase_deg),
        pair_blocks: live.pair.map_or(0, |p| p.blocks),
    }
}

fn step_response(ui: &mut Ui, st: &EditorState, f: &Frame, d: &mut TestRecData) {
    section(ui, "2. Playback frequency response (sweep bands)", |ui| {
        let live = f.snap.rec.sweep.as_ref();
        let armed = live.is_some();
        ui.horizontal_wrapped(|ui| {
            if !armed {
                if ui.button("Arm: start collecting").clicked() {
                    send(st, RecCommand::SweepArm);
                }
            } else if ui.button("Stop and keep").clicked() {
                send(st, RecCommand::SweepStop);
            }
            if ui.button("Clear").clicked() {
                send(st, RecCommand::SweepClear);
            }
            if armed {
                ui.colored_label(
                    Color32::from_rgb(230, 190, 60),
                    "ARMED - play a sweep track; silence, speech and noise between bands are ignored",
                );
            }
        });
        grey(
            ui,
            "While armed every short block of the input is analysed; only blocks with one clear sinusoid count. \
             Several tracks can be played in one session (they average per frequency cell) - press Stop when done.",
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Sweep modulation");
            let mut sel = d.sweep_mod;
            choice(
                ui,
                &mut sel,
                &[
                    (None, "Auto"),
                    (Some(Mod::Left), "L only"),
                    (Some(Mod::Right), "R only"),
                    (Some(Mod::Lateral), "lateral"),
                    (Some(Mod::Vertical), "vertical"),
                ],
            );
            d.sweep_mod = sel;
            // detection (captured or live)
            let preview;
            let sw = match live {
                Some(l) => {
                    preview = live_to_captured(l, st.shared.applied_trim_db());
                    Some(&preview)
                }
                None => d.sweep.as_ref(),
            };
            if let Some(sw) = sw {
                ui.label(format!(
                    "detected: {}  (L-R {}, phase {})",
                    sw.detected.map_or("unclear", |m| m.name()),
                    opt(sw.level_diff_db, 1, " dB"),
                    opt(sw.phase_deg, 0, " deg")
                ));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Recorded characteristic");
            choice(
                ui,
                &mut d.profile_mut().nominal,
                &[
                    (NominalKind::ConstantVelocity, "constant velocity"),
                    (NominalKind::FlatAfterRiaa, "flat after RIAA"),
                    (NominalKind::Turnover, "turnover"),
                ],
            );
            if d.profile().nominal == NominalKind::Turnover {
                ui.add(
                    DragValue::new(&mut d.profile_mut().turnover_hz)
                        .range(20.0..=20_000.0)
                        .suffix(" Hz"),
                )
                .on_hover_text("Constant amplitude below, constant velocity above this frequency.");
            }
            ui.separator();
            ui.checkbox(&mut d.preamp_riaa, "playback preamp applies RIAA")
                .on_hover_text(
                    "On: the expected curve of a constant-velocity record is the RIAA de-emphasis. Off (flat preamp): the expected curve changes accordingly.",
                );
            ui.checkbox(&mut d.riaa_iec, "IEC roll-off")
                .on_hover_text("Include the IEC 7950 us low-frequency roll-off in the preamp's RIAA.");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Normalise");
            choice(
                ui,
                &mut d.norm,
                &[
                    (NormMode::AtFreq, "at frequency"),
                    (NormMode::ToReference, "to step-1 reference"),
                ],
            );
            match d.norm {
                NormMode::AtFreq => {
                    ui.add(
                        DragValue::new(&mut d.norm_freq_hz)
                            .range(20.0..=20_000.0)
                            .suffix(" Hz"),
                    );
                }
                NormMode::ToReference => {
                    ui.add(
                        DragValue::new(&mut d.sweep_rel_ref_db)
                            .range(-60.0..=60.0)
                            .speed(0.1)
                            .suffix(" dB"),
                    )
                    .on_hover_text("Level of the sweep relative to the reference band (0 if it is recorded at the same velocity).");
                }
            }
            ui.separator();
            ui.label("Tone threshold");
            ui.add(
                DragValue::new(&mut d.min_snr_db)
                    .range(6.0..=60.0)
                    .suffix(" dB"),
            )
            .on_hover_text("A block counts only if the residual after removing the fitted sinusoid is this far below it. Takes effect when arming.");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Scale");
            for r in [3.0, 6.0, 12.0] {
                if ui
                    .selectable_label((d.range_db - r).abs() < 1e-9, format!("+-{r:.0} dB"))
                    .clicked()
                {
                    d.range_db = r;
                }
            }
            ui.label("Smoothing");
            for s in CURVE_SMOOTHING {
                let name = if s == 0 {
                    "off".to_string()
                } else {
                    format!("1/{s} oct")
                };
                if ui.selectable_label(d.smoothing == s, name).clicked() {
                    d.smoothing = s;
                }
            }
            ui.label("Export");
            choice(
                ui,
                &mut d.export_channel,
                &[
                    (ExportChannel::Average, "average"),
                    (ExportChannel::Left, "L"),
                    (ExportChannel::Right, "R"),
                ],
            );
        });
        // curve to show: the live preview while armed, else the stored sweep
        let mut shown = d.clone();
        if let Some(l) = live {
            shown.sweep = Some(live_to_captured(l, st.shared.applied_trim_db()));
        }
        match shown.response() {
            Ok(r) => {
                plot(ui, &r.per_channel, &r.selected, d.range_db);
                ui.label(shown.response_text());
                if let Some(l) = live {
                    ui.label(format!(
                        "blocks accepted L/R {}/{} of {}/{} candidates",
                        l.accepted[0], l.accepted[1], l.candidates[0], l.candidates[1]
                    ));
                }
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut d.curve_name);
                    copy_button(ui, "Copy curve (JSON)", || {
                        curve_json(&d.curve_name, &r.selected)
                    });
                    copy_button(ui, "Copy results", || shown.response_text());
                });
                grey(
                    ui,
                    "Curve JSON: {\"version\":1,\"kind\":\"response_curve\",\"name\":...,\"points\":[{\"freq_hz\":..,\"db\":..}]} - ascending frequency, dB deviation \
                     from nominal, covered frequencies only; read by the EQ plugin's loop calibration as playback-chain correction.",
                );
            }
            Err(e) => grey(ui, &e),
        }
    });
}

fn plot(ui: &mut Ui, per_ch: &[Vec<(f64, f64)>; 2], sel: &[(f64, f64)], range: f64) {
    let (resp, painter) =
        ui.allocate_painter(egui::vec2(ui.available_width(), 220.0), Sense::hover());
    let r = resp.rect;
    painter.rect_filled(r, 2.0, PLOT_BG);
    let plot = egui::Rect::from_min_max(
        Pos2::new(r.min.x + 36.0, r.min.y + 6.0),
        Pos2::new(r.max.x - 8.0, r.max.y - 18.0),
    );
    let (mut lo, mut hi) = (f64::MAX, 0.0f64);
    for &(fr, _) in sel.iter().chain(per_ch[0].iter()).chain(per_ch[1].iter()) {
        lo = lo.min(fr);
        hi = hi.max(fr);
    }
    if lo >= hi {
        lo = 800.0;
        hi = 20_000.0;
    }
    let (f_lo, f_hi) = ((lo * 0.9) as f32, (hi * 1.1) as f32);
    let view = SpecView {
        x0: plot.min.x,
        y0: plot.min.y,
        w: plot.width(),
        h: plot.height(),
        f_lo,
        f_hi,
        db_min: -range as f32,
        db_max: range as f32,
    };
    for (fr, label) in freq_ticks(f64::from(f_lo), f64::from(f_hi)) {
        let x = view.freq_to_x(fr as f32);
        painter.line_segment(
            [Pos2::new(x, plot.min.y), Pos2::new(x, plot.max.y)],
            Stroke::new(1.0_f32, Color32::from_gray(if label { 52 } else { 32 })),
        );
        if label {
            caption(
                &painter,
                Pos2::new(x, plot.max.y + 2.0),
                Align2::CENTER_TOP,
                &freq_label(fr),
                GREY,
            );
        }
    }
    let step = if range <= 3.0 {
        1.0
    } else if range <= 6.0 {
        2.0
    } else {
        4.0
    };
    let mut db = -range;
    while db <= range + 1e-9 {
        let y = view.db_to_y(db as f32);
        let zero = db.abs() < 1e-9;
        painter.line_segment(
            [Pos2::new(plot.min.x, y), Pos2::new(plot.max.x, y)],
            Stroke::new(1.0_f32, Color32::from_gray(if zero { 90 } else { 40 })),
        );
        caption(
            &painter,
            Pos2::new(plot.min.x - 4.0, y),
            Align2::RIGHT_CENTER,
            &format!("{db:+.0}"),
            GREY,
        );
        db += step;
    }
    let clip = painter.with_clip_rect(plot);
    let line = |pts: &[(f64, f64)], c: Color32, w: f32| {
        let v: Vec<Pos2> = pts
            .iter()
            .map(|&(fr, d)| Pos2::new(view.freq_to_x(fr as f32), view.db_to_y(d as f32)))
            .collect();
        if v.len() > 1 {
            clip.add(Shape::line(v, Stroke::new(w, c)));
        }
    };
    line(
        &per_ch[0],
        Color32::from_rgba_unmultiplied(90, 170, 255, 120),
        1.0,
    );
    line(
        &per_ch[1],
        Color32::from_rgba_unmultiplied(255, 150, 80, 120),
        1.0,
    );
    line(sel, Color32::WHITE, 2.0);
    caption(
        &painter,
        Pos2::new(plot.min.x + 6.0, plot.min.y + 4.0),
        Align2::LEFT_TOP,
        "deviation from nominal (white = exported curve, blue L, orange R)",
        GREY,
    );
}

fn step_wf(ui: &mut Ui, st: &EditorState, f: &Frame, d: &mut TestRecData) {
    section(ui, "3. Speed and wow & flutter (reference tone)", |ui| {
        let busy = f.snap.rec.busy.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Carrier {:.0} Hz (profile)",
                d.profile().ref_freq_hz
            ));
            ui.label("channel");
            choice(ui, &mut d.wf_channel, &[(0, "L"), (1, "R")]);
            ui.label("duration");
            ui.add(
                DragValue::new(&mut d.wf_seconds)
                    .range(3.0..=30.0)
                    .suffix(" s"),
            );
            if ui
                .add_enabled(!busy, egui::Button::new("Measure"))
                .clicked()
            {
                send(st, RecCommand::WowFlutter);
            }
            progress(ui, f, |k| matches!(k, RecKind::WowFlutter));
            if ui.button("Clear").clicked() {
                send(st, RecCommand::ClearWf);
            }
        });
        ui.label(
            egui::RichText::new(WF_NOTE)
                .color(Color32::from_rgb(230, 190, 60))
                .size(11.0),
        );
        if let Some(w) = d.wf {
            egui::Grid::new("rec-wf")
                .num_columns(4)
                .spacing([16.0, 3.0])
                .show(ui, |ui| {
                    ui.label("speed deviation");
                    ui.label(
                        egui::RichText::new(format!("{:+.3} %", w.speed_pct))
                            .monospace()
                            .size(15.0),
                    );
                    ui.label("mean frequency");
                    ui.label(egui::RichText::new(format!("{:.3} Hz", w.mean_hz)).monospace());
                    ui.end_row();
                    for (name, p, r, s) in [
                        (
                            "unweighted",
                            w.unweighted_peak_pct,
                            w.unweighted_rms_pct,
                            w.unweighted_2sigma_pct,
                        ),
                        (
                            "weighted",
                            w.weighted_peak_pct,
                            w.weighted_rms_pct,
                            w.weighted_2sigma_pct,
                        ),
                    ] {
                        ui.label(name);
                        ui.label(egui::RichText::new(format!("peak {p:.3} %")).monospace());
                        ui.label(egui::RichText::new(format!("rms {r:.3} %")).monospace());
                        ui.label(egui::RichText::new(format!("2 sigma {s:.3} %")).monospace());
                        ui.end_row();
                    }
                });
            copy_button(ui, "Copy results", || d.wf_text());
        }
    });
}

fn step_tracking(ui: &mut Ui, st: &EditorState, f: &Frame, d: &mut TestRecData) {
    section(ui, "4. Tracking ability", |ui| {
        let busy = f.snap.rec.busy.is_some();
        let amps = d.profile().track_amps_um.clone();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Bands at {:.0} Hz, amplitude (peak):",
                d.profile().track_freq_hz
            ));
            for (i, a) in amps.iter().enumerate() {
                if ui
                    .selectable_label(d.track_sel == i, format!("{a:.0} um"))
                    .clicked()
                {
                    d.track_sel = i;
                }
            }
            ui.checkbox(&mut d.track_auto, "auto-advance");
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(format!(
                        "Measure {:.0} um band",
                        amps.get(d.track_sel).copied().unwrap_or(0.0)
                    )),
                )
                .clicked()
            {
                send(st, RecCommand::Tracking(d.track_sel));
            }
            progress(ui, f, |k| matches!(k, RecKind::Tracking(..)));
            ui.label("duration");
            ui.add(
                DragValue::new(&mut d.track_seconds)
                    .range(1.0..=30.0)
                    .suffix(" s"),
            );
            ui.separator();
            ui.label("THD limit");
            ui.add(
                DragValue::new(&mut d.track_thd_pct)
                    .range(0.1..=50.0)
                    .speed(0.1)
                    .suffix(" %"),
            );
            ui.label("THD jump");
            ui.add(
                DragValue::new(&mut d.track_jump_db)
                    .range(1.0..=30.0)
                    .suffix(" dB"),
            );
            ui.label("level shortfall");
            ui.add(
                DragValue::new(&mut d.track_shortfall_db)
                    .range(0.1..=10.0)
                    .speed(0.1)
                    .suffix(" dB"),
            );
            if ui.button("Clear").clicked() {
                send(st, RecCommand::ClearTracking);
            }
        });
        grey(
            ui,
            "v_peak = 2 pi f A (315 Hz, 50 um = 9.90 cm/s). A band mistracks when THD exceeds the limit, THD jumps by more than \
             the set amount versus the previous band, or the level falls short of the expected step 20 log10(A/A_prev).",
        );
        if d.tracking.is_empty() {
            return;
        }
        let rep = evaluate_tracking(&d.tracking, d);
        egui::Grid::new("rec-track")
            .striped(true)
            .num_columns(7)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                for h in [
                    "amplitude",
                    "velocity",
                    "exp. step",
                    "meas. step",
                    "L level / THD / THD+N",
                    "R level / THD / THD+N",
                    "",
                ] {
                    ui.label(egui::RichText::new(h).color(GREY));
                }
                ui.end_row();
                for (row, e) in d.tracking.iter().zip(&rep.rows) {
                    ui.label(format!("{:.0} um", e.amp_um));
                    ui.label(format!("{:.2} cm/s", e.velocity_cm_s));
                    ui.label(opt(e.expected_step_db, 2, " dB"));
                    ui.label(opt(e.measured_step_db, 2, " dB"));
                    for c in &row.ch {
                        let mut t = egui::RichText::new(format!(
                            "{:.2} dBFS  {}  {}",
                            c.level_dbfs,
                            opt(c.thd_percent, 2, " %"),
                            opt(c.thd_n_percent, 2, " %")
                        ))
                        .monospace();
                        if !c.driven {
                            t = t.color(GREY);
                        }
                        ui.label(t);
                    }
                    let (txt, col) = if e.mistracked {
                        ("MISTRACKING", WARN)
                    } else {
                        ("clean", Color32::from_rgb(110, 200, 120))
                    };
                    ui.label(egui::RichText::new(txt).color(col).strong())
                        .on_hover_text(e.reasons.join("\n"));
                    ui.end_row();
                }
            });
        ui.label(
            egui::RichText::new(format!(
                "Highest amplitude tracked cleanly: {}",
                rep.highest_clean_um
                    .map_or("none".to_string(), |a| format!("{a:.0} um"))
            ))
            .strong(),
        );
        copy_button(ui, "Copy results", || d.tracking_text());
    });
}

fn step_summary(ui: &mut Ui, d: &TestRecData, trims: [f64; 2]) {
    section(ui, "5. Summary", |ui| {
        let text = d.summary_text(trims);
        ui.label(egui::RichText::new(&text).monospace().size(12.0));
        copy_button(ui, "Copy summary", || text.clone());
    });
}
