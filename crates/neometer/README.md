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
| input trims (`trim_l` / `trim_r`) | audio thread | `audio.rs`, before everything below |
| RIAA pre-emphasis, vinyl meters, velocity / displacement | audio thread | `vinyl.rs` |
| generator | audio thread | `testgen.rs` |
| loudness (M/S/I, LRA) | analysis thread | `neo-measure::loudness` |
| goniometer, spectra, RTA | analysis thread | `neo-measure::spectrum` |
| tone / THD / THD+N, wow & flutter, crosstalk, noise | analysis thread | latched captures |
| test-record calibration (reference bands, sweep response, W&F, tracking) | analysis thread | `testrec.rs`, `neo-measure::{sweep_track, velocity}` |

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
default -18, range -60..0), `lf_crossover` (Hz, default 300, 50..1000) and
`trim_l` / `trim_r` (input trims, dB, default 0, range -24..+24). The trims
are linearly ramped over 20 ms and applied to the pass-through audio **before
everything else** (ring, meters, vinyl meters), so all pages see the levelled
signal; with both trims at 0 dB the pass-through stays bit-exact. In a mono
layout `trim_l` applies. Everything else (page, FFT size, window, averaging, smoothing, channel,
dB/frequency ranges, correlation time, loudness target, HF corner, the
pre-emphasis options of the vinyl page, warning thresholds, test durations,
latched reference tone level) is persisted non-automatable state, and so is
the whole test-record state (profile, options, measurements and the last
captured sweep; key `test-record`). Missing or invalid values in a saved state fall back to
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

All levels are after RIAA **pre-emphasis** (`neo_measure::riaa` design, 0 dB
at 1 kHz, +13.73 dB at 10 kHz with the default options) for four signals: L, R,
lateral `(L+R)/2` and vertical `(L-R)/2`. The page has two checkboxes (persisted
editor state): **IEC** (IEC 7950 us low-frequency roll-off; the bass boost of
the pre-emphasis is limited by a 5 Hz high-pass) and **ultrasonic pole**
(3.18 us, about 50 kHz). The audio thread holds all four designs and switches
between them (the filter state restarts); off/off is the default.

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
  reference re-scales the held maxima retroactively. The L and R rows show the
  *lateral-equivalent* velocity of that channel's level (a mono signal reads the
  same on L, R and lateral); the single-channel groove-wall velocity is
  `v_ch = v_lateral / sqrt(2)` for the same level, see the Test record page.
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

### Test record (input calibration)

Levels the PLAYBACK input (turntable -> phono preamp -> interface) with a test
record and verifies the chain, before any loop calibration. The page has five
steps; each measurement is latched with a progress bar and a result table and
every step has "Copy results" (plain text to the clipboard). Results, the
profile and all options are persisted in the plugin state (key `test-record`);
the analysis thread stores finished measurements there, the editor only edits
settings.

**Profiles.** *Ortofon Test Record* (default) and *Custom*; every value is
editable (name, reference frequency, quoted velocity and RMS/peak, reference
band layout, sweep range, nominal characteristic with turnover, tracking
frequency and amplitudes). **The Ortofon values come from public listings, not
from the sleeve: check your sleeve and edit what differs.** Used values:

| tracks (both sides) | content | used for |
|---|---|---|
| 1-4 | frequency sweeps, constant velocity, from 800 Hz (upper limit 50 kHz in one listing, 20 kHz implied elsewhere) | step 2. Which track is L / R / lateral / vertical is **not assumed**: choose or auto-detect per measurement |
| 5-8 | 1000 Hz, 5 cm/s **RMS**, alternating right and left channel | steps 1 and 3 |
| 9-14 | 315 Hz lateral, peak amplitude 50, 60, 70, 80, 90, 100 um | step 4 |
| 15 | square wave 2.7 ms, duty 3:7 | oscilloscope check, no tool here |

There is no 3150 Hz band and no sweep below 800 Hz on this record. The Custom
profile starts with a lateral 1 kHz band of 5 cm/s peak, a 20 Hz..20 kHz sweep
recorded flat after RIAA, and the same tracking bands.

#### Procedure with the Ortofon record

0. Cartridge, tracking force and anti-skating set up, generator off, trims at
   0 dB, `reference_level` untouched. Open the *Test record* page.
