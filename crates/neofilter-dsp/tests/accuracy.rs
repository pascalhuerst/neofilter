//! Matched digital filters versus the analytic analog prototypes.
mod common;
use common::*;
use neofilter_dsp::response::{analog_band_response_db, band_response_db};
use neofilter_dsp::{BandParams, Shape, Slope};
use std::collections::BTreeMap;

const RATES: [f64; 5] = [44100.0, 48000.0, 88200.0, 96000.0, 192000.0];
/// Deviations are only judged where the analog response is above this (dB);
/// deeper stop band tails are inaudible and make relative errors meaningless.
const MASK_DB: f64 = -30.0;

/// (max |digital - analog| in dB over 10 Hz..0.4 fs, same over 0.4..0.45 fs)
fn deviation(p: &BandParams, fs: f64) -> (f64, f64) {
    let lo = grid(10.0, 0.4 * fs, 240);
    let hi = grid(0.4 * fs, 0.45 * fs, 24);
    let dev = |fr: &[f64]| {
        let mut a = vec![0.0; fr.len()];
        let mut d = vec![0.0; fr.len()];
        analog_band_response_db(p, 1.0, fs, fr, &mut a);
        band_response_db(p, 1.0, fs, fr, &mut d);
        a.iter()
            .zip(&d)
            .filter(|(a, _)| **a >= MASK_DB)
            .map(|(a, d)| (a - d).abs())
            .fold(0.0, f64::max)
    };
    (dev(&lo), dev(&hi))
}

fn bucket(f: f64, fs: f64) -> usize {
    let r = f / fs;
    [0.02, 0.05, 0.1, 0.2, 0.3, 1.0]
        .iter()
        .position(|&b| r <= b)
        .unwrap()
}
const BUCKETS: [&str; 6] = ["<=.02fs", "<=.05fs", "<=.1fs", "<=.2fs", "<=.3fs", ">.3fs"];

/// PLAN.md accuracy targets: 0.1 dB up to 0.4 fs, 0.5 dB up to 0.45 fs.
const PLAN_LO: f64 = 0.1;
const PLAN_HI: f64 = 0.5;

// Regression tolerances (dB) per shape (order of `Shape::ALL`) and band
// frequency bucket relative to fs: (10 Hz..0.4 fs, 0.4..0.45 fs).
//
// THESE ARE LOOSER THAN THE PLAN.md TARGETS (0.1 / 0.5 dB) in the upper
// buckets, deliberately and visibly: they are the measured worst case over
// the whole grid below (all slopes, gains -15/+6/+18 dB, Q 0.5/1/3/10, five
// sample rates) times 1.25, rounded up. A biquad with exact-ish poles cannot
// follow an analog response whose resonances/zeros sit at a large fraction of
// the sample rate, and Q 10 shelves, 96 dB/oct band passes and wide notches
// are the extreme corners. The `core` print-out of the grid test shows the
// everyday domain (Q 1..3, |gain| <= 6 dB, <= 24 dB/oct); see its numbers in
// the crate docs of `matched`. Do not raise an entry to make a failing test
// pass: find out why the design got worse.
const TOL: [[(f64, f64); 6]; 9] = [
    // Bell
    [
        (0.03, 0.05),
        (0.2, 0.4),
        (0.25, 0.5),
        (0.4, 0.55),
        (0.3, 0.55),
        (0.6, 1.5),
    ],
    // Low Shelf
    [
        (0.009, 0.015),
        (0.08, 0.15),
        (0.35, 0.45),
        (1.5, 2.),
        (1.5, 2.5),
        (2.5, 15.),
    ],
    // Low Cut
    [
        (0.005, 0.007),
        (0.07, 0.095),
        (0.25, 0.4),
        (0.9, 1.5),
        (0.6, 0.9),
        (2.5, 3.),
    ],
    // High Shelf
    [
        (0.01, 0.02),
        (0.08, 0.15),
        (0.35, 0.45),
        (1.5, 2.),
        (1.5, 2.5),
        (2., 15.),
    ],
    // High Cut
    [
        (0.35, 0.01),
        (0.35, 0.6),
        (0.75, 0.8),
        (1.5, 2.),
        (1.5, 2.),
        (2.5, 3.5),
    ],
    // Notch
    [
        (0.007, 0.01),
        (0.15, 0.2),
        (0.65, 0.75),
        (4.5, 6.5),
        (4.5, 8.),
        (4.5, 8.5),
    ],
    // Band Pass
    [
        (0.4, 0.55),
        (1.5, 0.7),
        (2., 2.),
        (2.5, 2.5),
        (2.5, 2.5),
        (5.5, 5.),
    ],
    // Tilt Shelf
    [
        (0.01, 0.02),
        (0.08, 0.15),
        (0.35, 0.45),
        (1.5, 2.),
        (1.5, 2.5),
        (2., 15.),
    ],
    // Flat Tilt
    [
        (0.02, 0.025),
        (0.02, 0.025),
        (0.02, 0.025),
        (0.015, 0.02),
        (0.015, 0.015),
        (0.015, 0.015),
    ],
];

