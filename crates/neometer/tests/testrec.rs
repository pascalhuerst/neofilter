//! Test-record input calibration: trims in the audio path and the analysis
//! pipeline of the "Test record" page (no GUI, no threads).
mod common;
use common::*;
use neo_measure::riaa::{analog_response, Mode, Options};
use neo_measure::velocity::reference_level_dbfs;
use neometer::analysis::{Command, RecCommand};
use neometer::params::NeoMeterParams;
use neometer::testrec::{curve_json, evaluate_tracking, Mod, NormMode, RefBand, TestRecData};
use nih_plug::prelude::Params;
use std::f64::consts::PI;

const FS: f64 = 48_000.0;

fn secs(s: f64) -> usize {
    (s * FS) as usize
}

fn tone(f: f64, db: f64, phase: f64, n: usize) -> Vec<f32> {
    let a = 10f64.powf(db / 20.0);
    (0..n)
        .map(|i| (a * (2.0 * PI * f * i as f64 / FS + phase).sin()) as f32)
        .collect()
}

fn cmd(rig: &Rig, c: RecCommand) {
    rig.shared.send(Command::Rec(c));
}

/// Play the L-only and R-only reference bands and measure both.
fn measure_bands(rig: &mut Rig, f: f64, l_db: f64, r_db: f64, xt_lr: f64, xt_rl: f64) {
    let n = secs(5.5);
    // left band: L at l_db, R is crosstalk at l_db - xt_lr
    cmd(rig, RecCommand::Ref(Mod::Left));
    rig.feed(&tone(f, l_db, 0.3, n), &tone(f, l_db - xt_lr, 0.5, n), 480);
    // right band
    cmd(rig, RecCommand::Ref(Mod::Right));
    rig.feed(&tone(f, r_db - xt_rl, 1.0, n), &tone(f, r_db, 1.2, n), 480);
}

fn band(d: &TestRecData, m: Mod) -> RefBand {
    d.ref_bands.iter().find(|b| b.slot == m).cloned().unwrap()
}

#[test]
fn reference_bands_levels_balance_separation_speed_and_trims() {
    let mut rig = Rig::new(FS);
    measure_bands(&mut rig, 1002.0, -20.5, -19.2, 28.0, 31.0);
    let d = rig.shared.testrec.get();
    assert_eq!(d.ref_bands.len(), 2);
    let (bl, br) = (band(&d, Mod::Left), band(&d, Mod::Right));
    assert_eq!(bl.detected, Some(Mod::Left));
    assert_eq!(br.detected, Some(Mod::Right));
    assert!((bl.ch[0].level_dbfs + 20.5).abs() < 0.02, "{:?}", bl.ch[0]);
    assert!((br.ch[1].level_dbfs + 19.2).abs() < 0.02, "{:?}", br.ch[1]);
    assert!(bl.ch[0].driven && !bl.ch[1].driven);
    assert!(bl.ch[0].thd_percent.unwrap() < 0.01);
    assert!(bl.ch[0].noise_dbfs.unwrap() < -100.0, "{:?}", bl.ch[0]);

    let c = d.ref_calibration(rig.shared.applied_trim_db()).unwrap();
    assert!((c.level_dbfs[0] + 20.5).abs() < 0.02);
    assert!((c.level_dbfs[1] + 19.2).abs() < 0.02);
    assert!((c.balance_db + 1.3).abs() < 0.03, "{}", c.balance_db);
    assert!((c.xtalk_l_to_r_db.unwrap() - 28.0).abs() < 0.2);
    assert!((c.xtalk_r_to_l_db.unwrap() - 31.0).abs() < 0.2);
    assert!((c.speed_pct - 0.2).abs() < 0.01, "{}", c.speed_pct);
    assert!((c.suggested_trim_db[0] - 2.5).abs() < 0.02);
    assert!((c.suggested_trim_db[1] - 1.2).abs() < 0.02);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);

    // apply the trims (host parameters), let the ramp finish, re-measure the
    // same signals through the trimmed path
    let mut rig = Rig::new(FS);
    rig.params.trim_db = c.suggested_trim_db;
    rig.feed(&vec![0.0; secs(0.2)], &vec![0.0; secs(0.2)], 480);
    measure_bands(&mut rig, 1002.0, -20.5, -19.2, 28.0, 31.0);
    let d2 = rig.shared.testrec.get();
    assert_eq!(band(&d2, Mod::Left).trim_db, c.suggested_trim_db);
    let c2 = d2.ref_calibration(rig.shared.applied_trim_db()).unwrap();
    assert!(
        (c2.level_dbfs[0] + 18.0).abs() < 0.02,
        "{:?}",
        c2.level_dbfs
    );
    assert!(
        (c2.level_dbfs[1] + 18.0).abs() < 0.02,
        "{:?}",
        c2.level_dbfs
    );
    // suggestions are stable (already on target)
    assert!((c2.suggested_trim_db[0] - c.suggested_trim_db[0]).abs() < 0.02);
    assert!((c2.suggested_trim_db[1] - c.suggested_trim_db[1]).abs() < 0.02);
    // velocity calibration: 5 cm/s RMS on one channel = 7.071 cm/s peak, twice
    // the 3.536 cm/s channel velocity of the 5 cm/s lateral reference: -6.02 dB
    let want = reference_level_dbfs(-18.0, 5.0 * 2f64.sqrt());
    assert!((c2.reference_level_dbfs - want).abs() < 0.02);
    assert!((want - (-18.0 - 6.0206)).abs() < 1e-3);
    // calibration without re-measuring (levels now on target through the
    // stored capture trims)
    let c3 = d.ref_calibration(c.suggested_trim_db).unwrap();
    assert!((c3.reference_level_dbfs - want).abs() < 0.03);
}

