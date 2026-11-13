# Matching the reference EQ's digital design

NeoFilter's parameters follow the reference EQ described in the README. The
curves it produces depend not only on the analog prototypes but also on the
rule that turns each analog section into a digital biquad. That rule is
identified from black-box measurements and implemented in
`crates/neofilter-dsp/src/refmatch.rs`; the Python specification is
`tools/reference-ab/refmodel.py` (authoritative if the two ever disagree).

Status: **Bell (all slopes) and High Cut (even orders) are covered. Everything
else still uses the analog-matched design** (`matched.rs`). See "Not covered".

## How the rule was identified

1. Impulse responses of single bands were rendered by the reference EQ (32-bit
   float output, several sample rates, many frequency / gain / Q / slope
   settings) and only the audio output was analysed.
2. Poles and zeros of each measured impulse response were identified with
   Prony's method (`tools/reference-ab/ident.py`) and compared with the poles
   and zeros of the analog prototype. For Bell and High Cut the digital poles
   do not follow the usual impulse-invariant or bilinear mapping; the
   structure that fits is "a biquad whose squared magnitude equals the analog
   one at a few frequencies".
3. The number and placement of the match points, and the Nyquist target, were
   fitted to the measured curves (`refmodel.py`). The constants below are the
   result of that fit, not derived from any documentation.
4. The final model was checked against all fixtures (below).

## The rule

Every 2nd-order analog section `(s^2 + bz s + cz) / (s^2 + bp s + cp)` becomes a
digital biquad whose **squared magnitude** equals the analog section's squared
magnitude at five frequencies: DC, three interior frequencies and Nyquist, where
the Nyquist value is the analog value at a frequency `f_x <= Nyquist`.

The squared magnitude of a biquad is linear in `(X0, X1, X2)` in the phi basis
(`|P|^2 = X0 phi0 + X1 phi1 + X2 phi2`, `phi1 = sin^2(w/2)`, `phi0 = 1 - phi1`,
`phi2 = 4 phi0 phi1`) for numerator and denominator, so the five conditions are a
homogeneous 5x6 linear system; its null vector is factored into a minimum-phase
biquad (`_factor` in the Python). For conditioning the system is solved in the
basis `(1, t, t^2)` with `t = u / u_s`, `u = sin^2(pi f / fs)`, `u_s` the largest
interior `u`, rows scaled to unit maximum, and converted to `(X0, X1, X2)`
afterwards. The Rust code uses Gaussian elimination with full pivoting and
retries with a one-sided Jacobi SVD if the result fails its self-check.

### Bell

Sections come from the Butterworth-shelf band prototype that `analog.rs`
builds (same as `bell_sections()` in the Python). `Q` below is the displayed Q.

- Bell-type section (zero and pole at the same frequency `f0`: the only
  section of a 12 dB/oct bell and the middle section of odd prototype orders):
  interior points `f0 (1 - d), f0 (1 - 0.05 d), f0` with `d = clamp(1/Q, 0.1, 0.8)`;
  `f_x = clamp(1.2 f0, 0.9 Nyq, Nyq)`.
- Other sections: `f_e` is the lower extremum frequency of the analog
  section's magnitude, except that the upper extremum is used when it exists,
  is below 0.999 Nyq, and the lower one is either below 0.1 x the upper one or
  insignificant (`|1 - |H(f_lo)|^2 / |H(0)|^2| < 1e-5`). Interior points
  `f_e (1 - d), 0.999 f_e, f_e` with `d = min(0.95, 0.5/Q + 0.006)`;
  `f_x = clamp(1.8 f_e, 0.83 Nyq, Nyq)`.

### High Cut, even orders

Butterworth sections (the highest-Q section's Q is multiplied by the displayed
Q, as before), each with interior points `c f0 / 2, c f0, f0` where
`c = max(0.5, sqrt(1 - 1/(2 Q^2)))` (0.5 when there is no resonance peak), and
the Nyquist value equal to the analog value at Nyquist.

### Nyquist fallback

