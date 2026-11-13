//! Plugin parameters.
//!
//! Parameter ids are frozen (see CLAUDE.md). nih-plug's nested arrays generate
//! `{field}_{n}` (n = 1..=32) in group `Band {n}`:
//! `used_n`, `on_n`, `shape_n`, `freq_n`, `gain_n`, `q_n`, `slope_n`, `place_n`.
//! Globals: `bypass`, `out_gain` (dB), `gain_scale` (percent, 0..200).

use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, Placement, Shape, Slope};
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::Arc;

/// Plugin-side mirror of [`Shape`] with stable ids for saved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum BandShape {
    #[id = "bell"]
    #[name = "Bell"]
    Bell,
    #[id = "low_shelf"]
    #[name = "Low Shelf"]
    LowShelf,
    #[id = "low_cut"]
    #[name = "Low Cut"]
    LowCut,
    #[id = "high_shelf"]
    #[name = "High Shelf"]
    HighShelf,
    #[id = "high_cut"]
    #[name = "High Cut"]
    HighCut,
    #[id = "notch"]
    #[name = "Notch"]
    Notch,
    #[id = "band_pass"]
    #[name = "Band Pass"]
    BandPass,
    #[id = "tilt_shelf"]
    #[name = "Tilt Shelf"]
    TiltShelf,
    #[id = "flat_tilt"]
    #[name = "Flat Tilt"]
    FlatTilt,
}

impl From<Shape> for BandShape {
    fn from(s: Shape) -> Self {
        match s {
            Shape::Bell => Self::Bell,
            Shape::LowShelf => Self::LowShelf,
            Shape::LowCut => Self::LowCut,
            Shape::HighShelf => Self::HighShelf,
            Shape::HighCut => Self::HighCut,
            Shape::Notch => Self::Notch,
            Shape::BandPass => Self::BandPass,
            Shape::TiltShelf => Self::TiltShelf,
            Shape::FlatTilt => Self::FlatTilt,
        }
    }
}

impl From<BandShape> for Shape {
    fn from(s: BandShape) -> Self {
        match s {
            BandShape::Bell => Self::Bell,
            BandShape::LowShelf => Self::LowShelf,
            BandShape::LowCut => Self::LowCut,
            BandShape::HighShelf => Self::HighShelf,
            BandShape::HighCut => Self::HighCut,
            BandShape::Notch => Self::Notch,
            BandShape::BandPass => Self::BandPass,
            BandShape::TiltShelf => Self::TiltShelf,
            BandShape::FlatTilt => Self::FlatTilt,
        }
    }
}

/// Plugin-side mirror of [`Slope`] with stable ids for saved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum BandSlope {
    #[id = "s6"]
    #[name = "6 dB/oct"]
    Db6,
    #[id = "s12"]
    #[name = "12 dB/oct"]
    Db12,
    #[id = "s18"]
    #[name = "18 dB/oct"]
    Db18,
    #[id = "s24"]
    #[name = "24 dB/oct"]
    Db24,
    #[id = "s30"]
    #[name = "30 dB/oct"]
    Db30,
    #[id = "s36"]
    #[name = "36 dB/oct"]
    Db36,
    #[id = "s48"]
    #[name = "48 dB/oct"]
    Db48,
    #[id = "s72"]
    #[name = "72 dB/oct"]
    Db72,
    #[id = "s96"]
    #[name = "96 dB/oct"]
    Db96,
}

impl From<Slope> for BandSlope {
    fn from(s: Slope) -> Self {
        match s {
            Slope::Db6 => Self::Db6,
            Slope::Db12 => Self::Db12,
            Slope::Db18 => Self::Db18,
            Slope::Db24 => Self::Db24,
            Slope::Db30 => Self::Db30,
            Slope::Db36 => Self::Db36,
            Slope::Db48 => Self::Db48,
            Slope::Db72 => Self::Db72,
            Slope::Db96 => Self::Db96,
        }
    }
}

impl From<BandSlope> for Slope {
    fn from(s: BandSlope) -> Self {
        match s {
            BandSlope::Db6 => Self::Db6,
            BandSlope::Db12 => Self::Db12,
            BandSlope::Db18 => Self::Db18,
            BandSlope::Db24 => Self::Db24,
            BandSlope::Db30 => Self::Db30,
            BandSlope::Db36 => Self::Db36,
            BandSlope::Db48 => Self::Db48,
            BandSlope::Db72 => Self::Db72,
            BandSlope::Db96 => Self::Db96,
        }
    }
}

