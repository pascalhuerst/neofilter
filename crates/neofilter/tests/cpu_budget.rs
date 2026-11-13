//! Worst-case CPU measurements (run manually, no timing assertions):
//! `cargo test -p neofilter --release --test cpu_budget -- --ignored --nocapture`
mod common;
use common::*;
use neofilter::audio::{AudioParams, AudioPath};
use neofilter_dsp::response::total_response_db;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, Placement, Shape, Slope};
use std::time::Instant;

fn worst_bands(shift: f64) -> [BandParams; NUM_BANDS] {
    std::array::from_fn(|i| BandParams {
        used: true,
        enabled: true,
        shape: [Shape::Bell, Shape::LowCut, Shape::HighCut, Shape::HighShelf][i % 4],
        freq_hz: (100.0 + 400.0 * i as f64) * (1.0 + shift),
        gain_db: 6.0,
        q: 2.0,
        slope: Slope::Db96,
        placement: Placement::Stereo,
    })
}

#[test]
#[ignore]
fn audio_path_cpu_all_bands_96_db_at_96k() {
    let fs = 96000.0;
    let secs = 10.0;
    let blocks = (secs * fs / 512.0) as usize;
    let mut noise = Noise(5);
    let (mut l, mut r) = (noise.vec(512), noise.vec(512));
    for l_or_r in [&mut l, &mut r] {
        for v in l_or_r.iter_mut() {
            *v *= 0.1;
        }
    }
    for (name, modulate) in [("static", false), ("all bands moving every block", true)] {
        let mut path = AudioPath::new(fs, 512);
        let mut worst_block = 0.0f64;
        let t0 = Instant::now();
        for k in 0..blocks {
            let bands = worst_bands(if modulate {
                0.2 * (k as f64 * 0.01).sin()
            } else {
                0.0
            });
            let p = AudioParams {
                bands: &bands,
                gain_scale: 1.0,
                out_gain_db: 0.0,
                bypass: false,
            };
            let (mut a, mut b) = (l.clone(), r.clone());
            let t = Instant::now();
            path.process_stereo(&mut a, &mut b, &p);
            worst_block = worst_block.max(t.elapsed().as_secs_f64());
        }
        let el = t0.elapsed().as_secs_f64();
        let budget = 512.0 / fs;
        println!(
            "{NUM_BANDS} bands x 96 dB/oct, {fs} Hz, {name}: {:.2} % of one core on average, \
             worst block {:.2} % of its {:.2} ms real-time budget",
            100.0 * el / secs,
            100.0 * worst_block / budget,
            budget * 1e3
        );
    }
}

#[test]
#[ignore]
fn editor_curve_cost_for_all_bands() {
    let fs = 48000.0;
    let bands = worst_bands(0.0);
    for points in [500usize, 1000] {
        let freqs = log_freqs(10.0, 30000.0, points);
        let mut out = vec![0.0; points];
        let n = 200;
        let t = Instant::now();
        for _ in 0..n {
            total_response_db(&bands, 1.0, fs, &freqs, &mut out);
            std::hint::black_box(&out);
        }
        let per = t.elapsed().as_secs_f64() / n as f64;
        println!(
            "total curve, {NUM_BANDS} bands at 96 dB/oct, {points} points: {:.3} ms/frame",
            per * 1e3
        );
    }
}