If the five-point solution is not realisable (negative power terms, no real
factorisation, unstable), `f_x` is lowered to the largest realisable value
(bisection, at most 32 steps; if even the lowest candidate fails a coarse scan
is tried first). Example from the fixtures: High Cut 13607 Hz, Q 1, 48 dB/oct
at 48 kHz.

## Additions that are not part of the identified rule

These exist to keep the plugin from ever producing a bad filter. None of them
triggers on any fixture case (the accuracy numbers are identical with and
without them):

- Self-check: the designed biquad must hit its own five match points (relative
  1e-6 in squared magnitude) and be finite and stable.
- Analog-tracking guard (Bell only): the section must stay within 0.5 dB of its
  analog prototype between 10 Hz and 0.1 fs. For sections with two extrema the
  other extremum is tried if the first candidate fails. This was added because
  the discrete choice of extremum otherwise produced an isolated 1.7 dB error at
  one parameter value (48 dB/oct bell, 1 kHz, +6 dB, Q about 3.25 at 44.1 kHz:
  the upper extremum is selected only in a narrow Q window, and its fit is far
  off at low frequencies), which `tests/continuity.rs` caught.
- If the rule cannot be applied to a section (points at or above Nyquist, no
  extremum, no realisable solution, non-finite, unstable, failed check or
  guard), that section uses the analog-matched design instead. No fixture
  section needs this.

Design cost (release build, this machine): about 9 us per 96 dB/oct bell band
(8 sections), averaged over a spread of frequencies and Qs at 48 kHz.

## Measured accuracy

Single bands (`crates/neofilter-dsp/tests/reference_curves.rs`, 198 measured
cases: Bell and High Cut at 44.1 / 48 / 96 kHz; reference points below -90 dB
ignored):

| | worst | median |
|---|---|---|
| all cases | 0.0168 dB (High Cut 15 kHz, Q 0.8, 24 dB/oct at 48 kHz) | 0.0001 dB |
| Bell cases | 0.0060 dB (3 kHz, +9 dB, Q 0.6, 24 dB/oct at 44.1 kHz) | |

Factory presets, total curve 40 Hz to 20 kHz
(`crates/neofilter/tests/reference_presets.rs`), worst deviation in dB:

| preset | 44.1 kHz | 48 kHz | 96 kHz |
|---|---|---|---|
| Friend EQ A | 0.0009 | 0.0011 | 0.0038 |
| Friend EQ B | 0.0121 | 0.0151 | 0.0017 |
| Friend EQ A+B | 0.0126 | 0.0157 | 0.0033 |

The largest deviations are at the low end. Preset B contains the 13.6 kHz
48 dB/oct High Cut (Nyquist fallback) at 44.1 / 48 kHz. Preset A (and so A+B)
contains a Low Cut, which is not covered (below), which probably explains the
0.0038 dB at 96 kHz; this was not isolated.

## Not covered yet

- Shelves (Low / High Shelf), including their Q convention.
- Low Cut, Notch, Band Pass.
- Tilt Shelf (its gain convention) and Flat Tilt.
- Odd-order cuts (6, 18, 30 dB/oct High Cut): first-order sections, rule not
  identified.
- Band frequencies above about 0.83 Nyquist, where small deviations remain
  (few fixture cases there); band frequencies are clamped at 0.49 fs anyway.
- Bell/High Cut settings far from the measured grid are extrapolations of the
  fitted rule (for example very wide, very steep bells at low Q).

Because Bell and High Cut now intentionally deviate from the analog prototype
near Nyquist, `tests/accuracy.rs` compares them with the analog prototype only
for band frequencies up to 0.02 fs and, within that, only up to 0.1 fs. Their
accuracy is governed by the reference fixtures above.

## Reproducing

See `tools/reference-ab/README.md`. `refmodel.py` (the specification),
`checkfix.py` (model versus fixtures) and `python3 -I tools/reference-ab/checkfix.py`
style snippets can be used to compare coefficients with the Rust design.
