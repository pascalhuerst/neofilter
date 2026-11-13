//! Delay estimation between a reference and a return signal.
//!
//! Generalised cross-correlation (FFT based, optional PHAT weighting) with
//! sub-sample refinement and a confidence figure.
//!
//! Realtime: **off-thread analysis only** (allocates; cost is a few FFTs of
//! size >= `len(reference) + len(return)` plus ~20 evaluations of the
//! band-limited interpolated correlation).
//!
//! Convention: the return `y` is the reference `x` delayed by `d` samples,
//! `y[n] = g * x[n - d]`, so a positive result means the return arrives later.
//! A negative gain `g` is reported through [`DelayEstimate::inverted`] and
//! does not disturb the delay estimate.
//!
//! ```
//! use neo_measure::delay::{estimate_delay, DelayOptions};
//! let x: Vec<f64> = (0..4000).map(|i| ((i * i * 7919) % 1000) as f64 - 500.0).collect();
//! let mut y = vec![0.0; 100];
//! y.extend(x.iter().map(|v| -0.5 * v));
//! let e = estimate_delay(&x, &y, &DelayOptions::new(1000)).unwrap();
//! assert!((e.delay_samples - 100.0).abs() < 0.05 && e.inverted);
//! ```

use crate::util::{irfft, next_pow2, rfft, Complex};
use std::f64::consts::PI;

/// Search and weighting options.
#[derive(Debug, Clone)]
pub struct DelayOptions {
    /// Smallest lag searched (negative: return may lead the reference).
    pub min_lag: isize,
    /// Largest lag searched, in samples (e.g. `5 * fs` for 5 s).
    pub max_lag: usize,
    /// PHAT weighting (whitens the cross spectrum; sharper peak, better for
    /// reverberant/coloured returns, worse at very low SNR in empty bands).
    pub phat: bool,
}

impl DelayOptions {
    /// Search lags `0..=max_lag`, plain cross-correlation.
    pub fn new(max_lag: usize) -> Self {
        Self {
            min_lag: 0,
            max_lag,
            phat: false,
        }
    }
}

/// Result of [`estimate_delay`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DelayEstimate {
    /// Delay of the return relative to the reference, samples (fractional).
    pub delay_samples: f64,
    /// Integer lag of the correlation maximum.
    pub integer_lag: isize,
    /// True when the return has inverted polarity (negative correlation peak).
    pub inverted: bool,
    /// Peak-to-sidelobe ratio (|peak| / largest |correlation| outside the
    /// main lobe), linear. Values above ~3-4 indicate a trustworthy
    /// detection; `INFINITY` if there is no sidelobe region at all.
    pub confidence: f64,
    /// Normalised correlation coefficient at the peak, in `-1..1` (0.0 when
    /// PHAT weighting is used, where it has no meaning).
    pub peak_correlation: f64,
}

impl DelayEstimate {
    /// Confidence as dB (20 log10).
    pub fn confidence_db(&self) -> f64 {
        20.0 * self.confidence.log10()
    }
}

