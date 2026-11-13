//! Audio-rate behaviour of the engine under automation: no clicks, no level
//! bursts while a parameter moves.
mod common;
use common::*;
use neofilter_dsp::{BandParams, EqEngine, Shape, Slope};

/// Ratio of the largest second difference of `y` to what a smooth sine of
/// frequency `f` and the local amplitude could produce. A smooth signal gives
/// at most about 1; a click gives much more.
fn click_ratio(y: &[f64], f: f64, fs: f64, skip: usize) -> (f64, usize) {
    let w2 = (2.0 * std::f64::consts::PI * f / fs).powi(2);
    let mut worst = (0.0, 0);
    let win = (fs / f) as usize * 2;
    for i in skip.max(1)..y.len() - 1 {
        let d2 = (y[i + 1] - 2.0 * y[i] + y[i - 1]).abs();
        let lo = i.saturating_sub(win);
        let hi = (i + win).min(y.len());
        let amp = y[lo..hi].iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let r = d2 / (w2 * amp).max(2e-5);
        if r > worst.0 {
            worst = (r, i);
        }
    }
    worst
}

#[derive(Clone, Copy, Debug)]
enum Axis {
    Freq,
    Gain,
    Q,
}

fn run_sweep(
    base: BandParams,
    axis: Axis,
    lo: f64,
    hi: f64,
    secs: f64,
    fs: f64,
    sine_hz: f64,
) -> (f64, usize, f64) {
    let n = (secs * fs) as usize + (0.3 * fs) as usize;
    let x = sine(n, sine_hz, fs, 0.5);
    let mut y = x.clone();
    let mut e = EqEngine::new(fs);
    e.set_band(0, &base);
    let sweep_n = (secs * fs) as usize;
    let block = 64;
    let mut pos = 0;
    let mut peak: f64 = 0.0;
    while pos < n {
        let m = block.min(n - pos);
        let t = (pos as f64 / sweep_n as f64).min(1.0);
        let mut p = base;
        match axis {
            Axis::Freq => p.freq_hz = lo * (hi / lo).powf(t),
            Axis::Gain => p.gain_db = lo + (hi - lo) * t,
            Axis::Q => p.q = lo * (hi / lo).powf(t),
        }
        e.set_band(0, &p);
        e.process_mono(&mut y[pos..pos + m]);
        pos += m;
    }
    for v in &y {
        peak = peak.max(v.abs());
    }
    let (r, at) = click_ratio(&y, sine_hz, fs, 2000);
    (r, at, peak)
}

#[test]
fn flat_tilt_gain_automation_has_no_bursts_or_clicks() {
    // Regression: the whole pivot normalisation used to sit in the first
    // section (a pole pair near 2 Hz with up to +-31 dB of gain). Refreshing
    // its state while the gain moved injected low-frequency garbage: a 100 Hz
    // sine of 0.5 came out at amplitude 16 (+30 dB) during a half second sweep.
    for fs in [44100.0, 96000.0] {
        for secs in [0.15, 0.5, 2.0] {
            let base = band(Shape::FlatTilt, 200.0, 0.0, 1.0, Slope::Db12);
            let (r, at, peak) = run_sweep(base, Axis::Gain, -30.0, 30.0, secs, fs, 100.0);
            // the static gain at 100 Hz never exceeds +3 dB (0.707 peak)
            assert!(peak < 0.8, "fs {fs} {secs}s: burst, peak {peak}");
            assert!(r < 3.0, "fs {fs} {secs}s: click ratio {r} at {at}");
        }
    }
}

#[test]
fn flat_tilt_pivot_automation_has_no_bursts_or_clicks() {
    let base = band(Shape::FlatTilt, 200.0, 20.0, 1.0, Slope::Db12);
    for secs in [0.15, 1.0] {
        let (r, at, peak) = run_sweep(base, Axis::Freq, 30.0, 2000.0, secs, 48000.0, 100.0);
        assert!(peak < 1.0, "{secs}s: burst, peak {peak}");
        // the pivot moving across the signal modulates its level by 13 dB; the
        // remaining ratio (up to ~6) is a step of about -78 dBFS
        assert!(r < 8.0, "{secs}s: click ratio {r} at {at}");
    }
}

#[test]
fn bell_automation_has_no_clicks() {
    for fs in [44100.0, 96000.0] {
        for slope in [Slope::Db12, Slope::Db24, Slope::Db48] {
            for (axis, lo, hi, secs) in [
                (Axis::Freq, 30.0, 2000.0, 0.15),
                (Axis::Gain, -30.0, 30.0, 0.15),
                (Axis::Q, 0.2, 30.0, 0.15),
            ] {
                let base = band(Shape::Bell, 300.0, 12.0, 4.0, slope);
                let (r, at, peak) = run_sweep(base, axis, lo, hi, secs, fs, 100.0);
                assert!(
                    r < 3.0,
                    "fs {fs} {slope:?} {axis:?}: click ratio {r} at {at}"
                );
                assert!(peak < 20.0, "fs {fs} {slope:?} {axis:?}: peak {peak}");
            }
        }
    }
}
