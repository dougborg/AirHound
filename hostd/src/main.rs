//! AirHound host daemon — WiFi + BLE surveillance detection and WiGLE CSV logger.
//!
//! Scans for BLE advertisements and WiFi networks, logs ALL observations to
//! WiGLE CSV v1.6 format, and outputs NDJSON matches to stdout for matched
//! surveillance devices.

mod ble;
#[cfg(target_os = "macos")]
mod location_macos;
#[cfg(target_os = "macos")]
mod wifi_macos;
mod wigle;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
use airhound::scanner::ScanEvent;

use wigle::{Location, WigleWriter};

static SCANNING: AtomicBool = AtomicBool::new(true);

#[derive(Parser)]
#[command(
    name = "airhound-hostd",
    about = "AirHound host daemon — WiFi/BLE scanner with WiGLE CSV output"
)]
struct Args {
    /// Output WiGLE CSV file path [default: airhound-YYYYMMDD-HHMMSS.csv]
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Serialize a `DeviceMessage` and write it as NDJSON to stdout.
fn emit_message(msg: &DeviceMessage<'_>) {
    let mut buf = [0u8; protocol::MAX_MSG_LEN];
    if let Some(len) = comm::serialize_message(msg, &mut buf) {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let _ = out.write_all(&buf[..len]);
        let _ = out.flush();
    }
}

/// Generate default output filename with current timestamp.
fn default_output_path() -> PathBuf {
    let ts = Local::now().format("%Y%m%d-%H%M%S");
    PathBuf::from(format!("airhound-{ts}.csv"))
}

/// Get the current GPS location (macOS: CoreLocation, other: always None).
fn get_location() -> Option<Location> {
    #[cfg(target_os = "macos")]
    {
        location_macos::get_current_location()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let args = Args::parse();

    log::info!(
        "AirHound hostd v{} starting on {}",
        protocol::VERSION,
        BOARD_NAME
    );

    // Initialize CoreLocation for GPS
    #[cfg(target_os = "macos")]
    location_macos::start();

    // Open WiGLE CSV output file
    let output_path = args.output.unwrap_or_else(default_output_path);
    let csv_file = match std::fs::File::create(&output_path) {
        Ok(f) => f,
        Err(e) => {
            log::error!(
                "Failed to create output file {}: {e}",
                output_path.display()
            );
            std::process::exit(1);
        }
    };
    let wigle_writer = match WigleWriter::new(csv_file) {
        Ok(w) => w,
        Err(e) => {
            log::error!("Failed to write WiGLE header: {e}");
            std::process::exit(1);
        }
    };
    log::info!("WiGLE CSV output: {}", output_path.display());

    // Shared state
    let config = Arc::new(Mutex::new(FilterConfig::new()));
    let mac_index = Arc::new(MacIndex::from_defaults());
    let start_time = Instant::now();

    log::info!(
        "Filter loaded: {} signatures, {} rules, {} MAC prefixes",
        defaults::SIG_COUNT,
        defaults::DEFAULT_RULE_DB.rules.len(),
        defaults::MAC_PREFIXES.len(),
    );

    // Unified channel: scanners → filter task
    let (scan_tx, scan_rx) = mpsc::channel::<ScanEvent>(32);

    // Spawn BLE scanner
    let ble_tx = scan_tx.clone();
    let ble_handle = tokio::spawn(async move {
        let adapter = match ble::get_adapter().await {
            Ok(a) => a,
            Err(e) => {
                log::error!("No BLE adapter found: {e}");
                return;
            }
        };
        log::info!("BLE adapter found");

        if let Err(e) = ble::scan_loop(adapter, ble_tx).await {
            log::error!("BLE scan error: {e}");
        }
    });

    // Spawn WiFi scanner (macOS: CoreWLAN; other platforms: no-op)
    let wifi_tx = scan_tx.clone();
    let wifi_handle = tokio::spawn(async move {
        #[cfg(target_os = "macos")]
        if let Err(e) = wifi_macos::scan_loop(wifi_tx).await {
            log::error!("WiFi scan error: {e}");
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = wifi_tx;
            log::info!("WiFi scanning not available on this platform");
        }
    });

    // Drop the original sender so the channel closes when all scanners exit
    drop(scan_tx);

    // Spawn filter + output task (owns wigle writer exclusively — no Arc/Mutex needed)
    let filter_config = config.clone();
    let filter_mac_index = mac_index.clone();
    let filter_start = start_time;
    let filter_handle = tokio::spawn(async move {
        filter_task(
            scan_rx,
            filter_config,
            filter_mac_index,
            filter_start,
            wigle_writer,
        )
        .await;
    });

    // Spawn stdin command reader
    let cmd_config = config.clone();
    let cmd_start = start_time;
    let stdin_handle = tokio::spawn(async move {
        stdin_command_loop(cmd_config, cmd_start).await;
    });

    // Scanner exits are non-fatal — they drop their tx handles, which eventually
    // closes the scan channel and lets filter_task drain and exit naturally.
    tokio::spawn(async move {
        let _ = ble_handle.await;
        log::warn!("BLE scanner exited");
    });
    tokio::spawn(async move {
        let _ = wifi_handle.await;
        log::warn!("WiFi scanner exited");
    });
    tokio::spawn(async move {
        let _ = stdin_handle.await;
        log::debug!("Stdin reader exited");
    });

    // Shut down on ctrl-c or when the filter task exits (all scanners gone)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            log::info!("Shutting down...");
        }
        _ = filter_handle => {
            log::info!("All scanners exited, shutting down");
        }
    }
}

