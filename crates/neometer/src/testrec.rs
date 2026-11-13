//! Test-record input calibration: persisted state (profile, options and the
//! measurements), and the pure computations behind the "Test record" page.
//!
//! Everything in here is plain data and arithmetic; the analysis thread
//! ([`crate::analysis`]) produces the measurements and stores them in
//! [`TestRecState`], the editor only reads it and sends commands.
//!
//! # Steps
//!
//! 1. **Reference level, balance, separation**: [`measure_ref_band`] on a
//!    left-only band and a right-only band (or one lateral / vertical band),
//!    [`TestRecData::ref_calibration`] turns them into levels, balance,
//!    crosstalk, speed, suggested trims and the velocity calibration.
//! 2. **Response**: the analysis thread tracks the sweep
//!    ([`neo_measure::sweep_track`]) into a [`CapturedSweep`];
//!    [`TestRecData::response`] builds the deviation curve and
//!    [`curve_json`] the export.
//! 3. **Speed, W&F**: [`measure_wf`].
//! 4. **Tracking**: [`measure_track_row`] per band, [`evaluate_tracking`].
//! 5. **Summary**: [`TestRecData::summary_text`].
//!
//! The velocity conventions are those of [`neo_measure::velocity`].

use crate::params::{TRIM_MAX_DB, TRIM_MIN_DB};
use neo_measure::sweep_track::{
    deviation_points, deviation_stats, expected_db, norm_offset_at, smooth_points_octave,
    Characteristic, DeviationStats,
};
use neo_measure::tone::{
    analyze_tone, channel_separation_db, phase_difference_deg, tone_level_at, ToneConfig,
};
use neo_measure::velocity::{
    amplitude_um_to_velocity_cm_s, channel_velocity_peak, detect_modulation, level_step_db,
    reference_level_dbfs, rms_to_peak, Modulation,
};
use neo_measure::wow_flutter::{analyze_wow_flutter, WowFlutterConfig};
use nih_plug::params::persist::PersistentField;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Largest number of stored tracking rows / sweep cells (state files are
/// untrusted).
const MAX_ROWS: usize = 64;
const MAX_CELLS: usize = 4096;

// ---------------------------------------------------------------------------
// Persisted enums

/// Serializable mirror of [`Modulation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Mod {
    #[default]
    Left,
    Right,
    Lateral,
    Vertical,
}

impl Mod {
    pub fn m(self) -> Modulation {
        match self {
            Mod::Left => Modulation::Left,
            Mod::Right => Modulation::Right,
            Mod::Lateral => Modulation::Lateral,
            Mod::Vertical => Modulation::Vertical,
        }
    }
    pub fn from_m(m: Modulation) -> Self {
        match m {
            Modulation::Left => Mod::Left,
            Modulation::Right => Mod::Right,
            Modulation::Lateral => Mod::Lateral,
            Modulation::Vertical => Mod::Vertical,
        }
    }
    pub fn name(self) -> &'static str {
        self.m().name()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProfileKind {
    #[default]
    Ortofon,
    Custom,
}

/// How the 1 kHz reference bands of a record are organised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RefLayout {
    /// A left-only and a right-only band.
    #[default]
    LeftRight,
    /// One lateral band (both channels in phase).
    Lateral,
    /// One vertical band (both channels in anti-phase).
    Vertical,
}