#[test]
fn wrong_band_is_flagged_by_the_detection() {
    let mut rig = Rig::new(FS);
    // the user says "left band" but plays a lateral band (in phase)
    cmd(&rig, RecCommand::Ref(Mod::Left));
    let n = secs(5.5);
    rig.feed(
        &tone(1000.0, -20.0, 0.0, n),
        &tone(1000.0, -20.0, 0.0, n),
        480,
    );
    let d = rig.shared.testrec.get();
    assert_eq!(band(&d, Mod::Left).detected, Some(Mod::Lateral));
    // vertical band as a custom-profile slot
    cmd(&rig, RecCommand::Ref(Mod::Vertical));
    rig.feed(
        &tone(1000.0, -20.0, 0.0, n),
        &tone(1000.0, -20.0, PI, n),
        480,
    );
    let d = rig.shared.testrec.get();
    assert_eq!(band(&d, Mod::Vertical).detected, Some(Mod::Vertical));
    assert!(band(&d, Mod::Vertical).phase_deg.unwrap().abs() > 175.0);
    // both channels driven: levels of both channels read directly
    assert!((band(&d, Mod::Vertical).ch[1].level_dbfs + 20.0).abs() < 0.02);
    let c = d.ref_calibration([0.0; 2]).unwrap();
    // a lateral/vertical-only record: 5 cm/s peak lateral = reference itself
    assert!(c.warnings.is_empty() || c.warnings.iter().all(|w| w.contains("detected")));
}

#[test]
fn silent_input_gives_a_message_not_a_result() {
    let mut rig = Rig::new(FS);
    cmd(&rig, RecCommand::Ref(Mod::Left));
    rig.feed(&vec![0.0; secs(5.5)], &vec![0.0; secs(5.5)], 480);
    assert!(rig.shared.testrec.get().ref_bands.is_empty());
    assert!(!rig.shared.snapshot().rec.message.is_empty());
}

