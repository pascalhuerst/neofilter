//! Host-independent audio path: f32 in/out around the f64 RIAA filters, with
//! click-free mode/option changes, smoothed trim, soft bypass and metering.
//!
//! * All eight filters (two modes times four option combinations) run on every
//!   sample so that their state is always valid; the output is a weighted sum
//!   in which exactly one weight is 1. Changing mode/options ramps the weights
//!   linearly to the new one-hot vector over 15 ms (a crossfade between two
//!   warm filters; no coefficient is ever swapped under a running state, and
//!   nothing is redesigned on the audio thread).
//! * Trim ramps linearly in dB over 20 ms; bypass crossfades dry/wet over
//!   10 ms and then leaves the buffer bit-exact.
//! * Non-finite input samples are replaced by silence in the wet path.
//! * Meters: sample peak with hold, and a latching over-0-dBFS indicator,
//!   measured on the samples written to the host buffer.
//! * Nothing here allocates after construction.

use crate::dsp::{Cascade, CascadeState, Mode, Options, RiaaFilters};
use atomic_float::AtomicF32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const FADE_S: f64 = 0.015;
const TRIM_RAMP_S: f64 = 0.020;
const BYPASS_RAMP_S: f64 = 0.010;
/// Meter fall-back rate (dB per second) of the running peak.
pub const PEAK_FALL_DB_PER_S: f64 = 20.0;
/// How long the peak-hold value is held (seconds).
pub const PEAK_HOLD_S: f64 = 2.0;

const SLOTS: usize = 8;

fn slot(mode: Mode, o: Options) -> usize {
    let m = match mode {
        Mode::PreEmphasis => 0,
        Mode::DeEmphasis => 1,
    };
    m * 4 + usize::from(o.iec) + 2 * usize::from(o.ultrasonic)
}

/// Meter values shared with the GUI.
#[derive(Debug, Default)]
pub struct Meters {
    peak: AtomicF32,
    hold: AtomicF32,
    clipped: AtomicBool,
    reset_request: AtomicBool,
}

impl Meters {
    /// Running sample peak (linear), falling at 20 dB/s.
    pub fn peak(&self) -> f32 {
        self.peak.load(Ordering::Relaxed)
    }
    /// Held sample peak (linear).
    pub fn hold(&self) -> f32 {
        self.hold.load(Ordering::Relaxed)
    }
    /// True once any output sample reached 0 dBFS or more since the last reset.
    pub fn clipped(&self) -> bool {
        self.clipped.load(Ordering::Relaxed)
    }
    /// Clear the clip latch and the peak-hold value.
    pub fn reset(&self) {
        self.clipped.store(false, Ordering::Relaxed);
        self.reset_request.store(true, Ordering::Relaxed);
    }
}

/// Parameters for one `process_*` call.
#[derive(Debug, Clone, Copy)]
pub struct AudioParams {
    pub mode: Mode,
    pub options: Options,
    pub trim_db: f64,
    pub bypass: bool,
}

impl Default for AudioParams {
    fn default() -> Self {
        Self {
            mode: Mode::PreEmphasis,
            options: Options::default(),
            trim_db: 0.0,
            bypass: false,
        }
    }
}

fn db_to_lin(db: f64) -> f64 {
    if db == 0.0 {
        1.0
    } else {
        10f64.powf(db / 20.0)
    }
}

#[inline]
fn clean(s: f32) -> f64 {
    if s.is_finite() {
        f64::from(s)
    } else {
        0.0
    }
}

/// Linear ramp; with `db` set the ramped value is a dB gain.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    cur: f64,
    target: f64,
    step: f64,
    left: usize,
    db: bool,
    lin: f64,
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

pub struct AudioPath {
    cascades: [Cascade; SLOTS],
    states: [[CascadeState; 2]; SLOTS],
    weights: [f64; SLOTS],
    weight_step: [f64; SLOTS],
    fade_left: usize,
    active: usize,
    trim: Ramp,
    /// 0 = fully wet, 1 = fully dry.
    mix: Ramp,
    sample_rate: f64,
    fade_len: usize,
    trim_len: usize,
    mix_len: usize,
    fresh: bool,
    meters: Arc<Meters>,
    peak: f32,
    hold: f32,
    hold_left: f64,
}

impl AudioPath {
    /// Build from a design (see [`RiaaFilters::design`], which allocates and is
    /// slow, so do it outside the audio thread).
    pub fn new(filters: &RiaaFilters, meters: Arc<Meters>) -> Self {
        let mut p = Self {
            cascades: [Cascade::identity(); SLOTS],
            states: [[CascadeState::default(); 2]; SLOTS],
            weights: [0.0; SLOTS],
            weight_step: [0.0; SLOTS],
            fade_left: 0,
            active: 0,
            trim: Ramp::new(0.0, true),
            mix: Ramp::new(0.0, false),
            sample_rate: 48_000.0,
            fade_len: 1,
            trim_len: 1,
            mix_len: 1,
            fresh: true,
            meters,
            peak: 0.0,
            hold: 0.0,
            hold_left: 0.0,
        };
        p.prepare(filters);
        p
    }

