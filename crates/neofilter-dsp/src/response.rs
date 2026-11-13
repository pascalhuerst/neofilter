//! Analytic magnitude responses of the digital filters (for the GUI and tests).

use crate::types::BandParams;

/// Magnitude response in dB of one band's *digital* filter at the given
/// frequencies (no smoothing, target params). `out_db` must be at least as
/// long as `freqs_hz`; extra entries are left untouched.
///
/// Stub: fills zeros.
pub fn band_response_db(
    params: &BandParams,
    gain_scale: f64,
    sample_rate: f64,
    freqs_hz: &[f64],
    out_db: &mut [f64],
) {
    let _ = (params, gain_scale, sample_rate);
    out_db[..freqs_hz.len()].fill(0.0);
}

/// Sum of all used+enabled bands (placement ignored). Same buffer rules as
/// [`band_response_db`].
///
/// Stub: fills zeros.
pub fn total_response_db(
    bands: &[BandParams],
    gain_scale: f64,
    sample_rate: f64,
    freqs_hz: &[f64],
    out_db: &mut [f64],
) {
    let _ = (bands, gain_scale, sample_rate);
    out_db[..freqs_hz.len()].fill(0.0);
}
