//! Analog s-domain prototypes (cuts, shelves, tilt, bell, notch, band pass)
//! as cascades of 1st/2nd-order sections in real frequency units (rad/s).
//!
//! A band design is an [`AnalogBand`]: at most [`MAX_SECTIONS`] sections
//! `(b0 + b1 s + b2 s^2) / (a0 + a1 s + a2 s^2)` (first-order sections are
//! degenerate: `b2 = a2 = 0`) whose product is the transfer function. The same
//! object is (a) the input of the matched z-transform and (b) the analytic
//! ground truth ([`AnalogBand::magnitude`]) that tests compare against.
//!
//! All formulas are the textbook ones from `PLAN.md`; they are built in
//! normalised form (w0 = 1) and then frequency scaled `s -> s / w0`.
//!
//! Definitions chosen here (see also `conventions.rs`):
//! * Notch: zeros of multiplicity M on the jw axis at w0, poles are the
//!   Butterworth band-stop poles of order M with bandwidth `1/Qs`; unity gain at
//!   DC and infinity. Gain is ignored.
//! * Band pass: Butterworth low-pass order M through the LP->BP map, unity gain
//!   at w0. Gain is ignored.
//! * Flat tilt: see [`FLAT_TILT_SHELVES`].

use crate::conventions::{
    bell_prototype_order, cut_shelf_order, FLAT_TILT_OCTAVES_PER_GAIN, MAX_DESIGN_FREQ_RATIO,
    Q_DISPLAY_TO_STD,
};
use crate::types::{
    BandParams, Shape, Slope, MAX_FREQ_HZ, MAX_Q, MAX_SECTIONS, MIN_FREQ_HZ, MIN_Q,
};
use std::f64::consts::PI;
use std::ops::{Add, Div, Mul, Sub};

/// Minimal complex number (the crate has no runtime dependencies).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    pub fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
    pub fn scale(self, k: f64) -> Self {
        Self::new(self.re * k, self.im * k)
    }
    pub fn recip(self) -> Self {
        let n = self.norm_sqr();
        Self::new(self.re / n, -self.im / n)
    }
    /// Principal square root (numerically stable for all quadrants).
    pub fn sqrt(self) -> Self {
        let r = self.abs();
        if r == 0.0 {
            return Self::new(0.0, 0.0);
        }
        if self.re >= 0.0 {
            let t = ((r + self.re) * 0.5).sqrt();
            Self::new(t, self.im / (2.0 * t))
        } else {
            let t = ((r - self.re) * 0.5).sqrt();
            Self::new(self.im.abs() / (2.0 * t), t.copysign(self.im))
        }
    }
}

impl Add for Complex {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for Complex {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for Complex {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}
#[allow(clippy::suspicious_arithmetic_impl)]
impl Div for Complex {
    type Output = Self;
    fn div(self, o: Self) -> Self {
        self * o.recip()
    }
}

/// One analog section `(b0 + b1 s + b2 s^2) / (a0 + a1 s + a2 s^2)`, `s` in rad/s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalogSection {
    pub b: [f64; 3],
    pub a: [f64; 3],
}

impl AnalogSection {
    pub const IDENTITY: Self = Self {
        b: [1.0, 0.0, 0.0],
        a: [1.0, 0.0, 0.0],
    };

    /// Squared magnitude at `s = j w` (`w` in rad/s).
    pub fn mag_sq(&self, w: f64) -> f64 {
        let w2 = w * w;
        let nr = self.b[0] - self.b[2] * w2;
        let dr = self.a[0] - self.a[2] * w2;
        let n = nr * nr + self.b[1] * self.b[1] * w2;
        let d = dr * dr + self.a[1] * self.a[1] * w2;
        n / d
    }

    /// Complex response at `s = j w`.
    pub fn response(&self, w: f64) -> Complex {
        let w2 = w * w;
        let n = Complex::new(self.b[0] - self.b[2] * w2, self.b[1] * w);
        let d = Complex::new(self.a[0] - self.a[2] * w2, self.a[1] * w);
        n / d
    }

