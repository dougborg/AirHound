//! Informational read-only benchmark of a user-supplied consistent SQLite snapshot.
//! Does not connect to a service, open a writer, or acquire receivers.
use airhound_hostd::storage::Reader;
use clap::Parser;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{
    fs, io,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Parser)]
struct Args {
    /// Existing consistent snapshot, never a default/live database path.
    #[arg(long)]
    snapshot: PathBuf,
    /// New private output directory; existing paths are refused.
    #[arg(long)]
    output: PathBuf,
    /// Stored run ID; defaults to the newest stored run.
    #[arg(long)]
    run: Option<String>,
    /// Explicitly export raw pages for the separate presentation benchmark.
    #[arg(long)]
    export_pages: bool,
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if !fs::symlink_metadata(&args.snapshot)?.is_file() {
        return Err("snapshot must be a regular file, not a symlink".into());
    }
    let reader = Reader::open(&args.snapshot)?;
    let connection = Connection::open_with_flags(
        &args.snapshot,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA query_only=ON")?;
    let run = match args.run {
        Some(value) => value,
        None => connection.query_row(
            "SELECT id FROM runs ORDER BY started_at_us DESC,id DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )?,
    };
    let (low, high): (Option<i64>, Option<i64>) = connection.query_row(
        "SELECT min(id),max(id) FROM records WHERE run_id=?1",
        [&run],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (low, high) = low.zip(high).ok_or("selected run has no records")?;
    // Three bounded regions, eight pages each, three measured repetitions.
    let starts = [
        0,
        low + (high - low) / 2,
        low.max(high.saturating_sub(8000)),
    ];
    fs::DirBuilder::new().mode(0o700).create(&args.output)?;
    if args.export_pages {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(args.output.join("pages"))?;
    }
    let mut samples = Vec::new();
    let mut unique_rows = 0;
    let mut max_page_bytes = 0;
    for (region, start) in starts.into_iter().enumerate() {
        let mut original = Vec::new();
        for repetition in 0..3 {
            let mut after = start;
            for index in 0..8 {
                let began = Instant::now();
                let page = reader.records(&run, after, 500)?;
                let reader_us = began.elapsed().as_micros();
                let began = Instant::now();
                let bytes = serde_json::to_vec(&page)?;
                let serialization_us = began.elapsed().as_micros();
                // Data/ordering assertions are independent of all measured durations.
                assert!(bytes.len() <= 512 * 1024);
                assert!(page
                    .records
                    .windows(2)
                    .all(|rows| rows[0].cursor < rows[1].cursor));
                assert!(page.records.first().is_none_or(|row| row.cursor > after));
                assert!(page
                    .records
                    .last()
                    .is_none_or(|row| row.cursor == page.next_after));
                if repetition == 0 {
                    if args.export_pages {
                        write_new(
                            &args.output.join(format!("pages/{region}-{index:02}.json")),
                            &bytes,
                        )?;
                    }
                    unique_rows += page.records.len();
                    original.push(bytes.clone());
                } else {
                    assert!(
                        original[index] == bytes,
                        "snapshot page changed during benchmark"
                    );
                }
                max_page_bytes = max_page_bytes.max(bytes.len());
                samples.push(json!({"region":region,"repetition":repetition,"page":index,"rows":page.records.len(),"bytes":bytes.len(),"reader_us":reader_us,"serialization_us":serialization_us}));
                after = page.next_after;
                if !page.has_more {
                    break;
                }
            }
        }
    }
    write_new(
        &args.output.join("reader-results.json"),
        &serde_json::to_vec_pretty(&json!({
            "build_profile":if cfg!(debug_assertions) { "debug" } else { "release" },
            "backend_source":airhound_hostd::build_info::current()["git_revision"],
            "timing_role":"informational; no elapsed-time correctness gate",
            "cache_scope":"OS cache is uncontrolled; repeated measurements warm it",
            "reader_scope":"SQLite selection, JSON parse, metadata projection and internal byte-limit serialization; excludes response serialization",
            "source_bytes":fs::metadata(&args.snapshot)?.len(),
            "selected_run_state":connection.query_row("SELECT state FROM runs WHERE id=?1", [&run], |row| row.get::<_,String>(0))?,
            "sampled_rows_including_possible_region_overlap":unique_rows,"max_page_bytes":max_page_bytes,
            "stable_repeat_bytes":true,"raw_pages_exported":args.export_pages,"measurements":samples
        }))?,
    )?;
    Ok(())
}
