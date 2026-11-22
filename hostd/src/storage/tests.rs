use super::*;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ah-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn database(&self) -> PathBuf {
        self.0.join("recording.sqlite")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn record(source: &str) -> RecordInput {
    RecordInput {
        source_id: source.into(),
        received_at_us: 1_800_000_000_123_456,
        payload: json!({
            "type":"ble_observation","address":null,"rssi":null,
            "peripheral_id":"platform-scoped-id","service_uuid":"12345678-1234-5678-9012-123456789abc",
            "manufacturer_bytes":[0,255,128],"capture":{"ticks":4294967295u64,"clock":"source_boot_ms"},
            "replay":true,"detection":{"rules":["fixture@1"],"policy":"silent","alert":"suppressed"}
        }),
    }
}
fn setup(f: &Fixture) -> (Store, String) {
    let mut store = Store::open(&f.database()).unwrap();
    let run = store
        .start(RecordingMode::Replay, &json!({"fixture":true}))
        .unwrap();
    store
        .add_source(
            &run,
            "receiver-a",
            &json!({"physical_identity":null,"gateway":"fixture"}),
        )
        .unwrap();
    (store, run)
}
#[test]
fn round_trip_preserves_unknowns_binary_fields_evidence_and_finish() {
    let f = Fixture::new();
    let (mut store, run) = setup(&f);
    let input = record("receiver-a");
    store.append(&run, std::slice::from_ref(&input)).unwrap();
    store.transition(&run, "paused", "operator pause").unwrap();
    store
        .transition(&run, "recording", "operator resume")
        .unwrap();
    store
        .transition(&run, "finished", "receiver stopped and records committed")
        .unwrap();
    assert!(store.append(&run, std::slice::from_ref(&input)).is_err());
    assert!(store
        .transition(&run, "recording", "cannot restart finished run")
        .is_err());
    let reader = Reader::open(&f.database()).unwrap();
    let runs = reader.runs(10).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].record_count, 1);
    assert_eq!(runs[0].state, "finished");
    assert!(runs[0].ended_at_us.is_some());
    let page = reader.records(&run, 0, 10).unwrap();
    assert_eq!(page.records[0].payload, input.payload);
    assert!(!page.has_more);
    assert_eq!(page.records[0].received_at_us, input.received_at_us);
    drop(reader);
    drop(store);
    let _reopened = Store::open(&f.database()).unwrap();
    assert_eq!(
        Reader::open(&f.database()).unwrap().runs(1).unwrap()[0].state,
        "finished"
    );
}
#[test]
fn writer_ownership_and_recovery_are_separate_from_observers() {
    let f = Fixture::new();
    let (mut store, run) = setup(&f);
    store.append(&run, &[record("receiver-a")]).unwrap();
    assert!(matches!(Store::open(&f.database()),Err(e) if e.kind()==io::ErrorKind::WouldBlock));
    let reader = Reader::open(&f.database()).unwrap();
    assert_eq!(reader.runs(1).unwrap()[0].state, "recording");
    drop(store);
    let _recovered = Store::open(&f.database()).unwrap();
    let runs = reader.runs(1).unwrap();
    assert_eq!(runs[0].state, "interrupted");
    assert_eq!(runs[0].record_count, 1);
}
#[test]
fn batches_rollback_all_rows_and_counts_on_validation_or_foreign_key_failure() {
    let f = Fixture::new();
    let (mut store, run) = setup(&f);
    for bad in [
        record("missing-source"),
        RecordInput {
            received_at_us: -1,
            ..record("receiver-a")
        },
        RecordInput {
            payload: json!({"type":"x","oversized":"x".repeat(MAX_RECORD_BYTES)}),
            ..record("receiver-a")
        },
    ] {
        assert!(store.append(&run, &[record("receiver-a"), bad]).is_err());
        let reader = Reader::open(&f.database()).unwrap();
        assert_eq!(reader.runs(1).unwrap()[0].record_count, 0);
        assert!(reader.records(&run, 0, 10).unwrap().records.is_empty());
    }
    assert!(store
        .append(&run, &vec![record("receiver-a"); MAX_BATCH_RECORDS + 1])
        .is_err());
    assert!(store.add_source(&run, "", &json!({})).is_err());
}
#[test]
fn cursor_and_byte_bounds_do_not_skip_records() {
    let f = Fixture::new();
    let (mut store, run) = setup(&f);
    let large = RecordInput {
        payload: json!({"type":"fixture","bytes":"x".repeat(120_000)}),
        ..record("receiver-a")
    };
    store.append(&run, &vec![large; 5]).unwrap();
    let reader = Reader::open(&f.database()).unwrap();
    let first = reader.records(&run, 0, 500).unwrap();
    assert_eq!(first.records.len(), 4);
    assert!(first.has_more);
    let second = reader.records(&run, first.next_after, 500).unwrap();
    assert_eq!(second.records.len(), 1);
    assert!(!second.has_more);
    assert!(reader.records(&run, -1, 10).is_err());
    assert!(reader.records(&run, 0, 0).is_err());
    assert!(reader.records(&run, 0, 501).is_err());
    assert!(reader.runs(101).is_err());
}
#[test]
fn unrelated_future_and_nonregular_files_are_not_modified() {
    let f = Fixture::new();
    let path = f.database();
    assert!(Reader::open(&path).is_err());
    assert!(!path.exists());
    let conn = Connection::open(&path).unwrap();
    conn.execute("CREATE TABLE foreign_data(value TEXT)", [])
        .unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(Store::open(&f.0).is_err());
    let link = f.0.join("link.sqlite");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(Store::open(&link).is_err());
    let f = Fixture::new();
    let store = Store::open(&f.database()).unwrap();
    store
        .connection
        .pragma_update(None, "user_version", 99)
        .unwrap();
    drop(store);
    assert!(Store::open(&f.database()).is_err());
    assert!(Reader::open(&f.database()).is_err());
}
#[test]
fn migration_history_is_checked_and_failed_upgrade_rolls_back() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrations::apply(&mut conn, migrations::ALL).unwrap();
    let steps = [
        migrations::Migration {
            version: 1,
            name: "initial_recording",
            sql: include_str!("schema.sql"),
        },
        migrations::Migration {
            version: 2,
            name: "bad_upgrade",
            sql: "CREATE TABLE upgrade_probe(value TEXT); INSERT INTO nonexistent VALUES(1);",
        },
    ];
    assert!(migrations::apply(&mut conn, &steps).is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='upgrade_probe'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    migrations::apply(&mut conn, migrations::ALL).unwrap();
    conn.execute("UPDATE schema_migrations SET sha256=?1", ["0".repeat(64)])
        .unwrap();
    assert!(migrations::apply(&mut conn, migrations::ALL).is_err());
}
#[test]
fn strict_types_and_failed_disk_growth_leave_committed_state_consistent() {
    let f = Fixture::new();
    let (mut store, run) = setup(&f);
    assert!(store
        .connection
        .execute("UPDATE runs SET record_count='not-a-number'", [])
        .is_err());
    let pages: i64 = store
        .connection
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .unwrap();
    store
        .connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    let large = RecordInput {
        payload: json!({"type":"fixture","payload":"x".repeat(120_000)}),
        ..record("receiver-a")
    };
    assert!(store.append(&run, &[large]).is_err());
    assert_eq!(
        Reader::open(&f.database()).unwrap().runs(1).unwrap()[0].record_count,
        0
    );
    let integrity: String = store
        .connection
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
}
#[test]
fn wal_reader_snapshot_does_not_block_recording() {
    let f = Fixture::new();
    let (mut store, run) = setup(&f);
    let reader = Reader::open(&f.database()).unwrap();
    reader.connection.execute_batch("BEGIN").unwrap();
    assert_eq!(reader.runs(1).unwrap()[0].record_count, 0);
    store.append(&run, &[record("receiver-a")]).unwrap();
    assert_eq!(reader.runs(1).unwrap()[0].record_count, 0);
    reader.connection.execute_batch("COMMIT").unwrap();
    assert_eq!(reader.runs(1).unwrap()[0].record_count, 1);
}

