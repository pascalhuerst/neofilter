//! Offline calibration from two recordings (used by `neofilter-cli calibrate`).
//!
//! Same analysis and fit code as the live session: [`analyze_capture`] then
//! [`plan_fit`]. The recordings are the signal sent into the loop and the
//! return of the loop.

use super::analysis::{analyze_capture, CalError, Capture, Measurement};
use super::fit::{apply_bands, plan_fit, FitPlan};
use super::settings::CalPersist;
use neofilter_dsp::response::total_response_db;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;

/// How the offline recordings relate to NeoFilter.
pub struct OfflineInput<'a> {
    pub sample_rate: f64,
    /// Reference (mono) as sent into the loop.
    pub sent: &'a [f32],
    /// Return, left and right (a mono return is passed twice).
    pub ret: [&'a [f32]; 2],
    /// The bands currently active in the chain (manual and calibration).
    pub bands: [BandParams; NUM_BANDS],
    pub gain_scale: f64,
    /// `sent` was recorded after the EQ (so the measured transfer function
    /// lacks the EQ, which is added back); otherwise it is the pre-EQ signal
    /// (exactly what the plugin's tap records).
    pub sent_is_post_eq: bool,
}

/// Result of an offline calibration.
pub struct OfflineResult {
    pub measurement: Measurement,
    pub plan: FitPlan,
    /// The full band array with the fitted calibration bands written in.
    pub bands: [BandParams; NUM_BANDS],
}

/// Analyse the recordings and fit calibration bands. `persist` carries the
/// options, the calibration band mask and the playback curve.
pub fn calibrate_offline(
    input: &OfflineInput,
    persist: &CalPersist,
) -> Result<OfflineResult, CalError> {
    let n = input
        .sent
        .len()
        .min(input.ret[0].len())
        .min(input.ret[1].len());
    let mut cap = Capture::default();
    cap.reserve(n);
    for i in 0..n {
        cap.push([input.sent[i], input.ret[0][i], input.ret[1][i]]);
    }
    let mut s = persist.settings.clone();
    s.sanitize();
    let mut m = analyze_capture(&cap, input.sample_rate, &s)?;
    if input.sent_is_post_eq {
        let mut eq = vec![0.0; m.freqs.len()];
        total_response_db(
            &input.bands,
            input.gain_scale,
            input.sample_rate,
            &m.freqs,
            &mut eq,
        );
        for (v, e) in m.mag_db.iter_mut().zip(&eq) {
            *v += e;
        }
        for ch in m.channels.iter_mut().flatten() {
            for (v, e) in ch.mag_db.iter_mut().zip(&eq) {
                *v += e;
            }
        }
    }
    let plan = plan_fit(
        &m,
        &input.bands,
        persist.band_mask,
        input.gain_scale,
        &s,
        persist.correction.as_ref(),
    )
    .map_err(CalError::Other)?;
    let bands = apply_bands(&input.bands, plan.mask, &plan.bands)
        .ok_or_else(|| CalError::Other("fitted bands do not fit the calibration slots".into()))?;
    Ok(OfflineResult {
        measurement: m,
        plan,
        bands,
    })
}
