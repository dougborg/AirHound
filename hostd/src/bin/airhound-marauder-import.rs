//! Offline only: never opens a serial port or starts a radio scan.
use airhound_hostd::marauder::{self, MAX_CAPTURE_BYTES};
use clap::Parser;
use std::io::{self, Read, Write};

#[derive(Parser)]
#[command(about = "Import a saved Marauder v1.21.0 WiFi PCAP or binary serial capture as NDJSON")]
struct Args {
    /// Saved binary capture (not a terminal's text transcript)
    input: std::path::PathBuf,
    /// Decode [BUF/BEGIN]/[BUF/CLOSE] serial wrapping and intervening console text
    #[arg(long)]
    serial: bool,
    /// User-assigned sensor/chip identity; USB names alone may collide
    #[arg(long)]
    source_id: String,
}
fn main() -> io::Result<()> {
    let args = Args::parse();
    if args.source_id.trim().is_empty() || args.source_id.len() > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source-id must be 1–128 bytes",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(args.input)?
        .take(MAX_CAPTURE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let index = airhound::mac_index::MacIndex::from_defaults();
    let mut out = io::BufWriter::new(io::stdout().lock());
    let count = marauder::decode(&bytes, args.serial, |record| {
        serde_json::to_writer(
            &mut out,
            &marauder::observation(record, &args.source_id, &index),
        )?;
        out.write_all(b"\n")
    })?;
    out.flush()?;
    eprintln!(
        "Imported {count} packets; capture loss, RSSI, receive channel and wall time unavailable."
    );
    Ok(())
}
