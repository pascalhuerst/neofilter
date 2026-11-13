//! Spectrum analyzer hand-off: audio thread -> GUI thread.
//!
//! # How the audio side must use this (for the `process()` / `initialize()` author)
//!
//! Construct once, in `Default for NeoFilter` (or equivalent):
//!
//! ```ignore
//! let (analyzer_in, analyzer_out) = analyzer::analyzer(params.editor_state.clone());
//! let handles = editor::EditorHandles::new(analyzer_out); // keep for `editor()`
//! // store `analyzer_in: AnalyzerInput` and `handles` in the plugin struct
//! ```
//!
//! * `initialize()`: call `self.analyzer_in.set_sample_rate(buffer_config.sample_rate)`.
//!   This must always be called; the editor uses it for the response curve too.
//! * `process()`, per buffer, *before* any EQ processing and with the unmodified
//!   input: `self.analyzer_in.push_pre(left, Some(right))` (or `push_pre(mono, None)`
//!   for mono layouts). *After* EQ + output gain, with the final output:
//!   `self.analyzer_in.push_post(left, Some(right))`. Slices are `f32` and may have
//!   any length (the host block size). Pass the same number of samples per
//!   pre/post call.
//! * Both calls are real-time safe (no allocation, no locks) and return after two
//!   relaxed atomic loads when the editor is closed or the analyzer is switched off.
//!   Do not gate the calls yourself.
//!
//! # Design
//!
//! Each of pre/post has a single-producer single-consumer ring of mono-summed
//! `f32` samples (stored as `AtomicU32` bit patterns, so no `unsafe`). The GUI
//! pulls new samples every frame, and runs the window + FFT (`realfft`, Hann, 4096
//! points, 50 % overlap) on the GUI thread. If the GUI falls behind, old samples
//! are dropped, never the audio thread blocked. The output side lives in an
//! `Arc<Mutex<AnalyzerOutput>>` that only the GUI thread locks, so the editor can
//! be closed and re-opened any number of times.
//!
//! The spectrum is in dBFS of a sine's amplitude (a full-scale sine reads 0 dB),
//! with instant attack and linear-in-dB decay (peak hold style).

use atomic_float::AtomicF32;
use nih_plug_egui::EguiState;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// FFT length used by the analyzer.
pub const FFT_SIZE: usize = 4096;
/// Hop between FFTs (50 % overlap).
pub const HOP: usize = FFT_SIZE / 2;
/// Ring capacity per channel (power of two).
const RING_LEN: usize = 1 << 15;
/// Spectrum floor in dB.
pub const FLOOR_DB: f32 = -120.0;
/// Display decay in dB per second.
pub const DECAY_DB_PER_SEC: f32 = 24.0;
/// At most this many FFTs are run per `update()` per channel.
const MAX_HOPS_PER_UPDATE: usize = 6;

/// State shared between audio thread, plugin and editor.
pub struct AnalyzerShared {
    /// User switch (analyzer button).
    enabled: AtomicBool,
    /// Test/standalone override: ignore the editor-open state.
    ignore_editor_state: AtomicBool,
    sample_rate: AtomicF32,
    editor_state: Arc<EguiState>,
}

impl AnalyzerShared {
    /// Whether the audio side should feed samples.
    pub fn active(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
            && (self.ignore_editor_state.load(Ordering::Relaxed) || self.editor_state.is_open())
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Feed samples even if the editor is closed (tests, standalone tools).
    pub fn set_ignore_editor_state(&self, ignore: bool) {
        self.ignore_editor_state.store(ignore, Ordering::Relaxed);
    }

    /// Last sample rate set by the audio side (48000 until known).
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate.load(Ordering::Relaxed)
    }
}

/// SPSC ring of f32 samples. Lossy: the producer never waits.
struct Ring {
    buf: Box<[AtomicU32]>,
    /// Total number of samples ever written.
    head: AtomicU64,
}

impl Ring {
    fn new() -> Self {
        Self {
            buf: (0..RING_LEN).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicU64::new(0),
        }
    }
}

struct Producer {
    ring: Arc<Ring>,
    written: u64,
}

impl Producer {
    fn push(&mut self, l: &[f32], r: Option<&[f32]>) {
        let n = match r {
            Some(r) => l.len().min(r.len()),
            None => l.len(),
        };
        let mask = RING_LEN as u64 - 1;
        for i in 0..n {
            let s = match r {
                Some(r) => 0.5 * (l[i] + r[i]),
                None => l[i],
            };
            self.ring.buf[((self.written + i as u64) & mask) as usize]
                .store(s.to_bits(), Ordering::Relaxed);
        }
        self.written += n as u64;
        self.ring.head.store(self.written, Ordering::Release);
    }
}

