//! The measure / fit / apply / auto state machine.
//!
//! A [`Session`] lives on the control side (editor, tests, CLI-less hosts). It
//! owns the capture of the running measurement, drains the audio thread's
//! lock-free ring ([`CalLink`]) when polled, runs the analysis and the fit on
//! a worker thread (or inline in tests), and hands back band arrays that the
//! caller must write through `ParamSetter`. It never touches the audio thread
//! except through the atomics of [`CalLink`].
//!
//! Safety rules enforced here:
//!
//! * nothing starts without an explicit call (`start_measure`, `start_auto`);
//! * a failed measurement leaves no measurement and no plan behind, so
//!   nothing can be applied from it;
//! * `apply` refuses when the bands changed since the measurement;
//! * the capture is stopped on clipping (as soon as it is seen), on overrun,
//!   when the audio engine stalls, and when bypass is switched on;
//! * auto mode stops on any error and when the loop gets worse.

use super::analysis::{
    analyze_capture, estimate_loop_delay_window, level_stats, CalError, Capture, Measurement,
    SKIP_S,
};
use super::fit::{apply_bands, deviation_curve, deviation_stats, plan_fit, resolve_mask, FitPlan};
use super::settings::{CalPersist, CalSettings, LoopSummary};
use super::tap::CalLink;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Probe window for the live delay estimate.
const PROBE_WINDOW_S: f64 = 1.5;
/// Seconds between live delay probes.
const PROBE_STEP_S: f64 = 0.5;
/// Wall-clock time without audio progress after which a measurement is aborted.
const STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// What the session needs to know about the plugin at every call.
#[derive(Clone, Copy)]
pub struct Env<'a> {
    pub bands: &'a [BandParams; NUM_BANDS],
    pub gain_scale: f64,
    pub bypass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Measuring,
    Analyzing,
    Fitting,
    /// Auto mode: waiting for the changed EQ to show up in the loop.
    Settling,
}

/// One line of the iteration history.
#[derive(Debug, Clone, PartialEq)]
pub struct IterationRecord {
    /// 1-based measurement number of this run.
    pub index: usize,
    pub delay_ms: [Option<f64>; 2],
    /// In-range deviation of the loop from flat, measured at this iteration.
    pub rms_db: f64,
    pub max_db: f64,
    /// Calibration bands in use after this iteration's apply (0 if not applied).
    pub bands: usize,
    pub applied: bool,
}

/// Placeholder kept for API symmetry with GUI code.
pub type Action = Option<[BandParams; NUM_BANDS]>;

enum JobResult {
    Analysis(Box<Result<Measurement, CalError>>),
    Plan(Box<Result<FitPlan, String>>),
}

struct Run {
    settings: CalSettings,
    sample_rate: f64,
    /// Next capture length (frames) at which to probe the delay.
    next_probe: usize,
    delay_known: bool,
    last_audio: u64,
    last_progress: Instant,
}

/// Control-side calibration state machine.
pub struct Session {
    link: Arc<CalLink>,
    threaded: bool,
    phase: Phase,
    auto: bool,
    run: Option<Run>,
    capture: Capture,
    job: Option<Receiver<JobResult>>,
    measurement: Option<Measurement>,
    plan: Option<FitPlan>,
    history: Vec<IterationRecord>,
    iterations: usize,
    status: String,
    error: Option<CalError>,
    settle_until: u64,
    stop_requested: bool,
}

impl Session {
    pub fn new(link: Arc<CalLink>) -> Self {
        Self {
            link,
            threaded: true,
            phase: Phase::Idle,
            auto: false,
            run: None,
            capture: Capture::default(),
            job: None,
            measurement: None,
            plan: None,
            history: Vec::new(),
            iterations: 0,
            status: "Idle".into(),
            error: None,
            settle_until: 0,
            stop_requested: false,
        }
    }

    /// Run analysis and fit inline instead of on a worker thread (tests).
    pub fn set_threaded(&mut self, threaded: bool) {
        self.threaded = threaded;
    }

