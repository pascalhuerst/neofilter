//! The audio path must not allocate after `prepare`. nih-plug's
//! `assert_process_allocs` feature installs `assert_no_alloc`'s allocator in
//! debug builds, which aborts the process on any allocation inside
//! `assert_no_alloc(..)`; the test therefore only passes if nothing allocates.
mod common;
use assert_no_alloc::assert_no_alloc;
use common::*;
use neofilter::audio::{AudioParams, AudioPath};

#[test]
fn audio_path_does_not_allocate() {
    let a = preset("friend-eq-a");
    let b = preset("friend-eq-b");
    let mut n = Noise(9);
    let (mut l, mut r) = (n.vec(3000), n.vec(3000));
    let mut m = n.vec(3000);
    let mut path = AudioPath::new(48000.0, 512);

    assert_no_alloc(|| {
        for round in 0..6 {
            let bands = if round % 2 == 0 { &a } else { &b };
            let p = AudioParams {
                bands,
                gain_scale: 1.0 - 0.1 * round as f64,
                out_gain_db: round as f64,
                bypass: round % 3 == 1,
            };
            path.process_stereo(&mut l, &mut r, &p); // larger than max block: chunked
            path.process_mono(&mut m[..100], &p);
            path.process_stereo(&mut l[..7], &mut r[..7], &p);
            if round == 3 {
                path.reset();
            }
        }
    });
}
