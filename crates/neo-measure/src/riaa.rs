//! RIAA pre-/de-emphasis: analytic reference curves and the digital design.
//!
//! No plugin types in here; the module is plain f64 DSP.
//!
//! # Analytic curves
//!
//! With `T1 = 3180 us`, `T2 = 318 us`, `T3 = 75 us` the RIAA playback
//! (de-emphasis) response is `(1 + sT2) / ((1 + sT1)(1 + sT3))`; the recording
//! (pre-emphasis) response is its reciprocal. Both are normalised to 0 dB at
//! 1 kHz (magnitude only; the phase is that of the unnormalised formula).
//!
//! Options:
//! * `ultrasonic`: pole `T5 = 3.18 us` (about 50 kHz) in the pre-emphasis,
//!   i.e. `P = 1 / (H_de (1 + sT5))`; the de-emphasis is the reciprocal
//!   `H_de (1 + sT5)`.
//! * `iec`: IEC low-frequency roll-off `T4 = 7950 us`. De-emphasis gets an
//!   extra `s T4 / (1 + s T4)` (high-pass, about 20 Hz). The exact inverse of
//!   that is unbounded at DC, so in *pre-emphasis* mode the bass boost is
//!   limited by a 5 Hz first-order high-pass (`T_L = 31.8 ms`):
//!   `P = (1 + sT4) / (sT4) * (s T_L / (1 + s T_L)) / H_de_base`.
//!   Pre-emphasis with IEC followed by de-emphasis with IEC is therefore
//!   unity only above about 20 Hz and rolls off below 5 Hz.
//!
//! # Digital design
//!
//! For each sample rate, [`RiaaFilters::design`] (allocating, not for the audio
//! thread) fits a minimum-phase IIR to the analytic *de-emphasis* by nonlinear
//! least squares on the complex response error (log magnitude and phase over a
//! log grid, 10 Hz .. 20 kHz), reweighted towards a minimax solution. Each
//! biquad is parameterised as a bilinear transform of an analog section with
//! positive `(w0, Q)` for numerator and denominator, so every candidate is
//! stable and minimum phase by construction. The pre-emphasis is the exact
//! inverse (numerators and denominators swapped), so pre followed by de is
//! unity to machine precision.
//!
//! Above 20 kHz the response is only weakly guided (towards the analytic curve
//! with a floor on the de-emphasis, so the pre-emphasis gain stays bounded);
//! the per-rate behaviour is measured in the tests and listed in the README.

use crate::riaa_tables as tables;
use std::f64::consts::PI;

pub const T1: f64 = 3180e-6;
pub const T2: f64 = 318e-6;
pub const T3: f64 = 75e-6;
/// IEC low-frequency roll-off time constant.
pub const T4: f64 = 7950e-6;
/// Ultrasonic pole time constant (about 50 kHz).
pub const T5: f64 = 3.18e-6;
/// Time constant of the 5 Hz limiter used with IEC pre-emphasis.
pub const T_LIMIT: f64 = 1.0 / (2.0 * PI * 5.0);
/// Normalisation frequency.
pub const REF_HZ: f64 = 1000.0;
/// Accuracy band edges.
pub const BAND_LO_HZ: f64 = 20.0;
pub const BAND_HI_HZ: f64 = 20000.0;

/// Maximum number of biquads in a [`Cascade`] (4 fitted + 1 IEC).
pub const MAX_SECTIONS: usize = 5;
/// Most biquads the fit may use (the IEC section is on top of these).
const MAX_FIT_SECTIONS: usize = 4;

// ---------------------------------------------------------------------------
// Complex numbers (tiny, to avoid a dependency)

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

#[allow(clippy::should_implement_trait)] // plain named methods are enough here
impl Complex {
    pub const ONE: Complex = Complex { re: 1.0, im: 0.0 };
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    pub fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
    pub fn mul(self, o: Complex) -> Complex {
        Complex::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    pub fn div(self, o: Complex) -> Complex {
        let d = o.re * o.re + o.im * o.im;
        Complex::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
    pub fn add(self, o: Complex) -> Complex {
        Complex::new(self.re + o.re, self.im + o.im)
    }
    pub fn scale(self, k: f64) -> Complex {
        Complex::new(self.re * k, self.im * k)
    }
    /// Magnitude in dB.
    pub fn db(self) -> f64 {
        20.0 * self.norm().log10()
    }
}

// ---------------------------------------------------------------------------
// Analytic curves

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Recording ("inverse RIAA", cutting).
    #[default]
    PreEmphasis,
    /// Playback.
    DeEmphasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// IEC 7950 us low-frequency roll-off (see module docs for pre-emphasis).
    pub iec: bool,
    /// 3.18 us ultrasonic pole.
    pub ultrasonic: bool,
}

impl Options {
    pub const ALL: [Options; 4] = [
        Options {
            iec: false,
            ultrasonic: false,
        },
        Options {
            iec: true,
            ultrasonic: false,
        },
        Options {
            iec: false,
            ultrasonic: true,
        },
        Options {
            iec: true,
            ultrasonic: true,
        },
    ];

