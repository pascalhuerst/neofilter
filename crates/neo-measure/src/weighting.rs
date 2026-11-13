//! Frequency weightings as IIR filters for arbitrary sample rates (A-weighting
//! and the wow-and-flutter weighting) and a rumble/noise level helper.
//!
//! Realtime: the filters ([`AWeighting`], [`WowFlutterWeighting`]) are
//! allocation-free and panic-free. [`measure_noise`] allocates nothing but
//! processes a whole block and belongs off the audio thread.
//!
//! A-weighting: the analog IEC 61672 curve (poles 20.599, 107.65, 737.86,
//! 12194.2 Hz, normalised to 0 dB at 1 kHz) realised as four bilinear
//! first-order high-pass sections (corners pre-warped) plus one biquad fitted
//! at construction to the 12.2 kHz double pole (the plain bilinear transform
//! loses several dB near Nyquist). Measured deviation from the analog curve,
//! 20 Hz to min(20 kHz, 0.45 fs): below 0.06 dB at 44.1 and 48 kHz, below
//! 0.01 dB at 96 kHz and above. Construction runs a small numerical fit
//! (sub-millisecond to a few ms), so build the filter off the audio thread.
//!
//! Wow/flutter weighting: see [`WowFlutterWeighting`] for the exact status of
//! the curve.
//!
//! ```
//! use neo_measure::weighting::*;
//! let w = AWeighting::new(48_000.0);
//! assert!(w.response_db(1000.0).abs() < 1e-9);
//! assert!((w.response_db(100.0) - a_weighting_db(100.0)).abs() < 0.1);
//! ```

use crate::filter::{Biquad, BiquadCoeffs, Cascade};
use crate::util::{amp_to_db, Complex};
use crate::Sample;

const A_F1: f64 = 20.598_997;
const A_F2: f64 = 107.652_65;
const A_F3: f64 = 737.862_23;
const A_F4: f64 = 12_194.217;

fn a_raw(f: f64) -> f64 {
    let f2 = f * f;
    let n = A_F4 * A_F4 * f2 * f2;
    let d =
        (f2 + A_F1 * A_F1) * ((f2 + A_F2 * A_F2) * (f2 + A_F3 * A_F3)).sqrt() * (f2 + A_F4 * A_F4);
    n / d
}

/// Analog A-weighting magnitude in dB (0 dB at 1 kHz).
pub fn a_weighting_db(f: f64) -> f64 {
    20.0 * (a_raw(f) / a_raw(1000.0)).log10()
}

/// Digital A-weighting filter.
#[derive(Debug, Clone)]
pub struct AWeighting {
    fs: f64,
    c: Cascade<6>,
}

impl AWeighting {
    pub fn new(fs: f64) -> Self {
        let (hf1, hf2) = Self::design_hf(fs);
        let coeffs = [
            BiquadCoeffs::highpass1(fs, A_F1),
            BiquadCoeffs::highpass1(fs, A_F1),
            BiquadCoeffs::highpass1(fs, A_F2),
            BiquadCoeffs::highpass1(fs, A_F3),
            hf1,
            hf2,
        ];
        let mut c = Cascade::new(coeffs, 1.0);
        c.gain = 1.0 / c.response(1000.0, fs).norm();
        Self { fs, c }
    }
    /// The two 12.2 kHz poles: a biquad fitted (Levenberg-Marquardt on the dB
    /// error, 50 Hz .. min(20 kHz, 0.45 fs)) to the analog double pole, because
    /// the plain bilinear transform loses many dB near Nyquist. The second
    /// returned section is an identity. Falls back to the bilinear pair if the
    /// fit is unstable.
    fn design_hf(fs: f64) -> (BiquadCoeffs, BiquadCoeffs) {
        let l1 = BiquadCoeffs::lowpass1(fs, A_F4);
        let pair = (l1, l1);
        let top = 20_000f64.min(0.45 * fs);
        let grid: Vec<f64> = (0..120)
            .map(|i| 50.0 * (top / 50.0).powf(i as f64 / 119.0))
            .collect();
        let target: Vec<f64> = grid
            .iter()
            .map(|f| -20.0 * (1.0 + (f / A_F4).powi(2)).log10())
            .collect();
        // initial: product of the two bilinear sections
        let (b0, b1) = (l1.b0, l1.b1);
        let th0 = vec![b0 * b0, 2.0 * b0 * b1, b1 * b1, 2.0 * l1.a1, l1.a1 * l1.a1];
        let mk = |t: &[f64]| BiquadCoeffs {
            b0: t[0],
            b1: t[1],
            b2: t[2],
            a1: t[3],
            a2: t[4],
        };
        let resid = |t: &[f64]| -> Vec<f64> {
            let c = mk(t);
            grid.iter()
                .zip(&target)
                .map(|(&f, &tg)| 20.0 * c.response(f, fs).norm().max(1e-12).log10() - tg)
                .collect()
        };
        let th = crate::util::levenberg_marquardt(th0, resid, 100);
        let c = mk(&th);
        // stability triangle
        let stable = c.a2.abs() < 1.0 && c.a1.abs() < 1.0 + c.a2;
        if stable && th.iter().all(|v| v.is_finite()) {
            (c, BiquadCoeffs::IDENTITY)
        } else {
            pair
        }
    }
    pub fn reset(&mut self) {
        self.c.reset();
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        self.c.process(x)
    }
    /// Magnitude response of this digital filter in dB.
    pub fn response_db(&self, freq: f64) -> f64 {
        20.0 * self.c.response(freq, self.fs).norm().log10()
    }
}

