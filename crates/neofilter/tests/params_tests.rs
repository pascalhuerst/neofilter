use neofilter::params::*;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, Placement, Shape, Slope};
use nih_plug::prelude::*;

fn expected_ids() -> Vec<String> {
    let mut v: Vec<String> = ["bypass", "out_gain", "gain_scale"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for n in 1..=NUM_BANDS {
        for f in ["used", "on", "shape", "freq", "gain", "q", "slope", "place"] {
            v.push(format!("{f}_{n}"));
        }
    }
    v
}

#[test]
fn param_ids_unique_and_snapshot() {
    let p = NeoFilterParams::default();
    let map = p.param_map();
    let ids: Vec<String> = map.iter().map(|(id, _, _)| id.clone()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "duplicate ids");
    assert_eq!(ids, expected_ids());
    assert_eq!(ids.len(), 3 + 8 * 32);
    // Bands 1..=24 are the original 24-band layout, byte for byte and in the
    // same order (state saved before the 24 -> 32 change must keep loading);
    // bands 25..=32 are appended after them.
    let mut legacy: Vec<String> = ["bypass", "out_gain", "gain_scale"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for n in 1..=24 {
        for f in ["used", "on", "shape", "freq", "gain", "q", "slope", "place"] {
            legacy.push(format!("{f}_{n}"));
        }
    }
    assert_eq!(ids[..legacy.len()], legacy[..], "bands 1..=24 ids changed");
    assert_eq!(ids[legacy.len()], "used_25");
    assert_eq!(ids.last().unwrap(), "place_32");
    // spot checks of the host-visible hashed ids of the original bands
    // (nih-plug: h = h*31 + byte over the id, top bit cleared)
    let hash = |id: &str| {
        id.bytes()
            .fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(u32::from(c)))
            & !(1 << 31)
    };
    assert_eq!(hash("freq_1"), 0x3484_642a, "freq_1 hash");
    let mut hashes: Vec<u32> = ids.iter().map(|i| hash(i)).collect();
    hashes.sort_unstable();
    hashes.dedup();
    assert_eq!(hashes.len(), ids.len(), "host id hash collision");
    // literal spot checks so the scheme itself cannot drift
    assert!(ids.contains(&"freq_1".to_string()));
    assert!(ids.contains(&"place_32".to_string()));
    // groups
    let g = |id: &str| map.iter().find(|(i, _, _)| i == id).unwrap().2.clone();
    assert_eq!(g("freq_1"), "Band 1");
    assert_eq!(g("q_32"), "Band 32");
    assert_eq!(g("bypass"), "");
}

#[test]
fn enum_ids_stable() {
    let p = NeoFilterParams::default();
    let shape = &p.bands[0].shape;
    let ids: Vec<&str> = BandShape::ids().unwrap().to_vec();
    assert_eq!(
        ids,
        [
            "bell",
            "low_shelf",
            "low_cut",
            "high_shelf",
            "high_cut",
            "notch",
            "band_pass",
            "tilt_shelf",
            "flat_tilt"
        ]
    );
    assert_eq!(Param::step_count(shape), Some(8));
    assert_eq!(
        BandSlope::ids().unwrap().to_vec(),
        ["s6", "s12", "s18", "s24", "s30", "s36", "s48", "s72", "s96"]
    );
    assert_eq!(
        BandPlace::ids().unwrap().to_vec(),
        ["stereo", "left", "right", "mid", "side"]
    );
}

#[test]
fn defaults() {
    let p = NeoFilterParams::default();
    assert!(!p.bypass.value());
    assert!(p.bypass.flags().contains(ParamFlags::BYPASS));
    assert_eq!(p.out_gain.value(), 0.0);
    assert_eq!(p.gain_scale.value(), 100.0);
    assert_eq!(p.gain_scale_factor(), 1.0);
    for b in p.band_snapshot() {
        assert_eq!(b, BandParams::default());
    }
    let b = &p.bands[5];
    assert!(!b.used.value() && b.on.value());
    assert_eq!(b.freq.value(), 1000.0);
    assert_eq!(b.gain.value(), 0.0);
    assert_eq!(b.q.value(), 1.0);
    assert_eq!(b.slope.value(), BandSlope::Db12);
    assert_eq!(b.place.value(), BandPlace::Stereo);
    assert_eq!(b.shape.value(), BandShape::Bell);
}

#[test]
fn freq_parser_table() {
    let cases: &[(&str, f32)] = &[
        ("41.1", 41.1),
        ("41.1 Hz", 41.1),
        ("41,1 hz", 41.1),
        ("9.729k", 9729.0),
        ("9.729 kHz", 9729.0),
        ("9729", 9729.0),
        ("9,729 kHz", 9729.0),
        ("  15484  ", 15484.0),
        ("1 K", 1000.0),
        ("15 484", 15484.0),
        ("15 484 Hz", 15484.0),
        ("15\u{a0}484 Hz", 15484.0),
        ("15\u{202f}484", 15484.0),
        ("1 000,5", 1000.5),
        ("1e3", 1000.0),
    ];
    for (s, want) in cases {
        let got = parse_freq(s).unwrap_or_else(|| panic!("{s}"));
        assert!((got - want).abs() <= want * 1e-6, "{s}: {got} vs {want}");
    }
    assert_eq!(parse_freq("abc"), None);
    assert_eq!(parse_freq(""), None);
    assert_eq!(parse_freq("k"), None);
    assert_eq!(parse_freq("Hz"), None);
    assert_eq!(parse_freq("1e999"), None);
    assert_eq!(parse_freq("NaN"), None);
    assert_eq!(parse_freq("inf"), None);
}

#[test]
fn freq_formatter_table() {
    let cases: &[(f32, &str)] = &[
        (41.1, "41.1 Hz"),
        (722.9, "722.9 Hz"),
        (233.0, "233.0 Hz"),
        (1000.0, "1.000 kHz"),
        (9729.0, "9.729 kHz"),
        (15484.0, "15.484 kHz"),
        (30000.0, "30.000 kHz"),
        (10.0, "10.0 Hz"),
    ];
    let p = NeoFilterParams::default();
    for (v, want) in cases {
        assert_eq!(format_freq(*v), *want);
        let n = p.bands[0].freq.preview_normalized(*v);
        assert_eq!(p.bands[0].freq.normalized_value_to_string(n, true), *want);
    }
}

#[test]
fn gain_parser_and_formatter() {
    let p = NeoFilterParams::default();
    let g = &p.bands[0].gain;
    for (s, want) in [
        ("+2.58", 2.58f32),
        ("-1.32 dB", -1.32),
        ("−2,50", -2.5),
        ("15.69dB", 15.69),
        ("0", 0.0),
    ] {
        assert!((parse_db(s).unwrap() - want).abs() < 1e-6, "{s}");
    }
    assert_eq!(parse_db("x"), None);
    assert_eq!(parse_db(""), None);
    assert_eq!(parse_db("dB"), None);
    assert_eq!(parse_db("+"), None);
    // formatting incl. unit, negative zero avoided
    assert_eq!(
        g.normalized_value_to_string(g.preview_normalized(2.58), true),
        "2.58 dB"
    );
    assert_eq!(
        g.normalized_value_to_string(g.preview_normalized(-5.38), true),
        "-5.38 dB"
    );
    assert_eq!(
        g.normalized_value_to_string(g.preview_normalized(-0.001), true),
        "0.00 dB"
    );
    // string -> normalized -> plain
    let n = g.string_to_normalized_value("+2.58").unwrap();
    assert!((g.preview_plain(n) - 2.58).abs() < 1e-4);
    // out gain / gain scale
    assert!((parse_percent("150 %").unwrap() - 150.0).abs() < 1e-6);
    assert!((parse_q("0,68").unwrap() - 0.68).abs() < 1e-6);
    assert_eq!(p.gain_scale.normalized_value_to_string(0.5, true), "100 %");
}

#[test]
fn q_formatter() {
    let p = NeoFilterParams::default();
    let q = &p.bands[0].q;
    assert_eq!(
        q.normalized_value_to_string(q.preview_normalized(0.68), true),
        "0.68"
    );
    assert_eq!(
        q.normalized_value_to_string(q.preview_normalized(10.91), true),
        "10.91"
    );
}

fn assert_round_trip(p: &FloatParam, v: f32) {
    let n = p.preview_normalized(v);
    let back = p.preview_plain(n);
    let tol = (v.abs() * 1e-4).max(1e-4);
    assert!((back - v).abs() <= tol, "{v} -> {n} -> {back}");
}

#[test]
fn normalized_round_trip_edges_and_typical() {
    let p = NeoFilterParams::default();
    let b = &p.bands[0];
    for v in [
        10.0, 28.6, 41.1, 85.7, 722.9, 1000.0, 9729.0, 15484.0, 15959.0, 30000.0,
    ] {
        assert_round_trip(&b.freq, v);
    }
    for v in [0.025, 0.68, 1.0, 1.05, 2.97, 10.91, 14.14, 40.0] {
        assert_round_trip(&b.q, v);
    }
    for v in [-30.0, -5.38, 0.0, 15.69, 30.0] {
        assert_round_trip(&b.gain, v);
    }
}

#[test]
fn enum_mirrors_exhaustive() {
    for s in Shape::ALL {
        assert_eq!(Shape::from(BandShape::from(s)), s);
    }
    for s in Slope::ALL {
        assert_eq!(Slope::from(BandSlope::from(s)), s);
    }
    for s in Placement::ALL {
        assert_eq!(Placement::from(BandPlace::from(s)), s);
    }
    // variant order matches the dsp order (index based host mapping)
    assert_eq!(BandShape::variants().len(), Shape::ALL.len());
    assert_eq!(BandSlope::variants().len(), Slope::ALL.len());
    assert_eq!(BandPlace::variants().len(), Placement::ALL.len());
    for (i, s) in Shape::ALL.iter().enumerate() {
        assert_eq!(BandShape::from_index(i), BandShape::from(*s));
        assert_eq!(BandShape::variants()[i], s.name());
    }
    for (i, s) in Slope::ALL.iter().enumerate() {
        assert_eq!(BandSlope::from_index(i), BandSlope::from(*s));
        assert_eq!(BandSlope::variants()[i], s.name());
    }
    for (i, s) in Placement::ALL.iter().enumerate() {
        assert_eq!(BandPlace::from_index(i), BandPlace::from(*s));
        assert_eq!(BandPlace::variants()[i], s.name());
    }
}
