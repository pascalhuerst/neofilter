# NeoFilter

NeoFilter is a 24-band parametric EQ plugin (CLAP and VST3) written in Rust.
It is developed on Linux (Bitwig first) and also builds for macOS and Windows.

Its band parameters (Shape, Frequency, Gain, Q, Slope) follow the same
conventions as FabFilter Pro-Q 3. A setting from a Pro-Q 3 band table can be
typed in number by number and is designed to give a closely matching curve.

> **Status:** v0.1 is feature complete but has not yet been verified in a real
> host or against measurements of the reference EQ, see
> [Accuracy](#accuracy).

## Not affiliated with FabFilter

NeoFilter is an independent, clean-room implementation built from published
DSP literature (see [How it works](#how-it-works)). It is not affiliated with
or endorsed by FabFilter and contains no FabFilter code. "FabFilter" and
"Pro-Q" are trademarks of their respective owner and are used here only to
describe parameter compatibility.

Exact equivalence of the Q scaling and of high-slope bells depends on a
calibration step against measured impulse responses, described in
[PLAN.md](PLAN.md#calibration-black-box-optional-but-recommended); until that
has been done, treat those two cases as approximate.

## Accuracy

The digital filters follow the analog prototypes closely in the everyday
range, but not everywhere. The enforced, measured tolerances are in
[`crates/neofilter-dsp/tests/accuracy.rs`](crates/neofilter-dsp/tests/accuracy.rs):

- Bells in the everyday range (Q 1 to 3, up to 6 dB gain, up to 24 dB/oct)
  stay within about 0.23 dB of the analog curve up to 16 kHz at 44.1 kHz.
- High-Q shelves, cuts, wide notches and 96 dB/oct band passes, and any band
  near Nyquist, deviate from the analog prototype by several dB (up to about
  15 dB in the extreme corners of the grid, close to Nyquist). The editor
  curve shows the actual digital response, so what you see is what you hear.
- The blanket 0.1 dB / 0.5 dB target of the original plan was **not** met.
  Improving near-Nyquist matching (joint / free-pole fit) is future work.

## Features

- 24 bands, each with: used, enabled, shape, frequency (10 Hz to 30 kHz),
  gain (±30 dB), Q (0.025 to 40), slope, and stereo placement
  (Stereo / Left / Right / Mid / Side).
- Shapes: Bell, Low Shelf, Low Cut, High Shelf, High Cut, Notch, Band Pass,
  Tilt Shelf, Flat Tilt.
- Slopes: 6, 12, 18, 24, 30, 36, 48, 72, 96 dB/oct.
- Zero-latency (minimum-phase) processing, analog-matched responses up to
  Nyquist, 64-bit internal processing, click-free parameter changes.
- Global output gain (±36 dB), gain scale (0 to 200 %), host-mapped bypass.
- Stereo and mono layouts.
- Resizable GUI (default 900x560): response curve (sum and per-band),
  draggable band nodes, selected-band strip with text entry for all values,
  pre/post spectrum analyzer, preset menu, "Copy preset" / "Paste preset".
- Factory presets "Default", "Friend EQ A" and "Friend EQ B".
- `neofilter-cli`: lists presets, prints/exports a preset's response curve,
  renders WAV files through a preset, writes impulse responses and compares a
  measured impulse-response WAV with the model (see [CLI](#command-line-tool)).

Not included in v0.1: linear-phase and natural-phase modes, dynamic EQ,
brickwall slope, EQ match, band solo, auto-gain, surround. Only the
zero-latency minimum-phase mode exists.

## Install

Download the archive for your platform from the Releases page. Each archive
contains `neofilter.clap`, `neofilter.vst3`, this README and the license.

| Platform | CLAP | VST3 |
|---|---|---|
| Linux | `~/.clap/` | `~/.vst3/` |
| macOS | `~/Library/Audio/Plug-Ins/CLAP/` | `~/Library/Audio/Plug-Ins/VST3/` |
| Windows | `C:\Program Files\Common Files\CLAP\` | `C:\Program Files\Common Files\VST3\` |

Copy the bundle (the whole `.vst3` directory on every platform) there, then
rescan plug-ins in your host. In Bitwig: Settings, Locations, rescan. CLAP is
recommended.

**macOS:** the builds are not signed or notarized. After copying, remove the
quarantine flag or macOS will refuse to load the plug-in:

```sh
xattr -cr ~/Library/Audio/Plug-Ins/CLAP/neofilter.clap
xattr -cr ~/Library/Audio/Plug-Ins/VST3/neofilter.vst3
```

Release archives come with a `SHA256SUMS` file for verification.

## Build from source

You need a recent stable Rust toolchain and the development packages for X11,
xcb, OpenGL and the audio backends (needed by the egui GUI and nih-plug).

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

Then:

```sh
cargo xtask bundle neofilter --release   # target/bundled/neofilter.{clap,vst3}
cp -r target/bundled/neofilter.clap ~/.clap/
cp -r target/bundled/neofilter.vst3 ~/.vst3/
```

## Usage

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
`shape_{n}`, `slope_{n}`, `place_{n}` with n = 1..24, plus `out_gain`,
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
are listed in [PLAN.md](PLAN.md). Applying a preset resets unused bands.

Gain scale multiplies the dB gain of every Bell, Shelf and Tilt band, so you
can dial an overall EQ amount up or down.

Flat Tilt: the Gain value is the level difference across 10 octaves (roughly
20 Hz to 20 kHz), so the slope is Gain / 10 dB per octave (30 dB is 3 dB/oct).
Positive gain lifts the highs and lowers the lows, pivoting at the band
frequency; slope and Q are ignored.

## Command line tool

`neofilter-cli` reuses the plugin's presets and audio path. Everywhere a
`--preset <id>` is accepted you can use `--preset-file preset.json` instead
(the JSON produced by "Copy preset"). The model includes the preset's output
gain.

```sh
cargo run -p neofilter-cli -- list
cargo run -p neofilter-cli -- curve --preset friend-eq-a --sample-rate 48000 \
    --points 512 [--analog] [--csv curve.csv]
cargo run -p neofilter-cli -- render --preset friend-eq-a --input in.wav --output out.wav
cargo run -p neofilter-cli -- impulse --preset friend-eq-a --sample-rate 48000 --output ir.wav
cargo run -p neofilter-cli -- compare --preset friend-eq-a --ir measured.wav
```

`render` reads 8 to 32-bit integer or 32-bit float WAV (mono or stereo) and
writes 32-bit float. `compare` takes the FFT of a measured impulse response
(left channel, the WAV's sample rate) and reports max and RMS deviation in dB
against the digital model between 20 Hz and 20 kHz, the worst frequency and
the ten worst third-octave regions.

## How it works

Each band is designed as an analog prototype (Butterworth-based cascade of
first- and second-order sections; bells use a low-pass to band-pass
transform). Each section is mapped to a digital biquad with a matched
z-transform, so the magnitude response follows the analog curve right up to
Nyquist instead of cramping. The biquads run as 64-bit transposed direct
form II cascades, with smoothed parameters and crossfades on topology
changes. The GUI draws the curve from the actual digital coefficients.

References:

- M. Vicanek, "Matched Second Order Digital Filters" (2016) and "Matched
  Two-Pole Digital Shelving Filters".
- S. Orfanidis, "High-Order Digital Parametric Equalizer Design", JAES 2005.
- R. Bristow-Johnson, "Audio EQ Cookbook".
- V. Zavalishin, "The Art of VA Filter Design".

Full design notes are in [PLAN.md](PLAN.md).

## Development

```sh
cargo test --workspace
cargo test -p neofilter-dsp                              # DSP only (fast)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Layout:

- `crates/neofilter-dsp`: pure DSP library. All compatibility conventions live
  in `src/conventions.rs`.
- `crates/neofilter`: the plugin (params, presets, processing, egui editor,
  analyzer).
- `crates/neofilter-cli`: response export and impulse-response comparison.
- `xtask`: bundler (`cargo xtask bundle`).
- `.github/workflows`: CI (`ci.yml`) and releases (`release.yml`).

Contributor rules are in [CLAUDE.md](CLAUDE.md).

### Releasing

1. Bump `version` under `[workspace.package]` in `Cargo.toml` and update
   [CHANGELOG.md](CHANGELOG.md).
2. Commit, then `git tag v0.1.0 && git push --tags`.

The release workflow checks that the tag matches the Cargo version, runs the
tests, builds Linux x86_64, macOS universal and Windows x86_64 bundles, and
creates a GitHub Release with archives and checksums. Tags containing a
hyphen (for example `v0.2.0-rc1`) are marked as pre-releases.

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). The GPL is required because
nih-plug's VST3 bindings are GPLv3.