#[test]
fn accuracy_grid() {
    let freqs = [30.0, 200.0, 1000.0, 4000.0, 9000.0, 16000.0];
    let gains = [-15.0, 6.0, 18.0];
    let qs = [0.5, 1.0, 3.0, 10.0];
    // worst[(shape, bucket)] = (lo, hi, description of lo, description of hi)
    let mut worst: BTreeMap<(usize, usize), (f64, f64, String, String)> = BTreeMap::new();
    let mut core: BTreeMap<(usize, usize), (f64, f64)> = BTreeMap::new();
    let (mut total, mut meets_plan, mut core_total, mut core_meets) =
        (0usize, 0usize, 0usize, 0usize);
    for (si, shape) in Shape::ALL.iter().enumerate() {
        for slope in Slope::ALL {
            for &fs in &RATES {
                for &f in &freqs {
                    if f > 0.4 * fs {
                        continue;
                    }
                    let uses_gain = matches!(
                        shape,
                        Shape::Bell
                            | Shape::LowShelf
                            | Shape::HighShelf
                            | Shape::TiltShelf
                            | Shape::FlatTilt
                    );
                    let gl: &[f64] = if uses_gain { &gains } else { &[0.0] };
                    let ql: &[f64] = if *shape == Shape::FlatTilt {
                        &[1.0]
                    } else {
                        &qs
                    };
                    if *shape == Shape::FlatTilt && slope != Slope::Db12 {
                        continue;
                    }
                    for &g in gl {
                        for &q in ql {
                            let p = band(*shape, f, g, q, slope);
                            let (lo, hi) = deviation(&p, fs);
                            assert!(lo.is_finite() && hi.is_finite());
                            let b = bucket(f, fs);
                            total += 1;
                            let ok = lo <= PLAN_LO && hi <= PLAN_HI;
                            meets_plan += ok as usize;
                            if (1.0..=3.0).contains(&q)
                                && g.abs() <= 6.0
                                && slope.db_per_oct() <= 24
                            {
                                core_total += 1;
                                core_meets += ok as usize;
                                let c = core.entry((si, b)).or_insert((0.0, 0.0));
                                c.0 = c.0.max(lo);
                                c.1 = c.1.max(hi);
                            }
                            let desc = format!(
                                "{} {}dB/oct g{g} q{q} f{f} fs{fs}",
                                shape.name(),
                                slope.db_per_oct()
                            );
                            let e = worst.entry((si, b)).or_insert((
                                0.0,
                                0.0,
                                String::new(),
                                String::new(),
                            ));
                            if lo > e.0 {
                                e.0 = lo;
                                e.2 = desc.clone();
                            }
                            if hi > e.1 {
                                e.1 = hi;
                                e.3 = desc.clone();
                            }
                            let (tl, th) = TOL[si][b];
                            assert!(
                                lo <= tl && hi <= th,
                                "{desc}: lo {lo:.4} (tol {tl}) hi {hi:.4} (tol {th})"
                            );
                        }
                    }
                }
            }
        }
    }
    println!("grid points: {total}, meeting the PLAN targets (0.1/0.5 dB): {meets_plan}");
    println!("core domain points: {core_total}, meeting the PLAN targets: {core_meets}");
    println!("--- core (Q 1..3, |gain| <= 6 dB, <= 24 dB/oct): worst dB (lo, hi) ---");
    for ((si, b), (lo, hi)) in &core {
        println!(
            "{:10} {:8} lo {:7.4} hi {:7.4}",
            Shape::ALL[*si].name(),
            BUCKETS[*b],
            lo,
            hi
        );
    }
    println!("--- all grid points: worst dB ---");
    for ((si, b), (lo, hi, dl, dh)) in &worst {
        println!(
            "{:10} {:8} lo {:7.4} [{}]  hi {:7.4} [{}]",
            Shape::ALL[*si].name(),
            BUCKETS[*b],
            lo,
            dl,
            hi,
            dh
        );
    }
    // The everyday bell (Q 1..3, small gains) must meet the PLAN target below
    // 0.1 fs: that is the region where the classic cramping problem is solved.
    for b in 0..3 {
        let (lo, hi) = core[&(0, b)];
        assert!(
            lo <= PLAN_LO && hi <= PLAN_HI,
            "core bell bucket {}: {lo} {hi}",
            BUCKETS[b]
        );
    }
}