impl RefLayout {
    /// Slots (bands) the user has to measure.
    pub fn slots(self) -> &'static [Mod] {
        match self {
            RefLayout::LeftRight => &[Mod::Left, Mod::Right],
            RefLayout::Lateral => &[Mod::Lateral],
            RefLayout::Vertical => &[Mod::Vertical],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NominalKind {
    /// Constant velocity (the Ortofon sweeps).
    #[default]
    ConstantVelocity,
    /// Recorded with RIAA pre-emphasis (flat after an RIAA playback).
    FlatAfterRiaa,
    /// Constant amplitude below the turnover, constant velocity above.
    Turnover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NormMode {
    /// Deviation is 0 dB at the normalisation frequency.
    #[default]
    AtFreq,
    /// Referenced to the level measured in step 1.
    ToReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportChannel {
    /// Mean of the channels that carry the sweep.
    #[default]
    Average,
    Left,
    Right,
}

// ---------------------------------------------------------------------------
// Profile

/// A test record description. Every value is editable in the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// Reference band frequency, Hz (also the W&F carrier).
    pub ref_freq_hz: f64,
    /// Quoted reference velocity, cm/s.
    pub ref_velocity_cm_s: f64,
    /// The quoted velocity is RMS (else peak).
    pub ref_is_rms: bool,
    pub ref_layout: RefLayout,
    /// Frequency range of the sweep bands, Hz.
    pub sweep_lo_hz: f64,
    pub sweep_hi_hz: f64,
    pub nominal: NominalKind,
    pub turnover_hz: f64,
    /// Tracking bands: frequency (Hz) and peak amplitudes (um, lateral).
    pub track_freq_hz: f64,
    pub track_amps_um: Vec<f64>,
}

impl Profile {
    /// Values from public listings of the Ortofon test record; check the
    /// sleeve.
    pub fn ortofon() -> Self {
        Self {
            name: "Ortofon Test Record".into(),
            ref_freq_hz: 1000.0,
            ref_velocity_cm_s: 5.0,
            ref_is_rms: true,
            ref_layout: RefLayout::LeftRight,
            sweep_lo_hz: 800.0,
            sweep_hi_hz: 50_000.0,
            nominal: NominalKind::ConstantVelocity,
            turnover_hz: 500.0,
            track_freq_hz: 315.0,
            track_amps_um: vec![50.0, 60.0, 70.0, 80.0, 90.0, 100.0],
        }
    }

    pub fn custom() -> Self {
        Self {
            name: "Custom".into(),
            ref_freq_hz: 1000.0,
            ref_velocity_cm_s: 5.0,
            ref_is_rms: false,
            ref_layout: RefLayout::Lateral,
            sweep_lo_hz: 20.0,
            sweep_hi_hz: 20_000.0,
            nominal: NominalKind::FlatAfterRiaa,
            turnover_hz: 500.0,
            track_freq_hz: 315.0,
            track_amps_um: vec![50.0, 60.0, 70.0, 80.0, 90.0, 100.0],
        }
    }

    /// Quoted reference velocity as a peak value (cm/s).
    pub fn ref_velocity_peak_cm_s(&self) -> f64 {
        if self.ref_is_rms {
            rms_to_peak(self.ref_velocity_cm_s)
        } else {
            self.ref_velocity_cm_s
        }
    }

    /// Peak velocity of the wall of one channel in a reference band of
    /// `slot` (see [`neo_measure::velocity`]).
    pub fn ref_channel_velocity_peak_cm_s(&self, slot: Mod) -> f64 {
        channel_velocity_peak(slot.m(), self.ref_velocity_peak_cm_s())
    }

    pub fn characteristic(&self) -> Characteristic {
        match self.nominal {
            NominalKind::ConstantVelocity => Characteristic::ConstantVelocity,
            NominalKind::FlatAfterRiaa => Characteristic::FlatAfterRiaa,
            NominalKind::Turnover => Characteristic::TurnoverAmplitudeBelow(self.turnover_hz),
        }
    }

    fn sanitize(&mut self) {
        let fin =
            |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.name.truncate(80);
        self.ref_freq_hz = fin(self.ref_freq_hz, 20.0, 20_000.0, 1000.0);
        self.ref_velocity_cm_s = fin(self.ref_velocity_cm_s, 0.1, 100.0, 5.0);
        self.sweep_lo_hz = fin(self.sweep_lo_hz, 1.0, 50_000.0, 800.0);
        self.sweep_hi_hz = fin(
            self.sweep_hi_hz,
            self.sweep_lo_hz * 1.1,
            200_000.0,
            50_000.0,
        );
        self.turnover_hz = fin(self.turnover_hz, 20.0, 20_000.0, 500.0);
        self.track_freq_hz = fin(self.track_freq_hz, 20.0, 5_000.0, 315.0);
        self.track_amps_um.retain(|a| a.is_finite() && *a > 0.0);
        self.track_amps_um.truncate(16);
        if self.track_amps_um.is_empty() {
            self.track_amps_um = vec![50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::ortofon()
    }
}

// ---------------------------------------------------------------------------
// Measurements (persisted)

/// One channel of a measured tone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChanTone {
    pub freq_hz: f64,
    /// Peak level, dBFS.
    pub level_dbfs: f64,
    pub thd_percent: Option<f64>,
    pub thd_n_percent: Option<f64>,
    /// Residual (noise and non-harmonic) level in 20 Hz..20 kHz, dBFS.
    pub noise_dbfs: Option<f64>,
    /// The channel carries the band (else the level is crosstalk / noise).
    pub driven: bool,
}

/// A measured 1 kHz reference band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefBand {
    /// Which band the user said this was.
    pub slot: Mod,
    pub seconds: f64,
    pub ch: [ChanTone; 2],
    /// Phase of L relative to R at the tone frequency, degrees.
    pub phase_deg: Option<f64>,
    /// Modulation detected from the level difference and the phase.
    pub detected: Option<Mod>,
    /// Driven channel level minus the other channel's level, dB (left / right
    /// bands only).
    pub separation_db: Option<f64>,
    /// Input trims (dB) that were applied while measuring.
    pub trim_db: [f64; 2],
}

/// Wow and flutter measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WfMeas {
    pub channel: usize,
    pub carrier_hz: f64,
    pub mean_hz: f64,
    pub speed_pct: f64,
    pub unweighted_peak_pct: f64,
    pub unweighted_rms_pct: f64,
    pub unweighted_2sigma_pct: f64,
    pub weighted_peak_pct: f64,
    pub weighted_rms_pct: f64,
    pub weighted_2sigma_pct: f64,
}

/// A measured tracking band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackRow {
    pub amp_um: f64,
    pub freq_hz: f64,
    pub ch: [ChanTone; 2],
}

/// Captured sweep levels: per channel `(grid index, frequency, level dBFS)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CapturedSweep {
    pub cells: [Vec<(u32, f64, f64)>; 2],
    /// Input trims (dB) during the capture.
    pub trim_db: [f64; 2],
    /// Modulation detected from the inter-channel level and phase.
    pub detected: Option<Mod>,
    pub level_diff_db: Option<f64>,
    pub phase_deg: Option<f64>,
    pub pair_blocks: usize,
}

/// Everything persisted for the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TestRecData {
    pub profile_kind: ProfileKind,
    pub ortofon: Profile,
    pub custom: Profile,
    // step 1
    pub ref_seconds: f64,
    pub target_dbfs: f64,
    pub ref_bands: Vec<RefBand>,
    // step 2
    pub sweep_mod: Option<Mod>,
    pub norm: NormMode,
    pub norm_freq_hz: f64,
    /// Level of the sweep relative to the reference band, dB (for
    /// [`NormMode::ToReference`]).
    pub sweep_rel_ref_db: f64,
    pub preamp_riaa: bool,
    pub riaa_iec: bool,
    pub min_snr_db: f64,
    pub range_db: f64,
    pub smoothing: u32,
    pub export_channel: ExportChannel,
    pub curve_name: String,
    pub sweep: Option<CapturedSweep>,
    // step 3
    pub wf_seconds: f64,
    pub wf_channel: usize,
    pub wf: Option<WfMeas>,
    // step 4
    pub track_seconds: f64,
    pub track_sel: usize,
    pub track_auto: bool,
    pub track_thd_pct: f64,
    pub track_jump_db: f64,
    pub track_shortfall_db: f64,
    pub tracking: Vec<TrackRow>,
}

