//! Level-versus-frequency tracking of gliding sweeps and stepped spot tones,
//! plus the helpers that turn the tracked levels into a deviation curve
//! (nominal characteristics, normalisation, smoothing).
//!
//! Realtime: **off-thread analysis** (construction and `push` allocate
//! occasionally; one FFT per block per size).
//!
//! # Method
//!
//! The input is analysed in overlapping (87.5 %) Hann-windowed blocks. A *bank*
//! of block lengths, each four times shorter than the previous one, makes
//! every block hold between [`SweepConfig::cycles_lo`] and
//! `4 * cycles_lo` cycles of the tone it is responsible for (8..32 by
//! default); the block whose range contains the dominant spectral peak is the
//! only one that accepts it, so a tone is never counted twice.
//!
//! For every block:
//!
//! 1. the dominant peak of a 2x zero-padded FFT is located and refined by log-
//!    parabolic interpolation (frequency `f`),
//! 2. the linear chirp rate `k` is estimated from the peak frequencies of the
//!    two half blocks (a gliding sweep moves a few percent within a block),
//! 3. the block is **de-chirped** and its amplitude read from the windowed
//!    DTFT at `(f, k)`: `A = 2 |sum w x e^{-j(2 pi f t + pi k t^2)}| / sum w`
//!    (peak amplitude, full scale sine = 0 dBFS). This makes the level
//!    independent of the sweep rate,
//! 4. the block is accepted only if the residual after removing the fitted
//!    tone is at least [`SweepConfig::min_snr_db`] below the tone (with the
//!    mean removed first). Silence, voice announcements, noise bursts and
//!    stepped-tone transitions inside a block fail this test; the test is on
//!    the total block power, so a single clear sinusoid is required.
//!
//! Accepted `(frequency, level)` pairs fall into cells of a log grid
//! ([`SweepConfig::points_per_octave`], 24 by default, anchored at `f_lo`).
//! The cell statistic is the **median** of the levels (robust to the odd
//! mis-accepted block) reported at the median frequency of the blocks. It
//! works identically for sweeps up or down and for spot tones (which fill one
//! cell with many blocks). A cell counts as *covered* with at least
//! [`SweepConfig::min_count`] accepted blocks.
//!
//! With two channels the block of the weaker channel is additionally
//! evaluated at the `(f, k)` of the stronger one, which gives the inter-
//! channel level difference and phase of each block (see [`PairStats`]); that
//! is what the modulation detection of [`crate::velocity`] uses.
//!
//! # Nominal characteristics
//!
//! [`Characteristic`] describes how the recorded groove velocity varies with
//! frequency; [`expected_db`] combines it with the playback preamp (RIAA
//! de-emphasis or flat) into the level the playback chain should deliver,
//! assuming a velocity-sensitive (magnetic) cartridge:
//!
//! | characteristic | recorded velocity | expected output, RIAA preamp | flat preamp |
//! |---|---|---|---|
//! | constant velocity | constant | RIAA de-emphasis curve | flat |
//! | flat after RIAA | RIAA pre-emphasis curve (cut with RIAA) | flat | pre-emphasis curve |
//! | const. velocity above / const. amplitude below turnover `f_t` | `min(f, f_t) / f_t` | that times the de-emphasis | that |
//!
//! The deviation reported is `level - expected - norm_offset`, i.e. the
//! measured curve relative to the expected one, forced to 0 dB at the
//! normalisation frequency (or to a supplied offset).

use crate::riaa::{analog_response, Mode, Options};
use crate::util::Complex;
use crate::Sample;
use realfft::{RealFftPlanner, RealToComplex};
use std::f64::consts::PI;
use std::sync::Arc;

/// Per-cell cap of stored block results (reservoir sampling beyond it).
const CELL_CAP: usize = 512;
/// Smallest block length used (samples).
const MIN_BLOCK: usize = 64;

/// Tracker settings.
#[derive(Debug, Clone, Copy)]
pub struct SweepConfig {
    pub sample_rate: f64,
    /// Lowest tracked frequency, also the anchor of the grid.
    pub f_lo: f64,
    /// Highest tracked frequency.
    pub f_hi: f64,
    /// Grid density (default 24 per octave).
    pub points_per_octave: usize,
    /// Required tone-to-residual ratio of a block, dB (default 25).
    pub min_snr_db: f64,
    /// Blocks whose tone is below this level (dBFS) are ignored.
    pub min_level_dbfs: f64,
    /// Accepted blocks needed for a cell to count as covered (default 2).
    pub min_count: usize,
    /// Cycles of the tone in the longest block of each size (default 8).
    pub cycles_lo: f64,
}

impl SweepConfig {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            f_lo: 20.0,
            f_hi: (0.45 * sample_rate).min(40_000.0),
            points_per_octave: 24,
            min_snr_db: 25.0,
            min_level_dbfs: -100.0,
            min_count: 2,
            cycles_lo: 8.0,
        }
    }
}

