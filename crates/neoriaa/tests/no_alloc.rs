//! The audio path must not allocate after construction. nih-plug's
//! `assert_process_allocs` feature installs `assert_no_alloc`'s allocator in
//! debug builds, which aborts the process on any allocation inside
//! `assert_no_alloc(..)`; the test therefore only passes if nothing allocates.
mod common;
use assert_no_alloc::assert_no_alloc;
use common::*;
use neo_measure::riaa::{Mode, Options};

#[test]
fn audio_path_does_not_allocate() {
    let (mut path, meters) = path(48_000.0);
    let mut n = Noise(9);
    let (mut l, mut r) = (n.vec(3000, 0.5), n.vec(3000, 0.5));
    let mut m = n.vec(3000, 0.5);
    assert_no_alloc(|| {
        for round in 0..8 {
            let p = neoriaa::audio::AudioParams {
                mode: if round % 2 == 0 {
                    Mode::PreEmphasis
                } else {
                    Mode::DeEmphasis
                },
                options: Options {
                    iec: round % 3 == 0,
                    ultrasonic: round % 4 < 2,
                },
                trim_db: round as f64 - 4.0,
                bypass: round % 3 == 1,
            };
            path.process_stereo(&mut l, &mut r, &p);
            path.process_mono(&mut m[..100], &p);
            path.process_stereo(&mut l[..7], &mut r[..7], &p);
            if round == 3 {
                path.reset();
                meters.reset();
            }
            let _ = (meters.peak(), meters.hold(), meters.clipped());
        }
    });
}
