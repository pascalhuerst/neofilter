#![allow(dead_code)]
//! Helpers shared by the audio path tests.

use neofilter::presets::factory_presets;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;
use std::f64::consts::PI;

pub fn preset(id: &str) -> [BandParams; NUM_BANDS] {
    factory_presets()
        .iter()
        .find(|p| p.id == id)
        .expect("preset exists")
        .band_params()
}

pub fn log_freqs(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| lo * (hi / lo).powf(i as f64 / (n - 1) as f64))
        .collect()
}

/// Magnitude in dB of the DFT of `x` at `freq` (direct evaluation).
pub fn dft_db(x: &[f32], freq: f64, sr: f64) -> f64 {
    let w = 2.0 * PI * freq / sr;
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let (s, c) = (w * n as f64).sin_cos();
        re += f64::from(v) * c;
        im -= f64::from(v) * s;
    }
    10.0 * (re * re + im * im).log10()
}

pub struct Noise(pub u64);
impl Noise {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0) as f32
    }
    pub fn vec(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.next()).collect()
    }
}