impl Default for TestRecData {
    fn default() -> Self {
        Self {
            profile_kind: ProfileKind::Ortofon,
            ortofon: Profile::ortofon(),
            custom: Profile::custom(),
            ref_seconds: 5.0,
            target_dbfs: -18.0,
            ref_bands: Vec::new(),
            sweep_mod: None,
            norm: NormMode::AtFreq,
            norm_freq_hz: 1000.0,
            sweep_rel_ref_db: 0.0,
            preamp_riaa: true,
            riaa_iec: false,
            min_snr_db: 25.0,
            range_db: 6.0,
            smoothing: 0,
            export_channel: ExportChannel::Average,
            curve_name: "Playback chain".into(),
            sweep: None,
            wf_seconds: 10.0,
            wf_channel: 0,
            wf: None,
            track_seconds: 3.0,
            track_sel: 0,
            track_auto: true,
            track_thd_pct: 3.0,
            track_jump_db: 6.0,
            track_shortfall_db: 1.0,
            tracking: Vec::new(),
        }
    }
}

/// Selectable averaging times of the reference bands.
pub const REF_SECONDS: [f64; 4] = [2.0, 5.0, 10.0, 20.0];
/// Selectable smoothing of the exported curve (0 = off, else 1/n octave).
pub const CURVE_SMOOTHING: [u32; 4] = [0, 12, 6, 3];

impl TestRecData {
    pub fn profile(&self) -> &Profile {
        match self.profile_kind {
            ProfileKind::Ortofon => &self.ortofon,
            ProfileKind::Custom => &self.custom,
        }
    }

    pub fn profile_mut(&mut self) -> &mut Profile {
        match self.profile_kind {
            ProfileKind::Ortofon => &mut self.ortofon,
            ProfileKind::Custom => &mut self.custom,
        }
    }

    /// Clamp everything to values the analysis accepts (state files are
    /// untrusted).
    pub fn sanitized(mut self) -> Self {
        let fin =
            |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.ortofon.sanitize();
        self.custom.sanitize();
        self.ref_seconds = fin(self.ref_seconds, 1.0, 30.0, 5.0);
        self.target_dbfs = fin(self.target_dbfs, -60.0, -1.0, -18.0);
        self.norm_freq_hz = fin(self.norm_freq_hz, 20.0, 20_000.0, 1000.0);
        self.sweep_rel_ref_db = fin(self.sweep_rel_ref_db, -60.0, 60.0, 0.0);
        self.min_snr_db = fin(self.min_snr_db, 6.0, 60.0, 25.0);
        self.range_db = fin(self.range_db, 1.0, 40.0, 6.0);
        if !CURVE_SMOOTHING.contains(&self.smoothing) {
            self.smoothing = 0;
        }
        self.curve_name.truncate(80);
        self.wf_seconds = fin(self.wf_seconds, 3.0, 30.0, 10.0);
        self.wf_channel = self.wf_channel.min(1);
        self.track_seconds = fin(self.track_seconds, 1.0, 30.0, 3.0);
        self.track_thd_pct = fin(self.track_thd_pct, 0.01, 100.0, 3.0);
        self.track_jump_db = fin(self.track_jump_db, 0.5, 40.0, 6.0);
        self.track_shortfall_db = fin(self.track_shortfall_db, 0.1, 20.0, 1.0);
        let n = self.profile().track_amps_um.len();
        self.track_sel = self.track_sel.min(n.saturating_sub(1));
        self.ref_bands.truncate(3);
        self.tracking.truncate(MAX_ROWS);
        if let Some(s) = self.sweep.as_mut() {
            for c in s.cells.iter_mut() {
                c.truncate(MAX_CELLS);
                c.retain(|p| p.1.is_finite() && p.2.is_finite() && p.1 > 0.0);
            }
            for t in s.trim_db.iter_mut() {
                *t = fin(*t, TRIM_MIN_DB.into(), TRIM_MAX_DB.into(), 0.0);
            }
        }
        self
    }

    /// Store a reference band, replacing the one of the same slot.
    pub fn set_ref_band(&mut self, b: RefBand) {
        self.ref_bands.retain(|x| x.slot != b.slot);
        self.ref_bands.push(b);
    }

    /// Store a tracking row, replacing the one of the same amplitude.
    pub fn set_track_row(&mut self, r: TrackRow) {
        self.tracking.retain(|x| (x.amp_um - r.amp_um).abs() > 1e-9);
        self.tracking.push(r);
        self.tracking.sort_by(|a, b| a.amp_um.total_cmp(&b.amp_um));
    }

    fn band(&self, slot: Mod) -> Option<&RefBand> {
        self.ref_bands.iter().find(|b| b.slot == slot)
    }

    /// Per-channel level (dBFS) of the reference band as it would read with
    /// the input trims `trim_db`, and the bands used.
    pub fn ref_levels_at_trim(&self, trim_db: [f64; 2]) -> Option<([f64; 2], Vec<&RefBand>)> {
        let shift = |b: &RefBand, c: usize| b.ch[c].level_dbfs + trim_db[c] - b.trim_db[c];
        if let (Some(l), Some(r)) = (self.band(Mod::Left), self.band(Mod::Right)) {
            return Some(([shift(l, 0), shift(r, 1)], vec![l, r]));
        }
        for m in [Mod::Lateral, Mod::Vertical] {
            if let Some(b) = self.band(m) {
                return Some(([shift(b, 0), shift(b, 1)], vec![b]));
            }
        }
        None
    }

