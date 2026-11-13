//! Reference-matched digital design rules (see `docs/reference-matching.md`).
//!
//! Every 2nd-order analog section `(s^2 + bz s + cz) / (s^2 + bp s + cp)` becomes
//! a digital biquad whose *squared magnitude* equals the analog section's at
//! five frequencies: DC, three interior points and Nyquist, where the Nyquist
//! value is the analog value at some `f_x <= Nyquist`. The squared magnitude of
//! a biquad is linear in `(X0, X1, X2)` in the phi basis (numerator and
//! denominator), so the five conditions are a homogeneous 5x6 linear system;
//! its null vector is factored into a minimum-phase biquad.
//!
//! Everything here is allocation-free, panic-free and bounded in run time, so
//! it may be called from the audio thread. Functions return `None` when the
//! rule cannot be applied; callers fall back to [`crate::matched`].

use crate::analog::AnalogSection;
use crate::biquad::BiquadCoeffs;
use std::f64::consts::PI;

/// Maximum bisection steps when lowering the Nyquist target.
const BISECT_ITERS: usize = 32;
/// Grid steps tried when even the lowest Nyquist target is not realisable.
const SCAN_STEPS: usize = 48;
/// Relative tolerance of the self-check of the squared magnitude at the five
/// match points.
const SELF_CHECK_REL: f64 = 1e-6;

/// A monic analog section `(s^2 + bz s + cz) / (s^2 + bp s + cp)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monic {
    pub bz: f64,
    pub cz: f64,
    pub bp: f64,
    pub cp: f64,
}

impl Monic {
    /// Monic form of an analog section with non-zero `s^2` coefficients.
    pub fn from_section(s: &AnalogSection) -> Option<Self> {
        if !(s.b[2] > 0.0 && s.a[2] > 0.0) {
            return None;
        }
        let m = Self {
            bz: s.b[1] / s.b[2],
            cz: s.b[0] / s.b[2],
            bp: s.a[1] / s.a[2],
            cp: s.a[0] / s.a[2],
        };
        // Squared magnitude equals the section's only if the gains agree.
        let ok = (s.b[2] / s.a[2] - 1.0).abs() < 1e-9 && m.cp > 0.0 && m.cp.is_finite();
        ok.then_some(m)
    }

    /// Monic form of a section whose leading coefficients differ by the
    /// constant gain `k = b2 / a2` (returned): `section = k * monic`.
    pub fn from_section_with_gain(s: &AnalogSection) -> Option<(Self, f64)> {
        if !(s.b[2] > 0.0 && s.a[2] > 0.0) {
            return None;
        }
        let k = s.b[2] / s.a[2];
        let m = Self {
            bz: s.b[1] / s.b[2],
            cz: s.b[0] / s.b[2],
            bp: s.a[1] / s.a[2],
            cp: s.a[0] / s.a[2],
        };
        (k.is_finite() && k > 0.0 && m.cp > 0.0 && m.cp.is_finite()).then_some((m, k))
    }

    /// Squared magnitude at `f` Hz.
    pub fn mag_sq(&self, f: f64) -> f64 {
        let w2 = (2.0 * PI * f).powi(2);
        let n = (self.cz - w2).powi(2) + self.bz * self.bz * w2;
        let d = (self.cp - w2).powi(2) + self.bp * self.bp * w2;
        n / d
    }
}

