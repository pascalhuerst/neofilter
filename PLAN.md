# NeoFilter — implementation plan

NeoFilter is a 24-band parametric EQ plugin (CLAP + VST3) written in Rust for
Linux (Bitwig first), whose band parameters (Shape / Frequency / Gain / Q /
Slope) use the same conventions as FabFilter Pro-Q 3 so that settings made
there can be typed in and give the same curve.

## Ground rules

- **Clean-room.** The proprietary installers in `orig/` are never unpacked,
  disassembled or otherwise inspected, and never committed. All DSP comes from
  published literature (see References). Behavioural equivalence is verified
  only by black-box measurement (impulse responses rendered by a licensed
  user), see "Calibration".
- **Sound quality first.** f64 internal processing, analog-matched
  ("decramped") filter responses up to Nyquist, click-free parameter changes,
  no allocation/locks/syscalls on the audio thread.
- Not affiliated with FabFilter; no FabFilter names in the UI or plugin
  metadata. README may say "parameter-compatible with Pro-Q 3 settings".

## Scope

In scope (v0.1):

- 24 bands; per band: used, enabled, shape, frequency (10 Hz–30 kHz), gain
  (±30 dB), Q (0.025–40), slope, stereo placement (Stereo / Left / Right /
  Mid / Side).
- Shapes: Bell, Low Shelf, Low Cut, High Shelf, High Cut, Notch, Band Pass,
  Tilt Shelf, Flat Tilt.
- Slopes: 6, 12, 18, 24, 30, 36, 48, 72, 96 dB/oct.
- Zero-latency (minimum-phase, analog-matched) processing mode.
- Global: output gain (±36 dB), gain scale (0–200 %), bypass (host-mapped).
- GUI (egui): response curve, draggable band nodes, selected-band strip,
  pre/post spectrum analyzer, factory preset menu.
- Factory presets "Friend EQ A" and "Friend EQ B" (tables below).
- `neofilter-cli`: prints/exports the response curve of a preset and compares
  it against a measured impulse-response WAV (calibration).
- CI + release pipelines for GitHub Actions, README.

Out of scope (v0.1, keep the architecture open for them): linear-phase and
natural-phase modes, dynamic EQ, brickwall slope, EQ match, band solo,
auto-gain, surround.

## Repository layout

```
Cargo.toml                 workspace
crates/neofilter-dsp/      pure DSP library, no plugin deps, heavily tested
crates/neofilter/          the plugin (nih-plug): params, process, presets, editor
crates/neofilter-cli/      response export + IR comparison tool
xtask/                     nih_plug_xtask (cargo xtask bundle neofilter --release)
.github/workflows/         ci.yml, release.yml
```

Framework: `nih-plug` (git dependency, pinned rev) with `nih_plug_egui`.
Exports CLAP and VST3. License GPL-3.0-or-later (required by the VST3
bindings nih-plug uses).

## DSP design

### Pipeline per band

```
display params ──► analog prototype (cascade of 1st/2nd-order s-domain sections)
               ──► matched z-transform per section (decramped biquads)
               ──► TDF-II biquad cascade, f64
```

Everything is designed as an analog prototype first; the same prototype is
used (a) for the audio coefficients and (b) evaluated analytically as the
ground truth in tests. The GUI curve is drawn from the *digital* coefficients
so it shows what is actually heard.

### Parameter conventions (single source of truth: `neofilter-dsp/src/conventions.rs`)

These constants encode the Pro-Q-compatible conventions. They are based on
public documentation/measurements and are the only things that need touching
if calibration shows a deviation.

- `Q_DISPLAY_TO_STD = 1/sqrt(2)`: displayed Q 1.0 == textbook Q 0.7071
  (Q 1.0 on a 12 dB/oct cut is Butterworth).
- Frequency of shelves = half-gain (midpoint) frequency. Frequency of cuts =
  −3 dB point at display Q 1.
- Slope → order:
  - Cuts, shelves, tilt: order N = slope / 6.
  - Bell, notch, band pass: low-pass prototype order M = max(1, slope / 12)
    (36 dB/oct → M = 3), band order 2M. 6 dB/oct behaves as 12.
- Gain scale multiplies the dB gain of every Bell/Shelf/Tilt band.

### Analog prototypes

Notation: G = linear gain, w0 = 2*pi*f, Qs = Q_display * Q_DISPLAY_TO_STD,
Butterworth dampings for order N: `d_k = 2*cos(theta_k)` of the pole pairs,
plus one real pole if N is odd.

- **Low/High Cut, order N**: Butterworth. Q control: the pole pair with the
  highest Q has its Q multiplied by `Q_display` (so Q_display = 1 is exactly
  Butterworth; for N = 2 the section Q is Qs). N = 1: Q has no effect.
- **Low/High Shelf, order N**: symmetric Butterworth shelf. With
  g = G^(1/N), low-shelf sections (normalised to w0 = 1):
  - 2nd order: `(s^2 + d_k*sqrt(g)*s + g) / (s^2 + d_k*s/sqrt(g) + 1/g)` … scaled so
    that DC gain of the whole cascade is G, HF gain 1 and |H(j w0)| = sqrt(G)
  - 1st order: `(s + sqrt(g)) / (s + 1/sqrt(g))`
  - High shelf = low shelf of gain 1/G times G. Q control as for cuts (applied
    to both numerator and denominator damping of the highest-Q section).