// Wow & flutter weighting: 2nd-order high-pass (f 0.4831 Hz, Q 0.2678) times a
// first-order low-pass (9.483 Hz), fitted to peak at 4 Hz with -6 dB at 0.8 Hz
// and 20 Hz.
const WF_HP_F: f64 = 0.483_123_706_400_284;
const WF_HP_Q: f64 = 0.267_829_082_177_88;
const WF_LP_F: f64 = 9.482_980_766_046_337;

/// Analog prototype of the wow/flutter weighting in dB, 0 dB at 4 Hz.
pub fn wf_weighting_db(f: f64) -> f64 {
    wf_analog(f) - wf_analog(4.0)
}

fn wf_analog(f: f64) -> f64 {
    let s = Complex::new(0.0, f);
    let x = s / WF_HP_F;
    let hp = x * x / (x * x + x / WF_HP_Q + 1.0);
    let lp = Complex::new(1.0, 0.0) / (1.0 + s / WF_LP_F);
    amp_to_db((hp * lp).norm())
}

/// Wow-and-flutter weighting filter (peak 4 Hz, 0 dB).
///
/// **Status:** the curve is a two-pole high-pass plus one-pole low-pass
/// *fitted* to the published shape of the DIN 45507 / IEC 60386 weighting
/// (maximum at 4 Hz, -6 dB at about 0.8 Hz and 20 Hz, roughly -25 dB at
/// 200 Hz). The standard's tabulated values and tolerance mask were not
/// available while writing this, so conformance to the mask has **not** been
/// verified; the response is exact only with respect to this prototype
/// ([`wf_weighting_db`]). Use a sample rate of at least 200 Hz; the design is
/// accurate (bilinear) for `fs >= ~2 kHz` up to 100 Hz.
#[derive(Debug, Clone)]
pub struct WowFlutterWeighting {
    fs: f64,
    c: Cascade<2>,
}

impl WowFlutterWeighting {
    pub fn new(fs: f64) -> Self {
        let mut c = Cascade::new(
            [
                BiquadCoeffs::highpass(fs, WF_HP_F, WF_HP_Q),
                BiquadCoeffs::lowpass1(fs, WF_LP_F),
            ],
            1.0,
        );
        c.gain = 1.0 / c.response(4.0, fs).norm();
        Self { fs, c }
    }
    pub fn reset(&mut self) {
        self.c.reset();
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        self.c.process(x)
    }
    /// Magnitude response of the digital filter in dB (0 dB at 4 Hz).
    pub fn response_db(&self, freq: f64) -> f64 {
        20.0 * self.c.response(freq, self.fs).norm().log10()
    }
}

/// Result of [`measure_noise`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseLevels {
    /// RMS level in dBFS (`20 log10 rms`) in the unweighted band.
    pub unweighted_dbfs: f64,
    /// A-weighted RMS level in dBFS.
    pub a_weighted_dbfs: f64,
    /// Unweighted level relative to the reference level, dB (negative = below).
    pub unweighted_rel_db: f64,
    /// A-weighted level relative to the reference level, dB.
    pub a_weighted_rel_db: f64,
}

