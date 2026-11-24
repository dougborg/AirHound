//! Concurrent receiver ownership, bounded ingress, and source-tagged output.
use crate::{
    control::ScanControl,
    health::{Phase, Scanner},
    observation::ScanEvent,
    *,
};
use airhound_hostd::session::{SessionConfig, Source};
use std::{future::Future, pin::Pin, time::Duration};
use tokio::task::JoinSet;

const LINE_LIMIT: usize = 128 * 1024;
type ScanFuture = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;
type Start = Box<dyn FnOnce(mpsc::Sender<ScanEvent>, ScanControl) -> ScanFuture + Send>;

pub(crate) struct ReceiverTask {
    id: String,
    scanner: Scanner,
    assignment: serde_json::Value,
    shutdown_budget: Duration,
    #[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
    needs_macos_location: bool,
    start: Start,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl ReceiverTask {
    pub(crate) fn fixture<F, Fut>(start: F) -> Self
    where
        F: FnOnce(mpsc::Sender<ScanEvent>, ScanControl) -> Fut + Send + 'static,
        Fut: Future<Output = io::Result<()>> + Send + 'static,
    {
        Self {
            id: "fixture".into(),
            scanner: Scanner::Ble,
            assignment: serde_json::json!({"mode":"replay"}),
            shutdown_budget: Duration::from_millis(50),
            needs_macos_location: false,
            start: Box::new(move |tx, control| Box::pin(start(tx, control))),
        }
    }
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct EventSource {
    session_id: String,
    pub(crate) receiver_id: String,
    /// Host ingress order within this source, not a device capture sequence.
    ingress_sequence: u64,
}

pub(crate) fn load(path: &std::path::Path) -> io::Result<SessionConfig> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session configuration must be a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session file exceeds 64 KiB",
        ));
    }
    SessionConfig::from_json(serde_json::from_slice(&bytes).map_err(io::Error::other)?)?.resolve()
}

/// The same load/prepare preflight used by a real session, without invoking any
/// source future. File/USB permissions, firmware and RF capabilities are unverified.
pub(crate) async fn preview(plan: SessionConfig) -> io::Result<serde_json::Value> {
    let plan = plan.resolve()?;
    let tasks = prepare(plan.clone()).await?;
    let assignments: Vec<_> = tasks
        .iter()
        .map(|task| {
            serde_json::json!({
                "receiver_id": task.id,
                "requested_assignment": task.assignment,
                "shutdown_budget_ms": task.shutdown_budget.as_millis(),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "type": "session_validation", "schema_version": 1, "valid": true,
        "validation_scope": "configuration_and_local_resource_metadata",
        "readiness": "unverified", "resolved_config": plan,
        "receivers": assignments,
        "location": plan.location,
        "location_readiness": "not_connected",
    }))
}

pub(crate) async fn write_preview(
    path: &std::path::Path,
    output: &mut impl Write,
) -> io::Result<()> {
    let result = async { preview(load(path)?).await }.await;
    match result {
        Ok(value) => observation::write_json(output, &value),
        Err(error) => {
            observation::write_json(
                output,
                &serde_json::json!({
                    "type": "session_validation", "schema_version": 1, "valid": false,
                    "validation_scope": "configuration_and_local_resource_metadata",
                    "readiness": "unverified",
                    "error": {"code": "invalid_session", "message": error.to_string()},
                }),
            )?;
            Err(error)
        }
    }
}

pub(crate) async fn prepare(plan: SessionConfig) -> io::Result<Vec<ReceiverTask>> {
    let plan = plan.resolve()?;
    crate::session_resources::validate(&plan)?;
    let mut tasks = Vec::new();
    for receiver in plan.receivers {
        match &receiver.source {
            Source::Rayhunter { decoder, .. } => {
                crate::rayhunter_source::validate_decoder(decoder)?
            }
            Source::NativeBle { adapter } => {
                crate::receiver::validate_selection(adapter.as_deref(), cfg!(target_os = "macos"))
                    .map_err(io::Error::other)?
            }
            Source::NativeWifi {} if !cfg!(target_os = "macos") => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "native_wifi requires macOS; Linux uses a configured monitor interface",
                ))
            }
            Source::LinuxWifi { .. } if !cfg!(target_os = "linux") => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "linux_wifi requires Linux",
                ))
            }
            _ => {}
        }
        let source = receiver.source;
        tasks.push(ReceiverTask {
            id: receiver.id,
            scanner: if matches!(source, Source::Rayhunter { .. }) {
                Scanner::Cellular
            } else if source.is_ble() {
                Scanner::Ble
            } else {
                Scanner::Wifi
            },
            needs_macos_location: matches!(source, Source::NativeWifi {}),
            assignment: source.assignment(),
            shutdown_budget: source.shutdown_budget(),
            start: Box::new(move |tx, control| Box::pin(start_source(source, tx, control))),
        });
    }
    Ok(tasks)
}

