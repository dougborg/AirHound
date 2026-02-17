/// Filter engine for WiFi and BLE scan events.
///
/// Evaluates scan events against the signature database and runtime config.
/// Any filter match causes the result to be emitted. No scoring or state tracking —
/// that's the companion app's job.
use heapless::Vec;

use crate::defaults::{
    self, BLE_MANUFACTURER_IDS, BLE_NAME_PATTERNS, BLE_SERVICE_UUIDS_16, MAC_PREFIXES, SSID_EXACT,
    SSID_KEYWORDS, SSID_PATTERNS, WIFI_NAME_KEYWORDS,
};
use crate::protocol::{MatchDetail, MatchReason};

/// Runtime filter configuration. Allows the companion app to adjust
/// filtering without reflashing.
#[derive(Clone, Copy)]
pub struct FilterConfig {
    /// Minimum RSSI threshold (dBm). Signals weaker than this are ignored.
    pub min_rssi: i8,
    /// Whether WiFi scanning is enabled
    pub wifi_enabled: bool,
    /// Whether BLE scanning is enabled
    pub ble_enabled: bool,
}

impl FilterConfig {
    pub const fn new() -> Self {
        Self {
            min_rssi: -90,
            wifi_enabled: true,
            ble_enabled: true,
        }
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Input data for filtering a WiFi scan event
pub struct WiFiScanInput<'a> {
    pub mac: &'a [u8; 6],
    pub ssid: &'a str,
    pub rssi: i8,
}

/// Input data for filtering a BLE scan event
pub struct BleScanInput<'a> {
    pub mac: &'a [u8; 6],
    pub name: &'a str,
    pub rssi: i8,
    /// 16-bit service UUIDs found in advertisement
    pub service_uuids_16: &'a [u16],
    /// Manufacturer company ID (0 if not present)
    pub manufacturer_id: u16,
}

/// Result of filter evaluation
pub struct FilterResult {
    /// Whether any filter matched
    pub matched: bool,
    /// Up to 4 match reasons
    pub matches: Vec<MatchReason, 4>,
}

impl FilterResult {
    fn new() -> Self {
        Self {
            matched: false,
            matches: Vec::new(),
        }
    }

    fn add_match(&mut self, filter_type: &'static str, detail: &str) {
        if self.matches.len() < 4 {
            let mut d = MatchDetail::new();
            // Truncate detail to fit
            let truncated = if detail.len() <= 32 {
                detail
            } else {
                &detail[..32]
            };
            let _ = d.push_str(truncated);
            let _ = self.matches.push(MatchReason {
                filter_type,
                detail: d,
            });
        }
        self.matched = true;
    }
}

/// Evaluate a WiFi scan event against all signatures.
pub fn filter_wifi(input: &WiFiScanInput, config: &FilterConfig) -> FilterResult {
    let mut result = FilterResult::new();

    if !config.wifi_enabled {
        return result;
    }

    // RSSI threshold check
    if input.rssi < config.min_rssi {
        return result;
    }

    // MAC OUI prefix check
    check_mac_oui(input.mac, &mut result);

    // SSID structured pattern check (e.g., Flock-XXXXXX)
    for pattern in SSID_PATTERNS {
        if pattern.matches(input.ssid) {
            result.add_match("ssid_pattern", pattern.description);
        }
    }

    // SSID exact match check
    for &exact in SSID_EXACT {
        if input.ssid == exact {
            result.add_match("ssid_exact", exact);
        }
    }

    // SSID keyword substring check (case-insensitive)
    let ssid_lower: Vec<u8, 33> = input
        .ssid
        .bytes()
        .take(33)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    let ssid_lower_str = core::str::from_utf8(&ssid_lower).unwrap_or("");

    for &keyword in SSID_KEYWORDS {
        if ssid_lower_str.contains(keyword) {
            result.add_match("ssid_keyword", keyword);
        }
    }

    // WiFi name keyword check (from FlockOff — matches partial names)
    for &keyword in WIFI_NAME_KEYWORDS {
        if ssid_lower_str.contains(keyword) {
            // Only add if not already matched by SSID_KEYWORDS
            if !SSID_KEYWORDS.contains(&keyword) {
                result.add_match("wifi_name", keyword);
            }
        }
    }

    result
}