    fn index(self) -> usize {
        usize::from(self.iec) + 2 * usize::from(self.ultrasonic)
    }
}

/// `1 + s T` at `s = j 2 pi f`.
fn one_plus(t: f64, f: f64) -> Complex {
    Complex::new(1.0, 2.0 * PI * f * t)
}

/// `s T / (1 + s T)` at `s = j 2 pi f`.
fn highpass(t: f64, f: f64) -> Complex {
    Complex::new(0.0, 2.0 * PI * f * t).div(one_plus(t, f))
}

/// De-emphasis of the *design* family (no IEC): base times optional `(1+sT5)`.
fn de_family_raw(ultrasonic: bool, f: f64) -> Complex {
    let mut h = one_plus(T2, f).div(one_plus(T1, f).mul(one_plus(T3, f)));
    if ultrasonic {
        h = h.mul(one_plus(T5, f));
    }
    h
}

/// Unnormalised analytic response.
fn raw_response(mode: Mode, opts: Options, f: f64) -> Complex {
    let de = de_family_raw(opts.ultrasonic, f);
    match (mode, opts.iec) {
        (Mode::DeEmphasis, false) => de,
        (Mode::DeEmphasis, true) => de.mul(highpass(T4, f)),
        (Mode::PreEmphasis, false) => Complex::ONE.div(de),
        (Mode::PreEmphasis, true) => highpass(T_LIMIT, f).div(de.mul(highpass(T4, f))),
    }
}

/// Analytic (analog) response at `f` Hz, normalised to 0 dB at 1 kHz.
pub fn analog_response(mode: Mode, opts: Options, f: f64) -> Complex {
    raw_response(mode, opts, f).scale(1.0 / raw_response(mode, opts, REF_HZ).norm())
}

// ---------------------------------------------------------------------------
// Biquads and cascades

/// Direct-form coefficients, `a0 = 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Biquad {
    pub const IDENTITY: Biquad = Biquad {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalised(b: [f64; 3], a: [f64; 3]) -> Biquad {
        Biquad {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }

    /// Frequency response at normalised angular frequency `w` (rad/sample).
    pub fn response_w(&self, w: f64) -> Complex {
        let z1 = Complex::new(w.cos(), -w.sin());
        let z2 = z1.mul(z1);
        let num = Complex::new(self.b0, 0.0)
            .add(z1.scale(self.b1))
            .add(z2.scale(self.b2));
        let den = Complex::ONE.add(z1.scale(self.a1)).add(z2.scale(self.a2));
        num.div(den)
    }

    /// Swap numerator and denominator (valid for a minimum-phase section).
    pub fn inverse(&self) -> Biquad {
        let ib0 = 1.0 / self.b0;
        Biquad {
            b0: ib0,
            b1: self.a1 * ib0,
            b2: self.a2 * ib0,
            a1: self.b1 * ib0,
            a2: self.b2 * ib0,
        }
    }

    /// Largest pole radius.
    pub fn pole_radius(&self) -> f64 {
        root_radius(1.0, self.a1, self.a2)
    }

    /// Largest zero radius.
    pub fn zero_radius(&self) -> f64 {
        root_radius(self.b0, self.b1, self.b2)
    }

    fn scaled(&self, g: f64) -> Biquad {
        Biquad {
            b0: self.b0 * g,
            b1: self.b1 * g,
            b2: self.b2 * g,
            ..*self
        }
    }
}

/// Largest root magnitude of `c0 z^2 + c1 z + c2`.
fn root_radius(c0: f64, c1: f64, c2: f64) -> f64 {
    if c0.abs() < 1e-300 {
        return if c1.abs() < 1e-300 {
            0.0
        } else {
            (c2 / c1).abs()
        };
    }
    let (p, q) = (c1 / c0, c2 / c0);
    let disc = p * p - 4.0 * q;
    if disc < 0.0 {
        q.abs().sqrt()
    } else {
        let r = disc.sqrt();
        ((-p + r) / 2.0).abs().max(((-p - r) / 2.0).abs())
    }
}

/// A fixed-capacity cascade of biquads (no heap; `Copy`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cascade {
    sections: [Biquad; MAX_SECTIONS],
    len: usize,
}

impl Cascade {
    pub fn identity() -> Self {
        Self {
            sections: [Biquad::IDENTITY; MAX_SECTIONS],
            len: 0,
        }
    }

    /// Panics if the cascade is full (design-time only).
    pub fn push(&mut self, s: Biquad) {
        assert!(self.len < MAX_SECTIONS);
        self.sections[self.len] = s;
        self.len += 1;
    }

