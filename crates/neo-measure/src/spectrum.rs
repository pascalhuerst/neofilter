//! Welch-averaged FFT spectrum analyzer, fractional-octave smoothing,
//! IEC 61260 third-octave band levels and log-frequency display helpers.
//!
//! Realtime: **off-thread analysis** (construction allocates; `push` does not
//! allocate after construction and cannot panic, so a GUI thread may call it,
//! but it costs one FFT per hop and does not belong on the audio thread). The
//! free functions that return `Vec`s allocate.
//!
//! # Scaling conventions
//!
//! With window `w` (length `N`), `S1 = sum w`, `S2 = sum w^2` and raw bins
//! `X_k`:
//!
//! * *Amplitude spectrum* `A_k = 2 |X_k| / S1` (DC: `|X_0| / S1`): a sine of
//!   peak amplitude `a` centred on a bin reads `a`, so **a full-scale sine is
//!   0 dBFS** (peak-referenced). Flat-top reads it within 0.01 dB at any
//!   frequency, Hann and Blackman-Harris only at bin centres (scalloping).
//! * *Density* `D_k = 2 |X_k|^2 / (fs S2)` (one-sided, DC/Nyquist not
//!   doubled), unit `FS^2/Hz`: white noise of variance `s^2` reads `2 s^2/fs`
//!   and `sum D_k * df = s^2`.
//! * *Band levels* use mean-square power and are expressed so that a
//!   full-scale sine inside the band reads 0 dB.
//!
//! Averaging is always done in the power domain `|X_k|^2`.
//!
//! ```
//! use neo_measure::spectrum::*;
//! use neo_measure::util::Window;
//! let cfg = SpectrumConfig::new(48_000.0, 8192, Window::FlatTop);
//! let mut an = SpectrumAnalyzer::new(cfg).unwrap();
//! let sine: Vec<f64> = (0..16384).map(|i| (2.0*std::f64::consts::PI*1000.0*i as f64/48000.0).sin()).collect();
//! an.push(&sine);
//! let peak = an.amplitude_db().into_iter().fold(f64::MIN, f64::max);
//! assert!(peak.abs() < 0.01);
//! ```

use crate::util::{pow_to_db, window_sums, Complex, Window};
use crate::Sample;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Averaging mode of the spectrum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Averaging {
    /// Show the latest frame only.
    None,
    /// Exponential averaging with the given time constant in seconds
    /// (per-hop coefficient `1 - exp(-hop / (fs tau))`).
    Exponential(f64),
    /// Equal-weight average of all frames since the last `reset`.
    Infinite,
}

/// Analyzer configuration.
#[derive(Debug, Clone, Copy)]
pub struct SpectrumConfig {
    pub sample_rate: f64,
    /// Power of two in `1024..=65536`.
    pub fft_size: usize,
    pub window: Window,
    /// Overlap fraction `0.0..=0.9` (0.5 / 0.75 typical).
    pub overlap: f64,
    pub averaging: Averaging,
}

impl SpectrumConfig {
    /// 50 % overlap, infinite averaging off (exponential 0.5 s).
    pub fn new(sample_rate: f64, fft_size: usize, window: Window) -> Self {
        Self {
            sample_rate,
            fft_size,
            window,
            overlap: 0.5,
            averaging: Averaging::Exponential(0.5),
        }
    }
}

/// Streaming Welch spectrum analyzer for one channel (use one instance per
/// channel, e.g. L, R, M, S).
pub struct SpectrumAnalyzer {
    cfg: SpectrumConfig,
    plan: Arc<dyn RealToComplex<f64>>,
    win: Vec<f64>,
    s1: f64,
    s2: f64,
    ring: Vec<f64>,
    pos: usize,
    filled: usize,
    since: usize,
    hop: usize,
    buf: Vec<f64>,
    spec: Vec<Complex>,
    scratch: Vec<Complex>,
    avg: Vec<f64>,
    peak: Vec<f64>,
    frames: u64,
}

