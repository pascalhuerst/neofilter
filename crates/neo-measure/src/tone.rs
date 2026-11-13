//! Single-tone analysis: frequency, level, THD, THD+N, and channel
//! separation.
//!
//! Realtime: **off-thread analysis** (allocates).
//!
//! Method: the block is windowed (default 4-term Blackman-Harris, -92 dB
//! sidelobes), the peak is located on a zero-padded FFT and then refined by
//! maximising the windowed DTFT magnitude over frequency (golden section),
//! which gives a frequency error far below 0.01 Hz for clean tones. Levels
//! are read from the DTFT at the refined frequency (`2|X|/sum(w)`; peak
//! amplitude, full scale = 0 dBFS) and harmonic levels at the exact
//! multiples. THD+N integrates the total windowed power in the band
//! `[band_low, band_high]` and removes the fundamental's main lobe
//! (+-4.5 un-padded bins); the measurement floor of the method is below
//! -120 dB.
//!
//! ```
//! use neo_measure::tone::*;
//! let x: Vec<f64> = (0..48_000).map(|i| 0.5 * (2.0*std::f64::consts::PI*1000.0*i as f64/48_000.0).sin()).collect();
//! let a = analyze_tone(&x, &ToneConfig::new(48_000.0)).unwrap();
//! assert!((a.frequency_hz - 1000.0).abs() < 0.01 && (a.level_dbfs + 6.02).abs() < 0.01);
//! ```

use crate::util::{next_pow2, rfft, window_sums, Window};
use std::f64::consts::PI;

/// Analysis settings.
#[derive(Debug, Clone, Copy)]
pub struct ToneConfig {
    pub sample_rate: f64,
    /// Highest harmonic order considered (2..); harmonics above Nyquist or
    /// `band_high` are skipped.
    pub max_harmonic: usize,
    /// Lower edge of the THD+N band (default 20 Hz).
    pub band_low: f64,
    /// Upper edge of the THD+N band (default 20 kHz, capped at Nyquist).
    pub band_high: f64,
    pub window: Window,
}

impl ToneConfig {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            max_harmonic: 10,
            band_low: 20.0,
            band_high: 20_000.0,
            window: Window::BlackmanHarris,
        }
    }
}

/// One harmonic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Harmonic {
    pub order: usize,
    pub frequency_hz: f64,
    /// Level in dBFS (peak-referenced).
    pub level_dbfs: f64,
    /// Level relative to the fundamental, dB.
    pub rel_db: f64,
}

/// Result of [`analyze_tone`].
#[derive(Debug, Clone)]
pub struct ToneAnalysis {
    pub frequency_hz: f64,
    /// Fundamental peak level in dBFS (full-scale sine = 0 dBFS).
    pub level_dbfs: f64,
    pub harmonics: Vec<Harmonic>,
    /// THD relative to the fundamental (harmonics 2..=N), in dB.
    pub thd_db: f64,
    pub thd_percent: f64,
    /// THD+N relative to the fundamental over the configured band, dB.
    pub thd_n_db: f64,
    pub thd_n_percent: f64,
}

struct Prepared {
    xw: Vec<f64>,
    s1: f64,
    s2: f64,
}

fn prepare(x: &[f64], win: Window) -> Prepared {
    let w = win.coefficients(x.len());
    let (s1, s2) = window_sums(&w);
    let xw = x.iter().zip(&w).map(|(a, b)| a * b).collect();
    Prepared { xw, s1, s2 }
}

/// |sum x_w[i] e^{-j 2 pi f i / fs}|
fn dtft_mag(xw: &[f64], f: f64, fs: f64) -> f64 {
    let step = 2.0 * PI * f / fs;
    let (ds, dc) = step.sin_cos();
    let (mut s, mut c) = (0.0f64, 1.0f64);
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in xw.iter().enumerate() {
        re += v * c;
        im -= v * s;
        let (ns, nc) = (s * dc + c * ds, c * dc - s * ds);
        s = ns;
        c = nc;
        if i % 512 == 511 {
            let (sn, cn) = ((i + 1) as f64 * step).sin_cos();
            s = sn;
            c = cn;
        }
    }
    (re * re + im * im).sqrt()
}

fn refine(xw: &[f64], f_lo: f64, f_hi: f64, fs: f64) -> f64 {
    let gr = 0.5 * (5f64.sqrt() - 1.0);
    let (mut a, mut b) = (f_lo, f_hi);
    let (mut x1, mut x2) = (b - gr * (b - a), a + gr * (b - a));
    let (mut f1, mut f2) = (dtft_mag(xw, x1, fs), dtft_mag(xw, x2, fs));
    for _ in 0..40 {
        if f1 > f2 {
            b = x2;
            x2 = x1;
            f2 = f1;
            x1 = b - gr * (b - a);
            f1 = dtft_mag(xw, x1, fs);
        } else {
            a = x1;
            x1 = x2;
            f1 = f2;
            x2 = a + gr * (b - a);
            f2 = dtft_mag(xw, x2, fs);
        }
    }
    0.5 * (a + b)
}

