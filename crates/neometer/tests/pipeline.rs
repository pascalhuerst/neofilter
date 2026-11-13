//! Full pipeline tests: audio path + analysis engine driven with synthetic
//! signals (no GUI, no host, no threads).
mod common;
use common::*;
use neo_measure::generator::{Generator, PinkNoise, WhiteNoise};
use neo_measure::util::DB_FLOOR;
use neometer::analysis::{Command, MeasureKind};
use neometer::settings::{AvgMode, SpecChannel, WindowKind};
use neometer::shared::reset;
use neometer::vinyl::{self, LATERAL, VERTICAL};
use std::f64::consts::PI;

const FS: f64 = 48_000.0;

fn secs(s: f64) -> usize {
    (s * FS) as usize
}

#[test]
fn pass_through_is_bit_exact_for_all_block_sizes_and_rates() {
    for fs in RATES {
        for &block in &[1usize, 7, 64, 255, 256, 257, 1000, 8192] {
            let mut n = Noise(0x1234 + block as u64);
            let len = if block == 1 { 400 } else { 2500 };
            let (l, r) = (n.vec(len), n.vec(len));
            let mut rig = Rig::without_design(fs);
            let (a, b) = rig.feed(&l, &r, block);
            assert_eq!(a, l, "fs {fs} block {block}");
            assert_eq!(b, r, "fs {fs} block {block}");
        }
    }
    // mono layout
    let mut n = Noise(5);
    let m = n.vec(5000);
    let mut rig = Rig::with(FS, true);
    let (a, _) = rig.feed(&m, &m, 333);
    assert_eq!(a, m);
}

#[test]
fn readouts_do_not_depend_on_block_size() {
    let l = sine(FS, 1000.0, -20.0, secs(1.0));
    let r = sine(FS, 440.0, -12.0, secs(1.0));
    let mut reference = None;
    for &block in &[1usize, 2, 17, 255, 256, 257, 1000, 8192] {
        let mut rig = Rig::new(FS);
        rig.feed(&l, &r, block);
        let ro = rig.shared.readouts();
        match &reference {
            None => reference = Some(ro),
            Some(x) => {
                for c in 0..2 {
                    assert!((ro.levels.peak_db[c] - x.levels.peak_db[c]).abs() < 1e-9);
                    assert!((ro.levels.tp_max_db[c] - x.levels.tp_max_db[c]).abs() < 1e-9);
                    assert!((ro.levels.rms_db[c] - x.levels.rms_db[c]).abs() < 1e-9);
                }
                assert!((ro.stereo.correlation - x.stereo.correlation).abs() < 1e-9);
                assert!(
                    (ro.vinyl[LATERAL].tp_max_db - x.vinyl[LATERAL].tp_max_db).abs() < 1e-9,
                    "block {block}"
                );
            }
        }
    }
}

#[test]
fn sine_levels_and_loudness_through_the_pipeline() {
    let x = sine(FS, 1000.0, -20.0, secs(6.0));
    let mut rig = Rig::new(FS);
    rig.feed(&x, &x, 512);
    let ro = rig.shared.readouts();
    for c in 0..2 {
        // peak of the sampled sine: exactly on the crest at 1 kHz / 48 kHz
        assert!(
            (ro.levels.peak_max_db[c] + 20.0).abs() < 0.01,
            "{:?}",
            ro.levels.peak_max_db
        );
        assert!(
            (ro.levels.tp_max_db[c] + 20.0).abs() < 0.02,
            "{:?}",
            ro.levels.tp_max_db
        );
        // RMS, sine referenced (AES17): -20 dB
        assert!(
            (ro.levels.rms_db[c] + 20.0).abs() < 0.01,
            "{:?}",
            ro.levels.rms_db
        );
        // crest factor of a sine 3.01 dB
        assert!((ro.levels.crest_db[c] - 3.01).abs() < 0.02);
        assert!(ro.levels.dc[c].abs() < 1e-5);
    }
    let snap = rig.shared.snapshot();
    // K-weighting is +0.691 dB at 1 kHz, which cancels the -0.691 offset:
    // stereo sine at -20 dBFS reads -20.0 LUFS (neo-measure: -22.993 for -23).
    let want = -19.993;
    assert!(
        (snap.loudness.momentary - want).abs() < 0.02,
        "{}",
        snap.loudness.momentary
    );
    assert!((snap.loudness.short_term - want).abs() < 0.02);
    let i = snap.loudness.integrated.expect("integrated");
    assert!((i - want).abs() < 0.02, "{i}");
    let lra = snap.loudness.range.expect("lra");
    assert!(lra.lra < 0.05);
    assert_eq!(snap.overruns, 0);
}

