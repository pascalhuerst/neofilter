//! Host-independent audio path tests.
mod common;
use common::*;
use neo_measure::riaa::{analog_response, Complex, Mode, Options};
use neoriaa::audio::{AudioParams, AudioPath};
use realfft::RealFftPlanner;
use std::sync::atomic::Ordering;

fn de() -> AudioParams {
    AudioParams {
        mode: Mode::DeEmphasis,
        ..AudioParams::default()
    }
}

#[test]
fn bypass_is_bit_transparent() {
    let (mut p, _) = path(48_000.0);
    let mut n = Noise(1);
    let mut l = n.vec(5000, 1.5);
    let mut r = n.vec(5000, 1.5);
    l[10] = f32::NAN;
    l[11] = f32::INFINITY;
    r[12] = -0.0;
    r[13] = f32::MIN_POSITIVE / 4.0; // denormal
    let (l0, r0) = (l.clone(), r.clone());
    let bp = AudioParams {
        bypass: true,
        trim_db: 9.0, // trim must not leak into the dry path
        ..AudioParams::default()
    };
    p.process_stereo(&mut l, &mut r, &bp);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&l), bits(&l0));
    assert_eq!(bits(&r), bits(&r0));

    // engage, then bypass again: after the 10 ms crossfade it is exact again
    let mut x = n.vec(4800, 0.3);
    p.process_mono(&mut x, &AudioParams::default());
    let mut y = n.vec(4800, 0.3);
    let y0 = y.clone();
    p.process_mono(&mut y, &bp);
    // the last 100 samples are well past the ramp (480 samples)
    assert_eq!(bits(&y[4000..]), bits(&y0[4000..]));
    assert_ne!(bits(&y[..100]), bits(&y0[..100]));
}

#[test]
fn pre_then_de_is_unity_at_float_precision() {
    // The exact (1e-9) cascade identity is tested on the f64 filters in
    // neo-measure tests/riaa_design.rs; here the f32 interface bounds the error.
    for fs in [48_000.0, 96_000.0] {
        let (mut pre, _) = path(fs);
        let (mut de_path, _) = path(fs);
        let mut n = Noise(5);
        let x0 = n.vec(30_000, 0.05);
        let mut x = x0.clone();
        pre.process_mono(&mut x, &AudioParams::default());
        de_path.process_mono(&mut x, &de());
        let worst = x
            .iter()
            .zip(&x0)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 2e-6, "{fs}: {worst}");
    }
}

#[test]
fn impulse_response_matches_designed_response() {
    for (fs, n) in [(48_000.0, 1usize << 17), (192_000.0, 1 << 19)] {
        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(n);
        for mode in [Mode::PreEmphasis, Mode::DeEmphasis] {
            for o in Options::ALL {
                let (mut p, _) = path(fs);
                let pr = AudioParams {
                    mode,
                    options: o,
                    ..AudioParams::default()
                };
                let amp = 0.01f32;
                let mut buf = vec![0.0f32; n];
                buf[0] = amp;
                p.process_mono(&mut buf, &pr);
                let mut ir: Vec<f64> = buf.iter().map(|&v| f64::from(v) / f64::from(amp)).collect();
                let mut spec = fft.make_output_vec();
                fft.process(&mut ir, &mut spec).unwrap();
                let design = design(fs);
                for (k, bin) in spec.iter().enumerate().skip(1) {
                    let hz = k as f64 * fs / n as f64;
                    if !(20.0..=20_000.0).contains(&hz) {
                        continue;
                    }
                    let h = Complex::new(bin.re, bin.im);
                    let d = design.response(mode, o, hz);
                    let err = h.div(d).add(Complex::new(-1.0, 0.0)).norm();
                    assert!(err < 2e-3, "{fs} {mode:?} {o:?} {hz}: {err}");
                    // and, with the design error, still on the analytic curve
                    let db = h.div(analog_response(mode, o, hz)).db().abs();
                    assert!(db < 0.02, "{fs} {mode:?} {o:?} {hz}: {db} dB");
                }
            }
        }
    }
}

#[test]
fn block_size_independence() {
    let fs = 48_000.0;
    let mut n = Noise(3);
    let input = n.vec(12_000, 0.4);
    let first = AudioParams::default();
    let second = AudioParams {
        mode: Mode::DeEmphasis,
        options: Options {
            iec: true,
            ultrasonic: true,
        },
        trim_db: -6.0,
        bypass: false,
    };
    let run = |block: usize| -> Vec<u32> {
        let (mut p, _) = path(fs);
        let mut out = input.clone();
        for (half, params) in [(0..6000, &first), (6000..12_000, &second)] {
            for chunk in out[half].chunks_mut(block) {
                p.process_mono(chunk, params);
            }
        }
        out.iter().map(|x| x.to_bits()).collect()
    };
    let reference = run(6000);
    for block in [1, 7, 64, 333, 1000] {
        assert_eq!(run(block), reference, "block {block}");
    }
}

