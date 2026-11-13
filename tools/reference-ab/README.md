# Reference A/B measurement

Black-box comparison of NeoFilter against a reference EQ plugin that is
installed on this machine (hosted headlessly with the `pedalboard` Python
package; a Windows plugin works through yabridge). Only the plugin's audio
output is measured.

```sh
python3 -m venv --system-site-packages /tmp/abtest/venv
/tmp/abtest/venv/bin/pip install pedalboard

# preset JSON for the reference (any NeoFilter preset JSON works)
cargo run --release -p neofilter-cli -- import-ffp --input some.ffp --output /tmp/abtest/p.json

# set the bands on the reference, render its impulse response
/tmp/abtest/venv/bin/python measure.py /tmp/abtest/p.json 48000 /tmp/abtest/ref.wav

# deviation between the reference and NeoFilter's model
cargo run --release -p neofilter-cli -- compare --preset-file /tmp/abtest/p.json --ir /tmp/abtest/ref.wav
```

`batch.py DIR RATES` measures every `*.json` in a directory at several sample
rates; `curve.py` prints reference / digital / analog curves side by side
(paths inside it are hard-coded to `/tmp/abtest/probe`); `probe.py` lists the
reference plugin's parameters. The plugin path is hard-coded at the top of
the scripts.

## Rule identification and fixtures

- `refmodel.py`: the Python specification of the reference-matched digital
  design (Bell, even-order High Cut); the Rust port is
  `crates/neofilter-dsp/src/refmatch.rs`. See `docs/reference-matching.md`.
- `lab.py`, `ident.py`: helpers for measuring single bands on the reference
  plugin and identifying poles/zeros of an impulse response (Prony).
- `fixtures.py`, `presetfix.py`: generate the test fixtures
  `crates/neofilter-dsp/tests/fixtures/reference_curves.json` and
  `crates/neofilter/tests/fixtures/reference_presets.json` from measured
  impulse responses.
- `checkfix.py`: model-versus-fixture check (worst deviation per case).

Several of these scripts hard-code `/tmp/abtest/...` paths (inputs, outputs,
the model directory) and the reference plugin's location; adjust them before
use. The fixtures in the repository are the measured data, the scripts only
document how they were made.
