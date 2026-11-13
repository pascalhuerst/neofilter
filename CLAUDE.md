# NeoFilter suite

Three plugins (CLAP + VST3) in Rust, built on nih-plug: **NeoFilter** (32-band
parametric EQ with loop calibration), **NeoRIAA** (RIAA pre-/de-emphasis) and
**NeoMeter** (metering and measurement), on top of the `neo-measure` library.
Design and phase plan: `PLAN.md` (EQ) and `docs/PLAN-vinyl-suite.md` (suite,
with status and open items) — read them before changing DSP or params.
Other docs: `docs/calibration.md`, `docs/reference-matching.md`, crate READMEs.

## Hard rules

- **Never read, unpack, disassemble or commit anything in `orig/`.** This is a
  clean-room project: DSP comes only from published literature and black-box
  measurement of audio output.
- Never `git add -A` / `git add .` — add explicit paths. The `*.jpg` files in
  the repo root, `example_preset.ffp`, `ffp/` and `orig/` are the user's and
  stay untracked.
- Do not push. Do not rewrite history.
- No FabFilter names in code identifiers, UI strings or plugin metadata.
- Parameter ids are frozen; never rename or reorder:
  - NeoFilter: `freq_{n}`, `gain_{n}`, `q_{n}`, `used_{n}`, `on_{n}`,
    `shape_{n}`, `slope_{n}`, `place_{n}`, n = 1..32; `out_gain`,
    `gain_scale`, `bypass`. Enum value ids too (shapes `bell`, `low_shelf`,
    `low_cut`, `high_shelf`, `high_cut`, `notch`, `band_pass`, `tilt_shelf`,
    `flat_tilt`; slopes `s6`..`s96`; placements `stereo`, `left`, `right`,
    `mid`, `side`). Persisted keys: `editor-state`, `calibration`.
  - NeoRIAA: `mode` (values `pre`, `de`), `iec`, `ultrasonic`, `trim`,
    `bypass`. Persisted key: `editor-state`.
  - NeoMeter: `reference_level`, `lf_crossover`, `trim_l`, `trim_r`.
    Persisted keys: `editor-state`, `meter-settings`, `test-record`.
  - Plugin ids and VST3 class ids are frozen too (`org.neofilter.{neofilter,
    neoriaa,neometer}`); the id lists are pinned by `tests/params_tests.rs`
    (NeoFilter) and `tests/plugin_meta.rs` (NeoRIAA, NeoMeter).

## Commands

```sh
cargo test --workspace            # all tests; takes a few minutes, be patient
cargo test -p neofilter-dsp       # EQ DSP only (fast)
cargo test -p neo-measure         # measurement library
cargo test -p neoriaa
cargo test -p neometer
cargo test -p neofilter           # plugin, calibration loop tests
cargo test -p neofilter-cli
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask bundle -p neofilter -p neoriaa -p neometer --release   # target/bundled/*.{clap,vst3}
cp -r target/bundled/*.clap ~/.clap/                               # install for Bitwig
```

`cargo xtask bundle` takes one package name or repeated `-p name` (the
space-separated form `bundle a b c` is NOT valid). macOS universal builds:
`bundle-universal -p ... --release`.

## Layout

- `crates/neofilter-dsp` — pure EQ DSP, no plugin/GUI deps. Analog prototypes →
  matched z-transform / reference rule (`refmatch.rs`) → f64 TDF-II cascades;
  `autofit.rs` fits bands to a curve. All reference-compatibility conventions
  live in `src/conventions.rs` only.
- `crates/neofilter` — plugin: `params.rs`, `presets.rs`, `lib.rs` (process,
  layouts incl. the "Loop return" sidechain), `audio.rs`, `calibration/`
  (engine, settings, noise), `editor/` (egui), `analyzer.rs`.
- `crates/neofilter-cli` — list, curve, render, impulse, compare, calibrate,
  export-ffp, import-ffp.
- `crates/neo-measure` — measurement library (levels, loudness, stereo,
  spectrum, delay, transfer, tone, wow/flutter, generator, `riaa` design,
  velocity, sweep_track). No plugin deps.
- `crates/neoriaa` — RIAA plugin (`audio.rs`, `params.rs`, `editor/`); the
  design is `neo_measure::riaa`.
- `crates/neometer` — metering plugin: audio thread + analysis thread +
  egui editor; `testrec.rs` is the test-record calibration.
- `tools/reference-ab` — scripts behind the reference measurements (documentation).
- `xtask` — bundler. `.github/workflows` — CI and release.

## Coding standards

- Audio thread (`process` and everything it calls): no allocation, locks,
  I/O, or panics. Fixed-size arrays; `assert_process_allocs` feature of
  nih-plug is enabled in debug builds. Each plugin has a no-allocation test.
- Internal audio math in `f64`. No `unsafe` outside what nih-plug requires.
- Every DSP change comes with tests that compare against the analytic analog
  prototype, a measured fixture, or a stated numeric tolerance. Do not loosen
  a tolerance to make a test pass without writing down why in the test.
- Coefficient design must never return NaN/inf or unstable poles; clamp and
  fall back instead. Non-finite input samples are contained, never propagated.
- Keep `cargo clippy -- -D warnings` and `cargo fmt --check` clean.
- Commits: small, imperative subject, one logical change, tests green.

## Safety rules for generators and calibration

NeoFilter's calibration and NeoMeter's generator can send signal into a
cutting chain, so:

- A noise or test-signal generator is **never** started from saved state,
  from automation or from `initialize()`; it exists only while the user
  presses a button, is never a parameter and is never persisted. It stops on
  Stop, error, editor close, reset and deactivate. A visible indicator ("NOISE
  ON" in NeoFilter, "GENERATOR ON" in NeoMeter) must be lit whenever it is
  audible.
- NeoFilter calibration noise level is capped at -6 dBFS RMS (default -20
  dBFS pink); the correction applied by the fit is limited to +6 dB boost /
  -12 dB cut by default (enforced on the summed curve), range 30 Hz to
  16 kHz. Do not raise these defaults or remove the caps without an explicit
  decision. Safety stops (return too low / clipping / no loop / low
  coherence / bypass) must keep aborting and applying nothing.
- NeoRIAA has no limiter or clipper by design: it must not alter the signal
  except by the selected curve, trim and bypass.
- The calibration is driven by the editor; it needs the editor open and
  aborts if the capture is not drained.
