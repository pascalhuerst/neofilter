//! Coefficient continuity under slow parameter sweeps: the digital response
//! may differ from the analog prototype, but that difference must vary
//! smoothly with the parameter. An isolated spike means the design switched
//! to a different fit candidate for one step, which is heard as zipper noise
//! while automating.
mod common;
use common::*;
use neofilter_dsp::response::{analog_band_response_db, band_response_db};
use neofilter_dsp::{BandParams, Shape, Slope};

#[derive(Clone, Copy, Debug)]
enum Axis {
    Freq,
    Gain,
    Q,
}

fn point(base: &BandParams, axis: Axis, t: f64) -> BandParams {
    let mut p = *base;
    match axis {
        // 30 Hz .. 8 kHz, gain -24..24 dB, Q 0.3 .. 20: the useful range
        Axis::Freq => p.freq_hz = 30.0 * (8000.0f64 / 30.0).powf(t),
        Axis::Gain => p.gain_db = -24.0 + 48.0 * t,
        Axis::Q => p.q = 0.3 * (20.0f64 / 0.3).powf(t),
    }
    p
}

/// Largest spike (dB) of the digital-minus-analog error between frequencies
/// 20 Hz and 0.4 fs, where the analog response is above -40 dB: how far one
/// step's error is from the mean of its neighbours, beyond what the
/// neighbours themselves differ by.
fn worst_spike(base: &BandParams, axis: Axis, fs: f64, steps: usize) -> (f64, f64, f64) {
    let fr = grid(20.0, 0.4 * fs, 80);
    let mut errs: Vec<Vec<f64>> = Vec::new();
    let mut lev: Vec<Vec<f64>> = Vec::new();
    for i in 0..=steps {
        let p = point(base, axis, i as f64 / steps as f64);
        let mut d = vec![0.0; fr.len()];
        let mut a = vec![0.0; fr.len()];
        band_response_db(&p, 1.0, fs, &fr, &mut d);
        analog_band_response_db(&p, 1.0, fs, &fr, &mut a);
        errs.push(d.iter().zip(&a).map(|(x, y)| x - y).collect());
        lev.push(a);
    }
    let mut worst = (0.0, 0.0, 0.0);
    for i in 1..steps {
        for k in 0..fr.len() {
            if lev[i][k] < -40.0 {
                continue;
            }
            let mid = errs[i][k];
            let (lo, hi) = (errs[i - 1][k], errs[i + 1][k]);
            let spike = (mid - 0.5 * (lo + hi)).abs() - 3.0 * (hi - lo).abs();
            if spike > worst.0 {
                worst = (spike, i as f64 / steps as f64, fr[k]);
            }
        }
    }
    worst
}

#[test]
fn response_error_has_no_isolated_jumps_in_the_useful_range() {
    // Measured worst case in this range: 0.105 dB (Bell 48 dB/oct, 11 kHz at
    // 44.1 kHz). 0.25 dB leaves room without hiding a real candidate switch.
    // Known and not covered: Notch / Low Cut at Q < 0.3 and anything beyond
    // 0.4 fs, where single-step spikes of several dB occur (see README).
    let steps = 400;
    for fs in [44100.0, 96000.0] {
        for shape in [
            Shape::Bell,
            Shape::LowShelf,
            Shape::HighShelf,
            Shape::HighCut,
            Shape::TiltShelf,
            Shape::BandPass,
        ] {
            for slope in [Slope::Db12, Slope::Db24, Slope::Db48, Slope::Db96] {
                for (axis, f0, q, g) in [
                    (Axis::Freq, 1000.0, 1.0, 6.0),
                    (Axis::Freq, 1000.0, 5.0, -12.0),
                    (Axis::Q, 1000.0, 1.0, 6.0),
                    (Axis::Q, 6000.0, 1.0, -12.0),
                    (Axis::Gain, 3000.0, 3.0, 6.0),
                ] {
                    let base = band(shape, f0, g, q, slope);
                    let (s, t, f) = worst_spike(&base, axis, fs, steps);
                    assert!(
                        s < 0.25,
                        "fs {fs} {shape:?} {slope:?} {axis:?} (f0 {f0} q {q} g {g}): \
                         {s:.3} dB spike at t={t:.3}, {f:.0} Hz"
                    );
                }
            }
        }
    }
}
