/// Configurable filter engine for WiFi and BLE scan results.
///
/// Evaluates scan results against compiled-in defaults and runtime config.
/// Any filter match causes the result to be emitted. No scoring or state tracking —
/// that's the companion app's job.

use heapless::Vec;

use crate::defaults::{
    self, BLE_MANUFACTURER_IDS, BLE_NAME_PATTERNS, BLE_SERVICE_UUIDS_16, MAC_PREFIXES,
    SSID_EXACT, SSID_KEYWORDS, SSID_PATTERNS, WIFI_NAME_KEYWORDS,
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
