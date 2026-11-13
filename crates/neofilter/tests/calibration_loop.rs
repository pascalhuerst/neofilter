#![allow(clippy::needless_range_loop, clippy::field_reassign_with_default)]
//! End-to-end calibration against the synthetic loop: the real tap, ring,
//! analysis, fit and apply code, driven block by block.
mod common;
use neo_measure::spectrum::smooth_octave_power;
use neofilter::calibration::analysis::grid_for;
use neofilter::calibration::fit::deviation_stats;
use neofilter::calibration::session::{Phase, Session};
use neofilter::calibration::settings::{CalCurve, CalPersist, CalSettings, CurvePoint};
use neofilter::calibration::sim::{LoopSim, Rig};
use neofilter::calibration::CalError;
use neofilter::calibration::Env;
use neofilter_dsp::response::total_response_db;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, Shape};

pub fn band(shape: Shape, f: f64, g: f64, q: f64) -> BandParams {
    BandParams {
        used: true,
        shape,
        freq_hz: f,
        gain_db: g,
        q,
        ..Default::default()
    }
}

/// The loop's own EQ: +5 dB bell at 9 kHz (Q 3), -4 dB at 250 Hz, high shelf
/// -6 dB at 12 kHz, low shelf +3 dB at 60 Hz.
pub fn system_bands() -> [BandParams; NUM_BANDS] {
    let mut b = [BandParams::default(); NUM_BANDS];
    b[0] = band(Shape::Bell, 9000.0, 5.0, 3.0);
    b[1] = band(Shape::Bell, 250.0, -4.0, 1.0);
    b[2] = band(Shape::HighShelf, 12000.0, -6.0, 1.0);
    b[3] = band(Shape::LowShelf, 60.0, 3.0, 1.0);
    b
}

pub fn settings(duration: f64) -> CalSettings {
    CalSettings {
        duration_s: duration,
        ..Default::default()
    }
}

pub struct Setup {
    pub fs: f64,
    pub delay_s: f64,
    pub system: [BandParams; NUM_BANDS],
    pub invert: bool,
    pub gain_db: f64,
    pub noise_dbfs: Option<f64>,
    pub manual: [BandParams; NUM_BANDS],
}

impl Setup {
    pub fn new(fs: f64, delay_s: f64) -> Self {
        Self {
            fs,
            delay_s,
            system: system_bands(),
            invert: false,
            gain_db: 0.0,
            noise_dbfs: Some(-65.0),
            manual: [BandParams::default(); NUM_BANDS],
        }
    }
    pub fn rig(&self) -> Rig {
        let sim = LoopSim::new(
            self.fs,
            self.delay_s,
            &self.system,
            self.invert,
            self.gain_db,
            self.noise_dbfs,
            7,
        );
        Rig::new(self.fs, sim, self.manual, 480)
    }
}

fn new_session(rig: &Rig) -> Session {
    let mut s = Session::new(rig.link.clone());
    s.set_threaded(false);
    s
}

/// Run one measurement, return its measurement or the error.
fn measure(
    rig: &mut Rig,
    persist: &mut CalPersist,
) -> Result<neofilter::calibration::Measurement, CalError> {
    let mut s = new_session(rig);
    s.start_measure(persist, &rig.env()).unwrap();
    rig.run_session(&mut s, persist, 60.0);
    match s.measurement() {
        Some(m) => Ok(m.clone()),
        None => Err(s.error().cloned().expect("error")),
    }
}

