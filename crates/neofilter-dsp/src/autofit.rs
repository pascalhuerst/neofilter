//! Fit a target magnitude curve with NeoFilter bands (auto-calibration).
//!
//! Greedy band placement followed by damped Gauss-Newton (Levenberg-Marquardt)
//! refinement of all `(log f, gain, log Q)` on the residual computed with the
//! *real digital band responses* ([`crate::response::band_response_db`]).
//!
//! Method:
//! 1. The target is clamped to `[-max_cut, +max_boost]` and points with zero
//!    weight are ignored by the cost (but still evaluated for the summary).
//! 2. Repeatedly: place a bell at the largest weighted residual (Q from the
//!    half-height width of the residual lobe, or a 12 dB/oct Q 1 shelf when
//!    the lobe reaches the edge of the data and is wide), then refine all
//!    bands jointly. Stop when the weighted residual RMS is below
//!    `target_rms_db`, when adding a band improves the RMS by less than
//!    `min_improvement_db`, or at `max_bands`.
//! 3. Bands with `|gain| < 0.1 dB` are removed, near-duplicate bells merged,
//!    the rest refined once more.
//! 4. The *sum* of all bands is checked on a dense grid from 10 Hz to the
//!    Nyquist frequency; if overlapping bands exceed `max_boost_db` (or
//!    `-max_cut_db`) by more than 0.25 dB the offending gains are scaled down.
//!
//! The cost carries a mild penalty on `gain^2`, so bands in regions without
//! data (zero weight) stay close to 0 dB. The result is deterministic.
//! Allocates; never call from the audio thread.

#![allow(clippy::needless_range_loop)]

use crate::response::band_response_db;
use crate::types::{BandParams, Placement, Shape, Slope};

/// Tolerance (dB) by which the summed curve may exceed the gain limits.
pub const LIMIT_TOLERANCE_DB: f64 = 0.25;
/// Bands with a smaller gain magnitude (dB) are dropped.
pub const NEGLIGIBLE_GAIN_DB: f64 = 0.1;

/// Options of [`fit_bands`].
#[derive(Debug, Clone)]
pub struct FitOptions {
    /// Maximum number of bands (bells and shelves together).
    pub max_bands: usize,
    pub allow_low_shelf: bool,
    pub allow_high_shelf: bool,
    /// Largest boost in dB (positive); applies to every band and to the sum.
    pub max_boost_db: f64,
    /// Largest cut in dB as a positive number; applies to every band and to the sum.
    pub max_cut_db: f64,
    pub min_q: f64,
    pub max_q: f64,
    pub min_freq_hz: f64,
    pub max_freq_hz: f64,
    /// Stop adding bands when the weighted residual RMS is below this (dB).
    pub target_rms_db: f64,
    /// A new band must improve the weighted RMS by at least this much (dB).
    pub min_improvement_db: f64,
    /// Plugin gain scale (1.0 = 100 %). Limits and the reported gains refer to
    /// the *effective* curve; the band `gain_db` is divided by this scale.
    pub gain_scale: f64,
    /// Strength of the penalty on band gains (see module docs).
    pub regularisation: f64,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            max_bands: 12,
            allow_low_shelf: false,
            allow_high_shelf: false,
            max_boost_db: 6.0,
            max_cut_db: 12.0,
            min_q: 0.3,
            max_q: 10.0,
            min_freq_hz: 20.0,
            max_freq_hz: 20_000.0,
            target_rms_db: 0.1,
            min_improvement_db: 0.004,
            gain_scale: 1.0,
            regularisation: 1e-3,
        }
    }
}

/// Result of [`fit_bands`].
#[derive(Debug, Clone)]
pub struct FitResult {
    /// Fitted bands (`used`, enabled, stereo), sorted by frequency.
    pub bands: Vec<BandParams>,
    /// Fitted curve at the target frequencies, dB.
    pub fitted_db: Vec<f64>,
    /// Weighted RMS of `fitted - clamped target` over points with weight > 0.
    pub residual_rms_db: f64,
    /// Largest `|fitted - clamped target|` over points with weight > 0.
    pub residual_max_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Bell,
    LowShelf,
    HighShelf,
}

/// Internal band: log frequency, effective gain, log Q.
#[derive(Debug, Clone, Copy)]
struct B {
    kind: Kind,
    lf: f64,
    g: f64,
    lq: f64,
}

