//! `neofilter-cli calibrate` against the synthetic loop of the plugin tests.
use neo_measure::generator::{Generator, PinkNoise};
use neofilter::audio::{AudioParams, AudioPath};
use neofilter::calibration::settings::{CalCurve, CurvePoint};
use neofilter::calibration::sim::LoopSim;
use neofilter::presets::PresetFile;
use neofilter_dsp::response::total_response_db;
use neofilter_dsp::types::NUM_BANDS;
use neofilter_dsp::{BandParams, Shape};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FS: f64 = 48_000.0;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_neofilter-cli"))
        .args(args)
        .output()
        .expect("run cli")
}

struct Dir(PathBuf);
impl Dir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("neofilter-cal-{}-{name}", std::process::id()));
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

fn band(shape: Shape, f: f64, g: f64, q: f64) -> BandParams {
    BandParams {
        used: true,
        shape,
        freq_hz: f,
        gain_db: g,
        q,
        ..Default::default()
    }
}

fn system() -> [BandParams; NUM_BANDS] {
    let mut b = [BandParams::default(); NUM_BANDS];
    b[0] = band(Shape::Bell, 9000.0, 5.0, 3.0);
    b[1] = band(Shape::Bell, 250.0, -4.0, 1.0);
    b[2] = band(Shape::HighShelf, 12000.0, -6.0, 1.0);
    b[3] = band(Shape::LowShelf, 60.0, 3.0, 1.0);
    b
}

struct Recording {
    pre: Vec<f32>,
    post: Vec<f32>,
    ret: [Vec<f32>; 2],
}

/// Pink noise through NeoFilter's EQ (`eq`) and the simulated loop.
fn record(
    secs: f64,
    delay_s: f64,
    sys: &[BandParams; NUM_BANDS],
    eq: &[BandParams; NUM_BANDS],
    gain_db: f64,
    noise_dbfs: Option<f64>,
) -> Recording {
    let n = (secs * FS) as usize;
    let block = 480;
    let mut gen = PinkNoise::new(FS, -20.0, 99);
    let mut sim = LoopSim::new(FS, delay_s, sys, false, gain_db, noise_dbfs, 5);
    let mut path = AudioPath::new(FS, block);
    let p = AudioParams {
        bands: eq,
        gain_scale: 1.0,
        out_gain_db: 0.0,
        bypass: false,
    };
    let (mut pre, mut post) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut rl, mut rr) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let mut done = 0;
    while done < n {
        let k = block.min(n - done);
        let mut l: Vec<f32> = (0..k).map(|_| gen.next_sample() as f32).collect();
        let mut r = l.clone();
        let (mut a, mut b) = (vec![0.0f32; k], vec![0.0f32; k]);
        sim.pull(&mut a, &mut b);
        pre.extend_from_slice(&l);
        path.process_stereo(&mut l, &mut r, &p);
        post.extend_from_slice(&l);
        sim.push(&l, &r);
        rl.extend_from_slice(&a);
        rr.extend_from_slice(&b);
        done += k;
    }
    Recording {
        pre,
        post,
        ret: [rl, rr],
    }
}

