//! Realtime part of the calibration: noise injection and capture.
//!
//! [`CalTap::process_stereo`] / [`CalTap::process_mono`] run on the audio
//! thread *before* the EQ. They never allocate or lock. While a measurement
//! runs they
//!
//! * replace (or add to) the input with noise, with a raised-cosine fade of
//!   [`FADE_S`] at start and stop,
//! * push time-aligned frames `[pre-EQ reference (mono), return L, return R]`
//!   into a lock-free ring ([`CalLink`]) for the analysis side.
//!
//! The control side ([`CalLink`]) is shared with the editor through atomics.
//! A measurement only starts when [`CalLink::start`] is called, ends by
//! itself after `max_frames` frames, on ring overrun, on [`CalLink::stop`],
//! and on [`CalTap::reset`] (plugin reset/deactivate). Nothing here is a
//! plugin parameter, so a host can neither automate nor restore it.

use super::settings::{InjectMode, NoiseKind, Routing, NOISE_LEVEL_MAX_DB, NOISE_LEVEL_MIN_DB};
use crossbeam::queue::ArrayQueue;
use neo_measure::generator::{Generator, PinkNoise, WhiteNoise};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::*};
use std::sync::Arc;

/// Capacity of the capture ring in frames (about 5.5 s at 48 kHz). A
/// measurement is aborted when more than half of it is undrained.
pub const RING_FRAMES: usize = 1 << 18;
/// Fade time of the noise at start and stop.
pub const FADE_S: f64 = 0.02;

/// One captured frame: reference, return L, return R.
pub type Frame = [f32; 3];

/// State shared between the audio thread and the control side.
pub struct CalLink {
    /// Bit 0: measurement requested; upper bits: request counter.
    cmd: AtomicU64,
    kind: AtomicU8,
    routing: AtomicU8,
    mode: AtomicU8,
    level_bits: AtomicU32,
    max_frames: AtomicU64,
    captured: AtomicU64,
    overrun: AtomicBool,
    capturing: AtomicBool,
    noise_active: AtomicBool,
    audio_frames: AtomicU64,
    sample_rate: AtomicU32,
    ring: ArrayQueue<Frame>,
}

impl Default for CalLink {
    fn default() -> Self {
        Self::new()
    }
}

impl CalLink {
    pub fn new() -> Self {
        Self {
            cmd: AtomicU64::new(0),
            kind: AtomicU8::new(0),
            routing: AtomicU8::new(2),
            mode: AtomicU8::new(0),
            level_bits: AtomicU32::new((-20.0f32).to_bits()),
            max_frames: AtomicU64::new(0),
            captured: AtomicU64::new(0),
            overrun: AtomicBool::new(false),
            capturing: AtomicBool::new(false),
            noise_active: AtomicBool::new(false),
            audio_frames: AtomicU64::new(0),
            sample_rate: AtomicU32::new(48_000.0f32.to_bits()),
            ring: ArrayQueue::new(RING_FRAMES),
        }
    }

    /// Start a measurement (explicit user action). `level_db` is capped at
    /// -6 dBFS. Stale frames are discarded first.
    pub fn start(
        &self,
        kind: NoiseKind,
        routing: Routing,
        mode: InjectMode,
        level_db: f64,
        max_frames: u64,
    ) {
        while self.ring.pop().is_some() {}
        let lvl = if level_db.is_finite() {
            level_db.clamp(NOISE_LEVEL_MIN_DB, NOISE_LEVEL_MAX_DB)
        } else {
            NOISE_LEVEL_MIN_DB
        };
        self.kind.store(kind as u8, Relaxed);
        self.routing.store(routing as u8, Relaxed);
        self.mode.store(mode as u8, Relaxed);
        self.level_bits.store((lvl as f32).to_bits(), Relaxed);
        self.max_frames.store(max_frames, Relaxed);
        self.captured.store(0, Relaxed);
        self.overrun.store(false, Relaxed);
        let mut c = self.cmd.load(Acquire);
        while let Err(now) =
            self.cmd
                .compare_exchange_weak(c, (((c >> 1) + 1) << 1) | 1, AcqRel, Acquire)
        {
            c = now;
        }
    }

