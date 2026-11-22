use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Default)]
struct RecordingWriter(Arc<Mutex<Vec<u8>>>);
impl Write for RecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl RecordingWriter {
    fn messages(&self) -> Vec<serde_json::Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
fn task<F, Fut>(id: &str, start: F) -> ReceiverTask
where
    F: FnOnce(mpsc::Sender<ScanEvent>, ScanControl) -> Fut + Send + 'static,
    Fut: Future<Output = io::Result<()>> + Send + 'static,
{
    ReceiverTask {
        id: id.into(),
        scanner: Scanner::Ble,
        assignment: serde_json::json!({"mode":"test"}),
        shutdown_budget: Duration::from_millis(50),
        needs_macos_location: false,
        start: Box::new(move |tx, control| Box::pin(start(tx, control))),
    }
}
fn marker(n: usize) -> ScanEvent {
    ScanEvent::BleSourceStatus(
        serde_json::json!({"type":"test_packet","n":n,"source":"original","ts":123}),
    )
}
async fn execute(receivers: Vec<ReceiverTask>, output: RecordingWriter) -> io::Result<()> {
    tokio::time::timeout(
        Duration::from_secs(3),
        run(
            receivers,
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            output,
            None,
            |_, _, _| std::future::pending(),
            std::future::pending(),
        ),
    )
    .await
    .expect("session hung")
}

#[tokio::test]
async fn concurrent_bursts_preserve_each_source_order_and_identity() {
    let output = RecordingWriter::default();
    let receivers = ["left", "right"]
        .into_iter()
        .map(|id| {
            task(id, |tx, control| async move {
                control.set_health(Scanner::Ble, Phase::Running, "test_running");
                for n in 0..160 {
                    tx.send(marker(n)).await.map_err(io::Error::other)?;
                }
                Ok(())
            })
        })
        .collect();
    execute(receivers, output.clone()).await.unwrap();
    let messages = output.messages();
    let session = messages[0]["session_id"].clone();
    assert_eq!(messages[0]["receiver_count"], 2);
    for id in ["left", "right"] {
        let packets: Vec<_> = messages
            .iter()
            .filter(|m| m["type"] == "test_packet" && m["session_source"]["receiver_id"] == id)
            .collect();
        assert_eq!(packets.len(), 160);
        for (n, packet) in packets.into_iter().enumerate() {
            assert_eq!(packet["n"], n);
            assert_eq!(packet["session_source"]["ingress_sequence"], n);
            assert_eq!(packet["session_source"]["session_id"], session);
            assert_eq!(packet["source"], "original");
            assert_eq!(packet["ts"], 123);
        }
    }
    let health = messages
        .iter()
        .rev()
        .find(|m| m["type"] == "scanner_health")
        .unwrap();
    for receiver in health["receivers"].as_array().unwrap() {
        assert_eq!(receiver["scanners"]["ble"]["state"], "stopped");
        assert_eq!(receiver["scanners"]["wifi"]["state"], "disabled");
    }
}

#[tokio::test]
async fn error_and_panic_do_not_stop_the_other_receiver() {
    for panic in [false, true] {
        let output = RecordingWriter::default();
        let error = execute(
            vec![
                task("failed", move |_, control| async move {
                    control.set_health(Scanner::Ble, Phase::Running, "test_running");
                    assert!(!panic, "injected receiver panic");
                    Err(io::Error::other("injected receiver failure"))
                }),
                task("survivor", |tx, _| async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    tx.send(marker(7)).await.map_err(io::Error::other)?;
                    Ok(())
                }),
            ],
            output.clone(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("failed"), "{error}");
        let messages = output.messages();
        assert!(messages
            .iter()
            .any(|m| m["n"] == 7 && m["session_source"]["receiver_id"] == "survivor"));
        let health = messages
            .iter()
            .rev()
            .find(|m| m["type"] == "scanner_health")
            .unwrap();
        let failed = health["receivers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "failed")
            .unwrap();
        assert_eq!(failed["scanners"]["ble"]["state"], "unavailable");
    }
}

#[tokio::test]
async fn shutdown_reaches_every_receiver_and_drains_their_final_records() {
    let stopped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (ready_tx, mut ready_rx) = mpsc::channel(2);
    let receivers = ["first", "second"]
        .into_iter()
        .map(|id| {
            let stopped = stopped.clone();
            let ready = ready_tx.clone();
            task(id, move |tx, control| async move {
                let mut state = control.subscribe();
                ready.send(()).await.unwrap();
                while !state.borrow().shutdown {
                    state.changed().await.unwrap();
                }
                tx.send(marker(9)).await.map_err(io::Error::other)?;
                stopped.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        })
        .collect();
    let output = RecordingWriter::default();
    run(
        receivers,
        FilterConfig::new(),
        WigleWriter::new(Vec::new()).unwrap(),
        output.clone(),
        None,
        |_, _, _| std::future::pending(),
        async move {
            ready_rx.recv().await.unwrap();
            ready_rx.recv().await.unwrap();
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(stopped.load(Ordering::SeqCst), 2);
    assert_eq!(output.messages().iter().filter(|m| m["n"] == 9).count(), 2);
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn shutdown_deadline_aborts_unresponsive_receiver_and_reports_uncertainty() {
    let dropped = Arc::new(AtomicBool::new(false));
    let held = dropped.clone();
    let (ready, started) = tokio::sync::oneshot::channel();
    let receiver = task("stuck", move |tx, _| async move {
        let _held = (Dropped(held), tx);
        let _ = ready.send(());
        std::future::pending().await
    });
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        run(
            vec![receiver],
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            RecordingWriter::default(),
            None,
            |_, _, _| std::future::pending(),
            async move {
                started.await.unwrap();
                Ok(())
            },
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.to_string().contains("cleanup unverified"), "{error}");
    assert!(dropped.load(Ordering::SeqCst));
}

struct FailRecords;
impl Write for FailRecords {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes
            .windows(b"session_source".len())
            .any(|w| w == b"session_source")
        {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed consumer"))
        } else {
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn failed_output_shuts_down_remaining_receiver() {
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    let receiver = task("producer", move |tx, control| async move {
        let mut state = control.subscribe();
        tx.send(marker(1)).await.map_err(io::Error::other)?;
        while !state.borrow().shutdown {
            state.changed().await.unwrap();
        }
        flag.store(true, Ordering::SeqCst);
        Ok(())
    });
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        run(
            vec![receiver],
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            FailRecords,
            None,
            |_, _, _| std::future::pending(),
            std::future::pending(),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(stopped.load(Ordering::SeqCst));
}

fn identity() -> EventSource {
    EventSource {
        session_id: "session".into(),
        receiver_id: "receiver".into(),
        ingress_sequence: 7,
    }
}
#[test]
fn source_writer_handles_fragmented_and_multiple_records_without_replacing_payload() {
    let identity = identity();
    let mut bytes = Vec::new();
    let mut writer = SourceWriter::new(&mut bytes, Some(&identity));
    writer.write_all(b"{\"type\":\"one\",\"ts\":42,").unwrap();
    writer
        .write_all(b"\"source\":\"raw\"}\n{\"type\":\"two\"}\n")
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let records: Vec<serde_json::Value> = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["source"], "raw");
    assert_eq!(records[0]["ts"], 42);
    for record in records {
        assert_eq!(record["session_source"]["ingress_sequence"], 7);
    }
}
#[test]
fn source_writer_rejects_malformed_nonobject_oversized_and_incomplete_records() {
    let id = identity();
    for input in [
        b"not json\n".as_slice(),
        b"[]\n",
        &vec![b'x'; LINE_LIMIT + 1],
    ] {
        let mut bytes = Vec::new();
        let mut writer = SourceWriter::new(&mut bytes, Some(&id));
        assert!(writer.write_all(input).is_err());
        assert!(bytes.is_empty());
    }
    let mut bytes = Vec::new();
    let mut writer = SourceWriter::new(&mut bytes, Some(&id));
    writer.write_all(b"{\"unfinished\":").unwrap();
    assert!(writer.flush().is_err());
}
#[test]
fn source_writer_legacy_passthrough_and_underlying_failure() {
    let mut bytes = Vec::new();
    let mut writer = SourceWriter::new(&mut bytes, None);
    writer.write_all(b"not framed").unwrap();
    writer.flush().unwrap();
    drop(writer);
    assert_eq!(bytes, b"not framed");
    let id = identity();
    let mut failing = FailRecords;
    let mut writer = SourceWriter::new(&mut failing, Some(&id));
    assert_eq!(
        writer.write_all(b"{}\n").unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
}

struct TempFile(std::path::PathBuf);
impl TempFile {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "airhound-session-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[test]
fn session_file_loading_is_bounded_and_validates_before_starting() {
    let file = TempFile::new();
    assert_eq!(load(&file.0).unwrap_err().kind(), io::ErrorKind::NotFound);
    for input in [
        b"{}".as_slice(),
        b"not json",
        b"{\"version\":2,\"receivers\":[]}",
        &vec![b' '; 65_537],
    ] {
        std::fs::write(&file.0, input).unwrap();
        assert!(load(&file.0).is_err());
    }
    std::fs::write(&file.0, br#"{"version":1,"receivers":[{"id":"saved","source":{"kind":"ble_pcap","path":"test.pcap"}}]}"#).unwrap();
    let plan = load(&file.0).unwrap();
    assert_eq!(plan.receivers[0].id, "saved");
}

#[tokio::test]
async fn prepare_validates_platform_without_starting_hardware() {
    let plan: SessionConfig = serde_json::from_value(serde_json::json!({"version":1,"receivers":[
        {"id":"native","source":{"kind":"native_ble"}},
        {"id":"sniffle","source":{"kind":"sniffle","port":"/unavailable/serial","python":"python3","helper":"helper.py"}},
        {"id":"saved","source":{"kind":"ble_pcap","path":"saved.pcap"}},
        {"id":"alfa","source":{"kind":"external_wifi","executable":"helper","firmware":"firmware.bin","channels":[1,6,11],"dwell_ms":100}}
    ]})).unwrap();
    let tasks = prepare(plan).await.unwrap();
    assert_eq!(tasks.len(), 4);
    assert_eq!(tasks[1].assignment["channel"], 37);
    assert!(matches!(tasks[3].scanner, Scanner::Wifi));
    assert_eq!(tasks[3].shutdown_budget, Duration::from_secs(40));
    let invalid_adapter: SessionConfig =
        serde_json::from_value(serde_json::json!({"version":1,"receivers":[
            {"id":"bad","source":{"kind":"native_ble","adapter":"invalid"}}
        ]}))
        .unwrap();
    assert!(prepare(invalid_adapter).await.is_err());
    #[cfg(target_os = "macos")]
    let wrong_platform = "linux_wifi";
    #[cfg(not(target_os = "macos"))]
    let wrong_platform = "native_wifi";
    let source = if wrong_platform == "linux_wifi" {
        serde_json::json!({"kind":wrong_platform,"interface":"mon0"})
    } else {
        serde_json::json!({"kind":wrong_platform})
    };
    let unsupported: SessionConfig = serde_json::from_value(
        serde_json::json!({"version":1,"receivers":[{"id":"unsupported","source":source}]}),
    )
    .unwrap();
    assert!(prepare(unsupported).await.is_err());
}

#[tokio::test]
async fn missing_replay_is_attributed_and_does_not_prevent_other_sources() {
    let file = TempFile::new();
    let plan: SessionConfig = serde_json::from_value(serde_json::json!({"version":1,"receivers":[
        {"id":"missing-capture","source":{"kind":"ble_pcap","path":file.0}}
    ]}))
    .unwrap();
    let mut tasks = prepare(plan).await.unwrap();
    tasks.push(task("survivor", |tx, _| async move {
        tx.send(marker(12)).await.map_err(io::Error::other)?;
        Ok(())
    }));
    let output = RecordingWriter::default();
    let error = execute(tasks, output.clone()).await.unwrap_err();
    assert!(error.to_string().contains("missing-capture"));
    assert!(output.messages().iter().any(|m| m["n"] == 12));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_health_identifies_single_panicked_receiver() {
    for _ in 0..30 {
        let output = RecordingWriter::default();
        let _ = execute(
            vec![task("panic", |_, control| async move {
                control.set_health(Scanner::Ble, Phase::Running, "test_running");
                panic!("single receiver panic");
            })],
            output.clone(),
        )
        .await
        .unwrap_err();
        let messages = output.messages();
        let last = messages
            .iter()
            .rev()
            .find(|m| m["type"] == "scanner_health")
            .unwrap();
        assert_eq!(
            last["receivers"][0]["scanners"]["ble"]["state"],
            "unavailable"
        );
    }
}

#[tokio::test]
async fn explicit_unavailable_backend_state_survives_clean_return() {
    let output = RecordingWriter::default();
    execute(
        vec![task("unavailable", |_, control| async move {
            control.set_health(Scanner::Ble, Phase::Unavailable, "adapter_powered_off");
            Ok(())
        })],
        output.clone(),
    )
    .await
    .unwrap();
    let messages = output.messages();
    let last = messages
        .iter()
        .rev()
        .find(|m| m["type"] == "scanner_health")
        .unwrap();
    assert_eq!(
        last["receivers"][0]["scanners"]["ble"]["state"],
        "unavailable"
    );
    assert_eq!(
        last["receivers"][0]["scanners"]["ble"]["reason"],
        "adapter_powered_off"
    );
}

#[tokio::test]
async fn delayed_external_status_cannot_resurrect_finished_receiver() {
    let output = RecordingWriter::default();
    let mut receiver = task("wifi", |tx, control| async move {
        control.finish_receiver(Scanner::Wifi, true);
        tx.send(ScanEvent::ExternalStatus {
            usb_selector: Some("usb:1:2".into()),
            state: "running".into(),
            dropped: 3,
            producer_dropped: 4,
            reconnect_attempts: 2,
        })
        .await
        .map_err(io::Error::other)?;
        Ok(())
    });
    receiver.scanner = Scanner::Wifi;
    execute(vec![receiver], output.clone()).await.unwrap();
    let messages = output.messages();
    let historical = messages
        .iter()
        .find(|m| m["type"] == "external_scanner_status")
        .unwrap();
    assert_eq!(historical["host_dropped_observations"], 3);
    assert_eq!(historical["session_source"]["receiver_id"], "wifi");
    let current = messages
        .iter()
        .rev()
        .find(|m| m["type"] == "scanner_health")
        .unwrap();
    assert_eq!(
        current["receivers"][0]["scanners"]["wifi"]["state"],
        "unavailable"
    );
}
