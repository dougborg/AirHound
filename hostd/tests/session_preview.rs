#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod support;

use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("airhound-preview-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn run(&self, plan: &Value) -> std::process::Output {
        let config = self.0.join("session.json");
        std::fs::write(&config, serde_json::to_vec(plan).unwrap()).unwrap();
        Command::new(env!("CARGO_BIN_EXE_airhound-hostd"))
            .current_dir(&self.0)
            .args(["session", "validate", "--config"])
            .arg(config)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn preview_resolves_channel_allocation_without_opening_receivers() {
    let fixture = Fixture::new("allocation");
    let source = |port: &str, channel: Option<u8>| {
        json!({
            "kind":"sniffle", "port":port, "python":"/nonexistent/python",
            "helper":"/nonexistent/helper", "channel":channel
        })
    };
    let output = fixture.run(&json!({"version":1,"receivers":[
        {"id":"automatic","source":source("/nonexistent/radio-a",None)},
        {"id":"pinned","source":source("/nonexistent/radio-b",Some(37))}
    ]}));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["valid"], true);
    assert_eq!(reply["readiness"], "unverified");
    assert_eq!(
        reply["resolved_config"]["receivers"][0]["source"]["channel"],
        38
    );
    assert_eq!(reply["receivers"][0]["requested_assignment"]["channel"], 38);
    assert_eq!(reply["receivers"][1]["requested_assignment"]["channel"], 37);
    // Preview created neither a capture file nor a scanner process: its supplied
    // receiver executables and ports do not even exist.
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn invalid_plans_produce_structured_failures_without_capture_output() {
    let fixture = Fixture::new("invalid");
    for plan in [
        json!({"version":99,"receivers":[]}),
        json!({"version":1,"receivers":[
            {"id":"a","source":{"kind":"native_ble"}},
            {"id":"b","source":{"kind":"native_ble"}}
        ]}),
        json!({"padding":"x".repeat(65_537)}),
    ] {
        let output = fixture.run(&plan);
        assert!(!output.status.success());
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["valid"], false);
        assert_eq!(reply["error"]["code"], "invalid_session");
        assert!(reply.get("resolved_config").is_none());
    }
    let directory = Command::new(env!("CARGO_BIN_EXE_airhound-hostd"))
        .args(["session", "validate", "--config"])
        .arg(&fixture.0)
        .output()
        .unwrap();
    assert!(!directory.status.success());
    let reply: Value = serde_json::from_slice(&directory.stdout).unwrap();
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("regular file"));
    let conflict = Command::new(env!("CARGO_BIN_EXE_airhound-hostd"))
        .args([
            "--output",
            "must-not-exist.csv",
            "session",
            "validate",
            "--config",
        ])
        .arg(fixture.0.join("session.json"))
        .current_dir(&fixture.0)
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    assert!(!fixture.0.join("must-not-exist.csv").exists());
}

#[cfg(unix)]
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn fifo_configuration_is_rejected_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new("fifo");
    let path = fixture.0.join("config.fifo");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let output = support::output(
        Command::new(env!("CARGO_BIN_EXE_airhound-hostd"))
            .args(["session", "validate", "--config"])
            .arg(path),
    )
    .unwrap();
    assert!(!output.status.success());
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("regular file"));
}
