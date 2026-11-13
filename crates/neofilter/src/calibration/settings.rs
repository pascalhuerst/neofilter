//! Persisted calibration settings, the band ownership mask and the
//! playback-chain correction curve.

use serde::{Deserialize, Serialize};

/// Highest noise level the generator accepts (dBFS RMS).
pub const NOISE_LEVEL_MAX_DB: f64 = -6.0;
/// Lowest noise level (dBFS RMS).
pub const NOISE_LEVEL_MIN_DB: f64 = -80.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NoiseKind {
    #[default]
    Pink,
    White,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Routing {
    Left,
    Right,
    /// Identical noise on both channels (mono, the default).
    #[default]
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InjectMode {
    /// The selected channel(s) carry only noise (the other one is muted).
    #[default]
    Replace,
    /// Noise is added to the programme.
    Add,
}

/// User options of the calibration. Persisted with the plugin state; every
/// field has a default so older/newer state files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalSettings {
    pub noise: NoiseKind,
    /// RMS level in dBFS, capped at [`NOISE_LEVEL_MAX_DB`].
    pub level_db: f64,
    pub routing: Routing,
    pub mode: InjectMode,
    /// Measurement length after the loop delay, seconds (4..=60).
    pub duration_s: f64,
    pub ref_lo_hz: f64,
    pub ref_hi_hz: f64,
    pub range_lo_hz: f64,
    pub range_hi_hz: f64,
    /// Limits apply to the total calibration curve.
    pub max_boost_db: f64,
    pub max_cut_db: f64,
    /// Smoothing: 1/N octave (0 = none).
    pub smoothing: u32,
    pub max_bands: usize,
    /// Allow one low and one high shelf in the fit.
    pub use_shelves: bool,
    /// Every band is a calibration band.
    pub replace_all: bool,
    /// Points below this coherence get zero weight.
    pub coherence_min: f64,
    /// Mean in-range coherence below this aborts the measurement.
    pub coherence_abort: f64,
    pub low_level_db: f64,
    pub clip_db: f64,
    /// Minimum delay-estimate confidence (peak to sidelobe ratio).
    pub delay_confidence_min: f64,
    pub max_delay_s: f64,
    /// Auto mode: stop when the in-range RMS deviation is below this.
    pub auto_threshold_db: f64,
    pub auto_max_iterations: usize,
    /// Settle time after Apply is `2 * delay + settle_extra_s`.
    pub settle_extra_s: f64,
}

impl Default for CalSettings {
    fn default() -> Self {
        Self {
            noise: NoiseKind::Pink,
            level_db: -20.0,
            routing: Routing::Both,
            mode: InjectMode::Replace,
            duration_s: 12.0,
            ref_lo_hz: 300.0,
            ref_hi_hz: 3000.0,
            range_lo_hz: 30.0,
            range_hi_hz: 16_000.0,
            max_boost_db: 6.0,
            max_cut_db: 12.0,
            smoothing: 6,
            max_bands: 12,
            use_shelves: true,
            replace_all: false,
            coherence_min: 0.7,
            coherence_abort: 0.5,
            low_level_db: -60.0,
            clip_db: -0.1,
            delay_confidence_min: 3.0,
            max_delay_s: 5.0,
            auto_threshold_db: 0.5,
            auto_max_iterations: 5,
            settle_extra_s: 1.0,
        }
    }
}

fn fin(v: f64, lo: f64, hi: f64, d: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        d
    }
}

