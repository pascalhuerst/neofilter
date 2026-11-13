use neofilter::params::*;
use neofilter::presets::*;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{Placement, Shape, Slope};
use Shape::{Bell, HighCut, HighShelf, LowCut};

// (band number, shape, freq, gain, q, slope, enabled)
type Row = (usize, Shape, f64, f64, f64, u32, bool);

const A: [Row; 15] = [
    (1, Bell, 41.1, 2.58, 1.00, 12, true),
    (2, HighShelf, 722.9, -1.32, 1.00, 12, true),
    (3, Bell, 98.0, -1.83, 2.73, 24, true),
    (4, Bell, 35.9, 2.54, 2.75, 24, true),
    (5, Bell, 9729.0, 2.46, 3.24, 48, true),
    (6, Bell, 233.0, -2.50, 1.00, 12, true),
    (7, LowCut, 28.6, 0.0, 1.00, 12, true),
    (8, Bell, 177.7, -5.38, 0.68, 12, true),
    (9, Bell, 2093.0, -2.99, 1.25, 48, true),
    (10, Bell, 5074.0, 4.90, 10.91, 24, true),
    (11, HighCut, 4657.0, 0.0, 1.00, 6, false),
    (12, Bell, 392.7, -2.87, 2.97, 12, true),
    (13, Bell, 2035.0, 2.78, 5.00, 48, true),
    (14, Bell, 11787.0, 2.45, 4.59, 48, true),
    (15, Bell, 15484.0, 15.69, 7.23, 24, true),
];

const B: [Row; 9] = [
    (2, HighCut, 13607.0, 0.0, 1.00, 48, true),
    (4, Bell, 4317.0, 10.58, 1.05, 24, true),
    (5, Bell, 85.7, 7.62, 1.77, 24, true),
    (6, Bell, 701.3, -7.36, 2.01, 12, true),
    (8, Bell, 15959.0, 8.51, 14.14, 48, true),
    (9, Bell, 13195.0, -4.62, 10.00, 36, true),
    (10, Bell, 6282.0, -3.54, 10.00, 36, true),
    (11, Bell, 10011.0, 2.64, 5.95, 12, true),
    (12, Bell, 165.7, 2.06, 2.97, 48, true),
];

/// Friend EQ A+B: bands 1..=15 are A's, bands 16..=24 are B's bands 2, 4, 5, 6,
/// 8, 9, 10, 11, 12 in table order. Written out literally (not derived from
/// `A` / `B`) so the mapping itself is pinned down.
const AB: [Row; 24] = [
    (1, Bell, 41.1, 2.58, 1.00, 12, true),
    (2, HighShelf, 722.9, -1.32, 1.00, 12, true),
    (3, Bell, 98.0, -1.83, 2.73, 24, true),
    (4, Bell, 35.9, 2.54, 2.75, 24, true),
    (5, Bell, 9729.0, 2.46, 3.24, 48, true),
    (6, Bell, 233.0, -2.50, 1.00, 12, true),
    (7, LowCut, 28.6, 0.0, 1.00, 12, true),
    (8, Bell, 177.7, -5.38, 0.68, 12, true),
    (9, Bell, 2093.0, -2.99, 1.25, 48, true),
    (10, Bell, 5074.0, 4.90, 10.91, 24, true),
    (11, HighCut, 4657.0, 0.0, 1.00, 6, false),
    (12, Bell, 392.7, -2.87, 2.97, 12, true),
    (13, Bell, 2035.0, 2.78, 5.00, 48, true),
    (14, Bell, 11787.0, 2.45, 4.59, 48, true),
    (15, Bell, 15484.0, 15.69, 7.23, 24, true),
    (16, HighCut, 13607.0, 0.0, 1.00, 48, true),
    (17, Bell, 4317.0, 10.58, 1.05, 24, true),
    (18, Bell, 85.7, 7.62, 1.77, 24, true),
    (19, Bell, 701.3, -7.36, 2.01, 12, true),
    (20, Bell, 15959.0, 8.51, 14.14, 48, true),
    (21, Bell, 13195.0, -4.62, 10.00, 36, true),
    (22, Bell, 6282.0, -3.54, 10.00, 36, true),
    (23, Bell, 10011.0, 2.64, 5.95, 12, true),
    (24, Bell, 165.7, 2.06, 2.97, 48, true),
];

