# Loop calibration

NeoFilter can measure the whole vinyl cutting loop with noise and fit its own
EQ bands so that the loop becomes flat:

```
programme -> NeoFilter -> RIAA pre-emphasis -> cutter
                 ^                                |
                 |  (sidechain)                   v  groove, played back
                 +------ phono preamp <---- pickup  a quarter turn later
```

The cut groove is played back 0.3-0.5 s later (plus converter latency). The
return of the phono preamp goes into NeoFilter's **sidechain input**. While
you measure, NeoFilter replaces its input with noise, so the noise passes
through the current EQ like programme, and records noise and return.

> **Safety.** The measurement sends noise to the cutter. Excess high-frequency
> level destroys a cutter head. Keep the cutter chain's own protection
> (limiter, HF limiter) active, keep the noise level at the default or lower,
> and press **Stop** at any doubt. Noise never starts by itself: only the
> Measure and Auto buttons start it, it is never stored in the project state,
> never an automatable parameter, and it stops on Stop, on any error, when the
> editor is closed, on reset and on deactivate. The "NOISE ON" indicator in the
> top bar is lit whenever noise is audible. The level is capped at -6 dBFS RMS;
> the correction is limited to +6 dB boost / -12 dB cut by default.

## DAW routing

NeoFilter has a stereo layout with an optional stereo sidechain input called
"Loop return" (the first layout, "Stereo with loop return sidechain"). The
plain stereo and mono layouts without sidechain are still offered, and state
saved by earlier versions loads unchanged.

1. Insert NeoFilter on the programme track, before the RIAA plugin.
2. Record-arm / monitor the input that carries the phono preamp output on an
   audio track (low-latency monitoring on) and route that track to NeoFilter's
   sidechain input (in the host: sidechain source of the plugin = that track).
3. The track that carries the return must be monitored with the same
   converters that the cutter chain uses, otherwise their latency is included
   (this is fine: it is part of the measured delay, up to 5 s).
4. Keep the plugin on a stereo layout with sidechain; the Calibrate panel
   reports "return too low" if the sidechain is not connected.

## Procedure

1. **Level the playback input.** Play a test record and set the preamp output
   and the interface input gain with the level meter (NeoMeter). The return
   should read clearly above -60 dBFS RMS and stay below clipping (peaks below
   -0.1 dBFS); about -20 to -12 dBFS for the noise is comfortable.
2. **Playback curve (optional, recommended).** The loop includes the pickup
   and the phono preamp, which are not ideal. If you know their deviation
   from nominal (from the test record, measured in NeoMeter), copy its curve
   JSON and in the Calibrate panel press **Paste curve**, then Ctrl+V. The
   calibration then flattens the *cutting chain* instead of the playback
   chain. The curve is stored with the plugin state; **Clear** removes it.
3. Open the **Calibrate** panel (top bar). Check the options below.
4. Put the stylus in the groove of a *test cut* (a blank section you may
   waste). Press **Measure** for a single measurement, or **Auto**.
5. Manual: **Measure**, look at the curves and readouts, press **Fit**, then
   **Apply**; repeat Measure/Fit/Apply until the deviation is small. Auto does
   exactly that: Measure, Fit, Apply, wait (settle time), repeat, and stops
   when the in-range RMS deviation is below the threshold (0.5 dB), after 5
   applies, or on any safety stop. The history shows delay, RMS and maximum
   deviation, and band count for each measurement.
6. Press **Stop** whenever you want the noise gone immediately.

The first **Fit** reserves every band that is unused at that moment as a
*calibration band*; existing bands are *manual* bands and are never touched
(they are part of the measured chain). With **Replace all** every band becomes
a calibration band. **Clear calibration** unuses the calibration bands and
forgets the reservation.

## Options (defaults)

| option | default | notes |
|---|---|---|
| Noise | pink | pink or white |
| Level | -20 dBFS RMS | -80 ... -6 (hard cap -6) |
| Channels | L+R mono | left, right, or identical noise on both |
| Replace input / Add | replace | replace mutes the programme on the used channel(s) |
| Duration | 12 s after the delay | 4-60 s; the first `delay + 0.5 s` is skipped |
| Range | 30 Hz - 16 kHz | corrections fade to 0 dB over half an octave outside |
| Max boost / max cut | +6 dB / -12 dB | applied to the *total* calibration curve |
| Smoothing | 1/6 octave | 0 = none |
| Max bands | 12 | at most the number of free calibration slots |
| Replace all | off | every band is rewritten |

Fixed behaviour (settings structure, not in the panel): reference band
300 Hz - 3 kHz for the 0 dB level; coherence below 0.7 gets zero weight in the
fit; mean in-range coherence below 0.5 aborts; return RMS below -60 dBFS or a
peak at or above -0.1 dBFS aborts; delay confidence below 3 means "no loop
detected"; auto settle time `2 x delay + 1 s`; auto threshold 0.5 dB RMS; auto
maximum 5 applies.

## Safety stops

* Return below -60 dBFS RMS: "return level too low" (nothing connected, stylus
  not in the groove, sidechain not routed).
