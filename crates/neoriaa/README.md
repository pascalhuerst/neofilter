# NeoRIAA

RIAA pre-emphasis ("inverse RIAA") and de-emphasis plugin (CLAP + VST3) for
vinyl cutting chains. It sits in front of the cutter amplifier, so the curve
has to be accurate and nothing may alter the signal behind your back: there is
no limiter and no soft clipper, only a peak meter and a clip indicator.

```
cargo test -p neoriaa
cargo xtask bundle neoriaa --release     # target/bundled/neoriaa.{clap,vst3}
```

## Curves

Time constants `T1 = 3180 us`, `T2 = 318 us`, `T3 = 75 us`.

* De-emphasis (playback): `(1 + sT2) / ((1 + sT1)(1 + sT3))`
* Pre-emphasis (recording, default): its reciprocal

Both are normalised to 0 dB at 1 kHz (de-emphasis: +19.27 dB at 20 Hz,
+16.95 dB at 50 Hz, -13.73 dB at 10 kHz, -19.62 dB at 20 kHz; the formula is
authoritative and is what the tests check).

Options (parameters `iec`, `ultrasonic`):

* **IEC 7950 us** (`T4`): de-emphasis gets an extra `sT4/(1 + sT4)` high-pass
  (corner 20 Hz). The exact inverse is unbounded at DC, so in **pre-emphasis**
  mode "IEC on" means the inverse of that high-pass (a bass boost, +3 dB at
  20 Hz) **limited by a first-order 5 Hz high-pass**:
  `P = (1 + sT4)/(sT4) * sTL/(1 + sTL) / H_de`, `TL = 1/(2 pi 5 Hz)`.
  Pre-emphasis with IEC followed by de-emphasis with IEC is therefore unity
  only above about 20 Hz and rolls off below 5 Hz.
* **3.18 us pole** (`T5`, about 50 kHz, "Neumann"): pre-emphasis is multiplied
  by `1/(1 + sT5)`; de-emphasis by `(1 + sT5)`.

## Parameters

Ids are stable: `mode` (Pre-emphasis (cut), default / De-emphasis
(playback)), `iec`, `ultrasonic`, `trim` (-36 .. +12 dB, default 0, ramped in
dB over 20 ms), `bypass` (host bypass, 10 ms crossfade, then bit-exact dry).
Layouts: stereo and mono, zero latency.

Changing mode or options crossfades (15 ms) between filters that have been
running all the time, so nothing is redesigned or swapped under a running
state and there are no clicks. The price is that all eight filters (two modes
times four option combinations) run per sample (a few percent of one core at
48 kHz, under 10 % at 384 kHz).

The meter shows the output sample peak (running peak falls 20 dB/s, hold 2 s)
and a latching clip LED that trips at |out| >= 1.0 (0 dBFS) on the samples
written to the host buffer, before the host converts. Reset clears both.
Non-finite input samples are replaced by silence in the processed path.

## Digital design

`neo_measure::riaa::RiaaFilters::design(sample_rate)` (called from `initialize()`,
never on the audio thread; precomputed tables for the common rates 44.1-384 kHz,
a bounded-time quick design for other rates)
fits a minimum-phase biquad cascade to the analytic *de-emphasis* by
Levenberg-Marquardt on the complex response (log magnitude and phase on a log
grid, 10 Hz .. 20 kHz, reweighted towards a minimax fit). Each biquad is the
bilinear transform of an analog section with positive `(w0, Q)`, so every
candidate is stable and minimum phase by construction. The pre-emphasis is
the exact inverse (numerators and denominators swapped), so pre followed by
de is unity to machine precision (tested to 1e-9 on noise at every rate).
Magnitude accuracy is prioritised over phase accuracy; 0 dB at 1 kHz is exact.
The IEC sections are exact first-order bilinear sections added on top.
Processing is a transposed direct form II cascade in f64 with denormal
flushing.

### Accuracy, 20 Hz .. 20 kHz, vs the analytic curve

Worst error over a 3000-point log grid (printed by
`cargo test -p neoriaa --test dsp_design -- --nocapture`); pre- and
de-emphasis and the IEC option give the same numbers to within the IEC
section's rounding. Orders include 1 for the extra IEC section when enabled.