fn check(id: &str, rows: &[Row]) {
    let p = factory_presets().iter().find(|p| p.id == id).unwrap();
    let bp = p.band_params();
    for (i, b) in bp.iter().enumerate() {
        match rows.iter().find(|r| r.0 == i + 1) {
            Some(&(_, shape, f, g, q, s, on)) => {
                assert!(b.used, "{id} band {}", i + 1);
                assert_eq!(b.enabled, on, "{id} band {}", i + 1);
                assert_eq!(b.shape, shape);
                assert_eq!(b.freq_hz, f);
                assert_eq!(b.gain_db, g);
                assert_eq!(b.q, q);
                assert_eq!(b.slope.db_per_oct(), s);
                assert_eq!(b.placement, Placement::Stereo);
            }
            None => assert_eq!(
                *b,
                neofilter_dsp::BandParams::default(),
                "{id} band {}",
                i + 1
            ),
        }
    }
    assert_eq!(p.gain_scale, 100.0);
    assert_eq!(p.out_gain_db, 0.0);
}

#[test]
fn presets_match_plan_tables() {
    let ps = factory_presets();
    assert_eq!(ps.len(), 4);
    assert_eq!((ps[0].id, ps[0].name), ("default", "Default"));
    assert_eq!((ps[1].id, ps[1].name), ("friend-eq-a", "Friend EQ A"));
    assert_eq!((ps[2].id, ps[2].name), ("friend-eq-b", "Friend EQ B"));
    assert_eq!(
        (ps[3].id, ps[3].name),
        ("friend-eq-ab", "Friend EQ A+B (series)")
    );
    check("default", &[]);
    check("friend-eq-ab", &AB);
    check("friend-eq-a", &A);
    check("friend-eq-b", &B);
}

#[test]
fn combined_preset_layout() {
    let bp = factory_presets()[3].band_params();
    assert_eq!(bp.iter().filter(|b| b.used).count(), 24);
    assert_eq!(bp.iter().filter(|b| b.used && b.enabled).count(), 23);
    assert!(bp[10].used && !bp[10].enabled, "A band 11 stays disabled");
    for (i, b) in bp.iter().enumerate().skip(24) {
        assert_eq!(
            *b,
            neofilter_dsp::BandParams::default(),
            "spare band {}",
            i + 1
        );
    }
    // the original presets keep their band positions
    let a = factory_presets()[1].band_params();
    let b = factory_presets()[2].band_params();
    assert_eq!(a[..15], bp[..15]);
    for (k, src) in [1usize, 3, 4, 5, 7, 8, 9, 10, 11].into_iter().enumerate() {
        assert_eq!(bp[15 + k], b[src], "B band {} -> band {}", src + 1, 16 + k);
    }
}

#[test]
fn preset_counts() {
    let ps = factory_presets();
    let a = ps[1].band_params();
    assert_eq!(a.iter().filter(|b| b.used).count(), 15);
    assert_eq!(a.iter().filter(|b| b.used && b.enabled).count(), 14);
    assert!(a[10].used && !a[10].enabled);
    assert_eq!(ps[2].band_params().iter().filter(|b| b.used).count(), 9);
    assert_eq!(ps[0].band_params().iter().filter(|b| b.used).count(), 0);
}

#[test]
fn preset_values_survive_param_ranges() {
    let p = NeoFilterParams::default();
    let b = &p.bands[0];
    use nih_plug::prelude::Param;
    for preset in factory_presets() {
        for band in preset.band_params().iter().filter(|b| b.used) {
            for (param, v) in [
                (&b.freq, band.freq_hz as f32),
                (&b.gain, band.gain_db as f32),
                (&b.q, band.q as f32),
            ] {
                let back = param.preview_plain(param.preview_normalized(v));
                assert!((back - v).abs() <= v.abs() * 1e-4 + 1e-5, "{v} -> {back}");
            }
        }
    }
}

#[test]
fn json_round_trip() {
    for p in factory_presets() {
        let f = p.to_file();
        let json = f.to_json();
        let back = PresetFile::from_json(&json).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.band_params(), p.band_params());
    }
    let json = factory_presets()[1].to_file().to_json();
    assert!(json.contains("\"high_shelf\""));
    assert!(json.contains("\"slope\": 48"));
}