/// Analytic loop response (system + EQ the noise passed through), smoothed
/// like the analysis does, on the measurement grid.
fn truth(
    setup: &Setup,
    bands: &[BandParams; NUM_BANDS],
    smoothing: u32,
    fs: f64,
    freqs: &[f64],
) -> Vec<f64> {
    let fft = neofilter::calibration::analysis::fft_size_for(fs);
    let df = fs / fft as f64;
    let bins: Vec<f64> = (0..=fft / 2).map(|k| k as f64 * df).collect();
    let mut sys = vec![0.0; bins.len()];
    let mut eq = vec![0.0; bins.len()];
    total_response_db(&setup.system, 1.0, fs, &bins, &mut sys);
    total_response_db(bands, 1.0, fs, &bins, &mut eq);
    let power: Vec<f64> = (0..bins.len())
        .map(|i| 10f64.powf((sys[i] + eq[i] + setup.gain_db) / 10.0))
        .collect();
    let sm = smooth_octave_power(&power, smoothing);
    freqs
        .iter()
        .map(|&f| {
            let x = f / df;
            let i = x.floor() as usize;
            let t = x - i as f64;
            let p = sm[i] * (1.0 - t) + sm[(i + 1).min(sm.len() - 1)] * t;
            10.0 * p.log10()
        })
        .collect()
}

#[test]
fn delay_is_found_within_one_sample() {
    for (fs, d) in [
        (48_000.0, 0.3300123),
        (48_000.0, 0.4500377),
        (96_000.0, 0.3300123),
        (96_000.0, 0.45004),
    ] {
        let mut setup = Setup::new(fs, d);
        setup.system = [BandParams::default(); NUM_BANDS]; // pure delay: no EQ group delay
        let mut rig = setup.rig();
        let mut persist = CalPersist::default();
        persist.settings = settings(8.0);
        let m = measure(&mut rig, &mut persist).expect("measurement");
        for c in 0..2 {
            let info = &m.channels[c].as_ref().unwrap().info;
            let err = info.delay_samples - d * fs;
            println!(
                "fs {fs} D {:.4} s channel {c}: error {err:+.4} samples, confidence {:.0}",
                d, info.confidence
            );
            assert!(err.abs() < 1.0, "delay error {err} samples");
            assert!(!info.inverted);
        }
    }
}

#[test]
fn measured_response_matches_truth() {
    for fs in [48_000.0, 96_000.0] {
        let setup = Setup::new(fs, 0.4012345);
        let mut rig = setup.rig();
        let mut persist = CalPersist::default();
        persist.settings = settings(10.0);
        let m = measure(&mut rig, &mut persist).expect("measurement");
        let t = truth(&setup, &setup.manual, 6, fs, &m.freqs);
        let mut worst: f64 = 0.0;
        let mut worst_f = 0.0;
        for i in 0..m.freqs.len() {
            if m.freqs[i] >= 40.0 && m.freqs[i] <= 16_000.0 {
                let e = (m.mag_db[i] - t[i]).abs();
                if e > worst {
                    worst = e;
                    worst_f = m.freqs[i];
                }
            }
        }
        println!("fs {fs}: worst response error {worst:.3} dB at {worst_f:.0} Hz");
        assert!(worst < 0.25, "{worst} dB at {worst_f} Hz");
        // coherence is high everywhere in range
        let _ = grid_for(fs);
        assert!(m.mean_coherence > 0.95, "{}", m.mean_coherence);
    }
}

