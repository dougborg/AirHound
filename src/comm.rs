/// Communication helpers — NDJSON serialization, command parsing, line reader.
///
/// Pure protocol logic with no hardware or OS dependencies.
/// BLE GATT definitions and channel types are in the firmware binary (`main.rs`).
use crate::filter::FilterConfig;
use crate::protocol::{DeviceMessage, HostCommand, RawCommand, MAX_MSG_LEN};

/// BLE GATT service UUIDs for AirHound.
///
/// These duplicate the string literals in the `#[gatt_service]` and `#[characteristic]`
/// proc macro attributes in the firmware binary — Rust proc macros require string literals,
/// so we can't reference these constants there. Kept here as the canonical source of truth.
#[allow(dead_code)]
pub mod ble_uuids {
    /// AirHound primary service UUID
    pub const SERVICE: &str = "4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d";
    /// TX characteristic — scan results, notify
    pub const TX_CHAR: &str = "4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d";
    /// RX characteristic — commands, write
    pub const RX_CHAR: &str = "4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d";
}

/// BLE advertising name
pub const BLE_ADV_NAME: &str = "AirHound";

/// Maximum BLE notification payload (MTU-3)
pub const BLE_MAX_NOTIFY: usize = 20;

// ── Serialization helpers ──────────────────────────────────────────────

/// Serialize a DeviceMessage to JSON bytes and write to the output buffer.
/// Returns the number of bytes written, or None if serialization failed.
pub fn serialize_message(msg: &DeviceMessage, buf: &mut [u8]) -> Option<usize> {
    match serde_json_core::to_slice(msg, buf) {
        Ok(len) if len < buf.len() => {
            // Append newline for NDJSON
            buf[len] = b'\n';
            Some(len + 1)
        }
        _ => None,
    }
}

/// Parse a MAC address string "AA:BB:CC:DD:EE:FF" into a 6-byte array.
pub fn parse_mac_string(s: &str) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let mut idx = 0;
    for part in s.split(':') {
        if idx >= 6 || part.len() != 2 {
            return None;
        }
        mac[idx] = u8::from_str_radix(part, 16).ok()?;
        idx += 1;
    }
    if idx == 6 {
        Some(mac)
    } else {
        None
    }
}

/// Deserialize a HostCommand from a JSON byte slice.
///
/// Uses [`RawCommand`] as an intermediate because `serde_json_core` does not
/// support internally tagged enums (no `deserialize_any`).
pub fn parse_command(data: &[u8]) -> Option<HostCommand> {
    use crate::protocol::OperatingMode;

    // Strip trailing newline/whitespace
    let trimmed = trim_trailing_whitespace(data);
    if trimmed.is_empty() {
        return None;
    }
    let (raw, _) = serde_json_core::from_slice::<RawCommand>(trimmed).ok()?;
    match raw.cmd.as_str() {
        "start" => Some(HostCommand::Start),
        "stop" => Some(HostCommand::Stop),
        "status" => Some(HostCommand::GetStatus),
        "set_rssi" => raw
            .min_rssi
            .map(|min_rssi| HostCommand::SetRssi { min_rssi }),
        "set_buzzer" => raw
            .enabled
            .map(|enabled| HostCommand::SetBuzzer { enabled }),
        "set_mode" => {
            let mode_str = raw.mode.as_ref()?;
            let mode = match mode_str.as_str() {
                "ap" => OperatingMode::Ap,
                "client" => OperatingMode::Client,
                "dongle" => OperatingMode::Dongle,
                "standalone" => OperatingMode::Standalone,
                _ => return None,
            };
            Some(HostCommand::SetMode { mode })
        }
        "add_proximity" => {
            let mac_str = raw.mac.as_ref()?;
            let mac = parse_mac_string(mac_str.as_str())?;
            let label = raw.label.unwrap_or_default();
            Some(HostCommand::AddProximityTarget { mac, label })
        }
        "remove_proximity" => {
            let mac_str = raw.mac.as_ref()?;
            let mac = parse_mac_string(mac_str.as_str())?;
            Some(HostCommand::RemoveProximityTarget { mac })
        }
        "clear_proximity" => Some(HostCommand::ClearProximityTargets),
        "add_watchlist" => {
            let id = raw.id?;
            let mac_str = raw.mac.as_ref()?;
            let mac = parse_mac_string(mac_str.as_str())?;
            let full_mac = raw.full_mac.unwrap_or(false);
            let label = raw.label.unwrap_or_default();
            Some(HostCommand::AddWatchlist {
                id,
                mac,
                full_mac,
                label,
            })
        }
        "remove_watchlist" => {
            let id = raw.id?;
            Some(HostCommand::RemoveWatchlist { id })
        }
        "enable_category" => {
            let category = raw.category?;
            let enabled = raw.enabled?;
            Some(HostCommand::EnableCategory { category, enabled })
        }
        "export_wigle" => Some(HostCommand::ExportWigle),
        _ => None,
    }
}

