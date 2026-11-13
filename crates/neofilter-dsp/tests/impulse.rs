//! The running filter (impulse response through the real f64 TDF-II code)
//! must equal the analytic digital response of its coefficients, including at
//! very low frequencies and high sample rates where the poles sit next to z = 1.
mod common;
use common::*;
use neofilter_dsp::band::BandFilter;
use neofilter_dsp::response::band_response_db;
use neofilter_dsp::{BandParams, EqEngine, Shape, Slope};
use realfft::RealFftPlanner;

/// Max |dB(FFT of impulse response) - dB(analytic)| over ~1500 log-spaced
/// bins from 10 Hz to 0.45 fs, skipping bins where the response is below
/// -90 dB (the IR's own rounding noise floor).
fn ir_deviation(ir: &[f64], p: &BandParams, fs: f64) -> f64 {
    let n = ir.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let mut input = ir.to_vec();
    let mut spec = fft.make_output_vec();
    fft.process(&mut input, &mut spec).unwrap();
    let mut freqs = Vec::new();
    let mut bins = Vec::new();
    let mut k_prev = 0;
    for f in grid(10.0, 0.45 * fs, 1500) {
        let k = (f / fs * n as f64).round() as usize;
        if k > k_prev && k < spec.len() {
            freqs.push(k as f64 * fs / n as f64);
            bins.push(k);
            k_prev = k;
        }
    }
    let mut analytic = vec![0.0; freqs.len()];
    band_response_db(p, 1.0, fs, &freqs, &mut analytic);
    let mut worst: f64 = 0.0;
    for (i, &k) in bins.iter().enumerate() {
        if analytic[i] < -90.0 {
            continue;
        }
        let measured = 20.0 * spec[k].norm().log10();
        worst = worst.max((measured - analytic[i]).abs());
    }
    worst
}

fn impulse_response_of(p: &BandParams, fs: f64, n: usize) -> Vec<f64> {
    let mut f = BandFilter::new();
    f.redesign(p, 1.0, fs);
    let mut ir = vec![0.0; n];
    ir[0] = 1.0;
    f.process(0, &mut ir);
    ir
}

#[test]
fn impulse_response_matches_analytic_digital_response() {
    // (params, fs, fft length, tolerance dB)
    let cases: Vec<(BandParams, f64, usize, f64)> = vec![
        (bell(1000.0, 9.0, 2.0), 48000.0, 1 << 17, 1e-6),
        (
            band(Shape::HighShelf, 8000.0, -12.0, 1.0, Slope::Db24),
            44100.0,
            1 << 17,
            1e-6,
        ),
        (
            band(Shape::LowCut, 30.0, 0.0, 1.0, Slope::Db48),
            48000.0,
            1 << 20,
            1e-6,
        ),
        (
            band(Shape::HighCut, 13607.0, 0.0, 1.0, Slope::Db48),
            44100.0,
            1 << 17,
            1e-6,
        ),
        (
            band(Shape::Notch, 4000.0, 0.0, 4.0, Slope::Db24),
            96000.0,
            1 << 19,
            1e-6,
        ),
        (
            band(Shape::FlatTilt, 1000.0, 12.0, 1.0, Slope::Db12),
            48000.0,
            1 << 20,
            1e-6,
        ),
        // poles next to z = 1: 10 Hz at 192 kHz
        (bell(10.0, 12.0, 1.0), 192000.0, 1 << 21, 1e-5),
        (bell(10.0, -12.0, 10.0), 192000.0, 1 << 21, 1e-4),
        // (a boost at Q 40 rings for ~14 s here; the cut has the same zeros but
        // broad poles, so the FFT length covers the whole decay)
        (bell(10.0, -24.0, 40.0), 192000.0, 1 << 22, 1e-3),
        (bell(10.0, 6.0, 0.025), 192000.0, 1 << 21, 1e-4),
        (
            band(Shape::LowCut, 10.0, 0.0, 1.0, Slope::Db96),
            192000.0,
            1 << 21,
            1e-3,
        ),
        (
            band(Shape::LowCut, 10.0, 0.0, 40.0, Slope::Db12),
            192000.0,
            1 << 22,
            1e-3,
        ),
        (
            band(Shape::LowShelf, 10.0, 18.0, 1.0, Slope::Db12),
            192000.0,
            1 << 21,
            1e-4,
        ),
        (
            band(Shape::HighShelf, 10.0, -18.0, 1.0, Slope::Db12),
            192000.0,
            1 << 21,
            1e-4,
        ),
        (bell(20.0, 12.0, 5.0), 96000.0, 1 << 21, 1e-4),
    ];
    for (p, fs, n, tol) in cases {
        let ir = impulse_response_of(&p, fs, n);
        let dev = ir_deviation(&ir, &p, fs);
        println!(
            "{:?} {:.0} Hz Q{} {}dB/oct @ {fs}: deviation {dev:.2e} dB",
            p.shape,
            p.freq_hz,
            p.q,
            p.slope.db_per_oct()
        );
        assert!(dev <= tol, "{p:?} @ {fs}: {dev} dB > {tol}");
    }
}

#[test]
fn engine_impulse_response_matches_total_response() {
    // Whole engine (smoothing settled): impulse response vs. sum of band curves.
    let fs = 48000.0;
    let bands = [
        bell(60.0, 4.0, 1.4),
        band(Shape::LowShelf, 200.0, -3.0, 0.8, Slope::Db18),
        bell(3200.0, 6.0, 3.0),
        band(Shape::HighCut, 16000.0, 0.0, 1.0, Slope::Db24),
    ];
    let mut e = EqEngine::new(fs);
    for (i, b) in bands.iter().enumerate() {
        e.set_band(i, b);
    }
    let n = 1 << 18;
    let mut ir = vec![0.0; n];
    ir[0] = 1.0;
    e.process_mono(&mut ir);
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let mut spec = fft.make_output_vec();
    fft.process(&mut ir, &mut spec).unwrap();
    let freqs: Vec<f64> = (1..n / 2)
        .step_by(97)
        .map(|k| k as f64 * fs / n as f64)
        .filter(|f| *f > 15.0 && *f < 20000.0)
        .collect();
    let mut total = vec![0.0; freqs.len()];
    neofilter_dsp::response::total_response_db(&bands, 1.0, fs, &freqs, &mut total);
    let mut worst: f64 = 0.0;
    for (i, f) in freqs.iter().enumerate() {
        let k = (f / fs * n as f64).round() as usize;
        worst = worst.max((20.0 * spec[k].norm().log10() - total[i]).abs());
    }
    println!("engine IR vs total_response_db: {worst:.2e} dB");
    assert!(worst < 1e-6, "{worst}");
}
