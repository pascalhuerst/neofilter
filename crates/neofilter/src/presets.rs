//! Factory presets (const tables from PLAN.md) and user preset (de)serialisation.
//!
//! Band index N of the plan tables is array index N-1. Gain scale is in percent
//! (matching the `gain_scale` parameter).

use crate::params::{
    BandPlace, BandShape, BandSlope, NeoFilterParams, GAIN_SCALE_MAX_PCT, OUT_GAIN_MAX_DB,
};
use neofilter_dsp::types::{MAX_FREQ_HZ, MAX_GAIN_DB, MAX_Q, MIN_FREQ_HZ, MIN_Q, NUM_BANDS};
use neofilter_dsp::{BandParams, Placement, Shape, Slope};
use nih_plug::prelude::*;
use serde::{Deserialize, Serialize};

/// One used band of a factory preset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresetBand {
    pub shape: Shape,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
    pub slope: Slope,
    pub placement: Placement,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FactoryPreset {
    pub id: &'static str,
    pub name: &'static str,
    /// `None` = unused band (reset to defaults when applied).
    pub bands: [Option<PresetBand>; NUM_BANDS],
    /// Gain scale in percent.
    pub gain_scale: f32,
    pub out_gain_db: f32,
}

impl FactoryPreset {
    /// DSP view of all bands (pure data, no plugin framework needed).
    pub fn band_params(&self) -> [BandParams; NUM_BANDS] {
        std::array::from_fn(|i| match &self.bands[i] {
            None => BandParams::default(),
            Some(b) => BandParams {
                used: true,
                enabled: b.enabled,
                shape: b.shape,
                freq_hz: b.freq_hz,
                gain_db: b.gain_db,
                q: b.q,
                slope: b.slope,
                placement: b.placement,
            },
        })
    }

    /// Convert to a user preset file structure.
    pub fn to_file(&self) -> PresetFile {
        PresetFile::from_band_params(
            self.name,
            self.gain_scale,
            self.out_gain_db,
            &self.band_params(),
        )
    }
}

const fn b(shape: Shape, freq_hz: f64, gain_db: f64, q: f64, slope: Slope) -> Option<PresetBand> {
    Some(PresetBand {
        shape,
        freq_hz,
        gain_db,
        q,
        slope,
        placement: Placement::Stereo,
        enabled: true,
    })
}

const fn bell(freq_hz: f64, gain_db: f64, q: f64, slope: Slope) -> Option<PresetBand> {
    b(Shape::Bell, freq_hz, gain_db, q, slope)
}

const fn disabled(p: Option<PresetBand>) -> Option<PresetBand> {
    match p {
        Some(mut x) => {
            x.enabled = false;
            Some(x)
        }
        None => None,
    }
}

const N: Option<PresetBand> = None;

const DEFAULT: FactoryPreset = FactoryPreset {
    id: "default",
    name: "Default",
    bands: [N; NUM_BANDS],
    gain_scale: 100.0,
    out_gain_db: 0.0,
};

/// Friend EQ A: bands 1..=15 (the rest of the 32 slots are unused).
const A_BANDS: [Option<PresetBand>; 15] = [
    bell(41.1, 2.58, 1.00, Slope::Db12),
    b(Shape::HighShelf, 722.9, -1.32, 1.00, Slope::Db12),
    bell(98.0, -1.83, 2.73, Slope::Db24),
    bell(35.9, 2.54, 2.75, Slope::Db24),
    bell(9729.0, 2.46, 3.24, Slope::Db48),
    bell(233.0, -2.50, 1.00, Slope::Db12),
    b(Shape::LowCut, 28.6, 0.0, 1.00, Slope::Db12),
    bell(177.7, -5.38, 0.68, Slope::Db12),
    bell(2093.0, -2.99, 1.25, Slope::Db48),
    bell(5074.0, 4.90, 10.91, Slope::Db24),
    disabled(b(Shape::HighCut, 4657.0, 0.0, 1.00, Slope::Db6)),
    bell(392.7, -2.87, 2.97, Slope::Db12),
    bell(2035.0, 2.78, 5.00, Slope::Db48),
    bell(11787.0, 2.45, 4.59, Slope::Db48),
    bell(15484.0, 15.69, 7.23, Slope::Db24),
];