impl SpectrumAnalyzer {
    /// `None` when the configuration is invalid.
    pub fn new(cfg: SpectrumConfig) -> Option<Self> {
        let n = cfg.fft_size;
        if !(1024..=65536).contains(&n)
            || !n.is_power_of_two()
            || !(cfg.sample_rate > 0.0)
            || !(0.0..=0.9).contains(&cfg.overlap)
        {
            return None;
        }
        let plan = RealFftPlanner::<f64>::new().plan_fft_forward(n);
        let win = cfg.window.coefficients(n);
        let (s1, s2) = window_sums(&win);
        let hop = (((1.0 - cfg.overlap) * n as f64).round() as usize).max(1);
        let spec = plan.make_output_vec();
        let scratch = plan.make_scratch_vec();
        let m = n / 2 + 1;
        Some(Self {
            cfg,
            plan,
            win,
            s1,
            s2,
            ring: vec![0.0; n],
            pos: 0,
            filled: 0,
            since: 0,
            hop,
            buf: vec![0.0; n],
            spec,
            scratch,
            avg: vec![0.0; m],
            peak: vec![0.0; m],
            frames: 0,
        })
    }

    pub fn config(&self) -> &SpectrumConfig {
        &self.cfg
    }
    /// Number of bins (`fft_size/2 + 1`).
    pub fn bins(&self) -> usize {
        self.avg.len()
    }
    /// Bin spacing in Hz.
    pub fn bin_hz(&self) -> f64 {
        self.cfg.sample_rate / self.cfg.fft_size as f64
    }
    /// Equivalent noise bandwidth of the window in Hz.
    pub fn enbw_hz(&self) -> f64 {
        self.cfg.sample_rate * self.s2 / (self.s1 * self.s1)
    }
    /// Frames accumulated since the last reset.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Clear averages and peak hold (keeps the input history).
    pub fn reset(&mut self) {
        self.avg.iter_mut().for_each(|v| *v = 0.0);
        self.peak.iter_mut().for_each(|v| *v = 0.0);
        self.frames = 0;
    }
    /// Clear only the peak hold.
    pub fn reset_peak(&mut self) {
        self.peak.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Feed samples; returns the number of new frames analysed.
    pub fn push<S: Sample>(&mut self, x: &[S]) -> usize {
        let n = self.cfg.fft_size;
        let mut done = 0;
        for &s in x {
            self.ring[self.pos] = s.into();
            self.pos += 1;
            if self.pos == n {
                self.pos = 0;
            }
            if self.filled < n {
                self.filled += 1;
            }
            self.since += 1;
            if self.filled == n && self.since >= self.hop {
                self.since = 0;
                self.frame();
                done += 1;
            }
        }
        done
    }

    fn frame(&mut self) {
        let n = self.cfg.fft_size;
        for i in 0..n {
            self.buf[i] = self.ring[(self.pos + i) % n] * self.win[i];
        }
        if self
            .plan
            .process_with_scratch(&mut self.buf, &mut self.spec, &mut self.scratch)
            .is_err()
        {
            return;
        }
        let alpha = match self.cfg.averaging {
            Averaging::None => 1.0,
            Averaging::Exponential(t) => {
                1.0 - (-(self.hop as f64) / (self.cfg.sample_rate * t.max(1e-9))).exp()
            }
            Averaging::Infinite => 1.0 / (self.frames + 1) as f64,
        };
        let a = if self.frames == 0 { 1.0 } else { alpha };
        for k in 0..self.avg.len() {
            let p = self.spec[k].norm_sqr();
            self.avg[k] += a * (p - self.avg[k]);
            if self.avg[k] > self.peak[k] {
                self.peak[k] = self.avg[k];
            }
        }
        self.frames += 1;
    }

    fn amp_scale(&self, k: usize) -> f64 {
        let f = if k == 0 || k == self.cfg.fft_size / 2 {
            1.0
        } else {
            2.0
        };
        f / self.s1
    }

    /// Averaged amplitude spectrum in dBFS (peak-referenced, see module doc).
    pub fn amplitude_db(&self) -> Vec<f64> {
        self.amp_db_of(&self.avg)
    }
    /// Peak-hold amplitude spectrum in dBFS.
    pub fn peak_amplitude_db(&self) -> Vec<f64> {
        self.amp_db_of(&self.peak)
    }
    fn amp_db_of(&self, p: &[f64]) -> Vec<f64> {
        p.iter()
            .enumerate()
            .map(|(k, &v)| {
                let s = self.amp_scale(k);
                pow_to_db(v * s * s)
            })
            .collect()
    }
    /// Averaged density in dB re `FS^2/Hz` (see module doc).
    pub fn density_db(&self) -> Vec<f64> {
        let fs = self.cfg.sample_rate;
        self.avg
            .iter()
            .enumerate()
            .map(|(k, &v)| {
                let f = if k == 0 || k == self.cfg.fft_size / 2 {
                    1.0
                } else {
                    2.0
                };
                pow_to_db(f * v / (fs * self.s2))
            })
            .collect()
    }
    /// Averaged linear *amplitude-squared* spectrum (`A_k^2`), the domain in
    /// which smoothing should be done.
    pub fn amplitude_power(&self) -> Vec<f64> {
        self.avg
            .iter()
            .enumerate()
            .map(|(k, &v)| {
                let s = self.amp_scale(k);
                v * s * s
            })
            .collect()
    }
    /// Frequency of bin `k` in Hz.
    pub fn bin_frequency(&self, k: usize) -> f64 {
        k as f64 * self.bin_hz()
    }

    /// IEC 61260 third-octave band levels (class-1 band edges) in dB, a
    /// full-scale sine inside a band reads 0 dB. Bands with centres in
    /// `f_min..=f_max` that fit below Nyquist are returned.
    pub fn third_octave_db(&self, f_min: f64, f_max: f64) -> Vec<BandLevel> {
        let n = self.cfg.fft_size as f64;
        let df = self.bin_hz();
        // mean-square per bin: 2|X|^2/(N S2), DC and Nyquist single
        let ms: Vec<f64> = self
            .avg
            .iter()
            .enumerate()
            .map(|(k, &v)| {
                let f = if k == 0 || k == self.cfg.fft_size / 2 {
                    1.0
                } else {
                    2.0
                };
                f * v / (n * self.s2)
            })
            .collect();
        third_octave_centres(f_min, f_max)
            .into_iter()
            .filter(|b| b.f_hi <= self.cfg.sample_rate / 2.0)
            .map(|b| {
                let mut p = 0.0;
                let k0 = ((b.f_lo / df - 0.5).floor().max(0.0)) as usize;
                let k1 = (((b.f_hi / df) + 0.5).ceil() as usize).min(ms.len() - 1);
                for (k, &m) in ms.iter().enumerate().take(k1 + 1).skip(k0) {
                    let lo = (k as f64 - 0.5) * df;
                    let hi = (k as f64 + 0.5) * df;
                    let ov = (hi.min(b.f_hi) - lo.max(b.f_lo)).max(0.0) / df;
                    p += ov * m;
                }
                BandLevel {
                    level_db: pow_to_db(2.0 * p),
                    ..b
                }
            })
            .collect()
    }
}

/// One third-octave band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLevel {
    /// Exact (base-10) centre frequency `1000 * 10^(x/10)`.
    pub centre: f64,
    /// Nominal label frequency (20, 25, 31.5, ... 20000).
    pub nominal: f64,
    /// Lower band edge `centre * 10^(-0.05)`.
    pub f_lo: f64,
    /// Upper band edge `centre * 10^(0.05)`.
    pub f_hi: f64,
    /// Level in dB (0.0 for the list returned by [`third_octave_centres`]).
    pub level_db: f64,
}

