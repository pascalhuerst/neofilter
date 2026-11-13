//! Wow and flutter measurement from a 3150 Hz (or 3000 Hz) test tone.
//!
//! Realtime: **off-thread analysis** (allocates; processes a recorded block).
//!
//! Method: the tone is mixed down with a complex oscillator at the nominal
//! frequency, low-pass filtered (4th-order Butterworth, 800 Hz), the
//! instantaneous frequency is obtained from the phase difference of
//! consecutive samples (no phase unwrapping needed), anti-alias filtered and
//! decimated to roughly 1 kHz. The mean frequency gives the **speed
//! deviation** (`mean / nominal - 1`), the deviation from the mean in percent
//! is the flutter signal. It is evaluated
//!
//! * *unweighted* in a 0.2 Hz .. 200 Hz band (2nd-order Butterworth
//!   high-pass and low-pass), and
//! * *weighted* with [`WowFlutterWeighting`] (peak at 4 Hz),
//!
//! each giving the peak, a quasi-peak, the RMS (WRMS for the weighted case)
//! and the 2-sigma value (`2 * standard deviation`).
//!
//! **Status of the standard conformance:** the weighting curve is a fit to
//! the published shape of the DIN 45507 / IEC 60386 curve (see
//! [`WowFlutterWeighting`]); the quasi-peak detector uses a 10 ms attack and
//! 1.5 s decay time constant which are best-knowledge values, not verified
//! against the standard. Peak, RMS and 2-sigma values are exact for the
//! filtered signal. Results are validated against synthetic FM tones.
//!
//! The first `settle_s` seconds (default 2 s) are discarded because the
//! 0.2 Hz high-pass needs time to settle; provide at least ~4 s of signal.
//!
//! ```
//! use neo_measure::wow_flutter::*;
//! let fs = 48_000.0;
//! let x: Vec<f64> = (0..fs as usize * 6).map(|i| {
//!     let t = i as f64 / fs;
//!     (2.0 * std::f64::consts::PI * 3150.0 * t + (3.15 / 4.0) * (2.0 * std::f64::consts::PI * 4.0 * t).sin()).sin()
//! }).collect();
//! let r = analyze_wow_flutter(&x, &WowFlutterConfig::new(fs, 3150.0)).unwrap();
//! assert!((r.weighted.peak_pct - 0.1).abs() < 0.005);
//! ```

use crate::filter::{Biquad, BiquadCoeffs};
use crate::util::{rfft, window_sums, Window};
use crate::weighting::{wf_weighting_db, WowFlutterWeighting};
use std::f64::consts::PI;

/// Analysis settings.
#[derive(Debug, Clone, Copy)]
pub struct WowFlutterConfig {
    pub sample_rate: f64,
    /// Nominal test-tone frequency (3150 or 3000 Hz).
    pub nominal_hz: f64,
    /// Unweighted band, low edge (default 0.2 Hz).
    pub band_low: f64,
    /// Unweighted band, high edge (default 200 Hz).
    pub band_high: f64,
    /// Seconds discarded at the start for filter settling (default 2 s).
    pub settle_s: f64,
    /// Quasi-peak attack time constant, seconds (default 0.01).
    pub qp_attack_s: f64,
    /// Quasi-peak decay time constant, seconds (default 1.5).
    pub qp_decay_s: f64,
}

impl WowFlutterConfig {
    pub fn new(sample_rate: f64, nominal_hz: f64) -> Self {
        Self {
            sample_rate,
            nominal_hz,
            band_low: 0.2,
            band_high: 200.0,
            settle_s: 2.0,
            qp_attack_s: 0.01,
            qp_decay_s: 1.5,
        }
    }
}

/// Statistics of one (weighted or unweighted) flutter signal, in percent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlutterStats {
    /// Largest absolute deviation.
    pub peak_pct: f64,
    /// Maximum of the quasi-peak detector output.
    pub quasi_peak_pct: f64,
    /// RMS (the weighted value is the usual "WRMS").
    pub rms_pct: f64,
    /// `2 * standard deviation`.
    pub two_sigma_pct: f64,
}