/// Friend EQ B: bands 1..=12 (bands 1, 3, 7 are unused in the table).
const B_BANDS: [Option<PresetBand>; 12] = [
    N,
    b(Shape::HighCut, 13607.0, 0.0, 1.00, Slope::Db48),
    N,
    bell(4317.0, 10.58, 1.05, Slope::Db24),
    bell(85.7, 7.62, 1.77, Slope::Db24),
    bell(701.3, -7.36, 2.01, Slope::Db12),
    N,
    bell(15959.0, 8.51, 14.14, Slope::Db48),
    bell(13195.0, -4.62, 10.00, Slope::Db36),
    bell(6282.0, -3.54, 10.00, Slope::Db36),
    bell(10011.0, 2.64, 5.95, Slope::Db12),
    bell(165.7, 2.06, 2.97, Slope::Db48),
];

/// 0-based indices into `B_BANDS` of the bands carried into the combined
/// preset (B2, B4, B5, B6, B8, B9, B10, B11, B12), in table order.
const AB_B_SOURCE: [usize; 9] = [1, 3, 4, 5, 7, 8, 9, 10, 11];

/// Band table padded with unused slots up to `NUM_BANDS`.
const fn pad<const L: usize>(src: &[Option<PresetBand>; L]) -> [Option<PresetBand>; NUM_BANDS] {
    let mut out = [N; NUM_BANDS];
    let mut i = 0;
    while i < L {
        out[i] = src[i];
        i += 1;
    }
    out
}

/// Friend EQ A followed by Friend EQ B in one instance (the two EQs are linear
/// and run in series, so one instance with all their bands is equivalent).
///
/// Band mapping (1-based):
/// * bands 1..=15 = A's bands 1..=15 (band 11 is used but disabled)
/// * bands 16..=24 = B's used bands in table order:
///   B2 -> 16, B4 -> 17, B5 -> 18, B6 -> 19, B8 -> 20, B9 -> 21, B10 -> 22,
///   B11 -> 23, B12 -> 24
/// * bands 25..=32 are unused spares
const fn combine_ab() -> [Option<PresetBand>; NUM_BANDS] {
    let mut out = pad(&A_BANDS);
    let mut k = 0;
    while k < AB_B_SOURCE.len() {
        out[A_BANDS.len() + k] = B_BANDS[AB_B_SOURCE[k]];
        k += 1;
    }
    out
}

const FRIEND_EQ_A: FactoryPreset = FactoryPreset {
    id: "friend-eq-a",
    name: "Friend EQ A",
    bands: pad(&A_BANDS),
    gain_scale: 100.0,
    out_gain_db: 0.0,
};

const FRIEND_EQ_B: FactoryPreset = FactoryPreset {
    id: "friend-eq-b",
    name: "Friend EQ B",
    bands: pad(&B_BANDS),
    gain_scale: 100.0,
    out_gain_db: 0.0,
};

const FRIEND_EQ_AB: FactoryPreset = FactoryPreset {
    id: "friend-eq-ab",
    name: "Friend EQ A+B (series)",
    bands: combine_ab(),
    gain_scale: 100.0,
    out_gain_db: 0.0,
};

static FACTORY_PRESETS: [FactoryPreset; 4] = [DEFAULT, FRIEND_EQ_A, FRIEND_EQ_B, FRIEND_EQ_AB];

/// All factory presets: Default, Friend EQ A, Friend EQ B, Friend EQ A+B.
pub fn factory_presets() -> &'static [FactoryPreset] {
    &FACTORY_PRESETS
}

fn put<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

