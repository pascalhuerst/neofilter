//! Cutter-head metering: levels after RIAA pre-emphasis, the high-frequency
//! band load, and recorded velocity / displacement estimates.
//!
//! Realtime: everything here is allocation-free after construction and runs
//! on the audio thread.
//!
//! # Why on the audio thread
//!
//! The pre-emphasis boosts the top octave by up to ~20 dB, so peaks after
//! pre-emphasis are what a cutter head (or a limiter in front of it) sees.
//! Peaks must not miss a single sample, which a lossy hand-off to an analysis
//! thread cannot guarantee; the filter is a handful of biquads per channel and
//! cheap. It is designed at `initialize()` for the host sample rate with
//! [`neo_measure::riaa::RiaaFilters`] (pre-emphasis, no IEC roll-off, no
//! ultrasonic pole), normalised to 0 dB at 1 kHz.
//!
//! # Signals
//!
//! Four signals are metered, all after pre-emphasis: `L`, `R`, **lateral**
//! `(L + R) / 2` and **vertical** `(L - R) / 2` (the same M/S convention as
//! `neo_measure::stereo`).
//!
//! # Calibration (estimates!)
//!
//! `reference_level` (dBFS) is the pre-emphasised signal level that
//! corresponds to a recorded peak lateral velocity of 5 cm/s at 1 kHz:
//!
//! ```text
//! v [cm/s]  = 5 * |x_pre| / 10^(reference_level / 20)
//! d [um]    = 5 * |D|     / 10^(reference_level / 20) * 1e4
//! ```
//!
//! where `x_pre` is the pre-emphasised signal and `D = integral(x_pre dt)` is
//! its leaky integral (see [`LeakyIntegrator`]). Velocity is assumed
//! proportional to the pre-emphasised signal and displacement is the integral
//! of velocity; these are estimates that are only as good as the calibration
//! value. The numbers are per signal: a mono signal at the reference level
//! reads 5 cm/s on L, R and lateral and 0 on vertical.

use neo_measure::filter::{Biquad, BiquadCoeffs};
use neo_measure::levels::{PeakHold, PeakMeter, RmsMeter, TruePeakMeter};
use neo_measure::riaa::{Cascade, CascadeState};
use neo_measure::util::amp_to_db;
use std::f64::consts::PI;

/// Recorded peak lateral velocity (cm/s) at the reference level.
pub const REF_VELOCITY_CM_S: f64 = 5.0;
/// Default calibration: dBFS that corresponds to 5 cm/s.
pub const DEFAULT_REFERENCE_DBFS: f64 = -18.0;
/// Low cut of the displacement integrator.
pub const INTEGRATOR_CORNER_HZ: f64 = 10.0;
/// Frequency at which the integrator's bilinear warping is pinned (exact there).
pub const INTEGRATOR_MATCH_HZ: f64 = 1000.0;

/// Signals metered after pre-emphasis.
pub const SIGNALS: [&str; 4] = ["L", "R", "Lateral", "Vertical"];
pub const L: usize = 0;
pub const R: usize = 1;
pub const LATERAL: usize = 2;
pub const VERTICAL: usize = 3;

/// Velocity in cm/s of a pre-emphasised amplitude given in dBFS.
pub fn velocity_cm_s(level_db: f64, reference_dbfs: f64) -> f64 {
    if level_db <= -399.0 {
        return 0.0;
    }
    REF_VELOCITY_CM_S * 10f64.powf((level_db - reference_dbfs) / 20.0)
}

/// Displacement in micrometres from a linear peak of the leaky integral `D`
/// (amplitude * seconds).
pub fn displacement_um(integral_peak: f64, reference_dbfs: f64) -> f64 {
    REF_VELOCITY_CM_S * integral_peak / 10f64.powf(reference_dbfs / 20.0) * 1e4
}

/// Leaky integrator `H(s) = 1 / (s + wc)` (an ideal integrator with a low cut
/// at `corner_hz`), bilinear-transformed with the warping pinned at
/// [`INTEGRATOR_MATCH_HZ`]. For `f >> corner` the gain is `1 / (2 pi f)`
/// (exact at 1 kHz; the 10 Hz corner costs 0.0005 % there, but −3 dB at
/// 10 Hz and −1 dB at 20 Hz). Above ~fs/20 the bilinear warp makes it read low
/// (about −1 % at 0.05 fs); displacement there is tiny anyway.
#[derive(Debug, Clone, Copy)]
pub struct LeakyIntegrator {
    b: f64,
    a1: f64,
    x1: f64,
    y1: f64,
}

