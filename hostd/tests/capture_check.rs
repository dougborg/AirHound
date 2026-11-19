#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg_attr(coverage_nightly, coverage(off))]
mod offline_cli {
    use serde_json::{json, Value};
    use std::{
        fs::OpenOptions,
        io::Write,
        path::PathBuf,
        process::{Command, Output},
    };

    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "airhound-capture-check-{}-{}-{}.ndjson",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.write_all(bytes).unwrap();
            Self(path)
        }
        fn run(&self) -> Output {
            Command::new(env!("CARGO_BIN_EXE_airhound-capture-check"))
                .arg(&self.0)
                .output()
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn beacon(sequence: u64) -> Value {
        json!({
            "schema_version":1,"type":"beacon","source":"mt7610u-lab",
            "session_id":"synthetic-private-session","sequence":sequence,
            "bssid":"02:12:34:56:78:9A","ssid_bytes":[255,0,128],
            "advertised_channel":36,"receive_channel":36,
            "elapsed_ms":sequence,"dwell_elapsed_ms":sequence,"channel_epoch":0,
            "observed_at_unix_ms":1700000000000u64 + sequence,
            "rssi_dbm":null,"estimated_rssi_dbm":-81,"rssi_validated":false,
            "rssi_estimate_basis":"mt76_eeprom_formula","raw_rssi":[187,0,0,0],
            "full_calibration":false
        })
    }

    #[test]
    fn valid_capture_prints_only_aggregate_counts_and_preserves_input() {
        let mut second = beacon(1);
        second["estimated_rssi_dbm"] = Value::Null;
        let bytes = format!("{}\n{}\n", beacon(0), second).into_bytes();
        let fixture = Fixture::new(&bytes);
        let output = fixture.run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            summary,
            json!({
                "schema_version":1,"records":2,"bytes":bytes.len(),
                "distinct_bssids":1,"distinct_receive_channels":1,
                "estimated_signal_records":1,"missing_signal_records":1
            })
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains("02:12:34:56:78:9A"));
        assert!(!text.contains("synthetic-private-session"));
        assert!(!text.contains("ssid_bytes"));
        assert_eq!(std::fs::read(&fixture.0).unwrap(), bytes);
    }

    #[test]
    fn unsupported_version_exits_nonzero_without_a_summary_or_captured_identifiers() {
        let mut invalid = beacon(0);
        invalid["schema_version"] = json!(2);
        let fixture = Fixture::new(format!("{invalid}\n").as_bytes());
        let output = fixture.run();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("unsupported capture schema"));
        assert!(!diagnostic.contains("02:12:34:56:78:9A"));
        assert!(!diagnostic.contains("synthetic-private-session"));
    }

    #[test]
    fn incomplete_final_record_rejects_even_a_valid_prefix() {
        let fixture = Fixture::new(format!("{}\n{}", beacon(0), beacon(1)).as_bytes());
        let output = fixture.run();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("incomplete newline-delimited record"));
    }
}