| fs (kHz) | 3.18 us pole | order | max mag error (dB) | max phase error (deg) | target (dB / deg) |
|---|---|---|---|---|---|
| 44.1  | off | 4 | 0.0029 | 71.2  | 0.02 / 2   |
| 44.1  | on  | 4 | 0.0020 | 50.7  | 0.02 / 2   |
| 48    | off | 4 | 0.0024 | 60.9  | 0.02 / 2   |
| 48    | on  | 6 | 0.0036 | 32.6  | 0.02 / 2   |
| 88.2  | off | 8 | 0.0018 | 13.6  | 0.01 / 0.5 |
| 88.2  | on  | 8 | 0.0014 | 1.14  | 0.01 / 0.5 |
| 96    | off | 8 | 0.0016 | 10.7  | 0.01 / 0.5 |
| 96    | on  | 6 | 0.0009 | 0.85  | 0.01 / 0.5 |
| 176.4 | off | 8 | 0.0070 | 0.33  | 0.01 / 0.5 |
| 176.4 | on  | 4 | 0.0001 | 0.14  | 0.01 / 0.5 |
| 192   | off | 8 | 0.0039 | 0.21  | 0.01 / 0.5 |
| 192   | on  | 4 | 0.00003| 0.05  | 0.01 / 0.5 |
| 384   | off | 4 | 0.0018 | 0.13  | 0.01 / 0.5 |
| 384   | on  | 4 | < 0.00001 | 0.003 | 0.01 / 0.5 |

**Magnitude meets its target at every rate (by a factor of 1.4 to 10 or
more). Phase meets its target only from 176.4 kHz up.** At 88.2 and 96 kHz
the phase misses by about 2x with the 3.18 us pole (1.14 / 0.85 deg vs
0.5 deg) and by 20x without it (13.6 / 10.7 deg); at 44.1 and 48 kHz it
misses by a factor of 15 to 35 (33 to 71 deg vs 2 deg). This is not an
optimiser shortfall. A stable minimum-phase IIR is fixed (Hilbert relation) by
its magnitude, and its phase must return to 0 at Nyquist, whereas the
analog RIAA phase is still about -85 deg at 20 kHz because its magnitude keeps
falling 6 dB/octave forever. Fitting magnitude to 0.003 dB at 44.1 kHz leaves
the phase near 71 deg; trading magnitude for phase only gets to about 55 deg
at 0.5 dB. The 3.18 us pole makes the analog magnitude flatten, which is
why that option meets the phase target sooner. The measured values are
enforced in the tests (with a margin), so a regression shows up; the targets
themselves are checked and the misses are printed.
The group-delay consequence is a small change of waveform shape for transient
material, not of the frequency balance.

### Behaviour above 20 kHz

The fit is only weakly guided above 20 kHz (the response must stay within
6 dB of the analytic curve, floored at -36 dB for the de-emphasis). The
digital pre-emphasis keeps rising towards Nyquist, always stays finite and
below +46 dB, and never dips more than a few dB below its 20 kHz value
(the tests enforce `max < 46 dB`, `min > 12 dB`). Pre-emphasis gain re 1 kHz
without IEC (analog value at Nyquist in brackets):

| fs (kHz) | 20 kHz | max above 20 kHz (= Nyquist) off / on | analog at Nyquist off / on |
|---|---|---|---|
| 44.1  | 19.6 dB | 20.2 / 19.5 dB | 20.5 / 19.7 dB |
| 48    | 19.6 dB | 22.4 / 26.3 dB | 21.2 / 20.3 dB |
| 88.2  | 19.6 dB | 30.8 / 29.8 dB | 26.5 / 24.0 dB |
| 96    | 19.6 dB | 31.6 / 30.2 dB | 27.2 / 24.4 dB |
| 176.4 | 19.6 dB | 38.3 / 32.3 dB | 32.5 / 26.3 dB |
| 192   | 19.6 dB | 38.7 / 32.5 dB | 33.2 / 26.5 dB |
| 384   | 19.6 dB | 42.0 / 33.3 dB | 39.2 / 27.3 dB |

(The 3.18 us pole lowers the 20 kHz value to 19.0 dB.) Without the pole
this is a rise of about 6 dB/octave from 20 kHz to roughly 20 dB (at
44.1 kHz) up to 42 dB (at 384 kHz) at Nyquist, as in the analog curve, but a few
dB higher than it near Nyquist. If your amplifier or lacquer should not
see that, enable the 3.18 us pole. The editor plots 10 Hz up to Nyquist or
40 kHz, whichever is lower, and shades the region outside 20 Hz .. 20 kHz in
the error plot.

## Editor

520x360 (resizable): mode selector, IEC and pole toggles, bypass, trim, the
response plot (analog target and digital curve), the error plot magnified to
+-0.05 dB, gain readout at 20 Hz / 1 kHz / 10 kHz / 20 kHz, peak meter with
hold, clip LED with reset button, sample rate. The painting code has not been
exercised in a host by the tests (only the scales, ticks, formatting and
curve cache are unit-tested).
