# Vinyl cutting suite — plan

Three additions to the NeoFilter repository for calibrating and running a
vinyl cutting chain:

1. **NeoRIAA** — accurate RIAA pre-emphasis ("inverse RIAA") plugin.
2. **NeoMeter** — metering / measurement plugin for mastering and cutting.
3. **Auto-calibration** inside NeoFilter — measure the cut→playback loop and
   fit EQ bands for a flat response.

Signal chain assumed:

```
program ─► NeoFilter (EQ + calibration) ─► NeoRIAA (pre-emphasis) ─► cutter amp/head
                 ▲ sidechain                                              │ groove
                 └──────────── phono preamp ◄── playback stylus ◄─────────┘ (~1/4 rev later)
```

## Workspace layout (new)

```
crates/neo-measure/   pure analysis DSP (no plugin deps): meters, spectra, transfer function, delay
crates/neoriaa/       RIAA plugin (cdylib) — filter design lives in neofilter-dsp::riaa
crates/neometer/      metering plugin (cdylib, egui)
crates/neofilter-dsp/ + riaa.rs (RIAA design), + autofit.rs (fit EQ bands to a target curve)
crates/neofilter/     + sidechain input, calibration engine and GUI panel
```

Bundles: `cargo xtask bundle neofilter neoriaa neometer --release`.
Same rules as the rest of the repo (CLAUDE.md): realtime-safe audio thread,
f64 internal math, tests with stated tolerances, no product names in code.

## 1. NeoRIAA

Standard RIAA time constants: T1 = 3180 µs, T2 = 318 µs, T3 = 75 µs.

- Playback (de-emphasis): `(1 + s·T2) / ((1 + s·T1)(1 + s·T3))`.
- Recording (pre-emphasis, "inverse RIAA"): the reciprocal. It is improper
  (rises 6 dB/oct forever), +19.6 dB at 20 kHz relative to 1 kHz.
- Options: mode (Pre-emphasis for cutting = default / De-emphasis for
  playback), IEC 7950 µs low-frequency roll-off (off by default), 3.18 µs
  "Neumann" ultrasonic pole for the pre-emphasis (off by default; when off the
  digital filter still has to stop rising at Nyquist), 0 dB reference at
  1 kHz, output trim (−36..+12 dB), clip indicator and peak meter (the
  pre-emphasis adds up to ~20 dB at HF).
- Digital design, per sample rate, computed at `initialize()` (not on the
  audio thread): design the minimum-phase de-emphasis filter as an IIR whose
  complex response matches the analog one, then invert it for pre-emphasis
  (so pre∘de is exactly unity). Start from matched poles/zero plus a
  numerator fit; add correction section(s) fitted by least squares on the
  complex response until the targets are met.
- Accuracy targets (tests against the analytic curve, 20 Hz–20 kHz):
  magnitude ≤ 0.01 dB and phase ≤ 0.5° at 88.2 kHz and above; ≤ 0.02 dB and
  ≤ 2° at 44.1/48 kHz. Report the achieved numbers per rate in the README;
  if a target cannot be met at 44.1 kHz, say by how much instead of
  loosening silently. Published check values: +19.274 dB @ 20 Hz,
  0 dB @ 1 kHz, −13.734 dB @ 10 kHz, −19.620 dB @ 20 kHz (de-emphasis).
- Zero latency, mono/stereo, f64, state-safe at any rate 44.1–384 kHz.

## 2. neo-measure (library) and NeoMeter (plugin)

Library (all tested against synthetic signals with known answers):

