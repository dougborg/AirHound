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

// Hardware-specific modules (binary crate only)
#[cfg(feature = "m5stickc")]
mod buzzer;
#[cfg(feature = "m5stickc")]
mod display;

// Re-export library modules so binary submodules (display, buzzer) can use crate::*
pub(crate) use airhound::{board, comm, defaults, filter, protocol, scanner};

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::timer::timg::TimerGroup;
use static_cell::StaticCell;

use trouble_host::prelude::*;

use comm::LineReader;
use filter::{filter_ble, filter_wifi, format_mac, BleScanInput, FilterConfig, WiFiScanInput};
use protocol::{DeviceMessage, HostCommand, MacString, MsgBuffer, MAX_MSG_LEN, VERSION};
use scanner::{BleEvent, ScanEvent, WiFiEvent};

// ── BLE GATT server definition ──────────────────────────────────────
//
// Moved from comm.rs — proc macros depend on trouble-host which is
// firmware-only. The UUID constants in comm::ble_uuids are the canonical
// source; proc macros require string literals.

#[gatt_service(uuid = "4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d")]
struct AirHoundGattService {
    /// TX — filtered scan results, notify-only.
    /// Messages are chunked into BLE_MAX_NOTIFY-sized pieces.
    /// The companion accumulates until it sees '\n' (NDJSON delimiter).
    #[characteristic(uuid = "4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d", notify)]
    tx: [u8; 20],

    /// RX — host commands, write-only.
    /// Companion sends NDJSON commands which are accumulated via LineReader.
    #[characteristic(uuid = "4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d", write)]
    rx: [u8; 20],
}

/// Top-level AirHound GATT server.
#[gatt_server]
struct AirHoundServer {
    airhound_service: AirHoundGattService,
}

// ── Channel type aliases ──────────────────────────────────────────────

type ScanChannel = Channel<CriticalSectionRawMutex, ScanEvent, 16>;
type OutputChannel = Channel<CriticalSectionRawMutex, MsgBuffer, 8>;
type BleOutputChannel = Channel<CriticalSectionRawMutex, MsgBuffer, 4>;
type CommandChannel = Channel<CriticalSectionRawMutex, HostCommand, 4>;

// ── Static channels and shared state ─────────────────────────────────

/// Static channel for scan events from WiFi sniffer ISR + BLE scan task
pub(crate) static SCAN_CHANNEL: ScanChannel = Channel::new();

/// Static channel for serialized output messages
static OUTPUT_CHANNEL: OutputChannel = Channel::new();

/// Static channel for host commands
static CMD_CHANNEL: CommandChannel = Channel::new();

/// Static channel for BLE output — serial task clones messages here
/// for the GATT server to send as notifications.
static BLE_OUTPUT_CHANNEL: BleOutputChannel = Channel::new();

/// Static filter config — shared between tasks via critical-section Mutex.
/// Safe on Embassy's single-threaded executor; the Mutex only guards against
/// ISR access (WiFi sniffer callback).
static FILTER_CONFIG: Mutex<Cell<FilterConfig>> = Mutex::new(Cell::new(FilterConfig::new()));

/// Whether scanning is active (toggled by host Start/Stop commands)
pub(crate) static SCANNING: AtomicBool = AtomicBool::new(true);

/// Number of connected BLE clients
static BLE_CLIENTS: AtomicU8 = AtomicU8::new(0);

/// Match counters for display
pub(crate) static WIFI_MATCH_COUNT: AtomicU32 = AtomicU32::new(0);
pub(crate) static BLE_MATCH_COUNT: AtomicU32 = AtomicU32::new(0);

/// Last match description for display
pub(crate) static LAST_MATCH: Mutex<RefCell<heapless::String<32>>> =
    Mutex::new(RefCell::new(heapless::String::new()));

/// Whether the buzzer is enabled (M5StickC only)
#[cfg(feature = "m5stickc")]
pub(crate) static BUZZER_ENABLED: AtomicBool = AtomicBool::new(true);

/// Signal channel for buzzer beeps (M5StickC only)
#[cfg(feature = "m5stickc")]
pub(crate) static BUZZER_SIGNAL: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();

