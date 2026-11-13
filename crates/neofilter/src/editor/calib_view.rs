//! Pure logic behind the calibration panel (no egui): which buttons are
//! enabled, what the status line and readouts say, which curves are drawn
//! and which bands have to be written. Unit-tested; `calib.rs` only paints.

use crate::calibration::analysis::{ChannelInfo, Measurement};
use crate::calibration::fit::{deviation_curve, FitPlan};
use crate::calibration::session::{IterationRecord, Phase, Session};
use crate::calibration::settings::{CalCurve, CalSettings};
use crate::calibration::CalError;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;

/// Which buttons may be pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enabled {
    pub measure: bool,
    pub fit: bool,
    pub apply: bool,
    pub auto: bool,
    pub stop: bool,
    pub clear: bool,
}

pub fn enabled(phase: Phase, has_measurement: bool, has_plan: bool, noise_on: bool) -> Enabled {
    let idle = phase == Phase::Idle;
    Enabled {
        measure: idle,
        fit: idle && has_measurement,
        apply: idle && has_plan,
        auto: idle,
        stop: !idle || noise_on,
        clear: idle,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Busy,
    Warn,
    Error,
}

/// Status line text and severity.
pub fn status_line(session: &Session) -> (String, Severity) {
    let sev = match (session.phase(), session.error()) {
        (_, Some(CalError::Stopped)) => Severity::Warn,
        (_, Some(_)) => Severity::Error,
        (Phase::Idle, None) => Severity::Info,
        _ => Severity::Busy,
    };
    (session.status().to_string(), sev)
}

/// Measurement progress in `0..=1`.
pub fn progress_fraction(captured: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (captured as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

/// `"371.20 ms (17817.8 samples), inverted"`.
pub fn delay_text(i: &ChannelInfo) -> String {
    format!(
        "{:.2} ms ({:.1} samples){}",
        i.delay_ms,
        i.delay_samples,
        if i.inverted { ", inverted" } else { "" }
    )
}

/// Readout lines of a measurement: one per usable channel.
pub fn channel_lines(m: &Measurement) -> Vec<String> {
    ["L", "R"]
        .iter()
        .enumerate()
        .map(|(c, name)| match &m.channels[c] {
            Some(d) => format!(
                "{name}: delay {}, confidence {:.0}, level {:.1} dBFS, coherence {:.2}",
                delay_text(&d.info),
                d.info.confidence.min(999.0),
                d.info.level_rms_db,
                d.info.mean_coherence
            ),
            None => format!("{name}: no usable return"),
        })
        .collect()
}

pub fn history_line(r: &IterationRecord) -> String {
    let delay = r
        .delay_ms
        .iter()
        .flatten()
        .next()
        .map_or("-".to_string(), |d| format!("{d:.1} ms"));
    format!(
        "#{}  delay {delay}  rms {:.2} dB  max {:.2} dB  {}",
        r.index,
        r.rms_db,
        r.max_db,
        if r.applied {
            format!("applied, {} bands", r.bands)
        } else {
            "not applied".to_string()
        }
    )
}

/// Name and covered range of the playback curve for the panel.
pub fn curve_label(c: Option<&CalCurve>) -> String {
    match c {
        None => "none".to_string(),
        Some(c) => {
            let (lo, hi) = c.covered_range().unwrap_or((0.0, 0.0));
            let name = if c.name.is_empty() {
                "unnamed"
            } else {
                c.name.as_str()
            };
            format!(
                "{name}: {} - {} Hz ({} points)",
                fmt_hz(lo),
                fmt_hz(hi),
                c.points.len()
            )
        }
    }
}

fn fmt_hz(f: f64) -> String {
    if f >= 1000.0 {
        format!("{:.1} k", f / 1000.0).replace(".0 k", " k")
    } else {
        format!("{f:.0}")
    }
}

/// Slots whose parameters differ and must be written (and nothing else).
pub fn bands_to_write(
    current: &[BandParams; NUM_BANDS],
    new: &[BandParams; NUM_BANDS],
) -> Vec<usize> {
    (0..NUM_BANDS).filter(|&i| current[i] != new[i]).collect()
}

/// Curves shown in the main plot, all on the measurement grid, dB.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Series {
    pub freqs: Vec<f64>,
    /// Measured loop response (includes the current EQ), 0 dB = reference-band mean.
    pub measured: Vec<f64>,
    pub coherence: Vec<f64>,
    /// Requested total calibration curve (`E_cal_new`, limited).
    pub target: Vec<f64>,
    /// Total calibration curve of the fitted bands.
    pub fitted: Vec<f64>,
    /// Predicted deviation after applying the fit.
    pub predicted: Vec<f64>,
}

/// Series to draw: the plan if there is one, otherwise just the measurement.
pub fn series(
    m: Option<&Measurement>,
    plan: Option<&FitPlan>,
    s: &CalSettings,
    c: Option<&CalCurve>,
) -> Option<Series> {
    if let Some(p) = plan {
        return Some(Series {
            freqs: p.freqs.clone(),
            measured: p.deviation_db.clone(),
            coherence: p.coherence.clone(),
            target: p.target_cal_db.clone(),
            fitted: p.fitted_cal_db.clone(),
            predicted: p.predicted_db.clone(),
        });
    }
    let m = m?;
    Some(Series {
        freqs: m.freqs.clone(),
        measured: deviation_curve(m, s, c),
        coherence: m.coherence.clone(),
        ..Default::default()
    })
}

/// Map coherence `0..=1` to a dB value in the lowest fifth of the plot so it
/// can share the dB axis: 0 -> `-range`, 1 -> `-0.6 * range`.
pub fn coherence_to_db(c: f64, db_range: f64) -> f64 {
    -db_range + 0.4 * db_range * c.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::tap::CalLink;
    use std::sync::Arc;

    #[test]
    fn buttons_follow_the_phase() {
        let idle = enabled(Phase::Idle, false, false, false);
        assert!(idle.measure && idle.auto && !idle.fit && !idle.apply && !idle.stop);
        let idle_m = enabled(Phase::Idle, true, false, false);
        assert!(idle_m.fit && !idle_m.apply);
        let idle_p = enabled(Phase::Idle, true, true, false);
        assert!(idle_p.apply);
        let busy = enabled(Phase::Measuring, true, true, true);
        assert!(
            !busy.measure && !busy.auto && !busy.fit && !busy.apply && busy.stop && !busy.clear
        );
        // Stop stays available while the noise fades out after a stop
        assert!(enabled(Phase::Idle, false, false, true).stop);
    }

    #[test]
    fn status_severity() {
        let link = Arc::new(CalLink::new());
        let s = Session::new(link);
        let (t, sev) = status_line(&s);
        assert_eq!((t.as_str(), sev), ("Idle", Severity::Info));
    }

    #[test]
    fn progress_and_curve_labels() {
        assert_eq!(progress_fraction(0, 0), 0.0);
        assert!((progress_fraction(50, 200) - 0.25).abs() < 1e-6);
        assert_eq!(progress_fraction(500, 200), 1.0);
        assert_eq!(curve_label(None), "none");
        let c = CalCurve::new(
            "Test record",
            vec![
                crate::calibration::settings::CurvePoint {
                    freq_hz: 800.0,
                    db: -0.4,
                },
                crate::calibration::settings::CurvePoint {
                    freq_hz: 20_000.0,
                    db: 1.0,
                },
            ],
        );
        assert_eq!(
            curve_label(Some(&c)),
            "Test record: 800 - 20 k Hz (2 points)"
        );
    }

    #[test]
    fn only_changed_bands_are_written() {
        let a = [BandParams::default(); NUM_BANDS];
        let mut b = a;
        b[3].used = true;
        b[7].gain_db = 1.0;
        assert_eq!(bands_to_write(&a, &b), vec![3, 7]);
        assert!(bands_to_write(&a, &a).is_empty());
    }

    #[test]
    fn history_and_coherence_mapping() {
        let r = IterationRecord {
            index: 2,
            delay_ms: [Some(371.2), Some(371.2)],
            rms_db: 0.47,
            max_db: 0.9,
            bands: 7,
            applied: true,
        };
        assert_eq!(
            history_line(&r),
            "#2  delay 371.2 ms  rms 0.47 dB  max 0.90 dB  applied, 7 bands"
        );
        assert_eq!(coherence_to_db(0.0, 12.0), -12.0);
        assert!((coherence_to_db(1.0, 12.0) + 7.2).abs() < 1e-12);
        assert_eq!(coherence_to_db(5.0, 12.0), coherence_to_db(1.0, 12.0));
        assert!(series(None, None, &CalSettings::default(), None).is_none());
    }
}
