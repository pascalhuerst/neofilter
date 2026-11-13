//! Single bands versus curves measured on the reference EQ (black-box impulse
//! responses): Bell (all slopes) and High Cut (even orders).
mod common;
use common::*;
use neofilter_dsp::response::band_response_db;
use neofilter_dsp::Shape;

/// Reference points below this level are ignored (measurement noise floor).
const FLOOR_DB: f64 = -90.0;

struct CaseResult {
    desc: String,
    shape: Shape,
    worst: f64,
}

fn run() -> Vec<CaseResult> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/reference_curves.json"
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut out = Vec::new();
    for c in json["cases"].as_array().unwrap() {
        let num = |k: &str| c[k].as_f64().unwrap();
        let shape = match c["shape"].as_str().unwrap() {
            "bell" => Shape::Bell,
            "high_cut" => Shape::HighCut,
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
        let mut worst = 0.0f64;
        for (g, w) in got.iter().zip(&want) {
            if *w >= FLOOR_DB {
                worst = worst.max((g - w).abs());
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
            worst,
        });
    }
    out
}

#[test]
fn matches_reference_curves() {
    let res = run();
    assert_eq!(res.len(), 198);
    let mut all: Vec<f64> = res.iter().map(|r| r.worst).collect();
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = all[all.len() / 2];
    let worst = res
        .iter()
        .max_by(|a, b| a.worst.partial_cmp(&b.worst).unwrap())
        .unwrap();
    let worst_bell = res
        .iter()
        .filter(|r| r.shape == Shape::Bell)
        .max_by(|a, b| a.worst.partial_cmp(&b.worst).unwrap())
        .unwrap();
    println!(
        "cases {}, median {median:.6} dB, worst {:.5} dB [{}], worst bell {:.5} dB [{}]",
        res.len(),
        worst.worst,
        worst.desc,
        worst_bell.worst,
        worst_bell.desc
    );
    for r in &res {
        assert!(r.worst <= 0.02, "{}: {} dB", r.desc, r.worst);
        if r.shape == Shape::Bell {
            assert!(r.worst <= 0.01, "{}: {} dB", r.desc, r.worst);
        }
    }
    assert!(median < 0.0005, "median {median}");
}

/// The reference rule must apply to every section of every fixture case: the
/// matched design must never be needed as a fallback for them.
#[test]
fn reference_rule_applies_to_every_fixture_section() {
    use neofilter_dsp::analog::AnalogBand;
    use neofilter_dsp::refmatch::{design_bell_section, design_highcut_section};
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/reference_curves.json"
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut sections = 0;
    for c in json["cases"].as_array().unwrap() {
        let num = |k: &str| c[k].as_f64().unwrap();
        let bell = c["shape"] == "bell";
        let shape = if bell { Shape::Bell } else { Shape::HighCut };
        let (fs, q) = (num("sample_rate"), num("q"));
        let a = AnalogBand::design(
            shape,
            num("freq_hz"),
            num("gain_db"),
            q,
            slope_from_db(num("slope") as u32),
        );
        for s in a.sections() {
            let r = if bell {
                design_bell_section(s, q, fs)
            } else {
                design_highcut_section(s, fs)
            };
            assert!(r.is_some(), "{} {} Hz", c["shape"], num("freq_hz"));
            sections += 1;
        }
    }
    println!("{sections} fixture sections, no fallback to the matched design");
}

/// The analog-tracking guard of the Bell rule is a safety net only: on every
/// fixture section the rule alone gives the same coefficients.
#[test]
fn guard_does_not_trigger_on_fixtures() {
    use neofilter_dsp::analog::AnalogBand;
    use neofilter_dsp::refmatch::{design_bell_section, design_bell_section_rule_only};
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/reference_curves.json"
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let (mut n, mut triggered) = (0, 0);
    for c in json["cases"].as_array().unwrap() {
        if c["shape"] != "bell" {
            continue;
        }
        let num = |k: &str| c[k].as_f64().unwrap();
        let (fs, q) = (num("sample_rate"), num("q"));
        let a = AnalogBand::design(
            Shape::Bell,
            num("freq_hz"),
            num("gain_db"),
            q,
            slope_from_db(num("slope") as u32),
        );
        for s in a.sections() {
            n += 1;
            triggered +=
                (design_bell_section(s, q, fs) != design_bell_section_rule_only(s, q, fs)) as usize;
        }
    }
    println!("{n} bell fixture sections, guard triggered {triggered} times");
    assert_eq!(triggered, 0);
}
