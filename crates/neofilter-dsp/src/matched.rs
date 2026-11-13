//! Matched ("decramped") analog -> digital section transform.
//!
//! Following Vicanek, "Matched Second Order Digital Filters" (2016), generalised
//! to arbitrary analog sections.
//!
//! Both polynomials of a biquad are handled in the *phi basis*: with
//! `phi1 = sin^2(w/2)`, `phi0 = 1 - phi1`, `phi2 = 4 phi0 phi1`,
//! `|P(e^jw)|^2 = P0 phi0 + P1 phi1 + P2 phi2` where for `1 + a1 z^-1 + a2 z^-2`
//! `P0 = (1 + a1 + a2)^2`, `P1 = (1 - a1 + a2)^2`, `P2 = -4 a2`. A pair
//! `(B, A)` of such forms is turned back into a minimum-phase biquad in closed
//! form ([`factor`]).
//!
//! Three methods are implemented and measured (see `tests/accuracy.rs`):
//!
//! 1. [`Method::ThreePoint`]: Vicanek's recipe. Poles by impulse invariance
//!    `z = exp(p / fs)`; `B0`, `B1`, `B2` from exact magnitude matches at DC,
//!    Nyquist and one frequency near the section's pole/zero frequency.
//! 2. [`Method::LeastSquares`]: same exact poles, DC exact, the remaining two
//!    numerator coefficients by weighted relative least squares over a dense
//!    set of frequencies (half-octave grid plus clusters whose spread follows
//!    the damping of the poles and zeros). Much better than three points when
//!    features are sharp or close to Nyquist, and the only way to get a double
//!    zero at DC (low cut) right.
//! 3. [`Method::Refined`] (default): starts from 2. and, if the log-magnitude
//!    error at the sample set exceeds 0.005 dB, refines numerator *and*
//!    denominator with a few Sanathanan-Koerner iterations (each a small linear
//!    weighted least squares problem in the phi basis, DC constraint kept
//!    exact). This gives up exact impulse invariance of the poles for the sake
//!    of overall magnitude accuracy; it is what makes wide shelves and bells at
//!    several kHz accurate.
//!
//! Structure that must not be lost is preserved: a double analog zero at DC
//! stays a double zero at z = 1 (`B ~ phi1^2`), and zeros on the jw axis (notch)
//! stay exactly on the unit circle at the right frequency.
//!
//! Guard rails: every fit is checked for feasibility (non-negative forms),
//! finiteness and stability; failures fall back to the previous stage, then to
//! a prewarped bilinear transform, finally to a unity filter. Complex pole
//! angles are clamped below Nyquist. Low frequencies are handled with
//! `expm1`-based `1 + a1 + a2` so poles next to z = 1 do not cancel.

// Small dense linear algebra and phi-basis sums read better with index loops.
#![allow(clippy::needless_range_loop)]

use crate::analog::{AnalogBand, AnalogSection};
use crate::biquad::BiquadCoeffs;
use crate::types::MAX_SECTIONS;
use std::f64::consts::PI;

/// Largest digital pole angle (rad/sample) for complex poles.
const THETA_MAX: f64 = 0.98 * PI;
/// Largest third match frequency (rad/sample) of the three-point method.
const WM_MAX: f64 = 0.9 * PI;
/// Relative weight of the Nyquist point in the least-squares fit.
const NYQ_WEIGHT: f64 = 0.5;
/// Above this frequency (rad/sample, 0.4 fs) the fit weights errors less: the
/// accuracy targets are tighter below 0.4 fs than between 0.4 and 0.5 fs.
const HIGH_BAND_START: f64 = 0.8 * PI;
const HIGH_BAND_WEIGHT: f64 = 0.1;
/// Levels (power, relative to the section's maximum or 0 dB) below this start
/// to be down-weighted in all fits and validations: -40 dB.
const STOPBAND_REF: f64 = 1e-4;
/// Refinement is skipped when the stage-1 error is below this (dB).
const REFINE_THRESHOLD_DB: f64 = 0.005;
/// Maximum number of Sanathanan-Koerner iterations.
const REFINE_ITERS: usize = 4;