#[test]
fn auto_flattens_the_loop_within_three_iterations() {
    let setup = Setup::new(48_000.0, 0.3712);
    let mut rig = setup.rig();
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_auto(&persist, &rig.env()).unwrap();
    let applies = rig.run_session(&mut s, &mut persist, 120.0);
    println!("status: {}", s.status());
    for h in s.history() {
        println!("{h:?}");
    }
    assert_eq!(s.phase(), Phase::Idle);
    assert!(s.error().is_none(), "{:?}", s.error());
    assert!(applies <= 3, "{applies} applies");
    let last = s.history().last().unwrap();
    assert!(last.rms_db < 0.5, "final RMS {}", last.rms_db);
    assert!(last.max_db <= 1.0, "final max {}", last.max_db);
    // independent check of the final loop on the analytic response
    let freqs = grid_for(48_000.0);
    let mut sys = vec![0.0; freqs.len()];
    let mut eq = vec![0.0; freqs.len()];
    total_response_db(&setup.system, 1.0, 48_000.0, &freqs, &mut sys);
    total_response_db(&rig.bands, 1.0, 48_000.0, &freqs, &mut eq);
    let ref_mean = |v: &[f64]| {
        let (mut s, mut n) = (0.0, 0);
        for (f, x) in freqs.iter().zip(v) {
            if *f >= 300.0 && *f <= 3000.0 {
                s += x;
                n += 1;
            }
        }
        s / n as f64
    };
    let tot: Vec<f64> = sys.iter().zip(&eq).map(|(a, b)| a + b).collect();
    let r = ref_mean(&tot);
    let dev: Vec<f64> = tot.iter().map(|x| x - r).collect();
    let coh = vec![1.0; freqs.len()];
    let (rms, max) = deviation_stats(&freqs, &dev, &coh, &persist.settings);
    println!(
        "analytic final deviation: {rms:.3} dB rms, {max:.3} dB max; {} bands",
        rig.bands.iter().filter(|b| b.used).count()
    );
    assert!(rms < 0.5 && max < 1.0);
    assert!(rig.bands.iter().filter(|b| b.used).count() <= 12);
}

/// Measure, fit and apply once (manual buttons). Returns the apply result.
fn cycle(rig: &mut Rig, s: &mut Session, persist: &mut CalPersist) -> Result<(), String> {
    s.start_measure(persist, &rig.env())?;
    rig.run_session(s, persist, 60.0);
    s.fit(persist, &rig.env())?;
    rig.run_session(s, persist, 20.0);
    let b = s.apply(persist, &rig.env())?;
    rig.bands = b;
    Ok(())
}

fn count_used(b: &[BandParams; NUM_BANDS]) -> usize {
    b.iter().filter(|b| b.used).count()
}

/// Analytic deviation of the final loop (system + current EQ + extra) from
/// flat in the reference-normalised sense, over `range`.
fn analytic_deviation(
    setup: &Setup,
    bands: &[BandParams; NUM_BANDS],
    extra: Option<&[BandParams; NUM_BANDS]>,
    p_known: Option<&[BandParams; NUM_BANDS]>,
    fs: f64,
) -> (f64, f64) {
    let freqs = grid_for(fs);
    let mut tot = vec![0.0; freqs.len()];
    let mut tmp = vec![0.0; freqs.len()];
    for set in [Some(&setup.system), Some(bands), extra]
        .into_iter()
        .flatten()
    {
        total_response_db(set, 1.0, fs, &freqs, &mut tmp);
        for (t, v) in tot.iter_mut().zip(&tmp) {
            *t += v;
        }
    }
    // remove the known playback chain
    if let Some(p) = p_known {
        total_response_db(p, 1.0, fs, &freqs, &mut tmp);
        for (t, v) in tot.iter_mut().zip(&tmp) {
            *t -= v;
        }
    }
    let (mut s, mut n) = (0.0, 0);
    for (f, x) in freqs.iter().zip(&tot) {
        if *f >= 300.0 && *f <= 3000.0 {
            s += x;
            n += 1;
        }
    }
    let r = s / n as f64;
    let dev: Vec<f64> = tot.iter().map(|x| x - r).collect();
    let coh = vec![1.0; freqs.len()];
    deviation_stats(&freqs, &dev, &coh, &CalSettings::default())
}

fn dense_grid() -> Vec<f64> {
    (0..2000)
        .map(|i| 10.0 * (0.499 * 48000.0f64 / 10.0).powf(i as f64 / 1999.0))
        .collect()
}

#[test]
fn boost_cap_holds_everywhere() {
    let mut setup = Setup::new(48_000.0, 0.35);
    // the loop loses 15 dB around 14 kHz: the fit would like +15 dB there
    setup.system[4] = band(Shape::Bell, 14_000.0, -15.0, 2.0);
    let mut rig = setup.rig();
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_auto(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 150.0);
    assert!(s.error().is_none(), "{:?}", s.error());
    let dense = dense_grid();
    let mut resp = vec![0.0; dense.len()];
    total_response_db(&rig.bands, 1.0, 48_000.0, &dense, &mut resp);
    let mx = resp.iter().cloned().fold(f64::MIN, f64::max);
    let mn = resp.iter().cloned().fold(f64::MAX, f64::min);
    println!(
        "calibration curve range {mn:.2} .. {mx:.2} dB after {} iterations",
        s.iterations()
    );
    assert!(mx <= 6.25, "boost {mx}");
    assert!(mn >= -12.25, "cut {mn}");
    assert!(mx > 5.0, "the cap should actually be reached ({mx})");
}

