# Changelog

All notable changes to this project are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- 24-band parametric EQ plugin (CLAP and VST3) with Bell, Low Shelf, Low Cut,
  High Shelf, High Cut, Notch, Band Pass, Tilt Shelf and Flat Tilt shapes and
  slopes from 6 to 96 dB/oct, using Pro-Q 3 compatible parameter conventions.
- Zero-latency minimum-phase processing with analog-matched biquad cascades
  in 64-bit precision, smoothed parameters and click-free topology changes.
- Per-band stereo placement (Stereo, Left, Right, Mid, Side), output gain and
  gain scale.
- egui editor with response curve, draggable nodes and spectrum analyzer.
- Factory presets "Friend EQ A" and "Friend EQ B".
- `neofilter-cli` for response export and impulse-response comparison.
- GitHub Actions CI and tag-triggered release workflow.
