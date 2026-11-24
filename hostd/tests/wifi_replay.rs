#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod support;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "airhound-wifi-replay-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn capture() -> Vec<u8> {
    let mut bytes = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    bytes.extend(65536u32.to_le_bytes());
    bytes.extend(127u32.to_le_bytes());
    for frequency in [2412u16, 5955, 5956] {
        let mut packet = vec![0, 0, 13, 0, 0x28, 0, 0, 0];
        packet.extend(frequency.to_le_bytes());
        packet.extend([0x40, 1, (-40i8) as u8]);
        let mut frame = vec![0u8; 36];
        frame[0] = 0x80;
        frame[4..10].fill(255);
        frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame.extend([0, 12]);
        frame.extend(b"Flock-123456");
        packet.extend(frame);
        for number in [
            1_700_000_000u32,
            123_456,
            packet.len() as u32,
            packet.len() as u32,
        ] {
            bytes.extend(number.to_le_bytes());
        }
        bytes.extend(packet);
    }
    bytes
}
fn run(args: &[&std::ffi::OsStr]) -> std::process::Output {
    support::output(
        Command::new(env!("CARGO_BIN_EXE_airhound-hostd"))
            .args(args)
            .stdin(Stdio::null()),
    )
    .unwrap()
}
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn standalone_and_session_replay_preserve_six_ghz_time_source_and_csv() {
    for session in [false, true] {
        let fixture = Fixture::new();
        let pcap = fixture.path("capture.pcap");
        let csv = fixture.path("out.csv");
        let database = fixture.path("recording.sqlite");
        std::fs::write(&pcap, capture()).unwrap();
        let config = fixture.path("session.json");
        std::fs::write(&config,serde_json::to_vec(&serde_json::json!({"version":1,"alert_policy":"silent","receivers":[{"id":"recorded-wifi","source":{"kind":"wifi_pcap","path":pcap}}]})).unwrap()).unwrap();
        let args = if session {
            vec![std::ffi::OsStr::new("--session-config"), config.as_os_str()]
        } else {
            vec![std::ffi::OsStr::new("--wifi-pcap"), pcap.as_os_str()]
        };
        let mut args = args;
        if !session {
            args.extend([
                std::ffi::OsStr::new("--alert-policy"),
                std::ffi::OsStr::new("silent"),
            ]);
        }
        args.extend([std::ffi::OsStr::new("--output"), csv.as_os_str()]);
        args.extend([std::ffi::OsStr::new("--database"), database.as_os_str()]);
        let output = run(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let reader = airhound_hostd::storage::Reader::open(&database).unwrap();
        let runs = reader.runs(1).unwrap();
        assert_eq!(runs[0].state, "finished");
        assert_eq!(runs[0].mode, "replay");
        assert_eq!(runs[0].record_count as usize, records.len());
        let stored = reader.records(&runs[0].id, 0, 100).unwrap();
        assert!(!stored.has_more);
        assert_eq!(
            stored
                .records
                .iter()
                .map(|record| &record.payload)
                .collect::<Vec<_>>(),
            records.iter().collect::<Vec<_>>()
        );
        let rows: Vec<_> = records
            .iter()
            .filter(|row| row["type"] == "wifi_observation")
            .collect();
        assert_eq!(rows.len(), 2);
        for (row, frequency, band) in [(rows[0], 2412, "2.4GHz"), (rows[1], 5955, "6GHz")] {
            assert_eq!(row["detection"]["policy"], "silent");
            assert_eq!(row["detection"]["alert"], "suppressed");
            assert_eq!(row["ch"], 1);
            assert_eq!(row["frequency_mhz"], frequency);
            assert_eq!(row["band"], band);
            assert_eq!(row["replay"], true);
            assert_eq!(row["capture"]["captured_at_us"], 1_700_000_000_123_456u64);
            assert_eq!(
                row["session_source"]["receiver_id"],
                if session {
                    "recorded-wifi"
                } else {
                    "wifi-pcap"
                }
            );
        }
        assert_eq!(rows[1]["capture"]["record_sequence"], 2);
        assert!(records
            .iter()
            .any(|row| row["type"] == "session_started" && row["alert_policy"] == "silent"));
        let evidence: Vec<_> = records
            .iter()
            .filter(|row| row["type"] == "detection")
            .collect();
        assert_eq!(evidence.len(), 2);
        assert!(evidence.iter().all(|row| row["replay"] == true));
        let status = records
            .iter()
            .find(|row| row["type"] == "wifi_replay_status")
            .unwrap();
        assert_eq!(status["state"], "finished");
        assert_eq!(status["records"], 3);
        assert_eq!(status["rejected_records"], 1);
        let csv = std::fs::read_to_string(csv).unwrap();
        let body = csv.lines().skip(1).collect::<Vec<_>>().join("\n");
        let mut reader = csv::Reader::from_reader(body.as_bytes());
        let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(&rows[0][5], "2412");
        assert_eq!(&rows[1][5], "5955");
        for row in rows {
            assert_eq!(&row[3], "2023-11-14 22:13:20");
            for column in 7..=10 {
                assert_eq!(&row[column], "");
            }
        }
        assert!(!String::from_utf8_lossy(&output.stderr).contains("BLE receiver:"));
    }
}
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn replay_rejects_bad_files_and_live_source_conflicts() {
    let fixture = Fixture::new();
    let pcap = fixture.path("bad.pcap");
    let csv = fixture.path("bad.csv");
    std::fs::write(&pcap, b"not a capture").unwrap();
    let output = run(&[
        "--wifi-pcap".as_ref(),
        pcap.as_os_str(),
        "--output".as_ref(),
        csv.as_os_str(),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("wifi_replay_status"));
    for args in [
        vec!["--wifi-pcap", "x", "--no-wifi"],
        vec!["--wifi-pcap", "x", "--ble-adapter", "hci0"],
        vec!["--wifi-pcap", "x", "--gpsd", "127.0.0.1:2947"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_airhound-hostd"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}

#[test]
#[cfg(unix)]
#[cfg_attr(coverage_nightly, coverage(off))]
fn replay_rejects_directories_and_fifos_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let fifo = fixture.path("capture.fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    for (index, input) in [&fixture.0, &fifo].into_iter().enumerate() {
        let csv = fixture.path(&format!("nonregular-{index}.csv"));
        let output = run(&[
            "--wifi-pcap".as_ref(),
            input.as_os_str(),
            "--output".as_ref(),
            csv.as_os_str(),
        ]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("regular saved PCAP file"));
    }
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn two_replay_sources_record_without_merging_identity_or_evidence() {
    let fixture = Fixture::new();
    let a = fixture.path("a.pcap");
    let b = fixture.path("b.pcap");
    std::fs::write(&a, capture()).unwrap();
    std::fs::write(&b, capture()).unwrap();
    let config = fixture.path("session.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "version":1,"alert_policy":"silent","receivers":[
                {"id":"a","source":{"kind":"wifi_pcap","path":a}},
                {"id":"b","source":{"kind":"wifi_pcap","path":b}}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let database = fixture.path("recording.sqlite");
    let csv = fixture.path("out.csv");
    let output = run(&[
        "--session-config".as_ref(),
        config.as_os_str(),
        "--database".as_ref(),
        database.as_os_str(),
        "--output".as_ref(),
        csv.as_os_str(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let reader = airhound_hostd::storage::Reader::open(&database).unwrap();
    let runs = reader.runs(1).unwrap();
    assert_eq!(runs[0].state, "finished");
    let stored = reader.records(&runs[0].id, 0, 100).unwrap();
    assert_eq!(
        stored
            .records
            .iter()
            .map(|r| &r.payload)
            .collect::<Vec<_>>(),
        records.iter().collect::<Vec<_>>()
    );
    for source in ["a", "b"] {
        for kind in ["wifi_observation", "detection"] {
            let matching: Vec<_> = stored
                .records
                .iter()
                .filter(|r| r.source_id == source && r.kind == kind)
                .collect();
            assert_eq!(matching.len(), 2);
            assert!(matching
                .iter()
                .all(|r| r.payload["session_source"]["receiver_id"] == source));
        }
    }
}
