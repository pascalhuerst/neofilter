//! The audio-thread entry points must not allocate. A counting global
//! allocator (enabled per thread, so parallel tests cannot interfere) checks
//! `set_band`, `process_*`, the band redesign and the GUI response functions.
mod common;
use common::*;
use neofilter_dsp::response::{analog_band_response_db, band_response_db, total_response_db};
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, EqEngine, Placement, Shape, Slope};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if TRACK.try_with(|t| t.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if TRACK.try_with(|t| t.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocations_in<F: FnOnce()>(f: F) -> usize {
    ALLOCS.store(0, Ordering::SeqCst);
    TRACK.with(|t| t.set(true));
    f();
    TRACK.with(|t| t.set(false));
    ALLOCS.load(Ordering::SeqCst)
}

#[test]
fn engine_and_response_do_not_allocate() {
    // the counter itself works
    assert!(allocations_in(|| drop(std::hint::black_box(vec![1u8; 64]))) >= 1);

    // set up everything that is allowed to allocate first
    let fs = 48000.0;
    let mut e = EqEngine::new(fs);
    let mut l = vec![0.1; 4096];
    let mut r = vec![-0.1; 4096];
    let mut mono = vec![0.2; 4096];
    let freqs = grid(10.0, 30000.0, 500);
    let mut out = vec![0.0; 500];
    let bands: Vec<BandParams> = (0..NUM_BANDS)
        .map(|i| {
            band(
                Shape::ALL[i % 9],
                100.0 * (i + 1) as f64,
                3.0,
                2.0,
                Slope::ALL[i % 9],
            )
        })
        .collect();

    let n = allocations_in(|| {
        // many bands, all shapes, placements, topology changes, toggles
        for round in 0..6 {
            for (i, b) in bands.iter().enumerate() {
                let mut p = *b;
                p.gain_db = if round % 2 == 0 { 3.0 } else { -6.0 };
                p.slope = Slope::ALL[(i + round) % 9];
                p.placement = Placement::ALL[(i + round) % 5];
                p.enabled = (i + round) % 7 != 0;
                p.freq_hz *= 1.0 + 0.2 * round as f64;
                e.set_band(i, &p);
            }
            e.set_gain_scale(0.5 + 0.2 * round as f64);
            e.process_stereo(&mut l, &mut r);
            e.process_stereo(&mut l[..1], &mut r[..1]);
            e.process_mono(&mut mono);
        }
        e.reset();
        e.set_sample_rate(96000.0);
        for (i, b) in bands.iter().enumerate() {
            e.set_band(i, b);
        }
        e.process_stereo(&mut l, &mut r);
        // GUI thread helpers
        band_response_db(&bands[3], 1.0, fs, &freqs, &mut out);
        analog_band_response_db(&bands[3], 1.0, fs, &freqs, &mut out);
        total_response_db(&bands, 1.0, fs, &freqs, &mut out);
    });
    assert_eq!(n, 0, "{n} allocations on the audio / response paths");
}
