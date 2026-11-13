//! Pro-Q-compatible parameter conventions. Single source of truth: if
//! calibration shows a deviation, adjust the constants/functions here only.

use crate::types::{Shape, Slope};

/// Displayed Q 1.0 corresponds to textbook Q 1/sqrt(2).
pub const Q_DISPLAY_TO_STD: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Exponent of the displayed Q in the shelf resonance law: the highest-Q
/// Butterworth pair of a Low / High / Tilt Shelf gets the section Q
/// `Q_butterworth * Q_display^SHELF_Q_EXPONENT` (zero and pole frequencies are
/// unchanged, first-order shelves have no Q). This is a measured fit to the
/// reference EQ (12 and 24 dB/oct, low and high shelf, boost and cut, Q 0.5 to
/// 6), with a residual of at most 0.003 dB in the fixtures; it is not derived
/// from any documentation.
pub const SHELF_Q_EXPONENT: f64 = 0.47;

// Tilt shelf convention (measured): `tilt(g) = high_shelf(2 g) - g dB` at the
// same frequency, slope and Q, so a +6 dB tilt runs from -6 dB at low
// frequencies through 0 dB at the band frequency to +6 dB at high ones.
// Implemented in `analog.rs`.

/// Flat tilt definition: `gain_db` (after gain scale) is the total level
/// difference in dB across this many octaves, i.e. the tilt is
/// `gain_db / FLAT_TILT_OCTAVES_PER_GAIN` dB per octave. Positive gain lifts
/// the highs and lowers the lows, pivoting (0 dB) at the band frequency.
/// 10 octaves is roughly 20 Hz to 20 kHz, so +-30 dB is +-3 dB/oct and the
/// end-to-end level difference across the audible band is about `gain_db`.
pub const FLAT_TILT_OCTAVES_PER_GAIN: f64 = 10.0;

/// Highest design frequency as a fraction of the sample rate. Band
/// frequencies above this are clamped (the analog prototype is designed at the
/// clamped frequency, so ground truth and digital filter agree).
pub const MAX_DESIGN_FREQ_RATIO: f64 = 0.49;

/// Order N of a cut, shelf or tilt filter: slope / 6.
pub fn cut_shelf_order(slope: Slope) -> usize {
    (slope.db_per_oct() / 6) as usize
}

/// Low-pass prototype order M of a bell/notch/band-pass: max(1, slope / 12).
/// The resulting band filter has order 2M (6 dB/oct behaves as 12).
///
/// Notch and band pass definitions (not specified by the public Pro-Q
/// documentation): both use `Qs = Q_DISPLAY_TO_STD * Q_display` and a
/// Butterworth prototype of order M through the low-pass to band-pass map
/// `s -> Qs (s/w0 + w0/s)`. The band pass has unity gain at the band
/// frequency (for M = 1 the -3 dB edges are `1/Qs` band-frequency units
/// apart); the notch has M-fold zeros on the jw axis at the band frequency and
/// the same Butterworth band-stop poles, unity gain at DC and infinity. Gain is
/// ignored by both. The formulas live in `analog.rs`.
pub fn bell_prototype_order(slope: Slope) -> usize {
    ((slope.db_per_oct() / 12) as usize).max(1)
}

/// Analog filter order for a shape and slope (band order for bell-like shapes).
/// Flat tilt ignores the slope and returns 1.
pub fn order_for(shape: Shape, slope: Slope) -> usize {
    match shape {
        Shape::LowCut | Shape::HighCut | Shape::LowShelf | Shape::HighShelf | Shape::TiltShelf => {
            cut_shelf_order(slope)
        }
        Shape::Bell | Shape::Notch | Shape::BandPass => 2 * bell_prototype_order(slope),
        Shape::FlatTilt => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn q_factor() {
        assert_relative_eq!(Q_DISPLAY_TO_STD, 2.0_f64.sqrt() / 2.0, epsilon = 1e-15);
        assert_relative_eq!(Q_DISPLAY_TO_STD * Q_DISPLAY_TO_STD, 0.5, epsilon = 1e-15);
    }

    #[test]
    fn cut_orders() {
        let o: Vec<usize> = Slope::ALL.iter().map(|&s| cut_shelf_order(s)).collect();
        assert_eq!(o, [1, 2, 3, 4, 5, 6, 8, 12, 16]);
    }

    #[test]
    fn bell_orders() {
        let m: Vec<usize> = Slope::ALL
            .iter()
            .map(|&s| bell_prototype_order(s))
            .collect();
        assert_eq!(m, [1, 1, 1, 2, 2, 3, 4, 6, 8]);
        assert_eq!(bell_prototype_order(Slope::Db36), 3);
    }

    #[test]
    fn order_for_shapes() {
        assert_eq!(order_for(Shape::LowCut, Slope::Db24), 4);
        assert_eq!(order_for(Shape::Bell, Slope::Db36), 6);
        assert_eq!(order_for(Shape::Notch, Slope::Db6), 2);
        assert_eq!(order_for(Shape::FlatTilt, Slope::Db96), 1);
    }
}