/// Evaluate a BLE scan event against all signatures.
pub fn filter_ble(input: &BleScanInput, config: &FilterConfig) -> FilterResult {
    let mut result = FilterResult::new();

    if !config.ble_enabled {
        return result;
    }

    // RSSI threshold check
    if input.rssi < config.min_rssi {
        return result;
    }

    // MAC OUI prefix check
    check_mac_oui(input.mac, &mut result);

    // BLE device name pattern check (case-insensitive substring)
    if !input.name.is_empty() {
        let name_lower: Vec<u8, 33> = input
            .name
            .bytes()
            .take(33)
            .map(|b| b.to_ascii_lowercase())
            .collect();
        let name_lower_str = core::str::from_utf8(&name_lower).unwrap_or("");

        for &pattern in BLE_NAME_PATTERNS {
            let pattern_lower: Vec<u8, 33> = pattern
                .bytes()
                .take(33)
                .map(|b| b.to_ascii_lowercase())
                .collect();
            let pattern_lower_str = core::str::from_utf8(&pattern_lower).unwrap_or("");

            if name_lower_str.contains(pattern_lower_str) {
                result.add_match("ble_name", pattern);
            }
        }
    }

    // BLE service UUID check (16-bit)
    for &uuid in input.service_uuids_16 {
        if BLE_SERVICE_UUIDS_16.contains(&uuid) {
            result.add_match("ble_uuid", "Raven service UUID");
        }
        if defaults::BLE_STANDARD_UUIDS_16.contains(&uuid) {
            result.add_match("ble_uuid_std", "Raven standard UUID");
        }
    }

    // BLE manufacturer ID check
    if input.manufacturer_id != 0 {
        if BLE_MANUFACTURER_IDS.contains(&input.manufacturer_id) {
            result.add_match("ble_mfr", "Known manufacturer ID");
        }
    }

    result
}

/// Check MAC address against known OUI prefixes
fn check_mac_oui(mac: &[u8; 6], result: &mut FilterResult) {
    let oui = [mac[0], mac[1], mac[2]];
    for &(ref prefix, vendor) in MAC_PREFIXES {
        if oui == *prefix {
            result.add_match("mac_oui", vendor);
            return; // Only report first match (a MAC can only match one OUI)
        }
    }
}

