//! Analysis of a finished capture: delay, transfer function, coherence.
//!
//! Off-thread only (allocates, a few hundred ms of FFT work).
//!
//! The capture holds, per frame, the pre-EQ reference `x` (mono) and the two
//! sidechain return channels `y`. Per usable return channel:
//!
//! 1. levels: peak (clipping) and RMS of the last quarter (return too low),
//! 2. delay by cross-correlation of 4 s of reference (starting 0.5 s in)
//!    against the return, lags `0..=max_delay`, with sub-sample refinement,
//!    polarity and confidence,
//! 3. transfer function `H = Sxy/Sxx` (Hann, 75 % overlap, FFT size scaled
//!    with the sample rate, 16384 at 48 kHz), delay compensated, using the
//!    return from `delay + 0.5 s` on,
//! 4. smoothing (1/N octave power average) and resampling to a log grid with
//!    [`POINTS_PER_OCTAVE`] points per octave, 20 Hz upwards.
//!
//! `H` is the whole loop *including the EQ the noise passed through*.

use super::settings::CalSettings;
use neo_measure::delay::{estimate_delay, DelayOptions};
use neo_measure::spectrum::{interp_linear, log_frequencies, smooth_octave_power};
use neo_measure::transfer::{TransferConfig, TransferEstimator};
use std::fmt;

/// Internal log grid density.
pub const POINTS_PER_OCTAVE: usize = 48;
/// Seconds of capture skipped at the start (fade-in, EQ settling), in
/// addition to the loop delay.
pub const SKIP_S: f64 = 0.5;

/// Captured streams (same length).
#[derive(Debug, Clone, Default)]
pub struct Capture {
    pub reference: Vec<f32>,
    pub ret: [Vec<f32>; 2],
}

impl Capture {
    pub fn len(&self) -> usize {
        self.reference.len()
    }
    pub fn is_empty(&self) -> bool {
        self.reference.is_empty()
    }
    pub fn reserve(&mut self, total: usize) {
        let extra = total.saturating_sub(self.reference.len());
        self.reference.reserve(extra);
        self.ret[0].reserve(extra);
        self.ret[1].reserve(extra);
    }
    pub fn push(&mut self, f: [f32; 3]) {
        self.reference.push(f[0]);
        self.ret[0].push(f[1]);
        self.ret[1].push(f[2]);
    }
}

/// Why a measurement was rejected. Nothing may be applied after any of these.
#[derive(Debug, Clone, PartialEq)]
pub enum CalError {
    TooShort,
    /// Return level (dBFS RMS) below the threshold on every channel.
    ReturnTooLow(f64),
    /// Return peak (dBFS) at or above the clip threshold.
    ReturnClipped(f64),
    /// No correlation between reference and return (best confidence given).
    NoLoop(f64),
    /// Mean in-range coherence too low (stylus lifted, noise, wrong routing).
    LowCoherence(f64),
    /// The capture lost frames (editor closed or stalled).
    CaptureOverrun,
    /// The return signal vanished during the measurement.
    SignalLost,
    Stopped,
    Other(String),
}

