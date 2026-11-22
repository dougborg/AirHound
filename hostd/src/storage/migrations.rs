//! Forward-only migrations. Published migration SQL is immutable.
use super::{invalid, now_us, sql, APPLICATION_ID};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::io;

pub(super) struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}
pub(super) const ALL: &[Migration] = &[Migration {
    version: 1,
    name: "initial_recording",
    sql: include_str!("schema.sql"),
}];
fn checksum(script: &str) -> String {
    Sha256::digest(script.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn apply(connection: &mut Connection, migrations: &[Migration]) -> io::Result<()> {
    for (index, migration) in migrations.iter().enumerate() {
        if migration.version != index as i64 + 1 {
            return Err(invalid("migration versions must be contiguous"));
        }
    }
    let app: i64 = connection
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(sql)?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(sql)?;
    if version < 0 || version > migrations.len() as i64 {
        return Err(invalid("database schema is newer than this application"));
    }
    if app == 0 && version == 0 {
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if tables != 0 {
            return Err(invalid("database is not an AirHound recording"));
        }
    } else if app != APPLICATION_ID || version == 0 {
        return Err(invalid("unsupported database identity or schema version"));
    } else {
        let count: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .map_err(sql)?;
        if count != version {
            return Err(invalid("inconsistent migration history"));
        }
        for migration in migrations.iter().take(version as usize) {
            let (name, hash): (String, String) = connection
                .query_row(
                    "SELECT name,sha256 FROM schema_migrations WHERE version=?1",
                    [migration.version],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(sql)?;
            if name != migration.name || hash != checksum(migration.sql) {
                return Err(invalid("migration history does not match this application"));
            }
        }
    }
    // Validate identity/history before changing database settings or schema.
    connection
        .busy_timeout(std::time::Duration::from_secs(2))
        .map_err(sql)?;
    connection
        .execute_batch("PRAGMA foreign_keys=ON")
        .map_err(sql)?;
    for migration in migrations.iter().skip(version as usize) {
        let tx = connection.transaction().map_err(sql)?;
        tx.execute_batch(migration.sql).map_err(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations(version,name,sha256,applied_at_us) VALUES(?1,?2,?3,?4)",
            params![
                migration.version,
                migration.name,
                checksum(migration.sql),
                now_us()?
            ],
        )
        .map_err(sql)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(sql)?;
        tx.pragma_update(None, "user_version", migration.version)
            .map_err(sql)?;
        let violations: i64 = tx
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .map_err(sql)?;
        if violations != 0 {
            return Err(invalid("migration violates foreign keys"));
        }
        tx.commit().map_err(sql)?;
    }
    Ok(())
}
