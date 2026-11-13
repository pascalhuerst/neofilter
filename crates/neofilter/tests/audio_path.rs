mod common;
use common::*;
use neofilter::audio::{AudioParams, AudioPath};
use neofilter_dsp::response::total_response_db;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, Placement, Shape, Slope};

const FLAT: [BandParams; NUM_BANDS] = [BandParams {
    used: false,
    enabled: true,
    shape: Shape::Bell,
    freq_hz: 1000.0,
    gain_db: 0.0,
    q: 1.0,
    slope: Slope::Db12,
    placement: Placement::Stereo,
}; NUM_BANDS];

fn params(bands: &[BandParams; NUM_BANDS]) -> AudioParams<'_> {
    AudioParams {
        bands,
        gain_scale: 1.0,
        out_gain_db: 0.0,
        bypass: false,
    }
}

fn impulse(n: usize) -> Vec<f32> {
    let mut v = vec![0.0; n];
    v[0] = 1.0;
    v
}

#[test]
fn default_state_is_bit_transparent() {
    let mut n = Noise(1);
    let (l0, r0) = (n.vec(5000), n.vec(5000));
    let mut path = AudioPath::new(48000.0, 512);
    let (mut l, mut r) = (l0.clone(), r0.clone());
    path.process_stereo(&mut l, &mut r, &params(&FLAT));
    assert_eq!(l, l0);
    assert_eq!(r, r0);
    let mut m = l0.clone();
    path.process_mono(&mut m, &params(&FLAT));
    assert_eq!(m, l0);
}

#[test]
fn preset_a_matches_analytic_response() {
    let bands = preset("friend-eq-a");
    let freqs = log_freqs(20.0, 20000.0, 150);
    for sr in [44100.0, 48000.0] {
        let mut path = AudioPath::new(sr, 512);
        let mut l = impulse(1 << 17);
        let mut r = l.clone();
        path.process_stereo(&mut l, &mut r, &params(&bands));
        let mut want = vec![0.0; freqs.len()];
        total_response_db(&bands, 1.0, sr, &freqs, &mut want);
        let mut worst: f64 = 0.0;
        for (f, w) in freqs.iter().zip(&want) {
            let got = dft_db(&l, *f, sr);
            worst = worst.max((got - w).abs());
            assert!((got - dft_db(&r, *f, sr)).abs() < 1e-6);
        }
        println!("preset A at {sr}: worst deviation {worst:.5} dB");
        assert!(worst < 0.01, "worst {worst} dB at {sr}");
    }
}

#[test]
fn mono_matches_stereo() {
    let bands = preset("friend-eq-a");
    let mut n = Noise(7);
    let x = n.vec(10_000);
    let mut path = AudioPath::new(48000.0, 512);
    let (mut l, mut r) = (x.clone(), x.clone());
    path.process_stereo(&mut l, &mut r, &params(&bands));
    let mut mono = AudioPath::new(48000.0, 512);
    let mut m = x.clone();
    mono.process_mono(&mut m, &params(&bands));
    let worst = l
        .iter()
        .zip(&m)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    println!("mono vs stereo max diff {worst:e}");
    assert!(worst < 1e-7);
}

#[test]
fn output_gain_static_and_ramped() {
    let mut n = Noise(3);
    let x = n.vec(2000);
    let mut path = AudioPath::new(48000.0, 256);
    let mut p = params(&FLAT);
    p.out_gain_db = 6.0;
    let mut y = x.clone();
    let mut y2 = x.clone();
    path.process_stereo(&mut y, &mut y2, &p);
    let g = 10f64.powf(6.0 / 20.0);
    for (a, b) in x.iter().zip(&y) {
        assert!((f64::from(*a) * g - f64::from(*b)).abs() < 1e-6);
    }
    // ramp down to -12 dB: constant input, gain must move smoothly and arrive
    p.out_gain_db = -12.0;
    let mut l = vec![1.0f32; 4800];
    let mut r = l.clone();
    path.process_stereo(&mut l, &mut r, &p);
    let max_step = l
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    println!("gain ramp max step {max_step:e}");
    assert!(max_step < 5e-3); // 18 dB over 20 ms from gain 2.0: ~4.3e-3 per sample
    assert!(l.windows(2).all(|w| w[1] <= w[0]));
    let want = 10f64.powf(-12.0 / 20.0) as f32;
    assert!((l[4799] - want).abs() < 1e-6);
    assert!(l[100] > 1.5); // still ramping after ~2 ms
}