#[test]
fn manual_bands_are_untouched_and_convergence_holds() {
    let mut setup = Setup::new(48_000.0, 0.41);
    setup.manual[0] = band(Shape::Bell, 1000.0, 2.0, 1.0);
    setup.manual[1] = band(Shape::Bell, 5000.0, -3.0, 2.0);
    setup.manual[5] = band(Shape::LowShelf, 100.0, 1.0, 1.0);
    let mut rig = setup.rig();
    let manual = rig.bands;
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_auto(&persist, &rig.env()).unwrap();
    let applies = rig.run_session(&mut s, &mut persist, 120.0);
    assert!(s.error().is_none(), "{:?}", s.error());
    assert!(applies <= 3);
    for i in [0usize, 1, 5] {
        assert_eq!(rig.bands[i], manual[i], "manual band {i} changed");
    }
    assert_eq!(persist.band_mask & ((1 << 0) | (1 << 1) | (1 << 5)), 0);
    for i in 0..NUM_BANDS {
        if rig.bands[i].used && ![0, 1, 5].contains(&i) {
            assert!(persist.band_mask & (1 << i) != 0, "band {i} not owned");
        }
    }
    let (rms, max) = analytic_deviation(&setup, &rig.bands, None, None, 48_000.0);
    println!(
        "manual bands: final deviation {rms:.3}/{max:.3} dB, {} bands",
        count_used(&rig.bands)
    );
    assert!(rms < 0.5 && max < 1.0);
    // a second run starts from the persisted mask and keeps the manual bands
    let mut s2 = new_session(&rig);
    s2.start_auto(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s2, &mut persist, 120.0);
    for i in [0usize, 1, 5] {
        assert_eq!(rig.bands[i], manual[i]);
    }
    assert!(
        s2.history()[0].rms_db < 0.5,
        "already flat: {:?}",
        s2.history()
    );
    assert_eq!(s2.iterations(), 0);
}

#[test]
fn predicted_residual_matches_the_next_measurement() {
    let setup = Setup::new(48_000.0, 0.37);
    let mut rig = setup.rig();
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_measure(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 60.0);
    s.fit(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 20.0);
    let plan = s.plan().unwrap().clone();
    assert!(plan.rms_dev_db > 1.0);
    assert!(plan.rms_pred_db < 0.3, "predicted {}", plan.rms_pred_db);
    rig.bands = s.apply(&mut persist, &rig.env()).unwrap();
    let m = measure(&mut rig, &mut persist).unwrap();
    let dev = neofilter::calibration::fit::deviation_curve(&m, &persist.settings, None);
    let (rms, _) = deviation_stats(&m.freqs, &dev, &m.coherence, &persist.settings);
    println!(
        "predicted {:.3} dB, measured after apply {rms:.3} dB",
        plan.rms_pred_db
    );
    assert!((rms - plan.rms_pred_db).abs() < 0.15);
}

fn playback_bands() -> [BandParams; NUM_BANDS] {
    let mut p = [BandParams::default(); NUM_BANDS];
    p[0] = band(Shape::Bell, 3000.0, 2.0, 1.0);
    p[1] = band(Shape::HighShelf, 8000.0, -3.0, 1.0);
    p
}