    /// Step 1 results with the input trims `trims_now` currently applied.
    pub fn ref_calibration(&self, trims_now: [f64; 2]) -> Option<RefCalibration> {
        let (lv, bands) = self.ref_levels_at_trim(trims_now)?;
        let p = self.profile();
        let mut warnings = Vec::new();
        for b in &bands {
            match b.detected {
                Some(d) if d != b.slot => warnings.push(format!(
                    "the {} band was detected as {}",
                    b.slot.name(),
                    d.name()
                )),
                None => warnings.push(format!(
                    "the {} band's modulation could not be detected",
                    b.slot.name()
                )),
                _ => {}
            }
        }
        let driven_freq: Vec<f64> = bands
            .iter()
            .flat_map(|b| b.ch.iter().filter(|c| c.driven).map(|c| c.freq_hz))
            .collect();
        let tone_hz = driven_freq.iter().sum::<f64>() / driven_freq.len().max(1) as f64;
        let speed_pct = (tone_hz / p.ref_freq_hz - 1.0) * 100.0;
        let mut suggested = [0.0; 2];
        let mut clamped = false;
        for c in 0..2 {
            let want = trims_now[c] + self.target_dbfs - lv[c];
            let s = want.clamp(TRIM_MIN_DB.into(), TRIM_MAX_DB.into());
            clamped |= (s - want).abs() > 1e-9;
            suggested[c] = s;
        }
        let (mut x_lr, mut x_rl) = (None, None);
        for b in &bands {
            match b.slot {
                Mod::Left => x_lr = b.separation_db,
                Mod::Right => x_rl = b.separation_db,
                _ => {}
            }
        }
        let v_ch = p.ref_channel_velocity_peak_cm_s(bands[0].slot);
        let reference = 0.5
            * (reference_level_dbfs(lv[0], v_ch)
                + reference_level_dbfs(
                    lv[1],
                    p.ref_channel_velocity_peak_cm_s(bands[bands.len() - 1].slot),
                ));
        Some(RefCalibration {
            level_dbfs: lv,
            balance_db: lv[0] - lv[1],
            xtalk_l_to_r_db: x_lr,
            xtalk_r_to_l_db: x_rl,
            tone_hz,
            speed_pct,
            suggested_trim_db: suggested,
            trims_clamped: clamped,
            reference_level_dbfs: reference,
            channel_velocity_peak_cm_s: v_ch,
            warnings,
        })
    }

    /// The modulation the sweep is treated as: the user's choice, else the
    /// detected one.
    pub fn sweep_modulation(&self) -> Option<Modulation> {
        self.sweep_mod
            .map(Mod::m)
            .or_else(|| self.sweep.as_ref().and_then(|s| s.detected).map(Mod::m))
    }

    /// Step 2: deviation curves of the captured sweep. `Err` explains what
    /// is missing.
    pub fn response(&self) -> Result<Response, String> {
        let sw = self.sweep.as_ref().ok_or("no sweep captured yet")?;
        let p = self.profile();
        let (c, riaa, iec) = (p.characteristic(), self.preamp_riaa, self.riaa_iec);
        let valid = match self.sweep_modulation() {
            Some(Modulation::Left) => [true, false],
            Some(Modulation::Right) => [false, true],
            _ => [true, true],
        };
        // reference offsets (ToReference)
        let ref_levels = match self.norm {
            NormMode::ToReference => {
                let (l, _) = self
                    .ref_levels_at_trim(sw.trim_db)
                    .ok_or("measure the reference band(s) in step 1 first")?;
                Some(l)
            }
            NormMode::AtFreq => None,
        };
        let mut raw: [Vec<(u32, f64, f64)>; 2] = [Vec::new(), Vec::new()];
        let mut dev: [Vec<(u32, f64, f64)>; 2] = [Vec::new(), Vec::new()];
        for ch in 0..2 {
            raw[ch] = sw.cells[ch]
                .iter()
                .copied()
                .filter(|p2| p2.1 >= p.sweep_lo_hz && p2.1 <= p.sweep_hi_hz)
                .collect();
            if raw[ch].is_empty() {
                continue;
            }
            let pts: Vec<(f64, f64)> = raw[ch].iter().map(|q| (q.1, q.2)).collect();
            let off = match ref_levels {
                None => norm_offset_at(&pts, c, riaa, iec, self.norm_freq_hz).ok_or_else(|| {
                    format!(
                        "the normalisation frequency {:.0} Hz is not covered by the sweep",
                        self.norm_freq_hz
                    )
                }),
                Some(l) => {
                    Ok(l[ch] + self.sweep_rel_ref_db - expected_db(c, riaa, iec, p.ref_freq_hz))
                }
            };
            let off = match off {
                Ok(o) => o,
                Err(e) if valid[ch] => return Err(e),
                Err(_) => continue,
            };
            let d = deviation_points(&pts, c, riaa, iec, off);
            dev[ch] = raw[ch]
                .iter()
                .zip(d)
                .map(|(r, (f, v))| (r.0, f, v))
                .collect();
        }
        if dev[0].is_empty() && dev[1].is_empty() {
            return Err("no covered sweep points in the profile's sweep range".into());
        }
        // average of the channels that carry the sweep (same grid cells)
        let avg: Vec<(f64, f64)> = match (
            valid[0] && !dev[0].is_empty(),
            valid[1] && !dev[1].is_empty(),
        ) {
            (true, true) => {
                let mut v = Vec::new();
                for a in &dev[0] {
                    if let Some(b) = dev[1].iter().find(|b| b.0 == a.0) {
                        v.push((0.5 * (a.1 + b.1), 0.5 * (a.2 + b.2)));
                    }
                }
                v
            }
            (true, false) => dev[0].iter().map(|q| (q.1, q.2)).collect(),
            (false, true) => dev[1].iter().map(|q| (q.1, q.2)).collect(),
            (false, false) => Vec::new(),
        };
        let strip =
            |v: &[(u32, f64, f64)]| -> Vec<(f64, f64)> { v.iter().map(|q| (q.1, q.2)).collect() };
        let sm = |v: Vec<(f64, f64)>| smooth_points_octave(&v, self.smoothing);
        let per_channel = [sm(strip(&dev[0])), sm(strip(&dev[1]))];
        let average = sm(avg);
        let selected = match self.export_channel {
            ExportChannel::Average => average.clone(),
            ExportChannel::Left => per_channel[0].clone(),
            ExportChannel::Right => per_channel[1].clone(),
        };
        let covered = match (selected.first(), selected.last()) {
            (Some(a), Some(b)) => Some((a.0, b.0)),
            _ => None,
        };
        Ok(Response {
            stats_all: deviation_stats(&selected, 0.0, f64::MAX),
            stats_1_15k: deviation_stats(&selected, 1000.0, 15_000.0),
            per_channel,
            average,
            selected,
            covered,
            valid,
        })
    }

