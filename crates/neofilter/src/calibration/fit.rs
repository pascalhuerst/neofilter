//! From a measurement to calibration bands.
//!
//! Terminology (all curves on the measurement grid, dB):
//!
//! * `M`: measured loop response (reference -> return), *including* the EQ the
//!   noise passed through (manual and calibration bands).
//! * `P`: optional playback-chain correction (pickup/preamp, relative to
//!   nominal). `D = (M - P)` normalised to 0 dB mean over the reference band
//!   is the **deviation** of the loop from flat.
//! * `E_cal_current`: summed response of the *calibration* bands at
//!   measurement time. Manual bands are part of the measured chain and stay
//!   untouched.
//! * New calibration curve `E_cal_new = E_cal_current - D`, with the
//!   correction `-D` faded to 0 over half an octave outside the fit range,
//!   points below the coherence threshold interpolated and given zero
//!   weight, and the *total* clamped to `[-max_cut, +max_boost]`.
//!   `autofit::fit_bands` approximates `E_cal_new` with bands.

use super::analysis::{mean_in_range, Measurement};
use super::settings::{CalCurve, CalSettings};
use neofilter_dsp::autofit::{bands_response_db, fit_bands, FitOptions};
use neofilter_dsp::response::total_response_db;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;

/// Weight of points outside the fit range (they only keep the curve from
/// moving there).
const OUTSIDE_WEIGHT: f64 = 0.3;
/// Octaves over which the correction fades out beyond the fit range.
const TAPER_OCT: f64 = 0.5;

/// Everything computed from one measurement (also what the editor draws).
#[derive(Debug, Clone)]
pub struct FitPlan {
    pub freqs: Vec<f64>,
    pub coherence: Vec<f64>,
    /// Per-channel measured response normalised to its reference-band mean (no `P` removed).
    pub channel_db: [Option<Vec<f64>>; 2],
    /// `M` of the combined channels, normalised (no `P` removed).
    pub measured_db: Vec<f64>,
    /// `D = (M - P)` normalised.
    pub deviation_db: Vec<f64>,
    pub current_cal_db: Vec<f64>,
    /// Requested `E_cal_new` (clamped).
    pub target_cal_db: Vec<f64>,
    /// `target - current`: the correction asked for.
    pub correction_db: Vec<f64>,
    pub fitted_cal_db: Vec<f64>,
    /// Predicted `D` after applying the fit: `D - (fitted - current)`.
    pub predicted_db: Vec<f64>,
    pub weights: Vec<f64>,
    pub bands: Vec<BandParams>,
    /// In-range RMS / max of `D` (valid coherence points only).
    pub rms_dev_db: f64,
    pub max_dev_db: f64,
    pub rms_pred_db: f64,
    pub max_pred_db: f64,
    /// Fit residual against the clamped target.
    pub fit_rms_db: f64,
    pub fit_max_db: f64,
    /// Calibration slots (ascending) used for this plan.
    pub cal_slots: Vec<usize>,
    /// Effective ownership mask of this plan.
    pub mask: u32,
    /// Band array the measurement was taken with (validated again on apply).
    pub snapshot: [BandParams; NUM_BANDS],
    pub gain_scale: f64,
    pub warnings: Vec<String>,
}

/// Effective mask for a fit and the mask to persist.
///
/// * `replace_all`: every band (and every band is persisted as a calibration band);
/// * no persisted mask yet: all currently unused bands become calibration bands;
/// * otherwise the persisted mask.
pub fn resolve_mask(
    bands: &[BandParams; NUM_BANDS],
    persisted: u32,
    replace_all: bool,
) -> (u32, u32) {
    if replace_all {
        return (u32::MAX, u32::MAX);
    }
    if persisted != 0 {
        return (persisted, persisted);
    }
    let m = bands
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.used)
        .fold(0u32, |a, (i, _)| a | (1 << i));
    (m, m)
}

