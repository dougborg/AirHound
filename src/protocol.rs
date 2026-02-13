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

/// Commands sent from the companion app to the device
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd")]
pub enum HostCommand {
    /// Start scanning
    #[serde(rename = "start")]
    Start,
    /// Stop scanning
    #[serde(rename = "stop")]
    Stop,
    /// Request current status
    #[serde(rename = "status")]
    GetStatus,
    /// Update minimum RSSI threshold
    #[serde(rename = "set_rssi")]
    SetRssi {
        /// Minimum RSSI (negative dBm value)
        min_rssi: i8,
    },
    /// Enable or disable the buzzer (M5StickC only)
    #[serde(rename = "set_buzzer")]
    SetBuzzer { enabled: bool },
}

/// Firmware version string
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum size of a serialized JSON message
pub const MAX_MSG_LEN: usize = 512;

/// Buffer type for serialized JSON messages
pub type MsgBuffer = Vec<u8, MAX_MSG_LEN>;
