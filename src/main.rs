//! AirHound — RF wardriving companion device
//!
//! A thin sensor/relay that scans WiFi and BLE, filters results against
//! known surveillance device signatures, and emits matches as NDJSON
//! over BLE GATT notifications and serial.
//!
//! The companion app (DeFlock or similar) handles analysis, scoring,
//! alerting, GPS tagging, and storage.

#![no_std]
#![no_main]

extern crate alloc;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

mod board;
mod comm;
mod defaults;
mod filter;
mod protocol;
mod scanner;

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::timer::timg::TimerGroup;
use static_cell::StaticCell;

use trouble_host::prelude::*;

use crate::comm::{BleOutputChannel, LineReader};
use crate::filter::{filter_ble, filter_wifi, format_mac, BleScanInput, FilterConfig, WiFiScanInput};
use crate::protocol::{DeviceMessage, MacString, VERSION};
use crate::scanner::{BleEvent, ScanChannel, ScanEvent, ScanEventHandler, WiFiEvent};

/// Static channel for scan events from WiFi sniffer ISR + BLE scan task
pub(crate) static SCAN_CHANNEL: ScanChannel = Channel::new();

/// Static channel for serialized output messages
static OUTPUT_CHANNEL: comm::OutputChannel = Channel::new();

/// Static channel for host commands
static CMD_CHANNEL: comm::CommandChannel = Channel::new();

/// Static channel for BLE output — serial task clones messages here
/// for the GATT server to send as notifications.
static BLE_OUTPUT_CHANNEL: BleOutputChannel = Channel::new();

/// Static filter config — shared between tasks via critical-section Mutex.
/// Safe on Embassy's single-threaded executor; the Mutex only guards against
/// ISR access (WiFi sniffer callback).
static FILTER_CONFIG: Mutex<Cell<FilterConfig>> = Mutex::new(Cell::new(FilterConfig::new()));

/// Whether scanning is active (toggled by host Start/Stop commands)
static SCANNING: AtomicBool = AtomicBool::new(true);

/// Number of connected BLE clients
static BLE_CLIENTS: AtomicU8 = AtomicU8::new(0);