fn slots_of(mask: u32) -> Vec<usize> {
    (0..NUM_BANDS).filter(|&i| mask & (1 << i) != 0).collect()
}

/// Write `fitted` into the calibration slots of `bands`; remaining
/// calibration slots become unused. Values are rounded to `f32` like host
/// parameters. Manual bands are untouched. Returns `None` if the bands do
/// not fit into the calibration slots.
pub fn apply_bands(
    bands: &[BandParams; NUM_BANDS],
    mask: u32,
    fitted: &[BandParams],
) -> Option<[BandParams; NUM_BANDS]> {
    let slots = slots_of(mask);
    if fitted.len() > slots.len() {
        return None;
    }
    let mut out = *bands;
    for (k, &slot) in slots.iter().enumerate() {
        out[slot] = match fitted.get(k) {
            Some(b) => BandParams {
                used: true,
                enabled: true,
                freq_hz: f64::from(b.freq_hz as f32),
                gain_db: f64::from(b.gain_db as f32),
                q: f64::from(b.q as f32),
                ..*b
            },
            None => BandParams::default(),
        };
    }
    Some(out)
}

/// Response of the calibration bands (mask) alone.
pub fn cal_response_db(
    bands: &[BandParams; NUM_BANDS],
    mask: u32,
    gain_scale: f64,
    sample_rate: f64,
    freqs: &[f64],
) -> Vec<f64> {
    let mut only = *bands;
    for (i, b) in only.iter_mut().enumerate() {
        if mask & (1 << i) == 0 {
            b.used = false;
        }
    }
    let mut out = vec![0.0; freqs.len()];
    total_response_db(&only, gain_scale, sample_rate, freqs, &mut out);
    out
}

/// In-range (RMS, max |.|) of `dev` over points whose coherence is valid.
pub fn deviation_stats(freqs: &[f64], dev: &[f64], coh: &[f64], s: &CalSettings) -> (f64, f64) {
    let (mut se, mut n, mut mx) = (0.0, 0usize, 0.0f64);
    for i in 0..freqs.len() {
        if freqs[i] >= s.range_lo_hz && freqs[i] <= s.range_hi_hz && coh[i] >= s.coherence_min {
            se += dev[i] * dev[i];
            n += 1;
            mx = mx.max(dev[i].abs());
        }
    }
    if n == 0 {
        (0.0, 0.0)
    } else {
        ((se / n as f64).sqrt(), mx)
    }
}

/// Fade 1 inside the range, cos^2 down to 0 over [`TAPER_OCT`] outside.
fn taper(f: f64, lo: f64, hi: f64) -> f64 {
    let d = if f < lo {
        (lo / f).log2()
    } else if f > hi {
        (f / hi).log2()
    } else {
        return 1.0;
    };
    if d >= TAPER_OCT {
        0.0
    } else {
        (0.5 * std::f64::consts::PI * d / TAPER_OCT).cos().powi(2)
    }
}

/// Linear interpolation over log f of `v` at the invalid points from the
/// valid ones; ends held. `None` without any valid point.
fn fill_invalid(freqs: &[f64], v: &[f64], valid: &[bool]) -> Option<Vec<f64>> {
    let idx: Vec<usize> = (0..v.len()).filter(|&i| valid[i]).collect();
    if idx.is_empty() {
        return None;
    }
    let mut out = v.to_vec();
    for i in 0..v.len() {
        if valid[i] {
            continue;
        }
        let p = idx.partition_point(|&k| k < i);
        out[i] = if p == 0 {
            v[idx[0]]
        } else if p == idx.len() {
            v[idx[idx.len() - 1]]
        } else {
            let (a, b) = (idx[p - 1], idx[p]);
            let t = (freqs[i] / freqs[a]).ln() / (freqs[b] / freqs[a]).ln();
            v[a] + t * (v[b] - v[a])
        };
    }
    Some(out)
}