fn amp_db(a: f64) -> f64 {
    crate::util::amp_to_db(a)
}

/// Analyse the dominant tone of `x`. `None` for blocks shorter than 64
/// samples, silent input, or an invalid configuration.
pub fn analyze_tone(x: &[f64], cfg: &ToneConfig) -> Option<ToneAnalysis> {
    let fs = cfg.sample_rate;
    let n = x.len();
    if n < 64 || !(fs > 0.0) || x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let p = prepare(x, cfg.window);
    let nfft = next_pow2(n).max(1024) * 2;
    let spec = rfft(&p.xw, nfft);
    if spec.is_empty() {
        return None;
    }
    let df = fs / nfft as f64;
    let k_lo = ((cfg.band_low.max(1.0) / df).ceil() as usize).max(1);
    let nyq = fs / 2.0;
    let hi = cfg.band_high.min(nyq);
    let k_hi = ((hi / df).floor() as usize).min(spec.len() - 1);
    if k_lo >= k_hi {
        return None;
    }
    let (mut kp, mut best) = (k_lo, 0.0);
    for (k, v) in spec.iter().enumerate().take(k_hi + 1).skip(k_lo) {
        let m = v.norm_sqr();
        if m > best {
            best = m;
            kp = k;
        }
    }
    if best <= 0.0 {
        return None;
    }
    let f0 = refine(&p.xw, (kp as f64 - 1.0) * df, (kp as f64 + 1.0) * df, fs);
    let a1 = 2.0 * dtft_mag(&p.xw, f0, fs) / p.s1;
    let mut harmonics = Vec::new();
    let mut hp = 0.0;
    for h in 2..=cfg.max_harmonic.max(1) {
        let fh = f0 * h as f64;
        if fh >= hi.min(nyq * 0.999) {
            break;
        }
        // exact multiple; tolerate tiny fundamental error by a local refine
        let fr = refine(&p.xw, fh - 0.5 * df, fh + 0.5 * df, fs);
        let a = 2.0 * dtft_mag(&p.xw, fr, fs) / p.s1;
        hp += a * a;
        harmonics.push(Harmonic {
            order: h,
            frequency_hz: fr,
            level_dbfs: amp_db(a),
            rel_db: amp_db(a / a1),
        });
    }
    let thd = hp.sqrt() / a1;
    // THD+N: Parseval on the padded one-sided spectrum
    let lobe = 4.5 * fs / n as f64;
    let mut total = 0.0;
    let mut fund = 0.0;
    for (k, v) in spec.iter().enumerate().take(k_hi + 1).skip(k_lo) {
        let f = k as f64 * df;
        let m = v.norm_sqr();
        if (f - f0).abs() <= lobe {
            fund += m;
        } else {
            total += m;
        }
    }
    let _ = p.s2;
    let thdn = (total / fund.max(1e-300)).sqrt();
    Some(ToneAnalysis {
        frequency_hz: f0,
        level_dbfs: amp_db(a1),
        harmonics,
        thd_db: amp_db(thd),
        thd_percent: thd * 100.0,
        thd_n_db: amp_db(thdn),
        thd_n_percent: thdn * 100.0,
    })
}

/// Level (dBFS, peak-referenced) of the component at exactly `freq` Hz,
/// measured with a Blackman-Harris windowed DTFT.
pub fn tone_level_at(x: &[f64], fs: f64, freq: f64) -> f64 {
    if x.len() < 16 {
        return crate::util::DB_FLOOR;
    }
    let p = prepare(x, Window::BlackmanHarris);
    amp_db(2.0 * dtft_mag(&p.xw, freq, fs) / p.s1)
}

