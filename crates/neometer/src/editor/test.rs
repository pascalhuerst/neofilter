//! Test page: signal generator and latched analyses of the incoming signal.

use super::scale::*;
use super::widgets::*;
use super::{EditorState, Frame};
use crate::analysis::{Command, MeasureKind};
use crate::settings::{NOISE_DURATIONS, TONE_DURATIONS, WF_DURATIONS};
use crate::testgen::{GenSettings, Kind, Mode, Routing};
use nih_plug_egui::egui::{self, Color32, DragValue, RichText, Ui, Vec2};

fn duration_choice(ui: &mut Ui, v: &mut f64, items: &[f64]) {
    for &d in items {
        if ui
            .selectable_label((*v - d).abs() < 1e-9, format!("{d:.0} s"))
            .clicked()
        {
            *v = d;
        }
    }
}

pub fn show(ui: &mut Ui, st: &mut EditorState, f: &mut Frame) {
    let mut g = st.shared.gen.get();
    let g0 = g;
    ui.columns(2, |cols| {
        generator(&mut cols[0], &mut g);
        analyses(&mut cols[1], st, f);
    });
    if g != g0 {
        st.shared.gen.set(g);
    }
}

fn generator(ui: &mut Ui, g: &mut GenSettings) {
    section(ui, "Signal generator", |ui| {
        ui.horizontal(|ui| {
            let (txt, col) = if g.on {
                ("OUTPUT ON", Color32::from_rgb(200, 50, 40))
            } else {
                ("Output off", Color32::from_gray(60))
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(txt).strong().size(16.0))
                        .fill(col)
                        .min_size(Vec2::new(150.0, 32.0)),
                )
                .clicked()
            {
                g.on = !g.on;
            }
            ui.label(
                RichText::new("always off when the plugin loads; not saved, not automatable")
                    .color(GREY)
                    .size(11.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Waveform");
            for k in Kind::ALL {
                if ui.selectable_label(g.kind == k, k.name()).clicked() {
                    g.kind = k;
                }
            }
        });
        match g.kind {
            Kind::Sine => {
                ui.horizontal(|ui| {
                    ui.label("Frequency");
                    for (hz, name) in [(1000.0, "1 kHz"), (3150.0, "3150 Hz"), (10_000.0, "10 kHz")]
                    {
                        if ui
                            .selectable_label((g.freq_hz - hz).abs() < 1e-9, name)
                            .clicked()
                        {
                            g.freq_hz = hz;
                        }
                    }
                    ui.add(
                        DragValue::new(&mut g.freq_hz)
                            .range(1.0..=96_000.0)
                            .speed(5.0)
                            .suffix(" Hz"),
                    );
                });
            }
            Kind::Sweep => {
                ui.horizontal(|ui| {
                    ui.label("Log sweep 20 Hz .. 20 kHz, looping, duration");
                    ui.add(
                        DragValue::new(&mut g.sweep_s)
                            .range(1.0..=120.0)
                            .speed(0.2)
                            .suffix(" s"),
                    );
                });
            }
            _ => {}
        }
        ui.horizontal(|ui| {
            ui.label("Level");
            ui.add(
                DragValue::new(&mut g.level_db)
                    .range(-120.0..=0.0)
                    .speed(0.1)
                    .suffix(" dBFS"),
            );
            ui.label(
                RichText::new("peak for tones, RMS for noise")
                    .color(GREY)
                    .size(11.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Channels");
            for r in Routing::ALL {
                if ui.selectable_label(g.routing == r, r.name()).clicked() {
                    g.routing = r;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Output");
            choice(
                ui,
                &mut g.mode,
                &[
                    (Mode::Replace, "replaces input"),
                    (Mode::Add, "added to input"),
                ],
            );
        });
        ui.label(
            RichText::new(
                "Switching on/off fades over 20 ms. The meters and analyses always see the plugin INPUT, so \
                 route the generator out through your chain and bring the return back into the input.",
            )
            .color(GREY)
            .size(11.0),
        );
    });
}

fn analyses(ui: &mut Ui, st: &mut EditorState, f: &mut Frame) {
    let busy = f.snap.measuring;
    if let Some((_, p)) = busy {
        ui.horizontal(|ui| {
            ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(220.0)
                    .text("measuring ..."),
            );
            if ui.button("Cancel").clicked() {
                st.shared.send(Command::Cancel);
            }
        });
    }
    let res = &f.snap.results;
    if !res.message.is_empty() {
        ui.colored_label(WARN, &res.message);
    }
    let start = |ui: &mut Ui, shared: &crate::shared::Shared, label: &str, k: MeasureKind| {
        if ui
            .add_enabled(busy.is_none(), egui::Button::new(label))
            .clicked()
        {
            shared.send(Command::Start(k));
        }
    };
    section(ui, "Input channel and tone", |ui| {
        ui.horizontal(|ui| {
            ui.label("Channel");
            choice(ui, &mut f.set.tone_right, &[(false, "L"), (true, "R")]);
            ui.label("Time");
            duration_choice(ui, &mut f.set.tone_duration_s, &TONE_DURATIONS);
        });
        ui.horizontal(|ui| {
            start(ui, &st.shared, "Measure tone", MeasureKind::Tone);
            start(ui, &st.shared, "Set reference", MeasureKind::Reference);
            if ui.button("Clear results").clicked() {
                st.shared.send(Command::ClearResults);
            }
        });
        if let Some(t) = &res.tone {
            let a = &t.analysis;
            ui.horizontal(|ui| {
                readout(ui, "Frequency", &format!("{:.3} Hz", a.frequency_hz), None);
                ui.add_space(10.0);
                readout(ui, "Level", &format!("{} dBFS", fmt_db(a.level_dbfs)), None);
            });
            ui.horizontal(|ui| {
                readout(
                    ui,
                    "THD",
                    &format!("{:.4} %  ({} dB)", a.thd_percent, fmt_db(a.thd_db)),
                    None,
                );
            });
            ui.horizontal(|ui| {
                readout(
                    ui,
                    "THD+N",
                    &format!("{:.4} %  ({} dB)", a.thd_n_percent, fmt_db(a.thd_n_db)),
                    None,
                );
            });
            ui.label(
                RichText::new(format!(
                    "channel {}",
                    if t.channel == 0 { "L" } else { "R" }
                ))
                .color(GREY)
                .size(11.0),
            );
        }
        if let Some(r) = f.set.noise_ref_dbfs {
            readout(
                ui,
                "Reference tone",
                &format!("{} dBFS", fmt_db(r)),
                Some(ACCENT),
            );
        } else {
            ui.label(
                RichText::new("No reference latched: noise is read relative to 0 dBFS.")
                    .color(GREY)
                    .size(11.0),
            );
        }
    });
    section(ui, "Channel separation", |ui| {
        ui.horizontal(|ui| {
            start(ui, &st.shared, "Measure L -> R", MeasureKind::Crosstalk(0));
            start(ui, &st.shared, "Measure R -> L", MeasureKind::Crosstalk(1));
        });
        ui.label(
            RichText::new("Drive one channel with a tone (the other silent), then measure.")
                .color(GREY)
                .size(11.0),
        );
        ui.horizontal(|ui| {
            readout(
                ui,
                "L -> R",
                &res.xtalk[0].map_or("--".into(), |v| format!("{} dB", fmt_db(v))),
                None,
            );
            ui.add_space(14.0);
            readout(
                ui,
                "R -> L",
                &res.xtalk[1].map_or("--".into(), |v| format!("{} dB", fmt_db(v))),
                None,
            );
        });
    });
    section(ui, "Noise / rumble", |ui| {
        ui.horizontal(|ui| {
            ui.label("Time");
            duration_choice(ui, &mut f.set.noise_duration_s, &NOISE_DURATIONS);
            ui.label("Band");
            ui.add(
                DragValue::new(&mut f.set.noise_band_low)
                    .range(1.0..=1000.0)
                    .speed(1.0)
                    .suffix(" Hz"),
            );
            ui.add(
                DragValue::new(&mut f.set.noise_band_high)
                    .range(1000.0..=96_000.0)
                    .speed(50.0)
                    .suffix(" Hz"),
            );
            start(ui, &st.shared, "Measure noise", MeasureKind::Noise);
        });
        if let Some(n) = &res.noise {
            egui::Grid::new("noise")
                .num_columns(5)
                .spacing([14.0, 3.0])
                .show(ui, |ui| {
                    for h in [
                        "",
                        "unweighted dBFS",
                        "re ref dB",
                        "A-weighted dBFS",
                        "re ref dB",
                    ] {
                        ui.label(RichText::new(h).color(GREY));
                    }
                    ui.end_row();
                    for (c, name) in ["L", "R"].iter().enumerate() {
                        let l = n.levels[c];
                        ui.label(*name);
                        for v in [
                            l.unweighted_dbfs,
                            l.unweighted_rel_db,
                            l.a_weighted_dbfs,
                            l.a_weighted_rel_db,
                        ] {
                            ui.label(RichText::new(fmt_db(v)).monospace());
                        }
                        ui.end_row();
                    }
                });
            match n.reference_rms_dbfs {
                Some(r) => ui.label(
                    RichText::new(format!("reference = tone RMS {r:.2} dBFS"))
                        .color(GREY)
                        .size(11.0),
                ),
                None => ui.label(
                    RichText::new("no reference: relative values are absolute dBFS")
                        .color(GREY)
                        .size(11.0),
                ),
            };
        }
    });
    section(ui, "Wow & flutter", |ui| {
        ui.horizontal(|ui| {
            ui.label("Tone");
            choice(
                ui,
                &mut f.set.wf_nominal_hz,
                &[(3150.0, "3150 Hz"), (3000.0, "3000 Hz")],
            );
            ui.label("Time");
            duration_choice(ui, &mut f.set.wf_duration_s, &WF_DURATIONS);
            start(ui, &st.shared, "Measure", MeasureKind::WowFlutter);
        });
        if let Some(w) = &res.wf {
            let r = &w.result;
            readout(
                ui,
                "Mean frequency",
                &format!("{:.3} Hz", r.mean_frequency_hz),
                None,
            );
            readout(
                ui,
                "Speed deviation",
                &format!("{} %", fmt_signed(r.speed_deviation_pct, 3)),
                None,
            );
            egui::Grid::new("wf")
                .num_columns(5)
                .spacing([14.0, 3.0])
                .show(ui, |ui| {
                    for h in ["", "peak %", "quasi-peak %", "RMS %", "2 sigma %"] {
                        ui.label(RichText::new(h).color(GREY));
                    }
                    ui.end_row();
                    for (name, s) in [("weighted", r.weighted), ("unweighted", r.unweighted)] {
                        ui.label(name);
                        for v in [s.peak_pct, s.quasi_peak_pct, s.rms_pct, s.two_sigma_pct] {
                            ui.label(RichText::new(format!("{v:.4}")).monospace());
                        }
                        ui.end_row();
                    }
                });
        }
        ui.label(
            RichText::new(
                "The weighting filter is a fit to the standard's curve (peak 4 Hz, -6 dB at 0.8 and 20 Hz), \
                 not a verified mask; the quasi-peak ballistics are best-knowledge values. Use for relative comparisons.",
            )
            .color(GREY)
            .size(11.0),
        );
    });
}
