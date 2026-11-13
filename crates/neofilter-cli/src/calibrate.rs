//! `neofilter-cli calibrate`: the plugin's calibration analysis and fit on
//! two recordings.

use crate::{model, wavio};
use clap::Args;
use neofilter::calibration::offline::{calibrate_offline, OfflineInput};
use neofilter::calibration::settings::{CalCurve, CalPersist, CalSettings};
use neofilter::presets::PresetFile;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct CalibrateArgs {
    /// Signal sent into the loop (noise), WAV. Stereo files are averaged to
    /// mono unless --sent-channel says otherwise.
    #[arg(long, value_name = "WAV")]
    pub sent: PathBuf,
    /// Return of the loop (through the playback chain), WAV, mono or stereo.
    #[arg(long = "return", value_name = "WAV")]
    pub ret: PathBuf,
    /// Factory preset id of the EQ the noise passed through (default: no EQ).
    #[arg(long, conflicts_with = "preset_file")]
    pub preset: Option<String>,
    /// Preset JSON of the EQ the noise passed through (the plugin's "Copy preset").
    #[arg(long, value_name = "FILE")]
    pub preset_file: Option<PathBuf>,
    /// The sent file was recorded after the EQ (default: before the EQ, as the plugin's tap does).
    #[arg(long)]
    pub sent_is_post_eq: bool,
    /// Which channel of a stereo sent file is the reference.
    #[arg(long, value_enum, default_value_t = SentChannel::Mean)]
    pub sent_channel: SentChannel,
    /// Playback-chain correction curve (JSON `response_curve`).
    #[arg(long, value_name = "FILE")]
    pub playback_curve: Option<PathBuf>,
    /// Maximum boost of the total calibration curve in dB.
    #[arg(long, default_value_t = 6.0)]
    pub max_boost: f64,
    /// Maximum cut of the total calibration curve in dB.
    #[arg(long, default_value_t = 12.0)]
    pub max_cut: f64,
    /// Fit range in Hz, `LOW:HIGH`.
    #[arg(long, default_value = "30:16000", value_name = "LOW:HIGH")]
    pub range: String,
    /// Reference band for the 0 dB level in Hz, `LOW:HIGH`.
    #[arg(long, default_value = "300:3000", value_name = "LOW:HIGH")]
    pub ref_band: String,
    /// Maximum number of calibration bands.
    #[arg(long, default_value_t = 12)]
    pub max_bands: usize,
    /// Smoothing in 1/N octave (0 = none).
    #[arg(long, default_value_t = 6)]
    pub smoothing: u32,
    /// Make every band a calibration band (the whole EQ is rewritten).
    #[arg(long)]
    pub replace_all: bool,
    /// Calibration band numbers (1-based, e.g. `13-32,5`). Default: all
    /// bands that are unused in the current preset.
    #[arg(long, value_name = "LIST")]
    pub cal_slots: Option<String>,
    /// Write the fitted preset JSON here (default: print it).
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
pub enum SentChannel {
    Left,
    Right,
    Mean,
}

fn parse_range(s: &str, what: &str) -> Result<(f64, f64), String> {
    let bad = || format!("{what}: expected LOW:HIGH in Hz, got '{s}'");
    let (a, b) = s.split_once(':').ok_or_else(bad)?;
    let (a, b): (f64, f64) = (
        a.trim().parse().map_err(|_| bad())?,
        b.trim().parse().map_err(|_| bad())?,
    );
    if !(a.is_finite() && b.is_finite() && a > 0.0 && b > a) {
        return Err(bad());
    }
    Ok((a, b))
}

fn parse_slots(s: &str) -> Result<u32, String> {
    let mut mask = 0u32;
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let bad = || format!("--cal-slots: cannot parse '{part}'");
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (
                a.parse::<usize>().map_err(|_| bad())?,
                b.parse::<usize>().map_err(|_| bad())?,
            ),
            None => {
                let v = part.parse::<usize>().map_err(|_| bad())?;
                (v, v)
            }
        };
        if a < 1 || b > 32 || a > b {
            return Err(format!("--cal-slots: '{part}' is outside 1-32"));
        }
        for i in a..=b {
            mask |= 1 << (i - 1);
        }
    }
    Ok(mask)
}

