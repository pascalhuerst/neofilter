//! Test-signal generators.
//!
//! Realtime: **every type here is audio-thread safe** (allocation-free after
//! construction, panic-free `next_sample` / `fill`). Construction of
//! [`PinkNoise`] runs a small numerical filter fit (~milliseconds) and must
//! therefore happen off the audio thread.
//!
//! # Level convention
//!
//! * Tonal generators ([`Sine`], [`LogSweep`], [`DualTone`] per tone):
//!   `level_dbfs` is the **peak** amplitude, 0 dBFS = amplitude 1.0.
//! * Noise generators ([`WhiteNoise`], [`PinkNoise`]): `level_dbfs` is the
//!   **RMS** value (a sine of the same level has 3 dB higher peak).
//!
//! Noise is Gaussian and fully deterministic for a given seed.
//!
//! ```
//! use neo_measure::generator::{Generator, PinkNoise};
//! let mut g = PinkNoise::new(48_000.0, -20.0, 1);
//! let mut buf = [0.0f64; 256];
//! g.fill(&mut buf);
//! ```

use crate::util::db_to_amp;
use std::f64::consts::PI;

/// Common interface of all generators.
pub trait Generator {
    /// Next output sample.
    fn next_sample(&mut self) -> f64;
    /// Fill a slice.
    fn fill(&mut self, out: &mut [f64]) {
        for o in out.iter_mut() {
            *o = self.next_sample();
        }
    }
    /// Fill an `f32` slice.
    fn fill_f32(&mut self, out: &mut [f32]) {
        for o in out.iter_mut() {
            *o = self.next_sample() as f32;
        }
    }
}

/// Deterministic seedable PRNG (SplitMix64 seeding + xoshiro256**-free simple
/// xorshift64*; statistically adequate for noise generation).
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // splitmix64 scramble so that small seeds give unrelated streams
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self {
            state: if z == 0 { 0x1234_5678_9ABC_DEF1 } else { z },
            spare: None,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal (Box-Muller).
    pub fn gaussian(&mut self) -> f64 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let u1 = 1.0 - self.uniform(); // (0,1]
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let (s, c) = (2.0 * PI * u2).sin_cos();
        self.spare = Some(r * s);
        r * c
    }
}

/// Sine generator (phase accumulator).
#[derive(Debug, Clone)]
pub struct Sine {
    phase: f64,
    inc: f64,
    amp: f64,
}

impl Sine {
    /// `level_dbfs` is the peak level.
    pub fn new(fs: f64, freq: f64, level_dbfs: f64) -> Self {
        Self::with_phase(fs, freq, level_dbfs, 0.0)
    }
    /// Start phase in radians.
    pub fn with_phase(fs: f64, freq: f64, level_dbfs: f64, phase: f64) -> Self {
        Self {
            phase: phase.rem_euclid(2.0 * PI),
            inc: 2.0 * PI * freq / fs,
            amp: db_to_amp(level_dbfs),
        }
    }
    pub fn set_frequency(&mut self, fs: f64, freq: f64) {
        self.inc = 2.0 * PI * freq / fs;
    }
    pub fn set_level_dbfs(&mut self, level_dbfs: f64) {
        self.amp = db_to_amp(level_dbfs);
    }
}

impl Generator for Sine {
    #[inline]
    fn next_sample(&mut self) -> f64 {
        let y = self.amp * self.phase.sin();
        self.phase += self.inc;
        if self.phase >= 2.0 * PI {
            self.phase -= 2.0 * PI;
        }
        y
    }
}

/// Two simultaneous sines (e.g. SMPTE 60 Hz + 7 kHz at 4:1 or CCIF 19+20 kHz).
/// Each tone has its own peak level.
#[derive(Debug, Clone)]
pub struct DualTone {
    a: Sine,
    b: Sine,
}

impl DualTone {
    pub fn new(fs: f64, f1: f64, level1_dbfs: f64, f2: f64, level2_dbfs: f64) -> Self {
        Self {
            a: Sine::new(fs, f1, level1_dbfs),
            b: Sine::new(fs, f2, level2_dbfs),
        }
    }
}

impl Generator for DualTone {
    #[inline]
    fn next_sample(&mut self) -> f64 {
        self.a.next_sample() + self.b.next_sample()
    }
}

