//! Standalone consistent snapshots; never copy a live WAL database as raw files.
use super::{invalid, sql, Reader};
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_BYTES: i64 = 512 * 1024 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(10);
struct Temporary {
    path: PathBuf,
    file: File,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if let (Ok(current), Ok(owned)) =
            (std::fs::symlink_metadata(&self.path), self.file.metadata())
        {
            if current.dev() == owned.dev() && current.ino() == owned.ino() {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}
impl Reader {
    /// Blocking SQLite online backup, capped at 512 MiB and a ten-second copy /
    /// verification budget. The destination must not exist and its parent must.
    /// Published snapshots use DELETE journaling so no WAL sidecars are needed.
    pub fn export(&self, destination: &Path) -> io::Result<Value> {
        if !destination.is_absolute() {
            return Err(invalid("snapshot destination must be absolute"));
        }
        let name = destination
            .file_name()
            .ok_or_else(|| invalid("snapshot requires a filename"))?;
        let parent = destination
            .parent()
            .ok_or_else(|| invalid("snapshot requires a directory"))?
            .canonicalize()?;
        let destination = parent.join(name);
        match std::fs::symlink_metadata(&destination) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "snapshot destination already exists",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let deadline = Instant::now() + TIME_LIMIT;
        let page_size: i64 = self
            .connection
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(sql)?;
        let pages: i64 = self
            .connection
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(sql)?;
        if page_size <= 0 || pages < 0 || pages.saturating_mul(page_size) > MAX_BYTES {
            return Err(invalid("snapshot exceeds 512 MiB limit"));
        }
        let nonce: String = self
            .connection
            .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
            .map_err(sql)?;
        let path = parent.join(format!(".airhound-snapshot-{nonce}.sqlite"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let temporary = Temporary { path, file };
        let mut target = Connection::open_with_flags(
            &temporary.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        target
            .progress_handler(1000, Some(move || Instant::now() >= deadline))
            .map_err(sql)?;
        {
            let backup = Backup::new(&self.connection, &mut target).map_err(sql)?;
            loop {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "snapshot copy deadline exceeded",
                    ));
                }
                let state = backup.step(128).map_err(sql)?;
                if i64::from(backup.progress().pagecount).saturating_mul(page_size) > MAX_BYTES {
                    return Err(invalid("snapshot grew beyond 512 MiB limit"));
                }
                match state {
                    StepResult::Done => break,
                    StepResult::Busy | StepResult::Locked => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    _ => {}
                }
            }
        }
        let mode: String = target
            .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
            .map_err(sql)?;
        if mode != "delete" {
            return Err(io::Error::other("snapshot could not leave WAL mode"));
        }
        let integrity: String = target
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(sql)?;
        let violations: i64 = target
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .map_err(sql)?;
        if integrity != "ok" || violations != 0 {
            return Err(io::Error::other("snapshot integrity check failed"));
        }
        target.close().map_err(|(_, error)| sql(error))?;
        temporary.file.sync_all()?;
        let size = temporary.file.metadata()?.len();
        // Atomic no-clobber publication on the same filesystem. The temporary
        // inode remains owned until after the destination link is durable.
        std::fs::hard_link(&temporary.path, &destination)?;
        File::open(&parent)?.sync_all()?;
        Ok(
            json!({"path":destination,"bytes":size,"journal_mode":"delete","consistent_snapshot":true}),
        )
    }
}