/// One covered grid cell of one channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepCell {
    /// Index on the grid.
    pub index: usize,
    /// Median frequency of the accepted blocks.
    pub freq_hz: f64,
    /// Median peak level, dBFS.
    pub level_db: f64,
    /// Accepted blocks.
    pub count: usize,
}

/// Inter-channel statistics over all blocks where both channels were
/// accepted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairStats {
    /// Median of `level(L) - level(R)` in dB.
    pub level_diff_db: f64,
    /// Circular mean of the phase of L relative to R, degrees (-180..180).
    pub phase_deg: f64,
    /// Phase coherence `|mean unit phasor|` (1 = steady).
    pub phase_coherence: f64,
    pub blocks: usize,
}

/// Block counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackerStats {
    /// Blocks in which a spectral peak was found in its size range.
    pub candidates: [u64; 2],
    /// Blocks accepted as a clean tone.
    pub accepted: [u64; 2],
}

#[derive(Default, Clone)]
struct Cell {
    seen: u64,
    v: Vec<(f32, f32)>,
}

impl Cell {
    fn add(&mut self, f: f64, level: f64) {
        self.seen += 1;
        if self.v.len() < CELL_CAP {
            self.v.push((f as f32, level as f32));
        } else {
            let idx =
                ((self.seen.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) % CELL_CAP as u64) as usize;
            self.v[idx] = (f as f32, level as f32);
        }
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

struct Tone {
    f: f64,
    k: f64,
    amp: f64,
}

/// FFT machinery and window of one block size.
struct Analyzer {
    n: usize,
    w: Vec<f64>,
    w_half: Vec<f64>,
    sum_w: f64,
    sum_w2: f64,
    full: Arc<dyn RealToComplex<f64>>,
    half: Arc<dyn RealToComplex<f64>>,
    inp: Vec<f64>,
    out: Vec<Complex>,
    inp_h: Vec<f64>,
    out_h: Vec<Complex>,
    xw: Vec<f64>,
    /// Frequency range this size is responsible for.
    range: (f64, f64),
}

fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * (i as f64 + 0.5) / n as f64).cos())
        .collect()
}

/// Log-parabolic peak interpolation: offset of the peak from bin `k` in bins.
fn interp(mag: &[f64], k: usize) -> f64 {
    if k == 0 || k + 1 >= mag.len() {
        return 0.0;
    }
    let (a, b, c) = (
        mag[k - 1].max(1e-300).ln(),
        mag[k].max(1e-300).ln(),
        mag[k + 1].max(1e-300).ln(),
    );
    let d = a - 2.0 * b + c;
    if d >= 0.0 {
        return 0.0;
    }
    (0.5 * (a - c) / d).clamp(-1.0, 1.0)
}

/// `sum xw[i] e^{-j(om m + g m^2)}` with `m = i - (n - 1) / 2`.
fn dtft_chirp(xw: &[f64], fs: f64, f: f64, k: f64) -> Complex {
    let n = xw.len();
    let c = (n as f64 - 1.0) / 2.0;
    let om = 2.0 * PI * f / fs;
    let g = PI * k / (fs * fs);
    let q = Complex::new((2.0 * g).cos(), -(2.0 * g).sin());
    let mut acc = Complex::new(0.0, 0.0);
    let mut i = 0;
    while i < n {
        // re-synchronise z and r exactly every 256 samples
        let m = i as f64 - c;
        let p0 = om * m + g * m * m;
        let mut z = Complex::new(p0.cos(), -p0.sin());
        let p1 = om + g * (2.0 * m + 1.0);
        let mut r = Complex::new(p1.cos(), -p1.sin());
        let end = (i + 256).min(n);
        for &v in &xw[i..end] {
            acc += z * v;
            z *= r;
            r *= q;
        }
        i = end;
    }
    acc
}

impl Analyzer {
    fn new(n: usize, range: (f64, f64)) -> Self {
        let mut p = RealFftPlanner::<f64>::new();
        let full = p.plan_fft_forward(2 * n);
        let half = p.plan_fft_forward(n);
        let w = hann(n);
        let sum_w = w.iter().sum();
        let sum_w2 = w.iter().map(|v| v * v).sum();
        Self {
            n,
            w_half: hann(n / 2),
            sum_w,
            sum_w2,
            inp: full.make_input_vec(),
            out: full.make_output_vec(),
            inp_h: half.make_input_vec(),
            out_h: half.make_output_vec(),
            full,
            half,
            xw: vec![0.0; n],
            w,
            range,
        }
    }

    /// Peak (bin, refined frequency) of `mag` within `[f_lo, f_hi]`.
    fn peak(mag: &[f64], df: f64, f_lo: f64, f_hi: f64) -> Option<f64> {
        let k_lo = ((f_lo / df).ceil() as usize).max(2);
        let k_hi = ((f_hi / df).floor() as usize).min(mag.len().saturating_sub(3));
        if k_lo >= k_hi {
            return None;
        }
        let mut kp = k_lo;
        let mut best = 0.0;
        for (k, &m) in mag.iter().enumerate().take(k_hi + 1).skip(k_lo) {
            if m > best {
                best = m;
                kp = k;
            }
        }
        if best <= 0.0 {
            return None;
        }
        Some((kp as f64 + interp(mag, kp)) * df)
    }