    /// Stop the measurement (noise fades out).
    pub fn stop(&self) {
        self.cmd.fetch_and(!1, AcqRel);
    }

    /// Change the total frame count of the running measurement.
    pub fn set_max_frames(&self, frames: u64) {
        self.max_frames.store(frames, Release);
    }

    pub fn max_frames(&self) -> u64 {
        self.max_frames.load(Acquire)
    }

    /// A measurement is requested and not finished (capture running).
    pub fn is_running(&self) -> bool {
        self.cmd.load(Acquire) & 1 == 1 || self.capturing.load(Acquire)
    }

    /// Noise is currently audible at the output (including the fades).
    pub fn noise_active(&self) -> bool {
        self.noise_active.load(Relaxed)
    }

    /// Frames captured since the last [`start`](Self::start).
    pub fn captured_frames(&self) -> u64 {
        self.captured.load(Acquire)
    }

    /// True if the ring overflowed (capture lost frames; the measurement was stopped).
    pub fn overrun(&self) -> bool {
        self.overrun.load(Acquire)
    }

    /// Frames processed by the audio thread since creation (the audio clock).
    pub fn audio_frames(&self) -> u64 {
        self.audio_frames.load(Acquire)
    }

    pub fn sample_rate(&self) -> f64 {
        f64::from(f32::from_bits(self.sample_rate.load(Relaxed)))
    }

    /// Take the next captured frame (consumer side, single consumer).
    pub fn pop(&self) -> Option<Frame> {
        self.ring.pop()
    }

    /// True once the audio thread has finished the measurement (no more
    /// frames will be pushed).
    pub fn finished(&self) -> bool {
        self.cmd.load(Acquire) & 1 == 0 && !self.capturing.load(Acquire)
    }
}

/// Audio-thread half. Construct (slow: fits the pink filter) off the audio thread.
pub struct CalTap {
    link: Arc<CalLink>,
    pink: PinkNoise,
    white: WhiteNoise,
    kind: NoiseKind,
    routing: Routing,
    mode: InjectMode,
    gain: f64,
    capturing: bool,
    seen_cmd: u64,
    /// Fade position 0..1.
    pos: f64,
    step: f64,
    fading_in: bool,
}

const SEED: u64 = 0x5EED_CA11;

