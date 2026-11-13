//! Preset selection and the analytic response model.

use clap::Args;
use neofilter::presets::{factory_presets, PresetFile};
use neofilter_dsp::response::{analog_band_response_db, total_response_db};
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::BandParams;
use std::path::PathBuf;

/// Where the preset comes from; accepted by every command that needs one.
#[derive(Args, Debug, Clone)]
pub struct PresetSel {
    /// Factory preset id (see `list`).
    #[arg(
        long,
        conflicts_with = "preset_file",
        required_unless_present = "preset_file"
    )]
    pub preset: Option<String>,
    /// Preset JSON file (the plugin's "Copy preset" format).
    #[arg(long, value_name = "FILE")]
    pub preset_file: Option<PathBuf>,
}

/// A resolved preset: everything needed to model or render it.
pub struct Model {
    pub name: String,
    pub bands: [BandParams; NUM_BANDS],
    /// 1.0 = 100 %.
    pub gain_scale: f64,
    pub out_gain_db: f64,
}

impl PresetSel {
    pub fn resolve(&self) -> Result<Model, String> {
        if let Some(path) = &self.preset_file {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read preset file {}: {e}", path.display()))?;
            let f = PresetFile::from_json(&text)
                .map_err(|e| format!("invalid preset file {}: {e}", path.display()))?;
            return Ok(Model {
                name: if f.name.is_empty() {
                    path.display().to_string()
                } else {
                    f.name.clone()
                },
                bands: f.band_params(),
                gain_scale: f64::from(f.gain_scale) / 100.0,
                out_gain_db: f64::from(f.out_gain_db),
            });
        }
        let id = self.preset.as_deref().unwrap_or_default();
        let presets = factory_presets();
        match presets.iter().find(|p| p.id == id) {
            Some(p) => Ok(Model {
                name: p.name.to_string(),
                bands: p.band_params(),
                gain_scale: f64::from(p.gain_scale) / 100.0,
                out_gain_db: f64::from(p.out_gain_db),
            }),
            None => {
                let ids: Vec<&str> = presets.iter().map(|p| p.id).collect();
                Err(format!(
                    "unknown preset '{id}' (available: {})",
                    ids.join(", ")
                ))
            }
        }
    }
}

impl Model {
    /// Model response in dB (including output gain): digital (what is heard)
    /// or the analog prototype sum.
    pub fn response_db(&self, sample_rate: f64, freqs: &[f64], analog: bool) -> Vec<f64> {
        let mut out = vec![0.0; freqs.len()];
        if analog {
            let mut tmp = vec![0.0; freqs.len()];
            for b in self.bands.iter().filter(|b| b.used && b.enabled) {
                analog_band_response_db(b, self.gain_scale, sample_rate, freqs, &mut tmp);
                for (o, t) in out.iter_mut().zip(&tmp) {
                    *o += t;
                }
            }
        } else {
            total_response_db(&self.bands, self.gain_scale, sample_rate, freqs, &mut out);
        }
        for o in &mut out {
            *o += self.out_gain_db;
        }
        out
    }
}

/// `n` log-spaced frequencies from `lo` to `hi` inclusive.
pub fn log_space(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let d = (n.max(2) - 1) as f64;
    (0..n).map(|i| lo * (hi / lo).powf(i as f64 / d)).collect()
}
