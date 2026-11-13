mod common;
use common::*;
use neofilter_dsp::{BandParams, EqEngine, Placement, Shape, Slope};

const FS: f64 = 48000.0;

fn stereo_noise(n: usize, seed: u64) -> (Vec<f64>, Vec<f64>) {
    let mut ns = Noise(seed);
    let mut l = vec![0.0; n];
    let mut r = vec![0.0; n];
    ns.fill(&mut l);
    ns.fill(&mut r);
    (l, r)
}

fn max_diff(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f64::max)
}

#[test]
fn null_test_zero_gain_is_bit_transparent() {
    let (l0, r0) = stereo_noise(5000, 1);
    let mut e = EqEngine::new(FS);
    for (i, shape) in [
        Shape::Bell,
        Shape::LowShelf,
        Shape::HighShelf,
        Shape::TiltShelf,
        Shape::FlatTilt,
    ]
    .iter()
    .enumerate()
    {
        e.set_band(
            i,
            &band(*shape, 500.0 * (i + 1) as f64, 0.0, 1.5, Slope::Db24),
        );
    }
    let (mut l, mut r) = (l0.clone(), r0.clone());
    e.process_stereo(&mut l, &mut r);
    assert!(max_diff(&l, &l0) <= 1e-12 && max_diff(&r, &r0) <= 1e-12);

    // gain scale 0 makes every gain band flat as well
    let mut e = EqEngine::new(FS);
    e.set_gain_scale(0.0);
    for i in 0..5 {
        e.set_band(i, &bell(1000.0 * (i + 1) as f64, 12.0, 2.0));
    }
    let (mut l, mut r) = (l0.clone(), r0.clone());
    e.process_stereo(&mut l, &mut r);
    assert!(max_diff(&l, &l0) <= 1e-12 && max_diff(&r, &r0) <= 1e-12);
}

#[test]
fn unused_and_disabled_bands_do_nothing() {
    let (l0, r0) = stereo_noise(2000, 2);
    let mut e = EqEngine::new(FS);
    let mut p = bell(1000.0, 30.0, 4.0);
    p.used = false;
    e.set_band(0, &p);
    p.used = true;
    p.enabled = false;
    e.set_band(1, &p);
    let (mut l, mut r) = (l0.clone(), r0.clone());
    e.process_stereo(&mut l, &mut r);
    assert_eq!((l, r), (l0, r0));
}

#[test]
fn out_of_range_index_and_garbage_params_are_safe() {
    let mut e = EqEngine::new(FS);
    e.set_band(24, &bell(1000.0, 6.0, 1.0));
    e.set_band(usize::MAX, &bell(1000.0, 6.0, 1.0));
    let mut p = bell(f64::NAN, f64::INFINITY, f64::NAN);
    e.set_band(0, &p);
    p.freq_hz = -5.0;
    p.gain_db = f64::NEG_INFINITY;
    p.q = 1e9;
    e.set_band(1, &p);
    e.set_gain_scale(f64::NAN);
    let (mut l, mut r) = stereo_noise(3000, 3);
    e.process_stereo(&mut l, &mut r);
    assert!(l.iter().chain(&r).all(|x| x.is_finite()));
}