/// Filter task: receives ScanEvents, writes ALL to WiGLE CSV, emits NDJSON for matches.
async fn filter_task(
    mut rx: mpsc::Receiver<ScanEvent>,
    config: Arc<Mutex<FilterConfig>>,
    mac_index: Arc<MacIndex>,
    start_time: Instant,
    mut wigle: WigleWriter<std::fs::File>,
) {
    let mut mac_buf = protocol::MacString::new();
    let mut last_flush = Instant::now();

    while let Some(event) = rx.recv().await {
        if !SCANNING.load(Ordering::Relaxed) {
            continue;
        }

        let cfg = *config.lock().unwrap_or_else(|e| e.into_inner());
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let location = get_location();

        match event {
            ScanEvent::Ble(ref ble_event) => {
                // Write observation to WiGLE CSV (all networks, not just matches)
                mac_buf.clear();
                protocol::format_mac(&ble_event.mac, &mut mac_buf);
                if let Err(e) = wigle.write_ble(
                    &mac_buf,
                    ble_event.name.as_str(),
                    &now,
                    ble_event.rssi,
                    ble_event.manufacturer_id,
                    location.as_ref(),
                ) {
                    log::warn!("WiGLE write error: {e}");
                }

                // Run filter and emit NDJSON if matched
                handle_ble_event(ble_event, &cfg, &mac_index, start_time);
            }
            ScanEvent::WiFi(ref wifi_event) => {
                // Write observation to WiGLE CSV (all networks, not just matches)
                mac_buf.clear();
                protocol::format_mac(&wifi_event.mac, &mut mac_buf);
                let freq = wigle::channel_to_frequency(wifi_event.channel);
                if let Err(e) = wigle.write_wifi(
                    &mac_buf,
                    wifi_event.ssid.as_str(),
                    wifi_event.security.as_str(),
                    &now,
                    wifi_event.channel,
                    freq,
                    wifi_event.rssi,
                    location.as_ref(),
                ) {
                    log::warn!("WiGLE write error: {e}");
                }

                // Run filter and emit NDJSON if matched
                handle_wifi_event(wifi_event, &cfg, &mac_index, start_time);
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }

        // Flush periodically (every 5s) to balance durability and I/O overhead
        if last_flush.elapsed() >= std::time::Duration::from_secs(5) {
            if let Err(e) = wigle.flush() {
                log::warn!("WiGLE flush error: {e}");
            }
            last_flush = Instant::now();
        }
    }

    // Flush remaining buffered writes on shutdown
    if let Err(e) = wigle.flush() {
        log::warn!("WiGLE flush error on shutdown: {e}");
    }
}

/// Filter and emit a BLE scan event (NDJSON on stdout if matched).
fn handle_ble_event(
    event: &airhound::scanner::BleEvent,
    cfg: &FilterConfig,
    mac_index: &MacIndex,
    start_time: Instant,
) {
    let input = BleScanInput {
        mac: &event.mac,
        name: event.name.as_str(),
        rssi: event.rssi,
        service_uuids_16: &event.service_uuids_16,
        manufacturer_id: event.manufacturer_id,
        raw_ad: &event.raw_ad,
    };

    let result = filter_ble_with_rules(&input, cfg, &defaults::DEFAULT_RULE_DB, mac_index);
    if !result.matched {
        return;
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

    emit_message(&msg);
}

/// Filter and emit a WiFi scan event (NDJSON on stdout if matched).
fn handle_wifi_event(
    event: &airhound::scanner::WiFiEvent,
    cfg: &FilterConfig,
    mac_index: &MacIndex,
    start_time: Instant,
) {
    let input = WiFiScanInput {
        mac: &event.mac,
        ssid: event.ssid.as_str(),
        rssi: event.rssi,
    };

    let result = filter_wifi_with_rules(&input, cfg, &defaults::DEFAULT_RULE_DB, mac_index);
    if !result.matched {
        return;
    }

    let mut mac_str = protocol::MacString::new();
    protocol::format_mac(&event.mac, &mut mac_str);

    let ts = start_time.elapsed().as_millis() as u32;

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

    emit_message(&msg);
}

/// Read NDJSON commands from stdin and dispatch them.
async fn stdin_command_loop(config: Arc<Mutex<FilterConfig>>, start_time: Instant) {
    // Run stdin reading in a blocking thread
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<HostCommand>(8);

    std::thread::spawn(move || {
        let stdin = io::stdin();
        let reader = stdin.lock();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if let Some(cmd) = comm::parse_command(line.as_bytes()) {
                if cmd_tx.blocking_send(cmd).is_err() {
                    break;
                }
            }
        }
    });

    while let Some(cmd) = cmd_rx.recv().await {
        let mut cfg = config.lock().unwrap_or_else(|e| e.into_inner());
        let mut scanning = SCANNING.load(Ordering::Relaxed);

        comm::handle_command(&cmd, &mut cfg, &mut scanning);
        SCANNING.store(scanning, Ordering::Relaxed);

        // Handle status request
        if matches!(cmd, HostCommand::GetStatus) {
            let uptime_secs = start_time.elapsed().as_secs() as u32;
            let msg = DeviceMessage::Status {
                scanning,
                uptime: uptime_secs,
                heap_free: 0, // not meaningful on host
                ble_clients: 0,
                board: BOARD_NAME,
                version: protocol::VERSION,
            };
            emit_message(&msg);
        }
    }
}
