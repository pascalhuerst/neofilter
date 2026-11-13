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

/// Solve `a x = b` in place by Gaussian elimination with partial pivoting.
/// Returns `false` for a singular system.
pub(crate) fn solve_linear(a: &mut [Vec<f64>], b: &mut [f64]) -> bool {
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

/// Levenberg-Marquardt least squares with a numerical Jacobian. `resid`
/// maps parameters to a residual vector; returns the final parameters.
pub(crate) fn levenberg_marquardt(
    mut th: Vec<f64>,
    resid: impl Fn(&[f64]) -> Vec<f64>,
    iterations: usize,
) -> Vec<f64> {
    let n = th.len();
    let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let mut r = resid(&th);
    let mut c = cost(&r);
    let mut lam = 1e-3;
    for _ in 0..iterations {
        let mut jac = vec![vec![0.0; n]; r.len()];
        for j in 0..n {
            let mut t2 = th.clone();
            let h = 1e-7 * (1.0 + th[j].abs());
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
            if !solve_linear(&mut a, &mut rhs) {
                lam *= 10.0;
                continue;
            }
            let t2: Vec<f64> = th.iter().zip(&rhs).map(|(a, b)| a + b).collect();
            let r2 = resid(&t2);
            let c2 = cost(&r2);
            if c2 < c {
                improved = (c - c2) / c.max(1e-30) > 1e-10;
                th = t2;
                r = r2;
                c = c2;
                lam = (lam * 0.3).max(1e-12);
                break;
            }
            lam *= 5.0;
        }
        if !improved {
            break;
        }
    }
    th
}
