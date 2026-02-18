/// Configurable filter engine for WiFi and BLE scan results.
///
/// Evaluates scan results against compiled-in defaults and runtime config.
/// Any filter match causes the result to be emitted. No scoring or state tracking —
/// that's the companion app's job.
use heapless::Vec;

use crate::defaults::{
    self, BLE_AD_BYTES_PATTERNS, BLE_MANUFACTURER_IDS, BLE_NAME_PATTERNS, BLE_SERVICE_UUIDS_16,
    MAC_PREFIXES, SSID_EXACT, SSID_KEYWORDS, SSID_PATTERNS, WIFI_NAME_KEYWORDS,
};
use crate::protocol::{MatchDetail, MatchReason};
use crate::rules::{evaluate_rules, RuleDb, SigMatchSet};

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

/// Input data for filtering a WiFi scan result
pub struct WiFiScanInput<'a> {
    pub mac: &'a [u8; 6],
    pub ssid: &'a str,
    pub rssi: i8,
}

/// Input data for filtering a BLE scan result
pub struct BleScanInput<'a> {
    pub mac: &'a [u8; 6],
    pub name: &'a str,
    pub rssi: i8,
    /// 16-bit service UUIDs found in advertisement
    pub service_uuids_16: &'a [u16],
    /// Manufacturer company ID (0 if not present)
    pub manufacturer_id: u16,
    /// Raw advertisement data bytes for byte-pattern matching
    pub raw_ad: &'a [u8],
}

/// Result of filter evaluation
pub struct FilterResult {
    /// Whether any filter matched
    pub matched: bool,
    /// Up to 4 match reasons
    pub matches: Vec<MatchReason, 4>,
    /// Bitset of which signature indices matched (for rule evaluation)
    pub sig_matches: SigMatchSet,
    /// Names of matched rules (populated by `filter_*_with_rules`)
    pub rule_names: Vec<&'static str, 4>,
}