/// Estimate the delay of `ret` relative to `reference`. Returns `None` for
/// empty/silent input or an empty search range.
pub fn estimate_delay(
    reference: &[f64],
    ret: &[f64],
    opts: &DelayOptions,
) -> Option<DelayEstimate> {
    let (nx, ny) = (reference.len(), ret.len());
    if nx < 2 || ny < 2 {
        return None;
    }
    let n = next_pow2(nx + ny);
    let xs = rfft(reference, n);
    let ys = rfft(ret, n);
    if xs.is_empty() || ys.is_empty() {
        return None;
    }
    let exx: f64 = reference.iter().map(|v| v * v).sum();
    let eyy: f64 = ret.iter().map(|v| v * v).sum();
    if exx <= 0.0 || eyy <= 0.0 || !exx.is_finite() || !eyy.is_finite() {
        return None;
    }
    let mut g: Vec<Complex> = ys.iter().zip(&xs).map(|(y, x)| y * x.conj()).collect();
    if opts.phat {
        let mean = g.iter().map(|v| v.norm()).sum::<f64>() / g.len() as f64;
        let eps = 0.3 * mean;
        for v in g.iter_mut() {
            *v /= v.norm() + eps;
        }
    }
    let c = irfft(&g, n);
    if c.is_empty() {
        return None;
    }
    // valid lags: [-(nx-1), ny-1]; search window intersected with it
    let lo = opts.min_lag.max(-(nx as isize - 1));
    let hi = (opts.max_lag as isize).min(ny as isize - 1);
    if lo > hi {
        return None;
    }
    let at = |lag: isize| c[lag.rem_euclid(n as isize) as usize];
    let mut best = lo;
    let mut bv = at(lo).abs();
    for lag in lo..=hi {
        let v = at(lag).abs();
        if v > bv {
            bv = v;
            best = lag;
        }
    }
    if bv <= 0.0 || !bv.is_finite() {
        return None;
    }
    let sign = if at(best) < 0.0 { -1.0 } else { 1.0 };

    // sub-sample refinement on the band-limited interpolated correlation
    let eval = |tau: f64| -> f64 {
        let mut s = 0.0;
        let step = 2.0 * PI * tau / n as f64;
        for (k, v) in g.iter().enumerate() {
            let w = if k == 0 || k == n / 2 { 1.0 } else { 2.0 };
            let (sn, cs) = (step * k as f64).sin_cos();
            s += w * (v.re * cs - v.im * sn);
        }
        sign * s / n as f64
    };
    let (mut a, mut b) = (best as f64 - 1.0, best as f64 + 1.0);
    let gr = 0.5 * (5f64.sqrt() - 1.0);
    let (mut x1, mut x2) = (b - gr * (b - a), a + gr * (b - a));
    let (mut f1, mut f2) = (eval(x1), eval(x2));
    for _ in 0..22 {
        if f1 > f2 {
            b = x2;
            x2 = x1;
            f2 = f1;
            x1 = b - gr * (b - a);
            f1 = eval(x1);
        } else {
            a = x1;
            x1 = x2;
            f1 = f2;
            x2 = a + gr * (b - a);
            f2 = eval(x2);
        }
    }
    let delay = 0.5 * (a + b);

    // main-lobe extent -> guard region
    let pk = at(best).abs();
    let mut ext = 0isize;
    while ext < 4096 && at(best + ext + 1).abs() > 0.25 * pk && best + ext + 1 <= ny as isize - 1 {
        ext += 1;
    }
    let mut ext2 = 0isize;
    while ext2 < 4096
        && at(best - ext2 - 1).abs() > 0.25 * pk
        && best - ext2 - 1 >= -(nx as isize - 1)
    {
        ext2 += 1;
    }
    let guard = 2 * ext.max(ext2) + 4;
    let mut side = 0.0f64;
    for lag in -(nx as isize - 1)..=(ny as isize - 1) {
        if (lag - best).abs() > guard {
            side = side.max(at(lag).abs());
        }
    }
    let confidence = if side > 0.0 { pk / side } else { f64::INFINITY };
    let peak_correlation = if opts.phat {
        0.0
    } else {
        sign * eval(delay) / (exx * eyy).sqrt()
    };
    Some(DelayEstimate {
        delay_samples: delay,
        integer_lag: best,
        inverted: sign < 0.0,
        confidence,
        peak_correlation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{Generator, WhiteNoise};

    /// Circularly delay `x` (zero padded to `n`) by `d` samples through an FFT
    /// phase ramp, applying `shape(f_norm)` as zero-phase gain.
    fn delayed(x: &[f64], d: f64, n: usize, shape: impl Fn(f64) -> f64) -> Vec<f64> {
        let mut s = rfft(x, n);
        let m = s.len();
        for (k, v) in s.iter_mut().enumerate() {
            let f = k as f64 / (m - 1) as f64; // 0..1 of Nyquist
            let g = if f > 0.97 { 0.0 } else { shape(f) };
            *v *= Complex::from_polar(g, -PI * k as f64 * d / (m - 1) as f64);
        }
        irfft(&s, n)
    }

    fn noise(len: usize, seed: u64) -> Vec<f64> {
        let mut g = WhiteNoise::new(-20.0, seed);
        (0..len).map(|_| g.next_sample()).collect()
    }

    #[test]
    fn integer_and_fractional_clean() {
        for (fs, d) in [
            (48_000.0, 96_000.3),
            (96_000.0, 192_000.0 - 0.7),
            (48_000.0, 1234.5),
        ] {
            let x = noise(fs as usize, 1);
            let n = next_pow2(x.len() + d as usize + 10);
            let y = delayed(&x, d, n, |_| 1.0);
            let e = estimate_delay(&x, &y, &DelayOptions::new((5.0 * fs) as usize)).unwrap();
            assert!(
                (e.delay_samples - d).abs() < 0.02,
                "{} vs {d}",
                e.delay_samples
            );
            assert!(!e.inverted);
            eprintln!("delay clean err {:.5}", e.delay_samples - d);
            assert!(e.confidence > 10.0);
            assert!(e.peak_correlation > 0.9);
        }
    }

    #[test]
    fn noisy_filtered_inverted() {
        let fs = 48_000.0;
        let d = 7000.37;
        let x = noise(2 * fs as usize, 2);
        let n = next_pow2(x.len() + d as usize + 10);
        let nz = noise(n, 99);
        let tilts: [(&str, Box<dyn Fn(f64) -> f64>); 3] = [
            (
                "tilt+20",
                Box::new(|f| {
                    10f64
                        .powf(20.0 * (f.max(1e-3).log10() / 3.0) / 20.0 / 1.0 * 1.0)
                        .min(10.0)
                        .max(0.1)
                }),
            ),
            (
                "tilt-20",
                Box::new(|f| {
                    10f64
                        .powf(-20.0 * (f.max(1e-3).log10() / 3.0) / 20.0)
                        .min(10.0)
                        .max(0.1)
                }),
            ),
            (
                "band",
                Box::new(|f| if f > 0.05 && f < 0.2 { 1.0 } else { 0.0 }),
            ),
        ];
        for (name, shape) in tilts {
            for snr_db in [20.0, 0.0] {
                let mut y = delayed(&x, d, n, &shape);
                // scale for attenuation and set SNR relative to y power
                let py: f64 = y.iter().map(|v| v * v).sum::<f64>() / y.len() as f64;
                let pn: f64 = nz.iter().map(|v| v * v).sum::<f64>() / nz.len() as f64;
                let k = (py / pn / 10f64.powf(snr_db / 10.0)).sqrt();
                for (v, w) in y.iter_mut().zip(&nz) {
                    *v = -0.3 * (*v + k * w);
                }
                for phat in [false, true] {
                    let mut o = DelayOptions::new(5 * fs as usize);
                    o.phat = phat;
                    let e = estimate_delay(&x, &y, &o).unwrap();
                    // the in-band tilt/band filters are zero phase, so no group delay
                    // PHAT whitens noise-only bins of the band-limited case, hence looser
                    let tol = if snr_db < 10.0 {
                        1.0
                    } else if phat {
                        0.25
                    } else {
                        0.1
                    };
                    assert!(
                        (e.delay_samples - d).abs() < tol,
                        "{name} snr {snr_db} phat {phat}: {}",
                        e.delay_samples
                    );
                    eprintln!(
                        "delay {name} snr {snr_db} phat {phat}: err {:.4} conf {:.1}",
                        e.delay_samples - d,
                        e.confidence
                    );
                    assert!(e.inverted, "{name}");
                    assert!(e.confidence > 3.0, "{name} {} ", e.confidence);
                }
            }
        }
    }

    #[test]
    fn edge_cases() {
        assert!(estimate_delay(&[], &[1.0], &DelayOptions::new(10)).is_none());
        assert!(estimate_delay(&[0.0; 100], &[1.0; 100], &DelayOptions::new(10)).is_none());
        let x = noise(1000, 4);
        // negative delay with min_lag
        let mut y = x[50..].to_vec();
        y.extend([0.0; 50]);
        let mut o = DelayOptions::new(100);
        o.min_lag = -100;
        let e = estimate_delay(&x, &y, &o).unwrap();
        assert!((e.delay_samples + 50.0).abs() < 0.05);
        // pure noise vs unrelated noise: low confidence
        let z = noise(1000, 5);
        let e = estimate_delay(&x, &z, &DelayOptions::new(100)).unwrap();
        assert!(e.confidence < 2.5, "{}", e.confidence);
    }
}