async fn start_source(
    source: Source,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    match source {
        Source::Rayhunter { endpoint, decoder } => {
            crate::rayhunter_source::scan(endpoint, decoder, tx, control).await
        }
        Source::NativeBle { adapter } => {
            let Some((adapter, receiver)) =
                ble::initialize(receiver::selected(adapter.as_deref()), &control, &tx).await?
            else {
                return Ok(());
            };
            control.set_receiver(receiver);
            ble::scan_loop(adapter, tx, control)
                .await
                .map_err(io::Error::other)
        }
        Source::NativeWifi {} => {
            #[cfg(target_os = "macos")]
            {
                wifi_macos::scan_loop(tx, control)
                    .await
                    .map_err(io::Error::other)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (tx, control);
                Err(io::Error::other("native WiFi unavailable"))
            }
        }
        Source::FlipperMarauder { port } => crate::flipper_source::scan(port, tx, control).await,
        Source::AirhoundSerial { port, baud } => {
            crate::serial_source::scan(&port, baud, tx, control).await
        }
        Source::LinuxWifi { interface } => {
            #[cfg(target_os = "linux")]
            {
                wifi_linux::scan_loop(&interface, tx, control)
                    .await
                    .map_err(io::Error::other)
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (interface, tx, control);
                Err(io::Error::other("Linux monitor capture unavailable"))
            }
        }
        Source::Sniffle {
            port,
            python,
            helper,
            module_path,
            channel,
            coded,
        } => {
            ble_source::scan(
                ble_source::Source::Sniffle {
                    port,
                    python,
                    helper,
                    module_path,
                    channel: channel.expect("resolved plan"),
                    coded,
                },
                tx,
                control,
            )
            .await
        }
        Source::WifiPcap { path } => wifi_pcap::scan(path, tx, control).await,
        Source::MarauderCapture { path, format } => {
            crate::marauder_replay::scan(path, format, tx, control).await
        }
        Source::BlePcap { path } => {
            ble_source::scan(ble_source::Source::Pcap(path), tx, control).await
        }
        Source::ExternalWifi {
            usb_selector,
            executable,
            firmware,
            channels,
            dwell_ms,
        } => {
            external_scanner::scan_loop(
                external_scanner::Options {
                    usb_selector,
                    executable,
                    firmware,
                    channels,
                    dwell_ms,
                    #[cfg(test)]
                    fixture_script: None,
                },
                tx,
                control,
            )
            .await
        }
    }
}

pub(crate) async fn run<W, O, C, CF, S>(
    receivers: Vec<ReceiverTask>,
    config: FilterConfig,
    writer: WigleWriter<W>,
    output: O,
    gps: Option<gpsd::Options>,
    commands: C,
    shutdown: S,
) -> io::Result<()>
where
    W: Write + Send + 'static,
    O: Write + Send + 'static,
    C: FnOnce(Arc<Mutex<FilterConfig>>, Instant, ScanControl) -> CF,
    CF: Future<Output = ()> + Send + 'static,
    S: Future<Output = io::Result<()>>,
{
    run_controlled(
        receivers,
        (config, ScanControl::new(true)),
        writer,
        output,
        gps,
        commands,
        shutdown,
    )
    .await
}