- **Tilt Shelf**: low shelf of gain 1/G cascaded with constant gain sqrt(G)
  (positive gain lifts highs by G/2 dB and lowers lows by G/2 dB). Order from slope.
- **Flat Tilt**: constant dB/octave tilt pivoting at f, approximated by a
  cascade of 1st-order shelves spaced over 10 Hz–30 kHz (≤ 8 sections,
  ripple < 0.1 dB over 20 Hz–20 kHz). Slope ignored.
- **Bell, prototype order M**: Butterworth low-shelf prototype of order M and
  gain G (above), then low-pass→band-pass transform
  `s → Qs * (s/w0 + w0/s)`. M = 1 gives exactly the classic peaking filter
  `(s^2 + s*w0*sqrt(G)/Qs + w0^2) / (s^2 + s*w0/(sqrt(G)*Qs) + w0^2)`.
  Each 2nd-order prototype section becomes a 4th-order section which is
  factored into two biquads (closed-form complex-root factorisation).
- **Notch**: bell prototype in the limit G → 0 (zeros on the jw axis).
- **Band Pass**: Butterworth low-pass of order M through the same LP→BP transform.

Maximum per band: 8 biquads (fixed-size array, no allocation).

### Matched (decramped) z-transform

Per analog section (Vicanek, "Matched Second Order Digital Filters", 2016,
generalised):

- Poles: impulse-invariant mapping `p → exp(p/fs)` (exact pole frequency/Q).
- Zeros: solve for the numerator so that the digital magnitude equals the
  analog magnitude at DC, at Nyquist and at a third frequency near w0
  (use the phi-basis formulation; choose the minimum-phase solution).
- 1st-order sections: matched pole, numerator fitted at DC and Nyquist.
- Guard rails: if the 3-point fit has no real solution (can happen for
  extreme settings very close to Nyquist), fall back in this order: move the
  third match point; then bilinear transform with prewarping. Never produce
  NaN or unstable coefficients. Frequencies ≥ 0.49*fs are clamped.

Accuracy targets (tests): for all shapes/slopes, |digital − analog| magnitude
≤ 0.1 dB from 10 Hz to 0.4*fs and ≤ 0.5 dB up to 0.45*fs at 44.1/48/96 kHz
for band frequencies ≤ 16 kHz (document any setting that cannot meet this).

### Runtime engine

- `Biquad` TDF-II, f64 state; `BandFilter` = up to 8 sections x 2 channels;
  `EqEngine` = 24 bands in series.
- Stereo placement: Stereo processes L/R; Left/Right only that channel;
  Mid/Side bands process M = (L+R)/2 or S = (L−R)/2 with exact reconstruction.
  Mono buses: placement ignored.
- Smoothing: freq (log domain), gain (dB) and Q (log) are smoothed (~20 ms);
  while smoothing is active coefficients are recomputed every 32 samples.
- Band enable / used toggles and topology changes (shape, slope, placement)
  are click-free: 10 ms equal-gain crossfade between old and new band state.
- Denormals: flush-to-zero via nih-plug's FTZ guard plus tiny DC-free
  anti-denormal handling in the biquad if needed.
- Coefficient design must be allocation-free and callable from the audio
  thread. `reset()` clears state. Sample-rate change redesigns everything.
- Zero latency reported to the host.

## Plugin layer

- Params: `#[nested(array, group = "Band N")]` band params with stable ids
  `b{n}_used`, `b{n}_on`, `b{n}_shape`, `b{n}_freq`, `b{n}_gain`, `b{n}_q`,
  `b{n}_slope`, `b{n}_place` (n = 1..24) + `out_gain`, `gain_scale`,
  `bypass`. Ids are frozen once released.
- Freq: skewed (log) range, Hz/kHz formatting; gain dB; Q 2 decimals.
- Audio layouts: stereo→stereo, mono→mono. `f32` buffers converted to f64
  internally. Sample-accurate automation enabled.
- Factory presets: Rust consts in `presets.rs`; GUI menu applies them through
  `ParamSetter` (so host undo/automation sees them). Unused bands are reset.
  Band indices in presets equal the band numbers in the tables below.
- Analyzer: lock-free ring/triple buffer from audio thread → GUI, FFT
  (`realfft`) computed on the GUI side, pre and post spectrum.

## GUI (egui, resizable, default 900x560)

- Log-frequency grid 10 Hz–30 kHz, dB scale ±30/±12/±6 selectable.
- Sum curve (digital response, incl. gain scale), selected band curve filled.
- Nodes: drag = freq/gain, wheel = Q, double-click empty = add band,
  double-click node = reset/remove, right-click = shape/slope/placement menu.
