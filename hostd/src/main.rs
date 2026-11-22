//! AirHound host daemon — WiFi + BLE surveillance detection and WiGLE CSV logger.
//!
//! Scans for BLE advertisements and WiFi networks, logs ALL observations to
//! WiGLE CSV v1.6 format, and outputs NDJSON matches to stdout for matched
//! surveillance devices.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg(all(target_os = "macos", feature = "macos-private-addresses"))]
mod address_macos;
mod ble;
mod ble_payload;
mod ble_remote_id;
mod control;
mod gpsd;
mod health;
mod receiver;
mod session_resources;
mod session_runtime;
use health::{Phase, Scanner};
mod ble_packet;
mod ble_source;
mod external_scanner;
use airhound_hostd::external_wifi;
mod observation;
use control::ScanControl;
#[cfg(target_os = "macos")]
mod location_macos;
#[cfg(any(target_os = "linux", test))]
mod radiotap;
#[cfg(target_os = "linux")]
mod wifi_linux;
#[cfg(target_os = "macos")]
mod wifi_macos;
mod wigle;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod performance;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::Local;
use clap::Parser;
use tokio::sync::mpsc;

use airhound::board::BOARD_NAME;
use airhound::comm;
use airhound::defaults;
use airhound::filter::{
    filter_ble_with_rules, filter_wifi_with_rules, BleScanInput, FilterConfig, WiFiScanInput,
};
use airhound::mac_index::MacIndex;
use airhound::protocol::{self, DeviceMessage, HostCommand};
use observation::ScanEvent;

use wigle::{Location, WigleWriter};

#[derive(Parser)]
#[command(
    name = "airhound-hostd",
    about = "AirHound host daemon — WiFi/BLE scanner with WiGLE CSV output"
)]
#[cfg_attr(target_os = "linux", command(group(clap::ArgGroup::new("wifi_source").args(["wifi_interface", "external_scanner"]))))]
struct Args {
    /// Concurrent receivers described by a versioned JSON session file
    #[arg(long, conflicts_with_all = ["list_ble_adapters", "ble_adapter", "ble_pcap", "sniffle_port", "sniffle_helper", "sniffle_module_path", "sniffle_coded", "sniffle_python", "sniffle_channel", "external_channels", "external_dwell_ms", "external_scanner", "external_firmware", "no_ble", "no_wifi"])]
    #[cfg_attr(target_os = "linux", arg(conflicts_with = "wifi_interface"))]
    session_config: Option<PathBuf>,
    /// List BLE receivers and capability evidence as JSON, without starting a scan
    #[arg(long, conflicts_with_all = ["output", "overwrite", "no_ble", "ble_adapter"])]
    list_ble_adapters: bool,
    /// Select a listed Linux BlueZ adapter (e.g. hci0); CoreBluetooth is system-managed
    #[arg(long, conflicts_with = "no_ble")]
    ble_adapter: Option<String>,
    /// Replay classic DLT256 BLE PCAP through the shared observation pipeline
    #[arg(long, conflicts_with_all = ["sniffle_port", "ble_adapter", "no_ble", "list_ble_adapters"])]
    ble_pcap: Option<PathBuf>,
    /// Explicit serial port for a Sniffle 1.11.0 receiver (no automatic flashing)
    #[arg(long, requires = "sniffle_helper", conflicts_with_all = ["ble_adapter", "no_ble", "list_ble_adapters"])]
    sniffle_port: Option<String>,
    /// Path to scripts/sniffle-source.py, executed as a separate process
    #[arg(long, requires = "sniffle_port")]
    sniffle_helper: Option<PathBuf>,
    #[arg(long, default_value = "python3")]
    sniffle_python: PathBuf,
    /// Directory containing the user-installed Sniffle Python package
    #[arg(long, requires = "sniffle_port")]
    sniffle_module_path: Option<PathBuf>,
    /// Fixed primary advertising channel; extended secondary following is enabled
    #[arg(long, default_value_t = 37, value_parser = clap::value_parser!(u8).range(37..=39))]
    sniffle_channel: u8,
    /// Listen for Coded PHY primary advertisements instead of 1M
    #[arg(long, requires = "sniffle_port")]
    sniffle_coded: bool,
    /// Output WiGLE CSV file path [default: airhound-YYYYMMDD-HHMMSS.csv]
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Replace an existing output file (requires --output)
    #[arg(long, requires = "output")]
    overwrite: bool,
    /// Minimum RSSI for detection messages; CSV still records all observations
    #[arg(long, default_value_t = -90, allow_hyphen_values = true,
          value_parser = clap::value_parser!(i8).range(-128..=0))]
    rssi: i8,
    /// Disable BLE scanning
    #[arg(long, conflicts_with = "no_wifi")]
    #[cfg_attr(target_os = "linux", arg(requires = "wifi_source"))]
    no_ble: bool,
    /// Linux monitor-mode interface (must be configured externally)
    #[cfg(target_os = "linux")]
    #[arg(long, conflicts_with = "no_wifi")]
    wifi_interface: Option<String>,
    /// Explicit companion executable (direct execution; no shell).
    #[arg(long, requires = "external_firmware", conflicts_with = "no_wifi")]
    external_scanner: Option<PathBuf>,
    /// Pinned firmware file supplied to the external receiver.
    #[arg(long, requires = "external_scanner")]
    external_firmware: Option<PathBuf>,
    /// External receiver channels; replaces the platform WiFi scanner.
    #[arg(long, value_delimiter = ',', default_value = "1")]
    external_channels: Vec<u8>,
    #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u16).range(100..=10000))]
    external_dwell_ms: u16,
    /// Read GNSS fixes from an already-running gpsd server (e.g. 127.0.0.1:2947).
    #[arg(long)]
    gpsd: Option<String>,
    /// gpsd device path; otherwise bind to the first reporting device for this session.
    #[arg(long, requires = "gpsd")]
    gpsd_device: Option<String>,
    /// Disable WiFi scanning
    #[arg(long)]
    no_wifi: bool,
    /// Increase diagnostic verbosity (-v debug, -vv trace); RUST_LOG overrides
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn open_output(path: &std::path::Path, overwrite: bool) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(overwrite)
        .truncate(overwrite)
        .create_new(!overwrite)
        .open(path)
}

/// Serialize and flush one NDJSON message, reporting serialization and I/O failures.
fn write_message(out: &mut impl Write, msg: &DeviceMessage<'_>) -> io::Result<()> {
    let mut buf = [0u8; protocol::MAX_MSG_LEN];
    let len = comm::serialize_message(msg, &mut buf)
        .ok_or_else(|| io::Error::other("NDJSON message exceeds output buffer"))?;
    out.write_all(&buf[..len])?;
    out.flush()
}

/// Generate default output filename with current timestamp.
fn default_output_path() -> PathBuf {
    let ts = Local::now().format("%Y%m%d-%H%M%S");
    PathBuf::from(format!("airhound-{ts}.csv"))
}

/// Get the current GPS location (macOS: CoreLocation, other: always None).
fn get_location(control: &ScanControl) -> Option<Location> {
    if let Some(location) = control.external_location() {
        return location;
    }
    #[cfg(target_os = "macos")]
    {
        location_macos::get_current_location()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn prepare_output(args: &Args) -> io::Result<WigleWriter<std::fs::File>> {
    let path = args.output.clone().unwrap_or_else(default_output_path);
    let writer = WigleWriter::new(open_output(&path, args.overwrite)?)?;
    log::info!("WiGLE CSV output: {}", path.display());
    Ok(writer)
}

impl Args {
    fn filter_config(&self) -> FilterConfig {
        FilterConfig {
            min_rssi: self.rssi,
            ble_enabled: !self.no_ble,
            wifi_enabled: !self.no_wifi,
        }
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();
    let level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format_timestamp_millis()
        .init();
    receiver::validate_selection(args.ble_adapter.as_deref(), cfg!(target_os = "macos"))
        .map_err(io::Error::other)?;
    if args.list_ble_adapters {
        let receivers =
            tokio::time::timeout(std::time::Duration::from_secs(10), receiver::discover())
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "BLE adapter enumeration timed out; check permission and power",
                    )
                })?
                .map_err(io::Error::other)?;
        let mut descriptions: Vec<_> = receivers.into_iter().map(|(_, info)| info).collect();
        descriptions.sort_by(|a, b| a.adapter_id.cmp(&b.adapter_id));
        return observation::write_json(
            &mut io::stdout(),
            &serde_json::json!({
                "type":"ble_adapters", "schema_version":1, "backend":receiver::backend(), "adapters":descriptions
            }),
        );
    }
    if let Some(path) = args.session_config.as_ref() {
        let plan = session_runtime::load(path)?;
        let receivers = session_runtime::prepare(plan).await?;
        let writer = prepare_output(&args)?;
        return session_runtime::run(
            receivers,
            args.filter_config(),
            writer,
            io::stdout(),
            args.gpsd.clone().map(|endpoint| gpsd::Options {
                endpoint,
                device: args.gpsd_device.clone(),
            }),
            stdin_command_loop,
            shutdown_signal(),
        )
        .await;
    }
    if args.external_scanner.is_some() {
        let channels = &args.external_channels;
        if channels.is_empty()
            || channels.len() > 38
            || channels
                .iter()
                .any(|c| !external_wifi::supported_receive_channel(*c))
            || channels
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != channels.len()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Choose unique supported external channels",
            ));
        }
    }
    let external = args
        .external_scanner
        .as_ref()
        .map(|path| external_scanner::Options {
            executable: path.clone(),
            firmware: args
                .external_firmware
                .clone()
                .expect("clap requires firmware"),
            channels: args.external_channels.clone(),
            dwell_ms: args.external_dwell_ms,
            #[cfg(test)]
            fixture_script: None,
        });
    let writer = prepare_output(&args)?;
    // An explicit missing selection is a startup error, not a fallback or a half-running session.
    let selected_ble = if let Some(id) = args.ble_adapter.as_deref() {
        Some(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                receiver::selected(Some(id)),
            )
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "BLE adapter selection timed out")
            })?
            .map_err(io::Error::other)?,
        )
    } else {
        None
    };
    let packet_source = if let Some(path) = args.ble_pcap.clone() {
        Some(ble_source::Source::Pcap(path))
    } else {
        args.sniffle_port
            .clone()
            .map(|port| ble_source::Source::Sniffle {
                port,
                python: args.sniffle_python.clone(),
                helper: args.sniffle_helper.clone().expect("required by clap"),
                module_path: args.sniffle_module_path.clone(),
                channel: args.sniffle_channel,
                coded: args.sniffle_coded,
            })
    };
    let fatal_ble_errors = packet_source.is_some();
    #[cfg(target_os = "macos")]
    location_macos::start();
    run_session(
        if external.is_some() {
            SessionPolicy::external()
        } else {
            SessionPolicy::native()
        }
        .with_fatal_ble(fatal_ble_errors)
        .with_gpsd(args.gpsd.clone().map(|endpoint| gpsd::Options {
            endpoint,
            device: args.gpsd_device.clone(),
        })),
        args.filter_config(),
        writer,
        io::stdout(),
        Scanners {
            ble: |tx, control| async move {
                if let Some(source) = packet_source {
                    return ble_source::scan(source, tx, control).await;
                }
                let selected = match selected_ble {
                    Some(selected) => Some(selected),
                    None => ble::initialize(receiver::selected(None), &control, &tx).await?,
                };
                let Some((adapter, receiver)) = selected else {
                    return Ok(());
                };
                control.set_receiver(receiver);
                log::info!(
                    "BLE receiver: {}",
                    control.receiver_id().as_deref().unwrap_or("unknown")
                );
                ble::scan_loop(adapter, tx, control)
                    .await
                    .map_err(io::Error::other)
            },
            wifi: |tx, control| async move {
                if let Some(options) = external {
                    return external_scanner::scan_loop(options, tx, control).await;
                }
                #[cfg(target_os = "macos")]
                {
                    wifi_macos::scan_loop(tx, control)
                        .await
                        .map_err(io::Error::other)
                }
                #[cfg(target_os = "linux")]
                {
                    match args.wifi_interface {
                        Some(interface) => wifi_linux::scan_loop(&interface, tx, control)
                            .await
                            .map_err(io::Error::other),
                        None => {
                            control.set_health(
                                Scanner::Wifi,
                                Phase::Disabled,
                                "monitor_interface_not_configured",
                            );
                            log::info!(
                                "WiFi disabled: select a monitor interface with --wifi-interface"
                            );
                            Ok(())
                        }
                    }
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    drop(tx);
                    Ok(())
                }
            },
        },
        stdin_command_loop,
        shutdown_signal(),
    )
    .await
}

