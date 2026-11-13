//! GUI choices that are persisted with the plugin state (not host parameters)
//! and the few of them the audio thread needs (as atomics).
//!
//! The generator is deliberately **not** part of this state, see
//! [`crate::testgen`].

use atomic_float::AtomicF32;
use neo_measure::spectrum::Averaging;
use neo_measure::util::Window;
use nih_plug::params::persist::PersistentField;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Page {
    #[default]
    Levels,
    Stereo,
    Spectrum,
    Vinyl,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WindowKind {
    Hann,
    #[default]
    BlackmanHarris,
    FlatTop,
}

impl WindowKind {
    pub const ALL: [WindowKind; 3] = [Self::Hann, Self::BlackmanHarris, Self::FlatTop];
    pub fn window(self) -> Window {
        match self {
            Self::Hann => Window::Hann,
            Self::BlackmanHarris => Window::BlackmanHarris,
            Self::FlatTop => Window::FlatTop,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Hann => "Hann",
            Self::BlackmanHarris => "Blackman-Harris",
            Self::FlatTop => "Flat top",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AvgMode {
    Off,
    #[default]
    Exponential,
    Infinite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpecChannel {
    Left,
    Right,
    Mid,
    Side,
    /// L and R overlaid.
    #[default]
    Both,
}

impl SpecChannel {
    pub const ALL: [SpecChannel; 5] = [Self::Left, Self::Right, Self::Mid, Self::Side, Self::Both];
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "L",
            Self::Right => "R",
            Self::Mid => "Mid",
            Self::Side => "Side",
            Self::Both => "L+R",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GonioMode {
    /// Rotated 45 degrees (mono = vertical line).
    #[default]
    MidSide,
    /// L on x, R on y.
    LeftRight,
}

pub const FFT_SIZES: [usize; 5] = [4096, 8192, 16384, 32768, 65536];
/// Fractional-octave smoothing choices: 0 = off, else 1/n octave.
pub const SMOOTHING: [u32; 5] = [0, 1, 3, 6, 12];
/// Stereo / LF correlation integration time constants, seconds.
pub const CORR_TAUS: [f32; 4] = [0.1, 0.3, 1.0, 3.0];
pub const TARGET_LUFS: [f64; 4] = [-14.0, -16.0, -23.0, -9.0];
pub const TONE_DURATIONS: [f64; 4] = [1.0, 2.0, 5.0, 10.0];
pub const WF_DURATIONS: [f64; 4] = [5.0, 10.0, 20.0, 30.0];
pub const NOISE_DURATIONS: [f64; 4] = [2.0, 5.0, 10.0, 20.0];

/// All persisted GUI choices. Unknown / missing fields fall back to defaults
/// so old states keep loading.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsData {
    pub page: Page,
    // levels
    pub target_lufs: f64,
    pub target_custom: bool,
    // stereo
    pub corr_tau_s: f32,
    pub gonio_mode: GonioMode,
    /// Warn when vertical exceeds lateral by more than this many dB.
    pub vert_warn_db: f32,
    /// Warn when the LF correlation falls below this value.
    pub lf_corr_warn: f32,
    // spectrum
    pub fft_size: usize,
    pub window: WindowKind,
    pub averaging: AvgMode,
    pub avg_tau_s: f32,
    pub peak_hold: bool,
    pub smoothing: u32,
    pub spec_channel: SpecChannel,
    pub rta: bool,
    pub db_min: f32,
    pub db_max: f32,
    pub f_lo: f32,
    pub f_hi: f32,
    // vinyl
    pub hf_corner_hz: f32,
    // test
    pub tone_right: bool,
    pub tone_duration_s: f64,
    pub wf_nominal_hz: f64,
    pub wf_duration_s: f64,
    pub noise_duration_s: f64,
    pub noise_band_low: f64,
    pub noise_band_high: f64,
    /// Latched reference tone level (peak dBFS) for the noise readings.
    pub noise_ref_dbfs: Option<f64>,
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            page: Page::Levels,
            target_lufs: -14.0,
            target_custom: false,
            corr_tau_s: 0.3,
            gonio_mode: GonioMode::MidSide,
            vert_warn_db: 0.0,
            lf_corr_warn: 0.0,
            fft_size: 16384,
            window: WindowKind::BlackmanHarris,
            averaging: AvgMode::Exponential,
            avg_tau_s: 0.5,
            peak_hold: false,
            smoothing: 0,
            spec_channel: SpecChannel::Both,
            rta: false,
            db_min: -120.0,
            db_max: 0.0,
            f_lo: 20.0,
            f_hi: 20_000.0,
            hf_corner_hz: 5000.0,
            tone_right: false,
            tone_duration_s: 2.0,
            wf_nominal_hz: 3150.0,
            wf_duration_s: 10.0,
            noise_duration_s: 5.0,
            noise_band_low: 20.0,
            noise_band_high: 20_000.0,
            noise_ref_dbfs: None,
        }
    }
}

impl SettingsData {
    /// Clamp everything to values the DSP accepts (state files are untrusted).
    pub fn sanitized(mut self) -> Self {
        let fin =
            |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let finf =
            |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        if !FFT_SIZES.contains(&self.fft_size) {
            self.fft_size = 16384;
        }
        if !SMOOTHING.contains(&self.smoothing) {
            self.smoothing = 0;
        }
        self.target_lufs = fin(self.target_lufs, -60.0, 0.0, -14.0);
        self.corr_tau_s = finf(self.corr_tau_s, 0.01, 30.0, 0.3);
        self.vert_warn_db = finf(self.vert_warn_db, -30.0, 30.0, 0.0);
        self.lf_corr_warn = finf(self.lf_corr_warn, -1.0, 1.0, 0.0);
        self.avg_tau_s = finf(self.avg_tau_s, 0.01, 60.0, 0.5);
        self.db_min = finf(self.db_min, -200.0, 0.0, -120.0);
        self.db_max = finf(self.db_max, self.db_min + 10.0, 24.0, 0.0);
        self.f_lo = finf(self.f_lo, 5.0, 20_000.0, 20.0);
        self.f_hi = finf(self.f_hi, self.f_lo * 1.5, 96_000.0, 20_000.0);
        self.hf_corner_hz = finf(self.hf_corner_hz, 3000.0, 10_000.0, 5000.0);
        self.tone_duration_s = fin(self.tone_duration_s, 0.5, 30.0, 2.0);
        self.wf_nominal_hz = fin(self.wf_nominal_hz, 100.0, 20_000.0, 3150.0);
        self.wf_duration_s = fin(self.wf_duration_s, 3.0, 60.0, 10.0);
        self.noise_duration_s = fin(self.noise_duration_s, 0.5, 60.0, 5.0);
        self.noise_band_low = fin(self.noise_band_low, 1.0, 1000.0, 20.0);
        self.noise_band_high = fin(self.noise_band_high, 1000.0, 96_000.0, 20_000.0);
        if let Some(v) = self.noise_ref_dbfs {
            if !v.is_finite() {
                self.noise_ref_dbfs = None;
            }
        }
        self
    }

    pub fn averaging_mode(&self) -> Averaging {
        match self.averaging {
            AvgMode::Off => Averaging::None,
            AvgMode::Exponential => Averaging::Exponential(f64::from(self.avg_tau_s)),
            AvgMode::Infinite => Averaging::Infinite,
        }
    }
}

/// Shared, persisted settings plus audio-thread mirrors.
pub struct Settings {
    data: RwLock<SettingsData>,
    corr_tau_s: AtomicF32,
    hf_corner_hz: AtomicF32,
}

impl Default for Settings {
    fn default() -> Self {
        let d = SettingsData::default();
        Self {
            corr_tau_s: AtomicF32::new(d.corr_tau_s),
            hf_corner_hz: AtomicF32::new(d.hf_corner_hz),
            data: RwLock::new(d),
        }
    }
}

impl Settings {
    pub fn get(&self) -> SettingsData {
        self.data.read().map(|d| *d).unwrap_or_default()
    }

    pub fn replace(&self, new: SettingsData) {
        let new = new.sanitized();
        if let Ok(mut d) = self.data.write() {
            *d = new;
        }
        self.corr_tau_s.store(new.corr_tau_s, Ordering::Relaxed);
        self.hf_corner_hz.store(new.hf_corner_hz, Ordering::Relaxed);
    }

    /// Modify the settings (GUI thread).
    pub fn update(&self, f: impl FnOnce(&mut SettingsData)) {
        let mut d = self.get();
        f(&mut d);
        self.replace(d);
    }

    /// Audio thread: correlation integration time (seconds).
    pub fn corr_tau_s(&self) -> f32 {
        self.corr_tau_s.load(Ordering::Relaxed)
    }

    /// Audio thread: lower corner of the vinyl HF band (Hz).
    pub fn hf_corner_hz(&self) -> f32 {
        self.hf_corner_hz.load(Ordering::Relaxed)
    }
}

impl<'a> PersistentField<'a, SettingsData> for Settings {
    fn set(&self, new_value: SettingsData) {
        self.replace(new_value);
    }
    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&SettingsData) -> R,
    {
        f(&self.get())
    }
}

impl<'a> PersistentField<'a, SettingsData> for Arc<Settings> {
    fn set(&self, new_value: SettingsData) {
        self.as_ref().replace(new_value);
    }
    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&SettingsData) -> R,
    {
        f(&self.get())
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_garbage() {
        let mut d = SettingsData::default();
        d.fft_size = 1234;
        d.f_lo = f32::NAN;
        d.hf_corner_hz = 1e9;
        d.noise_ref_dbfs = Some(f64::INFINITY);
        let s = d.sanitized();
        assert_eq!(s.fft_size, 16384);
        assert_eq!(s.f_lo, 20.0);
        assert_eq!(s.hf_corner_hz, 10_000.0);
        assert_eq!(s.noise_ref_dbfs, None);
    }

    #[test]
    fn json_round_trip_and_missing_fields() {
        let mut d = SettingsData::default();
        d.page = Page::Vinyl;
        d.fft_size = 65536;
        let s = serde_json::to_string(&d).unwrap();
        let back: SettingsData = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
        let partial: SettingsData = serde_json::from_str(r#"{"page":"Spectrum"}"#).unwrap();
        assert_eq!(partial.page, Page::Spectrum);
        assert_eq!(partial.fft_size, 16384);
    }

    #[test]
    fn mirrors_follow_updates() {
        let s = Settings::default();
        s.update(|d| {
            d.corr_tau_s = 1.0;
            d.hf_corner_hz = 7000.0;
        });
        assert_eq!(s.corr_tau_s(), 1.0);
        assert_eq!(s.hf_corner_hz(), 7000.0);
    }
}
