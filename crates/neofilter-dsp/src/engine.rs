//! The 24-band EQ engine: the API the plugin codes against.

use crate::types::{BandParams, NUM_BANDS};

/// 24-band equaliser. Currently a pass-through; the DSP agent replaces the
/// internals but keeps these signatures.
#[derive(Debug, Clone)]
pub struct EqEngine {
    sample_rate: f64,
    gain_scale: f64,
    bands: [BandParams; NUM_BANDS],
}

impl EqEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            gain_scale: 1.0,
            bands: [BandParams::default(); NUM_BANDS],
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
    }

    /// Clear all filter state.
    pub fn reset(&mut self) {}

    /// Set target params for one band (smoothed internally). Cheap when
    /// unchanged. Realtime-safe. Out-of-range indices are ignored.
    pub fn set_band(&mut self, index: usize, params: &BandParams) {
        if let Some(b) = self.bands.get_mut(index) {
            *b = *params;
        }
    }

    /// 0.0..=2.0, scales dB gain of bell/shelf/tilt bands.
    pub fn set_gain_scale(&mut self, scale: f64) {
        self.gain_scale = scale;
    }

    /// Process a stereo block in place. Realtime-safe.
    pub fn process_stereo(&mut self, left: &mut [f64], right: &mut [f64]) {
        let _ = (left, right);
    }

    /// Process a mono block in place. Realtime-safe.
    pub fn process_mono(&mut self, samples: &mut [f64]) {
        let _ = samples;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough() {
        let mut e = EqEngine::new(48000.0);
        e.set_band(0, &BandParams::default());
        let mut l = [0.5, -0.25];
        let mut r = [0.1, 0.2];
        e.process_stereo(&mut l, &mut r);
        assert_eq!(l, [0.5, -0.25]);
        assert_eq!(r, [0.1, 0.2]);
    }
}
