//! Level meters: sample peak with hold/decay, true peak (ITU-R BS.1770-4),
//! RMS, crest factor and DC offset.
//!
//! Realtime: **all types in this module are audio-thread safe** (allocation
//! free after construction, panic free `process`). Construction allocates
//! (filter tables, windows).
//!
//! Level conventions: dBFS peak = `20 log10(|x|)` with full scale = 1.0.
//! RMS values are given both raw (`20 log10(rms)`, a full-scale sine reads
//! -3.01 dB) and sine-referenced (`+3.01 dB`, a full-scale sine reads 0 dB).
//!
//! ```
//! use neo_measure::levels::TruePeakMeter;
//! let mut tp = TruePeakMeter::new(48_000.0);
//! let x: Vec<f64> = (0..480).map(|n| 1.41421356 * (std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4).sin()).collect();
//! tp.process(&x);
//! tp.flush();
//! assert!((tp.true_peak_db() - 3.01).abs() < 0.1);
//! ```

use crate::util::amp_to_db;
use crate::Sample;
use std::f64::consts::PI;

/// Peak value with hold time and exponential (dB/s) decay.
#[derive(Debug, Clone)]
pub struct PeakHold {
    level: f64,
    hold_left: u64,
    hold: u64,
    decay: f64,
    max: f64,
}

impl PeakHold {
    /// `hold_s` seconds at the peak value, then decay of `decay_db_per_s`.
    pub fn new(fs: f64, hold_s: f64, decay_db_per_s: f64) -> Self {
        Self {
            level: 0.0,
            hold_left: 0,
            hold: (hold_s.max(0.0) * fs) as u64,
            decay: 10f64.powf(-decay_db_per_s.max(0.0) / 20.0 / fs),
            max: 0.0,
        }
    }
    /// Feed one non-negative value.
    #[inline]
    pub fn update(&mut self, v: f64) {
        let v = if v.is_finite() { v.abs() } else { 0.0 };
        if v >= self.level {
            self.level = v;
            self.hold_left = self.hold;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.level *= self.decay;
        }
        if v > self.max {
            self.max = v;
        }
    }
    /// Displayed (held / decaying) value, linear.
    pub fn level(&self) -> f64 {
        self.level
    }
    /// Largest value since construction or [`PeakHold::reset_max`].
    pub fn max(&self) -> f64 {
        self.max
    }
    pub fn reset_max(&mut self) {
        self.max = 0.0;
    }
    pub fn reset(&mut self) {
        self.level = 0.0;
        self.hold_left = 0;
        self.max = 0.0;
    }
}

/// Sample-peak meter with hold and decay.
#[derive(Debug, Clone)]
pub struct PeakMeter {
    hold: PeakHold,
    block: f64,
}

impl PeakMeter {
    pub fn new(fs: f64, hold_s: f64, decay_db_per_s: f64) -> Self {
        Self {
            hold: PeakHold::new(fs, hold_s, decay_db_per_s),
            block: 0.0,
        }
    }
    /// Process a block; returns the block's own sample peak (linear).
    pub fn process<S: Sample>(&mut self, x: &[S]) -> f64 {
        let mut b = 0.0f64;
        for &s in x {
            let v: f64 = s.into();
            let a = if v.is_finite() { v.abs() } else { 0.0 };
            b = b.max(a);
            self.hold.update(a);
        }
        self.block = b;
        b
    }
    /// Held/decayed peak, linear.
    pub fn peak(&self) -> f64 {
        self.hold.level()
    }
    pub fn peak_db(&self) -> f64 {
        amp_to_db(self.hold.level())
    }
    /// Peak of the last processed block, linear.
    pub fn block_peak(&self) -> f64 {
        self.block
    }
    /// Maximum since reset, dBFS.
    pub fn max_db(&self) -> f64 {
        amp_to_db(self.hold.max())
    }
    pub fn reset_max(&mut self) {
        self.hold.reset_max();
    }
    pub fn reset(&mut self) {
        self.hold.reset();
        self.block = 0.0;
    }
}

const TP_TAPS: usize = 32; // taps per phase