impl B {
    fn nparams(&self) -> usize {
        if self.kind == Kind::Bell {
            3
        } else {
            2
        }
    }
}

struct Ctx<'a> {
    f: Vec<f64>,
    w: Vec<f64>,
    t: Vec<f64>,
    sr: f64,
    o: &'a FitOptions,
    wsum: f64,
    mu: f64,
    scale: f64,
}

fn to_params(b: &B, scale: f64) -> BandParams {
    BandParams {
        used: true,
        enabled: true,
        shape: match b.kind {
            Kind::Bell => Shape::Bell,
            Kind::LowShelf => Shape::LowShelf,
            Kind::HighShelf => Shape::HighShelf,
        },
        freq_hz: b.lf.exp(),
        gain_db: b.g / scale,
        q: if b.kind == Kind::Bell {
            b.lq.exp()
        } else {
            1.0
        },
        slope: Slope::Db12,
        placement: Placement::Stereo,
    }
}

/// Solve `A x = b` (dense, symmetric positive definite expected) in place.
fn solve(a: &mut [Vec<f64>], b: &mut [f64]) -> bool {
    let n = b.len();
    for c in 0..n {
        let mut p = c;
        for r in c + 1..n {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        if !(a[p][c].abs() > 1e-300) {
            return false;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let k = a[r][c] / a[c][c];
            if k != 0.0 {
                for j in c..n {
                    a[r][j] -= k * a[c][j];
                }
                b[r] -= k * b[c];
            }
        }
    }
    for c in (0..n).rev() {
        let mut s = b[c];
        for j in c + 1..n {
            s -= a[c][j] * b[j];
        }
        b[c] = s / a[c][c];
    }
    b.iter().all(|v| v.is_finite())
}

impl Ctx<'_> {
    fn resp(&self, b: &B, out: &mut [f64]) {
        band_response_db(&to_params(b, self.scale), self.scale, self.sr, &self.f, out);
    }

    fn clamp(&self, b: &mut B) {
        let o = self.o;
        b.lf = b.lf.clamp(o.min_freq_hz.ln(), o.max_freq_hz.ln());
        b.g = b.g.clamp(-o.max_cut_db, o.max_boost_db);
        b.lq = b.lq.clamp(o.min_q.ln(), o.max_q.ln());
    }

    /// Cost `sum w r^2 + mu sum g^2` for the given summed model.
    fn cost(&self, model: &[f64], bands: &[B]) -> f64 {
        let mut c = 0.0;
        for i in 0..self.f.len() {
            let r = model[i] - self.t[i];
            c += self.w[i] * r * r;
        }
        c + self.mu * bands.iter().map(|b| b.g * b.g).sum::<f64>()
    }

    fn model_of(&self, resps: &[Vec<f64>]) -> Vec<f64> {
        let mut m = vec![0.0; self.f.len()];
        for r in resps {
            for (a, b) in m.iter_mut().zip(r) {
                *a += b;
            }
        }
        m
    }

    fn rms(&self, model: &[f64]) -> f64 {
        let s: f64 = (0..self.f.len())
            .map(|i| {
                let r = model[i] - self.t[i];
                self.w[i] * r * r
            })
            .sum();
        (s / self.wsum).sqrt()
    }

    /// Levenberg-Marquardt refinement of all bands; returns the final cost.
    fn refine(&self, bands: &mut [B], max_iter: usize) -> f64 {
        let n = self.f.len();
        let nb = bands.len();
        if nb == 0 {
            return self.cost(&vec![0.0; n], bands);
        }
        let mut resps: Vec<Vec<f64>> = vec![vec![0.0; n]; nb];
        for (b, r) in bands.iter().zip(resps.iter_mut()) {
            self.resp(b, r);
        }
        let mut model = self.model_of(&resps);
        let mut cost = self.cost(&model, bands);
        let mut lambda = 1e-2;
        let steps = [1e-3, 0.02, 1e-3];
        let mut tmp = vec![0.0; n];
        for _ in 0..max_iter {
            // parameter layout
            let mut idx = Vec::new(); // (band, param 0=lf 1=g 2=lq)
            for (j, b) in bands.iter().enumerate() {
                idx.push((j, 0));
                idx.push((j, 1));
                if b.nparams() == 3 {
                    idx.push((j, 2));
                }
            }
            let np = idx.len();
            // Jacobian columns by forward differences
            let mut jac: Vec<Vec<f64>> = Vec::with_capacity(np);
            for &(j, p) in &idx {
                let mut pb = bands[j];
                let h = steps[p];
                match p {
                    0 => pb.lf += h,
                    1 => pb.g += h,
                    _ => pb.lq += h,
                }
                self.resp(&pb, &mut tmp);
                jac.push((0..n).map(|i| (tmp[i] - resps[j][i]) / h).collect());
            }
            let res: Vec<f64> = (0..n).map(|i| model[i] - self.t[i]).collect();
            let mut h = vec![vec![0.0; np]; np];
            let mut grad = vec![0.0; np];
            for a in 0..np {
                for b in a..np {
                    let mut s = 0.0;
                    for i in 0..n {
                        s += self.w[i] * jac[a][i] * jac[b][i];
                    }
                    h[a][b] = s;
                    h[b][a] = s;
                }
                let mut s = 0.0;
                for i in 0..n {
                    s += self.w[i] * jac[a][i] * res[i];
                }
                grad[a] = s;
                if idx[a].1 == 1 {
                    h[a][a] += self.mu;
                    grad[a] += self.mu * bands[idx[a].0].g;
                }
            }
            let mut improved = false;
            for _ in 0..8 {
                let mut m = h.clone();
                for a in 0..np {
                    m[a][a] += lambda * (h[a][a].abs() + 1e-6) + 1e-9;
                }
                let mut rhs: Vec<f64> = grad.iter().map(|v| -v).collect();
                if !solve(&mut m, &mut rhs) {
                    lambda *= 10.0;
                    continue;
                }
                let mut nbands = bands.to_vec();
                for (k, &(j, p)) in idx.iter().enumerate() {
                    let d = rhs[k];
                    match p {
                        0 => nbands[j].lf += d.clamp(-0.5, 0.5),
                        1 => nbands[j].g += d.clamp(-6.0, 6.0),
                        _ => nbands[j].lq += d.clamp(-0.7, 0.7),
                    }
                }
                for b in nbands.iter_mut() {
                    self.clamp(b);
                }
                let mut nresps = resps.clone();
                for j in 0..nb {
                    self.resp(&nbands[j], &mut nresps[j]);
                }
                let nmodel = self.model_of(&nresps);
                let ncost = self.cost(&nmodel, &nbands);
                if ncost < cost {
                    let rel = (cost - ncost) / cost.max(1e-30);
                    bands.copy_from_slice(&nbands);
                    resps = nresps;
                    model = nmodel;
                    cost = ncost;
                    lambda = (lambda * 0.3).max(1e-7);
                    improved = rel > 1e-7;
                    if !improved {
                        // converged
                        return cost;
                    }
                    break;
                }
                lambda *= 4.0;
                if lambda > 1e8 {
                    break;
                }
            }
            if !improved {
                break;
            }
        }
        cost
    }

    /// Candidate band for the largest weighted residual `r = t - model`.
    fn propose(&self, r: &[f64], have_low: bool, have_high: bool) -> Option<B> {
        let n = self.f.len();
        let mut best = None;
        let mut bv = 0.0;
        for i in 0..n {
            let v = self.w[i] * r[i].abs();
            if self.w[i] > 0.0 && v > bv {
                bv = v;
                best = Some(i);
            }
        }
        let i = best?;
        let peak = r[i];
        let sign = peak.signum();
        let half = peak.abs() * 0.5;
        let same = |k: usize| r[k] * sign >= half;
        let mut il = i;
        while il > 0 && same(il - 1) {
            il -= 1;
        }
        let mut ir = i;
        while ir + 1 < n && same(ir + 1) {
            ir += 1;
        }
        let o = self.o;
        let step = if n > 1 {
            (self.f[n - 1] / self.f[0]).log2() / (n - 1) as f64
        } else {
            0.1
        };
        let bw = ((self.f[ir] / self.f[il]).log2() + step).max(step);
        let wide = bw >= 1.5;
        let g = peak.clamp(-o.max_cut_db, o.max_boost_db);
        if o.allow_low_shelf && !have_low && il == 0 && wide {
            let mut b = B {
                kind: Kind::LowShelf,
                lf: self.f[ir].ln(),
                g: r[0].clamp(-o.max_cut_db, o.max_boost_db),
                lq: 0.0,
            };
            self.clamp(&mut b);
            return Some(b);
        }
        if o.allow_high_shelf && !have_high && ir == n - 1 && wide {
            let mut b = B {
                kind: Kind::HighShelf,
                lf: self.f[il].ln(),
                g: r[n - 1].clamp(-o.max_cut_db, o.max_boost_db),
                lq: 0.0,
            };
            self.clamp(&mut b);
            return Some(b);
        }
        let p2 = 2f64.powf(bw);
        let q = (p2.sqrt() / (p2 - 1.0).max(1e-6)).clamp(o.min_q, o.max_q);
        let mut b = B {
            kind: Kind::Bell,
            lf: self.f[i].ln(),
            g,
            lq: q.ln(),
        };
        self.clamp(&mut b);
        Some(b)
    }

    /// Dense grid for the limit check: 10 Hz .. 0.499 fs.
    fn dense_grid(&self) -> Vec<f64> {
        let hi = (0.499 * self.sr).min(30_000.0);
        let n = 480;
        (0..n)
            .map(|i| 10.0 * (hi / 10.0).powf(i as f64 / (n - 1) as f64))
            .collect()
    }

    /// Scale gains down so that the summed curve respects the limits.
    fn enforce_limits(&self, bands: &mut [B]) {
        let grid = self.dense_grid();
        let mut tmp = vec![0.0; grid.len()];
        for _ in 0..40 {
            let mut sum = vec![0.0; grid.len()];
            for b in bands.iter() {
                band_response_db(
                    &to_params(b, self.scale),
                    self.scale,
                    self.sr,
                    &grid,
                    &mut tmp,
                );
                for (s, t) in sum.iter_mut().zip(&tmp) {
                    *s += t;
                }
            }
            let mx = sum.iter().cloned().fold(f64::MIN, f64::max);
            let mn = sum.iter().cloned().fold(f64::MAX, f64::min);
            let over_boost = mx > self.o.max_boost_db + LIMIT_TOLERANCE_DB;
            let over_cut = -mn > self.o.max_cut_db + LIMIT_TOLERANCE_DB;
            if !over_boost && !over_cut {
                return;
            }
            if over_boost {
                let k = (self.o.max_boost_db + 0.05).max(0.0) / mx;
                for b in bands.iter_mut().filter(|b| b.g > 0.0) {
                    b.g *= k.min(0.98);
                }
            }
            if over_cut {
                let k = (self.o.max_cut_db + 0.05).max(0.0) / -mn;
                for b in bands.iter_mut().filter(|b| b.g < 0.0) {
                    b.g *= k.min(0.98);
                }
            }
        }
    }
}

