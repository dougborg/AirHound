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