    pub fn sections(&self) -> &[Biquad] {
        &self.sections[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Filter order (2 per section, minus 1 for first-order sections).
    pub fn order(&self) -> usize {
        self.sections()
            .iter()
            .map(|s| if s.a2 == 0.0 && s.b2 == 0.0 { 1 } else { 2 })
            .sum()
    }

    pub fn response(&self, f_hz: f64, sample_rate: f64) -> Complex {
        let w = 2.0 * PI * f_hz / sample_rate;
        self.sections()
            .iter()
            .fold(Complex::ONE, |acc, s| acc.mul(s.response_w(w)))
    }

    /// Exact inverse (sections inverted); used for pre = 1 / de.
    pub fn inverse(&self) -> Cascade {
        let mut c = Cascade::identity();
        for s in self.sections() {
            c.push(s.inverse());
        }
        c
    }

    /// Scale the overall gain.
    fn scale_gain(&mut self, g: f64) {
        if self.len > 0 {
            self.sections[0] = self.sections[0].scaled(g);
        }
    }

    /// True if all poles and zeros are inside the unit circle.
    pub fn is_stable_min_phase(&self) -> bool {
        self.sections()
            .iter()
            .all(|s| s.pole_radius() < 1.0 && s.zero_radius() < 1.0)
    }
}

/// Per-channel filter state (transposed direct form II, f64).
#[derive(Debug, Clone, Copy, Default)]
pub struct CascadeState {
    z: [[f64; 2]; MAX_SECTIONS],
}

impl CascadeState {
    pub fn reset(&mut self) {
        self.z = [[0.0; 2]; MAX_SECTIONS];
    }

    /// Process one sample.
    #[inline]
    pub fn process(&mut self, c: &Cascade, x: f64) -> f64 {
        let mut v = x;
        for (s, z) in c.sections[..c.len].iter().zip(self.z.iter_mut()) {
            let y = s.b0 * v + z[0];
            z[0] = s.b1 * v - s.a1 * y + z[1];
            z[1] = s.b2 * v - s.a2 * y;
            // Denormal guard: flush tiny states (far below any audible level).
            if z[0].abs() < 1e-30 {
                z[0] = 0.0;
            }
            if z[1].abs() < 1e-30 {
                z[1] = 0.0;
            }
            v = y;
        }
        v
    }
}

// ---------------------------------------------------------------------------
// Fitting

/// First-order bilinear sections for IEC, `k = 2 fs`.
fn bilinear_first(b: [f64; 2], a: [f64; 2]) -> Biquad {
    Biquad::normalised([b[0], b[1], 0.0], [a[0], a[1], 0.0])
}

/// `s T4 / (1 + s T4)`.
fn iec_highpass_section(k: f64) -> Biquad {
    bilinear_first([k * T4, -k * T4], [1.0 + k * T4, 1.0 - k * T4])
}

/// `(T4/TL)(1 + s TL) / (1 + s T4)`: inverse of `HP4 / HP5`.
fn iec_limited_boost_inverse_section(k: f64) -> Biquad {
    let g = T4 / T_LIMIT;
    bilinear_first(
        [g * (1.0 + k * T_LIMIT), g * (1.0 - k * T_LIMIT)],
        [1.0 + k * T4, 1.0 - k * T4],
    )
}

/// Parameters of one fitted section: ln of (wz, Qz, wp, Qp).
const PER_SECTION: usize = 4;

fn section_biquad(p: &[f64], k: f64) -> Biquad {
    let (wz, qz, wp, qp) = (p[0].exp(), p[1].exp(), p[2].exp(), p[3].exp());
    // N(s) = 1 + s/(w Q) + s^2/w^2 with s = k (1 - z^-1)/(1 + z^-1).
    let poly = |w: f64, q: f64| -> [f64; 3] {
        let (n1, n2) = (1.0 / (w * q), 1.0 / (w * w));
        [
            1.0 + n1 * k + n2 * k * k,
            2.0 - 2.0 * n2 * k * k,
            1.0 - n1 * k + n2 * k * k,
        ]
    };
    Biquad::normalised(poly(wz, qz), poly(wp, qp))
}

fn params_to_cascade(p: &[f64], k: f64) -> Cascade {
    let mut c = Cascade::identity();
    for sec in p[1..].chunks(PER_SECTION) {
        c.push(section_biquad(sec, k));
    }
    c.scale_gain(p[0].exp());
    c
}

/// Fit grid with the precomputed target.
struct FitProblem {
    /// `e^{-jw}` on the accuracy grid.
    z1: Vec<Complex>,
    target: Vec<Complex>,
    weight: Vec<f64>,
    /// Phase residual weight relative to the magnitude residual.
    alpha: f64,
    /// `e^{-jw}` at the reference frequency.
    z1_ref: Complex,
    /// Out-of-band guidance points (`e^{-jw}`, guidance log-magnitude).
    z1_ext: Vec<Complex>,
    ext_logmag: Vec<f64>,
    k: f64,
}

/// Weighting scales: residual 1 = this much error.
const MAG_SCALE_NP: f64 = 0.01 / 8.685889638065035; // 0.01 dB
const PHASE_SCALE_RAD: f64 = 0.5 * PI / 180.0;
/// Tolerance band (dB) around the guidance curve above 20 kHz.
const EXT_TOL_DB: f64 = 6.0;
/// Weight of the exact-0-dB-at-1-kHz residual.
const REF_WEIGHT: f64 = 30.0;
/// Floor of the de-emphasis (re 1 kHz) used for guidance above 20 kHz: the
/// pre-emphasis gain is steered to stay below about 36 dB (plus tolerance).
const EXT_FLOOR_DB: f64 = -36.0;

fn unit_z1(w: f64) -> Complex {
    Complex::new(w.cos(), -w.sin())
}

fn eval_cascade_z(c: &Cascade, z1: Complex) -> Complex {
    let z2 = z1.mul(z1);
    let mut acc = Complex::ONE;
    for s in c.sections() {
        let num = Complex::new(s.b0, 0.0)
            .add(z1.scale(s.b1))
            .add(z2.scale(s.b2));
        let den = Complex::ONE.add(z1.scale(s.a1)).add(z2.scale(s.a2));
        acc = acc.mul(num.div(den));
    }
    acc
}

impl FitProblem {
    fn new(fs: f64, ultrasonic: bool) -> Self {
        let norm = de_family_raw(ultrasonic, REF_HZ).norm();
        let target_at = |f: f64| de_family_raw(ultrasonic, f).scale(1.0 / norm);
        let n = 140;
        let (lo, hi) = (10.0f64, BAND_HI_HZ.min(0.45 * fs));
        let (mut z1, mut target) = (Vec::new(), Vec::new());
        for i in 0..n {
            let f = lo * (hi / lo).powf(i as f64 / (n - 1) as f64);
            z1.push(unit_z1(2.0 * PI * f / fs));
            target.push(target_at(f));
        }
        let weight = vec![1.0; z1.len()];
        let top = fs / 2.0 * 0.995;
        let (mut z1_ext, mut ext_logmag) = (Vec::new(), Vec::new());
        if top > hi * 1.02 {
            let m = 40;
            for i in 1..=m {
                let f = hi * (top / hi).powf(i as f64 / m as f64);
                z1_ext.push(unit_z1(2.0 * PI * f / fs));
                let mag = target_at(f).norm().max(10f64.powf(EXT_FLOOR_DB / 20.0));
                ext_logmag.push(mag.ln());
            }
        }
        Self {
            z1,
            target,
            weight,
            alpha: 1.0,
            z1_ref: unit_z1(2.0 * PI * REF_HZ / fs),
            z1_ext,
            ext_logmag,
            k: 2.0 * fs,
        }
    }

    fn n_res(&self) -> usize {
        2 * self.z1.len() + self.z1_ext.len() + 1
    }

    fn residuals(&self, p: &[f64], out: &mut Vec<f64>) {
        out.clear();
        let c = params_to_cascade(p, self.k);
        for ((&z, t), &wt) in self.z1.iter().zip(&self.target).zip(&self.weight) {
            let r = eval_cascade_z(&c, z).div(*t);
            out.push(wt * r.norm().ln() / MAG_SCALE_NP);
            out.push(wt * self.alpha * r.arg() / PHASE_SCALE_RAD);
        }
        // exact 0 dB at the reference frequency (the design is normalised there)
        out.push(REF_WEIGHT * eval_cascade_z(&c, self.z1_ref).norm().ln() / MAG_SCALE_NP);
        let tol = EXT_TOL_DB / 8.685889638065035;
        for (&z, &lm) in self.z1_ext.iter().zip(&self.ext_logmag) {
            // hinge: free inside +-EXT_TOL_DB of the guidance curve
            let e = eval_cascade_z(&c, z).norm().ln() - lm;
            out.push((e.abs() - tol).max(0.0) * e.signum() / MAG_SCALE_NP);
        }
    }

    fn cost(&self, p: &[f64], buf: &mut Vec<f64>) -> f64 {
        self.residuals(p, buf);
        let c: f64 = buf.iter().map(|r| r * r).sum();
        if c.is_finite() {
            c
        } else {
            f64::INFINITY
        }
    }

    fn clamp(&self, p: &mut [f64]) {
        p[0] = p[0].clamp(-30.0, 30.0);
        for sec in p[1..].chunks_mut(PER_SECTION) {
            for (i, v) in sec.iter_mut().enumerate() {
                *v = if i % 2 == 0 {
                    v.clamp((2.0 * PI * 0.2f64).ln(), (self.k * 1e3).ln())
                } else {
                    v.clamp(0.05f64.ln(), 50f64.ln())
                };
            }
        }
    }
}

/// Solve `A x = b` by Gaussian elimination with partial pivoting. Returns
/// `None` if singular.
#[allow(clippy::needless_range_loop)]
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in col + 1..n {
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|c| a[r][c] * x[c]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

/// Levenberg-Marquardt with a numeric Jacobian. Never returns a worse point
/// than the start.
#[allow(clippy::needless_range_loop)]
fn levenberg_marquardt(prob: &FitProblem, start: &[f64], iters: usize) -> Vec<f64> {
    let n = start.len();
    let m = prob.n_res();
    let mut p = start.to_vec();
    let mut buf = Vec::with_capacity(m);
    let mut cost = prob.cost(&p, &mut buf);
    let mut lambda = 1e-3;
    let mut rp = Vec::with_capacity(m);
    for _ in 0..iters {
        let mut jac = vec![vec![0.0; m]; n];
        prob.residuals(&p, &mut buf);
        for j in 0..n {
            // forward differences (the residuals are smooth in log-parameters)
            let h = 1e-7;
            let mut q = p.clone();
            q[j] = p[j] + h;
            prob.residuals(&q, &mut rp);
            for i in 0..m {
                jac[j][i] = (rp[i] - buf[i]) / h;
            }
        }
        let mut jtj = vec![vec![0.0; n]; n];
        let mut g = vec![0.0; n];
        for a in 0..n {
            for b in a..n {
                let s: f64 = jac[a].iter().zip(&jac[b]).map(|(x, y)| x * y).sum();
                jtj[a][b] = s;
                jtj[b][a] = s;
            }
            g[a] = jac[a].iter().zip(&buf).map(|(x, r)| x * r).sum();
        }
        let mut improved = false;
        for _ in 0..14 {
            let mut a = jtj.clone();
            for (i, row) in a.iter_mut().enumerate() {
                row[i] += lambda * jtj[i][i].max(1e-12);
            }
            if let Some(d) = solve(a, g.iter().map(|v| -v).collect()) {
                let mut q: Vec<f64> = p.iter().zip(&d).map(|(x, dx)| x + dx).collect();
                prob.clamp(&mut q);
                let c = prob.cost(&q, &mut buf);
                if c < cost {
                    let rel = (cost - c) / cost.max(1e-300);
                    p = q;
                    cost = c;
                    lambda = (lambda / 5.0).max(1e-12);
                    improved = true;
                    if rel < 1e-12 {
                        return p;
                    }
                    break;
                }
            }
            lambda *= 8.0;
            if lambda > 1e14 {
                break;
            }
        }
        if !improved {
            break;
        }
    }
    p
}

/// Worst magnitude (dB) and phase (degrees) error of `c` against `target`
/// (a function of Hz) over a dense log grid in the accuracy band.
fn band_error(c: &Cascade, fs: f64, target: &dyn Fn(f64) -> Complex, npts: usize) -> (f64, f64) {
    let (mut mag, mut ph) = (0.0f64, 0.0f64);
    for i in 0..npts {
        let hi = BAND_HI_HZ.min(0.45 * fs);
        let f = BAND_LO_HZ * (hi / BAND_LO_HZ).powf(i as f64 / (npts - 1) as f64);
        let r = c.response(f, fs).div(target(f));
        mag = mag.max((20.0 * r.norm().log10()).abs());
        ph = ph.max(r.arg().abs().to_degrees());
    }
    (mag, ph)
}

/// Design goals (max magnitude error in dB, max phase error in degrees).
pub fn accuracy_goal(fs: f64) -> (f64, f64) {
    if fs >= 88_000.0 {
        (0.01, 0.5)
    } else {
        (0.02, 2.0)
    }
}

fn initial_params(fs: f64, prob: &FitProblem, ultrasonic: bool) -> Vec<f64> {
    let k = 2.0 * fs;
    // poles (T1, T3) as one second-order section, zeros (T2 and a high one).
    let wp = 1.0 / (T1 * T3).sqrt();
    let qp = (T1 * T3).sqrt() / (T1 + T3);
    let (z1, z2) = if ultrasonic { (T2, T5) } else { (T2, 1.0 / k) };
    let wz = 1.0 / (z1 * z2).sqrt();
    let qz = (z1 * z2).sqrt() / (z1 + z2);
    let mut p = vec![0.0, wz.ln(), qz.ln(), wp.ln(), qp.ln()];
    prob.clamp(&mut p);
    let c = params_to_cascade(&p, k);
    p[0] = -c.response(REF_HZ, fs).norm().ln();
    p
}

/// Extra section: a mild pair at a fraction of the sample rate.
fn extra_section(fs: f64, which: usize) -> [f64; 4] {
    let f = fs * [0.2, 0.08, 0.3, 0.12][which % 4];
    let w = 2.0 * PI * f;
    [w.ln(), 0.7f64.ln(), (w * 1.15).ln(), 0.7f64.ln()]
}

/// Magnitude error (fraction of the goal) the design aims for before it
/// starts trading anything for phase accuracy.
const MAG_MARGIN: f64 = 0.25;
/// Phase error (degrees) after the magnitude-first pass above which no
/// further phase refinement is attempted.
const PHASE_HOPELESS_DEG: f64 = 30.0;

#[derive(Debug, Clone, PartialEq)]
struct FitResult {
    params: Vec<f64>,
    cascade: Cascade,
    mag_db: f64,
    phase_deg: f64,
    /// Phase weight of the optimiser pass that produced the result.
    alpha: f64,
}

impl FitResult {
    /// Quality, smaller is better. Tier 0: both goals met with margin (rank by
    /// the worse relative error); tier 1: both goals met; tier 2: magnitude within `MAG_MARGIN` of its
    /// goal, ranked by phase error; tier 3: everything else, ranked by
    /// magnitude error. Magnitude accuracy always outranks phase accuracy.
    fn metric(&self, goal: (f64, f64)) -> f64 {
        let (m, p) = (self.mag_db / goal.0, self.phase_deg / goal.1);
        if m <= 0.6 && p <= 0.6 {
            m.max(p)
        } else if m <= 0.9 && p <= 0.9 {
            2.0 + m.max(p)
        } else if m <= MAG_MARGIN {
            10.0 + p
        } else {
            1e3 + m
        }
    }

    fn meets(&self, goal: (f64, f64)) -> bool {
        self.metric(goal) < 1.0
    }
}

/// Fit sections (up to `max_sections`) to the de-emphasis family curve.
fn fit_design_family(fs: f64, ultrasonic: bool, max_sections: usize) -> (FitResult, usize) {
    let goal = accuracy_goal(fs);
    let mut prob = FitProblem::new(fs, ultrasonic);
    let norm = de_family_raw(ultrasonic, REF_HZ).norm();
    let target = move |f: f64| de_family_raw(ultrasonic, f).scale(1.0 / norm);
    let eval = |p: &[f64], prob: &FitProblem| -> FitResult {
        let c = params_to_cascade(p, prob.k);
        let (m, ph) = band_error(&c, fs, &target, 1200);
        FitResult {
            params: p.to_vec(),
            cascade: c,
            mag_db: m,
            phase_deg: ph,
            alpha: prob.alpha,
        }
    };
    let accept = |r: &FitResult| r.cascade.is_stable_min_phase() && r.mag_db.is_finite();

    let mut best_by_n: Vec<Option<FitResult>> = vec![None; max_sections];
    let mut best_params_n1 = initial_params(fs, &prob, ultrasonic);
    let alphas = [0.01, 1.0, 0.3, 0.1, 0.03, 0.001];
    'outer: for &alpha in &alphas {
        prob.alpha = alpha;
        for n in 1..=max_sections {
            let start = match &best_by_n[n - 1] {
                Some(b) => b.params.clone(),
                None if n == 1 => best_params_n1.clone(),
                None => {
                    let mut s = best_by_n[n - 2]
                        .as_ref()
                        .map_or_else(|| best_params_n1.clone(), |b| b.params.clone());
                    s.extend_from_slice(&extra_section(fs, n - 2));
                    s
                }
            };
            prob.weight.iter_mut().for_each(|w| *w = 1.0);
            let mut p = levenberg_marquardt(&prob, &start, 300);
            let mut cand = eval(&p, &prob);
            // Lawson-style reweighting towards a minimax fit.
            for _ in 0..3 {
                let c = params_to_cascade(&p, prob.k);
                let errs: Vec<f64> = prob
                    .z1
                    .iter()
                    .zip(&prob.target)
                    .map(|(&z, t)| {
                        let r = eval_cascade_z(&c, z).div(*t);
                        ((20.0 * r.norm().log10()).abs() / goal.0)
                            .max(prob.alpha * r.arg().abs().to_degrees() / goal.1)
                    })
                    .collect();
                let mean = errs.iter().sum::<f64>() / errs.len() as f64;
                for (wt, e) in prob.weight.iter_mut().zip(&errs) {
                    *wt *= ((e + 0.05 * mean) / (mean + 1e-30)).sqrt();
                }
                let rms = (prob.weight.iter().map(|w| w * w).sum::<f64>()
                    / prob.weight.len() as f64)
                    .sqrt();
                prob.weight.iter_mut().for_each(|w| *w /= rms);
                p = levenberg_marquardt(&prob, &p, 100);
                let r = eval(&p, &prob);
                if accept(&r) && r.metric(goal) < cand.metric(goal) {
                    cand = r;
                }
            }
            let cur_ok = accept(&cand);
            let better = match &best_by_n[n - 1] {
                None => true,
                Some(b) => cand.metric(goal) < b.metric(goal),
            };
            if cur_ok && better {
                if n == 1 {
                    best_params_n1 = cand.params.clone();
                }
                best_by_n[n - 1] = Some(cand);
            }
            if let Some(b) = &best_by_n[n - 1] {
                if b.meets(goal) {
                    break 'outer;
                }
            }
        }
        // After the magnitude-first pass: if the phase error is hopelessly
        // large (inherent to minimum-phase filters near Nyquist), stop here.
        let best_phase = best_by_n
            .iter()
            .flatten()
            .filter(|r| r.mag_db <= MAG_MARGIN * goal.0)
            .map(|r| r.phase_deg)
            .fold(f64::INFINITY, f64::min);
        if best_phase > PHASE_HOPELESS_DEG {
            break;
        }
    }
    // Lowest-metric candidate; a higher order must be clearly better (2 %).
    let mut best: Option<(usize, &FitResult)> = None;
    for (i, r) in best_by_n.iter().enumerate() {
        if let Some(r) = r {
            if best.is_none_or(|(_, b)| r.metric(goal) < b.metric(goal) * 0.98) {
                best = Some((i + 1, r));
            }
        }
    }
    match best {
        Some((n, r)) => (r.clone(), n),
        None => {
            let p = initial_params(fs, &prob, ultrasonic);
            (eval(&p, &prob), 1)
        }
    }
}

/// Measured quality of one designed filter against its analytic curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterReport {
    /// Filter order of the cascade.
    pub order: usize,
    /// Worst |magnitude error| in dB, 20 Hz .. 20 kHz.
    pub max_mag_err_db: f64,
    /// Worst |phase error| in degrees, 20 Hz .. 20 kHz.
    pub max_phase_err_deg: f64,
}

/// Result of fitting one de-emphasis family (with or without the ultrasonic
/// pole) at one sample rate: the optimiser parameters plus the achieved
/// accuracy. `params = [ln gain, (ln wz, ln Qz, ln wp, ln Qp) per section]`
/// (analog section parameters, bilinear-transformed with `k = 2 fs`).
#[derive(Debug, Clone, PartialEq)]
pub struct FamilyFit {
    pub params: Vec<f64>,
    /// Phase weight of the optimiser pass that produced the fit.
    pub alpha: f64,
    /// Worst magnitude error in dB over 20 Hz .. min(20 kHz, 0.45 fs).
    pub mag_db: f64,
    /// Worst phase error in degrees over the same band.
    pub phase_deg: f64,
}

impl FamilyFit {
    fn from_result(r: &FitResult) -> Self {
        Self {
            params: r.params.clone(),
            alpha: r.alpha,
            mag_db: r.mag_db,
            phase_deg: r.phase_deg,
        }
    }