1. **Tracks 5-8.** Play a track that drives one channel; while it plays press
   *Measure left band* (or *right band*, matching the channel it drives; the
   detection column must agree, otherwise the table turns red) and wait for the
   progress bar (2 / 5 / 10 / 20 s averaging; the track must be at least that
   long). Do the same for the other channel on the next track. Read levels,
   balance, crosstalk and speed. Press *Apply trims* (sets `trim_l` / `trim_r`
   so both channels read the target, default -18 dBFS), then measure both
   bands again to verify, then *Set velocity calibration* (sets
   `reference_level`).
2. **Tracks 1-4** (sweeps). Press *Arm*, play a sweep track, optionally the
   next ones (they accumulate per frequency cell), press *Stop and keep*. The
   page shows the detected modulation (L only / R only / lateral / vertical,
   from channel level difference and phase); override it if you know better.
   Check *Recorded characteristic* = constant velocity and *playback preamp
   applies RIAA* = on (turn it off for a flat preamp), normalise at 1 kHz,
   read the deviation plot and the statistics, press *Copy curve (JSON)* to
   paste the playback-chain correction into the EQ plugin's loop calibration.
3. **Tracks 5-8** again: *Measure* in the wow & flutter block (carrier = the
   profile's 1 kHz, 10 s default).
4. **Tracks 9-14.** Select the amplitude (50 .. 100 um), play the matching
   band, *Measure*; the selection advances automatically. The table flags
   mistracking and names the highest clean amplitude.
5. The **summary** block collects everything for *Copy summary*.

#### Step 1: reference level, balance, separation

Per band, over the averaging time, on the (trimmed) input: tone frequency,
level (peak dBFS, `neo_measure::tone`), THD (harmonics 2..10), THD+N and the
residual noise level (THD+N minus THD, in dBFS, 20 Hz..20 kHz), the inter-
channel phase of L relative to R at the tone frequency, and the detected
modulation: level difference `|L-R| > 12 dB` means the louder channel only;
`|L-R| <= 6 dB` with phase `< 45 deg` is lateral, `> 135 deg` vertical;
anything else is "unclear".

From the left and right bands (or one lateral/vertical band):

* level per channel from its **own** band, balance `L - R`;
* crosstalk L->R = level(L) - level(R) of the left band at the tone frequency
  (`channel_separation_db`), R->L likewise from the right band;
* speed deviation `(mean tone frequency / nominal - 1) * 100 %`;
* suggested trim per channel `trim = trim_during_capture + target - level`
  (limited to +-24 dB; "Apply trims" sets the host parameters; the capture
  remembers the trims that were active, so levels are shown for the trims that
  are applied *now*);
* **velocity calibration.** `reference_level` is the dBFS of a mono (L = R,
  in phase) signal at 5 cm/s *peak lateral* at 1 kHz. Conventions
  (`neo_measure::velocity`): `v_peak = sqrt(2) v_rms`; for the 45/45 system a
  one-channel band of channel (wall) velocity `v_ch` has lateral and vertical
  components `v_ch / sqrt(2)`; the same in-phase signal in both channels is
  purely lateral with `v_lat = sqrt(2) v_ch` (anti-phase: vertical). 5 cm/s
  lateral therefore means `v_ch = 3.536 cm/s` peak per channel, and
  ```
  reference_level = level_ch - 20 log10( v_ch(record) / (5 / sqrt 2) )
  ```
  The Ortofon band is 5 cm/s RMS on one channel = 7.071 cm/s peak, so
  `reference_level = level_ch - 6.02 dB`; at the -18 dBFS target that is
  -24.02 dBFS. A lateral/vertical band of quoted velocity `v` has
  `v_ch = v / sqrt 2`. **This assumes the sleeve's value is the channel
  (groove-wall) velocity of a one-channel band**; if your sleeve quotes
  lateral velocity, set the profile to a lateral band. The formula is in the
  tooltip of the button. Note that the vinyl page's L and R rows show the
  *lateral-equivalent* velocity of their level (see above), so with the band
  at the calibrated level the L row reads 10 cm/s, not 7.07.

#### Step 2: playback frequency response

`neo_measure::sweep_track` runs on the analysis thread while armed: it analyses
short overlapping blocks per channel, finds the dominant sinusoid (interpolated
FFT peak, chirp rate from the half blocks, de-chirped DTFT for the level, so
the reading does not depend on the sweep rate), accepts the block only if the
residual is at least *Tone threshold* (default 25 dB) below the tone and puts
the level on a grid of 24 cells per octave (median of the block levels at the
median block frequency, so it works for sweeps up or down and for stepped spot
tones; a cell is covered with at least 2 accepted blocks). Silence, speech,
noise between bands and tone transitions are rejected. The coverage (lowest and
highest covered frequency) is shown.

Expected level of the playback chain for a velocity-sensitive cartridge:

| recorded characteristic | recorded velocity | RIAA preamp | flat preamp |
|---|---|---|---|
| constant velocity (Ortofon sweeps) | constant | RIAA de-emphasis | flat |
| flat after RIAA | RIAA pre-emphasis | flat | RIAA pre-emphasis |
| constant velocity above / constant amplitude below turnover `f_t` | `min(f, f_t) / f_t` | that times the de-emphasis | that |

`deviation(f) = level(f) - expected(f) - offset`, with `offset` forcing 0 dB at
the normalisation frequency (default 1 kHz, linear interpolation in log
frequency between covered cells) or, with *to step-1 reference*, from the
reference band's level (corrected for the trims, plus the sweep's level
relative to the reference band, default 0 dB; assumes the sweep is recorded at
the reference velocity). The RIAA curves are the analytic ones
(`neo_measure::riaa::analog_response`, optional IEC roll-off). Only points
inside the profile's sweep range are used. Per channel and for the average (mean
of the channels that carry the sweep: only L for an L-only sweep, only R for
R-only, both otherwise; averaged per grid cell) the page shows the curve on a
+-3/6/12 dB scale, maximum / minimum / RMS deviation over the covered range and
over 1-15 kHz, optional 1/12, 1/6 or 1/3 octave smoothing (applied to the
display and the export).