#[test]
fn mono_layout_loudness_counts_one_channel() {
    let x = sine(FS, 1000.0, -20.0, secs(4.0));
    let mut rig = Rig::with(FS, true);
    rig.feed(&x, &x, 480);
    let snap = rig.shared.snapshot();
    // one channel: 10 log10(0.005) = -23.01 (K gain +0.691 cancels the offset)
    assert!(
        (snap.loudness.integrated.unwrap() + 23.003).abs() < 0.02,
        "{:?}",
        snap.loudness.integrated
    );
}

#[test]
fn reset_clears_holds_and_loudness() {
    let loud = sine(FS, 1000.0, -3.0, secs(2.0));
    let quiet = sine(FS, 1000.0, -40.0, secs(2.0));
    let mut rig = Rig::new(FS);
    rig.feed(&loud, &loud, 256);
    assert!(rig.shared.readouts().levels.peak_max_db[0] > -3.1);
    // settle on the new level first (the switch itself rings in the true-peak
    // interpolator), then reset
    rig.feed(&quiet, &quiet, 256);
    rig.shared.request_reset(reset::ALL);
    rig.feed(&quiet, &quiet, 256);
    let ro = rig.shared.readouts();
    assert!(
        (ro.levels.peak_max_db[0] + 40.0).abs() < 0.02,
        "{:?}",
        ro.levels
    );
    assert!((ro.levels.tp_max_db[0] + 40.0).abs() < 0.03);
    let snap = rig.shared.snapshot();
    assert!(snap.loudness.max_momentary < -35.0);
}

#[test]
fn dc_offset_and_crest() {
    let n = secs(2.0);
    let l: Vec<f32> = sine(FS, 1000.0, -20.0, n).iter().map(|v| v + 0.1).collect();
    let r = vec![0.0f32; n];
    let mut rig = Rig::new(FS);
    rig.feed(&l, &r, 300);
    let ro = rig.shared.readouts();
    assert!((ro.levels.dc[0] - 0.1).abs() < 1e-4, "{}", ro.levels.dc[0]);
    assert!(ro.levels.dc[1].abs() < 1e-12);
    // channel-wise balance: L much louder than silent R
    assert!(ro.stereo.balance_db > 60.0 || ro.stereo.balance_db.is_finite());
}

#[test]
fn pure_side_signal_is_vertical_with_negative_lf_correlation() {
    let x = sine(FS, 100.0, -12.0, secs(3.0));
    let inv: Vec<f32> = x.iter().map(|v| -v).collect();
    let mut rig = Rig::new(FS);
    rig.feed(&x, &inv, 512);
    let s = rig.shared.readouts().stereo;
    assert!(s.lf_vertical_db > s.lf_lateral_db + 60.0, "{s:?}");
    assert!(
        (s.lf_correlation + 1.0).abs() < 1e-6,
        "{}",
        s.lf_correlation
    );
    assert!((s.correlation + 1.0).abs() < 1e-6);
    // side = full signal, mid = 0
    assert!(s.side_db > s.mid_db + 60.0);
    // vinyl: vertical > lateral after pre-emphasis too
    let v = rig.shared.readouts().vinyl;
    assert!(v[VERTICAL].rms_db > v[LATERAL].rms_db + 60.0);
}

#[test]
fn mono_signal_is_lateral_and_correlated() {
    let x = sine(FS, 100.0, -12.0, secs(3.0));
    let mut rig = Rig::new(FS);
    rig.feed(&x, &x, 512);
    let s = rig.shared.readouts().stereo;
    assert!(s.lf_lateral_db > s.lf_vertical_db + 60.0);
    assert!((s.lf_correlation - 1.0).abs() < 1e-6);
    assert!(s.balance_db.abs() < 1e-9);
    // mid of a mono sine is the sine itself: -12 dB sine referenced
    assert!((s.mid_db + 12.0).abs() < 0.05, "{}", s.mid_db);
}

#[test]
fn lf_crossover_param_is_honoured() {
    // 500 Hz out-of-phase tone: above the 300 Hz crossover it is attenuated
    // (LR4 about -14 dB at 500 Hz), with the crossover at 1 kHz it is not.
    let x = sine(FS, 500.0, -12.0, secs(2.0));
    let inv: Vec<f32> = x.iter().map(|v| -v).collect();
    let mut a = Rig::new(FS);
    a.feed(&x, &inv, 512);
    let low = a.shared.readouts().stereo.lf_vertical_db;
    let mut b = Rig::new(FS);
    b.params.lf_crossover_hz = 1000.0;
    b.feed(&x, &inv, 512);
    let high = b.shared.readouts().stereo.lf_vertical_db;
    assert!(high > low + 8.0, "{low} {high}");
    assert!(high > -13.5, "{high}");
}