    fn cascade(&self, fs: f64) -> Cascade {
        params_to_cascade(&self.params, 2.0 * fs)
    }
}

/// Full optimiser for one family: 0.1 .. 1.5 s in a release build, many
/// times that in debug. Not for `initialize()` at an arbitrary rate; use
/// [`RiaaFilters::design`], which uses the shipped tables or the quick design.
pub fn optimise_family(sample_rate: f64, ultrasonic: bool) -> FamilyFit {
    let fs = clamp_rate(sample_rate);
    FamilyFit::from_result(&fit_design_family(fs, ultrasonic, MAX_FIT_SECTIONS).0)
}

/// Bounded-time design for any rate: warm start from the shipped table entry
/// nearest in log frequency, then a short Levenberg-Marquardt refinement
/// (magnitude-first weighting as in the full optimiser). Never worse than the
/// warm start; stable and minimum phase by construction.
pub fn quick_family(sample_rate: f64, ultrasonic: bool) -> FamilyFit {
    let fs = clamp_rate(sample_rate);
    let nearest = tables::TABLE
        .iter()
        .min_by(|a, b| (a.fs / fs).ln().abs().total_cmp(&(b.fs / fs).ln().abs()))
        .expect("table is not empty");
    let fam = &nearest.family[usize::from(ultrasonic)];
    let goal = accuracy_goal(fs);
    let mut prob = FitProblem::new(fs, ultrasonic);
    prob.alpha = fam.alpha;
    let norm = de_family_raw(ultrasonic, REF_HZ).norm();
    let target = move |f: f64| de_family_raw(ultrasonic, f).scale(1.0 / norm);
    let eval = |p: &[f64]| -> FitResult {
        let c = params_to_cascade(p, 2.0 * fs);
        let (m, ph) = band_error(&c, fs, &target, 1200);
        FitResult {
            params: p.to_vec(),
            cascade: c,
            mag_db: m,
            phase_deg: ph,
            alpha: fam.alpha,
        }
    };
    let accept = |r: &FitResult| r.cascade.is_stable_min_phase() && r.mag_db.is_finite();
    let mut start = fam.params.to_vec();
    prob.clamp(&mut start);
    let mut best = eval(&start);
    let mut p = start;
    for pass in 0..QUICK_PASSES {
        p = levenberg_marquardt(&prob, &p, QUICK_ITERS);
        let r = eval(&p);
        if accept(&r) && (!accept(&best) || r.metric(goal) < best.metric(goal)) {
            best = r;
        }
        if pass + 1 < QUICK_PASSES {
            reweight_minimax(&mut prob, &p, goal);
        }
    }
    FamilyFit::from_result(&best)
}

/// LM passes / iterations of [`quick_family`]; sized for < 50 ms in release.
const QUICK_PASSES: usize = 2;
const QUICK_ITERS: usize = 30;

fn clamp_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() {
        sample_rate.clamp(8_000.0, 768_000.0)
    } else {
        48_000.0
    }
}

/// One Lawson-style minimax reweighting step.
fn reweight_minimax(prob: &mut FitProblem, p: &[f64], goal: (f64, f64)) {
    let c = params_to_cascade(p, prob.k);
    let errs: Vec<f64> = prob
        .z1
        .iter()
        .zip(&prob.target)
        .map(|(&z, t)| {
            let r = eval_cascade_z(&c, z).div(*t);
            ((20.0 * r.norm().log10()).abs() / goal.0)
                .max(prob.alpha * r.arg().abs().to_degrees() / goal.1)
        })
        .collect();
    let mean = errs.iter().sum::<f64>() / errs.len() as f64;
    for (wt, e) in prob.weight.iter_mut().zip(&errs) {
        *wt *= ((e + 0.05 * mean) / (mean + 1e-30)).sqrt();
    }
    let rms = (prob.weight.iter().map(|w| w * w).sum::<f64>() / prob.weight.len() as f64).sqrt();
    prob.weight.iter_mut().for_each(|w| *w /= rms);
}

/// How a [`RiaaFilters`] set was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignSource {
    /// Precomputed optimiser result for one of [`TABLE_RATES`] (microseconds).
    Table,
    /// [`quick_family`]: warm start plus short refinement (bounded time).
    Quick,
    /// Full optimiser run ([`RiaaFilters::design_full`]).
    Full,
}

