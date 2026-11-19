// SPDX-License-Identifier: MIT
//! Offline validation only: no scanner process, USB access, CSV, or location.
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

#[path = "../external_wifi.rs"]
mod external_wifi;
use clap::Parser;
use std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(about = "Validate a versioned external WiFi capture and print aggregate counts")]
struct Args {
    input: PathBuf,
}

fn main() -> io::Result<()> {
    let summary = external_wifi::validate(File::open(Args::parse().input)?)?;
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, &summary)?;
    output.write_all(b"\n")?;
    output.flush()
}
