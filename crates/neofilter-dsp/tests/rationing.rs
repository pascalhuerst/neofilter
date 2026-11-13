//! After any sequence of parameter changes and enough processing, the engine
//! must realise exactly the filters of the final target parameters (the
//! redesign rationing must not starve or leave stale coefficients).
mod common;
use common::*;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, EqEngine, Placement, Shape, Slope};

fn random_band(n: &mut Noise, shape_pool: &[Shape]) -> BandParams {
    let u = |n: &mut Noise| 0.5 * (n.next() + 1.0);
    BandParams {
        used: u(n) > 0.1,
        enabled: u(n) > 0.1,
        shape: shape_pool[(u(n) * shape_pool.len() as f64) as usize % shape_pool.len()],
        freq_hz: 20.0 * 800.0f64.powf(u(n)),
        gain_db: -20.0 + 40.0 * u(n),
        q: 0.3 * 60.0f64.powf(u(n)),
        slope: Slope::ALL[(u(n) * 9.0) as usize % 9],
        placement: Placement::Stereo,
    }
}

/// Impulse response of the engine as it is now (after letting any tail die).
fn impulse_response(e: &mut EqEngine, len: usize) -> Vec<f64> {
    let mut z = vec![0.0; 48000];
    e.process_mono(&mut z);
    let mut x = vec![0.0; len];
    x[0] = 1.0;
    e.process_mono(&mut x);
    x
}

#[test]
fn engine_converges_to_a_fresh_design_after_bulk_changes() {
    let pool: Vec<Shape> = Shape::ALL.to_vec();
    for fs in [44100.0, 96000.0] {
        let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
        let mut e = EqEngine::new(fs);
        let mut target = [BandParams::default(); NUM_BANDS];
        for round in 0..6 {
            for b in target.iter_mut() {
                *b = random_band(&mut noise, &pool);
            }
            // preset load while audio runs; sometimes the next load arrives
            // before the previous one has settled
            let blocks = if round % 2 == 0 { 3 } else { 400 };
            for _ in 0..blocks {
                for (i, b) in target.iter().enumerate() {
                    e.set_band(i, b);
                }
                let mut x = sine(256, 440.0, fs, 0.3);
                e.process_mono(&mut x);
            }
        }
        for (i, b) in target.iter().enumerate() {
            e.set_band(i, b);
        }
        // settle: smoothing + fades + any postponed redesigns
        let mut x = vec![0.0; fs as usize * 3];
        e.process_mono(&mut x);
        let got = impulse_response(&mut e, 4096);

        let mut fresh = EqEngine::new(fs);
        for (i, b) in target.iter().enumerate() {
            fresh.set_band(i, b);
        }
        let want = impulse_response(&mut fresh, 4096);
        let err = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(
            err < 1e-9,
            "fs {fs}: settled engine differs from a fresh design by {err}"
        );
    }
}

fn busy(fs: f64) -> EqEngine {
    let mut noise = Noise(7);
    let mut e = EqEngine::new(fs);
    for i in 0..NUM_BANDS {
        let mut b = random_band(&mut noise, &Shape::ALL);
        b.used = true;
        b.enabled = true;
        b.gain_db = 18.0;
        e.set_band(i, &b);
    }
    e
}

#[test]
fn non_finite_input_does_not_poison_the_filters() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let fs = 48000.0;
        let mut e = busy(fs);
        let mut clean = busy(fs);
        let mut noise = Noise(11);
        let mut x = vec![0.0; 48000];
        noise.fill(&mut x);
        for v in &mut x {
            *v *= 0.1;
        }
        let mut y = x.clone();
        y[1000] = bad;
        y[1001] = bad;
        e.process_mono(&mut y);
        let mut z = x.clone();
        clean.process_mono(&mut z);
        // the bad samples themselves may produce garbage for a short time, but
        // after a quarter of a second of good input the output is finite again
        // and the same as the engine that never saw them
        let tail_ok = y[1000 + 12000..].iter().all(|v| v.is_finite());
        assert!(tail_ok, "{bad}: output not finite after recovery");
        let err = y[1000 + 24000..]
            .iter()
            .zip(&z[1000 + 24000..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(err < 1e-6, "{bad}: engine did not recover, error {err}");
    }
}

#[test]
fn unusual_sample_rates_and_extreme_settings_are_stable() {
    use neofilter_dsp::band::{BandDesign, BandFilter};
    for fs in [22050.0, 32000.0, 384000.0] {
        for shape in Shape::ALL {
            for slope in [Slope::Db6, Slope::Db24, Slope::Db96] {
                for f in [10.0, 55.0, 3000.0, 22000.0, 30000.0] {
                    for q in [0.025, 1.0, 40.0] {
                        for g in [-30.0, 30.0] {
                            // gain scale 200 %: +-60 dB effective
                            let p = band(shape, f, g, q, slope);
                            let d = BandDesign::design(&p, 2.0, fs);
                            for c in d.sections() {
                                assert!(
                                    c.is_finite() && c.is_stable(),
                                    "{shape:?} {slope:?} f={f} q={q} g={g} fs={fs}: {c:?}"
                                );
                            }
                            let mut filt = BandFilter::new();
                            filt.redesign(&p, 2.0, fs);
                            let mut buf = vec![0.0; 4000];
                            Noise(5).fill(&mut buf);
                            filt.process(0, &mut buf);
                            assert!(
                                buf.iter().all(|v| v.is_finite() && v.abs() < 1e7),
                                "{shape:?} {slope:?} f={f} q={q} g={g} fs={fs}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Worst case for the redesign rationing: every one of the NUM_BANDS bands is
/// moved at once and every band is a 96 dB/oct shape (up to 16 sections each),
/// i.e. several times the 16-section budget per 32-sample interval. The engine
/// must still land on the fresh design promptly (not just "eventually").
#[test]
fn full_band_count_at_96_db_converges_within_one_second() {
    let mut noise = Noise(0xdead_beef);
    for fs in [44100.0, 96000.0] {
        let mut mk = |gain: f64| -> [BandParams; NUM_BANDS] {
            std::array::from_fn(|i| {
                let u = 0.5 * (noise.next() + 1.0);
                BandParams {
                    used: true,
                    enabled: true,
                    shape: [Shape::Bell, Shape::LowCut, Shape::HighCut, Shape::HighShelf][i % 4],
                    freq_hz: 40.0 * 400.0f64.powf(u),
                    gain_db: gain,
                    q: 0.7 + 2.0 * u,
                    slope: Slope::Db96,
                    placement: Placement::Stereo,
                }
            })
        };
        let first = mk(3.0);
        let second = mk(-4.0);
        let mut e = EqEngine::new(fs);
        for (i, b) in first.iter().enumerate() {
            e.set_band(i, b);
        }
        let mut x = vec![0.0; 4800];
        e.process_mono(&mut x);
        // preset load while audio runs
        for (i, b) in second.iter().enumerate() {
            e.set_band(i, b);
        }
        let mut x = vec![0.0; fs as usize];
        e.process_mono(&mut x);
        let got = impulse_response(&mut e, 4096);
        let mut fresh = EqEngine::new(fs);
        for (i, b) in second.iter().enumerate() {
            fresh.set_band(i, b);
        }
        let want = impulse_response(&mut fresh, 4096);
        let err = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        println!("fs {fs}: {NUM_BANDS} x 96 dB/oct, 1 s after the load: error {err:e}");
        assert!(err < 1e-9, "fs {fs}: not converged after 1 s: {err}");
    }
}