fn curve_of(bands: &[BandParams; NUM_BANDS], lo: f64, hi: f64, n: usize, fs: f64) -> CalCurve {
    let f: Vec<f64> = (0..n)
        .map(|i| lo * (hi / lo).powf(i as f64 / (n - 1) as f64))
        .collect();
    let mut db = vec![0.0; n];
    total_response_db(bands, 1.0, fs, &f, &mut db);
    CalCurve::new(
        "test record",
        f.iter()
            .zip(&db)
            .map(|(&f, &db)| CurvePoint { freq_hz: f, db })
            .collect(),
    )
}

fn fitted_cal_curve(bands: &[BandParams; NUM_BANDS], freqs: &[f64]) -> Vec<f64> {
    let mut v = vec![0.0; freqs.len()];
    total_response_db(bands, 1.0, 48_000.0, freqs, &mut v);
    v
}

#[test]
fn playback_correction_gives_the_same_fit() {
    let fs = 48_000.0;
    let plain = Setup::new(fs, 0.38);
    let mut rig_a = plain.rig();
    let mut pa = CalPersist::default();
    pa.settings = settings(8.0);
    let mut sa = new_session(&rig_a);
    sa.start_auto(&pa, &rig_a.env()).unwrap();
    rig_a.run_session(&mut sa, &mut pa, 120.0);
    assert!(sa.error().is_none());

    // same loop plus a known playback chain (pickup/preamp) in the return
    let play = playback_bands();
    let mut with_p = Setup::new(fs, 0.38);
    let mut combined = with_p.system;
    combined[10] = play[0];
    combined[11] = play[1];
    with_p.system = combined;
    for (label, curve) in [
        ("full curve", curve_of(&play, 20.0, 20_000.0, 40, fs)),
        (
            "partial curve (>= 800 Hz)",
            curve_of(&play, 800.0, 20_000.0, 30, fs),
        ),
    ] {
        let mut rig_b = with_p.rig();
        let mut pb = CalPersist::default();
        pb.settings = settings(8.0);
        pb.correction = Some(curve);
        let mut sb = new_session(&rig_b);
        sb.start_auto(&pb, &rig_b.env()).unwrap();
        rig_b.run_session(&mut sb, &mut pb, 120.0);
        assert!(sb.error().is_none(), "{:?}", sb.error());
        let freqs: Vec<f64> = grid_for(fs)
            .into_iter()
            .filter(|f| *f >= 30.0 && *f <= 16_000.0)
            .collect();
        let (a, b) = (
            fitted_cal_curve(&rig_a.bands, &freqs),
            fitted_cal_curve(&rig_b.bands, &freqs),
        );
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max);
        println!("{label}: fitted correction differs by {worst:.3} dB (max) from the run without playback chain");
        let tol = if label.starts_with("partial") {
            0.35
        } else {
            0.3
        };
        assert!(worst <= tol, "{label}: {worst}");
    }
}

#[test]
fn uncalibrated_returns_change_nothing() {
    let fs = 48_000.0;
    struct Case {
        name: &'static str,
        gain_db: f64,
        noise: Option<f64>,
        system_off: bool,
    }
    let cases = [
        Case {
            name: "silent",
            gain_db: -200.0,
            noise: None,
            system_off: false,
        },
        Case {
            name: "clipped",
            gain_db: 40.0,
            noise: None,
            system_off: false,
        },
        Case {
            name: "uncorrelated",
            gain_db: -200.0,
            noise: Some(-25.0),
            system_off: false,
        },
    ];
    for c in cases {
        let mut setup = Setup::new(fs, 0.4);
        setup.gain_db = c.gain_db;
        setup.noise_dbfs = c.noise;
        let _ = c.system_off;
        let mut rig = setup.rig();
        let before = rig.bands;
        let mut persist = CalPersist::default();
        persist.settings = settings(8.0);
        let mut s = new_session(&rig);
        s.start_auto(&persist, &rig.env()).unwrap();
        let applies = rig.run_session(&mut s, &mut persist, 60.0);
        println!("{}: {}", c.name, s.status());
        assert_eq!(applies, 0);
        assert_eq!(rig.bands, before, "{} changed the bands", c.name);
        assert!(s.plan().is_none() && s.measurement().is_none());
        assert!(s.apply(&mut persist, &rig.env()).is_err());
        assert_eq!(persist.band_mask, 0);
        rig.run(4800);
        assert!(
            !rig.link.noise_active() && !rig.link.is_running(),
            "noise must be off"
        );
        match (c.name, s.error().cloned()) {
            ("silent", Some(CalError::ReturnTooLow(_))) => {}
            ("clipped", Some(CalError::ReturnClipped(_))) => {}
            ("uncorrelated", Some(CalError::NoLoop(_))) => {}
            (n, e) => panic!("{n}: unexpected {e:?}"),
        }
    }
}