/// Format a 6-byte MAC address into "AA:BB:CC:DD:EE:FF" string
pub fn format_mac(mac: &[u8; 6], buf: &mut crate::protocol::MacString) {
    use core::fmt::Write;
    let _ = write!(
        buf,
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> FilterConfig {
        FilterConfig::new()
    }

    // ── WiFi filter tests ───────────────────────────────────────────

    #[test]
    fn wifi_known_flock_safety_mac_matches() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            ssid: "SomeNetwork",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "mac_oui"));
    }

    #[test]
    fn wifi_silicon_labs_mac_matches() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x58, 0x8E, 0x81, 0xAA, 0xBB, 0xCC],
            ssid: "",
            rssi: -60,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.matched);
        assert_eq!(result.matches[0].filter_type, "mac_oui");
        assert!(result.matches[0].detail.contains("Silicon Labs"));
    }

    #[test]
    fn wifi_ssid_pattern_flock_matches() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "Flock-A1B2C3",
            rssi: -40,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ssid_pattern"));
    }

    #[test]
    fn wifi_ssid_pattern_penguin_matches() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "Penguin-1234567890",
            rssi: -40,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ssid_pattern"));
    }

    #[test]
    fn wifi_ssid_pattern_flock_wrong_suffix_no_pattern_match() {
        let config = default_config();
        // Too short suffix — pattern should NOT match, but keyword "flock" still matches
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "Flock-A1B",
            rssi: -40,
        };
        let result = filter_wifi(&input, &config);
        // No ssid_pattern match (wrong suffix length)
        assert!(!result
            .matches
            .iter()
            .any(|m| m.filter_type == "ssid_pattern"));
        // But keyword "flock" still matches
        assert!(result.matched);
    }

    #[test]
    fn wifi_ssid_exact_fs_ext_battery_matches() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "FS Ext Battery",
            rssi: -40,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "ssid_exact"));
    }

    #[test]
    fn wifi_ssid_keyword_case_insensitive() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "MyFLOCKNetwork",
            rssi: -40,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ssid_keyword"));
    }

    #[test]
    fn wifi_no_match_for_innocent_network() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03],
            ssid: "Linksys-Home",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config);
        assert!(!result.matched);
        assert!(result.matches.is_empty());
    }

    #[test]
    fn wifi_rssi_below_threshold_no_match() {
        let config = FilterConfig {
            min_rssi: -70,
            ..default_config()
        };
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03], // Known Flock Safety OUI
            ssid: "Flock-A1B2C3",
            rssi: -80, // Below -70 threshold
        };
        let result = filter_wifi(&input, &config);
        assert!(!result.matched);
    }

    #[test]
    fn wifi_disabled_no_match() {
        let config = FilterConfig {
            wifi_enabled: false,
            ..default_config()
        };
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            ssid: "Flock-A1B2C3",
            rssi: -40,
        };
        let result = filter_wifi(&input, &config);
        assert!(!result.matched);
    }

    #[test]
    fn wifi_multiple_match_reasons() {
        let config = default_config();
        // MAC matches Flock Safety AND SSID matches Flock pattern AND keyword
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            ssid: "Flock-A1B2C3",
            rssi: -40,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.matched);
        assert!(result.matches.len() >= 2);
    }

    // ── BLE filter tests ────────────────────────────────────────────

    #[test]
    fn ble_name_flock_matches() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Camera",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "ble_name"));
    }

    #[test]
    fn ble_name_fs_ext_battery_matches() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "FS Ext Battery",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
    }

    #[test]
    fn ble_name_pigvision_case_insensitive() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "PIGVISION-device",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
    }

    #[test]
    fn ble_manufacturer_id_xuntong_matches() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "ble_mfr"));
    }

    #[test]
    fn ble_raven_service_uuid_matches() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS service
            manufacturer_id: 0,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "ble_uuid"));
    }

    #[test]
    fn ble_standard_uuid_matches() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x1819], // Location and Navigation
            manufacturer_id: 0,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_uuid_std"));
    }

    #[test]
    fn ble_no_match_for_unknown_device() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03],
            name: "My Headphones",
            rssi: -50,
            service_uuids_16: &[0x180F], // Battery Service (not surveillance)
            manufacturer_id: 0x004C,     // Apple (not in our list)
        };
        let result = filter_ble(&input, &config);
        assert!(!result.matched);
    }

    #[test]
    fn ble_disabled_no_match() {
        let config = FilterConfig {
            ble_enabled: false,
            ..default_config()
        };
        let input = BleScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            name: "Flock",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8,
        };
        let result = filter_ble(&input, &config);
        assert!(!result.matched);
    }

    #[test]
    fn ble_rssi_below_threshold_no_match() {
        let config = FilterConfig {
            min_rssi: -60,
            ..default_config()
        };
        let input = BleScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            name: "Flock",
            rssi: -70,
            service_uuids_16: &[],
            manufacturer_id: 0,
        };
        let result = filter_ble(&input, &config);
        assert!(!result.matched);
    }

    // ── format_mac tests ────────────────────────────────────────────

    #[test]
    fn format_mac_correct_output() {
        let mac = [0xB4, 0x1E, 0x52, 0xAB, 0xCD, 0xEF];
        let mut buf = crate::protocol::MacString::new();
        format_mac(&mac, &mut buf);
        assert_eq!(buf.as_str(), "B4:1E:52:AB:CD:EF");
    }

    #[test]
    fn format_mac_zero_padded() {
        let mac = [0x00, 0x0A, 0x0B, 0x00, 0x00, 0x01];
        let mut buf = crate::protocol::MacString::new();
        format_mac(&mac, &mut buf);
        assert_eq!(buf.as_str(), "00:0A:0B:00:00:01");
    }
}
