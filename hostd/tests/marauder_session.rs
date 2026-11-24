//! Actual service/client roundtrip; saved fixture only, no device or network access.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod support;
#[path = "../crates/airhound-client/src/test_clock.rs"]
mod test_clock;
#[cfg_attr(coverage_nightly, coverage(off))]
mod replay {
    use airhound_api::CaptureAction;
    use airhound_client::Client;
    use serde_json::json;
    use std::{
        path::PathBuf,
        process::{Command, Stdio},
        time::Duration,
    };
    struct Service {
        child: crate::support::ServiceChild,
        root: PathBuf,
    }
    impl Drop for Service {
        fn drop(&mut self) {
            let _ = self.child.child.kill();
            let _ = self.child.child.wait();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[tokio::test(start_paused = true)]
    async fn marauder_service_client_profile_and_durable_query_roundtrip() {
        let time = crate::test_clock::ManualTime::hold().await;
        let root = std::env::temp_dir().join(format!("ah-marauder-service-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("capture.bin");
        let mut bytes = b"[BUF/BEGIN]\xd4\xc3\xb2\xa1\x02\x00\x04\x00".to_vec();
        for n in [0u32, 0, 1514, 105, 4294, 967295, 4, 4] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend([0x08, 0, 0, 0]);
        bytes.extend(b"[BUF/CLOSE]");
        std::fs::write(&path, bytes).unwrap();
        let socket = root.join("private/control.sock");
        let database = root.join("recordings.sqlite");
        let child = crate::support::ServiceChild::spawn(
            Command::new(env!("CARGO_BIN_EXE_airhound-hostd"))
                .args(["serve", "--capture", "--socket"])
                .arg(&socket)
                .arg("--database")
                .arg(&database)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .unwrap();
        let mut service = Service { child, root };
        let mut client = Client::connect(&socket, Duration::from_secs(3))
            .await
            .unwrap();
        assert!(client.status().await.unwrap()["session_sources"]
            .as_array()
            .unwrap()
            .contains(&json!("marauder_capture")));
        let config = json!({"version":1,"receivers":[{"id":"awok-file","source":{"kind":"marauder_capture","path":path,"format":"serial"}}]});
        assert_eq!(
            client.validate(config.clone()).await.unwrap()["valid"],
            true
        );
        client
            .save_profile(
                "saved-awok".into(),
                "Saved capture".into(),
                config.clone(),
                false,
            )
            .await
            .unwrap();
        let loaded = client.load_profile("saved-awok".into()).await.unwrap();
        assert_eq!(loaded["profile"]["config"], config);
        let started = client
            .capture(CaptureAction::Start {
                request_id: "fixture-import".into(),
                config,
            })
            .await
            .unwrap();
        let run = started["run_id"].as_str().unwrap().to_owned();
        // WAL writes can precede the transaction's visible commit. Await the
        // exact run's post-commit witness, including if it arrived before Start
        // returned, then independently verify durable history through the API.
        service.child.wait_for_terminal_commit(&run).unwrap();
        drop(client);
        // A fresh authenticated client sees the completed durable recording.
        let mut client = Client::connect(&socket, Duration::from_secs(3))
            .await
            .unwrap();
        let history = client.recordings(None, 10).await.unwrap();
        assert_eq!(history["runs"][0]["id"], run);
        assert_eq!(history["runs"][0]["state"], "finished");
        assert_eq!(history["runs"][0]["mode"], "replay");
        let page = client.recording_records(run, 0, 100).await.unwrap();
        let record = page["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "wifi_packet_observation")
            .unwrap();
        let value = &record["payload"];
        assert_eq!(value["timestamp_micros32"], u32::MAX);
        assert_eq!(value["replay"], true);
        assert_eq!(value["session_source"]["receiver_id"], "awok-file");
        assert!(value["receiver_location"].is_null());
        assert!(value["rssi_dbm"].is_null());
        assert!(value["mac"].is_null());
        // Metadata-only profile loading retains an editable setup after the file disappears.
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            client.load_profile("saved-awok".into()).await.unwrap()["resources"]["valid"],
            false
        );
        time.finish().await;
    }
}