/// Set every band parameter plus gain scale / output gain through gestures.
fn apply_state(
    bands: &[BandParams; NUM_BANDS],
    gain_scale: f32,
    out_gain_db: f32,
    params: &NeoFilterParams,
    setter: &ParamSetter,
) {
    for (set, p) in params.bands.iter().zip(bands) {
        put(setter, &set.used, p.used);
        put(setter, &set.on, p.enabled);
        put(setter, &set.shape, BandShape::from(p.shape));
        put(setter, &set.freq, p.freq_hz as f32);
        put(setter, &set.gain, p.gain_db as f32);
        put(setter, &set.q, p.q as f32);
        put(setter, &set.slope, BandSlope::from(p.slope));
        put(setter, &set.place, BandPlace::from(p.placement));
    }
    put(setter, &params.gain_scale, gain_scale);
    put(setter, &params.out_gain, out_gain_db);
}

/// Apply a factory preset; unused bands are reset to defaults.
pub fn apply(preset: &FactoryPreset, params: &NeoFilterParams, setter: &ParamSetter) {
    apply_state(
        &preset.band_params(),
        preset.gain_scale,
        preset.out_gain_db,
        params,
        setter,
    );
}

/// Apply a user preset file; bands not in the file are reset to defaults.
pub fn apply_file(file: &PresetFile, params: &NeoFilterParams, setter: &ParamSetter) {
    apply_state(
        &file.band_params(),
        file.gain_scale,
        file.out_gain_db,
        params,
        setter,
    );
}

/// Current preset file format version.
pub const PRESET_FILE_VERSION: u32 = 1;

fn d_version() -> u32 {
    PRESET_FILE_VERSION
}
fn d_gain_scale() -> f32 {
    100.0
}
fn d_true() -> bool {
    true
}
fn d_shape() -> String {
    "bell".into()
}
fn d_freq() -> f64 {
    1000.0
}
fn d_q() -> f64 {
    1.0
}
fn d_slope() -> u32 {
    12
}
fn d_place() -> String {
    "stereo".into()
}

/// One band of a user preset. `band` is the 1-based band number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetFileBand {
    pub band: usize,
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_shape")]
    pub shape: String,
    #[serde(default = "d_freq")]
    pub freq_hz: f64,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default = "d_q")]
    pub q: f64,
    #[serde(default = "d_slope")]
    pub slope: u32,
    #[serde(default = "d_place")]
    pub placement: String,
}

/// User preset file (JSON). Only used bands are stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetFile {
    #[serde(default = "d_version")]
    pub version: u32,
    #[serde(default)]
    pub name: String,
    /// Percent, 0..200.
    #[serde(default = "d_gain_scale")]
    pub gain_scale: f32,
    #[serde(default)]
    pub out_gain_db: f32,
    #[serde(default)]
    pub bands: Vec<PresetFileBand>,
}

/// Stable string id of a shape.
pub fn shape_id(s: Shape) -> &'static str {
    match s {
        Shape::Bell => "bell",
        Shape::LowShelf => "low_shelf",
        Shape::LowCut => "low_cut",
        Shape::HighShelf => "high_shelf",
        Shape::HighCut => "high_cut",
        Shape::Notch => "notch",
        Shape::BandPass => "band_pass",
        Shape::TiltShelf => "tilt_shelf",
        Shape::FlatTilt => "flat_tilt",
    }
}

fn shape_from_id(s: &str) -> Option<Shape> {
    Shape::ALL.into_iter().find(|x| shape_id(*x) == s)
}

/// Stable string id of a placement.
pub fn placement_id(p: Placement) -> &'static str {
    match p {
        Placement::Stereo => "stereo",
        Placement::Left => "left",
        Placement::Right => "right",
        Placement::Mid => "mid",
        Placement::Side => "side",
    }
}

fn placement_from_id(s: &str) -> Option<Placement> {
    Placement::ALL.into_iter().find(|x| placement_id(*x) == s)
}