/// One shipped optimiser result (see `riaa_tables.rs`).
pub(crate) struct TableFamily {
    pub params: &'static [f64],
    pub alpha: f64,
    pub mag_db: f64,
    pub phase_deg: f64,
}

/// Shipped results for one sample rate: `family[0]` plain, `[1]` ultrasonic.
pub(crate) struct TableEntry {
    pub fs: f64,
    pub family: [TableFamily; 2],
}

/// The shipped fit of one family at a table rate (`None` for other rates).
pub fn table_family(sample_rate: f64, ultrasonic: bool) -> Option<FamilyFit> {
    let e = tables::TABLE
        .iter()
        .find(|e| (e.fs - sample_rate).abs() < 1e-6)?;
    let f = &e.family[usize::from(ultrasonic)];
    Some(FamilyFit {
        params: f.params.to_vec(),
        alpha: f.alpha,
        mag_db: f.mag_db,
        phase_deg: f.phase_deg,
    })
}

/// Sample rates with a shipped precomputed design (see `riaa_tables.rs`,
/// generated by `cargo run --release -p neo-measure --example riaa_tables`).
pub const TABLE_RATES: [f64; 8] = [
    44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0, 352_800.0, 384_000.0,
];

/// All filters (both modes, all option combinations) for one sample rate.
#[derive(Debug, Clone)]
pub struct RiaaFilters {
    sample_rate: f64,
    source: DesignSource,
    cascades: [[Cascade; 4]; 2],
    reports: [[FilterReport; 4]; 2],
}

