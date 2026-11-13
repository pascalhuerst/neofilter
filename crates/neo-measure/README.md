# neo-measure

Pure-DSP metering and measurement library (f64 internally, `f32`/`f64` slices
accepted where noted, no GUI, no plugin dependencies, no global state, no
`unsafe`). Every meter is verified in the unit tests against signals with an
analytically known answer; the numbers are in the "Verification" section.

**Realtime classes.** *RT* = allocation-free after construction and panic-free
in `process`. *Off* = analysis that allocates; run it on a GUI/worker thread.

| module | class | purpose |
|---|---|---|
| `levels` | RT | sample peak (hold/decay), BS.1770-4 true peak, RMS, crest, DC |
| `loudness` | RT (+ gating statistics off-thread) | K-weighting, M/S/I loudness, LRA |
| `stereo` | RT | correlation, M/S, balance, goniometer, LF vertical/lateral |
| `generator` | RT (pink construction fits a filter: build off-thread) | sine, log sweep, white/pink noise, dual tone, gated start/stop |
| `spectrum` | Off (push is alloc-free) | Welch FFT, averaging, peak hold, smoothing, 1/3-octave, log display bins |
| `delay` | Off | GCC delay estimation up to seconds, polarity, confidence |
| `transfer` | Off | H1 transfer function, phase, coherence |
| `tone` | Off | tone frequency/level, THD, THD+N, channel separation |
| `wow_flutter` | Off | FM demodulation of a 3150/3000 Hz (or any) carrier, weighted/unweighted |
| `riaa` | design Off, cascades RT | analytic RIAA curves, minimum-phase digital pre-/de-emphasis |
| `velocity` | RT | groove amplitude/velocity, RMS/peak, 45/45 channel-lateral conversions, reference-level calibration, modulation detection |
| `sweep_track` | Off | level vs frequency of gliding sweeps and spot tones, nominal characteristics, deviation curves |
| `weighting` | RT filters | A-weighting, wow/flutter weighting, noise-level helper |
| `filter`, `util` | RT / Off | biquads, LR4 low-pass, windows, dB helpers |

Conventions: dBFS peak = `20 log10 |x|` (full-scale sine = 0 dBFS for spectra
and tone analysis); RMS meters report `20 log10 rms` (full-scale sine =
-3.01 dB) and offer a sine-referenced variant; noise generators are specified
by RMS level, tone generators by peak level. Silence is reported as
`f64::NEG_INFINITY` (loudness) or `-400 dB` (`util::DB_FLOOR`) elsewhere.
Short/empty input returns `None` or neutral values, never panics.

## Usage sketches

```rust
use neo_measure::{levels::*, loudness::*, stereo::*, generator::*, spectrum::*,
                  delay::*, transfer::*, tone::*, wow_flutter::*, weighting::*};
use neo_measure::util::Window;

// levels (audio thread)
let mut peak = PeakMeter::new(fs, 1.0 /*hold s*/, 20.0 /*dB/s*/);
let mut tp = TruePeakMeter::new(fs);          // per channel
peak.process(&block); tp.process(&block);
let (p_db, tp_db) = (peak.peak_db(), tp.true_peak_db());
let mut rms = RmsMeter::window(fs, 0.3);      // or ::exponential(fs, tau)
rms.process(&block); let _ = rms.rms_db_sine_ref();

// loudness
let mut lm = LoudnessMeter::new(fs, 2);
lm.process_stereo(&l, &r);
let (m, s) = (lm.momentary_lufs(), lm.short_term_lufs());
let i = lm.integrated_lufs();                 // Option<f64>, gated
let lra = lm.loudness_range();                // Option<LoudnessRange>

// stereo
let mut st = StereoMeter::new(fs, 0.3);       // correlation / M / S / balance
st.process(&l, &r);
let mut lf = LowFreqStereo::new(fs, LowFreqStereo::DEFAULT_CROSSOVER_HZ, 0.3);
lf.process(&l, &r);                           // lf.correlation(), lf.vertical_db(), lf.lateral_db()
let mut gon = Goniometer::new(2048, 4);       // gon.process(&l,&r); gon.copy_points(&mut out)

// generators
let mut noise = Gated::new(PinkNoise::new(fs, -20.0, 1234), fs, 0.02);
noise.start(); noise.fill(&mut buf);          // click-free ramps; stop() fades out
let mut sweep = LogSweep::new(fs, 20.0, 20_000.0, 10.0, -6.0, false);

// spectrum
let mut cfg = SpectrumConfig::new(fs, 16384, Window::FlatTop);
cfg.averaging = Averaging::Exponential(0.5);
let mut an = SpectrumAnalyzer::new(cfg).unwrap();
an.push(&block);
let db = an.amplitude_db();                   // dBFS (peak-referenced), peak_amplitude_db() for hold
let smooth = smooth_octave_power(&an.amplitude_power(), 6);          // 1/6 octave
let disp = resample_log_power(&smooth, an.bin_hz(), 20.0, 20e3, 512, Combine::Mean);
let bands = an.third_octave_db(20.0, 20_000.0);

// delay + transfer (loop calibration)
let d = estimate_delay(&reference, &ret, &DelayOptions::new((5.0 * fs) as usize)).unwrap();
let mut tcfg = TransferConfig::new(fs, 65536);
tcfg.delay_samples = d.delay_samples;
let mut est = TransferEstimator::new(tcfg).unwrap();
est.push(&ref_block, &ret_block);             // streaming; or estimate_transfer(&x, &y, &tcfg)
let r = est.result().unwrap();                // r.magnitude_db(), r.phase_rad(), r.coherence
let (f, mag) = r.on_log_grid(&r.smoothed_magnitude_db(6), 20.0, 20e3, 400);

// tone, wow & flutter, weighting
let t = analyze_tone(&capture, &ToneConfig::new(fs)).unwrap(); // t.frequency_hz, t.thd_percent, ...
let xt = channel_separation_db(&driven, &other, fs);
let wf = analyze_wow_flutter(&tone3150, &WowFlutterConfig::new(fs, 3150.0)).unwrap();
let noise = measure_noise(&silence, fs, 10.0, 20_000.0, -20.0).unwrap();
let mut aw = AWeighting::new(fs);
```

