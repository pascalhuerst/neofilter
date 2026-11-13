//! Stereo metering: correlation, mid/side levels, balance, goniometer points,
//! and low-frequency (vinyl) correlation and vertical/lateral levels.
//!
//! Realtime: **all types are audio-thread safe** (allocation-free after
//! construction, panic-free `process`).
//!
//! Conventions: `M = (L + R) / 2`, `S = (L - R) / 2` (a mono signal gives
//! `M = L`, `S = 0`). For disc cutting the *lateral* (mono-compatible) motion
//! corresponds to `M` and the *vertical* motion to `S`. Levels are RMS in dB
//! (`20 log10 rms`, full-scale sine = -3.01 dB). All quantities are
//! integrated with a one-pole mean-square integrator of selectable time
//! constant.
//!
//! ```
//! use neo_measure::stereo::StereoMeter;
//! let mut m = StereoMeter::new(48_000.0, 0.3);
//! let l: Vec<f64> = (0..48_000).map(|i| (i as f64 * 0.05).sin()).collect();
//! m.process(&l, &l);
//! assert!((m.correlation() - 1.0).abs() < 1e-9);
//! ```

use crate::filter::LinkwitzRileyLp;
use crate::util::amp_to_db;
use crate::Sample;

/// Correlation / M-S / balance meter.
#[derive(Debug, Clone)]
pub struct StereoMeter {
    a: f64,
    ll: f64,
    rr: f64,
    lr: f64,
}

impl StereoMeter {
    /// `tau_s`: integration time constant in seconds.
    pub fn new(fs: f64, tau_s: f64) -> Self {
        Self {
            a: 1.0 - (-1.0 / (fs * tau_s.max(1e-9))).exp(),
            ll: 0.0,
            rr: 0.0,
            lr: 0.0,
        }
    }
    pub fn set_time_constant(&mut self, fs: f64, tau_s: f64) {
        self.a = 1.0 - (-1.0 / (fs * tau_s.max(1e-9))).exp();
    }
    pub fn reset(&mut self) {
        self.ll = 0.0;
        self.rr = 0.0;
        self.lr = 0.0;
    }
    #[inline]
    pub fn process_frame(&mut self, l: f64, r: f64) {
        let (l, r) = (
            if l.is_finite() { l } else { 0.0 },
            if r.is_finite() { r } else { 0.0 },
        );
        self.ll += self.a * (l * l - self.ll);
        self.rr += self.a * (r * r - self.rr);
        self.lr += self.a * (l * r - self.lr);
    }
    /// Process two equally long blocks (extra samples of the longer one are
    /// ignored).
    pub fn process<S: Sample>(&mut self, l: &[S], r: &[S]) {
        for (&a, &b) in l.iter().zip(r) {
            self.process_frame(a.into(), b.into());
        }
    }
    /// Correlation coefficient `-1..=1` (0.0 while a channel is silent).
    pub fn correlation(&self) -> f64 {
        let d = (self.ll * self.rr).sqrt();
        if d > 1e-30 {
            (self.lr / d).clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }
    pub fn left_db(&self) -> f64 {
        amp_to_db(self.ll.max(0.0).sqrt())
    }
    pub fn right_db(&self) -> f64 {
        amp_to_db(self.rr.max(0.0).sqrt())
    }
    /// Mean square of mid.
    pub fn mid_power(&self) -> f64 {
        (0.25 * (self.ll + self.rr + 2.0 * self.lr)).max(0.0)
    }
    /// Mean square of side.
    pub fn side_power(&self) -> f64 {
        (0.25 * (self.ll + self.rr - 2.0 * self.lr)).max(0.0)
    }
    pub fn mid_db(&self) -> f64 {
        amp_to_db(self.mid_power().sqrt())
    }
    pub fn side_db(&self) -> f64 {
        amp_to_db(self.side_power().sqrt())
    }
    /// `L - R` level difference in dB (positive: left louder); 0.0 if both
    /// channels are silent.
    pub fn balance_db(&self) -> f64 {
        if self.ll <= 0.0 && self.rr <= 0.0 {
            0.0
        } else {
            self.left_db() - self.right_db()
        }
    }
    /// Linear balance `-1` (left only) .. `+1` (right only) from RMS values.
    pub fn balance(&self) -> f64 {
        let (l, r) = (self.ll.max(0.0).sqrt(), self.rr.max(0.0).sqrt());
        if l + r > 0.0 {
            (r - l) / (r + l)
        } else {
            0.0
        }
    }
}

/// Decimated goniometer (phase scope) point buffer.
///
/// Every `decimation`-th frame is stored as `[x, y] = [(L-R)/sqrt2,
/// (L+R)/sqrt2]` (so mono is a vertical line, as on a classic goniometer).
#[derive(Debug, Clone)]
pub struct Goniometer {
    pts: Vec<[f32; 2]>,
    head: usize,
    len: usize,
    dec: usize,
    count: usize,
}

impl Goniometer {
    pub fn new(capacity: usize, decimation: usize) -> Self {
        Self {
            pts: vec![[0.0; 2]; capacity.max(1)],
            head: 0,
            len: 0,
            dec: decimation.max(1),
            count: 0,
        }
    }
    pub fn reset(&mut self) {
        self.head = 0;
        self.len = 0;
        self.count = 0;
    }
    pub fn process<S: Sample>(&mut self, l: &[S], r: &[S]) {
        let k = std::f64::consts::FRAC_1_SQRT_2;
        for (&a, &b) in l.iter().zip(r) {
            self.count += 1;
            if self.count >= self.dec {
                self.count = 0;
                let (a, b): (f64, f64) = (a.into(), b.into());
                self.pts[self.head] = [((a - b) * k) as f32, ((a + b) * k) as f32];
                self.head = (self.head + 1) % self.pts.len();
                self.len = (self.len + 1).min(self.pts.len());
            }
        }
    }
    /// Number of stored points.
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Copy the newest points (oldest first) into `out`; returns the count.
    pub fn copy_points(&self, out: &mut [[f32; 2]]) -> usize {
        let n = self.len.min(out.len());
        let cap = self.pts.len();
        for (i, o) in out.iter_mut().take(n).enumerate() {
            // newest n points: start at head - n
            *o = self.pts[(self.head + cap - n + i) % cap];
        }
        n
    }
}

/// Low-frequency stereo meter for disc cutting: correlation and
/// vertical (S) / lateral (M) level below a crossover (default 300 Hz,
/// 4th-order Linkwitz-Riley low-pass on each channel).
#[derive(Debug, Clone)]
pub struct LowFreqStereo {
    lp_l: LinkwitzRileyLp,
    lp_r: LinkwitzRileyLp,
    inner: StereoMeter,
}

impl LowFreqStereo {
    /// Default crossover of the vinyl workflow.
    pub const DEFAULT_CROSSOVER_HZ: f64 = 300.0;

