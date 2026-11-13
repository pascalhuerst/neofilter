#![allow(dead_code)]
//! Shared helpers for the integration tests.

use neofilter_dsp::{BandParams, Placement, Shape, Slope};
use std::f64::consts::PI;

pub fn band(shape: Shape, freq_hz: f64, gain_db: f64, q: f64, slope: Slope) -> BandParams {
    BandParams {
        used: true,
        enabled: true,
        shape,
        freq_hz,
        gain_db,
        q,
        slope,
        placement: Placement::Stereo,
    }
}

pub fn bell(freq_hz: f64, gain_db: f64, q: f64) -> BandParams {
    band(Shape::Bell, freq_hz, gain_db, q, Slope::Db12)
}

pub fn slope_from_db(db: u32) -> Slope {
    *Slope::ALL.iter().find(|s| s.db_per_oct() == db).unwrap()
}

/// Small deterministic xorshift noise in [-1, 1).
pub struct Noise(pub u64);
impl Noise {
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
    pub fn fill(&mut self, buf: &mut [f64]) {
        for x in buf {
            *x = self.next();
        }
    }
}

pub fn sine(n: usize, freq: f64, fs: f64, amp: f64) -> Vec<f64> {
    (0..n)
        .map(|i| amp * (2.0 * PI * freq * i as f64 / fs).sin())
        .collect()
}

/// Largest absolute sample-to-sample difference in `x[range]`.
pub fn max_jump(x: &[f64], from: usize, to: usize) -> f64 {
    (from.max(1)..to.min(x.len()))
        .map(|i| (x[i] - x[i - 1]).abs())
        .fold(0.0, f64::max)
}

/// Log-spaced frequency grid.
pub fn grid(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| lo * (hi / lo).powf(i as f64 / (n as f64 - 1.0)))
        .collect()
}
