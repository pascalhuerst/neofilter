//! Background analysis thread: lifecycle and hand-off.
mod common;
use common::*;
use neometer::analysis::AnalysisThread;
use neometer::audio::{AudioParams, AudioPath};
use neometer::shared::Shared;
use std::time::{Duration, Instant};

const FS: f64 = 48_000.0;

fn wait_for<T>(mut f: impl FnMut() -> Option<T>, what: &str) -> T {
    let t = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "timeout waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn thread_starts_analyses_and_joins_over_20_cycles() {
    let cascade = pre_emphasis(FS);
    let x = sine(FS, 1000.0, -20.0, 24_000);
    for cycle in 0..20 {
        let shared = Shared::standalone();
        let (thread, prod) = AnalysisThread::spawn(shared.clone(), FS, 2);
        assert!(thread.is_running());
        let mut audio = AudioPath::new(shared.clone());
        audio.prepare(FS, false, cascade, prod);
        let p = AudioParams::default();
        let (mut l, mut r) = (x.clone(), x.clone());
        for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
            audio.process_stereo(a, b, &p);
        }
        assert_eq!(l, x);
        // the thread picks the frames up (momentary loudness needs 400 ms)
        let snap = wait_for(
            || {
                let s = shared.snapshot();
                s.loudness.momentary.is_finite().then_some(s)
            },
            "loudness",
        );
        assert!(
            (snap.loudness.momentary + 19.993).abs() < 0.05,
            "cycle {cycle}"
        );
        assert!(
            snap.gonio.is_empty(),
            "idle thread must not run the goniometer"
        );
        assert!(snap.spectrum.is_none());
        let seq_before = shared.snapshot().seq;
        drop(audio);
        drop(thread); // joins
        let seq_after = shared.snapshot().seq;
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(
            shared.snapshot().seq,
            seq_after,
            "thread still publishing after drop"
        );
        assert!(seq_after >= seq_before);
    }
}

#[test]
fn open_editor_enables_goniometer_and_spectrum() {
    let cascade = pre_emphasis(FS);
    let shared = Shared::standalone();
    shared.set_force_active(true);
    let (thread, prod) = AnalysisThread::spawn(shared.clone(), FS, 2);
    let mut audio = AudioPath::new(shared.clone());
    audio.prepare(FS, false, cascade, prod);
    let p = AudioParams::default();
    let mut n = Noise(4);
    // ~4 s of audio, fed in realtime-ish bursts
    for _ in 0..10 {
        let (mut l, mut r) = (n.vec(19_200), n.vec(19_200));
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            audio.process_stereo(a, b, &p);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let snap = wait_for(
        || {
            let s = shared.snapshot();
            (!s.gonio.is_empty() && s.spectrum.is_some()).then_some(s)
        },
        "gonio + spectrum",
    );
    assert!(snap.gonio.len() <= neometer::analysis::GONIO_POINTS);
    assert_eq!(snap.spectrum.as_ref().unwrap().curves.len(), 2);
    assert_eq!(snap.overruns, 0);
    drop(audio);
    drop(thread);
}

#[test]
fn audio_thread_is_never_blocked_by_a_stalled_consumer() {
    // The analysis thread exists but the host feeds 10 s of audio in one
    // burst (faster than real time): the audio calls must return promptly and
    // the output must stay untouched; whatever does not fit is counted.
    let shared = Shared::standalone();
    let (thread, prod) = AnalysisThread::spawn(shared.clone(), FS, 2);
    let mut audio = AudioPath::new(shared.clone());
    audio.prepare(FS, false, pre_emphasis(FS), prod);
    let p = AudioParams::default();
    let mut n = Noise(8);
    let src = n.vec(4096);
    let t = Instant::now();
    for _ in 0..200 {
        let (mut a, mut b) = (src.clone(), src.clone());
        audio.process_stereo(&mut a, &mut b, &p);
        assert_eq!(a, src);
    }
    let per_block = t.elapsed() / 200;
    assert!(per_block < Duration::from_millis(100), "{per_block:?}");
    drop(audio);
    drop(thread);
}