#[test]
fn wow_flutter_from_a_1khz_carrier() {
    let n = secs(12.0);
    // +-0.1 % deviation at 4 Hz: beta = 1 Hz / 4 Hz
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / FS;
            (0.1 * (2.0 * PI * 1000.0 * t + 0.25 * (2.0 * PI * 4.0 * t).sin()).sin()) as f32
        })
        .collect();
    let mut rig = Rig::new(FS);
    cmd(&rig, RecCommand::WowFlutter);
    rig.feed(&x, &x, 480);
    let w = rig.shared.testrec.get().wf.expect("wf result");
    assert_eq!(w.carrier_hz, 1000.0);
    assert!(w.speed_pct.abs() < 0.005, "{}", w.speed_pct);
    // 0.1 % peak; the weighting is 0 dB at 4 Hz
    assert!((w.weighted_peak_pct - 0.1).abs() < 0.005, "{w:?}");
    assert!((w.unweighted_peak_pct - 0.1).abs() < 0.005, "{w:?}");
    assert!((w.weighted_rms_pct - 0.0707).abs() < 0.0035, "{w:?}");
}

#[test]
fn wow_flutter_reports_speed_error() {
    let x = tone(1002.0, -10.0, 0.0, secs(11.0));
    let mut rig = Rig::new(FS);
    cmd(&rig, RecCommand::WowFlutter);
    rig.feed(&x, &x, 480);
    let w = rig.shared.testrec.get().wf.expect("wf");
    assert!((w.speed_pct - 0.2).abs() < 0.005, "{}", w.speed_pct);
}