    /// Window and demean `x` into `self.xw`; returns the windowed energy.
    fn prepare(&mut self, x: &[f64]) -> f64 {
        let mean = x.iter().sum::<f64>() / x.len() as f64;
        let mut e = 0.0;
        for ((o, &v), &w) in self.xw.iter_mut().zip(x).zip(&self.w) {
            *o = (v - mean) * w;
            e += *o * *o;
        }
        e
    }

    fn spectrum(
        plan: &Arc<dyn RealToComplex<f64>>,
        inp: &mut [f64],
        out: &mut [Complex],
        src: &[f64],
    ) -> Vec<f64> {
        inp[..src.len()].copy_from_slice(src);
        inp[src.len()..].fill(0.0);
        if plan.process(inp, out).is_err() {
            return Vec::new();
        }
        out.iter().map(|c| c.norm_sqr()).collect()
    }

    /// Full analysis of a block. Returns the tone if the block is in this
    /// size's range and clean enough; the flag says whether a candidate peak
    /// in range existed.
    fn analyze(&mut self, x: &[f64], cfg: &SweepConfig) -> (bool, Option<Tone>) {
        let fs = cfg.sample_rate;
        let n = self.n;
        let e = self.prepare(x);
        if e <= 1e-30 {
            return (false, None);
        }
        // 2x zero padded spectrum
        let xw = std::mem::take(&mut self.xw);
        let mag = Self::spectrum(&self.full, &mut self.inp, &mut self.out, &xw);
        let df = fs / (2 * n) as f64;
        let Some(fc) = Self::peak(&mag, df, cfg.f_lo, cfg.f_hi) else {
            self.xw = xw;
            return (false, None);
        };
        if fc < self.range.0 || fc >= self.range.1 {
            self.xw = xw;
            return (false, None);
        }
        // chirp rate from the two halves (window applied to demeaned x)
        let h = n / 2;
        let mean = x.iter().sum::<f64>() / n as f64;
        let mut halves = [0.0f64; 2];
        let dfh = fs / n as f64;
        let mut ok = true;
        for (idx, hv) in halves.iter_mut().enumerate() {
            let seg: Vec<f64> = x[idx * h..(idx + 1) * h]
                .iter()
                .zip(&self.w_half)
                .map(|(&v, &w)| (v - mean) * w)
                .collect();
            let m = Self::spectrum(&self.half, &mut self.inp_h, &mut self.out_h, &seg);
            // search +-35 % around fc: the chirp moves only slightly
            match Self::peak(&m, dfh, fc * 0.65, fc * 1.35) {
                Some(f) => *hv = f,
                None => ok = false,
            }
        }
        let k = if ok {
            (halves[1] - halves[0]) / (h as f64 / fs)
        } else {
            0.0
        };
        // keep k inside what a few percent of drift per block allows
        let kmax = 0.5 * fc / (n as f64 / fs);
        let k = k.clamp(-kmax, kmax);
        let z = dtft_chirp(&xw, fs, fc, k);
        self.xw = xw;
        let amp = 2.0 * z.norm() / self.sum_w;
        let tone_e = 0.5 * amp * amp * self.sum_w2;
        let frac = (tone_e / e).clamp(0.0, 1.0);
        let snr = if frac >= 1.0 - 1e-12 {
            300.0
        } else {
            10.0 * (frac / (1.0 - frac)).log10()
        };
        if snr < cfg.min_snr_db || 20.0 * amp.max(1e-300).log10() < cfg.min_level_dbfs {
            return (true, None);
        }
        (true, Some(Tone { f: fc, k, amp }))
    }
}

struct SizeState {
    an: Analyzer,
    hop: usize,
    buf: [Vec<f64>; 2],
}

/// The tracker. See the module documentation.
pub struct SweepTracker {
    cfg: SweepConfig,
    channels: usize,
    sizes: Vec<SizeState>,
    cells: [Vec<Cell>; 2],
    pair_diff: Vec<f32>,
    pair_re: f64,
    pair_im: f64,
    pair_n: u64,
    stats: TrackerStats,
}

