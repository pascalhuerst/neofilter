//! NeoFilter command line tool: preset listing, response export, offline
//! rendering and impulse-response comparison (calibration).

mod calibrate;
mod compare;
mod ffp;
mod model;
mod wavio;

use clap::{Parser, Subcommand};
use model::{log_space, PresetSel};
use neofilter::audio::{AudioParams, AudioPath};
use neofilter::presets::{factory_presets, PresetFile};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

/// NeoFilter command line tool.
///
/// The model includes the preset's output gain, as the plugin does.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the factory presets.
    List,
    /// Print or export a preset's magnitude response (20 Hz - 20 kHz).
    Curve {
        #[command(flatten)]
        preset: PresetSel,
        #[arg(long, default_value_t = 48000.0)]
        sample_rate: f64,
        #[arg(long, default_value_t = 512)]
        points: usize,
        /// Use the analog prototype instead of the digital filters.
        #[arg(long)]
        analog: bool,
        /// Write CSV to this file instead of printing.
        #[arg(long, value_name = "FILE")]
        csv: Option<PathBuf>,
    },
    /// Process a WAV file through a preset (output: 32-bit float WAV).
    Render {
        #[command(flatten)]
        preset: PresetSel,
        #[arg(long, value_name = "WAV")]
        input: PathBuf,
        #[arg(long, value_name = "WAV")]
        output: PathBuf,
    },
    /// Write a preset's impulse response (mono, 32-bit float WAV).
    Impulse {
        #[command(flatten)]
        preset: PresetSel,
        #[arg(long, default_value_t = 48000)]
        sample_rate: u32,
        /// Length in samples.
        #[arg(long, default_value_t = 131072)]
        length: usize,
        #[arg(long, value_name = "WAV")]
        output: PathBuf,
    },
    /// Compare a measured impulse response with the model's digital response.
    Compare {
        #[command(flatten)]
        preset: PresetSel,
        /// Measured impulse response (mono, or the left channel is used).
        #[arg(long, value_name = "WAV")]
        ir: PathBuf,
    },
    /// Write a preset as an `.ffp` EQ preset file (24 bands, Stereo placement only).
    ///
    /// Settings this tool does not model (dynamics, global options) are
    /// copied from the template, which must be an existing `.ffp` export.
    ExportFfp {
        #[command(flatten)]
        preset: PresetSel,
        /// An existing `.ffp` file exported from the target EQ.
        #[arg(long, value_name = "FFP")]
        template: PathBuf,
        #[arg(long, value_name = "FFP")]
        output: PathBuf,
    },
    /// Fit calibration bands from a recording of the sent noise and of the
    /// loop return (the plugin's loop calibration, offline).
    ///
    /// Prints delay, deviation statistics and the fitted bands; writes the
    /// complete fitted preset (usable with "Paste preset").
    Calibrate(calibrate::CalibrateArgs),
    /// Convert an `.ffp` EQ preset file into NeoFilter preset JSON (for "Paste preset").
    ImportFfp {
        #[arg(long, value_name = "FFP")]
        input: PathBuf,
        /// Write the JSON to this file instead of printing it.
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
}

fn read_ffp(path: &std::path::Path) -> Result<ffp::FfpFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    ffp::FfpFile::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn check_rate(sr: f64) -> Result<(), String> {
    if sr.is_finite() && (8000.0..=768_000.0).contains(&sr) {
        Ok(())
    } else {
        Err(format!("sample rate {sr} out of range 8000..768000"))
    }
}

/// Run a preset through the plugin's audio path.
fn run_path(model: &model::Model, sample_rate: f64, channels: &mut [Vec<f32>]) {
    const BLOCK: usize = 4096;
    let mut path = AudioPath::new(sample_rate, BLOCK);
    let p = AudioParams {
        bands: &model.bands,
        gain_scale: model.gain_scale,
        out_gain_db: model.out_gain_db,
        bypass: false,
    };
    match channels {
        [m] => {
            for chunk in m.chunks_mut(BLOCK) {
                path.process_mono(chunk, &p);
            }
        }
        [l, r] => {
            for (cl, cr) in l.chunks_mut(BLOCK).zip(r.chunks_mut(BLOCK)) {
                path.process_stereo(cl, cr, &p);
            }
        }
        _ => {}
    }
}