/// Low frequencies at high sample rates: the matched design must agree with
/// the analog prototype (the runtime precision is tested in `impulse.rs`).
#[test]
fn low_frequency_high_rate_corner_matches_analog() {
    let cases = [
        (bell(10.0, 12.0, 1.0), 192000.0, 0.01),
        (bell(10.0, -12.0, 10.0), 192000.0, 0.01),
        (bell(10.0, 24.0, 40.0), 192000.0, 0.01),
        (bell(10.0, 6.0, 0.025), 192000.0, 0.01),
        (
            band(Shape::LowCut, 10.0, 0.0, 1.0, Slope::Db96),
            192000.0,
            0.05,
        ),
        (
            band(Shape::LowCut, 10.0, 0.0, 40.0, Slope::Db12),
            192000.0,
            0.01,
        ),
        (
            band(Shape::LowShelf, 10.0, 18.0, 1.0, Slope::Db12),
            192000.0,
            0.01,
        ),
        (
            band(Shape::HighShelf, 10.0, -18.0, 1.0, Slope::Db12),
            192000.0,
            0.01,
        ),
        (bell(20.0, 12.0, 5.0), 96000.0, 0.01),
        (
            band(Shape::Notch, 10.0, 0.0, 5.0, Slope::Db24),
            192000.0,
            0.05,
        ),
    ];
    for (p, fs, tol) in cases {
        let (lo, hi) = deviation(&p, fs);
        println!(
            "{:?} {} Hz Q{} {}dB/oct @ {fs}: lo {lo:.5} dB, hi {hi:.5} dB",
            p.shape,
            p.freq_hz,
            p.q,
            p.slope.db_per_oct()
        );
        assert!(lo <= tol && hi <= tol, "{p:?} @ {fs}: {lo} {hi}");
    }
}

/// Structure that must survive the digital mapping exactly.
#[test]
fn notch_depth_and_cut_stopband_slope_are_preserved() {
    for &fs in &[44100.0, 96000.0] {
        // notch: exact zero at the band frequency (deep, not just "small")
        for &(f, q) in &[(100.0, 2.0), (3000.0, 4.0), (9000.0, 1.0)] {
            let p = band(Shape::Notch, f, 0.0, q, Slope::Db24);
            let mut d = [0.0];
            band_response_db(&p, 1.0, fs, &[f], &mut d);
            assert!(d[0] < -120.0, "notch {f} Q{q} @ {fs}: {} dB", d[0]);
        }
        // low cut: the stop band keeps falling at the analog slope far below
        // the cut (double zero at DC survives): 12 dB/oct => 24 dB per 2 octaves
        let p = band(Shape::LowCut, 1000.0, 0.0, 1.0, Slope::Db12);
        let mut d = [0.0; 2];
        band_response_db(&p, 1.0, fs, &[20.0, 40.0], &mut d);
        assert!((d[1] - d[0] - 12.0).abs() < 0.05, "{d:?}");
    }
}

/// Boost and cut with the same settings must be exact inverses (analog), and
/// the digital versions must cancel to within the matching accuracy.
#[test]
fn digital_boost_cut_cancel() {
    for slope in Slope::ALL {
        for &(f, q) in &[(100.0, 1.0), (2000.0, 3.0), (6000.0, 0.7)] {
            let up = band(Shape::Bell, f, 10.0, q, slope);
            let dn = band(Shape::Bell, f, -10.0, q, slope);
            let fr = grid(20.0, 20000.0, 200);
            let (mut a, mut b) = (vec![0.0; 200], vec![0.0; 200]);
            band_response_db(&up, 1.0, 48000.0, &fr, &mut a);
            band_response_db(&dn, 1.0, 48000.0, &fr, &mut b);
            let worst = a
                .iter()
                .zip(&b)
                .map(|(x, y)| (x + y).abs())
                .fold(0.0, f64::max);
            // Both curves carry the same kind of matching error; they do not
            // cancel exactly (see the accuracy table) but stay within it.
            assert!(worst < 0.6, "{slope:?} {f} {q}: {worst}");
        }
    }
}