/// SIGTERM uses the same cooperative scanner shutdown as Ctrl-C.
async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = terminate.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

#[derive(Clone)]
struct SessionPolicy {
    shutdown_budget: std::time::Duration,
    fatal_wifi_errors: bool,
    fatal_ble_errors: bool,
    gpsd: Option<gpsd::Options>,
}
impl SessionPolicy {
    fn with_fatal_ble(mut self, fatal: bool) -> Self {
        self.fatal_ble_errors = fatal;
        self
    }
    fn with_gpsd(mut self, options: Option<gpsd::Options>) -> Self {
        self.gpsd = options;
        self
    }
    fn native() -> Self {
        Self {
            shutdown_budget: std::time::Duration::from_secs(1),
            fatal_wifi_errors: false,
            fatal_ble_errors: false,
            gpsd: None,
        }
    }
    fn external() -> Self {
        Self {
            shutdown_budget: std::time::Duration::from_secs(40),
            fatal_wifi_errors: true,
            fatal_ble_errors: false,
            gpsd: None,
        }
    }
}

struct Scanners<B, W> {
    ble: B,
    wifi: W,
}

// Inject only the OS boundaries; startup, task ownership and shutdown are shared
// unchanged between the executable and in-memory integration tests.
async fn run_session<W, O, B, BF, F, FF, C, CF, S>(
    policy: SessionPolicy,
    config: FilterConfig,
    writer: WigleWriter<W>,
    output: O,
    scanners: Scanners<B, F>,
    commands: C,
    shutdown: S,
) -> io::Result<()>
where
    W: Write + Send + 'static,
    O: Write + Send + 'static,
    B: FnOnce(mpsc::Sender<ScanEvent>, ScanControl) -> BF + Send + 'static,
    BF: std::future::Future<Output = io::Result<()>> + Send,
    F: FnOnce(mpsc::Sender<ScanEvent>, ScanControl) -> FF + Send + 'static,
    FF: std::future::Future<Output = io::Result<()>> + Send,
    C: FnOnce(Arc<Mutex<FilterConfig>>, Instant, ScanControl) -> CF,
    CF: std::future::Future<Output = ()> + Send + 'static,
    S: std::future::Future<Output = io::Result<()>>,
{
    log::info!(
        "AirHound hostd v{} starting on {}",
        protocol::VERSION,
        BOARD_NAME
    );
    let control = ScanControl::new(true);
    let location_task = policy.gpsd.map(|options| {
        let source = gpsd::Source::new(options.device.clone());
        control.set_location_source(source.clone());
        tokio::spawn(gpsd::run(options, source, control.subscribe()))
    });
    let config = Arc::new(Mutex::new(config));
    let cfg = *config.lock().unwrap_or_else(|e| e.into_inner());
    let start = Instant::now();
    let (tx, rx) = mpsc::channel(32);
    let ble_tx = tx.clone();
    let (failure_tx, mut failures) = mpsc::channel(2);
    let ble_failure_tx = failure_tx.clone();
    let ble_control = control.clone();
    let ble = tokio::spawn(async move {
        if cfg.ble_enabled {
            ble_control.set_health(Scanner::Ble, Phase::Initializing, "adapter_initializing");
            if let Err(e) = (scanners.ble)(ble_tx, ble_control.clone()).await {
                ble_control.set_receiver_availability("unavailable");
                ble_control.set_health(
                    Scanner::Ble,
                    Phase::Unavailable,
                    "adapter_failed_check_permission_or_power",
                );
                log::error!("BLE scan error: {e}");
                if policy.fatal_ble_errors {
                    let _ = ble_failure_tx.send(e).await;
                }
            }
        } else {
            ble_control.set_health(Scanner::Ble, Phase::Disabled, "disabled_by_configuration");
        }
    });
    let wifi_control = control.clone();
    let wifi = tokio::spawn(async move {
        if cfg.wifi_enabled {
            wifi_control.set_health(Scanner::Wifi, Phase::Initializing, "adapter_initializing");
            if let Err(e) = (scanners.wifi)(tx, wifi_control.clone()).await {
                wifi_control.set_health(Scanner::Wifi, Phase::Unavailable, "adapter_failed");
                log::error!("WiFi scan error: {e}");
                if policy.fatal_wifi_errors {
                    let _ = failure_tx.send(e).await;
                }
            }
        } else {
            wifi_control.set_health(Scanner::Wifi, Phase::Disabled, "disabled_by_configuration");
        }
    });
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
    let result = tokio::select! {
        Some(error) = failures.recv() => {
            control.shutdown(); command_task.abort();
            let forced = stop_scanners(ble, wifi, policy.shutdown_budget).await;
            let _ = filter.await;
            if forced { log::error!("Scanner task cleanup exceeded shutdown budget"); }
            Err(error)
        }
        signal = shutdown => {
            control.shutdown();
            command_task.abort();
            let forced = stop_scanners(ble, wifi, policy.shutdown_budget).await;
            let drained = signal.and(filter.await.map_err(io::Error::other).and_then(|result| result));
            if forced && policy.fatal_wifi_errors { Err(io::Error::other("external scanner cleanup task aborted; radio state unverified")) } else { match failures.try_recv() { Ok(error) => Err(error), Err(_) => drained } }
        }
        result = &mut filter => {
            control.shutdown();
            command_task.abort();
            let forced = stop_scanners(ble, wifi, policy.shutdown_budget).await;
            let result = result.map_err(io::Error::other).and_then(|result| result);
            if forced && policy.fatal_wifi_errors { Err(io::Error::other("external scanner cleanup task aborted; radio state unverified")) } else { match failures.try_recv() { Ok(error) => Err(error), Err(_) => result } }
        }
    };
    let _ = command_task.await;
    if let Some(task) = location_task {
        let _ = task.await;
    }
    result
}

async fn stop_scanners(
    ble: tokio::task::JoinHandle<()>,
    wifi: tokio::task::JoinHandle<()>,
    budget: std::time::Duration,
) -> bool {
    async fn stop(mut task: tokio::task::JoinHandle<()>, budget: std::time::Duration) -> bool {
        match tokio::time::timeout(budget, &mut task).await {
            Ok(result) => result.is_err(),
            Err(_) => {
                log::warn!("Scanner exceeded shutdown budget; canceling its task");
                task.abort();
                let _ = task.await;
                true
            }
        }
    }
    let (ble_forced, wifi_forced) = tokio::join!(stop(ble, budget), stop(wifi, budget));
    ble_forced || wifi_forced
}

