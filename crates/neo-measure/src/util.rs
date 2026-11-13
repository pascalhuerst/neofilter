//! Small shared helpers: dB conversions, FFT wrappers, windows.
//!
//! Realtime: the dB helpers are trivially safe; FFT helpers allocate.

use realfft::num_complex::Complex64;
use realfft::RealFftPlanner;

/// Complex number type used throughout (re-export of `num_complex::Complex64`).
pub type Complex = Complex64;

/// Smallest value reported by the dB helpers instead of `-inf`.
pub const DB_FLOOR: f64 = -400.0;

/// Amplitude ratio to dB (20 log10), floored at [`DB_FLOOR`].
pub fn amp_to_db(x: f64) -> f64 {
    if x > 0.0 {
        (20.0 * x.log10()).max(DB_FLOOR)
    } else {
        DB_FLOOR
    }
}

/// Power ratio to dB (10 log10), floored at [`DB_FLOOR`].
pub fn pow_to_db(x: f64) -> f64 {
    if x > 0.0 {
        (10.0 * x.log10()).max(DB_FLOOR)
    } else {
        DB_FLOOR
    }
}

/// dB to amplitude ratio.
pub fn db_to_amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Smallest power of two >= `n` (and >= 1).
pub fn next_pow2(n: usize) -> usize {
    n.max(1).next_power_of_two()
}

/// Forward real FFT of `x` zero-padded / truncated to `n` points. Returns
/// `n/2+1` bins. `n` must be even and >= 2 (otherwise the result is empty).
pub fn rfft(x: &[f64], n: usize) -> Vec<Complex> {
    if n < 2 || n % 2 != 0 {
        return Vec::new();
    }
    let mut planner = RealFftPlanner::<f64>::new();
    let plan = planner.plan_fft_forward(n);
    let mut inp = vec![0.0; n];
    let m = x.len().min(n);
    inp[..m].copy_from_slice(&x[..m]);
    let mut out = plan.make_output_vec();
    if plan.process(&mut inp, &mut out).is_err() {
        return Vec::new();
    }
    out
}

/// Inverse of [`rfft`] including the `1/n` normalisation.
pub fn irfft(spec: &[Complex], n: usize) -> Vec<f64> {
    if n < 2 || n % 2 != 0 || spec.len() != n / 2 + 1 {
        return Vec::new();
    }
    let mut planner = RealFftPlanner::<f64>::new();
    let plan = planner.plan_fft_inverse(n);
    let mut s = spec.to_vec();
    s[0].im = 0.0;
    s[n / 2].im = 0.0;
    let mut out = vec![0.0; n];
    if plan.process(&mut s, &mut out).is_err() {
        return Vec::new();
    }
    let g = 1.0 / n as f64;
    out.iter_mut().for_each(|v| *v *= g);
    out
}

/// Analysis window shapes (periodic / DFT-even forms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Rectangular,
    Hann,
    /// 4-term minimum Blackman-Harris (-92 dB sidelobes).
    BlackmanHarris,
    /// 5-term flat-top (scalloping loss < 0.01 dB).
    FlatTop,
}

impl Window {
    /// Window coefficients of length `n`.
    pub fn coefficients(self, n: usize) -> Vec<f64> {
        let a: &[f64] = match self {
            Window::Rectangular => &[1.0],
            Window::Hann => &[0.5, 0.5],
            Window::BlackmanHarris => &[0.35875, 0.48829, 0.14128, 0.01168],
            Window::FlatTop => &[
                0.21557895,
                0.41663158,
                0.277263158,
                0.083578947,
                0.006947368,
            ],
        };
        (0..n)
            .map(|i| {
                let ph = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
                a.iter()
                    .enumerate()
                    .map(|(k, c)| {
                        let s = if k % 2 == 0 { 1.0 } else { -1.0 };
                        s * c * (k as f64 * ph).cos()
                    })
                    .sum()
            })
            .collect()
    }
}

/// `(sum w, sum w^2)` of a window.
pub fn window_sums(w: &[f64]) -> (f64, f64) {
    (w.iter().sum(), w.iter().map(|v| v * v).sum())
}
