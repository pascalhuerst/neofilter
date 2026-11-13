//! Loudness per ITU-R BS.1770-4 and EBU R128 / Tech 3341 / Tech 3342:
//! K-weighting for any sample rate, momentary (400 ms), short-term (3 s) and
//! integrated loudness (absolute -70 LUFS and relative -10 LU gating) and
//! loudness range (LRA).
//!
//! Realtime: [`KWeighting`] is allocation-free. [`LoudnessMeter`] allocates
//! only at construction and when the gating history (one `f64` per 100 ms for
//! the gated blocks and one for short-term values) outgrows the reserved
//! capacity (default: 1 hour, ~0.6 MB); `process_*` never panics. The
//! statistics queries ([`LoudnessMeter::integrated_lufs`],
//! [`LoudnessMeter::loudness_range`]) allocate and should run off the audio
//! thread.
//!
//! Channels: mono (weight 1) and stereo (weights 1, 1). Block hop is 100 ms
//! (`round(0.1 fs)` samples), i.e. 75 % overlap of 400 ms gating blocks.
//! Loudness values are `-0.691 + 10 log10(sum_i G_i z_i)`; "no data yet" or
//! silence is reported as `f64::NEG_INFINITY` by the plain getters.
//!
//! ```
//! use neo_measure::loudness::LoudnessMeter;
//! let mut m = LoudnessMeter::new(48_000.0, 2);
//! let s: Vec<f64> = (0..48_000 * 5).map(|i| 0.0707946 * (2.0*std::f64::consts::PI*1000.0*i as f64/48_000.0).sin()).collect();
//! m.process_stereo(&s, &s);
//! assert!((m.integrated_lufs().unwrap() + 23.0).abs() < 0.1);
//! ```

use crate::filter::{Biquad, BiquadCoeffs};
use crate::Sample;
use std::f64::consts::PI;

/// Offset of the BS.1770 loudness formula.
pub const LKFS_OFFSET: f64 = -0.691;

const SHELF_F0: f64 = 1_681.974_450_955_533;
const SHELF_G_DB: f64 = 3.999_843_853_973_347;
const SHELF_Q: f64 = 0.707_175_236_955_419_6;
const HP_F0: f64 = 38.135_470_876_024_44;
const HP_Q: f64 = 0.500_327_037_323_877_3;

/// Stage-1 (high-shelf "pre-filter") coefficients for sample rate `fs`.
pub fn shelf_coeffs(fs: f64) -> BiquadCoeffs {
    let k = (PI * SHELF_F0 / fs).tan();
    let vh = 10f64.powf(SHELF_G_DB / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / SHELF_Q + k * k;
    BiquadCoeffs {
        b0: (vh + vb * k / SHELF_Q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / SHELF_Q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / SHELF_Q + k * k) / a0,
    }
}

/// Stage-2 (revised low-frequency B-curve high-pass) coefficients.
pub fn highpass_coeffs(fs: f64) -> BiquadCoeffs {
    let k = (PI * HP_F0 / fs).tan();
    let a0 = 1.0 + k / HP_Q + k * k;
    BiquadCoeffs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / HP_Q + k * k) / a0,
    }
}

/// The two-stage K-weighting filter (one channel).
#[derive(Debug, Clone, Copy)]
pub struct KWeighting {
    shelf: Biquad,
    hp: Biquad,
}

impl KWeighting {
    pub fn new(fs: f64) -> Self {
        Self {
            shelf: Biquad::new(shelf_coeffs(fs)),
            hp: Biquad::new(highpass_coeffs(fs)),
        }
    }
    pub fn reset(&mut self) {
        self.shelf.reset();
        self.hp.reset();
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        self.hp.process(self.shelf.process(x))
    }
    /// Magnitude response in dB at `freq`.
    pub fn response_db(fs: f64, freq: f64) -> f64 {
        20.0 * (shelf_coeffs(fs).response(freq, fs) * highpass_coeffs(fs).response(freq, fs))
            .norm()
            .log10()
    }
}

/// Loudness range result (EBU Tech 3342).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessRange {
    /// LRA in LU (`high - low`).
    pub lra: f64,
    /// 10th percentile of the gated short-term loudness, LUFS.
    pub low: f64,
    /// 95th percentile, LUFS.
    pub high: f64,
}

fn lufs(e: f64) -> f64 {
    if e > 0.0 {
        LKFS_OFFSET + 10.0 * e.log10()
    } else {
        f64::NEG_INFINITY
    }
}