    /// Substitute `s -> s / w0` (normalised prototype to real frequency).
    fn scaled(&self, w0: f64) -> Self {
        let k1 = 1.0 / w0;
        let k2 = k1 * k1;
        Self {
            b: [self.b[0], self.b[1] * k1, self.b[2] * k2],
            a: [self.a[0], self.a[1] * k1, self.a[2] * k2],
        }
    }

    fn scale_gain(&mut self, k: f64) {
        for c in &mut self.b {
            *c *= k;
        }
    }
}

/// Fixed-capacity cascade of analog sections: a band's analog prototype.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalogBand {
    sections: [AnalogSection; MAX_SECTIONS],
    len: usize,
}

impl Default for AnalogBand {
    fn default() -> Self {
        Self::identity()
    }
}

/// True for shapes whose `gain_db` is used (and scaled by the gain scale).
pub fn shape_uses_gain(shape: Shape) -> bool {
    matches!(
        shape,
        Shape::Bell | Shape::LowShelf | Shape::HighShelf | Shape::TiltShelf | Shape::FlatTilt
    )
}

/// Effective dB gain of a band: `gain_db * gain_scale` for gain shapes, else 0.
pub fn effective_gain_db(shape: Shape, gain_db: f64, gain_scale: f64) -> f64 {
    if shape_uses_gain(shape) {
        gain_db * gain_scale
    } else {
        0.0
    }
}

/// Design frequency actually used: clamped to the legal range and below
/// `MAX_DESIGN_FREQ_RATIO * sample_rate`. NaN falls back to 1 kHz.
///
/// Rationale: an analog resonance above Nyquist cannot be represented by an
/// impulse-invariant pole, so such bands are pinned at 0.49 fs (0.98 Nyquist)
/// instead of aliasing. The curve then saturates smoothly instead of jumping.
pub fn design_frequency(freq_hz: f64, sample_rate: f64) -> f64 {
    let f = if freq_hz.is_finite() { freq_hz } else { 1000.0 };
    f.clamp(MIN_FREQ_HZ, MAX_FREQ_HZ)
        .min(MAX_DESIGN_FREQ_RATIO * sample_rate)
}

/// Butterworth pair damping `d_k = 2 sin((2k-1) pi / (2 n))`, k = 1..n/2.
/// k = 1 is the highest-Q pair.
fn butter_d(n: usize, k: usize) -> f64 {
    2.0 * (((2 * k - 1) as f64) * PI / (2.0 * n as f64)).sin()
}

/// Angle `x_k` with `d_k = 2 sin x_k`.
fn butter_x(n: usize, k: usize) -> f64 {
    ((2 * k - 1) as f64) * PI / (2.0 * n as f64)
}

/// Factor the palindromic quartic that a prototype root `r` (Im > 0, and its
/// conjugate) maps to under `s' = qs (s + 1/s)` into two monic real quadratics
/// `(c0, c1, 1)`: "A" holding the roots of larger magnitude and "B" the
/// reciprocal ones. Closed form: `s^2 - (r/qs) s + 1 = 0`.
fn band_quads(r: Complex, qs: f64) -> ([f64; 3], [f64; 3]) {
    let c = r.scale(1.0 / qs);
    let w = (c * c - Complex::new(4.0, 0.0)).sqrt();
    let p = c + w;
    let m = c - w;
    let big = if p.norm_sqr() >= m.norm_sqr() { p } else { m }.scale(0.5);
    let small = big.recip();
    (
        [big.norm_sqr(), -2.0 * big.re, 1.0],
        [small.norm_sqr(), -2.0 * small.re, 1.0],
    )
}

/// Flat tilt: a fixed (pivot independent) ladder of first-order shelves on a
/// log-frequency grid, packed pairwise into 8 sections, with the shelf step
/// chosen from the requested dB/octave and the whole cascade normalised to
/// exactly 0 dB at the pivot. A fixed ladder means moving the pivot only
/// changes a constant gain, so there are no discontinuities.
pub const FLAT_TILT_SHELVES: usize = 16;
const FLAT_TILT_SPACING_OCT: f64 = 1.0796;
/// Lowest shelf centre in Hz (highest is about 146 kHz).
const FLAT_TILT_F_LO: f64 = 2.0;
/// The outermost shelves carry `sqrt_r^1.3`: compensates the missing tails
/// beyond the ladder ends (found by a parameter sweep; ripple < 0.005 dB at
/// 3 dB/oct over 20 Hz - 20 kHz for any pivot).
const FLAT_TILT_EDGE_EXP: f64 = 1.3;