    pub fn link(&self) -> &Arc<CalLink> {
        &self.link
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn is_auto(&self) -> bool {
        self.auto
    }
    /// Anything running (measuring, analysing, settling).
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn error(&self) -> Option<&CalError> {
        self.error.as_ref()
    }
    pub fn measurement(&self) -> Option<&Measurement> {
        self.measurement.as_ref()
    }
    pub fn plan(&self) -> Option<&FitPlan> {
        self.plan.as_ref()
    }
    pub fn history(&self) -> &[IterationRecord] {
        &self.history
    }
    /// Fits applied in the current (or last) run.
    pub fn iterations(&self) -> usize {
        self.iterations
    }
    /// Capture progress `(captured, total)` in frames while measuring.
    pub fn progress(&self) -> (u64, u64) {
        (self.link.captured_frames(), self.link.max_frames())
    }
    /// Noise is audible right now.
    pub fn noise_on(&self) -> bool {
        self.link.noise_active()
    }

    fn fail(&mut self, e: CalError) {
        self.link.stop();
        self.status = e.to_string();
        self.error = Some(e);
        self.phase = Phase::Idle;
        self.auto = false;
        self.run = None;
        self.job = None;
        self.measurement = None;
        self.plan = None;
        self.capture = Capture::default();
    }

    /// Stop everything: noise fades out, nothing is applied, state is kept
    /// except the unfinished capture.
    pub fn stop(&mut self) {
        let was_busy = self.busy() || self.link.is_running();
        self.link.stop();
        self.stop_requested = true;
        self.phase = Phase::Idle;
        self.auto = false;
        self.run = None;
        self.job = None;
        self.capture = Capture::default();
        if was_busy {
            self.status = "Stopped".into();
            self.error = Some(CalError::Stopped);
        }
    }

    /// Begin one measurement (explicit user action).
    pub fn start_measure(&mut self, p: &CalPersist, env: &Env) -> Result<(), String> {
        self.begin(p, env, false)
    }

    /// Begin the Measure -> Fit -> Apply loop.
    pub fn start_auto(&mut self, p: &CalPersist, env: &Env) -> Result<(), String> {
        self.history.clear();
        self.iterations = 0;
        self.begin(p, env, true)
    }

    fn begin(&mut self, p: &CalPersist, env: &Env, auto: bool) -> Result<(), String> {
        if self.busy() {
            return Err("calibration is already running".into());
        }
        if env.bypass {
            return Err("switch bypass off before measuring".into());
        }
        if !(env.gain_scale.is_finite() && env.gain_scale >= 0.05) {
            return Err("gain scale is too small for calibration".into());
        }
        let mut s = p.settings.clone();
        s.sanitize();
        let fs = self.link.sample_rate();
        // Until the delay is known, capture up to the probe horizon.
        let horizon = (SKIP_S + PROBE_WINDOW_S + s.max_delay_s + 0.5) * fs;
        self.capture = Capture::default();
        self.capture.reserve(horizon as usize);
        self.measurement = None;
        self.plan = None;
        self.error = None;
        self.stop_requested = false;
        self.run = Some(Run {
            next_probe: ((SKIP_S + PROBE_WINDOW_S + 0.5) * fs) as usize,
            settings: s.clone(),
            sample_rate: fs,
            delay_known: false,
            last_audio: self.link.audio_frames(),
            last_progress: Instant::now(),
        });
        self.auto = auto;
        self.phase = Phase::Measuring;
        self.status = "Measuring: noise on, waiting for the return...".into();
        self.link
            .start(s.noise, s.routing, s.mode, s.level_db, horizon as u64);
        Ok(())
    }

    /// Fit the last measurement with the current settings (manual mode).
    pub fn fit(&mut self, p: &CalPersist, env: &Env) -> Result<(), String> {
        if self.busy() {
            return Err("busy".into());
        }
        let m = self
            .measurement
            .clone()
            .ok_or("no measurement yet: press Measure first")?;
        self.start_fit_job(m, p, env);
        Ok(())
    }

    fn start_fit_job(&mut self, m: Measurement, p: &CalPersist, env: &Env) {
        let bands = *env.bands;
        let (mask, gs) = (p.band_mask, env.gain_scale);
        let mut s = p.settings.clone();
        s.sanitize();
        let corr = p.correction.clone();
        self.phase = Phase::Fitting;
        self.status = "Fitting...".into();
        self.spawn(move || {
            JobResult::Plan(Box::new(plan_fit(&m, &bands, mask, gs, &s, corr.as_ref())))
        });
    }

    fn spawn(&mut self, f: impl FnOnce() -> JobResult + Send + 'static) {
        let (tx, rx) = channel();
        if self.threaded {
            std::thread::spawn(move || {
                let _ = tx.send(f());
            });
        } else {
            let _ = tx.send(f());
        }
        self.job = Some(rx);
    }

    /// Manual Apply: the bands to write (full array; unused calibration
    /// slots come back as unused). Fails when there is no plan or the bands
    /// changed since the measurement.
    pub fn apply(
        &mut self,
        p: &mut CalPersist,
        env: &Env,
    ) -> Result<[BandParams; NUM_BANDS], String> {
        if self.busy() {
            return Err("busy".into());
        }
        let out = self.apply_plan(p, env)?;
        self.status = format!(
            "Applied {} calibration bands",
            self.plan.as_ref().map_or(0, |pl| pl.bands.len())
        );
        Ok(out)
    }

    fn apply_plan(
        &mut self,
        p: &mut CalPersist,
        env: &Env,
    ) -> Result<[BandParams; NUM_BANDS], String> {
        let plan = self
            .plan
            .as_ref()
            .ok_or("no fit to apply: press Fit first")?;
        if plan.snapshot != *env.bands || (plan.gain_scale - env.gain_scale).abs() > 1e-9 {
            return Err("bands changed since the measurement: measure again".into());
        }
        if env.bypass {
            return Err("bypass is on".into());
        }
        let out = apply_bands(env.bands, plan.mask, &plan.bands)
            .ok_or("fitted bands do not fit into the calibration slots")?;
        let n = plan.bands.len();
        let (_, keep) = resolve_mask(env.bands, p.band_mask, p.settings.replace_all);
        p.band_mask = keep;
        if let Some(r) = self.history.last_mut() {
            if !r.applied {
                r.applied = true;
                r.bands = n;
            }
        }
        // The fit belongs to the old loop: it must not be applied twice.
        self.plan = None;
        self.measurement = None;
        Ok(out)
    }

    /// Unuse every calibration band (and forget the ownership mask).
    pub fn clear_calibration(&mut self, p: &mut CalPersist, env: &Env) -> [BandParams; NUM_BANDS] {
        let mask = if p.settings.replace_all {
            u32::MAX
        } else {
            p.band_mask
        };
        let mut out = *env.bands;
        for (i, b) in out.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                *b = BandParams::default();
            }
        }
        p.band_mask = 0;
        self.plan = None;
        self.measurement = None;
        self.status = "Calibration bands cleared".into();
        out
    }

    /// Advance the state machine. Call regularly (editor frame, test loop).
    /// Returns bands to write when auto mode applies a fit.
    pub fn poll(&mut self, p: &mut CalPersist, env: &Env) -> Action {
        match self.phase {
            Phase::Idle => None,
            Phase::Measuring => {
                self.poll_measuring(env);
                None
            }
            Phase::Analyzing | Phase::Fitting => self.poll_job(p, env),
            Phase::Settling => {
                if self.link.audio_frames() >= self.settle_until {
                    self.phase = Phase::Idle;
                    if let Err(e) = self.begin(p, env, true) {
                        self.fail(CalError::Other(e));
                    }
                }
                None
            }
        }
    }

    fn poll_measuring(&mut self, env: &Env) {
        let Some((fs, s, delay_known)) = self
            .run
            .as_ref()
            .map(|r| (r.sample_rate, r.settings.clone(), r.delay_known))
        else {
            return;
        };
        // finished() before draining: everything pushed before it is in the ring
        let finished = self.link.finished();
        while let Some(f) = self.link.pop() {
            self.capture.push(f);
        }
        if env.bypass {
            return self.fail(CalError::Other("bypass was switched on".into()));
        }
        // stall watchdog
        let audio = self.link.audio_frames();
        let mut stalled = false;
        if let Some(r) = self.run.as_mut() {
            if audio != r.last_audio {
                r.last_audio = audio;
                r.last_progress = Instant::now();
            } else if r.last_progress.elapsed() > STALL_TIMEOUT && !finished {
                stalled = true;
            }
        }
        if stalled {
            return self.fail(CalError::Other("the audio engine is not running".into()));
        }
        let n = self.capture.len();
        // clipping: stop immediately
        if n > 0 {
            let from = n.saturating_sub(4096);
            let peak = (0..2)
                .map(|c| level_stats(&self.capture.ret[c][from..n]).1)
                .fold(-400.0, f64::max);
            if peak >= s.clip_db {
                return self.fail(CalError::ReturnClipped(peak));
            }
        }
        if self.link.overrun() {
            return self.fail(CalError::CaptureOverrun);
        }
        if finished {
            if self.link.captured_frames() < self.link.max_frames() {
                return self.fail(CalError::Other("measurement interrupted".into()));
            }
            if !delay_known {
                // Horizon reached without a confident delay: report why.
                return self.fail(classify_failure(&self.capture, &s));
            }
            return self.finish_capture();
        }
        // live delay probe
        let due = self
            .run
            .as_ref()
            .is_some_and(|r| !r.delay_known && n >= r.next_probe);
        if due {
            if let Some(r) = self.run.as_mut() {
                r.next_probe = n + (PROBE_STEP_S * fs) as usize;
            }
            let max_lag = (s.max_delay_s * fs) as usize;
            let mut best: Option<f64> = None;
            for c in 0..2 {
                if let Some(e) = estimate_loop_delay_window(
                    &self.capture.reference,
                    &self.capture.ret[c],
                    fs,
                    max_lag,
                    PROBE_WINDOW_S,
                ) {
                    if e.confidence >= s.delay_confidence_min {
                        best = Some(best.map_or(e.delay_samples, |b: f64| b.max(e.delay_samples)));
                    }
                }
            }
            if let Some(d) = best {
                if let Some(r) = self.run.as_mut() {
                    r.delay_known = true;
                }
                let total = ((SKIP_S + s.duration_s) * fs + d + 0.05 * fs).ceil() as u64;
                self.capture.reserve(total as usize);
                self.link.set_max_frames(total.max(n as u64 + 1));
                self.status = format!(
                    "Measuring: loop delay {:.1} ms, noise on...",
                    1000.0 * d / fs
                );
            }
        }
        if delay_known || self.run.as_ref().is_some_and(|r| r.delay_known) {
            let (c, t) = self.progress();
            self.status = format!("Measuring... {:.0} / {:.0} s", c as f64 / fs, t as f64 / fs);
        }
    }

    fn finish_capture(&mut self) {
        let Some(run) = self.run.as_ref() else {
            return;
        };
        let cap = std::mem::take(&mut self.capture);
        let (fs, s) = (run.sample_rate, run.settings.clone());
        self.phase = Phase::Analyzing;
        self.status = "Analysing...".into();
        self.spawn(move || JobResult::Analysis(Box::new(analyze_capture(&cap, fs, &s))));
    }

    fn poll_job(&mut self, p: &mut CalPersist, env: &Env) -> Action {
        let res = match self.job.as_ref().map(|j| j.try_recv()) {
            Some(Ok(r)) => r,
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return None,
            _ => {
                self.fail(CalError::Other("analysis worker failed".into()));
                return None;
            }
        };
        self.job = None;
        match res {
            JobResult::Analysis(r) => match *r {
                Err(e) => {
                    self.fail(e);
                    None
                }
                Ok(m) => self.on_measurement(m, p, env),
            },
            JobResult::Plan(plan) => match *plan {
                Err(e) => {
                    self.fail(CalError::Other(e));
                    None
                }
                Ok(plan) => self.on_plan(plan, p, env),
            },
        }
    }

    fn on_measurement(&mut self, m: Measurement, p: &mut CalPersist, env: &Env) -> Action {
        let mut s = p.settings.clone();
        s.sanitize();
        let dev = deviation_curve(&m, &s, p.correction.as_ref());
        let (rms, max) = deviation_stats(&m.freqs, &dev, &m.coherence, &s);
        self.history.push(IterationRecord {
            index: self.history.len() + 1,
            delay_ms: m.delay_ms(),
            rms_db: rms,
            max_db: max,
            bands: 0,
            applied: false,
        });
        p.last = Some(LoopSummary {
            delay_ms: m.delay_ms(),
            rms_deviation_db: rms,
            max_deviation_db: max,
            bands: 0,
            note: "measured".into(),
        });
        self.measurement = Some(m.clone());
        if !self.auto {
            self.phase = Phase::Idle;
            self.status =
                format!("Measured: deviation {rms:.2} dB RMS / {max:.2} dB max. Press Fit.");
            return None;
        }
        // auto: converged, or diverging, or out of iterations?
        if rms < s.auto_threshold_db {
            self.finish_auto(format!(
                "Converged: {rms:.2} dB RMS after {} applies",
                self.iterations
            ));
            return None;
        }
        let n = self.history.len();
        if n >= 3 && rms > 1.5 * self.history[n - 2].rms_db + 0.5 {
            self.fail(CalError::Other(format!(
                "loop got worse ({:.2} -> {rms:.2} dB RMS): stopped, nothing more applied",
                self.history[n - 2].rms_db
            )));
            return None;
        }
        if self.iterations >= s.auto_max_iterations {
            self.finish_auto(format!(
                "Stopped after {} iterations: {rms:.2} dB RMS remaining",
                self.iterations
            ));
            return None;
        }
        self.start_fit_job(m, p, env);
        None
    }

    fn finish_auto(&mut self, msg: String) {
        self.link.stop();
        self.phase = Phase::Idle;
        self.auto = false;
        self.run = None;
        self.status = msg;
    }

    fn on_plan(&mut self, plan: FitPlan, p: &mut CalPersist, env: &Env) -> Action {
        let (rms, max, bands) = (plan.rms_dev_db, plan.max_dev_db, plan.bands.len());
        p.last = Some(LoopSummary {
            delay_ms: self
                .measurement
                .as_ref()
                .map_or([None; 2], |m| m.delay_ms()),
            rms_deviation_db: rms,
            max_deviation_db: max,
            bands,
            note: plan.warnings.join("; "),
        });
        let delay_s = self
            .measurement
            .as_ref()
            .map_or(0.0, |m| m.max_delay_samples() / m.sample_rate);
        let fs = self
            .measurement
            .as_ref()
            .map_or(48_000.0, |m| m.sample_rate);
        self.plan = Some(plan);
        if !self.auto {
            self.phase = Phase::Idle;
            self.status = format!(
                "Fit: {bands} bands, predicted {:.2} dB RMS (now {rms:.2}). Press Apply.",
                self.plan.as_ref().map_or(0.0, |pl| pl.rms_pred_db)
            );
            return None;
        }
        match self.apply_plan(p, env) {
            Ok(bands) => {
                self.iterations += 1;
                let mut s = p.settings.clone();
                s.sanitize();
                self.settle_until = self.link.audio_frames()
                    + ((2.0 * delay_s + s.settle_extra_s) * fs).ceil() as u64;
                self.phase = Phase::Settling;
                self.status = format!(
                    "Applied iteration {}: waiting {:.1} s for the loop to settle",
                    self.iterations,
                    2.0 * delay_s + s.settle_extra_s
                );
                Some(bands)
            }
            Err(e) => {
                self.fail(CalError::Other(e));
                None
            }
        }
    }
}

/// Which safety stop explains a capture without a confident delay.
fn classify_failure(cap: &Capture, s: &CalSettings) -> CalError {
    let n = cap.len();
    let from = n / 4;
    let mut best_rms = -400.0f64;
    let mut peak = -400.0f64;
    for c in 0..2 {
        let (rms, pk) = level_stats(&cap.ret[c][from..n]);
        best_rms = best_rms.max(rms);
        peak = peak.max(pk);
    }
    if peak >= s.clip_db {
        CalError::ReturnClipped(peak)
    } else if best_rms < s.low_level_db {
        CalError::ReturnTooLow(best_rms)
    } else {
        CalError::NoLoop(0.0)
    }
}
