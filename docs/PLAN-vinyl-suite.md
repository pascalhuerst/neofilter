# Vinyl cutting suite — plan and status

**Status: implemented.** All three parts exist and are tested; what deviated
from this plan and what is still open is listed under
[Deviations](#deviations-from-the-plan) and
[Open items and future work](#open-items-and-future-work) at the end. The text
below is the original plan, annotated with "As built" where it differs.

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
crates/neoriaa/       RIAA plugin (cdylib) — filter design lives in neo_measure::riaa (as built)
crates/neometer/      metering plugin (cdylib, egui)
crates/neofilter-dsp/ + autofit.rs (fit EQ bands to a target curve)
crates/neofilter/     + sidechain input, calibration engine and GUI panel
```

Bundles: `cargo xtask bundle -p neofilter -p neoriaa -p neometer --release`
(as built: nih_plug_xtask takes either one package name or repeated `-p`; the
plan's space-separated form is not valid).
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
- As built: the design is `neo_measure::riaa` (shared with NeoMeter), a
  Levenberg-Marquardt fit of a minimum-phase biquad cascade with
  precomputed tables for 44.1 to 384 kHz and a bounded-time quick design for
  other rates; pre-emphasis is the exact inverse of the fitted de-emphasis.
- Accuracy targets (tests against the analytic curve, 20 Hz–20 kHz). **As
  built: magnitude meets the targets at every rate (worst about 0.007 dB);
  phase meets them only from 176.4 kHz** (worst 71 degrees at 44.1 kHz, 61 at
  48 kHz, 11 to 14 at 88.2 / 96 kHz without the ultrasonic pole). This is a
  property of stable minimum-phase IIR filters (magnitude fixes phase), not
  an optimiser shortfall; see `crates/neoriaa/README.md`. Original targets:
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
    As built: the weighting filter is a fit (peak 4 Hz, -6 dB at 0.8 and
    20 Hz) because the tabulated mask was not available, and the quasi-peak
    ballistics are unverified; use for comparison only.
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

As built, beyond the plan: input trims (`trim_l`, `trim_r`) and a sixth page,
**Test record** (input calibration with a test record: reference level,
balance, crosstalk, playback sweep response with an exportable curve for
NeoFilter's calibration, speed and wow & flutter, tracking ability; built-in
Ortofon Test Record profile from public listings, unverified against the
sleeve), with `neo_measure::{velocity, sweep_track}` behind it. IEC and
ultrasonic pre-emphasis options on the Vinyl page.

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
- As built: the calibration is driven by the editor's poll (it needs the
  editor open while measuring; closing it aborts and stops the noise); the
  noise level is hard-capped at -6 dBFS, and the noise generator never starts
  from saved state or automation. Details: `docs/calibration.md`.
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

## Deviations from the plan

- Bundling several plugins: `cargo xtask bundle -p neofilter -p neoriaa -p
  neometer --release`, not space-separated names.
- RIAA design lives in `neo_measure::riaa`, not `neofilter-dsp::riaa`, so
  NeoRIAA and NeoMeter share it.
- RIAA phase targets are not met below 176.4 kHz (magnitude is).
- Wow & flutter weighting is a fit, not the tabulated standard mask.
- Calibration is driven by the editor poll rather than a background thread
  inside the plugin; it requires the editor open.
- NeoMeter gained input trims and the Test record page; NeoFilter's
  calibration can use the playback curve it exports.
- Tilt shelf convention and shelf resonance law of NeoFilter were corrected
  to measured behaviour (see `docs/reference-matching.md`).
- The plan's "EBU test signals within 0.1 LU" is met by synthesised
  equivalents, not the official files.

## Open items and future work

Verification
- Verify all three plugins in a real host (state restore, sidechain routing,
  automation, editor input). Only `clap-validator` on NeoFilter was run, early
  on. Re-run it on all three bundles.
- Review the three editors visually; the painting code was written without
  being rendered.
- Run the loop calibration on a real lathe loop, with the cutter's protection
  active.
- Check the Ortofon profile against the sleeve; measure a record with a sweep
  below 800 Hz to verify the playback chain there.
- Run the official EBU Tech 3341/3342 files.
- Verify W&F weighting and ballistics against the standard (obtain the
  tabulated mask) and use a 3150 Hz carrier.

DSP
- NeoRIAA phase at 44.1 to 96 kHz (needs a different approach, for example an
  optional linear-phase or all-pass-compensated mode, at the price of
  latency).
- Identify the reference rule for the remaining NeoFilter shapes (Low Cut,
  Notch, Band Pass, odd-order cuts, Flat Tilt, other shelves near Nyquist) and
  measure at more sample rates.
- Calibration: phase correction, radius-dependent HF loss, running without the
  editor open (background driver).

Product
- Linear-phase and natural-phase modes, dynamic EQ, brickwall slope, EQ match,
  band solo, auto-gain, surround (NeoFilter, out of scope for v0.1).
- `.ffp` export of more than 24 bands or non-Stereo placement is not possible
  with the format as understood; unverified shape and slope codes.
- Signing and notarisation of macOS builds.