#[test]
fn json_tolerant_and_clamping() {
    let f = PresetFile::from_json(
        r#"{"gain_scale": 500, "out_gain_db": -99,
            "bands":[{"band": 3, "freq_hz": 1e9, "gain_db": 99, "q": 0, "slope": 50},
                     {"band": 32, "shape": "notch", "placement": "mid"}]}"#,
    )
    .unwrap();
    assert_eq!(f.version, 1);
    assert_eq!(f.gain_scale, 200.0);
    assert_eq!(f.out_gain_db, -36.0);
    let bp = f.band_params();
    assert!(bp[2].used && bp[2].enabled);
    assert_eq!(bp[2].freq_hz, 30000.0);
    assert_eq!(bp[2].gain_db, 30.0);
    assert_eq!(bp[2].q, 0.025);
    assert_eq!(bp[2].slope, Slope::Db48);
    assert_eq!(bp[31].shape, Shape::Notch);
    assert_eq!(bp[31].placement, Placement::Mid);
    assert_eq!(bp[31].freq_hz, 1000.0);
    assert_eq!(bp.iter().filter(|b| b.used).count(), 2);
    assert_eq!(bp.len(), NUM_BANDS);

    // empty object is valid
    let e = PresetFile::from_json("{}").unwrap();
    assert_eq!(e.gain_scale, 100.0);
    assert!(e.bands.is_empty());

    assert!(PresetFile::from_json("not json").is_err());
    assert!(PresetFile::from_json(r#"{"bands":[{"band":33}]}"#).is_err());
    assert!(PresetFile::from_json(r#"{"bands":[{"band":0}]}"#).is_err());
    assert!(PresetFile::from_json(r#"{"bands":[{"band":1,"shape":"zzz"}]}"#).is_err());
    assert!(PresetFile::from_json(r#"{"bands":[{"band":1,"placement":"up"}]}"#).is_err());
}

// ---- apply() through a recording GuiContext ----

use nih_plug::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<(ParamPtr, &'static str, f32)>>,
}

impl GuiContext for Recorder {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn request_resize(&self) -> bool {
        true
    }
    unsafe fn raw_begin_set_parameter(&self, p: ParamPtr) {
        self.events.lock().unwrap().push((p, "begin", 0.0));
    }
    unsafe fn raw_set_parameter_normalized(&self, p: ParamPtr, n: f32) {
        self.events.lock().unwrap().push((p, "set", n));
    }
    unsafe fn raw_end_set_parameter(&self, p: ParamPtr) {
        self.events.lock().unwrap().push((p, "end", 0.0));
    }
    fn get_state(&self) -> PluginState {
        unimplemented!()
    }
    fn set_state(&self, _: PluginState) {
        unimplemented!()
    }
}

fn expected_normalized(params: &NeoFilterParams, preset: &FactoryPreset) -> HashMap<ParamPtr, f32> {
    let mut m = HashMap::new();
    for (set, p) in params.bands.iter().zip(preset.band_params()) {
        m.insert(set.used.as_ptr(), set.used.preview_normalized(p.used));
        m.insert(set.on.as_ptr(), set.on.preview_normalized(p.enabled));
        m.insert(
            set.shape.as_ptr(),
            set.shape.preview_normalized(BandShape::from(p.shape)),
        );
        m.insert(
            set.freq.as_ptr(),
            set.freq.preview_normalized(p.freq_hz as f32),
        );
        m.insert(
            set.gain.as_ptr(),
            set.gain.preview_normalized(p.gain_db as f32),
        );
        m.insert(set.q.as_ptr(), set.q.preview_normalized(p.q as f32));
        m.insert(
            set.slope.as_ptr(),
            set.slope.preview_normalized(BandSlope::from(p.slope)),
        );
        m.insert(
            set.place.as_ptr(),
            set.place.preview_normalized(BandPlace::from(p.placement)),
        );
    }
    m.insert(
        params.gain_scale.as_ptr(),
        params.gain_scale.preview_normalized(preset.gain_scale),
    );
    m.insert(
        params.out_gain.as_ptr(),
        params.out_gain.preview_normalized(preset.out_gain_db),
    );
    m
}

#[test]
fn apply_sets_every_parameter_with_gestures() {
    for preset in factory_presets() {
        let params = NeoFilterParams::default();
        let rec = Recorder::default();
        let setter = ParamSetter::new(&rec);
        apply(preset, &params, &setter);
        let want = expected_normalized(&params, preset);
        assert_eq!(want.len(), 8 * NUM_BANDS + 2);

        let events = rec.events.lock().unwrap();
        assert_eq!(events.len(), want.len() * 3);
        // each parameter: begin, set, end consecutively
        for chunk in events.chunks(3) {
            assert_eq!(chunk[0].1, "begin");
            assert_eq!(chunk[1].1, "set");
            assert_eq!(chunk[2].1, "end");
            assert!(chunk[0].0 == chunk[1].0 && chunk[1].0 == chunk[2].0);
            let exp = want.get(&chunk[1].0).expect("unexpected parameter");
            assert!((chunk[1].2 - exp).abs() < 1e-6, "{preset:?}");
        }
        let touched: std::collections::HashSet<_> = events.iter().map(|e| e.0).collect();
        assert_eq!(touched.len(), want.len(), "every parameter exactly once");
    }
}
