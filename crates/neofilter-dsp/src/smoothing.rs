//! Parameter smoothing and crossfade timing.
//!
//! Frequency (log domain), gain (dB) and Q (log domain) glide towards their
//! targets with a one-pole filter evaluated once per coefficient update
//! (`UPDATE_INTERVAL` samples). Topology changes use a short linear
//! (equal-gain) crossfade between two coherent filter outputs.

/// Samples between coefficient updates while parameters are moving.
pub const UPDATE_INTERVAL: usize = 32;
/// Smoothing time constant in seconds.
pub const SMOOTH_TIME_S: f64 = 0.020;
/// Crossfade duration in seconds.
pub const FADE_TIME_S: f64 = 0.010;

/// One-pole coefficient applied once per update interval.
pub fn update_coeff(sample_rate: f64) -> f64 {
    1.0 - (-(UPDATE_INTERVAL as f64) / (SMOOTH_TIME_S * sample_rate)).exp()
}

/// Crossfade length in samples (at least one update interval).
pub fn fade_len(sample_rate: f64) -> usize {
    ((FADE_TIME_S * sample_rate).round() as usize).max(UPDATE_INTERVAL)
}

/// A value gliding towards a target, snapping when close enough.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smoother {
    current: f64,
    target: f64,
    threshold: f64,
}

impl Smoother {
    pub fn new(value: f64, threshold: f64) -> Self {
        Self {
            current: value,
            target: value,
            threshold,
        }
    }

    pub fn current(&self) -> f64 {
        self.current
    }

    pub fn target(&self) -> f64 {
        self.target
    }

    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    /// Jump to the target.
    pub fn snap(&mut self) {
        self.current = self.target;
    }

    pub fn snap_to(&mut self, value: f64) {
        self.current = value;
        self.target = value;
    }

    pub fn is_settled(&self) -> bool {
        self.current == self.target
    }

    /// One update step with coefficient `coeff` (see [`update_coeff`]).
    /// Returns `true` if the value changed.
    pub fn step(&mut self, coeff: f64) -> bool {
        if self.current == self.target {
            return false;
        }
        let d = self.target - self.current;
        if d.abs() <= self.threshold {
            self.current = self.target;
        } else {
            self.current += d * coeff;
        }
        true
    }
}

/// Smoothed parameters of one band: ln(frequency in Hz), gain in dB, ln(Q).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSmoother {
    pub ln_freq: Smoother,
    pub gain_db: Smoother,
    pub ln_q: Smoother,
}

impl BandSmoother {
    pub fn new(freq_hz: f64, gain_db: f64, q: f64) -> Self {
        Self {
            // 1e-5 in ln units = 0.001 % (far below anything audible)
            ln_freq: Smoother::new(freq_hz.ln(), 1e-5),
            gain_db: Smoother::new(gain_db, 1e-5),
            ln_q: Smoother::new(q.ln(), 1e-5),
        }
    }

    pub fn set_targets(&mut self, freq_hz: f64, gain_db: f64, q: f64) {
        self.ln_freq.set_target(freq_hz.ln());
        self.gain_db.set_target(gain_db);
        self.ln_q.set_target(q.ln());
    }

    pub fn snap(&mut self) {
        self.ln_freq.snap();
        self.gain_db.snap();
        self.ln_q.snap();
    }

    pub fn is_settled(&self) -> bool {
        self.ln_freq.is_settled() && self.gain_db.is_settled() && self.ln_q.is_settled()
    }

    /// Advance one update; returns `true` if anything moved.
    pub fn step(&mut self, coeff: f64) -> bool {
        let a = self.ln_freq.step(coeff);
        let b = self.gain_db.step(coeff);
        let c = self.ln_q.step(coeff);
        a || b || c
    }

    /// Current `(freq_hz, gain_db, q)`.
    pub fn values(&self) -> (f64, f64, f64) {
        (
            self.ln_freq.current().exp(),
            self.gain_db.current(),
            self.ln_q.current().exp(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn smoother_converges_with_time_constant() {
        let fs = 48000.0;
        let c = update_coeff(fs);
        let mut s = Smoother::new(0.0, 1e-9);
        s.set_target(1.0);
        let steps = (SMOOTH_TIME_S * fs / UPDATE_INTERVAL as f64).round() as usize;
        for _ in 0..steps {
            s.step(c);
        }
        // after one time constant: 1 - 1/e
        assert_abs_diff_eq!(s.current(), 1.0 - (-1.0f64).exp(), epsilon = 5e-3);
        for _ in 0..10_000 {
            s.step(c);
        }
        assert!(s.is_settled());
        assert_eq!(s.current(), 1.0);
    }

    #[test]
    fn band_smoother_round_trip() {
        let mut b = BandSmoother::new(1000.0, 3.0, 2.0);
        let (f, g, q) = b.values();
        assert_abs_diff_eq!(f, 1000.0, epsilon = 1e-9);
        assert_eq!((g, q), (3.0, 2.0f64.ln().exp()));
        b.set_targets(2000.0, -3.0, 0.5);
        assert!(!b.is_settled());
        b.snap();
        let (f, g, q) = b.values();
        assert_abs_diff_eq!(f, 2000.0, epsilon = 1e-9);
        assert_abs_diff_eq!(g, -3.0, epsilon = 1e-12);
        assert_abs_diff_eq!(q, 0.5, epsilon = 1e-12);
        assert!(!b.step(0.1));
    }

    #[test]
    fn fade_len_scales() {
        assert_eq!(fade_len(48000.0), 480);
        assert_eq!(fade_len(1000.0), UPDATE_INTERVAL);
    }
}
