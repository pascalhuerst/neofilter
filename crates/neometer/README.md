# NeoMeter

Pass-through metering and measurement plugin (CLAP + VST3) for mastering and
vinyl cutting. Stereo in, stereo out unchanged (bit-exact while the generator
is off), also mono to mono, zero latency. All DSP comes from
[`neo-measure`](../neo-measure/README.md); this crate is the plugin, the
audio-path/analysis plumbing and the egui editor.

Build: `cargo xtask bundle neometer --release` gives
`target/bundled/neometer.{clap,vst3}`. Tests: `cargo test -p neometer`
(the first run of a test binary designs the RIAA filters, which takes seconds
in a debug build).

## Architecture

| what | where | how |
|---|---|---|
| sample peak, true peak, RMS, DC, crest | audio thread | `neo-measure::levels` |
| correlation, balance, M/S, LF vertical/lateral | audio thread | `neo-measure::stereo` |
| RIAA pre-emphasis, vinyl meters, velocity / displacement | audio thread | `vinyl.rs` |
| generator | audio thread | `testgen.rs` |
| loudness (M/S/I, LRA) | analysis thread | `neo-measure::loudness` |
| goniometer, spectra, RTA | analysis thread | `neo-measure::spectrum` |
| tone / THD / THD+N, wow & flutter, crosstalk, noise | analysis thread | latched captures |

* The audio thread never allocates, locks or blocks. It publishes meter
  values through a wait-free triple buffer (every >= 256 frames) and pushes
  the **input** frames into a lock-free SPSC ring (262144 frames: 5.5 s at
  48 kHz, 1.4 s at 192 kHz). If the analysis thread falls behind, the
  newest frames are dropped and counted; the count is shown in the top bar
  ("dropped N frames"). Peak, RMS, stereo and vinyl meters are on the audio
  thread and unaffected; loudness and spectra may miss samples then.
* **Why pre-emphasis is on the audio thread:** peaks after pre-emphasis must
  not miss a sample, and a lossy hand-off cannot guarantee that. The filter
  is a few biquads per channel. It is designed in `initialize()` for the host
  rate (about 0.5 s in release builds, 3 s at 96 kHz; re-used when the rate
  does not change).
* The analysis thread (`neometer-analysis`) is started by `initialize()` and
  stopped and joined by `deactivate()` or when the plugin is dropped. It
  always drains the ring and runs the cheap loudness meter, because the
  integrated loudness needs every sample since the reset. While no editor is
  open and no measurement is latched it polls every 25 ms and publishes at
  5 Hz and skips goniometer, spectrum and capture. With the editor open
  or a measurement running: 5 ms / 30 Hz. The GUI reads a
  `Mutex<Arc<Snapshot>>` shared only with this thread and a triple buffer
  shared only with the audio thread.
* All meters and analyses see the plugin **input**. The generator only
  changes the output, so the intended use is: generator out through the
  chain, return back into the input.

## Parameters and state

Host parameters (ids frozen): `reference_level` (vinyl calibration, dBFS,
default -18, range -60..0) and `lf_crossover` (Hz, default 300, 50..1000).
Everything else (page, FFT size, window, averaging, smoothing, channel,
dB/frequency ranges, correlation time, loudness target, HF corner, warning
thresholds, test durations, latched reference tone level) is persisted
non-automatable state. Missing or invalid values in a saved state fall back to
defaults and are clamped.

**The generator is neither a parameter nor persisted.** It is created off,
forced off again in `initialize()`, and a saved project can never start
emitting a signal. A red "GENERATOR ON" badge with an Off button is shown on
every page while it is running.

## Pages and definitions

Level conventions: dBFS peak = `20 log10 |x|`. **RMS values are sine
referenced** (a full-scale sine reads 0 dB, so a -20 dBFS sine reads -20.00 dB
RMS); `neo-measure` reports the raw `20 log10 rms` which is 3.01 dB lower.
Silence reads `-inf`. All dB readouts have 0.01 resolution.