/// IEC 61260-1 base-10 third-octave bands with exact centre frequencies
/// `1000 * 10^(x/10)` inside `f_min..=f_max` (level fields are 0).
pub fn third_octave_centres(f_min: f64, f_max: f64) -> Vec<BandLevel> {
    const MANT: [f64; 10] = [1.0, 1.25, 1.6, 2.0, 2.5, 3.15, 4.0, 5.0, 6.3, 8.0];
    let mut v = Vec::new();
    for x in -60i32..=40 {
        let fc = 1000.0 * 10f64.powf(x as f64 / 10.0);
        let dec = x.div_euclid(10);
        let nominal = MANT[x.rem_euclid(10) as usize] * 10f64.powi(dec + 3);
        if nominal < f_min * (1.0 - 1e-9) || nominal > f_max * (1.0 + 1e-9) {
            continue;
        }
        v.push(BandLevel {
            centre: fc,
            nominal,
            f_lo: fc * 10f64.powf(-0.05),
            f_hi: fc * 10f64.powf(0.05),
            level_db: 0.0,
        });
    }
    v
}

fn oct_range(k: usize, fraction: u32, len: usize) -> (usize, usize) {
    if k == 0 {
        return (0, 0);
    }
    let r = 2f64.powf(0.5 / fraction.max(1) as f64);
    let lo = ((k as f64 / r).round() as usize).min(k).max(1);
    let hi = ((k as f64 * r).round() as usize).max(k).min(len - 1);
    (lo, hi)
}