/// The caller owns session control independently of any client connection.
pub(crate) async fn run_controlled<W, O, C, CF, S>(
    receivers: Vec<ReceiverTask>,
    state: (FilterConfig, ScanControl),
    writer: WigleWriter<W>,
    mut output: O,
    gps: Option<gpsd::Options>,
    commands: C,
    shutdown: S,
) -> io::Result<()>
where
    W: Write + Send + 'static,
    O: Write + Send + 'static,
    C: FnOnce(Arc<Mutex<FilterConfig>>, Instant, ScanControl) -> CF,
    CF: Future<Output = ()> + Send + 'static,
    S: Future<Output = io::Result<()>>,
{
    let (config, control) = state;
    let alert_policy = config.alert_policy;
    let config = Arc::new(Mutex::new(config));
    let start = Instant::now();
    let session_id = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos()
    );
    observation::write_json(
        &mut output,
        &serde_json::json!({"type":"session_started","schema_version":1,"session_id":session_id,"receiver_count":receivers.len(),"alert_policy":alert_policy}),
    )?;
    let location_task = gps.map(|options| {
        let source = gpsd::Source::new(options.device.clone());
        control.set_location_source(source.clone());
        tokio::spawn(gpsd::run(options, source, control.subscribe()))
    });
    #[cfg(all(target_os = "macos", not(test)))]
    if receivers
        .iter()
        .any(|receiver| receiver.needs_macos_location)
    {
        location_macos::start();
    }
    let budget = receivers
        .iter()
        .map(|r| r.shutdown_budget)
        .max()
        .unwrap_or(Duration::from_secs(5));
    let (tx, rx) = mpsc::channel(32);
    let mut tasks = JoinSet::new();
    let mut owners = std::collections::HashMap::new();
    for receiver in receivers {
        let child_control = control.fork_receiver(receiver.id.clone(), receiver.assignment);
        child_control.set_health(Scanner::Cellular, Phase::Disabled, "not_assigned");
        child_control.set_health(Scanner::Ble, Phase::Disabled, "not_assigned");
        child_control.set_health(Scanner::Wifi, Phase::Disabled, "not_assigned");
        child_control.set_health(
            receiver.scanner,
            Phase::Initializing,
            "receiver_initializing",
        );
        let (source_tx, mut source_rx) = mpsc::channel(32);
        let worker_control = child_control.clone();
        let id = receiver.id.clone();
        let worker = tasks.spawn(async move {
            // Keep ingress open until terminal health is recorded, including unwind
            // and task cancellation. Otherwise the filter can finish before JoinError.
            let mut completion = Completion {
                control: worker_control.clone(),
                scanner: receiver.scanner,
                failed: true,
                _ingress: source_tx.clone(),
            };
            let result = (receiver.start)(source_tx, worker_control).await;
            completion.failed = result.is_err();
            drop(completion);
            result.map_err(|e| io::Error::other(format!("receiver {id}: {e}")))
        });
        owners.insert(
            worker.id(),
            (receiver.id.clone(), receiver.scanner, child_control.clone()),
        );
        let forward = tx.clone();
        let session_id = session_id.clone();
        let relay_id = receiver.id.clone();
        let relay_control = child_control.clone();
        let relay = tasks.spawn(async move {
            let mut sequence = 0u64;
            while let Some(event) = source_rx.recv().await {
                let identity = EventSource {
                    session_id: session_id.clone(),
                    receiver_id: receiver.id.clone(),
                    ingress_sequence: sequence,
                };
                sequence = sequence
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("receiver sequence exhausted"))?;
                if forward
                    .send(ScanEvent::Sourced {
                        identity,
                        control: child_control.clone(),
                        event: Box::new(event),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(())
        });
        owners.insert(relay.id(), (relay_id, receiver.scanner, relay_control));
    }
    let mut lifetime_tx = Some(tx);
    let mut filter = tokio::spawn(filter_task(
        rx,
        config.clone(),
        Arc::new(MacIndex::from_defaults()),
        start,
        writer,
        output,
        control.clone(),
    ));
    let command_task = tokio::spawn(commands(config, start, control.clone()));
    tokio::pin!(shutdown);
    let mut failure = None;
    let mut filter_done = false;
    let result = loop {
        if tasks.is_empty() {
            lifetime_tx.take();
        }
        tokio::select! {
            signal = &mut shutdown => break signal,
            result = &mut filter => { filter_done = true; break result.map_err(io::Error::other).and_then(|r| r); }
            result = tasks.join_next_with_id(), if !tasks.is_empty() => {
                let result = task_result(result.expect("nonempty join set"), &mut owners);
                if let Err(error) = result {
                    log::error!("{error}");
                    failure.get_or_insert(error);
                }
            }
        }
    };
    control.shutdown();
    command_task.abort();
    let cleanup = tokio::time::timeout(budget, async {
        while let Some(result) = tasks.join_next_with_id().await {
            if let Err(error) = task_result(result, &mut owners) {
                failure.get_or_insert(error);
            }
        }
    })
    .await;
    if cleanup.is_err() {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        failure.get_or_insert(io::Error::other(
            "receiver shutdown deadline exceeded; radio cleanup unverified",
        ));
    }
    lifetime_tx.take();
    if !filter_done {
        if let Err(error) = filter.await.map_err(io::Error::other).and_then(|r| r) {
            failure.get_or_insert(error);
        }
    }
    let _ = command_task.await;
    if let Some(task) = location_task {
        let _ = task.await;
    }
    result.and(match failure {
        Some(error) => Err(error),
        None => Ok(()),
    })
}

struct Completion {
    control: ScanControl,
    scanner: Scanner,
    failed: bool,
    _ingress: mpsc::Sender<ScanEvent>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.control.finish_receiver(self.scanner, self.failed);
    }
}

