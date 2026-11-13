//! Transfer-function estimation between a reference `x` and a return `y`.
//!
//! `H1(f) = Sxy / Sxx` with Welch averaging, delay compensation (integer part
//! by stream alignment, fractional part as a phase ramp), magnitude, unwrapped
//! phase and magnitude-squared coherence.
//!
//! Realtime: **off-thread analysis** (allocates at construction and while
//! buffering in [`TransferEstimator::push`]).
//!
//! Conventions: `y[n] ~ (h * x)[n - d]`, with `d = cfg.delay_samples` (>= or <
//! 0). The reported phase is that of the system `h` alone (delay removed).
//! Coherence is the plain Welch estimate
//! `|Sxy|^2 / (Sxx Syy)`; with `M` frames it is biased upwards by roughly
//! `(1 - gamma^2)^2 / M`, so use plenty of frames (>= 50).
//!
//! Frequency resolution matters at sharp low-frequency features: the Hann
//! window smears the response over about +-2 bins (error grows with the
//! square of the bin spacing; 65536 points at 48 kHz give < 0.01 dB for a
//! 12 dB/octave corner at 30 Hz).
//!
//! The window should be Hann (default) or similar and the FFT frame must be
//! longer than the system's impulse response plus the residual delay error.
//!
//! ```
//! use neo_measure::transfer::*;
//! let x: Vec<f64> = (0..20000).map(|i| ((i * 7919 % 1009) as f64) - 504.0).collect();
//! let y: Vec<f64> = std::iter::repeat(0.0).take(10).chain(x.iter().map(|v| 0.5 * v)).collect();
//! let mut cfg = TransferConfig::new(48_000.0, 1024);
//! cfg.delay_samples = 10.0;
//! let r = estimate_transfer(&x, &y, &cfg).unwrap();
//! assert!((r.magnitude_db()[100] + 6.02).abs() < 0.01);
//! ```

use crate::spectrum::{smooth_octave_complex, smooth_octave_power};
use crate::util::{window_sums, Complex, Window};
use realfft::{RealFftPlanner, RealToComplex};
use std::f64::consts::PI;
use std::sync::Arc;

/// Estimator configuration.
#[derive(Debug, Clone, Copy)]
pub struct TransferConfig {
    pub sample_rate: f64,
    /// Even FFT size >= 64 (power of two recommended; 16384..65536 typical).
    pub fft_size: usize,
    /// Overlap fraction `0..=0.9`.
    pub overlap: f64,
    pub window: Window,
    /// Delay of `y` relative to `x` in samples, including the fraction.
    pub delay_samples: f64,
}

impl TransferConfig {
    /// Hann window, 50 % overlap, no delay.
    pub fn new(sample_rate: f64, fft_size: usize) -> Self {
        Self {
            sample_rate,
            fft_size,
            overlap: 0.5,
            window: Window::Hann,
            delay_samples: 0.0,
        }
    }
}

/// Streaming H1 estimator.
pub struct TransferEstimator {
    cfg: TransferConfig,
    plan: Arc<dyn RealToComplex<f64>>,
    win: Vec<f64>,
    hop: usize,
    d_int: i64,
    ramp: Vec<Complex>,
    xb: Vec<f64>,
    yb: Vec<f64>,
    x_base: i64,
    y_base: i64,
    next: i64,
    sxx: Vec<f64>,
    syy: Vec<f64>,
    sxy: Vec<Complex>,
    frames: u64,
    fx: Vec<f64>,
    fy: Vec<f64>,
    ox: Vec<Complex>,
    oy: Vec<Complex>,
    scratch: Vec<Complex>,
}

impl TransferEstimator {
    /// `None` for an invalid configuration.
    pub fn new(cfg: TransferConfig) -> Option<Self> {
        let n = cfg.fft_size;
        if n < 64
            || n % 2 != 0
            || !(0.0..=0.9).contains(&cfg.overlap)
            || !(cfg.sample_rate > 0.0)
            || !cfg.delay_samples.is_finite()
            || cfg.delay_samples.abs() > 1e9
        {
            return None;
        }
        let plan = RealFftPlanner::<f64>::new().plan_fft_forward(n);
        let d_int = cfg.delay_samples.floor() as i64;
        let frac = cfg.delay_samples - d_int as f64;
        let m = n / 2 + 1;
        let ramp = (0..m)
            .map(|k| Complex::from_polar(1.0, -2.0 * PI * k as f64 * frac / n as f64))
            .collect();
        let hop = (((1.0 - cfg.overlap) * n as f64).round() as usize).max(1);
        Some(Self {
            win: cfg.window.coefficients(n),
            plan: plan.clone(),
            hop,
            d_int,
            ramp,
            xb: Vec::new(),
            yb: Vec::new(),
            x_base: 0,
            y_base: 0,
            next: d_int.max(0),
            sxx: vec![0.0; m],
            syy: vec![0.0; m],
            sxy: vec![Complex::new(0.0, 0.0); m],
            frames: 0,
            fx: vec![0.0; n],
            fy: vec![0.0; n],
            ox: plan.make_output_vec(),
            oy: plan.make_output_vec(),
            scratch: plan.make_scratch_vec(),
            cfg,
        })
    }