/// How the digital section is derived from the analog one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Vicanek: exact poles, numerator matched at DC, Nyquist and one more point.
    ThreePoint,
    /// Exact poles, numerator by weighted relative least squares.
    LeastSquares,
    /// Least squares followed by free-pole refinement (default).
    Refined,
}

/// Method used by [`match_section`].
pub const DEFAULT_METHOD: Method = Method::Refined;

/// Digital denominator with cancellation-free helper values.
struct Den {
    a1: f64,
    a2: f64,
    /// `1 + a1 + a2` (accurate also for poles next to z = 1).
    r0: f64,
    /// `1 - a1 + a2`.
    r1: f64,
    /// Natural frequency of the analog poles in rad/s.
    nat: f64,
    first_order: bool,
}

fn digital_den(sec: &AnalogSection, fs: f64) -> Option<Den> {
    let [a0, a1, a2] = sec.a;
    if a2 == 0.0 {
        if !(a1 > 0.0 && a0 > 0.0) {
            return None;
        }
        let p = -a0 / a1;
        let x = p / fs;
        let e = x.exp();
        return Some(Den {
            a1: -e,
            a2: 0.0,
            r0: -x.exp_m1(),
            r1: 1.0 + e,
            nat: -p,
            first_order: true,
        });
    }
    if !(a2 > 0.0 && a1 >= 0.0 && a0 > 0.0) {
        return None;
    }
    let half = 0.5 * a1 / a2;
    let m0 = a0 / a2;
    let disc = half * half - m0;
    let nat = m0.sqrt();
    if disc < 0.0 {
        let x = -half / fs;
        let r = x.exp();
        let theta = ((-disc).sqrt() / fs).min(THETA_MAX);
        let (s, c) = (0.5 * theta).sin_cos();
        let omr = -x.exp_m1(); // 1 - r
        Some(Den {
            a1: -2.0 * r * theta.cos(),
            a2: r * r,
            r0: omr * omr + 4.0 * r * s * s,
            r1: omr * omr + 4.0 * r * c * c,
            nat,
            first_order: false,
        })
    } else {
        let sq = disc.sqrt();
        let p1 = -(half + sq);
        let p2 = if p1 != 0.0 { m0 / p1 } else { 0.0 };
        let (x1, x2) = (p1 / fs, p2 / fs);
        let (e1, e2) = (x1.exp(), x2.exp());
        Some(Den {
            a1: -(e1 + e2),
            a2: e1 * e2,
            r0: x1.exp_m1() * x2.exp_m1(),
            r1: (1.0 + e1) * (1.0 + e2),
            nat,
            first_order: false,
        })
    }
}

/// Natural frequency (rad/s) of the numerator roots, if it has a finite
/// positive one.
fn zero_natural(sec: &AnalogSection) -> Option<f64> {
    let [b0, b1, b2] = sec.b;
    if b2 > 0.0 && b0 > 0.0 {
        Some((b0 / b2).sqrt())
    } else if b2 == 0.0 && b1 > 0.0 && b0 > 0.0 {
        Some(b0 / b1)
    } else {
        None
    }
}

fn phis(omega: f64) -> (f64, f64, f64) {
    let p1 = (0.5 * omega).sin().powi(2);
    let p0 = 1.0 - p1;
    (p0, p1, 4.0 * p0 * p1)
}

/// Quadratic form `p0 phi0 + p1 phi1 + p2 phi2`.
#[inline]
fn form_eval(p: &[f64; 3], phi1: f64) -> f64 {
    let phi0 = 1.0 - phi1;
    p[0] * phi0 + p[1] * phi1 + 4.0 * p[2] * phi0 * phi1
}

/// A form is a valid magnitude-squared of a real polynomial iff it is
/// non-negative on [0, 1].
fn form_feasible(p: &[f64; 3]) -> bool {
    if !(p[0] >= 0.0 && p[1] >= 0.0 && p.iter().all(|v| v.is_finite())) {
        return false;
    }
    let w = 0.5 * (p[0].sqrt() + p[1].sqrt());
    w * w + p[2] >= 0.0
}