/// True-peak meter after ITU-R BS.1770-4 Annex 2 (single channel; run one per
/// channel and take the maximum).
///
/// Oversampling factor: 4x below 88.2 kHz, 2x for 88.2-176.3 kHz, 1x above
/// (as the standard prescribes for >= 96 kHz material, 2x is used at 96 kHz).
/// The interpolator is a 32-taps-per-phase Kaiser-windowed-sinc polyphase
/// filter (the standard's 48-tap example filter is a *suggestion*; this longer
/// design keeps the passband flat to about 0.4 fs and reads inter-sample
/// peaks of the 1/4-fs worst case within 0.01 dB). Phase 0 of the filter is an
/// exact identity, so the result is never below the sample peak.
///
/// Latency: `TP_TAPS/2` input samples; call [`TruePeakMeter::flush`] at the
/// end of a finite signal to evaluate the tail.
#[derive(Debug, Clone)]
pub struct TruePeakMeter {
    l: usize,
    coef: Vec<f64>, // l * TP_TAPS
    ring: Vec<f64>, // 2 * TP_TAPS
    pos: usize,
    hold: PeakHold,
    block: f64,
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-18 * sum {
            break;
        }
    }
    sum
}

impl TruePeakMeter {
    pub fn new(fs: f64) -> Self {
        Self::with_hold(fs, 0.0, 0.0)
    }
    pub fn with_hold(fs: f64, hold_s: f64, decay_db_per_s: f64) -> Self {
        let l = if fs < 88_000.0 {
            4
        } else if fs < 176_000.0 {
            2
        } else {
            1
        };
        let beta = 7.5;
        let half = TP_TAPS as f64 / 2.0;
        let mut coef = vec![0.0; l * TP_TAPS];
        for p in 0..l {
            let mut sum = 0.0;
            for i in 0..TP_TAPS {
                let t = i as f64 - half + 1.0 - p as f64 / l as f64;
                let r = t / half;
                let w = if r.abs() < 1.0 {
                    bessel_i0(beta * (1.0 - r * r).sqrt()) / bessel_i0(beta)
                } else {
                    0.0
                };
                let s = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * t).sin() / (PI * t)
                };
                coef[p * TP_TAPS + i] = s * w;
                sum += s * w;
            }
            if p != 0 {
                for i in 0..TP_TAPS {
                    coef[p * TP_TAPS + i] /= sum;
                }
            } else {
                for i in 0..TP_TAPS {
                    let t = i as f64 - half + 1.0;
                    coef[i] = if t.abs() < 1e-12 { 1.0 } else { 0.0 };
                }
            }
        }
        Self {
            l,
            coef,
            ring: vec![0.0; 2 * TP_TAPS],
            pos: 0,
            hold: PeakHold::new(fs, hold_s, decay_db_per_s),
            block: 0.0,
        }
    }

    /// Oversampling factor in use.
    pub fn oversampling(&self) -> usize {
        self.l
    }

    #[inline]
    fn push(&mut self, v: f64) -> f64 {
        let v = if v.is_finite() { v } else { 0.0 };
        self.ring[self.pos] = v;
        self.ring[self.pos + TP_TAPS] = v;
        self.pos = (self.pos + 1) % TP_TAPS;
        // oldest sample is at ring[pos]; window = ring[pos..pos+TP_TAPS]
        let w = &self.ring[self.pos..self.pos + TP_TAPS];
        let mut peak = 0.0f64;
        for p in 0..self.l {
            let c = &self.coef[p * TP_TAPS..(p + 1) * TP_TAPS];
            let y: f64 = c.iter().zip(w).map(|(a, b)| a * b).sum();
            peak = peak.max(y.abs());
        }
        peak
    }

    /// Process a block; returns the block's true peak (linear).
    pub fn process<S: Sample>(&mut self, x: &[S]) -> f64 {
        let mut b = 0.0f64;
        for &s in x {
            let p = self.push(s.into());
            b = b.max(p);
            self.hold.update(p);
        }
        self.block = b;
        b
    }

    /// Push `TP_TAPS/2` zeros so the last real samples are evaluated.
    pub fn flush(&mut self) {
        for _ in 0..TP_TAPS / 2 {
            let p = self.push(0.0);
            self.hold.update(p);
        }
    }

    /// Held/decayed true peak, linear.
    pub fn true_peak(&self) -> f64 {
        self.hold.level()
    }
    /// Maximum true peak since reset, linear.
    pub fn max_true_peak(&self) -> f64 {
        self.hold.max()
    }
    /// Maximum true peak since reset in dBTP.
    pub fn true_peak_db(&self) -> f64 {
        amp_to_db(self.hold.max())
    }
    /// True peak of the last block, linear.
    pub fn block_peak(&self) -> f64 {
        self.block
    }
    pub fn reset_max(&mut self) {
        self.hold.reset_max();
    }
    pub fn reset(&mut self) {
        self.hold.reset();
        self.ring.iter_mut().for_each(|v| *v = 0.0);
        self.block = 0.0;
    }
}