#[test]
fn mid_side_placement_semantics() {
    let n = 12000;
    let x = sine(n, 1000.0, FS, 0.3);
    let zero = vec![0.0; n];
    let run = |placement: Placement, l: &[f64], r: &[f64]| {
        let mut e = EqEngine::new(FS);
        let mut p = bell(1000.0, 12.0, 2.0);
        p.placement = placement;
        e.set_band(0, &p);
        let (mut l, mut r) = (l.to_vec(), r.to_vec());
        e.process_stereo(&mut l, &mut r);
        (l, r)
    };
    // pure mid signal (L = R)
    let (ml, mr) = run(Placement::Mid, &x, &x);
    let (sl, sr) = run(Placement::Stereo, &x, &x);
    assert!(max_diff(&ml, &sl) < 1e-12 && max_diff(&mr, &sr) < 1e-12);
    // a Side band leaves pure mid untouched
    let (l, r) = run(Placement::Side, &x, &x);
    assert!(max_diff(&l, &x) < 1e-12 && max_diff(&r, &x) < 1e-12);
    // pure side signal (L = -R)
    let neg: Vec<f64> = x.iter().map(|v| -v).collect();
    let (l, r) = run(Placement::Side, &x, &neg);
    let (el, _) = run(Placement::Stereo, &x, &zero);
    assert!(max_diff(&l, &el) < 1e-12);
    assert!(max_diff(&r, &l.iter().map(|v| -v).collect::<Vec<_>>()) < 1e-12);
    // a Mid band leaves pure side untouched
    let (l, r) = run(Placement::Mid, &x, &neg);
    assert!(max_diff(&l, &x) < 1e-12 && max_diff(&r, &neg) < 1e-12);
    // Left / Right only touch their channel
    let (l, r) = run(Placement::Left, &x, &x);
    assert!(max_diff(&l, &sl) < 1e-12 && max_diff(&r, &x) < 1e-12);
    let (l, r) = run(Placement::Right, &x, &x);
    assert!(max_diff(&l, &x) < 1e-12 && max_diff(&r, &sr) < 1e-12);
}

#[test]
fn mid_side_reconstruction_is_exact_for_flat_but_active_bands() {
    // Gain far below audibility but above the "flat" shortcut (1e-9 dB): the
    // filters run, M/S must reconstruct L/R to the (1e-10) filter deviation.
    let (l0, r0) = stereo_noise(6000, 4);
    for placement in [Placement::Mid, Placement::Side] {
        let mut e = EqEngine::new(FS);
        for i in 0..4 {
            let mut p = bell(300.0 * 3f64.powi(i), 1.5e-9, 1.0 + i as f64);
            p.placement = placement;
            e.set_band(i as usize, &p);
        }
        let (mut l, mut r) = (l0.clone(), r0.clone());
        e.process_stereo(&mut l, &mut r);
        assert!(max_diff(&l, &l0) < 1e-8, "{placement:?}");
        assert!(max_diff(&r, &r0) < 1e-8, "{placement:?}");
    }
}

fn busy_engine(fs: f64) -> EqEngine {
    let mut e = EqEngine::new(fs);
    let mut i = 0;
    let mut add = |e: &mut EqEngine, p: BandParams| {
        e.set_band(i, &p);
        i += 1;
    };
    add(&mut e, band(Shape::LowCut, 30.0, 0.0, 1.0, Slope::Db24));
    add(&mut e, bell(120.0, -4.0, 2.0));
    add(&mut e, {
        let mut p = bell(2500.0, 6.0, 3.0);
        p.placement = Placement::Mid;
        p
    });
    add(&mut e, {
        let mut p = band(Shape::HighShelf, 6000.0, 3.0, 1.0, Slope::Db18);
        p.placement = Placement::Side;
        p
    });
    add(&mut e, {
        let mut p = band(Shape::Notch, 1000.0, 0.0, 4.0, Slope::Db12);
        p.placement = Placement::Left;
        p
    });
    add(&mut e, band(Shape::HighCut, 15000.0, 0.0, 1.0, Slope::Db48));
    e
}

