//! Plugin-level guarantees of the calibration: layouts, state, noise safety,
//! and an allocation-free audio path while measuring.
use assert_no_alloc::assert_no_alloc;
use neofilter::audio::{AudioParams, AudioPath};
use neofilter::calibration::settings::{
    CalCurve, CalPersist, CurvePoint, InjectMode, NoiseKind, Routing,
};
use neofilter::calibration::{CalLink, CalTap};
use neofilter::params::NeoFilterParams;
use neofilter::NeoFilter;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;
use nih_plug::prelude::*;
use std::sync::Arc;

#[test]
fn layouts_keep_the_old_ones_and_add_a_stereo_sidechain() {
    let l = NeoFilter::AUDIO_IO_LAYOUTS;
    let main = |i: usize| {
        (
            l[i].main_input_channels.map(|c| c.get()),
            l[i].main_output_channels.map(|c| c.get()),
        )
    };
    assert!(l
        .iter()
        .any(|x| main_of(x) == (Some(2), Some(2)) && x.aux_input_ports.is_empty()));
    assert!(l
        .iter()
        .any(|x| main_of(x) == (Some(1), Some(1)) && x.aux_input_ports.is_empty()));
    let aux: Vec<_> = l.iter().filter(|x| !x.aux_input_ports.is_empty()).collect();
    assert_eq!(aux.len(), 1);
    assert_eq!(aux[0].aux_input_ports.len(), 1);
    assert_eq!(aux[0].aux_input_ports[0].get(), 2);
    let _ = main(0);
}

fn main_of(x: &AudioIOLayout) -> (Option<u32>, Option<u32>) {
    (
        x.main_input_channels.map(|c| c.get()),
        x.main_output_channels.map(|c| c.get()),
    )
}

#[test]
fn old_state_without_calibration_still_loads() {
    let p = NeoFilterParams::default();
    // state saved by a version without calibration: no "calibration" field
    let mut map = std::collections::BTreeMap::new();
    map.insert(
        "editor-state".to_string(),
        "{\"size\":[900,560],\"open\":false}".to_string(),
    );
    p.deserialize_fields(&map);
    assert_eq!(*p.calibration.lock().unwrap(), CalPersist::default());
    assert_eq!(p.band_snapshot(), [BandParams::default(); NUM_BANDS]);
}

#[test]
fn calibration_state_roundtrips_without_a_running_flag() {
    let p = NeoFilterParams::default();
    {
        let mut c = p.calibration.lock().unwrap();
        c.band_mask = 0xFFF0;
        c.settings.level_db = -25.0;
        c.settings.max_bands = 8;
        c.correction = Some(CalCurve::new(
            "record",
            vec![
                CurvePoint {
                    freq_hz: 800.0,
                    db: -0.4,
                },
                CurvePoint {
                    freq_hz: 16_000.0,
                    db: 1.0,
                },
            ],
        ));
    }
    let map = p.serialize_fields();
    let json = map.get("calibration").expect("persisted");
    assert!(
        !json.contains("running") && !json.contains("measuring"),
        "{json}"
    );
    let q = NeoFilterParams::default();
    q.deserialize_fields(&map);
    assert_eq!(
        *q.calibration.lock().unwrap(),
        *p.calibration.lock().unwrap()
    );
}

#[test]
fn corrupt_calibration_state_falls_back_to_defaults() {
    let p = NeoFilterParams::default();
    let mut map = std::collections::BTreeMap::new();
    map.insert("calibration".to_string(), "{ not json".to_string());
    p.deserialize_fields(&map);
    assert_eq!(*p.calibration.lock().unwrap(), CalPersist::default());
}

#[test]
fn noise_is_off_after_construction_restore_and_reset() {
    let link = Arc::new(CalLink::new());
    let mut tap = CalTap::new(link.clone(), 48_000.0);
    let mut l = vec![0.1f32; 4800];
    let mut r = vec![0.1f32; 4800];
    // construction
    tap.process_stereo(&mut l, &mut r, &[], &[]);
    assert!(l.iter().all(|&v| v == 0.1) && !link.noise_active() && !link.is_running());
    // state restore does not start anything: parameters/persist hold no run state
    let p = NeoFilterParams::default();
    let map = p.serialize_fields();
    p.deserialize_fields(&map);
    tap.process_stereo(&mut l, &mut r, &[], &[]);
    assert!(l.iter().all(|&v| v == 0.1) && !link.is_running());
    // reset / prepare while measuring
    link.start(
        NoiseKind::Pink,
        Routing::Both,
        InjectMode::Replace,
        -20.0,
        1_000_000,
    );
    tap.process_stereo(&mut l, &mut r, &[], &[]);
    assert!(link.noise_active());
    tap.reset();
    l.fill(0.1);
    r.fill(0.1);
    tap.process_stereo(&mut l, &mut r, &[], &[]);
    assert!(l.iter().all(|&v| v == 0.1) && !link.noise_active() && !link.is_running());
    link.start(
        NoiseKind::White,
        Routing::Left,
        InjectMode::Add,
        -20.0,
        1_000_000,
    );
    tap.prepare(44_100.0);
    tap.process_stereo(&mut l, &mut r, &[], &[]);
    assert!(l.iter().all(|&v| v == 0.1) && !link.is_running());
}

#[test]
fn no_parameter_can_start_noise() {
    // The tap has a single input, `CalLink::start`, called by the editor's
    // Measure/Auto buttons. No automatable parameter refers to calibration,
    // so neither automation nor a state restore can reach it.
    let params = NeoFilterParams::default();
    for (id, _, _) in params.param_map() {
        let ok = id == "bypass"
            || id == "out_gain"
            || id == "gain_scale"
            || ["freq", "gain", "q", "used", "on", "shape", "slope", "place"]
                .iter()
                .any(|p| id.starts_with(&format!("{p}_")));
        assert!(ok, "unexpected parameter {id}");
    }
    assert_eq!(params.param_map().len(), 3 + 8 * 32);
}

#[test]
fn calibrating_audio_path_does_not_allocate() {
    let link = Arc::new(CalLink::new());
    let mut tap = CalTap::new(link.clone(), 48_000.0);
    let mut path = AudioPath::new(48_000.0, 512);
    let bands = [BandParams::default(); NUM_BANDS];
    let p = AudioParams {
        bands: &bands,
        gain_scale: 1.0,
        out_gain_db: 0.0,
        bypass: false,
    };
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let ret = vec![0.01f32; 512];
    let mut m = vec![0.0f32; 512];
    for (kind, routing, mode) in [
        (NoiseKind::Pink, Routing::Both, InjectMode::Replace),
        (NoiseKind::White, Routing::Left, InjectMode::Add),
        (NoiseKind::Pink, Routing::Right, InjectMode::Replace),
    ] {
        link.start(kind, routing, mode, -20.0, 20_000);
        assert_no_alloc(|| {
            for _ in 0..30 {
                tap.process_stereo(&mut l, &mut r, &ret, &ret);
                path.process_stereo(&mut l, &mut r, &p);
                tap.process_mono(&mut m, &ret, &[]);
                path.process_mono(&mut m, &p);
            }
            link.stop();
            for _ in 0..4 {
                tap.process_stereo(&mut l, &mut r, &ret, &ret);
            }
        });
        while link.pop().is_some() {}
    }
    // also reset and the idle path
    assert_no_alloc(|| {
        tap.reset();
        tap.process_stereo(&mut l, &mut r, &[], &[]);
    });
}