impl FilterResult {
    fn new() -> Self {
        Self {
            matched: false,
            matches: Vec::new(),
            sig_matches: SigMatchSet::new(),
            rule_names: Vec::new(),
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

/// Evaluate a WiFi scan result against all configured filters.
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
    for (i, pattern) in SSID_PATTERNS.iter().enumerate() {
        if pattern.matches(input.ssid) {
            result.sig_matches.set(defaults::SIG_IDX_SSID_PATTERN_START + i as u16);
            result.add_match("ssid_pattern", pattern.description);
        }
    }

    // SSID exact match check
    for (i, &exact) in SSID_EXACT.iter().enumerate() {
        if input.ssid == exact {
            result.sig_matches.set(defaults::SIG_IDX_SSID_EXACT_START + i as u16);
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

    for (i, &keyword) in SSID_KEYWORDS.iter().enumerate() {
        if ssid_lower_str.contains(keyword) {
            result.sig_matches.set(defaults::SIG_IDX_SSID_KEYWORD_START + i as u16);
            result.add_match("ssid_keyword", keyword);
        }
    }

    // WiFi name keyword check (from FlockOff — matches partial names)
    for (i, &keyword) in WIFI_NAME_KEYWORDS.iter().enumerate() {
        if ssid_lower_str.contains(keyword) {
            result.sig_matches.set(defaults::SIG_IDX_WIFI_NAME_START + i as u16);
            // Only add if not already matched by SSID_KEYWORDS
            if !SSID_KEYWORDS.contains(&keyword) {
                result.add_match("wifi_name", keyword);
            }
        }
    }

    result
}

/// Evaluate a BLE scan result against all configured filters.
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

        for (i, &pattern) in BLE_NAME_PATTERNS.iter().enumerate() {
            let pattern_lower: Vec<u8, 33> = pattern
                .bytes()
                .take(33)
                .map(|b| b.to_ascii_lowercase())
                .collect();
            let pattern_lower_str = core::str::from_utf8(&pattern_lower).unwrap_or("");

            if name_lower_str.contains(pattern_lower_str) {
                result.sig_matches.set(defaults::SIG_IDX_BLE_NAME_START + i as u16);
                result.add_match("ble_name", pattern);
            }
        }
    }

    // BLE service UUID check (16-bit)
    for &uuid in input.service_uuids_16 {
        for (i, &known) in BLE_SERVICE_UUIDS_16.iter().enumerate() {
            if uuid == known {
                result.sig_matches.set(defaults::SIG_IDX_BLE_UUID_START + i as u16);
                result.add_match("ble_uuid", "Raven service UUID");
            }
        }
        for (i, &known) in defaults::BLE_STANDARD_UUIDS_16.iter().enumerate() {
            if uuid == known {
                result.sig_matches.set(defaults::SIG_IDX_BLE_STD_UUID_START + i as u16);
                result.add_match("ble_uuid_std", "Raven standard UUID");
            }
        }
    }

    // BLE manufacturer ID check
    if input.manufacturer_id != 0 {
        for (i, &known) in BLE_MANUFACTURER_IDS.iter().enumerate() {
            if input.manufacturer_id == known {
                result.sig_matches.set(defaults::SIG_IDX_BLE_MFR_START + i as u16);
                result.add_match("ble_mfr", "Known manufacturer ID");
            }
        }
    }

    // BLE advertisement byte pattern check
    if !input.raw_ad.is_empty() {
        check_ble_ad_bytes(input.raw_ad, &mut result);
    }

    result
}

/// Evaluate a WiFi scan event against signatures and then against rules.
pub fn filter_wifi_with_rules(
    input: &WiFiScanInput,
    config: &FilterConfig,
    db: &RuleDb,
) -> FilterResult {
    let mut result = filter_wifi(input, config);
    apply_rules(&mut result, db);
    result
}

/// Evaluate a BLE scan event against signatures and then against rules.
pub fn filter_ble_with_rules(
    input: &BleScanInput,
    config: &FilterConfig,
    db: &RuleDb,
) -> FilterResult {
    let mut result = filter_ble(input, config);
    apply_rules(&mut result, db);
    result
}

/// Run rule evaluation on a filter result and populate `rule_names`.
fn apply_rules(result: &mut FilterResult, db: &RuleDb) {
    if !result.matched {
        return;
    }
    let matched_indices = evaluate_rules(db, &result.sig_matches);
    for &idx in &matched_indices {
        if let Some(rule) = db.rules.get(idx as usize) {
            let _ = result.rule_names.push(rule.name);
        }
    }
}

/// Check MAC address against known OUI prefixes
fn check_mac_oui(mac: &[u8; 6], result: &mut FilterResult) {
    let oui = [mac[0], mac[1], mac[2]];
    for (i, &(ref prefix, vendor)) in MAC_PREFIXES.iter().enumerate() {
        if oui == *prefix {
            result.sig_matches.set(defaults::SIG_IDX_MAC_OUI_START + i as u16);
            result.add_match("mac_oui", vendor);
            return; // Only report first match (a MAC can only match one OUI)
        }
    }
}

/// Extract manufacturer-specific data sections from raw AD bytes.
///
/// AD type 0xFF = manufacturer-specific data. The data after the type byte
/// contains the company ID (2 bytes LE) followed by manufacturer payload.
/// We return the full data portion (including company ID bytes) for pattern matching.
fn find_manufacturer_data(raw_ad: &[u8]) -> Option<&[u8]> {
    let mut pos = 0;
    while pos < raw_ad.len() {
        let len = raw_ad[pos] as usize;
        if len == 0 || pos + 1 + len > raw_ad.len() {
            break;
        }
        let ad_type = raw_ad[pos + 1];
        if ad_type == 0xFF {
            return Some(&raw_ad[pos + 2..pos + 1 + len]);
        }
        pos += 1 + len;
    }
    None
}

/// Check raw BLE advertisement data against known byte patterns.
fn check_ble_ad_bytes(raw_ad: &[u8], result: &mut FilterResult) {
    let mfr_data = find_manufacturer_data(raw_ad);

    for (i, pattern) in BLE_AD_BYTES_PATTERNS.iter().enumerate() {
        let matched = match pattern.offset {
            Some(offset) => {
                // Fixed offset match within manufacturer-specific data
                if let Some(data) = mfr_data {
                    data.len() >= offset + pattern.bytes.len()
                        && data[offset..offset + pattern.bytes.len()] == *pattern.bytes
                } else {
                    false
                }
            }
            None => {
                // Search anywhere in raw AD data
                raw_ad
                    .windows(pattern.bytes.len())
                    .any(|w| w == pattern.bytes)
            }
        };
        if matched {
            result.sig_matches.set(defaults::SIG_IDX_BLE_AD_BYTES_START + i as u16);
            result.add_match("ble_ad_bytes", pattern.description);
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
            raw_ad: &[],
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
            raw_ad: &[],
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
            raw_ad: &[],
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
            raw_ad: &[],
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
            raw_ad: &[],
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
            raw_ad: &[],
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
            raw_ad: &[],
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
            raw_ad: &[],
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
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config);
        assert!(!result.matched);
    }

    // ── BLE AD bytes tests ────────────────────────────────────────

    #[test]
    fn ble_ad_bytes_airtag_findmy_matches() {
        let config = default_config();
        // Realistic AirTag AD: manufacturer-specific data with Apple FindMy header
        // AD structure: len=0x1B, type=0xFF, data=[0x4C, 0x00, 0x12, 0x19, ...]
        let raw_ad = [
            0x1B, 0xFF, 0x4C, 0x00, 0x12, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_ad_bytes"
                && m.detail.as_str() == "Apple AirTag"));
    }

    #[test]
    fn ble_ad_bytes_flipper_zero_white_matches() {
        let config = default_config();
        // AD containing Flipper Zero White bytes [0x80, 0x30] somewhere
        let raw_ad = [
            0x02, 0x01, 0x06, // Flags
            0x05, 0xFF, 0x80, 0x30, 0x01, 0x02, // Manufacturer data with 0x80,0x30
        ];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_ad_bytes"
                && m.detail.as_str() == "Flipper Zero"));
    }

    #[test]
    fn ble_ad_bytes_flipper_zero_black_matches() {
        let config = default_config();
        // Flipper Zero Black: [0x81, 0x30]
        let raw_ad = [0x04, 0xFF, 0x81, 0x30, 0x01];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config);
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_ad_bytes"));
    }

    #[test]
    fn ble_ad_bytes_airtag_wrong_offset_no_match() {
        let config = default_config();
        // AirTag bytes at wrong position (not at start of manufacturer data)
        // Manufacturer data: [0x00, 0x4C, 0x00, 0x12, 0x19] — offset by 1
        let raw_ad = [0x06, 0xFF, 0x00, 0x4C, 0x00, 0x12, 0x19];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config);
        // Should NOT match AirTag (offset-sensitive pattern)
        assert!(!result
            .matches
            .iter()
            .any(|m| m.detail.as_str() == "Apple AirTag"));
    }

    #[test]
    fn ble_ad_bytes_no_match_for_random_data() {
        let config = default_config();
        let raw_ad = [
            0x02, 0x01, 0x06, // Flags
            0x03, 0xFF, 0xAA, 0xBB, // Random manufacturer data
        ];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config);
        assert!(!result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_ad_bytes"));
    }

    #[test]
    fn ble_ad_bytes_empty_raw_ad_no_match() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config);
        assert!(!result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_ad_bytes"));
    }

    // ── sig_matches bitset population tests ────────────────────────

    #[test]
    fn sig_matches_populated_for_wifi_mac_oui() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03], // Flock Safety OUI = index 0
            ssid: "",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.sig_matches.get(defaults::SIG_IDX_MAC_OUI_START + 0));
    }

    #[test]
    fn sig_matches_populated_for_wifi_ssid_pattern() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "Flock-A1B2C3",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.sig_matches.get(defaults::SIG_IDX_SSID_PATTERN_START + 0));
        // Also sets keyword "flock"
        assert!(result.sig_matches.get(defaults::SIG_IDX_SSID_KEYWORD_START + 0));
    }

    #[test]
    fn sig_matches_populated_for_wifi_ssid_exact() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "FS Ext Battery",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config);
        assert!(result.sig_matches.get(defaults::SIG_IDX_SSID_EXACT_START + 0));
    }

    #[test]
    fn sig_matches_populated_for_ble_name() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Camera",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config);
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_NAME_START + 0)); // "Flock"
    }

    #[test]
    fn sig_matches_populated_for_ble_uuid() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS = index 0
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config);
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_UUID_START + 0));
    }

    #[test]
    fn sig_matches_populated_for_ble_mfr() {
        let config = default_config();
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8, // XUNTONG = index 0
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config);
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_MFR_START + 0));
    }

    #[test]
    fn sig_matches_populated_for_ble_ad_bytes() {
        let config = default_config();
        let raw_ad = [
            0x1B, 0xFF, 0x4C, 0x00, 0x12, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config);
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_AD_BYTES_START + 0)); // AirTag
    }

    // ── Rule integration tests ──────────────────────────────────────
    //
    // End-to-end: realistic scan inputs → filter_*_with_rules() → assert rule names

    #[test]
    fn rule_flock_safety_camera_via_oui() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03], // Flock Safety OUI
            ssid: "SomeNetwork",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_flock_safety_camera_via_silicon_labs_oui() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0x58, 0x8E, 0x81, 0xAA, 0xBB, 0xCC], // Silicon Labs 58:8E:81
            ssid: "",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_flock_safety_camera_via_ssid_pattern() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "Flock-A1B2C3",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_flock_safety_camera_via_ssid_exact() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "FS Ext Battery",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_flock_safety_camera_via_ssid_keyword() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "MyFlockThing",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_flock_safety_camera_via_ble_name() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Camera",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_flock_safety_camera_via_allof_mfr_and_name() {
        // The nested allOf(xuntong_mfr, flock_ble_name) branch
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Device",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8, // XUNTONG
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_flock_safety_no_match_mfr_only() {
        // xuntong_mfr alone should NOT trigger Flock Safety Camera rule
        // (allOf needs both mfr AND ble name)
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Random Device", // no "Flock" in name
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        // It matches on ble_mfr signature, but should NOT trigger the rule
        assert!(result.matched); // still a signature match
        assert!(!result.rule_names.contains(&"Flock Safety Camera"));
    }

    #[test]
    fn rule_raven_acoustic_sensor_via_gps_uuid() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS service
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Raven Acoustic Sensor"));
    }

    #[test]
    fn rule_raven_acoustic_sensor_via_error_uuid() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3500], // Raven Error service
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Raven Acoustic Sensor"));
    }

    #[test]
    fn rule_raven_no_match_wrong_uuid() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x180A], // Standard UUID — matches signature but NOT Raven rule
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched); // signature match (std UUID)
        assert!(!result.rule_names.contains(&"Raven Acoustic Sensor"));
    }

    #[test]
    fn rule_apple_airtag_matches() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let raw_ad = [
            0x1B, 0xFF, 0x4C, 0x00, 0x12, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Apple AirTag"));
    }

    #[test]
    fn rule_apple_airtag_no_match_wrong_bytes() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        // Apple manufacturer data but NOT FindMy format
        let raw_ad = [0x05, 0xFF, 0x4C, 0x00, 0x01, 0x02];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(!result.rule_names.contains(&"Apple AirTag"));
    }

    #[test]
    fn rule_flipper_zero_white_matches() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let raw_ad = [
            0x02, 0x01, 0x06, // Flags
            0x05, 0xFF, 0x80, 0x30, 0x01, 0x02,
        ];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flipper Zero"));
    }

    #[test]
    fn rule_flipper_zero_black_matches() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let raw_ad = [0x04, 0xFF, 0x81, 0x30, 0x01];
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flipper Zero"));
    }

    #[test]
    fn rule_flipper_zero_no_match_wrong_bytes() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let raw_ad = [0x04, 0xFF, 0x82, 0x30, 0x01]; // 0x82 instead of 0x80/0x81
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(!result.rule_names.contains(&"Flipper Zero"));
    }

    #[test]
    fn rule_no_rules_for_innocent_device() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03],
            name: "My Headphones",
            rssi: -50,
            service_uuids_16: &[0x180F], // Battery service
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(!result.matched);
        assert!(result.rule_names.is_empty());
    }

    #[test]
    fn rule_no_rules_for_wifi_innocent() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03],
            ssid: "Linksys-Home",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(!result.matched);
        assert!(result.rule_names.is_empty());
    }

    #[test]
    fn rule_multiple_rules_can_match_same_event() {
        // A BLE event that matches both Flock and another rule isn't realistic,
        // but let's verify the engine returns multiple rule matches.
        // We need a device with Flock BLE name AND a Raven UUID
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Device",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock Safety Camera"));
        assert!(result.rule_names.contains(&"Raven Acoustic Sensor"));
        assert_eq!(result.rule_names.len(), 2);
    }

    #[test]
    fn rule_disabled_scan_no_rules() {
        let config = FilterConfig {
            wifi_enabled: false,
            ..default_config()
        };
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            ssid: "Flock-A1B2C3",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(!result.matched);
        assert!(result.rule_names.is_empty());
    }

    #[test]
    fn rule_rssi_below_threshold_no_rules() {
        let config = FilterConfig {
            min_rssi: -40,
            ..default_config()
        };
        let db = &defaults::DEFAULT_RULE_DB;
        let input = WiFiScanInput {
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            ssid: "Flock-A1B2C3",
            rssi: -50, // below -40 threshold
        };
        let result = filter_wifi_with_rules(&input, &config, db);
        assert!(!result.matched);
        assert!(result.rule_names.is_empty());
    }

    // ── Signature index consistency tests ────────────────────────────

    #[test]
    fn sig_index_ranges_are_contiguous() {
        // Verify no gaps in the index mapping
        assert_eq!(defaults::SIG_IDX_MAC_OUI_START, 0);
        assert_eq!(
            defaults::SIG_IDX_SSID_PATTERN_START,
            defaults::SIG_IDX_MAC_OUI_START + defaults::MAC_PREFIXES.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_SSID_EXACT_START,
            defaults::SIG_IDX_SSID_PATTERN_START + defaults::SSID_PATTERNS.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_SSID_KEYWORD_START,
            defaults::SIG_IDX_SSID_EXACT_START + defaults::SSID_EXACT.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_WIFI_NAME_START,
            defaults::SIG_IDX_SSID_KEYWORD_START + defaults::SSID_KEYWORDS.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_BLE_NAME_START,
            defaults::SIG_IDX_WIFI_NAME_START + defaults::WIFI_NAME_KEYWORDS.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_BLE_UUID_START,
            defaults::SIG_IDX_BLE_NAME_START + defaults::BLE_NAME_PATTERNS.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_BLE_STD_UUID_START,
            defaults::SIG_IDX_BLE_UUID_START + defaults::BLE_SERVICE_UUIDS_16.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_BLE_MFR_START,
            defaults::SIG_IDX_BLE_STD_UUID_START + defaults::BLE_STANDARD_UUIDS_16.len() as u16
        );
        assert_eq!(
            defaults::SIG_IDX_BLE_AD_BYTES_START,
            defaults::SIG_IDX_BLE_MFR_START + defaults::BLE_MANUFACTURER_IDS.len() as u16
        );
    }

    #[test]
    fn sig_count_fits_in_bitset() {
        assert!(
            defaults::SIG_COUNT <= crate::rules::MAX_SIGNATURES,
            "SIG_COUNT {} exceeds MAX_SIGNATURES {}",
            defaults::SIG_COUNT,
            crate::rules::MAX_SIGNATURES
        );
    }

    #[test]
    fn default_rule_db_is_valid() {
        let db = &defaults::DEFAULT_RULE_DB;
        for rule in db.rules {
            let end = rule.expr_start as usize + rule.expr_len as usize;
            assert!(
                end <= db.nodes.len(),
                "Rule '{}' references nodes [{}, {}) but pool has {} nodes",
                rule.name,
                rule.expr_start,
                end,
                db.nodes.len()
            );
        }
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
