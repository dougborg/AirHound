//! AirHound — ESP-IDF std firmware
//!
//! Thread-based implementation using FreeRTOS threads and std::sync::mpsc
//! channels. Feature-equivalent to the no_std Embassy firmware but uses
//! ESP-IDF services (NimBLE via esp32-nimble, WiFi via esp-idf-svc).

#[cfg(feature = "m5stickc")]
mod buzzer;
#[cfg(feature = "m5stickc")]
mod display;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use airhound::{board, comm, defaults, filter, protocol, scanner};

use comm::LineReader;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::task::block_on;
use esp_idf_svc::sys::{
    esp, esp_get_free_heap_size, esp_wifi_set_channel, esp_wifi_set_promiscuous,
    esp_wifi_set_promiscuous_rx_cb, wifi_promiscuous_pkt_t, wifi_promiscuous_pkt_type_t,
    wifi_second_chan_t_WIFI_SECOND_CHAN_NONE,
};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use filter::{filter_ble, filter_wifi, format_mac, BleScanInput, FilterConfig, WiFiScanInput};
use protocol::{DeviceMessage, HostCommand, MacString, MsgBuffer, MAX_MSG_LEN, VERSION};
use scanner::{BleEvent, ScanEvent, WiFiEvent};

use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{BLEAdvertisementData, BLEDevice, BLEScan, NimbleProperties};

// ── Shared state (same atomics as no_std) ────────────────────────────

pub(crate) static SCANNING: AtomicBool = AtomicBool::new(true);
pub(crate) static BLE_CLIENTS: AtomicU8 = AtomicU8::new(0);
pub(crate) static WIFI_MATCH_COUNT: AtomicU32 = AtomicU32::new(0);
pub(crate) static BLE_MATCH_COUNT: AtomicU32 = AtomicU32::new(0);
pub(crate) static BUZZER_ENABLED: AtomicBool = AtomicBool::new(true);
static FILTER_CONFIG: Mutex<FilterConfig> = Mutex::new(FilterConfig::new());
pub(crate) static LAST_MATCH: Mutex<heapless::String<32>> = Mutex::new(heapless::String::new());

/// Boot time — captured once in main, used for uptime calculation.
static BOOT_INSTANT: Mutex<Option<Instant>> = Mutex::new(None);

pub(crate) fn uptime_secs() -> u32 {
    BOOT_INSTANT
        .lock()
        .ok()
        .and_then(|i| i.map(|boot| boot.elapsed().as_secs() as u32))
        .unwrap_or(0)
}

fn uptime_millis_u32() -> u32 {
    BOOT_INSTANT
        .lock()
        .ok()
        .and_then(|i| i.map(|boot| (boot.elapsed().as_millis() & 0xFFFF_FFFF) as u32))
        .unwrap_or(0)
}

// ── Global scan channel sender (for WiFi promisc callback) ───────────

static SCAN_TX: Mutex<Option<SyncSender<ScanEvent>>> = Mutex::new(None);

// ── WiFi promiscuous callback ────────────────────────────────────────

