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

/// Maximum length for match reason detail strings
pub type MatchDetail = String<32>;

/// A single match reason
#[derive(Debug, Clone, Serialize)]
pub struct MatchReason {
    /// Signature type that triggered this match: "mac_oui", "ssid_pattern",
    /// "ssid_keyword", "ble_name", "ble_uuid", "ble_mfr".
    /// Note: field rename to `signature_type` is tracked for protocol v2 (#9).
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
    /// Drone sighting from ODID data
    #[serde(rename = "drone")]
    DroneSighting {
        mac: &'a MacString,
        rssi: i8,
        /// Transport source: "ble", "wifi_nan", "wifi_beacon"
        source: &'static str,
        /// UAS ID from BasicId message
        #[serde(skip_serializing_if = "Option::is_none")]
        uas_id: Option<&'a NameString>,
        /// UA type code
        #[serde(skip_serializing_if = "Option::is_none")]
        ua_type: Option<u8>,
        /// Latitude in degrees
        #[serde(skip_serializing_if = "Option::is_none")]
        lat: Option<f64>,
        /// Longitude in degrees
        #[serde(skip_serializing_if = "Option::is_none")]
        lon: Option<f64>,
        /// Altitude in meters
        #[serde(skip_serializing_if = "Option::is_none")]
        alt: Option<f32>,
        /// Speed in m/s
        #[serde(skip_serializing_if = "Option::is_none")]
        speed: Option<f32>,
        /// Operator ID
        #[serde(skip_serializing_if = "Option::is_none")]
        operator_id: Option<&'a NameString>,
        /// Matched rule name
        #[serde(skip_serializing_if = "Option::is_none")]
        rule: Option<&'a str>,
        /// Match category
        #[serde(skip_serializing_if = "Option::is_none")]
        category: Option<&'static str>,
        /// Uptime in milliseconds when captured
        ts: u32,
    },
    /// Proximity update for a tracked target
    #[serde(rename = "proximity")]
    ProximityUpdate {
        mac: &'a MacString,
        rssi: i8,
        /// Estimated distance in meters
        distance: f32,
        /// Beep interval in milliseconds
        interval: u16,
        /// User-assigned label
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<&'a str>,
        /// Uptime in milliseconds
        ts: u32,
    },
    /// Security or system alert
    #[serde(rename = "alert")]
    Alert {
        /// Alert type identifier (e.g., "deauth_flood", "evil_twin")
        alert_type: &'static str,
        /// Human-readable description
        message: &'a str,
        /// Severity level: "info", "warning", "critical"
        severity: &'static str,
        /// Related MAC address if applicable
        #[serde(skip_serializing_if = "Option::is_none")]
        mac: Option<&'a MacString>,
        /// Uptime in milliseconds
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

/// Operating mode for the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    Ap,
    Client,
    Dongle,
    Standalone,
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
    /// Set operating mode
    SetMode { mode: OperatingMode },
    /// Add a MAC address to the proximity tracking list
    AddProximityTarget {
        mac: [u8; 6],
        label: heapless::String<32>,
    },
    /// Remove a MAC address from the proximity tracking list
    RemoveProximityTarget { mac: [u8; 6] },
    /// Clear all proximity targets
    ClearProximityTargets,
    /// Add a watchlist entry
    AddWatchlist {
        id: u16,
        mac: [u8; 6],
        full_mac: bool,
        label: heapless::String<32>,
    },
    /// Remove a watchlist entry by ID
    RemoveWatchlist { id: u16 },
    /// Enable or disable a match category
    EnableCategory {
        category: heapless::String<16>,
        enabled: bool,
    },
    /// Export data in WiGLE format
    ExportWigle,
}

/// Wire format for host commands — flat struct that `serde_json_core` can
/// deserialize without `deserialize_any`. Converted to [`HostCommand`] in
/// `comm::parse_command()`.
#[derive(Deserialize)]
pub(crate) struct RawCommand {
    pub cmd: heapless::String<32>,
    #[serde(default)]
    pub min_rssi: Option<i8>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub mode: Option<heapless::String<16>>,
    #[serde(default)]
    pub mac: Option<MacString>,
    #[serde(default)]
    pub id: Option<u16>,
    #[serde(default)]
    pub full_mac: Option<bool>,
    #[serde(default)]
    pub label: Option<heapless::String<32>>,
    #[serde(default)]
    pub category: Option<heapless::String<16>>,
}

/// Format a 6-byte MAC address into "AA:BB:CC:DD:EE:FF" string.
pub fn format_mac(mac: &[u8; 6], buf: &mut MacString) {
    use core::fmt::Write;
    buf.clear();
    let _ = write!(
        buf,
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
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
        assert_eq!(
            HostCommand::SetMode {
                mode: crate::protocol::OperatingMode::Ap
            },
            HostCommand::SetMode {
                mode: crate::protocol::OperatingMode::Ap
            }
        );
        assert_eq!(HostCommand::ExportWigle, HostCommand::ExportWigle);
        assert_eq!(
            HostCommand::ClearProximityTargets,
            HostCommand::ClearProximityTargets
        );
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

    // ── DroneSighting serialization ────────────────────────────────

    #[test]
    fn serialize_drone_sighting_minimal() {
        let mac = MacString::try_from("AA:BB:CC:DD:EE:FF").unwrap();
        let msg = DeviceMessage::DroneSighting {
            mac: &mac,
            rssi: -55,
            source: "ble",
            uas_id: None,
            ua_type: None,
            lat: None,
            lon: None,
            alt: None,
            speed: None,
            operator_id: None,
            rule: None,
            category: None,
            ts: 5000,
        };
        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""type":"drone""#));
        assert!(json.contains(r#""source":"ble""#));
        assert!(json.contains(r#""rssi":-55"#));
        // Optional fields should be omitted
        assert!(!json.contains("uas_id"));
        assert!(!json.contains("lat"));
        assert!(!json.contains("operator_id"));
    }

    #[test]
    fn serialize_drone_sighting_full() {
        let mac = MacString::try_from("AA:BB:CC:DD:EE:FF").unwrap();
        let uas_id = NameString::try_from("UAS-12345").unwrap();
        let operator_id = NameString::try_from("OP-001").unwrap();
        let msg = DeviceMessage::DroneSighting {
            mac: &mac,
            rssi: -40,
            source: "wifi_nan",
            uas_id: Some(&uas_id),
            ua_type: Some(2),
            lat: Some(37.7749),
            lon: Some(-122.4194),
            alt: Some(100.5),
            speed: Some(12.3),
            operator_id: Some(&operator_id),
            rule: Some("DJI Drone"),
            category: Some("Drone"),
            ts: 10000,
        };
        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""uas_id":"UAS-12345""#));
        assert!(json.contains(r#""ua_type":2"#));
        assert!(json.contains(r#""operator_id":"OP-001""#));
        assert!(json.contains(r#""rule":"DJI Drone""#));
        assert!(json.contains(r#""category":"Drone""#));
        assert!(len < 512, "DroneSighting must fit in 512-byte buffer");
    }

    // ── ProximityUpdate serialization ────────────────────────────────

    #[test]
    fn serialize_proximity_update_minimal() {
        let mac = MacString::try_from("11:22:33:44:55:66").unwrap();
        let msg = DeviceMessage::ProximityUpdate {
            mac: &mac,
            rssi: -60,
            distance: 5.2,
            interval: 500,
            label: None,
            ts: 8000,
        };
        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""type":"proximity""#));
        assert!(json.contains(r#""interval":500"#));
        assert!(!json.contains("label"));
    }

    #[test]
    fn serialize_proximity_update_with_label() {
        let mac = MacString::try_from("11:22:33:44:55:66").unwrap();
        let msg = DeviceMessage::ProximityUpdate {
            mac: &mac,
            rssi: -45,
            distance: 1.5,
            interval: 200,
            label: Some("Suspect Vehicle"),
            ts: 9000,
        };
        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""label":"Suspect Vehicle""#));
    }

    // ── Alert serialization ────────────────────────────────────────

    #[test]
    fn serialize_alert_minimal() {
        let msg = DeviceMessage::Alert {
            alert_type: "deauth_flood",
            message: "Deauth flood detected on channel 6",
            severity: "warning",
            mac: None,
            ts: 12000,
        };
        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""type":"alert""#));
        assert!(json.contains(r#""alert_type":"deauth_flood""#));
        assert!(json.contains(r#""severity":"warning""#));
        assert!(!json.contains(r#""mac""#));
    }

    #[test]
    fn serialize_alert_with_mac() {
        let mac = MacString::try_from("AA:BB:CC:DD:EE:FF").unwrap();
        let msg = DeviceMessage::Alert {
            alert_type: "evil_twin",
            message: "Possible evil twin AP",
            severity: "critical",
            mac: Some(&mac),
            ts: 15000,
        };
        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(json.contains(r#""mac":"AA:BB:CC:DD:EE:FF""#));
        assert!(json.contains(r#""severity":"critical""#));
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