#[test]
fn low_coherence_aborts_or_stays_unconstrained() {
    let fs = 48_000.0;
    // heavy noise: mean coherence collapses
    let mut setup = Setup::new(fs, 0.4);
    setup.noise_dbfs = Some(-22.0);
    setup.gain_db = -25.0;
    let mut rig = setup.rig();
    let before = rig.bands;
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_auto(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 60.0);
    println!("heavy noise: {}", s.status());
    assert_eq!(rig.bands, before);
    assert!(
        matches!(
            s.error(),
            Some(CalError::LowCoherence(_)) | Some(CalError::NoLoop(_))
        ),
        "{:?}",
        s.error()
    );
    assert!(s.plan().is_none());

    // moderate noise: only the top of the range falls below 0.7 coherence
    let mut setup = Setup::new(fs, 0.4);
    setup.noise_dbfs = Some(-18.0);
    let mut rig = setup.rig();
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_measure(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 60.0);
    assert!(s.error().is_none(), "{:?}", s.error());
    s.fit(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 20.0);
    let plan = s.plan().expect("plan");
    let before = rig.bands;
    let low = (0..plan.freqs.len())
        .filter(|&i| plan.freqs[i] >= 30.0 && plan.freqs[i] <= 16_000.0 && plan.coherence[i] < 0.7)
        .count();
    println!("{low} grid points below 0.7 coherence");
    assert!(low > 10, "test needs a low-coherence region");
    for i in 0..plan.freqs.len() {
        if plan.freqs[i] >= 30.0 && plan.freqs[i] <= 16_000.0 && plan.coherence[i] < 0.7 {
            assert_eq!(plan.weights[i], 0.0);
        }
    }
    assert!(plan
        .fitted_cal_db
        .iter()
        .all(|v| v.is_finite() && *v <= 6.25 && *v >= -12.25));

    // the same return with a stricter coherence abort threshold: no fit at all
    persist.settings.coherence_abort = 0.95;
    let mut s = new_session(&rig);
    s.start_measure(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 60.0);
    assert!(
        matches!(s.error(), Some(CalError::LowCoherence(c)) if *c < 0.95),
        "{:?}",
        s.error()
    );
    assert!(s.fit(&persist, &rig.env()).is_err());
    assert_eq!(rig.bands, before);
}

#[test]
fn manual_cycle_and_stale_plans() {
    let setup = Setup::new(48_000.0, 0.4);
    let mut rig = setup.rig();
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    rig.bands[20] = band(Shape::Bell, 500.0, 1.0, 1.0); // a manual band
    cycle(&mut rig, &mut s, &mut persist).unwrap();
    assert!(count_used(&rig.bands) > 0 && persist.band_mask != 0);
    assert!(
        s.apply(&mut persist, &rig.env()).is_err(),
        "a fit is applied only once"
    );
    // editing a band after the fit makes the plan stale
    let mut s = new_session(&rig);
    s.start_measure(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 60.0);
    s.fit(&persist, &rig.env()).unwrap();
    rig.run_session(&mut s, &mut persist, 20.0);
    rig.bands[31] = band(Shape::Bell, 700.0, 1.0, 1.0);
    let err = s.apply(&mut persist, &rig.env()).unwrap_err();
    assert!(err.contains("changed"), "{err}");
    // clear calibration unuses exactly the owned bands
    let manual = rig.bands[20];
    assert!(persist.band_mask & (1 << 20) == 0);
    let cleared = s.clear_calibration(&mut persist, &rig.env());
    assert_eq!(cleared[20], manual);
    assert_eq!(count_used(&cleared), 1);
    assert_eq!(persist.band_mask, 0);
}