/// Get a snapshot of the current filter config.
fn get_filter_config() -> FilterConfig {
    critical_section::with(|cs| FILTER_CONFIG.borrow(cs).get())
}

// ── WiFi sniffer (moved from scanner.rs — references SCAN_CHANNEL) ──

/// WiFi sniffer callback — called from ISR context by the esp-radio sniffer.
///
/// Parses raw 802.11 frames using `parse_wifi_frame()` (ieee80211 crate)
/// and pushes matching events to the scan channel via `try_send` (non-blocking).
fn wifi_sniffer_callback(pkt: esp_radio::wifi::sniffer::PromiscuousPkt<'_>) {
    let rssi = pkt.rx_cntl.rssi as i8;
    let channel = pkt.rx_cntl.channel as u8;
    if let Some(event) = scanner::parse_wifi_frame(pkt.data, rssi, channel) {
        let _ = SCAN_CHANNEL.try_send(ScanEvent::WiFi(event));
    }
}

// FFI binding for WiFi channel control.
// The symbol is linked via esp-radio's WiFi driver.
unsafe extern "C" {
    fn esp_wifi_set_channel(primary: u8, second: u32) -> i32;
}

/// WiFi channel hop task — cycles through 2.4 GHz channels to capture
/// traffic across all channels.
#[embassy_executor::task]
async fn wifi_channel_hop_task() {
    loop {
        for &ch in scanner::WIFI_CHANNELS {
            unsafe {
                esp_wifi_set_channel(ch, 0);
            }
            Timer::after(Duration::from_millis(scanner::DEFAULT_DWELL_MS)).await;
        }
    }
}

// ── BLE scan event handler (moved from scanner.rs) ──────────────────

/// EventHandler for BLE advertisement reports from trouble-host.
///
/// Receives advertisement reports from the BLE stack runner, parses them
/// using `BleAdvParser`, and pushes results to the scan channel.
/// Called synchronously from the runner — must not block.
struct ScanEventHandler;

impl EventHandler for ScanEventHandler {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        while let Some(Ok(report)) = it.next() {
            let addr_bytes: &[u8; 6] = report.addr.raw().try_into().unwrap();
            let event = scanner::BleAdvParser::parse(addr_bytes, report.rssi, report.data);
            let _ = SCAN_CHANNEL.try_send(ScanEvent::Ble(event));
        }
    }
}