fn run(cli: Cli) -> Result<String, String> {
    let mut out = String::new();
    match cli.command {
        Command::List => {
            for p in factory_presets() {
                let n = p.bands.iter().filter(|b| b.is_some()).count();
                let _ = writeln!(out, "{:<14} {} ({n} bands)", p.id, p.name);
            }
        }
        Command::Curve {
            preset,
            sample_rate,
            points,
            analog,
            csv,
        } => {
            check_rate(sample_rate)?;
            if !(2..=1_000_000).contains(&points) {
                return Err("--points must be between 2 and 1000000".into());
            }
            let model = preset.resolve()?;
            let hi = compare::F_HI.min(0.49 * sample_rate);
            let freqs = log_space(compare::F_LO, hi, points);
            let db = model.response_db(sample_rate, &freqs, analog);
            let mut text = String::from("freq_hz,db\n");
            for (f, d) in freqs.iter().zip(&db) {
                let _ = writeln!(text, "{f:.3},{d:.4}");
            }
            match csv {
                Some(path) => {
                    std::fs::write(&path, text)
                        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                    let _ = writeln!(out, "wrote {points} points to {}", path.display());
                }
                None => out = text,
            }
        }
        Command::Render {
            preset,
            input,
            output,
        } => {
            let model = preset.resolve()?;
            let mut audio = wavio::read(&input)?;
            check_rate(f64::from(audio.sample_rate))?;
            run_path(&model, f64::from(audio.sample_rate), &mut audio.channels);
            wavio::write(&output, &audio)?;
            let _ = writeln!(
                out,
                "rendered {} samples x {} ch through '{}' to {}",
                audio.channels[0].len(),
                audio.channels.len(),
                model.name,
                output.display()
            );
        }
        Command::Impulse {
            preset,
            sample_rate,
            length,
            output,
        } => {
            check_rate(f64::from(sample_rate))?;
            if !(16..=50_000_000).contains(&length) {
                return Err("--length must be between 16 and 50000000".into());
            }
            let model = preset.resolve()?;
            let mut ch = vec![vec![0.0f32; length]];
            ch[0][0] = 1.0;
            run_path(&model, f64::from(sample_rate), &mut ch);
            let audio = wavio::Audio {
                sample_rate,
                channels: ch,
            };
            wavio::write(&output, &audio)?;
            let _ = writeln!(
                out,
                "wrote {length} samples at {sample_rate} Hz to {}",
                output.display()
            );
        }
        Command::Compare { preset, ir } => {
            let model = preset.resolve()?;
            let audio = wavio::read(&ir)?;
            let sr = f64::from(audio.sample_rate);
            check_rate(sr)?;
            let r = compare::compare(&audio.channels[0], sr, &model)?;
            let _ = writeln!(
                out,
                "preset '{}', IR {} samples at {} Hz, {} bins in {}-{} Hz",
                model.name,
                audio.channels[0].len(),
                audio.sample_rate,
                r.bins,
                compare::F_LO,
                compare::F_HI.min(0.49 * sr)
            );
            let _ = writeln!(
                out,
                "max deviation: {:.4} dB at {:.1} Hz",
                r.max_abs_db, r.worst_freq
            );
            let _ = writeln!(out, "rms deviation: {:.4} dB", r.rms_db);
            let _ = writeln!(out, "worst regions (third-octave):");
            let _ = writeln!(
                out,
                "  {:>10} {:>10} {:>10} {:>9} {:>9}",
                "from Hz", "to Hz", "worst Hz", "max dB", "mean dB"
            );
            for g in &r.regions {
                let _ = writeln!(
                    out,
                    "  {:>10.1} {:>10.1} {:>10.1} {:>9.4} {:>+9.4}",
                    g.lo, g.hi, g.worst_freq, g.max_abs_db, g.mean_db
                );
            }
        }
        Command::Calibrate(args) => out = calibrate::run(args)?,
        Command::ExportFfp {
            preset,
            template,
            output,
        } => {
            let model = preset.resolve()?;
            if (model.gain_scale - 1.0).abs() > 1e-9 || model.out_gain_db.abs() > 1e-9 {
                return Err(
                    "preset uses gain scale or output gain, which cannot be exported".into(),
                );
            }
            let mut file = read_ffp(&template)?;
            file.set_bands(&model.bands)?;
            std::fs::write(&output, file.to_bytes())
                .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
            let used = model.bands.iter().filter(|b| b.used).count();
            let _ = writeln!(
                out,
                "wrote '{}' ({used} bands) to {}",
                model.name,
                output.display()
            );
        }
        Command::ImportFfp { input, output } => {
            let bands = read_ffp(&input)?
                .bands()
                .map_err(|e| format!("{}: {e}", input.display()))?;
            let name = input
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let json = PresetFile::from_band_params(&name, 100.0, 0.0, &bands).to_json();
            match output {
                Some(path) => {
                    std::fs::write(&path, json)
                        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                    let used = bands.iter().filter(|b| b.used).count();
                    let _ = writeln!(out, "wrote {used} bands to {}", path.display());
                }
                None => {
                    let _ = writeln!(out, "{json}");
                }
            }
        }
    }
    Ok(out)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