/// Plugin-side mirror of [`Placement`] with stable ids for saved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum BandPlace {
    #[id = "stereo"]
    #[name = "Stereo"]
    Stereo,
    #[id = "left"]
    #[name = "Left"]
    Left,
    #[id = "right"]
    #[name = "Right"]
    Right,
    #[id = "mid"]
    #[name = "Mid"]
    Mid,
    #[id = "side"]
    #[name = "Side"]
    Side,
}

impl From<Placement> for BandPlace {
    fn from(p: Placement) -> Self {
        match p {
            Placement::Stereo => Self::Stereo,
            Placement::Left => Self::Left,
            Placement::Right => Self::Right,
            Placement::Mid => Self::Mid,
            Placement::Side => Self::Side,
        }
    }
}

impl From<BandPlace> for Placement {
    fn from(p: BandPlace) -> Self {
        match p {
            BandPlace::Stereo => Self::Stereo,
            BandPlace::Left => Self::Left,
            BandPlace::Right => Self::Right,
            BandPlace::Mid => Self::Mid,
            BandPlace::Side => Self::Side,
        }
    }
}

/// Frequency range limits (Hz).
pub const FREQ_MIN: f32 = 10.0;
pub const FREQ_MAX: f32 = 30000.0;
/// Q range limits.
pub const Q_MIN: f32 = 0.025;
pub const Q_MAX: f32 = 40.0;
/// Band gain limit (dB, symmetric).
pub const GAIN_MAX_DB: f32 = 30.0;
/// Output gain limit (dB, symmetric).
pub const OUT_GAIN_MAX_DB: f32 = 36.0;
/// Gain scale range (percent).
pub const GAIN_SCALE_MAX_PCT: f32 = 200.0;

/// Frequency range: skew factor 0.2 puts 1 kHz near the middle of the control.
pub fn freq_range() -> FloatRange {
    FloatRange::Skewed {
        min: FREQ_MIN,
        max: FREQ_MAX,
        factor: 0.2,
    }
}

/// Q range: skew factor 0.2 puts Q 1 near the middle of the control.
pub fn q_range() -> FloatRange {
    FloatRange::Skewed {
        min: Q_MIN,
        max: Q_MAX,
        factor: 0.2,
    }
}

pub fn gain_range() -> FloatRange {
    FloatRange::Linear {
        min: -GAIN_MAX_DB,
        max: GAIN_MAX_DB,
    }
}

/// Parse a plain number, accepting decimal comma, the Unicode minus sign and
/// spaces (also no-break and thin spaces) as digit group separators, as in
/// "15 484".
fn parse_number(s: &str) -> Option<f64> {
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            ',' => '.',
            '\u{2212}' => '-',
            c => c,
        })
        .collect();
    let v: f64 = s.parse().ok()?;
    v.is_finite().then_some(v)
}

/// Strip a case-insensitive ASCII suffix, if present.
fn strip_suffix_ci<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let n = s.len().checked_sub(suffix.len())?;
    if s.is_char_boundary(n) && s[n..].eq_ignore_ascii_case(suffix) {
        Some(&s[..n])
    } else {
        None
    }
}

/// Parse "41.1", "41.1 Hz", "9.729k", "9729", "9,729 kHz" into Hz.
pub fn parse_freq(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = strip_suffix_ci(s, "hz").unwrap_or(s).trim();
    let (num, mult) = match strip_suffix_ci(s, "k") {
        Some(n) => (n, 1000.0),
        None => (s, 1.0),
    };
    Some((parse_number(num)? * mult) as f32)
}

/// Format a frequency: Hz with one decimal below 1 kHz, kHz with three decimals above.
pub fn format_freq(hz: f32) -> String {
    if hz >= 999.95 {
        format!("{:.3} kHz", f64::from(hz) / 1000.0)
    } else {
        format!("{hz:.1} Hz")
    }
}

/// Parse a dB value such as "+2.58", "-1.32 dB", "2,5".
pub fn parse_db(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = strip_suffix_ci(s, "db").unwrap_or(s);
    Some(parse_number(s)? as f32)
}

