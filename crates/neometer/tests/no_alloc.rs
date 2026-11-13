//! The audio path must not allocate after `prepare`. nih-plug's
//! `assert_process_allocs` feature installs `assert_no_alloc`'s allocator in
//! debug builds, which aborts the process on any allocation inside
//! `assert_no_alloc(..)`; the test therefore only passes if nothing allocates.
mod common;
use assert_no_alloc::assert_no_alloc;
use common::*;
use neometer::shared::reset;
use neometer::testgen::{GenSettings, Kind, Mode, Routing};

#[test]
fn audio_path_does_not_allocate() {
    let fs = 48_000.0;
    let mut stereo = Rig::new(fs);
    let mut mono = Rig::with(fs, true);
    let mut n = Noise(9);
    let (mut l, mut r) = (n.vec(3000), n.vec(3000));
    let mut m = n.vec(3000);

    assert_no_alloc(|| {
        for round in 0..8 {
            let kinds = [Kind::Sine, Kind::Sweep, Kind::White, Kind::Pink];
            stereo.shared.gen.set(GenSettings {
                on: round % 3 != 2,
                kind: kinds[round % 4],
                routing: Routing::ALL[round % 4],
                mode: if round % 2 == 0 {
                    Mode::Replace
                } else {
                    Mode::Add
                },
                level_db: -30.0 + round as f64,
                freq_hz: 500.0 + 100.0 * round as f64,
                sweep_s: 2.0 + round as f64,
            });
            stereo.params.lf_crossover_hz = 100.0 + 50.0 * round as f64;
            stereo.shared.settings.update(|d| {
                d.corr_tau_s = [0.1, 0.3, 1.0, 3.0][round % 4];
                d.hf_corner_hz = 3000.0 + 500.0 * round as f32;
            });
            if round == 3 {
                stereo.shared.request_reset(reset::ALL);
            }
            // larger than the chunk size, odd sizes, tiny blocks
            stereo.audio.process_stereo(&mut l, &mut r, &stereo.params);
            stereo
                .audio
                .process_stereo(&mut l[..7], &mut r[..7], &stereo.params);
            stereo
                .audio
                .process_stereo(&mut l[..1], &mut r[..1], &stereo.params);
            mono.shared.gen.set(GenSettings {
                on: true,
                ..GenSettings::default()
            });
            mono.audio.process_mono(&mut m, &mono.params);
            stereo.audio.publish();
        }
    });
}
