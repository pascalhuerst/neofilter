//! Bell and High Cut designs (reference rule with matched fallback) never
//! produce NaN or unstable sections, and stay cheap.
mod common;
use common::*;
use neofilter_dsp::band::BandDesign;
use neofilter_dsp::response::band_response_db;
use neofilter_dsp::{Shape, Slope};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]
    #[test]
    fn bell_and_high_cut_are_finite_and_stable(
        hc in any::<bool>(),
        fs in prop::sample::select(vec![22050.0, 32000.0, 44100.0, 48000.0, 88200.0, 96000.0, 176400.0, 192000.0, 384000.0]),
        f in 10.0f64..30000.0,
        g in -30.0f64..30.0,
        q in 0.025f64..40.0,
        si in 0usize..9,
    ) {
        let shape = if hc { Shape::HighCut } else { Shape::Bell };
        let p = band(shape, f, g, q, Slope::ALL[si]);
        let d = BandDesign::design(&p, 1.0, fs);
        for c in d.sections() {
            prop_assert!(c.is_finite() && c.is_stable(), "{c:?}");
        }
        let freqs = grid(10.0, 0.5 * fs, 64);
        let mut out = vec![0.0; freqs.len()];
        band_response_db(&p, 1.0, fs, &freqs, &mut out);
        prop_assert!(out.iter().all(|v| v.is_finite()));
    }
}

/// Design cost of a 96 dB/oct bell (8 sections), release builds only matter;
/// the bound is generous so that debug builds pass too.
#[test]
fn design_cost_is_small() {
    let n = 2000;
    let mut acc = 0usize;
    let t = std::time::Instant::now();
    for i in 0..n {
        let p = band(
            Shape::Bell,
            200.0 + 7.0 * i as f64,
            9.0,
            1.0 + (i % 7) as f64,
            Slope::Db96,
        );
        acc += BandDesign::design(&p, 1.0, 48000.0).len();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
    println!("96 dB/oct bell design: {us:.1} us per band ({acc})");
    assert!(us < 2000.0);
}

/// Counts (and bounds) the Bell sections for which the analog-tracking guard changes the
/// result of the rule alone (guard triggered): over a dense pseudo-random
/// sample of the property-test ranges.
#[test]
fn guard_trigger_rate_in_the_proptest_ranges() {
    use neofilter_dsp::analog::AnalogBand;
    use neofilter_dsp::refmatch::{design_bell_section, design_bell_section_rule_only};
    let rates = [
        22050.0, 32000.0, 44100.0, 48000.0, 88200.0, 96000.0, 176400.0, 192000.0, 384000.0,
    ];
    let mut rng = Noise(0x9E3779B97F4A7C15);
    let u = |r: &mut Noise| 0.5 * (r.next() + 1.0);
    let (mut sections, mut triggered, mut fell_back, mut high_q) = (0usize, 0usize, 0usize, 0usize);
    for _ in 0..40000 {
        let fs = rates[(u(&mut rng) * rates.len() as f64) as usize % rates.len()];
        let f = 10.0 + u(&mut rng) * 29990.0;
        let g = -30.0 + 60.0 * u(&mut rng);
        let q = 0.025 + u(&mut rng) * 39.975;
        let slope = Slope::ALL[(u(&mut rng) * 9.0) as usize % 9];
        let a = AnalogBand::design(Shape::Bell, f.min(0.49 * fs), g, q, slope);
        for s in a.sections() {
            sections += 1;
            let guarded = design_bell_section(s, q, fs);
            let alone = design_bell_section_rule_only(s, q, fs);
            if guarded != alone {
                triggered += 1;
                fell_back += guarded.is_none() as usize;
                high_q += (q >= 3.0) as usize;
            }
        }
    }
    println!(
        "{sections} bell sections, guard triggered {triggered} times ({fell_back} fell back to the \
         matched design, {high_q} with Q >= 3)"
    );
    // Rare (high-Q corners, mostly far from the audio-typical range) but real:
    // the guard stays in place. The bound documents today's rate.
    assert!(triggered * 100 < sections, "{triggered} of {sections}");
}