impl CalTap {
    /// Allocates and designs the noise filters. Not realtime-safe.
    pub fn new(link: Arc<CalLink>, sample_rate: f64) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 1000.0 {
            sample_rate
        } else {
            48_000.0
        };
        link.sample_rate.store((sr as f32).to_bits(), Relaxed);
        Self {
            link,
            pink: PinkNoise::new(sr, 0.0, SEED),
            white: WhiteNoise::new(0.0, SEED),
            kind: NoiseKind::Pink,
            routing: Routing::Both,
            mode: InjectMode::Replace,
            gain: 0.0,
            capturing: false,
            seen_cmd: 0,
            pos: 0.0,
            step: 1.0 / (FADE_S * sr).max(1.0),
            fading_in: false,
        }
    }

    pub fn link(&self) -> &Arc<CalLink> {
        &self.link
    }

    /// New sample rate (rebuilds the pink filter; not realtime-safe) and a hard stop.
    pub fn prepare(&mut self, sample_rate: f64) {
        let link = self.link.clone();
        *self = Self::new(link, sample_rate);
        self.reset();
    }

    /// Hard stop: noise silent immediately, any request cancelled. Safe on the audio thread.
    pub fn reset(&mut self) {
        self.link.stop();
        self.capturing = false;
        self.pos = 0.0;
        self.fading_in = false;
        self.link.capturing.store(false, Release);
        self.link.noise_active.store(false, Relaxed);
        self.seen_cmd = self.link.cmd.load(Acquire);
    }

    /// Whether noise is (still) being emitted.
    pub fn noise_active(&self) -> bool {
        self.fading_in || self.pos > 0.0
    }

    fn sync(&mut self) {
        let cmd = self.link.cmd.load(Acquire);
        let on = cmd & 1 == 1;
        if on && (!self.capturing || cmd != self.seen_cmd) {
            self.seen_cmd = cmd;
            self.kind = if self.link.kind.load(Relaxed) == 1 {
                NoiseKind::White
            } else {
                NoiseKind::Pink
            };
            self.routing = match self.link.routing.load(Relaxed) {
                0 => Routing::Left,
                1 => Routing::Right,
                _ => Routing::Both,
            };
            self.mode = if self.link.mode.load(Relaxed) == 1 {
                InjectMode::Add
            } else {
                InjectMode::Replace
            };
            let lvl = f64::from(f32::from_bits(self.link.level_bits.load(Relaxed)))
                .clamp(NOISE_LEVEL_MIN_DB, NOISE_LEVEL_MAX_DB);
            self.gain = 10f64.powf(lvl / 20.0);
            self.capturing = true;
            self.fading_in = true;
            self.link.capturing.store(true, Release);
        } else if !on && self.capturing {
            self.end_capture();
        }
    }

    fn end_capture(&mut self) {
        self.capturing = false;
        self.fading_in = false;
        self.link.capturing.store(false, Release);
    }

    #[inline]
    fn noise(&mut self) -> f64 {
        if self.fading_in {
            self.pos = (self.pos + self.step).min(1.0);
        } else {
            self.pos = (self.pos - self.step).max(0.0);
        }
        let g = 0.5 - 0.5 * (std::f64::consts::PI * self.pos).cos();
        let x = match self.kind {
            NoiseKind::Pink => self.pink.next_sample(),
            NoiseKind::White => self.white.next_sample(),
        };
        x * g * self.gain
    }

    /// Push a frame; ends the measurement on ring overrun or when complete.
    #[inline]
    fn capture(&mut self, frame: Frame) {
        let n = self.link.captured.load(Relaxed);
        if n >= self.link.max_frames.load(Acquire) {
            self.finish();
            return;
        }
        // Nobody drains the ring (editor closed or stalled): stop the noise
        // after half the ring instead of letting it run on.
        if self.link.ring.len() >= RING_FRAMES / 2 || self.link.ring.push(frame).is_err() {
            self.link.overrun.store(true, Release);
            self.finish();
            return;
        }
        self.link.captured.store(n + 1, Release);
        if n + 1 >= self.link.max_frames.load(Acquire) {
            self.finish();
        }
    }

    fn finish(&mut self) {
        // clear the request unless the control side started a new one meanwhile
        let _ = self
            .link
            .cmd
            .compare_exchange(self.seen_cmd, self.seen_cmd & !1, AcqRel, Acquire);
        self.end_capture();
    }

    fn begin_block(&mut self, n: usize) {
        self.link.audio_frames.fetch_add(n as u64, Release);
        self.sync();
    }

    fn end_block(&mut self) {
        self.link.noise_active.store(self.noise_active(), Relaxed);
    }

    /// Stereo block before the EQ. `ret_l`/`ret_r` are the sidechain return
    /// (may be empty or shorter when no sidechain is connected: missing
    /// samples read as silence).
    pub fn process_stereo(&mut self, l: &mut [f32], r: &mut [f32], ret_l: &[f32], ret_r: &[f32]) {
        self.begin_block(l.len().min(r.len()));
        if !self.capturing && !self.noise_active() {
            return;
        }
        let n = l.len().min(r.len());
        for i in 0..n {
            let inject = self.capturing || self.noise_active();
            let noise = if inject { self.noise() } else { 0.0 };
            let (mut a, mut b) = (f64::from(l[i]), f64::from(r[i]));
            if inject {
                match (self.mode, self.routing) {
                    (InjectMode::Replace, Routing::Both) => {
                        a = noise;
                        b = noise;
                    }
                    (InjectMode::Replace, Routing::Left) => {
                        a = noise;
                        b = 0.0;
                    }
                    (InjectMode::Replace, Routing::Right) => {
                        a = 0.0;
                        b = noise;
                    }
                    (InjectMode::Add, Routing::Both) => {
                        a += noise;
                        b += noise;
                    }
                    (InjectMode::Add, Routing::Left) => a += noise,
                    (InjectMode::Add, Routing::Right) => b += noise,
                }
                l[i] = a as f32;
                r[i] = b as f32;
            }
            if self.capturing {
                let reference = match self.routing {
                    Routing::Left => l[i],
                    Routing::Right => r[i],
                    Routing::Both => 0.5 * (l[i] + r[i]),
                };
                let rl = ret_l.get(i).copied().unwrap_or(0.0);
                let rr = ret_r.get(i).copied().unwrap_or(0.0);
                self.capture([reference, finite(rl), finite(rr)]);
            }
        }
        self.end_block();
    }

    /// Mono block before the EQ; the return may have one or two channels.
    pub fn process_mono(&mut self, m: &mut [f32], ret_l: &[f32], ret_r: &[f32]) {
        self.begin_block(m.len());
        if !self.capturing && !self.noise_active() {
            return;
        }
        for (i, sample) in m.iter_mut().enumerate() {
            let inject = self.capturing || self.noise_active();
            let noise = if inject { self.noise() } else { 0.0 };
            if inject {
                *sample = match self.mode {
                    InjectMode::Replace => noise as f32,
                    InjectMode::Add => (f64::from(*sample) + noise) as f32,
                };
            }
            if self.capturing {
                let rl = ret_l.get(i).copied().unwrap_or(0.0);
                // a mono return feeds both channels
                let rr = if ret_r.is_empty() {
                    rl
                } else {
                    ret_r.get(i).copied().unwrap_or(0.0)
                };
                self.capture([*sample, finite(rl), finite(rr)]);
            }
        }
        self.end_block();
    }
}