/// Export address-bearing observations to CSV, matches and peripheral identities to NDJSON.
async fn filter_task<W: Write, O: Write>(
    mut rx: mpsc::Receiver<ScanEvent>,
    config: Arc<Mutex<FilterConfig>>,
    mac_index: Arc<MacIndex>,
    start_time: Instant,
    mut wigle: WigleWriter<W>,
    mut output: O,
    control: ScanControl,
) -> io::Result<()> {
    let mut mac_buf = protocol::MacString::new();
    let anonymous_mac_index = MacIndex::new();
    let mut flush_tick = tokio::time::interval(std::time::Duration::from_secs(5));
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut last_health = None;
    loop {
        let health = control.health_message();
        if last_health.as_ref() != Some(&health) {
            observation::write_json(&mut output, &health)?;
            last_health = Some(health);
        }
        let event = tokio::select! {
            event = rx.recv() => match event {
                Some(event) => event,
                None => {
                    let health = control.health_message();
                    if last_health.as_ref() != Some(&health) { observation::write_json(&mut output, &health)?; }
                    break;
                },
            },
            _ = flush_tick.tick() => {
                wigle.flush()?;
                continue;
            }
        };
        let (event, source, event_control) = match event {
            ScanEvent::Sourced {
                identity,
                control,
                event,
            } => (*event, Some(identity), control),
            event => (event, None, control.clone()),
        };
        let control = event_control;
        let mut output = session_runtime::SourceWriter::new(&mut output, source.as_ref());
        if let ScanEvent::ExternalStatus {
            ref state,
            dropped,
            producer_dropped,
            reconnect_attempts,
        } = event
        {
            control.set_health(
                Scanner::Wifi,
                match state.as_str() {
                    "running" => Phase::Running,
                    "paused" => Phase::Paused,
                    _ => Phase::Unavailable,
                },
                "external_receiver_state",
            );
            let value = serde_json::json!({"type":"external_scanner_status","source":"mt7610u-lab","state":state,"host_dropped_observations":dropped,"producer_dropped_observations":producer_dropped,"reconnect_attempts":reconnect_attempts});
            observation::write_json(&mut output, &value)?;
            continue;
        }
        if let ScanEvent::BleSourceStatus(ref value) = event {
            observation::write_json(&mut output, value)?;
            continue;
        }
        if !control.load(Ordering::Relaxed) {
            if matches!(&event, ScanEvent::BlePacket(packet) if packet.replay) {
                let mut state = control.subscribe();
                while !state.borrow().enabled && !state.borrow().shutdown {
                    if state.changed().await.is_err() {
                        break;
                    }
                }
                if state.borrow().shutdown {
                    break;
                }
            } else {
                continue;
            }
        }

        let cfg = *config.lock().unwrap_or_else(|e| e.into_inner());
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let location = get_location(&control);

        match event {
            ScanEvent::Sourced { .. } => return Err(io::Error::other("nested receiver envelope")),
            ScanEvent::BleSourceStatus(_) => unreachable!(),
            ScanEvent::BlePacket(ref packet) => {
                let state = *control.subscribe().borrow();
                let accepted_generation = if state.shutdown {
                    state.generation.wrapping_sub(1)
                } else {
                    state.generation
                };
                if !packet.replay && packet.ingestion_generation != accepted_generation {
                    continue;
                }
                let result = packet
                    .advertisement
                    .as_ref()
                    .filter(|_| packet.matchable())
                    .map(|snapshot| {
                        let input = BleScanInput {
                            address_type: if packet.address_type == "public" {
                                airhound::filter::BleAddressType::Public
                            } else {
                                airhound::filter::BleAddressType::Random
                            },
                            mac: packet.mac.as_ref().expect("addressed advertisement"),
                            name: &packet.name,
                            rssi: packet.rssi.expect("matchable RSSI"),
                            service_uuids_16: &[],
                            manufacturer_id: 0,
                            raw_ad: &[],
                        };
                        snapshot.with_view(|data| {
                            airhound::filter::filter_ble_packet_with_rules(
                                &input,
                                data,
                                &cfg,
                                &airhound::defaults::DEFAULT_RULE_DB,
                                &mac_index,
                            )
                        })
                    });
                let mut value = serde_json::to_value(packet).map_err(io::Error::other)?;
                value["type"] = serde_json::json!("ble_packet_observation");
                value["schema_version"] = serde_json::json!(1);
                value["identity_scope"] = serde_json::json!("ble_air_address");
                value["detection"] = serde_json::to_value(
                    result
                        .as_ref()
                        .map(|r| r.detection(airhound::evidence::AlertPolicy::RulesAndWatchlist)),
                )
                .map_err(io::Error::other)?;
                observation::write_json(&mut output, &value)?;
                if packet.matchable()
                    && cfg.ble_enabled
                    && packet.rssi.is_some_and(|rssi| rssi >= cfg.min_rssi)
                {
                    let context = ble_remote_id::Context {
                        peripheral_id: packet.address.as_deref().expect("address"),
                        identity_scope: "ble_air_address",
                        address: packet.address.as_deref(),
                        address_type: packet.address_type,
                        receiver_id: Some(&packet.source_id),
                        rssi: packet.rssi,
                        ts: start_time.elapsed().as_millis() as u32,
                    };
                    for record in &packet
                        .advertisement
                        .as_ref()
                        .expect("advertisement")
                        .service_data
                    {
                        ble_remote_id::emit_packet(
                            &mut output,
                            &context,
                            &record.uuid.0,
                            &record.payload,
                        )?;
                    }
                }
                if packet.matchable() {
                    let stamp =
                        chrono::DateTime::from_timestamp_micros(packet.captured_at_us as i64)
                            .ok_or_else(|| io::Error::other("invalid capture timestamp"))?
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string();
                    let manufacturer = packet
                        .advertisement
                        .as_ref()
                        .and_then(|s| s.manufacturer_data.first())
                        .map_or(0, |m| m.company_id);
                    wigle.write_ble(
                        packet.address.as_deref().expect("addressed advertisement"),
                        &packet.name,
                        &stamp,
                        packet.rssi.expect("matchable RSSI"),
                        manufacturer,
                        if packet.replay {
                            None
                        } else {
                            location.as_ref()
                        },
                    )?;
                }
            }
            ScanEvent::Ble(ref ble_event) => {
                // Write observation to WiGLE CSV (all networks, not just matches)
                mac_buf.clear();
                protocol::format_mac(&ble_event.mac, &mut mac_buf);
                wigle.write_ble(
                    &mac_buf,
                    ble_event.name.as_str(),
                    &now,
                    ble_event.rssi,
                    ble_event.manufacturer_id,
                    location.as_ref(),
                )?;

                // Run filter and emit NDJSON if matched
                handle_ble_event(ble_event, &cfg, &mac_index, start_time, &mut output)?;
            }
            ScanEvent::BleSnapshot {
                ref event,
                ref id,
                address,
                ref snapshot,
            } => {
                let observed_mac = address.map(|mac| {
                    let mut text = protocol::MacString::new();
                    protocol::format_mac(&mac, &mut text);
                    text
                });
                if let Some(mac) = observed_mac.as_ref().filter(|_| snapshot.rssi_available) {
                    wigle.write_ble(
                        mac,
                        event.name.as_str(),
                        &now,
                        event.rssi,
                        event.manufacturer_id,
                        location.as_ref(),
                    )?;
                }
                let input = BleScanInput {
                    address_type: if snapshot.address_type == "public" {
                        airhound::filter::BleAddressType::Public
                    } else {
                        airhound::filter::BleAddressType::Unknown
                    },
                    mac: if address.is_some() {
                        &event.mac
                    } else {
                        &[0; 6]
                    },
                    name: event.name.as_str(),
                    rssi: event.rssi,
                    service_uuids_16: &[],
                    manufacturer_id: 0,
                    raw_ad: &[],
                };
                let index = if address.is_some() {
                    &mac_index
                } else {
                    &anonymous_mac_index
                };
                let mut snapshot_cfg = cfg;
                snapshot_cfg.ble_enabled &= snapshot.rssi_available;
                let result = snapshot.with_view(|data| {
                    airhound::filter::filter_ble_snapshot(&input, data, &snapshot_cfg, index)
                });
                // Linux consumers keep the legacy matched-record stream during migration.
                if !cfg!(target_os = "macos") && address.is_some() && snapshot.rssi_available {
                    emit_ble_match(event, &result, start_time, &mut output)?;
                }
                observation::write(
                    &mut output,
                    &observation::BleObservation {
                        kind: "ble_observation",
                        detection: result
                            .detection(airhound::evidence::AlertPolicy::RulesAndWatchlist),
                        peripheral_id: id,
                        address: observed_mac.as_ref().map(|s| s.as_str()),
                        address_source: observed_mac.as_ref().map(|_| {
                            if cfg!(target_os = "macos") {
                                "corebluetooth_private"
                            } else {
                                "btleplug"
                            }
                        }),
                        address_type: snapshot.address_type,
                        identity_scope: if cfg!(target_os = "macos") {
                            "corebluetooth"
                        } else {
                            "btleplug"
                        },
                        name: event.name.as_str(),
                        rssi: event.rssi,
                        service_uuids: &event.service_uuids_16,
                        manufacturer_id: event.manufacturer_id,
                        matches: &result.matches,
                        ts: start_time.elapsed().as_millis() as u32,
                        advertisement: Some(snapshot),
                    },
                )?;
                if cfg.ble_enabled && (!snapshot.rssi_available || event.rssi >= cfg.min_rssi) {
                    let context = ble_remote_id::Context {
                        peripheral_id: id,
                        identity_scope: if cfg!(target_os = "macos") {
                            "corebluetooth"
                        } else {
                            "btleplug"
                        },
                        address: observed_mac.as_ref().map(|s| s.as_str()),
                        address_type: snapshot.address_type,
                        receiver_id: snapshot.receiver_id.as_deref(),
                        rssi: snapshot.rssi_available.then_some(event.rssi),
                        ts: start_time.elapsed().as_millis() as u32,
                    };
                    for record in &snapshot.service_data {
                        ble_remote_id::emit(
                            &mut output,
                            &context,
                            &record.uuid.0,
                            &record.payload,
                            record.truncated,
                        )?;
                    }
                }
            }
            ScanEvent::BlePeripheral {
                ref event,
                ref id,
                address,
            } => {
                let input = BleScanInput {
                    address_type: airhound::filter::BleAddressType::Unknown,
                    mac: &[0; 6],
                    name: event.name.as_str(),
                    rssi: event.rssi,
                    service_uuids_16: &event.service_uuids_16,
                    manufacturer_id: event.manufacturer_id,
                    raw_ad: &event.raw_ad,
                };
                // Address type is unknown; do not treat rotating addresses as hardware identity.
                let mut result = airhound::filter::filter_ble(&input, &cfg, &anonymous_mac_index);
                result.basis = "possibly_cached";
                let observed_mac = address.map(|mac| {
                    let mut text = protocol::MacString::new();
                    protocol::format_mac(&mac, &mut text);
                    text
                });
                if let Some(mac) = &observed_mac {
                    wigle.write_ble(
                        mac,
                        event.name.as_str(),
                        &now,
                        event.rssi,
                        event.manufacturer_id,
                        location.as_ref(),
                    )?;
                }
                observation::write(
                    &mut output,
                    &observation::BleObservation {
                        kind: "ble_observation",
                        detection: result
                            .detection(airhound::evidence::AlertPolicy::RulesAndWatchlist),
                        advertisement: None,
                        peripheral_id: id,
                        address: observed_mac.as_ref().map(|s| s.as_str()),
                        address_source: observed_mac.as_ref().map(|_| "corebluetooth_private"),
                        address_type: "unknown",
                        identity_scope: if cfg!(target_os = "macos") {
                            "corebluetooth"
                        } else {
                            "btleplug"
                        },
                        name: event.name.as_str(),
                        rssi: event.rssi,
                        service_uuids: &event.service_uuids_16,
                        manufacturer_id: event.manufacturer_id,
                        matches: &result.matches,
                        ts: start_time.elapsed().as_millis() as u32,
                    },
                )?;
            }
            ScanEvent::ExternalWiFi {
                ref event,
                generation,
            } => {
                if cfg.wifi_enabled && control.subscribe().borrow().generation == generation {
                    observation::write_external(&mut output, event, &mac_index)?;
                }
            }
            ScanEvent::ExternalStatus { .. } => unreachable!("status handled before pause gate"),
            ScanEvent::WiFi(ref wifi_event) => {
                // Write observation to WiGLE CSV (all networks, not just matches)
                mac_buf.clear();
                protocol::format_mac(&wifi_event.mac, &mut mac_buf);
                let freq = wigle::channel_to_frequency(wifi_event.channel);
                wigle.write_wifi(
                    &mac_buf,
                    wifi_event.ssid.as_str(),
                    wifi_event.security.as_str(),
                    &now,
                    wifi_event.channel,
                    freq,
                    wifi_event.rssi,
                    location.as_ref(),
                )?;

                // Run filter and emit NDJSON if matched
                handle_wifi_event(wifi_event, &cfg, &mac_index, start_time, &mut output)?;
            }
            #[cfg(any(target_os = "linux", test))]
            ScanEvent::Odid(ref event) => {
                if cfg.wifi_enabled && event.rssi >= cfg.min_rssi {
                    if let Some(frame) = airhound::odid::parse_odid_service_info(&event.raw_data) {
                        emit_odid(&frame, event, start_time, &mut output)?;
                    }
                }
            }
        }
    }

    // Flush remaining buffered writes on shutdown
    wigle.flush()
}

