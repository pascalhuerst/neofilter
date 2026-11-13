//! Pure display logic of the editor: number formatting, scales, colour
//! thresholds, warning rules and plot mappings. No egui types, so everything
//! here is unit-tested.

use crate::analysis::SpectrumData;
use crate::settings::GonioMode;
use neo_measure::util::DB_FLOOR;

/// Levels at or below this read "-inf".
pub const SILENCE_DB: f64 = -150.0;

/// dB with 0.01 resolution, `-inf` for silence / non-finite input.
pub fn fmt_db(v: f64) -> String {
    if !v.is_finite() || v <= SILENCE_DB {
        "-inf".to_string()
    } else {
        format!("{v:.2}")
    }
}

/// Loudness in LUFS / LU with 0.01 resolution; `None`, `-inf` and NaN read "--".
pub fn fmt_lufs(v: Option<f64>) -> String {
    match v {
        Some(x) if x.is_finite() => format!("{x:.2}"),
        _ => "--".to_string(),
    }
}

/// Signed value with an explicit plus sign.
pub fn fmt_signed(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return "--".to_string();
    }
    format!("{v:+.decimals$}")
}

/// Correlation coefficient `+0.98`.
pub fn fmt_corr(c: f64) -> String {
    fmt_signed(c.clamp(-1.0, 1.0), 2)
}

/// DC offset as percent of full scale with enough digits for small values.
pub fn fmt_dc(dc: f64) -> String {
    if !dc.is_finite() {
        "--".into()
    } else {
        format!("{:+.4} %", dc * 100.0)
    }
}

/// `440.0 Hz`, `1.00 kHz`, `12.5 kHz`.
pub fn fmt_freq(f: f64) -> String {
    if !f.is_finite() {
        "--".into()
    } else if f >= 1000.0 {
        format!("{:.2} kHz", f / 1000.0)
    } else if f >= 100.0 {
        format!("{f:.1} Hz")
    } else {
        format!("{f:.2} Hz")
    }
}

/// Velocity in cm/s (3 significant-ish digits).
pub fn fmt_velocity(v: f64) -> String {
    if !v.is_finite() || v < 1e-4 {
        "0.000".into()
    } else {
        format!("{v:.3}")
    }
}

/// Displacement in micrometres.
pub fn fmt_displacement(um: f64) -> String {
    if !um.is_finite() || um < 1e-3 {
        "0.00".into()
    } else if um >= 100.0 {
        format!("{um:.1}")
    } else {
        format!("{um:.2}")
    }
}

/// Position of `db` on a bar from `min_db` (0.0) to `max_db` (1.0), clamped.
pub fn bar_fraction(db: f64, min_db: f64, max_db: f64) -> f32 {
    if !db.is_finite() || max_db <= min_db {
        return 0.0;
    }
    (((db - min_db) / (max_db - min_db)).clamp(0.0, 1.0)) as f32
}

/// Colour class of a peak level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Ok,
    Hot,
    Over,
}

/// Peak colour zones: green below -6, yellow up to -1, red above (a true peak
/// at or over -1 dBTP is a delivery problem for most platforms).
pub fn peak_zone(db: f64) -> Zone {
    if db > -1.0 {
        Zone::Over
    } else if db > -6.0 {
        Zone::Hot
    } else {
        Zone::Ok
    }
}

/// RGB for a zone.
pub fn zone_rgb(z: Zone) -> [u8; 3] {
    match z {
        Zone::Ok => [70, 190, 110],
        Zone::Hot => [230, 190, 60],
        Zone::Over => [230, 80, 70],
    }
}

/// Colour of a correlation bar: red when out of phase, yellow near zero,
/// green when positive.
pub fn correlation_rgb(c: f64) -> [u8; 3] {
    if c < 0.0 {
        zone_rgb(Zone::Over)
    } else if c < 0.3 {
        zone_rgb(Zone::Hot)
    } else {
        zone_rgb(Zone::Ok)
    }
}

/// Warnings of the low-frequency vertical/lateral meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LfWarning {
    /// Vertical level exceeds lateral by more than the margin.
    pub vertical: bool,
    /// LF correlation below the threshold.
    pub correlation: bool,
}

impl LfWarning {
    pub fn any(self) -> bool {
        self.vertical || self.correlation
    }
}

/// Evaluate the warning rules. Silence never warns (a `-inf` vertical level
/// would otherwise compare against a `-inf` lateral level).
pub fn lf_warning(
    vertical_db: f64,
    lateral_db: f64,
    correlation: f64,
    margin_db: f64,
    corr_threshold: f64,
) -> LfWarning {
    let audible = vertical_db > SILENCE_DB || lateral_db > SILENCE_DB;
    LfWarning {
        vertical: audible && vertical_db - lateral_db > margin_db,
        correlation: audible && correlation < corr_threshold,
    }
}