#[test]
fn tracking_table_finds_the_highest_clean_amplitude() {
    let mut rig = Rig::new(FS);
    let amps = [50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
    // distortion (third harmonic) rises at 80 um
    let h3 = [0.003, 0.003, 0.004, 0.05, 0.10, 0.2];
    for (i, (&a, &h)) in amps.iter().zip(&h3).enumerate() {
        let level = 10f64.powf((-30.0 + 20.0 * (a / 50.0f64).log10()) / 20.0);
        let n = secs(3.5);
        let x: Vec<f32> = (0..n)
            .map(|k| {
                let p = 2.0 * PI * 315.0 * k as f64 / FS;
                (level * (p.sin() + h * (3.0 * p).sin())) as f32
            })
            .collect();
        cmd(&rig, RecCommand::Tracking(i));
        rig.feed(&x, &x, 480);
    }
    let d = rig.shared.testrec.get();
    assert_eq!(d.tracking.len(), 6);
    assert_eq!(d.track_sel, 5, "auto-advanced to the last band");
    for (r, &a) in d.tracking.iter().zip(&amps) {
        assert_eq!(r.amp_um, a);
        assert!((r.freq_hz - 315.0).abs() < 0.05);
    }
    assert!((d.tracking[2].ch[0].thd_percent.unwrap() - 0.4).abs() < 0.02);
    let rep = evaluate_tracking(&d.tracking, &d);
    assert_eq!(rep.highest_clean_um, Some(70.0), "{:?}", rep.rows);
    assert!(rep.rows[3].mistracked && !rep.rows[2].mistracked);
    // expected velocity of 50 um at 315 Hz and the step to 60 um
    assert!((rep.rows[0].velocity_cm_s - 9.896).abs() < 0.001);
    assert!((rep.rows[1].expected_step_db.unwrap() - 1.58).abs() < 0.01);
    // measured level steps follow the amplitudes
    assert!((rep.rows[1].measured_step_db.unwrap() - 1.5836).abs() < 0.03);
}

/// Constant-velocity sweep 800 Hz..20 kHz through the ideal RIAA
/// de-emphasis and a known "error EQ", on both channels.
fn sweep_signal(secs_total: f64, f0: f64, f1: f64, err: &dyn Fn(f64) -> f64) -> Vec<f32> {
    let n = (secs_total * FS) as usize;
    let r = (f1 / f0).ln() / secs_total;
    let mut ph = 0.0;
    (0..n)
        .map(|i| {
            let t = i as f64 / FS;
            let f = f0 * (r * t).exp();
            ph += 2.0 * PI * f / FS;
            let g = analog_response(Mode::DeEmphasis, Options::default(), f).norm()
                * 10f64.powf(err(f) / 20.0);
            (0.2 * g * ph.sin()) as f32
        })
        .collect()
}

fn err_eq(f: f64) -> f64 {
    let o = (f / 1000.0).log2();
    -0.8 * o + 1.5 * (-(o * o) / 6.0).exp() * (2.0 * o).sin() + 0.6 * (o / 4.0).sin()
}

#[test]
fn response_capture_end_to_end_and_export() {
    let x = sweep_signal(30.0, 800.0, 20_000.0, &err_eq);
    let mut rig = Rig::new(FS);
    rig.shared.set_force_active(true);
    cmd(&rig, RecCommand::SweepArm);
    // some silence and a voice-like burst before the sweep
    let lead = vec![0.0f32; secs(1.0)];
    rig.feed(&lead, &lead, 480);
    let mut cl = vec![0.0f32; secs(3.0)];
    for (i, v) in cl.iter_mut().enumerate() {
        let t = i as f64 / FS;
        *v = (0.1
            * (1..=8)
                .map(|h| (2.0 * PI * 120.0 * h as f64 * t).sin() / h as f64)
                .sum::<f64>()) as f32;
    }
    rig.feed(&cl, &cl, 480);
    rig.feed(&x, &x, 480);
    // live view while armed
    let live = rig.shared.snapshot().rec.sweep.clone().expect("armed");
    assert!(live.accepted[0] > 100);
    assert!(live.coverage.iter().filter(|&&c| c).count() > 100);
    cmd(&rig, RecCommand::SweepStop);
    rig.engine.pump();
    let d = rig.shared.testrec.get();
    let sw = d.sweep.as_ref().expect("captured");
    assert_eq!(sw.detected, Some(Mod::Lateral), "{sw:?}");
    assert!(sw.level_diff_db.unwrap().abs() < 0.05);
    assert!(sw.phase_deg.unwrap().abs() < 1.0);
    // nothing below the sweep start
    assert!(sw.cells[0].iter().all(|c| c.1 >= 799.0));
    let resp = d.response().unwrap();
    assert_eq!(resp.valid, [true, true]);
    let (lo, hi) = resp.covered.unwrap();
    assert!((800.0..830.0).contains(&lo), "{lo}");
    assert!(hi > 19_000.0, "{hi}");
    // JSON export parses back
    let json = curve_json("Playback chain", &resp.selected);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["version"], 1);
    assert_eq!(v["kind"], "response_curve");
    assert_eq!(v["name"], "Playback chain");
    let pts = v["points"].as_array().unwrap();
    assert!(pts.len() > 100);
    let mut last = 0.0;
    let mut worst = 0.0f64;
    for p in pts {
        let f = p["freq_hz"].as_f64().unwrap();
        let db = p["db"].as_f64().unwrap();
        assert!(f >= 800.0 && f > last, "{f}");
        last = f;
        // the deviation from nominal is the error EQ relative to 1 kHz
        let want = err_eq(f) - err_eq(1000.0);
        worst = worst.max((db - want).abs());
    }
    assert!(worst < 0.25, "worst deviation error {worst}");
    // without the RIAA preamp the expected curve is flat: the RIAA slope shows
    let mut d2 = d.clone();
    d2.preamp_riaa = false;
    let r2 = d2.response().unwrap();
    assert!(r2.stats_all.unwrap().min_db < -10.0);
    // per-channel export
    d2 = d.clone();
    d2.export_channel = neometer::testrec::ExportChannel::Left;
    assert!(!d2.response().unwrap().selected.is_empty());
    // normalisation to the reference measurement of step 1
    let mut d3 = d.clone();
    d3.norm = NormMode::ToReference;
    assert!(d3.response().is_err(), "needs step 1");
}