/// Minimum-phase factor `(c0, c1, c2)` with `|c0 + c1 z^-1 + c2 z^-2|^2` equal
/// to the form `p`. Closed form (Vicanek).
fn factor(p: &[f64; 3]) -> Option<[f64; 3]> {
    let (r0, r1) = (p[0].max(0.0).sqrt(), p[1].max(0.0).sqrt());
    let w = 0.5 * (r0 + r1);
    let disc = w * w + p[2];
    if !(disc > -1e-12 * w * w) || !disc.is_finite() {
        return None;
    }
    let c0 = 0.5 * (w + disc.max(0.0).sqrt());
    let c2 = if c0 > 0.0 { -p[2] / (4.0 * c0) } else { 0.0 };
    Some([c0, 0.5 * (r0 - r1), c2])
}

/// Frequencies (rad/sample) at which the fits sample the analog response.
const MAX_POINTS: usize = 128;

/// Sample set: `phi1` and analog target power `T = |H|^2` per point.
struct Samples {
    phi1: [f64; MAX_POINTS],
    t: [f64; MAX_POINTS],
    wt: [f64; MAX_POINTS],
    n: usize,
}

impl Samples {
    /// `validation = true` builds an independent, denser set (quarter-octave
    /// grid, interleaved cluster offsets) used to choose between candidates.
    fn build(sec: &AnalogSection, fs: f64, features: &[(f64, f64)], validation: bool) -> Self {
        let mut s = Self {
            phi1: [0.0; MAX_POINTS],
            t: [0.0; MAX_POINTS],
            wt: [1.0; MAX_POINTS],
            n: 0,
        };
        let mut push = |w: f64| {
            if s.n < MAX_POINTS && w > 1e-12 && w < 0.995 * PI {
                let t = sec.mag_sq(w * fs);
                if t > 0.0 && t.is_finite() {
                    s.phi1[s.n] = (0.5 * w).sin().powi(2);
                    s.t[s.n] = t;
                    s.wt[s.n] = if w > HIGH_BAND_START {
                        HIGH_BAND_WEIGHT
                    } else {
                        1.0
                    };
                    s.n += 1;
                }
            }
        };
        // grid from 20 Hz (or 0.01 pi) up to 0.99 pi: half-octave for fitting,
        // quarter-octave offset by an eighth for validation
        let (lo, step) = if validation {
            ((2.0 * PI * 14.0 / fs).min(0.007 * PI), 2f64.powf(0.25))
        } else {
            (
                (2.0 * PI * 20.0 / fs).min(0.01 * PI),
                std::f64::consts::SQRT_2,
            )
        };
        let mut w = lo;
        while w < 0.99 * PI {
            push(w);
            w *= step;
        }
        push(0.99 * PI);
        if validation {
            push(0.9995 * PI);
        }
        for &(wc, zeta) in features {
            if !(wc > 0.0 && wc.is_finite()) {
                continue;
            }
            let z = zeta.clamp(1e-4, 1.0);
            let offsets: [f64; 6] = if validation {
                [0.25, 0.75, 1.5, 3.0, 6.0, 12.0]
            } else {
                [0.0, 0.5, 1.0, 2.0, 4.0, 8.0]
            };
            for u in offsets {
                push(wc * (1.0 + z * u));
                if u > 0.0 {
                    push(wc / (1.0 + z * u));
                }
            }
        }
        // Level weighting: errors in deep stop bands (relative to the largest
        // level of the section or 0 dB) are inaudible and must not steer the fit.
        let tmax = s.t[..s.n].iter().fold(
            sec.mag_sq(0.0).max(sec.mag_sq(PI * fs)).max(1.0),
            |m, &t| m.max(t),
        );
        for i in 0..s.n {
            let r = (s.t[i] / tmax / STOPBAND_REF).sqrt().clamp(0.01, 1.0);
            s.wt[i] *= r * r;
        }
        s
    }
}