/// Difference to the loudness target in LU (`None` when there is no reading).
pub fn target_delta(lufs: Option<f64>, target: f64) -> Option<f64> {
    lufs.filter(|v| v.is_finite()).map(|v| v - target)
}

/// Frequency/level to pixel mapping of the spectrum plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecView {
    pub x0: f32,
    pub y0: f32,
    pub w: f32,
    pub h: f32,
    pub f_lo: f32,
    pub f_hi: f32,
    pub db_min: f32,
    pub db_max: f32,
}

impl SpecView {
    pub fn freq_to_x(&self, f: f32) -> f32 {
        let t = (f / self.f_lo).ln() / (self.f_hi / self.f_lo).ln();
        self.x0 + t * self.w
    }
    pub fn x_to_freq(&self, x: f32) -> f32 {
        let t = (x - self.x0) / self.w;
        self.f_lo * (self.f_hi / self.f_lo).powf(t)
    }
    pub fn db_to_y(&self, db: f32) -> f32 {
        let t = (db - self.db_min) / (self.db_max - self.db_min);
        self.y0 + self.h * (1.0 - t)
    }
    pub fn y_to_db(&self, y: f32) -> f32 {
        let t = 1.0 - (y - self.y0) / self.h;
        self.db_min + t * (self.db_max - self.db_min)
    }
}

/// Grid frequencies in `f_lo..=f_hi`: `(frequency, labelled)`; 1-2-5 per decade
/// are labelled, 3-4-6-7-8-9 are minor lines.
pub fn freq_ticks(f_lo: f64, f_hi: f64) -> Vec<(f64, bool)> {
    let mut v = Vec::new();
    if !(f_lo > 0.0 && f_hi > f_lo) {
        return v;
    }
    let mut decade = 10f64.powf(f_lo.log10().floor());
    while decade <= f_hi {
        for m in 1..10 {
            let f = decade * m as f64;
            if f >= f_lo && f <= f_hi {
                v.push((f, matches!(m, 1 | 2 | 5)));
            }
        }
        decade *= 10.0;
    }
    v
}

/// Frequency label: `20`, `100`, `1k`, `20k`.
pub fn freq_label(f: f64) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 1e-9 {
            format!("{}k", k.round() as i64)
        } else {
            format!("{k}k")
        }
    } else {
        format!("{}", f.round() as i64)
    }
}

/// Horizontal dB grid lines between `db_min` and `db_max`.
pub fn db_ticks(db_min: f64, db_max: f64) -> Vec<f64> {
    let span = (db_max - db_min).max(1.0);
    let step = if span <= 30.0 {
        5.0
    } else if span <= 70.0 {
        10.0
    } else if span <= 150.0 {
        20.0
    } else {
        40.0
    };
    let mut v = Vec::new();
    let mut t = (db_min / step).ceil() * step;
    while t <= db_max + 1e-9 {
        v.push(t);
        t += step;
    }
    v
}

/// Level of the curve at frequency `f` (nearest display point). Returns the
/// grid frequency and level. `None` for an empty curve.
pub fn spectrum_readout(sp: &SpectrumData, db: &[f32], f: f64) -> Option<(f64, f64)> {
    if db.len() < 2 || f.is_nan() || f <= 0.0 {
        return None;
    }
    let n = (db.len() - 1) as f64;
    let t = (f / sp.f_lo).ln() / (sp.f_hi / sp.f_lo).ln();
    let i = (t * n).round().clamp(0.0, n) as usize;
    Some((sp.grid_hz(i), f64::from(db[i])))
}

