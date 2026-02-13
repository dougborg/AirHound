/// WiFi sniffer and BLE scanning engine.
///
/// WiFi: Promiscuous mode with channel hopping, ieee80211 crate for frame parsing.
/// BLE: trouble-host Scanner with EventHandler for advertisement reports.
///
/// Both scanners send parsed results through async channels to the filter task.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use heapless::Vec;

use ieee80211::match_frames;
use ieee80211::mgmt_frame::{BeaconFrame, ProbeRequestFrame, ProbeResponseFrame};

use trouble_host::prelude::*;

/// WiFi channels to scan (2.4 GHz only — ESP32/ESP32-S3 promiscuous mode is 2.4 GHz)
pub const WIFI_CHANNELS: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

/// Default dwell time per channel in milliseconds.
/// 120ms ensures reliable beacon capture (beacons broadcast every ~100ms).
/// Full cycle: 13 channels × 120ms = 1.56s.
pub const DEFAULT_DWELL_MS: u64 = 120;

/// A parsed WiFi frame event
#[derive(Debug, Clone)]
pub struct WiFiEvent {
    pub mac: [u8; 6],
    pub ssid: heapless::String<33>,
    pub rssi: i8,
    pub channel: u8,
    pub frame_type: FrameType,
}

/// WiFi frame type classification
#[derive(Debug, Clone, Copy)]
pub enum FrameType {
    Beacon,
    ProbeRequest,
    ProbeResponse,
    Data,
    Other,
}

impl FrameType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameType::Beacon => "beacon",
            FrameType::ProbeRequest => "probe_req",
            FrameType::ProbeResponse => "probe_resp",
            FrameType::Data => "data",
            FrameType::Other => "other",
        }
    }
}

/// A parsed BLE advertisement event
#[derive(Debug, Clone)]
pub struct BleEvent {
    pub mac: [u8; 6],
    pub name: heapless::String<33>,
    pub rssi: i8,
    /// 16-bit service UUIDs extracted from AD structures
    pub service_uuids_16: Vec<u16, 8>,
    /// Manufacturer company ID (0 if not present)
    pub manufacturer_id: u16,
}

/// Unified scan event for the filter task
#[derive(Debug, Clone)]
pub enum ScanEvent {
    WiFi(WiFiEvent),
    Ble(BleEvent),
}

/// Async channel type for scan events
pub type ScanChannel = Channel<CriticalSectionRawMutex, ScanEvent, 16>;

/// Parse a raw 802.11 frame into a WiFiEvent using the ieee80211 crate.
///
/// Management frames (beacons, probes) are parsed with full SSID extraction.
/// Data and other frame types fall through to a raw header parse that extracts
/// the transmitter MAC (Address 2, offset 10) for OUI-prefix matching.
///
/// Safe to call from ISR context (no allocation, no blocking).
pub fn parse_wifi_frame(frame: &[u8], rssi: i8, channel: u8) -> Option<WiFiEvent> {
    let result = match_frames! {
        frame,
        beacon = BeaconFrame<'_> => {
            build_wifi_event(
                &beacon.header.transmitter_address.0,
                beacon.body.ssid().unwrap_or(""),
                rssi, channel, FrameType::Beacon,
            )
        }
        probe_req = ProbeRequestFrame<'_> => {
            build_wifi_event(
                &probe_req.header.transmitter_address.0,
                probe_req.body.ssid().unwrap_or(""),
                rssi, channel, FrameType::ProbeRequest,
            )
        }
        probe_resp = ProbeResponseFrame<'_> => {
            build_wifi_event(
                &probe_resp.header.transmitter_address.0,
                probe_resp.body.ssid().unwrap_or(""),
                rssi, channel, FrameType::ProbeResponse,
            )
        }
    };

    match result {
        Ok(event) => Some(event),
        Err(_) => {
            // Fallback: extract transmitter MAC (Address 2) from any frame.
            // Minimum 16 bytes: 2 (frame ctrl) + 2 (duration) + 6 (addr1) + 6 (addr2)
            if frame.len() < 16 {
                return None;
            }
            let frame_type = match (frame[0] >> 2) & 0x3 {
                2 => FrameType::Data,
                _ => FrameType::Other,
            };
            let mac: [u8; 6] = frame[10..16].try_into().ok()?;
            Some(build_wifi_event(&mac, "", rssi, channel, frame_type))
        }
    }
}