/// Fractional-octave smoothing (`1/fraction` octave, rectangular window of
/// width `2^(1/fraction)` centred geometrically on each bin) of a **linear
/// power** spectrum on a linear-frequency grid (bin `k` = `k * df`; `df`
/// cancels). Power is averaged, so a flat spectrum stays flat and total
/// level is preserved on average. DC is left untouched.
pub fn smooth_octave_power(power: &[f64], fraction: u32) -> Vec<f64> {
    let n = power.len();
    let mut pre = vec![0.0; n + 1];
    for i in 0..n {
        pre[i + 1] = pre[i] + power[i];
    }
    (0..n)
        .map(|k| {
            let (lo, hi) = oct_range(k, fraction, n);
            (pre[hi + 1] - pre[lo]) / (hi + 1 - lo) as f64
        })
        .collect()
}

/// As [`smooth_octave_power`] for a complex response (average of the complex
/// values, i.e. "complex smoothing", which keeps phase information).
pub fn smooth_octave_complex(values: &[Complex], fraction: u32) -> Vec<Complex> {
    let n = values.len();
    let mut pre = vec![Complex::new(0.0, 0.0); n + 1];
    for i in 0..n {
        pre[i + 1] = pre[i] + values[i];
    }
    (0..n)
        .map(|k| {
            let (lo, hi) = oct_range(k, fraction, n);
            (pre[hi + 1] - pre[lo]) / (hi + 1 - lo) as f64
        })
        .collect()
}

/// `n` log-spaced frequencies from `f_lo` to `f_hi` inclusive.
pub fn log_frequencies(f_lo: f64, f_hi: f64, n: usize) -> Vec<f64> {
    if n == 0 || f_lo <= 0.0 || f_hi <= f_lo {
        return Vec::new();
    }
    if n == 1 {
        return vec![f_lo];
    }
    (0..n)
        .map(|i| f_lo * (f_hi / f_lo).powf(i as f64 / (n - 1) as f64))
        .collect()
}

/// How several FFT bins falling into one display bin are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    /// Mean of the linear power values (RTA-like).
    Mean,
    /// Maximum (peak display).
    Max,
}