/// `D = (M - P)` normalised to 0 dB mean over the reference band.
pub fn deviation_curve(
    m: &Measurement,
    s: &CalSettings,
    correction: Option<&CalCurve>,
) -> Vec<f64> {
    let raw: Vec<f64> = m
        .freqs
        .iter()
        .zip(&m.mag_db)
        .map(|(&f, &a)| a - correction.map_or(0.0, |c| c.eval(f)))
        .collect();
    let r = mean_in_range(&m.freqs, &raw, s.ref_lo_hz, s.ref_hi_hz);
    raw.iter().map(|x| x - r).collect()
}

/// Compute the fit for a measurement.
pub fn plan_fit(
    m: &Measurement,
    bands: &[BandParams; NUM_BANDS],
    persisted_mask: u32,
    gain_scale: f64,
    s: &CalSettings,
    correction: Option<&CalCurve>,
) -> Result<FitPlan, String> {
    if !(gain_scale.is_finite() && gain_scale >= 0.05) {
        return Err("gain scale is too small for calibration".into());
    }
    let (mask, _) = resolve_mask(bands, persisted_mask, s.replace_all);
    let cal_slots = slots_of(mask);
    if cal_slots.is_empty() {
        return Err("no free band slots for calibration".into());
    }
    let f = &m.freqs;
    let n = f.len();
    let norm = |v: &[f64]| -> Vec<f64> {
        let r = mean_in_range(f, v, s.ref_lo_hz, s.ref_hi_hz);
        v.iter().map(|x| x - r).collect()
    };
    let measured = norm(&m.mag_db);
    let channel_db = [0, 1].map(|c| m.channels[c].as_ref().map(|d| norm(&d.mag_db)));
    let dev = deviation_curve(m, s, correction);

    let valid: Vec<bool> = m.coherence.iter().map(|&c| c >= s.coherence_min).collect();
    let in_range = |x: f64| x >= s.range_lo_hz && x <= s.range_hi_hz;
    if !(0..n).any(|i| valid[i] && in_range(f[i])) {
        return Err("no frequency point has sufficient coherence".into());
    }
    let neg: Vec<f64> = dev.iter().map(|d| -d).collect();
    let filled = fill_invalid(f, &neg, &valid).ok_or("no valid points")?;

    let current = cal_response_db(bands, mask, gain_scale, m.sample_rate, f);
    let mut weights = vec![0.0; n];
    let mut target = vec![0.0; n];
    for i in 0..n {
        let t = taper(f[i], s.range_lo_hz, s.range_hi_hz);
        let corr = filled[i] * t;
        target[i] = (current[i] + corr).clamp(-s.max_cut_db, s.max_boost_db);
        weights[i] = if in_range(f[i]) {
            if valid[i] {
                1.0
            } else {
                0.0
            }
        } else {
            OUTSIDE_WEIGHT
        };
    }

    // fit on every second grid point (about 24 points per octave)
    let sel: Vec<usize> = (0..n).step_by(2).collect();
    let pts: Vec<(f64, f64)> = sel.iter().map(|&i| (f[i], target[i])).collect();
    let w: Vec<f64> = sel.iter().map(|&i| weights[i]).collect();
    let opts = FitOptions {
        max_bands: s.max_bands.min(cal_slots.len()),
        allow_low_shelf: s.use_shelves,
        allow_high_shelf: s.use_shelves,
        max_boost_db: s.max_boost_db,
        max_cut_db: s.max_cut_db,
        min_freq_hz: 20.0,
        max_freq_hz: 20_000.0,
        gain_scale,
        ..Default::default()
    };
    let fit = fit_bands(&pts, Some(&w), m.sample_rate, &opts);
    let fitted = bands_response_db(&fit.bands, gain_scale, m.sample_rate, f);
    let predicted: Vec<f64> = (0..n).map(|i| dev[i] - (fitted[i] - current[i])).collect();
    let (rms_dev, max_dev) = deviation_stats(f, &dev, &m.coherence, s);
    let (rms_pred, max_pred) = deviation_stats(f, &predicted, &m.coherence, s);
    let mut warnings = Vec::new();
    if (0..n).any(|i| in_range(f[i]) && valid[i] && (target[i] - (current[i] - dev[i])).abs() > 0.5)
    {
        warnings.push("correction limited by the boost/cut limits".into());
    }
    if valid.iter().zip(f).any(|(v, &x)| !v && in_range(x)) {
        warnings.push("some frequencies have low coherence and are left unconstrained".into());
    }
    Ok(FitPlan {
        freqs: f.clone(),
        coherence: m.coherence.clone(),
        channel_db,
        measured_db: measured,
        deviation_db: dev,
        correction_db: (0..n).map(|i| target[i] - current[i]).collect(),
        current_cal_db: current,
        target_cal_db: target,
        fitted_cal_db: fitted,
        predicted_db: predicted,
        weights,
        bands: fit.bands,
        rms_dev_db: rms_dev,
        max_dev_db: max_dev,
        rms_pred_db: rms_pred,
        max_pred_db: max_pred,
        fit_rms_db: fit.residual_rms_db,
        fit_max_db: fit.residual_max_db,
        cal_slots,
        mask,
        snapshot: *bands,
        gain_scale,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_resolution() {
        let mut b = [BandParams::default(); NUM_BANDS];
        b[0].used = true;
        b[5].used = true;
        let (m, keep) = resolve_mask(&b, 0, false);
        assert_eq!(m, keep);
        assert_eq!(m & 1, 0);
        assert_eq!(m & (1 << 5), 0);
        assert_eq!(m.count_ones(), 30);
        // persisted mask wins
        assert_eq!(resolve_mask(&b, 0b110, false), (0b110, 0b110));
        // replace all makes every band a calibration band
        assert_eq!(resolve_mask(&b, 0b110, true), (u32::MAX, u32::MAX));
    }

    #[test]
    fn apply_only_touches_calibration_slots() {
        let mut b = [BandParams::default(); NUM_BANDS];
        b[0] = BandParams {
            used: true,
            gain_db: 3.0,
            ..Default::default()
        };
        b[2] = BandParams {
            used: true,
            gain_db: -2.0,
            ..Default::default()
        };
        let mask = !1u32; // slot 0 manual, slot 2 is a calibration slot with an old band
        let fitted = [BandParams {
            used: true,
            freq_hz: 1_234.567_891,
            gain_db: 1.0,
            ..Default::default()
        }];
        let out = apply_bands(&b, mask, &fitted).unwrap();
        assert_eq!(out[0], b[0]);
        assert!(out[1].used && (out[1].freq_hz - 1_234.567_891).abs() < 1e-3);
        assert_eq!(out[1].freq_hz, f64::from(1_234.567_891_f64 as f32));
        assert_eq!(out[2], BandParams::default());
        assert!(apply_bands(&b, 0b1, &[fitted[0], fitted[0]]).is_none());
    }

    #[test]
    fn taper_and_fill() {
        assert_eq!(taper(100.0, 30.0, 16000.0), 1.0);
        assert!((taper(30.0 / 2f64.powf(0.25), 30.0, 16000.0) - 0.5).abs() < 1e-9);
        assert_eq!(taper(10.0, 30.0, 16000.0), 0.0);
        let f = [1.0, 2.0, 4.0, 8.0];
        let v = [5.0, 0.0, 0.0, 1.0];
        let valid = [true, false, false, true];
        let o = fill_invalid(&f, &v, &valid).unwrap();
        assert!((o[1] - 5.0 * 2.0 / 3.0 - 1.0 / 3.0).abs() < 1e-9, "{o:?}");
        assert!(fill_invalid(&f, &v, &[false; 4]).is_none());
        let o = fill_invalid(&f, &v, &[false, false, true, false]).unwrap();
        assert_eq!(o, [0.0; 4]);
    }
}