/// Audio-thread end.
pub struct AnalyzerInput {
    shared: Arc<AnalyzerShared>,
    pre: Producer,
    post: Producer,
}

impl AnalyzerInput {
    /// Call from `initialize()`.
    pub fn set_sample_rate(&mut self, sr: f32) {
        if sr.is_finite() && sr > 0.0 {
            self.shared.sample_rate.store(sr, Ordering::Relaxed);
        }
    }

    /// Feed mono-summed (L+R)/2 pre-EQ samples. Real-time safe.
    pub fn push_pre(&mut self, samples_l: &[f32], samples_r: Option<&[f32]>) {
        if self.shared.active() {
            self.pre.push(samples_l, samples_r);
        }
    }

    /// Feed mono-summed (L+R)/2 post-EQ samples. Real-time safe.
    pub fn push_post(&mut self, samples_l: &[f32], samples_r: Option<&[f32]>) {
        if self.shared.active() {
            self.post.push(samples_l, samples_r);
        }
    }
}

/// Peak-hold with linear-in-dB decay: `new` wins if larger than the decayed `prev`.
pub fn peak_decay(prev: f32, new: f32, drop_db: f32) -> f32 {
    new.max(prev - drop_db).max(FLOOR_DB)
}

/// Magnitude of an FFT bin as dBFS of a sine's amplitude.
/// `window_sum` is the sum of the window coefficients (coherent gain).
pub fn bin_to_db(bin: Complex<f32>, window_sum: f32) -> f32 {
    let amp = 2.0 * bin.norm() / window_sum;
    (20.0 * amp.max(1e-9).log10()).max(FLOOR_DB)
}

/// Resample a linear-frequency spectrum onto `out.len()` log-spaced columns
/// spanning `f_lo..f_hi`. Each column takes the maximum of the bins it covers, or
/// the linearly interpolated value when it is narrower than a bin.
pub fn column_levels(spectrum: &[f32], bin_hz: f32, f_lo: f32, f_hi: f32, out: &mut [f32]) {
    let n = out.len();
    if n == 0 || spectrum.is_empty() || bin_hz <= 0.0 {
        out.fill(FLOOR_DB);
        return;
    }
    let ratio = (f_hi / f_lo).ln();
    let edge = |c: usize| f_lo * (ratio * c as f32 / n as f32).exp();
    let last = spectrum.len() - 1;
    for (c, o) in out.iter_mut().enumerate() {
        let (a, b) = (edge(c) / bin_hz, edge(c + 1) / bin_hz);
        let (ia, ib) = (a.ceil() as usize, b.floor() as usize);
        *o = if ia <= ib {
            spectrum[ia.min(last)..=ib.min(last)]
                .iter()
                .copied()
                .fold(FLOOR_DB, f32::max)
        } else {
            let x = (0.5 * (a + b)).min(last as f32);
            let i = x.floor() as usize;
            let t = x - i as f32;
            spectrum[i] * (1.0 - t) + spectrum[(i + 1).min(last)] * t
        };
    }
}

struct Channel {
    ring: Arc<Ring>,
    pos: u64,
    /// Last FFT_SIZE samples, oldest first after `linearize`.
    history: Vec<f32>,
    write: usize,
    since_hop: usize,
    spectrum: Vec<f32>,
}

impl Channel {
    fn new(ring: Arc<Ring>) -> Self {
        Self {
            ring,
            pos: 0,
            history: vec![0.0; FFT_SIZE],
            write: 0,
            since_hop: 0,
            spectrum: vec![FLOOR_DB; FFT_SIZE / 2 + 1],
        }
    }

    fn clear(&mut self) {
        self.history.fill(0.0);
        self.since_hop = 0;
        self.spectrum.fill(FLOOR_DB);
    }
}