- Bottom strip for the selected band: shape, slope, freq, gain, Q, placement,
  enable, remove. Text entry on all values (to type in a friend's numbers).
- Top bar: preset menu, gain scale, output gain, analyzer toggle.

## Factory presets

### Friend EQ A

| Band | Shape | Freq (Hz) | Gain (dB) | Q | Slope (dB/oct) | On |
|---|---|---|---|---|---|---|
| 1 | Bell | 41.1 | +2.58 | 1.00 | 12 | yes |
| 2 | High Shelf | 722.9 | −1.32 | 1.00 | 12 | yes |
| 3 | Bell | 98.0 | −1.83 | 2.73 | 24 | yes |
| 4 | Bell | 35.9 | +2.54 | 2.75 | 24 | yes |
| 5 | Bell | 9729 | +2.46 | 3.24 | 48 | yes |
| 6 | Bell | 233.0 | −2.50 | 1.00 | 12 | yes |
| 7 | Low Cut | 28.6 | – | 1.00 | 12 | yes |
| 8 | Bell | 177.7 | −5.38 | 0.68 | 12 | yes |
| 9 | Bell | 2093 | −2.99 | 1.25 | 48 | yes |
| 10 | Bell | 5074 | +4.90 | 10.91 | 24 | yes |
| 11 | High Cut | 4657 | – | 1.00 | 6 | **no** |
| 12 | Bell | 392.7 | −2.87 | 2.97 | 12 | yes |
| 13 | Bell | 2035 | +2.78 | 5.00 | 48 | yes |
| 14 | Bell | 11787 | +2.45 | 4.59 | 48 | yes |
| 15 | Bell | 15484 | +15.69 | 7.23 | 24 | yes |

### Friend EQ B ("Part 1", 9 active bands; bands 1, 3, 7 unused)

| Band | Shape | Freq (Hz) | Gain (dB) | Q | Slope (dB/oct) |
|---|---|---|---|---|---|
| 2 | High Cut | 13607 | – | 1.00 | 48 |
| 4 | Bell | 4317 | +10.58 | 1.05 | 24 |
| 5 | Bell | 85.7 | +7.62 | 1.77 | 24 |
| 6 | Bell | 701.3 | −7.36 | 2.01 | 12 |
| 8 | Bell | 15959 | +8.51 | 14.14 | 48 |
| 9 | Bell | 13195 | −4.62 | 10.00 | 36 |
| 10 | Bell | 6282 | −3.54 | 10.00 | 36 |
| 11 | Bell | 10011 | +2.64 | 5.95 | 12 |
| 12 | Bell | 165.7 | +2.06 | 2.97 | 48 |

## Calibration (black-box, optional but recommended)

The Q scaling and the bandwidth definition of high-slope bells are the two
conventions that cannot be confirmed from literature. To verify:

1. On the reference system, render a unit impulse (or sine sweep) through the
   reference EQ with the preset, at 48 kHz, 32-bit float WAV.
2. `cargo run -p neofilter-cli -- compare --preset friend-eq-a --ir ir.wav`
   prints max / RMS deviation in dB and a per-band hint.
3. Adjust constants in `conventions.rs` only.

## Testing

- `neofilter-dsp` unit tests: prototype identities (DC/HF gain, midpoint
  gain, −3 dB points, Butterworth flatness, bell peak gain == G at f0 for all
  M, symmetry boost/cut = inverse), matched-vs-analog accuracy grid,
  stability (all poles inside unit circle) over a randomised parameter grid
  (proptest), no NaN/inf, impulse-response FFT == analytic digital response,
  M/S reconstruction exact when bands are flat, null test (all gains 0 →
  bit-transparent within 1e-12), smoothing produces no discontinuity above a
  threshold on step changes, sample-rate sweep 44.1–192 kHz.
- Plugin tests: param ids unique and stable (snapshot list), presets map to
  the tables above exactly, preset apply resets unused bands.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- Bundle smoke test: `cargo xtask bundle neofilter --release` produces
  `.clap` and `.vst3`; run `clap-validator` if available.

## Phases (each ends with green tests and a commit)

1. **Scaffold** — workspace, crates, nih-plug pass-through plugin that
   bundles, shared types (`Shape`, `Slope`, `Placement`, `BandParams`) in the
   dsp crate, stub module files so later phases own disjoint files.
2. In parallel:
   - **DSP core** — `neofilter-dsp` (prototypes, matched transform, engine, tests).
   - **Params + presets** — plugin params, preset tables, state tests.
   - **CI + README** — workflows, README, LICENSE.
3. In parallel:
   - **Integration** — process(): smoothing, engine, placement, output gain,
     analyzer tap; CLI tool.
   - **Editor** — egui GUI.
4. **Verification** — independent review, full test/clippy/bundle run,
   install to `~/.clap` / `~/.vst3`, fix findings.

## References

- M. Vicanek, "Matched Second Order Digital Filters" (2016) and "Matched
  Two-Pole Digital Shelving Filters".
- S. Orfanidis, "High-Order Digital Parametric Equalizer Design", JAES 2005.
- R. Bristow-Johnson, "Audio EQ Cookbook".
- V. Zavalishin, "The Art of VA Filter Design".
- nih-plug: https://github.com/robbert-vdh/nih-plug