impl LeakyIntegrator {
    pub fn new(fs: f64, corner_hz: f64) -> Self {
        let wc = 2.0 * PI * corner_hz;
        let wm = 2.0 * PI * INTEGRATOR_MATCH_HZ.min(0.2 * fs);
        let k = wm / (wm / (2.0 * fs)).tan();
        // H(z) = (1 + z^-1) / (k (1 - z^-1) + wc (1 + z^-1))
        let d = k + wc;
        Self {
            b: 1.0 / d,
            a1: (wc - k) / d,
            x1: 0.0,
            y1: 0.0,
        }
    }
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b * (x + self.x1) - self.a1 * self.y1;
        self.x1 = x;
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        y
    }
    /// Gain at `f` Hz.
    pub fn gain(&self, f: f64, fs: f64) -> f64 {
        let w = 2.0 * PI * f / fs;
        let (re, im) = (self.b * (1.0 + w.cos()), -self.b * w.sin());
        let (dr, di) = (1.0 + self.a1 * w.cos(), -self.a1 * w.sin());
        ((re * re + im * im) / (dr * dr + di * di)).sqrt()
    }
}

/// Readings of one metered signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignalReadout {
    /// Held sample peak after pre-emphasis, dBFS.
    pub peak_db: f64,
    pub peak_max_db: f64,
    /// Held true peak, dBTP.
    pub tp_db: f64,
    pub tp_max_db: f64,
    /// RMS (300 ms), sine-referenced dB.
    pub rms_db: f64,
    /// RMS of the band above the HF corner (300 ms), sine-referenced dB.
    pub hf_rms_db: f64,
    /// Held peak of the HF band, dBFS.
    pub hf_peak_db: f64,
    pub hf_peak_max_db: f64,
    /// Held peak of the leaky integral `D` (amplitude * s), linear.
    pub disp_now: f64,
    pub disp_max: f64,
}

impl Default for SignalReadout {
    fn default() -> Self {
        let f = neo_measure::util::DB_FLOOR;
        Self {
            peak_db: f,
            peak_max_db: f,
            tp_db: f,
            tp_max_db: f,
            rms_db: f,
            hf_rms_db: f,
            hf_peak_db: f,
            hf_peak_max_db: f,
            disp_now: 0.0,
            disp_max: 0.0,
        }
    }
}

const HOLD_S: f64 = 2.0;
const DECAY_DB_S: f64 = 20.0;
const RMS_S: f64 = 0.3;

struct Channel {
    peak: PeakMeter,
    tp: TruePeakMeter,
    rms: RmsMeter,
    hf: [Biquad; 2],
    hf_rms: RmsMeter,
    hf_peak: PeakMeter,
    integ: LeakyIntegrator,
    disp: PeakHold,
}

impl Channel {
    fn new(fs: f64, corner: f64) -> Self {
        let mut c = Self {
            peak: PeakMeter::new(fs, HOLD_S, DECAY_DB_S),
            tp: TruePeakMeter::with_hold(fs, HOLD_S, DECAY_DB_S),
            rms: RmsMeter::window(fs, RMS_S),
            hf: [Biquad::new(BiquadCoeffs::IDENTITY); 2],
            hf_rms: RmsMeter::window(fs, RMS_S),
            hf_peak: PeakMeter::new(fs, HOLD_S, DECAY_DB_S),
            integ: LeakyIntegrator::new(fs, INTEGRATOR_CORNER_HZ),
            disp: PeakHold::new(fs, HOLD_S, DECAY_DB_S),
        };
        c.set_corner(fs, corner);
        c
    }

    fn set_corner(&mut self, fs: f64, corner: f64) {
        // 4th-order Butterworth high-pass
        self.hf[0].set_coeffs(BiquadCoeffs::highpass(fs, corner, 0.541_196_100_146_197));
        self.hf[1].set_coeffs(BiquadCoeffs::highpass(fs, corner, 1.306_562_964_876_376_4));
    }

    fn reset(&mut self) {
        self.peak.reset();
        self.tp.reset_hold();
        self.rms.reset();
        self.hf_rms.reset();
        self.hf_peak.reset();
        self.disp.reset();
    }

    fn process(&mut self, x: &[f64], hf: &mut [f64]) {
        self.peak.process(x);
        self.tp.process(x);
        self.rms.process(x);
        for (o, &v) in hf.iter_mut().zip(x) {
            let a = self.hf[0].process(v);
            let y = self.hf[1].process(a);
            *o = y;
            let d = self.integ.process(v);
            self.disp.update(d.abs());
        }
        self.hf_rms.process(hf);
        self.hf_peak.process(hf);
    }