#[test]
fn single_channel_sweep_uses_only_the_driven_channel() {
    let x = sweep_signal(12.0, 800.0, 8_000.0, &err_eq);
    let quiet: Vec<f32> = x.iter().map(|v| v * 0.03).collect();
    let mut rig = Rig::new(FS);
    cmd(&rig, RecCommand::SweepArm);
    rig.feed(&x, &quiet, 480);
    cmd(&rig, RecCommand::SweepStop);
    rig.engine.pump();
    let d = rig.shared.testrec.get();
    assert_eq!(d.sweep.as_ref().unwrap().detected, Some(Mod::Left));
    let r = d.response().unwrap();
    assert_eq!(r.valid, [true, false]);
    // the average is the left channel alone
    assert_eq!(r.average.len(), r.per_channel[0].len());
    for (a, b) in r.average.iter().zip(&r.per_channel[0]) {
        assert!((a.1 - b.1).abs() < 1e-9);
    }
    // user override of the modulation
    let mut d2 = d.clone();
    d2.sweep_mod = Some(Mod::Lateral);
    assert_eq!(d2.response().unwrap().valid, [true, true]);
}

#[test]
fn trims_unity_exact_smoothed_and_before_the_meters() {
    let n = secs(1.0);
    let x = tone(1000.0, -20.0, 0.0, n);
    // 0 dB: bit exact
    let mut rig = Rig::new(FS);
    rig.params.trim_db = [0.0, 0.0];
    let (a, b) = rig.feed(&x, &x, 300);
    assert_eq!(a, x);
    assert_eq!(b, x);
    // +6 dB on L only, ramped over 20 ms
    rig.params.trim_db = [6.0, 0.0];
    let ones = vec![0.5f32; secs(0.1)];
    let (a, b) = rig.feed(&ones, &ones, 64);
    assert_eq!(b, ones, "R untouched");
    let g = 10f64.powf(6.0 / 20.0);
    let ramp = (0.02 * FS) as usize;
    // first sample almost unity, strictly rising, then constant
    assert!((f64::from(a[0]) / 0.5 - 1.0).abs() < 0.01, "{}", a[0]);
    for (i, w) in a.windows(2).take(ramp - 1).enumerate() {
        assert!(w[1] > w[0], "not rising at {i}: {} {}", w[0], w[1]);
    }
    assert!(a[ramp / 2] > 0.5 && f64::from(a[ramp / 2]) < 0.5 * g);
    assert!((f64::from(a[ramp + 5]) - 0.5 * g).abs() < 1e-6);
    assert!((f64::from(a[ones.len() - 1]) - 0.5 * g).abs() < 1e-6);
    // back to 0 dB: bit exact again after the ramp
    rig.params.trim_db = [0.0, 0.0];
    let _ = rig.feed(&ones, &ones, 64);
    let (a, _) = rig.feed(&x, &x, 77);
    assert_eq!(a, x);
    // before the meters: the pass-through, the peak meter, the vinyl meters
    // and the analysis ring all see +6 dB
    let mut rig = Rig::new(FS);
    rig.params.trim_db = [6.0, -6.0];
    rig.feed(&vec![0.0; secs(0.1)], &vec![0.0; secs(0.1)], 128);
    rig.shared.request_reset(neometer::shared::reset::ALL);
    let (a, b) = rig.feed(&x, &x, 128);
    let pk = |v: &[f32]| v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!((20.0 * f64::from(pk(&a)).log10() + 14.0).abs() < 0.01);
    assert!((20.0 * f64::from(pk(&b)).log10() + 26.0).abs() < 0.01);
    let ro = rig.shared.readouts();
    assert!(
        (ro.levels.peak_max_db[0] + 14.0).abs() < 0.02,
        "{:?}",
        ro.levels
    );
    assert!((ro.levels.peak_max_db[1] + 26.0).abs() < 0.02);
    assert!((ro.stereo.balance_db - 12.0).abs() < 0.05);
    assert!(
        (ro.vinyl[0].rms_db + 14.0).abs() < 0.05,
        "{:?}",
        ro.vinyl[0]
    );
    assert_eq!(rig.shared.applied_trim_db(), [6.0, -6.0]);
    // out-of-range / non-finite trims are clamped
    rig.params.trim_db = [100.0, f64::NAN];
    rig.feed(&vec![0.0; 480], &vec![0.0; 480], 128);
    assert_eq!(rig.shared.applied_trim_db(), [24.0, 0.0]);
    // mono layout trims its single channel
    let mut mono = Rig::with(FS, true);
    mono.params.trim_db = [6.0, 0.0];
    mono.feed(&vec![0.0; secs(0.1)], &vec![0.0; secs(0.1)], 128);
    let (a, _) = mono.feed(&x, &x, 128);
    assert!((20.0 * f64::from(pk(&a)).log10() + 14.0).abs() < 0.01);
}

