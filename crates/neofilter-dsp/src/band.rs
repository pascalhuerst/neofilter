//! Band filter: analog design -> matched coefficients -> per-channel state.

use crate::analog::{design_frequency, effective_gain_db, shape_uses_gain, AnalogBand};
use crate::biquad::{BiquadCoeffs, BiquadState};
use crate::matched::match_band;
use crate::types::{BandParams, MAX_SECTIONS};

/// Gains below this magnitude (dB) are treated as exactly flat.
pub const FLAT_GAIN_DB: f64 = 1e-9;

/// Digital coefficients of one band (no state). Allocation-free.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandDesign {
    coeffs: [BiquadCoeffs; MAX_SECTIONS],
    len: usize,
}

impl Default for BandDesign {
    fn default() -> Self {
        Self::identity()
    }
}

impl BandDesign {
    pub fn identity() -> Self {
        Self {
            coeffs: [BiquadCoeffs::IDENTITY; MAX_SECTIONS],
            len: 0,
        }
    }

    /// Design from user parameters (target values, no smoothing). Ignores the
    /// `used`/`enabled` flags. A gain shape with |gain| < `FLAT_GAIN_DB` is the
    /// identity (zero sections, zero cost).
    pub fn design(params: &BandParams, gain_scale: f64, sample_rate: f64) -> Self {
        let g = effective_gain_db(params.shape, params.gain_db, gain_scale);
        if shape_uses_gain(params.shape) && !(g.abs() >= FLAT_GAIN_DB) && g.is_finite() {
            return Self::identity();
        }
        let analog = AnalogBand::from_params(params, gain_scale, sample_rate);
        Self::from_analog(&analog, sample_rate)
    }

    /// Matched digital design of an existing analog prototype.
    pub fn from_analog(analog: &AnalogBand, sample_rate: f64) -> Self {
        let mut d = Self::identity();
        d.len = match_band(analog, sample_rate, &mut d.coeffs);
        d
    }

    pub fn sections(&self) -> &[BiquadCoeffs] {
        &self.coeffs[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Squared magnitude at `omega` rad/sample.
    pub fn mag_sq(&self, omega: f64) -> f64 {
        self.sections().iter().map(|c| c.mag_sq(omega)).product()
    }
}

/// Frequency actually designed for (after clamping), re-exported for callers.
pub fn clamped_frequency(params: &BandParams, sample_rate: f64) -> f64 {
    design_frequency(params.freq_hz, sample_rate)
}

/// A band filter: coefficients plus TDF-II state for two channels.
#[derive(Debug, Clone, Copy)]
pub struct BandFilter {
    design: BandDesign,
    state: [[BiquadState; MAX_SECTIONS]; 2],
}

impl Default for BandFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl BandFilter {
    pub fn new() -> Self {
        Self {
            design: BandDesign::identity(),
            state: [[BiquadState::ZERO; MAX_SECTIONS]; 2],
        }
    }

    /// Redesign coefficients while the filter is running (parameters moving
    /// smoothly): the state of every section is recomputed from its sample
    /// history with the new coefficients (see [`BiquadState::refresh`]), unless
    /// the band became the identity, which clears it.
    /// Allocation-free; safe on the audio thread.
    pub fn redesign(&mut self, params: &BandParams, gain_scale: f64, sample_rate: f64) {
        self.design = BandDesign::design(params, gain_scale, sample_rate);
        if self.design.is_empty() {
            self.reset();
        } else {
            for ch in &mut self.state {
                for (c, s) in self.design.coeffs[..self.design.len]
                    .iter()
                    .zip(ch.iter_mut())
                {
                    s.refresh(c);
                }
            }
        }
    }

    pub fn design(&self) -> &BandDesign {
        &self.design
    }

    pub fn is_identity(&self) -> bool {
        self.design.is_empty()
    }

    pub fn reset(&mut self) {
        self.state = [[BiquadState::ZERO; MAX_SECTIONS]; 2];
    }

    /// Process a block of channel `ch` (0 or 1) in place.
    pub fn process(&mut self, ch: usize, buf: &mut [f64]) {
        let st = &mut self.state[ch & 1];
        for (c, s) in self.design.coeffs[..self.design.len]
            .iter()
            .zip(st.iter_mut())
        {
            s.process(c, buf);
            s.sanitize();
        }
    }
}
