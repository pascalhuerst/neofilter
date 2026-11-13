//! Analysis pipeline: everything that is too heavy or allocates (loudness
//! gating statistics, goniometer, FFT spectra, latched tone / THD / wow &
//! flutter / crosstalk / noise measurements).
//!
//! [`Engine`] is plain single-threaded code driven by [`Engine::step`] and
//! [`Engine::publish`] so that the whole pipeline can be tested without
//! threads; [`AnalysisThread`] runs it in the background.
//!
//! # Thread behaviour
//!
//! The thread owns the consumer end of the sample ring and is the only writer
//! of the [`Snapshot`] the editor reads. It always drains the ring and runs
//! the (cheap) K-weighted loudness meter, because integrated loudness needs
//! every sample since the last reset. While no editor is open and no
//! measurement is latched it polls at 25 ms and publishes at 5 Hz; goniometer,
//! spectrum and capture are skipped entirely. With the editor open or a
//! measurement running it polls every 5 ms and publishes at 30 Hz.
//! Dropping an [`AnalysisThread`] stops and joins it.

use crate::ring::{self, Consumer, Producer};
use crate::settings::{SettingsData, SpecChannel};
use crate::shared::{reset, Shared};
use crate::testrec::{self, CapturedSweep, Mod, TestRecData};
use neo_measure::loudness::{LoudnessMeter, LoudnessRange};
use neo_measure::spectrum::{
    resample_log_power, smooth_octave_power, Averaging, Combine, SpectrumAnalyzer, SpectrumConfig,
};
use neo_measure::stereo::Goniometer;
use neo_measure::sweep_track::{PairStats, SweepConfig, SweepTracker};
use neo_measure::tone::{analyze_tone, channel_separation_db, ToneAnalysis, ToneConfig};
use neo_measure::util::pow_to_db;
use neo_measure::velocity::detect_modulation;
use neo_measure::weighting::{measure_noise, NoiseLevels};
use neo_measure::wow_flutter::{analyze_wow_flutter, WowFlutterConfig, WowFlutterResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Frames in the ring (about 5.5 s at 48 kHz, 1.4 s at 192 kHz).
pub const RING_FRAMES: usize = 1 << 18;
/// Goniometer points kept.
pub const GONIO_POINTS: usize = 2048;
/// Display points of the spectrum curves.
pub const SPECTRUM_POINTS: usize = 1024;
/// Longest capture (seconds) regardless of the settings.
pub const MAX_CAPTURE_S: f64 = 30.0;
const BLOCK: usize = 4096;
/// A peak-referenced sine is 3.01 dB above its RMS.
pub const SINE_PEAK_TO_RMS_DB: f64 = 3.010_299_956_639_812;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasureKind {
    /// Frequency, level, THD, THD+N of the selected channel.
    Tone,
    /// Wow and flutter of the selected channel.
    WowFlutter,
    /// Crosstalk with the tone on channel `0` (L, reads L->R) or `1` (R).
    Crosstalk(usize),
    /// Noise / rumble on both channels.
    Noise,
    /// Measure the tone level of the selected channel and latch it as the
    /// noise reference.
    Reference,
}

/// Measurements of the test-record page. Their parameters (durations,
/// profile, thresholds) are read from [`crate::testrec::TestRecState`] when
/// the command is handled; results are written back there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecCommand {
    /// Reference band the user plays (left-only, right-only, lateral,
    /// vertical).
    Ref(Mod),
    /// Tracking band with the given index in the profile's amplitude list.
    Tracking(usize),
    /// Wow and flutter of the profile's reference tone.
    WowFlutter,
    /// Start collecting the sweep response.
    SweepArm,
    /// Stop collecting and keep the result.
    SweepStop,
    /// Discard the collected / captured sweep.
    SweepClear,
    /// Abort a running reference / tracking / W&F capture.
    Cancel,
    ClearRef,
    ClearTracking,
    ClearWf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Start(MeasureKind),
    Cancel,
    ClearResults,
    Rec(RecCommand),
}

/// What a running test-record capture measures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecKind {
    Ref(Mod),
    /// Amplitude (um) and index in the profile list.
    Tracking(f64, usize),
    WowFlutter,
}