/// Filter and emit a BLE scan event (NDJSON on stdout if matched).
fn handle_ble_event(
    event: &airhound::scanner::BleEvent,
    cfg: &FilterConfig,
    mac_index: &MacIndex,
    start_time: Instant,
    output: &mut impl Write,
) -> io::Result<()> {
    let input = BleScanInput {
        address_type: airhound::filter::BleAddressType::Unknown,
        mac: &event.mac,
        name: event.name.as_str(),
        rssi: event.rssi,
        service_uuids_16: &event.service_uuids_16,
        manufacturer_id: event.manufacturer_id,
        raw_ad: &event.raw_ad,
    };

    let result = filter_ble_with_rules(&input, cfg, &defaults::DEFAULT_RULE_DB, mac_index);
    emit_ble_match(event, &result, start_time, output)
}

fn emit_ble_match(
    event: &airhound::scanner::BleEvent,
    result: &airhound::filter::FilterResult,
    start_time: Instant,
    output: &mut impl Write,
) -> io::Result<()> {
    if !result.matched {
        return Ok(());
    }

    let mut mac_str = protocol::MacString::new();
    protocol::format_mac(&event.mac, &mut mac_str);

    let first_uuid = if !event.service_uuids_16.is_empty() {
        let mut s = protocol::UuidString::new();
        let _ = core::fmt::Write::write_fmt(
            &mut s,
            format_args!("0x{:04X}", event.service_uuids_16[0]),
        );
        Some(s)
    } else {
        None
    };

    let ts = start_time.elapsed().as_millis() as u32;

    let msg = DeviceMessage::BleScan {
        mac: &mac_str,
        name: &event.name,
        rssi: event.rssi,
        uuid: first_uuid.as_ref(),
        mfr: event.manufacturer_id,
        matches: &result.matches,
        rule: result.rule_names.first().copied(),
        ts,
    };

    write_message(output, &msg)?;
    write_message(
        output,
        &DeviceMessage::Detection {
            id: mac_str.as_str(),
            scope: "ble_address",
            ts,
            detection: result.detection(airhound::evidence::AlertPolicy::RulesAndWatchlist),
        },
    )
}

/// Filter and emit a WiFi scan event (NDJSON on stdout if matched).
fn handle_wifi_event(
    event: &airhound::scanner::WiFiEvent,
    cfg: &FilterConfig,
    mac_index: &MacIndex,
    start_time: Instant,
    output: &mut impl Write,
) -> io::Result<()> {
    let input = WiFiScanInput {
        mac: &event.mac,
        ssid: event.ssid.as_str(),
        rssi: event.rssi,
    };

    let result = filter_wifi_with_rules(&input, cfg, &defaults::DEFAULT_RULE_DB, mac_index);

    let mut mac_str = protocol::MacString::new();
    protocol::format_mac(&event.mac, &mut mac_str);

    let ts = start_time.elapsed().as_millis() as u32;

    observation::write_json(
        output,
        &serde_json::json!({
            "type":"wifi_observation", "mac":mac_str.as_str(), "ssid":event.ssid.as_str(),
            "rssi":event.rssi, "ch":event.channel, "frame":event.frame_type.as_str(),
            "security":event.security.as_str(), "ts":ts,
            "detection":result.detection(airhound::evidence::AlertPolicy::RulesAndWatchlist)
        }),
    )?;
    if !result.matched {
        return Ok(());
    }
    let msg = DeviceMessage::WiFiScan {
        mac: &mac_str,
        ssid: &event.ssid,
        rssi: event.rssi,
        ch: event.channel,
        frame: event.frame_type.as_str(),
        matches: &result.matches,
        rule: result.rule_names.first().copied(),
        ts,
    };

    write_message(output, &msg)?;
    write_message(
        output,
        &DeviceMessage::Detection {
            id: mac_str.as_str(),
            scope: "wifi_address",
            ts,
            detection: result.detection(airhound::evidence::AlertPolicy::RulesAndWatchlist),
        },
    )
}

/// Read bounded command lines, preserving framing across arbitrary read chunks.
fn read_commands(
    mut reader: impl BufRead,
    tx: mpsc::Sender<Option<HostCommand>>,
) -> io::Result<()> {
    let mut framing = comm::LineReader::new();
    let mut bytes = [0u8; 256];
    loop {
        let count = reader.read(&mut bytes)?;
        let input = if count == 0 {
            &b"\n"[..]
        } else {
            &bytes[..count]
        };
        for &byte in input {
            if let Some(line) = framing.feed(byte) {
                if tx.blocking_send(comm::parse_command(line)).is_err() {
                    return Ok(());
                }
            }
        }
        if count == 0 {
            return Ok(());
        }
    }
}

async fn command_loop(
    mut commands: mpsc::Receiver<Option<HostCommand>>,
    config: Arc<Mutex<FilterConfig>>,
    scanning_state: &ScanControl,
    start_time: Instant,
    mut output: impl Write,
) -> io::Result<()> {
    while let Some(cmd) = commands.recv().await {
        let mut cfg = config.lock().unwrap_or_else(|e| e.into_inner());
        let mut scanning = scanning_state.load(Ordering::Relaxed);
        let result = match cmd.as_ref() {
            Some(cmd) => {
                comm::execute_command(cmd, &mut cfg, &mut scanning, false, None).map(|_| ())
            }
            None => Err(protocol::CommandError::InvalidCommand),
        };
        scanning_state.store(scanning, Ordering::Relaxed);
        write_message(&mut output, &comm::command_response(cmd.as_ref(), result))?;
        if matches!(cmd, Some(HostCommand::GetStatus)) {
            observation::write_json(&mut output, &scanning_state.health_message())?;
            write_message(
                &mut output,
                &DeviceMessage::Status {
                    scanning,
                    uptime: start_time.elapsed().as_secs() as u32,
                    heap_free: 0,
                    ble_clients: 0,
                    board: BOARD_NAME,
                    version: protocol::VERSION,
                },
            )?;
        }
    }
    Ok(())
}

/// Read NDJSON commands from stdin and dispatch them.
async fn stdin_command_loop(
    config: Arc<Mutex<FilterConfig>>,
    start_time: Instant,
    control: ScanControl,
) {
    input_command_loop(
        io::BufReader::new(io::stdin()),
        io::stdout(),
        config,
        start_time,
        &control,
    )
    .await;
}

async fn input_command_loop(
    input: impl BufRead + Send + 'static,
    output: impl Write,
    config: Arc<Mutex<FilterConfig>>,
    start_time: Instant,
    scanning: &ScanControl,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    std::thread::spawn(move || {
        if let Err(e) = read_commands(input, cmd_tx) {
            log::error!("Command input failed: {e}");
        }
    });
    if let Err(e) = command_loop(cmd_rx, config, scanning, start_time, output).await {
        log::error!("Command output failed: {e}");
    }
}

