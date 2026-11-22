//! Durable desktop records. Call these blocking operations on a dedicated writer.
//! Wire payloads remain intact; no identity, timestamp or RF metadata is inferred.
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const APPLICATION_ID: i64 = 1_095_257_668;
const SCHEMA_VERSION: i64 = 1;
mod migrations;
pub mod writer;
pub const MAX_RECORD_BYTES: usize = 128 * 1024;
pub const MAX_BATCH_RECORDS: usize = 128;
pub const MAX_PAGE_RECORDS: usize = 500;
const MAX_PAGE_BYTES: usize = 512 * 1024;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn sql(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}
pub fn now_us() -> io::Result<i64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_micros();
    i64::try_from(value).map_err(io::Error::other)
}
fn json(value: &Value, limit: usize) -> io::Result<String> {
    let encoded = serde_json::to_string(value).map_err(io::Error::other)?;
    if encoded.len() > limit {
        return Err(invalid("storage JSON exceeds size limit"));
    }
    Ok(encoded)
}
fn identifier(value: &str) -> io::Result<()> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(invalid(
            "source identifier must contain 1–128 non-control bytes",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    Live,
    Replay,
    Import,
}
impl RecordingMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Replay => "replay",
            Self::Import => "import",
        }
    }
}
#[derive(Clone, Debug)]
pub struct RecordInput {
    pub source_id: String,
    pub received_at_us: i64,
    /// Exact structured record, including original clock and replay/evidence fields.
    pub payload: Value,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredRecord {
    pub cursor: i64,
    pub source_id: String,
    pub kind: String,
    pub received_at_us: i64,
    pub payload: Value,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordPage {
    pub records: Vec<StoredRecord>,
    pub next_after: i64,
    pub has_more: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub state: String,
    pub mode: String,
    pub started_at_us: i64,
    pub ended_at_us: Option<i64>,
    pub record_count: i64,
    pub config: Value,
}

/// One writer owner per database. The advisory lock outlives the SQLite connection.
/// Readers use `Reader`; opening a second writer must never interrupt a live run.
pub struct Store {
    connection: Connection,
    _owner: File,
}
impl Store {
    pub fn open(path: &Path) -> io::Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| invalid("database requires a filename"))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let path = parent.canonicalize()?.join(name);
        let mut lock_name = name.to_os_string();
        lock_name.push(".airhound-lock");
        let lock_path = path.with_file_name(lock_name);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let owner = options.open(lock_path)?;
        if !owner.metadata()?.is_file() {
            return Err(invalid("database ownership lock must be a regular file"));
        }
        // SAFETY: the descriptor is live; the nonblocking lock belongs to this handle.
        if unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "database already has a writer",
            ));
        }
        // Keep our ownership lock off the database inode: on macOS flock can
        // conflict with SQLite's locks. Never unlink this sidecar while in use.
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(invalid(
                "database must be a regular file without hard links",
            ));
        }
        // Close the validation descriptor before SQLite acquires its own locks.
        drop(file);
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        migrations::apply(&mut connection, migrations::ALL)?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(sql)?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
            )
            .map_err(sql)?;
        let at = now_us()?;
        let tx = connection.transaction().map_err(sql)?;
        tx.execute("INSERT INTO transitions(run_id,state,at_us,reason) SELECT id,'interrupted',?1,'writer restarted before durable finish' FROM runs WHERE state IN ('recording','paused')",[at]).map_err(sql)?;
        tx.execute("UPDATE runs SET state='interrupted',ended_at_us=?1 WHERE state IN ('recording','paused')",[at]).map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(Self {
            connection,
            _owner: owner,
        })
    }
    pub fn start(&mut self, mode: RecordingMode, config: &Value) -> io::Result<String> {
        let config = json(config, 65536)?;
        let at = now_us()?;
        let tx = self.connection.transaction().map_err(sql)?;
        let id:String=tx.query_row("INSERT INTO runs(id,state,mode,started_at_us,config_json) VALUES(lower(hex(randomblob(16))),'recording',?1,?2,?3) RETURNING id",params![mode.as_str(),at,config],|r|r.get(0)).map_err(sql)?;
        tx.execute(
            "INSERT INTO transitions(run_id,state,at_us,reason) VALUES(?1,'recording',?2,'start')",
            params![id, at],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(id)
    }
    pub fn add_source(&mut self, run: &str, source: &str, metadata: &Value) -> io::Result<()> {
        identifier(source)?;
        let metadata = json(metadata, 65536)?;
        self.connection
            .execute(
                "INSERT INTO sources(run_id,source_id,metadata_json) VALUES(?1,?2,?3)",
                params![run, source, metadata],
            )
            .map_err(sql)?;
        Ok(())
    }
    /// All records and the committed count advance in the same transaction.
    pub fn append(&mut self, run: &str, records: &[RecordInput]) -> io::Result<()> {
        if records.len() > MAX_BATCH_RECORDS {
            return Err(invalid("storage batch exceeds record limit"));
        }
        let tx = self.connection.transaction().map_err(sql)?;
        let state: Option<String> = tx
            .query_row("SELECT state FROM runs WHERE id=?1", [run], |r| r.get(0))
            .optional()
            .map_err(sql)?;
        if !matches!(state.as_deref(), Some("recording" | "paused")) {
            return Err(invalid("run is not accepting records"));
        }
        for record in records {
            identifier(&record.source_id)?;
            let kind = record
                .payload
                .get("type")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty() && v.len() <= 128)
                .ok_or_else(|| invalid("record requires a bounded type"))?;
            let payload = json(&record.payload, MAX_RECORD_BYTES)?;
            tx.execute("INSERT INTO records(run_id,source_id,kind,received_at_us,payload_json) VALUES(?1,?2,?3,?4,?5)",params![run,record.source_id,kind,record.received_at_us,payload]).map_err(sql)?;
        }
        tx.execute(
            "UPDATE runs SET record_count=record_count+?2 WHERE id=?1",
            params![run, records.len() as i64],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }
    pub fn transition(&mut self, run: &str, next: &str, reason: &str) -> io::Result<()> {
        if reason.len() > 2048 {
            return Err(invalid("transition reason exceeds limit"));
        }
        let at = now_us()?;
        let tx = self.connection.transaction().map_err(sql)?;
        let current: Option<String> = tx
            .query_row("SELECT state FROM runs WHERE id=?1", [run], |r| r.get(0))
            .optional()
            .map_err(sql)?;
        let allowed = matches!(
            (current.as_deref(), next),
            (Some("recording"), "paused")
                | (Some("paused"), "recording")
                | (Some("recording" | "paused"), "finished" | "failed")
        );
        if !allowed {
            return Err(invalid("invalid recording state transition"));
        }
        let ended = matches!(next, "finished" | "failed").then_some(at);
        tx.execute(
            "UPDATE runs SET state=?2,ended_at_us=?3 WHERE id=?1",
            params![run, next, ended],
        )
        .map_err(sql)?;
        tx.execute(
            "INSERT INTO transitions(run_id,state,at_us,reason) VALUES(?1,?2,?3,?4)",
            params![run, next, at, reason],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }
}
fn validate_version(app: i64, version: i64) -> io::Result<()> {
    if app != APPLICATION_ID || version != SCHEMA_VERSION {
        return Err(invalid("unsupported database identity or schema version"));
    }
    Ok(())
}

/// Read-only, bounded cursor queries; does not create a database or recover runs.
pub struct Reader {
    connection: Connection,
}
impl Reader {
    pub fn open(path: &Path) -> io::Result<Self> {
        if !std::fs::symlink_metadata(path)?.is_file() {
            return Err(invalid("database reader requires a regular file"));
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        validate_version(
            connection
                .pragma_query_value(None, "application_id", |r| r.get(0))
                .map_err(sql)?,
            connection
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(sql)?,
        )?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(sql)?;
        Ok(Self { connection })
    }
    pub fn runs(&self, limit: usize) -> io::Result<Vec<Run>> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("run query limit must be 1–100"));
        }
        let mut statement=self.connection.prepare("SELECT id,state,mode,started_at_us,ended_at_us,record_count,config_json FROM runs ORDER BY started_at_us DESC,id DESC LIMIT ?1").map_err(sql)?;
        let mut rows = statement.query([limit as i64]).map_err(sql)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(sql)? {
            let config: String = row.get(6).map_err(sql)?;
            result.push(Run {
                id: row.get(0).map_err(sql)?,
                state: row.get(1).map_err(sql)?,
                mode: row.get(2).map_err(sql)?,
                started_at_us: row.get(3).map_err(sql)?,
                ended_at_us: row.get(4).map_err(sql)?,
                record_count: row.get(5).map_err(sql)?,
                config: serde_json::from_str(&config).map_err(io::Error::other)?,
            });
        }
        Ok(result)
    }
    pub fn records(&self, run: &str, after: i64, limit: usize) -> io::Result<RecordPage> {
        if after < 0 || !(1..=MAX_PAGE_RECORDS).contains(&limit) {
            return Err(invalid("invalid record cursor or limit"));
        }
        let mut statement=self.connection.prepare("SELECT id,source_id,kind,received_at_us,payload_json FROM records WHERE run_id=?1 AND id>?2 ORDER BY id LIMIT ?3").map_err(sql)?;
        let mut rows = statement
            .query(params![run, after, (limit + 1) as i64])
            .map_err(sql)?;
        let mut page = RecordPage {
            records: Vec::new(),
            next_after: after,
            has_more: false,
        };
        let mut bytes = 0;
        while let Some(row) = rows.next().map_err(sql)? {
            let payload: String = row.get(4).map_err(sql)?;
            if page.records.len() == limit || bytes + payload.len() > MAX_PAGE_BYTES {
                page.has_more = true;
                break;
            }
            bytes += payload.len();
            let cursor = row.get(0).map_err(sql)?;
            page.records.push(StoredRecord {
                cursor,
                source_id: row.get(1).map_err(sql)?,
                kind: row.get(2).map_err(sql)?,
                received_at_us: row.get(3).map_err(sql)?,
                payload: serde_json::from_str(&payload).map_err(io::Error::other)?,
            });
            page.next_after = cursor;
        }
        Ok(page)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
