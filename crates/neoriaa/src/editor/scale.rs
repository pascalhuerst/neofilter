//! Pure display logic: axis mapping, ticks, number formatting and the cached
//! response curves. No egui here, so everything is unit-tested.

use crate::dsp::{analog_response, Mode, Options, RiaaFilters};

/// Lowest displayed frequency.
pub const F_MIN: f64 = 10.0;
/// Upper edge of the plot (before limiting to Nyquist).
pub const F_MAX_LIMIT: f64 = 40_000.0;
/// Main plot range in dB (symmetric): covers every curve up to ~20 kHz.
pub const RESPONSE_DB: (f64, f64) = (-30.0, 40.0);
/// Magnified error plot: +-this many dB.
pub const ERROR_DB: f64 = 0.05;
/// Accuracy band: the error is only meaningful (and drawn solid) here.
pub const BAND: (f64, f64) = (20.0, 20_000.0);
/// Frequencies of the numeric readout.
pub const READOUT_HZ: [f64; 4] = [20.0, 1_000.0, 10_000.0, 20_000.0];
/// Peak meter span in dBFS.
pub const METER_DB: (f64, f64) = (-60.0, 6.0);
/// Number of points on every curve.
pub const CURVE_POINTS: usize = 400;

/// Right edge of the plot for a sample rate.
pub fn plot_f_max(sample_rate: f64) -> f64 {
    (sample_rate / 2.0).clamp(F_MIN * 10.0, F_MAX_LIMIT)
}

/// Mapping between a plot rectangle and (frequency, dB).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub x0: f32,
    pub y0: f32,
    pub width: f32,
    pub height: f32,
    pub f_min: f64,
    pub f_max: f64,
    pub db_min: f64,
    pub db_max: f64,
}

impl View {
    pub fn freq_to_x(&self, f: f64) -> f32 {
        let t = (f / self.f_min).ln() / (self.f_max / self.f_min).ln();
        self.x0 + t as f32 * self.width
    }

    pub fn x_to_freq(&self, x: f32) -> f64 {
        let t = f64::from((x - self.x0) / self.width);
        self.f_min * (self.f_max / self.f_min).powf(t)
    }

    /// Values outside the dB range map outside the rectangle; callers clamp.
    pub fn db_to_y(&self, db: f64) -> f32 {
        let t = (db - self.db_min) / (self.db_max - self.db_min);
        self.y0 + self.height * (1.0 - t as f32)
    }

    pub fn y_to_db(&self, y: f32) -> f64 {
        let t = 1.0 - f64::from((y - self.y0) / self.height);
        self.db_min + t * (self.db_max - self.db_min)
    }
}

/// Labelled frequency grid lines that fit below `f_max`.
pub fn freq_ticks(f_max: f64) -> Vec<(f64, &'static str)> {
    const ALL: [(f64, &str); 12] = [
        (10.0, "10"),
        (20.0, "20"),
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1_000.0, "1k"),
        (2_000.0, "2k"),
        (5_000.0, "5k"),
        (10_000.0, "10k"),
        (20_000.0, "20k"),
        (40_000.0, "40k"),
    ];
    ALL.into_iter().filter(|(f, _)| *f <= f_max).collect()
}

/// Horizontal grid lines every `step` dB inside `[lo, hi]`.
pub fn db_ticks(lo: f64, hi: f64, step: f64) -> Vec<f64> {
    let first = (lo / step).ceil() as i64;
    let last = (hi / step).floor() as i64;
    (first..=last).map(|i| i as f64 * step).collect()
}

/// Log-spaced frequencies from `f_min` to `f_max` (inclusive).
pub fn log_freqs(n: usize, f_min: f64, f_max: f64) -> Vec<f64> {
    (0..n)
        .map(|i| f_min * (f_max / f_min).powf(i as f64 / (n - 1) as f64))
        .collect()
}

pub fn format_db(db: f64) -> String {
    if db.is_finite() {
        format!("{db:+.2} dB")
    } else {
        "-inf dB".to_string()
    }
}

