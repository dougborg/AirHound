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
    /// Raw tagged parameters (IEs) from beacon/probe-response frames.
    /// Used for post-match ODID vendor IE parsing. Empty for data/other frames.
    /// Only present on ESP32-S3 — ESP32 doesn't use ODID beacon parsing and
    /// can't spare the ~65 bytes per scan event (8 slots × 65 = 520 bytes DRAM).
    #[cfg(not(feature = "esp32"))]
    pub raw_ies: Vec<u8, 64>,
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
    /// Raw advertisement data bytes (up to 62 bytes: AD + scan response)
    pub raw_ad: Vec<u8, 62>,
}

/// Source transport for an Open Drone ID message
#[cfg(not(feature = "esp32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OdidSource {
    Ble,
    WiFiNan,
    WiFiBeacon,
}

#[cfg(not(feature = "esp32"))]
impl OdidSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            OdidSource::Ble => "ble",
            OdidSource::WiFiNan => "wifi_nan",
            OdidSource::WiFiBeacon => "wifi_beacon",
        }
    }
}

/// A raw Open Drone ID event (stores raw service info bytes to keep ScanEvent small)
#[cfg(not(feature = "esp32"))]
#[derive(Debug, Clone)]
pub struct OdidEvent {
    pub source: OdidSource,
    pub mac: [u8; 6],
    pub rssi: i8,
    /// Raw ODID service info bytes — parsed by the filter task via `odid::parse_odid_wifi_nan()`.
    pub raw_data: heapless::Vec<u8, 64>,
}

/// Unified scan event for the filter task
#[derive(Debug, Clone)]
pub enum ScanEvent {
    WiFi(WiFiEvent),
    Ble(BleEvent),
    #[cfg(not(feature = "esp32"))]
    Odid(OdidEvent),
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
            // Beacon tagged params start at offset 36:
            // 2 FC + 2 dur + 18 addr (3x6) + 2 seq + 8 ts + 2 interval + 2 capability
            let ies = if frame.len() > 36 { &frame[36..] } else { &[] };
            build_wifi_event(
                &beacon.header.transmitter_address.0,
                beacon.body.ssid().unwrap_or(""),
                rssi, channel, FrameType::Beacon, ies,
            )
        }
        probe_req = ProbeRequestFrame<'_> => {
            build_wifi_event(
                &probe_req.header.transmitter_address.0,
                probe_req.body.ssid().unwrap_or(""),
                rssi, channel, FrameType::ProbeRequest, &[],
            )
        }
        probe_resp = ProbeResponseFrame<'_> => {
            // Probe response has same fixed header layout as beacon
            let ies = if frame.len() > 36 { &frame[36..] } else { &[] };
            build_wifi_event(
                &probe_resp.header.transmitter_address.0,
                probe_resp.body.ssid().unwrap_or(""),
                rssi, channel, FrameType::ProbeResponse, ies,
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
            Some(build_wifi_event(&mac, "", rssi, channel, frame_type, &[]))
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
    ies: &[u8],
) -> WiFiEvent {
    let _ = &ies; // used only when raw_ies is present (not ESP32)
    let mut ssid_str = heapless::String::new();
    let _ = ssid_str.push_str(ssid);
    #[cfg(not(feature = "esp32"))]
    let raw_ies = {
        let mut v = Vec::new();
        for &b in ies.iter().take(64) {
            let _ = v.push(b);
        }
        v
    };
    WiFiEvent {
        mac: *mac,
        ssid: ssid_str,
        rssi,
        channel,
        frame_type,
        #[cfg(not(feature = "esp32"))]
        raw_ies,
    }
}

#[cfg(not(feature = "esp32"))]
/// Try to parse ODID from a WiFi NAN action frame.
///
/// NAN uses Public Action frames (category 0x04, action 0x09 = Vendor Specific)
/// with OUI 50:6F:9A (Wi-Fi Alliance) and OUI Type 0x13 (NAN).
/// The NAN frame contains Service Descriptor Attributes (SDA) which may
/// carry ODID service info.
///
/// Returns `Some(OdidEvent)` if valid ODID data is found in the NAN frame.
pub fn try_parse_odid_nan(frame: &[u8], rssi: i8, _channel: u8) -> Option<OdidEvent> {
    // Minimum: 24 (MAC header) + 1 (category) + 1 (action) + 3 (OUI) + 1 (OUI type) = 30
    if frame.len() < 30 {
        return None;
    }

    // Frame control check: Action frame = subtype 0xD (bits 7:4 of byte 0)
    // Type = Management (0b00), subtype = Action (0b1101) → frame[0] = 0xD0
    if frame[0] != 0xD0 {
        return None;
    }

    // Category: Public Action = 0x04
    if frame[24] != 0x04 {
        return None;
    }

    // Action: Vendor Specific = 0x09
    if frame[25] != 0x09 {
        return None;
    }

    // OUI: Wi-Fi Alliance = 50:6F:9A
    if frame[26..29] != [0x50, 0x6F, 0x9A] {
        return None;
    }

    // OUI Type: NAN = 0x13
    if frame[29] != 0x13 {
        return None;
    }

    // Extract transmitter MAC (Address 2, offset 10)
    let mac: [u8; 6] = frame[10..16].try_into().ok()?;

    // NAN body starts at offset 30. Search for Service Descriptor Attribute
    // containing ODID service info. NAN attributes are TLV:
    //   Attribute ID (1 byte), Length (2 bytes LE), Body (Length bytes)
    // SDA = attribute ID 0x03. Within SDA, service info starts after
    // fixed fields and contains ODID messages.
    let nan_body = &frame[30..];
    let service_info = extract_nan_odid_service_info(nan_body)?;

    // Store raw service info bytes — parsing deferred to filter task
    let mut raw_data = heapless::Vec::new();
    for &b in service_info.iter().take(64) {
        let _ = raw_data.push(b);
    }

    Some(OdidEvent {
        source: OdidSource::WiFiNan,
        mac,
        rssi,
        raw_data,
    })
}