/// Process a received host command and update state accordingly.
///
/// Updates `config` and `scanning` as directed. Returns `Some(enabled)` for
/// `SetBuzzer` commands so the caller can apply hardware-specific side effects.
pub fn handle_command(
    cmd: &HostCommand,
    config: &mut FilterConfig,
    scanning: &mut bool,
) -> Option<bool> {
    match cmd {
        HostCommand::Start => {
            *scanning = true;
            log::info!("Scanning started by host command");
            None
        }
        HostCommand::Stop => {
            *scanning = false;
            log::info!("Scanning stopped by host command");
            None
        }
        HostCommand::GetStatus => {
            // Status message will be constructed by the caller with real uptime/heap data
            None
        }
        HostCommand::SetRssi { min_rssi } => {
            config.min_rssi = *min_rssi;
            log::info!("RSSI threshold set to {}", min_rssi);
            None
        }
        HostCommand::SetBuzzer { enabled } => {
            log::info!("Buzzer {}", if *enabled { "enabled" } else { "disabled" });
            Some(*enabled)
        }
        HostCommand::SetMode { mode } => {
            log::info!("Set mode: {:?}", mode);
            None
        }
        HostCommand::AddProximityTarget { mac, label } => {
            log::info!(
                "Add proximity target: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} label={}",
                mac[0],
                mac[1],
                mac[2],
                mac[3],
                mac[4],
                mac[5],
                label.as_str()
            );
            None
        }
        HostCommand::RemoveProximityTarget { mac } => {
            log::info!(
                "Remove proximity target: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                mac[0],
                mac[1],
                mac[2],
                mac[3],
                mac[4],
                mac[5]
            );
            None
        }
        HostCommand::ClearProximityTargets => {
            log::info!("Clear all proximity targets");
            None
        }
        HostCommand::AddWatchlist {
            id,
            mac,
            full_mac,
            label,
        } => {
            log::info!(
                "Add watchlist: id={} mac={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} full={} label={}",
                id, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
                full_mac, label.as_str()
            );
            None
        }
        HostCommand::RemoveWatchlist { id } => {
            log::info!("Remove watchlist: id={}", id);
            None
        }
        HostCommand::EnableCategory { category, enabled } => {
            log::info!("Enable category: {} = {}", category.as_str(), enabled);
            None
        }
        HostCommand::ExportWigle => {
            log::info!("Export WiGLE requested");
            None
        }
    }
}

// ── Serial NDJSON reader ───────────────────────────────────────────────

/// Serial NDJSON reader state machine.
/// Accumulates bytes until a newline is found, then yields the line.
pub struct LineReader {
    buf: [u8; MAX_MSG_LEN],
    pos: usize,
}

impl LineReader {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_MSG_LEN],
            pos: 0,
        }
    }

    /// Feed a byte into the reader. Returns a complete line (without newline)
    /// when one is detected.
    pub fn feed(&mut self, byte: u8) -> Option<&[u8]> {
        if byte == b'\n' || byte == b'\r' {
            if self.pos > 0 {
                let line = &self.buf[..self.pos];
                self.pos = 0;
                Some(line)
            } else {
                None
            }
        } else if self.pos < self.buf.len() {
            self.buf[self.pos] = byte;
            self.pos += 1;
            None
        } else {
            // Overflow — discard and reset
            self.pos = 0;
            None
        }
    }
}