/// Parse a percentage such as "150", "150 %".
pub fn parse_percent(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = s.strip_suffix('%').unwrap_or(s);
    Some(parse_number(s)? as f32)
}

/// Parse a plain Q value.
pub fn parse_q(s: &str) -> Option<f32> {
    Some(parse_number(s)? as f32)
}

fn fmt_fixed(digits: usize) -> Arc<dyn Fn(f32) -> String + Send + Sync> {
    formatters::v2s_f32_rounded(digits)
}

/// Parameters of a single band.
#[derive(Params)]
pub struct BandParamSet {
    #[id = "used"]
    pub used: BoolParam,
    #[id = "on"]
    pub on: BoolParam,
    #[id = "shape"]
    pub shape: EnumParam<BandShape>,
    #[id = "freq"]
    pub freq: FloatParam,
    #[id = "gain"]
    pub gain: FloatParam,
    #[id = "q"]
    pub q: FloatParam,
    #[id = "slope"]
    pub slope: EnumParam<BandSlope>,
    #[id = "place"]
    pub place: EnumParam<BandPlace>,
}

impl Default for BandParamSet {
    fn default() -> Self {
        Self {
            used: BoolParam::new("Used", false),
            on: BoolParam::new("Enabled", true),
            shape: EnumParam::new("Shape", BandShape::Bell),
            freq: FloatParam::new("Frequency", 1000.0, freq_range())
                .with_value_to_string(Arc::new(format_freq))
                .with_string_to_value(Arc::new(parse_freq)),
            gain: FloatParam::new("Gain", 0.0, gain_range())
                .with_unit(" dB")
                .with_value_to_string(fmt_fixed(2))
                .with_string_to_value(Arc::new(parse_db)),
            q: FloatParam::new("Q", 1.0, q_range())
                .with_value_to_string(fmt_fixed(2))
                .with_string_to_value(Arc::new(parse_q)),
            slope: EnumParam::new("Slope", BandSlope::Db12),
            place: EnumParam::new("Placement", BandPlace::Stereo),
        }
    }
}

impl BandParamSet {
    /// Current (unsmoothed) values as DSP band parameters. Allocation-free.
    pub fn to_dsp(&self) -> BandParams {
        BandParams {
            used: self.used.value(),
            enabled: self.on.value(),
            shape: self.shape.value().into(),
            freq_hz: f64::from(self.freq.value()),
            gain_db: f64::from(self.gain.value()),
            q: f64::from(self.q.value()),
            slope: self.slope.value().into(),
            placement: self.place.value().into(),
        }
    }
}

#[derive(Params)]
pub struct NeoFilterParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[id = "bypass"]
    pub bypass: BoolParam,

    /// Output gain in dB (smoothed linearly in dB).
    #[id = "out_gain"]
    pub out_gain: FloatParam,

    /// Gain scale in percent (0..200, default 100).
    #[id = "gain_scale"]
    pub gain_scale: FloatParam,

    #[nested(array, group = "Band")]
    pub bands: [BandParamSet; NUM_BANDS],
}

impl Default for NeoFilterParams {
    fn default() -> Self {
        Self {
            editor_state: crate::editor::default_state(),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            out_gain: FloatParam::new(
                "Output Gain",
                0.0,
                FloatRange::Linear {
                    min: -OUT_GAIN_MAX_DB,
                    max: OUT_GAIN_MAX_DB,
                },
            )
            .with_unit(" dB")
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_value_to_string(fmt_fixed(2))
            .with_string_to_value(Arc::new(parse_db)),
            gain_scale: FloatParam::new(
                "Gain Scale",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: GAIN_SCALE_MAX_PCT,
                },
            )
            .with_unit(" %")
            .with_value_to_string(fmt_fixed(0))
            .with_string_to_value(Arc::new(parse_percent)),
            bands: std::array::from_fn(|_| BandParamSet::default()),
        }
    }
}

impl NeoFilterParams {
    /// Snapshot of all bands' current values. Allocation-free.
    pub fn band_snapshot(&self) -> [BandParams; NUM_BANDS] {
        std::array::from_fn(|i| self.bands[i].to_dsp())
    }

    /// Gain scale as a factor (1.0 = 100 %).
    pub fn gain_scale_factor(&self) -> f32 {
        self.gain_scale.value() / 100.0
    }
}