#[test]
fn block_partitioning_does_not_change_the_result() {
    let n = 7000;
    let (l0, r0) = stereo_noise(n, 5);
    let reference = {
        let mut e = busy_engine(FS);
        let (mut l, mut r) = (l0.clone(), r0.clone());
        e.process_stereo(&mut l, &mut r);
        (l, r)
    };
    let patterns: [&[usize]; 4] = [&[1], &[7, 0, 33, 1, 64, 5], &[32], &[1000, 0, 0, 3, 513]];
    for pat in patterns {
        let mut e = busy_engine(FS);
        let (mut l, mut r) = (l0.clone(), r0.clone());
        let mut pos = 0;
        let mut k = 0;
        while pos < n {
            let len = pat[k % pat.len()].min(n - pos);
            e.process_stereo(&mut l[pos..pos + len], &mut r[pos..pos + len]);
            pos += len;
            k += 1;
        }
        let d = max_diff(&l, &reference.0).max(max_diff(&r, &reference.1));
        assert!(d < 1e-12, "partition {pat:?}: {d}");
    }
    // parameters changing while smoothing, applied at a block start
    let moving = |chunks: &[usize]| {
        let mut e = busy_engine(FS);
        let (mut l, mut r) = (l0.clone(), r0.clone());
        let mut p = bell(5000.0, 9.0, 1.0);
        p.placement = Placement::Stereo;
        e.process_stereo(&mut l[..320], &mut r[..320]);
        e.set_band(1, &p);
        let mut pos = 320;
        let mut k = 0;
        while pos < n {
            let len = chunks[k % chunks.len()].min(n - pos);
            e.process_stereo(&mut l[pos..pos + len], &mut r[pos..pos + len]);
            pos += len;
            k += 1;
        }
        (l, r)
    };
    let a = moving(&[6680]);
    let b = moving(&[1, 31, 0, 97]);
    assert!(max_diff(&a.0, &b.0) < 1e-12 && max_diff(&a.1, &b.1) < 1e-12);
}

#[test]
fn mono_matches_left_channel_and_ignores_placement() {
    let n = 4000;
    let (l0, r0) = stereo_noise(n, 6);
    let mut e = EqEngine::new(FS);
    // all bands as Stereo for the comparison
    let mut e2 = EqEngine::new(FS);
    for i in 0..6 {
        let mut p = bell(200.0 * (i + 1) as f64, 3.0, 1.0);
        p.placement = match i % 5 {
            0 => Placement::Stereo,
            1 => Placement::Mid,
            2 => Placement::Side,
            3 => Placement::Left,
            _ => Placement::Right,
        };
        e2.set_band(i, &p);
        p.placement = Placement::Stereo;
        e.set_band(i, &p);
    }
    let mut mono = l0.clone();
    e2.process_mono(&mut mono);
    let (mut l, mut r) = (l0.clone(), r0.clone());
    e.process_stereo(&mut l, &mut r);
    assert!(max_diff(&mono, &l) < 1e-12);
}

/// Max sample-to-sample jump of `y` in [from, to) must not exceed `factor`
/// times the larger steady-state jump measured before and after.
fn assert_smooth(y: &[f64], change_at: usize, settle: usize, factor: f64, what: &str) {
    let before = max_jump(y, change_at.saturating_sub(2000), change_at);
    let after = max_jump(y, y.len() - 2000, y.len());
    let during = max_jump(y, change_at, y.len() - 2000);
    let bound = factor * before.max(after);
    assert!(
        during <= bound,
        "{what}: jump {during:.5} > {bound:.5} (before {before:.5}, after {after:.5})"
    );
    let _ = settle;
}

fn run_with_change<F: Fn(&mut EqEngine), G: Fn(&mut EqEngine)>(
    setup: F,
    change: G,
    x: &[f64],
    at: usize,
) -> Vec<f64> {
    let mut e = EqEngine::new(FS);
    setup(&mut e);
    let mut y = x.to_vec();
    let (a, b) = y.split_at_mut(at);
    e.process_mono(a);
    change(&mut e);
    e.process_mono(b);
    y
}