/// Build a WiFiEvent from parsed frame components.
fn build_wifi_event(
    mac: &[u8; 6],
    ssid: &str,
    rssi: i8,
    channel: u8,
    frame_type: FrameType,
) -> WiFiEvent {
    let mut ssid_str = heapless::String::new();
    let _ = ssid_str.push_str(ssid);
    WiFiEvent {
        mac: *mac,
        ssid: ssid_str,
        rssi,
        channel,
        frame_type,
    }
}

/// Parse BLE advertisement data (AD structures) to extract service UUIDs
/// and manufacturer-specific data.
///
/// AD structure format: [length] [type] [data...]
/// Types we care about:
///   0x02/0x03 = Incomplete/Complete list of 16-bit service UUIDs
///   0x04/0x05 = Incomplete/Complete list of 32-bit service UUIDs
///   0x06/0x07 = Incomplete/Complete list of 128-bit service UUIDs
///   0x08/0x09 = Shortened/Complete local name
///   0xFF      = Manufacturer specific data (first 2 bytes = company ID, little-endian)
pub struct BleAdvParser;

impl BleAdvParser {
    /// Parse advertisement data bytes into a BleEvent.
    /// `addr` is the 6-byte advertiser address.
    /// `rssi` is the received signal strength.
    /// `ad_data` is the raw advertisement data bytes.
    pub fn parse(addr: &[u8; 6], rssi: i8, ad_data: &[u8]) -> BleEvent {
        let mut event = BleEvent {
            mac: *addr,
            name: heapless::String::new(),
            rssi,
            service_uuids_16: Vec::new(),
            manufacturer_id: 0,
        };

        let mut pos = 0;
        while pos < ad_data.len() {
            let len = ad_data[pos] as usize;
            if len == 0 || pos + 1 + len > ad_data.len() {
                break;
            }

            let ad_type = ad_data[pos + 1];
            let data = &ad_data[pos + 2..pos + 1 + len];

            match ad_type {
                // 16-bit service UUID lists
                0x02 | 0x03 => {
                    let mut i = 0;
                    while i + 1 < data.len() {
                        let uuid = u16::from_le_bytes([data[i], data[i + 1]]);
                        let _ = event.service_uuids_16.push(uuid);
                        i += 2;
                    }
                }
                // Shortened or Complete local name
                0x08 | 0x09 => {
                    if let Ok(name) = core::str::from_utf8(data) {
                        let _ = event.name.push_str(name);
                    }
                }
                // Manufacturer specific data
                0xFF => {
                    if data.len() >= 2 {
                        event.manufacturer_id = u16::from_le_bytes([data[0], data[1]]);
                    }
                }
                _ => {}
            }

            pos += 1 + len;
        }

        event
    }
}

/// EventHandler for BLE advertisement reports from trouble-host.
///
/// Receives advertisement reports from the BLE stack runner, parses them
/// using `BleAdvParser`, and pushes results to the scan channel.
/// Called synchronously from the runner — must not block.
pub struct ScanEventHandler;

impl EventHandler for ScanEventHandler {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        while let Some(Ok(report)) = it.next() {
            let addr_bytes: &[u8; 6] = report.addr.raw().try_into().unwrap();
            let event = BleAdvParser::parse(&addr_bytes, report.rssi, report.data);
            let _ = crate::SCAN_CHANNEL.try_send(ScanEvent::Ble(event));
        }
    }
}

/// WiFi sniffer callback — called from ISR context by the esp-radio sniffer.
///
/// Parses raw 802.11 frames using `parse_wifi_frame()` (ieee80211 crate)
/// and pushes matching events to the scan channel via `try_send` (non-blocking).
pub fn wifi_sniffer_callback(pkt: esp_radio::wifi::sniffer::PromiscuousPkt<'_>) {
    let rssi = pkt.rx_cntl.rssi as i8;
    let channel = pkt.rx_cntl.channel as u8;
    if let Some(event) = parse_wifi_frame(pkt.data, rssi, channel) {
        let _ = crate::SCAN_CHANNEL.try_send(ScanEvent::WiFi(event));
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
pub async fn wifi_channel_hop_task() {
    loop {
        for &ch in WIFI_CHANNELS {
            unsafe {
                esp_wifi_set_channel(ch, 0);
            }
            Timer::after(Duration::from_millis(DEFAULT_DWELL_MS)).await;
        }
    }
}
