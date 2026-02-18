/// JSON message protocol for communication between AirHound and companion apps.
///
/// All messages are newline-delimited JSON (NDJSON).
/// Uses `heapless` types for no_std/no-alloc operation.
use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

/// Maximum length for MAC address strings ("AA:BB:CC:DD:EE:FF")
pub type MacString = String<18>;

/// Maximum length for SSID / device name strings
pub type NameString = String<33>;

/// Maximum length for UUID strings
pub type UuidString = String<37>;

/// Maximum length for filter match detail strings
pub type MatchDetail = String<32>;

/// A single filter match reason
#[derive(Debug, Clone, Serialize)]
pub struct MatchReason {
    /// Filter type that matched: "mac_oui", "ssid_pattern", "ssid_keyword",
    /// "ble_name", "ble_uuid", "ble_mfr"
    #[serde(rename = "type")]
    pub filter_type: &'static str,
    /// Human-readable detail about what matched
    pub detail: MatchDetail,
}

/// Messages sent from the device to the companion app
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum DeviceMessage<'a> {
    /// WiFi scan result
    #[serde(rename = "wifi")]
    WiFiScan {
        mac: &'a MacString,
        ssid: &'a NameString,
        rssi: i8,
        ch: u8,
        /// Frame type: "beacon", "probe_req", "probe_resp", "data", "other"
        frame: &'static str,
        /// Why this result matched the filter
        #[serde(rename = "match")]
        matches: &'a Vec<MatchReason, 4>,
        /// First matched rule name, if any
        #[serde(skip_serializing_if = "Option::is_none")]
        rule: Option<&'a str>,
        /// Uptime in milliseconds when captured
        ts: u32,
    },
    /// BLE scan result
    #[serde(rename = "ble")]
    BleScan {
        mac: &'a MacString,
        name: &'a NameString,
        rssi: i8,
        /// Primary service UUID if detected
        #[serde(skip_serializing_if = "Option::is_none")]
        uuid: Option<&'a UuidString>,
        /// Manufacturer company ID
        mfr: u16,
        /// Why this result matched the filter
        #[serde(rename = "match")]
        matches: &'a Vec<MatchReason, 4>,
        /// First matched rule name, if any
        #[serde(skip_serializing_if = "Option::is_none")]
        rule: Option<&'a str>,
        /// Uptime in milliseconds when captured
        ts: u32,
    },
    /// Device status report
    #[serde(rename = "status")]
    Status {
        scanning: bool,
        /// Uptime in seconds
        uptime: u32,
        /// Free heap in bytes
        heap_free: u32,
        /// Number of connected BLE clients
        ble_clients: u8,
        /// Board identifier
        board: &'static str,
        /// Firmware version
        version: &'static str,
    },
}

/// Commands sent from the companion app to the device.
///
/// Deserialized manually via [`RawCommand`] in `comm::parse_command()` because
/// `serde_json_core` does not support internally tagged enums (`deserialize_any`).
#[derive(Debug, PartialEq)]
pub enum HostCommand {
    /// Start scanning
    Start,
    /// Stop scanning
    Stop,
    /// Request current status
    GetStatus,
    /// Update minimum RSSI threshold
    SetRssi {
        /// Minimum RSSI (negative dBm value)
        min_rssi: i8,
    },
    /// Enable or disable the buzzer (M5StickC only)
    SetBuzzer { enabled: bool },
}

/// Wire format for host commands — flat struct that `serde_json_core` can
/// deserialize without `deserialize_any`. Converted to [`HostCommand`] in
/// `comm::parse_command()`.
#[derive(Deserialize)]
pub(crate) struct RawCommand {
    pub cmd: heapless::String<16>,
    #[serde(default)]
    pub min_rssi: Option<i8>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Firmware version string
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum size of a serialized JSON message
pub const MAX_MSG_LEN: usize = 512;

/// Buffer type for serialized JSON messages
pub type MsgBuffer = Vec<u8, MAX_MSG_LEN>;

#[cfg(test)]
mod tests {
    use super::*;

    // ── HostCommand parsing (via comm::parse_command) ──────────────

    #[test]
    fn host_command_equality() {
        assert_eq!(HostCommand::Start, HostCommand::Start);
        assert_eq!(
            HostCommand::SetRssi { min_rssi: -75 },
            HostCommand::SetRssi { min_rssi: -75 }
        );
        assert_ne!(HostCommand::Start, HostCommand::Stop);
    }

    // ── DeviceMessage serialization ─────────────────────────────────

