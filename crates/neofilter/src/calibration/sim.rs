//! Synthetic loop for tests (and for generating test recordings).
//!
//! `return = delay(D, fractional) o gain o system_EQ(sent post-EQ) + noise`
//!
//! [`LoopSim`] is the loop itself: the "system" is an arbitrary band array
//! run through the real [`EqEngine`] (so it has exactly the digital
//! responses NeoFilter's bands have), followed by an optional polarity
//! inversion/gain, a fractional delay (windowed sinc, accurate to a few
//! thousandths of a dB below 0.4 fs) and additive white noise.
//!
//! [`Rig`] wires the real pieces together exactly like the plugin does per
//! block: `tap (noise injection + capture) -> AudioPath (EQ) -> LoopSim ->`
//! return of a later block. Use [`Rig::run_session`] to drive a [`Session`]
//! end to end (measure, fit, apply, settle, repeat).

use super::session::{Env, Session};
use super::settings::CalPersist;
use super::tap::{CalLink, CalTap};
use crate::audio::{AudioParams, AudioPath};
use neo_measure::generator::Rng;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, EqEngine};
use std::f64::consts::PI;
use std::sync::Arc;

const KERNEL_HALF: usize = 32;
const RING: usize = 1 << 20;

/// The playback loop.
pub struct LoopSim {
    fs: f64,
    delay_int: usize,
    taps: Vec<f64>,
    eq: EqEngine,
    gain: f64,
    noise_rms: f64,
    rng: Rng,
    ring: Vec<[f64; 2]>,
    written: u64,
    read: u64,
}

impl LoopSim {
    /// `delay_s` may be fractional in samples. `system` is the loop's own EQ
    /// (every `used` band is applied). `noise_dbfs` is white Gaussian noise
    /// added to the return (`None`: noiseless).
    pub fn new(
        fs: f64,
        delay_s: f64,
        system: &[BandParams; NUM_BANDS],
        invert: bool,
        gain_db: f64,
        noise_dbfs: Option<f64>,
        seed: u64,
    ) -> Self {
        let d = delay_s * fs;
        // position p = t - D = (t - floor(D) - 1) + (1 - frac): interpolate at
        // integer offset `delay_int` and fraction `mu` (towards the past).
        let delay_int = d.floor() as usize;
        let frac = d - d.floor();
        // y[t] = x(t - D) = sum_k x[t - delay_int - k] * h[k], h[k] = sinc(k - frac)
        let taps: Vec<f64> = (-(KERNEL_HALF as isize) + 1..=KERNEL_HALF as isize)
            .map(|k| {
                let x = k as f64 - frac;
                let s = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                };
                let w = 0.42
                    + 0.5 * (PI * x / (KERNEL_HALF as f64 + 1.0)).cos()
                    + 0.08 * (2.0 * PI * x / (KERNEL_HALF as f64 + 1.0)).cos();
                s * w
            })
            .collect();
        let norm: f64 = taps.iter().sum();
        let taps = taps.iter().map(|t| t / norm).collect();
        let mut eq = EqEngine::new(fs);
        eq.set_gain_scale(1.0);
        for (i, b) in system.iter().enumerate() {
            eq.set_band(i, b);
        }
        Self {
            fs,
            delay_int,
            taps,
            eq,
            gain: if invert { -1.0 } else { 1.0 } * 10f64.powf(gain_db / 20.0),
            noise_rms: noise_dbfs.map_or(0.0, |d| 10f64.powf(d / 20.0)),
            rng: Rng::new(seed),
            ring: vec![[0.0; 2]; RING],
            written: 0,
            read: 0,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.fs
    }

    /// Feed the block that left NeoFilter (post-EQ).
    pub fn push(&mut self, l: &[f32], r: &[f32]) {
        let n = l.len().min(r.len());
        let mut a: Vec<f64> = l[..n].iter().map(|&v| f64::from(v)).collect();
        let mut b: Vec<f64> = r[..n].iter().map(|&v| f64::from(v)).collect();
        self.eq.process_stereo(&mut a, &mut b);
        for i in 0..n {
            self.ring[(self.written as usize) & (RING - 1)] = [a[i] * self.gain, b[i] * self.gain];
            self.written += 1;
        }
    }