/// Nearest supported slope to a dB/oct value.
fn nearest_slope(v: u32) -> Slope {
    Slope::ALL
        .into_iter()
        .min_by_key(|s| s.db_per_oct().abs_diff(v))
        .unwrap_or(Slope::Db12)
}

fn clamp_finite(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl PresetFile {
    pub fn from_band_params(
        name: &str,
        gain_scale: f32,
        out_gain_db: f32,
        bands: &[BandParams; NUM_BANDS],
    ) -> Self {
        Self {
            version: PRESET_FILE_VERSION,
            name: name.to_string(),
            gain_scale,
            out_gain_db,
            bands: bands
                .iter()
                .enumerate()
                .filter(|(_, p)| p.used)
                .map(|(i, p)| PresetFileBand {
                    band: i + 1,
                    enabled: p.enabled,
                    shape: shape_id(p.shape).to_string(),
                    freq_hz: p.freq_hz,
                    gain_db: p.gain_db,
                    q: p.q,
                    slope: p.slope.db_per_oct(),
                    placement: placement_id(p.placement).to_string(),
                })
                .collect(),
        }
    }

    /// Band parameters for all 32 slots; bands absent from the file are default (unused).
    /// Values are clamped to their ranges; unknown shape/placement strings fall back
    /// to defaults (`from_json` rejects them beforehand).
    pub fn band_params(&self) -> [BandParams; NUM_BANDS] {
        let mut out = [BandParams::default(); NUM_BANDS];
        for fb in &self.bands {
            if !(1..=NUM_BANDS).contains(&fb.band) {
                continue;
            }
            out[fb.band - 1] = BandParams {
                used: true,
                enabled: fb.enabled,
                shape: shape_from_id(&fb.shape).unwrap_or(Shape::Bell),
                freq_hz: clamp_finite(fb.freq_hz, MIN_FREQ_HZ, MAX_FREQ_HZ, 1000.0),
                gain_db: clamp_finite(fb.gain_db, -MAX_GAIN_DB, MAX_GAIN_DB, 0.0),
                q: clamp_finite(fb.q, MIN_Q, MAX_Q, 1.0),
                slope: nearest_slope(fb.slope),
                placement: placement_from_id(&fb.placement).unwrap_or(Placement::Stereo),
            };
        }
        out
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Parse a preset. Missing optional fields get defaults, out-of-range numbers are
    /// clamped; unknown shape/placement ids and band numbers outside 1..=32 are errors.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut f: PresetFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        f.gain_scale = clamp_finite(
            f64::from(f.gain_scale),
            0.0,
            f64::from(GAIN_SCALE_MAX_PCT),
            100.0,
        ) as f32;
        f.out_gain_db = clamp_finite(
            f64::from(f.out_gain_db),
            -f64::from(OUT_GAIN_MAX_DB),
            f64::from(OUT_GAIN_MAX_DB),
            0.0,
        ) as f32;
        for fb in &mut f.bands {
            if !(1..=NUM_BANDS).contains(&fb.band) {
                return Err(format!(
                    "band number {} out of range 1..={NUM_BANDS}",
                    fb.band
                ));
            }
            if shape_from_id(&fb.shape).is_none() {
                return Err(format!("unknown shape '{}'", fb.shape));
            }
            if placement_from_id(&fb.placement).is_none() {
                return Err(format!("unknown placement '{}'", fb.placement));
            }
            fb.freq_hz = clamp_finite(fb.freq_hz, MIN_FREQ_HZ, MAX_FREQ_HZ, 1000.0);
            fb.gain_db = clamp_finite(fb.gain_db, -MAX_GAIN_DB, MAX_GAIN_DB, 0.0);
            fb.q = clamp_finite(fb.q, MIN_Q, MAX_Q, 1.0);
            fb.slope = nearest_slope(fb.slope).db_per_oct();
        }
        Ok(f)
    }
}