### Levels & Loudness

* Per channel: sample peak (held 2 s, then falling 20 dB/s) and maximum since
  reset; true peak (BS.1770-4, >= 4x oversampling below 88.2 kHz) held and
  maximum; RMS over a 300 ms rectangular window; crest factor = peak / RMS
  since reset; DC offset = mean since reset (shown in % of full scale).
* Loudness per ITU-R BS.1770-4: momentary (400 ms), short-term (3 s),
  integrated (-70 LUFS absolute and -10 LU relative gate), loudness range
  (EBU Tech 3342: gated short-term values, 10th..95th percentile), maximum
  momentary and short-term, maximum true peak of both channels. A K-weighted
  stereo sine of -20 dBFS reads -20.0 LUFS; a mono layout counts one channel.
* Target line: -14 / -16 / -23 / -9 LUFS or custom; a visual marker and the
  difference of the integrated value to it, nothing else.
* Reset clears holds, maxima, statistics and the loudness integration.

### Stereo

* Goniometer: 2048 points, `x = (L-R)/sqrt2`, `y = (L+R)/sqrt2` (M/S rotated;
  mono is a vertical line) or plain L/R.
* Correlation `sum(LR) / sqrt(sum(LL) sum(RR))` with a one-pole integrator of
  selectable time constant (0.1 / 0.3 / 1 / 3 s), balance (`L - R` level in
  dB), mid `(L+R)/2` and side `(L-R)/2` RMS levels.
* Low-frequency vertical / lateral: L and R are low-passed at the crossover
  (4th-order Linkwitz-Riley), then lateral = mid level, vertical = side level
  and correlation of the band. The readouts turn red and a warning appears
  when `vertical - lateral` exceeds an adjustable margin (default 0 dB) or the
  LF correlation falls below an adjustable threshold (default 0).
  Silence never warns. Changing the crossover restarts the filter state.

### Spectrum

Welch FFT, 4096..65536 points, 75 % overlap, Hann / Blackman-Harris / flat-top,
averaging off / exponential (time constant) / infinite, peak hold, smoothing
off / 1 octave / 1/3 / 1/6 / 1/12 octave (power domain), channels L / R / Mid /
Side / L+R overlay, dB and frequency range, cursor readout (frequency, level
per channel), freeze. A full-scale sine reads 0 dBFS (flat-top: within 0.01 dB
at any frequency). The 1024 display points are log spaced and combined by
**maximum** when several bins fall into a point (so tones keep their peak
level; broadband noise reads slightly high in unaveraged mode). The 1/3-octave
RTA mode shows IEC 61260 band power levels (equal energy per band reads flat,
pink noise gives a flat RTA) with exact base-10 centre frequencies.

### Vinyl

All levels are after RIAA **pre-emphasis** (`neoriaa` design: no IEC roll-off,
no ultrasonic pole, 0 dB at 1 kHz, +13.73 dB at 10 kHz) for four signals: L, R,
lateral `(L+R)/2` and vertical `(L-R)/2`.

* Peak, true peak, held and maximum, RMS.
* HF band: the pre-emphasised signal above a selectable corner (3..10 kHz,
  default 5 kHz, 4th-order Butterworth high-pass): RMS, held peak and maximum
  peak as a cutter-head load / sibilance indicator.