type TaskOwners = std::collections::HashMap<tokio::task::Id, (String, Scanner, ScanControl)>;
fn task_result(
    result: Result<(tokio::task::Id, io::Result<()>), tokio::task::JoinError>,
    owners: &mut TaskOwners,
) -> io::Result<()> {
    match result {
        Ok((id, result)) => {
            owners.remove(&id);
            result
        }
        Err(error) => {
            if let Some((id, scanner, control)) = owners.remove(&error.id()) {
                control.finish_receiver(scanner, true);
                Err(io::Error::other(format!("receiver {id}: {error}")))
            } else {
                Err(io::Error::other(error))
            }
        }
    }
}

/// Adds source identity to complete NDJSON records without changing their type
/// or original payload/capture timestamps. Paired evidence shares ingress_sequence.
pub(crate) struct SourceWriter<'a, W> {
    output: &'a mut W,
    source: Option<&'a EventSource>,
    pending: Vec<u8>,
    pub replay: Option<&'a crate::wifi_pcap::Metadata>,
    pub receiver_location: Option<serde_json::Value>,
    /// Typed live sources can carry an explicit location (including null)
    /// snapshotted before queueing. Never replace it with filter-time evidence.
    pub preserve_ingress_location: bool,
}
impl<'a, W: Write> SourceWriter<'a, W> {
    pub fn new(output: &'a mut W, source: Option<&'a EventSource>) -> Self {
        Self {
            output,
            source,
            pending: Vec::new(),
            replay: None,
            receiver_location: None,
            preserve_ingress_location: false,
        }
    }
}
impl<W: Write> Write for SourceWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.source.is_none() && self.replay.is_none() && self.receiver_location.is_none() {
            return self.output.write(bytes);
        }
        for chunk in bytes.split_inclusive(|b| *b == b'\n') {
            if self.pending.len() + chunk.len() > LINE_LIMIT {
                return Err(io::Error::other("source record exceeds output bound"));
            }
            self.pending.extend_from_slice(chunk);
            if chunk.last() == Some(&b'\n') {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&self.pending).map_err(io::Error::other)?;
                let object = value
                    .as_object_mut()
                    .ok_or_else(|| io::Error::other("source record is not an object"))?;
                if let Some(source) = self.source {
                    object.insert(
                        "session_source".into(),
                        serde_json::to_value(source).map_err(io::Error::other)?,
                    );
                }
                if let Some(replay) = self.replay {
                    object.insert("replay".into(), true.into());
                    object.insert(
                        "capture".into(),
                        serde_json::to_value(replay).map_err(io::Error::other)?,
                    );
                }
                // A receiver position is not the transmitter's position or proof
                // that producer timestamps are synchronized with the host.
                if self.replay.is_none()
                    && !self.preserve_ingress_location
                    && object.get("replay") != Some(&serde_json::Value::Bool(true))
                {
                    if let Some(location) = &self.receiver_location {
                        object.insert("receiver_location".into(), location.clone());
                    }
                }
                observation::write_json(self.output, &value)?;
                self.pending.clear();
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            return Err(io::Error::other("incomplete source record"));
        }
        self.output.flush()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