    /// Switch to a (new) design. The next `process_*` snaps to the parameters.
    pub fn prepare(&mut self, filters: &RiaaFilters) {
        for mode in [Mode::PreEmphasis, Mode::DeEmphasis] {
            for o in Options::ALL {
                self.cascades[slot(mode, o)] = *filters.cascade(mode, o);
            }
        }
        self.sample_rate = filters.sample_rate();
        self.fade_len = (FADE_S * self.sample_rate) as usize;
        self.trim_len = (TRIM_RAMP_S * self.sample_rate) as usize;
        self.mix_len = (BYPASS_RAMP_S * self.sample_rate) as usize;
        self.reset();
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Clear filter state; the next `process_*` snaps to the parameters.
    pub fn reset(&mut self) {
        self.states = [[CascadeState::default(); 2]; SLOTS];
        self.fade_left = 0;
        self.fresh = true;
    }

    fn apply(&mut self, p: &AudioParams) {
        let key = slot(p.mode, p.options);
        let trim = if p.trim_db.is_finite() {
            p.trim_db.clamp(-60.0, 24.0)
        } else {
            0.0
        };
        let mix = if p.bypass { 1.0 } else { 0.0 };
        if self.fresh {
            self.weights = [0.0; SLOTS];
            self.weights[key] = 1.0;
            self.active = key;
            self.fade_left = 0;
            self.trim = Ramp::new(trim, true);
            self.mix = Ramp::new(mix, false);
            self.fresh = false;
        } else {
            if key != self.active {
                self.active = key;
                self.fade_left = self.fade_len.max(1);
                for i in 0..SLOTS {
                    let t = if i == key { 1.0 } else { 0.0 };
                    self.weight_step[i] = (t - self.weights[i]) / self.fade_left as f64;
                }
            }
            self.trim.set_target(trim, self.trim_len);
            self.mix.set_target(mix, self.mix_len);
        }
    }

    /// Run all filters on one frame and return the weighted wet output.
    #[inline]
    fn frame<const N: usize>(&mut self, x: [f64; N]) -> [f64; N] {
        let mut out = [0.0; N];
        if self.fade_left > 0 {
            self.fade_left -= 1;
            if self.fade_left == 0 {
                self.weights = [0.0; SLOTS];
                self.weights[self.active] = 1.0;
            } else {
                for i in 0..SLOTS {
                    self.weights[i] += self.weight_step[i];
                }
            }
            for i in 0..SLOTS {
                for ch in 0..N {
                    let y = self.states[i][ch].process(&self.cascades[i], x[ch]);
                    out[ch] += self.weights[i] * y;
                }
            }
        } else {
            for i in 0..SLOTS {
                for ch in 0..N {
                    let y = self.states[i][ch].process(&self.cascades[i], x[ch]);
                    if i == self.active {
                        out[ch] = y;
                    }
                }
            }
        }
        out
    }

    /// Meter bookkeeping once per call.
    fn publish(&mut self, block_peak: f32, frames: usize) {
        let dt = frames as f64 / self.sample_rate;
        if self.meters.reset_request.swap(false, Ordering::Relaxed) {
            self.peak = 0.0;
            self.hold = 0.0;
            self.hold_left = 0.0;
        }
        let fall = 10f64.powf(-PEAK_FALL_DB_PER_S * dt / 20.0) as f32;
        self.peak = block_peak.max(self.peak * fall);
        if block_peak >= self.hold {
            self.hold = block_peak;
            self.hold_left = PEAK_HOLD_S;
        } else {
            self.hold_left -= dt;
            if self.hold_left <= 0.0 {
                self.hold = self.peak;
                self.hold_left = PEAK_HOLD_S;
            }
        }
        self.meters.peak.store(self.peak, Ordering::Relaxed);
        self.meters.hold.store(self.hold, Ordering::Relaxed);
    }

    #[inline]
    fn finish(&mut self, wet: f64, dry: f32, block_peak: &mut f32, clip: &mut bool) -> f32 {
        let g = self.trim.lin;
        let m = self.mix.lin;
        let out = if m == 1.0 && self.mix.settled() {
            dry
        } else if m == 0.0 {
            (wet * g) as f32
        } else {
            let w = wet * g;
            (w + m * (f64::from(dry) - w)) as f32
        };
        let a = out.abs();
        if a > *block_peak {
            *block_peak = a;
        }
        if a >= 1.0 {
            *clip = true;
        }
        out
    }

    /// Process a stereo block in place (any length).
    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32], p: &AudioParams) {
        self.apply(p);
        let n = left.len().min(right.len());
        let (mut peak, mut clip) = (0.0f32, false);
        for i in 0..n {
            let (dl, dr) = (left[i], right[i]);
            let [wl, wr] = self.frame([clean(dl), clean(dr)]);
            self.trim.next();
            self.mix.next();
            left[i] = self.finish(wl, dl, &mut peak, &mut clip);
            right[i] = self.finish(wr, dr, &mut peak, &mut clip);
        }
        if clip {
            self.meters.clipped.store(true, Ordering::Relaxed);
        }
        self.publish(peak, n);
    }

    /// Process a mono block in place (any length).
    pub fn process_mono(&mut self, x: &mut [f32], p: &AudioParams) {
        self.apply(p);
        let (mut peak, mut clip) = (0.0f32, false);
        for s in x.iter_mut() {
            let d = *s;
            let [w] = self.frame([clean(d)]);
            self.trim.next();
            self.mix.next();
            *s = self.finish(w, d, &mut peak, &mut clip);
        }
        if clip {
            self.meters.clipped.store(true, Ordering::Relaxed);
        }
        self.publish(peak, x.len());
    }
}