#[cfg(any(target_os = "linux", test))]
fn emit_odid(
    frame: &airhound::odid::OdidFrame,
    event: &airhound::scanner::OdidEvent,
    start: Instant,
    output: &mut impl Write,
) -> io::Result<()> {
    let mut mac = protocol::MacString::new();
    protocol::format_mac(&event.mac, &mut mac);
    let uas_id = frame.basic_id.as_ref().map(|id| {
        let mut s = protocol::NameString::new();
        let _ = s.push_str(&id.uas_id);
        s
    });
    let operator_id = frame.operator_id.as_ref().map(|id| {
        let mut s = protocol::NameString::new();
        let _ = s.push_str(&id.operator_id);
        s
    });
    write_message(
        output,
        &DeviceMessage::DroneSighting {
            mac: &mac,
            rssi: event.rssi,
            source: event.source.as_str(),
            uas_id: uas_id.as_ref(),
            ua_type: frame.basic_id.as_ref().map(|id| id.ua_type as u8),
            lat: frame.location.as_ref().map(|l| l.latitude),
            lon: frame.location.as_ref().map(|l| l.longitude),
            alt: frame.location.as_ref().map(|l| l.altitude_geo),
            speed: frame.location.as_ref().map(|l| l.speed_horizontal),
            operator_id: operator_id.as_ref(),
            rule: Some("Open Drone ID"),
            category: Some("drone"),
            ts: start.elapsed().as_millis() as u32,
        },
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn cli_validates_configuration() {
        use clap::Parser;
        let args =
            super::Args::try_parse_from(["hostd", "--rssi", "-128", "--no-wifi", "-vv"]).unwrap();
        assert_eq!(args.rssi, -128);
        assert!(args.no_wifi);
        assert_eq!(args.verbose, 2);
        assert!(super::Args::try_parse_from(["hostd", "--session-config", "session.json"]).is_ok());
        for option in [
            "--sniffle-python",
            "--sniffle-channel",
            "--external-channels",
            "--external-dwell-ms",
            "--ble-adapter",
            "--ble-pcap",
        ] {
            assert!(
                super::Args::try_parse_from([
                    "hostd",
                    "--session-config",
                    "session.json",
                    option,
                    "37"
                ])
                .is_err(),
                "{option}"
            );
        }

        for flags in [
            vec!["--rssi", "1"],
            vec!["--rssi", "-129"],
            vec!["--no-ble", "--no-wifi"],
            vec!["--overwrite"],
        ] {
            assert!(super::Args::try_parse_from(std::iter::once("hostd").chain(flags)).is_err());
        }
    }

    #[test]
    fn existing_output_requires_explicit_overwrite() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "airhound-output-{}-{}.csv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        super::open_output(&path, false)
            .unwrap()
            .write_all(b"keep me")
            .unwrap();
        assert_eq!(
            super::open_output(&path, false).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
        drop(super::open_output(&path, true).unwrap());
        assert!(std::fs::read(&path).unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone, Default)]
    struct RecordingWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
        flushes: Arc<AtomicUsize>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    // Existing observation/command assertions exclude additive health/evidence streams.
    fn data_output(output: &RecordingWriter) -> String {
        recorded(output)
            .lines()
            .filter(|line| {
                !matches!(
                    serde_json::from_str::<serde_json::Value>(line).unwrap()["type"].as_str(),
                    Some("scanner_health" | "detection" | "wifi_observation")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn failed_adapter_is_reported_even_without_observations() {
        let output = RecordingWriter::default();
        run_session(
            SessionPolicy::native(),
            FilterConfig {
                wifi_enabled: false,
                ..FilterConfig::new()
            },
            WigleWriter::new(Vec::new()).unwrap(),
            output.clone(),
            Scanners {
                ble: |_, _| async { Err(io::Error::other("denied")) },
                wifi: |_, _| async { panic!("disabled") },
            },
            |_, _, _| std::future::pending(),
            std::future::pending(),
        )
        .await
        .unwrap();
        let messages: Vec<serde_json::Value> = recorded(&output)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let health = messages.last().unwrap();
        assert_eq!(health["type"], "scanner_health");
        assert_eq!(health["scanners"]["ble"]["state"], "unavailable");
        assert_eq!(health["scanners"]["wifi"]["state"], "disabled");
        assert_eq!(health["scan_requested"], true);
    }

    #[tokio::test]
    async fn command_input_changes_state_and_returns_status_in_order() {
        let (tx, rx) = mpsc::channel(1);
        let input = b"{\"cmd\":\"stop\"}\n{\"cmd\":\"status\"}\n{\"cmd\":\"set_rssi\",\"min_rssi\":-65}\n{\"cmd\":\"start\"}\n{\"cmd\":\"status\"}";
        // BufReader may bypass its buffer for large reads, so constrain the
        // underlying stream itself to one byte per read.
        struct Bytewise(&'static [u8]);
        impl io::Read for Bytewise {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                if out.is_empty() || self.0.is_empty() {
                    return Ok(0);
                }
                out[0] = self.0[0];
                self.0 = &self.0[1..];
                Ok(1)
            }
        }
        let producer = std::thread::spawn(move || {
            read_commands(io::BufReader::with_capacity(1, Bytewise(input)), tx)
        });
        let config = Arc::new(Mutex::new(FilterConfig::new()));
        let scanning = ScanControl::new(true);
        let output = RecordingWriter::default();
        command_loop(
            rx,
            config.clone(),
            &scanning,
            Instant::now(),
            output.clone(),
        )
        .await
        .unwrap();
        producer.join().unwrap().unwrap();
        assert!(scanning.load(Ordering::Relaxed));
        assert_eq!(config.lock().unwrap().min_rssi, -65);
        let bytes = output.bytes.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<_> = text
            .lines()
            .filter(|line| line.contains("\"type\":\"status\""))
            .collect();
        assert_eq!(
            text.lines()
                .filter(|line| line.contains("\"type\":\"command_result\""))
                .count(),
            5
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"scanning\":false"));
        assert!(lines[1].contains("\"scanning\":true"));
        assert_eq!(output.flushes.load(Ordering::Relaxed), 9);
    }

    #[tokio::test]
    async fn oversized_command_cannot_execute_its_suffix() {
        let (tx, mut rx) = mpsc::channel(2);
        let producer = std::thread::spawn(move || {
            let mut input = vec![b'x'; protocol::MAX_MSG_LEN + 1];
            input.extend_from_slice(b"{\"cmd\":\"stop\"}\n{\"cmd\":\"status\"}\n");
            read_commands(io::Cursor::new(input), tx)
        });
        assert!(matches!(
            rx.recv().await,
            Some(Some(HostCommand::GetStatus))
        ));
        assert!(rx.recv().await.is_none());
        producer.join().unwrap().unwrap();
    }

    #[tokio::test]
    async fn command_errors_are_observable_without_state_changes() {
        let (tx, rx) = mpsc::channel(1);
        let producer = std::thread::spawn(move || {
            read_commands(io::Cursor::new(b"oops\n{\"cmd\":\"set_rssi\",\"min_rssi\":1}\n{\"cmd\":\"set_mode\",\"mode\":\"ap\"}\n"), tx)
        });
        let config = Arc::new(Mutex::new(FilterConfig::new()));
        let scanning = ScanControl::new(true);
        let mut output = Vec::new();
        command_loop(rx, config.clone(), &scanning, Instant::now(), &mut output)
            .await
            .unwrap();
        producer.join().unwrap().unwrap();
        let output = String::from_utf8(output).unwrap();
        for error in ["invalid_command", "invalid_argument", "unsupported"] {
            assert!(output.contains(error));
        }
        assert_eq!(output.lines().count(), 3);
        assert_eq!(config.lock().unwrap().min_rssi, -90);
        assert!(scanning.load(Ordering::Relaxed));
    }

    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    #[tokio::test]
    async fn command_output_failure_is_returned() {
        let (tx, rx) = mpsc::channel(1);
        tx.send(Some(HostCommand::GetStatus)).await.unwrap();
        drop(tx);
        let err = command_loop(
            rx,
            Arc::new(Mutex::new(FilterConfig::new())),
            &ScanControl::new(true),
            Instant::now(),
            FailingWriter,
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn shutdown_cancels_idle_and_backpressured_scanners_and_drains_output() {
        for busy in [false, true] {
            let recording = RecordingWriter::default();
            let writer = WigleWriter::new(recording.clone()).unwrap();
            let (tx, rx) = mpsc::channel(1);
            let event = airhound::scanner::BleAdvParser::parse(&[0x56; 6], -55, &[]);
            tx.send(ScanEvent::Ble(event.clone())).await.unwrap();
            let scanner = tokio::spawn(async move {
                if busy {
                    loop {
                        if tx.send(ScanEvent::Ble(event.clone())).await.is_err() {
                            break;
                        }
                    }
                } else {
                    let _hold_sender = tx;
                    std::future::pending::<()>().await;
                }
            });
            tokio::task::yield_now().await;
            let output = tokio::spawn(filter_task(
                rx,
                Arc::new(Mutex::new(FilterConfig::new())),
                Arc::new(MacIndex::from_defaults()),
                Instant::now(),
                writer,
                Vec::new(),
                ScanControl::new(true),
            ));
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                stop_scanners(
                    scanner,
                    tokio::spawn(async {}),
                    std::time::Duration::from_secs(1),
                )
                .await;
                output.await.unwrap()
            })
            .await
            .unwrap()
            .unwrap();
            assert!(String::from_utf8(recording.bytes.lock().unwrap().clone())
                .unwrap()
                .contains("56:56:56:56:56:56"));
        }
    }

    #[tokio::test]
    async fn csv_failure_terminates_output_and_closes_scan_channel() {
        struct FailAfterHeader {
            fail: Arc<AtomicBool>,
        }
        impl Write for FailAfterHeader {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.fail.load(Ordering::Relaxed) {
                    Err(io::ErrorKind::StorageFull.into())
                } else {
                    Ok(bytes.len())
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                if self.fail.load(Ordering::Relaxed) {
                    Err(io::ErrorKind::StorageFull.into())
                } else {
                    Ok(())
                }
            }
        }
        let fail = Arc::new(AtomicBool::new(false));
        let writer = WigleWriter::new(FailAfterHeader { fail: fail.clone() }).unwrap();
        fail.store(true, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(1);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            filter_task(
                rx,
                Arc::new(Mutex::new(FilterConfig::new())),
                Arc::new(MacIndex::from_defaults()),
                Instant::now(),
                writer,
                Vec::new(),
                ScanControl::new(true),
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::StorageFull);
        assert!(tx.is_closed());
    }

    #[tokio::test(start_paused = true)]
    async fn output_flushes_while_scanners_are_idle_and_on_channel_close() {
        let recording = RecordingWriter::default();
        let writer = WigleWriter::new(recording.clone()).unwrap();
        let (tx, rx) = mpsc::channel(2);
        let output = tokio::spawn(filter_task(
            rx,
            Arc::new(Mutex::new(FilterConfig::new())),
            Arc::new(MacIndex::from_defaults()),
            Instant::now(),
            writer,
            Vec::new(),
            ScanControl::new(true),
        ));
        // Let the initial interval tick finish before queuing an observation.
        tokio::task::yield_now().await;
        let initial_flushes = recording.flushes.load(Ordering::Relaxed);
        let event = airhound::scanner::BleAdvParser::parse(&[0x12; 6], -55, &[]);
        tx.send(ScanEvent::Ble(event)).await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert!(recording.flushes.load(Ordering::Relaxed) > initial_flushes);
        let bytes = recording.bytes.lock().unwrap().clone();
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains("12:12:12:12:12:12"));

        let event = airhound::scanner::BleAdvParser::parse(&[0x34; 6], -55, &[]);
        tx.send(ScanEvent::Ble(event)).await.unwrap();
        drop(tx);
        output.await.unwrap().unwrap();
        let bytes = recording.bytes.lock().unwrap().clone();
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains("34:34:34:34:34:34"));
    }
    fn ble_match() -> ScanEvent {
        let mut event = airhound::scanner::BleAdvParser::parse(&[0x12; 6], -45, &[]);
        event.name.push_str("FS Ext Battery").unwrap();
        event.service_uuids_16.push(0x180a).unwrap();
        ScanEvent::Ble(event)
    }

    fn wifi_match() -> ScanEvent {
        ScanEvent::WiFi(airhound::scanner::WiFiEvent {
            mac: [0x34; 6],
            ssid: "Flock-123456".try_into().unwrap(),
            rssi: -50,
            channel: 6,
            frame_type: airhound::scanner::FrameType::Beacon,
            raw_ies: Default::default(),
            security: "[WPA2-PSK]".try_into().unwrap(),
        })
    }

    fn recorded(writer: &RecordingWriter) -> String {
        String::from_utf8(writer.bytes.lock().unwrap().clone()).unwrap()
    }

    #[tokio::test]
    async fn session_records_all_observations_but_only_emits_matches() {
        let csv = RecordingWriter::default();
        let output = RecordingWriter::default();
        run_session(
            SessionPolicy::native(),
            FilterConfig::new(),
            WigleWriter::new(csv.clone()).unwrap(),
            output.clone(),
            Scanners {
                ble: |tx: mpsc::Sender<ScanEvent>, _control| async move {
                    tx.send(ble_match()).await.unwrap();
                    tx.send(ScanEvent::Ble(airhound::scanner::BleAdvParser::parse(
                        &[0x56; 6],
                        -55,
                        &[],
                    )))
                    .await
                    .unwrap();
                    let ScanEvent::Ble(mut weak) = ble_match() else {
                        unreachable!()
                    };
                    weak.rssi = -110;
                    tx.send(ScanEvent::Ble(weak)).await.unwrap();
                    Ok(())
                },
                wifi: |tx: mpsc::Sender<ScanEvent>, _control| async move {
                    tx.send(wifi_match()).await.unwrap();
                    let ScanEvent::WiFi(mut other) = wifi_match() else {
                        unreachable!()
                    };
                    other.ssid = "Home".try_into().unwrap();
                    tx.send(ScanEvent::WiFi(other)).await.unwrap();
                    Ok(())
                },
            },
            |_, _, _| std::future::pending(),
            std::future::pending(),
        )
        .await
        .unwrap();
        let csv = recorded(&csv);
        assert_eq!(csv.lines().count(), 7, "{csv}");
        assert!(csv.contains("56:56:56:56:56:56"));
        assert!(csv.contains("-110"));
        let all: Vec<serde_json::Value> = recorded(&output)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let observations: Vec<_> = all
            .iter()
            .filter(|r| r["type"] == "wifi_observation")
            .collect();
        assert_eq!(observations.len(), 2);
        assert!(observations.iter().any(|r| r["ssid"] == "Home"));
        let evidence: Vec<_> = all.iter().filter(|r| r["type"] == "detection").collect();
        assert_eq!(evidence.len(), 2);
        for e in evidence {
            assert_eq!(e["detection"]["alert"], "suppressed");
            assert!(all
                .iter()
                .any(|r| r["mac"] == e["id"] && r["ts"] == e["ts"]));
        }
        let messages = data_output(&output);
        assert_eq!(messages.lines().count(), 2, "{messages}");
        assert!(messages.contains("Flock-123456"));
        assert!(messages.contains("FS Ext Battery"));
        assert!(messages.contains("0x180A"));
        assert!(!messages.contains("Home"));
    }

    #[tokio::test]
    async fn disabled_adapters_are_not_opened_and_backend_failure_is_isolated() {
        for ble_enabled in [true, false] {
            let output = RecordingWriter::default();
            let config = FilterConfig {
                ble_enabled,
                wifi_enabled: !ble_enabled,
                ..FilterConfig::new()
            };
            run_session(
                SessionPolicy::native(),
                config,
                WigleWriter::new(Vec::new()).unwrap(),
                output.clone(),
                Scanners {
                    ble: move |_: mpsc::Sender<ScanEvent>, _control| async move {
                        assert!(ble_enabled, "disabled BLE adapter opened");
                        Err(io::Error::other("adapter unavailable"))
                    },
                    wifi: move |tx: mpsc::Sender<ScanEvent>, _control| async move {
                        assert!(!ble_enabled, "disabled WiFi adapter opened");
                        tx.send(wifi_match()).await.unwrap();
                        Err(io::Error::other("adapter disconnected"))
                    },
                },
                |_, _, _| std::future::pending(),
                std::future::pending(),
            )
            .await
            .unwrap();
            assert_eq!(
                data_output(&output).lines().count(),
                usize::from(!ble_enabled)
            );
        }
        let output = RecordingWriter::default();
        run_session(
            SessionPolicy::native(),
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            output.clone(),
            Scanners {
                ble: |_, _| async { Err(io::Error::other("permission denied")) },
                wifi: |tx: mpsc::Sender<ScanEvent>, _control| async move {
                    tx.send(wifi_match()).await.unwrap();
                    Ok(())
                },
            },
            |_, _, _| std::future::pending(),
            std::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(data_output(&output).lines().count(), 1);
    }

    #[tokio::test]
    async fn session_shutdown_drains_queued_events_and_reports_signal_failure() {
        for signal_fails in [false, true] {
            let output = RecordingWriter::default();
            let (ready, wait) = tokio::sync::oneshot::channel();
            let result = run_session(
                SessionPolicy::native(),
                FilterConfig::new(),
                WigleWriter::new(Vec::new()).unwrap(),
                output.clone(),
                Scanners {
                    ble: move |tx: mpsc::Sender<ScanEvent>, _control| async move {
                        tx.send(ble_match()).await.unwrap();
                        ready.send(()).unwrap();
                        let _hold_sender = tx;
                        std::future::pending().await
                    },
                    wifi: |_, _| async { Ok(()) },
                },
                |_, _, _| std::future::pending(),
                async move {
                    wait.await.unwrap();
                    if signal_fails {
                        Err(io::ErrorKind::Interrupted.into())
                    } else {
                        Ok(())
                    }
                },
            )
            .await;
            assert_eq!(result.is_err(), signal_fails);
            if let Err(error) = result {
                assert_eq!(error.kind(), io::ErrorKind::Interrupted);
            }
            assert_eq!(data_output(&output).lines().count(), 1);
        }
    }

    #[tokio::test]
    async fn broken_event_output_stops_session_and_drops_scanners() {
        let (tx, mut dropped) = tokio::sync::mpsc::unbounded_channel();
        struct OnDrop(tokio::sync::mpsc::UnboundedSender<()>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let result = run_session(
            SessionPolicy::native(),
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            FailingWriter,
            Scanners {
                ble: move |events: mpsc::Sender<ScanEvent>, _control| async move {
                    let _drop = OnDrop(tx);
                    events.send(ble_match()).await.unwrap();
                    let _hold_sender = events;
                    std::future::pending().await
                },
                wifi: |_, _| async { Ok(()) },
            },
            |_, _, _| std::future::pending(),
            std::future::pending(),
        )
        .await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(dropped.recv().await, Some(()));
    }

    #[test]
    fn output_preparation_writes_header_and_protects_existing_session() {
        let default = default_output_path();
        let name = default.to_str().unwrap();
        assert!(name.starts_with("airhound-") && name.ends_with(".csv"));
        assert!(chrono::NaiveDateTime::parse_from_str(&name[9..24], "%Y%m%d-%H%M%S").is_ok());
        let path =
            std::env::temp_dir().join(format!("airhound-prepared-{}.csv", std::process::id()));
        let mut args = Args::try_parse_from([
            "hostd",
            "--output",
            path.to_str().unwrap(),
            "--rssi",
            "-70",
            "--no-wifi",
        ])
        .unwrap();
        let config = args.filter_config();
        assert_eq!(config.min_rssi, -70);
        assert!(config.ble_enabled && !config.wifi_enabled);
        prepare_output(&args).unwrap().flush().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"WigleWifi-1.6"));
        assert!(prepare_output(&args).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        args.overwrite = true;
        prepare_output(&args).unwrap().flush().unwrap();
        std::fs::remove_file(&path).unwrap();
        args.output = Some(path.join("missing/observations.csv"));
        assert!(prepare_output(&args).is_err());
    }
    #[tokio::test]
    async fn remote_id_pipeline_filters_invalid_weak_and_disabled_events() {
        use airhound::scanner::{OdidEvent, OdidSource};
        // ASTM operator-ID message, preceded by transport counter.
        let mut raw = vec![0, 0x52, 0];
        raw.extend_from_slice(b"GBR-OP-123ABCD");
        raw.resize(26, 0);
        let valid = OdidEvent {
            source: OdidSource::WiFiNan,
            mac: [0x78; 6],
            rssi: -50,
            raw_data: heapless::Vec::from_slice(&raw).unwrap(),
        };
        for enabled in [true, false] {
            let output = RecordingWriter::default();
            let (tx, rx) = mpsc::channel(4);
            tx.send(ScanEvent::Odid(valid.clone())).await.unwrap();
            let mut weak = valid.clone();
            weak.rssi = -120;
            tx.send(ScanEvent::Odid(weak)).await.unwrap();
            let mut invalid = valid.clone();
            invalid.raw_data.clear();
            tx.send(ScanEvent::Odid(invalid)).await.unwrap();
            drop(tx);
            filter_task(
                rx,
                Arc::new(Mutex::new(FilterConfig {
                    wifi_enabled: enabled,
                    ..FilterConfig::new()
                })),
                Arc::new(MacIndex::from_defaults()),
                Instant::now(),
                WigleWriter::new(Vec::new()).unwrap(),
                output.clone(),
                ScanControl::new(true),
            )
            .await
            .unwrap();
            let text = data_output(&output);
            assert_eq!(text.lines().count(), usize::from(enabled), "{text}");
            if enabled {
                assert!(text.contains("GBR-OP-123ABCD"));
                assert!(text.contains("wifi_nan"));
            }
        }
        let mut frame = airhound::odid::parse_odid_service_info(&raw).unwrap();
        frame.basic_id = Some(airhound::odid::BasicId {
            ua_type: airhound::odid::UaType::HelicopterOrMultirotor,
            id_type: airhound::odid::IdType::SerialNumber,
            uas_id: "DRONE-123".try_into().unwrap(),
        });
        frame.location = Some(airhound::odid::Location {
            status: airhound::odid::Status::Airborne,
            direction: 90.,
            speed_horizontal: 12.,
            speed_vertical: 0.,
            latitude: 40.,
            longitude: -105.,
            altitude_baro: 1600.,
            altitude_geo: 1610.,
            height_type: airhound::odid::HeightRef::OverTakeoff,
            height: 10.,
            horiz_accuracy: 1,
            vert_accuracy: 1,
            baro_accuracy: 1,
            speed_accuracy: 1,
            ts_accuracy: 1,
            timestamp: Some(10.),
        });
        let mut output = Vec::new();
        emit_odid(&frame, &valid, Instant::now(), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        for field in [
            "DRONE-123",
            "GBR-OP-123ABCD",
            "\"lat\":40",
            "\"lon\":-105",
            "\"alt\":1610",
            "\"speed\":12",
        ] {
            assert!(text.contains(field), "missing {field}: {text}");
        }
        assert_eq!(
            emit_odid(&frame, &valid, Instant::now(), &mut FailingWriter)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }
    #[tokio::test]
    async fn input_worker_handles_eof_and_failed_streams() {
        let config = Arc::new(Mutex::new(FilterConfig::new()));
        let scanning = ScanControl::new(true);
        let output = RecordingWriter::default();
        input_command_loop(
            io::Cursor::new(b"{\"cmd\":\"stop\"}\n{\"cmd\":\"status\"}"),
            output.clone(),
            config.clone(),
            Instant::now(),
            &scanning,
        )
        .await;
        assert!(!scanning.load(Ordering::Relaxed));
        assert_eq!(data_output(&output).lines().count(), 3);
        struct BrokenInput;
        impl io::Read for BrokenInput {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::ConnectionReset.into())
            }
        }
        input_command_loop(
            io::BufReader::new(BrokenInput),
            Vec::new(),
            config.clone(),
            Instant::now(),
            &scanning,
        )
        .await;
        input_command_loop(
            io::Cursor::new(b"{\"cmd\":\"status\"}\n"),
            FailingWriter,
            config,
            Instant::now(),
            &scanning,
        )
        .await;
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        tokio::task::spawn_blocking(move || {
            read_commands(io::Cursor::new(b"{\"cmd\":\"start\"}\n"), tx)
        })
        .await
        .unwrap()
        .unwrap();
    }

    #[test]
    fn oversized_message_fails_without_writing_partial_json() {
        static BOARD: [u8; protocol::MAX_MSG_LEN] = [b'x'; protocol::MAX_MSG_LEN];
        let board = std::str::from_utf8(&BOARD).unwrap();
        let message = DeviceMessage::Status {
            scanning: true,
            uptime: 0,
            heap_free: 0,
            ble_clients: 0,
            board,
            version: "test",
        };
        let mut output = Vec::new();
        assert!(write_message(&mut output, &message).is_err());
        assert!(output.is_empty());
        let ScanEvent::Ble(mut event) = ble_match() else {
            unreachable!()
        };
        event.service_uuids_16.clear();
        handle_ble_event(
            &event,
            &FilterConfig::new(),
            &MacIndex::from_defaults(),
            Instant::now(),
            &mut output,
        )
        .unwrap();
        assert!(!output.is_empty());
        assert!(!String::from_utf8(output).unwrap().contains("0x180A"));
    }
    #[tokio::test]
    async fn remote_id_flows_from_backend_properties_without_mac_or_signature_gate() {
        let rid_uuid = uuid::Uuid::parse_str("0000fffa-0000-1000-8000-00805f9b34fb").unwrap();
        let custom_uuid = uuid::Uuid::parse_str("1234fffa-1122-3344-5566-778899aabbcc").unwrap();
        let basic = |counter: u8| {
            let mut payload = vec![0; 27];
            payload[..4].copy_from_slice(&[0x0d, counter, 0x02, 0x12]);
            payload[4..8].copy_from_slice(b"TEST");
            payload
        };
        let hex = include_str!("../tests/fixtures/remote-id-service-data.hex").trim();
        let long: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let mut wrong_app = basic(1);
        wrong_app[0] = 0;
        let mut oversized = basic(1);
        oversized.resize(ble_payload::MAX_RECORD_BYTES + 1, 0);
        for enabled in [true, false] {
            let cases = [
                ("legacy1", rid_uuid, basic(1), Some(-45)),
                ("legacy2", rid_uuid, basic(2), Some(-45)),
                ("long", rid_uuid, long.clone(), Some(-45)),
                ("wrong_uuid", custom_uuid, basic(1), Some(-45)),
                ("wrong_app", rid_uuid, wrong_app.clone(), Some(-45)),
                ("short", rid_uuid, vec![0x0d, 1, 0], Some(-45)),
                ("truncated", rid_uuid, oversized.clone(), Some(-45)),
                ("unknown_rssi", rid_uuid, basic(3), None),
                ("weak", rid_uuid, basic(4), Some(-100)),
            ];
            let (tx, rx) = mpsc::channel(cases.len());
            for (id, uuid, payload, rssi) in cases {
                let mut props = btleplug::api::PeripheralProperties {
                    rssi,
                    ..Default::default()
                };
                props.service_data.insert(uuid, payload);
                let mut event = ble::observation(id, &props, None);
                if let ScanEvent::BleSnapshot { snapshot, .. } = &mut event {
                    snapshot.receiver_id = Some("test-receiver".into());
                }
                tx.send(event).await.unwrap();
            }
            drop(tx);
            let csv = RecordingWriter::default();
            let output = RecordingWriter::default();
            let mut config = FilterConfig::new();
            config.ble_enabled = enabled;
            filter_task(
                rx,
                Arc::new(Mutex::new(config)),
                Arc::new(MacIndex::from_defaults()),
                Instant::now(),
                WigleWriter::new(csv.clone()).unwrap(),
                output.clone(),
                ScanControl::new(true),
            )
            .await
            .unwrap();
            let messages: Vec<serde_json::Value> = data_output(&output)
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let decoded: Vec<_> = messages
                .iter()
                .filter(|m| m["type"] == "ble_remote_id")
                .collect();
            assert_eq!(decoded.len(), if enabled { 4 } else { 0 });
            if enabled {
                assert_eq!(decoded[0]["message_counter"], 1);
                assert_eq!(decoded[1]["message_counter"], 2);
                assert_eq!(decoded[2]["uas_id"], "SSEVTFG93700070");
                assert_eq!(decoded[3]["peripheral_id"], "unknown_rssi");
                assert!(decoded[3].get("rssi").is_none());
                let ordinary = messages
                    .iter()
                    .find(|m| {
                        m["type"] == "ble_observation" && m["peripheral_id"] == "unknown_rssi"
                    })
                    .unwrap();
                assert!(ordinary["match"].as_array().unwrap().is_empty());
                for value in decoded {
                    assert!(value.get("address").is_none());
                    assert!(value.get("mac").is_none());
                    assert_eq!(value["receiver_id"], "test-receiver");
                    assert_eq!(value["packet_completeness"], "unknown");
                }
            }
            assert_eq!(
                recorded(&csv).lines().count(),
                2,
                "UUID identity must not fabricate CSV addresses"
            );
        }
    }

    #[tokio::test]
    async fn backend_payloads_survive_conversion_filtering_and_output() {
        let mut props = btleplug::api::PeripheralProperties {
            rssi: Some(-45),
            ..Default::default()
        };
        let custom = uuid::Uuid::parse_str("1234fffa-1122-3344-5566-778899aabbcc").unwrap();
        props.services = vec![custom];
        props.manufacturer_data.insert(0, vec![]);
        let mut find_my = vec![0; 27];
        find_my[..2].copy_from_slice(&[0x12, 0x19]);
        props.manufacturer_data.insert(0x4c, find_my);
        props.service_data.insert(custom, vec![0x12, 0x19]);
        let (tx, rx) = mpsc::channel(2);
        tx.send(ble::observation("known-platform-id", &props, None))
            .await
            .unwrap();
        props.manufacturer_data.remove(&0x4c);
        // Identical bytes in the wrong service/company context must not match.
        props.manufacturer_data.insert(1, vec![0x12, 0x19, 0xaa]);
        tx.send(ble::observation("negative-platform-id", &props, None))
            .await
            .unwrap();
        drop(tx);
        let csv = RecordingWriter::default();
        let output = RecordingWriter::default();
        filter_task(
            rx,
            Arc::new(Mutex::new(FilterConfig::new())),
            Arc::new(MacIndex::from_defaults()),
            Instant::now(),
            WigleWriter::new(csv.clone()).unwrap(),
            output.clone(),
            ScanControl::new(true),
        )
        .await
        .unwrap();
        let messages: Vec<serde_json::Value> = data_output(&output)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["peripheral_id"], "known-platform-id");
        assert!(messages[0]["address"].is_null());
        assert_eq!(
            messages[0]["advertisement"]["advertised_services"][0],
            custom.to_string()
        );
        assert_eq!(
            messages[0]["advertisement"]["manufacturer_data"][0]["company_id"],
            0
        );
        assert_eq!(
            messages[0]["advertisement"]["service_data"][0]["payload"],
            serde_json::json!([0x12, 0x19])
        );
        assert_eq!(messages[0]["advertisement"]["freshness"], "possibly_cached");
        assert!(messages[0]["match"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["type"] == "ble_ad_bytes"));
        assert!(messages[1]["match"].as_array().unwrap().is_empty());
        assert_eq!(recorded(&csv).lines().count(), 2);
    }

    #[tokio::test]
    async fn peripheral_ids_are_preserved_without_fake_mac_exports() {
        let (tx, rx) = mpsc::channel(3);
        let ScanEvent::Ble(mut event) = ble_match() else {
            unreachable!()
        };
        event.mac = [0; 6];
        for id in [
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222",
        ] {
            tx.send(ScanEvent::BlePeripheral {
                event: event.clone(),
                id: id.into(),
                address: None,
            })
            .await
            .unwrap();
            event.name.clear();
            event.service_uuids_16.clear();
        }
        tx.send(ScanEvent::BlePeripheral {
            event,
            id: "33333333-3333-3333-3333-333333333333".into(),
            address: Some([0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]),
        })
        .await
        .unwrap();
        drop(tx);
        let csv = RecordingWriter::default();
        let output = RecordingWriter::default();
        filter_task(
            rx,
            Arc::new(Mutex::new(FilterConfig::new())),
            Arc::new(MacIndex::from_defaults()),
            Instant::now(),
            WigleWriter::new(csv.clone()).unwrap(),
            output.clone(),
            ScanControl::new(true),
        )
        .await
        .unwrap();
        assert_eq!(
            recorded(&csv).lines().count(),
            3,
            "UUID-only observations must not become WiGLE MAC rows"
        );
        let messages: Vec<serde_json::Value> = data_output(&output)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2]["address"], "12:34:56:78:9A:BC");
        assert_eq!(messages[2]["address_type"], "unknown");
        assert!(!recorded(&csv).contains("00:00:00:00:00:00"));
        assert!(recorded(&csv).contains("12:34:56:78:9A:BC"));
        assert_ne!(messages[0]["peripheral_id"], messages[1]["peripheral_id"]);
        assert!(messages
            .iter()
            .all(|m| m["type"] == "ble_observation" && m.get("mac").is_none()));
        assert!(!messages[0]["match"].as_array().unwrap().is_empty());
        assert!(messages[1]["match"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn shutdown_allows_cooperative_scanner_cleanup() {
        let (started, ready) = tokio::sync::oneshot::channel();
        let (cleaned, cleanup) = tokio::sync::oneshot::channel();
        run_session(
            SessionPolicy::native(),
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            Vec::new(),
            Scanners {
                ble: move |_tx, control: ScanControl| async move {
                    let mut state = control.subscribe();
                    started.send(()).unwrap();
                    state.changed().await.unwrap();
                    assert!(state.borrow().shutdown);
                    cleaned.send(()).unwrap();
                    Ok(())
                },
                wifi: |_, _| async { Ok(()) },
            },
            |_, _, _| std::future::pending(),
            async {
                ready.await.unwrap();
                Ok(())
            },
        )
        .await
        .unwrap();
        cleanup.await.unwrap();
    }
    #[tokio::test]
    async fn external_signal_estimates_preserve_provenance_and_never_enter_csv() {
        let record = serde_json::json!({"schema_version":1,"type":"beacon","source":"mt7610u-lab","session_id":"synthetic","sequence":0,"bssid":"02:00:00:00:00:01","ssid_bytes":b"Flock-123456".to_vec(),"advertised_channel":6,"receive_channel":1,"elapsed_ms":22,"observed_at_unix_ms":1700000000022u64,"dwell_elapsed_ms":22,"channel_epoch":0,"rssi_dbm":null,"estimated_rssi_dbm":-121,"rssi_validated":false,"rssi_estimate_basis":"mt76_eeprom_formula","raw_rssi":[146,0,0,0],"full_calibration":false});
        let event = external_wifi::SessionValidator::default()
            .accept(&serde_json::to_vec(&record).unwrap())
            .unwrap();
        let csv = RecordingWriter::default();
        let output = RecordingWriter::default();
        let (tx, rx) = mpsc::channel(2);
        tx.send(ScanEvent::ExternalWiFi {
            event: event.clone(),
            generation: 0,
        })
        .await
        .unwrap();
        tx.send(ScanEvent::ExternalWiFi {
            event,
            generation: 2,
        })
        .await
        .unwrap();
        drop(tx);
        let control = ScanControl::new(true);
        control.store(false, Ordering::Relaxed);
        control.store(true, Ordering::Relaxed);
        filter_task(
            rx,
            Arc::new(Mutex::new(FilterConfig::new())),
            Arc::new(MacIndex::from_defaults()),
            Instant::now(),
            WigleWriter::new(csv.clone()).unwrap(),
            output.clone(),
            control,
        )
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&data_output(&output)).unwrap();
        assert_eq!(value["type"], "external_wifi_observation");
        assert!(value["rssi_dbm"].is_null());
        assert_eq!(value["estimated_rssi_dbm"], -121);
        assert_eq!(value["rssi_validated"], false);
        assert_eq!(value["observed_at_unix_ms"], 1700000000022u64);
        assert_eq!(value["filter_policy"], "identity_only_unvalidated");
        assert!(!value["match"].as_array().unwrap().is_empty());
        assert!(!recorded(&csv).contains("02:00:00:00:00:01"));
        assert!(!recorded(&csv).contains("Flock-123456"));
    }

    #[tokio::test]
    async fn external_backend_failure_is_fatal_and_reports_shutdown_failure() {
        let result = run_session(
            SessionPolicy::external(),
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            Vec::new(),
            Scanners {
                ble: |_, _| async { Ok(()) },
                wifi: |_, _| async { Err(io::Error::other("companion cleanup unverified")) },
            },
            |_, _, _| std::future::pending(),
            std::future::pending(),
        )
        .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("companion cleanup unverified"));
    }
    #[test]
    fn packet_source_cli_constraints() {
        assert!(
            Args::try_parse_from(["airhound", "--ble-pcap", "capture.pcap", "--no-wifi"]).is_ok()
        );
        for flags in [
            vec!["--ble-pcap", "x", "--no-ble"],
            vec!["--sniffle-port", "x"],
            vec![
                "--sniffle-port",
                "x",
                "--sniffle-helper",
                "helper",
                "--ble-adapter",
                "hci0",
            ],
            vec!["--sniffle-helper", "helper"],
            vec!["--sniffle-channel", "40"],
        ] {
            let mut args = vec!["airhound"];
            args.extend(flags);
            assert!(Args::try_parse_from(args).is_err());
        }
    }
    #[tokio::test]
    async fn raw_packet_pipeline_attaches_rule_local_evidence_and_preserves_capture_context() {
        let (tx, rx) = mpsc::channel(8);
        let ad = [6, 9, b'F', b'l', b'o', b'c', b'k', 3, 0xff, 0xc8, 0x09];
        let mut packet = ble_packet::tests::packet(&ad);
        packet.replay = true;
        packet.source_id = "fixture".into();
        packet.generation = 2;
        packet.sequence = 7;
        tx.send(ScanEvent::BlePacket(Box::new(packet.clone())))
            .await
            .unwrap();
        let mut incomplete = packet.clone();
        incomplete.packet_completeness = "extended_chain_incomplete";
        tx.send(ScanEvent::BlePacket(Box::new(incomplete)))
            .await
            .unwrap();
        let mut unknown = packet.clone();
        unknown.rssi = None;
        tx.send(ScanEvent::BlePacket(Box::new(unknown)))
            .await
            .unwrap();
        drop(tx);
        let csv = RecordingWriter::default();
        let output = RecordingWriter::default();
        filter_task(
            rx,
            Arc::new(Mutex::new(FilterConfig::new())),
            Arc::new(MacIndex::from_defaults()),
            Instant::now(),
            WigleWriter::new(csv.clone()).unwrap(),
            output.clone(),
            ScanControl::new(true),
        )
        .await
        .unwrap();
        let messages: Vec<serde_json::Value> = data_output(&output)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(messages.len(), 3);
        let m = &messages[0];
        assert_eq!(m["type"], "ble_packet_observation");
        assert_eq!(m["source_id"], "fixture");
        assert_eq!(m["captured_at_us"], 1_700_000_000_123_456u64);
        assert_eq!(m["generation"], 2);
        assert_eq!(m["sequence"], 7);
        assert_eq!(m["detection"]["basis"], "observed_fields");
        assert!(!m["detection"]["rules"].as_array().unwrap().is_empty());
        assert!(messages[1]["detection"].is_null());
        assert!(messages[2]["detection"].is_null());
        let rows = recorded(&csv);
        assert_eq!(rows.lines().count(), 3);
        assert!(rows.contains("2023-11-14 22:13:20"));
    }
    #[tokio::test]
    async fn explicit_ble_source_failure_is_fatal() {
        let result = run_session(
            SessionPolicy::native().with_fatal_ble(true),
            FilterConfig::new(),
            WigleWriter::new(Vec::new()).unwrap(),
            Vec::new(),
            Scanners {
                ble: |_tx, _control| async { Err(io::Error::other("fixture source failure")) },
                wifi: |_tx, _control| async { std::future::pending::<io::Result<()>>().await },
            },
            |_, _, _| async {},
            std::future::pending(),
        )
        .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("fixture source failure"));
    }
    #[tokio::test]
    async fn queued_replay_survives_pause_and_live_queue_drains_on_shutdown() {
        for replay in [false, true] {
            let control = ScanControl::new(true);
            let mut packet = ble_packet::tests::packet(&[]);
            packet.replay = replay;
            let (tx, rx) = mpsc::channel(2);
            tx.send(ScanEvent::BlePacket(Box::new(packet)))
                .await
                .unwrap();
            drop(tx);
            if replay {
                control.store(false, Ordering::Relaxed);
            } else {
                control.shutdown();
            }
            let output = RecordingWriter::default();
            let consume = filter_task(
                rx,
                Arc::new(Mutex::new(FilterConfig::new())),
                Arc::new(MacIndex::from_defaults()),
                Instant::now(),
                WigleWriter::new(Vec::new()).unwrap(),
                output.clone(),
                control.clone(),
            );
            let resume = async {
                if replay {
                    tokio::task::yield_now().await;
                    control.store(true, Ordering::Relaxed);
                }
            };
            let (result, ()) = tokio::join!(consume, resume);
            result.unwrap();
            assert!(data_output(&output).contains("ble_packet_observation"));
        }
    }
}