**Export format** (*Copy curve (JSON)*), read by the EQ plugin's loop
calibration as playback-chain correction:

```json
{"version":1,"kind":"response_curve","name":"<text>","points":[{"freq_hz":800.0,"db":-0.4}, ...]}
```

`points` ascending in frequency, `db` = deviation from nominal (0.01 dB,
frequency 0.1 Hz), only covered frequencies (for this record >= 800 Hz); the
average of L and R by default, or L or R alone. The last captured sweep is
persisted in the plugin state.

#### Step 3: speed and wow & flutter

W&F of the profile's reference tone (default 1000 Hz) on L or R (3..30 s):
speed deviation, and peak, RMS and 2-sigma, unweighted and weighted
(`neo_measure::wow_flutter`). **Standards specify a 3150 Hz carrier. A 1 kHz
carrier and the fitted (not standard-verified) weighting curve make these
numbers indicative only.** The page says so.

#### Step 4: tracking ability

Per band (amplitude `A` um at `f` Hz): per channel level, THD, THD+N. Expected
peak velocity `v = 2 pi f A` (315 Hz, 50 um = 9.90 cm/s) and expected level
step versus the previous band `20 log10(A / A_prev)` (50 -> 60 um = +1.58 dB).
A channel that carries the band (within 20 dB of the stronger one) *mistracks*
when its THD exceeds the limit (default 3 %), when its THD rose by more than
6 dB versus the previous band (and is above a tenth of the limit), or when the
measured level step falls short of the expected one by more than 1 dB. The
highest clean amplitude is the largest one for which it and every lower
measured band is clean.

#### Step 5: summary

Levels, trims, balance, separation both ways, speed, response deviation and
covered range, W&F, tracking result as text.

#### Limitations of the test-record tools

* The Ortofon values are unverified against the sleeve (public listings).
* This record has nothing below 800 Hz: the playback response below 800 Hz is
  **not** verified, and the exported curve contains no points there.
* W&F from a 1 kHz carrier is indicative (above).
* The velocity calibration assumes the quoted band velocity is the channel
  velocity; the groove velocity at the stylus also depends on the cartridge
  geometry and stylus compliance, none of which is measured.
* Sweep levels assume a velocity-sensitive (moving magnet/coil) cartridge.
* The tone test needs a clean single sinusoid: sweeps with strong hum or
  rumble (residual above the threshold) are rejected block by block; lower the
  tone threshold if too little is covered.
* The editor page is only layout-tested headlessly (no rendering), the audio
  analysis is tested with synthetic signals, not a real record or host.

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
* The pre-emphasis comes from `neo_measure::riaa` (design for the host rate in
  `initialize()`).