/// Result of [`analyze_wow_flutter`].
#[derive(Debug, Clone)]
pub struct WowFlutterResult {
    /// Mean frequency of the tone in Hz.
    pub mean_frequency_hz: f64,
    /// `(mean / nominal - 1) * 100`.
    pub speed_deviation_pct: f64,
    pub unweighted: FlutterStats,
    pub weighted: FlutterStats,
    /// Flutter spectrum: frequencies in Hz (within the unweighted band).
    pub spectrum_hz: Vec<f64>,
    /// Peak deviation per frequency component in percent (unweighted).
    pub spectrum_pct: Vec<f64>,
    /// Same with the weighting curve applied.
    pub spectrum_weighted_pct: Vec<f64>,
    /// Rate of the demodulated flutter signal, Hz.
    pub flutter_rate_hz: f64,
}

fn butter4_lp(fs: f64, f: f64) -> [Biquad; 2] {
    [
        Biquad::new(BiquadCoeffs::lowpass(fs, f, 0.541_196_100_146_197)),
        Biquad::new(BiquadCoeffs::lowpass(fs, f, 1.306_562_964_876_376_4)),
    ]
}

fn stats(sig: &[f64], fs: f64, attack: f64, decay: f64) -> FlutterStats {
    let n = sig.len().max(1) as f64;
    let mean = sig.iter().sum::<f64>() / n;
    let ms = sig.iter().map(|v| v * v).sum::<f64>() / n;
    let var = sig.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
    let (aa, ad) = (
        1.0 - (-1.0 / (fs * attack.max(1e-6))).exp(),
        1.0 - (-1.0 / (fs * decay.max(1e-6))).exp(),
    );
    let (mut qp, mut qmax) = (0.0f64, 0.0f64);
    let mut peak = 0.0f64;
    for &v in sig {
        let r = v.abs();
        peak = peak.max(r);
        qp += if r > qp { aa } else { ad } * (r - qp);
        qmax = qmax.max(qp);
    }
    FlutterStats {
        peak_pct: peak,
        quasi_peak_pct: qmax,
        rms_pct: ms.sqrt(),
        two_sigma_pct: 2.0 * var.sqrt(),
    }
}

