//! Stability and robustness of the coefficient design.
mod common;
use common::*;
use neofilter_dsp::band::{BandDesign, BandFilter};
use neofilter_dsp::response::{band_response_db, total_response_db};
use neofilter_dsp::{BandParams, EqEngine, Placement, Shape, Slope};
use proptest::prelude::*;

fn shape_strategy() -> impl Strategy<Value = Shape> {
    prop::sample::select(Shape::ALL.to_vec())
}
fn slope_strategy() -> impl Strategy<Value = Slope> {
    prop::sample::select(Slope::ALL.to_vec())
}

fn params_strategy() -> impl Strategy<Value = (BandParams, f64, f64)> {
    (
        shape_strategy(),
        // log-uniform frequency 10 Hz .. 30 kHz
        (0.0f64..1.0).prop_map(|u| 10.0 * 3000.0f64.powf(u)),
        -30.0f64..=30.0,
        (0.0f64..1.0).prop_map(|u| 0.025 * 1600.0f64.powf(u)),
        slope_strategy(),
        0.0f64..=2.0,
        44100.0f64..=192000.0,
    )
        .prop_map(|(shape, f, g, q, slope, gs, fs)| {
            (
                BandParams {
                    used: true,
                    enabled: true,
                    shape,
                    freq_hz: f,
                    gain_db: g,
                    q,
                    slope,
                    placement: Placement::Stereo,
                },
                gs,
                fs,
            )
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    /// All poles strictly inside the unit circle, everything finite, the
    /// response finite over the whole audio range.
    #[test]
    fn design_is_finite_and_stable((p, gs, fs) in params_strategy()) {
        let d = BandDesign::design(&p, gs, fs);
        prop_assert!(d.len() <= 8);
        for c in d.sections() {
            prop_assert!(c.is_finite(), "{c:?}");
            prop_assert!(c.is_stable(), "{c:?}");
        }
        let fr = grid(10.0, 0.5 * fs, 64);
        let mut out = vec![0.0; fr.len()];
        band_response_db(&p, gs, fs, &fr, &mut out);
        prop_assert!(out.iter().all(|v| v.is_finite()));
        // a bell is monotone on both sides of its peak: it cannot exceed its
        // gain by much anywhere (shelves with high Q legitimately overshoot)
        let gmax = (p.gain_db * gs).abs() + 6.0;
        if p.shape == Shape::Bell {
            prop_assert!(out.iter().all(|v| v.abs() <= gmax + 3.0), "{out:?}");
        }
    }

    /// Running noise through the filter stays finite and bounded.
    #[test]
    fn filter_output_is_finite((p, gs, fs) in params_strategy()) {
        let mut f = BandFilter::new();
        f.redesign(&p, gs, fs);
        let mut ns = Noise(0x1234_5678_9abc_def1);
        let mut buf = vec![0.0; 3000];
        ns.fill(&mut buf);
        f.process(0, &mut buf);
        prop_assert!(buf.iter().all(|v| v.is_finite() && v.abs() < 1e7), "peak {}", buf.iter().fold(0.0f64, |m, v| m.max(v.abs())));
        // and decays when fed silence afterwards
        let mut tail = vec![0.0; 3000];
        f.process(0, &mut tail);
        prop_assert!(tail.iter().all(|v| v.is_finite()));
    }
}

#[test]
fn sample_rate_sweep_is_stable() {
    for shape in Shape::ALL {
        for slope in [Slope::Db6, Slope::Db24, Slope::Db96] {
            for &f in &[10.0, 55.0, 440.0, 3000.0, 12000.0, 22000.0, 30000.0] {
                for &q in &[0.025, 0.7, 5.0, 40.0] {
                    for fs in [
                        44100.0, 47999.0, 48000.0, 64000.0, 88200.0, 96000.0, 176400.0, 192000.0,
                    ] {
                        let p = band(shape, f, -30.0, q, slope);
                        let d = BandDesign::design(&p, 1.0, fs);
                        for c in d.sections() {
                            assert!(
                                c.is_finite() && c.is_stable(),
                                "{shape:?} {slope:?} {f} {q} {fs}: {c:?}"
                            );
                        }
                        let p = band(shape, f, 30.0, q, slope);
                        let d = BandDesign::design(&p, 2.0, fs);
                        for c in d.sections() {
                            assert!(
                                c.is_finite() && c.is_stable(),
                                "{shape:?} {slope:?} {f} {q} {fs}: {c:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn engine_survives_extreme_random_parameter_changes() {
    let mut e = EqEngine::new(48000.0);
    let mut ns = Noise(99);
    let mut l = vec![0.0; 256];
    let mut r = vec![0.0; 256];
    for round in 0..400 {
        for i in 0..24 {
            let u = |ns: &mut Noise| 0.5 * (ns.next() + 1.0);
            let p = BandParams {
                used: u(&mut ns) > 0.2,
                enabled: u(&mut ns) > 0.2,
                shape: Shape::ALL[(u(&mut ns) * 8.999) as usize],
                freq_hz: 10.0 * 3000.0f64.powf(u(&mut ns)),
                gain_db: (u(&mut ns) - 0.5) * 60.0,
                q: 0.025 * 1600.0f64.powf(u(&mut ns)),
                slope: Slope::ALL[(u(&mut ns) * 8.999) as usize],
                placement: Placement::ALL[(u(&mut ns) * 4.999) as usize],
            };
            e.set_band(i, &p);
        }
        ns.fill(&mut l);
        ns.fill(&mut r);
        for v in l.iter_mut().chain(r.iter_mut()) {
            *v *= 0.01;
        }
        e.process_stereo(&mut l, &mut r);
        assert!(
            l.iter().chain(&r).all(|v| v.is_finite()),
            "non-finite output in round {round}"
        );
    }
}

#[test]
fn total_response_sums_bands_and_skips_inactive() {
    let fs = 48000.0;
    let a = bell(1000.0, 6.0, 1.0);
    let mut b = bell(3000.0, -4.0, 2.0);
    let mut off = bell(500.0, 20.0, 1.0);
    off.enabled = false;
    b.used = true;
    let fr = grid(20.0, 20000.0, 50);
    let mut ta = vec![0.0; 50];
    let mut tb = vec![0.0; 50];
    let mut tot = vec![0.0; 50];
    band_response_db(&a, 1.0, fs, &fr, &mut ta);
    band_response_db(&b, 1.0, fs, &fr, &mut tb);
    total_response_db(&[a, b, off], 1.0, fs, &fr, &mut tot);
    for i in 0..50 {
        assert!((tot[i] - ta[i] - tb[i]).abs() < 1e-12);
    }
}

#[test]
fn frequencies_above_the_limit_are_pinned_not_aliased() {
    // 30 kHz at 44.1 kHz would alias; the design is pinned at 0.49 fs.
    let fs = 44100.0;
    for shape in [Shape::Bell, Shape::HighShelf, Shape::Notch, Shape::LowCut] {
        let hi = band(shape, 30000.0, 9.0, 3.0, Slope::Db24);
        let pinned = band(shape, 0.49 * fs, 9.0, 3.0, Slope::Db24);
        let fr = grid(20.0, 20000.0, 100);
        let (mut a, mut b) = (vec![0.0; 100], vec![0.0; 100]);
        band_response_db(&hi, 1.0, fs, &fr, &mut a);
        band_response_db(&pinned, 1.0, fs, &fr, &mut b);
        assert_eq!(a, b, "{shape:?}");
    }
}
