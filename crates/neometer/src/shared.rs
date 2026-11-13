//! State shared between the plugin, the audio thread, the analysis thread and
//! the editor.
//!
//! | what | direction | mechanism |
//! |---|---|---|
//! | cheap streaming meters ([`Readouts`]) | audio -> GUI | wait-free triple buffer |
//! | stereo frames for analysis | audio -> analysis | SPSC ring ([`crate::ring`]) |
//! | heavy results ([`Snapshot`]) | analysis -> GUI | `Mutex<Arc<Snapshot>>` (swap) |
//! | reset requests | GUI -> audio / analysis | atomic bit mask |
//! | measurement requests | GUI -> analysis | `Mutex<Vec<Command>>` |
//! | generator settings | GUI -> audio | atomics ([`GenControl`]) |
//! | applied input trims | audio -> analysis / GUI | atomics ([`Shared::applied_trim_db`]) |
//! | test-record state and results | GUI <-> analysis | `RwLock` ([`TestRecState`], persisted) |

use crate::analysis::{Command, Snapshot};
use crate::settings::Settings;
use crate::testgen::GenControl;
use crate::testrec::TestRecState;
use crate::vinyl::SignalReadout;
use atomic_float::AtomicF64;
use neo_measure::util::DB_FLOOR;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use triple_buffer::{Input, Output, TripleBuffer};

/// Reset bits for [`Shared::request_reset`].
pub mod reset {
    /// Peak / true peak / RMS / DC / crest statistics (audio thread).
    pub const LEVELS: u32 = 1;
    /// Stereo and LF meters (audio thread).
    pub const STEREO: u32 = 2;
    /// Vinyl meters and max holds (audio thread).
    pub const VINYL: u32 = 4;
    /// Loudness integration, LRA and maxima (analysis thread).
    pub const LOUDNESS: u32 = 8;
    /// Spectrum averages and peak hold (analysis thread).
    pub const SPECTRUM: u32 = 16;
    /// Spectrum peak hold only (analysis thread).
    pub const SPEC_PEAK: u32 = 32;
    pub const AUDIO: u32 = LEVELS | STEREO | VINYL;
    pub const ALL: u32 = AUDIO | LOUDNESS | SPECTRUM | SPEC_PEAK;
}

/// Per-channel level readouts (index 0 = L, 1 = R).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelsReadout {
    /// Held sample peak, dBFS.
    pub peak_db: [f64; 2],
    /// Maximum sample peak since reset, dBFS.
    pub peak_max_db: [f64; 2],
    /// Held true peak, dBTP.
    pub tp_db: [f64; 2],
    pub tp_max_db: [f64; 2],
    /// RMS over 300 ms, sine-referenced dB (a full-scale sine reads 0 dB).
    pub rms_db: [f64; 2],
    /// DC offset since reset (linear, full scale = 1).
    pub dc: [f64; 2],
    /// Peak / RMS since reset, dB.
    pub crest_db: [f64; 2],
}

impl Default for LevelsReadout {
    fn default() -> Self {
        Self {
            peak_db: [DB_FLOOR; 2],
            peak_max_db: [DB_FLOOR; 2],
            tp_db: [DB_FLOOR; 2],
            tp_max_db: [DB_FLOOR; 2],
            rms_db: [DB_FLOOR; 2],
            dc: [0.0; 2],
            crest_db: [0.0; 2],
        }
    }
}

/// Stereo readouts. RMS levels are sine-referenced dB, `M = (L+R)/2`,
/// `S = (L-R)/2`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoReadout {
    pub correlation: f64,
    /// `L - R` level difference, dB (positive = left louder).
    pub balance_db: f64,
    pub mid_db: f64,
    pub side_db: f64,
    pub lf_correlation: f64,
    pub lf_lateral_db: f64,
    pub lf_vertical_db: f64,
}

impl Default for StereoReadout {
    fn default() -> Self {
        Self {
            correlation: 0.0,
            balance_db: 0.0,
            mid_db: DB_FLOOR,
            side_db: DB_FLOOR,
            lf_correlation: 0.0,
            lf_lateral_db: DB_FLOOR,
            lf_vertical_db: DB_FLOOR,
        }
    }
}

/// Everything the audio thread publishes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Readouts {
    pub levels: LevelsReadout,
    pub stereo: StereoReadout,
    /// L, R, lateral, vertical after pre-emphasis.
    pub vinyl: [SignalReadout; 4],
    /// Generator output currently differs from the input.
    pub generator_active: bool,
}

