//! Reading and writing `.ffp` EQ preset files (the "FQ3p" flavour).
//!
//! The layout was worked out from a preset file exported by a user, not from
//! any program code: a 4-byte tag `FQ3p`, a little-endian `u32` version, a
//! `u32` value count, then that many little-endian `f32` values. The first
//! 24 x 13 values are the bands; the rest are global settings.
//!
//! Per band (13 values), in order: used, enabled, log2(frequency in Hz),
//! gain in dB, dynamic range, dynamics enabled, threshold, Q (logarithmic,
//! 0..1 over 0.025..40), shape index, slope index, stereo placement,
//! speakers, solo.
//!
//! Only the fields this tool understands are written; everything else
//! (dynamics, speakers, solo and all global settings) is copied from a
//! template file, so exporting always needs an existing `.ffp` as template.
//!
//! Confirmed against the sample file: frequency, gain and Q encodings, the
//! default slope index (1 = 12 dB/oct) and stereo placement 2 = Stereo. The
//! shape and slope index tables follow the order of the corresponding menus
//! and are otherwise unverified; non-stereo placements are refused on export
//! because their codes are unknown.

use neofilter_dsp::types::{MAX_Q, MIN_Q, NUM_BANDS};
use neofilter_dsp::{BandParams, Placement, Shape, Slope};

const MAGIC: &[u8; 4] = b"FQ3p";
/// Bands in the file format (fewer than `NUM_BANDS`).
pub const FILE_BANDS: usize = 24;
const PER_BAND: usize = 13;
const HEADER: usize = 12;

const F_USED: usize = 0;
const F_ENABLED: usize = 1;
const F_FREQ: usize = 2;
const F_GAIN: usize = 3;
const F_Q: usize = 7;
const F_SHAPE: usize = 8;
const F_SLOPE: usize = 9;
const F_PLACE: usize = 10;

const PLACE_STEREO: f32 = 2.0;

const SHAPES: [Shape; 9] = [
    Shape::Bell,
    Shape::LowShelf,
    Shape::LowCut,
    Shape::HighShelf,
    Shape::HighCut,
    Shape::Notch,
    Shape::BandPass,
    Shape::TiltShelf,
    Shape::FlatTilt,
];

const SLOPES: [Slope; 9] = [
    Slope::Db6,
    Slope::Db12,
    Slope::Db18,
    Slope::Db24,
    Slope::Db30,
    Slope::Db36,
    Slope::Db48,
    Slope::Db72,
    Slope::Db96,
];

/// A decoded preset file: header fields plus the raw values.
#[derive(Debug, Clone, PartialEq)]
pub struct FfpFile {
    pub version: u32,
    pub values: Vec<f32>,
}

fn q_to_norm(q: f64) -> f32 {
    let q = q.clamp(MIN_Q, MAX_Q);
    ((q / MIN_Q).ln() / (MAX_Q / MIN_Q).ln()) as f32
}

fn norm_to_q(n: f32) -> f64 {
    MIN_Q * (MAX_Q / MIN_Q).powf(f64::from(n).clamp(0.0, 1.0))
}

fn index_of<T: PartialEq>(table: &[T], v: &T) -> f32 {
    table.iter().position(|x| x == v).unwrap_or(0) as f32
}

fn lookup<T: Copy>(table: &[T], v: f32, what: &str, band: usize) -> Result<T, String> {
    let i = v.round();
    if !v.is_finite() || i < 0.0 || i as usize >= table.len() {
        return Err(format!("band {band}: unsupported {what} code {v}"));
    }
    Ok(table[i as usize])
}

