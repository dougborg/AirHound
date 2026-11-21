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