/// Map a linear-frequency spectrum (`values[k]` at `k*df`, linear
/// power-like quantities) onto `n` log-spaced display points between `f_lo`
/// and `f_hi`. Where a display point covers one or more bin centres they are
/// combined according to `mode`; where it covers none (low frequencies) the
/// value is linearly interpolated between the neighbouring bins.
pub fn resample_log_power(
    values: &[f64],
    df: f64,
    f_lo: f64,
    f_hi: f64,
    n: usize,
    mode: Combine,
) -> Vec<f64> {
    let fr = log_frequencies(f_lo, f_hi, n);
    if values.is_empty() || df <= 0.0 || fr.is_empty() {
        return Vec::new();
    }
    let ratio = if n > 1 {
        (f_hi / f_lo).powf(0.5 / (n - 1) as f64)
    } else {
        1.0
    };
    fr.iter()
        .map(|&f| {
            let a = (f / ratio / df).ceil().max(0.0) as usize;
            let b = ((f * ratio / df).floor() as usize).min(values.len() - 1);
            if a <= b {
                let s = &values[a..=b];
                match mode {
                    Combine::Mean => s.iter().sum::<f64>() / s.len() as f64,
                    Combine::Max => s.iter().cloned().fold(f64::MIN, f64::max),
                }
            } else {
                interp_linear(values, f / df)
            }
        })
        .collect()
}