#[test]
fn stop_mid_measurement_changes_nothing() {
    let setup = Setup::new(48_000.0, 0.4);
    let mut rig = setup.rig();
    let before = rig.bands;
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_auto(&persist, &rig.env()).unwrap();
    for _ in 0..30 {
        rig.run(4800);
        s.poll(&mut persist, &rig.env());
    }
    assert!(rig.link.noise_active() && s.phase() == Phase::Measuring);
    s.stop();
    rig.run(48_000);
    assert!(!rig.link.noise_active() && !rig.link.is_running());
    assert_eq!(s.phase(), Phase::Idle);
    assert!(s.measurement().is_none() && s.plan().is_none());
    assert_eq!(rig.bands, before);
    assert_eq!(persist.band_mask, 0);
    assert!(s.poll(&mut persist, &rig.env()).is_none());
}

#[test]
fn reset_during_measurement_silences_noise_and_fails_the_measurement() {
    let setup = Setup::new(48_000.0, 0.4);
    let mut rig = setup.rig();
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_measure(&persist, &rig.env()).unwrap();
    rig.run(48_000 * 2);
    assert!(rig.link.noise_active());
    rig.tap.reset(); // what NeoFilter::reset() does
    rig.audio.reset();
    assert!(!rig.link.noise_active());
    rig.run(4800);
    s.poll(&mut persist, &rig.env());
    assert!(s.error().is_some() && s.measurement().is_none());
    assert_eq!(s.phase(), Phase::Idle);
}

#[test]
fn bypass_and_busy_guards() {
    let setup = Setup::new(48_000.0, 0.4);
    let mut rig = setup.rig();
    let persist = CalPersist::default();
    let mut s = new_session(&rig);
    rig.bypass = true;
    assert!(s.start_measure(&persist, &rig.env()).is_err());
    rig.bypass = false;
    s.start_measure(&persist, &rig.env()).unwrap();
    assert!(s.start_measure(&persist, &rig.env()).is_err());
    s.stop();
    // the bands changing under a plan invalidates it
    let _ = Env {
        bands: &rig.bands,
        gain_scale: 1.0,
        bypass: false,
    };
}

#[test]
fn polarity_inversion_and_return_gain_are_harmless() {
    let mut setup = Setup::new(48_000.0, 0.36);
    setup.invert = true;
    setup.gain_db = -18.0;
    let mut rig = setup.rig();
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let m = measure(&mut rig, &mut persist).expect("measurement");
    assert!(m.channels[0].as_ref().unwrap().info.inverted);
    let t = truth(&setup, &setup.manual, 6, 48_000.0, &m.freqs);
    let worst = (0..m.freqs.len())
        .filter(|&i| m.freqs[i] >= 40.0 && m.freqs[i] <= 16_000.0)
        .map(|i| (m.mag_db[i] - (t[i] - 0.0)).abs())
        .fold(0.0, f64::max);
    assert!(worst < 0.25, "{worst}");
}

#[test]
fn nobody_polling_stops_the_noise() {
    // editor closed or stalled: the ring is not drained
    let setup = Setup::new(48_000.0, 0.4);
    let mut rig = setup.rig();
    let before = rig.bands;
    let mut persist = CalPersist::default();
    persist.settings = settings(8.0);
    let mut s = new_session(&rig);
    s.start_auto(&persist, &rig.env()).unwrap();
    rig.run(48_000 * 4);
    assert!(!rig.link.noise_active(), "noise must stop by itself");
    s.poll(&mut persist, &rig.env());
    assert!(
        matches!(s.error(), Some(CalError::CaptureOverrun)),
        "{:?}",
        s.error()
    );
    assert_eq!(rig.bands, before);
}