fn mode_index(m: Mode) -> usize {
    match m {
        Mode::PreEmphasis => 0,
        Mode::DeEmphasis => 1,
    }
}

impl RiaaFilters {
    /// Filters for `sample_rate` (clamped to 8 kHz ..= 768 kHz): the shipped
    /// table entry when the rate is one of [`TABLE_RATES`] (about 10 us),
    /// otherwise the bounded-time [`quick_family`] design (a few ms to tens
    /// of ms in release builds). Allocates; call from `initialize()`.
    pub fn design(sample_rate: f64) -> Self {
        let fs = clamp_rate(sample_rate);
        if let (Some(a), Some(b)) = (table_family(fs, false), table_family(fs, true)) {
            return Self::assemble(fs, &[a, b], DesignSource::Table);
        }
        Self::design_quick(fs)
    }

    /// Always the quick design (see [`quick_family`]).
    pub fn design_quick(sample_rate: f64) -> Self {
        let fs = clamp_rate(sample_rate);
        let fits = [false, true].map(|u| quick_family(fs, u));
        Self::assemble(fs, &fits, DesignSource::Quick)
    }

    /// Always the full optimiser (tables are generated with this). Takes
    /// 0.4 .. 3 s in release builds (see the README for the per-rate numbers).
    pub fn design_full(sample_rate: f64) -> Self {
        let fs = clamp_rate(sample_rate);
        let fits = [false, true].map(|u| optimise_family(fs, u));
        Self::assemble(fs, &fits, DesignSource::Full)
    }