fn presets() -> Vec<(&'static str, Vec<BandParams>)> {
    let s = slope_from_db;
    let a = vec![
        band(Shape::Bell, 41.1, 2.58, 1.00, s(12)),
        band(Shape::HighShelf, 722.9, -1.32, 1.0, s(12)),
        band(Shape::Bell, 98.0, -1.83, 2.73, s(24)),
        band(Shape::Bell, 35.9, 2.54, 2.75, s(24)),
        band(Shape::Bell, 9729.0, 2.46, 3.24, s(48)),
        band(Shape::Bell, 233.0, -2.50, 1.0, s(12)),
        band(Shape::LowCut, 28.6, 0.0, 1.0, s(12)),
        band(Shape::Bell, 177.7, -5.38, 0.68, s(12)),
        band(Shape::Bell, 2093.0, -2.99, 1.25, s(48)),
        band(Shape::Bell, 5074.0, 4.90, 10.91, s(24)),
        band(Shape::Bell, 392.7, -2.87, 2.97, s(12)),
        band(Shape::Bell, 2035.0, 2.78, 5.00, s(48)),
        band(Shape::Bell, 11787.0, 2.45, 4.59, s(48)),
        band(Shape::Bell, 15484.0, 15.69, 7.23, s(24)),
    ];
    let b = vec![
        band(Shape::HighCut, 13607.0, 0.0, 1.0, s(48)),
        band(Shape::Bell, 4317.0, 10.58, 1.05, s(24)),
        band(Shape::Bell, 85.7, 7.62, 1.77, s(24)),
        band(Shape::Bell, 701.3, -7.36, 2.01, s(12)),
        band(Shape::Bell, 15959.0, 8.51, 14.14, s(48)),
        band(Shape::Bell, 13195.0, -4.62, 10.0, s(36)),
        band(Shape::Bell, 6282.0, -3.54, 10.0, s(36)),
        band(Shape::Bell, 10011.0, 2.64, 5.95, s(12)),
        band(Shape::Bell, 165.7, 2.06, 2.97, s(48)),
    ];
    vec![("Friend EQ A", a), ("Friend EQ B", b)]
}

/// The two real-world settings from the project notes: print the deviation
/// from the analog prototype over 20 Hz - 20 kHz (run with `--nocapture`).
#[test]
fn friend_presets_deviation_from_analog() {
    let freqs = grid(20.0, 20000.0, 2000);
    for (name, bands) in presets() {
        for &fs in &[44100.0, 48000.0] {
            let mut analog = vec![0.0; freqs.len()];
            let mut digital = vec![0.0; freqs.len()];
            let mut tmp = vec![0.0; freqs.len()];
            let mut worst_band = (0.0f64, 0usize, 0.0f64);
            for (i, p) in bands.iter().enumerate() {
                analog_band_response_db(p, 1.0, fs, &freqs, &mut tmp);
                analog.iter_mut().zip(&tmp).for_each(|(a, t)| *a += t);
                band_response_db(p, 1.0, fs, &freqs, &mut tmp);
                digital.iter_mut().zip(&tmp).for_each(|(a, t)| *a += t);
                // per band
                let mut a1 = vec![0.0; freqs.len()];
                analog_band_response_db(p, 1.0, fs, &freqs, &mut a1);
                let (dev, at) = a1
                    .iter()
                    .zip(&tmp)
                    .zip(&freqs)
                    .map(|((a, d), f)| ((a - d).abs(), *f))
                    .fold((0.0f64, 0.0), |m, x| if x.0 > m.0 { x } else { m });
                if dev > worst_band.0 {
                    worst_band = (dev, i, at);
                }
            }
            let (dev, at) = analog
                .iter()
                .zip(&digital)
                .zip(&freqs)
                .map(|((a, d), f)| ((a - d).abs(), *f))
                .fold((0.0f64, 0.0), |m, x| if x.0 > m.0 { x } else { m });
            println!(
                "{name} @ {fs}: total max deviation {dev:.4} dB at {at:.0} Hz; worst single band #{} {:.4} dB at {:.0} Hz",
                worst_band.1 + 1,
                worst_band.0,
                worst_band.2
            );
            // Measured: A 0.22 / 0.15 dB (band 15484 Hz +15.7 dB Q 7.2 at
            // 18 kHz), B 1.5 / 1.9 dB (13.6 kHz 48 dB/oct low pass, 28 dB down
            // at 20 kHz). Guard with margin.
            let limit = if name.ends_with('A') { 0.35 } else { 2.5 };
            assert!(dev <= limit, "{name} @ {fs}: {dev} dB > {limit}");
        }
    }
}