#[test]
fn mono_equals_stereo_per_channel() {
    let fs = 96_000.0;
    let mut n = Noise(4);
    let l0 = n.vec(7000, 0.3);
    let r0 = n.vec(7000, 0.3);
    let pr = AudioParams {
        mode: Mode::PreEmphasis,
        options: Options {
            iec: true,
            ultrasonic: false,
        },
        trim_db: 3.0,
        bypass: false,
    };
    let (mut st, _) = path(fs);
    let (mut l, mut r) = (l0.clone(), r0.clone());
    st.process_stereo(&mut l, &mut r, &pr);
    for (src, stereo) in [(&l0, &l), (&r0, &r)] {
        let (mut m, _) = path(fs);
        let mut x = src.clone();
        m.process_mono(&mut x, &pr);
        assert_eq!(
            x.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            stereo.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// Switching mode must fade smoothly (the level legitimately changes by
/// about 17 dB at 200 Hz, so we test smoothness, not constant level) and,
/// once the fade is over, equal a path that ran in the new mode all along.
#[test]
fn mode_and_option_changes_are_click_free() {
    let fs = 48_000.0;
    let x = sine(24_000, fs, 200.0, 0.1);

    let steady = |pr: AudioParams| -> Vec<f32> {
        let (mut p, _) = path(fs);
        let mut y = x.clone();
        p.process_mono(&mut y, &pr);
        y
    };
    let pre = AudioParams::default();
    let steady_pre = steady(pre);
    let steady_de = steady(de());
    // per-sample step of the louder steady output (at 200 Hz that is the
    // de-emphasis, +8 dB; the pre-emphasis is -8 dB)
    let limit = max_step(&steady_pre[12_000..]).max(max_step(&steady_de[12_000..])) * 1.05;
    // skip the cold start of the very first samples (a sine starts abruptly)
    let skip = 2400;

    let (mut p, _) = path(fs);
    let mut y = x.clone();
    // blocks of 480 samples, switching mode every 4800 samples
    for (i, chunk) in y.chunks_mut(480).enumerate() {
        let pr = if (i / 10) % 2 == 0 { pre } else { de() };
        p.process_mono(chunk, &pr);
    }
    assert!(
        max_step(&y[skip..]) <= limit,
        "mode toggle: {} > {limit}",
        max_step(&y[skip..])
    );
    // after the fade, bit-equal to the steady-state de-emphasis output
    // (block 10 starts at sample 4800; fade = 720 samples)
    assert_eq!(&y[4800 + 800..9600], &steady_de[4800 + 800..9600]);
    assert_eq!(&y[9600 + 800..14_400], &steady_pre[9600 + 800..14_400]);

    // options toggled, trim stepped, bypass toggled
    for (name, a, b) in [
        (
            "iec",
            pre,
            AudioParams {
                options: Options {
                    iec: true,
                    ultrasonic: false,
                },
                ..pre
            },
        ),
        (
            "ultrasonic",
            pre,
            AudioParams {
                options: Options {
                    iec: false,
                    ultrasonic: true,
                },
                ..pre
            },
        ),
        (
            "trim",
            AudioParams {
                trim_db: -12.0,
                ..pre
            },
            pre,
        ),
        (
            "bypass",
            pre,
            AudioParams {
                bypass: true,
                ..pre
            },
        ),
    ] {
        let (mut p, _) = path(fs);
        let mut y = x.clone();
        for (i, chunk) in y.chunks_mut(240).enumerate() {
            p.process_mono(chunk, if (i / 20) % 2 == 0 { &a } else { &b });
        }
        // the louder side bounds the steady step; allow the IEC bass boost
        let bound = limit * 1.5;
        let step = max_step(&y[skip..]);
        assert!(step <= bound, "{name}: {step} > {bound}");
    }
}

#[test]
fn stable_and_finite_at_all_sample_rates() {
    for fs in RATES {
        let (mut p, _) = path(fs);
        let mut n = Noise(8);
        for mode in [Mode::PreEmphasis, Mode::DeEmphasis] {
            for o in Options::ALL {
                let pr = AudioParams {
                    mode,
                    options: o,
                    ..AudioParams::default()
                };
                let mut x = n.vec(8000, 0.5);
                x[100] = f32::NAN; // must not poison the filters
                x[101] = f32::NEG_INFINITY;
                let mut r = n.vec(8000, 0.5);
                p.process_stereo(&mut x, &mut r, &pr);
                for v in x.iter().chain(&r) {
                    assert!(v.is_finite() && v.abs() < 200.0, "{fs} {mode:?} {o:?}: {v}");
                }
            }
        }
        // decays to exact silence after the input stops
        let mut tail = vec![0.0f32; (fs * 1.5) as usize];
        p.process_mono(&mut tail, &AudioParams::default());
        assert!(tail[tail.len() - 100..].iter().all(|&v| v.abs() < 1e-6));
    }
}

#[test]
fn trim_applies_in_db_and_meters_track_peak_and_clip() {
    let fs = 48_000.0;
    let (mut p, m) = path(fs);
    // 1 kHz is the 0 dB point of every curve: output = input * trim
    let x = sine(48_000, fs, 1000.0, 0.25);
    let mut y = x.clone();
    let pr = AudioParams {
        trim_db: 6.0,
        ..AudioParams::default()
    };
    p.process_mono(&mut y, &pr);
    let peak = y[24_000..].iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let expect = 0.25 * 10f32.powf(6.0 / 20.0);
    assert!((peak - expect).abs() < 0.002, "{peak} vs {expect}");
    assert!(!m.clipped());
    let all = y.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    assert_eq!(m.peak(), all); // block peak, including the start-up transient
    assert!(m.hold() >= m.peak() - 1e-6);

    // over 0 dBFS latches; the latch survives quiet audio until reset
    let mut loud = sine(4800, fs, 1000.0, 0.9);
    p.process_mono(
        &mut loud,
        &AudioParams {
            trim_db: 6.0,
            ..AudioParams::default()
        },
    );
    assert!(m.clipped());
    let mut quiet = sine(4800, fs, 1000.0, 0.01);
    p.process_mono(&mut quiet, &pr);
    assert!(m.clipped());
    assert!(m.hold() > 1.0);
    m.reset();
    assert!(!m.clipped());
    let mut quiet = sine(4800, fs, 1000.0, 0.01);
    p.process_mono(&mut quiet, &pr);
    assert!(!m.clipped());
    assert!(m.hold() < 0.1, "hold after reset: {}", m.hold());

    // exactly full scale counts; bypass is metered as well
    let (mut p2, m2) = path(fs);
    let mut full = vec![1.0f32; 64];
    p2.process_mono(
        &mut full,
        &AudioParams {
            bypass: true,
            ..AudioParams::default()
        },
    );
    assert!(m2.clipped());
    assert_eq!(m2.peak(), 1.0);
    // running peak falls, hold stays for 2 s
    let mut silence = vec![0.0f32; 48_000];
    p2.process_mono(
        &mut silence,
        &AudioParams {
            bypass: true,
            ..AudioParams::default()
        },
    );
    assert!(
        m2.peak() < 0.2 && m2.hold() == 1.0,
        "{} {}",
        m2.peak(),
        m2.hold()
    );
    let _ = Ordering::Relaxed;
}

#[test]
fn trim_ramps_without_steps() {
    let fs = 48_000.0;
    let (mut p, _) = path(fs);
    let mut y = sine(9600, fs, 1000.0, 0.1);
    let (a, b) = y.split_at_mut(4800);
    p.process_mono(a, &AudioParams::default());
    p.process_mono(
        b,
        &AudioParams {
            trim_db: -36.0,
            ..AudioParams::default()
        },
    );
    // unramped sine step is 0.1 * 2 pi 1000/48000 = 0.0131; ramping down
    // never exceeds that
    assert!(max_step(&y[4800..]) < 0.0135, "{}", max_step(&y[4800..]));
    // reaches the target level
    let tail = y[9000..].iter().fold(0.0f32, |a, v| a.max(v.abs()));
    assert!(
        (tail - 0.1 * 10f32.powf(-36.0 / 20.0)).abs() < 5e-4,
        "{tail}"
    );
}

#[test]
fn prepare_for_new_rate_resets_cleanly() {
    let (mut p, _) = path(48_000.0);
    let mut x = sine(1000, 48_000.0, 100.0, 0.5);
    p.process_mono(&mut x, &AudioParams::default());
    p.prepare(&design(96_000.0));
    assert_eq!(p.sample_rate(), 96_000.0);
    let mut z = vec![0.0f32; 100];
    p.process_mono(&mut z, &AudioParams::default());
    assert!(z.iter().all(|&v| v == 0.0));
    let _: Option<AudioPath> = None;
}
