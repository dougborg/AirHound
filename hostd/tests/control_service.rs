//! Exercises the real CLI adapter and shared client without opening any receiver.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg_attr(coverage_nightly, coverage(off))]
mod passive_cli {
    use serde_json::{json, Value};
    use std::{
        io,
        path::PathBuf,
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    struct Service {
        child: Child,
        root: PathBuf,
    }
    impl Drop for Service {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn cli_attaches_validates_and_detaches_without_starting_capture() {
        let root = std::env::temp_dir().join(format!("ah-cli-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let database = root.join("recording.sqlite");
        let mut store = airhound_hostd::storage::Store::open(&database).unwrap();
        let run = store
            .start(
                airhound_hostd::storage::RecordingMode::Replay,
                &json!({"fixture":true}),
            )
            .unwrap();
        store.add_source(&run, "receiver", &json!({})).unwrap();
        let payload = json!({"type":"ble_observation","address":null,"replay":true,"detection":{"alert":"suppressed"}});
        store
            .append(
                &run,
                &[airhound_hostd::storage::RecordInput {
                    source_id: "receiver".into(),
                    received_at_us: 123,
                    payload: payload.clone(),
                }],
            )
            .unwrap();
        let socket = root.join("private/control.sock");
        let executable = env!("CARGO_BIN_EXE_airhound-hostd");
        let child = Command::new(executable)
            .args(["serve", "--socket"])
            .arg(&socket)
            .arg("--database")
            .arg(&database)
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut service = Service { child, root };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(
                service.child.try_wait().unwrap().is_none(),
                "service exited before bind"
            );
            assert!(Instant::now() < deadline, "service bind timeout");
            thread::sleep(Duration::from_millis(10));
        }
        let client = |extra: &[&str]| {
            Command::new(executable)
                .args(["client", "--socket"])
                .arg(&socket)
                .args(extra)
                .current_dir(&service.root)
                .output()
                .unwrap()
        };
        let first = client(&["status"]);
        let second = client(&["status"]);
        assert!(first.status.success() && second.status.success());
        let first: Value = serde_json::from_slice(&first.stdout).unwrap();
        let second: Value = serde_json::from_slice(&second.stdout).unwrap();
        assert_eq!(first["instance_id"], second["instance_id"]);
        assert_eq!(first["mode"], "passive_control");
        assert_eq!(first["capture_control"], false);
        assert_eq!(first["build"]["component"], "airhound-hostd");
        assert_eq!(first["build"]["control_protocol"], 1);
        assert!(first["build"]["display_version"].is_string());
        assert_eq!(first["location_providers"], serde_json::json!(["gpsd"]));
        let history = client(&["recordings", "--limit", "1"]);
        assert!(
            history.status.success(),
            "{}",
            String::from_utf8_lossy(&history.stderr)
        );
        let history: Value = serde_json::from_slice(&history.stdout).unwrap();
        assert_eq!(history["runs"][0]["id"], run);
        assert_eq!(history["runs"][0]["state"], "recording");
        let records = client(&["records", "--run-id", &run]);
        assert!(records.status.success());
        let records: Value = serde_json::from_slice(&records.stdout).unwrap();
        assert_eq!(records["records"][0]["payload"], payload);
        let snapshot_path = service.root.join("snapshot.sqlite");
        let exported = client(&["export", "--output", snapshot_path.to_str().unwrap()]);
        assert!(
            exported.status.success(),
            "{}",
            String::from_utf8_lossy(&exported.stderr)
        );
        let exported: Value = serde_json::from_slice(&exported.stdout).unwrap();
        assert_eq!(exported["consistent_snapshot"], true);
        let snapshot = airhound_hostd::storage::Reader::open(&snapshot_path).unwrap();
        assert_eq!(snapshot.runs(1).unwrap()[0].record_count, 1);
        drop(snapshot);
        assert!(
            !client(&["export", "--output", snapshot_path.to_str().unwrap()])
                .status
                .success()
        );
        assert!(!client(&["recordings", "--limit", "101"]).status.success());
        assert!(!client(&["records", "--run-id", &run, "--limit", "501"])
            .status
            .success());
        store
            .transition(&run, "finished", "fixture complete")
            .unwrap();
        let history: Value = serde_json::from_slice(&client(&["recordings"]).stdout).unwrap();
        assert_eq!(history["runs"][0]["state"], "finished");
        std::fs::write(
            service.root.join("plan.json"),
            serde_json::to_vec(
                &json!({"version":1,"receivers":[{"id":"ble","source":{"kind":"native_ble"}}]}),
            )
            .unwrap(),
        )
        .unwrap();
        let valid = client(&["validate", "--session-config", "plan.json"]);
        assert!(
            valid.status.success(),
            "{}",
            String::from_utf8_lossy(&valid.stderr)
        );
        let preview: Value = serde_json::from_slice(&valid.stdout).unwrap();
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["readiness"], "unverified");
        let saved = client(&[
            "profiles",
            "save",
            "--id",
            "fixture",
            "--name",
            "My setup",
            "--session-config",
            "plan.json",
        ]);
        assert!(
            saved.status.success(),
            "{}",
            String::from_utf8_lossy(&saved.stderr)
        );
        let saved: Value = serde_json::from_slice(&saved.stdout).unwrap();
        assert_eq!(saved["reception"], "not_tested");
        assert!(!client(&[
            "profiles",
            "save",
            "--id",
            "fixture",
            "--name",
            "Overwrite",
            "--session-config",
            "plan.json"
        ])
        .status
        .success());
        let listed: Value = serde_json::from_slice(&client(&["profiles", "list"]).stdout).unwrap();
        assert_eq!(listed["profiles"][0]["name"], "My setup");
        let loaded: Value =
            serde_json::from_slice(&client(&["profiles", "load", "--id", "fixture"]).stdout)
                .unwrap();
        assert_eq!(loaded["applied"], false);
        assert_eq!(loaded["profile"]["config"], saved["profile"]["config"]);
        assert_eq!(loaded["resources"]["valid"], true);
        assert_eq!(loaded["bindings"]["automatic_rebinding"], false);
        assert_eq!(loaded["validation"]["readiness"], "unverified");
        assert!(client(&["profiles", "delete", "--id", "fixture"])
            .status
            .success());
        assert!(!client(&["profiles", "load", "--id", "fixture"])
            .status
            .success());
        std::fs::write(
            service.root.join("plan.json"),
            b"{\"version\":99,\"receivers\":[]}",
        )
        .unwrap();
        let invalid = client(&["validate", "--session-config", "plan.json"]);
        assert!(!invalid.status.success());
        let error: Value = serde_json::from_slice(&invalid.stdout).unwrap();
        assert_eq!(error["error"]["code"], "invalid_session");
        assert!(client(&["status"]).status.success());
        assert!(!service.root.join("observations.csv").exists());
        // SAFETY: this is the owned child PID; it has not exited/reaped or been reused.
        assert_eq!(
            unsafe { libc::kill(service.child.id() as libc::pid_t, libc::SIGTERM) },
            0
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = service.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "service shutdown timeout");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::symlink_metadata(&socket).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn service_owns_replay_and_commits_before_finish_reply() {
    use serde_json::{json, Value};
    use std::process::{Command, Stdio};
    let root = std::env::temp_dir().join(format!("ah-capture-service-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let socket = root.join("private/control.sock");
    let database = root.join("recording.sqlite");
    // An empty classic radiotap capture tests EOF cleanup without RF activity.
    let mut bytes = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    bytes.extend(65536u32.to_le_bytes());
    bytes.extend(127u32.to_le_bytes());
    let pcap = root.join("empty.pcap");
    std::fs::write(&pcap, bytes).unwrap();
    let plan = root.join("plan.json");
    std::fs::write(&plan, serde_json::to_vec(&json!({"version":1,"receivers":[{"id":"replay","source":{"kind":"wifi_pcap","path":pcap}}]})).unwrap()).unwrap();
    let binary = env!("CARGO_BIN_EXE_airhound-hostd");
    let mut server = Command::new(binary)
        .args(["serve", "--capture", "--socket"])
        .arg(&socket)
        .arg("--database")
        .arg(&database)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    struct Cleanup(std::process::Child, std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !socket.exists() {
        assert!(server.try_wait().unwrap().is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut cleanup = Cleanup(server, root);
    let client = |args: &[&str]| {
        let output = Command::new(binary)
            .args(["client", "--socket"])
            .arg(&socket)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert_eq!(client(&["status"])["capture_control"], true);
    let started = client(&[
        "start",
        "--request-id",
        "acceptance-run",
        "--session-config",
        plan.to_str().unwrap(),
    ]);
    let id = started["run_id"].as_str().unwrap();
    // The start client has exited. A new connection finishes the service-owned run.
    let finished = client(&["finish", "--run-id", id]);
    assert_eq!(finished["state"], "finished");
    assert_eq!(finished["active"], false);
    let reader = airhound_hostd::storage::Reader::open(&database).unwrap();
    let runs = reader.runs(1).unwrap();
    assert_eq!(runs[0].id, id);
    assert_eq!(runs[0].state, "finished");
    assert!(runs[0].record_count > 0);
    assert_eq!(client(&["finish", "--run-id", id])["state"], "finished");
    assert_eq!(client(&["capture-status"])["active"], false);
    let retried = client(&[
        "start",
        "--request-id",
        "acceptance-run",
        "--session-config",
        plan.to_str().unwrap(),
    ]);
    assert_eq!(retried["run_id"], id);
    assert_eq!(retried["reused"], true);
    assert_eq!(reader.runs(10).unwrap().len(), 1);
    // SAFETY: this is the owned, live child process.
    assert_eq!(
        unsafe { libc::kill(cleanup.0.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    while cleanup.0.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!socket.exists());
    std::fs::remove_file(&pcap).unwrap();
    cleanup.0 = Command::new(binary)
        .args(["serve", "--capture", "--socket"])
        .arg(&socket)
        .arg("--database")
        .arg(&database)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !socket.exists() {
        assert!(cleanup.0.try_wait().unwrap().is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let retried = client(&[
        "start",
        "--request-id",
        "acceptance-run",
        "--session-config",
        plan.to_str().unwrap(),
    ]);
    assert_eq!(retried["run_id"], id);
    assert_eq!(retried["active"], false);
    assert_eq!(reader.runs(10).unwrap().len(), 1);
    drop(reader);
}