#[test]
fn parameter_steps_do_not_click() {
    let n = 48000;
    let at = 12000;
    let x = sine(n, 1000.0, FS, 0.4);
    let x12 = sine(n, 12000.0, FS, 0.4);
    let cases: Vec<(&str, BandParams, BandParams)> = vec![
        (
            "gain 0 -> +12",
            bell(1000.0, 0.0, 1.0),
            bell(1000.0, 12.0, 1.0),
        ),
        (
            "gain +12 -> -12",
            bell(1000.0, 12.0, 1.0),
            bell(1000.0, -12.0, 1.0),
        ),
        // sweeps across the tone would be audible amplitude changes, not clicks
        (
            "freq 200 -> 3000 (12 kHz tone)",
            bell(200.0, 12.0, 3.0),
            bell(3000.0, 12.0, 3.0),
        ),
        (
            "q 0.3 -> 30",
            bell(1000.0, 9.0, 0.3),
            bell(1000.0, 9.0, 30.0),
        ),
        (
            "lowcut 20 -> 4000",
            band(Shape::LowCut, 20.0, 0.0, 1.0, Slope::Db24),
            band(Shape::LowCut, 4000.0, 0.0, 1.0, Slope::Db24),
        ),
        (
            "highcut 18k -> 300",
            band(Shape::HighCut, 18000.0, 0.0, 1.0, Slope::Db48),
            band(Shape::HighCut, 300.0, 0.0, 1.0, Slope::Db48),
        ),
        (
            "shelf gain",
            band(Shape::HighShelf, 800.0, -6.0, 1.0, Slope::Db12),
            band(Shape::HighShelf, 800.0, 18.0, 1.0, Slope::Db12),
        ),
    ];
    for (name, a, b) in cases {
        let y = run_with_change(
            |e| {
                e.set_band(0, &a);
                e.process_mono(&mut []);
            },
            |e| e.set_band(0, &b),
            if name.contains("12 kHz") { &x12 } else { &x },
            at,
        );
        assert!(y.iter().all(|v| v.is_finite()));
        assert_smooth(&y, at, 4000, 1.3, name);
    }
}

#[test]
fn toggles_and_topology_changes_do_not_click() {
    let n = 48000;
    let at = 12000;
    let x = sine(n, 1000.0, FS, 0.4);
    let base = bell(1000.0, 15.0, 2.0);
    let mut off_used = base;
    off_used.used = false;
    let mut off_enabled = base;
    off_enabled.enabled = false;
    let mut shelf = base;
    shelf.shape = Shape::HighShelf;
    let mut steep = base;
    steep.slope = Slope::Db48;
    let mut cut = base;
    cut.shape = Shape::HighCut;
    cut.freq_hz = 700.0;
    let cases: Vec<(&str, BandParams, BandParams)> = vec![
        ("enabled -> disabled", base, off_enabled),
        ("disabled -> enabled", off_enabled, base),
        ("used -> unused", base, off_used),
        ("unused -> used", off_used, base),
        ("bell -> shelf", base, shelf),
        ("12 -> 48 dB/oct", base, steep),
        ("bell -> high cut", base, cut),
    ];
    for (name, a, b) in cases {
        let y = run_with_change(|e| e.set_band(0, &a), |e| e.set_band(0, &b), &x, at);
        assert!(y.iter().all(|v| v.is_finite()));
        assert_smooth(&y, at, 4000, 1.3, name);
    }
}

#[test]
fn placement_changes_do_not_click() {
    let n = 48000;
    let at = 12000;
    let x = sine(n, 1000.0, FS, 0.4);
    let y2: Vec<f64> = sine(n, 1000.0, FS, 0.1);
    for (from, to) in [
        (Placement::Stereo, Placement::Left),
        (Placement::Left, Placement::Right),
        (Placement::Stereo, Placement::Mid),
        (Placement::Mid, Placement::Side),
    ] {
        let mut a = bell(1000.0, 12.0, 2.0);
        a.placement = from;
        let mut b = a;
        b.placement = to;
        let mut e = EqEngine::new(FS);
        e.set_band(0, &a);
        let (mut l, mut r) = (x.clone(), y2.clone());
        e.process_stereo(&mut l[..at], &mut r[..at]);
        e.set_band(0, &b);
        e.process_stereo(&mut l[at..], &mut r[at..]);
        for (ch, y) in [("L", &l), ("R", &r)] {
            let before = max_jump(y, at - 2000, at);
            let after = max_jump(y, n - 2000, n);
            let during = max_jump(y, at, n - 2000);
            // channel amplitudes can legitimately differ a lot; just no spike
            let bound = 1.3 * before.max(after);
            assert!(during <= bound, "{from:?}->{to:?} {ch}: {during} > {bound}");
        }
    }
}

