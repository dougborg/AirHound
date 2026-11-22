//! Explicit local installation tool; never opens a capture device.
use clap::{Parser, Subcommand};
use std::{io, path::PathBuf};
#[derive(Parser)]
struct Args {
    #[arg(long)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    Status,
    Verify,
    Install {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        replace: bool,
    },
}
fn main() -> io::Result<()> {
    let args = Args::parse();
    let root = args
        .root
        .map(Ok)
        .unwrap_or_else(airhound_hostd::managed_helpers::default_root)?;
    let result = match args.action {
        Action::Status => airhound_hostd::managed_helpers::status(&root),
        Action::Verify => airhound_hostd::managed_helpers::verify(&root)?,
        Action::Install { bundle, replace } => {
            airhound_hostd::managed_helpers::install(&root, &bundle, replace)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