```rust
use neo_measure::{velocity::*, sweep_track::*};

// test-record calibration
let v = amplitude_um_to_velocity_cm_s(315.0, 50.0);          // 9.896 cm/s peak
let step = level_step_db(50.0, 60.0);                        // +1.58 dB
let v_ch = channel_velocity_peak(Modulation::Left, rms_to_peak(5.0)); // 7.071
let reference = reference_level_dbfs(-18.0, v_ch);           // -24.02 dBFS
let mut tr = SweepTracker::new(SweepConfig::new(fs), 2).unwrap();
tr.push(&l, Some(&r));                                       // streaming
let curve = tr.curve(0);                                     // covered cells (freq, level)
let pair = tr.pair();                                        // L-R level and phase -> detect_modulation
let pts: Vec<(f64, f64)> = curve.iter().map(|c| (c.freq_hz, c.level_db)).collect();
let off = norm_offset_at(&pts, Characteristic::ConstantVelocity, true, false, 1000.0).unwrap();
let dev = deviation_points(&pts, Characteristic::ConstantVelocity, true, false, off);
```

### Velocity conventions (`velocity`)

All peak values. `v = 2 pi f A` (315 Hz, 50 um = 9.896 cm/s peak);
`v_peak = sqrt(2) v_rms`. 45/45: a *channel velocity* `v_ch` is the motion of
one groove wall along its direction; one channel alone has lateral and
vertical components `v_ch / sqrt 2`; the same in-phase signal in both channels
is purely lateral with `v_lat = sqrt(2) v_ch` (anti-phase: vertical). The
calibration reference (5 cm/s peak lateral) therefore corresponds to
`v_ch = 5 / sqrt 2 = 3.536 cm/s` per channel and
`reference_level = level_ch - 20 log10(v_ch(record) / 3.536)`; the 5 cm/s RMS
single-channel band (7.071 cm/s peak) gives `level_ch - 6.02 dB`.
`detect_modulation(L-R level difference, phase)`: `> 12 dB` L or R only;
within 6 dB and `|phase| < 45 deg` lateral, `> 135 deg` vertical, else `None`.
`tone::phase_difference_deg` gives the phase of two channels at a frequency.

### Sweep tracking (`sweep_track`)

Overlapping (87.5 %) Hann blocks of a bank of lengths (each 4x shorter than
the previous; 8..32 cycles of the tone per block; no block shorter than 64
samples). Per block: 2x zero-padded FFT peak with log-parabolic interpolation,
chirp rate from the peak frequencies of the two half blocks, level from the
windowed DTFT at `(f, k)` after de-chirping, acceptance if the residual power
is at least `min_snr_db` (default 25) below the fitted sinusoid (the mean is
removed first). Accepted `(f, level)` go to a log grid (24 / octave from
`f_lo`); the cell value is the median level at the median frequency and a cell
is *covered* with >= `min_count` (2) accepted blocks. Two-channel trackers add
the level difference and phase (L relative to R) of blocks in which both
channels are accepted. Nominal characteristics, normalisation, octave smoothing
and deviation statistics are in the same module; the characteristic table is in
its rustdoc and in the NeoMeter README.