#[test]
fn first_set_band_snaps_without_smoothing() {
    let n = 2400; // 50 ms
    let x = sine(n, 1000.0, FS, 0.3);
    let p = bell(1000.0, 18.0, 1.0);
    let amp = |y: &[f64]| y[480..960].iter().fold(0.0f64, |m, v| m.max(v.abs()));

    // snapped: after new(), the very first set_band
    let mut e = EqEngine::new(FS);
    e.set_band(0, &p);
    let mut y = x.clone();
    e.process_mono(&mut y);
    let snapped = amp(&y);

    // smoothed: change from flat afterwards
    let mut e = EqEngine::new(FS);
    e.set_band(0, &bell(1000.0, 0.0, 1.0));
    e.process_mono(&mut [0.0; 64]);
    e.set_band(0, &p);
    let mut y = x.clone();
    e.process_mono(&mut y);
    let smoothed = amp(&y);
    let target = 0.3 * 10f64.powf(18.0 / 20.0);
    assert!(snapped > 0.9 * target, "snapped {snapped} target {target}");
    assert!(
        smoothed < 0.9 * snapped,
        "smoothed {smoothed} snapped {snapped}"
    );

    // reset() and set_sample_rate() re-arm the snap
    for rearm in 0..2 {
        let mut e = EqEngine::new(FS);
        e.set_band(0, &bell(1000.0, 0.0, 1.0));
        e.process_mono(&mut [0.0; 64]);
        if rearm == 0 {
            e.reset();
        } else {
            e.set_sample_rate(FS);
        }
        e.set_band(0, &p);
        let mut y = x.clone();
        e.process_mono(&mut y);
        assert!(amp(&y) > 0.9 * target, "rearm {rearm}");
    }
}

#[test]
fn settled_engine_matches_static_response() {
    // after smoothing, the engine's steady-state gain equals the design
    let fs = 96000.0;
    let mut e = EqEngine::new(fs);
    e.set_band(0, &bell(2000.0, 0.0, 1.0));
    e.process_mono(&mut [0.0; 32]);
    let p = bell(2000.0, 9.0, 2.0);
    e.set_band(0, &p);
    let x = sine(96000, 2000.0, fs, 0.2);
    let mut y = x.clone();
    e.process_mono(&mut y);
    let a = y[80000..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let mut want = [0.0];
    neofilter_dsp::response::band_response_db(&p, 1.0, fs, &[2000.0], &mut want);
    let got = 20.0 * (a / 0.2).log10();
    assert!((got - want[0]).abs() < 0.01, "{got} vs {}", want[0]);
}

#[test]
fn long_silence_stays_finite_and_decays() {
    let fs = 48000.0;
    let mut e = EqEngine::new(fs);
    e.set_band(0, &band(Shape::LowCut, 10.0, 0.0, 40.0, Slope::Db24));
    e.set_band(1, &bell(10.0, 30.0, 40.0));
    e.set_band(2, &bell(40.0, -30.0, 40.0));
    e.set_band(3, &band(Shape::HighShelf, 12000.0, 30.0, 10.0, Slope::Db96));
    e.set_band(4, &band(Shape::FlatTilt, 1000.0, 30.0, 1.0, Slope::Db12));
    let mut l = vec![0.0; 4800];
    let mut r = vec![0.0; 4800];
    Noise(7).fill(&mut l);
    Noise(8).fill(&mut r);
    e.process_stereo(&mut l, &mut r);
    let mut worst_tail = 0.0f64;
    for _ in 0..(60 * 10) {
        let mut l = vec![0.0; 4800];
        let mut r = vec![0.0; 4800];
        e.process_stereo(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
        assert!(l
            .iter()
            .chain(&r)
            .all(|v| *v == 0.0 || v.abs() >= f64::MIN_POSITIVE));
        worst_tail = l.iter().chain(&r).fold(0.0f64, |m, v| m.max(v.abs()));
    }
    assert!(worst_tail < 1e-6, "tail after 60 s: {worst_tail}");
}
