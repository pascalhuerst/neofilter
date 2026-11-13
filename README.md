# NeoFilter suite

A suite of three audio plugins (CLAP and VST3, written in Rust) for equalising
and for calibrating and running a vinyl cutting chain. Developed on Linux
(Bitwig first); it also builds for macOS and Windows.

| Plugin | What it is |
|---|---|
| **NeoFilter** | 32-band parametric EQ whose band parameters follow Pro-Q 3 conventions, with an automatic loop calibration for cutting chains |
| **NeoRIAA** | RIAA pre-emphasis (for cutting) and de-emphasis (for playback) |
| **NeoMeter** | Metering and measurement: levels, loudness, stereo, spectrum, vinyl-specific meters, test tones and a test-record calibration |

Under them sits **neo-measure**, the pure-DSP measurement library
([README](crates/neo-measure/README.md)).

> **Status:** version 0.1 is feature complete but young. Nothing has been
> verified in a real host yet beyond a `clap-validator` run on NeoFilter early
> in development, and the editors (GUIs) have not been reviewed visually; their
> layout and scaling code is only unit-tested. Read the
> [Known limitations](#known-limitations) before relying on it for a cut.

## Not affiliated with FabFilter

NeoFilter is an independent, clean-room implementation built from published
DSP literature (see [How it works](#how-it-works)) and black-box measurements
of audio output. It is not affiliated with or endorsed by FabFilter and
contains no FabFilter code. "FabFilter" and "Pro-Q" are trademarks of their
respective owner and are used here only to describe parameter compatibility.

## What it is for

Equalising, and the chain around a lathe: the programme goes through an EQ,
RIAA pre-emphasis and the cutter amplifier into the groove, and a stylus plays
it back a quarter turn later. The suite helps you level and verify the
playback input, apply an accurate pre-emphasis, and measure the whole loop and
fit EQ bands that make it flat.

```
program ─► NeoFilter (EQ + calibration) ─► NeoRIAA (pre-emphasis) ─► cutter amp / head
                 ▲ sidechain "Loop return"                                  │ groove
                 └──────── phono preamp ◄── playback stylus ◄───────────────┘ (~1/4 rev later)

NeoMeter sits anywhere you want to look at a signal, typically on the return.
```

## The plugins

### NeoFilter

- 32 bands, each with used, enabled, shape, frequency (10 Hz to 30 kHz), gain
  (±30 dB), Q (0.025 to 40), slope and stereo placement (Stereo / Left / Right
  / Mid / Side).
- Shapes: Bell, Low Shelf, Low Cut, High Shelf, High Cut, Notch, Band Pass,
  Tilt Shelf, Flat Tilt. Slopes 6, 12, 18, 24, 30, 36, 48, 72, 96 dB/oct.
- Zero latency (minimum phase), 64-bit internal processing, click-free
  parameter changes. Output gain (±36 dB), gain scale (0 to 200 %), soft
  bypass mapped to the host.
- Resizable editor (default 900x560): sum and per-band curves, draggable
  nodes, band strip with text entry, pre/post spectrum analyzer, preset menu,
  Copy preset / Paste preset.
- Factory presets "Default", "Friend EQ A", "Friend EQ B" and "Friend EQ A+B
  (series)".
- **Loop calibration:** an optional stereo sidechain input "Loop return" and a
  Calibrate panel that measures the cut-to-playback loop with noise, then
  fits and applies EQ bands to flatten it. See
  [docs/calibration.md](docs/calibration.md) for routing, procedure, options,
  safety stops and the maths.
- Layouts: stereo with the loop-return sidechain, plain stereo, mono. Plain
  layouts load state from earlier versions unchanged.
- Design rules measured from the reference EQ: see
  [Accuracy](#accuracy) and [docs/reference-matching.md](docs/reference-matching.md).

Not included: linear-phase and natural-phase modes, dynamic EQ, brickwall
slope, EQ match, band solo, auto-gain, surround.

### NeoRIAA

RIAA pre-emphasis ("inverse RIAA", the default) and de-emphasis plugin for
the cutting path. No limiter and no soft clipper: only a peak meter and a clip
indicator, so nothing alters the signal behind your back.

- Options: IEC 7950 µs low-frequency roll-off, 3.18 µs ultrasonic pole, output
  trim (-36 to +12 dB), soft bypass. Stereo and mono, zero latency, 64-bit.
- Magnitude within about 0.007 dB of the analytic curve from 20 Hz to 20 kHz at
  every tested sample rate (44.1 to 384 kHz). **Phase does not reach the
  original targets below 176.4 kHz**: the worst phase deviation in 20 Hz to
  20 kHz is 71 degrees at 44.1 kHz and 61 degrees at 48 kHz (without the
  3.18 µs pole), 11 to 14 degrees at 88.2 and 96 kHz, and below 0.4 degrees
  from 176.4 kHz. A minimum-phase IIR cannot do better, the reason and the
  full table are in the [NeoRIAA README](crates/neoriaa/README.md). The effect
  is a small change of waveform shape on transients, not of tonal balance.
- Above 20 kHz the digital pre-emphasis keeps rising (as the analog one does)
  and is a few dB higher than it near Nyquist; enable the pole if the
  amplifier should not see that.

### NeoMeter

A pass-through metering and measurement plugin (stereo or mono, zero latency;
bit-exact while the generator is off and the trims are 0 dB). Pages:

- **Levels & Loudness:** sample peak, true peak, RMS, crest factor, DC;
  ITU-R BS.1770-4 momentary, short-term and integrated loudness, loudness
  range.
- **Stereo:** goniometer, correlation, balance, mid/side, low-frequency
  vertical versus lateral with warnings.
- **Spectrum:** Welch FFT up to 65536 points, averaging, hold, smoothing,
  1/3-octave RTA.
- **Vinyl:** levels after RIAA pre-emphasis, high-frequency load indicator,
  estimated groove velocity and displacement, vertical bass.
- **Test:** tone generator (sine, sweep, noise), tone frequency / level / THD
  / THD+N, wow and flutter, channel separation, noise and rumble.
- **Test record:** input calibration with a test record: input trims,
  reference level and balance, playback frequency response, speed and wow &
  flutter, tracking ability. The built-in profile is for the Ortofon Test
  Record, with values taken from public listings and **not verified against
  the sleeve**. The playback-response curve can be copied as JSON and pasted
  into NeoFilter's calibration.

Details and definitions: [NeoMeter README](crates/neometer/README.md).
The generator never starts from saved state.

## Calibrating a cutting chain

In this order:

1. **Level and verify the playback input with a test record in NeoMeter.** Set
   the preamp and interface gain, apply the input trims, check balance,
   crosstalk, speed, playback response and tracking on the *Test record* page.
   Copy the playback-response curve if you want the calibration to flatten the
   cutting chain instead of the playback chain.
2. **Put NeoRIAA (pre-emphasis) in the cutting path,** after NeoFilter and in
   front of the cutter amplifier. Use IEC and the ultrasonic pole only if your
   cutting chain expects them.
3. **Run the loop calibration in NeoFilter.** Route the phono preamp return to
   its sidechain input, put the stylus in a test cut and press Measure or
   Auto in the Calibrate panel. Default safety limits: pink noise at -20 dBFS
   (hard cap -6 dBFS), correction range 30 Hz to 16 kHz, at most +6 dB boost
   and -12 dB cut. Noise only ever starts from the buttons, never from saved
   state or automation. **Keep your cutter's own protection active:** excess
   high-frequency level can destroy a cutter head.

The calibration needs the plugin editor to be open while it measures. It
corrects magnitude only. The same analysis runs offline with
`neofilter-cli calibrate` on recordings.

## Accuracy

**Matched to the reference EQ by measurement** (see
[docs/reference-matching.md](docs/reference-matching.md)):

- Bell (all slopes) and even-order High Cut (12, 24, 36, 48, 72, 96 dB/oct):
  198 measured single-band cases at 44.1 / 48 / 96 kHz, worst deviation
  0.017 dB, Bell cases worst 0.006 dB, median 0.0001 dB.
- Default-Q 12 dB/oct Low / High Shelf: within 0.001 dB of the measured
  curves over the whole measured range.
- Shelf resonance law (section Q follows displayed Q^0.47) and the tilt-shelf
  convention (below) were corrected to the measured behaviour. Shelves at
  other Q or 24 dB/oct, and tilt shelves, match the measurements within 0.01 dB
  up to 0.1 x the sample rate; above that they use the analog-matched design,
  whose behaviour in the reference EQ was not measured.
- Total curves of the factory presets agree with the measured ones to 0.016 dB
  or better between 40 Hz and 20 kHz (0.0009 to 0.016 dB depending on preset
  and sample rate).

Tests: `crates/neofilter-dsp/tests/reference_curves.rs`,
`reference_shelves.rs` and `crates/neofilter/tests/reference_presets.rs`.

**Remaining limitations:**

- Low Cut, Notch, Band Pass, Flat Tilt, odd-order cuts (6, 18, 30 dB/oct) and
  band frequencies close to Nyquist still use the analog-matched design and
  have not been compared with the reference EQ. High-Q shelves and cuts, wide
  notches and 96 dB/oct band passes can deviate from their analog prototype by
  several dB there (up to about 15 dB in the extreme corners next to Nyquist).
  The editor curve shows the actual digital response. Enforced tolerances:
  [`accuracy.rs`](crates/neofilter-dsp/tests/accuracy.rs).
- Bell and High Cut settings far from the measured grid are extrapolations of
  the fitted rule.
- Measurements were taken at 44.1, 48 and 96 kHz only.

## Known limitations

- **Nothing has been verified in a real host** beyond `clap-validator` on
  NeoFilter earlier in development. Treat live behaviour (sidechain routing,
  state restore, editor input) as untested.
- **The GUIs were not reviewed visually.** Layout and scaling helpers are
  unit-tested; painting code was never looked at by a person.
- Calibration needs the NeoFilter editor open while measuring, and has been
  exercised against simulated loops, not a real lathe.
- Wow & flutter weighting in `neo-measure` is a fit to the standard's curve
  (the tabulated mask was not available) and its quasi-peak ballistics are
  unverified. W&F from a 1 kHz carrier (the Ortofon record has no 3150 Hz
  band) is indicative only.
- Loudness and true peak are verified with synthesised equivalents of the EBU
  Tech 3341/3342 cases, not the official files.
- The Ortofon Test Record profile uses values from public listings, not from
  the sleeve. The record has no sweep below 800 Hz, so the playback chain is
  unverified below that.
- NeoRIAA phase deviates at low sample rates, see above.
- Velocity and displacement in NeoMeter are estimates that depend on the
  reference calibration.

## Using NeoFilter

### Transferring a Pro-Q 3 setting

To transfer a setting from a Pro-Q 3 band table, create a band and type the
values into the selected-band strip (click a value and type), or enter them
via your host's parameter editor:

| Table column | NeoFilter control |
|---|---|
| Shape | Shape |
| Freq (Hz) | Frequency (Hz or kHz) |
| Gain (dB) | Gain |
| Q | Q (displayed value, 1.00 = Butterworth on a 12 dB/oct cut) |
| Slope (dB/oct) | Slope |
| On / off | Band enable |
| Band number | Band N (placement defaults to Stereo) |

Parameter ids (`freq_{n}`, `gain_{n}`, `q_{n}`, `used_{n}`, `on_{n}`,
`shape_{n}`, `slope_{n}`, `place_{n}` with n = 1..32, plus `out_gain`,
`gain_scale`, `bypass`) are stable, so saved sessions and automation survive
updates.

Editor interactions:

- Drag a node: frequency and gain (Shift: fine, Ctrl/Cmd: lock to one axis).
- Mouse wheel over a node: Q.
- Double-click empty space: add a band in the first free slot (a low cut at
  the far left, a high cut at the far right, otherwise a bell).
- Double-click a node: reset its gain to 0 dB.
- Right-click a node: enable, shape, slope, placement, remove. Right-click
  empty space: add a band.
- Selected-band strip: enable, shape, remove, frequency, gain, Q, slope,
  placement, with previous/next band buttons.
- Top bar: preset menu, Copy preset (current settings as JSON to the
  clipboard), Paste preset (click it, then press Ctrl+V to apply a copied
  preset), gain scale, output gain, bypass, analyzer switch and dB range.

Bypass is a soft bypass (10 ms crossfade, no latency). The processing runs at
zero latency.

The factory presets "Friend EQ A" (15 bands) and "Friend EQ B" (9 bands) are
reproductions of two such tables and serve as worked examples; their values
are listed in [PLAN.md](PLAN.md#factory-presets). Applying a preset resets unused bands.

"Friend EQ A+B (series)" holds all 24 bands of the two presets in one
instance, with 8 spare bands. EQs in series are linear, so one instance with
all the bands has the same response as two instances in series (A then B);
it replaces the two instances. The test suite checks this: the responses add
up in dB to 1e-9 and the f64 engine output is identical to the two-engine
chain (the f32 plugin path differs only by f32 rounding, about 1e-7 relative).

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

Gain scale multiplies the dB gain of every Bell, Shelf and Tilt band, so you
can dial an overall EQ amount up or down.

Tilt Shelf: the Gain value is the total tilt. A +6 dB tilt shelf runs from
-6 dB at low frequencies through 0 dB at the band frequency to +6 dB at high
frequencies (a high shelf of twice the gain, shifted down by the gain). This
is the measured behaviour of the reference EQ; earlier development builds
ran -3 to +3 dB for the same setting.

Flat Tilt: the Gain value is the level difference across 10 octaves (roughly
20 Hz to 20 kHz), so the slope is Gain / 10 dB per octave (30 dB is 3 dB/oct).
Positive gain lifts the highs and lowers the lows, pivoting at the band
frequency; slope and Q are ignored.

## Preset exchange (`.ffp` files)

`neofilter-cli` can write and read Pro-Q 3 `.ffp` preset files, so settings do
not have to be typed in by hand in either direction:

```sh
# NeoFilter preset -> .ffp (needs any .ffp exported from Pro-Q 3 as template)
cargo run -p neofilter-cli -- export-ffp --preset friend-eq-a \
    --template exported.ffp --output friend-eq-a.ffp

# .ffp -> NeoFilter preset JSON (then "Paste preset" in the plugin)
cargo run -p neofilter-cli -- import-ffp --input exported.ffp --output preset.json
```

The file layout was worked out from an exported preset file only. Bands
(used, enabled, shape, frequency, gain, Q, slope) are converted; dynamics and
all global settings are copied from the template on export and ignored on
import. The format holds 24 bands, so presets using bands 25-32 cannot be
exported, and only Stereo placement is
supported. Shape codes for Bell, Low Shelf and High Shelf are confirmed by a
sample file; the other shape and slope codes follow the menu order and have
not been checked in Pro-Q 3: verify a generated preset there after loading
it.

## Command line tool

`neofilter-cli` reuses the plugin's presets and audio path. Everywhere a
`--preset <id>` is accepted you can use `--preset-file preset.json` instead
(the JSON produced by "Copy preset"). The model includes the preset's output
gain.

| Command | Purpose |
|---|---|
| `list` | list the factory presets |
| `curve` | print or export a preset's magnitude response (`--analog` for the prototype, `--csv`) |
| `render` | process a WAV file through a preset (reads 8 to 32-bit integer or 32-bit float, mono or stereo; writes 32-bit float) |
| `impulse` | write a preset's impulse response as WAV |
| `compare` | FFT of a measured impulse response (left channel) against the digital model: max and RMS deviation 20 Hz to 20 kHz, worst frequency, ten worst third-octave regions |
| `calibrate` | the plugin's loop calibration offline: fit calibration bands from a recording of the sent noise and of the loop return ([docs/calibration.md](docs/calibration.md#cli-workflow)) |
| `export-ffp` / `import-ffp` | convert presets to and from `.ffp` files (above) |

```sh
cargo run -p neofilter-cli -- list
cargo run -p neofilter-cli -- curve --preset friend-eq-a --sample-rate 48000 \
    --points 512 [--analog] [--csv curve.csv]
cargo run -p neofilter-cli -- render --preset friend-eq-a --input in.wav --output out.wav
cargo run -p neofilter-cli -- impulse --preset friend-eq-a --sample-rate 48000 --output ir.wav
cargo run -p neofilter-cli -- compare --preset friend-eq-a --ir measured.wav
cargo run -p neofilter-cli -- calibrate --sent sent.wav --return ret.wav --output fitted.json
```

`cargo run -p neofilter-cli -- <command> --help` lists every option.

## How it works

Each NeoFilter band is designed as an analog prototype (Butterworth-based
cascade of first- and second-order sections; bells use a low-pass to
band-pass transform). For Bell, even-order High Cut and default-Q 12 dB/oct
shelves, each section becomes a digital biquad by the rule identified from the
reference measurements (a biquad whose squared magnitude equals the analog one
at five frequencies, [docs/reference-matching.md](docs/reference-matching.md)).
All other sections use a matched z-transform, so the magnitude follows the
analog curve right up to Nyquist instead of cramping. The biquads run as
64-bit transposed direct form II cascades, with smoothed parameters and
crossfades on topology changes. The GUI draws the curve from the actual
digital coefficients.

NeoRIAA uses a minimum-phase biquad cascade fitted to the analytic curve
(precomputed for the common sample rates, `neo_measure::riaa`). The
calibration fits its correction with bells and shelves by greedy peak
placement and Levenberg-Marquardt refinement (`neofilter-dsp::autofit`).

References:

- M. Vicanek, "Matched Second Order Digital Filters" (2016) and "Matched
  Two-Pole Digital Shelving Filters".
- S. Orfanidis, "High-Order Digital Parametric Equalizer Design", JAES 2005.
- R. Bristow-Johnson, "Audio EQ Cookbook".
- V. Zavalishin, "The Art of VA Filter Design".
- ITU-R BS.1770-4 and EBU Tech 3341/3342 (loudness).

Design notes and status: [PLAN.md](PLAN.md) and
[docs/PLAN-vinyl-suite.md](docs/PLAN-vinyl-suite.md).

## Install

Download the archive for your platform from the Releases page. Each archive
contains the three plugins (`neofilter`, `neoriaa`, `neometer`, each as
`.clap` and `.vst3`), this README, the license and the `docs` folder.

| Platform | CLAP | VST3 |
|---|---|---|
| Linux | `~/.clap/` | `~/.vst3/` |
| macOS | `~/Library/Audio/Plug-Ins/CLAP/` | `~/Library/Audio/Plug-Ins/VST3/` |
| Windows | `C:\Program Files\Common Files\CLAP\` | `C:\Program Files\Common Files\VST3\` |

Copy the bundles (the whole `.vst3` directory on every platform) there, then
rescan plug-ins in your host. In Bitwig: Settings, Locations, rescan. CLAP is
recommended.

**macOS:** the builds are not signed or notarized. After copying, remove the
quarantine flag or macOS will refuse to load the plug-ins:

```sh
for p in neofilter neoriaa neometer; do
  xattr -cr ~/Library/Audio/Plug-Ins/CLAP/$p.clap
  xattr -cr ~/Library/Audio/Plug-Ins/VST3/$p.vst3
done
```

Release archives come with a `SHA256SUMS` file for verification.

## Build from source

You need a recent stable Rust toolchain (1.80 or newer) and the development
packages for X11, xcb, OpenGL and the audio backends (needed by the egui GUI
and nih-plug).

Debian / Ubuntu:

```sh
sudo apt-get install libasound2-dev libgl-dev libjack-dev libx11-xcb-dev \
  libxcb1-dev libxcb-dri2-0-dev libxcb-icccm4-dev libxcursor-dev \
  libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

Fedora:

```sh
sudo dnf install alsa-lib-devel libX11-devel libxcb-devel xcb-util-wm-devel \
  libXcursor-devel libxkbcommon-devel mesa-libGL-devel \
  pipewire-jack-audio-connection-kit-devel
```

Then bundle all three plugins (several packages need one `-p` each):

```sh
cargo xtask bundle -p neofilter -p neoriaa -p neometer --release
# -> target/bundled/{neofilter,neoriaa,neometer}.{clap,vst3}
mkdir -p ~/.clap ~/.vst3
cp -r target/bundled/*.clap ~/.clap/
cp -r target/bundled/*.vst3 ~/.vst3/
```

A single plugin: `cargo xtask bundle neofilter --release`. On macOS,
`bundle-universal -p ... --release` builds x86_64 and arm64 in one bundle.

## Development

```sh
cargo test --workspace      # everything; takes a few minutes
cargo test -p neofilter-dsp # fast
cargo test -p neo-measure
cargo test -p neoriaa
cargo test -p neometer
cargo test -p neofilter
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Layout:

- `crates/neofilter-dsp`: pure EQ DSP library. All compatibility conventions
  live in `src/conventions.rs`; also `autofit` (fit bands to a curve).
- `crates/neofilter`: the NeoFilter plugin (params, presets, processing,
  calibration, egui editor, analyzer).
- `crates/neofilter-cli`: command line tool.
- `crates/neo-measure`: measurement library (meters, spectra, transfer
  function, delay, RIAA design, test-record analysis).
- `crates/neoriaa`: the NeoRIAA plugin.
- `crates/neometer`: the NeoMeter plugin.
- `tools/reference-ab`: scripts used for the black-box reference
  measurements (documentation of how the fixtures were made).
- `xtask`: bundler (`cargo xtask bundle`).
- `.github/workflows`: CI (`ci.yml`) and releases (`release.yml`).
- `docs`: [calibration](docs/calibration.md),
  [reference matching](docs/reference-matching.md),
  [suite plan](docs/PLAN-vinyl-suite.md).

Contributor rules are in [CLAUDE.md](CLAUDE.md).

### Releasing

1. Bump `version` under `[workspace.package]` in `Cargo.toml` and update
   [CHANGELOG.md](CHANGELOG.md).
2. Commit, then `git tag v0.1.0 && git push --tags`.

The release workflow checks that the tag matches the Cargo version, runs the
tests, builds Linux x86_64, macOS universal and Windows x86_64 bundles of all
three plugins, and creates a GitHub Release with one archive per platform and
checksums. Tags containing a hyphen (for example `v0.2.0-rc1`) are marked as
pre-releases.

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). The GPL is required because
nih-plug's VST3 bindings are GPLv3.
