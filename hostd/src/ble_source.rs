//! Explicit external BLE source. Child process is separate from the MIT matcher.
use crate::{
    ble_packet::Reader,
    control::{wait_running, ScanControl},
    health::{Phase, Scanner},
    observation::ScanEvent,
};
use std::{
    io,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
mod child;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
    time::Instant,
};

#[derive(Clone, Debug)]
pub enum Source {
    Pcap(PathBuf),
    Sniffle {
        port: String,
        python: PathBuf,
        helper: PathBuf,
        module_path: Option<PathBuf>,
        channel: u8,
        coded: bool,
    },
}
#[derive(Default)]
struct Stats {
    packets: u64,
    host_drops: u64,
    generation: u64,
    reconnects: u64,
}
fn status(stats: &Stats, state: &str) -> ScanEvent {
    ScanEvent::BleSourceStatus(
        serde_json::json!({"type":"ble_source_status","schema_version":1,"state":state,"packets":stats.packets,"host_dropped_packets":stats.host_drops,"generation":stats.generation,"reconnect_attempts":stats.reconnects,"radio_loss":"unknown"}),
    )
}

async fn consume<R: AsyncRead + Unpin>(
    input: R,
    source_id: &str,
    replay: bool,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
    stats: &mut Stats,
) -> io::Result<bool> {
    consume_recovering(input, source_id, replay, tx, control, stats, None).await
}

async fn consume_recovering<R: AsyncRead + Unpin>(
    input: R,
    source_id: &str,
    replay: bool,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
    stats: &mut Stats,
    mut deadline: Option<Instant>,
) -> io::Result<bool> {
    let mut state = control.subscribe();
    if (state.borrow().shutdown || !state.borrow().enabled)
        && (!replay || !wait_running(&mut state, tx).await)
    {
        return Ok(false);
    }
    let deadline_future = tokio::time::sleep_until(
        deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86400)),
    );
    tokio::pin!(deadline_future);
    let header = tokio::time::timeout(Duration::from_secs(10), Reader::new(input));
    tokio::pin!(header);
    let mut reader = loop {
        tokio::select! {
            r=&mut header=>break r.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"BLE source header timeout"))??,
            _=state.changed()=>{if !replay || !wait_running(&mut state,tx).await{return Ok(false)}},
            _=tx.closed()=>return Ok(false),
            _=&mut deadline_future, if deadline.is_some()=>return Err(io::Error::new(io::ErrorKind::TimedOut, "sniffle_recovery_expired")),
        }
    };
    if (state.borrow().shutdown || !state.borrow().enabled)
        && (!replay || !wait_running(&mut state, tx).await)
    {
        return Ok(false);
    }
    control.set_health(
        Scanner::Ble,
        Phase::Running,
        if replay {
            "pcap_replay"
        } else {
            "sniffle_passive"
        },
    );
    control.set_receiver_availability("available");
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut sequence = 0u64;
    loop {
        let next = reader.next();
        tokio::pin!(next);
        let record = loop {
            tokio::select! {
                r=&mut next=>break r?,
                _=state.changed()=>{
                    if !replay || !wait_running(&mut state,tx).await { return Ok(false); }
                },
                _=tx.closed()=>return Ok(false),
                _=&mut deadline_future, if deadline.is_some()=>return Err(io::Error::new(io::ErrorKind::TimedOut, "sniffle_recovery_expired")),
                _=tick.tick()=>{let _=tx.try_send(status(stats,"running"));},
            }
        };
        let Some(mut packet) = record else {
            return Ok(true);
        };
        // A buffered record can become ready concurrently with Pause/Finish.
        // Recheck the current watch value rather than relying only on changed().
        if (state.borrow().shutdown || !state.borrow().enabled)
            && (!replay || !wait_running(&mut state, tx).await)
        {
            return Ok(false);
        }
        sequence += 1;
        stats.packets += 1;
        deadline = None;
        packet.source_id = source_id.into();
        packet.generation = stats.generation;
        packet.sequence = sequence;
        packet.replay = replay;
        packet.ingestion_generation = state.borrow().generation;
        if let Some(snapshot) = packet.advertisement.as_mut() {
            snapshot.receiver_id = Some(source_id.into());
        }
        let event = ScanEvent::BlePacket(Box::new(packet));
        if replay {
            let send = tx.send(event);
            tokio::pin!(send);
            loop {
                tokio::select! {r=&mut send=>{if r.is_err(){return Ok(false)}break;},_=state.changed()=>{if !wait_running(&mut state,tx).await{return Ok(false)}}}
            }
        } else if let Err(e) = tx.try_send(event) {
            if matches!(e, mpsc::error::TrySendError::Closed(_)) {
                return Ok(false);
            }
            stats.host_drops += 1;
        }
    }
}