fn trim_trailing_whitespace(data: &[u8]) -> &[u8] {
    let mut end = data.len();
    while end > 0
        && (data[end - 1] == b' '
            || data[end - 1] == b'\n'
            || data[end - 1] == b'\r'
            || data[end - 1] == b'\t')
    {
        end -= 1;
    }
    &data[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        DeviceMessage, HostCommand, MacString, MatchReason, NameString, VERSION,
    };
    use heapless::Vec;

    // ── serialize_message tests ─────────────────────────────────────

    #[test]
    fn serialize_produces_ndjson() {
        let msg = DeviceMessage::Status {
            scanning: true,
            uptime: 60,
            heap_free: 32000,
            ble_clients: 0,
            board: "test",
            version: VERSION,
        };
        let mut buf = [0u8; 512];
        let len = serialize_message(&msg, &mut buf).unwrap();
        assert!(len > 0);
        // Must end with newline (NDJSON)
        assert_eq!(buf[len - 1], b'\n');
        // Must be valid JSON before the newline
        let json = core::str::from_utf8(&buf[..len - 1]).unwrap();
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
    }

    #[test]
    fn serialize_returns_none_when_buffer_too_small() {
        let msg = DeviceMessage::Status {
            scanning: true,
            uptime: 60,
            heap_free: 32000,
            ble_clients: 0,
            board: "test",
            version: VERSION,
        };
        // Buffer too small for JSON + newline
        let mut buf = [0u8; 10];
        assert!(serialize_message(&msg, &mut buf).is_none());
    }

    #[test]
    fn serialize_wifi_scan_is_valid_json() {
        let mac = MacString::try_from("AA:BB:CC:DD:EE:FF").unwrap();
        let ssid = NameString::try_from("TestSSID").unwrap();
        let matches = Vec::<MatchReason, 4>::new();
        let msg = DeviceMessage::WiFiScan {
            mac: &mac,
            ssid: &ssid,
            rssi: -50,
            ch: 1,
            frame: "beacon",
            matches: &matches,
            rule: None,
            ts: 100,
        };
        let mut buf = [0u8; 512];
        let len = serialize_message(&msg, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len - 1]).unwrap();
        assert!(json.contains("\"type\":\"wifi\""));
    }

    // ── parse_command tests ─────────────────────────────────────────

    #[test]
    fn parse_start_command() {
        let cmd = parse_command(br#"{"cmd":"start"}"#).unwrap();
        assert!(matches!(cmd, HostCommand::Start));
    }

    #[test]
    fn parse_stop_command() {
        let cmd = parse_command(br#"{"cmd":"stop"}"#).unwrap();
        assert!(matches!(cmd, HostCommand::Stop));
    }

    #[test]
    fn parse_status_command() {
        let cmd = parse_command(br#"{"cmd":"status"}"#).unwrap();
        assert!(matches!(cmd, HostCommand::GetStatus));
    }

    #[test]
    fn parse_set_rssi_command() {
        let cmd = parse_command(br#"{"cmd":"set_rssi","min_rssi":-80}"#).unwrap();
        match cmd {
            HostCommand::SetRssi { min_rssi } => assert_eq!(min_rssi, -80),
            _ => panic!("Expected SetRssi"),
        }
    }

    #[test]
    fn parse_set_buzzer_command() {
        let cmd = parse_command(br#"{"cmd":"set_buzzer","enabled":true}"#).unwrap();
        match cmd {
            HostCommand::SetBuzzer { enabled } => assert!(enabled),
            _ => panic!("Expected SetBuzzer"),
        }
    }

    #[test]
    fn parse_command_strips_trailing_whitespace() {
        let cmd = parse_command(b"{\"cmd\":\"start\"}\n  \r\n").unwrap();
        assert!(matches!(cmd, HostCommand::Start));
    }

    #[test]
    fn parse_command_rejects_malformed_json() {
        assert!(parse_command(b"not json at all").is_none());
    }

    #[test]
    fn parse_command_rejects_empty_input() {
        assert!(parse_command(b"").is_none());
        assert!(parse_command(b"   \n").is_none());
    }

    #[test]
    fn parse_command_rejects_unknown_command() {
        assert!(parse_command(br#"{"cmd":"restart"}"#).is_none());
        assert!(parse_command(br#"{"cmd":"reboot"}"#).is_none());
    }

    #[test]
    fn parse_set_rssi_missing_field_returns_none() {
        assert!(parse_command(br#"{"cmd":"set_rssi"}"#).is_none());
    }

    #[test]
    fn parse_set_buzzer_missing_field_returns_none() {
        assert!(parse_command(br#"{"cmd":"set_buzzer"}"#).is_none());
    }

    #[test]
    fn round_trip_parse_then_handle() {
        let cmd = parse_command(br#"{"cmd":"set_rssi","min_rssi":-75}"#).unwrap();
        let mut config = FilterConfig::new();
        let mut scanning = true;
        handle_command(&cmd, &mut config, &mut scanning);
        assert_eq!(config.min_rssi, -75);
        assert!(scanning); // set_rssi should not change scanning state
    }

    // ── parse_mac_string tests ────────────────────────────────────

    #[test]
    fn parse_mac_string_valid() {
        let mac = parse_mac_string("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn parse_mac_string_lowercase() {
        let mac = parse_mac_string("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn parse_mac_string_too_short() {
        assert!(parse_mac_string("AA:BB:CC").is_none());
    }

    #[test]
    fn parse_mac_string_invalid_hex() {
        assert!(parse_mac_string("GG:HH:II:JJ:KK:LL").is_none());
    }

    #[test]
    fn parse_mac_string_wrong_separator() {
        assert!(parse_mac_string("AA-BB-CC-DD-EE-FF").is_none());
    }

    // ── New command parsing tests ─────────────────────────────────

    #[test]
    fn parse_set_mode_command() {
        let cmd = parse_command(br#"{"cmd":"set_mode","mode":"ap"}"#).unwrap();
        match cmd {
            HostCommand::SetMode { mode } => {
                assert_eq!(mode, crate::protocol::OperatingMode::Ap)
            }
            _ => panic!("Expected SetMode"),
        }
    }

    #[test]
    fn parse_set_mode_all_variants() {
        for (json_val, expected) in [
            ("ap", crate::protocol::OperatingMode::Ap),
            ("client", crate::protocol::OperatingMode::Client),
            ("dongle", crate::protocol::OperatingMode::Dongle),
            ("standalone", crate::protocol::OperatingMode::Standalone),
        ] {
            let json = format!(r#"{{"cmd":"set_mode","mode":"{}"}}"#, json_val);
            let cmd = parse_command(json.as_bytes()).unwrap();
            match cmd {
                HostCommand::SetMode { mode } => assert_eq!(mode, expected),
                _ => panic!("Expected SetMode for {}", json_val),
            }
        }
    }

    #[test]
    fn parse_set_mode_invalid_mode() {
        assert!(parse_command(br#"{"cmd":"set_mode","mode":"invalid"}"#).is_none());
    }

    #[test]
    fn parse_set_mode_missing_mode() {
        assert!(parse_command(br#"{"cmd":"set_mode"}"#).is_none());
    }

    #[test]
    fn parse_add_proximity_target() {
        let cmd =
            parse_command(br#"{"cmd":"add_proximity","mac":"AA:BB:CC:DD:EE:FF","label":"Car"}"#)
                .unwrap();
        match cmd {
            HostCommand::AddProximityTarget { mac, label } => {
                assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
                assert_eq!(label.as_str(), "Car");
            }
            _ => panic!("Expected AddProximityTarget"),
        }
    }

    #[test]
    fn parse_add_proximity_missing_mac() {
        assert!(parse_command(br#"{"cmd":"add_proximity","label":"Car"}"#).is_none());
    }

    #[test]
    fn parse_remove_proximity_target() {
        let cmd =
            parse_command(br#"{"cmd":"remove_proximity","mac":"11:22:33:44:55:66"}"#).unwrap();
        match cmd {
            HostCommand::RemoveProximityTarget { mac } => {
                assert_eq!(mac, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
            }
            _ => panic!("Expected RemoveProximityTarget"),
        }
    }

    #[test]
    fn parse_clear_proximity() {
        let cmd = parse_command(br#"{"cmd":"clear_proximity"}"#).unwrap();
        assert_eq!(cmd, HostCommand::ClearProximityTargets);
    }

    #[test]
    fn parse_add_watchlist() {
        let cmd = parse_command(
            br#"{"cmd":"add_watchlist","id":42,"mac":"AA:BB:CC:DD:EE:FF","full_mac":true,"label":"Target"}"#,
        )
        .unwrap();
        match cmd {
            HostCommand::AddWatchlist {
                id,
                mac,
                full_mac,
                label,
            } => {
                assert_eq!(id, 42);
                assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
                assert!(full_mac);
                assert_eq!(label.as_str(), "Target");
            }
            _ => panic!("Expected AddWatchlist"),
        }
    }

    #[test]
    fn parse_add_watchlist_missing_id() {
        assert!(parse_command(
            br#"{"cmd":"add_watchlist","mac":"AA:BB:CC:DD:EE:FF","full_mac":true}"#
        )
        .is_none());
    }

    #[test]
    fn parse_remove_watchlist() {
        let cmd = parse_command(br#"{"cmd":"remove_watchlist","id":7}"#).unwrap();
        match cmd {
            HostCommand::RemoveWatchlist { id } => assert_eq!(id, 7),
            _ => panic!("Expected RemoveWatchlist"),
        }
    }

    #[test]
    fn parse_enable_category() {
        let cmd = parse_command(br#"{"cmd":"enable_category","category":"Drone","enabled":false}"#)
            .unwrap();
        match cmd {
            HostCommand::EnableCategory { category, enabled } => {
                assert_eq!(category.as_str(), "Drone");
                assert!(!enabled);
            }
            _ => panic!("Expected EnableCategory"),
        }
    }

    #[test]
    fn parse_enable_category_missing_fields() {
        assert!(parse_command(br#"{"cmd":"enable_category","category":"Drone"}"#).is_none());
        assert!(parse_command(br#"{"cmd":"enable_category","enabled":true}"#).is_none());
    }

    #[test]
    fn parse_export_wigle() {
        let cmd = parse_command(br#"{"cmd":"export_wigle"}"#).unwrap();
        assert_eq!(cmd, HostCommand::ExportWigle);
    }

    // ── handle_command for new commands ────────────────────────────

    #[test]
    fn handle_new_commands_return_none() {
        let mut config = FilterConfig::new();
        let mut scanning = true;

        let commands = [
            HostCommand::SetMode {
                mode: crate::protocol::OperatingMode::Dongle,
            },
            HostCommand::AddProximityTarget {
                mac: [0; 6],
                label: heapless::String::new(),
            },
            HostCommand::RemoveProximityTarget { mac: [0; 6] },
            HostCommand::ClearProximityTargets,
            HostCommand::AddWatchlist {
                id: 1,
                mac: [0; 6],
                full_mac: true,
                label: heapless::String::new(),
            },
            HostCommand::RemoveWatchlist { id: 1 },
            HostCommand::EnableCategory {
                category: heapless::String::new(),
                enabled: true,
            },
            HostCommand::ExportWigle,
        ];

        for cmd in &commands {
            let result = handle_command(cmd, &mut config, &mut scanning);
            assert!(result.is_none(), "New command {:?} should return None", cmd);
        }
        // None of these should change scanning state
        assert!(scanning);
    }

    // ── handle_command tests ────────────────────────────────────────

    #[test]
    fn handle_start_sets_scanning_true() {
        let cmd = HostCommand::Start;
        let mut config = FilterConfig::new();
        let mut scanning = false;
        let result = handle_command(&cmd, &mut config, &mut scanning);
        assert!(scanning);
        assert!(result.is_none());
    }

    #[test]
    fn handle_stop_sets_scanning_false() {
        let cmd = HostCommand::Stop;
        let mut config = FilterConfig::new();
        let mut scanning = true;
        let result = handle_command(&cmd, &mut config, &mut scanning);
        assert!(!scanning);
        assert!(result.is_none());
    }

    #[test]
    fn handle_set_rssi_updates_config() {
        let cmd = HostCommand::SetRssi { min_rssi: -75 };
        let mut config = FilterConfig::new();
        let mut scanning = true;
        handle_command(&cmd, &mut config, &mut scanning);
        assert_eq!(config.min_rssi, -75);
    }

    #[test]
    fn handle_set_buzzer_returns_state() {
        let cmd = HostCommand::SetBuzzer { enabled: false };
        let mut config = FilterConfig::new();
        let mut scanning = true;
        let result = handle_command(&cmd, &mut config, &mut scanning);
        assert_eq!(result, Some(false));

        let cmd = HostCommand::SetBuzzer { enabled: true };
        let result = handle_command(&cmd, &mut config, &mut scanning);
        assert_eq!(result, Some(true));
    }

    #[test]
    fn handle_get_status_returns_none() {
        let cmd = HostCommand::GetStatus;
        let mut config = FilterConfig::new();
        let mut scanning = true;
        let result = handle_command(&cmd, &mut config, &mut scanning);
        assert!(result.is_none());
        // Should not modify state
        assert!(scanning);
    }

    // ── LineReader tests ────────────────────────────────────────────

    #[test]
    fn line_reader_yields_on_newline() {
        let mut reader = LineReader::new();
        assert!(reader.feed(b'h').is_none());
        assert!(reader.feed(b'i').is_none());
        let line = reader.feed(b'\n').unwrap();
        assert_eq!(line, b"hi");
    }

    #[test]
    fn line_reader_yields_on_carriage_return() {
        let mut reader = LineReader::new();
        reader.feed(b'o');
        reader.feed(b'k');
        let line = reader.feed(b'\r').unwrap();
        assert_eq!(line, b"ok");
    }

    #[test]
    fn line_reader_skips_empty_lines() {
        let mut reader = LineReader::new();
        assert!(reader.feed(b'\n').is_none());
        assert!(reader.feed(b'\r').is_none());
        assert!(reader.feed(b'\n').is_none());
    }

    #[test]
    fn line_reader_accumulates_json() {
        let mut reader = LineReader::new();
        let json = br#"{"cmd":"start"}"#;
        for &byte in &json[..json.len() - 1] {
            assert!(reader.feed(byte).is_none());
        }
        // Feed last byte
        assert!(reader.feed(json[json.len() - 1]).is_none());
        // Feed newline to yield
        let line = reader.feed(b'\n').unwrap();
        assert_eq!(line, &json[..]);
    }

    #[test]
    fn line_reader_handles_overflow() {
        let mut reader = LineReader::new();
        // Fill the buffer completely (MAX_MSG_LEN = 512 bytes)
        for i in 0..MAX_MSG_LEN {
            reader.feed(b'A' + (i % 26) as u8);
        }
        // Next byte overflows — should discard
        assert!(reader.feed(b'X').is_none());
        // After overflow reset, newline on empty buffer yields nothing
        assert!(reader.feed(b'\n').is_none());
        // But new data works
        reader.feed(b'o');
        reader.feed(b'k');
        let line = reader.feed(b'\n').unwrap();
        assert_eq!(line, b"ok");
    }

    #[test]
    fn line_reader_multiple_lines() {
        let mut reader = LineReader::new();
        for &b in b"line1\nline2\n" {
            if let Some(line) = reader.feed(b) {
                let s = core::str::from_utf8(line).unwrap();
                assert!(s == "line1" || s == "line2");
            }
        }
    }
}