impl CalSettings {
    /// Clamp every value into its valid range (used after loading state or
    /// editing in the GUI).
    pub fn sanitize(&mut self) {
        let d = Self::default();
        self.level_db = fin(
            self.level_db,
            NOISE_LEVEL_MIN_DB,
            NOISE_LEVEL_MAX_DB,
            d.level_db,
        );
        self.duration_s = fin(self.duration_s, 4.0, 60.0, d.duration_s);
        self.ref_lo_hz = fin(self.ref_lo_hz, 20.0, 10_000.0, d.ref_lo_hz);
        self.ref_hi_hz = fin(self.ref_hi_hz, self.ref_lo_hz * 1.2, 20_000.0, d.ref_hi_hz);
        self.range_lo_hz = fin(self.range_lo_hz, 20.0, 1000.0, d.range_lo_hz);
        self.range_hi_hz = fin(
            self.range_hi_hz,
            self.range_lo_hz * 2.0,
            20_000.0,
            d.range_hi_hz,
        );
        self.max_boost_db = fin(self.max_boost_db, 0.0, 12.0, d.max_boost_db);
        self.max_cut_db = fin(self.max_cut_db, 0.0, 24.0, d.max_cut_db);
        self.smoothing = self.smoothing.min(48);
        self.max_bands = self.max_bands.clamp(1, 32);
        self.coherence_min = fin(self.coherence_min, 0.0, 0.99, d.coherence_min);
        self.coherence_abort = fin(self.coherence_abort, 0.0, 0.99, d.coherence_abort);
        self.low_level_db = fin(self.low_level_db, -120.0, -20.0, d.low_level_db);
        self.clip_db = fin(self.clip_db, -6.0, 0.0, d.clip_db);
        self.delay_confidence_min =
            fin(self.delay_confidence_min, 1.5, 50.0, d.delay_confidence_min);
        self.max_delay_s = fin(self.max_delay_s, 0.5, 5.0, d.max_delay_s);
        self.auto_threshold_db = fin(self.auto_threshold_db, 0.05, 5.0, d.auto_threshold_db);
        self.auto_max_iterations = self.auto_max_iterations.clamp(1, 20);
        self.settle_extra_s = fin(self.settle_extra_s, 0.0, 30.0, d.settle_extra_s);
    }
}

/// Octaves over which a partial correction curve fades to 0 dB outside its range.
pub const TAPER_OCTAVES: f64 = 1.0;

/// One point of a response curve file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub freq_hz: f64,
    pub db: f64,
}

/// Playback-chain correction: the (known) response of pickup and phono
/// preamp in dB relative to nominal, shared with the metering plugin.
///
/// ```json
/// {"version":1,"kind":"response_curve","name":"...","points":[{"freq_hz":20.0,"db":-0.4}]}
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalCurve {
    pub version: u32,
    pub kind: String,
    #[serde(default)]
    pub name: String,
    pub points: Vec<CurvePoint>,
}

impl CalCurve {
    pub fn new(name: &str, points: Vec<CurvePoint>) -> Self {
        Self {
            version: 1,
            kind: "response_curve".into(),
            name: name.into(),
            points,
        }
    }

    /// Parse and validate (version 1, kind `response_curve`, at least two
    /// finite points with strictly ascending positive frequencies).
    pub fn from_json(text: &str) -> Result<Self, String> {
        let c: Self = serde_json::from_str(text.trim()).map_err(|e| e.to_string())?;
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("unsupported curve version {}", self.version));
        }
        if self.kind != "response_curve" {
            return Err(format!("not a response curve (kind '{}')", self.kind));
        }
        if self.points.len() < 2 {
            return Err("curve needs at least two points".into());
        }
        for p in &self.points {
            if !(p.freq_hz.is_finite() && p.freq_hz > 0.0 && p.db.is_finite()) {
                return Err("curve contains invalid numbers".into());
            }
        }
        if self.points.windows(2).any(|w| w[1].freq_hz <= w[0].freq_hz) {
            return Err("curve frequencies must be ascending".into());
        }
        Ok(())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// First and last frequency of the curve (the covered range).
    pub fn covered_range(&self) -> Option<(f64, f64)> {
        Some((self.points.first()?.freq_hz, self.points.last()?.freq_hz))
    }

    /// Deviation in dB at `f` Hz: linear interpolation over log frequency
    /// inside the covered range; outside it the end value tapers linearly
    /// (over log frequency) to 0 dB within [`TAPER_OCTAVES`] and stays 0 dB
    /// beyond. A partial curve (a test record that only covers >= 800 Hz)
    /// therefore never pushes a non-zero end value into the uncovered range.
    pub fn eval(&self, f: f64) -> f64 {
        let p = &self.points;
        let Some(first) = p.first() else { return 0.0 };
        let last = p[p.len() - 1];
        if f.is_nan() || f <= 0.0 {
            return 0.0;
        }
        if f < first.freq_hz {
            let t = 1.0 - (first.freq_hz / f).log2() / TAPER_OCTAVES;
            return first.db * t.max(0.0);
        }
        if f > last.freq_hz {
            let t = 1.0 - (f / last.freq_hz).log2() / TAPER_OCTAVES;
            return last.db * t.max(0.0);
        }
        let i = p.partition_point(|q| q.freq_hz <= f).clamp(1, p.len() - 1);
        let (a, b) = (p[i - 1], p[i]);
        let t = (f / a.freq_hz).ln() / (b.freq_hz / a.freq_hz).ln();
        a.db + t * (b.db - a.db)
    }
}