/// Nearest 1/3-octave band `(nominal, level)` to `f`.
pub fn band_readout(bands: &[(f32, f32)], f: f64) -> Option<(f64, f64)> {
    bands
        .iter()
        .min_by(|a, b| {
            let da = (f64::from(a.0).ln() - f.ln()).abs();
            let db = (f64::from(b.0).ln() - f.ln()).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|b| (f64::from(b.0), f64::from(b.1)))
}

/// Goniometer point `[x, y] = [(L-R)/sqrt2, (L+R)/sqrt2]` to unit-square
/// display coordinates (`x` right, `y` up, both `-1..=1`), for the chosen mode.
/// In L/R mode the display is `x = L`, `y = R`.
pub fn gonio_xy(p: [f32; 2], mode: GonioMode) -> [f32; 2] {
    match mode {
        GonioMode::MidSide => p,
        GonioMode::LeftRight => {
            let k = std::f32::consts::FRAC_1_SQRT_2;
            // x = (L - R)/sqrt2, y = (L + R)/sqrt2  ->  L = (x + y)/sqrt2, R = (y - x)/sqrt2
            [(p[0] + p[1]) * k, (p[1] - p[0]) * k]
        }
    }
}

/// Maximum of two readings that may be the floor value.
pub fn max_db(a: f64, b: f64) -> f64 {
    a.max(b).max(DB_FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{SpectrumCurve, SPECTRUM_POINTS};

    #[test]
    fn db_formatting_has_two_decimals_and_inf() {
        assert_eq!(fmt_db(-20.0), "-20.00");
        assert_eq!(fmt_db(-19.9949), "-19.99");
        assert_eq!(fmt_db(0.004), "0.00");
        assert_eq!(fmt_db(-400.0), "-inf");
        assert_eq!(fmt_db(f64::NEG_INFINITY), "-inf");
        assert_eq!(fmt_db(f64::NAN), "-inf");
        assert_eq!(fmt_lufs(Some(-23.0)), "-23.00");
        assert_eq!(fmt_lufs(Some(f64::NEG_INFINITY)), "--");
        assert_eq!(fmt_lufs(None), "--");
        assert_eq!(fmt_corr(0.987), "+0.99");
        assert_eq!(fmt_corr(-1.5), "-1.00");
        assert_eq!(fmt_signed(1.234, 1), "+1.2");
    }

    #[test]
    fn frequency_and_physical_formatting() {
        assert_eq!(fmt_freq(1000.0), "1.00 kHz");
        assert_eq!(fmt_freq(440.0), "440.0 Hz");
        assert_eq!(fmt_freq(31.5), "31.50 Hz");
        assert_eq!(fmt_velocity(5.0), "5.000");
        assert_eq!(fmt_velocity(0.0), "0.000");
        assert_eq!(fmt_displacement(7.9577), "7.96");
        assert_eq!(fmt_displacement(123.45), "123.5");
        assert_eq!(fmt_dc(0.001), "+0.1000 %");
        assert_eq!(freq_label(20.0), "20");
        assert_eq!(freq_label(1000.0), "1k");
        assert_eq!(freq_label(20_000.0), "20k");
        assert_eq!(freq_label(2500.0), "2.5k");
    }

    #[test]
    fn bar_fraction_clamps() {
        assert_eq!(bar_fraction(-60.0, -60.0, 0.0), 0.0);
        assert_eq!(bar_fraction(0.0, -60.0, 0.0), 1.0);
        assert!((bar_fraction(-30.0, -60.0, 0.0) - 0.5).abs() < 1e-6);
        assert_eq!(bar_fraction(10.0, -60.0, 0.0), 1.0);
        assert_eq!(bar_fraction(f64::NEG_INFINITY, -60.0, 0.0), 0.0);
        assert_eq!(bar_fraction(-10.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn peak_zones() {
        assert_eq!(peak_zone(-20.0), Zone::Ok);
        assert_eq!(peak_zone(-6.0), Zone::Ok);
        assert_eq!(peak_zone(-5.9), Zone::Hot);
        assert_eq!(peak_zone(-1.0), Zone::Hot);
        assert_eq!(peak_zone(-0.9), Zone::Over);
        assert_eq!(peak_zone(0.5), Zone::Over);
        assert_ne!(zone_rgb(Zone::Ok), zone_rgb(Zone::Over));
        assert_eq!(correlation_rgb(-0.2), zone_rgb(Zone::Over));
        assert_eq!(correlation_rgb(0.1), zone_rgb(Zone::Hot));
        assert_eq!(correlation_rgb(0.9), zone_rgb(Zone::Ok));
    }

    #[test]
    fn lf_warning_rules() {
        // vertical louder than lateral by more than the margin
        let w = lf_warning(-20.0, -30.0, 0.5, 0.0, 0.0);
        assert!(w.vertical && !w.correlation && w.any());
        // margin shifts the threshold
        assert!(!lf_warning(-20.0, -30.0, 0.5, 12.0, 0.0).vertical);
        // negative correlation
        let w = lf_warning(-40.0, -20.0, -0.1, 0.0, 0.0);
        assert!(!w.vertical && w.correlation);
        // threshold adjustable
        assert!(!lf_warning(-40.0, -20.0, -0.1, 0.0, -0.5).correlation);
        // silence never warns
        assert!(!lf_warning(-400.0, -400.0, 0.0, 0.0, 0.5).any());
        // exactly at the margin does not warn
        assert!(!lf_warning(-20.0, -20.0, 1.0, 0.0, 0.0).any());
    }

    #[test]
    fn target_delta_handles_missing() {
        assert_eq!(target_delta(Some(-13.0), -14.0), Some(1.0));
        assert_eq!(target_delta(None, -14.0), None);
        assert_eq!(target_delta(Some(f64::NEG_INFINITY), -14.0), None);
    }

    #[test]
    fn spec_view_round_trips() {
        let v = SpecView {
            x0: 10.0,
            y0: 20.0,
            w: 800.0,
            h: 400.0,
            f_lo: 20.0,
            f_hi: 20_000.0,
            db_min: -120.0,
            db_max: 0.0,
        };
        assert!((v.freq_to_x(20.0) - 10.0).abs() < 1e-3);
        assert!((v.freq_to_x(20_000.0) - 810.0).abs() < 1e-3);
        assert!((v.freq_to_x(632.455) - 410.0).abs() < 0.1); // geometric centre
        for f in [25.0f32, 440.0, 1234.0, 19_000.0] {
            assert!((v.x_to_freq(v.freq_to_x(f)) / f - 1.0).abs() < 1e-4);
        }
        assert_eq!(v.db_to_y(0.0), 20.0);
        assert_eq!(v.db_to_y(-120.0), 420.0);
        assert!((v.y_to_db(v.db_to_y(-33.3)) + 33.3).abs() < 1e-3);
    }

    #[test]
    fn tick_generation() {
        let t = freq_ticks(20.0, 20_000.0);
        assert_eq!(t.first(), Some(&(20.0, true)));
        assert_eq!(t.last(), Some(&(20_000.0, true)));
        assert!(t.contains(&(300.0, false)));
        assert!(t.contains(&(500.0, true)));
        assert!(t.iter().all(|&(f, _)| (20.0..=20_000.0).contains(&f)));
        assert!(freq_ticks(0.0, 10.0).is_empty());
        assert_eq!(
            db_ticks(-120.0, 0.0),
            [-120.0, -100.0, -80.0, -60.0, -40.0, -20.0, 0.0]
        );
        assert_eq!(
            db_ticks(-30.0, 0.0),
            [-30.0, -25.0, -20.0, -15.0, -10.0, -5.0, 0.0]
        );
    }

    #[test]
    fn cursor_readouts() {
        let db: Vec<f32> = (0..SPECTRUM_POINTS).map(|i| -(i as f32)).collect();
        let sp = SpectrumData {
            fft_size: 4096,
            bin_hz: 10.0,
            f_lo: 20.0,
            f_hi: 20_000.0,
            frames: 1,
            rta: false,
            curves: vec![SpectrumCurve {
                chan: crate::analysis::Chan::Left,
                db: db.clone(),
                peak_db: vec![],
                bands: vec![],
            }],
        };
        let (f, l) = spectrum_readout(&sp, &db, 20.0).unwrap();
        assert!((f - 20.0).abs() < 1e-9 && l == 0.0);
        let (f, l) = spectrum_readout(&sp, &db, 20_000.0).unwrap();
        assert!((f - 20_000.0).abs() < 1e-6 && l == -(SPECTRUM_POINTS as f64 - 1.0));
        let (f, _) = spectrum_readout(&sp, &db, 1000.0).unwrap();
        assert!((f / 1000.0 - 1.0).abs() < 0.004);
        assert!(spectrum_readout(&sp, &[], 1000.0).is_none());
        let bands = [(100.0f32, -10.0f32), (125.0, -12.0), (160.0, -14.0)];
        assert_eq!(band_readout(&bands, 130.0), Some((125.0, -12.0)));
        assert_eq!(band_readout(&[], 130.0), None);
    }

    #[test]
    fn goniometer_modes() {
        // mono (L = R = 1): M/S display is a vertical line, L/R display the diagonal
        let k = std::f32::consts::SQRT_2;
        let mono = [0.0, k];
        assert_eq!(gonio_xy(mono, GonioMode::MidSide), mono);
        let lr = gonio_xy(mono, GonioMode::LeftRight);
        assert!((lr[0] - 1.0).abs() < 1e-6 && (lr[1] - 1.0).abs() < 1e-6);
        // L only (L = 1, R = 0): x = 1/sqrt2, y = 1/sqrt2 -> L/R display (1, 0)
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let l_only = gonio_xy([s, s], GonioMode::LeftRight);
        assert!((l_only[0] - 1.0).abs() < 1e-6 && l_only[1].abs() < 1e-6);
    }
}
