//! "Friend EQ A+B (series)" must behave exactly like Friend EQ A followed by
//! Friend EQ B in two separate instances.
mod common;
use common::*;
use neofilter::audio::{AudioParams, AudioPath};
use neofilter_dsp::response::total_response_db;
use neofilter_dsp::{BandParams, EqEngine};

fn engine(fs: f64, bands: &[BandParams]) -> EqEngine {
    let mut e = EqEngine::new(fs);
    for (i, b) in bands.iter().enumerate() {
        e.set_band(i, b);
    }
    e
}

fn rel_err(got: &[f64], want: &[f64]) -> f64 {
    let peak = want.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    got.iter()
        .zip(want)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max)
        / peak
}

#[test]
fn response_is_the_sum_of_a_and_b_in_db() {
    let (a, b, ab) = (
        preset("friend-eq-a"),
        preset("friend-eq-b"),
        preset("friend-eq-ab"),
    );
    for fs in [44100.0, 48000.0, 96000.0] {
        let freqs = log_freqs(10.0, 0.99 * fs / 2.0, 2000);
        let (mut ra, mut rb, mut rab) = (vec![0.0; 2000], vec![0.0; 2000], vec![0.0; 2000]);
        total_response_db(&a, 1.0, fs, &freqs, &mut ra);
        total_response_db(&b, 1.0, fs, &freqs, &mut rb);
        total_response_db(&ab, 1.0, fs, &freqs, &mut rab);
        let worst = (0..2000)
            .map(|i| (rab[i] - (ra[i] + rb[i])).abs())
            .fold(0.0, f64::max);
        println!("fs {fs}: max |AB - (A+B)| = {worst:e} dB");
        assert!(worst < 1e-9, "fs {fs}: {worst}");
        // and it is not trivially flat
        assert!(rab.iter().any(|v| v.abs() > 1.0));
    }
}

#[test]
fn engine_matches_two_engines_in_series() {
    let (a, b, ab) = (
        preset("friend-eq-a"),
        preset("friend-eq-b"),
        preset("friend-eq-ab"),
    );
    for fs in [44100.0, 48000.0, 96000.0] {
        let mut noise = common::Noise(42);
        let n = 1 << 16;
        let white: Vec<f64> = (0..n).map(|_| 0.3 * f64::from(noise.next())).collect();
        let mut imp = vec![0.0; n];
        imp[0] = 1.0;
        for (name, input) in [("white noise", white), ("impulse", imp)] {
            let (mut ea, mut eb, mut eab) = (engine(fs, &a), engine(fs, &b), engine(fs, &ab));
            let mut chain = input.clone();
            ea.process_mono(&mut chain);
            eb.process_mono(&mut chain);
            let mut one = input.clone();
            eab.process_mono(&mut one);
            let err = rel_err(&one, &chain);
            println!("fs {fs} {name}: f64 engine relative error {err:e}");
            assert!(err < 1e-9, "fs {fs} {name}: {err}");
        }
    }
}

fn params(bands: &[BandParams; 32]) -> AudioParams<'_> {
    AudioParams {
        bands,
        gain_scale: 1.0,
        out_gain_db: 0.0,
        bypass: false,
    }
}

#[test]
fn f32_audio_path_matches_two_instances_up_to_f32_rounding() {
    // Two plugin instances exchange f32 samples between them; the combined
    // instance does not round in between, so only f32 rounding may differ.
    let (a, b, ab) = (
        preset("friend-eq-a"),
        preset("friend-eq-b"),
        preset("friend-eq-ab"),
    );
    for fs in [44100.0, 48000.0, 96000.0] {
        let mut noise = common::Noise(7);
        let white: Vec<f32> = noise.vec(1 << 16).iter().map(|v| 0.3 * v).collect();
        let mut imp = vec![0.0f32; 1 << 16];
        imp[0] = 1.0;
        for (name, input) in [("white noise", white), ("impulse", imp)] {
            let (mut pa, mut pb, mut pab) = (
                AudioPath::new(fs, 512),
                AudioPath::new(fs, 512),
                AudioPath::new(fs, 512),
            );
            let (mut chain_l, mut chain_r) = (input.clone(), input.clone());
            let (mut one_l, mut one_r) = (input.clone(), input.clone());
            for (cl, cr) in chain_l.chunks_mut(512).zip(chain_r.chunks_mut(512)) {
                pa.process_stereo(cl, cr, &params(&a));
                pb.process_stereo(cl, cr, &params(&b));
            }
            for (ol, or) in one_l.chunks_mut(512).zip(one_r.chunks_mut(512)) {
                pab.process_stereo(ol, or, &params(&ab));
            }
            let l: Vec<f64> = one_l.iter().map(|&v| f64::from(v)).collect();
            let c: Vec<f64> = chain_l.iter().map(|&v| f64::from(v)).collect();
            let err = rel_err(&l, &c);
            println!("fs {fs} {name}: f32 AudioPath relative error {err:e}");
            // f32 epsilon is 1.2e-7; allow a few ulps of the peak
            assert!(err < 1e-6, "fs {fs} {name}: {err}");
            assert_eq!(one_l, one_r);
        }
    }
}
