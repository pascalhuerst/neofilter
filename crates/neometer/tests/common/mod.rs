#![allow(dead_code)]
//! Shared test rig: audio path + synchronous analysis engine, synthetic
//! signals.

use neometer::analysis::{Engine, RING_FRAMES};
use neometer::audio::{AudioParams, AudioPath};
use neometer::riaa::{Cascade, Mode, Options, RiaaFilters};
use neometer::ring;
use neometer::shared::Shared;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::{Arc, Mutex, OnceLock};

/// Pre-emphasis cascade per sample rate (the design is slow in debug builds,
/// so it is computed once per rate per test process).
pub fn pre_emphasis(fs: f64) -> Cascade {
    static CACHE: OnceLock<Mutex<HashMap<u64, Cascade>>> = OnceLock::new();
    let m = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap();
    *g.entry(fs.to_bits())
        .or_insert_with(|| *RiaaFilters::design(fs).cascade(Mode::PreEmphasis, Options::default()))
}

pub struct Rig {
    pub fs: f64,
    pub shared: Arc<Shared>,
    pub audio: AudioPath,
    pub engine: Engine,
    pub params: AudioParams,
    pub mono: bool,
}

impl Rig {
    pub fn new(fs: f64) -> Self {
        Self::with(fs, false)
    }

    pub fn with(fs: f64, mono: bool) -> Self {
        Self::build(fs, mono, pre_emphasis(fs))
    }

    /// Rig with the pre-emphasis designed for 48 kHz at any rate: for tests
    /// that do not look at vinyl readouts (the design is slow in debug builds).
    pub fn without_design(fs: f64) -> Self {
        Self::build(fs, false, pre_emphasis(48_000.0))
    }

    fn build(fs: f64, mono: bool, cascade: Cascade) -> Self {
        let shared = Shared::standalone();
        let (prod, cons) = ring::channel(RING_FRAMES);
        let mut audio = AudioPath::new(shared.clone());
        audio.prepare(fs, mono, cascade, prod);
        let engine = Engine::new(shared.clone(), cons, fs, if mono { 1 } else { 2 });
        Self {
            fs,
            shared,
            audio,
            engine,
            params: AudioParams::default(),
            mono,
        }
    }

    /// Run a stereo signal through the audio path in blocks of `block`, in
    /// place, draining the analysis engine after every block.
    pub fn run(&mut self, l: &mut [f32], r: &mut [f32], block: usize) {
        let n = l.len();
        let mut i = 0;
        while i < n {
            let e = (i + block).min(n);
            if self.mono {
                self.audio.process_mono(&mut l[i..e], &self.params);
            } else {
                self.audio
                    .process_stereo(&mut l[i..e], &mut r[i..e], &self.params);
            }
            while self.engine.step() > 0 {}
            i = e;
        }
        self.audio.publish();
        self.engine.publish();
    }

    /// Feed a signal (copies it, returns the processed output).
    pub fn feed(&mut self, l: &[f32], r: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut a, mut b) = (l.to_vec(), r.to_vec());
        self.run(&mut a, &mut b, block);
        (a, b)
    }
}

pub fn sine(fs: f64, f: f64, db: f64, n: usize) -> Vec<f32> {
    let a = 10f64.powf(db / 20.0);
    (0..n)
        .map(|i| (a * (2.0 * PI * f * i as f64 / fs).sin()) as f32)
        .collect()
}

pub struct Noise(pub u64);
impl Noise {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
    }
    pub fn vec(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.next()).collect()
    }
}

pub const RATES: [f64; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];
