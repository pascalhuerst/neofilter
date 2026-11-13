//! Shared helpers: cached designs and deterministic test signals.
#![allow(dead_code)]

use neoriaa::audio::{AudioParams, AudioPath, Meters};
use neoriaa::dsp::RiaaFilters;
use std::sync::{Arc, OnceLock};

pub const RATES: [f64; 5] = [44_100.0, 48_000.0, 96_000.0, 192_000.0, 384_000.0];

/// One design per rate for the whole test binary (designing is slow).
pub fn design(fs: f64) -> Arc<RiaaFilters> {
    static CACHE: [OnceLock<Arc<RiaaFilters>>; 5] = [const { OnceLock::new() }; 5];
    let i = RATES.iter().position(|&r| r == fs).expect("cached rate");
    CACHE[i]
        .get_or_init(|| Arc::new(RiaaFilters::design(fs)))
        .clone()
}

pub fn path(fs: f64) -> (AudioPath, Arc<Meters>) {
    let m = Arc::new(Meters::default());
    (AudioPath::new(&design(fs), m.clone()), m)
}

pub struct Noise(pub u64);

impl Noise {
    /// Uniform in [-1, 1).
    pub fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
    }
    pub fn vec(&mut self, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|_| self.next() * amp).collect()
    }
}

pub fn sine(n: usize, fs: f64, hz: f64, amp: f64) -> Vec<f32> {
    (0..n)
        .map(|i| (amp * (2.0 * std::f64::consts::PI * hz * i as f64 / fs).sin()) as f32)
        .collect()
}

pub fn params() -> AudioParams {
    AudioParams::default()
}