/// Exponential (logarithmic) sine sweep `f(t) = f0 * (f1/f0)^(t/T)`.
/// After `duration` seconds the output is silent (`finished()` is true), or
/// restarts if `looping` is set. Peak level in dBFS.
#[derive(Debug, Clone)]
pub struct LogSweep {
    fs: f64,
    f0: f64,
    l: f64,
    amp: f64,
    len: u64,
    n: u64,
    looping: bool,
}

impl LogSweep {
    pub fn new(fs: f64, f0: f64, f1: f64, duration_s: f64, level_dbfs: f64, looping: bool) -> Self {
        let f0 = f0.max(1e-3);
        let f1 = f1.max(f0 * 1.000_001);
        let t = duration_s.max(1.0 / fs);
        Self {
            fs,
            f0,
            l: t / (f1 / f0).ln(),
            amp: db_to_amp(level_dbfs),
            len: (t * fs).round().max(1.0) as u64,
            n: 0,
            looping,
        }
    }
    pub fn finished(&self) -> bool {
        !self.looping && self.n >= self.len
    }
    pub fn len_samples(&self) -> u64 {
        self.len
    }
    pub fn restart(&mut self) {
        self.n = 0;
    }
    /// Instantaneous frequency at sample `n`.
    pub fn frequency_at(&self, n: u64) -> f64 {
        self.f0 * (n as f64 / self.fs / self.l).exp()
    }
}

impl Generator for LogSweep {
    fn next_sample(&mut self) -> f64 {
        if self.n >= self.len {
            if self.looping {
                self.n = 0;
            } else {
                return 0.0;
            }
        }
        let t = self.n as f64 / self.fs;
        let ph = 2.0 * PI * self.f0 * self.l * ((t / self.l).exp() - 1.0);
        self.n += 1;
        self.amp * ph.sin()
    }
}

/// White Gaussian noise, level = RMS in dBFS.
#[derive(Debug, Clone)]
pub struct WhiteNoise {
    rng: Rng,
    amp: f64,
}

impl WhiteNoise {
    pub fn new(level_rms_dbfs: f64, seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            amp: db_to_amp(level_rms_dbfs),
        }
    }
}

impl Generator for WhiteNoise {
    #[inline]
    fn next_sample(&mut self) -> f64 {
        self.amp * self.rng.gaussian()
    }
}

/// Pink (-3 dB/octave) Gaussian noise, level = RMS in dBFS.
///
/// White noise is shaped by a cascade of real pole/zero sections whose
/// positions are fitted at construction (Levenberg-Marquardt on the log
/// magnitude) for the actual sample rate, so the slope holds to a few
/// hundredths of a dB from 20 Hz up to `min(20 kHz, 0.45 fs)`; below 20 Hz
/// the response flattens out (finite power), above it is unconstrained.
#[derive(Debug, Clone)]
pub struct PinkNoise {
    rng: Rng,
    p: [f64; PINK_N],
    z: [f64; PINK_N],
    s: [f64; PINK_N],
    gain: f64,
}

const PINK_N: usize = 16;

fn sec_mag_db(p: f64, z: f64, c: f64) -> f64 {
    10.0 * ((1.0 - 2.0 * z * c + z * z) / (1.0 - 2.0 * p * c + p * p)).log10()
}

