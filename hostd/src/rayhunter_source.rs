//! Explicit Rayhunter subscription with optional owned ADB forwarding.
//! No modem or recording controls.
use crate::{
    control::{wait_running, ScanControl},
    health::{Phase, Scanner},
    observation::ScanEvent,
};
use airhound_hostd::rayhunter::{Reader, LINE_LIMIT};
use sha2::{Digest, Sha256};
use std::{io, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
    sync::mpsc,
    time::{timeout, Instant},
};

pub(crate) fn validate_decoder(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let m = std::fs::metadata(path)?;
    if !path.is_absolute()
        || !m.is_file()
        || m.len() > 64 * 1024 * 1024
        || m.permissions().mode() & 0o111 == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Rayhunter decoder must be a bounded executable regular file",
        ));
    }
    Ok(())
}

pub(crate) async fn decoder_hash(path: &std::path::Path) -> io::Result<String> {
    validate_decoder(path)?;
    let mut file = tokio::fs::File::open(path)
        .await?
        .take(64 * 1024 * 1024 + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 8192];
    let mut total = 0usize;
    loop {
        let size = file.read(&mut buffer).await?;
        if size == 0 {
            break;
        }
        total += size;
        if total > 64 * 1024 * 1024 {
            return Err(io::Error::other("Rayhunter decoder exceeds size bound"));
        }
        hash.update(&buffer[..size]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) async fn scan(
    endpoint: String,
    decoder: PathBuf,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    scan_inner(
        endpoint,
        decoder,
        None,
        tx,
        control,
        #[cfg(test)]
        None,
    )
    .await
}

pub(crate) async fn scan_adb(
    adb: airhound_hostd::session::RayhunterAdb,
    decoder: PathBuf,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    scan_inner(
        String::new(),
        decoder,
        Some(adb),
        tx,
        control,
        #[cfg(test)]
        None,
    )
    .await
}

async fn scan_inner(
    endpoint: String,
    decoder: PathBuf,
    adb: Option<airhound_hostd::session::RayhunterAdb>,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
    #[cfg(test)] ingress_witness: Option<mpsc::Sender<u64>>,
) -> io::Result<()> {
    let hash = decoder_hash(&decoder).await?;
    let adb_hash = match &adb {
        Some(config) => Some(decoder_hash(&config.executable).await?),
        None => None,
    };
    let mut state = control.subscribe();
    let origin = Instant::now();
    let mut connection = 0u64;
    let mut attempts = 0u8;
    let mut previous_subscription = None;
    loop {
        if !wait_running(&mut state, &tx).await {
            break;
        }
        connection = connection
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Rayhunter connection exhausted"))?;
        let current = decoder_hash(&decoder).await?;
        if current != hash {
            return Err(io::Error::other(
                "Rayhunter decoder changed during recording",
            ));
        }
        if state.borrow().shutdown || !state.borrow().enabled {
            continue;
        }
        let generation = state.borrow().generation;
        // A quick Pause/Resume may coalesce into one watch notification. The
        // generation boundary still proves that this subscription was interrupted.
        // Emit under the resumed generation so the filter does not discard it as
        // a stale paused event, and before any new helper record.
        if let Some((previous, previous_connection)) =
            previous_subscription.filter(|(previous, _)| *previous != generation)
        {
            let record = serde_json::json!({
                "type":"cellular_source_gap", "schema_version":1,
                "reason":"subscription_pause_resume", "deliberate":true,
                "previous_session_generation":previous, "session_generation":generation,
                "previous_connection_generation":previous_connection,
                "connection_generation":connection, "lost_records":null,
                "rf_loss":"unknown", "receiver_location":null,
                "host_received_at":chrono::Utc::now().to_rfc3339()
            });
            tokio::select! {
                result = tx.send(ScanEvent::Cellular { record, generation }) =>
                    if result.is_err() { break; },
                _ = state.changed() => continue,
            }
        }
        if state.borrow().shutdown
            || !state.borrow().enabled
            || state.borrow().generation != generation
        {
            continue;
        }
        control.set_health(
            Scanner::Cellular,
            Phase::Initializing,
            "rayhunter_connecting",
        );
        let forward = match &adb {
            Some(config) => Some(
                crate::adb_forward::Forward::open(
                    config.clone(),
                    adb_hash.clone().expect("managed ADB hash"),
                )
                .await?,
            ),
            None => None,
        };
        if state.borrow().shutdown
            || !state.borrow().enabled
            || state.borrow().generation != generation
        {
            if let Some(forward) = forward {
                forward.close().await?;
            }
            continue;
        }
        let selected_endpoint = forward
            .as_ref()
            .map_or(endpoint.as_str(), |f| f.endpoint.as_str());
        let mut child = match Command::new(&decoder)
            .args(["--endpoint", selected_endpoint])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                if let Some(forward) = forward {
                    forward.close().await?;
                }
                return Err(error);
            }
        };
        previous_subscription = Some((generation, connection));
        let mut input = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("missing helper stdout"))?,
        );
        let mut parser = Reader::default();
        let mut valid = false;
        let result: io::Result<bool> = async {
            loop {
                let mut bytes = Vec::new();
                let mut limited = (&mut input).take((LINE_LIMIT + 1) as u64);
                let read = timeout(Duration::from_secs(8), limited.read_until(b'\n', &mut bytes));
                tokio::pin!(read);
                let n = tokio::select! {
                    r = &mut read => r.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Rayhunter helper timeout"))??,
                    _ = state.changed() => return Ok(false),
                    _ = tx.closed() => return Ok(false),
                };
                if state.borrow().shutdown || !state.borrow().enabled || state.borrow().generation != generation {
                    return Ok(false);
                }
                if n == 0 { return Ok(true); }
                if bytes.len() > LINE_LIMIT || bytes.last() != Some(&b'\n') {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "Rayhunter IPC line bound/framing"));
                }
                let wall = chrono::Utc::now().to_rfc3339();
                let arrived = origin.elapsed().as_nanos().to_string();
                // Snapshot before parser work or any bounded-queue await. Later
                // GNSS fixes must not be paired with this earlier arrival time.
                let location = control.external_location_evidence().map(|mut evidence| {
                    evidence["association"] = "host_source_ingress".into();
                    evidence
                });
                let mut record = parser.accept(&bytes)?;
                record["host_received_at"] = wall.into();
                record["host_read_monotonic_ns"] = serde_json::json!(arrived);
                record["connection_generation"] = connection.into();
                record["decoder_sha256"] = hash.clone().into();
                record["location_time_basis"] = serde_json::json!("host_ingress_not_rf_capture");
                record["receiver_location"] = location.unwrap_or(serde_json::Value::Null);
                if let Some(forward) = &forward {
                    record["host_transport"] = forward.provenance.clone();
                }
                if !valid {
                    valid = true;
                    control.set_health(Scanner::Cellular, Phase::Running, "rayhunter_subscription");
                }
                // Per-invocation test witness: the snapshot is complete, but
                // this record has not entered the bounded output queue yet.
                #[cfg(test)]
                if let (Some(witness), Some(tick)) =
                    (&ingress_witness, record["event"]["tick"].as_u64())
                {
                    let _ = witness.try_send(tick);
                }
                let event = ScanEvent::Cellular { record, generation };
                // Bounded backpressure propagates to the raw server, whose sequence gaps
                // are explicit. Never silently drop raw chunks in the host adapter.
                tokio::select! {
                    r = tx.send(event) => if r.is_err() { return Ok(false); },
                    _ = state.changed() => return Ok(false),
                }
            }
        }.await;
        // Stop/Pause and protocol failure always reap this exact owned helper.
        // Cancellation additionally uses Tokio's kill-on-drop/reaper path.
        let _ = child.start_kill();
        let reaped = timeout(Duration::from_secs(3), child.wait()).await;
        if let Some(forward) = forward {
            forward.close().await?;
        }
        reaped.map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "Rayhunter helper reap timeout")
        })??;
        if state.borrow().shutdown || tx.is_closed() {
            break;
        }
        match result {
            Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(e),
            Ok(false) => {
                control.set_health(
                    Scanner::Cellular,
                    Phase::Paused,
                    "rayhunter_subscription_paused",
                );
                attempts = 0;
            }
            _ => {
                attempts += 1;
                if attempts > 3 {
                    return Err(io::Error::other("Rayhunter reconnect budget exhausted"));
                }
                control.set_health(
                    Scanner::Cellular,
                    Phase::Unavailable,
                    "rayhunter_reconnecting",
                );
                let record = serde_json::json!({"type":"cellular_source_gap","schema_version":1,"reason":"transport_disconnect","connection_generation":connection,"lost_records":null,"rf_loss":"unknown"});
                tokio::select! {r=tx.send(ScanEvent::Cellular{record,generation})=>if r.is_err(){break},_=state.changed()=>{}}
                tokio::select! {_=tokio::time::sleep(Duration::from_secs(u64::from(attempts)))=>{},_=state.changed()=>{}}
            }
        }
    }
    control.set_health(
        Scanner::Cellular,
        Phase::Stopped,
        "rayhunter_subscription_stopped",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::atomic::Ordering};
    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        script: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_interval(0.02)
        }
        fn with_interval(seconds: f64) -> Self {
            let root = std::env::temp_dir().join(format!(
                "airhound-cellular-fixture-{}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            let script = root.join("helper");
            std::fs::write(&script,r#"#!/usr/bin/env python3
import json,os,time
print(json.dumps({'ipc_version':1,'kind':'hello','stream':{'version':1,'replay':False,'format':'u32le_json_length+json+exact_device_read','stream_epoch':'fixture','next_sequence':0}}),flush=True)
tick=0
while True:
 print(json.dumps({'ipc_version':1,'kind':'status','pid':os.getpid(),'tick':tick}),flush=True)
 tick+=1
 time.sleep(INTERVAL)
"#.replace("INTERVAL", &seconds.to_string())).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self { root, script }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    async fn pid(rx: &mut mpsc::Receiver<ScanEvent>) -> (i32, u64) {
        timeout(Duration::from_secs(4), async {
            loop {
                let ScanEvent::Cellular { record, .. } = rx.recv().await.unwrap() else {
                    panic!("unexpected event")
                };
                if let Some(pid) = record["event"]["pid"].as_i64() {
                    return (
                        pid as i32,
                        record["connection_generation"].as_u64().unwrap(),
                    );
                }
            }
        })
        .await
        .unwrap()
    }
    async fn gone(pid: i32) {
        timeout(Duration::from_secs(3), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn ingress_fix_is_frozen_across_blocked_queue_and_later_filter_processing() {
        use crate::gpsd::tests::{feed, ready, tpv};
        use std::sync::{Arc, Mutex};
        for initially_available in [false, true] {
            // Slower than the old 120 ms sleep: the witness must synchronize
            // real ingress, rather than rely on helper scheduling speed.
            let f = Fixture::with_interval(0.2);
            let gps = ready();
            if initially_available {
                feed(&gps, tpv(chrono::Utc::now()));
            }
            let control = ScanControl::new(true);
            control.set_location_source(gps.clone());
            let (tx, mut rx) = mpsc::channel(1);
            let (witness_tx, mut witness_rx) = mpsc::channel(8);
            let task = tokio::spawn(scan_inner(
                "ws://127.0.0.1:9/raw-diag".into(),
                f.script.clone(),
                None,
                tx,
                control.clone(),
                Some(witness_tx),
            ));
            let (child, _) = pid(&mut rx).await;
            // pid() consumed tick 0. Tick 1 fills the only queue slot;
            // witnessing tick 2 proves its location snapshot predates the
            // GPS change, regardless of helper or coverage scheduling delays.
            timeout(Duration::from_secs(4), async {
                while witness_rx.recv().await.unwrap() != 2 {}
            })
            .await
            .unwrap();
            assert_eq!(rx.len(), 1, "tick 2 must face a full output queue");
            let changed_at = chrono::Utc::now();
            let mut later = tpv(changed_at);
            later["lat"] = 11.0.into();
            feed(&gps, later);
            let ScanEvent::Cellular { record: queued, .. } = rx.recv().await.unwrap() else {
                panic!()
            };
            let blocked = rx.recv().await.unwrap();
            let ScanEvent::Cellular { ref record, .. } = blocked else {
                panic!()
            };
            assert_eq!(
                record["event"]["tick"].as_u64().unwrap(),
                queued["event"]["tick"].as_u64().unwrap() + 1
            );
            let arrived =
                chrono::DateTime::parse_from_rfc3339(record["host_received_at"].as_str().unwrap())
                    .unwrap();
            assert!(
                arrived < changed_at,
                "must actually exercise a pre-change blocked event"
            );
            let snapshot = record["receiver_location"].clone();
            if initially_available {
                assert_eq!(snapshot["latitude"], 10.0);
                assert_eq!(snapshot["association"], "host_source_ingress");
                assert!(snapshot["fix_time"].is_string());
                assert!(snapshot["host_fix_age_ms"].is_u64());
                assert_eq!(snapshot["horizontal_accuracy_m"], 2.0);
                assert_eq!(snapshot["capture_time_synchronized"], false);
            } else {
                assert!(snapshot.is_null());
            }
            assert_eq!(
                control.external_location_evidence().unwrap()["latitude"],
                11.0
            );
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            gone(child).await;

            let (tx, rx) = mpsc::channel(1);
            tx.send(blocked).await.unwrap();
            drop(tx);
            let mut output = Vec::new();
            crate::filter_task(
                rx,
                Arc::new(Mutex::new(crate::FilterConfig::new())),
                Arc::new(crate::MacIndex::from_defaults()),
                std::time::Instant::now(),
                crate::WigleWriter::new(Vec::new()).unwrap(),
                &mut output,
                control,
            )
            .await
            .unwrap();
            let written = String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .find(|record| record["event"]["tick"].is_u64())
                .unwrap();
            assert_eq!(written["receiver_location"], snapshot);
        }
    }
    #[tokio::test]
    async fn pause_reaps_owned_helper_resume_new_generation_and_finish_no_orphan() {
        let f = Fixture::new();
        let control = ScanControl::new(true);
        let (tx, mut rx) = mpsc::channel(4);
        let task = tokio::spawn(scan(
            "ws://127.0.0.1:9/raw-diag".into(),
            f.script.clone(),
            tx,
            control.clone(),
        ));
        let (first, g1) = pid(&mut rx).await;
        control.store(false, Ordering::Relaxed);
        gone(first).await;
        while rx.try_recv().is_ok() {}
        control.store(true, Ordering::Relaxed);
        let (second, g2) = pid(&mut rx).await;
        assert_ne!(first, second);
        assert!(g2 > g1);
        control.shutdown();
        timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        gone(second).await;
    }
    #[tokio::test]
    async fn cancellation_with_full_queue_still_kills_owned_helper() {
        let f = Fixture::new();
        let (tx, mut rx) = mpsc::channel(1);
        let control = ScanControl::new(true);
        let task = tokio::spawn(scan(
            "ws://127.0.0.1:9/raw-diag".into(),
            f.script.clone(),
            tx,
            control,
        ));
        let (child, _) = pid(&mut rx).await;
        tokio::time::sleep(Duration::from_millis(70)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        gone(child).await;
    }
    #[tokio::test]
    async fn resume_refuses_changed_decoder_before_spawning_it() {
        let f = Fixture::new();
        let control = ScanControl::new(true);
        let (tx, mut rx) = mpsc::channel(4);
        let task = tokio::spawn(scan(
            "ws://127.0.0.1:9/raw-diag".into(),
            f.script.clone(),
            tx,
            control.clone(),
        ));
        let (first, _) = pid(&mut rx).await;
        control.store(false, Ordering::Relaxed);
        gone(first).await;
        std::fs::write(
            &f.script,
            "#!/usr/bin/env python3\nraise Exception('must not execute')\n",
        )
        .unwrap();
        control.store(true, Ordering::Relaxed);
        let error = timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("decoder changed"));
    }
}