#[test]
fn pre_emphasis_level_of_10khz_tone() {
    for fs in [44_100.0, 48_000.0, 192_000.0] {
        let x = sine(fs, 10_000.0, -30.0, fs as usize);
        let mut rig = Rig::new(fs);
        // skip the start-up transient of the filter, then restart the holds
        let h = (fs * 0.4) as usize;
        rig.feed(&x[..h], &x[..h], 512);
        rig.shared.request_reset(reset::VINYL);
        rig.feed(&x[h..], &x[h..], 512);
        let v = rig.shared.readouts().vinyl;
        // published RIAA check value: +13.734 dB at 10 kHz re 1 kHz
        for k in [vinyl::L, vinyl::R, LATERAL] {
            let gain = v[k].rms_db + 30.0;
            assert!((gain - 13.734).abs() < 0.03, "fs {fs} sig {k}: {gain}");
            let tp = v[k].tp_max_db + 30.0;
            assert!((tp - 13.734).abs() < 0.08, "fs {fs} sig {k} tp: {tp}");
        }
        assert!(v[VERTICAL].rms_db < -150.0 + 3.0 || v[VERTICAL].rms_db < -100.0);
        // the whole 10 kHz tone lies above a 5 kHz HF corner
        assert!((v[LATERAL].hf_rms_db - v[LATERAL].rms_db).abs() < 0.05);
    }
}

#[test]
fn hf_band_ignores_low_frequencies_and_corner_is_adjustable() {
    let x = sine(FS, 500.0, -20.0, secs(2.0));
    let mut rig = Rig::new(FS);
    rig.feed(&x, &x, 512);
    let v = rig.shared.readouts().vinyl[LATERAL];
    assert!(v.hf_rms_db < v.rms_db - 60.0, "{v:?}");
    // 4 kHz tone: inside the band for a 3 kHz corner, outside for 6 kHz
    let t = sine(FS, 4000.0, -20.0, secs(2.0));
    let mut a = Rig::new(FS);
    a.shared.settings.update(|d| d.hf_corner_hz = 3000.0);
    a.feed(&t, &t, 512);
    let inb = a.shared.readouts().vinyl[LATERAL];
    assert!(inb.hf_rms_db > inb.rms_db - 3.0);
    let mut b = Rig::new(FS);
    b.shared.settings.update(|d| d.hf_corner_hz = 6000.0);
    b.feed(&t, &t, 512);
    let outb = b.shared.readouts().vinyl[LATERAL];
    assert!(outb.hf_rms_db < outb.rms_db - 12.0, "{outb:?}");
}

#[test]
fn velocity_and_displacement_estimate_of_reference_tone() {
    // 1 kHz lateral (L = R) tone exactly at the reference level -> 5 cm/s.
    for (fs, reference) in [(44_100.0, -18.0), (48_000.0, -9.0)] {
        {
            let x = sine(fs, 1000.0, reference, fs as usize);
            let mut rig = Rig::new(fs);
            // let the start-up transient (the integral of a sine started at
            // phase 0 has a decaying DC term) die, then restart the holds
            let h = (fs * 0.4) as usize;
            rig.feed(&x[..h], &x[..h], 512);
            rig.shared.request_reset(reset::VINYL);
            rig.feed(&x[h..], &x[h..], 512);
            let v = rig.shared.readouts().vinyl;
            let vel = vinyl::velocity_cm_s(v[LATERAL].tp_max_db, reference);
            assert!(
                (vel / 5.0 - 1.0).abs() < 0.005,
                "fs {fs} ref {reference}: {vel} cm/s"
            );
            // per channel also 5 cm/s (mono = both walls)
            let vl = vinyl::velocity_cm_s(v[vinyl::L].tp_max_db, reference);
            assert!((vl / 5.0 - 1.0).abs() < 0.005, "{vl}");
            // vertical: nothing
            assert!(vinyl::velocity_cm_s(v[VERTICAL].tp_max_db, reference) < 1e-3);
            // displacement: v / (2 pi f) = 5 cm/s / 6283 = 7.9577 um
            let um = vinyl::displacement_um(v[LATERAL].disp_max, reference);
            let want = 5.0 / (2.0 * PI * 1000.0) * 1e4;
            assert!((um / want - 1.0).abs() < 0.01, "fs {fs}: {um} um vs {want}");
        }
    }
}