#[test]
fn tone_analysis_sees_the_trimmed_signal() {
    let mut rig = Rig::new(FS);
    rig.params.trim_db = [2.5, 1.2];
    rig.feed(&vec![0.0; secs(0.1)], &vec![0.0; secs(0.1)], 128);
    measure_bands(&mut rig, 1000.0, -20.5, -19.2, 40.0, 40.0);
    let d = rig.shared.testrec.get();
    let b = band(&d, Mod::Left);
    assert_eq!(b.trim_db, [2.5, 1.2]);
    assert!((b.ch[0].level_dbfs + 18.0).abs() < 0.02);
}

#[test]
fn persisted_state_survives_a_round_trip() {
    let mut rig = Rig::new(FS);
    let x = sweep_signal(8.0, 800.0, 6_000.0, &err_eq);
    cmd(&rig, RecCommand::SweepArm);
    rig.feed(&x, &x, 480);
    cmd(&rig, RecCommand::SweepStop);
    rig.engine.pump();
    let params = NeoMeterParams::default();
    let d = rig.shared.testrec.get();
    assert!(d.sweep.is_some());
    params.testrec.replace(d.clone());
    params.testrec.update(|t| {
        t.profile_kind = neometer::testrec::ProfileKind::Custom;
        t.custom.ref_freq_hz = 1500.0;
    });
    let saved = params.serialize_fields();
    assert!(saved.contains_key("test-record"));
    let restored = NeoMeterParams::default();
    restored.deserialize_fields(&saved);
    let r = restored.testrec.get();
    let (a, b) = (r.sweep.as_ref().unwrap(), d.sweep.as_ref().unwrap());
    // JSON floats may differ in the last bit
    for ch in 0..2 {
        assert_eq!(a.cells[ch].len(), b.cells[ch].len());
        for (p, q) in a.cells[ch].iter().zip(&b.cells[ch]) {
            assert!(p.0 == q.0 && (p.1 - q.1).abs() < 1e-9 && (p.2 - q.2).abs() < 1e-9);
        }
    }
    assert_eq!((a.detected, a.pair_blocks), (b.detected, b.pair_blocks));
    assert_eq!(r.custom.ref_freq_hz, 1500.0);
    assert_eq!(r.profile_kind, neometer::testrec::ProfileKind::Custom);
    assert!(r.response().is_ok());
    // garbage in the state is sanitised, not fatal
    let mut bad = saved.clone();
    bad.insert(
        "test-record".into(),
        "{\"ref_seconds\": -5, \"track_sel\": 99}".into(),
    );
    let p3 = NeoMeterParams::default();
    p3.deserialize_fields(&bad);
    let g = p3.testrec.get();
    assert_eq!(g.ref_seconds, 1.0);
    assert!(g.track_sel < 6);
}

#[test]
fn summary_text_mentions_every_step() {
    let mut rig = Rig::new(FS);
    measure_bands(&mut rig, 1002.0, -20.5, -19.2, 28.0, 31.0);
    let s = rig.shared.testrec.get().summary_text([0.0, 0.0]);
    for key in [
        "Ortofon Test Record",
        "balance",
        "Separation",
        "speed deviation",
        "suggested",
        "Velocity calibration",
        "Response:",
        "Wow & flutter: not measured",
        "Tracking: not measured",
    ] {
        assert!(s.contains(key), "missing {key:?} in\n{s}");
    }
}
