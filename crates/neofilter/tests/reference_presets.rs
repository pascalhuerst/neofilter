//! Total curves of the factory presets versus the reference EQ (measured).
mod common;
use common::*;
use neofilter_dsp::response::total_response_db;

#[test]
fn factory_presets_match_the_reference_curves() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/reference_presets.json"
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cases = json["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 9);
    for c in cases {
        let id = c["preset"].as_str().unwrap();
        let fs = c["sample_rate"].as_f64().unwrap();
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
        total_response_db(&preset(id), 1.0, fs, &freqs, &mut got);
        let (mut worst, mut at) = (0.0f64, 0.0);
        for ((g, w), f) in got.iter().zip(&want).zip(&freqs) {
            if (40.0..=20000.0).contains(f) && (g - w).abs() > worst {
                worst = (g - w).abs();
                at = *f;
            }
        }
        println!("{id} @ {fs}: worst {worst:.5} dB at {at:.0} Hz");
        assert!(worst <= 0.03, "{id} @ {fs}: {worst} dB at {at} Hz");
    }
}
