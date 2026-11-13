# Changelog

All notable changes to this project are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- 32-band parametric EQ plugin (CLAP and VST3) with Bell, Low Shelf, Low Cut,
  High Shelf, High Cut, Notch, Band Pass, Tilt Shelf and Flat Tilt shapes and
  slopes from 6 to 96 dB/oct, using Pro-Q 3 compatible parameter conventions.
- Zero-latency minimum-phase processing with analog-matched biquad cascades
  in 64-bit precision, smoothed parameters and click-free topology changes.
- Per-band stereo placement (Stereo, Left, Right, Mid, Side), output gain and
  gain scale.
- egui editor with response curve, draggable nodes, selected-band strip,
  spectrum analyzer, and "Copy preset" / "Paste preset" (JSON via clipboard).
- Factory presets "Default", "Friend EQ A", "Friend EQ B" and "Friend EQ A+B
  (series)" (A bands 1 to 15, then B bands 2, 4, 5, 6, 8, 9, 10, 11, 12 as
  bands 16 to 24, 8 spare bands): one instance replaces two in series.
- Band count raised from 24 to 32. Parameter ids for bands 1 to 24 are
  unchanged and the new bands are `*_25` to `*_32`, so saved state from the
  24-band plugin loads with bands 25 to 32 at their defaults (unused). Preset
  JSON accepts band numbers 1 to 32.
- Audio path with smoothed output gain and a click-free 10 ms soft bypass;
  host buffers of any size are processed without allocation.
- `neofilter-cli` with `list`, `curve`, `render`, `impulse` and `compare`
  commands (preset ids or preset JSON files).
- GitHub Actions CI and tag-triggered release workflow.

- `neofilter-cli export-ffp` / `import-ffp`: convert presets to and from
  `.ffp` preset files (bands only; 24 bands, Stereo placement).

### Known limitations

- The matched digital filters do not meet the blanket 0.1 dB / 0.5 dB target
  of the original plan: bells in the everyday range stay within about 0.23 dB
  up to 16 kHz at 44.1 kHz, but high-Q shelves/cuts, wide notches and
  96 dB/oct band passes near Nyquist deviate by several dB. The enforced
  tolerances are in `crates/neofilter-dsp/tests/accuracy.rs`.
- While a parameter is automated, the design may switch between fit candidates
  for single steps. In the useful range (Q 0.3 to 20, up to 8 kHz) this
  changes the response by at most about 0.1 dB at one frequency. For a Notch
  or Low Cut with Q below 0.3 and for anything beyond 0.4 x sample rate, such
  single-step jumps of several dB occur above 15 kHz
  (`crates/neofilter-dsp/tests/continuity.rs`).
- Sweeping the frequency of a very steep cut (96 dB/oct) across a signal
  rings like any fast-swept high-order filter; this is not a click but can be
  loud with high Q.
- "Paste preset" needs the editor window to receive the Ctrl+V key press;
  hosts that keep the keyboard focus for themselves will not deliver it.
- Not yet verified in a real host or against measurements of the reference EQ.
