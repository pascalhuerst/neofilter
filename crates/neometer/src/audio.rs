//! Host-independent audio path of NeoMeter.
//!
//! Per block (processed in chunks of [`CHUNK`] frames so no scratch memory
//! depends on the host block size):
//!
//! 0. the **input trims** (`trim_l` / `trim_r`, +-24 dB, ramped linearly over
//!    [`TRIM_RAMP_S`]) are applied to the buffer in place, so everything
//!    downstream (ring, meters, pass-through) sees the levelled signal; with
//!    both trims at 0 dB (and no ramp running) the buffer is not touched and
//!    the pass-through stays bit-exact,
//! 1. the (trimmed) input frames are pushed to the analysis ring (lossy, never
//!    blocks),
//! 2. the generator (if active) modifies the *output* in place; with the
//!    generator off the buffer is not touched, so the pass-through is
//!    bit-exact and has zero latency,
//! 3. the cheap, realtime-safe `neo-measure` meters run on the **input**
//!    signal: sample peak, true peak, RMS, DC, crest, correlation, M/S,
//!    balance, low-frequency vertical/lateral, and the pre-emphasised vinyl
//!    meters ([`crate::vinyl`]).
//!
//! The meter values are published (wait-free triple buffer) every
//! [`PUBLISH_EVERY`] frames. Nothing here allocates, locks or panics after
//! [`AudioPath::prepare`].

use crate::params::{TRIM_MAX_DB, TRIM_MIN_DB};
use crate::ring::Producer;
use crate::shared::{reset, LevelsReadout, Readouts, Shared, StereoReadout};
use crate::testgen::{GenEngine, GenSettings};
use crate::vinyl::Vinyl;
use neo_measure::levels::{BlockStats, PeakMeter, RmsMeter, TruePeakMeter};
use neo_measure::riaa::Cascade;
use neo_measure::stereo::{LowFreqStereo, StereoMeter};
use neo_measure::util::amp_to_db;
use std::sync::Arc;
use triple_buffer::Input;

/// Frames per processing chunk (also the scratch size).
pub const CHUNK: usize = 256;
/// Meter values are published after at least this many frames.
pub const PUBLISH_EVERY: usize = 256;
const HOLD_S: f64 = 2.0;
const DECAY_DB_S: f64 = 20.0;
const RMS_S: f64 = 0.3;
const SINE_REF_DB: f64 = 3.010_299_956_639_812;
/// Duration of the linear gain ramp after a trim change.
pub const TRIM_RAMP_S: f64 = 0.02;

/// Smoothed linear gain of one channel (no allocation).
#[derive(Debug, Clone, Copy)]
struct Trim {
    gain: f64,
    target_db: f64,
    step: f64,
    left: usize,
}

impl Trim {
    const UNITY: Trim = Trim {
        gain: 1.0,
        target_db: 0.0,
        step: 0.0,
        left: 0,
    };

    fn set(&mut self, db: f64, fs: f64) {
        let db = if db.is_finite() {
            db.clamp(f64::from(TRIM_MIN_DB), f64::from(TRIM_MAX_DB))
        } else {
            0.0
        };
        if (db - self.target_db).abs() < 1e-9 {
            return;
        }
        self.target_db = db;
        let target = if db == 0.0 {
            1.0
        } else {
            10f64.powf(db / 20.0)
        };
        let n = ((TRIM_RAMP_S * fs) as usize).max(1);
        self.step = (target - self.gain) / n as f64;
        self.left = n;
    }

    fn target_gain(&self) -> f64 {
        if self.target_db == 0.0 {
            1.0
        } else {
            10f64.powf(self.target_db / 20.0)
        }
    }

    fn apply(&mut self, x: &mut [f32]) {
        if self.left == 0 && self.gain == 1.0 {
            return;
        }
        for v in x {
            if self.left > 0 {
                self.gain += self.step;
                self.left -= 1;
                if self.left == 0 {
                    self.gain = self.target_gain();
                }
            }
            *v = (f64::from(*v) * self.gain) as f32;
        }
    }
}

/// Parameters read once per `process` call.
#[derive(Debug, Clone, Copy)]
pub struct AudioParams {
    pub lf_crossover_hz: f64,
    /// Input trims of L and R in dB (0 = untouched).
    pub trim_db: [f64; 2],
}

impl Default for AudioParams {
    fn default() -> Self {
        Self {
            lf_crossover_hz: LowFreqStereo::DEFAULT_CROSSOVER_HZ,
            trim_db: [0.0; 2],
        }
    }
}

struct Inner {
    fs: f64,
    mono: bool,
    peak: [PeakMeter; 2],
    tp: [TruePeakMeter; 2],
    rms: [RmsMeter; 2],
    stats: [BlockStats; 2],
    stereo: StereoMeter,
    lf: LowFreqStereo,
    lf_hz: f64,
    tau: f32,
    vinyl: Vinyl,
    cascades: [Cascade; 4],
    riaa_index: usize,
    trim: [Trim; 2],
    gen: GenEngine,
    prod: Producer,
    sl: Vec<f64>,
    sr: Vec<f64>,
    g: Vec<f64>,
    e: Vec<f64>,
    pending: usize,
}

