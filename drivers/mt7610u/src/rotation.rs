//! Private bounded observation recordings. Every segment is independently
//! replayable: a fresh session identifier, sequence zero, and complete NDJSON.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
};

pub(super) struct Rotation {
    directory: PathBuf,
    files: VecDeque<PathBuf>,
    writer: Option<BufWriter<File>>,
    prefix: String,
    index: u64,
    bytes: u64,
    sequence: u64,
    max_bytes: u64,
    keep: usize,
    source_session: Option<String>,
}
impl Rotation {
    pub fn new(directory: PathBuf, max_bytes: u64, keep: usize) -> Result<Self> {
        Self::with_clock(
            directory,
            max_bytes,
            keep,
            &mt7610u_lab::timing::Clock::default(),
        )
    }
    fn with_clock(
        directory: PathBuf,
        max_bytes: u64,
        keep: usize,
        clock: &mt7610u_lab::timing::Clock,
    ) -> Result<Self> {
        ensure!(
            (16 * 1024..=64 * 1024 * 1024).contains(&max_bytes),
            "rotation size must be 16 KiB..64 MiB"
        );
        ensure!(
            (1..=128).contains(&keep),
            "rotation retention must be 1..128 files"
        );
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .context("recording directory must be new (never reuse or overwrite)")?;
        Ok(Self {
            directory,
            files: VecDeque::new(),
            writer: None,
            prefix: format!("{}-{}", std::process::id(), clock.unix_time()?.as_nanos()),
            index: 0,
            bytes: 0,
            sequence: 0,
            max_bytes,
            keep,
            source_session: None,
        })
    }
    fn next(&mut self) -> Result<()> {
        self.finish()?;
        self.writer = None;
        if self.files.len() == self.keep {
            fs::remove_file(self.files.front().unwrap())?;
            self.files.pop_front();
        }
        let path = self
            .directory
            .join(format!("observations-{:06}.ndjson", self.index));
        self.writer = Some(crate::recording::new_private_file(path.clone())?);
        self.files.push_back(path);
        self.index += 1;
        self.bytes = 0;
        self.sequence = 0;
        Ok(())
    }
    pub fn observation(&mut self, bytes: &[u8]) -> Result<()> {
        let mut value: Value = serde_json::from_slice(bytes)?;
        let source_session = value["session_id"]
            .as_str()
            .context("observation missing session identifier")?
            .to_owned();
        if self.writer.is_none() || self.source_session.as_ref() != Some(&source_session) {
            self.next()?;
            self.source_session = Some(source_session);
        }
        loop {
            value["session_id"] = json!(format!("file-{}-{}", self.prefix, self.index));
            value["sequence"] = json!(self.sequence);
            let mut line = serde_json::to_vec(&value)?;
            line.push(b'\n');
            ensure!(
                line.len() <= 16 * 1024,
                "rotated observation exceeds protocol line limit"
            );
            if self.bytes + line.len() as u64 > self.max_bytes {
                self.next()?;
                continue;
            }
            self.writer.as_mut().unwrap().write_all(&line)?;
            self.bytes += line.len() as u64;
            self.sequence += 1;
            return Ok(());
        }
    }
    pub fn finish(&mut self) -> Result<()> {
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn rotates_private_complete_independent_sessions_with_bounded_retention() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mt7610-rotation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let clock = crate::test_support::ManualTime::default().clock();
        let mut recording = Rotation::with_clock(path.clone(), 16 * 1024, 2, &clock).unwrap();
        let line=serde_json::to_vec(&json!({"schema_version":1,"type":"beacon","session_id":"wire-session","sequence":55,"padding":"x".repeat(9000)})).unwrap();
        for _ in 0..5 {
            recording.observation(&line).unwrap();
        }
        recording.finish().unwrap();
        let files: Vec<_> = fs::read_dir(&path)
            .unwrap()
            .map(|f| f.unwrap().path())
            .collect();
        assert_eq!(files.len(), 2);
        let mut sessions = Vec::new();
        for file in &files {
            let bytes = fs::read(file).unwrap();
            assert!(bytes.ends_with(b"\n"));
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["sequence"], 0);
            sessions.push(v["session_id"].clone());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(file).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        assert_ne!(sessions[0], sessions[1]);
        assert!(Rotation::with_clock(path.clone(), 16 * 1024, 2, &clock).is_err());
        drop(recording);
        fs::remove_dir_all(path).unwrap();
    }
}
