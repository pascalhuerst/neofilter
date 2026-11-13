//! The digital responses against textbook formulas written down independently
//! of the crate's own analog model.
mod common;
use common::*;
use neofilter_dsp::response::band_response_db;
use neofilter_dsp::{Shape, Slope};
use std::f64::consts::PI;

fn resp(p: &neofilter_dsp::BandParams, fs: f64, f: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; f.len()];
    band_response_db(p, 1.0, fs, f, &mut out);
    out
}

/// RBJ cookbook peaking EQ, magnitude in dB.
fn rbj_peaking_db(f0: f64, gain_db: f64, q_std: f64, fs: f64, f: f64) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * f0 / fs;
    let alpha = w0.sin() / (2.0 * q_std);
    let (b0, b1, b2) = (1.0 + alpha * a, -2.0 * w0.cos(), 1.0 - alpha * a);
    let (a0, a1, a2) = (1.0 + alpha / a, -2.0 * w0.cos(), 1.0 - alpha / a);
    let w = 2.0 * PI * f / fs;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let n = ((b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2)).sqrt();
    let d = ((a0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2)).sqrt();
    20.0 * (n / d).log10()
}

fn analog_peaking_db(f0: f64, gain_db: f64, q_std: f64, f: f64) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    let x = f / f0;
    // (s^2 + s A/Q + 1) / (s^2 + s /(A Q) + 1), s = j x
    let re = 1.0 - x * x;
    let n = (re * re + (x * a / q_std).powi(2)).sqrt();
    let d = (re * re + (x / (a * q_std)).powi(2)).sqrt();
    20.0 * (n / d).log10()
}

#[test]
fn twelve_db_bell_matches_rbj_with_q_over_sqrt2() {
    for fs in [48000.0, 96000.0] {
        for &(f0, g, q) in &[
            (1000.0, 6.0, 1.0),
            (200.0, -9.0, 2.0),
            (3000.0, 12.0, 0.5),
            (100.0, 4.0, 8.0),
            (5000.0, -3.0, 4.0),
        ] {
            let p = band(Shape::Bell, f0, g, q, Slope::Db12);
            let freqs = grid(20.0, 0.2 * fs, 200);
            let got = resp(&p, fs, &freqs);
            for (f, d) in freqs.iter().zip(&got) {
                let qs = q * std::f64::consts::FRAC_1_SQRT_2;
                // the analog textbook curve, which the matched design aims at
                let want = analog_peaking_db(f0, g, qs, *f);
                assert!(
                    // Was 0.12 with the matched design. The reference rule
                    // deviates a little more from the analog curve towards
                    // 0.2 fs for wide bells: measured worst 0.148 dB (3 kHz,
                    // +12 dB, Q 0.5 at 48 kHz, at 9.6 kHz); every other case
                    // here is below 0.02 dB.
                    (d - want).abs() < 0.16,
                    "fs {fs} f0 {f0} g {g} q {q}: at {f:.0} Hz got {d:.3} analog {want:.3}"
                );
                // and the bilinear RBJ filter, which cramps a little near fs/5
                let rbj = rbj_peaking_db(f0, g, qs, fs, *f);
                assert!(
                    *f > 0.1 * fs || (d - rbj).abs() < 0.3,
                    "fs {fs} f0 {f0} g {g} q {q}: at {f:.0} Hz got {d:.3} RBJ {rbj:.3}"
                );
            }
        }
    }
}

fn mag_db_butter_hp(f0: f64, order: i32, f: f64) -> f64 {
    -10.0 * (1.0 + (f0 / f).powi(2 * order)).log10()
}

#[test]
fn butterworth_low_cut_points() {
    let fs = 48000.0;
    let p = band(Shape::LowCut, 1000.0, 0.0, 1.0, Slope::Db24);
    let r = resp(&p, fs, &[1000.0, 500.0, 250.0, 2000.0, 8000.0]);
    assert!((r[0] + 3.0103).abs() < 0.02, "-3 dB point {}", r[0]);
    assert!(
        (r[1] - mag_db_butter_hp(1000.0, 4, 500.0)).abs() < 0.05,
        "{}",
        r[1]
    );
    assert!(
        (r[1] + 24.1).abs() < 0.1,
        "-24 dB an octave below: {}",
        r[1]
    );
    assert!(
        (r[2] - mag_db_butter_hp(1000.0, 4, 250.0)).abs() < 0.1,
        "{}",
        r[2]
    );
    assert!(
        (r[3] - mag_db_butter_hp(1000.0, 4, 2000.0)).abs() < 0.05,
        "{}",
        r[3]
    );
    assert!(r[4].abs() < 0.05, "passband {}", r[4]);
}

