use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_neofilter-cli"))
        .args(args)
        .output()
        .expect("run cli")
}

struct Dir(PathBuf);
impl Dir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("neofilter-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
    fn file(&self, n: &str) -> String {
        self.0.join(n).to_string_lossy().into_owned()
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn field(text: &str, key: &str) -> f64 {
    let line = text.lines().find(|l| l.starts_with(key)).expect(key);
    line[key.len()..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn list_shows_factory_presets() {
    let o = cli(&["list"]);
    assert!(o.status.success());
    let s = stdout(&o);
    assert!(s.contains("friend-eq-a") && s.contains("friend-eq-b") && s.contains("default"));
    assert!(s.contains("friend-eq-ab") && s.contains("Friend EQ A+B (series)"));
}

#[test]
fn curve_of_the_combined_preset_is_the_sum_of_a_and_b() {
    let col = |preset: &str| -> Vec<f64> {
        let o = cli(&["curve", "--preset", preset, "--points", "100"]);
        assert!(o.status.success(), "{}", stderr(&o));
        stdout(&o)
            .lines()
            .filter_map(|l| l.split(',').nth(1)?.parse().ok())
            .collect()
    };
    let (a, b, ab) = (col("friend-eq-a"), col("friend-eq-b"), col("friend-eq-ab"));
    assert!(ab.len() >= 100 && ab.len() == a.len() && ab.len() == b.len());
    for i in 0..ab.len() {
        assert!((ab[i] - (a[i] + b[i])).abs() < 0.01, "point {i}");
    }
}

#[test]
fn impulse_then_compare_is_zero_deviation() {
    let d = Dir::new("ir");
    for (preset, sr) in [("friend-eq-a", "48000"), ("friend-eq-b", "44100")] {
        let ir = d.file("ir.wav");
        let o = cli(&[
            "impulse",
            "--preset",
            preset,
            "--sample-rate",
            sr,
            "--output",
            &ir,
        ]);
        assert!(o.status.success(), "{}", stderr(&o));
        let o = cli(&["compare", "--preset", preset, "--ir", &ir]);
        assert!(o.status.success(), "{}", stderr(&o));
        let s = stdout(&o);
        println!("{s}");
        let max = field(&s, "max deviation:");
        let rms = field(&s, "rms deviation:");
        assert!(max < 0.01, "max {max}");
        assert!(rms < 0.005, "rms {rms}");
    }
}

#[test]
fn compare_detects_a_different_preset() {
    let d = Dir::new("diff");
    let ir = d.file("ir.wav");
    assert!(
        cli(&["impulse", "--preset", "friend-eq-a", "--output", &ir])
            .status
            .success()
    );
    let o = cli(&["compare", "--preset", "friend-eq-b", "--ir", &ir]);
    assert!(o.status.success());
    assert!(field(&stdout(&o), "max deviation:") > 1.0);
}

#[test]
fn preset_file_is_accepted() {
    let d = Dir::new("pf");
    let json = d.file("p.json");
    std::fs::write(
        &json,
        r#"{"name":"t","bands":[{"band":1,"shape":"bell","freq_hz":1000,"gain_db":6,"q":1}]}"#,
    )
    .unwrap();
    let o = cli(&["curve", "--preset-file", &json, "--points", "64"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let peak = stdout(&o)
        .lines()
        .skip(1)
        .map(|l| l.split(',').nth(1).unwrap().parse::<f64>().unwrap())
        .fold(f64::MIN, f64::max);
    assert!((peak - 6.0).abs() < 0.2, "peak {peak}");
    let ir = d.file("ir.wav");
    assert!(cli(&["impulse", "--preset-file", &json, "--output", &ir])
        .status
        .success());
    let o = cli(&["compare", "--preset-file", &json, "--ir", &ir]);
    assert!(field(&stdout(&o), "max deviation:") < 0.01);
}

#[test]
fn curve_digital_analog_and_csv() {
    let d = Dir::new("curve");
    let o = cli(&["curve", "--preset", "friend-eq-a", "--points", "100"]);
    assert!(o.status.success());
    assert_eq!(stdout(&o).lines().count(), 101);
    let csv = d.file("a.csv");
    let o = cli(&[
        "curve",
        "--preset",
        "friend-eq-a",
        "--analog",
        "--points",
        "100",
        "--csv",
        &csv,
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("freq_hz,db\n"));
    assert_eq!(text.lines().count(), 101);
}

fn write_wav(path: &str, spec: hound::WavSpec, samples: &[f32]) {
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, _) => w.write_sample(s).unwrap(),
            (_, bits) => w
                .write_sample((f64::from(s) * 2f64.powi(i32::from(bits) - 1)) as i32)
                .unwrap(),
        }
    }
    w.finalize().unwrap();
}

#[test]
fn render_default_preset_is_transparent_for_all_formats() {
    let d = Dir::new("render");
    let input: Vec<f32> = (0..2000)
        .map(|i| 0.5 * (i as f32 * 0.05).sin() * (1.0 - i as f32 / 2000.0))
        .collect();
    for (ch, bits, float) in [
        (1u16, 16u16, false),
        (2, 24, false),
        (2, 32, false),
        (1, 32, true),
        (2, 32, true),
    ] {
        let mut samples = Vec::new();
        for &s in &input {
            for _ in 0..ch {
                samples.push(s);
            }
        }
        let spec = hound::WavSpec {
            channels: ch,
            sample_rate: 44100,
            bits_per_sample: bits,
            sample_format: if float {
                hound::SampleFormat::Float
            } else {
                hound::SampleFormat::Int
            },
        };
        let (inp, outp) = (d.file("in.wav"), d.file("out.wav"));
        write_wav(&inp, spec, &samples);
        let o = cli(&[
            "render", "--preset", "default", "--input", &inp, "--output", &outp,
        ]);
        assert!(o.status.success(), "{}", stderr(&o));
        let mut r = hound::WavReader::open(&outp).unwrap();
        let s = r.spec();
        assert_eq!(
            (
                s.channels,
                s.sample_rate,
                s.bits_per_sample,
                s.sample_format
            ),
            (ch, 44100, 32, hound::SampleFormat::Float)
        );
        let got: Vec<f32> = r.samples::<f32>().map(|v| v.unwrap()).collect();
        assert_eq!(got.len(), samples.len());
        let q = 2f32.powi(i32::from(bits) - 1);
        // quantised input is reproduced exactly (default preset is bit transparent)
        for (g, s) in got.iter().zip(&samples) {
            let want = if float { *s } else { (s * q).trunc() / q };
            assert!((g - want).abs() < 1e-6, "{g} vs {want} ({bits} bit)");
        }
    }
}

#[test]
fn render_applies_the_preset() {
    let d = Dir::new("render2");
    let n = 4000;
    let samples: Vec<f32> = (0..n)
        .map(|i| 0.1 * (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin())
        .collect();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let (inp, outp) = (d.file("in.wav"), d.file("out.wav"));
    write_wav(&inp, spec, &samples);
    let json = d.file("p.json");
    std::fs::write(
        &json,
        r#"{"bands":[{"band":1,"shape":"bell","freq_hz":1000,"gain_db":12,"q":1}]}"#,
    )
    .unwrap();
    let o = cli(&[
        "render",
        "--preset-file",
        &json,
        "--input",
        &inp,
        "--output",
        &outp,
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let got: Vec<f32> = hound::WavReader::open(&outp)
        .unwrap()
        .samples::<f32>()
        .map(|v| v.unwrap())
        .collect();
    let peak = got[n / 2..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
    // +12 dB at the centre: x3.98
    assert!((peak / 0.1 - 3.98).abs() < 0.1, "peak {peak}");
}

#[test]
fn errors_are_reported_cleanly() {
    let d = Dir::new("err");
    let missing = d.file("missing.wav");
    let out = d.file("o.wav");
    let bad_json = d.file("bad.json");
    std::fs::write(&bad_json, "{ not json").unwrap();
    let garbage = d.file("garbage.wav");
    std::fs::write(&garbage, b"this is not a wav file").unwrap();
    let eight = d.file("eight.wav");
    let spec = hound::WavSpec {
        channels: 3,
        sample_rate: 48000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    write_wav(&eight, spec, &[0.0; 30]);

    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["bogus"],
        vec!["curve"],
        vec!["curve", "--preset", "nope"],
        vec!["curve", "--preset", "default", "--preset-file", &bad_json],
        vec!["curve", "--preset-file", &bad_json],
        vec!["curve", "--preset-file", &missing],
        vec!["curve", "--preset", "default", "--points", "1"],
        vec!["curve", "--preset", "default", "--points", "abc"],
        vec!["curve", "--preset", "default", "--sample-rate", "5"],
        vec!["curve", "--preset", "default", "--sample-rate", "nan"],
        vec![
            "render", "--preset", "default", "--input", &missing, "--output", &out,
        ],
        vec![
            "render", "--preset", "default", "--input", &garbage, "--output", &out,
        ],
        vec![
            "render", "--preset", "default", "--input", &eight, "--output", &out,
        ],
        vec![
            "impulse", "--preset", "default", "--output", &out, "--length", "1",
        ],
        vec![
            "impulse",
            "--preset",
            "default",
            "--output",
            "/nonexistent-dir/x.wav",
        ],
        vec!["compare", "--preset", "default", "--ir", &missing],
        vec!["compare", "--preset", "default", "--ir", &garbage],
    ];
    for args in cases {
        let o = cli(&args);
        assert!(!o.status.success(), "{args:?} should fail");
        let code = o.status.code();
        assert!(matches!(code, Some(1) | Some(2)), "{args:?}: {code:?}");
        let e = stderr(&o);
        assert!(!e.contains("panicked"), "{args:?}: {e}");
        assert!(!e.is_empty(), "{args:?}");
    }
    let _ = Path::new(&out);
}

/// A minimal `.ffp` template: 24 unused default bands plus 46 global values.
fn ffp_template() -> Vec<u8> {
    let unused: [f32; 13] = [
        0.0, 0.0, 9.965_784, 0.0, 0.0, 1.0, 1.0, 0.5, 0.0, 1.0, 2.0, 1.0, 0.0,
    ];
    let mut values: Vec<f32> = Vec::new();
    for _ in 0..24 {
        values.extend_from_slice(&unused);
    }
    values.extend(std::iter::repeat_n(0.0, 46));
    let mut bytes = b"FQ3p".to_vec();
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

#[test]
fn ffp_export_then_import_reproduces_the_curve() {
    let d = Dir::new("ffp");
    let (tpl, ffp, json) = (d.file("tpl.ffp"), d.file("ab.ffp"), d.file("ab.json"));
    std::fs::write(&tpl, ffp_template()).unwrap();

    let o = cli(&[
        "export-ffp",
        "--preset",
        "friend-eq-ab",
        "--template",
        &tpl,
        "--output",
        &ffp,
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("24 bands"));
    assert_eq!(std::fs::read(&ffp).unwrap().len(), 1444);

    let o = cli(&["import-ffp", "--input", &ffp, "--output", &json]);
    assert!(o.status.success(), "{}", stderr(&o));

    let a = cli(&["curve", "--preset", "friend-eq-ab", "--points", "200"]);
    let b = cli(&["curve", "--preset-file", &json, "--points", "200"]);
    assert!(a.status.success() && b.status.success());
    let parse = |o: &Output| -> Vec<f64> {
        stdout(o)
            .lines()
            .skip(1)
            .map(|l| l.split(',').nth(1).unwrap().parse().unwrap())
            .collect()
    };
    let (a, b) = (parse(&a), parse(&b));
    assert_eq!(a.len(), 200);
    // The file stores f32 values; the curve must survive that within 0.001 dB.
    for (x, y) in a.iter().zip(&b) {
        assert!((x - y).abs() < 1e-3, "{x} vs {y}");
    }
}

#[test]
fn ffp_errors_are_reported_cleanly() {
    let d = Dir::new("ffp-err");
    let bad = d.file("bad.ffp");
    std::fs::write(&bad, b"not a preset").unwrap();
    for args in [
        vec!["import-ffp", "--input", &bad],
        vec!["import-ffp", "--input", "/nonexistent.ffp"],
        vec![
            "export-ffp",
            "--preset",
            "friend-eq-a",
            "--template",
            &bad,
            "--output",
            "/tmp/x.ffp",
        ],
    ] {
        let o = cli(&args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(stderr(&o).starts_with("error:"), "{}", stderr(&o));
    }
}