#[cfg(not(feature = "esp32"))]
/// Extract service info payload from NAN Service Descriptor Attributes.
///
/// Searches NAN TLV attributes for a Service Descriptor Attribute (0x03)
/// that contains service info and returns its payload. Does not verify
/// the payload is ODID-specific — callers must validate the contents.
fn extract_nan_odid_service_info(nan_body: &[u8]) -> Option<&[u8]> {
    let mut i = 0;
    while i + 3 <= nan_body.len() {
        let attr_id = nan_body[i];
        let attr_len = u16::from_le_bytes([nan_body[i + 1], nan_body[i + 2]]) as usize;
        let attr_end = i + 3 + attr_len;

        if attr_end > nan_body.len() {
            break;
        }

        // Service Descriptor Attribute (SDA) = 0x03
        if attr_id == 0x03 && attr_len >= 13 {
            let sda_body = &nan_body[i + 3..attr_end];
            // SDA fixed fields: Service ID (6) + Instance ID (1) +
            // Requestor Instance ID (1) + Service Control (1) = 9 bytes minimum
            if sda_body.len() >= 9 {
                if let Some(info) = extract_sda_service_info(sda_body, sda_body[8]) {
                    return Some(info);
                }
            }
        }

        i = attr_end;
    }
    None
}

#[cfg(not(feature = "esp32"))]
/// Parse the optional fields of a NAN SDA to extract the Service Info payload.
///
/// `sda_body` starts at the SDA body (after the TLV header).
/// `service_control` is byte 8 of the SDA body, indicating which optional fields are present.
fn extract_sda_service_info<'a>(sda_body: &'a [u8], service_control: u8) -> Option<&'a [u8]> {
    let has_service_info = (service_control & 0x02) != 0;
    let has_match_filter = (service_control & 0x04) != 0;
    let has_binding_bitmap = (service_control & 0x08) != 0;
    let has_service_response_filter = (service_control & 0x10) != 0;

    let mut offset = 9;

    // Skip Binding Bitmap if present (1 byte bitmap control + N bytes bitmap)
    if has_binding_bitmap {
        if offset >= sda_body.len() {
            return None;
        }
        let bitmap_len = 1 + ((sda_body[offset] & 0x0F) as usize + 1);
        offset += bitmap_len;
    }

    // Skip Match Filter if present (1 byte length + N bytes)
    if has_match_filter {
        if offset >= sda_body.len() {
            return None;
        }
        let mf_len = sda_body[offset] as usize;
        offset += 1 + mf_len;
    }

    // Skip Service Response Filter if present (1 byte length + N bytes)
    if has_service_response_filter {
        if offset >= sda_body.len() {
            return None;
        }
        let srf_len = sda_body[offset] as usize;
        offset += 1 + srf_len;
    }

    // Service Info: 1 byte length + N bytes payload
    if has_service_info {
        if offset >= sda_body.len() {
            return None;
        }
        let si_len = sda_body[offset] as usize;
        offset += 1;
        if offset + si_len <= sda_body.len() && si_len >= 25 {
            return Some(&sda_body[offset..offset + si_len]);
        }
    }

    None
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
        let mut raw_ad = Vec::new();
        for &b in ad_data.iter().take(62) {
            let _ = raw_ad.push(b);
        }

        let mut event = BleEvent {
            mac: *addr,
            name: heapless::String::new(),
            rssi,
            service_uuids_16: Vec::new(),
            manufacturer_id: 0,
            raw_ad,
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

    // ── OdidEvent / ScanEvent::Odid tests ─────────────────────────

    #[test]
    fn odid_source_variants() {
        assert_eq!(OdidSource::Ble, OdidSource::Ble);
        assert_ne!(OdidSource::Ble, OdidSource::WiFiNan);
        assert_ne!(OdidSource::WiFiNan, OdidSource::WiFiBeacon);
    }

    #[test]
    fn odid_event_construction() {
        let event = OdidEvent {
            source: OdidSource::Ble,
            mac: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            rssi: -45,
            raw_data: heapless::Vec::new(),
        };
        assert_eq!(event.source, OdidSource::Ble);
        assert_eq!(event.mac, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert_eq!(event.rssi, -45);
    }

    #[test]
    fn scan_event_odid_variant() {
        let event = ScanEvent::Odid(OdidEvent {
            source: OdidSource::WiFiBeacon,
            mac: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            rssi: -60,
            raw_data: heapless::Vec::new(),
        });
        match event {
            ScanEvent::Odid(ref odid) => {
                assert_eq!(odid.source, OdidSource::WiFiBeacon);
                assert_eq!(odid.rssi, -60);
            }
            _ => panic!("Expected ScanEvent::Odid"),
        }
    }

    // ── try_parse_odid_nan tests ─────────────────────────────────────

    /// Build a minimal NAN action frame with ODID service info.
    fn make_nan_odid_frame(mac: &[u8; 6], odid_msg: &[u8]) -> Vec<u8, 256> {
        let mut frame = Vec::new();
        // Frame control: Action frame (0xD0, 0x00)
        let _ = frame.push(0xD0);
        let _ = frame.push(0x00);
        // Duration
        let _ = frame.push(0x00);
        let _ = frame.push(0x00);
        // Addr1 (destination): broadcast
        for _ in 0..6 {
            let _ = frame.push(0xFF);
        }
        // Addr2 (transmitter)
        for &b in mac {
            let _ = frame.push(b);
        }
        // Addr3 (BSSID)
        for &b in mac {
            let _ = frame.push(b);
        }
        // Sequence control
        let _ = frame.push(0x00);
        let _ = frame.push(0x00);
        // Category: Public Action
        let _ = frame.push(0x04);
        // Action: Vendor Specific
        let _ = frame.push(0x09);
        // OUI: Wi-Fi Alliance
        let _ = frame.push(0x50);
        let _ = frame.push(0x6F);
        let _ = frame.push(0x9A);
        // OUI Type: NAN
        let _ = frame.push(0x13);

        // NAN Service Descriptor Attribute (0x03)
        // Attribute ID
        let _ = frame.push(0x03);
        // Calculate SDA body length: 6 (service_id) + 1 (instance) + 1 (req_instance)
        //   + 1 (control) + 1 (service_info_len) + odid_msg.len()
        let sda_len = 9 + 1 + odid_msg.len();
        let _ = frame.push((sda_len & 0xFF) as u8);
        let _ = frame.push(((sda_len >> 8) & 0xFF) as u8);
        // Service ID (6 bytes) — ODID service hash
        for _ in 0..6 {
            let _ = frame.push(0x00);
        }
        // Instance ID
        let _ = frame.push(0x01);
        // Requestor Instance ID
        let _ = frame.push(0x00);
        // Service Control: Service Info present (bit 1) = 0x02
        let _ = frame.push(0x02);
        // Service Info length
        let _ = frame.push(odid_msg.len() as u8);
        // Service Info = ODID message(s)
        for &b in odid_msg {
            let _ = frame.push(b);
        }

        frame
    }

    #[test]
    fn try_parse_odid_nan_valid() {
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        // Build a BasicId ODID message (type 0, 25 bytes)
        let mut odid_msg = [0u8; 25];
        odid_msg[0] = 0x02; // MessageType::BasicId (0 << 4) | IdType::SerialNumber (2)
        odid_msg[1] = 0x02; // UaType::HelicopterOrMultirotor
                            // UAS ID: "TEST1234" null-terminated
        odid_msg[2..10].copy_from_slice(b"TEST1234");

        let frame = make_nan_odid_frame(&mac, &odid_msg);
        let result = try_parse_odid_nan(&frame, -55, 6);
        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.source, OdidSource::WiFiNan);
        assert_eq!(event.mac, mac);
        assert_eq!(event.rssi, -55);
        assert!(!event.raw_data.is_empty());
        // Verify raw_data can be parsed back into a valid ODID frame
        let parsed = crate::odid::parse_odid_wifi_nan(&event.raw_data);
        assert!(parsed.is_some());
        assert!(parsed.unwrap().has_data());
    }

    #[test]
    fn try_parse_odid_nan_not_action_frame() {
        // Beacon frame control instead of action
        let mut frame = [0u8; 40];
        frame[0] = 0x80; // Beacon, not action
        assert!(try_parse_odid_nan(&frame, -50, 6).is_none());
    }

    #[test]
    fn try_parse_odid_nan_wrong_oui() {
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let mut frame = [0u8; 40];
        frame[0] = 0xD0; // Action
                         // fill addresses
        frame[10..16].copy_from_slice(&mac);
        frame[24] = 0x04; // Public Action
        frame[25] = 0x09; // Vendor Specific
        frame[26..29].copy_from_slice(&[0x00, 0x00, 0x00]); // Wrong OUI
        frame[29] = 0x13;
        assert!(try_parse_odid_nan(&frame, -50, 6).is_none());
    }

    #[test]
    fn try_parse_odid_nan_too_short() {
        assert!(try_parse_odid_nan(&[0xD0; 10], -50, 6).is_none());
    }

    #[test]
    fn ble_parse_zero_length_ad_stops() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let ad_data = [0x00, 0x09, b'A'];
        let event = BleAdvParser::parse(&addr, -50, &ad_data);
        assert!(event.name.is_empty());
    }
}