#[test]
fn dedicated_writer_finish_commits_all_accepted_records() {
    let f = Fixture::new();
    let writer = writer::Writer::start(
        f.database(),
        RecordingMode::Replay,
        json!({"fixture":true}),
        vec![("receiver-a".into(), json!({"physical_identity":null}))],
    )
    .unwrap();
    let run = writer.run_id.clone();
    for _ in 0..64 {
        writer.append(record("receiver-a")).unwrap();
    }
    writer.finish(true).unwrap();
    let reader = Reader::open(&f.database()).unwrap();
    let runs = reader.runs(1).unwrap();
    assert_eq!(runs[0].id, run);
    assert_eq!(runs[0].state, "finished");
    assert_eq!(runs[0].record_count, 64);
    assert_eq!(reader.records(&run, 0, 100).unwrap().records.len(), 64);
}

#[test]
fn dedicated_writer_never_finishes_after_rejection_or_database_error() {
    for source in ["", "unregistered"] {
        let f = Fixture::new();
        let writer = writer::Writer::start(
            f.database(),
            RecordingMode::Replay,
            json!({}),
            vec![("receiver-a".into(), json!({}))],
        )
        .unwrap();
        // Invalid identifiers fail at enqueue; unknown source IDs fail at commit.
        let _ = writer.append(record(source));
        assert!(writer.finish(true).is_err());
        let reader = Reader::open(&f.database()).unwrap();
        let runs = reader.runs(1).unwrap();
        assert_eq!(runs[0].state, "failed");
        assert_eq!(runs[0].record_count, 0);
    }
}