    /// Step 5: the summary as text.
    pub fn summary_text(&self, trims_now: [f64; 2]) -> String {
        let p = self.profile();
        let mut s = format!("NeoMeter test record summary\nProfile: {}\n", p.name);
        match self.ref_calibration(trims_now) {
            Some(r) => {
                s += &format!(
                    "Reference band {:.0} Hz ({}): L {:.2} dBFS, R {:.2} dBFS, balance L-R {:+.2} dB\n",
                    p.ref_freq_hz, p.ref_velocity_label(), r.level_dbfs[0], r.level_dbfs[1], r.balance_db
                );
                let x = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{v:.1} dB"));
                s += &format!(
                    "Separation: L->R {}, R->L {}\n",
                    x(r.xtalk_l_to_r_db),
                    x(r.xtalk_r_to_l_db)
                );
                s += &format!(
                    "Tone {:.2} Hz, speed deviation {:+.2} %\n",
                    r.tone_hz, r.speed_pct
                );
                s += &format!(
                    "Input trims now L {:+.1} dB, R {:+.1} dB; suggested for {:.1} dBFS: L {:+.2} dB, R {:+.2} dB\n",
                    trims_now[0], trims_now[1], self.target_dbfs, r.suggested_trim_db[0], r.suggested_trim_db[1]
                );
                s += &format!(
                    "Velocity calibration: reference level {:.2} dBFS = 5 cm/s peak lateral (channel velocity of the band {:.3} cm/s peak)\n",
                    r.reference_level_dbfs, r.channel_velocity_peak_cm_s
                );
                for w in &r.warnings {
                    s += &format!("Warning: {w}\n");
                }
            }
            None => s += "Reference band: not measured\n",
        }
        match self.response() {
            Ok(r) => {
                let f = |o: Option<DeviationStats>| {
                    o.map_or("n/a".to_string(), |d| {
                        format!(
                            "max {:+.2} / min {:+.2} / rms {:.2} dB",
                            d.max_db, d.min_db, d.rms_db
                        )
                    })
                };
                s += &format!(
                    "Response ({}, {}): covered {}\n  full range: {}\n  1-15 kHz: {}\n",
                    self.sweep_modulation()
                        .map_or("modulation unknown", |m| m.name()),
                    self.nominal_description(),
                    r.covered.map_or("n/a".to_string(), |(a, b)| format!(
                        "{a:.0} Hz .. {b:.0} Hz"
                    )),
                    f(r.stats_all),
                    f(r.stats_1_15k)
                );
            }
            Err(e) => s += &format!("Response: {e}\n"),
        }
        match &self.wf {
            Some(w) => {
                s += &format!(
                    "Wow & flutter ({:.0} Hz carrier, indicative; standards use 3150 Hz): speed {:+.2} %, unweighted peak {:.3} % rms {:.3} %, weighted peak {:.3} % WRMS {:.3} %\n",
                    w.carrier_hz, w.speed_pct, w.unweighted_peak_pct, w.unweighted_rms_pct, w.weighted_peak_pct, w.weighted_rms_pct
                );
            }
            None => s += "Wow & flutter: not measured\n",
        }
        if self.tracking.is_empty() {
            s += "Tracking: not measured\n";
        } else {
            let rep = evaluate_tracking(&self.tracking, self);
            s += &format!(
                "Tracking ({:.0} Hz): highest amplitude tracked cleanly {}\n",
                p.track_freq_hz,
                rep.highest_clean_um
                    .map_or("none".to_string(), |a| format!("{a:.0} um"))
            );
        }
        s
    }

    pub fn nominal_description(&self) -> String {
        let p = self.profile();
        let rec = match p.nominal {
            NominalKind::ConstantVelocity => "constant velocity".to_string(),
            NominalKind::FlatAfterRiaa => "flat after RIAA".to_string(),
            NominalKind::Turnover => format!("turnover {:.0} Hz", p.turnover_hz),
        };
        format!(
            "nominal: {rec}, preamp {}",
            if self.preamp_riaa { "RIAA" } else { "flat" }
        )
    }
}

impl Profile {
    /// "5 cm/s RMS" style description of the reference band.
    pub fn ref_velocity_label(&self) -> String {
        format!(
            "{:.2} cm/s {}",
            self.ref_velocity_cm_s,
            if self.ref_is_rms { "RMS" } else { "peak" }
        )
    }
}

/// Step 1 outcome ([`TestRecData::ref_calibration`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RefCalibration {
    /// Per channel level of the band with the current trims applied, dBFS.
    pub level_dbfs: [f64; 2],
    /// L minus R, dB.
    pub balance_db: f64,
    pub xtalk_l_to_r_db: Option<f64>,
    pub xtalk_r_to_l_db: Option<f64>,
    pub tone_hz: f64,
    /// `(tone / nominal - 1) * 100`.
    pub speed_pct: f64,
    /// Absolute trims (dB) that bring both channels to the target level.
    pub suggested_trim_db: [f64; 2],
    pub trims_clamped: bool,
    /// `reference_level` (dBFS) for the current levels.
    pub reference_level_dbfs: f64,
    pub channel_velocity_peak_cm_s: f64,
    pub warnings: Vec<String>,
}

/// Step 2 outcome ([`TestRecData::response`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// Deviation from nominal per channel `(freq, dB)` (smoothed if enabled).
    pub per_channel: [Vec<(f64, f64)>; 2],
    /// Mean of the channels carrying the sweep.
    pub average: Vec<(f64, f64)>,
    /// What [`curve_json`] exports and the statistics describe.
    pub selected: Vec<(f64, f64)>,
    pub covered: Option<(f64, f64)>,
    pub stats_all: Option<DeviationStats>,
    pub stats_1_15k: Option<DeviationStats>,
    /// Which channels carry the sweep.
    pub valid: [bool; 2],
}

#[derive(Serialize)]
struct PointJson {
    freq_hz: f64,
    db: f64,
}

#[derive(Serialize)]
struct CurveJson<'a> {
    version: u32,
    kind: &'a str,
    name: &'a str,
    points: Vec<PointJson>,
}

