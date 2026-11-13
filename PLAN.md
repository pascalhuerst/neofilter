# NeoFilter — implementation plan

> **Status (suite, v0.1 unreleased):** everything in "Scope" below is
> implemented. The repository has since grown into a suite (NeoFilter, NeoRIAA,
> NeoMeter, neo-measure); its plan and status are in
> [docs/PLAN-vinyl-suite.md](docs/PLAN-vinyl-suite.md), and all open items are
> listed in one place under "Open items and future work" there. Deviations
> from this document are marked "As built" below.

NeoFilter is a 32-band parametric EQ plugin (CLAP + VST3) written in Rust for
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

- 32 bands (originally 24; bands 25..32 were appended later, ids of 1..24
  unchanged); per band: used, enabled, shape, frequency (10 Hz–30 kHz), gain
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
  it against a measured impulse-response WAV (calibration). As built it also
  renders WAV files, writes impulse responses, converts `.ffp` files
  (`export-ffp`, `import-ffp`) and runs the loop calibration offline
  (`calibrate`).
- CI + release pipelines for GitHub Actions, README.
- Added after the first plan (see `docs/PLAN-vinyl-suite.md`): sidechain input
  and loop calibration, NeoRIAA, NeoMeter, neo-measure.

Out of scope (v0.1, keep the architecture open for them): linear-phase and
natural-phase modes, dynamic EQ, brickwall slope, EQ match, band solo,
auto-gain, surround.

## Repository layout

```
Cargo.toml                 workspace
crates/neofilter-dsp/      pure DSP library, no plugin deps, heavily tested (+ autofit)
crates/neofilter/          the plugin (nih-plug): params, process, presets, calibration, editor
crates/neofilter-cli/      CLI: curves, render, compare, .ffp exchange, calibrate
crates/neo-measure/        pure measurement library (meters, spectra, RIAA design, ...)
crates/neoriaa/            RIAA plugin
crates/neometer/           metering / measurement plugin
xtask/                     nih_plug_xtask (cargo xtask bundle -p neofilter -p neoriaa -p neometer --release)
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
  - High shelf = low shelf of gain 1/G times G. Q control (as built, measured):
    the highest-Q section's Q is multiplied by `Q_display^0.47`
    (`SHELF_Q_EXPONENT`); zero and pole frequencies are unchanged. The first
    plan (Q multiplied directly, as for cuts) did not match the measurements.
- **Tilt Shelf** (as built, measured convention): `tilt(g) = high_shelf(2 g)
  - g dB` at the same frequency, slope and Q. A +6 dB tilt runs from -6 dB at
  low frequencies through 0 dB at the band frequency to +6 dB at high ones.
  (The first plan, a low shelf of gain 1/G times sqrt(G), ran only -3 to +3 dB
  for the same setting and was wrong.) Order from slope.
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

Accuracy (as built): the original blanket target (0.1 dB up to 0.4 fs, 0.5 dB
up to 0.45 fs) was **not** met by the matched design. Bell (all slopes) and
even-order High Cut bands and default-Q 12 dB/oct shelves no longer use it:
they use the reference EQ's rule
identified by measurement (`src/refmatch.rs`, `docs/reference-matching.md`),
which reproduces 198 measured single-band curves to 0.017 dB worst case
(Bell 0.006 dB, median 0.0001 dB) and the factory presets' total curves to
0.016 dB or better; default-Q 12 dB/oct shelves match 58 measured shelf/tilt cases to
0.001 dB, and other shelves and tilts match them to 0.01 dB up to 0.1 fs
(`tests/reference_shelves.rs`). These bands intentionally deviate from the
analog prototype near Nyquist. The matched design remains for the other shapes (and as
a per-section fallback); its enforced regression tolerances per shape and
frequency bucket are in `crates/neofilter-dsp/tests/accuracy.rs` (for
bells and even-order high cuts only band frequencies up to 0.02 fs are compared
with the analog prototype there). Matched-design summary: high-Q shelves and
cuts, wide notches and 96 dB/oct band passes near Nyquist deviate by several dB
(up to ~15 dB in the extreme corners). See "Future work".

### Runtime engine

- `Biquad` TDF-II, f64 state; `BandFilter` = up to 8 sections x 2 channels;
  `EqEngine` = 32 bands in series.
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

- Params: `#[nested(array, group = "Band")]` band params; nih-plug generates
  ids `{field}_{n}` (n = 1..32) in group `Band {n}`: `used_n`, `on_n`,
  `shape_n`, `freq_n`, `gain_n`, `q_n`, `slope_n`, `place_n`; plus globals
  `out_gain` (dB), `gain_scale` (percent), `bypass`. Ids are frozen once released.
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

