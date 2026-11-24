//! Service-owned capture actor. Request cancellation never cancels a receiver.
use crate::{control::ScanControl, session_runtime, FilterConfig, WigleWriter};
use airhound_hostd::{
    session::{SessionConfig, Source},
    storage::{self, output::Completion, writer::Writer, RecordingMode},
};
use serde_json::{json, Value};
use std::{io, path::PathBuf, sync::atomic::Ordering, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

pub(crate) enum Action {
    Start { request_id: String, config: Value },
    Pause(String),
    Resume(String),
    Finish(String),
    Status,
    Identify(airhound_hostd::discovery::identify::Request),
    Shutdown,
}
struct Request {
    action: Action,
    reply: oneshot::Sender<io::Result<Value>>,
}
#[derive(Clone)]
pub(crate) struct Handle(mpsc::Sender<Request>);
impl Handle {
    pub async fn request(&self, action: Action) -> io::Result<Value> {
        let (reply, receiver) = oneshot::channel();
        self.0
            .send(Request { action, reply })
            .await
            .map_err(|_| io::Error::other("capture service stopped"))?;
        receiver
            .await
            .map_err(|_| io::Error::other("capture request was not acknowledged"))?
    }
}
pub(crate) fn spawn(database: PathBuf) -> (Handle, JoinHandle<io::Result<()>>) {
    spawn_with_active(database, None)
}
fn spawn_with_active(
    database: PathBuf,
    initial: Option<Active>,
) -> (Handle, JoinHandle<io::Result<()>>) {
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    let task = tokio::spawn(async move {
        let mut active = initial;
        let mut last: Option<Value> = None;
        // Capture-enabled service startup already reconciles orphans before IPC.
        let mut recovery = StorageRecovery::default();
        let mut maintenance = tokio::time::interval(Duration::from_secs(2));
        maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        maintenance.reset();
        loop {
            let request = tokio::select! {
                // Recovery writes occur only on this background timer,
                // never as a side effect of Status/history requests.
                biased;
                _ = maintenance.tick() => {
                    collect_finished(&mut active, &mut last, &mut recovery).await;
                    if active.is_none() { recovery.attempt(database.clone()).await; }
                    // Fixed delay after completion: a slow/busy SQLite retry
                    // must not make the next tick immediately ready and starve requests.
                    maintenance.reset();
                    continue;
                }
                request = rx.recv() => match request { Some(request) => request, None => break },
            };
            collect_finished(&mut active, &mut last, &mut recovery).await;
            let shutdown = matches!(request.action, Action::Shutdown);
            let paused = matches!(request.action, Action::Pause(_));
            let result = match request.action {
                Action::Start { request_id, config } => {
                    let path = database.clone();
                    let key = request_id.clone();
                    let original = config.clone();
                    let receipt = tokio::task::spawn_blocking(move || {
                        if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
                            return Err(invalid(
                                "start request id must contain 1–128 non-control bytes",
                            ));
                        }
                        match storage::Reader::open(&path) {
                            Ok(reader) => reader.start_receipt(&key, &original),
                            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                            Err(error) => Err(error),
                        }
                    })
                    .await
                    .map_err(io::Error::other)
                    .and_then(|result| result);
                    if let Ok(Some(mut receipt)) = receipt {
                        receipt["active"] = json!(active
                            .as_ref()
                            .is_some_and(|run| receipt["run_id"] == run.id));
                        Ok(receipt)
                    } else if let Err(error) = receipt {
                        Err(error)
                    } else if active.is_some() {
                        Err(invalid("a recording is already active"))
                    } else if recovery.pending {
                        Err(io::Error::other(recovery.start_error()))
                    } else {
                        match start(database.clone(), request_id, config).await {
                            Ok(run) => {
                                let status = run.status();
                                active = Some(run);
                                Ok(status)
                            }
                            Err(error) => {
                                if error.kind() != io::ErrorKind::InvalidInput {
                                    recovery.pending = true;
                                    recovery.error = Some(error.to_string());
                                }
                                Err(error)
                            }
                        }
                    }
                }
                Action::Identify(config) => {
                    // Actor serialization prevents a capture start or another probe racing
                    // endpoint ownership. An active run is never interrupted to identify it.
                    if request.reply.is_closed() {
                        continue;
                    }
                    let result = if let Some(run) = active.as_ref() {
                        run.identify(&config).await
                    } else {
                        airhound_hostd::discovery::identify::query(&config).await
                    };
                    serde_json::to_value(result).map_err(io::Error::other)
                }
                Action::Status => Ok(active.as_ref().map(Active::status).unwrap_or_else(
                    || json!({"active":false,"last":last,"storage_recovery":recovery.status()}),
                )),
                Action::Pause(id) | Action::Resume(id) => {
                    match active.as_ref().filter(|run| run.id == id) {
                        Some(run) => run.transition(paused).await,
                        None => Err(invalid("recording is not active in this service")),
                    }
                }
                Action::Finish(id) => {
                    if active.as_ref().is_some_and(|run| run.id == id) {
                        let run = active.take().expect("active run");
                        run.control.shutdown();
                        let mut result = run.complete().await;
                        recovery.note(&result);
                        last = Some(result.clone());
                        result["storage_recovery"] = recovery.status();
                        Ok(result)
                    } else if last.as_ref().is_some_and(|run| run["run_id"] == id) {
                        let mut result = last.clone().expect("last run");
                        result["storage_recovery"] = recovery.status();
                        Ok(result)
                    } else {
                        Err(invalid("recording is not active in this service"))
                    }
                }
                Action::Shutdown => {
                    if let Some(run) = active.take() {
                        run.control.shutdown();
                        let result = run.complete().await;
                        recovery.note(&result);
                        last = Some(result);
                    }
                    Ok(json!({"active":false,"last":last,"storage_recovery":recovery.status()}))
                }
            };
            let _ = request.reply.send(result);
            if shutdown {
                if recovery.pending {
                    return Err(io::Error::other("recording storage recovery remains pending; free disk space and restart the capture service to reconcile unfinished history"));
                }
                if last.as_ref().is_some_and(|run| run["state"] == "failed") {
                    return Err(io::Error::other(
                        "capture ended with a failure; inspect recording history",
                    ));
                }
                return Ok(());
            }
        }
        if let Some(run) = active {
            run.control.shutdown();
            let result = run.complete().await;
            if result["state"] != "finished" {
                return Err(io::Error::other("capture cleanup failed"));
            }
        }
        if recovery.pending {
            return Err(io::Error::other(
                "recording storage recovery remains pending",
            ));
        }
        Ok(())
    });
    (Handle(tx), task)
}
/// A failed terminal write can outlive its writer thread. Keep its exact run
/// identity/reason until an idle, exclusively locked recovery transaction commits.
/// A restart loses in-memory causality, so ordinary Store recovery uses interrupted.
#[derive(Default)]
struct StorageRecovery {
    pending: bool,
    failed: Option<(String, String)>,
    error: Option<String>,
}
impl StorageRecovery {
    fn note(&mut self, result: &Value) {
        if result["state"] == "failed" {
            if let (Some(id), Some(reason)) = (result["run_id"].as_str(), result["error"].as_str())
            {
                self.failed = Some((id.to_owned(), reason.to_owned()));
                self.pending = true;
                self.error = None;
            }
        }
    }
    fn status(&self) -> Value {
        json!({"pending":self.pending,"error":self.error})
    }
    fn start_error(&self) -> String {
        format!("Recording storage recovery is pending; free disk space or resolve the storage error before starting another recording. {}", self.error.as_deref().unwrap_or("The service will retry automatically while idle."))
    }
    async fn attempt(&mut self, database: PathBuf) {
        if !self.pending {
            return;
        }
        let failed = self.failed.clone();
        let result = tokio::task::spawn_blocking(move || {
            storage::Store::recover(
                &database,
                failed
                    .as_ref()
                    .map(|(id, reason)| (id.as_str(), reason.as_str())),
            )
        })
        .await
        .map_err(io::Error::other)
        .and_then(|result| result);
        match result {
            Ok(()) => {
                self.pending = false;
                self.failed = None;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
            }
        }
    }
}
async fn collect_finished(
    active: &mut Option<Active>,
    last: &mut Option<Value>,
    recovery: &mut StorageRecovery,
) {
    if active.as_ref().is_some_and(|run| run.task.is_finished()) {
        let result = active.take().expect("active run").complete().await;
        recovery.note(&result);
        *last = Some(result);
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
struct Active {
    id: String,
    control: ScanControl,
    recording: Completion,
    task: JoinHandle<io::Result<()>>,
}
impl Active {
    async fn identify(
        &self,
        request: &airhound_hostd::discovery::identify::Request,
    ) -> airhound_hostd::discovery::identify::Identification {
        use airhound_hostd::discovery::{self, identify::Identification};
        let inventory = discovery::discover(&discovery::Options::default()).await;
        if let Err(error) = request.validate(&inventory) {
            return Identification::new(request, "unavailable", error.to_string());
        }
        Identification::from_active_owner(request, &self.status())
    }

    fn status(&self) -> Value {
        json!({"active":true,"run_id":self.id,"state":if self.control.load(Ordering::Relaxed) { "recording" } else { "paused" },"receiver_health":self.control.health_message()})
    }
    async fn transition(&self, paused: bool) -> io::Result<Value> {
        if self.control.load(Ordering::Relaxed) != !paused {
            if paused {
                self.control.store(false, Ordering::Relaxed);
            }
            if let Err(error) = self.recording.transition(paused).await {
                self.control.shutdown();
                return Err(error);
            }
            if !paused {
                self.control.store(true, Ordering::Relaxed);
            }
        }
        Ok(self.status())
    }
    async fn complete(self) -> Value {
        let result = self
            .task
            .await
            .map_err(io::Error::other)
            .and_then(|result| result);
        match result {
            Ok(()) => json!({"active":false,"run_id":self.id,"state":"finished"}),
            Err(error) => {
                json!({"active":false,"run_id":self.id,"state":"failed","error":error.to_string()})
            }
        }
    }
}
async fn start(database: PathBuf, request_id: String, config: Value) -> io::Result<Active> {
    if serde_json::to_vec(&config).map_err(io::Error::other)?.len() > airhound_api::MAX_CONFIG_BYTES
    {
        return Err(invalid("session config exceeds 64 KiB"));
    }
    let plan: SessionConfig = SessionConfig::from_json(config.clone())?;
    let request = (request_id, config);
    let plan = plan.resolve()?;
    let mut filter = FilterConfig::new();
    if let Some(policy) = plan.alert_policy {
        filter.alert_policy = policy;
    }
    let config = json!({"session":plan,"alert_policy":filter.alert_policy});
    let replay = plan.receivers.iter().all(|receiver| {
        matches!(
            receiver.source,
            Source::BlePcap { .. } | Source::WifiPcap { .. } | Source::MarauderCapture { .. }
        )
    });
    let mut sources: Vec<_> = plan
        .receivers
        .iter()
        .map(|receiver| {
            (
                receiver.id.clone(),
                json!({"requested_source":receiver.source}),
            )
        })
        .collect();
    sources.push((
        storage::output::SESSION_SOURCE.into(),
        json!({"kind":"session_control"}),
    ));
    let gps = plan.location.as_ref().map(crate::gpsd::Options::from);
    let receivers = session_runtime::prepare(plan).await?;
    let writer = tokio::task::spawn_blocking(move || {
        Writer::start_request(
            database,
            if replay {
                RecordingMode::Replay
            } else {
                RecordingMode::Live
            },
            config,
            sources,
            Some(request),
        )
    })
    .await
    .map_err(io::Error::other)??;
    Ok(launch(receivers, filter, writer, gps))
}
fn launch(
    receivers: Vec<session_runtime::ReceiverTask>,
    filter: FilterConfig,
    writer: Writer,
    gps: Option<crate::gpsd::Options>,
) -> Active {
    let id = writer.run_id.clone();
    let (output, recording) = storage::output::attach(io::sink(), writer);
    let control = ScanControl::new(true);
    let capture_control = control.clone();
    let mut stop = control.subscribe();
    let completion = recording.clone();
    let task = tokio::spawn(async move {
        let csv = WigleWriter::new(io::sink())?;
        let result = session_runtime::run_controlled(
            receivers,
            (filter, capture_control),
            csv,
            output,
            gps,
            |_, _, _| async {},
            async move {
                loop {
                    if stop.borrow().shutdown {
                        return Ok(());
                    }
                    stop.changed().await.map_err(io::Error::other)?;
                }
            },
        )
        .await;
        let succeeded = result.is_ok();
        tokio::task::spawn_blocking(move || completion.finish(succeeded))
            .await
            .map_err(io::Error::other)??;
        result
    });
    Active {
        id,
        control,
        recording,
        task,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64},
        Arc,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ah-actor-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> PathBuf {
            self.0.join("capture.sqlite")
        }
        fn actor(
            &self,
            receiver: session_runtime::ReceiverTask,
        ) -> (Handle, JoinHandle<io::Result<()>>, String) {
            let writer = Writer::start(
                self.path(),
                RecordingMode::Replay,
                json!({}),
                vec![
                    ("fixture".into(), json!({})),
                    (storage::output::SESSION_SOURCE.into(), json!({})),
                ],
            )
            .unwrap();
            let id = writer.run_id.clone();
            let active = launch(vec![receiver], FilterConfig::new(), writer, None);
            let (handle, task) = spawn_with_active(self.path(), Some(active));
            (handle, task, id)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn rayhunter_raw_and_cells_survive_pause_resume_finish_and_history() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let helper = fixture.0.join("decoder");
        let raw = b"diagnostic fixture";
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(raw)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let records = json!([
            {"ipc_version":1,"kind":"hello","stream":{"version":1,"replay":false,"format":"u32le_json_length+json+exact_device_read","stream_epoch":"fixture","next_sequence":0}},
            {"ipc_version":1,"kind":"raw","frame":{"sequence":0,"raw_length":raw.len(),"read_monotonic_ns":12,"read_system_unix_ns":null,"recording_id":null},"offset":0,"hex":raw.iter().map(|b|format!("{b:02x}")).collect::<String>(),"sha256":hash},
            {"ipc_version":1,"kind":"cell","sequence":0,"message_index":0,"modem_timestamp":123,"observation":{"rat":"lte","kind":"cell_identity","earfcn":70000,"pci":42,"mcc":"001","mnc":"01","tac":123,"eci":456}}
        ]);
        // JSON is quoted as a JSON string literal: no shell is involved.
        let input = serde_json::to_string(&records.to_string()).unwrap();
        std::fs::write(&helper,format!("#!/usr/bin/env python3\nimport json,time\nfor r in json.loads({input}): print(json.dumps(r),flush=True)\nwhile True:\n print(json.dumps({{'ipc_version':1,'kind':'status'}}),flush=True)\n time.sleep(.1)\n")).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (handle, actor) = spawn(fixture.path());
        let config = json!({"version":1,"receivers":[{"id":"orbic","source":{"kind":"rayhunter","endpoint":"ws://127.0.0.1:9/raw-diag","decoder":helper}}]});
        let start = handle
            .request(Action::Start {
                request_id: "cellular-fixture".into(),
                config,
            })
            .await
            .unwrap();
        let id = start["run_id"].as_str().unwrap().to_owned();
        for expected in [1, 2] {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let records = storage::Reader::open(&fixture.path())
                        .unwrap()
                        .records(&id, 0, 500)
                        .unwrap()
                        .records;
                    if records
                        .iter()
                        .filter(|r| r.kind == "cellular_observation")
                        .count()
                        >= expected
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            if expected == 1 {
                handle.request(Action::Pause(id.clone())).await.unwrap();
                handle.request(Action::Resume(id.clone())).await.unwrap();
            }
        }
        handle.request(Action::Finish(id.clone())).await.unwrap();
        handle.request(Action::Shutdown).await.unwrap();
        actor.await.unwrap().unwrap();
        let records = storage::Reader::open(&fixture.path())
            .unwrap()
            .records(&id, 0, 500)
            .unwrap()
            .records;
        let cells = records
            .iter()
            .filter(|r| r.kind == "cellular_observation")
            .collect::<Vec<_>>();
        assert_eq!(cells.len(), 2);
        let gaps = records
            .iter()
            .filter(|r| r.kind == "cellular_source_gap")
            .collect::<Vec<_>>();
        assert_eq!(gaps.len(), 1);
        let gap = gaps[0];
        assert_eq!(gap.payload["reason"], "subscription_pause_resume");
        assert_eq!(gap.payload["deliberate"], true);
        assert_eq!(gap.payload["previous_connection_generation"], 1);
        assert_eq!(gap.payload["connection_generation"], 2);
        assert!(gap.payload["lost_records"].is_null());
        assert_eq!(gap.payload["rf_loss"], "unknown");
        assert!(gap.cursor > cells[0].cursor && gap.cursor < cells[1].cursor);
        let first_resumed = records
            .iter()
            .find(|r| r.payload["connection_generation"] == 2)
            .unwrap();
        assert_eq!(first_resumed.cursor, gap.cursor);
        for (i, cell) in cells.iter().enumerate() {
            assert_eq!(cell.payload["session_source"]["receiver_id"], "orbic");
            assert_eq!(cell.payload["observation"]["mnc"], "01");
            assert_eq!(cell.payload["observation"]["earfcn"], 70000);
            assert_eq!(cell.payload["connection_generation"], i + 1);
            assert!(cell.payload["capture_time"].is_null());
            assert!(cell.payload["receiver_location"].is_null());
            let before = records
                .iter()
                .take_while(|r| r.cursor != cell.cursor)
                .filter(|r| r.kind == "cellular_raw_chunk")
                .last()
                .unwrap();
            assert_eq!(
                before.payload["sha256"],
                cell.payload["raw_reference"]["sha256"]
            );
            assert_eq!(before.payload["frame_complete"], true);
        }
    }

    #[tokio::test]
    async fn marauder_replay_reaches_durable_records_with_boot_clock_and_unknown_radio_fields() {
        for format in ["pcap", "serial"] {
            let fixture = Fixture::new();
            let path = fixture.0.join("marauder.capture");
            let bytes = crate::marauder_replay::tests::fixture_bytes();
            std::fs::write(
                &path,
                if format == "serial" {
                    [b"[BUF/BEGIN]".as_slice(), &bytes, b"[BUF/CLOSE]"].concat()
                } else {
                    bytes
                },
            )
            .unwrap();
            let (handle, actor) = spawn(fixture.path());
            let config = json!({"version":1,"receivers":[{"id":"awok-file","source":{"kind":"marauder_capture","path":path,"format":format}}]});
            let started = handle
                .request(Action::Start {
                    request_id: format.into(),
                    config,
                })
                .await
                .unwrap();
            let id = started["run_id"].as_str().unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let status = handle.request(Action::Status).await.unwrap();
                    if status["active"] == false {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            handle.request(Action::Shutdown).await.unwrap();
            actor.await.unwrap().unwrap();
            let reader = storage::Reader::open(&fixture.path()).unwrap();
            let records = reader.records(id, 0, 100).unwrap().records;
            let packets: Vec<_> = records
                .iter()
                .filter(|r| r.kind == "wifi_packet_observation")
                .collect();
            assert_eq!(packets.len(), 2);
            let value = &packets[0].payload;
            assert_eq!(value["mac"], "02:01:02:03:04:05");
            assert_eq!(value["ssid"], "fixture");
            assert_eq!(value["timestamp_micros32"], u32::MAX);
            assert_eq!(
                value["timestamp_clock"],
                "device_boot_micros32_wrap_or_reset_unknown"
            );
            assert_eq!(value["session_source"]["receiver_id"], "awok-file");
            assert_eq!(value["replay"], true);
            assert_eq!(value["firmware_version_verified"], false);
            for key in [
                "rssi",
                "rssi_dbm",
                "receive_channel",
                "channel",
                "gps",
                "receiver_location",
                "capture",
            ] {
                assert!(value[key].is_null(), "unexpected {key}");
            }
            assert!(value["detection"].is_object());
            assert_eq!(packets[1].payload["timestamp_micros32"], 7);
            assert!(packets[1].payload["mac"].is_null());
            assert!(packets[1].payload["detection"].is_null());
        }
    }

    #[tokio::test]
    async fn configured_gpsd_fix_is_retained_with_live_observations_and_shutdown_closes_it() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let version =
                json!({"class":"VERSION","release":"fixture","proto_major":16,"proto_minor":1});
            let fix = json!({"class":"TPV","device":"fixture-gps","mode":3,"time":chrono::Utc::now().to_rfc3339(),"lat":10.,"lon":20.,"altMSL":15.,"eph":2.});
            socket
                .write_all(format!("{version}\n{fix}\n").as_bytes())
                .await
                .unwrap();
            let mut commands = Vec::new();
            socket.read_to_end(&mut commands).await.unwrap();
            assert_eq!(commands, b"?WATCH={\"enable\":true,\"json\":true};\n");
        });
        let receiver = session_runtime::ReceiverTask::fixture(|tx, control| async move {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while control.external_location_evidence().is_none() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .map_err(io::Error::other)?;
            tx.send(crate::observation::ScanEvent::WiFi(
                airhound::scanner::WiFiEvent {
                    mac: [0x34; 6],
                    ssid: "fixture".try_into().unwrap(),
                    rssi: -50,
                    channel: 6,
                    frequency_mhz: Some(2437),
                    frame_type: airhound::scanner::FrameType::Beacon,
                    raw_ies: Default::default(),
                    security: Default::default(),
                },
            ))
            .await
            .map_err(io::Error::other)?;
            let mut state = control.subscribe();
            while !state.borrow().shutdown {
                state.changed().await.map_err(io::Error::other)?;
            }
            Ok(())
        });
        let fixture = Fixture::new();
        let writer = Writer::start(
            fixture.path(),
            RecordingMode::Live,
            json!({"location":{"kind":"gpsd","endpoint":endpoint,"device":"fixture-gps"}}),
            vec![
                ("fixture".into(), json!({})),
                (storage::output::SESSION_SOURCE.into(), json!({})),
            ],
        )
        .unwrap();
        let id = writer.run_id.clone();
        let active = launch(
            vec![receiver],
            FilterConfig::new(),
            writer,
            Some(crate::gpsd::Options {
                endpoint,
                device: Some("fixture-gps".into()),
                managed_executable: None,
            }),
        );
        let (handle, actor) = spawn_with_active(fixture.path(), Some(active));
        let reader = storage::Reader::open(&fixture.path()).unwrap();
        let observation = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(record) = reader
                    .records(&id, 0, 100)
                    .unwrap()
                    .records
                    .into_iter()
                    .find(|record| record.kind == "wifi_observation")
                {
                    break record.payload;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(observation["receiver_location"]["latitude"], 10.);
        assert_eq!(observation["receiver_location"]["device"], "fixture-gps");
        assert_eq!(
            observation["receiver_location"]["association"],
            "host_processing_time"
        );
        assert_eq!(observation["session_source"]["receiver_id"], "fixture");
        let status = handle.request(Action::Status).await.unwrap();
        assert_eq!(
            status["receiver_health"]["location"]["daemon"]["version"],
            "fixture"
        );
        assert_eq!(
            handle.request(Action::Finish(id.clone())).await.unwrap()["state"],
            "finished"
        );
        assert_eq!(reader.runs(1).unwrap()[0].state, "finished");
        handle.request(Action::Shutdown).await.unwrap();
        actor.await.unwrap().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn cancelled_pause_request_still_commits_and_resume_and_finish_drain() {
        let f = Fixture::new();
        let cleaned = Arc::new(AtomicBool::new(false));
        let cleanup = cleaned.clone();
        let (state_tx, mut state_rx) = mpsc::unbounded_channel();
        let receiver = session_runtime::ReceiverTask::fixture(move |tx, control| async move {
            let mut state = control.subscribe();
            loop {
                let current = *state.borrow_and_update();
                if current.shutdown {
                    break;
                }
                if current.enabled {
                    tx.send(crate::observation::ScanEvent::BleSourceStatus(
                        json!({"type":"fixture_observation","address":null}),
                    ))
                    .await
                    .map_err(io::Error::other)?;
                }
                state_tx.send(current.enabled).unwrap();
                state.changed().await.map_err(io::Error::other)?;
            }
            cleanup.store(true, Ordering::Relaxed);
            Ok(())
        });
        let (handle, actor, id) = f.actor(receiver);
        assert_eq!(state_rx.recv().await, Some(true));
        let (reply, cancelled) = oneshot::channel();
        handle
            .0
            .send(Request {
                action: Action::Pause(id.clone()),
                reply,
            })
            .await
            .unwrap();
        drop(cancelled);
        let status = handle.request(Action::Status).await.unwrap();
        assert_eq!(status["state"], "paused");
        assert_eq!(state_rx.recv().await, Some(false));
        let reader = storage::Reader::open(&f.path()).unwrap();
        assert_eq!(reader.runs(1).unwrap()[0].state, "paused");
        assert_eq!(
            handle.request(Action::Pause(id.clone())).await.unwrap()["state"],
            "paused"
        );
        assert_eq!(
            handle.request(Action::Resume(id.clone())).await.unwrap()["state"],
            "recording"
        );
        assert_eq!(state_rx.recv().await, Some(true));
        assert!(handle
            .request(Action::Finish("wrong-run".into()))
            .await
            .is_err());
        let finish = handle.request(Action::Finish(id.clone())).await.unwrap();
        assert_eq!(finish["state"], "finished");
        assert!(cleaned.load(Ordering::Relaxed));
        assert_eq!(reader.runs(1).unwrap()[0].state, "finished");
        assert_eq!(
            reader
                .records(&id, 0, 100)
                .unwrap()
                .records
                .iter()
                .filter(|r| r.kind == "fixture_observation")
                .count(),
            2
        );
        handle.request(Action::Shutdown).await.unwrap();
        actor.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn receiver_failure_is_durable_and_service_shutdown_reports_failure() {
        let f = Fixture::new();
        let receiver = session_runtime::ReceiverTask::fixture(|_tx, _control| async {
            Err(io::Error::other("fixture receiver disconnected"))
        });
        let (handle, actor, id) = f.actor(receiver);
        let finished = handle.request(Action::Finish(id)).await.unwrap();
        assert_eq!(finished["state"], "failed");
        assert_eq!(
            storage::Reader::open(&f.path()).unwrap().runs(1).unwrap()[0].state,
            "failed"
        );
        handle.request(Action::Shutdown).await.unwrap();
        assert!(actor.await.unwrap().is_err());
    }
    #[tokio::test]
    async fn terminal_storage_failure_retries_in_background_and_blocks_new_start() {
        let f = Fixture::new();
        let (release, wait) = oneshot::channel::<()>();
        let receiver = session_runtime::ReceiverTask::fixture(move |_tx, _control| async move {
            wait.await.map_err(io::Error::other)?;
            Err(io::Error::other("fixture receiver ended"))
        });
        let (handle, actor, id) = f.actor(receiver);
        // A deterministic terminal-write failure, without disk filling or a
        // long-lived read transaction that could grow the writer's WAL.
        {
            let connection = rusqlite::Connection::open(f.path()).unwrap();
            connection.execute_batch("CREATE TRIGGER refuse_terminal BEFORE UPDATE OF state ON runs WHEN NEW.state IN ('failed','interrupted') BEGIN SELECT RAISE(FAIL,'fixture terminal storage unavailable'); END;").unwrap();
        }
        release.send(()).unwrap();
        let failed = handle.request(Action::Finish(id.clone())).await.unwrap();
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["storage_recovery"]["pending"], true);
        let original_reason = failed["error"].as_str().unwrap().to_owned();
        let saved = storage::Reader::open(&f.path())
            .unwrap()
            .runs(1)
            .unwrap()
            .remove(0);
        assert_eq!(saved.state, "recording");
        assert!(saved.ended_at_us.is_none());
        let count = saved.record_count;
        let error = handle
            .request(Action::Start {
                request_id: "next-recording".into(),
                config: json!({}),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("storage recovery is pending"));
        // Status exposes the pending condition; the independent timer owns writes.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = handle.request(Action::Status).await.unwrap();
                assert_eq!(status["last"]["state"], "failed");
                if status["storage_recovery"]["error"].is_string() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        {
            let connection = rusqlite::Connection::open(f.path()).unwrap();
            connection
                .execute_batch("DROP TRIGGER refuse_terminal;")
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // Poll only a short-lived read-only SQLite query. No actor
                // Status/Finish request may drive this terminal repair.
                let saved = storage::Reader::open(&f.path()).unwrap().runs(1).unwrap();
                if saved[0].state == "failed" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let saved = storage::Reader::open(&f.path())
            .unwrap()
            .runs(1)
            .unwrap()
            .remove(0);
        assert_eq!(saved.state, "failed");
        assert!(saved.ended_at_us.is_some());
        assert_eq!(saved.record_count, count);
        let connection = rusqlite::Connection::open(f.path()).unwrap();
        let reason: String = connection
            .query_row(
                "SELECT reason FROM transitions WHERE run_id=?1 AND state='failed'",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, original_reason);
        drop(connection);
        let repeated = handle.request(Action::Finish(id)).await.unwrap();
        assert_eq!(repeated["state"], "failed");
        assert_eq!(repeated["storage_recovery"]["pending"], false);
        let replay_path = f.0.join("next.pcap");
        std::fs::write(&replay_path, crate::marauder_replay::tests::fixture_bytes()).unwrap();
        let config = json!({"version":1,"receivers":[{"id":"next-file","source":{"kind":"marauder_capture","path":replay_path,"format":"pcap"}}]});
        let next = handle
            .request(Action::Start {
                request_id: "next-recording".into(),
                config,
            })
            .await
            .unwrap();
        assert_eq!(next["active"], true);
        assert_ne!(next["run_id"], failed["run_id"]);
        let finished = handle
            .request(Action::Finish(next["run_id"].as_str().unwrap().to_owned()))
            .await
            .unwrap();
        assert_eq!(finished["state"], "finished");
        let history = storage::Reader::open(&f.path()).unwrap().runs(10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history
                .iter()
                .find(|r| r.id == failed["run_id"].as_str().unwrap())
                .unwrap()
                .state,
            "failed"
        );
        handle.request(Action::Shutdown).await.unwrap();
        actor.await.unwrap().unwrap();
    }
}