/// GUI-thread end. Owns the FFT.
pub struct AnalyzerOutput {
    shared: Arc<AnalyzerShared>,
    pre: Channel,
    post: Channel,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    window_sum: f32,
    time: Vec<f32>,
    freq: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

/// Create the analyzer pair. `editor_state` tells the audio side whether the
/// editor is open.
pub fn analyzer(editor_state: Arc<EguiState>) -> (AnalyzerInput, AnalyzerOutput) {
    let shared = Arc::new(AnalyzerShared {
        enabled: AtomicBool::new(true),
        ignore_editor_state: AtomicBool::new(false),
        sample_rate: AtomicF32::new(48000.0),
        editor_state,
    });
    let pre_ring = Arc::new(Ring::new());
    let post_ring = Arc::new(Ring::new());
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos())
        .collect();
    let window_sum = window.iter().sum();
    let input = AnalyzerInput {
        shared: shared.clone(),
        pre: Producer {
            ring: pre_ring.clone(),
            written: 0,
        },
        post: Producer {
            ring: post_ring.clone(),
            written: 0,
        },
    };
    let output = AnalyzerOutput {
        shared,
        pre: Channel::new(pre_ring),
        post: Channel::new(post_ring),
        time: fft.make_input_vec(),
        freq: fft.make_output_vec(),
        scratch: fft.make_scratch_vec(),
        fft,
        window,
        window_sum,
    };
    (input, output)
}

impl AnalyzerOutput {
    pub fn shared(&self) -> Arc<AnalyzerShared> {
        self.shared.clone()
    }

    /// Frequency spacing of the spectrum bins.
    pub fn bin_hz(&self) -> f32 {
        self.shared.sample_rate() / FFT_SIZE as f32
    }

    /// Pre-EQ spectrum in dBFS (`FFT_SIZE / 2 + 1` bins).
    pub fn pre_db(&self) -> &[f32] {
        &self.pre.spectrum
    }

    /// Post-EQ spectrum in dBFS.
    pub fn post_db(&self) -> &[f32] {
        &self.post.spectrum
    }

