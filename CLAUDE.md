# NeoFilter

32-band parametric EQ plugin (CLAP + VST3) in Rust, built on nih-plug.
Design and phase plan: see `PLAN.md` — read it before changing DSP or params.

## Hard rules

- **Never read, unpack, disassemble or commit anything in `orig/`.** This is a
  clean-room project: DSP comes only from published literature.
- Never `git add -A` / `git add .` — add explicit paths. The `*.jpg` files in
  the repo root and `orig/` are the user's and stay untracked.
- Do not push. Do not rewrite history.
- No FabFilter names in code identifiers, UI strings or plugin metadata.
- Parameter ids are frozen (`freq_{n}`, `gain_{n}`, `q_{n}`, `used_{n}`, `on_{n}`, `shape_{n}`, `slope_{n}`, `place_{n}`, n = 1..32; `out_gain`, `gain_scale`, `bypass`); never rename or reorder.

## Commands

```sh
cargo test --workspace                                  # all tests
cargo test -p neofilter-dsp                             # DSP only (fast)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask bundle neofilter --release                  # target/bundled/neofilter.{clap,vst3}
cp -r target/bundled/neofilter.clap ~/.clap/            # install for Bitwig
```

## Layout

- `crates/neofilter-dsp` — pure DSP, no plugin/GUI deps. Analog prototypes →
  matched z-transform → f64 TDF-II cascades. All Pro-Q-compatibility
  conventions live in `src/conventions.rs` only.
- `crates/neofilter` — plugin: `params.rs`, `presets.rs`, `lib.rs` (process),
  `editor/` (egui), `analyzer.rs`.
- `crates/neofilter-cli` — response export / IR comparison.
- `xtask` — bundler.

## Coding standards

- Audio thread (`process` and everything it calls): no allocation, locks,
  I/O, or panics. Fixed-size arrays; `assert_process_allocs` feature of
  nih-plug is enabled in debug builds.
- Internal audio math in `f64`. No `unsafe` outside what nih-plug requires.
- Every DSP change comes with tests that compare against the analytic analog
  prototype or a stated numeric tolerance. Do not loosen a tolerance to make
  a test pass without writing down why in the test.
- Coefficient design must never return NaN/inf or unstable poles; clamp and
  fall back instead.
- Keep `cargo clippy -- -D warnings` and `cargo fmt --check` clean.
- Commits: small, imperative subject, one logical change, tests green.