    pub fn new(fs: f64, crossover_hz: f64, tau_s: f64) -> Self {
        Self {
            lp_l: LinkwitzRileyLp::new(fs, crossover_hz),
            lp_r: LinkwitzRileyLp::new(fs, crossover_hz),
            inner: StereoMeter::new(fs, tau_s),
        }
    }
    pub fn reset(&mut self) {
        self.lp_l.reset();
        self.lp_r.reset();
        self.inner.reset();
    }
    pub fn process<S: Sample>(&mut self, l: &[S], r: &[S]) {
        for (&a, &b) in l.iter().zip(r) {
            let (a, b): (f64, f64) = (a.into(), b.into());
            let fl = self.lp_l.process(if a.is_finite() { a } else { 0.0 });
            let fr = self.lp_r.process(if b.is_finite() { b } else { 0.0 });
            self.inner.process_frame(fl, fr);
        }
    }
    /// Correlation of the band below the crossover.
    pub fn correlation(&self) -> f64 {
        self.inner.correlation()
    }
    /// Lateral (mid) level below the crossover, dB.
    pub fn lateral_db(&self) -> f64 {
        self.inner.mid_db()
    }
    /// Vertical (side) level below the crossover, dB.
    pub fn vertical_db(&self) -> f64 {
        self.inner.side_db()
    }
    /// Vertical minus lateral level in dB (0 dB: equal vertical and lateral
    /// energy; large positive values warn of stylus-lifting bass).
    pub fn vertical_minus_lateral_db(&self) -> f64 {
        self.vertical_db() - self.lateral_db()
    }
    /// The underlying band-limited stereo meter.
    pub fn meter(&self) -> &StereoMeter {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{Generator, WhiteNoise};
    use std::f64::consts::PI;

    fn sine(f: f64, ph: f64, n: usize, fs: f64) -> Vec<f64> {
        (0..n)
            .map(|i| (2.0 * PI * f * i as f64 / fs + ph).sin())
            .collect()
    }

    #[test]
    fn correlation_cases() {
        let fs = 48_000.0;
        let a = sine(1000.0, 0.0, 96_000, fs);
        let neg: Vec<f64> = a.iter().map(|v| -v).collect();
        let quad = sine(1000.0, PI / 2.0, 96_000, fs);
        let mut m = StereoMeter::new(fs, 0.5);
        m.process(&a, &a);
        assert!((m.correlation() - 1.0).abs() < 1e-9);
        m.reset();
        m.process(&a, &neg);
        assert!((m.correlation() + 1.0).abs() < 1e-9);
        m.reset();
        m.process(&a, &quad);
        assert!(m.correlation().abs() < 0.01);
        // independent noise
        let mut g1 = WhiteNoise::new(-20.0, 1);
        let mut g2 = WhiteNoise::new(-20.0, 2);
        let n1: Vec<f64> = (0..480_000).map(|_| g1.next_sample()).collect();
        let n2: Vec<f64> = (0..480_000).map(|_| g2.next_sample()).collect();
        m.reset();
        m.process(&n1, &n2);
        assert!(m.correlation().abs() < 0.05);
        // known partial correlation: r = 0.6 l + 0.8 n -> rho = 0.6
        let r: Vec<f64> = n1.iter().zip(&n2).map(|(l, n)| 0.6 * l + 0.8 * n).collect();
        let mut m2 = StereoMeter::new(fs, 5.0);
        m2.process(&n1, &r);
        assert!(
            (m2.correlation() - 0.6).abs() < 0.03,
            "{}",
            m2.correlation()
        );
        // silence is neutral
        assert_eq!(StereoMeter::new(fs, 0.3).correlation(), 0.0);
    }

    #[test]
    fn ms_and_balance() {
        let fs = 48_000.0;
        let l = sine(500.0, 0.0, 96_000, fs);
        let r: Vec<f64> = l.iter().map(|v| 0.5 * v).collect();
        let mut m = StereoMeter::new(fs, 0.2);
        m.process(&l, &r);
        assert!((m.balance_db() - 6.0206).abs() < 0.01);
        assert!((m.balance() + 1.0 / 3.0).abs() < 0.01);
        // M = 0.75 sin, S = 0.25 sin
        assert!((m.mid_db() - 20.0 * (0.75 / 2f64.sqrt()).log10()).abs() < 0.01);
        assert!((m.side_db() - 20.0 * (0.25 / 2f64.sqrt()).log10()).abs() < 0.01);
        let mut mono = StereoMeter::new(fs, 0.2);
        mono.process(&l, &l);
        assert!(mono.side_db() < -200.0 && (mono.mid_db() - mono.left_db()).abs() < 1e-9);
    }

    #[test]
    fn goniometer_points() {
        let mut g = Goniometer::new(4, 2);
        let l = [
            1.0f32, 1.0, 0.5, 0.5, 0.2, 0.2, 0.1, 0.1, 0.0, 0.0, 0.3, 0.3,
        ];
        g.process(&l, &l); // mono -> x = 0
        assert_eq!(g.len(), 4);
        let mut out = [[0.0f32; 2]; 8];
        let n = g.copy_points(&mut out);
        assert_eq!(n, 4);
        assert!(out[..4].iter().all(|p| p[0].abs() < 1e-6));
        assert!((out[3][1] - 0.3 * 2f32.sqrt()).abs() < 1e-6); // newest last
        assert!((out[0][1] - 0.2 * 2f32.sqrt()).abs() < 1e-6);
        let mut side = Goniometer::new(8, 1);
        side.process(&[1.0f64], &[-1.0]);
        side.copy_points(&mut out);
        assert!((out[0][0] - 2f32.sqrt()).abs() < 1e-6 && out[0][1].abs() < 1e-6);
    }

    #[test]
    fn low_frequency_band() {
        let fs = 48_000.0;
        let n = 96_000;
        let lo = sine(80.0, 0.0, n, fs);
        let hi = sine(3000.0, 0.0, n, fs);
        // 80 Hz out of phase (vertical), 3 kHz in phase
        let l: Vec<f64> = lo.iter().zip(&hi).map(|(a, b)| a + b).collect();
        let r: Vec<f64> = lo.iter().zip(&hi).map(|(a, b)| -a + b).collect();
        let mut m = LowFreqStereo::new(fs, LowFreqStereo::DEFAULT_CROSSOVER_HZ, 0.2);
        m.process(&l, &r);
        assert!(m.correlation() < -0.999, "{}", m.correlation());
        assert!(m.vertical_minus_lateral_db() > 40.0);
        // broadband correlation would be ~0 here
        let mut b = StereoMeter::new(fs, 0.5);
        b.process(&l, &r);
        assert!(b.correlation().abs() < 0.05);
        // LF side level = 80 Hz sine amplitude 1 -> -3.01 dB (LR4 ~ unity there)
        assert!((m.vertical_db() + 3.01).abs() < 0.1, "{}", m.vertical_db());
        // LR4 is -6 dB at the crossover
        let mut f = LinkwitzRileyLp::new(fs, 300.0);
        let x = sine(300.0, 0.0, 48_000, fs);
        let y: Vec<f64> = x.iter().map(|&v| f.process(v)).collect();
        let pk = y[24_000..].iter().fold(0.0f64, |a, v| a.max(v.abs()));
        assert!((pk - 0.5).abs() < 0.005, "{pk}");
    }
}