fn solve(a: &mut [Vec<f64>], b: &mut [f64]) -> bool {
    let n = b.len();
    for c in 0..n {
        let mut piv = c;
        for r in c + 1..n {
            if a[r][c].abs() > a[piv][c].abs() {
                piv = r;
            }
        }
        if a[piv][c].abs() < 1e-300 {
            return false;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    for c in (0..n).rev() {
        let mut s = b[c];
        for k in c + 1..n {
            s -= a[c][k] * b[k];
        }
        b[c] = s / a[c][c];
    }
    true
}

impl PinkNoise {
    pub fn new(fs: f64, level_rms_dbfs: f64, seed: u64) -> Self {
        let (p, z, g) = Self::design(fs);
        let mut me = Self {
            rng: Rng::new(seed),
            p,
            z,
            s: [0.0; PINK_N],
            gain: g,
        };
        me.gain *= db_to_amp(level_rms_dbfs);
        me
    }

    /// Magnitude response in dB (relative to the unit-variance-input
    /// normalisation, i.e. 0 dB means the white-noise input power).
    pub fn response_db(&self, fs: f64, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / fs;
        let mut d = 20.0 * self.gain.log10();
        for k in 0..PINK_N {
            d += sec_mag_db(self.p[k], self.z[k], w.cos());
        }
        d
    }

    fn design(fs: f64) -> ([f64; PINK_N], [f64; PINK_N], f64) {
        let f_hi = 20_000f64.min(0.45 * fs);
        // frequency grid and weights
        let np = 160;
        let (f_lo_grid, f_top) = (4.0, 0.49 * fs);
        let grid: Vec<(f64, f64)> = (0..np)
            .map(|i| {
                let f = f_lo_grid * (f_top / f_lo_grid).powf(i as f64 / (np - 1) as f64);
                let w = if f >= 20.0 && f <= f_hi {
                    1.0
                } else if f < 20.0 {
                    0.2
                } else {
                    0.0
                };
                (2.0 * PI * f / fs, w)
            })
            .collect();
        let target = |w: f64| -10.0 * (w * fs / (2.0 * PI)).log10();
        // initial: pole per octave from 3 Hz, zero half an octave above
        let mut th = vec![0.0; 2 * PINK_N + 1];
        for k in 0..PINK_N {
            let fp = 3.0 * 2f64.powi(k as i32);
            let fz = fp * 2f64.sqrt();
            th[k] = (-2.0 * PI * fp / fs).exp();
            th[PINK_N + k] = (-2.0 * PI * fz / fs).exp();
        }
        let resid = |th: &[f64]| -> Vec<f64> {
            grid.iter()
                .map(|&(w, wt)| {
                    let cw = w.cos();
                    let mut d = th[2 * PINK_N];
                    for k in 0..PINK_N {
                        d += sec_mag_db(th[k], th[PINK_N + k], cw);
                    }
                    wt * (d - target(w))
                })
                .collect()
        };
        // initial offset
        let r0 = resid(&th);
        let mean: f64 = r0.iter().sum::<f64>() / r0.len() as f64;
        th[2 * PINK_N] = -mean;
        let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
        let mut r = resid(&th);
        let mut c = cost(&r);
        let mut lam = 1e-3;
        let n = th.len();
        for _ in 0..150 {
            // jacobian
            let mut jac = vec![vec![0.0; n]; r.len()];
            for j in 0..n {
                let mut t2 = th.clone();
                let h = 1e-7;
                t2[j] += h;
                let r2 = resid(&t2);
                for i in 0..r.len() {
                    jac[i][j] = (r2[i] - r[i]) / h;
                }
            }
            let mut jtj = vec![vec![0.0; n]; n];
            let mut jtr = vec![0.0; n];
            for i in 0..r.len() {
                for a in 0..n {
                    jtr[a] += jac[i][a] * r[i];
                    for b in 0..n {
                        jtj[a][b] += jac[i][a] * jac[i][b];
                    }
                }
            }
            let mut improved = false;
            for _ in 0..12 {
                let mut a = jtj.clone();
                for d in 0..n {
                    a[d][d] += lam * (jtj[d][d] + 1e-12);
                }
                let mut rhs: Vec<f64> = jtr.iter().map(|v| -v).collect();
                if !solve(&mut a, &mut rhs) {
                    lam *= 10.0;
                    continue;
                }
                let mut t2 = th.clone();
                for j in 0..n {
                    t2[j] += rhs[j];
                }
                for j in 0..2 * PINK_N {
                    t2[j] = t2[j].clamp(-0.99999, 0.99999);
                }
                let r2 = resid(&t2);
                let c2 = cost(&r2);
                if c2 < c {
                    th = t2;
                    r = r2;
                    let rel = (c - c2) / c.max(1e-30);
                    c = c2;
                    lam = (lam * 0.3).max(1e-12);
                    improved = rel > 1e-9;
                    break;
                }
                lam *= 5.0;
            }
            if !improved {
                break;
            }
        }
        let mut p = [0.0; PINK_N];
        let mut z = [0.0; PINK_N];
        p.copy_from_slice(&th[..PINK_N]);
        z.copy_from_slice(&th[PINK_N..2 * PINK_N]);
        // unit-variance normalisation: output variance of the fitted cascade
        let g0 = 10f64.powf(th[2 * PINK_N] / 20.0);
        let m = 1 << 15;
        let mut pw = 0.0;
        for i in 0..m {
            let w = PI * (i as f64 + 0.5) / m as f64;
            let mut d = 0.0;
            for k in 0..PINK_N {
                d += sec_mag_db(p[k], z[k], w.cos());
            }
            pw += 10f64.powf(d / 10.0);
        }
        pw /= m as f64;
        let norm = 1.0 / (g0 * g0 * pw).sqrt();
        (p, z, g0 * norm)
    }
}

impl Generator for PinkNoise {
    #[inline]
    fn next_sample(&mut self) -> f64 {
        let mut y = self.rng.gaussian() * self.gain;
        for k in 0..PINK_N {
            let o = y + self.s[k];
            self.s[k] = -self.z[k] * y + self.p[k] * o;
            y = o;
        }
        y
    }
}

/// Click-free start/stop wrapper using a raised-cosine ramp.
///
/// The wrapped generator keeps running; the ramp only scales its output.
/// Starts silent (`stopped`).
#[derive(Debug, Clone)]
pub struct Gated<G: Generator> {
    gen: G,
    pos: f64,
    step: f64,
    target_on: bool,
}

impl<G: Generator> Gated<G> {
    /// `ramp_s`: duration of a full 0..1 ramp in seconds (>= 1 sample).
    pub fn new(gen: G, fs: f64, ramp_s: f64) -> Self {
        Self {
            gen,
            pos: 0.0,
            step: 1.0 / (ramp_s * fs).max(1.0),
            target_on: false,
        }
    }
    pub fn start(&mut self) {
        self.target_on = true;
    }
    pub fn stop(&mut self) {
        self.target_on = false;
    }
    /// True while the output is not fully silent (ramping or on).
    pub fn is_active(&self) -> bool {
        self.target_on || self.pos > 0.0
    }
    pub fn inner_mut(&mut self) -> &mut G {
        &mut self.gen
    }
}

impl<G: Generator> Generator for Gated<G> {
    #[inline]
    fn next_sample(&mut self) -> f64 {
        let x = self.gen.next_sample();
        if self.target_on {
            self.pos = (self.pos + self.step).min(1.0);
        } else {
            self.pos = (self.pos - self.step).max(0.0);
        }
        let g = 0.5 - 0.5 * (PI * self.pos).cos();
        x * g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peak_and_rng_determinism() {
        let mut s = Sine::new(48_000.0, 1000.0, -6.0);
        let mut b = vec![0.0; 48_000];
        s.fill(&mut b);
        let pk = b.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        assert!((pk - db_to_amp(-6.0)).abs() < 1e-4);
        let mut a = WhiteNoise::new(-20.0, 7);
        let mut c = WhiteNoise::new(-20.0, 7);
        assert_eq!(a.next_sample(), c.next_sample());
    }

    #[test]
    fn white_noise_rms() {
        let mut g = WhiteNoise::new(-20.0, 3);
        let n = 1_000_000;
        let e: f64 = (0..n).map(|_| g.next_sample().powi(2)).sum::<f64>() / n as f64;
        let db = 10.0 * e.log10();
        assert!((db + 20.0).abs() < 0.02, "{db}");
    }

    #[test]
    fn pink_response_is_minus_3db_per_octave() {
        for fs in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let g = PinkNoise::new(fs, 0.0, 1);
            let hi = 20_000f64.min(0.45 * fs);
            let r0 = g.response_db(fs, 20.0);
            let mut worst = 0.0f64;
            let mut f = 20.0;
            while f <= hi {
                let want = r0 - 10.0 * (f / 20.0).log10();
                worst = worst.max((g.response_db(fs, f) - want).abs());
                f *= 1.02;
            }
            eprintln!("pink fs={fs} worst slope error {worst:.4} dB");
            assert!(worst < 0.05, "fs {fs}: {worst}");
        }
    }

    #[test]
    fn pink_rms_level() {
        let mut g = PinkNoise::new(48_000.0, -20.0, 5);
        let n = 1_000_000;
        let e: f64 = (0..n).map(|_| g.next_sample().powi(2)).sum::<f64>() / n as f64;
        let db = 10.0 * e.log10();
        assert!((db + 20.0).abs() < 0.3, "{db}");
    }

    #[test]
    fn sweep_frequency_and_gate() {
        let sw = LogSweep::new(48_000.0, 20.0, 20_000.0, 10.0, 0.0, false);
        assert!((sw.frequency_at(0) - 20.0).abs() < 1e-9);
        assert!((sw.frequency_at(480_000) - 20_000.0).abs() < 1e-6);
        let mut g = Gated::new(Sine::new(48_000.0, 1000.0, 0.0), 48_000.0, 0.01);
        assert_eq!(g.next_sample(), 0.0);
        g.start();
        let mut b = vec![0.0; 2000];
        g.fill(&mut b);
        assert!(b[0].abs() < 1e-3 && g.is_active());
        g.stop();
        g.fill(&mut b);
        assert!(!g.is_active());
    }
}