/// Analyse a recorded test tone. `None` if the block is too short (needs
/// `settle_s` + 0.5 s), contains non-finite values, or the configuration is
/// invalid.
pub fn analyze_wow_flutter(x: &[f64], cfg: &WowFlutterConfig) -> Option<WowFlutterResult> {
    let fs = cfg.sample_rate;
    if !(fs > 4.0 * cfg.nominal_hz)
        || !(cfg.nominal_hz > 0.0)
        || x.len() < ((cfg.settle_s + 0.5) * fs) as usize
        || x.iter().any(|v| !v.is_finite())
    {
        return None;
    }
    // quadrature mixing + low-pass
    let w = 2.0 * PI * cfg.nominal_hz / fs;
    let mut lp_i = butter4_lp(fs, 800.0f64.min(0.2 * fs));
    let mut lp_q = butter4_lp(fs, 800.0f64.min(0.2 * fs));
    let m = ((fs / 1000.0).floor() as usize).max(1);
    let fs_d = fs / m as f64;
    let mut aa = butter4_lp(fs, 250.0f64.min(0.4 * fs));
    let (mut pi, mut pq) = (0.0, 0.0);
    let mut inst = Vec::with_capacity(x.len() / m + 1);
    let (mut s, mut c) = (0.0f64, 1.0f64);
    let (ds, dc) = w.sin_cos();
    for (n, &v) in x.iter().enumerate() {
        let (re, im) = (v * c, -v * s);
        let (ns, nc) = (s * dc + c * ds, c * dc - s * ds);
        s = ns;
        c = nc;
        if n % 1024 == 1023 {
            let (sn, cn) = ((n + 1) as f64 * w).sin_cos();
            s = sn;
            c = cn;
        }
        let i0 = lp_i[0].process(re);
        let i = lp_i[1].process(i0);
        let q0 = lp_q[0].process(im);
        let q = lp_q[1].process(q0);
        let dphi = (q * pi - i * pq).atan2(i * pi + q * pq);
        pi = i;
        pq = q;
        let f = cfg.nominal_hz + dphi * fs / (2.0 * PI);
        let f0 = aa[0].process(f);
        let f = aa[1].process(f0);
        if n % m == 0 {
            inst.push(f);
        }
    }
    let skip = (cfg.settle_s * fs_d) as usize + 1;
    if inst.len() <= skip + 16 {
        return None;
    }
    let seg = &inst[skip..];
    let mean = seg.iter().sum::<f64>() / seg.len() as f64;
    if !(mean > 0.0) {
        return None;
    }
    // flutter signal in percent, filtered from the start so settling is
    // discarded consistently (filters run over the whole record)
    // the first ~100 ms of the demodulator are start-up garbage: hold the
    // first valid value instead
    let pre = ((0.1 * fs_d) as usize).min(inst.len() - 1);
    let pct: Vec<f64> = (0..inst.len())
        .map(|i| (inst[i.max(pre)] / mean - 1.0) * 100.0)
        .collect();
    let q = std::f64::consts::FRAC_1_SQRT_2;
    let hp_hi = cfg.band_high.min(0.45 * fs_d);
    let mut hp = Biquad::new(BiquadCoeffs::highpass(fs_d, cfg.band_low, q));
    let mut lp = Biquad::new(BiquadCoeffs::lowpass(fs_d, hp_hi, q));
    let mut wt = WowFlutterWeighting::new(fs_d);
    let (mut unw, mut wgt) = (Vec::with_capacity(seg.len()), Vec::with_capacity(seg.len()));
    // start the filters from the signal's mean so the 0.2 Hz high-pass does
    // not see a step (pct has ~zero mean over the segment)
    // prime all filters with the *time-reversed* beginning of the record,
    // which leaves them in a state consistent with a continuing signal
    // (avoids the start-up transient of the 0.2 Hz high-pass)
    let l = ((6.0 * fs_d) as usize).min(pct.len());
    for &v in pct[..l].iter().rev() {
        lp.process(hp.process(v));
        wt.process(v);
    }
    for (i, &v) in pct.iter().enumerate() {
        let u = lp.process(hp.process(v));
        let wv = wt.process(v);
        if i >= skip {
            unw.push(u);
            wgt.push(wv);
        }
    }
    let unweighted = stats(&unw, fs_d, cfg.qp_attack_s, cfg.qp_decay_s);
    let weighted = stats(&wgt, fs_d, cfg.qp_attack_s, cfg.qp_decay_s);

    // flutter spectrum of the unweighted signal (Hann, zero padded)
    let nfft = unw.len().next_power_of_two().clamp(4096, 1 << 20);
    let win = Window::Hann.coefficients(unw.len().min(nfft));
    let (s1, _) = window_sums(&win);
    let xw: Vec<f64> = unw.iter().zip(&win).map(|(a, b)| a * b).collect();
    let spec = rfft(&xw, nfft);
    let df = fs_d / nfft as f64;
    let (mut hz, mut sp, mut spw) = (Vec::new(), Vec::new(), Vec::new());
    for (k, v) in spec.iter().enumerate() {
        let f = k as f64 * df;
        if f < cfg.band_low {
            continue;
        }
        if f > cfg.band_high {
            break;
        }
        let a = 2.0 * v.norm() / s1;
        hz.push(f);
        sp.push(a);
        spw.push(a * 10f64.powf(wf_weighting_db(f) / 20.0));
    }
    Some(WowFlutterResult {
        mean_frequency_hz: mean,
        speed_deviation_pct: (mean / cfg.nominal_hz - 1.0) * 100.0,
        unweighted,
        weighted,
        spectrum_hz: hz,
        spectrum_pct: sp,
        spectrum_weighted_pct: spw,
        flutter_rate_hz: fs_d,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(fs: f64, fc: f64, dev_pct: f64, fmod: f64, secs: f64) -> Vec<f64> {
        let d = dev_pct / 100.0 * fc;
        (0..(secs * fs) as usize)
            .map(|i| {
                let t = i as f64 / fs;
                (2.0 * PI * fc * t + d / fmod * (2.0 * PI * fmod * t).sin()).sin()
            })
            .collect()
    }

    #[test]
    fn four_hz_reads_deviation_and_weighting_curve() {
        for fs in [48_000.0, 44_100.0, 96_000.0] {
            let cfg = WowFlutterConfig::new(fs, 3150.0);
            let r = analyze_wow_flutter(&fm(fs, 3150.0, 0.1, 4.0, 10.0), &cfg).unwrap();
            eprintln!(
                "W&F 4 Hz 0.1%: fs {fs}: unw peak {:.5} wtd peak {:.5} wrms {:.5} 2s {:.5} qp {:.5} speed {:.5}%",
                r.unweighted.peak_pct, r.weighted.peak_pct, r.weighted.rms_pct, r.weighted.two_sigma_pct, r.weighted.quasi_peak_pct, r.speed_deviation_pct
            );
            assert!((r.weighted.peak_pct - 0.1).abs() < 0.002);
            assert!((r.unweighted.peak_pct - 0.1).abs() < 0.004); // residual start-up transient of the 0.2 Hz high-pass
            assert!((r.weighted.rms_pct - 0.1 / 2f64.sqrt()).abs() < 0.001);
            assert!((r.weighted.two_sigma_pct - 0.1 * 2f64.sqrt()).abs() < 0.002);
            assert!(r.weighted.quasi_peak_pct > 0.09 && r.weighted.quasi_peak_pct < 0.1001);
            assert!(r.speed_deviation_pct.abs() < 0.002);
            // spectrum peak at 4 Hz with 0.1 %
            let (i, pk) = r
                .spectrum_pct
                .iter()
                .enumerate()
                .fold((0, 0.0), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
            assert!(
                (r.spectrum_hz[i] - 4.0).abs() < 0.1 && (pk - 0.1).abs() < 0.017 /* Hann scalloping */
            );
        }
    }

    #[test]
    fn weighting_attenuates_other_rates() {
        let fs = 48_000.0;
        let cfg = WowFlutterConfig::new(fs, 3150.0);
        for (fmod, secs) in [(0.5, 16.0), (30.0, 8.0), (10.0, 8.0), (1.0, 12.0)] {
            let r = analyze_wow_flutter(&fm(fs, 3150.0, 0.1, fmod, secs), &cfg).unwrap();
            let want = 0.1 * 10f64.powf(wf_weighting_db(fmod) / 20.0);
            eprintln!(
                "W&F {fmod} Hz: weighted peak {:.5} (curve {want:.5}), unweighted {:.5}",
                r.weighted.peak_pct, r.unweighted.peak_pct
            );
            assert!(
                (r.weighted.peak_pct - want).abs() < 0.04 * want + 0.001,
                "{fmod}"
            );
            // unweighted band 0.2..200 Hz passes all these rates unattenuated
            assert!((r.unweighted.peak_pct - 0.1).abs() < 0.004, "{fmod}");
        }
    }

    #[test]
    fn constant_speed_error_and_3000() {
        let fs = 48_000.0;
        let x: Vec<f64> = (0..fs as usize * 5)
            .map(|i| (2.0 * PI * 3000.0 * 1.02 * i as f64 / fs).sin())
            .collect();
        let r = analyze_wow_flutter(&x, &WowFlutterConfig::new(fs, 3000.0)).unwrap();
        assert!(
            (r.speed_deviation_pct - 2.0).abs() < 1e-4,
            "{}",
            r.speed_deviation_pct
        );
        assert!((r.mean_frequency_hz - 3060.0).abs() < 0.005);
        assert!(r.unweighted.peak_pct < 0.002 && r.weighted.peak_pct < 0.002);
        // speed error plus flutter together
        let mut y = fm(fs, 3150.0 * 0.97, 0.2, 5.0, 8.0);
        y.truncate(fs as usize * 8);
        let r = analyze_wow_flutter(&y, &WowFlutterConfig::new(fs, 3150.0)).unwrap();
        assert!((r.speed_deviation_pct + 3.0).abs() < 0.001);
        assert!(
            (r.weighted.peak_pct - 0.2 * 10f64.powf(wf_weighting_db(5.0) / 20.0)).abs() < 0.004
        );
    }

    #[test]
    fn short_or_bad_input() {
        let cfg = WowFlutterConfig::new(48_000.0, 3150.0);
        assert!(analyze_wow_flutter(&[], &cfg).is_none());
        assert!(analyze_wow_flutter(&vec![0.0; 48_000], &cfg).is_none());
        assert!(analyze_wow_flutter(&vec![f64::NAN; 480_000], &cfg).is_none());
    }
}
