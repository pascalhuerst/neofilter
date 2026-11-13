//! RIAA design tables and timing.
//!
//! ```text
//! cargo run --release -p neo-measure --example riaa_tables -- tables > crates/neo-measure/src/riaa_tables.rs
//! cargo run --release -p neo-measure --example riaa_tables -- time
//! ```
//!
//! `tables` runs the full optimiser for every rate in `TABLE_RATES` (a few
//! seconds in total) and prints the Rust source of `src/riaa_tables.rs`.
//! `time` prints the design time and accuracy of the full optimiser, the
//! quick design and the table lookup per rate (plus a few off-table rates).

use neo_measure::riaa::*;
use std::time::Instant;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("tables") => print_tables(),
        Some("time") => timing(),
        _ => eprintln!("usage: riaa_tables tables|time"),
    }
}

fn print_tables() {
    println!("//! Precomputed RIAA designs (generated; do not edit by hand).");
    println!("//!");
    println!(
        "//! Regenerate: `cargo run --release -p neo-measure --example riaa_tables -- tables`"
    );
    println!("//! `> crates/neo-measure/src/riaa_tables.rs`. Each entry is the result of the");
    println!("//! full optimiser (`optimise_family`) for the plain and the ultrasonic-pole");
    println!("//! de-emphasis family: parameters `[ln gain, (ln wz, ln Qz, ln wp, ln Qp)...]`,");
    println!("//! the phase weight of the winning pass and the worst magnitude (dB) / phase");
    println!("//! (degrees) error over 20 Hz .. min(20 kHz, 0.45 fs).");
    println!();
    println!("use crate::riaa::{{TableEntry, TableFamily}};");
    println!();
    println!("#[rustfmt::skip]");
    println!("pub(crate) static TABLE: &[TableEntry] = &[");
    for fs in TABLE_RATES {
        println!("    TableEntry {{");
        println!("        fs: {fs:?},");
        println!("        family: [");
        for ultrasonic in [false, true] {
            let f = optimise_family(fs, ultrasonic);
            println!("            TableFamily {{");
            println!("                params: &[");
            for p in &f.params {
                println!("                    {p:?},");
            }
            println!("                ],");
            println!("                alpha: {:?},", f.alpha);
            println!("                mag_db: {:?},", f.mag_db);
            println!("                phase_deg: {:?},", f.phase_deg);
            println!("            }},");
        }
        println!("        ],");
        println!("    }},");
    }
    println!("];");
}

fn timing() {
    println!("rate      full[ms]  quick[ms] table[ms] | full mag/ph  quick mag/ph  table mag/ph (worst over 8 filters)");
    let mut rates = TABLE_RATES.to_vec();
    rates.extend([22_050.0, 32_000.0, 64_000.0, 128_000.0, 768_000.0]);
    for fs in rates {
        let t = Instant::now();
        let full = RiaaFilters::design_full(fs);
        let tf = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let quick = RiaaFilters::design_quick(fs);
        let tq = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let tab = RiaaFilters::design(fs);
        let tt = t.elapsed().as_secs_f64() * 1e3;
        let worst = |f: &RiaaFilters| {
            let (mut m, mut p) = (0.0f64, 0.0f64);
            for mode in [Mode::PreEmphasis, Mode::DeEmphasis] {
                for o in Options::ALL {
                    let r = f.report(mode, o);
                    m = m.max(r.max_mag_err_db);
                    p = p.max(r.max_phase_err_deg);
                }
            }
            format!("{m:.4}/{p:.3}")
        };
        println!(
            "{fs:<9} {tf:9.1} {tq:9.2} {tt:9.3} | {:>12} {:>13} {:>12} ({:?})",
            worst(&full),
            worst(&quick),
            worst(&tab),
            tab.source()
        );
    }
}