#[inline]
fn finite(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_blocks(tap: &mut CalTap, total: usize, block: usize) -> (Vec<f32>, Vec<Frame>) {
        let mut out = Vec::new();
        let mut frames = Vec::new();
        let mut pos = 0;
        while pos < total {
            let n = block.min(total - pos);
            let mut l = vec![0.25f32; n];
            let mut r = vec![0.25f32; n];
            let ret: Vec<f32> = (0..n).map(|i| (pos + i) as f32 * 1e-6).collect();
            tap.process_stereo(&mut l, &mut r, &ret, &ret);
            out.extend_from_slice(&l);
            while let Some(f) = tap.link().pop() {
                frames.push(f);
            }
            pos += n;
        }
        (out, frames)
    }

    #[test]
    fn idle_is_transparent_and_silent() {
        let link = Arc::new(CalLink::new());
        let mut tap = CalTap::new(link.clone(), 48_000.0);
        let (out, frames) = run_blocks(&mut tap, 5000, 512);
        assert!(out.iter().all(|&v| v == 0.25));
        assert!(frames.is_empty());
        assert!(!link.noise_active() && !link.is_running());
        assert_eq!(link.audio_frames(), 5000);
    }

    #[test]
    fn measurement_runs_and_stops_by_itself() {
        let link = Arc::new(CalLink::new());
        let mut tap = CalTap::new(link.clone(), 48_000.0);
        link.start(
            NoiseKind::Pink,
            Routing::Both,
            InjectMode::Replace,
            -20.0,
            10_000,
        );
        let (out, frames) = run_blocks(&mut tap, 30_000, 480);
        assert_eq!(frames.len(), 10_000);
        assert!(link.finished() && !link.noise_active());
        // return channel is aligned with the reference frame by frame
        assert!((frames[100][1] - 100e-6).abs() < 1e-9);
        // replace: input programme (0.25) is gone, noise has about -20 dBFS RMS
        let mid = &out[5_000..9_000];
        let rms =
            (mid.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / mid.len() as f64).sqrt();
        let db = 20.0 * rms.log10();
        assert!((db + 20.0).abs() < 1.5, "{db}");
        // after the end the programme is back
        assert!(out[20_000..].iter().all(|&v| v == 0.25));
        // first sample is silent (fade-in)
        assert!(out[0].abs() < 1e-4);
    }

    #[test]
    fn captured_data_is_block_size_independent() {
        let mut caps = Vec::new();
        for block in [1usize, 64, 480, 1000, 4096] {
            let link = Arc::new(CalLink::new());
            let mut tap = CalTap::new(link.clone(), 48_000.0);
            link.start(
                NoiseKind::White,
                Routing::Both,
                InjectMode::Add,
                -30.0,
                6000,
            );
            let (_, frames) = run_blocks(&mut tap, 8000, block);
            caps.push(frames);
        }
        for c in &caps[1..] {
            assert_eq!(c, &caps[0]);
        }
        assert_eq!(caps[0].len(), 6000);
    }

    #[test]
    fn level_is_capped_and_routing_is_respected() {
        let link = Arc::new(CalLink::new());
        let mut tap = CalTap::new(link.clone(), 48_000.0);
        link.start(
            NoiseKind::White,
            Routing::Left,
            InjectMode::Replace,
            10.0,
            20_000,
        );
        let (_, _) = run_blocks(&mut tap, 100, 100);
        let mut l = vec![0.0f32; 20_000];
        let mut r = vec![0.5f32; 20_000];
        tap.process_stereo(&mut l, &mut r, &[], &[]);
        let rms = (l[2000..].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / 18_000.0).sqrt();
        assert!((20.0 * rms.log10() + 6.0).abs() < 0.5, "capped at -6 dBFS");
        assert!(
            r.iter().all(|&v| v == 0.0),
            "other channel muted in replace mode"
        );
    }

    #[test]
    fn stop_and_reset_silence_the_noise() {
        let link = Arc::new(CalLink::new());
        let mut tap = CalTap::new(link.clone(), 48_000.0);
        link.start(
            NoiseKind::Pink,
            Routing::Both,
            InjectMode::Replace,
            -20.0,
            1_000_000,
        );
        run_blocks(&mut tap, 4800, 480);
        assert!(link.noise_active());
        link.stop();
        let (out, _) = run_blocks(&mut tap, 4800, 480);
        // fades out within ~20 ms and the programme returns
        assert!(out[2000..].iter().all(|&v| v == 0.25));
        assert!(!link.noise_active());
        // reset is immediate
        link.start(
            NoiseKind::Pink,
            Routing::Both,
            InjectMode::Replace,
            -20.0,
            1_000_000,
        );
        run_blocks(&mut tap, 4800, 480);
        tap.reset();
        assert!(!link.is_running() && !link.noise_active());
        let (out, frames) = run_blocks(&mut tap, 480, 480);
        assert!(out.iter().all(|&v| v == 0.25) && frames.is_empty());
    }

    #[test]
    fn overrun_stops_the_measurement() {
        let link = Arc::new(CalLink::new());
        let mut tap = CalTap::new(link.clone(), 48_000.0);
        link.start(
            NoiseKind::White,
            Routing::Both,
            InjectMode::Replace,
            -20.0,
            u64::MAX,
        );
        // never drain
        let mut l = vec![0.0f32; 4096];
        let mut r = vec![0.0f32; 4096];
        for _ in 0..80 {
            tap.process_stereo(&mut l, &mut r, &[], &[]);
        }
        assert!(link.overrun() && link.finished());
    }

    #[test]
    fn mono_path_with_mono_return() {
        let link = Arc::new(CalLink::new());
        let mut tap = CalTap::new(link.clone(), 48_000.0);
        link.start(
            NoiseKind::White,
            Routing::Both,
            InjectMode::Replace,
            -20.0,
            500,
        );
        let mut m = vec![0.3f32; 600];
        let ret = vec![0.1f32; 600];
        tap.process_mono(&mut m, &ret, &[]);
        let mut n = 0;
        while let Some(f) = link.pop() {
            assert_eq!((f[1], f[2]), (0.1, 0.1));
            n += 1;
        }
        assert_eq!(n, 500);
    }
}