/// Null vector of a 5x6 matrix (rank 5) by Gaussian elimination with full
/// pivoting: about a hundred flops. Returns `None` on a zero pivot.
fn null_vector_ge(rows: &[[f64; 6]; 5]) -> Option<[f64; 6]> {
    let mut a = *rows;
    let mut perm = [0usize, 1, 2, 3, 4, 5];
    for k in 0..5 {
        let (mut pr, mut pc, mut best) = (k, k, 0.0f64);
        for (i, row) in a.iter().enumerate().skip(k) {
            for (j, v) in row.iter().enumerate().skip(k) {
                if v.abs() > best {
                    best = v.abs();
                    pr = i;
                    pc = j;
                }
            }
        }
        if !(best > 0.0) {
            return None;
        }
        a.swap(k, pr);
        if pc != k {
            for row in a.iter_mut() {
                row.swap(k, pc);
            }
            perm.swap(k, pc);
        }
        let pivot = a[k];
        for row in a.iter_mut().skip(k + 1) {
            let m = row[k] / pivot[k];
            for (e, p) in row.iter_mut().zip(&pivot).skip(k) {
                *e -= m * p;
            }
        }
    }
    let mut y = [0.0f64; 6];
    y[5] = 1.0;
    for k in (0..5).rev() {
        let mut sum = 0.0;
        for j in (k + 1)..6 {
            sum += a[k][j] * y[j];
        }
        y[k] = -sum / a[k][k];
    }
    let n = y.iter().map(|e| e * e).sum::<f64>().sqrt();
    let mut x = [0.0f64; 6];
    for k in 0..6 {
        x[perm[k]] = y[k] / n;
    }
    x.iter().all(|e| e.is_finite()).then_some(x)
}

/// Null vector of a 5x6 matrix (rank 5) by one-sided Jacobi (Hestenes) SVD of
/// the 6x6 matrix padded with a zero row. Highly relative-accurate, no heap.
fn null_vector(rows: &[[f64; 6]; 5]) -> Option<[f64; 6]> {
    let mut m = [[0.0f64; 6]; 6];
    m[..5].copy_from_slice(rows);
    let mut v = [[0.0f64; 6]; 6];
    for (i, r) in v.iter_mut().enumerate() {
        r[i] = 1.0;
    }
    for _ in 0..40 {
        let mut rotated = false;
        for p in 0..5 {
            for q in (p + 1)..6 {
                let (mut alpha, mut beta, mut gamma) = (0.0, 0.0, 0.0);
                for r in &m {
                    alpha += r[p] * r[p];
                    beta += r[q] * r[q];
                    gamma += r[p] * r[q];
                }
                if gamma == 0.0 || gamma.abs() <= 1e-16 * (alpha * beta).sqrt() {
                    continue;
                }
                rotated = true;
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                for r in m.iter_mut().chain(v.iter_mut()) {
                    let (x, y) = (r[p], r[q]);
                    r[p] = c * x - s * y;
                    r[q] = s * x + c * y;
                }
            }
        }
        if !rotated {
            break;
        }
    }
    let mut best = 0;
    let mut best_n = f64::INFINITY;
    for k in 0..6 {
        let n: f64 = m.iter().map(|r| r[k] * r[k]).sum();
        if n < best_n {
            best_n = n;
            best = k;
        }
    }
    let x: [f64; 6] = std::array::from_fn(|i| v[i][best]);
    x.iter().all(|e| e.is_finite()).then_some(x)
}

/// Minimum-phase biquad numerator/denominator `[c0, c1, c2]` from phi-basis
/// squared-magnitude coefficients `(X0, X1, X2)`.
fn factor(x: [f64; 3]) -> Option<[f64; 3]> {
    let [x0, x1, x2] = x;
    if !(x0 >= 0.0 && x1 >= 0.0) {
        return None;
    }
    let w = 0.5 * (x0.sqrt() + x1.sqrt());
    let disc = w * w + x2;
    if !(disc >= 0.0) {
        return None;
    }
    let c0 = 0.5 * (w + disc.sqrt());
    if !(c0 > 0.0) {
        return None;
    }
    Some([c0, 0.5 * (x0.sqrt() - x1.sqrt()), -x2 / (4.0 * c0)])
}