impl SweepTracker {
    /// `channels` is 1 or 2. `None` for an unusable configuration.
    pub fn new(cfg: SweepConfig, channels: usize) -> Option<Self> {
        if !(cfg.sample_rate > 1000.0)
            || !(cfg.f_lo > 0.0)
            || !(cfg.f_hi > cfg.f_lo * 2.0)
            || cfg.f_hi >= 0.5 * cfg.sample_rate
            || cfg.points_per_octave == 0
            || !(cfg.cycles_lo >= 4.0)
            || !(1..=2).contains(&channels)
        {
            return None;
        }
        // block lengths: the longest holds cycles_lo cycles of f_lo
        let mut sizes = Vec::new();
        let mut lo = cfg.f_lo;
        loop {
            let t = cfg.cycles_lo / lo;
            let n = ((t * cfg.sample_rate / 8.0).round() as usize * 8).max(MIN_BLOCK);
            let next_lo = 4.0 * lo;
            let next_n = cfg.cycles_lo / next_lo * cfg.sample_rate;
            let last = next_n < MIN_BLOCK as f64 || next_lo >= cfg.f_hi;
            let hi = if last { f64::INFINITY } else { next_lo };
            sizes.push(SizeState {
                hop: n / 8,
                buf: [Vec::with_capacity(2 * n), Vec::with_capacity(2 * n)],
                an: Analyzer::new(n, (lo, hi)),
            });
            if last {
                break;
            }
            lo = next_lo;
        }
        let ncell = Self::cell_index(&cfg, cfg.f_hi) + 1;
        Some(Self {
            cfg,
            channels,
            sizes,
            cells: [vec![Cell::default(); ncell], vec![Cell::default(); ncell]],
            pair_diff: Vec::new(),
            pair_re: 0.0,
            pair_im: 0.0,
            pair_n: 0,
            stats: TrackerStats::default(),
        })
    }

    fn cell_index(cfg: &SweepConfig, f: f64) -> usize {
        let i = (cfg.points_per_octave as f64 * (f / cfg.f_lo).log2()).round();
        if i < 0.0 {
            0
        } else {
            i as usize
        }
    }

    pub fn config(&self) -> &SweepConfig {
        &self.cfg
    }

    /// Number of grid cells.
    pub fn cell_count(&self) -> usize {
        self.cells[0].len()
    }

    /// Centre frequency of grid cell `i`.
    pub fn grid_hz(&self, i: usize) -> f64 {
        self.cfg.f_lo * (i as f64 / self.cfg.points_per_octave as f64).exp2()
    }

    /// Forget everything collected so far.
    pub fn reset(&mut self) {
        for s in &mut self.sizes {
            s.buf[0].clear();
            s.buf[1].clear();
        }
        for c in self.cells.iter_mut().flatten() {
            c.seen = 0;
            c.v.clear();
        }
        self.pair_diff.clear();
        self.pair_re = 0.0;
        self.pair_im = 0.0;
        self.pair_n = 0;
        self.stats = TrackerStats::default();
    }

    pub fn stats(&self) -> TrackerStats {
        self.stats
    }

    /// Feed samples. For a two-channel tracker `r` must be `Some` and as long
    /// as `l`; a one-channel tracker ignores `r`.
    pub fn push<S: Sample>(&mut self, l: &[S], r: Option<&[S]>) {
        let n = match (self.channels, r) {
            (2, Some(r)) => l.len().min(r.len()),
            (2, None) => return,
            _ => l.len(),
        };
        let mut pos = 0;
        while pos < n {
            // feed in slices no larger than the smallest hop-aware step
            let step = (n - pos).min(256);
            for si in 0..self.sizes.len() {
                let sz = &mut self.sizes[si];
                for i in pos..pos + step {
                    sz.buf[0].push(l[i].into());
                }
                if self.channels == 2 {
                    if let Some(r) = r {
                        for i in pos..pos + step {
                            sz.buf[1].push(r[i].into());
                        }
                    }
                }
                while self.sizes[si].buf[0].len() >= self.sizes[si].an.n {
                    self.process_block(si);
                }
            }
            pos += step;
        }
    }

    fn process_block(&mut self, si: usize) {
        let cfg = self.cfg;
        let chans = self.channels;
        let mut tones: [Option<Tone>; 2] = [None, None];
        {
            let sz = &mut self.sizes[si];
            let n = sz.an.n;
            for ch in 0..chans {
                let (cand, tone) = sz.an.analyze(&sz.buf[ch][..n], &cfg);
                if cand {
                    self.stats.candidates[ch] += 1;
                }
                tones[ch] = tone;
            }
        }
        for ch in 0..chans {
            if let Some(t) = &tones[ch] {
                self.stats.accepted[ch] += 1;
                let i = Self::cell_index(&cfg, t.f);
                if i < self.cells[ch].len() {
                    self.cells[ch][i].add(t.f, 20.0 * t.amp.log10());
                }
            }
        }
        if let (Some(a), Some(b)) = (&tones[0], &tones[1]) {
            // phase of L relative to R at the stronger channel's (f, k)
            let (f, k) = if a.amp >= b.amp {
                (a.f, a.k)
            } else {
                (b.f, b.k)
            };
            let sz = &mut self.sizes[si];
            let n = sz.an.n;
            let mut zs = [Complex::new(0.0, 0.0); 2];
            for (ch, zc) in zs.iter_mut().enumerate() {
                sz.an.prepare(&sz.buf[ch][..n]);
                *zc = dtft_chirp(&sz.an.xw, cfg.sample_rate, f, k);
            }
            let ph = zs[0] * zs[1].conj();
            if ph.norm() > 0.0 {
                self.pair_re += ph.re / ph.norm();
                self.pair_im += ph.im / ph.norm();
                self.pair_n += 1;
                self.pair_diff.push((20.0 * (a.amp / b.amp).log10()) as f32);
                if self.pair_diff.len() > 65_536 {
                    self.pair_diff.remove(0);
                }
            }
        }
        let sz = &mut self.sizes[si];
        let hop = sz.hop;
        for b in sz.buf.iter_mut() {
            if b.len() >= hop {
                b.drain(..hop);
            }
        }
    }