pub fn run(a: CalibrateArgs) -> Result<String, String> {
    let sel = model::PresetSel {
        preset: a.preset.clone(),
        preset_file: a.preset_file.clone(),
    };
    let current = if a.preset.is_some() || a.preset_file.is_some() {
        sel.resolve()?
    } else {
        model::Model {
            name: "NeoFilter calibration".into(),
            bands: [neofilter_dsp::BandParams::default(); neofilter_dsp::types::NUM_BANDS],
            gain_scale: 1.0,
            out_gain_db: 0.0,
        }
    };
    let sent = wavio::read(&a.sent)?;
    let ret = wavio::read(&a.ret)?;
    if sent.sample_rate != ret.sample_rate {
        return Err(format!(
            "sample rates differ: sent {} Hz, return {} Hz",
            sent.sample_rate, ret.sample_rate
        ));
    }
    let fs = f64::from(sent.sample_rate);
    let reference: Vec<f32> = match (sent.channels.as_slice(), a.sent_channel) {
        ([m], _) => m.clone(),
        ([l, _], SentChannel::Left) => l.clone(),
        ([_, r], SentChannel::Right) => r.clone(),
        ([l, r], SentChannel::Mean) => l.iter().zip(r).map(|(x, y)| 0.5 * (x + y)).collect(),
        _ => return Err("sent file must be mono or stereo".into()),
    };
    let (rl, rr) = match ret.channels.as_slice() {
        [m] => (m.as_slice(), m.as_slice()),
        [l, r] => (l.as_slice(), r.as_slice()),
        _ => return Err("return file must be mono or stereo".into()),
    };

    let (range_lo, range_hi) = parse_range(&a.range, "--range")?;
    let (ref_lo, ref_hi) = parse_range(&a.ref_band, "--ref-band")?;
    let correction = match &a.playback_curve {
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            Some(CalCurve::from_json(&text).map_err(|e| format!("{}: {e}", p.display()))?)
        }
        None => None,
    };
    let mut settings = CalSettings {
        max_boost_db: a.max_boost,
        max_cut_db: a.max_cut,
        range_lo_hz: range_lo,
        range_hi_hz: range_hi,
        ref_lo_hz: ref_lo,
        ref_hi_hz: ref_hi,
        max_bands: a.max_bands,
        smoothing: a.smoothing,
        replace_all: a.replace_all,
        ..Default::default()
    };
    settings.sanitize();
    let mask = match &a.cal_slots {
        Some(s) => parse_slots(s)?,
        None => 0,
    };
    let persist = CalPersist {
        settings: settings.clone(),
        band_mask: mask,
        correction: correction.clone(),
        last: None,
    };
    let result = calibrate_offline(
        &OfflineInput {
            sample_rate: fs,
            sent: &reference,
            ret: [rl, rr],
            bands: current.bands,
            gain_scale: current.gain_scale,
            sent_is_post_eq: a.sent_is_post_eq,
        },
        &persist,
    )
    .map_err(|e| format!("calibration failed: {e}; nothing was fitted"))?;

    let mut out = String::new();
    let m = &result.measurement;
    let _ = writeln!(
        out,
        "sample rate {fs} Hz, {:.1} s analysed; current EQ '{}'{}",
        reference.len() as f64 / fs,
        current.name,
        if a.sent_is_post_eq {
            " (sent is post-EQ)"
        } else {
            ""
        }
    );
    for (c, name) in ["left", "right"].iter().enumerate() {
        match &m.channels[c] {
            Some(d) => {
                let i = &d.info;
                let _ = writeln!(
                    out,
                    "delay {name}: {:.3} ms ({:.2} samples), confidence {:.1}, polarity {}, level {:.1} dBFS rms / {:.1} dBFS peak, coherence {:.3}",
                    i.delay_ms,
                    i.delay_samples,
                    i.confidence,
                    if i.inverted { "inverted" } else { "normal" },
                    i.level_rms_db,
                    i.peak_db,
                    i.mean_coherence
                );
            }
            None => {
                let _ = writeln!(out, "delay {name}: unusable channel (silent or no loop)");
            }
        }
    }
    let p = &result.plan;
    let _ = writeln!(
        out,
        "mean coherence in range: {:.3}; playback curve: {}",
        m.mean_coherence,
        correction.as_ref().map_or("none".to_string(), |c| {
            let (lo, hi) = c.covered_range().unwrap_or((0.0, 0.0));
            format!("'{}' ({lo:.0}-{hi:.0} Hz)", c.name)
        })
    );
    let _ = writeln!(
        out,
        "loop deviation in {range_lo:.0}-{range_hi:.0} Hz: rms {:.3} dB, max {:.3} dB",
        p.rms_dev_db, p.max_dev_db
    );
    let _ = writeln!(
        out,
        "predicted after fit: rms {:.3} dB, max {:.3} dB (fit residual {:.3} / {:.3} dB)",
        p.rms_pred_db, p.max_pred_db, p.fit_rms_db, p.fit_max_db
    );
    for w in &p.warnings {
        let _ = writeln!(out, "note: {w}");
    }
    let _ = writeln!(out, "fitted calibration bands ({}):", p.bands.len());
    for (b, slot) in p.bands.iter().zip(&p.cal_slots) {
        let _ = writeln!(
            out,
            "  band {:>2}: {:<10} {:>9.1} Hz {:>+7.2} dB Q {:.2}",
            slot + 1,
            neofilter::presets::shape_id(b.shape),
            b.freq_hz,
            b.gain_db,
            b.q
        );
    }
    let file = PresetFile::from_band_params(
        &current.name,
        (current.gain_scale * 100.0) as f32,
        current.out_gain_db as f32,
        &result.bands,
    );
    let json = file.to_json();
    match &a.output {
        Some(path) => {
            std::fs::write(path, &json)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            let _ = writeln!(out, "wrote fitted preset to {}", path.display());
        }
        None => {
            let _ = writeln!(out, "{json}");
        }
    }
    Ok(out)
}