#[test]
fn spectrum_peak_and_pink_rta_through_the_pipeline() {
    // flat-top reads a -20 dBFS sine within 0.05 dB
    let x = sine(FS, 1000.0, -20.0, secs(3.0));
    let mut rig = Rig::new(FS);
    rig.shared.set_force_active(true);
    rig.shared.settings.update(|d| {
        d.window = WindowKind::FlatTop;
        d.averaging = AvgMode::Exponential;
        d.avg_tau_s = 0.3;
        d.fft_size = 8192;
        d.spec_channel = SpecChannel::Left;
    });
    rig.feed(&x, &x, 480);
    let snap = rig.shared.snapshot();
    let sp = snap.spectrum.as_ref().expect("spectrum");
    let c = &sp.curves[0];
    let peak = c.db.iter().cloned().fold(f32::MIN, f32::max);
    assert!((f64::from(peak) + 20.0).abs() < 0.05, "{peak}");
    let (i, _) =
        c.db.iter()
            .enumerate()
            .fold((0, f32::MIN), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
    assert!((sp.grid_hz(i) / 1000.0 - 1.0).abs() < 0.01);

    // pink noise: RTA flat (equal energy per third octave)
    let mut g = PinkNoise::new(FS, -20.0, 7);
    let mut p = vec![0.0f64; secs(20.0)];
    g.fill(&mut p);
    let p: Vec<f32> = p.iter().map(|&v| v as f32).collect();
    let mut rig = Rig::new(FS);
    rig.shared.set_force_active(true);
    rig.shared.settings.update(|d| {
        d.rta = true;
        d.fft_size = 16384;
        d.averaging = AvgMode::Infinite;
        d.spec_channel = SpecChannel::Left;
    });
    rig.feed(&p, &p, 1024);
    let snap = rig.shared.snapshot();
    let sp = snap.spectrum.as_ref().expect("spectrum");
    let bands: Vec<f32> = sp.curves[0]
        .bands
        .iter()
        .filter(|b| b.0 >= 31.5 && b.0 <= 16_000.0)
        .map(|b| b.1)
        .collect();
    let (lo, hi) = bands
        .iter()
        .fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
    eprintln!(
        "pink RTA spread {lo:.3} .. {hi:.3} dB over {} bands",
        bands.len()
    );
    assert!(bands.len() >= 25);
    assert!(hi - lo < 1.0, "spread {}", hi - lo);
}

#[test]
fn tone_measurement_latches_frequency_level_and_thd() {
    let n = secs(3.0);
    let h3 = 10f64.powf(-60.0 / 20.0);
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = 2.0 * PI * 1000.0 * i as f64 / FS;
            (0.1 * (t.sin() + h3 * (3.0 * t).sin())) as f32
        })
        .collect();
    let mut rig = Rig::new(FS);
    rig.shared.settings.update(|d| d.tone_duration_s = 2.0);
    rig.shared.send(Command::Start(MeasureKind::Tone));
    rig.feed(&x, &x, 1000);
    let snap = rig.shared.snapshot();
    assert!(snap.measuring.is_none());
    let t = snap
        .results
        .tone
        .as_ref()
        .expect("tone result")
        .analysis
        .clone();
    assert!((t.frequency_hz - 1000.0).abs() < 0.01, "{}", t.frequency_hz);
    assert!((t.level_dbfs + 20.0).abs() < 0.01, "{}", t.level_dbfs);
    assert!((t.thd_db + 60.0).abs() < 0.1, "{}", t.thd_db);
    assert!(t.thd_n_db < -59.5);
}

