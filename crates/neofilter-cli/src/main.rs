//! NeoFilter command line tool (response export, IR comparison). Placeholder.

use clap::Parser;

/// NeoFilter command line tool.
#[derive(Parser)]
#[command(version, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
    println!("neofilter-cli: not implemented yet");
}