/// What the numerator looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
enum NumKind {
    /// Free (`B0` fixed by the DC match).
    Generic,
    /// Analog double zero at DC: `B = Bq phi1^2`.
    DoubleDc,
    /// Zeros on the unit circle at `phi1 = phiz`: `B = K (phi1 - phiz)^2`.
    Notch { phiz: f64 },
}

/// A candidate digital section in the phi basis.
#[derive(Debug, Clone, Copy)]
struct Form {
    b: [f64; 3],
    a: [f64; 3],
}

impl Form {
    /// Sum of squared dB errors over the samples (infinite if infeasible).
    fn objective(&self, s: &Samples) -> f64 {
        if !form_feasible(&self.a) || !form_feasible(&self.b) {
            return f64::INFINITY;
        }
        let mut e = 0.0;
        for i in 0..s.n {
            let b = form_eval(&self.b, s.phi1[i]);
            let a = form_eval(&self.a, s.phi1[i]);
            if !(b > 0.0 && a > 0.0) {
                return f64::INFINITY;
            }
            let d = (b / (a * s.t[i])).ln();
            e += s.wt[i] * d * d;
        }
        e
    }
}

/// Solve the dense symmetric system `m x = y` (n <= 4) with partial pivoting
/// and a tiny ridge.
fn solve_small(m: &mut [[f64; 4]; 4], y: &mut [f64; 4], n: usize) -> Option<[f64; 4]> {
    let mut scale = 0.0f64;
    for i in 0..n {
        scale = scale.max(m[i][i]);
    }
    if !(scale > 0.0) {
        return None;
    }
    for i in 0..n {
        m[i][i] += 1e-13 * scale;
    }
    for c in 0..n {
        let mut piv = c;
        for r in c + 1..n {
            if m[r][c].abs() > m[piv][c].abs() {
                piv = r;
            }
        }
        if m[piv][c].abs() < 1e-300 {
            return None;
        }
        m.swap(c, piv);
        y.swap(c, piv);
        for r in c + 1..n {
            let f = m[r][c] / m[c][c];
            for k in c..n {
                m[r][k] -= f * m[c][k];
            }
            y[r] -= f * y[c];
        }
    }
    let mut x = [0.0; 4];
    for r in (0..n).rev() {
        let mut v = y[r];
        for k in r + 1..n {
            v -= m[r][k] * x[k];
        }
        x[r] = v / m[r][r];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Stage 1 (exact poles): numerator fit.
///
/// Generic: `B0` exact from DC; `C1 = B1 + 4 B2`, `C2 = -4 B2` by weighted
/// relative least squares (`B - B0 phi0 = phi1 (C1 + C2 phi1)`, a better
/// conditioned basis than phi1, phi2).
fn fit_numerator(kind: NumKind, s: &Samples, den: &Den, t0: f64, tn: f64) -> Option<[f64; 3]> {
    let a = [den.r0 * den.r0, den.r1 * den.r1, -4.0 * den.a2];
    let b0 = t0 * a[0];
    let (mut m11, mut m12, mut m22, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut acc = |phi1: f64, t: f64, mult: f64| {
        let phi0 = 1.0 - phi1;
        let tgt = t * form_eval(&a, phi1);
        if !(tgt > 0.0 && tgt.is_finite()) {
            return;
        }
        let v = mult / (tgt * tgt);
        match kind {
            NumKind::Generic => {
                let (c1, c2) = (phi1, phi1 * phi1);
                let rhs = tgt - b0 * phi0;
                m11 += v * c1 * c1;
                m12 += v * c1 * c2;
                m22 += v * c2 * c2;
                y1 += v * c1 * rhs;
                y2 += v * c2 * rhs;
            }
            NumKind::DoubleDc => {
                let psi = phi1 * phi1;
                m11 += v * psi * psi;
                y1 += v * psi * tgt;
            }
            NumKind::Notch { phiz } => {
                let d = phi1 - phiz;
                let psi = d * d;
                // exclude the neighbourhood of the zero, where the relative
                // weights blow up
                if psi < 1e-6 * (phi1 + phiz) * (phi1 + phiz) {
                    return;
                }
                m11 += v * psi * psi;
                y1 += v * psi * tgt;
            }
        }
    };
    for i in 0..s.n {
        acc(s.phi1[i], s.t[i], s.wt[i]);
    }
    acc(1.0, tn, NYQ_WEIGHT);
    match kind {
        NumKind::Generic => {
            let det = m11 * m22 - m12 * m12;
            if !(det > 1e-14 * m11 * m22) {
                return None;
            }
            let c1 = (y1 * m22 - y2 * m12) / det;
            let c2 = (m11 * y2 - m12 * y1) / det;
            let b2 = -0.25 * c2;
            let b1 = (c1 - 4.0 * b2).max(0.0);
            let w = 0.5 * (b0.sqrt() + b1.sqrt());
            Some([b0, b1, b2.max(-w * w)])
        }
        NumKind::DoubleDc => {
            if !(m11 > 0.0) {
                return None;
            }
            let bq = (y1 / m11).max(0.0);
            Some([0.0, bq, -0.25 * bq])
        }
        NumKind::Notch { phiz } => {
            if !(m11 > 0.0) {
                return None;
            }
            let k = (y1 / m11).max(0.0);
            Some([k * phiz * phiz, k * (1.0 - phiz) * (1.0 - phiz), -0.25 * k])
        }
    }
}

/// Stage 2: Sanathanan-Koerner refinement of numerator and denominator.
/// `A0` (scale) and, for generic numerators, `B0 = T(0) A0` stay fixed.
fn refine(kind: NumKind, first_order: bool, s: &Samples, t0: f64, start: Form) -> Form {
    let mut cur = start;
    let mut e_cur = cur.objective(s);
    if !e_cur.is_finite() {
        return start;
    }
    let a0 = start.a[0];
    for _ in 0..REFINE_ITERS {
        // unknowns: numerator parameters, then denominator (D1, D2 | A1)
        let nb = match (kind, first_order) {
            (NumKind::Generic, false) => 2,
            (NumKind::Generic, true) => 1,
            _ => 1,
        };
        let nd = if first_order { 1 } else { 2 };
        let n = nb + nd;
        let mut m = [[0.0; 4]; 4];
        let mut y = [0.0; 4];
        for i in 0..s.n {
            let phi1 = s.phi1[i];
            let phi0 = 1.0 - phi1;
            let t = s.t[i];
            let a_prev = form_eval(&cur.a, phi1);
            let w = 1.0 / (t * a_prev);
            let w2 = w * w * s.wt[i];
            let mut row = [0.0; 4];
            let rhs = match kind {
                NumKind::Generic => {
                    row[0] = phi1;
                    if !first_order {
                        row[1] = phi1 * phi1;
                    }
                    a0 * phi0 * (t - t0)
                }
                NumKind::DoubleDc => {
                    row[0] = phi1 * phi1;
                    t * a0 * phi0
                }
                NumKind::Notch { phiz } => {
                    let d = phi1 - phiz;
                    row[0] = d * d;
                    t * a0 * phi0
                }
            };
            row[nb] = -t * phi1;
            if !first_order {
                row[nb + 1] = -t * phi1 * phi1;
            }
            for r in 0..n {
                for c in 0..n {
                    m[r][c] += w2 * row[r] * row[c];
                }
                y[r] += w2 * row[r] * rhs;
            }
        }
        let Some(x) = solve_small(&mut m, &mut y, n) else {
            break;
        };
        // Map the solution to a candidate form.
        let mut cand = cur;
        match kind {
            NumKind::Generic => {
                if first_order {
                    cand.b = [t0 * a0, x[0], 0.0];
                } else {
                    let (c1, c2) = (x[0], x[1]);
                    let b2 = -0.25 * c2;
                    cand.b = [t0 * a0, c1 - 4.0 * b2, b2];
                }
            }
            NumKind::DoubleDc => {
                cand.b = [0.0, x[0], -0.25 * x[0]];
            }
            NumKind::Notch { phiz } => {
                let k = x[0];
                cand.b = [k * phiz * phiz, k * (1.0 - phiz) * (1.0 - phiz), -0.25 * k];
            }
        }
        if first_order {
            cand.a = [a0, x[nb], 0.0];
        } else {
            let (d1, d2) = (x[nb], x[nb + 1]);
            let a2 = -0.25 * d2;
            cand.a = [a0, d1 - 4.0 * a2, a2];
        }
        // Damped acceptance: blend towards the candidate until feasible and
        // better.
        let mut accepted = false;
        let mut alpha = 1.0;
        for _ in 0..4 {
            let mut f = cur;
            for k in 0..3 {
                f.b[k] = cur.b[k] + alpha * (cand.b[k] - cur.b[k]);
                f.a[k] = cur.a[k] + alpha * (cand.a[k] - cur.a[k]);
            }
            let e = f.objective(s);
            if e < e_cur {
                let gain = e_cur - e;
                cur = f;
                accepted = gain > 1e-3 * e_cur;
                e_cur = e;
                break;
            }
            alpha *= 0.5;
        }
        if !accepted {
            break;
        }
    }
    cur
}

/// Numerator coefficients (scaled so that `|N|^2` equals the form `b`).
fn numerator_coeffs(kind: NumKind, b: &[f64; 3]) -> Option<[f64; 3]> {
    match kind {
        // exact structures are built directly: no cancellation, exact zeros
        NumKind::DoubleDc => {
            let k = 0.25 * b[1].max(0.0).sqrt();
            Some([k, -2.0 * k, k])
        }
        NumKind::Notch { phiz } => {
            let k = 0.25 * (-4.0 * b[2]).max(0.0).sqrt();
            Some([k, -2.0 * k * (1.0 - 2.0 * phiz), k])
        }
        NumKind::Generic => factor(b),
    }
}

/// Convert a form pair into biquad coefficients. With `exact_poles`
/// (`a1`, `a2` of the impulse-invariant poles) the denominator is used as is
/// (no spectral factorisation, no cancellation near z = 1).
fn form_to_coeffs(
    kind: NumKind,
    first_order: bool,
    f: &Form,
    exact_poles: Option<(f64, f64)>,
) -> Option<BiquadCoeffs> {
    if first_order {
        let (a1, u) = match exact_poles {
            Some((a1, _)) => (a1, 1.0),
            None => {
                let (ra, rb) = (f.a[0].sqrt(), f.a[1].sqrt());
                ((ra - rb) / (ra + rb), 0.5 * (ra + rb))
            }
        };
        let (nb0, nb1) = (f.b[0].max(0.0).sqrt(), f.b[1].max(0.0).sqrt());
        return Some(BiquadCoeffs {
            b0: 0.5 * (nb0 + nb1) / u,
            b1: 0.5 * (nb0 - nb1) / u,
            b2: 0.0,
            a1,
            a2: 0.0,
        });
    }
    let (u, a1, a2) = match exact_poles {
        Some((a1, a2)) => (1.0, a1, a2),
        None => {
            let [u, d1, d2] = factor(&f.a)?;
            if !(u > 0.0) {
                return None;
            }
            (u, d1 / u, d2 / u)
        }
    };
    let n = numerator_coeffs(kind, &f.b)?;
    Some(BiquadCoeffs {
        b0: n[0] / u,
        b1: n[1] / u,
        b2: n[2] / u,
        a1,
        a2,
    })
}

/// Prewarped bilinear transform of one section (fallback).
fn bilinear(sec: &AnalogSection, fs: f64, wp: f64) -> BiquadCoeffs {
    let x = (0.5 * wp / fs).clamp(1e-9, 0.49 * PI);
    let k = 2.0 * fs * x / x.tan();
    let tf = |c: &[f64; 3]| {
        [
            c[0] + c[1] * k + c[2] * k * k,
            2.0 * (c[0] - c[2] * k * k),
            c[0] - c[1] * k + c[2] * k * k,
        ]
    };
    let n = tf(&sec.b);
    let d = tf(&sec.a);
    BiquadCoeffs {
        b0: n[0] / d[0],
        b1: n[1] / d[0],
        b2: n[2] / d[0],
        a1: d[1] / d[0],
        a2: d[2] / d[0],
    }
}

/// Matched digital biquad for one analog section at sample rate `fs`.
///
/// Never returns non-finite or unstable coefficients (falls back to a
/// prewarped bilinear transform, and ultimately to a unity filter).
pub fn match_section(sec: &AnalogSection, fs: f64) -> BiquadCoeffs {
    match_section_with(sec, fs, DEFAULT_METHOD)
}

/// [`match_section`] with an explicit method (for tests and measurements).
pub fn match_section_with(sec: &AnalogSection, fs: f64, method: Method) -> BiquadCoeffs {
    match try_match(sec, fs, method) {
        Some(c) if c.is_finite() && c.is_stable() => c,
        _ => {
            let nat = digital_den(sec, fs).map_or(2.0 * PI * 1000.0, |d| d.nat);
            let c = bilinear(sec, fs, nat);
            if c.is_finite() && c.is_stable() {
                c
            } else {
                BiquadCoeffs::IDENTITY
            }
        }
    }
}

/// Vicanek's three-point numerator (see module docs). `None` if there is no
/// real minimum-phase solution at any of the tried third frequencies.
fn three_point(
    sec: &AnalogSection,
    den: &Den,
    fs: f64,
    r0: f64,
    r1: f64,
    znat: Option<f64>,
) -> Option<[f64; 3]> {
    let nat = match znat {
        Some(z) => (z * den.nat).sqrt(),
        None => den.nat,
    };
    let base = (nat / fs).clamp(1e-9, WM_MAX);
    for wm in [base, 0.5 * base, 0.25 * base, (2.0 * base).min(WM_MAX)] {
        let (p0, p1, p2) = phis(wm);
        if !(p2 > 0.0) {
            continue;
        }
        let a_w = den.r0 * den.r0 * p0 + den.r1 * den.r1 * p1 - 4.0 * den.a2 * p2;
        let b2 = (sec.mag_sq(wm * fs) * a_w - r0 * r0 * p0 - r1 * r1 * p1) / p2;
        let w = 0.5 * (r0 + r1);
        let disc = w * w + b2;
        if !(disc >= 0.0) || !b2.is_finite() {
            continue;
        }
        let b0 = 0.5 * (w + disc.sqrt());
        let bb2 = if b0 > 0.0 { -b2 / (4.0 * b0) } else { 0.0 };
        return Some([b0, 0.5 * (r0 - r1), bb2]);
    }
    None
}

fn damping(sec: &AnalogSection, nat: f64) -> f64 {
    if sec.a[2] > 0.0 {
        (0.5 * sec.a[1] / (sec.a[2] * nat)).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn try_match(sec: &AnalogSection, fs: f64, method: Method) -> Option<BiquadCoeffs> {
    let den = digital_den(sec, fs)?;
    let t0 = sec.mag_sq(0.0);
    let tn = sec.mag_sq(PI * fs);
    let (r0, r1) = (den.r0 * t0.sqrt(), den.r1 * tn.sqrt());
    if !(r0.is_finite() && r1.is_finite()) {
        return None;
    }
    let znat = zero_natural(sec);
    let first_order = den.first_order;
    let notch = !first_order && sec.b[1] == 0.0 && sec.b[0] > 0.0 && sec.b[2] > 0.0;
    let kind = if notch {
        let theta = (znat? / fs).min(THETA_MAX);
        NumKind::Notch {
            phiz: (0.5 * theta).sin().powi(2),
        }
    } else if sec.b[0] == 0.0 && sec.b[1] == 0.0 && sec.b[2] != 0.0 {
        NumKind::DoubleDc
    } else {
        NumKind::Generic
    };

    let exact_poles = Some((den.a1, den.a2));
    let a_form = [den.r0 * den.r0, den.r1 * den.r1, -4.0 * den.a2];

    // Sample sets with clusters around the pole and zero features.
    let zeta_p = damping(sec, den.nat);
    let mut feats = [(den.nat / fs, zeta_p), (0.0, 1.0)];
    if let Some(z) = znat {
        let zz = if sec.b[2] > 0.0 && sec.b[0] > 0.0 {
            0.5 * sec.b[1] / (sec.b[0] * sec.b[2]).sqrt()
        } else {
            1.0
        };
        feats[1] = (z / fs, zz);
    }
    let feats: &[(f64, f64)] = if first_order { &feats[..1] } else { &feats };

    // Stage 1: exact poles, numerator by least squares (or exact for
    // first-order sections).
    let (start, stage1) = if first_order {
        let start = Form {
            b: [r0 * r0, r1 * r1, 0.0],
            a: [a_form[0], a_form[1], 0.0],
        };
        (start, form_to_coeffs(kind, true, &start, exact_poles)?)
    } else {
        if method == Method::ThreePoint && kind == NumKind::Generic {
            let [b0, b1, b2] = three_point(sec, &den, fs, r0, r1, znat)?;
            return Some(BiquadCoeffs {
                b0,
                b1,
                b2,
                a1: den.a1,
                a2: den.a2,
            });
        }
        let pts = Samples::build(sec, fs, feats, false);
        let Some(b) = fit_numerator(kind, &pts, &den, t0, tn) else {
            // numerically hopeless sample set: three-point fallback
            let [b0, b1, b2] = three_point(sec, &den, fs, r0, r1, znat)?;
            return Some(BiquadCoeffs {
                b0,
                b1,
                b2,
                a1: den.a1,
                a2: den.a2,
            });
        };
        let start = Form { b, a: a_form };
        (start, form_to_coeffs(kind, false, &start, exact_poles)?)
    };
    if method != Method::Refined {
        return Some(stage1);
    }

    // Stage 2: pick the best candidate on an independent validation grid.
    let val = Samples::build(sec, fs, feats, true);
    let m1 = coeff_error_db(&stage1, &val);
    if m1 <= REFINE_THRESHOLD_DB {
        return Some(stage1);
    }
    let mut best = (m1, stage1);
    let mut consider = |c: BiquadCoeffs| {
        if c.is_finite() && c.is_stable() {
            let m = coeff_error_db(&c, &val);
            if m < best.0 {
                best = (m, c);
            }
        }
    };
    if !first_order && kind == NumKind::Generic {
        if let Some([b0, b1, b2]) = three_point(sec, &den, fs, r0, r1, znat) {
            consider(BiquadCoeffs {
                b0,
                b1,
                b2,
                a1: den.a1,
                a2: den.a2,
            });
        }
    }
    let train = Samples::build(sec, fs, feats, false);
    let refined = refine(kind, first_order, &train, t0, start);
    if let Some(c) = form_to_coeffs(kind, first_order, &refined, None) {
        consider(c);
    }
    Some(best.1)
}

/// Worst weighted dB error of a section over a sample set (what is heard,
/// evaluated from the actual coefficients). Errors are weighted like in the
/// fit (stop band level, above 0.4 fs).
fn coeff_error_db(c: &BiquadCoeffs, s: &Samples) -> f64 {
    let f = c.phi_form();
    let mut m: f64 = 0.0;
    for i in 0..s.n {
        let p = f.mag_sq_phi1(s.phi1[i]);
        if !(p > 0.0 && p.is_finite()) {
            return f64::INFINITY;
        }
        let e = (10.0 / std::f64::consts::LN_10 * (p / s.t[i]).ln()).abs();
        m = m.max(e * s.wt[i].sqrt());
    }
    m
}

/// Matched digital cascade for a whole analog band. Returns the number of
/// sections written to `out`.
pub fn match_band(analog: &AnalogBand, fs: f64, out: &mut [BiquadCoeffs; MAX_SECTIONS]) -> usize {
    let n = analog.len();
    for (o, s) in out.iter_mut().zip(analog.sections()) {
        *o = match_section(s, fs);
    }
    n
}