pub struct AudioPath {
    shared: Arc<Shared>,
    writer: Input<Readouts>,
    inner: Option<Inner>,
}

impl AudioPath {
    /// Not yet usable for metering: call [`AudioPath::prepare`]. Until then
    /// the buffers pass through untouched.
    pub fn new(shared: Arc<Shared>) -> Self {
        let writer = shared
            .take_live_writer()
            .expect("the live writer is taken once per Shared");
        Self {
            shared,
            writer,
            inner: None,
        }
    }

    /// Allocates and designs everything for `fs`. **Not realtime safe.**
    /// `riaa_pre` is the pre-emphasis cascade for `fs`
    /// (`RiaaFilters::design(fs).cascade(Mode::PreEmphasis, Options::default())`).
    pub fn prepare(&mut self, fs: f64, mono: bool, riaa_pre: Cascade, prod: Producer) {
        self.prepare_with_options(fs, mono, [riaa_pre; 4], prod);
    }

    /// Like [`AudioPath::prepare`] with the pre-emphasis cascades of all four
    /// option combinations, in the order of
    /// [`neo_measure::riaa::Options::ALL`]; the vinyl page selects among them
    /// ([`crate::settings::SettingsData::riaa_index`]).
    pub fn prepare_with_options(
        &mut self,
        fs: f64,
        mono: bool,
        cascades: [Cascade; 4],
        prod: Producer,
    ) {
        let st = self.shared.settings.get();
        let riaa_index = self.shared.settings.riaa_index();
        let tau = self.shared.settings.corr_tau_s();
        let lf_hz = LowFreqStereo::DEFAULT_CROSSOVER_HZ;
        self.inner = Some(Inner {
            fs,
            mono,
            peak: std::array::from_fn(|_| PeakMeter::new(fs, HOLD_S, DECAY_DB_S)),
            tp: std::array::from_fn(|_| TruePeakMeter::with_hold(fs, HOLD_S, DECAY_DB_S)),
            rms: std::array::from_fn(|_| RmsMeter::window(fs, RMS_S)),
            stats: Default::default(),
            stereo: StereoMeter::new(fs, f64::from(tau)),
            lf: LowFreqStereo::new(fs, lf_hz, f64::from(tau)),
            lf_hz,
            tau,
            vinyl: Vinyl::new(fs, cascades[riaa_index], f64::from(st.hf_corner_hz), CHUNK),
            cascades,
            riaa_index,
            trim: [Trim::UNITY; 2],
            gen: GenEngine::new(fs),
            prod,
            sl: vec![0.0; CHUNK],
            sr: vec![0.0; CHUNK],
            g: vec![0.0; CHUNK],
            e: vec![0.0; CHUNK],
            pending: 0,
        });
        self.shared.gen.force_off();
    }

    pub fn is_prepared(&self) -> bool {
        self.inner.is_some()
    }

    /// Drop the prepared state (and with it the ring producer).
    pub fn release(&mut self) {
        self.inner = None;
    }

    /// Publish the current meter values now (tests; `process` publishes on its
    /// own every [`PUBLISH_EVERY`] frames).
    pub fn publish(&mut self) {
        if let Some(i) = self.inner.as_mut() {
            i.publish(&mut self.writer);
        }
    }

    pub fn process_stereo(&mut self, l: &mut [f32], r: &mut [f32], p: &AudioParams) {
        self.run(l, Some(r), p);
    }

    pub fn process_mono(&mut self, m: &mut [f32], p: &AudioParams) {
        self.run(m, None, p);
    }

    fn run(&mut self, l: &mut [f32], mut r: Option<&mut [f32]>, p: &AudioParams) {
        let Some(inn) = self.inner.as_mut() else {
            return;
        };
        inn.apply_controls(&self.shared, p);
        let gs = self.shared.gen.get();
        let n = match &r {
            Some(r) => l.len().min(r.len()),
            None => l.len(),
        };
        let mut pos = 0;
        while pos < n {
            let end = (pos + CHUNK).min(n);
            let rc = r.as_deref_mut().map(|r| &mut r[pos..end]);
            inn.chunk(&mut l[pos..end], rc, &gs);
            pos = end;
        }
        inn.pending += n;
        if inn.pending >= PUBLISH_EVERY {
            inn.publish(&mut self.writer);
        }
    }
}

