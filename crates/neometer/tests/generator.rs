//! Generator behaviour through the audio path.
mod common;
use common::*;
use neometer::params::NeoMeterParams;
use neometer::testgen::{GenSettings, Kind, Mode, Routing};
use nih_plug::prelude::Params;

const FS: f64 = 48_000.0;

fn rig() -> Rig {
    Rig::without_design(FS)
}

fn on(r: &Rig, s: GenSettings) {
    r.shared.gen.set(GenSettings { on: true, ..s });
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn generator_is_off_at_construction_and_input_untouched() {
    let r = rig();
    assert!(!r.shared.gen.is_on());
    assert!(!r.shared.gen.get().on);
}

#[test]
fn generator_is_off_after_state_restore_and_not_in_the_state() {
    // A: generator running, state saved
    let a = NeoMeterParams::default();
    let shared_a = neometer::shared::Shared::new(a.settings.clone(), None);
    shared_a.gen.set(GenSettings {
        on: true,
        kind: Kind::White,
        ..GenSettings::default()
    });
    shared_a
        .settings
        .update(|d| d.page = neometer::settings::Page::Test);
    let saved = a.serialize_fields();
    let text = serde_json::to_string(&saved).unwrap();
    assert!(
        !text.to_lowercase().contains("gen"),
        "generator state leaked into {text}"
    );
    // B: fresh instance restores that state
    let b = NeoMeterParams::default();
    let shared_b = neometer::shared::Shared::new(b.settings.clone(), None);
    b.deserialize_fields(&saved);
    assert_eq!(b.settings.get().page, neometer::settings::Page::Test);
    assert!(!shared_b.gen.is_on());
    // and activation forces it off even if something switched it on before
    let mut r = rig();
    on(&r, GenSettings::default());
    assert!(r.shared.gen.is_on());
    let (prod, _cons) = neometer::ring::channel(1024);
    r.audio.prepare(FS, false, common::pre_emphasis(FS), prod);
    assert!(!r.shared.gen.is_on());
}

#[test]
fn sine_replaces_input_click_free_and_returns_to_bit_exact_pass_through() {
    let mut r = rig();
    let n = 48_000;
    let mut noise = Noise(3);
    let inp = noise.vec(n);
    // silent input, generator on 1 kHz -20 dBFS
    let zero = vec![0.0f32; n];
    on(&r, GenSettings::default());
    let (l, rr) = r.feed(&zero, &zero, 300);
    let steady = max_step(&l[20_000..30_000]);
    let all = max_step(&l);
    assert!(all < steady * 1.1 + 1e-4, "start click: {all} vs {steady}");
    assert_eq!(l, rr, "both channels identical");
    // steady-state level: peak -20 dBFS
    let pk = l[20_000..].iter().fold(0.0f32, |a, v| a.max(v.abs()));
    assert!((20.0 * (pk as f64).log10() + 20.0).abs() < 0.05, "{pk}");
    // stop while running: fade, no click
    r.shared.gen.force_off();
    let (l2, _) = r.feed(&zero, &zero, 300);
    assert!(max_step(&l2) < steady * 1.1 + 1e-4, "stop click");
    assert!(l2[4000..].iter().all(|&v| v == 0.0));
    // after the fade the input passes bit-exactly again
    let (a, b) = r.feed(&inp, &inp, 777);
    assert_eq!(a, inp);
    assert_eq!(b, inp);
}

#[test]
fn replace_crossfades_input_and_add_sums() {
    let mut r = rig();
    let inp = sine(FS, 220.0, -10.0, 24_000);
    on(
        &r,
        GenSettings {
            kind: Kind::Sine,
            freq_hz: 1000.0,
            level_db: -20.0,
            ..GenSettings::default()
        },
    );
    let (a, _) = r.feed(&inp, &inp, 512);
    // fully on: no 220 Hz left (replace), 1 kHz present
    let tail = &a[12_000..];
    let t: Vec<f64> = tail.iter().map(|&v| f64::from(v)).collect();
    let l1k = neo_measure::tone::tone_level_at(&t, FS, 1000.0);
    let l220 = neo_measure::tone::tone_level_at(&t, FS, 220.0);
    assert!((l1k + 20.0).abs() < 0.1, "{l1k}");
    assert!(l220 < -100.0, "{l220}");

    let mut r = rig();
    on(
        &r,
        GenSettings {
            mode: Mode::Add,
            ..GenSettings::default()
        },
    );
    let (a, _) = r.feed(&inp, &inp, 512);
    let t: Vec<f64> = a[12_000..].iter().map(|&v| f64::from(v)).collect();
    assert!((neo_measure::tone::tone_level_at(&t, FS, 220.0) + 10.0).abs() < 0.05);
    assert!((neo_measure::tone::tone_level_at(&t, FS, 1000.0) + 20.0).abs() < 0.05);
}

#[test]
fn routing_left_right_and_inverted() {
    let zero = vec![0.0f32; 24_000];
    for (routing, l_on, r_gain) in [
        (Routing::Left, true, 0.0f32),
        (Routing::Right, false, 1.0),
        (Routing::Both, true, 1.0),
        (Routing::BothInverted, true, -1.0),
    ] {
        let mut r = rig();
        on(
            &r,
            GenSettings {
                routing,
                ..GenSettings::default()
            },
        );
        let (l, rr) = r.feed(&zero, &zero, 512);
        let (l, rr) = (&l[12_000..], &rr[12_000..]);
        let lmax = l.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let rmax = rr.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert_eq!(lmax > 0.05, l_on, "{routing:?}");
        assert_eq!(rmax > 0.05, r_gain != 0.0, "{routing:?}");
        if r_gain != 0.0 && l_on {
            for i in 0..l.len() {
                assert!((rr[i] - r_gain * l[i]).abs() < 1e-6, "{routing:?}");
            }
        }
    }
}

#[test]
fn noise_levels_are_rms_and_sweep_runs() {
    for (kind, want) in [(Kind::White, -20.0), (Kind::Pink, -20.0)] {
        let mut r = rig();
        let zero = vec![0.0f32; 96_000];
        on(
            &r,
            GenSettings {
                kind,
                level_db: -20.0,
                ..GenSettings::default()
            },
        );
        let (l, _) = r.feed(&zero, &zero, 480);
        let e: f64 = l[24_000..]
            .iter()
            .map(|&v| f64::from(v).powi(2))
            .sum::<f64>()
            / (l.len() - 24_000) as f64;
        let db = 10.0 * e.log10();
        // pink noise carries most of its power at very low frequencies, so
        // 1.5 s of it has a large statistical spread (neo-measure: 0.3 dB over 20 s)
        let tol = if kind == Kind::Pink { 1.5 } else { 0.3 };
        assert!((db - want).abs() < tol, "{kind:?} {db}");
    }
    let mut r = rig();
    let zero = vec![0.0f32; 48_000];
    on(
        &r,
        GenSettings {
            kind: Kind::Sweep,
            sweep_s: 2.0,
            ..GenSettings::default()
        },
    );
    let (l, _) = r.feed(&zero, &zero, 480);
    let pk = l.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    assert!((pk - 0.1).abs() < 0.01, "{pk}");
}

#[test]
fn changing_the_waveform_while_running_fades_out_first() {
    let mut r = rig();
    let zero = vec![0.0f32; 24_000];
    on(&r, GenSettings::default());
    r.feed(&zero, &zero, 480);
    let steady = max_step(&r.feed(&zero, &zero, 480).0);
    r.shared.gen.set(GenSettings {
        on: true,
        kind: Kind::White,
        ..GenSettings::default()
    });
    let (l, _) = r.feed(&zero, &zero, 480);
    // the sine fades out to silence before the noise fades in: the first
    // samples are still the sine (no abrupt switch)
    assert!(max_step(&l[..400]) < steady * 1.2 + 1e-4);
}
