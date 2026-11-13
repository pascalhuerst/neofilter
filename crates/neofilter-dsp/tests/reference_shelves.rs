//! Shelf and tilt-shelf bands versus curves measured on the reference EQ
//! (black-box impulse responses, `fixtures/reference_shelves.json`).
mod common;
use common::*;
use neofilter_dsp::response::band_response_db;
use neofilter_dsp::Shape;

/// Reference points below this level are ignored (measurement noise floor).
const FLOOR_DB: f64 = -90.0;

struct CaseResult {
    desc: String,
    shape: Shape,
    default_q_12: bool,
    /// worst deviation over the whole fixture frequency range
    worst_all: f64,
    /// worst deviation for frequencies up to 0.1 fs
    worst_low: f64,
}

fn run() -> Vec<CaseResult> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/reference_shelves.json"
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut out = Vec::new();
    for c in json["cases"].as_array().unwrap() {
        let num = |k: &str| c[k].as_f64().unwrap();
        let shape = match c["shape"].as_str().unwrap() {
            "high_shelf" => Shape::HighShelf,
            "low_shelf" => Shape::LowShelf,
            "tilt_shelf" => Shape::TiltShelf,
            s => panic!("unknown shape {s}"),
        };
        let fs = num("sample_rate");
        let p = band(
            shape,
            num("freq_hz"),
            num("gain_db"),
            num("q"),
            slope_from_db(num("slope") as u32),
        );
        let freqs: Vec<f64> = c["freqs_hz"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let want: Vec<f64> = c["db"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let mut got = vec![0.0; freqs.len()];
        band_response_db(&p, 1.0, fs, &freqs, &mut got);
        let (mut worst_all, mut worst_low) = (0.0f64, 0.0f64);
        for ((g, w), f) in got.iter().zip(&want).zip(&freqs) {
            if *w >= FLOOR_DB {
                let e = (g - w).abs();
                worst_all = worst_all.max(e);
                if *f <= 0.1 * fs {
                    worst_low = worst_low.max(e);
                }
            }
        }
        out.push(CaseResult {
            desc: format!(
                "{} fs{fs} f{} g{} q{} {}dB/oct",
                c["shape"].as_str().unwrap(),
                num("freq_hz"),
                num("gain_db"),
                num("q"),
                num("slope")
            ),
            shape,
            default_q_12: num("q") == 1.0 && num("slope") == 12.0,
            worst_all,
            worst_low,
        });
    }
    out
}

#[test]
fn matches_reference_shelves() {
    let res = run();
    assert_eq!(res.len(), 58);
    let worst = |f: &dyn Fn(&CaseResult) -> bool, g: &dyn Fn(&CaseResult) -> f64| {
        res.iter()
            .filter(|r| f(r))
            .map(|r| (g(r), r.desc.clone()))
            .fold((0.0, String::new()), |a, b| if b.0 > a.0 { b } else { a })
    };
    let is_default = |r: &CaseResult| r.default_q_12 && r.shape != Shape::TiltShelf;
    let d = worst(&is_default, &|r| r.worst_all);
    let o = worst(&|r| !is_default(r), &|r| r.worst_low);
    println!(
        "default-Q 12 dB/oct shelves: worst {:.6} dB [{}]; all others (<= 0.1 fs): worst {:.5} dB [{}]",
        d.0, d.1, o.0, o.1
    );
    for r in &res {
        if is_default(r) {
            // Reference rule (`refmatch::design_shelf_section`): exact over
            // the whole measured range, including near Nyquist.
            assert!(r.worst_all <= 0.001, "{}: {} dB", r.desc, r.worst_all);
        } else {
            // Resonance law, 24 dB/oct and tilt: the analog prototype is
            // exact, but the digital design near Nyquist is the analog-matched
            // one (the reference's rule there is unmeasured), so only
            // frequencies up to 0.1 fs are judged. Tilt at 12 dB/oct, Q 1
            // uses the shelf rule but was measured at 96 kHz / 1-2 kHz only.
            assert!(r.worst_low <= 0.01, "{}: {} dB", r.desc, r.worst_low);
        }
    }
}