    /// Pull new samples and run any due FFTs. Call once per GUI frame.
    pub fn update(&mut self) {
        let drop_db = DECAY_DB_PER_SEC * HOP as f32 / self.shared.sample_rate();
        let Self {
            pre,
            post,
            fft,
            window,
            window_sum,
            time,
            freq,
            scratch,
            ..
        } = self;
        for ch in [pre, post] {
            Self::pull(
                ch,
                fft.as_ref(),
                window,
                *window_sum,
                time,
                freq,
                scratch,
                drop_db,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pull(
        ch: &mut Channel,
        fft: &dyn RealToComplex<f32>,
        window: &[f32],
        window_sum: f32,
        time: &mut [f32],
        freq: &mut [Complex<f32>],
        scratch: &mut [Complex<f32>],
        drop_db: f32,
    ) {
        let head = ch.ring.head.load(Ordering::Acquire);
        let mut lag = head.saturating_sub(ch.pos);
        if lag > (RING_LEN as u64) * 3 / 4 {
            // Fell too far behind (or the editor was closed for a long while):
            // restart from recent data, discarding stale samples.
            ch.pos = head - (MAX_HOPS_PER_UPDATE * HOP) as u64;
            lag = head - ch.pos;
        }
        // Bound the work per frame.
        let max = (MAX_HOPS_PER_UPDATE * HOP) as u64;
        if lag > max {
            ch.pos = head - max;
            lag = max;
        }
        let mask = RING_LEN as u64 - 1;
        for k in 0..lag {
            let s =
                f32::from_bits(ch.ring.buf[((ch.pos + k) & mask) as usize].load(Ordering::Relaxed));
            ch.history[ch.write] = s;
            ch.write = (ch.write + 1) % FFT_SIZE;
            ch.since_hop += 1;
            if ch.since_hop >= HOP {
                ch.since_hop = 0;
                // Oldest sample sits at `write`.
                for (i, t) in time.iter_mut().enumerate() {
                    *t = ch.history[(ch.write + i) % FFT_SIZE] * window[i];
                }
                if fft.process_with_scratch(time, freq, scratch).is_ok() {
                    for (lvl, bin) in ch.spectrum.iter_mut().zip(freq.iter()) {
                        *lvl = peak_decay(*lvl, bin_to_db(*bin, window_sum), drop_db);
                    }
                }
            }
        }
        ch.pos = head;
    }

    /// Reset both spectra to the floor.
    pub fn clear(&mut self) {
        self.pre.clear();
        self.post.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> (AnalyzerInput, AnalyzerOutput) {
        let (i, o) = analyzer(EguiState::from_size(100, 100));
        o.shared().set_ignore_editor_state(true);
        (i, o)
    }

    fn sine(f: f32, sr: f32, start: usize, n: usize, amp: f32) -> Vec<f32> {
        (start..start + n)
            .map(|i| amp * (std::f32::consts::TAU * f * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn sine_peak_in_right_bin_at_right_level() {
        let (mut i, mut o) = make();
        i.set_sample_rate(48000.0);
        // Bin-centred frequency: bin 85 -> 85 * 48000/4096 = 996.09 Hz.
        let f = 85.0 * 48000.0 / FFT_SIZE as f32;
        let x = sine(f, 48000.0, 0, 16384, 0.5);
        for chunk in x.chunks(509) {
            i.push_pre(chunk, None);
            o.update();
        }
        let s = o.pre_db();
        let (peak_bin, peak_db) =
            s.iter()
                .enumerate()
                .fold((0, f32::MIN), |a, (k, &v)| if v > a.1 { (k, v) } else { a });
        assert_eq!(peak_bin, 85);
        // 0.5 amplitude = -6.02 dBFS.
        assert!((peak_db + 6.02).abs() < 0.2, "peak {peak_db}");
        assert!(s[200] < -60.0);
        // Post channel untouched.
        assert!(o.post_db().iter().all(|&v| v == FLOOR_DB));
    }

    #[test]
    fn stereo_sum_and_mismatched_lengths() {
        let (mut i, mut o) = make();
        let l = sine(1000.0, 48000.0, 0, 8192, 0.5);
        let r = sine(1000.0, 48000.0, 0, 8192, 0.5);
        for (a, b) in l.chunks(777).zip(r.chunks(777)) {
            i.push_post(a, Some(b));
        }
        // Different lengths must not panic.
        i.push_post(&l[..10], Some(&r[..5]));
        o.update();
        let peak = o.post_db().iter().copied().fold(f32::MIN, f32::max);
        assert!(peak > -8.0 && peak < -5.0, "peak {peak}");
    }

    #[test]
    fn odd_block_sizes_match_single_block() {
        let x = sine(1234.0, 48000.0, 0, 9000, 0.3);
        let (mut i1, mut o1) = make();
        i1.push_pre(&x, None);
        o1.update();
        let (mut i2, mut o2) = make();
        for c in x.chunks(37) {
            i2.push_pre(c, None);
        }
        o2.update();
        assert_eq!(o1.pre_db(), o2.pre_db());
    }

    #[test]
    fn inactive_when_editor_closed_or_disabled() {
        let (mut i, mut o) = analyzer(EguiState::from_size(100, 100));
        let x = sine(1000.0, 48000.0, 0, 8192, 0.5);
        i.push_pre(&x, None);
        o.update();
        assert!(o.pre_db().iter().all(|&v| v == FLOOR_DB));
        o.shared().set_ignore_editor_state(true);
        o.shared().set_enabled(false);
        i.push_pre(&x, None);
        o.update();
        assert!(o.pre_db().iter().all(|&v| v == FLOOR_DB));
        o.shared().set_enabled(true);
        i.push_pre(&x, None);
        o.update();
        assert!(o.pre_db().iter().any(|&v| v > -20.0));
    }

    #[test]
    fn overrun_resyncs_without_panic() {
        let (mut i, mut o) = make();
        let x = sine(1000.0, 48000.0, 0, 200_000, 0.5);
        i.push_pre(&x, None);
        o.update();
        assert!(o.pre_db().iter().any(|&v| v > -20.0));
    }

    #[test]
    fn decay_is_linear_and_floored() {
        assert_eq!(peak_decay(-10.0, -50.0, 3.0), -13.0);
        assert_eq!(peak_decay(-10.0, -5.0, 3.0), -5.0);
        assert_eq!(peak_decay(FLOOR_DB, FLOOR_DB, 3.0), FLOOR_DB);
    }

    #[test]
    fn sample_rate_updates_bin_width() {
        let (mut i, o) = make();
        assert!((o.bin_hz() - 48000.0 / 4096.0).abs() < 1e-3);
        i.set_sample_rate(96000.0);
        i.set_sample_rate(f32::NAN);
        assert!((o.bin_hz() - 96000.0 / 4096.0).abs() < 1e-3);
    }

    #[test]
    fn columns_max_and_interpolate() {
        let mut spec = vec![-100.0; 2049];
        spec[100] = -10.0;
        let bin_hz = 48000.0 / 4096.0;
        let mut cols = vec![0.0; 400];
        column_levels(&spec, bin_hz, 10.0, 30000.0, &mut cols);
        let peak = cols.iter().copied().fold(f32::MIN, f32::max);
        assert!(peak > -11.0);
        // Low columns narrower than a bin interpolate (no floor jump).
        assert!(cols[0] <= -99.0);
    }
}