/// WiFi promiscuous mode callback.
///
/// Runs in the WiFi driver task context (not ISR on ESP-IDF, but still
/// must be non-blocking). Parses raw 802.11 frames and sends events
/// to the scan channel via try_send.
unsafe extern "C" fn promisc_rx_cb(
    buf: *mut std::ffi::c_void,
    _pkt_type: wifi_promiscuous_pkt_type_t,
) {
    let pkt = unsafe { &*(buf as *const wifi_promiscuous_pkt_t) };
    let rssi = pkt.rx_ctrl.rssi() as i8;
    let channel = pkt.rx_ctrl.channel() as u8;
    let sig_len = pkt.rx_ctrl.sig_len() as usize;

    if sig_len == 0 {
        return;
    }

    // Safety: payload is `sig_len` bytes starting at pkt.payload
    let payload = unsafe { std::slice::from_raw_parts(pkt.payload.as_ptr(), sig_len) };

    if let Some(event) = scanner::parse_wifi_frame(payload, rssi, channel) {
        if let Ok(guard) = SCAN_TX.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.try_send(ScanEvent::WiFi(event));
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    // Bind the ESP-IDF logger to the `log` facade
    esp_idf_svc::log::EspLogger::initialize_default();

    // Record boot time
    *BOOT_INSTANT.lock().unwrap() = Some(Instant::now());

    log::info!("AirHound v{} starting on {} (std)", VERSION, board::BOARD_NAME);
    log::info!(
        "Filter loaded: {} MAC prefixes, {} SSID patterns, {} BLE name patterns",
        defaults::MAC_PREFIXES.len(),
        defaults::SSID_PATTERNS.len(),
        defaults::BLE_NAME_PATTERNS.len(),
    );

    // ── Peripherals ──────────────────────────────────────────────────

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // Hold power on (M5StickC Plus2)
    #[cfg(feature = "m5stickc")]
    let _power_hold = {
        use esp_idf_svc::hal::gpio::PinDriver;
        let mut p = PinDriver::output(peripherals.pins.gpio4)?;
        p.set_high()?;
        p
    };

    // ── Channels ─────────────────────────────────────────────────────

    let (scan_tx, scan_rx) = mpsc::sync_channel::<ScanEvent>(16);
    let (output_tx, output_rx) = mpsc::sync_channel::<MsgBuffer>(8);
    let (ble_output_tx, ble_output_rx) = mpsc::sync_channel::<MsgBuffer>(4);
    let (cmd_tx, cmd_rx) = mpsc::sync_channel::<HostCommand>(4);
    #[cfg(feature = "m5stickc")]
    let (buzzer_tx, buzzer_rx) = mpsc::sync_channel::<()>(1);

    // Store scan_tx globally for WiFi promisc callback
    *SCAN_TX.lock().unwrap() = Some(scan_tx.clone());

    // ── Buzzer thread (M5StickC) ─────────────────────────────────────

    #[cfg(feature = "m5stickc")]
    {
        let ledc_timer = peripherals.ledc.timer0;
        let ledc_channel = peripherals.ledc.channel0;
        let buzzer_pin = peripherals.pins.gpio2;
        thread::Builder::new()
            .name("buzzer".into())
            .stack_size(2048)
            .spawn(move || {
                buzzer::buzzer_thread(buzzer_rx, ledc_timer, ledc_channel, buzzer_pin);
            })?;
        log::info!("Buzzer thread spawned");
    }

    // ── Display thread (M5StickC) ────────────────────────────────────

    #[cfg(feature = "m5stickc")]
    {
        let spi2 = peripherals.spi2;
        let mosi = peripherals.pins.gpio15;
        let clk = peripherals.pins.gpio13;
        let cs_pin = peripherals.pins.gpio5;
        let dc_pin = peripherals.pins.gpio14;
        let rst_pin = peripherals.pins.gpio12;
        let bl_pin = peripherals.pins.gpio27;
        thread::Builder::new()
            .name("display".into())
            .stack_size(4096)
            .spawn(move || {
                display::display_thread(spi2, mosi, clk, cs_pin, dc_pin, rst_pin, bl_pin);
            })?;
        log::info!("Display thread spawned");
    }

    // ── Filter thread ────────────────────────────────────────────────

    let filter_output_tx = output_tx.clone();
    #[cfg(feature = "m5stickc")]
    let filter_buzzer_tx = buzzer_tx.clone();
    thread::Builder::new()
        .name("filter".into())
        .stack_size(4096)
        .spawn(move || {
            filter_thread(
                scan_rx,
                filter_output_tx,
                #[cfg(feature = "m5stickc")]
                filter_buzzer_tx,
            );
        })?;
    log::info!("Filter thread spawned");

    // ── Output thread ────────────────────────────────────────────────

    thread::Builder::new()
        .name("output".into())
        .stack_size(4096)
        .spawn(move || {
            output_thread(output_rx, ble_output_tx);
        })?;
    log::info!("Output thread spawned");

    // ── Command thread ───────────────────────────────────────────────

    let cmd_output_tx = output_tx.clone();
    thread::Builder::new()
        .name("command".into())
        .stack_size(4096)
        .spawn(move || {
            command_thread(cmd_rx, cmd_output_tx);
        })?;
    log::info!("Command thread spawned");

    // ── Status thread ────────────────────────────────────────────────

    let status_output_tx = output_tx.clone();
    thread::Builder::new()
        .name("status".into())
        .stack_size(4096)
        .spawn(move || {
            status_thread(status_output_tx);
        })?;
    log::info!("Status thread spawned");

    // ── WiFi sniffer ─────────────────────────────────────────────────

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;
    wifi.set_configuration(&esp_idf_svc::wifi::Configuration::Client(Default::default()))?;
    wifi.start()?;

    // Enable promiscuous mode
    unsafe {
        esp!(esp_wifi_set_promiscuous(true))?;
        esp!(esp_wifi_set_promiscuous_rx_cb(Some(promisc_rx_cb)))?;
    }
    log::info!("WiFi sniffer initialized in promiscuous mode");

    // ── Channel hop thread ───────────────────────────────────────────

    thread::Builder::new()
        .name("chanhop".into())
        .stack_size(2048)
        .spawn(move || {
            channel_hop_thread();
        })?;
    log::info!("Channel hop thread spawned");

    // ── BLE (NimBLE) — runs on main thread ───────────────────────────

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

    ble_main(scan_tx, cmd_tx, ble_output_rx);
}

// ── Channel hopping ──────────────────────────────────────────────────

fn channel_hop_thread() {
    loop {
        for &ch in scanner::WIFI_CHANNELS {
            unsafe {
                esp_wifi_set_channel(ch, wifi_second_chan_t_WIFI_SECOND_CHAN_NONE);
            }
            thread::sleep(Duration::from_millis(scanner::DEFAULT_DWELL_MS));
        }
    }
}

// ── Filter thread ────────────────────────────────────────────────────

fn filter_thread(
    scan_rx: mpsc::Receiver<ScanEvent>,
    output_tx: SyncSender<MsgBuffer>,
    #[cfg(feature = "m5stickc")] buzzer_tx: SyncSender<()>,
) {
    log::info!("Filter thread started");

    while let Ok(event) = scan_rx.recv() {
        if !SCANNING.load(Ordering::Relaxed) {
            continue;
        }

        let config = *FILTER_CONFIG.lock().unwrap();

        match event {
            ScanEvent::WiFi(ref wifi) => {
                handle_wifi_event(
                    wifi,
                    &config,
                    &output_tx,
                    #[cfg(feature = "m5stickc")]
                    &buzzer_tx,
                );
            }
            ScanEvent::Ble(ref ble) => {
                handle_ble_event(
                    ble,
                    &config,
                    &output_tx,
                    #[cfg(feature = "m5stickc")]
                    &buzzer_tx,
                );
            }
        }
    }
}

fn handle_wifi_event(
    wifi: &WiFiEvent,
    config: &FilterConfig,
    output_tx: &SyncSender<MsgBuffer>,
    #[cfg(feature = "m5stickc")] buzzer_tx: &SyncSender<()>,
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

    if let Some(first) = result.matches.first() {
        if let Ok(mut s) = LAST_MATCH.lock() {
            s.clear();
            let _ = s.push_str(&first.detail);
        }
    }

    #[cfg(feature = "m5stickc")]
    let _ = buzzer_tx.try_send(());

    let mut mac_str = MacString::new();
    format_mac(&wifi.mac, &mut mac_str);

    let ts = uptime_millis_u32();

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

fn handle_ble_event(
    ble: &BleEvent,
    config: &FilterConfig,
    output_tx: &SyncSender<MsgBuffer>,
    #[cfg(feature = "m5stickc")] buzzer_tx: &SyncSender<()>,
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

    if let Some(first) = result.matches.first() {
        if let Ok(mut s) = LAST_MATCH.lock() {
            s.clear();
            let _ = s.push_str(&first.detail);
        }
    }

    #[cfg(feature = "m5stickc")]
    let _ = buzzer_tx.try_send(());

    let mut mac_str = MacString::new();
    format_mac(&ble.mac, &mut mac_str);

    let ts = uptime_millis_u32();

    let msg = DeviceMessage::BleScan {
        mac: &mac_str,
        name: &ble.name,
        rssi: ble.rssi,
        uuid: None,
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

// ── Output thread ────────────────────────────────────────────────────

fn output_thread(output_rx: mpsc::Receiver<MsgBuffer>, ble_output_tx: SyncSender<MsgBuffer>) {
    log::info!("Output thread started");

    while let Ok(msg) = output_rx.recv() {
        let _ = ble_output_tx.try_send(msg.clone());

        if let Ok(s) = std::str::from_utf8(&msg) {
            log::info!("{}", s.trim_end());
        }
    }
}

// ── Status thread ────────────────────────────────────────────────────

fn status_thread(output_tx: SyncSender<MsgBuffer>) {
    loop {
        thread::sleep(Duration::from_secs(30));

        let heap_free = unsafe { esp_get_free_heap_size() };

        let msg = DeviceMessage::Status {
            scanning: SCANNING.load(Ordering::Relaxed),
            uptime: uptime_secs(),
            heap_free,
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

// ── Command thread ───────────────────────────────────────────────────

fn command_thread(cmd_rx: mpsc::Receiver<HostCommand>, output_tx: SyncSender<MsgBuffer>) {
    while let Ok(cmd) = cmd_rx.recv() {
        let is_status_request = matches!(cmd, HostCommand::GetStatus);

        let mut config = *FILTER_CONFIG.lock().unwrap();
        let mut scanning = SCANNING.load(Ordering::Relaxed);

        let buzzer_state = comm::handle_command(&cmd, &mut config, &mut scanning);

        if let Some(enabled) = buzzer_state {
            BUZZER_ENABLED.store(enabled, Ordering::Relaxed);
        }

        *FILTER_CONFIG.lock().unwrap() = config;
        SCANNING.store(scanning, Ordering::Relaxed);

        if is_status_request {
            let heap_free = unsafe { esp_get_free_heap_size() };
            let msg = DeviceMessage::Status {
                scanning: SCANNING.load(Ordering::Relaxed),
                uptime: uptime_secs(),
                heap_free,
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

// ── BLE (NimBLE) main loop ───────────────────────────────────────────

fn ble_main(
    scan_tx: SyncSender<ScanEvent>,
    cmd_tx: SyncSender<HostCommand>,
    ble_output_rx: mpsc::Receiver<MsgBuffer>,
) -> ! {
    let ble_device = BLEDevice::take();
    let server = ble_device.get_server();

    // Track connections — NimBLE auto-restarts advertising on disconnect
    server.on_connect(|_server, desc| {
        log::info!("BLE client connected: {}", desc.address());
        BLE_CLIENTS.fetch_add(1, Ordering::Relaxed);
    });
    server.on_disconnect(|desc, _reason| {
        log::info!("BLE client disconnected: {}", desc.address());
        BLE_CLIENTS.fetch_sub(1, Ordering::Relaxed);
    });

    // Create GATT service with same UUIDs as no_std version
    let service_uuid = BleUuid::from_uuid128_string(comm::ble_uuids::SERVICE)
        .expect("invalid service UUID");
    let tx_uuid = BleUuid::from_uuid128_string(comm::ble_uuids::TX_CHAR)
        .expect("invalid TX UUID");
    let rx_uuid = BleUuid::from_uuid128_string(comm::ble_uuids::RX_CHAR)
        .expect("invalid RX UUID");

    let service = server.create_service(service_uuid);

    let tx_char = service.lock().create_characteristic(tx_uuid, NimbleProperties::NOTIFY);

    let rx_char = service.lock().create_characteristic(rx_uuid, NimbleProperties::WRITE);

    // RX write handler — parse incoming NDJSON commands
    let cmd_tx_clone = cmd_tx.clone();
    rx_char.lock().on_write(move |args| {
        thread_local! {
            static LINE_READER: std::cell::RefCell<LineReader> =
                std::cell::RefCell::new(LineReader::new());
        }
        LINE_READER.with(|lr| {
            let mut lr = lr.borrow_mut();
            for &byte in args.recv_data() {
                if let Some(line) = lr.feed(byte) {
                    if let Some(cmd) = comm::parse_command(line) {
                        let _ = cmd_tx_clone.try_send(cmd);
                    }
                }
            }
        });
    });

    // Configure and start advertising
    let mut adv_data = BLEAdvertisementData::new();
    adv_data.name(comm::BLE_ADV_NAME).add_service_uuid(service_uuid);
    ble_device
        .get_advertising()
        .lock()
        .set_data(&mut adv_data)
        .expect("BLE advertising data failed");
    ble_device
        .get_advertising()
        .lock()
        .start()
        .expect("BLE advertising start failed");
    log::info!("BLE advertising as '{}'", comm::BLE_ADV_NAME);

    // Start BLE scanning in a separate thread
    let ble_scan_tx = scan_tx.clone();
    thread::Builder::new()
        .name("blescan".into())
        .stack_size(4096)
        .spawn(move || {
            ble_scan_thread(ble_scan_tx);
        })
        .expect("BLE scan thread spawn failed");
    log::info!("BLE scan thread spawned");

    // TX notify loop — read from ble_output_rx, notify connected clients
    loop {
        match ble_output_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(msg) => {
                if BLE_CLIENTS.load(Ordering::Relaxed) == 0 {
                    continue;
                }
                for chunk in msg.chunks(comm::BLE_MAX_NOTIFY) {
                    let mut padded = [b'\n'; 20];
                    padded[..chunk.len()].copy_from_slice(chunk);
                    tx_char.lock().set_value(&padded).notify();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    unreachable!("BLE output channel disconnected");
}

// ── BLE scan thread ──────────────────────────────────────────────────

fn ble_scan_thread(scan_tx: SyncSender<ScanEvent>) {
    log::info!("BLE scan thread started");

    let ble_device = BLEDevice::take();
    let mut scan = BLEScan::new();
    scan.active_scan(true).interval(100).window(99);

    // Run scan in a loop with 5-second rounds
    loop {
        let _ = block_on(scan.start(ble_device, 5000, |device, data| {
            let addr_bytes = device.addr().as_be_bytes();
            let rssi = device.rssi();
            let payload = data.payload();
            let event = scanner::BleAdvParser::parse(&addr_bytes, rssi, payload);
            let _ = scan_tx.try_send(ScanEvent::Ble(event));
            None::<()> // Continue scanning
        }));
    }
}