    /// The covered cell `i` of channel `ch`.
    pub fn cell(&self, ch: usize, i: usize) -> Option<SweepCell> {
        let c = self.cells.get(ch)?.get(i)?;
        if (c.seen as usize) < self.cfg.min_count.max(1) || c.v.is_empty() {
            return None;
        }
        let mut f: Vec<f64> = c.v.iter().map(|p| f64::from(p.0)).collect();
        let mut l: Vec<f64> = c.v.iter().map(|p| f64::from(p.1)).collect();
        Some(SweepCell {
            index: i,
            freq_hz: median(&mut f),
            level_db: median(&mut l),
            count: c.seen as usize,
        })
    }

    /// Covered cells of channel `ch`, ascending frequency.
    pub fn curve(&self, ch: usize) -> Vec<SweepCell> {
        (0..self.cell_count())
            .filter_map(|i| self.cell(ch, i))
            .collect()
    }

    /// Coverage map of channel `ch` (one flag per grid cell).
    pub fn coverage(&self, ch: usize) -> Vec<bool> {
        (0..self.cell_count())
            .map(|i| self.cell(ch, i).is_some())
            .collect()
    }

    /// Lowest and highest covered frequency of channel `ch`.
    pub fn covered_range(&self, ch: usize) -> Option<(f64, f64)> {
        let c = self.curve(ch);
        Some((c.first()?.freq_hz, c.last()?.freq_hz))
    }

    /// Inter-channel statistics (two-channel trackers only).
    pub fn pair(&self) -> Option<PairStats> {
        if self.pair_n == 0 {
            return None;
        }
        let mut d: Vec<f64> = self.pair_diff.iter().map(|&v| f64::from(v)).collect();
        let n = self.pair_n as f64;
        Some(PairStats {
            level_diff_db: median(&mut d),
            phase_deg: self.pair_im.atan2(self.pair_re).to_degrees(),
            phase_coherence: (self.pair_re.hypot(self.pair_im)) / n,
            blocks: self.pair_n as usize,
        })
    }
}

// ---------------------------------------------------------------------------
// Nominal characteristics and deviation curves

/// How the recorded groove velocity varies with frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Characteristic {
    /// Constant velocity.
    ConstantVelocity,
    /// Recorded with the RIAA pre-emphasis (flat after an RIAA playback).
    FlatAfterRiaa,
    /// Constant amplitude (velocity proportional to `f`) below the turnover
    /// frequency (Hz), constant velocity above.
    TurnoverAmplitudeBelow(f64),
}

/// Expected playback level in dB (relative; the RIAA part is 0 dB at 1 kHz,
/// the velocity part is 0 dB at the turnover / constant) for a tone at `f`.
/// `preamp_riaa` selects an RIAA de-emphasis playback preamp (with the IEC
/// roll-off if `iec`) instead of a flat one.
pub fn expected_db(c: Characteristic, preamp_riaa: bool, iec: bool, f: f64) -> f64 {
    let opts = Options {
        iec,
        ultrasonic: false,
    };
    let de = || analog_response(Mode::DeEmphasis, opts, f).db();
    match (c, preamp_riaa) {
        (Characteristic::ConstantVelocity, true) => de(),
        (Characteristic::ConstantVelocity, false) => 0.0,
        (Characteristic::FlatAfterRiaa, true) => 0.0,
        (Characteristic::FlatAfterRiaa, false) => analog_response(Mode::PreEmphasis, opts, f).db(),
        (Characteristic::TurnoverAmplitudeBelow(ft), pre) => {
            let v = if ft > 0.0 && f < ft {
                20.0 * (f / ft).log10()
            } else {
                0.0
            };
            v + if pre { de() } else { 0.0 }
        }
    }
}

/// Log-frequency linear interpolation of `(f, db)` points (ascending `f`) at
/// `f`, only if `f` lies between two points not further apart than
/// `max_gap_oct` (or exactly on one).
pub fn interp_db_log(points: &[(f64, f64)], f: f64, max_gap_oct: f64) -> Option<f64> {
    let i = points.partition_point(|p| p.0 < f);
    if let Some(p) = points.get(i) {
        if (p.0 - f).abs() < 1e-9 {
            return Some(p.1);
        }
    }
    if i == 0 || i >= points.len() {
        return None;
    }
    let (a, b) = (points[i - 1], points[i]);
    let span = (b.0 / a.0).log2();
    if span > max_gap_oct || span <= 0.0 {
        return None;
    }
    let t = (f / a.0).log2() / span;
    Some(a.1 + t * (b.1 - a.1))
}

