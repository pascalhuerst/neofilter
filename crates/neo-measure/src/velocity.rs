//! Groove velocity / amplitude conversions and the 45/45 channel geometry used
//! to calibrate a playback input with a test record.
//!
//! Realtime: pure arithmetic (RT-safe); [`detect_modulation`] is trivial.
//!
//! # Conventions (all *peak* values unless stated)
//!
//! * **Amplitude to velocity.** A sinusoidal groove displacement of peak
//!   amplitude `A` at frequency `f` has peak velocity `v = 2 pi f A`.
//!   315 Hz at 50 um is 9.896 cm/s peak ([`amplitude_um_to_velocity_cm_s`]).
//! * **RMS to peak.** For a sine `v_peak = sqrt(2) v_rms`
//!   ([`rms_to_peak`]). Test records quote the 1 kHz reference band as
//!   5 cm/s **RMS** (7.071 cm/s peak).
//! * **Channel to lateral (45/45 system).** The two groove walls are
//!   orthogonal, each at 45 degrees to the record surface. A *channel
//!   velocity* `v_ch` is the velocity of one wall's modulation component
//!   along that wall's direction (the quantity a record sleeve quotes for a
//!   "left channel only" band, and what a cartridge channel responds to).
//!   Writing both channels with the **same in-phase** signal produces a purely
//!   lateral groove motion whose velocity is the vector sum of the two
//!   orthogonal contributions: `v_lat = sqrt(2) v_ch`; the same signal in
//!   anti-phase gives purely vertical motion `v_vert = sqrt(2) v_ch`. A
//!   one-channel band `v_ch` has lateral and vertical components of
//!   `v_ch / sqrt(2)` each.
//! * **Calibration.** NeoMeter's `reference_level` is the dBFS of a mono
//!   (L = R, in phase) signal whose recorded peak lateral velocity is
//!   [`REF_LATERAL_PEAK_CM_S`] = 5 cm/s at 1 kHz, i.e. whose *channel*
//!   velocity is `5 / sqrt(2)` = 3.536 cm/s peak per channel. A record band with
//!   channel velocity `v` read at `L` dBFS therefore gives
//!   `reference_level = L - 20 log10(v / (5 / sqrt 2))`
//!   ([`reference_level_dbfs`]). For the 5 cm/s RMS single-channel band
//!   (`v = 5 sqrt 2`) that is `L - 6.02 dB`.

use std::f64::consts::{PI, SQRT_2};

/// Reference peak lateral velocity of the calibration, cm/s.
pub const REF_LATERAL_PEAK_CM_S: f64 = 5.0;

/// Peak velocity in cm/s of a sinusoidal groove modulation of peak amplitude
/// `amplitude_um` micrometres at `freq_hz`: `v = 2 pi f A`.
pub fn amplitude_um_to_velocity_cm_s(freq_hz: f64, amplitude_um: f64) -> f64 {
    2.0 * PI * freq_hz * amplitude_um * 1e-4
}

/// Inverse of [`amplitude_um_to_velocity_cm_s`]: peak amplitude in um.
pub fn velocity_cm_s_to_amplitude_um(freq_hz: f64, velocity_cm_s: f64) -> f64 {
    if freq_hz > 0.0 {
        velocity_cm_s / (2.0 * PI * freq_hz * 1e-4)
    } else {
        0.0
    }
}

/// Level step in dB between two modulation amplitudes (or velocities):
/// `20 log10(next / previous)`. 50 um to 60 um is +1.58 dB.
pub fn level_step_db(previous: f64, next: f64) -> f64 {
    if previous > 0.0 && next > 0.0 {
        20.0 * (next / previous).log10()
    } else {
        0.0
    }
}

/// RMS of a sine to its peak value (`x sqrt 2`).
pub fn rms_to_peak(rms: f64) -> f64 {
    rms * SQRT_2
}

/// Peak of a sine to its RMS value (`x / sqrt 2`).
pub fn peak_to_rms(peak: f64) -> f64 {
    peak / SQRT_2
}

/// Which groove motion a band (or a sweep) carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modulation {
    /// Left channel only.
    Left,
    /// Right channel only.
    Right,
    /// Both channels equal and in phase (lateral groove motion).
    Lateral,
    /// Both channels equal and in anti-phase (vertical groove motion).
    Vertical,
}

impl Modulation {
    pub const ALL: [Modulation; 4] = [Self::Left, Self::Right, Self::Lateral, Self::Vertical];

    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "L only",
            Self::Right => "R only",
            Self::Lateral => "lateral",
            Self::Vertical => "vertical",
        }
    }
}

/// Velocity of one channel's wall for a band whose *quoted* peak velocity
/// `v_peak` refers to `modulation`: a left/right band quotes the channel
/// velocity itself, a lateral/vertical band quotes the groove velocity, which
/// is `sqrt 2` times the channel velocity (see the module docs).
pub fn channel_velocity_peak(modulation: Modulation, v_peak: f64) -> f64 {
    match modulation {
        Modulation::Left | Modulation::Right => v_peak,
        Modulation::Lateral | Modulation::Vertical => v_peak / SQRT_2,
    }
}

/// Lateral groove velocity produced by the same in-phase signal of channel
/// velocity `v_ch` in both channels: `sqrt 2 v_ch`.
pub fn lateral_from_channel(v_ch: f64) -> f64 {
    v_ch * SQRT_2
}

/// Channel velocity (peak) that corresponds to the reference lateral velocity
/// ([`REF_LATERAL_PEAK_CM_S`]): `5 / sqrt 2` cm/s.
pub fn reference_channel_velocity_cm_s() -> f64 {
    REF_LATERAL_PEAK_CM_S / SQRT_2
}

