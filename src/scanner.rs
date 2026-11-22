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
    /// Receive frequency supplied by the backend; channel alone cannot establish a band.
    pub frequency_mhz: Option<u16>,
    pub frame_type: FrameType,
    /// Raw tagged parameters (IEs) from beacon/probe-response frames.
    /// Used for post-match ODID vendor IE parsing. Empty for data/other frames.
    /// Only present on ESP32-S3 — ESP32 doesn't use ODID beacon parsing and
    /// can't spare the ~65 bytes per scan event (8 slots × 65 = 520 bytes DRAM).
    #[cfg(not(feature = "esp32"))]
    pub raw_ies: Vec<u8, 64>,
    /// WiFi security mode string for WiGLE AuthMode (e.g. "[WPA2-PSK][WPA3-SAE]").
    /// Only present on non-ESP32 platforms — ESP32 promiscuous mode doesn't expose
    /// security info and can't spare the ~65 bytes per scan event.
    /// Populated by hostd from `wifi_scan` crate on desktop platforms.
    #[cfg(not(feature = "esp32"))]
    pub security: heapless::String<64>,
}

/// Frame identity without receive-channel or signal-strength claims.
#[derive(Debug, Clone)]
pub struct WiFiIdentity {
    pub mac: [u8; 6],
    pub ssid: heapless::String<33>,
    pub frame_type: FrameType,
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

impl BleEvent {
    /// Borrow full UUID identities from retained AD; no extra queued storage.
    /// Capacity remains the raw_ad bound. This does not reconstruct omitted bytes.
    pub fn advertised_service_uuids(&self) -> impl Iterator<Item = crate::ble_data::Uuid> + '_ {
        crate::ble_data::AdStructures::new(&self.raw_ad).flat_map(|field| field.uuids())
    }
    /// Complete manufacturer sections in retained AD order; ID zero is valid.
    /// Like the UUID views, this cannot recover bytes beyond the raw buffer.
    pub fn manufacturer_data(
        &self,
    ) -> impl Iterator<Item = crate::ble_data::ManufacturerData<'_>> + '_ {
        crate::ble_data::AdStructures::new(&self.raw_ad).filter_map(|field| field.manufacturer())
    }
    pub fn service_data(&self) -> impl Iterator<Item = crate::ble_data::ServiceData<'_>> + '_ {
        crate::ble_data::AdStructures::new(&self.raw_ad).filter_map(|field| field.service())
    }
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
    /// Raw ODID service info bytes (message counter + message pack) — parsed
    /// by the filter task via `odid::parse_odid_service_info()`. Sized for a full
    /// pack of 9 messages.
    pub raw_data: heapless::Vec<u8, 256>,
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
    let identity = parse_wifi_identity(frame)?;
    Some(WiFiEvent {
        mac: identity.mac,
        ssid: identity.ssid,
        frame_type: identity.frame_type,
        rssi,
        channel,
        frequency_mhz: None,
        #[cfg(not(feature = "esp32"))]
        raw_ies: identity.raw_ies,
        #[cfg(not(feature = "esp32"))]
        security: heapless::String::new(),
    })
}

/// Parse identity only; no receive metadata is invented for saved bare frames.
pub fn parse_wifi_identity(frame: &[u8]) -> Option<WiFiIdentity> {
    let result = match_frames! {
        frame,
        beacon = BeaconFrame<'_> => {
            // Beacon tagged params start at offset 36:
            // 2 FC + 2 dur + 18 addr (3x6) + 2 seq + 8 ts + 2 interval + 2 capability
            let ies = if frame.len() > 36 { &frame[36..] } else { &[] };
            build_wifi_identity(
                &beacon.header.transmitter_address.0,
                beacon.body.ssid().unwrap_or(""),
                FrameType::Beacon, ies,
            )
        }
        probe_req = ProbeRequestFrame<'_> => {
            build_wifi_identity(
                &probe_req.header.transmitter_address.0,
                probe_req.body.ssid().unwrap_or(""),
                FrameType::ProbeRequest, &[],
            )
        }
        probe_resp = ProbeResponseFrame<'_> => {
            // Probe response has same fixed header layout as beacon
            let ies = if frame.len() > 36 { &frame[36..] } else { &[] };
            build_wifi_identity(
                &probe_resp.header.transmitter_address.0,
                probe_resp.body.ssid().unwrap_or(""),
                FrameType::ProbeResponse, ies,
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
            Some(build_wifi_identity(&mac, "", frame_type, &[]))
        }
    }
}