// Drain bounded lines concurrently so diagnostics cannot block the PCAP stream.
// Only the strict identity message changes metadata; all other stderr is diagnostic.
#[cfg(test)]
async fn read_identity(input: impl AsyncRead + Unpin, control: ScanControl) {
    read_helper_messages(input, control, Arc::new(Mutex::new(None)), None).await;
}
async fn read_helper_messages(
    mut input: impl AsyncRead + Unpin,
    control: ScanControl,
    failure: Arc<Mutex<Option<HelperFailure>>>,
    #[cfg(test)] identified: Option<mpsc::UnboundedSender<()>>,
) {
    use tokio::io::AsyncReadExt;
    let mut chunk = [0u8; 1024];
    let mut line = Vec::new();
    let mut oversized = false;
    while let Ok(count) = input.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if !oversized {
                    if let Ok(firmware) =
                        airhound_hostd::discovery::identify::Firmware::parse(&line)
                    {
                        control.set_receiver_firmware(Some(firmware));
                        #[cfg(test)]
                        if let Some(identified) = &identified {
                            let _ = identified.send(());
                        }
                    } else if let Some(error) = HelperFailure::parse(&line) {
                        *failure.lock().expect("helper failure lock") = Some(error);
                    } else if !line.is_empty() {
                        log::debug!("Sniffle helper: {:?}", String::from_utf8_lossy(&line));
                    }
                }
                line.clear();
                oversized = false;
            } else if line.len() < 4096 && !oversized {
                line.push(*byte);
            } else {
                oversized = true;
                line.clear();
            }
        }
    }
}

#[derive(Clone)]
struct HelperFailure {
    code: &'static str,
    retryable: bool,
}
impl HelperFailure {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        if value["type"] != "sniffle_error" || value["schema_version"] != 1 {
            return None;
        }
        let (code, retryable) = match value["code"].as_str()? {
            "firmware_query_timeout" => ("firmware_query_timeout", true),
            "transport_decode_errors" => ("transport_decode_errors", true),
            "serial_transport_unavailable" => ("serial_transport_unavailable", true),
            "unsupported_firmware" => ("unsupported_firmware", false),
            "invalid_borrowed_lease" => ("invalid_borrowed_lease", false),
            "serial_permission_denied" => ("serial_permission_denied", false),
            "missing_python_dependency" => ("missing_python_dependency", false),
            "helper_configuration_or_protocol_error" => {
                ("helper_configuration_or_protocol_error", false)
            }
            _ => return None,
        };
        (value["retryable"].as_bool()? == retryable).then_some(Self { code, retryable })
    }
}
struct AbortTask(tokio::task::JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[derive(Default)]
struct Recovery {
    since: Option<Instant>,
    attempts: u32,
}
impl Recovery {
    fn failed(&mut self, received: bool) {
        if received {
            *self = Self::default();
        }
        self.since.get_or_insert_with(Instant::now);
        self.attempts = self.attempts.saturating_add(1);
    }
    fn deadline(&self) -> Option<Instant> {
        self.since.map(|time| time + Duration::from_secs(90))
    }
    fn expired(&self) -> bool {
        self.deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
    fn delay(&self) -> Duration {
        let backoff = Duration::from_secs(1 << self.attempts.saturating_sub(1).min(3));
        self.deadline().map_or(backoff, |deadline| {
            backoff.min(deadline.saturating_duration_since(Instant::now()))
        })
    }
}
async fn require_capabilities(
    python: &std::path::Path,
    helper: &std::path::Path,
    lease: Arc<airhound_hostd::serial_lease::Lease>,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
) -> io::Result<()> {
    let mut command = Command::new(python);
    command.arg(helper).arg("--capabilities");
    let mut child = child::Child::start(command, lease, false)?;
    if !matches!(
        tokio::time::timeout(Duration::from_secs(3), child.ready()).await,
        Ok(Ok(()))
    ) {
        child.finish().await?;
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Sniffle helper capability process could not start; review the helper path",
        ));
    }
    let mut stdout = child.stdout.take().expect("stdout available").take(4097);
    let mut stderr = child.stderr.take().expect("stderr available");
    let mut output = Vec::new();
    let mut state = control.subscribe();
    let result = {
        let read = async {
            let mut sink = tokio::io::sink();
            tokio::try_join!(
                stdout.read_to_end(&mut output),
                tokio::io::copy(&mut stderr, &mut sink)
            )
            .map(|_| ())
        };
        let read = tokio::time::timeout(Duration::from_secs(3), read);
        tokio::pin!(read);
        loop {
            tokio::select! {
                result = &mut read => break result.ok().and_then(Result::ok),
                _ = tx.closed() => break None,
                _ = state.changed() => if state.borrow().shutdown { break None; },
            }
        }
    };
    child.finish().await?;
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap_or_default();
    let capabilities = value["capabilities"].as_array();
    if result.is_none()
        || output.len() > 4096
        || value["type"] != "sniffle_helper_capabilities"
        || value["schema_version"] != 1
        || !capabilities.is_some_and(|values| {
            values.contains(&serde_json::json!("borrowed_serial_lease_v1"))
                && values.contains(&serde_json::json!("structured_errors_v1"))
        })
    {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "Sniffle helper lacks supervised recovery support; install a current managed helper bundle and review the saved setup helper path"));
    }
    Ok(())
}