impl FfpFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < HEADER || &bytes[..4] != MAGIC {
            return Err("not an FQ3p preset file (bad header)".into());
        }
        let u32_at =
            |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let version = u32_at(4);
        let count = u32_at(8) as usize;
        if count < FILE_BANDS * PER_BAND {
            return Err(format!(
                "preset holds {count} values, expected at least {}",
                FILE_BANDS * PER_BAND
            ));
        }
        let need = count
            .checked_mul(4)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or("preset value count overflows")?;
        if bytes.len() != need {
            return Err(format!(
                "preset is {} bytes, header says {need}",
                bytes.len()
            ));
        }
        let values = bytes[HEADER..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        Ok(Self { version, values })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.values.len() * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(self.values.len() as u32).to_le_bytes());
        for v in &self.values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Decode the bands. Bands beyond the file's 24 are left unused.
    pub fn bands(&self) -> Result<[BandParams; NUM_BANDS], String> {
        let mut bands = [BandParams::default(); NUM_BANDS];
        for (i, rec) in self
            .values
            .as_chunks::<PER_BAND>()
            .0
            .iter()
            .take(FILE_BANDS)
            .enumerate()
        {
            let n = i + 1;
            if rec[F_USED] < 0.5 {
                continue;
            }
            if rec[F_PLACE] != PLACE_STEREO {
                return Err(format!(
                    "band {n}: stereo placement code {} is not supported (only Stereo)",
                    rec[F_PLACE]
                ));
            }
            if !rec[F_FREQ].is_finite() || !rec[F_GAIN].is_finite() || !rec[F_Q].is_finite() {
                return Err(format!("band {n}: non-finite value"));
            }
            bands[i] = BandParams {
                used: true,
                enabled: rec[F_ENABLED] >= 0.5,
                shape: lookup(&SHAPES, rec[F_SHAPE], "shape", n)?,
                freq_hz: 2f64.powf(f64::from(rec[F_FREQ])),
                gain_db: f64::from(rec[F_GAIN]),
                q: norm_to_q(rec[F_Q]),
                slope: lookup(&SLOPES, rec[F_SLOPE], "slope", n)?,
                placement: Placement::Stereo,
            };
        }
        Ok(bands)
    }

    /// Overwrite the band section with `bands`, keeping every value this
    /// tool does not understand as it is in `self` (the template).
    pub fn set_bands(&mut self, bands: &[BandParams; NUM_BANDS]) -> Result<(), String> {
        if let Some(i) = bands.iter().skip(FILE_BANDS).position(|b| b.used) {
            return Err(format!(
                "band {} is in use, but this file format only has {FILE_BANDS} bands",
                FILE_BANDS + i + 1
            ));
        }
        // Dynamics/speakers/solo of a band the template had in use must not
        // leak into our bands: take those fields from an unused template band.
        let neutral: Option<Vec<f32>> = self
            .values
            .as_chunks::<PER_BAND>()
            .0
            .iter()
            .take(FILE_BANDS)
            .find(|r| r[F_USED] < 0.5)
            .map(|r| r.to_vec());
        for (i, band) in bands.iter().take(FILE_BANDS).enumerate() {
            let rec = &mut self.values[i * PER_BAND..(i + 1) * PER_BAND];
            if let Some(n) = &neutral {
                rec.copy_from_slice(n);
            }
            if !band.used {
                rec[F_USED] = 0.0;
                rec[F_ENABLED] = 0.0;
                continue;
            }
            if band.placement != Placement::Stereo {
                return Err(format!(
                    "band {}: only Stereo placement can be exported",
                    i + 1
                ));
            }
            rec[F_USED] = 1.0;
            rec[F_ENABLED] = if band.enabled { 1.0 } else { 0.0 };
            rec[F_FREQ] = band.freq_hz.log2() as f32;
            rec[F_GAIN] = band.gain_db as f32;
            rec[F_Q] = q_to_norm(band.q);
            rec[F_SHAPE] = index_of(&SHAPES, &band.shape);
            rec[F_SLOPE] = index_of(&SLOPES, &band.slope);
            rec[F_PLACE] = PLACE_STEREO;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template shaped like a real export: band 1 in use with odd
    /// dynamics values, the rest unused defaults, 46 global values.
    fn template() -> FfpFile {
        let unused = [
            0.0, 0.0, 9.965_784, 0.0, 0.0, 1.0, 1.0, 0.5, 0.0, 1.0, 2.0, 1.0, 0.0,
        ];
        let mut values = Vec::new();
        values.extend_from_slice(&[
            1.0, 1.0, 7.3663, 19.35, -3.0, 0.0, 0.25, 0.36413, 3.0, 1.0, 2.0, 1.0, 1.0,
        ]);
        for _ in 1..FILE_BANDS {
            values.extend_from_slice(&unused);
        }
        values.extend((0..46).map(|i| i as f32 * 0.5));
        FfpFile { version: 4, values }
    }

    #[test]
    fn bytes_round_trip_and_size() {
        let t = template();
        let bytes = t.to_bytes();
        assert_eq!(bytes.len(), 12 + 358 * 4);
        assert_eq!(&bytes[..4], b"FQ3p");
        assert_eq!(FfpFile::parse(&bytes).unwrap(), t);
    }

    #[test]
    fn rejects_malformed_input() {
        let good = template().to_bytes();
        assert!(FfpFile::parse(b"").is_err());
        assert!(FfpFile::parse(b"RIFFxxxxxxxxxxxx").is_err());
        assert!(FfpFile::parse(&good[..good.len() - 1]).is_err());
        let mut huge = good.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(FfpFile::parse(&huge).is_err());
        let mut few = good[..12 + 40].to_vec();
        few[8..12].copy_from_slice(&10u32.to_le_bytes());
        assert!(FfpFile::parse(&few).is_err());
    }

    #[test]
    fn decodes_the_documented_encodings() {
        let b = template().bands().unwrap();
        assert!(b[0].used && b[0].enabled);
        assert_eq!(b[0].shape, Shape::HighShelf);
        assert_eq!(b[0].slope, Slope::Db12);
        assert!((b[0].freq_hz - 165.0).abs() < 0.1);
        assert!((b[0].gain_db - 19.35).abs() < 1e-5);
        assert!((b[0].q - 0.3669).abs() < 1e-3);
        assert!(b[1..].iter().all(|x| !x.used));
        // Normalised 0.5 is Q 1.
        assert!((norm_to_q(0.5) - 1.0).abs() < 1e-12);
        assert!((f64::from(q_to_norm(1.0)) - 0.5).abs() < 1e-7);
    }

    #[test]
    fn export_round_trips_and_keeps_unknown_fields() {
        let mut bands = [BandParams::default(); NUM_BANDS];
        bands[0] = BandParams {
            used: true,
            enabled: true,
            shape: Shape::Bell,
            freq_hz: 15484.0,
            gain_db: 15.69,
            q: 7.23,
            slope: Slope::Db24,
            placement: Placement::Stereo,
        };
        bands[10] = BandParams {
            used: true,
            enabled: false,
            shape: Shape::HighCut,
            freq_hz: 4657.0,
            gain_db: 0.0,
            q: 1.0,
            slope: Slope::Db6,
            placement: Placement::Stereo,
        };
        bands[23] = BandParams {
            used: true,
            enabled: true,
            shape: Shape::LowCut,
            freq_hz: 28.6,
            gain_db: 0.0,
            q: 0.68,
            slope: Slope::Db96,
            placement: Placement::Stereo,
        };
        let t = template();
        let mut f = t.clone();
        f.set_bands(&bands).unwrap();

        // Globals untouched, value count unchanged.
        assert_eq!(f.values.len(), t.values.len());
        assert_eq!(f.values[312..], t.values[312..]);
        // Band 1 was in use in the template with odd dynamics/solo values:
        // those must be replaced by the neutral ones.
        assert_eq!(f.values[4..7], [0.0, 1.0, 1.0]);
        assert_eq!(f.values[11..13], [1.0, 0.0]);
        // Raw codes.
        assert_eq!(f.values[8], 0.0); // bell
        assert_eq!(f.values[9], 3.0); // 24 dB/oct
        let r11 = &f.values[10 * 13..11 * 13];
        assert_eq!((r11[0], r11[1], r11[8], r11[9]), (1.0, 0.0, 4.0, 0.0));
        let r24 = &f.values[23 * 13..24 * 13];
        assert_eq!((r24[8], r24[9]), (2.0, 8.0));

        let back = FfpFile::parse(&f.to_bytes()).unwrap().bands().unwrap();
        for (a, b) in bands.iter().zip(back.iter()) {
            assert_eq!(a.used, b.used);
            if !a.used {
                continue;
            }
            assert_eq!(
                (a.enabled, a.shape, a.slope, a.placement),
                (b.enabled, b.shape, b.slope, b.placement)
            );
            assert!((a.freq_hz / b.freq_hz - 1.0).abs() < 1e-6);
            assert!((a.gain_db - b.gain_db).abs() < 1e-5);
            assert!((a.q / b.q - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn export_refuses_what_the_format_cannot_hold() {
        let mut bands = [BandParams::default(); NUM_BANDS];
        bands[24].used = true;
        assert!(template()
            .set_bands(&bands)
            .unwrap_err()
            .contains("band 25"));

        let mut bands = [BandParams::default(); NUM_BANDS];
        bands[2].used = true;
        bands[2].placement = Placement::Mid;
        assert!(template().set_bands(&bands).unwrap_err().contains("band 3"));
    }

    #[test]
    fn import_refuses_unknown_codes() {
        let mut t = template();
        t.values[8] = 9.0;
        assert!(t.bands().unwrap_err().contains("shape"));
        let mut t = template();
        t.values[9] = 9.0; // e.g. a brickwall slope
        assert!(t.bands().unwrap_err().contains("slope"));
        let mut t = template();
        t.values[10] = 3.0;
        assert!(t.bands().unwrap_err().contains("placement"));
    }
}