/// True peak of a finite buffer in dBTP.
pub fn true_peak_db<S: Sample>(x: &[S], fs: f64) -> f64 {
    let mut m = TruePeakMeter::new(fs);
    m.process(x);
    m.flush();
    m.true_peak_db()
}

/// Sample peak (linear) of a buffer.
pub fn sample_peak<S: Sample>(x: &[S]) -> f64 {
    x.iter().fold(0.0, |a, &s| {
        let v: f64 = s.into();
        if v.is_finite() {
            a.max(v.abs())
        } else {
            a
        }
    })
}

/// RMS of a buffer (0.0 for empty input).
pub fn rms<S: Sample>(x: &[S]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&s| s.into().powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

/// DC offset (mean) of a buffer.
pub fn dc_offset<S: Sample>(x: &[S]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().map(|&s| -> f64 { s.into() }).sum::<f64>() / x.len() as f64
}

/// Crest factor (peak / RMS) of a buffer in dB; 0.0 for silence.
pub fn crest_factor_db<S: Sample>(x: &[S]) -> f64 {
    let r = rms(x);
    if r > 0.0 {
        amp_to_db(sample_peak(x) / r)
    } else {
        0.0
    }
}

/// RMS meter with a sliding rectangular window or an exponential time
/// constant.
#[derive(Debug, Clone)]
pub struct RmsMeter {
    mode: RmsMode,
    sum: f64,
    acc: f64,
}

#[derive(Debug, Clone)]
enum RmsMode {
    Window {
        buf: Vec<f64>,
        pos: usize,
        since: usize,
    },
    Exp {
        a: f64,
    },
}

impl RmsMeter {
    /// Sliding window of `window_s` seconds (at least one sample).
    pub fn window(fs: f64, window_s: f64) -> Self {
        let n = ((window_s * fs).round() as usize).max(1);
        Self {
            mode: RmsMode::Window {
                buf: vec![0.0; n],
                pos: 0,
                since: 0,
            },
            sum: 0.0,
            acc: 0.0,
        }
    }
    /// One-pole mean-square integrator with time constant `tau_s`.
    pub fn exponential(fs: f64, tau_s: f64) -> Self {
        Self {
            mode: RmsMode::Exp {
                a: 1.0 - (-1.0 / (fs * tau_s.max(1e-9))).exp(),
            },
            sum: 0.0,
            acc: 0.0,
        }
    }
    pub fn process<S: Sample>(&mut self, x: &[S]) {
        for &s in x {
            let v: f64 = s.into();
            let q = if v.is_finite() { v * v } else { 0.0 };
            match &mut self.mode {
                RmsMode::Window { buf, pos, since } => {
                    self.sum += q - buf[*pos];
                    buf[*pos] = q;
                    *pos += 1;
                    if *pos == buf.len() {
                        *pos = 0;
                        // recompute to remove accumulated rounding drift
                        self.sum = buf.iter().sum();
                    }
                    *since = (*since + 1).min(buf.len());
                }
                RmsMode::Exp { a } => {
                    self.acc += *a * (q - self.acc);
                }
            }
        }
    }
    /// Mean square (linear power).
    pub fn mean_square(&self) -> f64 {
        match &self.mode {
            RmsMode::Window { buf, .. } => (self.sum / buf.len() as f64).max(0.0),
            RmsMode::Exp { .. } => self.acc.max(0.0),
        }
    }
    pub fn rms(&self) -> f64 {
        self.mean_square().sqrt()
    }
    /// `20 log10(rms)`; a full-scale sine reads -3.01 dB.
    pub fn rms_db(&self) -> f64 {
        amp_to_db(self.rms())
    }
    /// Sine-referenced: a full-scale sine reads 0 dB.
    pub fn rms_db_sine_ref(&self) -> f64 {
        self.rms_db() + 10.0 * 2f64.log10()
    }
    pub fn reset(&mut self) {
        self.sum = 0.0;
        self.acc = 0.0;
        if let RmsMode::Window { buf, pos, since } = &mut self.mode {
            buf.iter_mut().for_each(|v| *v = 0.0);
            *pos = 0;
            *since = 0;
        }
    }
}

/// Running statistics since the last reset: peak, RMS, DC offset, crest
/// factor.
#[derive(Debug, Clone, Default)]
pub struct BlockStats {
    n: u64,
    sum: f64,
    sumsq: f64,
    peak: f64,
}

impl BlockStats {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn process<S: Sample>(&mut self, x: &[S]) {
        for &s in x {
            let v: f64 = s.into();
            if !v.is_finite() {
                continue;
            }
            self.n += 1;
            self.sum += v;
            self.sumsq += v * v;
            self.peak = self.peak.max(v.abs());
        }
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn peak(&self) -> f64 {
        self.peak
    }
    /// Total RMS including DC.
    pub fn rms(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            (self.sumsq / self.n as f64).sqrt()
        }
    }
    /// DC offset (mean value).
    pub fn dc_offset(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum / self.n as f64
        }
    }
    /// RMS with the DC component removed.
    pub fn rms_ac(&self) -> f64 {
        let dc = self.dc_offset();
        (self.rms().powi(2) - dc * dc).max(0.0).sqrt()
    }
    /// Peak / RMS in dB (0.0 if silent).
    pub fn crest_factor_db(&self) -> f64 {
        let r = self.rms();
        if r > 0.0 {
            amp_to_db(self.peak / r)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

    /// Raised-cosine fade in/out (an abrupt onset would itself overshoot by
    /// the Gibbs phenomenon and legitimately read higher).
    fn faded(mut x: Vec<f64>) -> Vec<f64> {
        let n = x.len();
        for i in 0..300 {
            let g = 0.5 - 0.5 * (PI * i as f64 / 300.0).cos();
            x[i] *= g;
            x[n - 1 - i] *= g;
        }
        x
    }

    fn quarter_fs(amp: f64, n: usize) -> Vec<f64> {
        faded(
            (0..n)
                .map(|i| amp * (FRAC_PI_2 * i as f64 + FRAC_PI_4).sin())
                .collect(),
        )
    }

    #[test]
    fn true_peak_quarter_fs_45deg() {
        for (fs, ph_scale) in [
            (44_100.0, 1.0),
            (48_000.0, 1.0),
            (88_200.0, 1.0),
            (96_000.0, 1.0),
        ] {
            // at 2x the worst phase is still exact for fs/4 (samples at 0.5 offsets are hit)
            let _ = ph_scale;
            let x = quarter_fs(std::f64::consts::SQRT_2, 4800);
            assert!((sample_peak(&x) - 1.0).abs() < 1e-9);
            let tp = true_peak_db(&x, fs);
            eprintln!("true peak fs/4@45 at {fs}: {tp:.4} dBTP (expect 3.0103)");
            assert!((tp - 3.0103).abs() < 0.1, "{fs}: {tp}");
        }
        // -6 dBFS sample peak -> -3 dBTP
        let x = quarter_fs(std::f64::consts::SQRT_2 * 0.5, 4800);
        assert!((true_peak_db(&x, 48_000.0) + 2.99).abs() < 0.1);
        // 1x above 176 kHz: equals sample peak
        assert_eq!(TruePeakMeter::new(192_000.0).oversampling(), 1);
        assert_eq!(TruePeakMeter::new(96_000.0).oversampling(), 2);
    }

    #[test]
    fn true_peak_other_cases() {
        let fs = 48_000.0;
        // sine at 997 Hz, 0 dBFS peak anywhere in phase: 0 dBTP within 0.01 dB
        for ph in [0.0, 0.4, 1.0, 2.0] {
            let x = faded(
                (0..48_000)
                    .map(|i| (2.0 * PI * 997.0 * i as f64 / fs + ph).sin())
                    .collect(),
            );
            let tp = true_peak_db(&x, fs);
            assert!(tp.abs() < 0.01 && tp > -0.001, "{tp}");
        }
        // alternating +1/-1 full scale (fs/2): sample peak == true peak
        let alt = faded(
            (0..1000)
                .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
                .collect(),
        );
        let tp = true_peak_db(&alt, fs);
        assert!((-0.001..0.1).contains(&tp), "{tp}");
        // at 20 kHz (passband edge) the 48 kHz meter still reads within 0.5 dB
        let x = faded(
            (0..4800)
                .map(|i| (2.0 * PI * 20_000.0 * i as f64 / fs + 0.3).sin())
                .collect(),
        );
        assert!(true_peak_db(&x, fs).abs() < 0.5);
        // never below sample peak, f32 input
        let x32: Vec<f32> = (0..1000)
            .map(|i| ((i * 37) % 100) as f32 / 100.0 - 0.5)
            .collect();
        assert!(true_peak_db(&x32, fs) >= amp_to_db(sample_peak(&x32)) - 1e-9);
        // empty / nonfinite are safe
        assert!(true_peak_db::<f64>(&[], fs) < -300.0);
        assert!(true_peak_db(&[f64::NAN, 0.5], fs).is_finite());
    }

    #[test]
    fn peak_hold_decay() {
        let mut m = PeakMeter::new(1000.0, 0.1, 20.0);
        m.process(&[0.0, 1.0, 0.0]);
        assert_eq!(m.peak(), 1.0);
        m.process(&vec![0.0f32; 50]);
        assert_eq!(m.peak(), 1.0); // still held
        m.process(&vec![0.0f32; 1000]); // 1 s of decay at 20 dB/s
        assert!((m.peak_db() + 19.02).abs() < 0.05, "{}", m.peak_db());
        assert_eq!(m.max_db(), 0.0);
    }

    #[test]
    fn rms_and_stats() {
        let fs = 48_000.0;
        let x: Vec<f64> = (0..48_000)
            .map(|i| 0.5 * (2.0 * PI * 1000.0 * i as f64 / fs).sin() + 0.1)
            .collect();
        let mut w = RmsMeter::window(fs, 0.1);
        w.process(&x);
        let want = (0.125f64 + 0.01).sqrt();
        assert!((w.rms() - want).abs() < 1e-6);
        let mut e = RmsMeter::exponential(fs, 0.05);
        e.process(&x);
        assert!((e.rms() - want).abs() < 1e-3);
        let mut s = BlockStats::new();
        s.process(&x);
        assert!((s.dc_offset() - 0.1).abs() < 1e-9);
        assert!((s.rms_ac() - 0.5 / 2f64.sqrt()).abs() < 1e-6);
        assert!((s.peak() - 0.6).abs() < 1e-3);
        let sine: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * 1000.0 * i as f64 / fs).sin() as f32)
            .collect();
        assert!((crest_factor_db(&sine) - 3.0103).abs() < 0.01);
        let mut r = RmsMeter::window(fs, 0.05);
        r.process(&sine);
        assert!((r.rms_db_sine_ref()).abs() < 0.001);
        assert_eq!(RmsMeter::window(fs, 0.1).rms(), 0.0);
        assert_eq!(rms::<f64>(&[]), 0.0);
        assert_eq!(dc_offset::<f64>(&[]), 0.0);
    }
}