#[test]
fn butterworth_high_cut_points() {
    let fs = 96000.0;
    for (slope, order) in [(Slope::Db12, 2), (Slope::Db24, 4), (Slope::Db48, 8)] {
        let p = band(Shape::HighCut, 2000.0, 0.0, 1.0, slope);
        let r = resp(&p, fs, &[2000.0, 4000.0, 500.0]);
        assert!(
            (r[0] + 3.0103).abs() < 0.03,
            "{slope:?} -3 dB point {}",
            r[0]
        );
        let want = -10.0 * (1.0 + 2f64.powi(2 * order)).log10();
        assert!(
            (r[1] - want).abs() < 0.2,
            "{slope:?} octave above {} vs {want}",
            r[1]
        );
        assert!(r[2].abs() < 0.05);
    }
}

#[test]
fn shelves_reach_their_gain_and_midpoint() {
    let fs = 48000.0;
    for slope in [Slope::Db12, Slope::Db24] {
        let hs = band(Shape::HighShelf, 1000.0, 9.0, 1.0, slope);
        let r = resp(&hs, fs, &[20.0, 1000.0, 9000.0]);
        assert!(r[0].abs() < 0.05, "{slope:?} high shelf lows {}", r[0]);
        assert!((r[1] - 4.5).abs() < 0.1, "{slope:?} midpoint {}", r[1]);
        assert!((r[2] - 9.0).abs() < 0.3, "{slope:?} highs {}", r[2]);
        let ls = band(Shape::LowShelf, 1000.0, -9.0, 1.0, slope);
        let r = resp(&ls, fs, &[20.0, 1000.0, 9000.0]);
        assert!(
            (r[0] + 9.0).abs() < 0.05,
            "{slope:?} low shelf lows {}",
            r[0]
        );
        assert!((r[1] + 4.5).abs() < 0.1, "{slope:?} midpoint {}", r[1]);
        assert!(r[2].abs() < 0.3, "{slope:?} highs {}", r[2]);
    }
}

#[test]
fn tilt_shelf_is_antisymmetric_about_the_pivot() {
    let fs = 96000.0;
    for slope in [Slope::Db6, Slope::Db12, Slope::Db24] {
        for g in [6.0, -12.0] {
            let p = band(Shape::TiltShelf, 1000.0, g, 1.0, slope);
            let r0 = resp(&p, fs, &[1000.0])[0];
            assert!(r0.abs() < 0.05, "pivot {r0}");
            for k in [1.5, 2.0, 4.0, 8.0] {
                let r = resp(&p, fs, &[1000.0 * k, 1000.0 / k]);
                assert!(
                    (r[0] + r[1]).abs() < 0.08,
                    "{slope:?} g {g} k {k}: {} {}",
                    r[0],
                    r[1]
                );
            }
            let r = resp(&p, fs, &[10.0, 20000.0]);
            assert!((r[0] + g).abs() < 0.3 && (r[1] - g).abs() < 0.6, "{r:?}");
        }
    }
}

#[test]
fn zero_gain_is_exactly_flat_and_boost_cut_are_inverse() {
    let fs = 48000.0;
    let freqs = grid(20.0, 20000.0, 100);
    for shape in [
        Shape::Bell,
        Shape::LowShelf,
        Shape::HighShelf,
        Shape::TiltShelf,
        Shape::FlatTilt,
    ] {
        let p = band(shape, 1000.0, 0.0, 1.0, Slope::Db24);
        assert!(
            resp(&p, fs, &freqs).iter().all(|v| *v == 0.0),
            "{shape:?} not exactly flat at 0 dB"
        );
        for g in [0.001, 0.5, 6.0, 20.0] {
            let up = resp(&band(shape, 1000.0, g, 1.0, Slope::Db24), fs, &freqs);
            let dn = resp(&band(shape, 1000.0, -g, 1.0, Slope::Db24), fs, &freqs);
            for ((f, a), b) in freqs.iter().zip(&up).zip(&dn) {
                // not Nyquist-adjacent: the matched fit is not exactly inverse there
                if *f < 10000.0 {
                    assert!(
                        (a + b).abs() < 0.05 + 0.01 * g,
                        "{shape:?} g {g} at {f:.0}: {a} {b}"
                    );
                }
            }
        }
    }
}
