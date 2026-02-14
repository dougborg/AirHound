/// WiFi and BLE scan event types and parsers.
///
/// Pure parsing logic with no hardware or OS dependencies.
/// WiFi: ieee80211 crate for 802.11 frame parsing.
/// BLE: AD structure parser for advertisement data.
///
/// Hardware-specific code (sniffer callback, channel hopping, BLE event handler)
/// lives in the firmware binary (`main.rs`).
use heapless::Vec;

use ieee80211::match_frames;
use ieee80211::mgmt_frame::{BeaconFrame, ProbeRequestFrame, ProbeResponseFrame};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── FrameType tests ─────────────────────────────────────────────

    #[test]
    fn frame_type_as_str() {
        assert_eq!(FrameType::Beacon.as_str(), "beacon");
        assert_eq!(FrameType::ProbeRequest.as_str(), "probe_req");
        assert_eq!(FrameType::ProbeResponse.as_str(), "probe_resp");
        assert_eq!(FrameType::Data.as_str(), "data");
        assert_eq!(FrameType::Other.as_str(), "other");
    }

    // ── parse_wifi_frame tests ──────────────────────────────────────

    // Minimal valid 802.11 beacon frame for testing.
    // Frame control (2 bytes): 0x80, 0x00 = Beacon
    // Duration (2): 0x00, 0x00
    // Addr1/Dest (6): broadcast FF:FF:FF:FF:FF:FF
    // Addr2/Source (6): B4:1E:52:01:02:03
    // Addr3/BSSID (6): B4:1E:52:01:02:03
    // Seq ctrl (2): 0x00, 0x00
    // Timestamp (8): zeros
    // Beacon interval (2): 0x64, 0x00
    // Capability (2): 0x01, 0x00
    // SSID IE: tag=0, len=4, "Test"
    fn make_beacon_frame(ssid: &str, src_mac: &[u8; 6]) -> Vec<u8, 128> {
        let mut frame = Vec::new();
        // Frame control: beacon
        let _ = frame.push(0x80);
        let _ = frame.push(0x00);
        // Duration
        let _ = frame.push(0x00);
        let _ = frame.push(0x00);
        // Addr1 (destination): broadcast
        for _ in 0..6 {
            let _ = frame.push(0xFF);
        }
        // Addr2 (source/transmitter)
        for &b in src_mac {
            let _ = frame.push(b);
        }
        // Addr3 (BSSID)
        for &b in src_mac {
            let _ = frame.push(b);
        }
        // Sequence control
        let _ = frame.push(0x00);
        let _ = frame.push(0x00);
        // Timestamp (8 bytes)
        for _ in 0..8 {
            let _ = frame.push(0x00);
        }
        // Beacon interval
        let _ = frame.push(0x64);
        let _ = frame.push(0x00);
        // Capability info
        let _ = frame.push(0x01);
        let _ = frame.push(0x00);
        // SSID IE
        let _ = frame.push(0x00); // tag: SSID
        let _ = frame.push(ssid.len() as u8);
        for &b in ssid.as_bytes() {
            let _ = frame.push(b);
        }
        frame
    }

    #[test]
    fn parse_beacon_frame() {
        let mac = [0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03];
        let frame = make_beacon_frame("TestNet", &mac);
        let event = parse_wifi_frame(&frame, -50, 6).unwrap();
        assert_eq!(event.mac, mac);
        assert_eq!(event.ssid.as_str(), "TestNet");
        assert_eq!(event.rssi, -50);
        assert_eq!(event.channel, 6);
        assert_eq!(event.frame_type, FrameType::Beacon);
    }

    #[test]
    fn parse_beacon_empty_ssid() {
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let frame = make_beacon_frame("", &mac);
        let event = parse_wifi_frame(&frame, -70, 11).unwrap();
        assert_eq!(event.ssid.as_str(), "");
    }

    #[test]
    fn parse_too_short_frame_returns_none() {
        // Less than 16 bytes — can't even extract MAC
        let short = [0x80, 0x00, 0x00, 0x00, 0xFF, 0xFF];
        assert!(parse_wifi_frame(&short, -50, 1).is_none());
    }

    #[test]
    fn parse_data_frame_extracts_mac() {
        // Build a minimal data frame (type = 2)
        // Frame control: type=Data (0x08 = data frame, bits 2-3 = 10 = type 2)
        let mut frame = [0u8; 24];
        frame[0] = 0x08; // Frame control: Data
        frame[1] = 0x00;
        // Addr1 (6 bytes at offset 4)
        frame[4..10].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        // Addr2 (6 bytes at offset 10) — the MAC we want to extract
        frame[10..16].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33]);
        let event = parse_wifi_frame(&frame, -60, 3).unwrap();
        assert_eq!(event.mac, [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33]);
        assert_eq!(event.frame_type, FrameType::Data);
        assert_eq!(event.ssid.as_str(), "");
    }

    // ── BleAdvParser tests ──────────────────────────────────────────

    #[test]
    fn ble_parse_empty_ad_data() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let event = BleAdvParser::parse(&addr, -50, &[]);
        assert_eq!(event.mac, addr);
        assert_eq!(event.rssi, -50);
        assert!(event.name.is_empty());
        assert!(event.service_uuids_16.is_empty());
        assert_eq!(event.manufacturer_id, 0);
    }

    #[test]
    fn ble_parse_complete_local_name() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        // AD structure: len=6, type=0x09 (Complete Local Name), data="Flock"
        let ad_data = [0x06, 0x09, b'F', b'l', b'o', b'c', b'k'];
        let event = BleAdvParser::parse(&addr, -40, &ad_data);
        assert_eq!(event.name.as_str(), "Flock");
    }

    #[test]
    fn ble_parse_shortened_local_name() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        // AD structure: len=3, type=0x08 (Shortened Local Name), data="FS"
        let ad_data = [0x03, 0x08, b'F', b'S'];
        let event = BleAdvParser::parse(&addr, -40, &ad_data);
        assert_eq!(event.name.as_str(), "FS");
    }

    #[test]
    fn ble_parse_service_uuids_16() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        // AD structure: len=5, type=0x03 (Complete List 16-bit UUIDs)
        // UUIDs: 0x3100, 0x180A (little-endian)
        let ad_data = [0x05, 0x03, 0x00, 0x31, 0x0A, 0x18];
        let event = BleAdvParser::parse(&addr, -50, &ad_data);
        assert_eq!(event.service_uuids_16.len(), 2);
        assert_eq!(event.service_uuids_16[0], 0x3100);
        assert_eq!(event.service_uuids_16[1], 0x180A);
    }

    #[test]
    fn ble_parse_manufacturer_data() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        // AD structure: len=5, type=0xFF (Manufacturer Specific)
        // Company ID: 0x09C8 (little-endian: 0xC8, 0x09), then 2 bytes payload
        let ad_data = [0x05, 0xFF, 0xC8, 0x09, 0x01, 0x02];
        let event = BleAdvParser::parse(&addr, -50, &ad_data);
        assert_eq!(event.manufacturer_id, 0x09C8);
    }

    #[test]
    fn ble_parse_multiple_ad_structures() {
        let addr = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        // Structure 1: Complete local name "FS"
        // Structure 2: Manufacturer ID 0x09C8
        // Structure 3: 16-bit UUID 0x3100
        let ad_data = [
            // Name
            0x03, 0x09, b'F', b'S', // Manufacturer
            0x03, 0xFF, 0xC8, 0x09, // UUID
            0x03, 0x03, 0x00, 0x31,
        ];
        let event = BleAdvParser::parse(&addr, -45, &ad_data);
        assert_eq!(event.name.as_str(), "FS");
        assert_eq!(event.manufacturer_id, 0x09C8);
        assert_eq!(event.service_uuids_16.len(), 1);
        assert_eq!(event.service_uuids_16[0], 0x3100);
    }

    #[test]
    fn ble_parse_truncated_ad_structure_stops() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        // Structure claims len=10 but only 3 data bytes follow — should stop
        let ad_data = [0x0A, 0x09, b'A', b'B', b'C'];
        let event = BleAdvParser::parse(&addr, -50, &ad_data);
        // Parser should stop, not crash
        assert!(event.name.is_empty());
    }

    #[test]
    fn ble_parse_zero_length_ad_stops() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let ad_data = [0x00, 0x09, b'A'];
        let event = BleAdvParser::parse(&addr, -50, &ad_data);
        assert!(event.name.is_empty());
    }
}