/// Channel separation (crosstalk) in dB, positive numbers meaning better
/// separation: level of the tone in `driven` minus its level at the same
/// frequency in `other`. The frequency is taken from the driven channel.
/// Limited by the noise floor of `other` (the reading is then the noise in
/// the analysis bandwidth rather than crosstalk).
pub fn channel_separation_db(driven: &[f64], other: &[f64], fs: f64) -> Option<f64> {
    let a = analyze_tone(driven, &ToneConfig::new(fs))?;
    if other.len() != driven.len() {
        return None;
    }
    Some(a.level_dbfs - tone_level_at(other, fs, a.frequency_hz))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{Generator, WhiteNoise};

    fn tone(fs: f64, f: f64, n: usize, comps: &[(usize, f64, f64)]) -> Vec<f64> {
        // fundamental amplitude 1 plus (order, level dB rel, phase) harmonics
        (0..n)
            .map(|i| {
                let t = 2.0 * PI * f * i as f64 / fs;
                t.sin()
                    + comps
                        .iter()
                        .map(|&(h, db, ph)| 10f64.powf(db / 20.0) * (h as f64 * t + ph).sin())
                        .sum::<f64>()
            })
            .collect()
    }

    #[test]
    fn frequency_and_level_accuracy() {
        let fs = 48_000.0;
        let mut worst = 0.0f64;
        for f in [1000.0, 997.0, 1000.37, 3150.0, 55.55, 12_345.678] {
            let x: Vec<f64> = tone(fs, f, 48_000, &[]).iter().map(|v| 0.5 * v).collect();
            let a = analyze_tone(&x, &ToneConfig::new(fs)).unwrap();
            worst = worst.max((a.frequency_hz - f).abs());
            assert!(
                (a.level_dbfs + 6.0206).abs() < 0.005,
                "{f}: {}",
                a.level_dbfs
            );
            assert!(a.thd_db < -120.0, "{f}: {}", a.thd_db);
        }
        eprintln!("tone frequency worst error {worst:.2e} Hz");
        assert!(worst < 0.01);
        // short block
        let x = tone(fs, 1000.0, 4800, &[]);
        let a = analyze_tone(&x, &ToneConfig::new(fs)).unwrap();
        assert!((a.frequency_hz - 1000.0).abs() < 0.01);
    }

    #[test]
    fn harmonics_and_thd() {
        let fs = 48_000.0;
        let x = tone(fs, 1000.0, 96_000, &[(2, -60.0, 0.3), (3, -70.0, 1.1)]);
        let a = analyze_tone(&x, &ToneConfig::new(fs)).unwrap();
        let h2 = a.harmonics.iter().find(|h| h.order == 2).unwrap();
        let h3 = a.harmonics.iter().find(|h| h.order == 3).unwrap();
        eprintln!(
            "h2 {:.4} h3 {:.4} thd {:.3} dB",
            h2.rel_db, h3.rel_db, a.thd_db
        );
        assert!((h2.rel_db + 60.0).abs() < 0.05);
        assert!((h3.rel_db + 70.0).abs() < 0.05);
        let want = 10.0 * (1e-6f64 + 1e-7).log10();
        assert!((a.thd_db - want).abs() < 0.05);
        assert!((a.thd_percent - 100.0 * (1.1e-6f64).sqrt()).abs() < 0.001);
        // no noise: THD+N ~ THD
        assert!((a.thd_n_db - want).abs() < 0.2, "{} {want}", a.thd_n_db);
        // other tone frequency with the same harmonics, low f0
        let x = tone(fs, 100.0, 48_000, &[(2, -60.0, 0.0), (5, -80.0, 2.0)]);
        let a = analyze_tone(&x, &ToneConfig::new(fs)).unwrap();
        let h2 = a.harmonics.iter().find(|h| h.order == 2).unwrap();
        assert!((h2.rel_db + 60.0).abs() < 0.05);
    }

    #[test]
    fn thd_plus_noise() {
        let fs = 48_000.0;
        let mut g = WhiteNoise::new(-80.0, 9); // rms 1e-4 over 0..24k
        let mut x = tone(fs, 1000.0, 96_000, &[(2, -70.0, 0.0)]);
        for v in x.iter_mut() {
            *v += g.next_sample();
        }
        let a = analyze_tone(&x, &ToneConfig::new(fs)).unwrap();
        // noise inside 20 Hz..20 kHz: 1e-8 * (19980/24000); fundamental power 0.5
        let noise = 1e-8 * 19_980.0 / 24_000.0;
        let dist = 0.5 * 10f64.powf(-7.0);
        let want = 10.0 * ((noise + dist) / 0.5).log10();
        eprintln!("THD+N {:.3} dB want {want:.3}", a.thd_n_db);
        assert!((a.thd_n_db - want).abs() < 0.3);
    }

    #[test]
    fn separation_and_edge_cases() {
        let fs = 48_000.0;
        let l = tone(fs, 1000.0, 48_000, &[]);
        let r: Vec<f64> = l.iter().map(|v| 10f64.powf(-70.0 / 20.0) * v).collect();
        let s = channel_separation_db(&l, &r, fs).unwrap();
        assert!((s - 70.0).abs() < 0.01, "{s}");
        assert!(channel_separation_db(&l, &r[..100], fs).is_none());
        assert!(analyze_tone(&[], &ToneConfig::new(fs)).is_none());
        assert!(analyze_tone(&vec![0.0; 1000], &ToneConfig::new(fs)).is_none());
        assert!(tone_level_at(&[], fs, 1000.0) < -300.0);
    }
}
