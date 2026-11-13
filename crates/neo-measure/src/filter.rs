//! Biquad building blocks (f64, transposed direct form II).
//!
//! Realtime: everything here is allocation-free and panic-free.

use crate::util::Complex;
use std::f64::consts::PI;

/// Normalised biquad coefficients (`a0 == 1`).
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

    /// Complex response at `freq` Hz for sample rate `fs`.
    pub fn response(&self, freq: f64, fs: f64) -> Complex {
        let w = 2.0 * PI * freq / fs;
        let z1 = Complex::from_polar(1.0, -w);
        let z2 = z1 * z1;
        (self.b0 + self.b1 * z1 + self.b2 * z2) / (1.0 + self.a1 * z1 + self.a2 * z2)
    }

    /// Second-order Butterworth-type low-pass with quality factor `q`
    /// (bilinear transform, corner pre-warped).
    pub fn lowpass(fs: f64, f0: f64, q: f64) -> Self {
        let k = (PI * f0.clamp(1e-6, fs * 0.499) / fs).tan();
        let n = 1.0 / (1.0 + k / q + k * k);
        Self {
            b0: k * k * n,
            b1: 2.0 * k * k * n,
            b2: k * k * n,
            a1: 2.0 * (k * k - 1.0) * n,
            a2: (1.0 - k / q + k * k) * n,
        }
    }

    /// Second-order high-pass with quality factor `q`.
    pub fn highpass(fs: f64, f0: f64, q: f64) -> Self {
        let k = (PI * f0.clamp(1e-6, fs * 0.499) / fs).tan();
        let n = 1.0 / (1.0 + k / q + k * k);
        Self {
            b0: n,
            b1: -2.0 * n,
            b2: n,
            a1: 2.0 * (k * k - 1.0) * n,
            a2: (1.0 - k / q + k * k) * n,
        }
    }

    /// First-order low-pass `1/(1+s/w0)` as a biquad.
    pub fn lowpass1(fs: f64, f0: f64) -> Self {
        let k = (PI * f0.clamp(1e-6, fs * 0.499) / fs).tan();
        let n = 1.0 / (1.0 + k);
        Self {
            b0: k * n,
            b1: k * n,
            b2: 0.0,
            a1: (k - 1.0) * n,
            a2: 0.0,
        }
    }

    /// First-order high-pass `(s/w0)/(1+s/w0)` as a biquad.
    pub fn highpass1(fs: f64, f0: f64) -> Self {
        let k = (PI * f0.clamp(1e-6, fs * 0.499) / fs).tan();
        let n = 1.0 / (1.0 + k);
        Self {
            b0: n,
            b1: -n,
            b2: 0.0,
            a1: (k - 1.0) * n,
            a2: 0.0,
        }
    }
}

/// A running biquad section.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    c: BiquadCoeffs,
    z1: f64,
    z2: f64,
}

impl Biquad {
    pub fn new(c: BiquadCoeffs) -> Self {
        Self {
            c,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn coeffs(&self) -> BiquadCoeffs {
        self.c
    }
    pub fn set_coeffs(&mut self, c: BiquadCoeffs) {
        self.c = c;
    }
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.c.b0 * x + self.z1;
        self.z1 = self.c.b1 * x - self.c.a1 * y + self.z2;
        self.z2 = self.c.b2 * x - self.c.a2 * y;
        if y.is_finite() {
            y
        } else {
            self.reset();
            0.0
        }
    }
}

/// Fourth-order Linkwitz-Riley low-pass (two cascaded Butterworth sections).
#[derive(Debug, Clone, Copy)]
pub struct LinkwitzRileyLp {
    a: Biquad,
    b: Biquad,
}

impl LinkwitzRileyLp {
    pub fn new(fs: f64, f0: f64) -> Self {
        let c = BiquadCoeffs::lowpass(fs, f0, std::f64::consts::FRAC_1_SQRT_2);
        Self {
            a: Biquad::new(c),
            b: Biquad::new(c),
        }
    }
    pub fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.a.process(x);
        self.b.process(y)
    }
}

/// Cascade of `N` biquads with a gain.
#[derive(Debug, Clone)]
pub struct Cascade<const N: usize> {
    pub sections: [Biquad; N],
    pub gain: f64,
}

impl<const N: usize> Cascade<N> {
    pub fn new(c: [BiquadCoeffs; N], gain: f64) -> Self {
        Self {
            sections: c.map(Biquad::new),
            gain,
        }
    }
    pub fn reset(&mut self) {
        self.sections.iter_mut().for_each(Biquad::reset);
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let mut y = x * self.gain;
        for s in self.sections.iter_mut() {
            y = s.process(y);
        }
        y
    }
    /// Complex response including the gain.
    pub fn response(&self, freq: f64, fs: f64) -> Complex {
        self.sections
            .iter()
            .fold(Complex::new(self.gain, 0.0), |a, s| {
                a * s.coeffs().response(freq, fs)
            })
    }
}