See the module-level rustdoc for exact semantics, limits and conventions.

## Verification (measured, `cargo test -p neo-measure -- --nocapture`)

* Pink noise: fitted pole/zero cascade deviates < 0.002 dB from -3.01 dB/oct
  over 20 Hz..20 kHz (44.1/48/96/192 kHz); measured spectrally, third-octave
  band levels of 60 s of pink noise spread 0.18 dB (statistical).
* Spectrum: flat-top reads a 0 dBFS sine within 0.002 dB at any frequency;
  white-noise density within 0.05 dB (Hann/BH/flat-top).
* Delay: errors < 0.0001 samples clean; with +-20 dB tilt, SNR 0 dB, polarity
  inversion and attenuation < 0.005 samples; narrow band-pass (0.05-0.2
  Nyquist) 0.01-0.16 samples. Delays up to 2.0 s at 48/96 kHz.
* Transfer: noiseless pink excitation, fractional delay, 20 Hz-20 kHz:
  1/6-octave smoothed magnitude error 0.006 dB (single FFT bins < 0.02 dB), phase < 0.3 degree;
  coherence equals SNR/(1+SNR) within 0.001 for -5, 0 and +10 dB.
* True peak: fs/4 sine at 45 degrees reads +3.0105 dBTP (ideal 3.0103) at
  44.1/48/88.2/96 kHz.
* Loudness: K-weighting reproduces the published 48 kHz coefficients to 1e-15;
  -23 LUFS stereo sine reads -22.993 (48 kHz); gating sequences -23.014;
  LRA cases give exactly 10 / 5 / 20 / 15 LU.
* Tone: frequency error < 2e-6 Hz (1 s blocks), -60/-70 dB harmonics read
  to 0.001 dB, level error < 0.005 dB.
* A-weighting: < 0.06 dB from the analog curve at 44.1/48 kHz, < 0.01 dB at 96 kHz+.
* Wow and flutter: 0.1 % at 4 Hz reads 0.1003 % weighted peak, WRMS 0.0707 %,
  2 sigma 0.1414 %; 0.5/1/10/30 Hz follow the curve within 3 %; speed error
  to 1e-4 %.
* Sweep tracking (48 kHz, 50 dB SNR white noise, a known EQ with +-2 dB ripple
  and 1.5 dB/oct tilt): gliding log sweeps 800 Hz..20 kHz (30 s) and 20 Hz..
  20 kHz (45 s), up and down, every covered cell within 0.005 dB of the EQ
  (108 and 222 covered cells, none outside the swept range; the 20 Hz sweep
  is asserted covered from 30 Hz); stepped 1/3-octave spot tones (0.6 s each)
  separated by silence, white-noise bursts and voice-like harmonic bursts give
  exactly one covered cell per tone, within 0.05 dB; a constant-velocity sweep
  through the analytic RIAA de-emphasis reads -0.008..+0.009 dB after the
  nominal-characteristic conversion (the raw curve spans > 15 dB); noise,
  silence and a two-tone signal are rejected; stereo: a -28 dB in-phase copy
  reads 28.0 dB / 0 deg, an anti-phase copy reads 0 dB / 180 deg.
* Velocity: 315 Hz 50 um = 9.896 cm/s, level steps 50 -> 60 -> 70 um +1.584 /
  +1.339 dB, 5 cm/s RMS = 7.071 cm/s peak, reference conversion -6.0206 dB.
* Inter-channel phase: within 0.01 degree at 1002 Hz.

## Known limitations

* `sweep_track` needs a clean single sinusoid per block (residual below the
  threshold): hum or rumble close to the tone's level, or tones changing faster
  than a few percent within a block (a few cycles of block length), are
  rejected rather than mis-measured. Cells below about 1.3 octaves above `f_lo`
  of a fast sweep may be sparsely covered (long blocks, few per cell). The
  chirp is modelled as linear within a block.
* The wow/flutter weighting curve is a *fit* (peak 4 Hz, -6 dB at 0.8 and
  20 Hz) because the standard's tabulated mask was not available; the
  quasi-peak ballistics (10 ms / 1.5 s) are likewise unverified.
* EBU Tech 3341/3342 are covered by synthesised equivalents (1 kHz stereo
  sine segments), not by the official WAV files; multichannel (5.0) cases and
  the true-peak cases of Tech 3341 (15-23) are not reproduced one by one.
* True-peak filter is a 32-tap-per-phase Kaiser design, not the standard's
  48-tap example; passband flat to ~0.4 fs.
* H1 estimation has a small window-leakage bias at sharp low-frequency
  features that shrinks with FFT size (use 65536 points for 20 Hz detail).