const SUB_MOMENTARY: usize = 4;
const SUB_SHORT: usize = 30;

/// Mono/stereo loudness meter.
#[derive(Debug, Clone)]
pub struct LoudnessMeter {
    channels: usize,
    kw: [KWeighting; 2],
    hop: usize,
    count: usize,
    acc: f64,
    ring: [f64; SUB_SHORT],
    pos: usize,
    n_sub: u64,
    blocks: Vec<f64>,
    short: Vec<f64>,
    momentary: f64,
    short_term: f64,
    max_momentary: f64,
    max_short: f64,
}

impl LoudnessMeter {
    /// `channels`: 1 or 2 (other values are clamped).
    pub fn new(fs: f64, channels: usize) -> Self {
        Self::with_capacity(fs, channels, 3600.0)
    }
    /// Reserve history for `seconds` of programme (no allocation within it).
    pub fn with_capacity(fs: f64, channels: usize, seconds: f64) -> Self {
        let cap = (seconds.max(0.0) * 10.0) as usize + 8;
        Self {
            channels: channels.clamp(1, 2),
            kw: [KWeighting::new(fs), KWeighting::new(fs)],
            hop: ((0.1 * fs).round() as usize).max(1),
            count: 0,
            acc: 0.0,
            ring: [0.0; SUB_SHORT],
            pos: 0,
            n_sub: 0,
            blocks: Vec::with_capacity(cap),
            short: Vec::with_capacity(cap),
            momentary: 0.0,
            short_term: 0.0,
            max_momentary: 0.0,
            max_short: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.kw.iter_mut().for_each(KWeighting::reset);
        self.count = 0;
        self.acc = 0.0;
        self.ring = [0.0; SUB_SHORT];
        self.pos = 0;
        self.n_sub = 0;
        self.blocks.clear();
        self.short.clear();
        self.momentary = 0.0;
        self.short_term = 0.0;
        self.max_momentary = 0.0;
        self.max_short = 0.0;
    }

    #[inline]
    fn frame(&mut self, l: f64, r: f64) {
        let yl = self.kw[0].process(if l.is_finite() { l } else { 0.0 });
        self.acc += yl * yl;
        if self.channels == 2 {
            let yr = self.kw[1].process(if r.is_finite() { r } else { 0.0 });
            self.acc += yr * yr;
        }
        self.count += 1;
        if self.count >= self.hop {
            let sub = self.acc / self.hop as f64;
            self.count = 0;
            self.acc = 0.0;
            self.end_subblock(sub);
        }
    }

    fn mean_last(&self, n: usize) -> f64 {
        let mut s = 0.0;
        for i in 0..n {
            s += self.ring[(self.pos + SUB_SHORT - 1 - i) % SUB_SHORT];
        }
        s / n as f64
    }

    fn end_subblock(&mut self, sub: f64) {
        self.ring[self.pos] = sub;
        self.pos = (self.pos + 1) % SUB_SHORT;
        self.n_sub += 1;
        if self.n_sub >= SUB_MOMENTARY as u64 {
            let z = self.mean_last(SUB_MOMENTARY);
            self.momentary = z;
            self.max_momentary = self.max_momentary.max(z);
            self.blocks.push(z);
        }
        if self.n_sub >= SUB_SHORT as u64 {
            let z = self.mean_last(SUB_SHORT);
            self.short_term = z;
            self.max_short = self.max_short.max(z);
            self.short.push(z);
        }
    }

    /// Process a mono block (only for a 1-channel meter).
    pub fn process_mono<S: Sample>(&mut self, x: &[S]) {
        for &s in x {
            self.frame(s.into(), 0.0);
        }
    }
    /// Process a stereo block (right is ignored by a 1-channel meter).
    pub fn process_stereo<S: Sample>(&mut self, l: &[S], r: &[S]) {
        for (&a, &b) in l.iter().zip(r) {
            self.frame(a.into(), b.into());
        }
    }

    /// Momentary loudness (last 400 ms), updated every 100 ms.
    pub fn momentary_lufs(&self) -> f64 {
        lufs(self.momentary)
    }
    /// Short-term loudness (last 3 s), updated every 100 ms.
    pub fn short_term_lufs(&self) -> f64 {
        lufs(self.short_term)
    }
    pub fn max_momentary_lufs(&self) -> f64 {
        lufs(self.max_momentary)
    }
    pub fn max_short_term_lufs(&self) -> f64 {
        lufs(self.max_short)
    }

    /// Integrated loudness with absolute (-70 LUFS) and relative (-10 LU)
    /// gating; `None` until a block passes the absolute gate.
    pub fn integrated_lufs(&self) -> Option<f64> {
        let abs_thr = 10f64.powf((-70.0 - LKFS_OFFSET) / 10.0);
        let (mut s, mut n) = (0.0, 0usize);
        for &z in &self.blocks {
            if z > abs_thr {
                s += z;
                n += 1;
            }
        }
        if n == 0 {
            return None;
        }
        let rel_thr = (s / n as f64) * 0.1;
        let (mut s2, mut n2) = (0.0, 0usize);
        for &z in &self.blocks {
            if z > abs_thr && z > rel_thr {
                s2 += z;
                n2 += 1;
            }
        }
        if n2 == 0 {
            None
        } else {
            Some(lufs(s2 / n2 as f64))
        }
    }

    /// Loudness range (EBU Tech 3342): short-term values gated at -70 LUFS
    /// absolute and -20 LU relative; 10th to 95th percentile (linear
    /// interpolation between ranks). `None` if there is no data.
    pub fn loudness_range(&self) -> Option<LoudnessRange> {
        let abs_thr = 10f64.powf((-70.0 - LKFS_OFFSET) / 10.0);
        let g: Vec<f64> = self
            .short
            .iter()
            .copied()
            .filter(|&z| z > abs_thr)
            .collect();
        if g.is_empty() {
            return None;
        }
        let rel_thr = g.iter().sum::<f64>() / g.len() as f64 * 0.01;
        let mut v: Vec<f64> = g.into_iter().filter(|&z| z > rel_thr).map(lufs).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pct = |p: f64| {
            let x = p * (v.len() - 1) as f64;
            let i = x.floor() as usize;
            let j = (i + 1).min(v.len() - 1);
            v[i] + (x - i as f64) * (v[j] - v[i])
        };
        let (low, high) = (pct(0.10), pct(0.95));
        Some(LoudnessRange {
            lra: high - low,
            low,
            high,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stereo 1 kHz sine of the given loudness (identical channels, so the
    /// peak level in dBFS equals the loudness in LUFS) for `secs`.
    fn seg(fs: f64, lufs_level: f64, secs: f64, t0: &mut usize) -> Vec<f64> {
        let a = 10f64.powf(lufs_level / 20.0);
        let n = (secs * fs) as usize;
        let v = (0..n)
            .map(|i| a * (2.0 * PI * 1000.0 * (*t0 + i) as f64 / fs).sin())
            .collect();
        *t0 += n;
        v
    }

    fn run(fs: f64, parts: &[(f64, f64)]) -> LoudnessMeter {
        let mut m = LoudnessMeter::new(fs, 2);
        let mut t = 0;
        for &(l, s) in parts {
            let x = seg(fs, l, s, &mut t);
            m.process_stereo(&x, &x);
        }
        m
    }

    #[test]
    fn k_weighting_matches_published_48k_coefficients() {
        let s = shelf_coeffs(48_000.0);
        let h = highpass_coeffs(48_000.0);
        let want_s = [
            1.53512485958697,
            -2.69169618940638,
            1.19839281085285,
            -1.69065929318241,
            0.73248077421585,
        ];
        let got_s = [s.b0, s.b1, s.b2, s.a1, s.a2];
        let mut worst = 0.0f64;
        for (a, b) in got_s.iter().zip(want_s) {
            worst = worst.max((a - b).abs());
        }
        worst = worst.max((h.a1 + 1.99004745483398).abs());
        worst = worst.max((h.a2 - 0.99007225036621).abs());
        eprintln!("K-weighting 48k coefficient deviation: {worst:.2e}");
        assert!(worst < 1e-6);
        // gain at 997 Hz is +0.691 dB by construction of the -0.691 offset
        assert!((KWeighting::response_db(48_000.0, 997.0) - 0.691).abs() < 0.005);
        // high-frequency shelf ~ +4 dB
        assert!((KWeighting::response_db(48_000.0, 10_000.0) - 4.0).abs() < 0.3);
        // sample-rate independence of the analog response near 1 kHz
        for fs in [44_100.0, 96_000.0, 192_000.0] {
            let d = KWeighting::response_db(fs, 1000.0) - KWeighting::response_db(48_000.0, 1000.0);
            assert!(d.abs() < 0.03, "{fs} {d}"); // bilinear warping, 0.017 dB at 96 kHz
        }
    }

    #[test]
    fn tech3341_case1_2_reference_levels() {
        for fs in [48_000.0, 44_100.0, 96_000.0] {
            let m = run(fs, &[(-23.0, 20.0)]);
            let i = m.integrated_lufs().unwrap();
            eprintln!("case1 fs {fs}: integrated {i:.4}");
            assert!((i + 23.0).abs() < 0.05);
            assert!((m.momentary_lufs() + 23.0).abs() < 0.05);
            assert!((m.short_term_lufs() + 23.0).abs() < 0.05);
        }
        let m = run(48_000.0, &[(-33.0, 20.0)]);
        assert!((m.integrated_lufs().unwrap() + 33.0).abs() < 0.05);
        // 997 Hz -20 dBFS stereo reference and mono
        let m = run(48_000.0, &[(-20.0, 10.0)]);
        assert!((m.integrated_lufs().unwrap() + 20.0).abs() < 0.05);
        let mut mono = LoudnessMeter::new(48_000.0, 1);
        let x: Vec<f64> = (0..480_000)
            .map(|i| 0.1 * (2.0 * PI * 997.0 * i as f64 / 48_000.0).sin())
            .collect();
        mono.process_mono(&x);
        assert!((mono.integrated_lufs().unwrap() + 23.01).abs() < 0.1);
    }

    #[test]
    fn tech3341_gating_cases() {
        // case 3: -36 (10 s) / -23 (60 s) / -36 (10 s) -> -23
        let m = run(48_000.0, &[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)]);
        let i = m.integrated_lufs().unwrap();
        eprintln!("case3 {i:.4}");
        assert!((i + 23.0).abs() < 0.1);
        // case 4: adds -72 LUFS sections removed by the absolute gate
        let m = run(
            48_000.0,
            &[
                (-72.0, 10.0),
                (-36.0, 10.0),
                (-23.0, 60.0),
                (-36.0, 10.0),
                (-72.0, 10.0),
            ],
        );
        let i = m.integrated_lufs().unwrap();
        eprintln!("case4 {i:.4}");
        assert!((i + 23.0).abs() < 0.1);
        // case 5 style: -26 / -20 alternation, nothing is gated; expected
        // from the energy mean of the sections
        let parts = [
            (-26.0, 20.0),
            (-20.0, 20.0),
            (-26.0, 20.0),
            (-20.0, 20.0),
            (-26.0, 20.0),
        ];
        let m = run(48_000.0, &parts);
        let e: f64 = parts
            .iter()
            .map(|(l, s)| s * 10f64.powf(l / 10.0))
            .sum::<f64>()
            / 100.0;
        let want = 10.0 * e.log10();
        let i = m.integrated_lufs().unwrap();
        eprintln!("case5-style {i:.4} want {want:.4}");
        assert!((i - want).abs() < 0.1);
        // silence only: no integrated value
        assert!(run(48_000.0, &[(-90.0, 5.0)]).integrated_lufs().is_none());
        assert!(LoudnessMeter::new(48_000.0, 2).integrated_lufs().is_none());
        assert_eq!(
            LoudnessMeter::new(48_000.0, 2).momentary_lufs(),
            f64::NEG_INFINITY
        );
    }

    #[test]
    fn tech3342_lra_cases() {
        let cases: [(&[(f64, f64)], f64); 4] = [
            (&[(-20.0, 20.0), (-30.0, 20.0)], 10.0),
            (&[(-20.0, 20.0), (-15.0, 20.0)], 5.0),
            (&[(-40.0, 20.0), (-20.0, 20.0)], 20.0),
            (
                &[
                    (-50.0, 20.0),
                    (-35.0, 20.0),
                    (-20.0, 20.0),
                    (-35.0, 20.0),
                    (-50.0, 20.0),
                ],
                15.0,
            ),
        ];
        for (i, (parts, want)) in cases.iter().enumerate() {
            let m = run(48_000.0, parts);
            let r = m.loudness_range().unwrap();
            eprintln!("LRA case {}: {:.3} LU (expected {want})", i + 1, r.lra);
            assert!((r.lra - want).abs() < 1.0, "case {}: {}", i + 1, r.lra);
        }
        // constant level -> LRA 0
        let r = run(48_000.0, &[(-23.0, 20.0)]).loudness_range().unwrap();
        assert!(r.lra.abs() < 0.05);
    }
}