/// `reference_level` (dBFS of a mono signal at 5 cm/s peak lateral) from a
/// channel measured at `measured_dbfs` (peak) while the record band has the
/// channel velocity `record_channel_velocity_peak` (cm/s peak).
pub fn reference_level_dbfs(measured_dbfs: f64, record_channel_velocity_peak: f64) -> f64 {
    measured_dbfs
        - level_step_db(
            reference_channel_velocity_cm_s(),
            record_channel_velocity_peak,
        )
}

/// Classification thresholds of [`detect_modulation`].
pub const SINGLE_CHANNEL_DB: f64 = 12.0;
/// Level difference below which both channels count as equally driven.
pub const EQUAL_CHANNELS_DB: f64 = 6.0;

/// Guess the modulation of a tone from the channel levels (dB, `left - right`
/// as `level_diff_db`) and the inter-channel phase in degrees (`L` relative to
/// `R`, `-180..=180`):
///
/// * `|diff| > 12 dB`: only the louder channel is driven;
/// * `|diff| <= 6 dB` and `|phase| < 45`: lateral; `|phase| > 135`: vertical;
/// * otherwise `None` (ambiguous: partial crosstalk, other phase).
pub fn detect_modulation(level_diff_db: f64, phase_deg: f64) -> Option<Modulation> {
    if !level_diff_db.is_finite() {
        return None;
    }
    if level_diff_db > SINGLE_CHANNEL_DB {
        return Some(Modulation::Left);
    }
    if level_diff_db < -SINGLE_CHANNEL_DB {
        return Some(Modulation::Right);
    }
    if level_diff_db.abs() <= EQUAL_CHANNELS_DB && phase_deg.is_finite() {
        let p = phase_deg.abs();
        if p < 45.0 {
            return Some(Modulation::Lateral);
        }
        if p > 135.0 {
            return Some(Modulation::Vertical);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amplitude_velocity_hand_values() {
        // 2 pi * 315 * 50e-6 m = 0.098960 m/s
        let v = amplitude_um_to_velocity_cm_s(315.0, 50.0);
        assert!((v - 9.8960).abs() < 1e-3, "{v}");
        // 100 um: exactly twice
        assert!((amplitude_um_to_velocity_cm_s(315.0, 100.0) - 2.0 * v).abs() < 1e-12);
        // 1 kHz, 5 cm/s -> 7.9577 um
        let a = velocity_cm_s_to_amplitude_um(1000.0, 5.0);
        assert!((a - 7.9577).abs() < 1e-3, "{a}");
        assert!((amplitude_um_to_velocity_cm_s(1000.0, a) - 5.0).abs() < 1e-12);
        assert_eq!(velocity_cm_s_to_amplitude_um(0.0, 5.0), 0.0);
    }

    #[test]
    fn level_steps_between_amplitudes() {
        // 20 log10(60/50) = 1.5836, 20 log10(70/60) = 1.3389, 100/90 = 0.9151
        assert!((level_step_db(50.0, 60.0) - 1.5836).abs() < 1e-3);
        assert!((level_step_db(60.0, 70.0) - 1.3389).abs() < 1e-3);
        assert!((level_step_db(90.0, 100.0) - 0.9151).abs() < 1e-3);
        assert_eq!(level_step_db(0.0, 1.0), 0.0);
    }

    #[test]
    fn rms_peak_and_channel_lateral() {
        assert!((rms_to_peak(5.0) - 7.0711).abs() < 1e-4);
        assert!((peak_to_rms(rms_to_peak(3.3)) - 3.3).abs() < 1e-12);
        // single-channel band quotes the channel velocity itself
        assert_eq!(channel_velocity_peak(Modulation::Left, 7.0), 7.0);
        assert_eq!(channel_velocity_peak(Modulation::Right, 7.0), 7.0);
        // lateral / vertical bands quote the groove velocity
        assert!((channel_velocity_peak(Modulation::Lateral, 5.0) - 3.5355).abs() < 1e-4);
        assert!((channel_velocity_peak(Modulation::Vertical, 5.0) - 3.5355).abs() < 1e-4);
        assert!((lateral_from_channel(3.5355) - 5.0).abs() < 1e-3);
        assert!((reference_channel_velocity_cm_s() - 3.5355).abs() < 1e-4);
    }

    #[test]
    fn reference_level_conversion() {
        // 5 cm/s RMS on one channel = 7.071 cm/s peak, twice the channel
        // velocity of the 5 cm/s lateral reference: -6.0206 dB
        let v = rms_to_peak(5.0);
        let r = reference_level_dbfs(-18.0, v);
        assert!((r - (-18.0 - 6.0206)).abs() < 1e-3, "{r}");
        // a lateral band of 5 cm/s peak is the reference itself
        let v = channel_velocity_peak(Modulation::Lateral, 5.0);
        assert!((reference_level_dbfs(-18.0, v) + 18.0).abs() < 1e-9);
    }

    #[test]
    fn modulation_detection() {
        assert_eq!(detect_modulation(28.0, 12.0), Some(Modulation::Left));
        assert_eq!(detect_modulation(-31.0, 170.0), Some(Modulation::Right));
        assert_eq!(detect_modulation(0.3, 4.0), Some(Modulation::Lateral));
        assert_eq!(detect_modulation(-1.0, -178.0), Some(Modulation::Vertical));
        assert_eq!(detect_modulation(0.0, 90.0), None);
        assert_eq!(detect_modulation(9.0, 0.0), None);
        assert_eq!(detect_modulation(f64::NAN, 0.0), None);
    }
}