// ── Entry point ──────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Set up heap allocator (needed for BLE + WiFi coex stacks).
    // ESP32-S3 needs more heap for coex; ESP32 is tighter on DRAM.
    #[cfg(feature = "esp32")]
    {
        esp_alloc::heap_allocator!(size: 64 * 1024);
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

    // Hold power on (M5StickC Plus2 needs GPIO4 HIGH to stay powered)
    #[cfg(feature = "m5stickc")]
    let _power_hold = esp_hal::gpio::Output::new(
        peripherals.GPIO4,
        esp_hal::gpio::Level::High,
        esp_hal::gpio::OutputConfig::default(),
    );

    // Display + buzzer tasks (M5StickC only)
    #[cfg(feature = "m5stickc")]
    {
        spawner
            .spawn(display::display_task(
                peripherals.SPI2,
                peripherals.GPIO15,
                peripherals.GPIO13,
                peripherals.GPIO5,
                peripherals.GPIO14,
                peripherals.GPIO12,
                peripherals.GPIO27,
            ))
            .unwrap();
        log::info!("Display task spawned");

        spawner
            .spawn(buzzer::buzzer_task(peripherals.LEDC, peripherals.GPIO2))
            .unwrap();
        log::info!("Buzzer task spawned");
    }

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

    let connector =
        esp_radio::ble::controller::BleConnector::new(peripherals.BT, Default::default())
            .expect("BLE connector init failed");

    log::info!("BLE connector initialized");

    // ── WiFi sniffer initialization ─────────────────────────────────────

    let (_wifi_controller, wifi_interfaces) =
        esp_radio::wifi::new(peripherals.WIFI, Default::default()).expect("WiFi init failed");

    let mut sniffer = wifi_interfaces.sniffer;
    sniffer.set_receive_cb(wifi_sniffer_callback);
    sniffer
        .set_promiscuous_mode(true)
        .expect("Promiscuous mode failed");

    spawner.spawn(wifi_channel_hop_task()).unwrap();

    log::info!("WiFi sniffer initialized in promiscuous mode");

    let controller: ExternalController<_, 20> = ExternalController::new(connector);

    static HOST_RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 2>> = StaticCell::new();
    let resources = HOST_RESOURCES.init(HostResources::new());

    let address = Address::random([0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xab]);

    let stack = trouble_host::new(controller, resources).set_random_address(address);
    let Host {
        mut peripheral,
        central,
        mut runner,
        ..
    } = stack.build();

    log::info!("BLE radio initialized");

    // Create GATT server
    let server = AirHoundServer::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: comm::BLE_ADV_NAME,
        appearance: &appearance::UNKNOWN,
    }))
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
    server: &'s AirHoundServer<'_>,
) {
    let ble_rx = BLE_OUTPUT_CHANNEL.receiver();
    let mut line_reader = LineReader::new();

    loop {
        match embassy_futures::select::select(ble_rx.receive(), conn.next()).await {
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
    output_tx: &embassy_sync::channel::Sender<'_, CriticalSectionRawMutex, MsgBuffer, 8>,
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

    WIFI_MATCH_COUNT.fetch_add(1, Ordering::Relaxed);

    // Update last match description for display
    if let Some(first) = result.matches.first() {
        critical_section::with(|cs| {
            let mut s = LAST_MATCH.borrow(cs).borrow_mut();
            s.clear();
            let _ = s.push_str(&first.detail);
        });
    }

    // Trigger buzzer beep
    #[cfg(feature = "m5stickc")]
    let _ = BUZZER_SIGNAL.try_send(());

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

    let mut buf = MsgBuffer::new();
    buf.resize_default(MAX_MSG_LEN).ok();
    if let Some(len) = comm::serialize_message(&msg, &mut buf) {
        buf.truncate(len);
        let _ = output_tx.try_send(buf);
    }
}

async fn handle_ble_event(
    ble: &BleEvent,
    config: &FilterConfig,
    output_tx: &embassy_sync::channel::Sender<'_, CriticalSectionRawMutex, MsgBuffer, 8>,
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

    BLE_MATCH_COUNT.fetch_add(1, Ordering::Relaxed);

    // Update last match description for display
    if let Some(first) = result.matches.first() {
        critical_section::with(|cs| {
            let mut s = LAST_MATCH.borrow(cs).borrow_mut();
            s.clear();
            let _ = s.push_str(&first.detail);
        });
    }

    // Trigger buzzer beep
    #[cfg(feature = "m5stickc")]
    let _ = BUZZER_SIGNAL.try_send(());

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

    let mut buf = MsgBuffer::new();
    buf.resize_default(MAX_MSG_LEN).ok();
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

        let mut buf = MsgBuffer::new();
        buf.resize_default(MAX_MSG_LEN).ok();
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
        let is_status_request = matches!(cmd, HostCommand::GetStatus);

        let mut config = get_filter_config();
        let mut scanning = SCANNING.load(Ordering::Relaxed);

        let buzzer_state = comm::handle_command(&cmd, &mut config, &mut scanning);

        // Apply buzzer side effect (M5StickC only)
        #[cfg(feature = "m5stickc")]
        if let Some(enabled) = buzzer_state {
            BUZZER_ENABLED.store(enabled, Ordering::Relaxed);
        }

        // Suppress unused variable warning on boards without buzzer
        #[cfg(not(feature = "m5stickc"))]
        let _ = buzzer_state;

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

            let mut buf = MsgBuffer::new();
            buf.resize_default(MAX_MSG_LEN).ok();
            if let Some(len) = comm::serialize_message(&msg, &mut buf) {
                buf.truncate(len);
                let _ = output_tx.try_send(buf);
            }
        }
    }
}
