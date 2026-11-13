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
- Bell (all slopes) and even-order High Cut bands use the digital design rule
  identified from black-box measurements of the reference EQ: five-point
  squared-magnitude interpolation per section (`refmatch.rs`). Against 198
  measured single-band cases the worst deviation is 0.017 dB (Bell 0.006 dB,
  median 0.0001 dB); the factory presets match the measured total curves to
  0.016 dB or better. See `docs/reference-matching.md`. Bells and even-order
  High Cuts therefore deviate from the analog prototype near Nyquist on
  purpose; `tests/accuracy.rs` compares them with it only for band
  frequencies up to 0.02 fs.
- Default-Q 12 dB/oct Low and High Shelf bands use the measured five-point
  rule and match the measured curves to 0.001 dB; the shelf resonance law is
  now section Q = Butterworth Q x displayed Q^0.47 (measured fit). Other
  shelves, 24 dB/oct shelves and tilt shelves match the measurements to
  0.01 dB up to 0.1 x the sample rate (`reference_shelves.rs`, 58 cases).
- Bell sections switch to the upper magnitude extremum only when the lower one
  is insignificant; this fixes a 1.7 dB error for some 48 dB/oct bells.
- NeoFilter loop calibration: optional stereo sidechain input "Loop return"
  (new first audio layout; plain stereo and mono layouts remain), calibration
  engine (delay by cross-correlation, transfer function with coherence, fit
  with `neofilter-dsp::autofit`), the Calibrate panel with Measure / Fit /
  Apply / Auto / Stop, the "NOISE ON" indicator and overlay curves,
  playback-curve paste, safety stops (pink noise default -20 dBFS and hard cap
  -6 dBFS, +6 dB boost / -12 dB cut limits, 30 Hz to 16 kHz). Noise never
  starts from saved state or automation. See `docs/calibration.md`.
- `neofilter-cli calibrate`: the same analysis and fit offline on recordings.
- `neofilter-dsp::autofit`: fits a target curve with bells and optional
  shelves.
- **NeoRIAA** plugin (CLAP + VST3): RIAA pre-emphasis (default) and
  de-emphasis, IEC 7950 us option, 3.18 us ultrasonic pole option, output
  trim, peak meter and clip indicator, soft bypass, egui editor. Magnitude
  within about 0.007 dB of the analytic curve over 20 Hz to 20 kHz at 44.1 to
  384 kHz (see its README for the table).
- **neo-measure** library: levels (sample/true peak, RMS, crest, DC),
  BS.1770-4 loudness, stereo meters, Welch spectrum and 1/3-octave bands,
  delay estimation, H1 transfer function with coherence, tone/THD analysis,
  wow and flutter, A-weighting, generators, RIAA curves and digital design
  (`neo_measure::riaa`, precomputed designs for common rates plus a bounded
  quick design), velocity conversions and sweep level tracking.
- **NeoMeter** plugin (CLAP + VST3): pages Levels & Loudness, Stereo,
  Spectrum, Vinyl, Test and Test record; input trims `trim_l` / `trim_r`;
  pre-emphasis options (IEC, ultrasonic pole) on the Vinyl page; the
  generator never starts from saved state. The Test record page calibrates
  the playback input with a test record (reference band levels, balance,
  crosstalk, playback-response curve that NeoFilter's calibration can paste,
  wow & flutter, tracking), with a built-in Ortofon Test Record profile.

### Changed

- **Tilt Shelf gain convention** (measured): a tilt shelf is a high shelf of
  twice the gain shifted down by the gain, so +6 dB now runs from -6 dB at low
  frequencies to +6 dB at high ones (previously -3 to +3 dB). Saved sessions
  and presets that use Tilt Shelf bands will sound different.
- Shelf resonance law corrected to the measured Q^0.47 (above).
- The repository is now a suite: the README, plans, CLAUDE.md, CI and release
  workflows cover three plugins; releases ship NeoFilter, NeoRIAA and
  NeoMeter bundles together.

### Known limitations

- Low Cut, Notch, Band Pass, Flat Tilt, odd-order cuts, non-default-Q or
  24 dB/oct shelves and tilt shelves near Nyquist (above 0.1 x the sample
  rate) still use the analog-matched design, which does not meet the blanket
  0.1 dB / 0.5 dB target of the original plan: high-Q shelves/cuts, wide
  notches and 96 dB/oct band passes near Nyquist deviate by several dB from
  the analog prototype, and these cases have not been compared with the
  reference EQ. The enforced tolerances are in
  `crates/neofilter-dsp/tests/accuracy.rs`.
- Reference measurements cover 44.1, 48 and 96 kHz only.
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
- Nothing has been verified in a real host beyond `clap-validator` on
  NeoFilter early in development. The GUIs of all three plugins were not
  reviewed visually.
- Loop calibration needs the NeoFilter editor open while it measures, has
  only been exercised with simulated loops, and corrects magnitude only.
- NeoRIAA: magnitude is accurate, but phase misses the planned targets below
  176.4 kHz (71 degrees worst case at 44.1 kHz, 61 at 48 kHz, 11 to 14 at 88.2
  and 96 kHz without the ultrasonic pole). The digital pre-emphasis runs a few
  dB higher than the analog curve near Nyquist.
- Wow & flutter weighting is a fit to the standard's curve and its quasi-peak
  ballistics are unverified; W&F from a 1 kHz carrier is indicative only.
- Loudness and true peak are verified with synthesised equivalents of the EBU
  Tech 3341/3342 cases, not the official files.
- The Ortofon Test Record profile uses values from public listings, not from
  the sleeve. The record has no sweep below 800 Hz, so the playback chain is
  unverified there.