    /// Build all eight filters from the two fitted de-emphasis families (index
    /// 0: plain, 1: ultrasonic pole). The IEC variants add exact first-order
    /// sections; pre-emphasis is the exact inverse of de-emphasis.
    pub fn assemble(sample_rate: f64, fits: &[FamilyFit; 2], source: DesignSource) -> Self {
        let fs = clamp_rate(sample_rate);
        let k = 2.0 * fs;
        let mut cascades = [[Cascade::identity(); 4]; 2];
        for ultrasonic in [false, true] {
            let fit = fits[usize::from(ultrasonic)].cascade(fs);
            for iec in [false, true] {
                let opts = Options { iec, ultrasonic };
                // De-emphasis: fit (+ exact IEC high-pass).
                let mut de = fit;
                if iec {
                    de.push(iec_highpass_section(k));
                }
                // Pre-emphasis: inverse of the fit (+ limited IEC boost).
                let mut base = fit;
                if iec {
                    base.push(iec_limited_boost_inverse_section(k));
                }
                for (c, mode) in [(&mut de, Mode::DeEmphasis), (&mut base, Mode::PreEmphasis)] {
                    // normalise this combination to 0 dB at 1 kHz
                    let g = 1.0 / c.response(REF_HZ, fs).norm();
                    c.scale_gain(g);
                    cascades[mode_index(mode)][opts.index()] = match mode {
                        Mode::DeEmphasis => *c,
                        Mode::PreEmphasis => c.inverse(),
                    };
                }
            }
        }
        let mut reports = [[FilterReport {
            order: 0,
            max_mag_err_db: 0.0,
            max_phase_err_deg: 0.0,
        }; 4]; 2];
        for mode in [Mode::PreEmphasis, Mode::DeEmphasis] {
            for opts in Options::ALL {
                let c = &cascades[mode_index(mode)][opts.index()];
                let (m, p) = band_error(c, fs, &|f| analog_response(mode, opts, f), 1000);
                reports[mode_index(mode)][opts.index()] = FilterReport {
                    order: c.order(),
                    max_mag_err_db: m,
                    max_phase_err_deg: p,
                };
            }
        }
        Self {
            sample_rate: fs,
            source,
            cascades,
            reports,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Where the design came from.
    pub fn source(&self) -> DesignSource {
        self.source
    }

    pub fn cascade(&self, mode: Mode, opts: Options) -> &Cascade {
        &self.cascades[mode_index(mode)][opts.index()]
    }

    pub fn report(&self, mode: Mode, opts: Options) -> FilterReport {
        self.reports[mode_index(mode)][opts.index()]
    }

    /// Digital response at `f_hz`.
    pub fn response(&self, mode: Mode, opts: Options, f_hz: f64) -> Complex {
        self.cascade(mode, opts).response(f_hz, self.sample_rate)
    }
}
