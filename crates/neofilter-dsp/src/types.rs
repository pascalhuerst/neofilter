//! Shared band parameter types and limits.

/// Number of EQ bands.
pub const NUM_BANDS: usize = 24;
/// Maximum number of biquad sections per band.
pub const MAX_SECTIONS: usize = 8;
/// Minimum band frequency in Hz.
pub const MIN_FREQ_HZ: f64 = 10.0;
/// Maximum band frequency in Hz.
pub const MAX_FREQ_HZ: f64 = 30000.0;
/// Minimum displayed Q.
pub const MIN_Q: f64 = 0.025;
/// Maximum displayed Q.
pub const MAX_Q: f64 = 40.0;
/// Maximum absolute band gain in dB.
pub const MAX_GAIN_DB: f64 = 30.0;

/// Filter shape of a band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Bell,
    LowShelf,
    LowCut,
    HighShelf,
    HighCut,
    Notch,
    BandPass,
    TiltShelf,
    FlatTilt,
}

impl Shape {
    /// All shapes in declaration order.
    pub const ALL: [Self; 9] = [
        Self::Bell,
        Self::LowShelf,
        Self::LowCut,
        Self::HighShelf,
        Self::HighCut,
        Self::Notch,
        Self::BandPass,
        Self::TiltShelf,
        Self::FlatTilt,
    ];

    /// Human readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bell => "Bell",
            Self::LowShelf => "Low Shelf",
            Self::LowCut => "Low Cut",
            Self::HighShelf => "High Shelf",
            Self::HighCut => "High Cut",
            Self::Notch => "Notch",
            Self::BandPass => "Band Pass",
            Self::TiltShelf => "Tilt Shelf",
            Self::FlatTilt => "Flat Tilt",
        }
    }
}

/// Filter slope in dB per octave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slope {
    Db6,
    Db12,
    Db18,
    Db24,
    Db30,
    Db36,
    Db48,
    Db72,
    Db96,
}

impl Slope {
    /// All slopes in ascending order.
    pub const ALL: [Self; 9] = [
        Self::Db6,
        Self::Db12,
        Self::Db18,
        Self::Db24,
        Self::Db30,
        Self::Db36,
        Self::Db48,
        Self::Db72,
        Self::Db96,
    ];

    /// Slope in dB per octave.
    pub fn db_per_oct(&self) -> u32 {
        match self {
            Self::Db6 => 6,
            Self::Db12 => 12,
            Self::Db18 => 18,
            Self::Db24 => 24,
            Self::Db30 => 30,
            Self::Db36 => 36,
            Self::Db48 => 48,
            Self::Db72 => 72,
            Self::Db96 => 96,
        }
    }

    /// Human readable name, e.g. "12 dB/oct".
    pub fn name(&self) -> &'static str {
        match self {
            Self::Db6 => "6 dB/oct",
            Self::Db12 => "12 dB/oct",
            Self::Db18 => "18 dB/oct",
            Self::Db24 => "24 dB/oct",
            Self::Db30 => "30 dB/oct",
            Self::Db36 => "36 dB/oct",
            Self::Db48 => "48 dB/oct",
            Self::Db72 => "72 dB/oct",
            Self::Db96 => "96 dB/oct",
        }
    }
}

/// Which part of the stereo signal a band acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placement {
    Stereo,
    Left,
    Right,
    Mid,
    Side,
}

impl Placement {
    /// All placements in declaration order.
    pub const ALL: [Self; 5] = [Self::Stereo, Self::Left, Self::Right, Self::Mid, Self::Side];

    /// Human readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Stereo => "Stereo",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Mid => "Mid",
            Self::Side => "Side",
        }
    }
}

/// User-facing parameters of one band (display conventions, see `conventions`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandParams {
    /// Band exists (slot in use).
    pub used: bool,
    /// Band is enabled (not bypassed).
    pub enabled: bool,
    pub shape: Shape,
    /// Frequency in Hz.
    pub freq_hz: f64,
    /// Gain in dB (before gain scale).
    pub gain_db: f64,
    /// Displayed Q.
    pub q: f64,
    pub slope: Slope,
    pub placement: Placement,
}

impl Default for BandParams {
    fn default() -> Self {
        Self {
            used: false,
            enabled: true,
            shape: Shape::Bell,
            freq_hz: 1000.0,
            gain_db: 0.0,
            q: 1.0,
            slope: Slope::Db12,
            placement: Placement::Stereo,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slopes_ascending() {
        let v: Vec<u32> = Slope::ALL.iter().map(|s| s.db_per_oct()).collect();
        assert_eq!(v, [6, 12, 18, 24, 30, 36, 48, 72, 96]);
    }

    #[test]
    fn names_nonempty_and_unique() {
        let mut n: Vec<_> = Shape::ALL.iter().map(|s| s.name()).collect();
        n.sort();
        n.dedup();
        assert_eq!(n.len(), Shape::ALL.len());
        assert!(Placement::ALL.iter().all(|p| !p.name().is_empty()));
    }

    #[test]
    fn default_band() {
        let b = BandParams::default();
        assert!(!b.used && b.enabled);
        assert_eq!(b.shape, Shape::Bell);
        assert_eq!(b.slope, Slope::Db12);
    }
}