pub struct Shared {
    pub settings: Arc<Settings>,
    pub gen: GenControl,
    /// Test-record profile, options and measurements (persisted).
    pub testrec: Arc<TestRecState>,
    /// Input trims (dB) the audio thread is currently ramping to.
    applied_trim: [AtomicF64; 2],
    reset_flags: AtomicU32,
    commands: Mutex<Vec<Command>>,
    snapshot: Mutex<Arc<Snapshot>>,
    live_in: Mutex<Option<Input<Readouts>>>,
    live_out: Mutex<Output<Readouts>>,
    /// Tests / standalone tools: behave as if the editor were open.
    force_active: AtomicBool,
    editor_state: Option<Arc<EguiState>>,
}

impl Shared {
    pub fn new(settings: Arc<Settings>, editor_state: Option<Arc<EguiState>>) -> Arc<Self> {
        Self::with_testrec(settings, Arc::new(TestRecState::default()), editor_state)
    }

    pub fn with_testrec(
        settings: Arc<Settings>,
        testrec: Arc<TestRecState>,
        editor_state: Option<Arc<EguiState>>,
    ) -> Arc<Self> {
        let (input, output) = TripleBuffer::new(&Readouts::default()).split();
        Arc::new(Self {
            settings,
            gen: GenControl::new(),
            testrec,
            applied_trim: [AtomicF64::new(0.0), AtomicF64::new(0.0)],
            reset_flags: AtomicU32::new(0),
            commands: Mutex::new(Vec::new()),
            snapshot: Mutex::new(Arc::new(Snapshot::default())),
            live_in: Mutex::new(Some(input)),
            live_out: Mutex::new(output),
            force_active: AtomicBool::new(false),
            editor_state,
        })
    }

    /// Standalone instance for tests and tools.
    pub fn standalone() -> Arc<Self> {
        Self::new(Arc::new(Settings::default()), None)
    }

    /// The audio-side writer (taken once by [`crate::audio::AudioPath`]).
    pub(crate) fn take_live_writer(&self) -> Option<Input<Readouts>> {
        self.live_in.lock().ok().and_then(|mut g| g.take())
    }

    /// Audio thread: publish the input trims in effect.
    pub(crate) fn set_applied_trim_db(&self, db: [f64; 2]) {
        for (a, v) in self.applied_trim.iter().zip(db) {
            a.store(v, Ordering::Relaxed);
        }
    }

    /// Input trims (dB) the audio path applies (the host parameter values as
    /// last seen by the audio thread).
    pub fn applied_trim_db(&self) -> [f64; 2] {
        [
            self.applied_trim[0].load(Ordering::Relaxed),
            self.applied_trim[1].load(Ordering::Relaxed),
        ]
    }

    /// Latest audio-thread readouts.
    pub fn readouts(&self) -> Readouts {
        match self.live_out.lock() {
            Ok(mut o) => *o.read(),
            Err(_) => Readouts::default(),
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        match self.snapshot.lock() {
            Ok(g) => g.clone(),
            Err(_) => Arc::new(Snapshot::default()),
        }
    }

    pub(crate) fn publish_snapshot(&self, s: Snapshot) {
        if let Ok(mut g) = self.snapshot.lock() {
            *g = Arc::new(s);
        }
    }

    pub fn request_reset(&self, mask: u32) {
        self.reset_flags.fetch_or(mask, Ordering::AcqRel);
    }

    /// Take (clear and return) the pending reset bits in `mask`.
    pub(crate) fn take_reset(&self, mask: u32) -> u32 {
        if self.reset_flags.load(Ordering::Relaxed) & mask == 0 {
            return 0;
        }
        self.reset_flags.fetch_and(!mask, Ordering::AcqRel) & mask
    }

    pub fn send(&self, c: Command) {
        if let Ok(mut q) = self.commands.lock() {
            if q.len() < 16 {
                q.push(c);
            }
        }
    }

    pub(crate) fn drain_commands(&self, out: &mut Vec<Command>) {
        if let Ok(mut q) = self.commands.lock() {
            out.append(&mut q);
        }
    }

    pub fn set_force_active(&self, on: bool) {
        self.force_active.store(on, Ordering::Relaxed);
    }

    /// Whether the heavy analyses should run (editor open or forced).
    pub fn ui_active(&self) -> bool {
        self.force_active.load(Ordering::Relaxed)
            || self.editor_state.as_ref().is_some_and(|s| s.is_open())
    }
}