/// The export read by the EQ plugin's loop calibration:
/// `{"version":1,"kind":"response_curve","name":"...","points":[{"freq_hz":800.0,"db":-0.4},...]}`.
/// `points` must be ascending in frequency (they are sorted here); dB is the
/// deviation from nominal.
pub fn curve_json(name: &str, points: &[(f64, f64)]) -> String {
    let mut p: Vec<(f64, f64)> = points
        .iter()
        .copied()
        .filter(|q| q.0.is_finite() && q.1.is_finite())
        .collect();
    p.sort_by(|a, b| a.0.total_cmp(&b.0));
    let r = |v: f64, d: f64| (v * d).round() / d;
    let j = CurveJson {
        version: 1,
        kind: "response_curve",
        name,
        points: p
            .into_iter()
            .map(|(f, db)| PointJson {
                freq_hz: r(f, 10.0),
                db: r(db, 100.0) + 0.0,
            })
            .collect(),
    };
    serde_json::to_string(&j).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Measurements from captured audio

fn chan_from_analysis(a: &neo_measure::tone::ToneAnalysis, driven: bool) -> ChanTone {
    let thd = a.thd_percent / 100.0;
    let thdn = a.thd_n_percent / 100.0;
    let res = (thdn * thdn - thd * thd).max(1e-30).sqrt();
    ChanTone {
        freq_hz: a.frequency_hz,
        level_dbfs: a.level_dbfs,
        thd_percent: Some(a.thd_percent),
        thd_n_percent: Some(a.thd_n_percent),
        noise_dbfs: Some(a.level_dbfs + 20.0 * res.log10()),
        driven,
    }
}

fn crosstalk_chan(x: &[f64], fs: f64, f: f64) -> ChanTone {
    ChanTone {
        freq_hz: f,
        level_dbfs: tone_level_at(x, fs, f),
        thd_percent: None,
        thd_n_percent: None,
        noise_dbfs: None,
        driven: false,
    }
}

/// Analyse a reference band captured on both channels. `slot` is what the
/// user played; `trim_db` the input trims during the capture. `None` if there
/// is no tone on the driven channel(s).
pub fn measure_ref_band(
    l: &[f64],
    r: &[f64],
    fs: f64,
    slot: Mod,
    seconds: f64,
    trim_db: [f64; 2],
) -> Option<RefBand> {
    let cfg = ToneConfig::new(fs);
    let a = [analyze_tone(l, &cfg), analyze_tone(r, &cfg)];
    let driven = |c: usize| match slot {
        Mod::Left => c == 0,
        Mod::Right => c == 1,
        _ => true,
    };
    let f_d = match slot {
        Mod::Left => a[0].as_ref()?.frequency_hz,
        Mod::Right => a[1].as_ref()?.frequency_hz,
        _ => match (&a[0], &a[1]) {
            (Some(x), Some(y)) => {
                if x.level_dbfs >= y.level_dbfs {
                    x.frequency_hz
                } else {
                    y.frequency_hz
                }
            }
            (Some(x), None) | (None, Some(x)) => x.frequency_hz,
            _ => return None,
        },
    };
    let xs = [l, r];
    let ch: [ChanTone; 2] = std::array::from_fn(|c| match (&a[c], driven(c)) {
        (Some(an), true) => chan_from_analysis(an, true),
        _ => {
            let mut t = crosstalk_chan(xs[c], fs, f_d);
            t.driven = driven(c);
            t
        }
    });
    let phase_deg = phase_difference_deg(l, r, fs, f_d);
    let detected = detect_modulation(
        ch[0].level_dbfs - ch[1].level_dbfs,
        phase_deg.unwrap_or(f64::NAN),
    )
    .map(Mod::from_m);
    let separation_db = match slot {
        Mod::Left => channel_separation_db(l, r, fs),
        Mod::Right => channel_separation_db(r, l, fs),
        _ => None,
    };
    Some(RefBand {
        slot,
        seconds,
        ch,
        phase_deg,
        detected,
        separation_db,
        trim_db,
    })
}

/// Analyse a tracking band. A channel counts as driven when it is within
/// 20 dB of the stronger one.
pub fn measure_track_row(l: &[f64], r: &[f64], fs: f64, amp_um: f64) -> Option<TrackRow> {
    let cfg = ToneConfig::new(fs);
    let a = [analyze_tone(l, &cfg)?, analyze_tone(r, &cfg)?];
    let top = a[0].level_dbfs.max(a[1].level_dbfs);
    let ch = std::array::from_fn(|c| chan_from_analysis(&a[c], a[c].level_dbfs > top - 20.0));
    let freq_hz = if a[0].level_dbfs >= a[1].level_dbfs {
        a[0].frequency_hz
    } else {
        a[1].frequency_hz
    };
    Some(TrackRow {
        amp_um,
        freq_hz,
        ch,
    })
}

/// Wow and flutter of channel `channel` from a carrier of `carrier_hz`.
pub fn measure_wf(x: &[f64], fs: f64, carrier_hz: f64, channel: usize) -> Option<WfMeas> {
    let r = analyze_wow_flutter(x, &WowFlutterConfig::new(fs, carrier_hz))?;
    Some(WfMeas {
        channel,
        carrier_hz,
        mean_hz: r.mean_frequency_hz,
        speed_pct: r.speed_deviation_pct,
        unweighted_peak_pct: r.unweighted.peak_pct,
        unweighted_rms_pct: r.unweighted.rms_pct,
        unweighted_2sigma_pct: r.unweighted.two_sigma_pct,
        weighted_peak_pct: r.weighted.peak_pct,
        weighted_rms_pct: r.weighted.rms_pct,
        weighted_2sigma_pct: r.weighted.two_sigma_pct,
    })
}

// ---------------------------------------------------------------------------
// Tracking evaluation

/// One evaluated tracking band.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackEval {
    pub amp_um: f64,
    /// Expected peak velocity, cm/s.
    pub velocity_cm_s: f64,
    /// Expected level step versus the previous band, dB.
    pub expected_step_db: Option<f64>,
    /// Measured step of the driven channels versus the previous band (worst
    /// channel, i.e. the smallest), dB.
    pub measured_step_db: Option<f64>,
    pub mistracked: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackingReport {
    pub rows: Vec<TrackEval>,
    /// Highest amplitude such that it and every measured lower band are clean.
    pub highest_clean_um: Option<f64>,
}

/// Evaluate the tracking rows (ascending amplitude) with the thresholds of
/// `cfg` (THD limit, THD jump, level shortfall):
///
/// a band is *mistracked* if the THD of a driven channel exceeds the limit,
/// or the THD rose by more than the jump limit (dB) versus the previous band
/// while above a tenth of the limit, or the level step versus the previous
/// band falls short of the expected `20 log10(A / A_prev)` by more than the
/// shortfall limit.
pub fn evaluate_tracking(rows: &[TrackRow], cfg: &TestRecData) -> TrackingReport {
    let freq = cfg.profile().track_freq_hz;
    let mut sorted: Vec<&TrackRow> = rows.iter().collect();
    sorted.sort_by(|a, b| a.amp_um.total_cmp(&b.amp_um));
    let mut out: Vec<TrackEval> = Vec::new();
    let mut highest = None;
    let mut clean_so_far = true;
    for (i, row) in sorted.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| sorted[j]);
        let mut reasons = Vec::new();
        let mut exp_step = None;
        let mut meas_step: Option<f64> = None;
        if let Some(p) = prev {
            exp_step = Some(level_step_db(p.amp_um, row.amp_um));
        }
        for c in 0..2 {
            let t = &row.ch[c];
            if !t.driven {
                continue;
            }
            let name = ["L", "R"][c];
            if let Some(thd) = t.thd_percent {
                if thd > cfg.track_thd_pct {
                    reasons.push(format!(
                        "{name}: THD {thd:.2} % above {:.1} %",
                        cfg.track_thd_pct
                    ));
                }
                if let Some(pt) = prev.and_then(|p| p.ch[c].thd_percent) {
                    let jump = 20.0 * (thd.max(1e-9) / pt.max(1e-9)).log10();
                    if jump > cfg.track_jump_db && thd > 0.1 * cfg.track_thd_pct {
                        reasons.push(format!(
                            "{name}: THD up {jump:.1} dB versus the previous band"
                        ));
                    }
                }
            }
            if let (Some(p), Some(e)) = (prev, exp_step) {
                if p.ch[c].driven {
                    let m = t.level_dbfs - p.ch[c].level_dbfs;
                    meas_step = Some(meas_step.map_or(m, |x: f64| x.min(m)));
                    if e - m > cfg.track_shortfall_db {
                        reasons.push(format!(
                            "{name}: level step {m:+.2} dB, expected {e:+.2} dB"
                        ));
                    }
                }
            }
        }
        let bad = !reasons.is_empty();
        if bad {
            clean_so_far = false;
        }
        if clean_so_far {
            highest = Some(row.amp_um);
        }
        out.push(TrackEval {
            amp_um: row.amp_um,
            velocity_cm_s: amplitude_um_to_velocity_cm_s(freq, row.amp_um),
            expected_step_db: exp_step,
            measured_step_db: meas_step,
            mistracked: bad,
            reasons,
        });
    }
    TrackingReport {
        rows: out,
        highest_clean_um: highest,
    }
}

