#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::process::Command;
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn offline_cli_requires_source_identity_and_rejects_bad_input_without_output() {
    let binary = env!("CARGO_BIN_EXE_airhound-marauder-import");
    let missing = Command::new(binary).arg("unused.pcap").output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--source-id"));
    let bad = Command::new(binary)
        .args(["Cargo.toml", "--source-id", "fixture"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty());
    let empty = Command::new(binary)
        .args(["unused.pcap", "--source-id", " "])
        .output()
        .unwrap();
    assert!(!empty.status.success());
    assert!(empty.stdout.is_empty());
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn offline_cli_imports_binary_serial_capture() {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!(
        "airhound-marauder-fixture-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut fixture = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    fixture
        .write_all(b"console\n[BUF/BEGIN]\xd4\xc3\xb2\xa1\x02\x00\x04\x00")
        .unwrap();
    for n in [0u32, 0, 1514, 105, 0, 42, 11, 11] {
        fixture.write_all(&n.to_le_bytes()).unwrap();
    }
    fixture
        .write_all(b"[BUF/CLOSE][BUF/CLOSE]console\n")
        .unwrap();
    drop(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_airhound-marauder-import"))
        .arg(&path)
        .args(["--serial", "--source-id", "fixture-chip"])
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["source_id"], "fixture-chip");
    assert_eq!(value["timestamp_micros32"], 42);
    assert_eq!(value["frame_bytes"], serde_json::json!(b"[BUF/CLOSE]"));
    assert!(value["rssi_dbm"].is_null());
}