    #[test]
    fn serialize_status_message() {
        let msg = DeviceMessage::Status {
            scanning: true,
            uptime: 120,
            heap_free: 48000,
            ble_clients: 1,
            board: "test_board",
            version: "0.1.0",
        };
        let mut buf = [0u8; 256];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""type":"status""#));
        assert!(json.contains(r#""scanning":true"#));
        assert!(json.contains(r#""uptime":120"#));
        assert!(json.contains(r#""board":"test_board""#));
    }

    #[test]
    fn serialize_wifi_scan_message() {
        let mac = MacString::try_from("B4:1E:52:AB:CD:EF").unwrap();
        let ssid = NameString::try_from("Flock-A1B2C3").unwrap();
        let mut matches = Vec::<MatchReason, 4>::new();
        let mut detail = MatchDetail::new();
        let _ = detail.push_str("Flock Safety");
        let _ = matches.push(MatchReason {
            filter_type: "mac_oui",
            detail,
        });

        let msg = DeviceMessage::WiFiScan {
            mac: &mac,
            ssid: &ssid,
            rssi: -45,
            ch: 6,
            frame: "beacon",
            matches: &matches,
            rule: None,
            ts: 1000,
        };

        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""type":"wifi""#));
        assert!(json.contains(r#""mac":"B4:1E:52:AB:CD:EF""#));
        assert!(json.contains(r#""ssid":"Flock-A1B2C3""#));
        assert!(json.contains(r#""rssi":-45"#));
        assert!(json.contains(r#""ch":6"#));
        assert!(json.contains(r#""frame":"beacon""#));
    }

    #[test]
    fn serialize_ble_scan_message() {
        let mac = MacString::try_from("58:8E:81:AA:BB:CC").unwrap();
        let name = NameString::try_from("FS Ext Battery").unwrap();
        let matches = Vec::<MatchReason, 4>::new();

        let msg = DeviceMessage::BleScan {
            mac: &mac,
            name: &name,
            rssi: -60,
            uuid: None,
            mfr: 0x09C8,
            matches: &matches,
            rule: None,
            ts: 2000,
        };

        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""type":"ble""#));
        assert!(json.contains(r#""name":"FS Ext Battery""#));
        assert!(json.contains(r#""mfr":2504"#)); // 0x09C8 = 2504
                                                 // uuid should be omitted when None
        assert!(!json.contains("uuid"));
    }

    #[test]
    fn serialize_ble_scan_with_uuid() {
        let mac = MacString::try_from("00:11:22:33:44:55").unwrap();
        let name = NameString::try_from("Device").unwrap();
        let uuid = UuidString::try_from("00003100-0000-1000-8000-00805f9b34fb").unwrap();
        let matches = Vec::<MatchReason, 4>::new();

        let msg = DeviceMessage::BleScan {
            mac: &mac,
            name: &name,
            rssi: -70,
            uuid: Some(&uuid),
            mfr: 0,
            matches: &matches,
            rule: None,
            ts: 3000,
        };

        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""uuid":"00003100-0000-1000-8000-00805f9b34fb""#));
    }

    // ── Rule field serialization ────────────────────────────────────

    #[test]
    fn serialize_wifi_scan_with_rule() {
        let mac = MacString::try_from("B4:1E:52:AB:CD:EF").unwrap();
        let ssid = NameString::try_from("Flock-A1B2C3").unwrap();
        let matches = Vec::<MatchReason, 4>::new();

        let msg = DeviceMessage::WiFiScan {
            mac: &mac,
            ssid: &ssid,
            rssi: -45,
            ch: 6,
            frame: "beacon",
            matches: &matches,
            rule: Some("Flock Safety Camera"),
            ts: 1000,
        };

        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""rule":"Flock Safety Camera""#));
    }

    #[test]
    fn serialize_wifi_scan_without_rule_omits_field() {
        let mac = MacString::try_from("B4:1E:52:AB:CD:EF").unwrap();
        let ssid = NameString::try_from("Flock-A1B2C3").unwrap();
        let matches = Vec::<MatchReason, 4>::new();

        let msg = DeviceMessage::WiFiScan {
            mac: &mac,
            ssid: &ssid,
            rssi: -45,
            ch: 6,
            frame: "beacon",
            matches: &matches,
            rule: None,
            ts: 1000,
        };

        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(!json.contains("rule"));
    }

    #[test]
    fn serialize_ble_scan_with_rule() {
        let mac = MacString::try_from("00:00:00:00:00:00").unwrap();
        let name = NameString::try_from("").unwrap();
        let matches = Vec::<MatchReason, 4>::new();

        let msg = DeviceMessage::BleScan {
            mac: &mac,
            name: &name,
            rssi: -50,
            uuid: None,
            mfr: 0,
            matches: &matches,
            rule: Some("Apple AirTag"),
            ts: 5000,
        };

        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""rule":"Apple AirTag""#));
    }

    // ── Version constant ────────────────────────────────────────────

    #[test]
    fn version_is_semver() {
        let parts: heapless::Vec<&str, 4> = VERSION.split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "VERSION should be semver (major.minor.patch)"
        );
        for part in &parts {
            assert!(part.parse::<u32>().is_ok(), "'{part}' is not a number");
        }
    }
}