#[test]
fn dedicated_writer_capture_failure_retains_committed_evidence() {
    let f = Fixture::new();
    let writer = writer::Writer::start(
        f.database(),
        RecordingMode::Replay,
        json!({}),
        vec![("receiver-a".into(), json!({}))],
    )
    .unwrap();
    writer.append(record("receiver-a")).unwrap();
    writer.finish(false).unwrap();
    let reader = Reader::open(&f.database()).unwrap();
    let runs = reader.runs(1).unwrap();
    assert_eq!(runs[0].state, "failed");
    assert_eq!(runs[0].record_count, 1);
}

#[test]
fn recording_output_preserves_fragmented_source_and_evidence_records() {
    use std::io::Write;
    let f = Fixture::new();
    let writer = writer::Writer::start(
        f.database(),
        RecordingMode::Replay,
        json!({}),
        vec![
            ("receiver-a".into(), json!({})),
            (
                output::SESSION_SOURCE.into(),
                json!({"kind":"session_control"}),
            ),
        ],
    )
    .unwrap();
    let run = writer.run_id.clone();
    let (mut output, completion) = output::attach(Vec::new(), writer);
    let mut payload = record("receiver-a").payload;
    payload["session_source"] =
        json!({"receiver_id":"receiver-a","session_id":"original","ingress_sequence":42});
    let bytes = serde_json::to_vec(&payload).unwrap();
    for chunk in bytes.chunks(7) {
        output.write_all(chunk).unwrap();
    }
    output.write_all(b"\n").unwrap();
    output.flush().unwrap();
    drop(output);
    completion.finish(true).unwrap();
    let reader = Reader::open(&f.database()).unwrap();
    let page = reader.records(&run, 0, 10).unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].source_id, "receiver-a");
    assert_eq!(page.records[0].payload, payload);
}

#[test]
fn incomplete_output_cannot_finish_successfully() {
    use std::io::Write;
    let f = Fixture::new();
    let writer = writer::Writer::start(
        f.database(),
        RecordingMode::Replay,
        json!({}),
        vec![(output::SESSION_SOURCE.into(), json!({}))],
    )
    .unwrap();
    let (mut output, completion) = output::attach(Vec::new(), writer);
    output.write_all(b"{\"type\":\"observation\"}").unwrap();
    drop(output);
    assert!(completion.finish(true).is_err());
    let reader = Reader::open(&f.database()).unwrap();
    assert_eq!(reader.runs(1).unwrap()[0].state, "failed");
}

#[test]
fn run_history_pages_by_bytes_and_cursor_without_skipping() {
    let f = Fixture::new();
    let mut store = Store::open(&f.database()).unwrap();
    let mut expected = std::collections::BTreeSet::new();
    for _ in 0..12 {
        let run = store
            .start(
                RecordingMode::Replay,
                &json!({"padding":"x".repeat(60_000)}),
            )
            .unwrap();
        store.transition(&run, "finished", "fixture").unwrap();
        expected.insert(run);
    }
    let reader = Reader::open(&f.database()).unwrap();
    let first = reader.run_page(None, 100).unwrap();
    assert!(first.has_more);
    assert!(serde_json::to_vec(&first).unwrap().len() < 1024 * 1024);
    let second = reader.run_page(first.next_before.as_deref(), 100).unwrap();
    assert!(!second.has_more);
    let ids: Vec<_> = first
        .runs
        .iter()
        .chain(&second.runs)
        .map(|run| run.id.clone())
        .collect();
    assert_eq!(ids.len(), 12);
    assert_eq!(
        ids.into_iter().collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert!(reader.run_page(Some("bad cursor"), 10).is_err());
    assert!(reader
        .run_page(Some("00000000000000000000000000000000"), 10)
        .is_err());
}

#[test]
fn start_request_identity_commits_atomically_and_survives_restart() {
    let f = Fixture::new();
    let request = json!({"version":1,"receivers":[]});
    let mut store = Store::open(&f.database()).unwrap();
    let run = store
        .start_request(
            RecordingMode::Replay,
            &json!({}),
            Some(("retry-key", &request)),
        )
        .unwrap();
    assert!(store
        .start_request(
            RecordingMode::Replay,
            &json!({}),
            Some(("retry-key", &request))
        )
        .is_err());
    drop(store);
    let _store = Store::open(&f.database()).unwrap();
    let reader = Reader::open(&f.database()).unwrap();
    let receipt = reader
        .start_receipt("retry-key", &request)
        .unwrap()
        .unwrap();
    assert_eq!(receipt["run_id"], run);
    assert_eq!(receipt["state"], "interrupted");
    assert_eq!(reader.runs(10).unwrap().len(), 1);
    assert!(reader
        .start_receipt("retry-key", &json!({"different":true}))
        .is_err());
    assert!(reader.start_receipt("unknown", &request).unwrap().is_none());
    assert!(reader.start_receipt("", &request).is_err());
}
