//! AirHound host daemon — WiFi + BLE surveillance detection and WiGLE CSV logger.
//!
//! Scans for BLE advertisements and WiFi networks, logs ALL observations to
//! WiGLE CSV v1.6 format, and outputs NDJSON matches to stdout for matched
//! surveillance devices.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

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

/// Serialize and flush one NDJSON message, reporting serialization and I/O failures.
fn write_message(out: &mut impl Write, msg: &DeviceMessage<'_>) -> io::Result<()> {
    let mut buf = [0u8; protocol::MAX_MSG_LEN];
    let len = comm::serialize_message(msg, &mut buf)
        .ok_or_else(|| io::Error::other("NDJSON message exceeds output buffer"))?;
    out.write_all(&buf[..len])?;
    out.flush()
}

fn emit_message(msg: &DeviceMessage<'_>) {
    if let Err(e) = write_message(&mut io::stdout().lock(), msg) {
        log::error!("NDJSON output failed: {e}");
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
    let mut filter_handle = tokio::spawn(async move {
        filter_task(
            scan_rx,
            filter_config,
            filter_mac_index,
            filter_start,
            wigle_writer,
        )
        .await
    });

    // Spawn stdin command reader
    let cmd_config = config.clone();
    let cmd_start = start_time;
    let stdin_handle = tokio::spawn(async move {
        stdin_command_loop(cmd_config, cmd_start).await;
    });

    let ble_abort = ble_handle.abort_handle();
    let wifi_abort = wifi_handle.abort_handle();

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
            if let Err(e) = shutdown_scanners(&[ble_abort, wifi_abort], filter_handle).await {
                log::error!("Output task failed during shutdown: {e}");
            }
        }
        result = &mut filter_handle => {
            ble_abort.abort();
            wifi_abort.abort();
            match result {
                Ok(Ok(())) => log::info!("All scanners exited, shutting down"),
                Ok(Err(e)) => log::error!("CSV output failed: {e}"),
                Err(e) => log::error!("Output task failed: {e}"),
            }
        }
    }
}

async fn shutdown_scanners(
    scanners: &[tokio::task::AbortHandle],
    output: tokio::task::JoinHandle<io::Result<()>>,
) -> io::Result<()> {
    for scanner in scanners {
        scanner.abort();
    }
    // Aborted scanners drop their senders; drain observations before flushing.
    output.await.map_err(io::Error::other)?
}

/// Filter task: receives ScanEvents, writes ALL to WiGLE CSV, emits NDJSON for matches.
async fn filter_task<W: Write>(
    mut rx: mpsc::Receiver<ScanEvent>,
    config: Arc<Mutex<FilterConfig>>,
    mac_index: Arc<MacIndex>,
    start_time: Instant,
    mut wigle: WigleWriter<W>,
) -> io::Result<()> {
    let mut mac_buf = protocol::MacString::new();
    let mut flush_tick = tokio::time::interval(std::time::Duration::from_secs(5));
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let event = tokio::select! {
            event = rx.recv() => match event {
                Some(event) => event,
                None => break,
            },
            _ = flush_tick.tick() => {
                wigle.flush()?;
                continue;
            }
        };
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
                wigle.write_ble(
                    &mac_buf,
                    ble_event.name.as_str(),
                    &now,
                    ble_event.rssi,
                    ble_event.manufacturer_id,
                    location.as_ref(),
                )?;

                // Run filter and emit NDJSON if matched
                handle_ble_event(ble_event, &cfg, &mac_index, start_time);
            }
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
                handle_wifi_event(wifi_event, &cfg, &mac_index, start_time);
            }
            #[allow(unreachable_patterns)]
            _ => {}
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
    scanning_state: &AtomicBool,
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
async fn stdin_command_loop(config: Arc<Mutex<FilterConfig>>, start_time: Instant) {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    std::thread::spawn(move || {
        if let Err(e) = read_commands(io::stdin().lock(), cmd_tx) {
            log::error!("Command input failed: {e}");
        }
    });
    if let Err(e) = command_loop(cmd_rx, config, &SCANNING, start_time, io::stdout()).await {
        log::error!("Command output failed: {e}");
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
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
        let scanning = AtomicBool::new(true);
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
        assert_eq!(output.flushes.load(Ordering::Relaxed), 7);
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
        let scanning = AtomicBool::new(true);
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
            &AtomicBool::new(true),
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
            ));
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                shutdown_scanners(&[scanner.abort_handle()], output),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(scanner.await.unwrap_err().is_cancelled());
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
}
