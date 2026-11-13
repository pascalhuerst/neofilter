//! Measured impulse response versus the model's digital response.

use crate::model::Model;
use realfft::RealFftPlanner;

pub const F_LO: f64 = 20.0;
pub const F_HI: f64 = 20000.0;
/// Regions are third-octave bands.
const REGION_RATIO: f64 = 1.259_921_049_894_873_2; // 2^(1/3)

pub struct Region {
    pub lo: f64,
    pub hi: f64,
    pub worst_freq: f64,
    pub max_abs_db: f64,
    pub mean_db: f64,
}

pub struct Report {
    pub bins: usize,
    pub max_abs_db: f64,
    pub rms_db: f64,
    pub worst_freq: f64,
    /// Sorted by decreasing `max_abs_db`, at most 10.
    pub regions: Vec<Region>,
}

/// dB magnitude spectrum of `ir` zero-padded to a power of two (>= 4096).
/// Returns (bin spacing in Hz, dB per bin).
pub fn spectrum_db(ir: &[f32], sample_rate: f64) -> (f64, Vec<f64>) {
    let n = ir.len().next_power_of_two().max(4096);
    let mut buf = vec![0.0f64; n];
    for (b, &x) in buf.iter_mut().zip(ir) {
        *b = f64::from(x);
    }
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut out = fft.make_output_vec();
    // lengths always match, so process cannot fail
    let _ = fft.process(&mut buf, &mut out);
    let db = out
        .iter()
        .map(|c| 10.0 * c.norm_sqr().max(1e-40).log10())
        .collect();
    (sample_rate / n as f64, db)
}

pub fn compare(ir: &[f32], sample_rate: f64, model: &Model) -> Result<Report, String> {
    let (bin_hz, meas) = spectrum_db(ir, sample_rate);
    let hi = F_HI.min(0.49 * sample_rate);
    let idx: Vec<usize> = (0..meas.len())
        .filter(|&k| (F_LO..=hi).contains(&(k as f64 * bin_hz)))
        .collect();
    if idx.is_empty() {
        return Err("impulse response too short to resolve 20 Hz - 20 kHz".into());
    }
    let freqs: Vec<f64> = idx.iter().map(|&k| k as f64 * bin_hz).collect();
    let want = model.response_db(sample_rate, &freqs, false);
    let dev: Vec<f64> = idx.iter().zip(&want).map(|(&k, w)| meas[k] - w).collect();

    let mut max_abs = 0.0f64;
    let mut worst_freq = freqs[0];
    let mut sq = 0.0;
    for (f, d) in freqs.iter().zip(&dev) {
        sq += d * d;
        if d.abs() > max_abs {
            max_abs = d.abs();
            worst_freq = *f;
        }
    }

    let mut regions = Vec::new();
    let mut lo = F_LO;
    while lo < hi {
        let top = lo * REGION_RATIO;
        let mut r = Region {
            lo,
            hi: top.min(hi),
            worst_freq: lo,
            max_abs_db: 0.0,
            mean_db: 0.0,
        };
        let mut count = 0usize;
        for (f, d) in freqs.iter().zip(&dev) {
            if *f >= lo && *f < top {
                count += 1;
                r.mean_db += d;
                if d.abs() >= r.max_abs_db {
                    r.max_abs_db = d.abs();
                    r.worst_freq = *f;
                }
            }
        }
        if count > 0 {
            r.mean_db /= count as f64;
            regions.push(r);
        }
        lo = top;
    }
    regions.sort_by(|a, b| b.max_abs_db.total_cmp(&a.max_abs_db));
    regions.truncate(10);

    Ok(Report {
        bins: idx.len(),
        max_abs_db: max_abs,
        rms_db: (sq / idx.len() as f64).sqrt(),
        worst_freq,
        regions,
    })
}