/// Live view of the sweep tracker while armed.
#[derive(Debug, Clone, Default)]
pub struct SweepLive {
    /// Centre frequency of every grid cell.
    pub grid_hz: Vec<f64>,
    /// Covered cells per channel: `(grid index, frequency, level dBFS)`.
    pub cells: [Vec<(u32, f64, f64)>; 2],
    /// Coverage flag per grid cell (channel 0).
    pub coverage: Vec<bool>,
    pub accepted: [u64; 2],
    pub candidates: [u64; 2],
    pub pair: Option<PairStats>,
}

/// Test-record page view of the analysis state.
#[derive(Debug, Clone, Default)]
pub struct RecSnapshot {
    /// Running capture and its progress `0..=1`.
    pub busy: Option<(RecKindView, f32)>,
    /// `Some` while the sweep tracker is armed.
    pub sweep: Option<SweepLive>,
    /// Last problem, empty if none.
    pub message: String,
}

/// [`RecKind`] with a default, for [`RecSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecKindView(pub RecKind);

impl Default for RecKindView {
    fn default() -> Self {
        Self(RecKind::WowFlutter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessReadout {
    pub momentary: f64,
    pub short_term: f64,
    pub max_momentary: f64,
    pub max_short_term: f64,
    pub integrated: Option<f64>,
    pub range: Option<LoudnessRange>,
}

impl Default for LoudnessReadout {
    fn default() -> Self {
        Self {
            momentary: f64::NEG_INFINITY,
            short_term: f64::NEG_INFINITY,
            max_momentary: f64::NEG_INFINITY,
            max_short_term: f64::NEG_INFINITY,
            integrated: None,
            range: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chan {
    Left,
    Right,
    Mid,
    Side,
}

impl Chan {
    pub fn name(self) -> &'static str {
        match self {
            Chan::Left => "L",
            Chan::Right => "R",
            Chan::Mid => "Mid",
            Chan::Side => "Side",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpectrumCurve {
    pub chan: Chan,
    /// Level in dBFS on the log grid (empty in RTA mode).
    pub db: Vec<f32>,
    /// Peak hold on the same grid (empty unless peak hold is on).
    pub peak_db: Vec<f32>,
    /// 1/3-octave bands `(nominal Hz, dB)` (empty unless RTA mode).
    pub bands: Vec<(f32, f32)>,
}

#[derive(Debug, Clone)]
pub struct SpectrumData {
    pub fft_size: usize,
    pub bin_hz: f64,
    /// Log display grid: `SPECTRUM_POINTS` points from `f_lo` to `f_hi`.
    pub f_lo: f64,
    pub f_hi: f64,
    pub frames: u64,
    pub rta: bool,
    pub curves: Vec<SpectrumCurve>,
}

impl SpectrumData {
    /// Frequency of display point `i`.
    pub fn grid_hz(&self, i: usize) -> f64 {
        let n = SPECTRUM_POINTS - 1;
        self.f_lo * (self.f_hi / self.f_lo).powf(i as f64 / n as f64)
    }
}

#[derive(Debug, Clone)]
pub struct ToneResult {
    pub channel: usize,
    pub analysis: ToneAnalysis,
}

#[derive(Debug, Clone)]
pub struct WfResult {
    pub channel: usize,
    pub nominal_hz: f64,
    pub result: WowFlutterResult,
}

#[derive(Debug, Clone, Copy)]
pub struct NoiseResult {
    /// Levels of L and R. The `*_rel_db` fields are relative to the RMS
    /// reference below (or to 0 dBFS when none is latched).
    pub levels: [NoiseLevels; 2],
    pub reference_rms_dbfs: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct Results {
    pub tone: Option<ToneResult>,
    pub wf: Option<WfResult>,
    /// Channel separation in dB: `[L driven -> R, R driven -> L]`.
    pub xtalk: [Option<f64>; 2],
    pub noise: Option<NoiseResult>,
    /// Latched reference tone level (peak dBFS).
    pub reference: Option<f64>,
    /// Last problem ("no tone found", ...), empty if none.
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub seq: u64,
    pub sample_rate: f64,
    pub channels: usize,
    /// Frames the audio thread had to drop because this thread fell behind.
    pub overruns: u64,
    pub loudness: LoudnessReadout,
    /// `[(L-R)/sqrt2, (L+R)/sqrt2]` points, oldest first.
    pub gonio: Vec<[f32; 2]>,
    pub spectrum: Option<Arc<SpectrumData>>,
    /// Running measurement and its progress `0..=1`.
    pub measuring: Option<(MeasureKind, f32)>,
    pub results: Results,
    pub rec: RecSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SpecKey {
    fft: usize,
    window: crate::settings::WindowKind,
    avg: Averaging,
    chan: SpecChannel,
}

struct SpecState {
    key: SpecKey,
    an: Vec<(Chan, SpectrumAnalyzer)>,
}

struct RecCapture {
    kind: RecKind,
    target: usize,
    seconds: f64,
    trims: [f64; 2],
    l: Vec<f32>,
    r: Vec<f32>,
}

struct Capture {
    kind: MeasureKind,
    target: usize,
    l: Vec<f32>,
    r: Vec<f32>,
}

pub struct Engine {
    shared: Arc<Shared>,
    cons: Consumer,
    fs: f64,
    channels: usize,
    loud: LoudnessMeter,
    gonio: Goniometer,
    spec: Option<SpecState>,
    spec_data: Option<Arc<SpectrumData>>,
    spec_dirty: bool,
    capture: Option<Capture>,
    rec_cap: Option<RecCapture>,
    sweep: Option<SweepTracker>,
    sweep_trim: [f64; 2],
    rec_message: String,
    results: Results,
    loud_readout: LoudnessReadout,
    loud_stats_at: Option<Instant>,
    seq: u64,
    bl: Vec<f32>,
    br: Vec<f32>,
    ms: Vec<f64>,
    ss: Vec<f64>,
    cmds: Vec<Command>,
}

impl Engine {
    pub fn new(shared: Arc<Shared>, cons: Consumer, fs: f64, channels: usize) -> Self {
        let ch = channels.clamp(1, 2);
        Self {
            shared,
            cons,
            fs,
            channels: ch,
            loud: LoudnessMeter::new(fs, ch),
            gonio: Goniometer::new(GONIO_POINTS, 2),
            spec: None,
            spec_data: None,
            spec_dirty: true,
            capture: None,
            rec_cap: None,
            sweep: None,
            sweep_trim: [0.0; 2],
            rec_message: String::new(),
            results: Results::default(),
            loud_readout: LoudnessReadout::default(),
            loud_stats_at: None,
            seq: 0,
            bl: vec![0.0; BLOCK],
            br: vec![0.0; BLOCK],
            ms: vec![0.0; BLOCK],
            ss: vec![0.0; BLOCK],
            cmds: Vec::new(),
        }
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Heavy analyses run while the editor is open or a measurement runs.
    pub fn is_active(&self) -> bool {
        self.shared.ui_active()
            || self.capture.is_some()
            || self.rec_cap.is_some()
            || self.sweep.is_some()
    }

    fn handle_commands(&mut self, st: &SettingsData) {
        self.cmds.clear();
        self.shared.drain_commands(&mut self.cmds);
        let cmds = std::mem::take(&mut self.cmds);
        for c in &cmds {
            match *c {
                Command::Start(kind) => {
                    let dur = match kind {
                        MeasureKind::Tone | MeasureKind::Crosstalk(_) => st.tone_duration_s,
                        MeasureKind::WowFlutter => st.wf_duration_s,
                        MeasureKind::Noise => st.noise_duration_s,
                        MeasureKind::Reference => 1.0,
                    }
                    .clamp(0.25, MAX_CAPTURE_S);
                    let target = (dur * self.fs) as usize;
                    self.results.message.clear();
                    self.capture = Some(Capture {
                        kind,
                        target,
                        l: Vec::with_capacity(target),
                        r: Vec::with_capacity(target),
                    });
                }
                Command::Cancel => self.capture = None,
                Command::ClearResults => self.results = Results::default(),
                Command::Rec(c) => self.handle_rec(c),
            }
        }
        self.cmds = cmds;
        let bits = self
            .shared
            .take_reset(reset::LOUDNESS | reset::SPECTRUM | reset::SPEC_PEAK);
        if bits & reset::LOUDNESS != 0 {
            self.loud.reset();
            self.loud_readout = LoudnessReadout::default();
            self.loud_stats_at = None;
        }
        if bits & reset::SPECTRUM != 0 {
            if let Some(s) = self.spec.as_mut() {
                s.an.iter_mut().for_each(|(_, a)| a.reset());
            }
            self.spec_dirty = true;
        }
        if bits & reset::SPEC_PEAK != 0 {
            if let Some(s) = self.spec.as_mut() {
                s.an.iter_mut().for_each(|(_, a)| a.reset_peak());
            }
            self.spec_dirty = true;
        }
    }

    fn ensure_spectrum(&mut self, st: &SettingsData) {
        let key = SpecKey {
            fft: st.fft_size,
            window: st.window,
            avg: st.averaging_mode(),
            chan: st.spec_channel,
        };
        if self.spec.as_ref().is_some_and(|s| s.key == key) {
            return;
        }
        let chans: &[Chan] = match key.chan {
            SpecChannel::Left => &[Chan::Left],
            SpecChannel::Right => &[Chan::Right],
            SpecChannel::Mid => &[Chan::Mid],
            SpecChannel::Side => &[Chan::Side],
            SpecChannel::Both => &[Chan::Left, Chan::Right],
        };
        let mut an = Vec::new();
        for &c in chans {
            let mut cfg = SpectrumConfig::new(self.fs, key.fft, key.window.window());
            cfg.overlap = 0.75;
            cfg.averaging = key.avg;
            if let Some(a) = SpectrumAnalyzer::new(cfg) {
                an.push((c, a));
            }
        }
        self.spec = Some(SpecState { key, an });
        self.spec_dirty = true;
    }

    /// Pop and process up to one block of frames; returns the frames handled
    /// (0 = the ring was empty).
    pub fn step(&mut self) -> usize {
        let st = self.shared.settings.get();
        self.handle_commands(&st);
        let n = self.cons.pop(&mut self.bl, &mut self.br);
        if n == 0 {
            return 0;
        }
        for v in self.bl[..n].iter_mut().chain(self.br[..n].iter_mut()) {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
        let ui = self.shared.ui_active();
        if ui {
            self.ensure_spectrum(&st);
        }
        let (l, r) = (&self.bl[..n], &self.br[..n]);
        if self.channels == 1 {
            self.loud.process_mono(l);
        } else {
            self.loud.process_stereo(l, r);
        }
        if ui {
            self.gonio.process(l, r);
            if let Some(spec) = self.spec.as_mut() {
                for i in 0..n {
                    let (a, b) = (f64::from(l[i]), f64::from(r[i]));
                    self.ms[i] = 0.5 * (a + b);
                    self.ss[i] = 0.5 * (a - b);
                }
                for (c, an) in spec.an.iter_mut() {
                    match c {
                        Chan::Left => an.push(l),
                        Chan::Right => an.push(r),
                        Chan::Mid => an.push(&self.ms[..n]),
                        Chan::Side => an.push(&self.ss[..n]),
                    };
                }
                self.spec_dirty = true;
            }
        }
        if let Some(tr) = self.sweep.as_mut() {
            tr.push(l, Some(r));
        }
        let mut rec_done = false;
        if let Some(cap) = self.rec_cap.as_mut() {
            let take = n.min(cap.target - cap.l.len());
            cap.l.extend_from_slice(&l[..take]);
            cap.r.extend_from_slice(&r[..take]);
            rec_done = cap.l.len() >= cap.target;
        }
        if let Some(cap) = self.capture.as_mut() {
            let take = n.min(cap.target - cap.l.len());
            cap.l.extend_from_slice(&l[..take]);
            cap.r.extend_from_slice(&r[..take]);
            if cap.l.len() >= cap.target {
                if let Some(cap) = self.capture.take() {
                    self.finish_capture(cap, &st);
                }
            }
        }
        if rec_done {
            if let Some(cap) = self.rec_cap.take() {
                self.finish_rec(cap);
            }
        }
        n
    }

    fn finish_capture(&mut self, cap: Capture, st: &SettingsData) {
        let ch = usize::from(st.tone_right);
        let sel = if ch == 0 { &cap.l } else { &cap.r };
        let to64 = |x: &[f32]| x.iter().map(|&v| f64::from(v)).collect::<Vec<f64>>();
        self.results.message.clear();
        match cap.kind {
            MeasureKind::Tone => {
                let x = to64(sel);
                match analyze_tone(&x, &ToneConfig::new(self.fs)) {
                    Some(a) => {
                        self.results.tone = Some(ToneResult {
                            channel: ch,
                            analysis: a,
                        })
                    }
                    None => self.results.message = "No tone found (silent input?)".into(),
                }
            }
            MeasureKind::Reference => {
                let x = to64(sel);
                match analyze_tone(&x, &ToneConfig::new(self.fs)) {
                    Some(a) => {
                        self.results.reference = Some(a.level_dbfs);
                        self.shared
                            .settings
                            .update(|d| d.noise_ref_dbfs = Some(a.level_dbfs));
                    }
                    None => self.results.message = "No tone found for the reference".into(),
                }
            }
            MeasureKind::WowFlutter => {
                let x = to64(sel);
                let cfg = WowFlutterConfig::new(self.fs, st.wf_nominal_hz);
                match analyze_wow_flutter(&x, &cfg) {
                    Some(r) => {
                        self.results.wf = Some(WfResult {
                            channel: ch,
                            nominal_hz: st.wf_nominal_hz,
                            result: r,
                        })
                    }
                    None => {
                        self.results.message =
                            "Wow & flutter needs a clean 3150/3000 Hz tone and a longer capture"
                                .into()
                    }
                }
            }
            MeasureKind::Crosstalk(drive) => {
                let (d, o) = if drive == 0 {
                    (to64(&cap.l), to64(&cap.r))
                } else {
                    (to64(&cap.r), to64(&cap.l))
                };
                match channel_separation_db(&d, &o, self.fs) {
                    Some(v) => self.results.xtalk[drive.min(1)] = Some(v),
                    None => self.results.message = "No tone on the driven channel".into(),
                }
            }
            MeasureKind::Noise => {
                let reference = st.noise_ref_dbfs.map(|p| p - SINE_PEAK_TO_RMS_DB);
                let mk = |x: &[f32]| {
                    measure_noise(
                        x,
                        self.fs,
                        st.noise_band_low,
                        st.noise_band_high,
                        reference.unwrap_or(0.0),
                    )
                };
                match (mk(&cap.l), mk(&cap.r)) {
                    (Some(a), Some(b)) => {
                        self.results.noise = Some(NoiseResult {
                            levels: [a, b],
                            reference_rms_dbfs: reference,
                        })
                    }
                    _ => self.results.message = "Noise capture too short".into(),
                }
            }
        }
    }

    fn handle_rec(&mut self, c: RecCommand) {
        let data = self.shared.testrec.get();
        let start = |kind: RecKind, seconds: f64, fs: f64, trims: [f64; 2]| {
            let seconds = seconds.clamp(0.5, MAX_CAPTURE_S);
            let target = (seconds * fs) as usize;
            RecCapture {
                kind,
                target,
                seconds,
                trims,
                l: Vec::with_capacity(target),
                r: Vec::with_capacity(target),
            }
        };
        let trims = self.shared.applied_trim_db();
        self.rec_message.clear();
        match c {
            RecCommand::Ref(slot) => {
                self.rec_cap = Some(start(RecKind::Ref(slot), data.ref_seconds, self.fs, trims));
            }
            RecCommand::Tracking(i) => {
                let amps = &data.profile().track_amps_um;
                if let Some(&a) = amps.get(i) {
                    self.rec_cap = Some(start(
                        RecKind::Tracking(a, i),
                        data.track_seconds,
                        self.fs,
                        trims,
                    ));
                }
            }
            RecCommand::WowFlutter => {
                self.rec_cap = Some(start(RecKind::WowFlutter, data.wf_seconds, self.fs, trims));
            }
            RecCommand::Cancel => self.rec_cap = None,
            RecCommand::SweepArm => {
                let p = data.profile();
                let f_hi = (0.45 * self.fs).min(40_000.0);
                let mut cfg = SweepConfig::new(self.fs);
                cfg.f_lo = (0.5 * p.sweep_lo_hz).clamp(20.0, f_hi / 4.0);
                cfg.f_hi = f_hi;
                cfg.min_snr_db = data.min_snr_db;
                self.sweep = SweepTracker::new(cfg, 2);
                self.sweep_trim = trims;
                if self.sweep.is_none() {
                    self.rec_message = "Sweep tracking is not available at this sample rate".into();
                }
            }
            RecCommand::SweepStop => self.finish_sweep(),
            RecCommand::SweepClear => {
                self.sweep = None;
                self.shared.testrec.update(|d| d.sweep = None);
            }
            RecCommand::ClearRef => self.shared.testrec.update(|d| d.ref_bands.clear()),
            RecCommand::ClearTracking => self.shared.testrec.update(|d| d.tracking.clear()),
            RecCommand::ClearWf => self.shared.testrec.update(|d| d.wf = None),
        }
    }

    /// Stop the sweep tracker and store its result in the persisted state.
    fn finish_sweep(&mut self) {
        let Some(tr) = self.sweep.take() else {
            return;
        };
        let cells: [Vec<(u32, f64, f64)>; 2] = std::array::from_fn(|c| {
            tr.curve(c)
                .iter()
                .map(|k| (k.index as u32, k.freq_hz, k.level_db))
                .collect()
        });
        if cells[0].is_empty() && cells[1].is_empty() {
            self.rec_message =
                "No clean tone was found in the sweep (is the right band playing?)".into();
            return;
        }
        let pair = tr.pair();
        let detected = pair
            .filter(|p| p.phase_coherence > 0.5)
            .and_then(|p| detect_modulation(p.level_diff_db, p.phase_deg))
            .map(Mod::from_m);
        let sw = CapturedSweep {
            cells,
            trim_db: self.sweep_trim,
            detected,
            level_diff_db: pair.map(|p| p.level_diff_db),
            phase_deg: pair.map(|p| p.phase_deg),
            pair_blocks: pair.map_or(0, |p| p.blocks),
        };
        self.shared.testrec.update(|d| d.sweep = Some(sw));
    }

    fn finish_rec(&mut self, cap: RecCapture) {
        let to64 = |x: &[f32]| x.iter().map(|&v| f64::from(v)).collect::<Vec<f64>>();
        let (l, r) = (to64(&cap.l), to64(&cap.r));
        let fs = self.fs;
        let data: TestRecData = self.shared.testrec.get();
        match cap.kind {
            RecKind::Ref(slot) => {
                match testrec::measure_ref_band(&l, &r, fs, slot, cap.seconds, cap.trims) {
                    Some(b) => self.shared.testrec.update(|d| d.set_ref_band(b)),
                    None => {
                        self.rec_message =
                            "No tone found (silent input, or the wrong channel?)".into()
                    }
                }
            }
            RecKind::Tracking(amp, _) => match testrec::measure_track_row(&l, &r, fs, amp) {
                Some(row) => self.shared.testrec.update(|d| {
                    d.set_track_row(row);
                    if d.track_auto {
                        let n = d.profile().track_amps_um.len();
                        d.track_sel = (d.track_sel + 1).min(n.saturating_sub(1));
                    }
                }),
                None => self.rec_message = "No tone found for the tracking band".into(),
            },
            RecKind::WowFlutter => {
                let ch = data.wf_channel.min(1);
                let x = if ch == 0 { &l } else { &r };
                match testrec::measure_wf(x, fs, data.profile().ref_freq_hz, ch) {
                    Some(w) => self.shared.testrec.update(|d| d.wf = Some(w)),
                    None => {
                        self.rec_message =
                            "Wow & flutter needs a clean carrier tone and a longer capture".into()
                    }
                }
            }
        }
    }

    fn sweep_live(&self) -> Option<SweepLive> {
        let tr = self.sweep.as_ref()?;
        let n = tr.cell_count();
        let st = tr.stats();
        Some(SweepLive {
            grid_hz: (0..n).map(|i| tr.grid_hz(i)).collect(),
            cells: std::array::from_fn(|c| {
                tr.curve(c)
                    .iter()
                    .map(|k| (k.index as u32, k.freq_hz, k.level_db))
                    .collect()
            }),
            coverage: tr.coverage(0),
            accepted: st.accepted,
            candidates: st.candidates,
            pair: tr.pair(),
        })
    }

    fn build_spectrum(&self, st: &SettingsData) -> Option<SpectrumData> {
        let spec = self.spec.as_ref()?;
        let first = &spec.an.first()?.1;
        if first.frames() == 0 {
            return None;
        }
        let nyq = self.fs / 2.0;
        let f_lo = f64::from(st.f_lo).clamp(1.0, nyq * 0.5);
        let f_hi = f64::from(st.f_hi).clamp(f_lo * 1.5, nyq * 0.999);
        let bin_hz = first.bin_hz();
        let mut curves = Vec::new();
        for (c, an) in &spec.an {
            let mut curve = SpectrumCurve {
                chan: *c,
                db: Vec::new(),
                peak_db: Vec::new(),
                bands: Vec::new(),
            };
            if st.rta {
                curve.bands = an
                    .third_octave_db(20.0, 20_000.0)
                    .into_iter()
                    .map(|b| (b.nominal as f32, b.level_db as f32))
                    .collect();
            } else {
                let to_db = |power: Vec<f64>| -> Vec<f32> {
                    let p = if st.smoothing > 0 {
                        smooth_octave_power(&power, st.smoothing)
                    } else {
                        power
                    };
                    resample_log_power(&p, bin_hz, f_lo, f_hi, SPECTRUM_POINTS, Combine::Mean)
                        .into_iter()
                        .map(|v| pow_to_db(v) as f32)
                        .collect()
                };
                curve.db = to_db(an.amplitude_power());
                if st.peak_hold {
                    let p: Vec<f64> = an
                        .peak_amplitude_db()
                        .into_iter()
                        .map(|d| 10f64.powf(d / 10.0))
                        .collect();
                    curve.peak_db = to_db(p);
                }
            }
            curves.push(curve);
        }
        Some(SpectrumData {
            fft_size: first.config().fft_size,
            bin_hz,
            f_lo,
            f_hi,
            frames: first.frames(),
            rta: st.rta,
            curves,
        })
    }

    /// Build and publish a new [`Snapshot`].
    pub fn publish(&mut self) {
        let st = self.shared.settings.get();
        let ui = self.shared.ui_active();
        // Gating statistics allocate: at most twice a second.
        let stats_due = self
            .loud_stats_at
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        self.loud_readout.momentary = self.loud.momentary_lufs();
        self.loud_readout.short_term = self.loud.short_term_lufs();
        self.loud_readout.max_momentary = self.loud.max_momentary_lufs();
        self.loud_readout.max_short_term = self.loud.max_short_term_lufs();
        if stats_due {
            self.loud_readout.integrated = self.loud.integrated_lufs();
            self.loud_readout.range = self.loud.loudness_range();
            self.loud_stats_at = Some(Instant::now());
        }
        let mut gonio = Vec::new();
        if ui {
            gonio = vec![[0.0f32; 2]; self.gonio.len()];
            let n = self.gonio.copy_points(&mut gonio);
            gonio.truncate(n);
            if self.spec_dirty {
                self.spec_data = self.build_spectrum(&st).map(Arc::new);
                self.spec_dirty = false;
            }
        }
        self.seq += 1;
        let measuring = self
            .capture
            .as_ref()
            .map(|c| (c.kind, c.l.len() as f32 / c.target.max(1) as f32));
        self.shared.publish_snapshot(Snapshot {
            seq: self.seq,
            sample_rate: self.fs,
            channels: self.channels,
            overruns: self.cons.overruns(),
            loudness: self.loud_readout,
            gonio,
            spectrum: if ui { self.spec_data.clone() } else { None },
            measuring,
            results: self.results.clone(),
            rec: RecSnapshot {
                busy: self.rec_cap.as_ref().map(|c| {
                    (
                        RecKindView(c.kind),
                        c.l.len() as f32 / c.target.max(1) as f32,
                    )
                }),
                sweep: if ui { self.sweep_live() } else { None },
                message: self.rec_message.clone(),
            },
        });
    }

    /// Process everything currently in the ring and publish (tests).
    pub fn pump(&mut self) {
        while self.step() > 0 {}
        self.publish();
    }
}

/// Background analysis thread (stopped and joined on drop).
pub struct AnalysisThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AnalysisThread {
    /// Create the ring and start the thread. Returns the audio-side producer.
    /// `None` for the thread handle only if the OS refused to spawn (the
    /// plugin then just has no analysis pages).
    pub fn spawn(shared: Arc<Shared>, fs: f64, channels: usize) -> (Self, Producer) {
        let (prod, cons) = ring::channel(RING_FRAMES);
        let mut engine = Engine::new(shared, cons, fs, channels);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::Builder::new()
            .name("neometer-analysis".into())
            .spawn(move || {
                let mut last = Instant::now();
                while !stop2.load(Ordering::Relaxed) {
                    let active = engine.is_active();
                    let did = engine.step();
                    let period = if active {
                        Duration::from_millis(33)
                    } else {
                        Duration::from_millis(200)
                    };
                    if last.elapsed() >= period {
                        engine.publish();
                        last = Instant::now();
                    }
                    if did == 0 {
                        let nap = if active { 5 } else { 25 };
                        std::thread::park_timeout(Duration::from_millis(nap));
                    }
                }
            })
            .ok();
        (Self { stop, handle }, prod)
    }

    /// True while the thread is running.
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for AnalysisThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}