/// `level - expected` for every point, with `norm_offset` (the same quantity
/// at the normalisation frequency) subtracted.
pub fn deviation_points(
    points: &[(f64, f64)],
    c: Characteristic,
    preamp_riaa: bool,
    iec: bool,
    norm_offset: f64,
) -> Vec<(f64, f64)> {
    points
        .iter()
        .map(|&(f, l)| (f, l - expected_db(c, preamp_riaa, iec, f) - norm_offset))
        .collect()
}

/// `level - expected` of the curve at `f_norm` (interpolated within one
/// octave gaps): the offset that makes the deviation 0 dB there.
pub fn norm_offset_at(
    points: &[(f64, f64)],
    c: Characteristic,
    preamp_riaa: bool,
    iec: bool,
    f_norm: f64,
) -> Option<f64> {
    interp_db_log(points, f_norm, 1.0).map(|l| l - expected_db(c, preamp_riaa, iec, f_norm))
}

/// Fractional-octave smoothing of `(f, db)` points (ascending): each output is
/// the mean of the input points within `+-1/(2 fraction)` octave of it.
/// `fraction == 0` returns the input.
pub fn smooth_points_octave(points: &[(f64, f64)], fraction: u32) -> Vec<(f64, f64)> {
    if fraction == 0 {
        return points.to_vec();
    }
    let half = 0.5 / f64::from(fraction);
    points
        .iter()
        .map(|&(f, _)| {
            let (mut s, mut n) = (0.0, 0usize);
            for &(g, v) in points {
                if (g / f).log2().abs() <= half + 1e-12 {
                    s += v;
                    n += 1;
                }
            }
            (f, s / n.max(1) as f64)
        })
        .collect()
}

/// Maximum, minimum and RMS of a deviation curve over a frequency range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviationStats {
    pub max_db: f64,
    pub min_db: f64,
    pub rms_db: f64,
    pub points: usize,
}