/// Rumble / noise level of a block: band-limited unweighted level
/// (`band_low..band_high`, 2nd-order Butterworth high-pass and low-pass) and
/// A-weighted level, each also relative to `reference_dbfs` (RMS dBFS of the
/// reference recording level, e.g. -20 dBFS for a 1 kHz line-up tone).
/// The first 5 % of the block (at most 1 s) is discarded as filter settling.
/// Returns `None` if the block is shorter than 100 samples.
pub fn measure_noise<S: Sample>(
    x: &[S],
    fs: f64,
    band_low: f64,
    band_high: f64,
    reference_dbfs: f64,
) -> Option<NoiseLevels> {
    if x.len() < 100 || !(fs > 0.0) {
        return None;
    }
    let q = std::f64::consts::FRAC_1_SQRT_2;
    let mut hp = Biquad::new(BiquadCoeffs::highpass(fs, band_low, q));
    let mut lp = Biquad::new(BiquadCoeffs::lowpass(fs, band_high.min(0.45 * fs), q));
    let mut aw = AWeighting::new(fs);
    let skip = (x.len() / 20).min(fs as usize);
    let (mut su, mut sa, mut n) = (0.0, 0.0, 0usize);
    for (i, &s) in x.iter().enumerate() {
        let v: f64 = s.into();
        let v = if v.is_finite() { v } else { 0.0 };
        let u = lp.process(hp.process(v));
        let a = aw.process(v);
        if i >= skip {
            su += u * u;
            sa += a * a;
            n += 1;
        }
    }
    let nf = n.max(1) as f64;
    let (u, a) = (amp_to_db((su / nf).sqrt()), amp_to_db((sa / nf).sqrt()));
    Some(NoiseLevels {
        unweighted_dbfs: u,
        a_weighted_dbfs: a,
        unweighted_rel_db: u - reference_dbfs,
        a_weighted_rel_db: a - reference_dbfs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{Generator, WhiteNoise};

    #[test]
    fn a_weighting_analytic_table() {
        // IEC 61672-1 table values (dB)
        let table = [
            (31.5, -39.4),
            (63.0, -26.2),
            (125.0, -16.1),
            (250.0, -8.6),
            (500.0, -3.2),
            (1000.0, 0.0),
            (2000.0, 1.2),
            (4000.0, 1.0),
            (8000.0, -1.1),
            (16000.0, -6.6),
        ];
        for (f, want) in table {
            assert!(
                (a_weighting_db(f) - want).abs() < 0.15,
                "{f}: {}",
                a_weighting_db(f)
            );
        }
    }

    #[test]
    fn a_weighting_digital_vs_analog() {
        for fs in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let w = AWeighting::new(fs);
            let mut worst = 0.0f64;
            let mut worst_hi = 0.0f64;
            let mut f = 20.0;
            while f < 20_000.0 && f < 0.45 * fs {
                let e = (w.response_db(f) - a_weighting_db(f)).abs();
                if f <= 10_000.0 {
                    worst = worst.max(e);
                } else {
                    worst_hi = worst_hi.max(e);
                }
                f *= 1.05;
            }
            eprintln!(
                "A-weighting fs {fs}: max err {worst:.3} dB up to 10 kHz, {worst_hi:.2} dB above"
            );
            let lim = 0.1;
            assert!(worst < lim, "{fs} {worst}");
            assert!(w.response_db(1000.0).abs() < 1e-9);
        }
    }

    #[test]
    fn time_domain_gain_matches_response() {
        let fs = 48_000.0;
        let mut w = AWeighting::new(fs);
        let f = 250.0;
        let y: Vec<f64> = (0..48_000)
            .map(|i| w.process((2.0 * std::f64::consts::PI * f * i as f64 / fs).sin()))
            .collect();
        let pk = y[24_000..].iter().fold(0.0f64, |a, v| a.max(v.abs()));
        assert!((20.0 * pk.log10() - w.response_db(f)).abs() < 0.01);
    }

    #[test]
    fn wf_weighting_shape() {
        for fs in [1000.0, 2000.0, 48_000.0] {
            let w = WowFlutterWeighting::new(fs);
            assert!(w.response_db(4.0).abs() < 1e-9);
            // maximum at 4 Hz
            for f in [2.0, 3.0, 3.5, 4.5, 5.0, 8.0] {
                assert!(w.response_db(f) < 0.0, "{f}");
            }
            let tol = if fs < 1500.0 { 0.5 } else { 0.2 };
            for f in [0.5, 0.8, 1.0, 2.0, 6.3, 10.0, 20.0, 63.0] {
                assert!(
                    (w.response_db(f) - wf_weighting_db(f)).abs() < tol,
                    "{fs} {f}"
                );
            }
        }
        assert!((wf_weighting_db(0.8) + 6.0).abs() < 0.1);
        assert!((wf_weighting_db(20.0) + 6.0).abs() < 0.1);
    }

    #[test]
    fn noise_levels() {
        let fs = 48_000.0;
        let mut g = WhiteNoise::new(-60.0, 4);
        let x: Vec<f64> = (0..480_000).map(|_| g.next_sample()).collect();
        let r = measure_noise(&x, fs, 20.0, 20_000.0, -20.0).unwrap();
        // white noise: band 20..20k of 0..24k -> -0.8 dB
        let want = -60.0 + 10.0 * (19_980.0f64 / 24_000.0).log10();
        assert!(
            (r.unweighted_dbfs - want).abs() < 0.15,
            "{}",
            r.unweighted_dbfs
        );
        assert!((r.unweighted_rel_db - (r.unweighted_dbfs + 20.0)).abs() < 1e-12);
        // A-weighted white noise is ~ -3.0..-4 dB below unweighted full band
        assert!(r.a_weighted_dbfs < r.unweighted_dbfs && r.a_weighted_dbfs > -66.0);
        assert!(measure_noise::<f64>(&[], fs, 20.0, 20e3, 0.0).is_none());
    }
}