    pub fn config(&self) -> &TransferConfig {
        &self.cfg
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Discard accumulated statistics and buffered input.
    pub fn reset(&mut self) {
        self.xb.clear();
        self.yb.clear();
        self.x_base = 0;
        self.y_base = 0;
        self.next = self.d_int.max(0);
        self.sxx.iter_mut().for_each(|v| *v = 0.0);
        self.syy.iter_mut().for_each(|v| *v = 0.0);
        self.sxy
            .iter_mut()
            .for_each(|v| *v = Complex::new(0.0, 0.0));
        self.frames = 0;
    }

    /// Push time-aligned-by-wall-clock blocks of reference and return (the
    /// return still contains the delay). Blocks may differ in length; the
    /// streams are indexed independently. Returns frames added.
    pub fn push(&mut self, x: &[f64], y: &[f64]) -> usize {
        self.xb.extend_from_slice(x);
        self.yb.extend_from_slice(y);
        let n = self.cfg.fft_size as i64;
        let mut done = 0;
        loop {
            let xs = self.next - self.d_int;
            if self.next + n > self.y_base + self.yb.len() as i64
                || xs + n > self.x_base + self.xb.len() as i64
            {
                break;
            }
            let xo = (xs - self.x_base) as usize;
            let yo = (self.next - self.y_base) as usize;
            for i in 0..n as usize {
                self.fx[i] = self.xb[xo + i] * self.win[i];
                self.fy[i] = self.yb[yo + i] * self.win[i];
            }
            if self
                .plan
                .process_with_scratch(&mut self.fx, &mut self.ox, &mut self.scratch)
                .is_ok()
                && self
                    .plan
                    .process_with_scratch(&mut self.fy, &mut self.oy, &mut self.scratch)
                    .is_ok()
            {
                for k in 0..self.ox.len() {
                    let xv = self.ox[k] * self.ramp[k];
                    let yv = self.oy[k];
                    self.sxx[k] += xv.norm_sqr();
                    self.syy[k] += yv.norm_sqr();
                    self.sxy[k] += yv * xv.conj();
                }
                self.frames += 1;
                done += 1;
            }
            self.next += self.hop as i64;
        }
        // trim consumed history
        let keep_y = self.next - self.y_base;
        if keep_y > 0 {
            let k = (keep_y as usize).min(self.yb.len());
            self.yb.drain(..k);
            self.y_base += k as i64;
        }
        let keep_x = self.next - self.d_int - self.x_base;
        if keep_x > 0 {
            let k = (keep_x as usize).min(self.xb.len());
            self.xb.drain(..k);
            self.x_base += k as i64;
        }
        done
    }

    /// Current estimate, or `None` before the first frame.
    pub fn result(&self) -> Option<TransferResult> {
        if self.frames == 0 {
            return None;
        }
        let n = self.cfg.fft_size;
        let df = self.cfg.sample_rate / n as f64;
        let mut h = Vec::with_capacity(self.sxx.len());
        let mut coh = Vec::with_capacity(self.sxx.len());
        for k in 0..self.sxx.len() {
            let (sxx, syy, sxy) = (self.sxx[k], self.syy[k], self.sxy[k]);
            if sxx > 0.0 {
                h.push(sxy / sxx);
            } else {
                h.push(Complex::new(0.0, 0.0));
            }
            coh.push(if sxx > 0.0 && syy > 0.0 {
                (sxy.norm_sqr() / (sxx * syy)).clamp(0.0, 1.0)
            } else {
                0.0
            });
        }
        let (_, _s2) = window_sums(&self.win);
        Some(TransferResult {
            df,
            h,
            coherence: coh,
            frames: self.frames,
        })
    }
}

/// One-shot estimate on slices (`y` still contains the delay).
pub fn estimate_transfer(x: &[f64], y: &[f64], cfg: &TransferConfig) -> Option<TransferResult> {
    let mut e = TransferEstimator::new(*cfg)?;
    e.push(x, y);
    e.result()
}

/// Result of a transfer-function estimate.
#[derive(Debug, Clone)]
pub struct TransferResult {
    /// Bin spacing in Hz.
    pub df: f64,
    /// Complex response per bin `0..=N/2`.
    pub h: Vec<Complex>,
    /// Magnitude-squared coherence per bin, `0..1`.
    pub coherence: Vec<f64>,
    /// Number of averaged frames.
    pub frames: u64,
}

impl TransferResult {
    /// Bin frequencies in Hz.
    pub fn frequencies(&self) -> Vec<f64> {
        (0..self.h.len()).map(|k| k as f64 * self.df).collect()
    }
    /// Magnitude in dB.
    pub fn magnitude_db(&self) -> Vec<f64> {
        mag_db(&self.h)
    }
    /// Unwrapped phase in radians (delay-compensated, see module doc).
    pub fn phase_rad(&self) -> Vec<f64> {
        unwrap_phase(&self.h)
    }
    /// Fractional-octave smoothed complex response ("complex smoothing").
    pub fn smoothed_complex(&self, fraction: u32) -> Vec<Complex> {
        smooth_octave_complex(&self.h, fraction)
    }
    /// Fractional-octave smoothed magnitude in dB, averaging `|H|^2`.
    pub fn smoothed_magnitude_db(&self, fraction: u32) -> Vec<f64> {
        let p: Vec<f64> = self.h.iter().map(|v| v.norm_sqr()).collect();
        smooth_octave_power(&p, fraction)
            .into_iter()
            .map(crate::util::pow_to_db)
            .collect()
    }
    /// Resample any per-bin quantity (e.g. `magnitude_db()`, `coherence`,
    /// `phase_rad()`) onto `n` log-spaced frequencies in `f_lo..=f_hi` by
    /// linear interpolation between bins. Returns `(freqs, values)`.
    pub fn on_log_grid(
        &self,
        values: &[f64],
        f_lo: f64,
        f_hi: f64,
        n: usize,
    ) -> (Vec<f64>, Vec<f64>) {
        let fr = crate::spectrum::log_frequencies(f_lo, f_hi, n);
        let v = fr
            .iter()
            .map(|&f| crate::spectrum::interp_linear(values, f / self.df))
            .collect();
        (fr, v)
    }
}

/// Magnitude in dB of complex values.
pub fn mag_db(h: &[Complex]) -> Vec<f64> {
    h.iter().map(|v| crate::util::amp_to_db(v.norm())).collect()
}

/// Phase unwrapping (starting from the principal value of the first bin).
pub fn unwrap_phase(h: &[Complex]) -> Vec<f64> {
    let mut out = Vec::with_capacity(h.len());
    let mut prev = 0.0;
    let mut off = 0.0;
    for (i, v) in h.iter().enumerate() {
        let p = v.arg();
        if i > 0 {
            let d = p + off - prev;
            off -= 2.0 * PI * (d / (2.0 * PI)).round();
        }
        let u = p + off;
        out.push(u);
        prev = u;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{Biquad, BiquadCoeffs};
    use crate::generator::{Generator, PinkNoise, WhiteNoise};
    use crate::util::{irfft, rfft};

    /// Delay by a (fractional) number of samples via FFT phase ramp.
    fn frac_delay(x: &[f64], d: f64) -> Vec<f64> {
        let n = (x.len() + d.ceil() as usize + 16).next_power_of_two();
        let mut s = rfft(x, n);
        let m = s.len() - 1;
        for (k, v) in s.iter_mut().enumerate() {
            *v *= Complex::from_polar(1.0, -PI * k as f64 * d / m as f64);
        }
        irfft(&s, n)
    }

    fn system(fs: f64) -> (Vec<Biquad>, impl Fn(f64) -> Complex) {
        let c = [
            BiquadCoeffs::lowpass(fs, 6000.0, 0.8),
            BiquadCoeffs::highpass(fs, 10.0, 0.7),
            BiquadCoeffs::lowpass(fs, 1200.0, 2.0),
        ];
        let secs = c.iter().map(|c| Biquad::new(*c)).collect();
        (secs, move |f| {
            c.iter()
                .fold(Complex::new(1.0, 0.0), |a, c| a * c.response(f, fs))
        })
    }

    #[test]
    fn noiseless_recovery_with_fractional_delay() {
        let fs = 48_000.0;
        let mut g = PinkNoise::new(fs, -20.0, 5);
        let mut x: Vec<f64> = (0..fs as usize * 10).map(|_| g.next_sample()).collect();
        x.extend(std::iter::repeat_n(0.0, 4000));
        let (mut secs, resp) = system(fs);
        let filtered: Vec<f64> = x
            .iter()
            .map(|&v| secs.iter_mut().fold(v, |a, s| s.process(a)))
            .collect();
        let d = 1234.37;
        let y = frac_delay(&filtered, d);
        let mut cfg = TransferConfig::new(fs, 65536);
        cfg.delay_samples = d;
        cfg.overlap = 0.75;
        let r = estimate_transfer(&x, &y, &cfg).unwrap();
        let mag = r.magnitude_db();
        let ph = r.phase_rad();
        let sm = r.smoothed_complex(6);
        let truth: Vec<Complex> = (0..r.h.len()).map(|k| resp(k as f64 * r.df)).collect();
        let tsm = smooth_octave_complex(&truth, 6);
        let (mut wm, mut wp, mut ws) = (0.0f64, 0.0f64, 0.0f64);
        for k in 1..r.h.len() {
            let f = k as f64 * r.df;
            if !(20.0..=20_000.0).contains(&f) {
                continue;
            }
            let t = resp(f);
            let e = (mag[k] - 20.0 * t.norm().log10()).abs();
            wm = wm.max(e);
            let dp = (ph[k] - t.arg() + PI).rem_euclid(2.0 * PI) - PI;
            wp = wp.max(dp.abs());
            // 1/6-octave complex smoothing of the estimate vs smoothed truth
            ws = ws.max((20.0 * sm[k].norm().log10() - 20.0 * tsm[k].norm().log10()).abs());
        }
        eprintln!(
            "transfer noiseless (20 Hz-20 kHz): per-bin mag err {wm:.4} dB, 1/6-oct smoothed {ws:.4} dB, phase err {:.3} deg",
            wp.to_degrees()
        );
        // per-bin: single-bin outliers where |X| dips randomly; smoothed: tight
        assert!(wm < 0.1 && ws < 0.01 && wp.to_degrees() < 1.0);
        assert!(r.coherence[100] > 0.999);
        // the unwrapped phase must be continuous (no 2pi jumps)
        let (k0, k1) = ((20.0 / r.df) as usize, (15_000.0 / r.df) as usize);
        assert!(ph[k0..k1].windows(2).all(|w| (w[1] - w[0]).abs() < 3.0));
    }

    #[test]
    fn coherence_matches_snr_theory() {
        let fs = 48_000.0;
        for snr_db in [0.0, 10.0, -5.0] {
            let n = fs as usize * 20;
            let mut gx = WhiteNoise::new(-20.0, 1);
            let x: Vec<f64> = (0..n).map(|_| gx.next_sample()).collect();
            let mut y: Vec<f64> = x.iter().map(|v| 0.5 * v).collect(); // H = -6.02 dB
            let ps = 0.25 * 0.01;
            let sn = (ps / 10f64.powf(snr_db / 10.0)).sqrt();
            let mut gn = WhiteNoise::new(0.0, 2);
            for v in y.iter_mut() {
                *v += sn * gn.next_sample();
            }
            let mut cfg = TransferConfig::new(fs, 4096);
            cfg.overlap = 0.5;
            let r = estimate_transfer(&x, &y, &cfg).unwrap();
            let snr = 10f64.powf(snr_db / 10.0);
            let theory = snr / (1.0 + snr);
            let mc: f64 = r.coherence[10..2000].iter().sum::<f64>() / 1990.0;
            let sm = r.smoothed_magnitude_db(3);
            let worst = sm[150..1900]
                .iter()
                .map(|v| (v + 6.0206).abs())
                .fold(0.0, f64::max);
            eprintln!("coh snr {snr_db}: {mc:.4} vs {theory:.4}, 1/3-oct mag err {worst:.3} dB");
            assert!((mc - theory).abs() < 0.01);
            assert!(worst < if snr_db >= 10.0 { 0.1 } else { 0.25 }, "{worst}");
        }
    }

    #[test]
    fn streaming_equals_oneshot_and_misc() {
        let fs = 48_000.0;
        let mut g = WhiteNoise::new(-10.0, 3);
        let x: Vec<f64> = (0..60_000).map(|_| g.next_sample()).collect();
        let y = frac_delay(&x, 300.5);
        let mut cfg = TransferConfig::new(fs, 2048);
        cfg.delay_samples = 300.5;
        let a = estimate_transfer(&x, &y, &cfg).unwrap();
        let mut e = TransferEstimator::new(cfg).unwrap();
        for (cx, cy) in x.chunks(777).zip(y.chunks(777)) {
            e.push(cx, cy);
        }
        let b = e.result().unwrap();
        assert_eq!(a.frames, b.frames);
        assert!((a.magnitude_db()[300] - b.magnitude_db()[300]).abs() < 1e-9);
        assert!(a.magnitude_db()[300].abs() < 0.01);
        let (f, v) = a.on_log_grid(&a.magnitude_db(), 20.0, 20_000.0, 50);
        assert_eq!((f.len(), v.len()), (50, 50));
        assert!(TransferEstimator::new(TransferConfig::new(48_000.0, 7)).is_none());
        assert!(TransferEstimator::new(cfg).unwrap().result().is_none());
    }
}