    /// The next `n` return samples. Must be called before pushing the block
    /// that covers the same time span; needs `delay >= n + 32` samples.
    pub fn pull(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len().min(out_r.len());
        assert!(
            self.delay_int > n + KERNEL_HALF,
            "loop delay must exceed the block size"
        );
        let lo = -(KERNEL_HALF as i64) + 1;
        for i in 0..n {
            let t = (self.read + i as u64) as i64;
            let (mut a, mut b) = (0.0, 0.0);
            for (j, h) in self.taps.iter().enumerate() {
                let k = lo + j as i64;
                let idx = t - self.delay_int as i64 - k;
                if idx >= 0 {
                    let s = self.ring[(idx as usize) & (RING - 1)];
                    a += s[0] * h;
                    b += s[1] * h;
                }
            }
            if self.noise_rms > 0.0 {
                a += self.noise_rms * self.rng.gaussian();
                b += self.noise_rms * self.rng.gaussian();
            }
            out_l[i] = a as f32;
            out_r[i] = b as f32;
        }
        self.read += n as u64;
    }
}

/// Plugin audio path + calibration tap + simulated loop, block by block.
pub struct Rig {
    pub fs: f64,
    pub link: Arc<CalLink>,
    pub tap: CalTap,
    pub audio: AudioPath,
    pub sim: LoopSim,
    /// NeoFilter's band parameters (what the host parameters would hold).
    pub bands: [BandParams; NUM_BANDS],
    pub gain_scale: f64,
    pub bypass: bool,
    pub block: usize,
    /// Peak amplitude of a 220 Hz programme sine fed to the input (0 = silence).
    pub programme: f64,
    pub frames: u64,
    ret_l: Vec<f32>,
    ret_r: Vec<f32>,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl Rig {
    pub fn new(fs: f64, sim: LoopSim, bands: [BandParams; NUM_BANDS], block: usize) -> Self {
        let link = Arc::new(CalLink::new());
        Self {
            fs,
            tap: CalTap::new(link.clone(), fs),
            link,
            audio: AudioPath::new(fs, block),
            sim,
            bands,
            gain_scale: 1.0,
            bypass: false,
            block,
            programme: 0.0,
            frames: 0,
            ret_l: vec![0.0; block],
            ret_r: vec![0.0; block],
            l: vec![0.0; block],
            r: vec![0.0; block],
        }
    }

    /// Process one block of `n <= block` frames.
    pub fn step(&mut self, n: usize) {
        for i in 0..n {
            let x = if self.programme > 0.0 {
                self.programme
                    * (2.0 * PI * 220.0 * (self.frames + i as u64) as f64 / self.fs).sin()
            } else {
                0.0
            };
            self.l[i] = x as f32;
            self.r[i] = x as f32;
        }
        self.sim.pull(&mut self.ret_l[..n], &mut self.ret_r[..n]);
        self.tap.process_stereo(
            &mut self.l[..n],
            &mut self.r[..n],
            &self.ret_l[..n],
            &self.ret_r[..n],
        );
        let p = AudioParams {
            bands: &self.bands,
            gain_scale: self.gain_scale,
            out_gain_db: 0.0,
            bypass: self.bypass,
        };
        self.audio
            .process_stereo(&mut self.l[..n], &mut self.r[..n], &p);
        self.sim.push(&self.l[..n], &self.r[..n]);
        self.frames += n as u64;
    }

    pub fn run(&mut self, frames: usize) {
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.block);
            self.step(n);
            left -= n;
        }
    }

    pub fn env(&self) -> Env<'_> {
        Env {
            bands: &self.bands,
            gain_scale: self.gain_scale,
            bypass: self.bypass,
        }
    }

    /// Drive `session` until it is idle again or `max_s` seconds of audio
    /// passed. Bands that the session applies are written to `self.bands`.
    /// Returns the number of applies.
    pub fn run_session(
        &mut self,
        session: &mut Session,
        persist: &mut CalPersist,
        max_s: f64,
    ) -> usize {
        let limit = self.frames + (max_s * self.fs) as u64;
        let mut applies = 0;
        while self.frames < limit {
            self.step(self.block);
            let env = Env {
                bands: &self.bands,
                gain_scale: self.gain_scale,
                bypass: self.bypass,
            };
            if let Some(b) = session.poll(persist, &env) {
                self.bands = b;
                applies += 1;
            }
            if !session.busy() {
                break;
            }
        }
        applies
    }
}
