//! Automatic loop calibration (host independent).
//!
//! * [`tap`]: the realtime part. Noise injection at the plugin input and the
//!   lock-free capture of (pre-EQ reference, sidechain return L/R) frames.
//! * [`analysis`]: delay, transfer function and coherence of a finished
//!   capture (off-thread, allocating).
//! * [`fit`]: from a measurement to the target correction and the fitted
//!   calibration bands (uses `neofilter_dsp::autofit`).
//! * [`session`]: the measurement / fit / apply / auto state machine, polled
//!   by the editor (or by tests).
//! * [`sim`]: synthetic loop (delay, system EQ, noise) wired to the real tap
//!   and audio path, for tests.
//! * [`settings`]: persisted options, the playback-chain correction curve and
//!   the band ownership mask.

pub mod analysis;
pub mod fit;
pub mod session;
pub mod settings;
pub mod sim;
pub mod tap;

pub use analysis::{analyze_capture, CalError, Capture, ChannelInfo, Measurement};
pub use fit::{apply_bands, plan_fit, resolve_mask, FitPlan};
pub use session::Env;
pub use session::{Action, IterationRecord, Phase, Session};
pub use settings::{
    CalCurve, CalPersist, CalSettings, InjectMode, LoopSummary, NoiseKind, Routing,
};
pub use tap::{CalLink, CalTap};

/// Calibration state shared between the plugin (audio side keeps the `CalTap`)
/// and the editor: the lock-free link and the control-side session. The
/// session is only locked by the GUI thread.
pub struct CalShared {
    pub link: std::sync::Arc<CalLink>,
    pub session: std::sync::Mutex<Session>,
}

impl Default for CalShared {
    fn default() -> Self {
        let link = std::sync::Arc::new(CalLink::new());
        Self {
            session: std::sync::Mutex::new(Session::new(link.clone())),
            link,
        }
    }
}