/// Five-point magnitude interpolation. `pts` are `(digital_freq, analog_freq)`
/// pairs in Hz (the first must be DC); the digital squared magnitude equals
/// `magsq(analog_freq)` at `digital_freq`. Returns `None` if the solution is
/// not realisable (negative power terms, no real factorisation, unstable) or
/// fails its own self-check.
///
/// The system is solved in the basis `(1, t, t^2)`, `t = u / u_s`,
/// `u = sin^2(pi f / fs)`, `u_s` the largest interior `u`, rows scaled to unit
/// maximum, so that low frequencies at high sample rates stay conditioned.
pub fn interpolate<F: Fn(f64) -> f64>(
    magsq: &F,
    pts: &[(f64, f64); 5],
    fs: f64,
) -> Option<BiquadCoeffs> {
    let us: [f64; 5] = std::array::from_fn(|i| (PI * pts[i].0 / fs).sin().powi(2));
    let u_s = us
        .iter()
        .copied()
        .filter(|&u| u > 0.0 && u < 0.999999)
        .fold(0.0, f64::max);
    let u_s = if u_s > 0.0 { u_s } else { 1.0 };
    let mut vs = [0.0; 5];
    let mut rows = [[0.0; 6]; 5];
    for i in 0..5 {
        let v = magsq(pts[i].1);
        if !(v > 0.0 && v.is_finite()) {
            return None;
        }
        vs[i] = v;
        let t = us[i] / u_s;
        let mut r = [1.0, t, t * t, -v, -v * t, -v * t * t];
        let mx = r.iter().fold(0.0f64, |m, e| m.max(e.abs()));
        r.iter_mut().for_each(|e| *e /= mx);
        rows[i] = r;
    }
    // Fast elimination first; the self-check below rejects an inaccurate
    // result, in which case the slower, more accurate Jacobi SVD is used.
    if let Some(c) = null_vector_ge(&rows).and_then(|x| realise(x, u_s, &us, &vs)) {
        return Some(c);
    }
    realise(null_vector(&rows)?, u_s, &us, &vs)
}

/// Biquad from a null vector (see [`interpolate`]), self-checked at the match
/// points (`us` are their `sin^2`, `vs` the target squared magnitudes).
fn realise(mut x: [f64; 6], u_s: f64, us: &[f64; 5], vs: &[f64; 5]) -> Option<BiquadCoeffs> {
    if x[3] < 0.0 {
        x.iter_mut().for_each(|e| *e = -*e);
    }
    let to_x = |c: &[f64]| {
        let (c0, c1, c2) = (c[0], c[1] / u_s, c[2] / (u_s * u_s));
        [c0, c0 + c1 + c2, -c2 / 4.0]
    };
    let b = factor(to_x(&x[..3]))?;
    let a = factor(to_x(&x[3..]))?;
    let a0 = a[0];
    let c = BiquadCoeffs {
        b0: b[0] / a0,
        b1: b[1] / a0,
        b2: b[2] / a0,
        a1: a[1] / a0,
        a2: a[2] / a0,
    };
    if !c.is_finite() || !c.is_stable() {
        return None;
    }
    let form = c.phi_form();
    for i in 0..5 {
        let p = form.mag_sq_phi1(us[i]);
        if !((p / vs[i] - 1.0).abs() <= SELF_CHECK_REL) {
            return None;
        }
    }
    Some(c)
}