fn merge_close(bands: &mut Vec<B>) -> bool {
    let mut changed = false;
    'outer: loop {
        for i in 0..bands.len() {
            for j in i + 1..bands.len() {
                let (a, b) = (bands[i], bands[j]);
                if a.kind == Kind::Bell
                    && b.kind == Kind::Bell
                    && (a.lf - b.lf).abs() < 0.04
                    && (a.lq - b.lq).abs() < 0.4
                {
                    let wa = a.g.abs() + 1e-9;
                    let wb = b.g.abs() + 1e-9;
                    let m = B {
                        kind: Kind::Bell,
                        lf: (a.lf * wa + b.lf * wb) / (wa + wb),
                        g: a.g + b.g,
                        lq: (a.lq + b.lq) / 2.0,
                    };
                    bands[i] = m;
                    bands.remove(j);
                    changed = true;
                    continue 'outer;
                }
            }
        }
        return changed;
    }
}

/// Fit `target` (ascending `(Hz, dB)` points, usually log spaced) with bands.
///
/// `weights` (same length, `>= 0`; `None` = all 1) say how much each point
/// matters; zero weight leaves a point unconstrained. Fewer than 3 points
/// with positive weight, or a target already within the stopping RMS of 0 dB,
/// give an empty band list.
pub fn fit_bands(
    target: &[(f64, f64)],
    weights: Option<&[f64]>,
    sample_rate: f64,
    opts: &FitOptions,
) -> FitResult {
    let n_in = target.len();
    let empty = |db: Vec<f64>| FitResult {
        bands: Vec::new(),
        fitted_db: db,
        residual_rms_db: 0.0,
        residual_max_db: 0.0,
    };
    let scale = if opts.gain_scale.is_finite() && opts.gain_scale > 0.02 {
        opts.gain_scale
    } else {
        return empty(vec![0.0; n_in]);
    };
    if !(sample_rate.is_finite() && sample_rate > 1000.0) || opts.max_bands == 0 {
        return empty(vec![0.0; n_in]);
    }
    let boost = opts.max_boost_db.max(0.0);
    let cut = opts.max_cut_db.max(0.0);
    let mut o = opts.clone();
    o.max_boost_db = boost;
    o.max_cut_db = cut;
    o.max_freq_hz = o
        .max_freq_hz
        .min(0.49 * sample_rate)
        .max(o.min_freq_hz * 1.01);
    o.min_q = o.min_q.max(0.025);
    o.max_q = o.max_q.max(o.min_q);

    // Clean inputs: only usable points take part in the fit.
    let mut f = Vec::new();
    let mut t = Vec::new();
    let mut w = Vec::new();
    for (i, &(hz, db)) in target.iter().enumerate() {
        let wi = weights.map_or(1.0, |ws| ws.get(i).copied().unwrap_or(0.0));
        if hz.is_finite() && hz > 0.0 && db.is_finite() && wi.is_finite() && wi > 0.0 {
            f.push(hz);
            t.push(db.clamp(-cut, boost));
            w.push(wi);
        }
    }
    if f.len() < 3 {
        return empty(vec![0.0; n_in]);
    }
    let wsum: f64 = w.iter().sum();
    let ctx = Ctx {
        f,
        w,
        t,
        sr: sample_rate,
        wsum,
        mu: opts.regularisation.max(0.0) * wsum / 100.0,
        scale,
        o: &o,
    };

    let mut bands: Vec<B> = Vec::new();
    let mut model = vec![0.0; ctx.f.len()];
    let mut rms = ctx.rms(&model);
    while rms > o.target_rms_db && bands.len() < o.max_bands {
        let r: Vec<f64> = (0..ctx.f.len()).map(|i| ctx.t[i] - model[i]).collect();
        let have_low = bands.iter().any(|b| b.kind == Kind::LowShelf);
        let have_high = bands.iter().any(|b| b.kind == Kind::HighShelf);
        let Some(cand) = ctx.propose(&r, have_low, have_high) else {
            break;
        };
        let mut trial = bands.clone();
        trial.push(cand);
        ctx.refine(&mut trial, 14);
        let m = model_of_bands(&ctx, &trial);
        let nrms = ctx.rms(&m);
        if rms - nrms < o.min_improvement_db {
            break;
        }
        bands = trial;
        model = m;
        rms = nrms;
    }
    if !bands.is_empty() {
        ctx.refine(&mut bands, 60);
        for _ in 0..2 {
            let before = bands.len();
            bands.retain(|b| b.g.abs() >= NEGLIGIBLE_GAIN_DB);
            let merged = merge_close(&mut bands);
            if bands.len() == before && !merged {
                break;
            }
            ctx.refine(&mut bands, 30);
        }
        bands.retain(|b| b.g.abs() >= NEGLIGIBLE_GAIN_DB);
        ctx.enforce_limits(&mut bands);
        bands.sort_by(|a, b| a.lf.total_cmp(&b.lf));
    }

    // Summary over the caller's points.
    let params: Vec<BandParams> = bands.iter().map(|b| to_params(b, scale)).collect();
    let freqs: Vec<f64> = target.iter().map(|p| p.0).collect();
    let mut fitted = vec![0.0; n_in];
    let mut tmp = vec![0.0; n_in];
    for p in &params {
        band_response_db(p, scale, sample_rate, &freqs, &mut tmp);
        for (a, b) in fitted.iter_mut().zip(&tmp) {
            *a += b;
        }
    }
    let (mut se, mut sw, mut mx) = (0.0, 0.0, 0.0f64);
    for (i, &(_, db)) in target.iter().enumerate() {
        let wi = weights.map_or(1.0, |ws| ws.get(i).copied().unwrap_or(0.0));
        if wi > 0.0 && wi.is_finite() && db.is_finite() {
            let r = fitted[i] - db.clamp(-cut, boost);
            se += wi * r * r;
            sw += wi;
            mx = mx.max(r.abs());
        }
    }
    FitResult {
        bands: params,
        fitted_db: fitted,
        residual_rms_db: if sw > 0.0 { (se / sw).sqrt() } else { 0.0 },
        residual_max_db: mx,
    }
}