- **Levels**: sample peak, true peak (ITU-R BS.1770-4, ≥ 4× oversampling,
  error ≤ 0.1 dB on the standard's test cases), RMS (configurable window),
  crest factor, DC offset.
- **Loudness**: BS.1770-4 K-weighting (coefficients derived for any sample
  rate), momentary (400 ms), short-term (3 s), integrated with absolute and
  relative gating, loudness range (EBU Tech 3342). Verified with the EBU
  Tech 3341/3342 style test signals synthesised in tests (±0.1 LU).
- **Stereo**: correlation (−1..+1, selectable integration time), M/S levels,
  balance, goniometer point buffer.
- **Spectrum**: Welch averaged FFT (up to 65536 points, Hann / Blackman-Harris
  / flat-top), exponential or infinite averaging, peak hold, fractional-octave
  smoothing (1/1 … 1/48), and IEC 61260-style 1/3-octave band levels; also
  per-band L, R, M, S spectra.
- **Vinyl-specific**:
  - Vertical (side, L−R) vs lateral (mid, L+R) level, broadband and below a
    selectable frequency (default 300 Hz) — vertical bass is what lifts the
    stylus or cuts through the lacquer.
  - Low-frequency phase correlation (band-limited below 300 Hz).
  - Levels after RIAA pre-emphasis (what the cutter head sees): peak, RMS
    and a high-frequency band level (≥ 5 kHz) as a cutter-head load /
    sibilance indicator.
  - Recorded velocity estimate in cm/s given a calibration (default
    0 dBFS-reference configurable: reference level = 5 cm/s peak lateral at
    1 kHz), and displacement estimate (integrated velocity) with peak hold.
- **Measurement** (signal-referenced):
  - Delay estimation by generalised cross-correlation (search up to 5 s,
    sub-sample interpolation), with a confidence value.
  - Transfer function estimate H1 = Sxy/Sxx with Welch averaging, magnitude,
    phase (delay-compensated), and coherence.
  - Tone analysis: frequency (interpolated), level, THD and THD+N.
  - Wow & flutter from a 3150 Hz (or 3000 Hz) tone: FM demodulation, speed
    deviation in %, unweighted and DIN/IEC 386 weighted peak (2σ) values.
  - Channel separation (crosstalk) per channel with a tone on one channel.
  - Rumble / noise: unweighted and A-weighted levels relative to a reference.
- **Generator**: sine, log sweep, white and pink noise (seedable), dual-tone,
  with level in dBFS; used by NeoMeter's generator and by the calibration.

NeoMeter plugin (pass-through, stereo; egui, resizable): pages
*Levels & Loudness*, *Stereo* (goniometer, correlation, M/S, LF vertical),
*Spectrum* (hi-res, RTA, smoothing, hold, averaging), *Vinyl* (pre-emphasised
levels, velocity/displacement, vertical bass), *Test* (generator + tone, THD,
wow & flutter, crosstalk, noise). Audio thread only fills lock-free buffers;
analysis runs on the GUI side or a background thread. Reset buttons, numeric
readouts with 0.01 dB resolution, selectable reference level.

## 3. Auto-calibration in NeoFilter

- New optional stereo **sidechain input** = the playback return.
- Calibration panel in the editor. Steps:
  1. **Measure**: NeoFilter replaces/mixes its input with internally
     generated noise (pink by default, −20 dBFS default, selectable
     white/pink, level, channel routing: L, R, L+R mono; mono lateral is the
     default). The noise passes through the current EQ like programme.
  2. The engine records the pre-EQ noise and the sidechain return, estimates
     the loop **delay** (cross-correlation; the quarter-turn playback delay
     plus converter latency, up to 5 s) and the **transfer function**
     (averaged, with coherence), per channel.
  3. **Fit**: target correction = −(measured response) relative to its mean
     level in a reference band (default 300 Hz–3 kHz), smoothed (default
     1/6 octave), restricted to a frequency range (default 30 Hz–16 kHz) and
     to frequencies with coherence ≥ 0.7, and limited to a maximum boost
     (default +6 dB) and cut (default −12 dB). Because the noise went through
     the current EQ, the new EQ curve = current EQ curve + target correction.
     `neofilter-dsp::autofit` fits that curve with up to N bands (default 12;
     bells plus optional low/high shelf, 12 dB/oct, Q limits 0.3–10) using
     greedy peak picking followed by nonlinear least-squares refinement on the
     real digital band responses.
  4. **Apply** writes the fitted bands into the bands reserved for
     calibration (default: all bands that are unused when calibration starts;
     existing manual bands are left alone unless "replace all" is chosen),
     through the normal parameter path (host undo/automation sees it).
  5. Repeat Measure → Fit → Apply until the residual is within tolerance;
     the panel shows measured response, coherence, target, fitted curve,
     predicted residual, delay and RMS deviation per iteration.
- An optional **Auto** mode runs steps 1–5 in a loop with a settle time
  after each Apply (default 2× measured delay + 1 s) and stops when the RMS
  deviation in range is below a threshold (default 0.5 dB) or after a
  maximum number of iterations (default 5).
- Safety: noise is never started without the user pressing Measure/Auto;
  boost limit always applies; a level warning is shown if the return clips or
  is too low; measurement stops if coherence collapses (stylus lifted).
- Offline path: `neofilter-cli calibrate --sent sent.wav --return ret.wav
  [--current preset.json]` runs the same delay / transfer-function / fit code
  on recordings and prints the fitted preset JSON — usable for testing and
  for setups where a live loop is not possible.

Tests: synthetic loop (known delay incl. fractional, known "system" EQ built
from NeoFilter bands plus noise) → delay recovered within 1 sample, response
within 0.2 dB (coherent range), fitted correction flattens the loop to
≤ 0.5 dB RMS / ≤ 1 dB max in range within 3 iterations; boost limit respected;
low-coherence regions untouched.

## Phases

1. Skeleton crates and workspace wiring.
2. In parallel: `neo-measure` library · RIAA design + NeoRIAA plugin.
3. In parallel: NeoMeter plugin · auto-calibration in NeoFilter (autofit,
   sidechain, panel, CLI).
4. CI/release workflows for three bundles, README/docs, independent review,
   plugin validation, local install.