* **Velocity and displacement are estimates that depend on the calibration
  value `reference_level`**, the level in dBFS (after pre-emphasis) that
  corresponds to a recorded peak lateral velocity of 5 cm/s at 1 kHz:

  ```
  v [cm/s] = 5 * |x_pre| / 10^(reference_level / 20)
  d [um]   = 5 * |D|     / 10^(reference_level / 20) * 1e4,   D = integral of x_pre dt
  ```

  Assumptions: recorded velocity is proportional to the pre-emphasised
  signal, displacement is the integral of velocity. `|x_pre|` is the (true)
  peak of the signal, `D` is computed with a leaky integrator
  `1 / (s + 2 pi 10 Hz)` (bilinear, warping pinned at 1 kHz): gain
  `1 / (2 pi f)` for `f >> 10 Hz` (exact at 1 kHz, -3 dB at 10 Hz, -1 dB at
  20 Hz, reads low above about fs/20). A lateral (L = R) 1 kHz tone at the
  reference level reads 5.000 cm/s and `5 / (2 pi 1000) = 7.958 um`. A mono
  signal reads the same on L, R and lateral and zero on vertical. Velocity and
  displacement are derived in the GUI from the raw levels, so changing the
  reference re-scales the held maxima retroactively.
* Max holds include the start-up transient of a test tone (the integral of
  a sine started at phase 0 has a DC term); reset after the tone has settled.

### Test

* **Generator** (off by default, see above): sine (1 kHz / 3150 Hz / 10 kHz /
  custom), log sweep 20 Hz..20 kHz (looping, adjustable duration), white and
  pink noise; level in dBFS (**peak for tones, RMS for noise**, default
  -20); channels L / R / L+R / L+R inverted (R = -L, vertical); output
  replaces the input (default; cross-fades, un-routed channels are silent) or
  is added to it. Start/stop use a 20 ms raised-cosine envelope; changing the
  waveform fades out before the new one fades in; level changes are smoothed
  over 10 ms. Once faded out, the input passes through bit-exactly again.
* **Tone** (latched over 1/2/5/10 s, channel L or R): frequency, level (peak
  dBFS), THD (harmonics 2..10) and THD+N (20 Hz..20 kHz) in % and dB.
* **Set reference** latches the level of a 1 s tone measurement as the noise
  reference.
* **Wow & flutter** of a 3150 / 3000 Hz tone (5..30 s): speed deviation %, and
  peak, quasi-peak, RMS and 2-sigma values, weighted and unweighted. **The
  weighting filter is a fit to the standard's curve (peak 4 Hz, -6 dB at 0.8
  and 20 Hz) because the tabulated mask was not available, and the
  quasi-peak ballistics (10 ms / 1.5 s) are unverified** (see `neo-measure`).
  Use it for comparisons, not for conformance claims.
* **Channel separation**: drive L (or R) with a tone, press "Measure L -> R"
  (or "R -> L"): level of the tone in the driven channel minus its level at the
  same frequency in the other one, in dB (larger is better). Limited by the
  noise floor of the other channel.
* **Noise / rumble**: both channels, unweighted (default 20 Hz..20 kHz,
  adjustable; 2nd-order high/low-pass) and A-weighted RMS in dBFS, and relative
  to the latched reference (reference RMS = tone peak level - 3.01 dB, so the
  reading is dB below the tone RMS). Without a reference the relative columns
  equal the absolute ones.

Captures are limited to 30 s. Measurements run on the analysis thread; a long
one (wow & flutter at 192 kHz) can make that thread late for a moment, which
shows up as dropped frames, never as an audio problem.

## Limitations

* Velocity / displacement are estimates (above). Pre-emphasis uses the
  standard RIAA time constants only; real cutting chains differ (Neumann
  pole, limiters, cutter-head response).
* Spectrum update rate follows the FFT hop: 65536 points at 48 kHz refresh
  every 0.34 s.
* The editor's paint code was written without being able to render it; see
  the report notes for what most likely needs visual tuning.
* All `neo-measure` limitations apply (wow & flutter weighting is a fit,
  EBU 3341/3342 covered by synthesised equivalents, true-peak filter is a
  32-taps-per-phase Kaiser design).
* The RIAA filter design is compiled from `neoriaa/src/dsp.rs` by path
  (`#[path]`) instead of depending on the `neoriaa` crate: that crate exports
  plugin entry symbols (`GetPluginFactory`, `clap_entry`) that clash with this
  plugin's at link time.