* Clipping (return peak at or above -0.1 dBFS): the noise stops immediately,
  also in the middle of a measurement.
* No correlation between sent noise and return (delay confidence too low):
  "no loop detected".
* Mean in-range coherence below 0.5 (stylus lifted, strong noise): abort.
* Bypass switched on, audio engine stopped, capture not drained (editor
  closed), plugin reset or deactivated: the measurement is aborted and the
  noise stops.
* After any of these nothing is applied and no fit exists. Apply also refuses
  when a band was edited after the measurement.
* Auto stops when the loop gets worse instead of better.
* Boost and cut limits are enforced on the summed curve of all calibration
  bands from 10 Hz to Nyquist, not per band (tolerance +0.25 dB).

## What is measured

* **Delay** per return channel by generalised cross-correlation (PHAT
  weighted, 0-5 s, sub-sample refinement), with a peak-to-sidelobe
  confidence and polarity (an inverted return is reported, harmless). During
  the capture a first estimate is made after about 2.5 s so that the
  measurement can stop `duration` seconds after the delay is known.
* **Transfer function** `H = Sxy / Sxx` reference -> return (Welch, Hann, 75 %
  overlap, FFT 16384 at 48 kHz, scaled with the rate), delay compensated,
  magnitude smoothed (power average, default 1/6 octave) on a log grid of 48
  points per octave from 20 Hz, plus the coherence. The reference is the
  noise *before* the EQ, so `H` is the loop including the current EQ.
* Both return channels are measured; the power average is used.

## The maths in brief

* `M(f)` = measured loop response (includes the EQ the noise passed through).
* `P(f)` = playback curve (0 if none; outside its covered range it tapers
  linearly in log frequency to 0 dB over one octave beyond the first/last
  point, an end value is never held).
* `D(f) = (M - P)` minus its mean over the reference band: the deviation of
  the loop from flat.
* Manual bands are part of `M`. `E_cal(f)` is the summed response of the
  calibration bands at measurement time.
* `E_cal_new = clamp(E_cal - D * taper(f), -max_cut, +max_boost)`; `taper` is 1
  in the range and falls with cos^2 to 0 over half an octave outside. Points
  with coherence < 0.7 are interpolated from valid neighbours and get zero
  weight.
* `neofilter-dsp::autofit` fits `E_cal_new` with up to N bells (12 dB/octave,
  Q 0.3-10) and optionally one low and one high shelf: greedy peak placement,
  then joint bounded Levenberg-Marquardt on the real digital responses, with a
  final pass that scales gains until the summed curve respects the limits.
  It is deterministic.
* Predicted residual = `D + (fitted - E_cal)`; the panel draws it next to the
  target and fitted curves, the measured loop, and the coherence.

## CLI workflow

The same analysis and fit run offline:

```
neofilter-cli calibrate --sent sent.wav --return ret.wav \
    [--preset <id> | --preset-file current.json] [--sent-is-post-eq] \
    [--playback-curve curve.json] [--max-boost 6] [--max-cut 12] \
    [--range 30:16000] [--max-bands 12] [--smoothing 6] \
    [--replace-all] [--cal-slots 13-32] [--output fitted.json]
```

* `sent.wav` is the noise as sent into NeoFilter (before the EQ), `ret.wav` the
  recorded return (mono or stereo, same sample rate, no clipping). Use
  `--sent-is-post-eq` if `sent.wav` was recorded after the EQ.
* `--preset`/`--preset-file` describes the EQ the noise passed through
  (default: none). Unused bands become calibration bands unless `--cal-slots`
  (1-based) or `--replace-all` say otherwise; for a second pass on a fitted
  preset give the slots the first pass used.
* It prints delay, polarity, levels, coherence, the deviation before and
  predicted after the fit, and the fitted bands, and writes the complete
  preset JSON (`--output`, otherwise printed) for **Paste preset**.
* On any safety stop it prints the reason and writes nothing.

## Limitations

* The measurement assumes a time-invariant loop; wow, flutter and noise from
  the groove lower the coherence. Measure in a quiet section.
* The loop is measured through one stylus at one groove radius; radius
  dependent losses (HF loss towards the inside) are not modelled.
* Band shapes: bells and at most one low and one high shelf per fit. Fine
  structure narrower than Q 10 or below the smoothing resolution is not fitted.
* Phase is not corrected (minimum-phase EQ only); the measurement is a
  magnitude calibration.
* The correction is limited to the range and to the boost/cut limits; if the
  loop needs more, the residual is shown and not hidden.
* The playback curve is only as good as the test record; it is applied as a
  plain subtraction.
* Noise injection is at the plugin input: the programme on the used channel is
  muted ("Replace input") or mixed ("Add"). It does not model a cutter's
  own limiter or compression.
* The sidechain layout is declared first, so a new instance gets the sidechain
  port by default. Hosts that do not support sidechain on this plugin format
  can use the layouts without it, but then calibration cannot run live (the CLI
  still works).
* GUI behaviour and live loops have been exercised in simulation only (see the
  tests in `crates/neofilter/tests/calibration_loop.rs`).