impl AnalogBand {
    pub fn identity() -> Self {
        Self {
            sections: [AnalogSection::IDENTITY; MAX_SECTIONS],
            len: 0,
        }
    }

    pub fn sections(&self) -> &[AnalogSection] {
        &self.sections[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, s: AnalogSection) {
        if self.len < MAX_SECTIONS {
            self.sections[self.len] = s;
            self.len += 1;
        } else {
            debug_assert!(false, "section capacity exceeded");
        }
    }

    fn push_norm(&mut self, b: [f64; 3], a: [f64; 3], w0: f64) {
        self.push(AnalogSection { b, a }.scaled(w0));
    }

    /// Design from user parameters: applies the gain scale and clamps the
    /// frequency with [`design_frequency`].
    pub fn from_params(params: &BandParams, gain_scale: f64, sample_rate: f64) -> Self {
        let gain = effective_gain_db(params.shape, params.gain_db, gain_scale);
        Self::design(
            params.shape,
            design_frequency(params.freq_hz, sample_rate),
            gain,
            params.q,
            params.slope,
        )
    }

    /// Raw design: `gain_db` is the final dB gain (ignored by cuts, notch and
    /// band pass), `q` the displayed Q. Inputs are sanitised (NaN, ranges).
    pub fn design(shape: Shape, freq_hz: f64, gain_db: f64, q: f64, slope: Slope) -> Self {
        let f = if freq_hz.is_finite() { freq_hz } else { 1000.0 };
        let f = f.clamp(1.0, 1.0e6);
        let q = if q.is_finite() { q } else { 1.0 }.clamp(MIN_Q, MAX_Q);
        let gain_db = if gain_db.is_finite() { gain_db } else { 0.0 }.clamp(-90.0, 90.0);
        let w0 = 2.0 * PI * f;
        let gain = 10f64.powf(gain_db / 20.0);
        let mut band = Self::identity();
        match shape {
            Shape::LowCut => band.cut(false, w0, cut_shelf_order(slope), q),
            Shape::HighCut => band.cut(true, w0, cut_shelf_order(slope), q),
            Shape::LowShelf => band.shelf(false, w0, cut_shelf_order(slope), gain, q, 1.0),
            Shape::HighShelf => band.shelf(true, w0, cut_shelf_order(slope), gain, q, 1.0),
            Shape::TiltShelf => {
                band.shelf(true, w0, cut_shelf_order(slope), gain, q, 1.0 / gain.sqrt())
            }
            Shape::Bell => band.bell(w0, bell_prototype_order(slope), gain, q * Q_DISPLAY_TO_STD),
            Shape::Notch => band.notch(w0, bell_prototype_order(slope), q * Q_DISPLAY_TO_STD),
            Shape::BandPass => {
                band.band_pass(w0, bell_prototype_order(slope), q * Q_DISPLAY_TO_STD)
            }
            Shape::FlatTilt => band.flat_tilt(w0, gain_db),
        }
        band
    }

    /// Butterworth low-pass (`low_pass = true`, high cut) or high-pass
    /// (low cut) of order `n`. The highest-Q pair's Q is multiplied by `q`.
    fn cut(&mut self, low_pass: bool, w0: f64, n: usize, q: f64) {
        let num = if low_pass {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        for k in 1..=n / 2 {
            let mut d = butter_d(n, k);
            if k == 1 {
                d /= q;
            }
            self.push_norm(num, [1.0, d, 1.0], w0);
        }
        if n % 2 == 1 {
            let num1 = if low_pass {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            self.push_norm(num1, [1.0, 1.0, 0.0], w0);
        }
    }

    /// Symmetric Butterworth shelf of order `n` and linear gain `gain`
    /// (low shelf: DC gain G, HF gain 1; high shelf: mirrored). `k` is an
    /// extra constant (tilt). Highest-Q section's `d` is divided by `q`.
    fn shelf(&mut self, high: bool, w0: f64, n: usize, gain: f64, q: f64, k: f64) {
        let g = gain.powf(1.0 / n as f64);
        let sg = g.sqrt();
        for i in 1..=n / 2 {
            let mut d = butter_d(n, i);
            if i == 1 {
                d /= q;
            }
            let (b, a) = if high {
                ([1.0, d * sg, g], [1.0, d / sg, 1.0 / g])
            } else {
                ([g, d * sg, 1.0], [1.0 / g, d / sg, 1.0])
            };
            self.push_norm(b, a, w0);
        }
        if n % 2 == 1 {
            let (b, a) = if high {
                ([1.0, sg, 0.0], [1.0, 1.0 / sg, 0.0])
            } else {
                ([sg, 1.0, 0.0], [1.0 / sg, 1.0, 0.0])
            };
            self.push_norm(b, a, w0);
        }
        if k != 1.0 && self.len > 0 {
            self.sections[0].scale_gain(k);
        }
    }

    /// Bell: Butterworth low-shelf prototype of order `m` and gain `gain`
    /// through LP->BP `s -> qs (s + 1/s)`.
    fn bell(&mut self, w0: f64, m: usize, gain: f64, qs: f64) {
        let g = gain.powf(1.0 / m as f64);
        let sg = g.sqrt();
        for k in 1..=m / 2 {
            let x = butter_x(m, k);
            let (sx, cx) = x.sin_cos();
            // prototype section zeros: sg * (-sin x + j cos x), poles: that / g
            let rn = Complex::new(-sg * sx, sg * cx);
            let rd = Complex::new(-sx / sg, cx / sg);
            let (na, nb) = band_quads(rn, qs);
            let (da, db) = band_quads(rd, qs);
            self.push_norm(na, da, w0);
            self.push_norm(nb, db, w0);
        }
        if m % 2 == 1 {
            self.push_norm([1.0, sg / qs, 1.0], [1.0, 1.0 / (sg * qs), 1.0], w0);
        }
    }

    fn notch(&mut self, w0: f64, m: usize, qs: f64) {
        for k in 1..=m / 2 {
            let x = butter_x(m, k);
            let (sx, cx) = x.sin_cos();
            let (da, db) = band_quads(Complex::new(-sx, cx), qs);
            self.push_norm([1.0, 0.0, 1.0], da, w0);
            self.push_norm([1.0, 0.0, 1.0], db, w0);
        }
        if m % 2 == 1 {
            self.push_norm([1.0, 0.0, 1.0], [1.0, 1.0 / qs, 1.0], w0);
        }
    }

    fn band_pass(&mut self, w0: f64, m: usize, qs: f64) {
        let num = [0.0, 1.0 / qs, 0.0];
        for k in 1..=m / 2 {
            let x = butter_x(m, k);
            let (sx, cx) = x.sin_cos();
            let (da, db) = band_quads(Complex::new(-sx, cx), qs);
            self.push_norm(num, da, w0);
            self.push_norm(num, db, w0);
        }
        if m % 2 == 1 {
            self.push_norm(num, [1.0, 1.0 / qs, 1.0], w0);
        }
    }

    /// Constant dB/octave tilt: a ladder of first-order shelves, normalised to
    /// exactly 0 dB at `w0`.
    fn flat_tilt(&mut self, w0: f64, gain_db: f64) {
        let tilt_db_per_oct = gain_db / FLAT_TILT_OCTAVES_PER_GAIN;
        let sqrt_r = 10f64.powf(tilt_db_per_oct * FLAT_TILT_SPACING_OCT / 40.0);
        let mut zp = [(0.0, 0.0); FLAT_TILT_SHELVES];
        for (j, e) in zp.iter_mut().enumerate() {
            let wc = 2.0 * PI * FLAT_TILT_F_LO * 2f64.powf(FLAT_TILT_SPACING_OCT * j as f64);
            let sr = if j == 0 || j == FLAT_TILT_SHELVES - 1 {
                sqrt_r.powf(FLAT_TILT_EDGE_EXP)
            } else {
                sqrt_r
            };
            *e = (wc / sr, wc * sr); // (zero, pole), rising when r > 1
        }
        for pair in zp.chunks(2) {
            let (z1, p1) = pair[0];
            let (z2, p2) = pair[1];
            self.push(AnalogSection {
                b: [z1 * z2, z1 + z2, 1.0],
                a: [p1 * p2, p1 + p2, 1.0],
            });
        }
        let m = self.mag_sq(w0);
        let k = 1.0 / m.sqrt();
        self.sections[0].scale_gain(k);
    }

    /// Squared magnitude at angular frequency `w` (rad/s).
    pub fn mag_sq(&self, w: f64) -> f64 {
        self.sections().iter().map(|s| s.mag_sq(w)).product()
    }

    /// Complex response at `f_hz`.
    pub fn response(&self, f_hz: f64) -> Complex {
        let w = 2.0 * PI * f_hz;
        self.sections()
            .iter()
            .fold(Complex::new(1.0, 0.0), |acc, s| acc * s.response(w))
    }

    /// Linear magnitude at `f_hz`: the analytic ground truth.
    pub fn magnitude(&self, f_hz: f64) -> f64 {
        self.mag_sq(2.0 * PI * f_hz).sqrt()
    }

    /// Magnitude in dB at `f_hz` (sum of per-section logs, overflow safe).
    pub fn magnitude_db(&self, f_hz: f64) -> f64 {
        let w = 2.0 * PI * f_hz;
        self.sections()
            .iter()
            .map(|s| 10.0 * s.mag_sq(w).log10())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    fn band(shape: Shape, f: f64, g: f64, q: f64, slope: Slope) -> AnalogBand {
        AnalogBand::design(shape, f, g, q, slope)
    }

    #[test]
    fn complex_sqrt() {
        for &(re, im) in &[
            (3.0, 4.0),
            (-3.0, 4.0),
            (-3.0, -4.0),
            (3.0, -4.0),
            (-5.0, 0.0),
        ] {
            let z = Complex::new(re, im);
            let r = z.sqrt();
            let back = r * r;
            assert_abs_diff_eq!(back.re, re, epsilon = 1e-12);
            assert_abs_diff_eq!(back.im, im, epsilon = 1e-12);
        }
    }

    #[test]
    fn section_counts_within_capacity() {
        for shape in Shape::ALL {
            for slope in Slope::ALL {
                let b = band(shape, 1000.0, 6.0, 2.0, slope);
                assert!(
                    b.len() <= MAX_SECTIONS && !b.is_empty(),
                    "{shape:?} {slope:?}"
                );
            }
        }
    }

    #[test]
    fn butterworth_cut_minus3db_and_flatness() {
        for slope in Slope::ALL {
            let n = cut_shelf_order(slope) as f64;
            let lp = band(Shape::HighCut, 1000.0, 0.0, 1.0, slope);
            let hp = band(Shape::LowCut, 1000.0, 0.0, 1.0, slope);
            assert_abs_diff_eq!(lp.magnitude_db(1000.0), -3.0103, epsilon = 1e-3);
            assert_abs_diff_eq!(hp.magnitude_db(1000.0), -3.0103, epsilon = 1e-3);
            assert_abs_diff_eq!(lp.magnitude_db(0.01), 0.0, epsilon = 1e-6);
            assert_abs_diff_eq!(hp.magnitude_db(1.0e9), 0.0, epsilon = 1e-6);
            // Butterworth: |H|^2 = 1/(1+(f/fc)^(2n)).
            for &f in &[100.0, 500.0, 2000.0, 8000.0] {
                let want = -10.0 * (1.0 + (f / 1000.0f64).powf(2.0 * n)).log10();
                assert_abs_diff_eq!(lp.magnitude_db(f), want, epsilon = 1e-9);
                let want = -10.0 * (1.0 + (1000.0 / f).powf(2.0 * n)).log10();
                assert_abs_diff_eq!(hp.magnitude_db(f), want, epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn cut_q_scales_highest_q_pair() {
        // 12 dB/oct, Q display 2 => section Q = 2 * 0.7071 => peak of ~ +2.5 dB.
        let lp = band(Shape::HighCut, 1000.0, 0.0, 2.0, Slope::Db12);
        let qs = 2.0 * Q_DISPLAY_TO_STD;
        assert_abs_diff_eq!(lp.magnitude_db(1000.0), db(qs), epsilon = 1e-9);
        // 1st order ignores Q
        let a = band(Shape::HighCut, 1000.0, 0.0, 10.0, Slope::Db6);
        let b = band(Shape::HighCut, 1000.0, 0.0, 0.3, Slope::Db6);
        assert_abs_diff_eq!(
            a.magnitude_db(700.0),
            b.magnitude_db(700.0),
            epsilon = 1e-12
        );
    }

    #[test]
    fn shelf_identities() {
        for slope in Slope::ALL {
            for &gdb in &[-24.0, -6.0, 3.0, 12.0, 30.0] {
                let g = 10f64.powf(gdb / 20.0);
                let ls = band(Shape::LowShelf, 800.0, gdb, 1.0, slope);
                let hs = band(Shape::HighShelf, 800.0, gdb, 1.0, slope);
                assert_abs_diff_eq!(ls.magnitude(0.01), g, epsilon = 1e-6 * g);
                assert_abs_diff_eq!(ls.magnitude(1.0e7), 1.0, epsilon = 1e-6);
                assert_abs_diff_eq!(ls.magnitude_db(800.0), gdb / 2.0, epsilon = 1e-9);
                assert_abs_diff_eq!(hs.magnitude(0.01), 1.0, epsilon = 1e-6);
                assert_abs_diff_eq!(hs.magnitude(1.0e7), g, epsilon = 1e-6 * g);
                assert_abs_diff_eq!(hs.magnitude_db(800.0), gdb / 2.0, epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn shelf_q_keeps_midpoint_gain() {
        for &q in &[0.3, 1.0, 4.0, 20.0] {
            let hs = band(Shape::HighShelf, 800.0, 12.0, q, Slope::Db24);
            assert_abs_diff_eq!(hs.magnitude_db(800.0), 6.0, epsilon = 1e-9);
        }
    }

    #[test]
    fn tilt_shelf_symmetric() {
        for slope in [Slope::Db6, Slope::Db12, Slope::Db36] {
            let t = band(Shape::TiltShelf, 1000.0, 10.0, 1.0, slope);
            assert_abs_diff_eq!(t.magnitude_db(1000.0), 0.0, epsilon = 1e-9);
            assert_abs_diff_eq!(t.magnitude_db(0.01), -5.0, epsilon = 1e-4);
            assert_abs_diff_eq!(t.magnitude_db(1.0e7), 5.0, epsilon = 1e-4);
            // Antisymmetric around the pivot in log frequency.
            for &x in &[1.5, 3.0, 10.0] {
                assert_abs_diff_eq!(
                    t.magnitude_db(1000.0 * x),
                    -t.magnitude_db(1000.0 / x),
                    epsilon = 1e-9
                );
            }
        }
    }

    #[test]
    fn bell_peak_gain_is_exact_for_all_orders_and_q() {
        for slope in Slope::ALL {
            for &q in &[0.025, 0.3, 1.0, 5.0, 40.0] {
                for &gdb in &[-30.0, -3.0, 0.0, 6.0, 30.0] {
                    let b = band(Shape::Bell, 1234.0, gdb, q, slope);
                    assert_abs_diff_eq!(b.magnitude_db(1234.0), gdb, epsilon = 1e-8);
                }
            }
        }
    }

    #[test]
    fn bell_m1_is_classic_peaking_filter() {
        let (f0, g, q) = (2000.0, 9.0, 1.7);
        let gl = 10f64.powf(g / 20.0);
        let qs = q * Q_DISPLAY_TO_STD;
        let b = band(Shape::Bell, f0, g, q, Slope::Db12);
        assert_eq!(b.len(), 1);
        for &f in &[50.0, 700.0, 1900.0, 2000.0, 2300.0, 9000.0] {
            let s = Complex::new(0.0, f / f0);
            let one = Complex::new(1.0, 0.0);
            let num = s * s + s.scale(gl.sqrt() / qs) + one;
            let den = s * s + s.scale(1.0 / (gl.sqrt() * qs)) + one;
            let want = (num / den).abs();
            assert_abs_diff_eq!(b.magnitude(f), want, epsilon = 1e-12 * want);
        }
    }

    #[test]
    fn bell_boost_cut_are_inverses() {
        for slope in Slope::ALL {
            for &q in &[0.1, 1.0, 12.0] {
                let up = band(Shape::Bell, 3000.0, 11.0, q, slope);
                let dn = band(Shape::Bell, 3000.0, -11.0, q, slope);
                for &f in &[30.0, 900.0, 2500.0, 3000.0, 4100.0, 15000.0] {
                    let p = up.response(f) * dn.response(f);
                    assert_abs_diff_eq!(p.re, 1.0, epsilon = 1e-9);
                    assert_abs_diff_eq!(p.im, 0.0, epsilon = 1e-9);
                }
            }
        }
    }

    #[test]
    fn bell_factorisation_matches_direct_quartic() {
        // M = 2: evaluate the prototype shelf at s' = qs (s/w0 + w0/s) directly.
        let (f0, g, q) = (1500.0, 14.0, 3.0);
        let gl = 10f64.powf(g / 20.0);
        let qs = q * Q_DISPLAY_TO_STD;
        let b = band(Shape::Bell, f0, g, q, Slope::Db24);
        let gg = gl.powf(0.5);
        let d = butter_d(2, 1);
        for &f in &[100.0, 900.0, 1400.0, 1500.0, 1650.0, 3000.0, 12000.0] {
            let s = Complex::new(0.0, f / f0);
            let sp = (s + s.recip()).scale(qs);
            let one = Complex::new(1.0, 0.0);
            let num = sp * sp + sp.scale(d * gg.sqrt()) + one.scale(gg);
            let den = sp * sp + sp.scale(d / gg.sqrt()) + one.scale(1.0 / gg);
            let want = (num / den).abs();
            assert_abs_diff_eq!(b.magnitude(f), want, epsilon = 1e-10 * want);
        }
    }

    #[test]
    fn notch_and_bandpass() {
        for slope in Slope::ALL {
            let n = band(Shape::Notch, 1000.0, 0.0, 1.0, slope);
            let bp = band(Shape::BandPass, 1000.0, 0.0, 1.0, slope);
            assert!(n.magnitude(1000.0) < 1e-12);
            assert_abs_diff_eq!(n.magnitude(1.0), 1.0, epsilon = 1e-4);
            assert_abs_diff_eq!(n.magnitude(1.0e7), 1.0, epsilon = 1e-4);
            assert_abs_diff_eq!(bp.magnitude(1000.0), 1.0, epsilon = 1e-12);
            assert!(bp.magnitude(0.01) < 1e-3);
            assert!(bp.magnitude(1.0e7) < 1e-3);
            // Both are Butterworth-shaped: |H_bp|^2 + |H_notch|^2 = 1.
            for &f in &[300.0, 800.0, 1250.0, 3000.0] {
                let s = n.magnitude(f).powi(2) + bp.magnitude(f).powi(2);
                assert_abs_diff_eq!(s, 1.0, epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn bandpass_edges_are_minus3db_at_unit_q() {
        // M = 1, display Q such that Qs = 1: edges at f0 * (sqrt(5)/2 +- 1/2).
        let q = 1.0 / Q_DISPLAY_TO_STD;
        let bp = band(Shape::BandPass, 1000.0, 0.0, q, Slope::Db12);
        let hi = 1000.0 * (5f64.sqrt() + 1.0) / 2.0;
        assert_abs_diff_eq!(bp.magnitude_db(hi), -3.0103, epsilon = 1e-3);
    }

    #[test]
    fn flat_tilt_is_straight_line_in_audio_band() {
        let mut worst: f64 = 0.0;
        for &gdb in &[-30.0, -12.0, -3.0, 3.0, 12.0, 30.0] {
            for &f0 in &[10.0, 50.0, 632.0, 1000.0, 5000.0, 20000.0, 30000.0] {
                let b = band(Shape::FlatTilt, f0, gdb, 1.0, Slope::Db12);
                assert_abs_diff_eq!(b.magnitude_db(f0), 0.0, epsilon = 1e-9);
                let slope = gdb / FLAT_TILT_OCTAVES_PER_GAIN;
                let mut f = 20.0;
                while f <= 20000.0 {
                    let want = slope * (f / f0).log2();
                    worst = worst.max((b.magnitude_db(f) - want).abs());
                    f *= 1.01;
                }
            }
        }
        println!("flat tilt worst ripple: {worst:.4} dB");
        assert!(worst < 0.01, "ripple {worst}");
    }
}