fn write_wav(path: &str, channels: &[&[f32]]) {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: FS as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..channels[0].len() {
        for c in channels {
            w.write_sample(c[i]).unwrap();
        }
    }
    w.finalize().unwrap();
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn after(text: &str, key: &str) -> f64 {
    let line = text
        .lines()
        .find(|l| l.contains(key))
        .unwrap_or_else(|| panic!("{key} in {text}"));
    let rest = &line[line.find(key).unwrap() + key.len()..];
    rest.trim_start()
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// In-range deviation of system + eq from flat (300 Hz-3 kHz mean = 0).
fn analytic_deviation(sys: &[BandParams; NUM_BANDS], eq: &[BandParams; NUM_BANDS]) -> (f64, f64) {
    let f: Vec<f64> = (0..400)
        .map(|i| 20.0 * 1000f64.powf(i as f64 / 399.0))
        .collect();
    let (mut a, mut b) = (vec![0.0; f.len()], vec![0.0; f.len()]);
    total_response_db(sys, 1.0, FS, &f, &mut a);
    total_response_db(eq, 1.0, FS, &f, &mut b);
    let tot: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x + y).collect();
    let sel: Vec<f64> = f
        .iter()
        .zip(&tot)
        .filter(|(f, _)| (300.0..=3000.0).contains(*f))
        .map(|(_, v)| *v)
        .collect();
    let mean = sel.iter().sum::<f64>() / sel.len() as f64;
    let dev: Vec<f64> = f
        .iter()
        .zip(&tot)
        .filter(|(f, _)| (30.0..=16000.0).contains(*f))
        .map(|(_, v)| v - mean)
        .collect();
    let rms = (dev.iter().map(|d| d * d).sum::<f64>() / dev.len() as f64).sqrt();
    (rms, dev.iter().fold(0.0, |m: f64, d| m.max(d.abs())))
}

fn preset_bands(path: &str) -> [BandParams; NUM_BANDS] {
    PresetFile::from_json(&std::fs::read_to_string(path).unwrap())
        .unwrap()
        .band_params()
}

#[test]
fn calibrate_flattens_the_simulated_loop() {
    let d = Dir::new("flat");
    let sys = system();
    let none = [BandParams::default(); NUM_BANDS];
    let rec = record(10.0, 0.3712, &sys, &none, 0.0, Some(-65.0));
    write_wav(&d.file("sent.wav"), &[&rec.pre]);
    write_wav(&d.file("ret.wav"), &[&rec.ret[0], &rec.ret[1]]);
    let o = cli(&[
        "calibrate",
        "--sent",
        &d.file("sent.wav"),
        "--return",
        &d.file("ret.wav"),
        "--output",
        &d.file("fitted.json"),
    ]);
    let text = stdout(&o);
    println!("{text}");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!((after(&text, "delay left:") - 371.2).abs() < 0.05, "{text}");
    assert!(after(&text, "predicted after fit: rms") < 0.3, "{text}");
    assert!(text.contains("polarity normal"));
    let fitted = preset_bands(&d.file("fitted.json"));
    let used = fitted.iter().filter(|b| b.used).count();
    let (rms, max) = analytic_deviation(&sys, &fitted);
    println!("analytic after one pass: rms {rms:.3} max {max:.3} with {used} bands");
    assert!(used <= 12 && used > 0);
    assert!(rms <= 0.5 && max <= 1.0);

    // feed the fitted preset back: record again through it and measure
    let rec2 = record(10.0, 0.3712, &sys, &fitted, 0.0, Some(-65.0));
    write_wav(&d.file("sent2.wav"), &[&rec2.pre]);
    write_wav(&d.file("sent2_post.wav"), &[&rec2.post]);
    write_wav(&d.file("ret2.wav"), &[&rec2.ret[0], &rec2.ret[1]]);
    for post in [false, true] {
        let sent = d.file(if post { "sent2_post.wav" } else { "sent2.wav" });
        let mut args = vec![
            "calibrate",
            "--sent",
            &sent,
            "--return",
            "",
            "--preset-file",
            "",
            "--cal-slots",
            "1-32",
            "--output",
            "",
        ];
        let (ret2, fj, out2) = (
            d.file("ret2.wav"),
            d.file("fitted.json"),
            d.file("fitted2.json"),
        );
        args[4] = &ret2;
        args[6] = &fj;
        args[10] = &out2;
        if post {
            args.push("--sent-is-post-eq");
        }
        let o = cli(&args);
        let t = stdout(&o);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let rms = after(&t, "loop deviation in 30-16000 Hz: rms");
        println!("measured deviation of the fitted loop (post_eq={post}): rms {rms:.3}");
        assert!(rms <= 0.5, "{t}");
        assert!(after(&t, "max") <= 1.0 + 1e-9);
    }
}

#[test]
fn calibrate_with_manual_bands_and_partial_playback_curve() {
    let d = Dir::new("manual");
    // current EQ: one manual band in slot 1; loop has an extra playback chain
    let mut eq = [BandParams::default(); NUM_BANDS];
    eq[0] = band(Shape::Bell, 1000.0, 2.0, 1.0);
    let play = [band(Shape::HighShelf, 4000.0, -3.0, 1.0)];
    let mut sys = system();
    sys[10] = play[0];
    let rec = record(10.0, 0.45, &sys, &eq, 0.0, Some(-65.0));
    write_wav(&d.file("sent.wav"), &[&rec.pre]);
    write_wav(&d.file("ret.wav"), &[&rec.ret[0]]); // mono return
    let mut pb = [BandParams::default(); NUM_BANDS];
    pb[0] = play[0];
    let f: Vec<f64> = (0..20)
        .map(|i| 800.0 * 25f64.powf(i as f64 / 19.0))
        .collect();
    let mut db = vec![0.0; f.len()];
    total_response_db(&pb, 1.0, FS, &f, &mut db);
    let curve = CalCurve::new(
        "record",
        f.iter()
            .zip(&db)
            .map(|(&freq_hz, &db)| CurvePoint { freq_hz, db })
            .collect(),
    );
    std::fs::write(d.file("curve.json"), curve.to_json()).unwrap();
    std::fs::write(
        d.file("eq.json"),
        PresetFile::from_band_params("manual", 100.0, 0.0, &eq).to_json(),
    )
    .unwrap();
    let o = cli(&[
        "calibrate",
        "--sent",
        &d.file("sent.wav"),
        "--return",
        &d.file("ret.wav"),
        "--preset-file",
        &d.file("eq.json"),
        "--playback-curve",
        &d.file("curve.json"),
        "--output",
        &d.file("fitted.json"),
        "--max-bands",
        "10",
    ]);
    let t = stdout(&o);
    println!("{t}");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(t.contains("'record' (800-20000 Hz)"));
    let fitted = preset_bands(&d.file("fitted.json"));
    assert_eq!(fitted[0], eq[0], "manual band untouched");
    assert!(fitted.iter().filter(|b| b.used).count() <= 11);
    // flat after removing the known playback chain
    let mut sys_wo_p = system();
    sys_wo_p[10] = BandParams::default();
    let (rms, max) = analytic_deviation(&sys_wo_p, &fitted);
    println!("analytic (without playback chain): rms {rms:.3} max {max:.3}");
    assert!(rms <= 0.5 && max <= 1.0);
}

#[test]
fn calibrate_reports_failures_and_bad_arguments() {
    let d = Dir::new("fail");
    let sys = system();
    let none = [BandParams::default(); NUM_BANDS];
    let rec = record(8.0, 0.4, &sys, &none, -200.0, None); // silent return
    write_wav(&d.file("sent.wav"), &[&rec.pre]);
    write_wav(&d.file("ret.wav"), &[&rec.ret[0]]);
    let o = cli(&[
        "calibrate",
        "--sent",
        &d.file("sent.wav"),
        "--return",
        &d.file("ret.wav"),
        "--output",
        &d.file("x.json"),
    ]);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(
        e.contains("too low") && e.contains("nothing was fitted"),
        "{e}"
    );
    assert!(!Path::new(&d.file("x.json")).exists());
    for bad in [
        ["--range", "100"],
        ["--range", "100:50"],
        ["--cal-slots", "0"],
    ] {
        let o = cli(&[
            "calibrate",
            "--sent",
            &d.file("sent.wav"),
            "--return",
            &d.file("ret.wav"),
            bad[0],
            bad[1],
        ]);
        assert!(!o.status.success(), "{bad:?}");
    }
}