fn model_of_bands(ctx: &Ctx, bands: &[B]) -> Vec<f64> {
    let mut m = vec![0.0; ctx.f.len()];
    let mut tmp = vec![0.0; ctx.f.len()];
    for b in bands {
        ctx.resp(b, &mut tmp);
        for (a, t) in m.iter_mut().zip(&tmp) {
            *a += t;
        }
    }
    m
}

/// Summed response (dB) of fitted bands at `freqs` (convenience for callers).
pub fn bands_response_db(
    bands: &[BandParams],
    gain_scale: f64,
    sample_rate: f64,
    freqs: &[f64],
) -> Vec<f64> {
    let mut out = vec![0.0; freqs.len()];
    let mut tmp = vec![0.0; freqs.len()];
    for b in bands.iter().filter(|b| b.used && b.enabled) {
        band_response_db(b, gain_scale, sample_rate, freqs, &mut tmp);
        for (a, t) in out.iter_mut().zip(&tmp) {
            *a += t;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct Lcg(u64);
    impl Lcg {
        fn u(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, a: f64, b: f64) -> f64 {
            a + (b - a) * self.u()
        }
    }

    fn grid(n: usize, lo: f64, hi: f64) -> Vec<f64> {
        (0..n)
            .map(|i| lo * (hi / lo).powf(i as f64 / (n - 1) as f64))
            .collect()
    }

    fn bell(f: f64, g: f64, q: f64) -> BandParams {
        BandParams {
            used: true,
            shape: Shape::Bell,
            freq_hz: f,
            gain_db: g,
            q,
            ..Default::default()
        }
    }

    fn pts(freqs: &[f64], db: &[f64]) -> Vec<(f64, f64)> {
        freqs.iter().cloned().zip(db.iter().cloned()).collect()
    }

    /// Random bells separated by at least 0.5 octave.
    fn random_bells(rng: &mut Lcg, n: usize) -> Vec<BandParams> {
        let mut v: Vec<BandParams> = Vec::new();
        while v.len() < n {
            let f = (rng.range(40f64.ln(), 14000f64.ln())).exp();
            if v.iter().any(|b| (b.freq_hz / f).log2().abs() < 0.6) {
                continue;
            }
            let mag = rng.range(1.5, 7.0);
            let g = if rng.u() < 0.5 { mag } else { -mag };
            v.push(bell(f, g, rng.range(0.7, 6.0)));
        }
        v
    }

    #[test]
    fn recovers_random_curves() {
        let sr = 48000.0;
        let f = grid(200, 20.0, 20000.0);
        let mut worst_rms = 0.0f64;
        let mut worst_max = 0.0f64;
        for (seed, k) in (1..=3u64).flat_map(|s| (1..=10usize).map(move |k| (s, k))) {
            let mut rng = Lcg(seed * 1000 + k as u64);
            let truth = random_bells(&mut rng, k);
            let db = bands_response_db(&truth, 1.0, sr, &f);
            let opts = FitOptions {
                max_boost_db: 20.0,
                max_cut_db: 20.0,
                ..Default::default()
            };
            let r = fit_bands(&pts(&f, &db), None, sr, &opts);
            eprintln!(
                "k={k}: {} bands rms {:.3} max {:.3}",
                r.bands.len(),
                r.residual_rms_db,
                r.residual_max_db
            );
            worst_rms = worst_rms.max(r.residual_rms_db);
            worst_max = worst_max.max(r.residual_max_db);
            assert!(r.residual_rms_db <= 0.15, "k={k} rms {}", r.residual_rms_db);
            assert!(r.residual_max_db <= 0.5, "k={k} max {}", r.residual_max_db);
            assert!(r.bands.len() <= k + 2 || r.bands.len() == opts.max_bands);
        }
        eprintln!("worst rms {worst_rms:.3} max {worst_max:.3}");
    }

    #[test]
    fn tilt_plus_resonance() {
        let sr = 48000.0;
        let f = grid(240, 30.0, 16000.0);
        let db: Vec<f64> = f
            .iter()
            .map(|&x| {
                let tilt = 1.2 * (x / 1000.0).log2();
                let res = 4.0 / (1.0 + (3.0 * (x / 9000.0).log2()).powi(2) * 4.0);
                tilt + res
            })
            .collect();
        let opts = FitOptions {
            allow_low_shelf: true,
            allow_high_shelf: true,
            max_boost_db: 12.0,
            max_cut_db: 12.0,
            ..Default::default()
        };
        let r = fit_bands(&pts(&f, &db), None, sr, &opts);
        eprintln!(
            "tilt: {} bands rms {:.3} max {:.3}",
            r.bands.len(),
            r.residual_rms_db,
            r.residual_max_db
        );
        assert!(r.residual_rms_db < 0.3, "{}", r.residual_rms_db);
        assert!(r.residual_max_db < 0.8, "{}", r.residual_max_db);
        assert!(r.bands.len() <= 12);
    }

    #[test]
    fn respects_all_limits() {
        let sr = 44100.0;
        let f = grid(200, 20.0, 20000.0);
        // wants +15 dB in a broad hump and -25 dB in a notch
        let db: Vec<f64> = f
            .iter()
            .map(|&x| {
                15.0 * (-((x / 5000.0).log2().powi(2)) / 0.5).exp()
                    - 25.0 * (-((x / 300.0).log2().powi(2)) / 0.1).exp()
            })
            .collect();
        let opts = FitOptions {
            max_bands: 6,
            max_boost_db: 6.0,
            max_cut_db: 10.0,
            min_q: 0.5,
            max_q: 4.0,
            min_freq_hz: 100.0,
            max_freq_hz: 12000.0,
            ..Default::default()
        };
        let r = fit_bands(&pts(&f, &db), None, sr, &opts);
        assert!(r.bands.len() <= 6);
        for b in &r.bands {
            assert!(b.gain_db <= 6.0 + 1e-9 && b.gain_db >= -10.0 - 1e-9);
            assert!(b.q >= 0.5 - 1e-9 && b.q <= 4.0 + 1e-9, "q {}", b.q);
            assert!(b.freq_hz >= 100.0 - 1e-6 && b.freq_hz <= 12000.0 + 1e-6);
        }
        let dense = grid(1000, 10.0, 0.499 * sr);
        let sum = bands_response_db(&r.bands, 1.0, sr, &dense);
        let mx = sum.iter().cloned().fold(f64::MIN, f64::max);
        let mn = sum.iter().cloned().fold(f64::MAX, f64::min);
        assert!(mx <= 6.0 + LIMIT_TOLERANCE_DB, "max {mx}");
        assert!(mn >= -10.0 - LIMIT_TOLERANCE_DB, "min {mn}");
    }

    #[test]
    fn overlapping_bells_do_not_exceed_cap() {
        let sr = 48000.0;
        let f = grid(200, 20.0, 20000.0);
        // a plateau at the cap made of a sum that would overshoot when fitted
        let db: Vec<f64> = f
            .iter()
            .map(|&x| {
                if (3000.0..9000.0).contains(&x) {
                    6.0
                } else {
                    0.0
                }
            })
            .collect();
        let r = fit_bands(&pts(&f, &db), None, sr, &FitOptions::default());
        let dense = grid(1000, 10.0, 0.499 * sr);
        let sum = bands_response_db(&r.bands, 1.0, sr, &dense);
        let mx = sum.iter().cloned().fold(f64::MIN, f64::max);
        assert!(mx <= 6.0 + LIMIT_TOLERANCE_DB, "max {mx}");
    }

    #[test]
    fn zero_weights_leave_region_free_but_tame() {
        let sr = 48000.0;
        let f = grid(200, 20.0, 20000.0);
        let truth = vec![bell(300.0, -5.0, 2.0), bell(1500.0, 4.0, 2.0)];
        let db = bands_response_db(&truth, 1.0, sr, &f);
        // the region above 4 kHz carries garbage and zero weight
        let mut noisy = db.clone();
        let w: Vec<f64> = f
            .iter()
            .map(|&x| if x > 4000.0 { 0.0 } else { 1.0 })
            .collect();
        for (v, &x) in noisy.iter_mut().zip(&f) {
            if x > 4000.0 {
                *v = 30.0;
            }
        }
        let r = fit_bands(&pts(&f, &noisy), Some(&w), sr, &FitOptions::default());
        assert!(r.residual_rms_db < 0.15, "{}", r.residual_rms_db);
        let hf = grid(100, 4000.0, 20000.0);
        let sum = bands_response_db(&r.bands, 1.0, sr, &hf);
        let mx = sum.iter().cloned().fold(0.0f64, |a, b| a.max(b.abs()));
        assert!(mx < 1.5, "unconstrained region wandered to {mx} dB");
        for b in &r.bands {
            assert!(b.freq_hz < 4000.0 * 1.5 || b.gain_db.abs() < 1.0);
        }
    }

    #[test]
    fn degenerate_targets_give_no_bands() {
        let sr = 48000.0;
        let o = FitOptions::default();
        assert!(fit_bands(&[], None, sr, &o).bands.is_empty());
        assert!(fit_bands(&[(100.0, 3.0)], None, sr, &o).bands.is_empty());
        let f = grid(50, 20.0, 20000.0);
        let flat = pts(&f, &vec![0.0; 50]);
        let r = fit_bands(&flat, None, sr, &o);
        assert!(r.bands.is_empty() && r.fitted_db.len() == 50);
        let nan = pts(&f, &vec![f64::NAN; 50]);
        assert!(fit_bands(&nan, None, sr, &o).bands.is_empty());
        let zw = vec![0.0; 50];
        let one = pts(&f, &vec![3.0; 50]);
        assert!(fit_bands(&one, Some(&zw), sr, &o).bands.is_empty());
        let bad_sr = fit_bands(&one, None, 0.0, &o);
        assert!(bad_sr.bands.is_empty());
        let zero_bands = FitOptions {
            max_bands: 0,
            ..Default::default()
        };
        assert!(fit_bands(&one, None, sr, &zero_bands).bands.is_empty());
    }

    #[test]
    fn deterministic() {
        let sr = 48000.0;
        let f = grid(200, 20.0, 20000.0);
        let truth = random_bells(&mut Lcg(7), 5);
        let db = bands_response_db(&truth, 1.0, sr, &f);
        let a = fit_bands(&pts(&f, &db), None, sr, &FitOptions::default());
        let b = fit_bands(&pts(&f, &db), None, sr, &FitOptions::default());
        assert_eq!(a.bands, b.bands);
    }

    #[test]
    fn gain_scale_is_compensated() {
        let sr = 48000.0;
        let f = grid(200, 20.0, 20000.0);
        let truth = vec![bell(1000.0, 4.0, 2.0)];
        let db = bands_response_db(&truth, 1.0, sr, &f);
        let o = FitOptions {
            gain_scale: 0.5,
            ..Default::default()
        };
        let r = fit_bands(&pts(&f, &db), None, sr, &o);
        let back = bands_response_db(&r.bands, 0.5, sr, &f);
        let err = back
            .iter()
            .zip(&db)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(err < 0.3, "{err}");
        assert!(r.bands[0].gain_db > 6.0);
    }

    #[test]
    fn twelve_bands_on_200_points_is_fast() {
        let sr = 48000.0;
        let f = grid(200, 20.0, 20000.0);
        let truth = random_bells(&mut Lcg(99), 10);
        let db = bands_response_db(&truth, 1.0, sr, &f);
        let o = FitOptions {
            max_boost_db: 20.0,
            max_cut_db: 20.0,
            max_bands: 12,
            target_rms_db: 0.0,
            min_improvement_db: -1.0,
            ..Default::default()
        };
        let t0 = Instant::now();
        let r = fit_bands(&pts(&f, &db), None, sr, &o);
        let dt = t0.elapsed().as_secs_f64();
        eprintln!("12-band fit: {dt:.3} s ({} bands)", r.bands.len());
        // The bound is for release builds; unoptimised test builds are much slower.
        let limit = if cfg!(debug_assertions) { 20.0 } else { 1.0 };
        assert!(dt < limit, "{dt}");
    }
}
