//! Host-independent audio path: f32 in/out around the f64 [`EqEngine`], with
//! smoothed output gain and a click-free soft bypass.
//!
//! `NeoFilter::process` only gathers parameters and delegates to
//! [`AudioPath`], so the whole path can be tested without a plugin host.
//!
//! * Parameters are applied once per call (the CLAP/VST3 wrappers of nih-plug
//!   already split the host buffer at parameter events because
//!   `SAMPLE_ACCURATE_AUTOMATION` is set; the engine smooths and runs its
//!   updates on a fixed 32-sample grid, so output does not depend on the
//!   block partition). `EqEngine::set_band` is a compare-and-store for
//!   unchanged parameters, so pushing all 24 bands is cheap.
//! * Output gain is ramped linearly in dB over 20 ms.
//! * Bypass crossfades dry and wet linearly over 10 ms. The EQ keeps running
//!   while bypassed (its state stays valid, so un-bypassing needs no reset) and
//!   is zero latency, so the dry signal needs no delay. Once the crossfade has
//!   finished, the buffer is left untouched (bit-exact dry).
//! * Buffers longer than the prepared maximum are processed in chunks.
//! * Nothing here allocates after [`AudioPath::prepare`].

use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, EqEngine};

/// Output gain ramp time.
const GAIN_RAMP_S: f64 = 0.020;
/// Bypass crossfade time.
const BYPASS_RAMP_S: f64 = 0.010;

/// Parameters for one `process_*` call.
#[derive(Debug, Clone, Copy)]
pub struct AudioParams<'a> {
    pub bands: &'a [BandParams; NUM_BANDS],
    /// 1.0 = 100 %.
    pub gain_scale: f64,
    pub out_gain_db: f64,
    pub bypass: bool,
}

/// Linear ramp; `db` ramps convert to a linear gain per sample.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    cur: f64,
    target: f64,
    step: f64,
    left: usize,
    db: bool,
    /// Value as seen by the signal path (linear gain / mix).
    lin: f64,
}

fn db_to_lin(db: f64) -> f64 {
    if db == 0.0 {
        1.0
    } else {
        10f64.powf(db / 20.0)
    }
}

impl Ramp {
    fn new(value: f64, db: bool) -> Self {
        Self {
            cur: value,
            target: value,
            step: 0.0,
            left: 0,
            db,
            lin: if db { db_to_lin(value) } else { value },
        }
    }

    fn snap(&mut self, value: f64) {
        *self = Self::new(value, self.db);
    }

    fn set_target(&mut self, target: f64, len: usize) {
        if target != self.target {
            self.target = target;
            self.left = len.max(1);
            self.step = (target - self.cur) / self.left as f64;
        }
    }

    fn settled(&self) -> bool {
        self.left == 0
    }

    #[inline]
    fn next(&mut self) -> f64 {
        if self.left > 0 {
            self.left -= 1;
            self.cur = if self.left == 0 {
                self.target
            } else {
                self.cur + self.step
            };
            self.lin = if self.db {
                db_to_lin(self.cur)
            } else {
                self.cur
            };
        }
        self.lin
    }
}

/// Write `wet` (scaled by the gain ramp) mixed with the dry samples already
/// in `out` back to `out`.
fn finish(out: &mut [f32], wet: &[f64], gain: &mut Ramp, mix: &mut Ramp) {
    for (o, &w) in out.iter_mut().zip(wet) {
        let g = gain.next();
        let m = mix.next();
        let w = w * g;
        *o = if m == 0.0 {
            w as f32
        } else {
            let dry = f64::from(*o);
            (w + m * (dry - w)) as f32
        };
    }
}

pub struct AudioPath {
    engine: EqEngine,
    sample_rate: f64,
    left: Vec<f64>,
    right: Vec<f64>,
    gain: Ramp,
    /// 0 = fully wet, 1 = fully dry.
    mix: Ramp,
    gain_len: usize,
    mix_len: usize,
    /// Next call snaps gain and bypass to their targets (no ramp).
    fresh: bool,
}

impl Default for AudioPath {
    fn default() -> Self {
        Self::new(44100.0, 512)
    }
}