impl Inner {
    fn apply_controls(&mut self, shared: &Shared, p: &AudioParams) {
        let bits = shared.take_reset(reset::AUDIO);
        if bits & reset::LEVELS != 0 {
            for c in 0..2 {
                self.peak[c].reset();
                self.tp[c].reset_hold();
                self.rms[c].reset();
                self.stats[c].reset();
            }
        }
        if bits & reset::STEREO != 0 {
            self.stereo.reset();
            self.lf.reset();
        }
        if bits & reset::VINYL != 0 {
            self.vinyl.reset();
        }
        let tau = shared.settings.corr_tau_s();
        let lf_hz = if p.lf_crossover_hz.is_finite() {
            p.lf_crossover_hz.clamp(20.0, 0.45 * self.fs)
        } else {
            self.lf_hz
        };
        if tau != self.tau || (lf_hz - self.lf_hz).abs() > 1e-6 {
            self.tau = tau;
            self.stereo.set_time_constant(self.fs, f64::from(tau));
            if (lf_hz - self.lf_hz).abs() > 1e-6 {
                self.lf_hz = lf_hz;
            }
            // Construction is allocation free (plain structs); the filter
            // history restarts, which is what a crossover change implies.
            self.lf = LowFreqStereo::new(self.fs, self.lf_hz, f64::from(tau));
        }
        self.vinyl
            .set_corner(f64::from(shared.settings.hf_corner_hz()));
        let ri = shared.settings.riaa_index();
        if ri != self.riaa_index {
            self.riaa_index = ri;
            self.vinyl.set_cascade(self.cascades[ri]);
        }
        let fs = self.fs;
        for c in 0..2 {
            self.trim[c].set(p.trim_db[c], fs);
        }
        shared.set_applied_trim_db([self.trim[0].target_db, self.trim[1].target_db]);
    }

    fn chunk(&mut self, lc: &mut [f32], mut rc: Option<&mut [f32]>, gs: &GenSettings) {
        let m = lc.len();
        // 0. input trims (in place, before everything else)
        self.trim[0].apply(lc);
        if let Some(r) = rc.as_deref_mut() {
            self.trim[1].apply(r);
        }
        // 1. analysis ring gets the (trimmed) input
        match rc.as_deref() {
            Some(r) => self.prod.push(lc, r),
            None => self.prod.push(lc, lc),
        };
        // 2. f64 copies of the input for the meters
        for (o, &v) in self.sl.iter_mut().zip(lc.iter()) {
            *o = f64::from(v);
        }
        match rc.as_deref() {
            Some(r) => {
                for (o, &v) in self.sr.iter_mut().zip(r.iter()) {
                    *o = f64::from(v);
                }
            }
            None => {
                let (sl, sr) = (&self.sl, &mut self.sr);
                sr[..m].copy_from_slice(&sl[..m]);
            }
        }
        // 3. generator (output only)
        if self.gen.render(gs, &mut self.g[..m], &mut self.e[..m]) {
            let (gl, gr) = if self.mono {
                (1.0, 0.0)
            } else {
                GenEngine::gains(gs.routing)
            };
            GenEngine::mix(gs.mode, lc, &self.g[..m], &self.e[..m], gl);
            if let Some(r) = rc {
                GenEngine::mix(gs.mode, r, &self.g[..m], &self.e[..m], gr);
            }
        }
        // 4. meters
        let (sl, sr) = (&self.sl[..m], &self.sr[..m]);
        for c in 0..2 {
            let x = if c == 0 { sl } else { sr };
            self.peak[c].process(x);
            self.tp[c].process(x);
            self.rms[c].process(x);
            self.stats[c].process(x);
        }
        self.stereo.process(sl, sr);
        self.lf.process(sl, sr);
        self.vinyl.process(sl, sr);
    }

    fn publish(&mut self, w: &mut Input<Readouts>) {
        self.pending = 0;
        let mut lv = LevelsReadout::default();
        for c in 0..2 {
            lv.peak_db[c] = self.peak[c].peak_db();
            lv.peak_max_db[c] = self.peak[c].max_db();
            lv.tp_db[c] = amp_to_db(self.tp[c].true_peak());
            lv.tp_max_db[c] = amp_to_db(self.tp[c].max_true_peak());
            lv.rms_db[c] = self.rms[c].rms_db_sine_ref();
            lv.dc[c] = self.stats[c].dc_offset();
            lv.crest_db[c] = self.stats[c].crest_factor_db();
        }
        let s = &self.stereo;
        let lf = &self.lf;
        let st = StereoReadout {
            correlation: s.correlation(),
            balance_db: s.balance_db(),
            mid_db: s.mid_db() + SINE_REF_DB,
            side_db: s.side_db() + SINE_REF_DB,
            lf_correlation: lf.correlation(),
            lf_lateral_db: lf.lateral_db() + SINE_REF_DB,
            lf_vertical_db: lf.vertical_db() + SINE_REF_DB,
        };
        w.write(Readouts {
            levels: lv,
            stereo: st,
            vinyl: self.vinyl.readout(),
            generator_active: self.gen.is_active(),
        });
    }
}
