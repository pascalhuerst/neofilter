//! Biquad coefficients and f64 transposed direct form II state.
//!
//! TDF-II in f64 was measured against the analytic digital response for poles
//! as close to z = 1 as 10 Hz at 192 kHz (see `tests/`), so no fancier
//! structure is used.

/// Coefficients of `(b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl BiquadCoeffs {
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    pub fn is_finite(&self) -> bool {
        self.b0.is_finite()
            && self.b1.is_finite()
            && self.b2.is_finite()
            && self.a1.is_finite()
            && self.a2.is_finite()
    }

    /// Poles strictly inside the unit circle (Schur-Cohn triangle).
    pub fn is_stable(&self) -> bool {
        self.a2.abs() < 1.0 && self.a1.abs() < 1.0 + self.a2
    }

    /// Phi-basis form used for fast, cancellation-free magnitude evaluation.
    pub fn phi_form(&self) -> PhiForm {
        PhiForm::new(self)
    }

    /// Squared magnitude at `omega` rad/sample.
    pub fn mag_sq(&self, omega: f64) -> f64 {
        self.phi_form().mag_sq_phi1((0.5 * omega).sin().powi(2))
    }

    /// Complex response at `omega` rad/sample.
    pub fn response(&self, omega: f64) -> (f64, f64) {
        let (s1, c1) = omega.sin_cos();
        let (s2, c2) = (2.0 * omega).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let d = dr * dr + di * di;
        ((nr * dr + ni * di) / d, (ni * dr - nr * di) / d)
    }
}

/// `|P(e^jw)|^2 = P0 phi0 + P1 phi1 + P2 phi2` with `phi1 = sin^2(w/2)`,
/// `phi0 = 1 - phi1`, `phi2 = 4 phi0 phi1`, for numerator and denominator.
#[derive(Debug, Clone, Copy)]
pub struct PhiForm {
    pub b: [f64; 3],
    pub a: [f64; 3],
}

impl PhiForm {
    pub fn new(c: &BiquadCoeffs) -> Self {
        let b0 = c.b0 + c.b1 + c.b2;
        let b1 = c.b0 - c.b1 + c.b2;
        let a0 = 1.0 + c.a1 + c.a2;
        let a1 = 1.0 - c.a1 + c.a2;
        Self {
            b: [b0 * b0, b1 * b1, -4.0 * c.b0 * c.b2],
            a: [a0 * a0, a1 * a1, -4.0 * c.a2],
        }
    }

    /// Squared magnitude given `phi1 = sin^2(omega / 2)`.
    #[inline]
    pub fn mag_sq_phi1(&self, phi1: f64) -> f64 {
        let phi0 = 1.0 - phi1;
        let phi2 = 4.0 * phi0 * phi1;
        let n = self.b[0] * phi0 + self.b[1] * phi1 + self.b[2] * phi2;
        let d = self.a[0] * phi0 + self.a[1] * phi1 + self.a[2] * phi2;
        n / d
    }
}

/// TDF-II state of one section (one channel), plus the last two input and
/// output samples.
///
/// The sample history is only used by [`BiquadState::refresh`]: when the
/// coefficients change, the TDF-II state variables (which are sums of past
/// inputs and outputs weighted by the *old* coefficients) are recomputed from
/// the history with the *new* coefficients. This makes coefficient updates as
/// benign as in direct form I (which is the best-behaved structure under
/// modulation) while keeping the TDF-II inner loop.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BiquadState {
    pub s1: f64,
    pub s2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl BiquadState {
    pub const ZERO: Self = Self {
        s1: 0.0,
        s2: 0.0,
        x1: 0.0,
        x2: 0.0,
        y1: 0.0,
        y2: 0.0,
    };

    /// Process a block in place.
    #[inline]
    pub fn process(&mut self, c: &BiquadCoeffs, buf: &mut [f64]) {
        let (mut s1, mut s2) = (self.s1, self.s2);
        let (mut x1, mut x2, mut y1, mut y2) = (self.x1, self.x2, self.y1, self.y2);
        for x in buf.iter_mut() {
            let v = *x;
            let y = c.b0 * v + s1;
            s1 = c.b1 * v - c.a1 * y + s2;
            s2 = c.b2 * v - c.a2 * y;
            x2 = x1;
            x1 = v;
            y2 = y1;
            y1 = y;
            *x = y;
        }
        self.s1 = s1;
        self.s2 = s2;
        self.x1 = x1;
        self.x2 = x2;
        self.y1 = y1;
        self.y2 = y2;
    }

    /// Process one sample.
    #[inline]
    pub fn tick(&mut self, c: &BiquadCoeffs, v: f64) -> f64 {
        let y = c.b0 * v + self.s1;
        self.s1 = c.b1 * v - c.a1 * y + self.s2;
        self.s2 = c.b2 * v - c.a2 * y;
        self.x2 = self.x1;
        self.x1 = v;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// Recompute the state variables for new coefficients from the sample
    /// history (see the type docs).
    #[inline]
    pub fn refresh(&mut self, c: &BiquadCoeffs) {
        self.s1 = c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.s2 = c.b2 * self.x1 - c.a2 * self.y1;
    }

    /// Flush denormals/tiny values and heal non-finite state. Called once per
    /// processed chunk, so the cost is negligible.
    #[inline]
    pub fn sanitize(&mut self) {
        self.s1 = flush(self.s1);
        self.s2 = flush(self.s2);
        self.x1 = flush(self.x1);
        self.x2 = flush(self.x2);
        self.y1 = flush(self.y1);
        self.y2 = flush(self.y2);
    }
}

#[inline]
fn flush(x: f64) -> f64 {
    if x.abs() < 1e-100 || !x.is_finite() {
        0.0
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn phi_form_matches_direct_evaluation() {
        let c = BiquadCoeffs {
            b0: 1.3,
            b1: -0.4,
            b2: 0.2,
            a1: -1.1,
            a2: 0.45,
        };
        for i in 1..50 {
            let w = i as f64 * 0.0627;
            let (re, im) = c.response(w);
            assert_abs_diff_eq!(c.mag_sq(w), re * re + im * im, epsilon = 1e-12);
        }
    }

    #[test]
    fn refresh_with_same_coefficients_is_a_no_op() {
        let c = BiquadCoeffs {
            b0: 0.9,
            b1: -0.3,
            b2: 0.1,
            a1: -1.2,
            a2: 0.5,
        };
        let mut st = BiquadState::ZERO;
        let mut buf: Vec<f64> = (0..50).map(|i| (i as f64 * 0.37).sin()).collect();
        st.process(&c, &mut buf);
        let before = st;
        st.refresh(&c);
        assert_abs_diff_eq!(st.s1, before.s1, epsilon = 1e-13);
        assert_abs_diff_eq!(st.s2, before.s2, epsilon = 1e-13);
    }

    #[test]
    fn identity_passes() {
        let mut st = BiquadState::default();
        let mut buf = [0.5, -1.0, 2.0];
        st.process(&BiquadCoeffs::IDENTITY, &mut buf);
        assert_eq!(buf, [0.5, -1.0, 2.0]);
    }

    #[test]
    fn sanitize_cases() {
        let mut s = BiquadState {
            s1: 1e-200,
            s2: f64::NAN,
            ..BiquadState::ZERO
        };
        s.sanitize();
        assert_eq!(s, BiquadState::ZERO);
        let mut s = BiquadState {
            s1: 0.5,
            s2: -0.25,
            ..BiquadState::ZERO
        };
        s.sanitize();
        assert_eq!((s.s1, s.s2), (0.5, -0.25));
    }
}