/// Get a snapshot of the current filter config.
fn get_filter_config() -> FilterConfig {
    critical_section::with(|cs| FILTER_CONFIG.borrow(cs).get())
}

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Set up heap allocator (needed for BLE + WiFi coex stacks).
    // ESP32-S3 needs more heap for coex; ESP32 is tighter on DRAM.
    #[cfg(feature = "esp32")]
    {
        esp_alloc::heap_allocator!(size: 72 * 1024);
    }
    #[cfg(not(feature = "esp32"))]
    {
        esp_alloc::heap_allocator!(size: 128 * 1024);
    }

    // Start the RTOS — requires timer + software interrupt
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    log::info!("AirHound v{} starting on {}", VERSION, board::BOARD_NAME);

    log::info!(
        "Filter loaded: {} MAC prefixes, {} SSID patterns, {} BLE name patterns",
        defaults::MAC_PREFIXES.len(),
        defaults::SSID_PATTERNS.len(),
        defaults::BLE_NAME_PATTERNS.len(),
    );

    // Spawn non-BLE tasks
    spawner.spawn(filter_task()).unwrap();
    spawner.spawn(output_serial_task()).unwrap();
    spawner.spawn(status_task()).unwrap();
    spawner.spawn(command_task()).unwrap();

    log::info!(
        "Build target: {}",
        if cfg!(feature = "xiao") {
            "xiao (ESP32-S3)"
        } else if cfg!(feature = "m5stickc") {
            "m5stickc (ESP32)"
        } else {
            "unknown"
        }
    );

    // ── BLE radio initialization ───────────────────────────────────────
    // BLE must be initialized BEFORE WiFi for coexistence to work
    // (especially on ESP32-S3).

    let connector = esp_radio::ble::controller::BleConnector::new(
        peripherals.BT,
        Default::default(),
    )
    .expect("BLE connector init failed");

    log::info!("BLE connector initialized");

    // ── WiFi sniffer initialization ─────────────────────────────────────

    let (_wifi_controller, wifi_interfaces) = esp_radio::wifi::new(
        peripherals.WIFI,
        Default::default(),
    )
    .expect("WiFi init failed");

    let mut sniffer = wifi_interfaces.sniffer;
    sniffer.set_receive_cb(scanner::wifi_sniffer_callback);
    sniffer.set_promiscuous_mode(true).expect("Promiscuous mode failed");

    spawner.spawn(scanner::wifi_channel_hop_task()).unwrap();

    log::info!("WiFi sniffer initialized in promiscuous mode");

    let controller: ExternalController<_, 20> = ExternalController::new(connector);

    static HOST_RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 2>> = StaticCell::new();
    let resources = HOST_RESOURCES.init(HostResources::new());

    let address = Address::random([0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xab]);

    let stack = trouble_host::new(controller, resources)
        .set_random_address(address);
    let Host {
        mut peripheral,
        central,
        mut runner,
        ..
    } = stack.build();

    log::info!("BLE radio initialized");

    // Create GATT server
    let server = comm::AirHoundServer::new_with_config(
        GapConfig::Peripheral(PeripheralConfig {
            name: comm::BLE_ADV_NAME,
            appearance: &appearance::UNKNOWN,
        }),
    )
    .expect("GATT server init failed");

    // Event handler for BLE advertisement reports
    let scan_handler = ScanEventHandler;

    // ── BLE orchestration ──────────────────────────────────────────────
    //
    // Three concurrent futures via join3:
    //   1. BLE stack runner (drives HCI, delivers scan reports to handler)
    //   2. BLE scanner (starts scan, keeps session alive)
    //   3. GATT server (advertise, accept connections, send notifications)

    let _ = embassy_futures::join::join3(
        // ── Runner: drives the BLE stack ────────────────────────────────
        async {
            loop {
                if let Err(e) = runner.run_with_handler(&scan_handler).await {
                    log::error!("BLE runner error: {:?}", e);
                    Timer::after(Duration::from_secs(1)).await;
                }
            }
        },
        // ── Scanner: start BLE scan and keep session alive ──────────────
        async {
            let mut scanner = trouble_host::scan::Scanner::new(central);
            let config = ScanConfig::default();

            let result = scanner.scan(&config).await;
            let _session = match result {
                Ok(session) => session,
                Err(e) => {
                    log::error!("BLE scan failed to start: {:?}", e);
                    return;
                }
            };

            log::info!("BLE scan started (active, continuous)");
            // Session stays alive as long as _session exists.
            // Reports flow through ScanEventHandler on the runner.
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        },
        // ── GATT server: advertise, connect, notify ─────────────────────
        async {
            loop {
                // Build advertisement data
                let mut adv_data = [0u8; 31];
                let adv_len = match AdStructure::encode_slice(
                    &[
                        AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                        AdStructure::CompleteLocalName(comm::BLE_ADV_NAME.as_bytes()),
                    ],
                    &mut adv_data[..],
                ) {
                    Ok(len) => len,
                    Err(e) => {
                        log::error!("Ad encode error: {:?}", e);
                        Timer::after(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                // Start advertising
                let advertiser = match peripheral
                    .advertise(
                        &Default::default(),
                        Advertisement::ConnectableScannableUndirected {
                            adv_data: &adv_data[..adv_len],
                            scan_data: &[],
                        },
                    )
                    .await
                {
                    Ok(adv) => adv,
                    Err(e) => {
                        log::error!("BLE advertise error: {:?}", e);
                        Timer::after(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                log::info!("BLE advertising as '{}'", comm::BLE_ADV_NAME);

                // Wait for a central to connect
                let conn = match advertiser.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("BLE accept error: {:?}", e);
                        continue;
                    }
                };

                let gatt_conn = match conn.with_attribute_server(&server) {
                    Ok(gc) => gc,
                    Err(e) => {
                        log::error!("GATT setup error: {:?}", e);
                        continue;
                    }
                };

                log::info!("BLE client connected");
                BLE_CLIENTS.fetch_add(1, Ordering::Relaxed);

                // Handle the connection until disconnect
                handle_gatt_connection(&gatt_conn, &server).await;

                BLE_CLIENTS.fetch_sub(1, Ordering::Relaxed);
                log::info!("BLE client disconnected, re-advertising");
            }
        },
    )
    .await;
}

/// Handle a GATT connection: forward output messages as notifications
/// and process incoming writes as host commands.
async fn handle_gatt_connection<'s, P: PacketPool>(
    conn: &GattConnection<'_, 's, P>,
    server: &'s comm::AirHoundServer<'_>,
) {
    let ble_rx = BLE_OUTPUT_CHANNEL.receiver();
    let mut line_reader = LineReader::new();

    loop {
        match embassy_futures::select::select(
            ble_rx.receive(),
            conn.next(),
        )
        .await
        {
            embassy_futures::select::Either::First(msg) => {
                // Chunk the NDJSON message into BLE_MAX_NOTIFY-sized pieces.
                // Pad with newlines so the companion NDJSON parser sees
                // harmless empty lines instead of null bytes.
                for chunk in msg.chunks(comm::BLE_MAX_NOTIFY) {
                    let mut padded = [b'\n'; 20];
                    padded[..chunk.len()].copy_from_slice(chunk);
                    if server
                        .airhound_service
                        .tx
                        .notify(conn, &padded)
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            embassy_futures::select::Either::Second(event) => {
                match event {
                    GattConnectionEvent::Disconnected { .. } => return,
                    GattConnectionEvent::Gatt { event } => {
                        // Check if this is a write to our RX characteristic
                        if let GattEvent::Write(ref write_event) = event {
                            if write_event.handle() == server.airhound_service.rx.handle {
                                for &byte in write_event.data() {
                                    if let Some(line) = line_reader.feed(byte) {
                                        if let Some(cmd) = comm::parse_command(line) {
                                            let _ = CMD_CHANNEL.try_send(cmd);
                                        }
                                    }
                                }
                            }
                        }
                        // Must accept/reply to all GATT events
                        match event.accept() {
                            Ok(reply) => reply.send().await,
                            Err(_) => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Filter task — receives raw scan events, applies filters, and serializes
/// matching results to the output channel.
#[embassy_executor::task]
async fn filter_task() {
    log::info!("Filter task started");

    let scan_rx = SCAN_CHANNEL.receiver();
    let output_tx = OUTPUT_CHANNEL.sender();

    loop {
        let event = scan_rx.receive().await;

        if !SCANNING.load(Ordering::Relaxed) {
            continue;
        }

        let config = get_filter_config();

        match event {
            ScanEvent::WiFi(ref wifi) => {
                handle_wifi_event(wifi, &config, &output_tx).await;
            }
            ScanEvent::Ble(ref ble) => {
                handle_ble_event(ble, &config, &output_tx).await;
            }
        }
    }
}

async fn handle_wifi_event(
    wifi: &WiFiEvent,
    config: &FilterConfig,
    output_tx: &embassy_sync::channel::Sender<'_, CriticalSectionRawMutex, protocol::MsgBuffer, 8>,
) {
    let input = WiFiScanInput {
        mac: &wifi.mac,
        ssid: wifi.ssid.as_str(),
        rssi: wifi.rssi,
    };

    let result = filter_wifi(&input, config);
    if !result.matched {
        return;
    }

    let mut mac_str = MacString::new();
    format_mac(&wifi.mac, &mut mac_str);

    let ts = (Instant::now().as_millis() & 0xFFFF_FFFF) as u32;

    let msg = DeviceMessage::WiFiScan {
        mac: &mac_str,
        ssid: &wifi.ssid,
        rssi: wifi.rssi,
        ch: wifi.channel,
        frame: wifi.frame_type.as_str(),
        matches: &result.matches,
        ts,
    };

    let mut buf = protocol::MsgBuffer::new();
    buf.resize_default(protocol::MAX_MSG_LEN).ok();
    if let Some(len) = comm::serialize_message(&msg, &mut buf) {
        buf.truncate(len);
        let _ = output_tx.try_send(buf);
    }
}

async fn handle_ble_event(
    ble: &BleEvent,
    config: &FilterConfig,
    output_tx: &embassy_sync::channel::Sender<'_, CriticalSectionRawMutex, protocol::MsgBuffer, 8>,
) {
    let input = BleScanInput {
        mac: &ble.mac,
        name: ble.name.as_str(),
        rssi: ble.rssi,
        service_uuids_16: &ble.service_uuids_16,
        manufacturer_id: ble.manufacturer_id,
    };

    let result = filter_ble(&input, config);
    if !result.matched {
        return;
    }

    let mut mac_str = MacString::new();
    format_mac(&ble.mac, &mut mac_str);

    let ts = (Instant::now().as_millis() & 0xFFFF_FFFF) as u32;

    let msg = DeviceMessage::BleScan {
        mac: &mac_str,
        name: &ble.name,
        rssi: ble.rssi,
        uuid: None, // TODO: format primary UUID if present
        mfr: ble.manufacturer_id,
        matches: &result.matches,
        ts,
    };

    let mut buf = protocol::MsgBuffer::new();
    buf.resize_default(protocol::MAX_MSG_LEN).ok();
    if let Some(len) = comm::serialize_message(&msg, &mut buf) {
        buf.truncate(len);
        let _ = output_tx.try_send(buf);
    }
}

/// Serial output task — reads from output channel, logs to serial,
/// and forwards a clone to the BLE output channel.
#[embassy_executor::task]
async fn output_serial_task() {
    log::info!("Serial output task started");

    let output_rx = OUTPUT_CHANNEL.receiver();

    loop {
        let msg = output_rx.receive().await;

        // Forward to BLE output channel (non-blocking, drops if full or no client)
        let _ = BLE_OUTPUT_CHANNEL.try_send(msg.clone());

        // Log to serial via esp-println
        if let Ok(s) = core::str::from_utf8(&msg) {
            log::info!("{}", s.trim_end());
        }
    }
}

/// Periodic status reporting task
#[embassy_executor::task]
async fn status_task() {
    loop {
        Timer::after(Duration::from_secs(30)).await;

        let uptime_secs = (Instant::now().as_millis() / 1000) as u32;

        let msg = DeviceMessage::Status {
            scanning: SCANNING.load(Ordering::Relaxed),
            uptime: uptime_secs,
            heap_free: esp_alloc::HEAP.free() as u32,
            ble_clients: BLE_CLIENTS.load(Ordering::Relaxed),
            board: board::BOARD_NAME,
            version: VERSION,
        };

        let mut buf = protocol::MsgBuffer::new();
        buf.resize_default(protocol::MAX_MSG_LEN).ok();
        if let Some(len) = comm::serialize_message(&msg, &mut buf) {
            buf.truncate(len);
            let _ = OUTPUT_CHANNEL.try_send(buf);
        }
    }
}

/// Host command processing task — drains CMD_CHANNEL, updates filter config
/// and scanning state, responds to status requests.
#[embassy_executor::task]
async fn command_task() {
    let cmd_rx = CMD_CHANNEL.receiver();
    let output_tx = OUTPUT_CHANNEL.sender();

    loop {
        let cmd = cmd_rx.receive().await;
        let is_status_request = matches!(cmd, protocol::HostCommand::GetStatus);

        let mut config = get_filter_config();
        let mut scanning = SCANNING.load(Ordering::Relaxed);

        comm::handle_command(cmd, &mut config, &mut scanning);

        // Write back updated state
        critical_section::with(|cs| FILTER_CONFIG.borrow(cs).set(config));
        SCANNING.store(scanning, Ordering::Relaxed);

        // GetStatus: build and send a live status response
        if is_status_request {
            let uptime_secs = (Instant::now().as_millis() / 1000) as u32;
            let msg = DeviceMessage::Status {
                scanning: SCANNING.load(Ordering::Relaxed),
                uptime: uptime_secs,
                heap_free: esp_alloc::HEAP.free() as u32,
                ble_clients: BLE_CLIENTS.load(Ordering::Relaxed),
                board: board::BOARD_NAME,
                version: VERSION,
            };

            let mut buf = protocol::MsgBuffer::new();
            buf.resize_default(protocol::MAX_MSG_LEN).ok();
            if let Some(len) = comm::serialize_message(&msg, &mut buf) {
                buf.truncate(len);
                let _ = output_tx.try_send(buf);
            }
        }
    }
}