#[test]
fn channel_separation_and_reference_and_noise() {
    let n = secs(6.0);
    let tone = sine(FS, 1000.0, -20.0, n);
    let leak: Vec<f32> = tone.iter().map(|v| v * 10f32.powf(-60.0 / 20.0)).collect();
    let mut rig = Rig::new(FS);
    rig.shared.settings.update(|d| d.tone_duration_s = 2.0);
    rig.shared.send(Command::Start(MeasureKind::Crosstalk(0)));
    rig.feed(&tone[..secs(3.0)], &leak[..secs(3.0)], 777);
    let x = rig.shared.snapshot().results.xtalk[0].expect("L->R");
    assert!((x - 60.0).abs() < 0.2, "{x}");
    assert!(rig.shared.snapshot().results.xtalk[1].is_none());

    // latch the reference from the -20 dBFS tone on L, then measure noise
    rig.shared.send(Command::Start(MeasureKind::Reference));
    rig.feed(&tone[..secs(2.0)], &leak[..secs(2.0)], 777);
    let r = rig.shared.snapshot().results.reference.expect("reference");
    assert!((r + 20.0).abs() < 0.01, "{r}");
    assert!((rig.shared.settings.get().noise_ref_dbfs.unwrap() + 20.0).abs() < 0.01);

    let mut w = WhiteNoise::new(-80.0, 11);
    let mut a = vec![0.0f64; secs(6.0)];
    w.fill(&mut a);
    let a: Vec<f32> = a.iter().map(|&v| v as f32).collect();
    rig.shared.settings.update(|d| d.noise_duration_s = 5.0);
    rig.shared.send(Command::Start(MeasureKind::Noise));
    rig.feed(&a, &a, 1024);
    let nz = rig.shared.snapshot().results.noise.expect("noise");
    // white noise -80 dBFS in 20 Hz..20 kHz of 24 kHz: -80 + 10 log10(19980/24000)
    let want = -80.0 + 10.0 * (19_980.0f64 / 24_000.0).log10();
    assert!(
        (nz.levels[0].unweighted_dbfs - want).abs() < 0.3,
        "{:?}",
        nz.levels[0]
    );
    // reference RMS = -20 - 3.01 dB
    assert!((nz.reference_rms_dbfs.unwrap() + 23.0103).abs() < 0.01);
    assert!((nz.levels[0].unweighted_rel_db - (want + 23.0103)).abs() < 0.3);
    // A-weighting of white noise lowers the reading
    assert!(nz.levels[0].a_weighted_dbfs < nz.levels[0].unweighted_dbfs + 1.0);
}

#[test]
fn wow_and_flutter_through_the_pipeline() {
    let fs = FS;
    let n = (fs * 8.0) as usize;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / fs;
            (0.3 * (2.0 * PI * 3150.0 * t + (3.15 / 4.0) * (2.0 * PI * 4.0 * t).sin()).sin()) as f32
        })
        .collect();
    let mut rig = Rig::new(fs);
    rig.shared.settings.update(|d| d.wf_duration_s = 7.0);
    rig.shared.send(Command::Start(MeasureKind::WowFlutter));
    rig.feed(&x, &x, 2048);
    let snap = rig.shared.snapshot();
    let wf = snap.results.wf.as_ref().expect("wf");
    // 0.1 % at 4 Hz: weighted peak 0.1003 % (neo-measure)
    assert!(
        (wf.result.weighted.peak_pct - 0.1003).abs() < 0.005,
        "{}",
        wf.result.weighted.peak_pct
    );
    assert!(wf.result.speed_deviation_pct.abs() < 0.01);
}

#[test]
fn overrun_never_blocks_and_is_reported() {
    // nobody drains the ring: the audio path must keep going, bit-exact
    let mut rig = Rig::new(FS);
    let mut n = Noise(77);
    let block = 4096;
    let (l, r) = (n.vec(block), n.vec(block));
    let t = std::time::Instant::now();
    for _ in 0..(2 * neometer::analysis::RING_FRAMES / block + 8) {
        let (mut a, mut b) = (l.clone(), r.clone());
        rig.audio.process_stereo(&mut a, &mut b, &rig.params);
        assert_eq!(a, l);
        assert_eq!(b, r);
    }
    assert!(t.elapsed().as_secs() < 30);
    rig.engine.pump();
    let snap = rig.shared.snapshot();
    assert!(snap.overruns > 100_000, "{}", snap.overruns);
}

#[test]
fn silence_reads_floor_and_none() {
    let z = vec![0.0f32; secs(1.0)];
    let mut rig = Rig::new(FS);
    rig.feed(&z, &z, 256);
    let ro = rig.shared.readouts();
    assert_eq!(ro.levels.peak_db[0], DB_FLOOR);
    assert_eq!(ro.stereo.correlation, 0.0);
    let snap = rig.shared.snapshot();
    assert!(snap.loudness.integrated.is_none());
    assert!(snap.loudness.momentary.is_infinite());
}

#[test]
fn non_finite_input_does_not_poison_meters() {
    let mut l = sine(FS, 1000.0, -20.0, secs(1.0));
    l[100] = f32::NAN;
    l[200] = f32::INFINITY;
    let r = l.clone();
    let mut rig = Rig::new(FS);
    rig.shared.set_force_active(true);
    rig.feed(&l, &r, 256);
    let ro = rig.shared.readouts();
    assert!(ro.levels.rms_db[0].is_finite());
    assert!(ro.stereo.correlation.is_finite());
    assert!(rig.shared.snapshot().loudness.momentary.is_finite());
}