    fn readout(&self) -> SignalReadout {
        let sine = 10.0 * 2f64.log10();
        SignalReadout {
            peak_db: self.peak.peak_db(),
            peak_max_db: self.peak.max_db(),
            tp_db: amp_to_db(self.tp.true_peak()),
            tp_max_db: amp_to_db(self.tp.max_true_peak()),
            rms_db: self.rms.rms_db() + sine,
            hf_rms_db: self.hf_rms.rms_db() + sine,
            hf_peak_db: self.hf_peak.peak_db(),
            hf_peak_max_db: self.hf_peak.max_db(),
            disp_now: self.disp.level(),
            disp_max: self.disp.max(),
        }
    }
}

/// Pre-emphasis plus four metered signals.
pub struct Vinyl {
    fs: f64,
    cascade: Cascade,
    state: [CascadeState; 2],
    ch: [Channel; 4],
    corner: f64,
    pre: [Vec<f64>; 4],
    hf: Vec<f64>,
}

impl Vinyl {
    /// `max_chunk`: largest block handed to [`Vinyl::process`].
    pub fn new(fs: f64, cascade: Cascade, corner_hz: f64, max_chunk: usize) -> Self {
        Self {
            fs,
            cascade,
            state: [CascadeState::default(); 2],
            ch: std::array::from_fn(|_| Channel::new(fs, corner_hz)),
            corner: corner_hz,
            pre: std::array::from_fn(|_| vec![0.0; max_chunk]),
            hf: vec![0.0; max_chunk],
        }
    }

    pub fn reset(&mut self) {
        self.ch.iter_mut().for_each(Channel::reset);
    }

    /// Switch the pre-emphasis (e.g. IEC / ultrasonic variants). Restarts the
    /// filter state; allocation free.
    pub fn set_cascade(&mut self, cascade: Cascade) {
        self.cascade = cascade;
        self.state = [CascadeState::default(); 2];
    }

    pub fn set_corner(&mut self, corner_hz: f64) {
        if (corner_hz - self.corner).abs() > 0.5 {
            self.corner = corner_hz;
            for c in &mut self.ch {
                c.set_corner(self.fs, corner_hz);
            }
        }
    }

    /// Pre-emphasise and meter a block (`l.len() == r.len() <= max_chunk`).
    pub fn process(&mut self, l: &[f64], r: &[f64]) {
        let n = l.len().min(r.len()).min(self.hf.len());
        for i in 0..n {
            let a = self.state[0].process(&self.cascade, l[i]);
            let b = self.state[1].process(&self.cascade, r[i]);
            self.pre[L][i] = a;
            self.pre[R][i] = b;
            self.pre[LATERAL][i] = 0.5 * (a + b);
            self.pre[VERTICAL][i] = 0.5 * (a - b);
        }
        for k in 0..4 {
            self.ch[k].process(&self.pre[k][..n], &mut self.hf[..n]);
        }
    }

    pub fn readout(&self) -> [SignalReadout; 4] {
        std::array::from_fn(|k| self.ch[k].readout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrator_gain_matches_one_over_omega() {
        for fs in [44_100.0, 96_000.0, 192_000.0] {
            let it = LeakyIntegrator::new(fs, INTEGRATOR_CORNER_HZ);
            let g = it.gain(1000.0, fs);
            let want = 1.0 / (2.0 * PI * 1000.0);
            assert!((g / want - 1.0).abs() < 1e-3, "{fs}: {}", g / want);
            // -3 dB at the corner (analytic 1/(sqrt2 wc))
            let gc = it.gain(10.0, fs) * 2.0 * PI * 10.0;
            assert!((gc - 0.5f64.sqrt()).abs() < 0.01, "{gc}");
        }
    }

    #[test]
    fn calibration_formulas() {
        assert!((velocity_cm_s(-18.0, -18.0) - 5.0).abs() < 1e-12);
        assert!((velocity_cm_s(-12.0, -18.0) - 9.976).abs() < 0.001);
        assert_eq!(velocity_cm_s(-400.0, -18.0), 0.0);
        // v = 5 cm/s at 1 kHz -> d = v / (2 pi f) = 5/(6283) cm = 7.96 um
        let d = 1.0 / (2.0 * PI * 1000.0); // integral peak of a unit sine
        let um = displacement_um(d * 10f64.powf(-18.0 / 20.0), -18.0);
        assert!((um - 7.9577).abs() < 1e-3, "{um}");
    }
}