/// [`interpolate`] with DC, the three `interior` frequencies and Nyquist
/// (taking the analog value at `fx`). If not realisable, `fx` is lowered to the
/// largest realisable value (bounded bisection).
pub fn interpolate_with_nyquist_fallback<F: Fn(f64) -> f64>(
    magsq: &F,
    interior: [f64; 3],
    fs: f64,
    fx: f64,
) -> Option<BiquadCoeffs> {
    let nyq = 0.5 * fs;
    let mk = |fxx: f64| {
        interpolate(
            magsq,
            &[
                (0.0, 0.0),
                (interior[0], interior[0]),
                (interior[1], interior[1]),
                (interior[2], interior[2]),
                (nyq, fxx),
            ],
            fs,
        )
    };
    if let Some(r) = mk(fx) {
        return Some(r);
    }
    let imax = interior.iter().copied().fold(0.0, f64::max);
    let mut hi = fx;
    let mut lo = (imax * 1.0001).min(hi);
    if mk(lo).is_none() {
        let mut found = false;
        for i in 0..SCAN_STEPS {
            let t = hi + (lo - hi) * (i as f64 / (SCAN_STEPS - 1) as f64);
            if mk(t).is_some() {
                lo = t;
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    for _ in 0..BISECT_ITERS {
        let mid = 0.5 * (lo + hi);
        if mk(mid).is_some() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    mk(lo)
}

/// Positive extremum frequencies (Hz, ascending, at most two) of the section
/// magnitude, and their count.
pub fn extrema(m: &Monic) -> ([f64; 2], usize) {
    let n1 = m.bz * m.bz - 2.0 * m.cz;
    let n0 = m.cz * m.cz;
    let d1 = m.bp * m.bp - 2.0 * m.cp;
    let d0 = m.cp * m.cp;
    let a = d1 - n1;
    let b = 2.0 * (d0 - n0);
    let c = n1 * d0 - n0 * d1;
    let scale = (a.abs() * m.cp * m.cp)
        .max(b.abs() * m.cp)
        .max(c.abs())
        .max(1e-300);
    let mut r = [0.0f64; 2];
    let mut n = 0;
    if a.abs() * m.cp * m.cp < 1e-12 * scale {
        if b.abs() * m.cp > 1e-12 * scale {
            r[0] = -c / b;
            n = 1;
        }
    } else {
        let disc = b * b - 4.0 * a * c;
        if !(disc >= 0.0) {
            return ([0.0; 2], 0);
        }
        let s = disc.sqrt();
        let (x1, x2) = ((-b - s) / (2.0 * a), (-b + s) / (2.0 * a));
        r = if x1 <= x2 { [x1, x2] } else { [x2, x1] };
        n = 2;
    }
    let mut out = [0.0f64; 2];
    let mut k = 0;
    for &x in &r[..n] {
        if x > 0.0 && x.is_finite() {
            out[k] = x.sqrt() / (2.0 * PI);
            k += 1;
        }
    }
    (out, k)
}

/// Largest deviation (dB) tolerated between a designed section and its analog
/// prototype in the guarded range, see [`tracks_analog`].
const GUARD_DB: f64 = 0.5;
/// Points of the guard grid.
const GUARD_POINTS: usize = 24;

/// Sanity guard for the Bell rule: the five match points leave the response
/// between them unconstrained, and with a poorly chosen extremum the result
/// can be far off in the audible range. Up to 0.1 fs the reference and the
/// analog prototype agree to well within [`GUARD_DB`] on all measured cases,
/// so a larger deviation means this candidate must not be used.
fn tracks_analog(c: &BiquadCoeffs, m: &Monic, fs: f64) -> bool {
    let (lo, hi) = (10.0, 0.1 * fs);
    if !(hi > lo) {
        return true;
    }
    let form = c.phi_form();
    let ratio = (hi / lo).powf(1.0 / (GUARD_POINTS - 1) as f64);
    let mut f = lo;
    for _ in 0..GUARD_POINTS {
        let d = form.mag_sq_phi1((PI * f / fs).sin().powi(2));
        let a = m.mag_sq(f);
        if !(d > 0.0 && a > 0.0) || (10.0 * (d / a).log10()).abs() > GUARD_DB {
            return false;
        }
        f *= ratio;
    }
    true
}

/// Bell rule for one analog section (`q` is the displayed Q).
///
/// For sections with two extrema the rule picks one (see
/// `docs/reference-matching.md`); if that candidate fails the guard
/// ([`tracks_analog`]) the other extremum is tried, and if that fails as well
/// the result is `None` (the caller falls back to the matched design).
pub fn design_bell_section(sec: &AnalogSection, q: f64, fs: f64) -> Option<BiquadCoeffs> {
    bell_section(sec, q, fs, true)
}

/// The Bell rule exactly as identified, without the analog-tracking guard and
/// without trying the other extremum. For tests and diagnostics: it shows
/// whether the guard is still needed.
pub fn design_bell_section_rule_only(sec: &AnalogSection, q: f64, fs: f64) -> Option<BiquadCoeffs> {
    bell_section(sec, q, fs, false)
}

fn bell_section(sec: &AnalogSection, q: f64, fs: f64, guarded: bool) -> Option<BiquadCoeffs> {
    let m = Monic::from_section(sec)?;
    let nyq = 0.5 * fs;
    let ms = |f: f64| m.mag_sq(f);
    if (m.cz - m.cp).abs() <= 1e-9 * m.cp {
        // bell-type section: zero and pole at the same frequency
        let f0 = m.cp.sqrt() / (2.0 * PI);
        let d = (1.0 / q).clamp(0.1, 0.8);
        let interior = [f0 * (1.0 - d), f0 * (1.0 - 0.05 * d), f0];
        let fx = (1.2 * f0).max(0.9 * nyq).min(nyq);
        return bell_candidate(&m, interior, fs, fx, guarded);
    }
    let (ex, n) = extrema(&m);
    if n == 0 {
        return None;
    }
    let mut pick = 0;
    if n == 2 && ex[1] < 0.999 * nyq {
        // The lower extremum is skipped only when it is insignificant (the
        // section hardly differs from its DC level there), or when it is far
        // below the upper one and still shallow. A lower extremum that is a
        // real peak must be kept (see `docs/reference-matching.md`).
        let depth = (1.0 - ms(ex[0]) / ms(0.0)).abs();
        if depth < 1e-5 || (ex[0] < 0.1 * ex[1] && depth < 1e-3) {
            pick = 1;
        }
    }
    let d = (0.5 / q + 0.006).min(0.95);
    let try_ext = |fe: f64| {
        let interior = [fe * (1.0 - d), fe * 0.999, fe];
        let fx = (1.8 * fe).max(0.83 * nyq).min(nyq);
        bell_candidate(&m, interior, fs, fx, guarded)
    };
    if let Some(c) = try_ext(ex[pick]) {
        return Some(c);
    }
    if guarded && n == 2 {
        return try_ext(ex[1 - pick]);
    }
    None
}

fn bell_candidate(
    m: &Monic,
    interior: [f64; 3],
    fs: f64,
    fx: f64,
    guarded: bool,
) -> Option<BiquadCoeffs> {
    if !(interior[2] < 0.999 * 0.5 * fs && interior[0] > 0.0) {
        return None;
    }
    let c = interpolate_with_nyquist_fallback(&|f| m.mag_sq(f), interior, fs, fx)?;
    (!guarded || tracks_analog(&c, m, fs)).then_some(c)
}

/// Tolerance on `|Q - 1|` for the default-Q shelf rule.
pub const DEFAULT_Q_TOLERANCE: f64 = 1e-3;

/// Default-Q shelf rule for the single 2nd-order section of a 12 dB/oct Low /
/// High / Tilt Shelf (the caller checks shape, slope and the displayed Q, see
/// [`DEFAULT_Q_TOLERANCE`]). `f0` is the band frequency in Hz. The section may
/// carry a constant gain (`b2 / a2` of a high shelf or tilt), which is applied
/// to the numerator afterwards. Five points: DC, `c (1 - d)`, `c (1 - 0.05 d)`,
/// `c` with `c = f0 / 2`, `d = 1 / sqrt 2`, and Nyquist at the analog value at
/// Nyquist. `None` if `c` is not below 0.999 Nyquist or the solution is not
/// realisable; the caller then uses the matched design.
pub fn design_shelf_section(sec: &AnalogSection, f0: f64, fs: f64) -> Option<BiquadCoeffs> {
    let (m, k) = Monic::from_section_with_gain(sec)?;
    let nyq = 0.5 * fs;
    let c = 0.5 * f0;
    if !(c > 0.0 && c < 0.999 * nyq) {
        return None;
    }
    let d = std::f64::consts::FRAC_1_SQRT_2;
    let pts = [
        (0.0, 0.0),
        (c * (1.0 - d), c * (1.0 - d)),
        (c * (1.0 - 0.05 * d), c * (1.0 - 0.05 * d)),
        (c, c),
        (nyq, nyq),
    ];
    let mut r = interpolate(&|f| m.mag_sq(f), &pts, fs)?;
    r.b0 *= k;
    r.b1 *= k;
    r.b2 *= k;
    r.is_finite().then_some(r)
}

/// High-cut rule for one 2nd-order Butterworth-type low-pass section
/// `1 / (1 + (d/w0) s + s^2/w0^2)`.
pub fn design_highcut_section(sec: &AnalogSection, fs: f64) -> Option<BiquadCoeffs> {
    let nyq = 0.5 * fs;
    if !(sec.a[0] > 0.0 && sec.a[1] > 0.0 && sec.a[2] > 0.0) || sec.b[1] != 0.0 || sec.b[2] != 0.0 {
        return None;
    }
    let w0 = (sec.a[0] / sec.a[2]).sqrt();
    let f0 = w0 / (2.0 * PI);
    let q = (sec.a[0] * sec.a[2]).sqrt() / sec.a[1];
    let c = if q > std::f64::consts::FRAC_1_SQRT_2 {
        (1.0 - 1.0 / (2.0 * q * q)).sqrt().max(0.5)
    } else {
        0.5
    };
    if !(f0 < 0.999 * nyq) {
        return None;
    }
    let ms = |f: f64| sec.mag_sq(2.0 * PI * f);
    interpolate_with_nyquist_fallback(&ms, [0.5 * c * f0, c * f0, f0], fs, nyq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analog::AnalogBand;
    use crate::types::{Shape, Slope};

    fn db(p: f64) -> f64 {
        10.0 * p.log10()
    }

    fn bell_sections(f0: f64, g: f64, q: f64, slope: Slope) -> AnalogBand {
        AnalogBand::design(Shape::Bell, f0, g, q, slope)
    }

    #[test]
    fn solver_hits_all_five_points() {
        let fs = 48000.0;
        let a = bell_sections(3000.0, 9.0, 2.0, Slope::Db12);
        let m = Monic::from_section(&a.sections()[0]).unwrap();
        let ms = |f: f64| m.mag_sq(f);
        let pts = [
            (0.0, 0.0),
            (2400.0, 2400.0),
            (2850.0, 2850.0),
            (3000.0, 3000.0),
            (24000.0, 3600.0),
        ];
        let c = interpolate(&ms, &pts, fs).expect("realisable");
        assert!(c.is_stable());
        for (fd, fa) in pts {
            let got = c.mag_sq(2.0 * PI * fd / fs);
            let want = ms(fa);
            assert!(((got - want) / want).abs() < 1e-9, "{fd}: {got} vs {want}");
        }
    }

    #[test]
    fn solver_stays_accurate_at_low_frequency_and_high_rate() {
        let fs = 192000.0;
        let a = bell_sections(30.0, 12.0, 4.0, Slope::Db12);
        let m = Monic::from_section(&a.sections()[0]).unwrap();
        let c = design_bell_section(&a.sections()[0], 4.0, fs).unwrap();
        for f in [5.0, 20.0, 30.0, 45.0, 100.0, 1000.0] {
            let got = db(c.mag_sq(2.0 * PI * f / fs));
            assert!((got - db(m.mag_sq(f))).abs() < 1e-6, "{f}: {got}");
        }
    }

    #[test]
    fn boost_and_cut_are_exact_inverses() {
        for slope in [Slope::Db12, Slope::Db24, Slope::Db48, Slope::Db96] {
            for &(f0, q, fs) in &[(1000.0, 1.0, 48000.0), (9000.0, 3.0, 44100.0)] {
                let up = bell_sections(f0, 12.0, q, slope);
                let dn = bell_sections(f0, -12.0, q, slope);
                let total = |a: &AnalogBand, f: f64| -> f64 {
                    a.sections()
                        .iter()
                        .map(|s| {
                            let c = design_bell_section(s, q, fs).unwrap();
                            db(c.mag_sq(2.0 * PI * f / fs))
                        })
                        .sum()
                };
                for i in 0..200 {
                    let f = 20.0 * (0.49 * fs / 20.0f64).powf(i as f64 / 199.0);
                    let s = total(&up, f) + total(&dn, f);
                    assert!(s.abs() < 1e-8, "{slope:?} {f0} {f}: {s}");
                }
            }
        }
    }

    #[test]
    fn low_bell_agrees_with_the_analog_prototype() {
        let fs = 48000.0;
        let a = bell_sections(200.0, 9.0, 1.0, Slope::Db12);
        let c = design_bell_section(&a.sections()[0], 1.0, fs).unwrap();
        for i in 0..100 {
            let f = 20.0 * (2000.0f64 / 20.0).powf(i as f64 / 99.0);
            let d = db(c.mag_sq(2.0 * PI * f / fs)) - a.magnitude_db(f);
            assert!(d.abs() < 0.01, "{f}: {d}");
        }
    }

    #[test]
    fn extrema_of_bell_sections() {
        // 12 dB/oct bell: the single extremum is the band frequency
        let a = bell_sections(1000.0, 6.0, 1.0, Slope::Db12);
        let (e, n) = extrema(&Monic::from_section(&a.sections()[0]).unwrap());
        assert_eq!(n, 1);
        assert!((e[0] - 1000.0).abs() < 1e-6, "{e:?}");
        // 24 dB/oct bell: each section's extrema are stationary points
        let a = bell_sections(1000.0, 12.0, 2.0, Slope::Db24);
        for s in a.sections() {
            let m = Monic::from_section(s).unwrap();
            let (e, n) = extrema(&m);
            assert!(n >= 1);
            for &f in &e[..n] {
                let d = (m.mag_sq(f * 1.0001).ln() - m.mag_sq(f / 1.0001).ln()).abs();
                assert!(d < 1e-6, "{f}: {d}");
            }
        }
        // a section without a positive stationary point: pure real pole pair
        let flat = Monic {
            bz: 2.0,
            cz: 1.0,
            bp: 2.0,
            cp: 1.0,
        };
        let (_, n) = extrema(&flat);
        assert_eq!(n, 0);
    }

    /// 13.6 kHz, Q 1, 48 dB/oct high cut at 48 kHz: with Nyquist matched to the
    /// analog value the highest-Q section is not realisable, so the Nyquist
    /// target must be lowered.
    #[test]
    fn nyquist_fallback_is_needed_and_works() {
        let fs = 48000.0;
        let a = AnalogBand::design(Shape::HighCut, 13607.0, 0.0, 1.0, Slope::Db48);
        let mut needed = 0;
        for s in a.sections() {
            let c = design_highcut_section(s, fs).expect("fallback finds a solution");
            assert!(c.is_stable() && c.is_finite());
            let (f0, q) = (
                (s.a[0] / s.a[2]).sqrt() / (2.0 * PI),
                (s.a[0] * s.a[2]).sqrt() / s.a[1],
            );
            let cc = if q > std::f64::consts::FRAC_1_SQRT_2 {
                (1.0 - 0.5 / (q * q)).sqrt().max(0.5)
            } else {
                0.5
            };
            let ms = |f: f64| s.mag_sq(2.0 * PI * f);
            let direct = interpolate(
                &ms,
                &[
                    (0.0, 0.0),
                    (0.5 * cc * f0, 0.5 * cc * f0),
                    (cc * f0, cc * f0),
                    (f0, f0),
                    (0.5 * fs, 0.5 * fs),
                ],
                fs,
            );
            if direct.is_none() {
                needed += 1;
            }
            // DC and the interior points are still hit
            let got = c.mag_sq(2.0 * PI * f0 / fs);
            assert!(((got - ms(f0)) / ms(f0)).abs() < 1e-5);
        }
        assert!(needed >= 1, "the fallback was not exercised");
    }

    /// Largest deviation (dB) between a designed section and its analog
    /// prototype between 10 Hz and 0.1 fs.
    fn worst_vs_analog(c: &BiquadCoeffs, m: &Monic, fs: f64) -> f64 {
        (0..200)
            .map(|i| {
                let f = 10.0 * (0.1 * fs / 10.0).powf(i as f64 / 199.0);
                (db(c.mag_sq(2.0 * PI * f / fs)) - db(m.mag_sq(f))).abs()
            })
            .fold(0.0, f64::max)
    }

    /// The corrected extremum criterion alone (no guard, no second try) keeps
    /// the 48 dB/oct bell at 1 kHz, +6 dB, Q about 3.25 within 0.01 dB of the
    /// analog prototype; the old criterion was 1.7 dB off there.
    #[test]
    fn extremum_rule_needs_no_guard_around_q_3_25() {
        let fs = 44100.0;
        for (f0, q) in [
            (1000.0, 3.2),
            (1000.0, 3.24),
            (1000.0, 3.25),
            (1000.0, 3.26),
            (1000.0, 3.3),
            (500.0, 3.25),
        ] {
            let a = bell_sections(f0, 6.0, q, Slope::Db48);
            for s in a.sections() {
                let m = Monic::from_section(s).unwrap();
                let c = design_bell_section_rule_only(s, q, fs).expect("rule applies");
                let w = worst_vs_analog(&c, &m, fs);
                assert!(w <= 0.01, "{f0} Hz Q {q}: {w} dB");
            }
        }
    }

    #[test]
    fn default_q_shelf_rule_hits_its_points_and_falls_back() {
        for (shape, fs) in [
            (Shape::HighShelf, 44100.0),
            (Shape::LowShelf, 48000.0),
            (Shape::TiltShelf, 96000.0),
        ] {
            let f0 = 3000.0;
            let a = AnalogBand::design(shape, f0, 6.0, 1.0, Slope::Db12);
            let s = &a.sections()[0];
            let (m, k) = Monic::from_section_with_gain(s).unwrap();
            let c = design_shelf_section(s, f0, fs).expect("realisable");
            assert!(c.is_stable());
            let d = std::f64::consts::FRAC_1_SQRT_2;
            let cc = 0.5 * f0;
            for (fd, fa) in [
                (0.0, 0.0),
                (cc * (1.0 - d), cc * (1.0 - d)),
                (cc, cc),
                (0.5 * fs, 0.5 * fs),
            ] {
                let got = c.mag_sq(2.0 * PI * fd / fs);
                let want = k * k * m.mag_sq(fa);
                assert!(((got - want) / want).abs() < 1e-8, "{shape:?} {fd}");
            }
        }
        let a = AnalogBand::design(Shape::HighShelf, 3000.0, 6.0, 1.0, Slope::Db12);
        // c = f0 / 2 above 0.999 Nyquist
        assert!(design_shelf_section(&a.sections()[0], 3000.0, 3000.0).is_none());
    }

    #[test]
    fn unusable_inputs_return_none() {
        let a = bell_sections(1000.0, 6.0, 1.0, Slope::Db12);
        // band frequency at Nyquist
        assert!(design_bell_section(&a.sections()[0], 1.0, 2000.0).is_none());
        // not a bell section
        let hc = AnalogBand::design(Shape::HighCut, 1000.0, 0.0, 1.0, Slope::Db12);
        assert!(design_bell_section(&hc.sections()[0], 1.0, 48000.0).is_none());
        assert!(design_highcut_section(&a.sections()[0], 48000.0).is_none());
        assert!(design_highcut_section(&hc.sections()[0], 2000.0).is_none());
    }
}