/// Build a WiFiIdentity from parsed frame components.
fn build_wifi_identity(
    mac: &[u8; 6],
    ssid: &str,
    frame_type: FrameType,
    ies: &[u8],
) -> WiFiIdentity {
    let _ = &ies; // used only when raw_ies is present (not ESP32)
    let mut ssid_str = heapless::String::new();
    let _ = ssid_str.push_str(ssid);
    #[cfg(not(feature = "esp32"))]
    let raw_ies = {
        let mut v: Vec<u8, 64> = Vec::new();
        v.extend(ies.iter().copied().take(64));
        v
    };
    WiFiIdentity {
        mac: *mac,
        ssid: ssid_str,
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
    if frame.first() != Some(&0xD0) {
        return None;
    }
    try_parse_odid_wifi(frame, rssi, _channel)
}

/// Extract Remote ID from a complete WiFi beacon, probe response or NAN frame.
///
/// Copy only the Remote ID service data before the generic WiFi path truncates
/// its IEs. A transmitter's MAC OUI is not a Remote ID protocol identifier.
/// Decoding remains deferred to the filter task, outside the radio callback.
#[cfg(not(feature = "esp32"))]
pub fn try_parse_odid_wifi(frame: &[u8], rssi: i8, _channel: u8) -> Option<OdidEvent> {
    let rid = remoteid::wifi::parse_frame(frame).ok()?;
    let source = match rid.kind {
        remoteid::wifi::WifiKind::Beacon => OdidSource::WiFiBeacon,
        remoteid::wifi::WifiKind::Nan => OdidSource::WiFiNan,
    };
    let mut raw_data = heapless::Vec::new();
    raw_data.push(rid.counter).ok()?;
    raw_data.extend_from_slice(rid.payload).ok()?;

    Some(OdidEvent {
        source,
        mac: rid.source,
        rssi,
        raw_data,
    })
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
        let mut raw_ad: Vec<u8, 62> = Vec::new();
        raw_ad.extend(ad_data.iter().copied().take(62));

        let mut event = BleEvent {
            mac: *addr,
            name: heapless::String::new(),
            rssi,
            service_uuids_16: Vec::new(),
            manufacturer_id: 0,
            raw_ad,
        };

        let mut name_is_complete = false;
        for field in crate::ble_data::AdStructures::new(ad_data) {
            let ad_type = field.kind;
            let data = field.data;

            match ad_type {
                // 16-bit service UUID lists
                0x02 | 0x03 => {
                    // Use the same structural validation as raw/desktop
                    // matching: an odd-length list contributes no UUIDs.
                    for uuid in field.uuids().filter_map(crate::ble_data::Uuid::as_u16) {
                        let _ = event.service_uuids_16.push(uuid);
                    }
                }
                // Service Data carries its UUID even when no UUID-list AD
                // structure is present (e.g. legacy Remote ID advertising).
                0x16 if data.len() >= 2 => {
                    let uuid = u16::from_le_bytes([data[0], data[1]]);
                    if !event.service_uuids_16.contains(&uuid) {
                        let _ = event.service_uuids_16.push(uuid);
                    }
                }
                // Shortened or Complete local name
                0x08 | 0x09 => {
                    if !name_is_complete && (ad_type == 0x09 || event.name.is_empty()) {
                        if let Ok(name) = core::str::from_utf8(data) {
                            event.name.clear();
                            for ch in name.chars() {
                                if event.name.push(ch).is_err() {
                                    break;
                                }
                            }
                            name_is_complete = ad_type == 0x09;
                        }
                    }
                }
                // Manufacturer specific data
                0xFF if data.len() >= 2 => {
                    event.manufacturer_id = u16::from_le_bytes([data[0], data[1]]);
                }
                _ => {}
            }
        }

        event
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn raw_uuid_views_and_names_preserve_section_boundaries() {
        let custom = crate::ble_data::Uuid([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let mut raw = heapless::Vec::<u8, 62>::new();
        raw.extend_from_slice(&[
            4, 8, b'a', b'b', b'c', 4, 9, b'd', b'e', b'f', 3, 8, b'x', b'y',
        ])
        .unwrap();
        raw.extend_from_slice(&[17, 7]).unwrap();
        raw.extend(custom.0.iter().rev().copied());
        raw.extend_from_slice(&[5, 0x20, 0xfa, 0xff, 0, 0]).unwrap();
        let event = BleAdvParser::parse(&[0; 6], -40, &raw);
        assert_eq!(event.name, "def");
        assert_eq!(
            event
                .advertised_service_uuids()
                .collect::<std::vec::Vec<_>>(),
            [custom]
        );
        assert_eq!(
            event.service_data().next().unwrap().uuid,
            crate::ble_data::Uuid::from_u16(0xfffa)
        );
    }

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

    // Synthetic framing fixtures, not captured device evidence. A recognized
    // service prefix inside an invalid UUID list must not become rule evidence.
    #[test]
    fn malformed_uuid_lists_do_not_create_signature_evidence() {
        for kind in [0x02, 0x03] {
            let malformed = [4, kind, 0xfa, 0xff, 0xaa];
            let event = BleAdvParser::parse(&[0x02; 6], -50, &malformed);
            assert!(event.service_uuids_16.is_empty(), "AD type {kind:#x}");
            assert_eq!(event.advertised_service_uuids().count(), 0);
            let result = filter_parsed_ble(&event);
            assert!(!result.matched);
            assert!(result.rule_categories.is_empty());

            // A bad field does not erase an independently well-formed field.
            let mut mixed = malformed.to_vec();
            mixed.extend_from_slice(&[3, kind, 0xfa, 0xff]);
            let valid = BleAdvParser::parse(&[0x02; 6], -50, &mixed);
            assert_eq!(valid.service_uuids_16.as_slice(), &[0xfffa]);
            assert!(filter_parsed_ble(&valid)
                .rule_categories
                .contains(&crate::rules::MatchCategory::Drone));
        }
    }

    #[test]
    fn incomplete_uuid_fields_cannot_supply_a_known_prefix() {
        // Both complete and incomplete UUID-list types require complete AD
        // framing and an integral number of UUIDs, independent of list meaning.
        for kind in [0x02, 0x03] {
            let complete = [5, kind, 0xfa, 0xff, 0x00, 0x31];
            for end in 0..complete.len() {
                let event = BleAdvParser::parse(&[0x02; 6], -50, &complete[..end]);
                assert!(!filter_parsed_ble(&event).matched, "kind={kind} end={end}");
            }
            let event = BleAdvParser::parse(&[0x02; 6], -50, &complete);
            assert!(filter_parsed_ble(&event).matched);
        }
    }

    fn filter_parsed_ble(event: &BleEvent) -> crate::filter::FilterResult {
        crate::filter::filter_ble_with_rules(
            &crate::filter::BleScanInput {
                address_type: crate::filter::BleAddressType::Unknown,
                mac: &event.mac,
                name: event.name.as_str(),
                rssi: event.rssi,
                service_uuids_16: &event.service_uuids_16,
                manufacturer_id: event.manufacturer_id,
                raw_ad: &event.raw_ad,
            },
            &crate::filter::FilterConfig::new(),
            &crate::defaults::DEFAULT_RULE_DB,
            &crate::mac_index::MacIndex::new(),
        )
    }

    #[test]
    fn legacy_remote_id_service_data_matches_drone_rule() {
        let mut ad = [0u8; 31];
        ad[..6].copy_from_slice(&[30, 0x16, 0xFA, 0xFF, 0x0D, 7]);
        ad[6] = 0x02;
        ad[7] = 0x12;
        ad[8..16].copy_from_slice(b"LEGACY01");
        let event = BleAdvParser::parse(&[0x02; 6], -50, &ad);
        let input = crate::filter::BleScanInput {
            address_type: crate::filter::BleAddressType::Unknown,
            mac: &event.mac,
            name: event.name.as_str(),
            rssi: event.rssi,
            service_uuids_16: &event.service_uuids_16,
            manufacturer_id: event.manufacturer_id,
            raw_ad: &event.raw_ad,
        };
        let index = crate::mac_index::MacIndex::new();
        let result = crate::filter::filter_ble_with_rules(
            &input,
            &crate::filter::FilterConfig::new(),
            &crate::defaults::DEFAULT_RULE_DB,
            &index,
        );
        assert!(result
            .rule_categories
            .contains(&crate::rules::MatchCategory::Drone));
        let decoded = crate::odid::parse_odid_ble(&event.raw_ad).unwrap();
        assert_eq!(decoded.basic_id.unwrap().uas_id.as_str(), "LEGACY01");
    }

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
    fn make_nan_odid_frame(mac: &[u8; 6], odid_msg: &[u8]) -> Vec<u8, 512> {
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
        //   + 1 (control) + 1 (service_info_len) + 1 (counter) + pack
        let pack_len = 3 + odid_msg.len();
        let sda_len = 9 + 1 + 1 + pack_len;
        let _ = frame.push((sda_len & 0xFF) as u8);
        let _ = frame.push(((sda_len >> 8) & 0xFF) as u8);
        // Service ID (6 bytes) — hash of "org.opendroneid.remoteid"
        for &b in &[0x88, 0x69, 0x19, 0x9D, 0x92, 0x09] {
            let _ = frame.push(b);
        }
        // Instance ID
        let _ = frame.push(0x01);
        // Requestor Instance ID
        let _ = frame.push(0x00);
        // Service Control: Service Info present (bit 4) = 0x10
        let _ = frame.push(0x10);
        // Service Info length
        let _ = frame.push((1 + pack_len) as u8);
        // Service Info = message counter, then a message pack
        let _ = frame.push(0x00);
        let _ = frame.push(0xF2); // pack, protocol version 2
        let _ = frame.push(25); // single message size
        let _ = frame.push((odid_msg.len() / 25) as u8);
        for &b in odid_msg {
            let _ = frame.push(b);
        }

        frame
    }

    #[test]
    fn captured_beacon_survives_scanner_without_mac_signature() {
        // Tagged parameters from odid_wifi_bcn_sample.pcap, Open Drone ID
        // wireshark-dissector captures (Apache-2.0), as in odid.rs.
        let hex = concat!(
            "030106000e4742522d4f502d31323341424344dd85fa0bbc0dd0f0190500004d",
            "464731413031323334353637383900000000000050f610005c527ebcba251ba8",
            "8cb4b60000aa099808394100000a00300052656372656174696f6e616c000000",
            "00000000000000004004a485251b6edbb3b60100320000000015000000000000",
            "0050004742522d4f502d31323341424344000000000000000000",
        );
        let ies: Vec<u8, 256> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
        let mut frame: Vec<u8, 512> = Vec::new();
        frame.resize(36, 0).unwrap();
        frame[0] = 0x80;
        frame[10..16].copy_from_slice(&mac);
        // Ordinary IEs before Remote ID can exceed the old 64-byte buffer.
        frame.extend_from_slice(&[1, 80]).unwrap();
        frame.extend_from_slice(&[0; 80]).unwrap();
        frame.extend_from_slice(&ies).unwrap();
        for subtype in [0x80, 0x50] {
            frame[0] = subtype; // beacon and probe response
            let event = try_parse_odid_wifi(&frame, -60, 6).unwrap();
            assert_eq!(event.source, OdidSource::WiFiBeacon);
            assert_eq!(event.mac, mac);
            assert_eq!(event.rssi, -60);
            let decoded = crate::odid::parse_odid_service_info(&event.raw_data).unwrap();
            assert_eq!(decoded.basic_id.unwrap().uas_id.as_str(), "MFG1A0123456789");
            assert_eq!(
                decoded.operator_id.unwrap().operator_id.as_str(),
                "GBR-OP-123ABCD"
            );
        }
        // Every truncation inside the vendor IE must fail instead of emitting
        // a partial service payload. Earlier complete, unrelated IEs are fine.
        for end in 0..frame.len() {
            assert!(try_parse_odid_wifi(&frame[..end], -60, 6).is_none());
        }
    }

    #[test]
    fn nan_preserves_full_nine_message_pack() {
        let mut messages = [0u8; 9 * 25];
        messages[0] = 0x02; // Basic ID
        messages[1] = 0x12;
        messages[2..10].copy_from_slice(b"FULLPACK");
        for message in messages[25..].as_chunks_mut::<25>().0 {
            message[0] = 0x22; // Authentication pages
        }
        let frame = make_nan_odid_frame(&[0x02; 6], &messages);
        let event = try_parse_odid_wifi(&frame, -55, 6).unwrap();
        assert_eq!(event.raw_data.len(), 1 + 3 + 9 * 25);
        assert_eq!(&event.raw_data[4..], &messages);
        let decoded = crate::odid::parse_odid_service_info(&event.raw_data).unwrap();
        assert_eq!(decoded.basic_id.unwrap().uas_id.as_str(), "FULLPACK");
    }

    #[test]
    fn try_parse_odid_nan_valid() {
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        // Build a BasicId ODID message (type 0, 25 bytes)
        let mut odid_msg = [0u8; 25];
        odid_msg[0] = 0x02; // MessageType::BasicId (0 << 4) | protocol version 2
        odid_msg[1] = 0x12; // IdType::SerialNumber (1 << 4) | UaType::HelicopterOrMultirotor (2)
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

    /// A NAN action frame from the public Open Drone ID sample capture
    /// odid_wifi_sample.pcap (github.com/opendroneid/wireshark-dissector).
    #[test]
    fn try_parse_odid_nan_captured_frame() {
        let hex = concat!(
            "d0000000516f9a01000084cca8604324506f9a01017950060409506f9a130327",
            "008869199d92090100101d22f0190150004742522d4f502d3132334142434400",
            "00000000000000000e040001000222",
        );
        let frame: Vec<u8, 128> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let event = try_parse_odid_nan(&frame, -60, 6).unwrap();
        assert_eq!(event.mac, [0x84, 0xcc, 0xa8, 0x60, 0x43, 0x24]);
        let parsed = crate::odid::parse_odid_wifi_nan(&event.raw_data).unwrap();
        assert_eq!(
            parsed.operator_id.unwrap().operator_id.as_str(),
            "GBR-OP-123ABCD"
        );
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
