//! WAV reading and writing.

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::path::Path;

pub struct Audio {
    pub sample_rate: u32,
    /// One vector per channel (1 or 2), samples in [-1, 1).
    pub channels: Vec<Vec<f32>>,
}

/// Read 8..32-bit integer or 32-bit float WAV, mono or stereo.
pub fn read(path: &Path) -> Result<Audio, String> {
    let ctx = |e: hound::Error| format!("cannot read {}: {e}", path.display());
    let mut r = WavReader::open(path).map_err(ctx)?;
    let spec = r.spec();
    let nch = usize::from(spec.channels);
    if nch == 0 || nch > 2 {
        return Err(format!(
            "{}: {nch} channels not supported (mono or stereo only)",
            path.display()
        ));
    }
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => r.samples::<f32>().collect::<Result<_, _>>().map_err(ctx)?,
        (SampleFormat::Int, bits @ 8..=32) => {
            let scale = 1.0 / (2f64.powi(i32::from(bits) - 1));
            r.samples::<i32>()
                .map(|s| s.map(|v| (f64::from(v) * scale) as f32))
                .collect::<Result<_, _>>()
                .map_err(ctx)?
        }
        (fmt, bits) => {
            return Err(format!(
                "{}: unsupported sample format {fmt:?} with {bits} bits \
                 (use 16/24/32-bit integer or 32-bit float)",
                path.display()
            ))
        }
    };
    if samples.is_empty() {
        return Err(format!("{}: file contains no samples", path.display()));
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(format!("{}: file contains NaN/inf samples", path.display()));
    }
    let mut channels = vec![Vec::with_capacity(samples.len() / nch); nch];
    for frame in samples.chunks_exact(nch) {
        for (c, &s) in channels.iter_mut().zip(frame) {
            c.push(s);
        }
    }
    Ok(Audio {
        sample_rate: spec.sample_rate,
        channels,
    })
}

/// Write 32-bit float WAV.
pub fn write(path: &Path, audio: &Audio) -> Result<(), String> {
    let ctx = |e: hound::Error| format!("cannot write {}: {e}", path.display());
    let spec = WavSpec {
        channels: audio.channels.len() as u16,
        sample_rate: audio.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut w = WavWriter::create(path, spec).map_err(ctx)?;
    let n = audio.channels.first().map_or(0, Vec::len);
    for i in 0..n {
        for c in &audio.channels {
            w.write_sample(c[i]).map_err(ctx)?;
        }
    }
    w.finalize().map_err(ctx)
}