/// Error readout with 4 decimals (the plot is magnified to +-0.05 dB).
pub fn format_error_db(db: f64) -> String {
    format!("{db:+.4} dB")
}

pub fn format_freq(hz: f64) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if (k - k.round()).abs() < 1e-9 {
            format!("{k:.0} kHz")
        } else {
            format!("{k:.1} kHz")
        }
    } else {
        format!("{hz:.0} Hz")
    }
}

pub fn lin_to_db(x: f32) -> f64 {
    if x > 1e-6 {
        20.0 * f64::from(x).log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// Peak value as shown next to the meter ("-inf" when silent).
pub fn format_dbfs(x: f32) -> String {
    let db = lin_to_db(x);
    if db.is_finite() {
        format!("{db:+.1} dBFS")
    } else {
        "-inf dBFS".to_string()
    }
}

/// Position 0..1 on the meter bar for a linear peak value.
pub fn meter_fraction(x: f32) -> f32 {
    let db = lin_to_db(x);
    if !db.is_finite() {
        return 0.0;
    }
    ((db - METER_DB.0) / (METER_DB.1 - METER_DB.0)).clamp(0.0, 1.0) as f32
}

/// Where 0 dBFS lies on the meter bar.
pub fn meter_zero_fraction() -> f32 {
    ((0.0 - METER_DB.0) / (METER_DB.1 - METER_DB.0)) as f32
}

/// Cached curves for the current mode/options/sample rate.
#[derive(Debug, Clone, Default)]
pub struct CurveCache {
    key: Option<(Mode, Options, u64)>,
    pub freqs: Vec<f64>,
    /// Analytic target in dB.
    pub analog: Vec<f64>,
    /// Digital response in dB.
    pub digital: Vec<f64>,
    /// `digital - analog` in dB.
    pub error: Vec<f64>,
    /// (digital dB, analog dB) at [`READOUT_HZ`].
    pub readout: [(f64, f64); 4],
    pub f_max: f64,
}

impl CurveCache {
    /// Recompute if mode, options or design changed. Returns true if recomputed.
    pub fn update(&mut self, filters: &RiaaFilters, mode: Mode, opts: Options) -> bool {
        let fs = filters.sample_rate();
        let key = (mode, opts, fs.to_bits());
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        self.f_max = plot_f_max(fs);
        self.freqs = log_freqs(CURVE_POINTS, F_MIN, self.f_max);
        self.analog = self
            .freqs
            .iter()
            .map(|&f| analog_response(mode, opts, f).db())
            .collect();
        self.digital = self
            .freqs
            .iter()
            .map(|&f| filters.response(mode, opts, f).db())
            .collect();
        self.error = self
            .digital
            .iter()
            .zip(&self.analog)
            .map(|(d, a)| d - a)
            .collect();
        for (r, &f) in self.readout.iter_mut().zip(&READOUT_HZ) {
            *r = (
                filters.response(mode, opts, f.min(fs / 2.0 * 0.999)).db(),
                analog_response(mode, opts, f).db(),
            );
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        self.key.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            x0: 10.0,
            y0: 5.0,
            width: 400.0,
            height: 200.0,
            f_min: 10.0,
            f_max: 40_000.0,
            db_min: -30.0,
            db_max: 40.0,
        }
    }

    #[test]
    fn frequency_mapping_roundtrip_and_edges() {
        let v = view();
        assert!((v.freq_to_x(10.0) - 10.0).abs() < 1e-4);
        assert!((v.freq_to_x(40_000.0) - 410.0).abs() < 1e-3);
        for f in [10.0, 20.0, 440.0, 1000.0, 20_000.0, 40_000.0] {
            let back = v.x_to_freq(v.freq_to_x(f));
            assert!((back / f - 1.0).abs() < 1e-4, "{f} -> {back}");
        }
        // equal ratios are equally far apart
        let d1 = v.freq_to_x(200.0) - v.freq_to_x(100.0);
        let d2 = v.freq_to_x(2000.0) - v.freq_to_x(1000.0);
        assert!((d1 - d2).abs() < 1e-3);
    }

    #[test]
    fn db_mapping() {
        let v = view();
        assert!((v.db_to_y(40.0) - 5.0).abs() < 1e-4);
        assert!((v.db_to_y(-30.0) - 205.0).abs() < 1e-3);
        for db in [-30.0, -3.0, 0.0, 19.27, 40.0] {
            assert!((v.y_to_db(v.db_to_y(db)) - db).abs() < 1e-3);
        }
    }

    #[test]
    fn plot_limits() {
        assert_eq!(plot_f_max(44_100.0), 22_050.0);
        assert_eq!(plot_f_max(96_000.0), 40_000.0);
        assert_eq!(plot_f_max(384_000.0), 40_000.0);
        assert!(plot_f_max(1.0) >= 100.0);
    }

    #[test]
    fn ticks() {
        let t = freq_ticks(22_050.0);
        assert_eq!(t.first().unwrap().0, 10.0);
        assert_eq!(t.last().unwrap().0, 20_000.0);
        assert_eq!(freq_ticks(40_000.0).last().unwrap().0, 40_000.0);
        assert_eq!(db_ticks(-30.0, 40.0, 10.0).len(), 8);
        assert_eq!(
            db_ticks(-0.05, 0.05, 0.025),
            vec![-0.05, -0.025, 0.0, 0.025, 0.05]
        );
        let f = log_freqs(5, 10.0, 10_000.0);
        assert!((f[0] - 10.0).abs() < 1e-12 && (f[4] - 10_000.0).abs() < 1e-9);
        assert!((f[2] - 316.227766).abs() < 1e-3);
    }

    #[test]
    fn formatting() {
        assert_eq!(format_db(19.274), "+19.27 dB");
        assert_eq!(format_db(-13.734), "-13.73 dB");
        assert_eq!(format_db(0.0), "+0.00 dB");
        assert_eq!(format_db(f64::NEG_INFINITY), "-inf dB");
        assert_eq!(format_error_db(0.00123), "+0.0012 dB");
        assert_eq!(format_freq(20.0), "20 Hz");
        assert_eq!(format_freq(1000.0), "1 kHz");
        assert_eq!(format_freq(22_050.0), "22.1 kHz");
        assert_eq!(format_dbfs(1.0), "+0.0 dBFS");
        assert_eq!(format_dbfs(0.5), "-6.0 dBFS");
        assert_eq!(format_dbfs(0.0), "-inf dBFS");
    }

    #[test]
    fn meter_scale() {
        assert_eq!(meter_fraction(0.0), 0.0);
        assert_eq!(meter_fraction(1e-4), 0.0); // -80 dB, below the span
        assert_eq!(meter_fraction(100.0), 1.0);
        assert!((meter_fraction(1.0) - meter_zero_fraction()).abs() < 1e-6);
        assert!(meter_fraction(0.5) < meter_fraction(1.0));
        assert!((meter_zero_fraction() - 60.0 / 66.0).abs() < 1e-6);
    }

    #[test]
    fn curve_cache_matches_filter_and_caches() {
        let f = RiaaFilters::design(96_000.0);
        let mut c = CurveCache::default();
        assert!(c.is_empty());
        assert!(c.update(&f, Mode::PreEmphasis, Options::default()));
        assert!(!c.update(&f, Mode::PreEmphasis, Options::default()));
        assert!(c.update(&f, Mode::DeEmphasis, Options::default()));
        assert_eq!(c.freqs.len(), CURVE_POINTS);
        assert_eq!(c.f_max, 40_000.0);
        // de-emphasis check values on the readout
        assert!((c.readout[0].1 - 19.274).abs() < 1e-3);
        assert!(c.readout[1].1.abs() < 1e-9 && c.readout[1].0.abs() < 1e-9);
        assert!((c.readout[2].1 + 13.734).abs() < 1e-3);
        assert!((c.readout[3].1 + 19.620).abs() < 1e-3);
        // error stays inside the magnified scale inside the band
        for (&hz, e) in c.freqs.iter().zip(&c.error) {
            if (BAND.0..=BAND.1).contains(&hz) {
                assert!(e.abs() < ERROR_DB, "{hz}: {e}");
            }
        }
    }
}