// ---------------------------------------------------------------------------
// Persisted state holder

/// Shared, persisted test-record state.
#[derive(Default)]
pub struct TestRecState {
    data: RwLock<TestRecData>,
}

impl TestRecState {
    pub fn get(&self) -> TestRecData {
        self.data.read().map(|d| d.clone()).unwrap_or_default()
    }

    pub fn replace(&self, new: TestRecData) {
        let new = new.sanitized();
        if let Ok(mut d) = self.data.write() {
            *d = new;
        }
    }

    /// Modify the data in place (analysis or GUI thread).
    pub fn update(&self, f: impl FnOnce(&mut TestRecData)) {
        if let Ok(mut d) = self.data.write() {
            f(&mut d);
            let new = std::mem::take(&mut *d).sanitized();
            *d = new;
        }
    }
}

impl<'a> PersistentField<'a, TestRecData> for TestRecState {
    fn set(&self, new_value: TestRecData) {
        self.replace(new_value);
    }
    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&TestRecData) -> R,
    {
        f(&self.get())
    }
}

impl<'a> PersistentField<'a, TestRecData> for Arc<TestRecState> {
    fn set(&self, new_value: TestRecData) {
        self.as_ref().replace(new_value);
    }
    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&TestRecData) -> R,
    {
        f(&self.get())
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn tone(f: f64, level: f64, driven: bool, thd: f64) -> ChanTone {
        ChanTone {
            freq_hz: f,
            level_dbfs: level,
            thd_percent: Some(thd),
            thd_n_percent: Some(thd),
            noise_dbfs: None,
            driven,
        }
    }

    fn row(amp: f64, level: f64, thd: f64) -> TrackRow {
        TrackRow {
            amp_um: amp,
            freq_hz: 315.0,
            ch: [tone(315.0, level, true, thd), tone(315.0, level, true, thd)],
        }
    }

    #[test]
    fn ortofon_profile_values() {
        let p = Profile::ortofon();
        assert_eq!(p.ref_freq_hz, 1000.0);
        assert!(p.ref_is_rms);
        assert_eq!(p.sweep_lo_hz, 800.0);
        assert_eq!(p.track_amps_um, [50.0, 60.0, 70.0, 80.0, 90.0, 100.0]);
        // 5 cm/s RMS -> 7.071 cm/s peak per channel (L-only / R-only bands)
        assert!((p.ref_channel_velocity_peak_cm_s(Mod::Left) - 7.0711).abs() < 1e-4);
        let mut c = Profile::custom();
        c.ref_velocity_cm_s = 5.0;
        // 5 cm/s peak lateral -> 3.536 cm/s per channel
        assert!((c.ref_channel_velocity_peak_cm_s(Mod::Lateral) - 3.5355).abs() < 1e-4);
    }

    #[test]
    fn tracking_flags_and_highest_clean() {
        let d = TestRecData::default();
        // levels follow 20 log10(A); THD rises at 80 um
        let lv = |a: f64| -30.0 + 20.0 * (a / 50.0).log10();
        let rows = vec![
            row(50.0, lv(50.0), 0.4),
            row(60.0, lv(60.0), 0.45),
            row(70.0, lv(70.0), 0.6),
            row(80.0, lv(80.0) - 0.4, 4.5),
            row(90.0, lv(90.0) - 3.0, 12.0),
            row(100.0, lv(100.0) - 6.0, 20.0),
        ];
        let r = evaluate_tracking(&rows, &d);
        let bad: Vec<bool> = r.rows.iter().map(|e| e.mistracked).collect();
        assert_eq!(bad, [false, false, false, true, true, true], "{:?}", r.rows);
        assert_eq!(r.highest_clean_um, Some(70.0));
        assert!((r.rows[0].velocity_cm_s - 9.896).abs() < 1e-3);
        assert!((r.rows[1].expected_step_db.unwrap() - 1.5836).abs() < 1e-3);
        assert!(r.rows[0].expected_step_db.is_none());
        // level shortfall alone (THD fine) is flagged too
        let rows = vec![row(50.0, -30.0, 0.3), row(60.0, -30.0 + 0.2, 0.3)];
        let r = evaluate_tracking(&rows, &d);
        assert!(r.rows[1].mistracked);
        assert_eq!(r.highest_clean_um, Some(50.0));
        // a THD jump of > 6 dB flags even below the absolute limit
        let rows = vec![row(50.0, -30.0, 0.3), row(60.0, -30.0 + 1.5836, 1.0)];
        assert!(evaluate_tracking(&rows, &d).rows[1].mistracked);
        // undriven channels are ignored
        let mut rr = row(60.0, -30.0 + 1.5836, 0.3);
        rr.ch[1] = tone(315.0, -80.0, false, 50.0);
        let mut r0 = row(50.0, -30.0, 0.3);
        r0.ch[1] = tone(315.0, -80.0, false, 50.0);
        assert_eq!(
            evaluate_tracking(&[r0, rr], &d).highest_clean_um,
            Some(60.0)
        );
    }

    #[test]
    fn curve_json_format() {
        let j = curve_json(
            "My \"chain\"",
            &[(1000.04, 0.123_456), (800.0, -0.4), (f64::NAN, 1.0)],
        );
        assert_eq!(
            j,
            r#"{"version":1,"kind":"response_curve","name":"My \"chain\"","points":[{"freq_hz":800.0,"db":-0.4},{"freq_hz":1000.0,"db":0.12}]}"#
        );
    }

    #[test]
    fn state_json_round_trip_and_sanitising() {
        let mut d = TestRecData::default();
        d.sweep = Some(CapturedSweep {
            cells: [vec![(3, 810.0, -20.0)], vec![(3, 811.0, -21.0)]],
            trim_db: [1.0, -2.0],
            detected: Some(Mod::Lateral),
            level_diff_db: Some(0.1),
            phase_deg: Some(2.0),
            pair_blocks: 10,
        });
        d.tracking.push(row(50.0, -20.0, 0.1));
        let s = serde_json::to_string(&d).unwrap();
        let back: TestRecData = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
        let partial: TestRecData = serde_json::from_str(r#"{"ref_seconds":10.0}"#).unwrap();
        assert_eq!(partial.ref_seconds, 10.0);
        assert_eq!(partial.target_dbfs, -18.0);
        let mut bad = TestRecData::default();
        bad.ref_seconds = f64::NAN;
        bad.target_dbfs = 5.0;
        bad.wf_channel = 9;
        bad.track_sel = 99;
        bad.ortofon.track_amps_um = vec![f64::NAN, -1.0];
        let s = bad.sanitized();
        assert_eq!(s.ref_seconds, 5.0);
        assert_eq!(s.target_dbfs, -1.0);
        assert_eq!(s.wf_channel, 1);
        assert_eq!(s.ortofon.track_amps_um.len(), 6);
        assert_eq!(s.track_sel, 5);
    }

    #[test]
    fn calibration_from_stored_bands() {
        let mk = |slot, l, r, trim: [f64; 2]| RefBand {
            slot,
            seconds: 5.0,
            ch: [
                tone(1002.0, l, slot == Mod::Left, 0.1),
                tone(1002.0, r, slot == Mod::Right, 0.1),
            ],
            phase_deg: Some(0.0),
            detected: Some(slot),
            separation_db: Some(28.0),
            trim_db: trim,
        };
        let mut d = TestRecData::default();
        assert!(d.ref_calibration([0.0; 2]).is_none());
        d.set_ref_band(mk(Mod::Left, -20.5, -48.5, [0.0, 0.0]));
        assert!(
            d.ref_calibration([0.0; 2]).is_none(),
            "needs the right band"
        );
        d.set_ref_band(mk(Mod::Right, -50.2, -19.2, [0.0, 0.0]));
        let c = d.ref_calibration([0.0; 2]).unwrap();
        assert!((c.balance_db + 1.3).abs() < 1e-9);
        assert!((c.speed_pct - 0.2).abs() < 1e-9);
        assert!((c.suggested_trim_db[0] - 2.5).abs() < 1e-9);
        assert!((c.suggested_trim_db[1] - 1.2).abs() < 1e-9);
        // with the suggested trims applied the levels are on target and the
        // velocity calibration is target - 6.02 dB
        let c2 = d.ref_calibration(c.suggested_trim_db).unwrap();
        assert!((c2.level_dbfs[0] + 18.0).abs() < 1e-9 && (c2.level_dbfs[1] + 18.0).abs() < 1e-9);
        assert!((c2.reference_level_dbfs - (-18.0 - 6.0206)).abs() < 1e-3);
        assert!((c2.suggested_trim_db[0] - c.suggested_trim_db[0]).abs() < 1e-9);
        // clamping
        d.target_dbfs = -1.0;
        d.set_ref_band(mk(Mod::Left, -30.0, -58.0, [0.0, 0.0]));
        let c = d.ref_calibration([0.0; 2]).unwrap();
        assert!(c.trims_clamped);
        assert_eq!(c.suggested_trim_db[0], 24.0);
    }
}