/// Summary of the last measurement, persisted for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LoopSummary {
    pub delay_ms: [Option<f64>; 2],
    pub rms_deviation_db: f64,
    pub max_deviation_db: f64,
    pub bands: usize,
    pub note: String,
}

/// Everything persisted in the plugin state (never a "running" flag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CalPersist {
    pub settings: CalSettings,
    /// Bit `i` set: band slot `i` belongs to the calibration.
    pub band_mask: u32,
    pub correction: Option<CalCurve>,
    pub last: Option<LoopSummary>,
}

impl CalPersist {
    pub fn sanitized(mut self) -> Self {
        self.settings.sanitize();
        if let Some(c) = &self.correction {
            if c.validate().is_err() {
                self.correction = None;
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_roundtrip_and_interpolation() {
        let json = r#"{"version":1,"kind":"response_curve","name":"cart","points":[
            {"freq_hz":20.0,"db":-1.0},{"freq_hz":200.0,"db":0.0},{"freq_hz":20000.0,"db":2.0}]}"#;
        let c = CalCurve::from_json(json).unwrap();
        assert_eq!(c.name, "cart");
        assert!((c.eval(20.0) + 1.0).abs() < 1e-12);
        assert!((c.eval(20000.0) - 2.0).abs() < 1e-12);
        assert!((c.eval(63.2455532) + 0.5).abs() < 1e-6); // log-midpoint 20..200
        assert!((c.eval(2000.0) - 1.0).abs() < 1e-9);
        assert_eq!(CalCurve::from_json(&c.to_json()).unwrap(), c);
    }

    #[test]
    fn partial_curve_tapers_to_zero_over_one_octave() {
        let c = CalCurve::new(
            "record",
            vec![
                CurvePoint {
                    freq_hz: 800.0,
                    db: -2.0,
                },
                CurvePoint {
                    freq_hz: 8000.0,
                    db: 1.0,
                },
            ],
        );
        assert_eq!(c.covered_range(), Some((800.0, 8000.0)));
        assert!((c.eval(800.0) + 2.0).abs() < 1e-12);
        assert!((c.eval(400.0 * 2f64.sqrt()) + 1.0).abs() < 1e-9); // half an octave below
        assert_eq!(c.eval(400.0), 0.0);
        assert_eq!(c.eval(100.0), 0.0);
        assert!((c.eval(8000.0 * 2f64.sqrt()) - 0.5).abs() < 1e-9);
        assert_eq!(c.eval(16000.0), 0.0);
        assert_eq!(c.eval(20000.0), 0.0);
    }

    #[test]
    fn curve_rejects_bad_input() {
        assert!(CalCurve::from_json("{}").is_err());
        let bad = |s: &str| CalCurve::from_json(s).is_err();
        assert!(bad(
            r#"{"version":2,"kind":"response_curve","points":[{"freq_hz":1,"db":0},{"freq_hz":2,"db":0}]}"#
        ));
        assert!(bad(
            r#"{"version":1,"kind":"other","points":[{"freq_hz":1,"db":0},{"freq_hz":2,"db":0}]}"#
        ));
        assert!(bad(
            r#"{"version":1,"kind":"response_curve","points":[{"freq_hz":2,"db":0},{"freq_hz":1,"db":0}]}"#
        ));
        assert!(bad(
            r#"{"version":1,"kind":"response_curve","points":[{"freq_hz":1,"db":0}]}"#
        ));
        assert!(bad(
            r#"{"version":1,"kind":"response_curve","points":[{"freq_hz":-1,"db":0},{"freq_hz":2,"db":0}]}"#
        ));
    }

    #[test]
    fn settings_sanitize_and_defaults() {
        let mut s = CalSettings {
            level_db: 3.0,
            duration_s: 1000.0,
            max_bands: 0,
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.level_db, NOISE_LEVEL_MAX_DB);
        assert_eq!(s.duration_s, 60.0);
        assert_eq!(s.max_bands, 1);
        let d = CalSettings::default();
        assert_eq!(d.level_db, -20.0);
        assert_eq!(d.routing, Routing::Both);
        assert_eq!(d.mode, InjectMode::Replace);
        assert_eq!(d.duration_s, 12.0);
    }

    #[test]
    fn persist_loads_partial_json() {
        let p: CalPersist = serde_json::from_str(r#"{"band_mask":5}"#).unwrap();
        assert_eq!(p.band_mask, 5);
        assert_eq!(p.settings, CalSettings::default());
        let p: CalPersist = serde_json::from_str("{}").unwrap();
        assert_eq!(p, CalPersist::default());
    }
}