/// Linear interpolation of `values` at fractional index `x` (clamped).
pub fn interp_linear(values: &[f64], x: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let x = x.clamp(0.0, (values.len() - 1) as f64);
    let i = x.floor() as usize;
    let j = (i + 1).min(values.len() - 1);
    let t = x - i as f64;
    values[i] * (1.0 - t) + values[j] * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{Generator, Sine, WhiteNoise};

    fn run(an: &mut SpectrumAnalyzer, g: &mut impl Generator, n: usize) {
        let mut b = vec![0.0; n];
        g.fill(&mut b);
        an.push(&b);
    }

    #[test]
    fn flat_top_sine_reads_0_dbfs_anywhere() {
        let fs = 48_000.0;
        let mut worst = 0.0f64;
        for f in [1000.0, 1000.3, 1003.9, 1007.0 + 0.5 * fs / 8192.0, 12_345.6] {
            let mut cfg = SpectrumConfig::new(fs, 8192, Window::FlatTop);
            cfg.averaging = Averaging::Infinite;
            let mut an = SpectrumAnalyzer::new(cfg).unwrap();
            run(&mut an, &mut Sine::new(fs, f, 0.0), 8192 * 3);
            let pk = an.amplitude_db().into_iter().fold(f64::MIN, f64::max);
            worst = worst.max(pk.abs());
        }
        eprintln!("flat-top worst amplitude error {worst:.5} dB");
        assert!(worst < 0.01);
    }

    #[test]
    fn hann_bh_bin_centre_sine() {
        let fs = 48_000.0;
        let f = 100.0 * fs / 8192.0;
        for w in [Window::Hann, Window::BlackmanHarris] {
            let mut an = SpectrumAnalyzer::new(SpectrumConfig::new(fs, 8192, w)).unwrap();
            run(&mut an, &mut Sine::new(fs, f, -6.0), 8192 * 4);
            let pk = an.amplitude_db().into_iter().fold(f64::MIN, f64::max);
            assert!((pk + 6.0).abs() < 0.001, "{w:?} {pk}");
        }
    }

    #[test]
    fn white_noise_density_and_band_levels() {
        let fs = 48_000.0;
        let sigma_db = -20.0;
        for w in [Window::Hann, Window::BlackmanHarris, Window::FlatTop] {
            let mut cfg = SpectrumConfig::new(fs, 16384, w);
            cfg.averaging = Averaging::Infinite;
            cfg.overlap = 0.75;
            let mut an = SpectrumAnalyzer::new(cfg).unwrap();
            run(&mut an, &mut WhiteNoise::new(sigma_db, 11), 16384 * 120);
            let d = an.density_db();
            let want = 10.0 * (2.0 * 0.01 / fs).log10();
            let m: f64 = d[100..8000]
                .iter()
                .map(|v| 10f64.powf(v / 10.0))
                .sum::<f64>()
                / 7900.0;
            let got = 10.0 * m.log10();
            assert!((got - want).abs() < 0.05, "{w:?} {got} {want}");
            // third-octave: power = sigma^2 * bw/(fs/2); sine-referenced +3.01
            for b in an.third_octave_db(100.0, 10_000.0) {
                let p = 0.01 * (b.f_hi - b.f_lo) / (fs / 2.0);
                let want = 10.0 * (2.0 * p).log10();
                assert!(
                    (b.level_db - want).abs() < 0.35,
                    "{} {} {}",
                    b.nominal,
                    b.level_db,
                    want
                );
            }
        }
    }

    #[test]
    fn third_octave_sine_and_centres() {
        let c = third_octave_centres(20.0, 20_000.0);
        assert_eq!(c.len(), 31);
        assert!((c[0].centre - 19.9526).abs() < 1e-3 && c[0].nominal == 20.0);
        assert!(c.iter().any(|b| b.nominal == 31.5) && c.iter().any(|b| b.nominal == 1000.0));
        assert!((c.last().unwrap().centre - 19952.6).abs() < 0.1);
        let fs = 48_000.0;
        let mut cfg = SpectrumConfig::new(fs, 16384, Window::FlatTop);
        cfg.averaging = Averaging::Infinite;
        let mut an = SpectrumAnalyzer::new(cfg).unwrap();
        run(&mut an, &mut Sine::new(fs, 1000.0, 0.0), 16384 * 4);
        let b = an.third_octave_db(20.0, 20_000.0);
        let k = b.iter().find(|b| b.nominal == 1000.0).unwrap();
        assert!(k.level_db.abs() < 0.01, "{}", k.level_db);
        assert!(b
            .iter()
            .filter(|x| x.nominal != 1000.0)
            .all(|x| x.level_db < -60.0));
    }

    #[test]
    fn smoothing_and_resampling() {
        let flat = vec![2.0; 4097];
        assert!(smooth_octave_power(&flat, 3)
            .iter()
            .all(|v| (v - 2.0).abs() < 1e-12));
        let mut spike = vec![0.0; 4097];
        spike[1000] = 1.0;
        let s = smooth_octave_power(&spike, 3);
        assert!(s[1000] < 0.1 && s[1000] > 0.0 && s[500] == 0.0);
        let r = resample_log_power(&flat, 10.0, 20.0, 20_000.0, 100, Combine::Mean);
        assert!(r.iter().all(|v| (v - 2.0).abs() < 1e-12));
        assert!(SpectrumAnalyzer::new(SpectrumConfig::new(48_000.0, 1000, Window::Hann)).is_none());
    }

    #[test]
    fn pink_noise_has_equal_third_octave_levels() {
        // constant power per fractional-octave band <=> -3 dB/oct density
        let fs = 48_000.0;
        let mut cfg = SpectrumConfig::new(fs, 16384, Window::BlackmanHarris);
        cfg.averaging = Averaging::Infinite;
        cfg.overlap = 0.75;
        let mut an = SpectrumAnalyzer::new(cfg).unwrap();
        run(
            &mut an,
            &mut crate::generator::PinkNoise::new(fs, -20.0, 3),
            48_000 * 60,
        );
        let b = an.third_octave_db(63.0, 16_000.0);
        let (lo, hi) = b.iter().fold((f64::MAX, f64::MIN), |a, x| {
            (a.0.min(x.level_db), a.1.max(x.level_db))
        });
        eprintln!("pink third-octave spread {:.3} dB", hi - lo);
        assert!(hi - lo < 0.3);
    }

    #[test]
    fn peak_hold_and_exponential() {
        let fs = 48_000.0;
        let mut an = {
            let mut c = SpectrumConfig::new(fs, 2048, Window::Hann);
            c.averaging = Averaging::Exponential(0.05);
            SpectrumAnalyzer::new(c).unwrap()
        };
        run(&mut an, &mut Sine::new(fs, 1000.0, 0.0), 8192);
        run(&mut an, &mut WhiteNoise::new(-100.0, 1), 48_000 * 3);
        let cur = an.amplitude_db().into_iter().fold(f64::MIN, f64::max);
        let pk = an.peak_amplitude_db().into_iter().fold(f64::MIN, f64::max);
        assert!(pk > -3.0 && cur < -50.0);
        an.reset_peak();
        assert!(an.peak_amplitude_db().into_iter().fold(f64::MIN, f64::max) < -300.0);
    }
}
