//! Cached response curves (recomputed only when parameters or width change).

use neofilter_dsp::response::{band_response_db, total_response_db};
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;

use super::scale::{DISPLAY_F_MAX, DISPLAY_F_MIN};

#[derive(Default)]
pub struct CurveCache {
    pub freqs: Vec<f64>,
    pub band_db: Vec<Vec<f64>>,
    pub total_db: Vec<f64>,
    key: Option<([BandParams; NUM_BANDS], f64, f64, usize)>,
}

/// Log-spaced frequencies for `n` evenly spaced columns across the display.
pub fn column_freqs(n: usize, out: &mut Vec<f64>) {
    out.clear();
    let ratio = f64::from(DISPLAY_F_MAX / DISPLAY_F_MIN);
    let d = (n.max(2) - 1) as f64;
    out.extend((0..n).map(|i| f64::from(DISPLAY_F_MIN) * ratio.powf(i as f64 / d)));
}

impl CurveCache {
    /// Recompute if anything changed. `n` is the number of points.
    pub fn update(
        &mut self,
        bands: &[BandParams; NUM_BANDS],
        gain_scale: f64,
        sample_rate: f64,
        n: usize,
    ) {
        let key = (*bands, gain_scale, sample_rate, n);
        if self.key.as_ref() == Some(&key) {
            return;
        }
        if self.freqs.len() != n {
            column_freqs(n, &mut self.freqs);
        }
        self.band_db.resize_with(NUM_BANDS, Vec::new);
        for (b, out) in bands.iter().zip(self.band_db.iter_mut()) {
            out.resize(n, 0.0);
            if b.used && b.enabled {
                band_response_db(b, gain_scale, sample_rate, &self.freqs, out);
            } else {
                out.fill(0.0);
            }
        }
        self.total_db.resize(n, 0.0);
        total_response_db(
            bands,
            gain_scale,
            sample_rate,
            &self.freqs,
            &mut self.total_db,
        );
        self.key = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freqs_span_display_range() {
        let mut v = Vec::new();
        column_freqs(401, &mut v);
        assert_eq!(v.len(), 401);
        assert!((v[0] - 10.0).abs() < 1e-9);
        assert!((v[400] - 30000.0).abs() < 1e-6);
        assert!(v.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn cache_sizes_buffers() {
        let mut c = CurveCache::default();
        let mut b = [BandParams::default(); NUM_BANDS];
        b[0].used = true;
        c.update(&b, 1.0, 48000.0, 300);
        assert_eq!(c.total_db.len(), 300);
        assert_eq!(c.band_db.len(), NUM_BANDS);
        assert!(c.band_db.iter().all(|v| v.len() == 300));
        c.update(&b, 1.0, 48000.0, 250);
        assert_eq!(c.total_db.len(), 250);
        assert_eq!(c.freqs.len(), 250);
    }
}