#[cfg(test)]
struct TestHooks {
    retry: crate::test_support::TimerWitness,
    paused: mpsc::UnboundedSender<()>,
    identified: mpsc::UnboundedSender<()>,
}

pub async fn scan(
    source: Source,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    scan_inner(
        source,
        tx,
        control,
        #[cfg(test)]
        None,
    )
    .await
}

async fn scan_inner(
    source: Source,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
    #[cfg(test)] hooks: Option<TestHooks>,
) -> io::Result<()> {
    let mut state = control.subscribe();
    let mut stats = Stats::default();
    let (backend, id, info, configuration) = match &source {
        Source::Pcap(_) => (
            "ble_pcap",
            "ble-pcap".into(),
            "Classic DLT256 capture replay".into(),
            "recorded_capture",
        ),
        Source::Sniffle {
            port,
            channel,
            coded,
            ..
        } => (
            "sniffle",
            format!("sniffle:{port}"),
            format!(
                "Sniffle source, channel {channel}, coded={coded}; firmware not yet identified"
            ),
            "passive_fixed_channel",
        ),
    };
    control.set_receiver(crate::receiver::Receiver {
        producer_identity: None,
        external_identity: None,
        bridge_identity: None,
        bridge_status: None,
        backend,
        adapter_id: id,
        adapter_info: info,
        firmware: None,
        controller_address: None,
        selection: "explicit",
        availability: "initializing",
        scan_configuration: configuration,
        extended_advertising: Default::default(),
        coded_rx: Default::default(),
        controller_phy: None,
        capability_query: "raw_packet_metadata_only",
    });
    match source {
        Source::Pcap(path) => {
            // Replay is finite and never silently restarted from the beginning on pause.
            if !tokio::fs::metadata(&path).await?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "BLE replay requires a regular saved PCAP file",
                ));
            }
            let file = tokio::fs::File::open(path).await?;
            if !wait_running(&mut state, &tx).await {
                return Ok(());
            }
            let result = consume(file, "ble-pcap", true, &tx, &control, &mut stats).await;
            let completed = matches!(result, Ok(true));
            control.set_health(
                Scanner::Ble,
                if result.is_err() {
                    Phase::Unavailable
                } else {
                    Phase::Stopped
                },
                if result.is_err() {
                    "pcap_replay_failed"
                } else if completed {
                    "pcap_replay_finished"
                } else {
                    "pcap_replay_stopped"
                },
            );
            control.set_receiver_availability(if result.is_err() {
                "unavailable"
            } else {
                "stopped"
            });
            let _ = tx
                .send(status(
                    &stats,
                    if result.is_err() {
                        "failed"
                    } else if completed {
                        "finished"
                    } else {
                        "stopped"
                    },
                ))
                .await;
            result.map(|_| ())
        }
        Source::Sniffle {
            port,
            python,
            helper,
            module_path,
            channel,
            coded,
        } => {
            let lease = Arc::new(airhound_hostd::serial_lease::acquire(&port)?);
            let capability_result =
                require_capabilities(&python, &helper, lease.clone(), &tx, &control).await;
            if state.borrow().shutdown || tx.is_closed() {
                return Ok(());
            }
            capability_result?;
            let mut recovery = Recovery::default();
            loop {
                if !state.borrow().enabled {
                    control.set_receiver_firmware(None);
                    control.set_receiver_availability("paused");
                    control.set_health(Scanner::Ble, Phase::Paused, "sniffle_capture_paused");
                    #[cfg(test)]
                    if let Some(hooks) = &hooks {
                        let _ = hooks.paused.send(());
                    }
                    // A user pause suspends recovery. Resume gets a fresh outage
                    // budget, while the same configured endpoint stays reserved.
                    recovery = Recovery::default();
                }
                if !wait_running(&mut state, &tx).await {
                    break;
                }
                if recovery.expired() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "sniffle_recovery_expired",
                    ));
                }
                control.set_receiver_firmware(None);
                let endpoint = lease.matches(&port);
                let retry_missing =
                    matches!(&endpoint, Err(e) if e.kind() == io::ErrorKind::NotFound);
                if !retry_missing && !endpoint? {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "sniffle_endpoint_identity_changed",
                    ));
                }
                let before = stats.packets;
                let mut failure = None;
                if !retry_missing {
                    stats.generation += 1;
                    control.set_receiver_availability("initializing");
                    control.set_health(Scanner::Ble, Phase::Initializing, "sniffle_starting");
                    let mut command = Command::new(&python);
                    command
                        .arg(&helper)
                        .arg("--port")
                        .arg(&port)
                        .arg("--channel")
                        .arg(channel.to_string())
                        .arg("--lease-fd")
                        .arg(lease.raw_fd().to_string());
                    if let Some(path) = &module_path {
                        command.arg("--module-path").arg(path);
                    }
                    if coded {
                        command.arg("--coded");
                    }
                    let mut child = child::Child::start(command, lease.clone(), true)?;
                    let startup = tokio::select! {
                        result = tokio::time::timeout(Duration::from_secs(3), child.ready()) => Some(result),
                        _ = state.changed() => None,
                        _ = tx.closed() => None,
                    };
                    if !matches!(startup, Some(Ok(Ok(())))) {
                        child.finish().await?;
                        if state.borrow().shutdown || tx.is_closed() {
                            break;
                        }
                        if !state.borrow().enabled {
                            continue;
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "Sniffle helper could not start; review Python and helper paths",
                        ));
                    }
                    let messages = Arc::new(Mutex::new(None));
                    let output = child.stderr.take().expect("stderr available");
                    let mut identity = AbortTask(tokio::spawn(read_helper_messages(
                        output,
                        control.clone(),
                        messages.clone(),
                        #[cfg(test)]
                        hooks.as_ref().map(|hooks| hooks.identified.clone()),
                    )));
                    let result = consume_recovering(
                        child.stdout.take().expect("stdout available"),
                        &format!("sniffle:{port}"),
                        false,
                        &tx,
                        &control,
                        &mut stats,
                        recovery.deadline(),
                    )
                    .await;
                    child.finish().await?;
                    if tokio::time::timeout(Duration::from_secs(1), &mut identity.0)
                        .await
                        .is_err()
                    {
                        identity.0.abort();
                    }
                    failure = messages.lock().expect("helper failure lock").clone();
                    // Malformed PCAP/configuration is terminal. Transport EOF and
                    // header timeout may recover; never include input bytes in errors.
                    if matches!(&result, Err(error) if error.kind() == io::ErrorKind::InvalidData) {
                        failure = Some(HelperFailure {
                            code: "invalid_pcap",
                            retryable: false,
                        });
                    }
                }
                control.set_receiver_firmware(None);
                if state.borrow().shutdown || tx.is_closed() {
                    break;
                }
                if !state.borrow().enabled {
                    continue;
                }
                if let Some(error) = failure.filter(|error| !error.retryable) {
                    control.set_receiver_availability("unavailable");
                    control.set_health(Scanner::Ble, Phase::Unavailable, error.code);
                    return Err(io::Error::new(io::ErrorKind::InvalidData, error.code));
                }
                recovery.failed(stats.packets > before);
                stats.reconnects += 1;
                control.set_receiver_availability("recovering");
                control.set_health(Scanner::Ble, Phase::Unavailable, "sniffle_recovering");
                let _ = tx.try_send(status(&stats, "reconnecting"));
                if recovery.expired() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "sniffle_recovery_expired",
                    ));
                }
                let retry = tokio::time::sleep(recovery.delay());
                #[cfg(test)]
                let retry = async {
                    if let Some(hooks) = &hooks {
                        hooks
                            .retry
                            .clone()
                            .observe(retry.deadline(), retry, |_| true)
                            .await;
                    } else {
                        retry.await;
                    }
                };
                tokio::select! {
                    _=retry=>{},
                    _=state.changed()=>{},
                    _=tx.closed()=>break,
                }
            }
            control.set_receiver_firmware(None);
            control.set_receiver_availability("stopped");
            control.set_health(Scanner::Ble, Phase::Stopped, "sniffle_source_stopped");
            let _ = tx.try_send(status(&stats, "stopped"));
            Ok(())
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::ble_packet::tests::{frame, pcap};
    use tokio::io::AsyncWriteExt;
    struct Pty {
        _master: std::fs::File,
        _slave: std::fs::File,
        port: String,
    }
    impl Pty {
        fn new() -> Self {
            use std::os::fd::FromRawFd;
            let (mut master, mut slave) = (0, 0);
            let mut name = [0i8; 1024];
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        name.as_mut_ptr(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            Self {
                _master: unsafe { std::fs::File::from_raw_fd(master) },
                _slave: unsafe { std::fs::File::from_raw_fd(slave) },
                port: unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                    .to_str()
                    .unwrap()
                    .to_owned(),
            }
        }
    }
    #[tokio::test]
    async fn legacy_helper_fails_actionably_before_transport_open() {
        let pty = Pty::new();
        let (tx, _rx) = mpsc::channel(8);
        let result = scan(
            Source::Sniffle {
                port: pty.port.clone(),
                python: "/usr/bin/false".into(),
                helper: "unused".into(),
                module_path: None,
                channel: 37,
                coded: false,
            },
            tx,
            ScanControl::new(true),
        )
        .await
        .unwrap_err();
        assert_eq!(result.kind(), io::ErrorKind::Unsupported);
        assert!(result
            .to_string()
            .contains("review the saved setup helper path"));
        assert!(airhound_hostd::serial_lease::acquire(&pty.port).is_ok());
    }
    #[tokio::test]
    async fn ordinary_outage_pause_and_finish_preserve_one_endpoint_owner() {
        crate::test_support::manual_time(async {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let pty = Pty::new();
            let root = std::path::PathBuf::from("/tmp").join(format!(
                "ah-recovery-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            let events = tokio::net::UnixListener::bind(root.join("events")).unwrap();
            let port = root.join("configured-port");
            std::os::unix::fs::symlink(&pty.port, &port).unwrap();
            let helper = root.join("fake.py");
            let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/sniffle-source.py");
            let bytes = pcap(&[frame(0, &[])]).iter().map(|v| format!("{v:02x}")).collect::<String>();
            std::fs::write(&helper, format!(r#"import argparse,importlib.util,json,sys,socket
spec=importlib.util.spec_from_file_location('helper',{});h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
p=argparse.ArgumentParser();p.add_argument('--capabilities',action='store_true');p.add_argument('--port');p.add_argument('--lease-fd',type=int);p.add_argument('--channel');a=p.parse_args()
if a.capabilities: print(json.dumps(h.CAPABILITIES),flush=True);sys.exit(0)
commands=socket.socket(socket.AF_UNIX);commands.connect({})
with h.borrowed_port_lease(a.port,a.lease_fd):
 print(json.dumps(dict(type='sniffle_identity',schema_version=1,major=1,minor=11,patch=0,api_level=0)),file=sys.stderr,flush=True)
 sys.stdout.buffer.write(bytes.fromhex('{}'));sys.stdout.buffer.flush()
 assert commands.makefile('rb').readline(64)==b'disconnect\n'
"#, serde_json::to_string(script.to_str().unwrap()).unwrap(), serde_json::to_string(root.join("events").to_str().unwrap()).unwrap(), bytes)).unwrap();
            let (tx, mut rx) = mpsc::channel(100);
            let control = ScanControl::new(true);
            let copy = control.clone();
            let source = Source::Sniffle {
                port: port.to_str().unwrap().into(), python: "python3".into(), helper,
                module_path: None, channel: 37, coded: false,
            };
            let (retry, mut retries) = crate::test_support::TimerWitness::channel();
            let (paused, mut paused_events) = mpsc::unbounded_channel();
            let (identified, mut identities) = mpsc::unbounded_channel();
            let (done, result) = tokio::sync::oneshot::channel();
            let task = AbortTask(tokio::spawn(async move {
                let _ = done.send(scan_inner(source, tx, copy, Some(TestHooks { retry, paused, identified })).await);
            }));
            async fn packet(rx: &mut mpsc::Receiver<ScanEvent>) -> u64 {
                loop {
                    if let ScanEvent::BlePacket(packet) = rx.recv().await.expect("source ended before packet") {
                        return packet.generation;
                    }
                }
            }
            let generation = packet(&mut rx).await;
            identities.recv().await.expect("initial helper identity");
            assert_eq!(control.health_message()["receiver"]["firmware"]["minor"], 11);
            let (mut child_commands, _) = events.accept().await.unwrap();
            std::fs::remove_file(&port).unwrap();
            child_commands.write_all(b"disconnect\n").await.unwrap();
            // Each advance first proves the actual retry timer is registered
            // and polled before its deadline. No real outage duration is used.
            let outage_start = Instant::now();
            for _ in 0..5 {
                crate::test_support::advance_registered_timer(&mut retries).await;
                assert!(airhound_hostd::serial_lease::acquire(&pty.port).is_err());
            }
            assert!(Instant::now() - outage_start >= Duration::from_secs(20)); // virtual clock
            assert!(control.health_message()["receiver"].get("firmware").is_none());
            crate::test_support::advance_registered_timer_after(&mut retries, || {
                std::os::unix::fs::symlink(&pty.port, &port).unwrap();
            }).await;
            assert!(packet(&mut rx).await > generation);
            let (_next_child, _) = events.accept().await.unwrap();
            identities.recv().await.expect("new helper identity");
            assert_eq!(control.health_message()["receiver"]["firmware"]["minor"], 11);
            control.store(false, Ordering::SeqCst);
            paused_events.recv().await.expect("source ended before pause acknowledgement");
            assert_eq!(control.health().ble.state, Phase::Paused);
            assert!(airhound_hostd::serial_lease::acquire(&pty.port).is_err());
            control.shutdown();
            result.await.unwrap().unwrap();
            drop(task);
            assert!(airhound_hostd::serial_lease::acquire(&pty.port).is_ok());
            drop(events);
            std::fs::remove_dir_all(root).unwrap();
        }).await;
    }
    #[tokio::test]
    async fn already_paused_or_finished_buffered_input_never_publishes_running() {
        for shutdown in [false, true] {
            let control = ScanControl::new(true);
            if shutdown {
                control.shutdown();
            } else {
                control.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            let (tx, mut rx) = mpsc::channel(10);
            let mut stats = Stats::default();
            let bytes = pcap(&[frame(0, &[])]);
            assert!(!consume_recovering(
                bytes.as_slice(),
                "fixture",
                false,
                &tx,
                &control,
                &mut stats,
                None
            )
            .await
            .unwrap());
            assert_eq!(stats.packets, 0);
            assert!(rx.try_recv().is_err());
            assert_ne!(control.health().ble.state, Phase::Running);
        }
    }
    #[tokio::test(start_paused = true)]
    async fn outage_budget_tolerates_ordinary_reenumeration_but_not_headers_alone() {
        let mut recovery = Recovery::default();
        recovery.failed(false);
        assert_eq!(recovery.delay(), Duration::from_secs(1));
        tokio::time::advance(Duration::from_secs(25)).await;
        recovery.failed(false);
        assert!(!recovery.expired());
        assert_eq!(recovery.delay(), Duration::from_secs(2));
        tokio::time::advance(Duration::from_secs(65)).await;
        assert!(recovery.expired());
        recovery.failed(true);
        assert!(
            !recovery.expired(),
            "only valid reception resets outage budget"
        );
        assert_eq!(recovery.delay(), Duration::from_secs(1));
        let (mut writer, reader) = tokio::io::duplex(256);
        writer.write_all(&pcap(&[])).await.unwrap();
        let (tx, _rx) = mpsc::channel(20);
        let control = ScanControl::new(true);
        let result = consume_recovering(
            reader,
            "fixture",
            false,
            &tx,
            &control,
            &mut Stats::default(),
            Some(Instant::now() + Duration::from_secs(30)),
        )
        .await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
    #[test]
    fn helper_errors_are_bounded_codes_with_fixed_recovery_meaning() {
        let mismatch = br#"{"type":"sniffle_error","schema_version":1,"code":"unsupported_firmware","retryable":true}"#;
        assert!(HelperFailure::parse(mismatch).is_none());
        let valid = br#"{"type":"sniffle_error","schema_version":1,"code":"unsupported_firmware","retryable":false}"#;
        assert!(!HelperFailure::parse(valid).unwrap().retryable);
    }
    #[tokio::test]
    async fn firmware_identity_uses_bounded_valid_messages_and_clears_on_generation_change() {
        let c = ScanControl::new(true);
        c.set_receiver(crate::receiver::Receiver {
            producer_identity: None,
            external_identity: None,
            bridge_identity: None,
            bridge_status: None,
            backend: "sniffle",
            adapter_id: "sniffle:/dev/test".into(),
            adapter_info: "unidentified".into(),
            firmware: None,
            controller_address: None,
            selection: "explicit",
            availability: "initializing",
            scan_configuration: "passive_fixed_channel",
            extended_advertising: Default::default(),
            coded_rx: Default::default(),
            controller_phy: None,
            capability_query: "raw_packet_metadata_only",
        });
        assert!(c.health_message()["receiver"].get("firmware").is_none());
        let mut bytes = vec![b'x'; 5000];
        bytes.push(b'\n');
        bytes.extend_from_slice(b"not identity\n{}\n");
        bytes.extend_from_slice(br#"{"type":"sniffle_identity","schema_version":1,"major":1,"minor":11,"patch":0,"api_level":0}"#);
        bytes.push(b'\n');
        read_identity(bytes.as_slice(), c.clone()).await;
        let receiver = &c.health_message()["receiver"];
        assert_eq!(receiver["firmware"]["minor"], 11);
        assert!(receiver["adapter_info"]
            .as_str()
            .unwrap()
            .contains("reported by firmware"));
        c.set_receiver_firmware(None);
        assert!(c.health_message()["receiver"].get("firmware").is_none());
        assert!(!c.health_message()["receiver"]["adapter_info"]
            .as_str()
            .unwrap()
            .contains("1.11.0"));
    }
    #[tokio::test]
    async fn stream_pressure_is_counted_but_replay_is_lossless() {
        let bytes = pcap(&vec![frame(0, &[]); 5]);
        let (tx, mut rx) = mpsc::channel(2);
        let c = ScanControl::new(true);
        let mut stats = Stats::default();
        assert!(
            consume(bytes.as_slice(), "test", false, &tx, &c, &mut stats)
                .await
                .unwrap()
        );
        assert_eq!(stats.packets, 5);
        assert!(stats.host_drops >= 3);
        while rx.try_recv().is_ok() {}
        let producer = async {
            let mut stats = Stats::default();
            assert!(consume(bytes.as_slice(), "test", true, &tx, &c, &mut stats)
                .await
                .unwrap());
            stats
        };
        let consumer = async {
            let mut packets = 0;
            while packets < 5 {
                if let Some(ScanEvent::BlePacket(p)) = rx.recv().await {
                    packets += 1;
                    assert!(p.replay);
                    assert_eq!(p.sequence, packets);
                    assert_eq!(p.source_id, "test");
                }
            }
        };
        let (stats, ()) = tokio::join!(producer, consumer);
        assert_eq!(stats.host_drops, 0);
    }
    #[tokio::test(start_paused = true)]
    async fn partial_record_survives_status_tick() {
        let (mut writer, reader) = tokio::io::duplex(256);
        let bytes = pcap(&[frame(0, &[])]);
        let (tx, mut rx) = mpsc::channel(10);
        let c = ScanControl::new(true);
        let mut stats = Stats::default();
        let producer = async {
            consume(reader, "test", false, &tx, &c, &mut stats)
                .await
                .unwrap()
        };
        let feed = async {
            writer.write_all(&bytes[..30]).await.unwrap();
            tokio::time::sleep(Duration::from_secs(6)).await;
            writer.write_all(&bytes[30..]).await.unwrap();
            writer.shutdown().await.unwrap();
        };
        let (eof, ()) = tokio::join!(producer, feed);
        assert!(eof);
        assert_eq!(stats.packets, 1);
        let mut count = 0;
        while let Ok(e) = rx.try_recv() {
            if matches!(e, ScanEvent::BlePacket(_)) {
                count += 1
            }
        }
        assert_eq!(count, 1);
    }
    #[tokio::test(start_paused = true)]
    async fn header_timeout_and_closed_output_are_bounded() {
        let (_w, r) = tokio::io::duplex(128);
        let (tx, _rx) = mpsc::channel(1);
        let c = ScanControl::new(true);
        let mut stats = Stats::default();
        assert_eq!(
            consume(r, "test", false, &tx, &c, &mut stats)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let (_w, r) = tokio::io::duplex(128);
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        assert!(!consume(r, "test", false, &tx, &c, &mut stats)
            .await
            .unwrap());
    }
    async fn pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                future.as_mut().poll(cx).is_pending()
            ))
            .await
        );
    }
    #[tokio::test]
    async fn pause_and_resume_replay_without_restarting_or_losing_partial_record() {
        crate::test_support::manual_time(async {
            let (mut w, r) = tokio::io::duplex(256);
            let bytes = pcap(&[frame(0, &[])]);
            let (tx, mut rx) = mpsc::channel(8);
            let c = ScanControl::new(true);
            let mut stats = Stats::default();
            {
                let mut reader = Box::pin(consume(r, "test", true, &tx, &c, &mut stats));
                w.write_all(&bytes[..30]).await.unwrap();
                pending(reader.as_mut()).await; // consumed header and partial record; waiting for input
                assert_eq!(c.health().ble.state, Phase::Running);
                c.store(false, std::sync::atomic::Ordering::Relaxed);
                pending(reader.as_mut()).await; // observes Pause, then waits for Resume
                c.store(true, std::sync::atomic::Ordering::Relaxed);
                w.write_all(&bytes[30..]).await.unwrap();
                w.shutdown().await.unwrap();
                assert!(reader.await.unwrap());
            }
            assert_eq!(stats.packets, 1);
            assert!(std::iter::from_fn(|| rx.try_recv().ok())
                .any(|e| matches!(e, ScanEvent::BlePacket(_))));
        })
        .await;
    }
    #[tokio::test]
    async fn live_pause_and_shutdown_cancel_pending_reads() {
        crate::test_support::manual_time(async {
            for shutdown in [false, true] {
                let (mut w, r) = tokio::io::duplex(256);
                let bytes = pcap(&[]);
                let (tx, _rx) = mpsc::channel(8);
                let c = ScanControl::new(true);
                let mut stats = Stats::default();
                let mut reader = Box::pin(consume(r, "test", false, &tx, &c, &mut stats));
                w.write_all(&bytes).await.unwrap();
                pending(reader.as_mut()).await;
                assert_eq!(c.health().ble.state, Phase::Running);
                if shutdown {
                    c.shutdown()
                } else {
                    c.store(false, std::sync::atomic::Ordering::Relaxed)
                }
                assert!(!reader.await.unwrap());
            }
        })
        .await;
    }
    #[tokio::test]
    async fn explicit_file_failures_and_replay_completion() {
        let (tx, mut rx) = mpsc::channel(8);
        let c = ScanControl::new(true);
        assert!(scan(
            Source::Pcap("/no-such-airhound-pcap".into()),
            tx.clone(),
            c.clone()
        )
        .await
        .is_err());
        assert_eq!(
            scan(Source::Pcap(std::env::temp_dir()), tx.clone(), c.clone())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let path =
            std::env::temp_dir().join(format!("airhound-ble-replay-{}.pcap", std::process::id()));
        std::fs::write(&path, pcap(&[frame(0, &[])])).unwrap();
        scan(Source::Pcap(path.clone()), tx.clone(), c.clone())
            .await
            .unwrap();
        assert_eq!(c.health().ble.state, Phase::Stopped);
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, ScanEvent::BlePacket(_)))
        );
        std::fs::write(&path, b"bad pcap").unwrap();
        assert!(scan(Source::Pcap(path.clone()), tx, c).await.is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn missing_initial_endpoint_fails_without_starting_or_native_fallback() {
        let (tx, mut rx) = mpsc::channel(32);
        let c = ScanControl::new(true);
        let source = Source::Sniffle {
            port: "test".into(),
            python: "/usr/bin/false".into(),
            helper: "unused".into(),
            module_path: Some("unused".into()),
            channel: 39,
            coded: true,
        };
        assert!(scan(source, tx, c.clone()).await.is_err());
        assert!(c.health_message()["receiver"].get("firmware").is_none());
        assert!(!c.health_message()["receiver"]["adapter_info"]
            .as_str()
            .unwrap()
            .contains("1.11.0"));
        let messages: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(messages.is_empty());
    }
}