impl AudioPath {
    /// Allocates the scratch buffers (`max_block` samples per channel).
    pub fn new(sample_rate: f64, max_block: usize) -> Self {
        let mut p = Self {
            engine: EqEngine::new(sample_rate),
            sample_rate,
            left: Vec::new(),
            right: Vec::new(),
            gain: Ramp::new(0.0, true),
            mix: Ramp::new(0.0, false),
            gain_len: 1,
            mix_len: 1,
            fresh: true,
        };
        p.prepare(sample_rate, max_block);
        p
    }

    /// Set sample rate and scratch size. Allocates; call from `initialize()`.
    /// The next `process_*` snaps all state to the then-current parameters.
    pub fn prepare(&mut self, sample_rate: f64, max_block: usize) {
        let n = max_block.max(1);
        self.left.clear();
        self.left.resize(n, 0.0);
        self.right.clear();
        self.right.resize(n, 0.0);
        self.sample_rate = sample_rate;
        self.engine.set_sample_rate(sample_rate);
        // the engine clamps unusable rates; follow it
        let sr = self.engine.sample_rate();
        self.gain_len = (GAIN_RAMP_S * sr) as usize;
        self.mix_len = (BYPASS_RAMP_S * sr) as usize;
        self.fresh = true;
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn max_block(&self) -> usize {
        self.left.len()
    }

    /// Clear filter state; the next `process_*` snaps to the parameters.
    pub fn reset(&mut self) {
        self.engine.reset();
        self.fresh = true;
    }

    fn apply(&mut self, p: &AudioParams) {
        let mix_target = if p.bypass { 1.0 } else { 0.0 };
        let gain_db = if p.out_gain_db.is_finite() {
            p.out_gain_db
        } else {
            0.0
        };
        if self.fresh {
            self.gain.snap(gain_db);
            self.mix.snap(mix_target);
            self.fresh = false;
        } else {
            self.gain.set_target(gain_db, self.gain_len);
            self.mix.set_target(mix_target, self.mix_len);
        }
        // gain scale first: it determines how the first `set_band` snaps
        self.engine.set_gain_scale(p.gain_scale);
        for (i, b) in p.bands.iter().enumerate() {
            self.engine.set_band(i, b);
        }
    }

    /// True when the output is exactly the input (fully bypassed).
    fn dry_only(&self) -> bool {
        self.mix.settled() && self.mix.lin == 1.0
    }

    /// Process a stereo block in place (any length).
    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32], p: &AudioParams) {
        self.apply(p);
        let total = left.len().min(right.len());
        let max = self.left.len();
        let mut pos = 0;
        while pos < total {
            let n = (total - pos).min(max);
            let (l, r) = (&mut left[pos..pos + n], &mut right[pos..pos + n]);
            for (d, s) in self.left.iter_mut().zip(l.iter()) {
                *d = f64::from(*s);
            }
            for (d, s) in self.right.iter_mut().zip(r.iter()) {
                *d = f64::from(*s);
            }
            self.engine
                .process_stereo(&mut self.left[..n], &mut self.right[..n]);
            if self.dry_only() {
                // keep the gain ramp advancing in lockstep
                for _ in 0..n {
                    self.gain.next();
                }
            } else {
                let mut g = self.gain;
                let mut m = self.mix;
                finish(l, &self.left[..n], &mut g, &mut m);
                finish(r, &self.right[..n], &mut self.gain, &mut self.mix);
            }
            pos += n;
        }
    }

    /// Process a mono block in place (any length).
    pub fn process_mono(&mut self, x: &mut [f32], p: &AudioParams) {
        self.apply(p);
        let max = self.left.len();
        let mut pos = 0;
        while pos < x.len() {
            let n = (x.len() - pos).min(max);
            let chunk = &mut x[pos..pos + n];
            for (d, s) in self.left.iter_mut().zip(chunk.iter()) {
                *d = f64::from(*s);
            }
            self.engine.process_mono(&mut self.left[..n]);
            if self.dry_only() {
                for _ in 0..n {
                    self.gain.next();
                }
            } else {
                finish(chunk, &self.left[..n], &mut self.gain, &mut self.mix);
            }
            pos += n;
        }
    }
}
