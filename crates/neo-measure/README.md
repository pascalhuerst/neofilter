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
| `wow_flutter` | Off | FM demodulation of a 3150/3000 Hz tone, weighted/unweighted |
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

## Known limitations

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