impl fmt::Display for CalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "capture too short"),
            Self::ReturnTooLow(db) => write!(f, "return level too low ({db:.1} dBFS)"),
            Self::ReturnClipped(db) => write!(f, "return is clipping (peak {db:.2} dBFS)"),
            Self::NoLoop(c) => write!(f, "no loop detected (delay confidence {c:.1})"),
            Self::LowCoherence(c) => {
                write!(f, "coherence too low ({c:.2}): stylus lifted or no signal?")
            }
            Self::CaptureOverrun => write!(f, "capture overrun (keep the editor open)"),
            Self::SignalLost => write!(f, "return signal lost during the measurement"),
            Self::Stopped => write!(f, "stopped"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Per return channel figures.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelInfo {
    pub delay_samples: f64,
    pub delay_ms: f64,
    pub inverted: bool,
    pub confidence: f64,
    pub level_rms_db: f64,
    pub peak_db: f64,
    pub mean_coherence: f64,
}

#[derive(Debug, Clone)]
pub struct ChannelData {
    pub info: ChannelInfo,
    /// Smoothed magnitude on the grid, dB, not normalised.
    pub mag_db: Vec<f64>,
    pub coherence: Vec<f64>,
}

/// Result of [`analyze_capture`].
#[derive(Debug, Clone)]
pub struct Measurement {
    pub sample_rate: f64,
    pub freqs: Vec<f64>,
    pub channels: [Option<ChannelData>; 2],
    /// Power average of the usable channels, dB, not normalised.
    pub mag_db: Vec<f64>,
    /// Mean coherence of the usable channels.
    pub coherence: Vec<f64>,
    /// Mean coherence in the fit range.
    pub mean_coherence: f64,
    /// Largest return RMS over channels, dBFS.
    pub return_rms_db: f64,
    pub return_peak_db: f64,
}

impl Measurement {
    pub fn delay_ms(&self) -> [Option<f64>; 2] {
        [0, 1].map(|c| self.channels[c].as_ref().map(|d| d.info.delay_ms))
    }
    /// Largest delay of the usable channels, in samples.
    pub fn max_delay_samples(&self) -> f64 {
        self.channels
            .iter()
            .flatten()
            .map(|c| c.info.delay_samples)
            .fold(0.0, f64::max)
    }
}

/// FFT size for the transfer estimate: 16384 at 44.1/48 kHz, scaled with the rate.
pub fn fft_size_for(sample_rate: f64) -> usize {
    let k = (sample_rate / 48_000.0).max(0.25);
    ((16_384.0 * k).round() as usize)
        .next_power_of_two()
        .clamp(4096, 131_072)
}

/// Log grid used for measurements at `sample_rate`.
pub fn grid_for(sample_rate: f64) -> Vec<f64> {
    let hi = 20_000f64.min(0.45 * sample_rate);
    let n = ((hi / 20.0).log2() * POINTS_PER_OCTAVE as f64).ceil() as usize + 1;
    log_frequencies(20.0, hi, n)
}

fn db(x: f64) -> f64 {
    if x > 1e-20 {
        20.0 * x.log10()
    } else {
        -400.0
    }
}

/// (rms dBFS, peak dBFS) of a slice.
pub fn level_stats(x: &[f32]) -> (f64, f64) {
    if x.is_empty() {
        return (-400.0, -400.0);
    }
    let (mut s, mut p) = (0.0f64, 0.0f64);
    for &v in x {
        let v = f64::from(v);
        s += v * v;
        p = p.max(v.abs());
    }
    (db((s / x.len() as f64).sqrt()), db(p))
}

fn to_f64(x: &[f32]) -> Vec<f64> {
    x.iter().map(|&v| f64::from(v)).collect()
}

/// Delay estimate of `ret` relative to `reference` using a 4 s reference
/// window starting `SKIP_S` in. `max_lag` in samples.
pub fn estimate_loop_delay(
    reference: &[f32],
    ret: &[f32],
    sample_rate: f64,
    max_lag: usize,
) -> Option<neo_measure::delay::DelayEstimate> {
    estimate_loop_delay_window(reference, ret, sample_rate, max_lag, 4.0)
}

/// Like [`estimate_loop_delay`] with a reference window of `window_s`
/// seconds (the live session probes with 1.5 s while the capture runs).
pub fn estimate_loop_delay_window(
    reference: &[f32],
    ret: &[f32],
    sample_rate: f64,
    max_lag: usize,
    window_s: f64,
) -> Option<neo_measure::delay::DelayEstimate> {
    let n = reference.len().min(ret.len());
    let x0 = ((SKIP_S * sample_rate) as usize).min(n / 4);
    let nx = ((window_s * sample_rate) as usize).min((n - x0) / 3).max(2);
    if n < x0 + nx + 2 {
        return None;
    }
    let x = to_f64(&reference[x0..x0 + nx]);
    let y = to_f64(&ret[x0..n]);
    let max_lag = max_lag.min(y.len().saturating_sub(nx + 1));
    let mut o = DelayOptions::new(max_lag);
    // PHAT whitening: pink noise correlates over a long skirt, whitening gives a
    // sharp peak and a much better peak-to-sidelobe ratio.
    o.phat = true;
    estimate_delay(&x, &y, &o)
}

/// Analyse a capture. Applies the safety checks of the plan: low level,
/// clipping, no loop, low coherence. On error nothing may be fitted.
pub fn analyze_capture(
    cap: &Capture,
    sample_rate: f64,
    s: &CalSettings,
) -> Result<Measurement, CalError> {
    let n = cap
        .reference
        .len()
        .min(cap.ret[0].len())
        .min(cap.ret[1].len());
    let fft = fft_size_for(sample_rate);
    let skip = (SKIP_S * sample_rate) as usize;
    if n < skip + 6 * fft {
        return Err(CalError::TooShort);
    }
    let tail = &|c: usize| &cap.ret[c][n - n / 4..n];
    // levels
    let mut stats = [(-400.0, -400.0); 2];
    for (c, st) in stats.iter_mut().enumerate() {
        let (rms, _) = level_stats(tail(c));
        let (_, peak) = level_stats(&cap.ret[c][..n]);
        *st = (rms, peak);
    }
    let alive: Vec<usize> = (0..2).filter(|&c| stats[c].0 >= s.low_level_db).collect();
    let best_rms = stats[0].0.max(stats[1].0);
    if alive.is_empty() {
        return Err(CalError::ReturnTooLow(best_rms));
    }
    let max_peak = alive.iter().map(|&c| stats[c].1).fold(-400.0, f64::max);
    if max_peak >= s.clip_db {
        return Err(CalError::ReturnClipped(max_peak));
    }

    // delay per alive channel
    let max_lag = (s.max_delay_s * sample_rate) as usize;
    let mut usable = Vec::new();
    let mut best_conf = 0.0f64;
    for &c in &alive {
        if let Some(e) =
            estimate_loop_delay(&cap.reference[..n], &cap.ret[c][..n], sample_rate, max_lag)
        {
            best_conf = best_conf.max(e.confidence.min(1e6));
            if e.confidence >= s.delay_confidence_min {
                usable.push((c, e));
            }
        }
    }
    if usable.is_empty() {
        return Err(CalError::NoLoop(best_conf));
    }

    // transfer function per usable channel
    let freqs = grid_for(sample_rate);
    let x_all = to_f64(&cap.reference[skip..n]);
    let mut channels: [Option<ChannelData>; 2] = [None, None];
    for (c, e) in usable {
        let mut cfg = TransferConfig::new(sample_rate, fft);
        cfg.overlap = 0.75;
        cfg.delay_samples = e.delay_samples;
        let y_all = to_f64(&cap.ret[c][skip..n]);
        let mut est = TransferEstimator::new(cfg)
            .ok_or_else(|| CalError::Other("invalid transfer configuration".into()))?;
        est.push(&x_all, &y_all);
        let r = est.result().ok_or(CalError::TooShort)?;
        if r.frames < 8 {
            return Err(CalError::TooShort);
        }
        let mag_bins = if s.smoothing > 0 {
            r.smoothed_magnitude_db(s.smoothing)
        } else {
            r.magnitude_db()
        };
        let coh_bins = if s.smoothing > 0 {
            smooth_octave_power(&r.coherence, s.smoothing)
        } else {
            r.coherence.clone()
        };
        let resample =
            |v: &[f64]| -> Vec<f64> { freqs.iter().map(|&f| interp_linear(v, f / r.df)).collect() };
        let mag = resample(&mag_bins);
        let coh = resample(&coh_bins);
        let mean_coh = mean_in_range(&freqs, &coh, s.range_lo_hz, s.range_hi_hz);
        channels[c] = Some(ChannelData {
            info: ChannelInfo {
                delay_samples: e.delay_samples,
                delay_ms: 1000.0 * e.delay_samples / sample_rate,
                inverted: e.inverted,
                confidence: e.confidence,
                level_rms_db: stats[c].0,
                peak_db: stats[c].1,
                mean_coherence: mean_coh,
            },
            mag_db: mag,
            coherence: coh,
        });
    }

    // combine
    let used: Vec<&ChannelData> = channels.iter().flatten().collect();
    let m = freqs.len();
    let mut mag = vec![0.0; m];
    let mut coh = vec![0.0; m];
    for i in 0..m {
        let p: f64 = used
            .iter()
            .map(|d| 10f64.powf(d.mag_db[i] / 10.0))
            .sum::<f64>()
            / used.len() as f64;
        mag[i] = 10.0 * p.max(1e-40).log10();
        coh[i] = used.iter().map(|d| d.coherence[i]).sum::<f64>() / used.len() as f64;
    }
    let mean_coherence = mean_in_range(&freqs, &coh, s.range_lo_hz, s.range_hi_hz);
    if mean_coherence < s.coherence_abort {
        return Err(CalError::LowCoherence(mean_coherence));
    }
    Ok(Measurement {
        sample_rate,
        freqs,
        channels,
        mag_db: mag,
        coherence: coh,
        mean_coherence,
        return_rms_db: best_rms,
        return_peak_db: max_peak,
    })
}

/// Mean of `v` over grid points inside `lo..=hi`.
pub fn mean_in_range(freqs: &[f64], v: &[f64], lo: f64, hi: f64) -> f64 {
    let mut s = 0.0;
    let mut n = 0usize;
    for (&f, &x) in freqs.iter().zip(v) {
        if f >= lo && f <= hi {
            s += x;
            n += 1;
        }
    }
    if n > 0 {
        s / n as f64
    } else {
        0.0
    }
}