/// Statistics of the points with `f_lo <= f <= f_hi` (`None` if there are
/// none).
pub fn deviation_stats(points: &[(f64, f64)], f_lo: f64, f_hi: f64) -> Option<DeviationStats> {
    let sel: Vec<f64> = points
        .iter()
        .filter(|p| p.0 >= f_lo && p.0 <= f_hi && p.1.is_finite())
        .map(|p| p.1)
        .collect();
    if sel.is_empty() {
        return None;
    }
    Some(DeviationStats {
        max_db: sel.iter().cloned().fold(f64::MIN, f64::max),
        min_db: sel.iter().cloned().fold(f64::MAX, f64::min),
        rms_db: (sel.iter().map(|v| v * v).sum::<f64>() / sel.len() as f64).sqrt(),
        points: sel.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Window;

    const FS: f64 = 48_000.0;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    /// Known EQ (dB) used by the tests: a smooth, non-trivial curve.
    fn eq_db(f: f64) -> f64 {
        let o = (f / 1000.0).log2();
        -1.5 * o + 2.0 * (-(o * o) / 8.0).exp() * (3.0 * o).sin() + 1.0
    }

    /// Log sweep f0 -> f1 over `dur` seconds, amplitude `a0 * gain(f_inst)`.
    fn sweep(f0: f64, f1: f64, dur: f64, gain: &dyn Fn(f64) -> f64) -> Vec<f64> {
        let n = (dur * FS) as usize;
        let r = (f1 / f0).ln() / dur;
        let mut ph = 0.0;
        (0..n)
            .map(|i| {
                let t = i as f64 / FS;
                let f = f0 * (r * t).exp();
                ph += 2.0 * PI * f / FS;
                0.5 * gain(f) * ph.sin()
            })
            .collect()
    }

    fn add_noise(x: &mut [f64], snr_db: f64, seed: u64) {
        let mut rng = Rng(seed);
        // white noise relative to a 0.5 amplitude unity-gain sine
        let sigma = (0.5f64 / 2f64.sqrt()) * 10f64.powf(-snr_db / 20.0);
        for v in x {
            *v += sigma * 3f64.sqrt() * rng.next();
        }
    }

    fn check(tr: &SweepTracker, lo: f64, sweep_lo: f64, hi: f64, tol: f64) -> usize {
        let mut covered = 0;
        for i in 0..tr.cell_count() {
            let fc = tr.grid_hz(i);
            let cell = tr.cell(0, i);
            if fc > lo * 1.05 && fc < hi / 1.05 {
                let c = cell.unwrap_or_else(|| panic!("cell {i} ({fc:.0} Hz) not covered"));
                let want = 20.0 * 0.5f64.log10() + eq_db(c.freq_hz);
                assert!(
                    (c.level_db - want).abs() < tol,
                    "{:.1} Hz: {:.3} vs {:.3}",
                    c.freq_hz,
                    c.level_db,
                    want
                );
                covered += 1;
            } else if fc < sweep_lo / 1.1 || fc > hi * 1.1 {
                assert!(cell.is_none(), "cell {i} ({fc:.0} Hz) wrongly covered");
            }
        }
        covered
    }

    fn run(x: &[f64], cfg: SweepConfig) -> SweepTracker {
        let mut tr = SweepTracker::new(cfg, 1).unwrap();
        for ch in x.chunks(4000) {
            tr.push(ch, None);
        }
        tr
    }

    #[test]
    fn gliding_sweeps_up_and_down_with_noise() {
        // 800 Hz..20 kHz, 30 s, and 20 Hz..20 kHz, 45 s; up and down; 50 dB SNR
        for (f0, f1, dur, cov_lo) in [(800.0, 20_000.0, 30.0, 800.0), (20.0, 20_000.0, 45.0, 30.0)]
        {
            for up in [true, false] {
                let (a, b) = if up { (f0, f1) } else { (f1, f0) };
                let mut x = sweep(a, b, dur, &|f| 10f64.powf(eq_db(f) / 20.0));
                add_noise(&mut x, 50.0, 7);
                let mut cfg = SweepConfig::new(FS);
                cfg.f_lo = 20.0;
                let tr = run(&x, cfg);
                let n = check(&tr, cov_lo, f0, f1, 0.2);
                assert!(n > 60, "{f0}..{f1} up={up}: only {n} cells");
            }
        }
    }

    #[test]
    fn stepped_tones_gaps_and_bursts() {
        // spot tones 800 Hz.. 16 kHz (1/3 octave steps, 0.6 s each), with
        // silence, white noise bursts and voice-like harmonic bursts between
        let mut rng = Rng(99);
        let mut x: Vec<f64> = Vec::new();
        let mut freqs = Vec::new();
        let mut f = 800.0;
        let mut seg = 0;
        while f < 16_500.0 {
            let n = (0.6 * FS) as usize;
            let g = 10f64.powf(eq_db(f) / 20.0);
            let start = x.len();
            x.extend((0..n).map(|i| 0.5 * g * (2.0 * PI * f * i as f64 / FS).sin()));
            let _ = start;
            freqs.push(f);
            // separator
            let m = (0.3 * FS) as usize;
            match seg % 3 {
                0 => x.resize(x.len() + m, 0.0),
                1 => x.extend((0..m).map(|_| 0.3 * rng.next())),
                _ => {
                    // voice-like: 130 Hz + 12 harmonics with formant shape + vibrato
                    let mut ph = 0.0;
                    x.extend((0..m).map(|i| {
                        let t = i as f64 / FS;
                        let f0 = 130.0 * (1.0 + 0.05 * (2.0 * PI * 5.0 * t).sin());
                        ph += 2.0 * PI * f0 / FS;
                        (1..=12)
                            .map(|h| (h as f64 * ph).sin() / (1.0 + (h as f64 - 4.0).powi(2) / 4.0))
                            .sum::<f64>()
                            * 0.15
                    }));
                }
            }
            seg += 1;
            f *= 2f64.powf(1.0 / 3.0);
        }
        add_noise(&mut x, 50.0, 3);
        let tr = run(&x, SweepConfig::new(FS));
        let c = tr.curve(0);
        // exactly one covered cell per tone (cells are 1/24 octave wide and the
        // tones are 8 cells apart), no cells from the separators
        assert_eq!(
            c.len(),
            freqs.len(),
            "{:?}",
            c.iter().map(|c| c.freq_hz).collect::<Vec<_>>()
        );
        for (cell, f) in c.iter().zip(&freqs) {
            assert!(
                (cell.freq_hz / f - 1.0).abs() < 1e-3,
                "{} vs {f}",
                cell.freq_hz
            );
            let want = 20.0 * 0.5f64.log10() + eq_db(*f);
            assert!(
                (cell.level_db - want).abs() < 0.05,
                "{f}: {} vs {want}",
                cell.level_db
            );
        }
        let cov = tr.coverage(0);
        assert_eq!(cov.iter().filter(|&&b| b).count(), freqs.len());
    }

    #[test]
    fn constant_velocity_through_riaa_reads_flat_after_nominal() {
        // constant-velocity sweep (constant amplitude) through the ideal
        // de-emphasis: the deviation from the nominal characteristic is flat
        let gain = |f: f64| analog_response(Mode::DeEmphasis, Options::default(), f).norm();
        let mut x = sweep(800.0, 20_000.0, 30.0, &gain);
        add_noise(&mut x, 50.0, 11);
        let tr = run(&x, SweepConfig::new(FS));
        let pts: Vec<(f64, f64)> = tr
            .curve(0)
            .iter()
            .map(|c| (c.freq_hz, c.level_db))
            .collect();
        let off =
            norm_offset_at(&pts, Characteristic::ConstantVelocity, true, false, 1000.0).unwrap();
        let dev = deviation_points(&pts, Characteristic::ConstantVelocity, true, false, off);
        let s = deviation_stats(&dev, 0.0, 1e9).unwrap();
        assert!(s.max_db.abs() < 0.05 && s.min_db.abs() < 0.05, "{s:?}");
        // without the nominal conversion the RIAA curve is plainly visible
        let raw = deviation_stats(&pts, 0.0, 1e9).unwrap();
        assert!(raw.max_db - raw.min_db > 15.0);
    }

    #[test]
    fn stereo_pair_statistics_and_crosstalk() {
        // L carries the sweep, R a -28 dB copy in phase: L only
        let x = sweep(800.0, 8000.0, 10.0, &|_| 1.0);
        let l: Vec<f64> = x.clone();
        let r: Vec<f64> = x.iter().map(|v| v * 10f64.powf(-28.0 / 20.0)).collect();
        let mut tr = SweepTracker::new(SweepConfig::new(FS), 2).unwrap();
        for (a, b) in l.chunks(4000).zip(r.chunks(4000)) {
            tr.push(a, Some(b));
        }
        let p = tr.pair().unwrap();
        assert!((p.level_diff_db - 28.0).abs() < 0.1, "{p:?}");
        assert!(p.phase_deg.abs() < 2.0, "{p:?}");
        // equal and in anti-phase: vertical
        let r2: Vec<f64> = x.iter().map(|v| -v).collect();
        let mut tr = SweepTracker::new(SweepConfig::new(FS), 2).unwrap();
        for (a, b) in l.chunks(4000).zip(r2.chunks(4000)) {
            tr.push(a, Some(b));
        }
        let p = tr.pair().unwrap();
        assert!(
            p.level_diff_db.abs() < 0.05 && p.phase_deg.abs() > 175.0,
            "{p:?}"
        );
    }

    #[test]
    fn rejects_noise_silence_and_two_tones() {
        let mut rng = Rng(5);
        let noise: Vec<f64> = (0..48_000 * 3).map(|_| 0.3 * rng.next()).collect();
        let tr = run(&noise, SweepConfig::new(FS));
        assert!(tr.curve(0).is_empty());
        let tr = run(&vec![0.0; 48_000], SweepConfig::new(FS));
        assert!(tr.curve(0).is_empty());
        // two equal tones are not "one clear sinusoid"
        let two: Vec<f64> = (0..48_000 * 2)
            .map(|i| {
                let t = i as f64 / FS;
                0.3 * (2.0 * PI * 1000.0 * t).sin() + 0.3 * (2.0 * PI * 1700.0 * t).sin()
            })
            .collect();
        let tr = run(&two, SweepConfig::new(FS));
        assert!(tr.curve(0).is_empty());
        let _ = Window::Hann;
    }

    #[test]
    fn nominal_characteristics() {
        // constant velocity + RIAA preamp = de-emphasis: +/-: 20 kHz -19.6 dB
        let d = expected_db(Characteristic::ConstantVelocity, true, false, 20_000.0);
        assert!((d + 19.62).abs() < 0.05, "{d}");
        assert_eq!(
            expected_db(Characteristic::ConstantVelocity, false, false, 5e3),
            0.0
        );
        assert!(expected_db(Characteristic::FlatAfterRiaa, true, false, 5e3).abs() < 1e-12);
        let p = expected_db(Characteristic::FlatAfterRiaa, false, false, 20_000.0);
        assert!((p - 19.62).abs() < 0.05, "{p}");
        // turnover 500 Hz: -6.02 dB at 250 Hz from the velocity part
        let t = Characteristic::TurnoverAmplitudeBelow(500.0);
        let a = expected_db(t, false, false, 250.0);
        assert!((a + 6.0206).abs() < 1e-3);
        assert_eq!(expected_db(t, false, false, 800.0), 0.0);
    }

    #[test]
    fn helpers_interp_smooth_stats() {
        let pts = [(1000.0, 0.0), (2000.0, 6.0), (8000.0, 12.0)];
        assert!((interp_db_log(&pts, 1_414.213_562, 1.0).unwrap() - 3.0).abs() < 1e-6);
        assert_eq!(interp_db_log(&pts, 2000.0, 1.0), Some(6.0));
        assert!(interp_db_log(&pts, 4000.0, 1.0).is_none()); // 2 octave gap
        assert!(interp_db_log(&pts, 500.0, 1.0).is_none());
        let sm = smooth_points_octave(&[(1000.0, 0.0), (1010.0, 2.0), (4000.0, 5.0)], 3);
        assert!((sm[0].1 - 1.0).abs() < 1e-12 && (sm[2].1 - 5.0).abs() < 1e-12);
        let s = deviation_stats(&[(1.0, -1.0), (2.0, 3.0), (3.0, 2.0)], 1.5, 5.0).unwrap();
        assert_eq!((s.max_db, s.min_db, s.points), (3.0, 2.0, 2));
        assert!(deviation_stats(&[(1.0, 1.0)], 2.0, 3.0).is_none());
    }
}
