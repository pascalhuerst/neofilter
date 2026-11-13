//! Analytic magnitude responses of the digital filters (for the GUI and tests).
//!
//! Everything here is allocation-free and cheap enough to call from the GUI
//! thread every frame (24 bands x ~500 points): per point one `sin` and a
//! handful of multiplies per section, evaluated in the cancellation-free phi
//! basis (see [`crate::biquad::PhiForm`]).

use crate::analog::AnalogBand;
use crate::band::BandDesign;
use crate::biquad::PhiForm;
use crate::types::{BandParams, MAX_SECTIONS};
use std::f64::consts::PI;

/// Floor for responses of exact zeros (notch centre), in dB.
pub const DB_FLOOR: f64 = -400.0;

fn to_db(power_ratio: f64) -> f64 {
    if power_ratio > 0.0 {
        (10.0 * power_ratio.log10()).max(DB_FLOOR)
    } else {
        DB_FLOOR
    }
}

/// dB response of a designed band at `freq_hz` (digital, what is heard).
fn design_db(forms: &[PhiForm], n: usize, freq_hz: f64, sample_rate: f64) -> f64 {
    let phi1 = (PI * freq_hz / sample_rate).sin().powi(2);
    let mut p = 1.0;
    for f in &forms[..n] {
        p *= f.mag_sq_phi1(phi1);
    }
    to_db(p)
}

fn forms_of(d: &BandDesign) -> ([PhiForm; MAX_SECTIONS], usize) {
    let mut forms = [PhiForm {
        b: [1.0, 1.0, 0.0],
        a: [1.0, 1.0, 0.0],
    }; MAX_SECTIONS];
    for (f, c) in forms.iter_mut().zip(d.sections()) {
        *f = c.phi_form();
    }
    (forms, d.len())
}

/// Magnitude response in dB of one band's *digital* filter at the given
/// frequencies (no smoothing, target params). `used` / `enabled` are ignored:
/// this is the response the band would have if active. `out_db` must be at
/// least as long as `freqs_hz`; extra entries are left untouched.
pub fn band_response_db(
    params: &BandParams,
    gain_scale: f64,
    sample_rate: f64,
    freqs_hz: &[f64],
    out_db: &mut [f64],
) {
    let d = BandDesign::design(params, gain_scale, sample_rate);
    let (forms, n) = forms_of(&d);
    for (o, &f) in out_db.iter_mut().zip(freqs_hz) {
        *o = design_db(&forms, n, f, sample_rate);
    }
}

/// Sum of all used+enabled bands (placement ignored). Same buffer rules as
/// [`band_response_db`].
pub fn total_response_db(
    bands: &[BandParams],
    gain_scale: f64,
    sample_rate: f64,
    freqs_hz: &[f64],
    out_db: &mut [f64],
) {
    let m = freqs_hz.len().min(out_db.len());
    out_db[..m].fill(0.0);
    for b in bands.iter().filter(|b| b.used && b.enabled) {
        let d = BandDesign::design(b, gain_scale, sample_rate);
        if d.is_empty() {
            continue;
        }
        let (forms, n) = forms_of(&d);
        for (o, &f) in out_db[..m].iter_mut().zip(freqs_hz) {
            *o += design_db(&forms, n, f, sample_rate);
        }
    }
}

/// Ground-truth *analog* prototype response in dB of one band (same gain
/// scale and design-frequency clamp as the digital design; `used`/`enabled`
/// ignored). For tests, the CLI and calibration.
pub fn analog_band_response_db(
    params: &BandParams,
    gain_scale: f64,
    sample_rate: f64,
    freqs_hz: &[f64],
    out_db: &mut [f64],
) {
    let a = AnalogBand::from_params(params, gain_scale, sample_rate);
    for (o, &f) in out_db.iter_mut().zip(freqs_hz) {
        *o = a.magnitude_db(f).max(DB_FLOOR);
    }
}