### Friend EQ A+B (series) — `friend-eq-ab`

Friend EQ A and Friend EQ B were used as two instances in series. Linear
filters in series commute and concatenate, so one instance holding all bands
is equivalent. Band 1..15 are A's bands 1..15 (11 disabled); B's used bands
follow in table order; bands 25..32 are spare.

Band mapping of "Friend EQ A+B (series)" (`friend-eq-ab`):

| Combined band | Source |
|---|---|
| 1 to 15 | Friend EQ A bands 1 to 15 (band 11 is used but disabled) |
| 16 | Friend EQ B band 2 |
| 17 | Friend EQ B band 4 |
| 18 | Friend EQ B band 5 |
| 19 | Friend EQ B band 6 |
| 20 | Friend EQ B band 8 |
| 21 | Friend EQ B band 9 |
| 22 | Friend EQ B band 10 |
| 23 | Friend EQ B band 11 |
| 24 | Friend EQ B band 12 |
| 25 to 32 | unused spare bands |

Built in `presets.rs` from the same A and B tables (no retyped numbers).
Verified: dB responses add to within 1e-9 at 44.1/48/96 kHz; the f64 engine
output equals the A-then-B engine chain; the f32 plugin path differs by f32
rounding only.

## Future work

- Identify the reference rule for the remaining shapes (Low Cut, Notch, Band
  Pass, odd-order cuts, Flat Tilt, shelves other than default-Q 12 dB/oct, and
  shelf/tilt behaviour above 0.1 fs) and near Nyquist; today they use the
  analog-matched design (see `docs/reference-matching.md`, "Not covered yet").
- Reference measurements exist for 44.1 / 48 / 96 kHz only.
- Out-of-scope v0.1 items: linear-phase and natural-phase modes, dynamic EQ,
  brickwall slope, EQ match, band solo, auto-gain, surround.

## Calibration (black-box, optional but recommended)

Status: done for Bell, High Cut, shelves and tilt, see
`docs/reference-matching.md` (the Q scaling, the shelf resonance law and the
tilt convention turned out to need the measured rules). The workflow below
is what was used and remains valid for checking further shapes. The Q scaling
and the bandwidth definition of high-slope bells are conventions that cannot
be confirmed from literature. To verify:

1. On the reference system, render a unit impulse (or sine sweep) through the
   reference EQ with the preset, at 48 kHz, 32-bit float WAV.
2. `cargo run -p neofilter-cli -- compare --preset friend-eq-a --ir ir.wav`
   prints max / RMS deviation in dB, the worst frequency and the ten worst
   third-octave regions.
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
- Bundle smoke test: `cargo xtask bundle -p neofilter -p neoriaa -p neometer
  --release` produces `.clap` and `.vst3` for each; run `clap-validator` if
  available (it was run on NeoFilter early in development only).

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
   install to `~/.clap` / `~/.vst3`, fix findings. (Done for the first
   plugin; verification in a real host is still open for the suite, see
   `docs/PLAN-vinyl-suite.md`.)

## References

- M. Vicanek, "Matched Second Order Digital Filters" (2016) and "Matched
  Two-Pole Digital Shelving Filters".
- S. Orfanidis, "High-Order Digital Parametric Equalizer Design", JAES 2005.
- R. Bristow-Johnson, "Audio EQ Cookbook".
- V. Zavalishin, "The Art of VA Filter Design".
- nih-plug: https://github.com/robbert-vdh/nih-plug