#[test]
fn bypass_crossfade_is_linear_click_free_and_dry_afterwards() {
    let bands = preset("friend-eq-a");
    let sr = 44100.0;
    let n = 4000;
    let x: Vec<f32> = (0..n)
        .map(|i| (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / sr).sin()) as f32)
        .collect();
    let mut wet = x.clone();
    let mut wet_r = x.clone();
    AudioPath::new(sr, 512).process_stereo(&mut wet, &mut wet_r, &params(&bands));

    let mut path = AudioPath::new(sr, 512);
    let mut out = x.clone();
    let mut out_r = x.clone();
    let start = 1000;
    path.process_stereo(&mut out[..start], &mut out_r[..start], &params(&bands));
    let mut p = params(&bands);
    p.bypass = true;
    path.process_stereo(&mut out[start..], &mut out_r[start..], &p);
    let ramp = (0.010 * sr) as usize;
    let mut worst: f64 = 0.0;
    for i in 0..n {
        let m = if i < start {
            0.0
        } else {
            ((i - start + 1) as f64 / ramp as f64).min(1.0)
        };
        let want = f64::from(wet[i]) + m * f64::from(x[i] - wet[i]);
        worst = worst.max((want - f64::from(out[i])).abs());
        assert_eq!(out[i], out_r[i]);
    }
    println!("bypass crossfade deviation from ideal linear fade {worst:e}");
    assert!(worst < 1e-6);
    // wet and dry differ noticeably, so a hard switch would have been a click
    let diff = (start..start + ramp)
        .map(|i| (wet[i] - x[i]).abs())
        .fold(0.0, f32::max);
    assert!(diff > 0.05, "wet/dry difference {diff}");
    // exactly dry after the fade
    assert_eq!(&out[start + ramp + 1..], &x[start + ramp + 1..]);

    // un-bypass: fades back to the (still running) EQ without a step
    let mut back = x.clone();
    let mut back_r = x.clone();
    path.process_stereo(&mut back, &mut back_r, &params(&bands));
    let mut full_wet = x.clone();
    let mut full_wet_r = x.clone();
    // the EQ ran on [x, x]; reproduce its output as reference
    let mut reference = AudioPath::new(sr, 512);
    let mut tmp = x.clone();
    let mut tmp_r = x.clone();
    reference.process_stereo(&mut tmp, &mut tmp_r, &params(&bands));
    reference.process_stereo(&mut full_wet, &mut full_wet_r, &params(&bands));
    let mut worst: f64 = 0.0;
    for i in 0..n {
        let m = (1.0 - (i + 1) as f64 / ramp as f64).max(0.0);
        let want = f64::from(full_wet[i]) + m * f64::from(x[i] - full_wet[i]);
        worst = worst.max((want - f64::from(back[i])).abs());
    }
    println!("un-bypass deviation from ideal linear fade {worst:e}");
    assert!(worst < 1e-6);
}

#[test]
fn block_sizes_and_oversized_blocks_give_identical_output() {
    let bands = preset("friend-eq-b");
    let mut n = Noise(11);
    let (xl, xr) = (n.vec(9000), n.vec(9000));
    let run = |block: usize, max_block: usize| {
        let mut path = AudioPath::new(48000.0, max_block);
        let (mut l, mut r) = (xl.clone(), xr.clone());
        let mut p = params(&bands);
        p.out_gain_db = -3.0;
        for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            path.process_stereo(cl, cr, &p);
        }
        (l, r)
    };
    let (rl, rr) = run(9000, 9000);
    assert!(rl.iter().chain(&rr).all(|v| v.is_finite()));
    for block in [1usize, 7, 64, 512, 4096] {
        let (l, r) = run(block, 512);
        assert_eq!(l, rl, "left, block {block}");
        assert_eq!(r, rr, "right, block {block}");
    }
    // host block larger than the prepared maximum (and a tiny maximum)
    for max_block in [1usize, 64, 100] {
        let (l, r) = run(9000, max_block);
        assert_eq!(l, rl, "max_block {max_block}");
        assert_eq!(r, rr);
    }
}

#[test]
fn no_nan_with_extreme_parameters() {
    let mut bands = preset("friend-eq-a");
    for (i, b) in bands.iter_mut().enumerate() {
        b.used = true;
        b.gain_db = if i % 2 == 0 { 30.0 } else { -30.0 };
        b.q = if i % 3 == 0 { 40.0 } else { 0.025 };
        b.freq_hz = [10.0, 30000.0, 20000.0, 500.0][i % 4];
    }
    let mut n = Noise(5);
    let mut path = AudioPath::new(44100.0, 512);
    let mut p = params(&bands);
    p.out_gain_db = 36.0;
    p.gain_scale = 2.0;
    for _ in 0..20 {
        let (mut l, mut r) = (n.vec(512), n.vec(512));
        path.process_stereo(&mut l, &mut r, &p);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }
}
