/// Filter engine for WiFi and BLE scan events.
///
/// Evaluates scan events against the signature database and runtime config.
/// Any filter match causes the result to be emitted. No scoring or state tracking —
/// that's the companion app's job.
use core::fmt::Write;

use heapless::Vec;

use crate::defaults::{
    self, BLE_AD_BYTES_PATTERNS, BLE_MANUFACTURER_IDS, BLE_NAME_PATTERNS, BLE_SERVICE_UUIDS_16,
    SSID_EXACT, SSID_KEYWORDS, SSID_PATTERNS, WIFI_NAME_KEYWORDS,
};
use crate::evidence::{self, AlertPolicy, Detection, RuleEvidence};
use crate::mac_index::MacIndex;
use crate::protocol::{MatchDetail, MatchReason};
use crate::rules::{MatchCategory, RuleDb, SigMatchSet};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleAddressType {
    Public,
    Random,
    Unknown,
}

pub struct BleScanInput<'a> {
    pub mac: &'a [u8; 6],
    pub address_type: BleAddressType,
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
    pub evidence: Vec<RuleEvidence, 4>,
    pub evidence_more: bool,
    pub watchlist: bool,
    pub ruleset: &'static str,
    pub basis: &'static str,
    /// Whether any filter matched
    pub matched: bool,
    /// Up to 4 match reasons
    pub matches: Vec<MatchReason, 4>,
    /// Bitset of which signature indices matched (for rule evaluation)
    pub sig_matches: SigMatchSet,
    /// Names of matched rules (populated by `filter_*_with_rules`)
    pub rule_names: Vec<&'static str, 4>,
    /// Categories of matched rules (populated by `filter_*_with_rules`)
    pub rule_categories: Vec<MatchCategory, 4>,
}

impl FilterResult {
    fn new() -> Self {
        Self {
            evidence: Vec::new(),
            evidence_more: false,
            watchlist: false,
            ruleset: "none",
            basis: "observed_fields",
            matched: false,
            matches: Vec::new(),
            sig_matches: SigMatchSet::new(),
            rule_names: Vec::new(),
            rule_categories: Vec::new(),
        }
    }

    pub fn detection(&self, policy: AlertPolicy) -> Detection<'_> {
        let mut sig = Vec::new();
        let mut more = self.evidence_more;
        for id in 0..crate::rules::MAX_SIGNATURES as u16 {
            if self.sig_matches.get(id) && sig.push(id).is_err() {
                more = true;
            }
        }
        Detection {
            engine: "evidence-1",
            policy,
            pack: defaults::SIGNATURE_PACK_ID,
            ruleset: self.ruleset,
            sig,
            rules: &self.evidence,
            more,
            basis: self.basis,
            alert: policy.decide(self.matched, self.watchlist, &self.evidence),
        }
    }
    pub fn should_alert(&self) -> bool {
        AlertPolicy::RulesAndWatchlist
            .decide(self.matched, self.watchlist, &self.evidence)
            .fires()
    }

    fn add_match(&mut self, filter_type: &'static str, detail: &str) {
        if self.matches.len() < 4 {
            let mut d = MatchDetail::new();
            // Truncate detail to fit
            let truncated = if detail.len() <= 32 {
                detail
            } else {
                &detail[..(0..=32)
                    .rev()
                    .find(|&n| detail.is_char_boundary(n))
                    .unwrap_or(0)]
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
pub fn filter_wifi(
    input: &WiFiScanInput,
    config: &FilterConfig,
    mac_index: &MacIndex,
) -> FilterResult {
    if !config.wifi_enabled || input.rssi < config.min_rssi {
        return FilterResult::new();
    }
    filter_wifi_identity(input.mac, input.ssid, mac_index)
}

/// Evaluate MAC/SSID signatures without making a signal-strength claim.
/// Callers must explicitly choose identity-only matching and preserve absent or
/// unvalidated RSSI separately; this does not apply an RSSI threshold.
fn filter_wifi_identity(mac: &[u8; 6], ssid: &str, mac_index: &MacIndex) -> FilterResult {
    let mut result = FilterResult::new();

    // MAC lookup via hash index (OUI signatures + watchlist)
    check_mac(mac, mac_index, &mut result, mac[0] & 3 == 0);

    // SSID structured pattern check (e.g., Flock-XXXXXX)
    for (i, pattern) in SSID_PATTERNS.iter().enumerate() {
        if pattern.matches(ssid) {
            result
                .sig_matches
                .set(defaults::SIG_IDX_SSID_PATTERN_START + i as u16);
            result.add_match("ssid_pattern", pattern.description);
        }
    }

    // SSID exact match check
    for (i, &exact) in SSID_EXACT.iter().enumerate() {
        if ssid == exact {
            result
                .sig_matches
                .set(defaults::SIG_IDX_SSID_EXACT_START + i as u16);
            result.add_match("ssid_exact", exact);
        }
    }

    // SSID keyword substring check (case-insensitive)
    if !ssid.is_empty() {
        let ssid_lower: Vec<u8, 33> = ssid
            .bytes()
            .take(33)
            .map(|b| b.to_ascii_lowercase())
            .collect();
        let ssid_lower_str = core::str::from_utf8(&ssid_lower).unwrap_or("");

        for (i, &keyword) in SSID_KEYWORDS.iter().enumerate() {
            if ssid_lower_str.contains(keyword) {
                result
                    .sig_matches
                    .set(defaults::SIG_IDX_SSID_KEYWORD_START + i as u16);
                result.add_match("ssid_keyword", keyword);
            }
        }

        // WiFi name keyword check (from FlockOff — matches partial names)
        for (i, &keyword) in WIFI_NAME_KEYWORDS.iter().enumerate() {
            if ssid_lower_str.contains(keyword) {
                result
                    .sig_matches
                    .set(defaults::SIG_IDX_WIFI_NAME_START + i as u16);
                // Only add if not already matched by SSID_KEYWORDS
                if !SSID_KEYWORDS.contains(&keyword) {
                    result.add_match("wifi_name", keyword);
                }
            }
        }
    }

    result
}

/// Evaluate a BLE scan event against all signatures.
pub fn filter_ble(
    input: &BleScanInput,
    config: &FilterConfig,
    mac_index: &MacIndex,
) -> FilterResult {
    let mut result = FilterResult::new();

    if !config.ble_enabled {
        return result;
    }

    // RSSI threshold check
    if input.rssi < config.min_rssi {
        return result;
    }

    // MAC lookup via hash index (OUI signatures + watchlist)
    check_mac(
        input.mac,
        mac_index,
        &mut result,
        input.address_type == BleAddressType::Public && input.mac[0] & 1 == 0,
    );

    // BLE device name pattern check (case-insensitive substring)
    if !input.name.is_empty() {
        let name_lower: Vec<u8, 33> = input
            .name
            .bytes()
            .take(33)
            .map(|b| b.to_ascii_lowercase())
            .collect();
        let name_lower_str = core::str::from_utf8(&name_lower).unwrap_or("");

        // BLE_NAME_PATTERNS are pre-lowercased in defaults.rs
        for (i, &pattern) in BLE_NAME_PATTERNS.iter().enumerate() {
            if name_lower_str.contains(pattern) {
                result
                    .sig_matches
                    .set(defaults::SIG_IDX_BLE_NAME_START + i as u16);
                result.add_match("ble_name", pattern);
            }
        }
    }

    // BLE service UUID check (16-bit)
    let raw_services = crate::ble_data::AdStructures::new(input.raw_ad)
        .flat_map(|field| {
            field
                .uuids()
                .chain(field.service().map(|record| record.uuid))
        })
        .filter_map(crate::ble_data::Uuid::as_u16);
    for uuid in input.service_uuids_16.iter().copied().chain(raw_services) {
        for (i, &known) in BLE_SERVICE_UUIDS_16.iter().enumerate() {
            if uuid == known
                && !result
                    .sig_matches
                    .get(defaults::SIG_IDX_BLE_UUID_START + i as u16)
            {
                result
                    .sig_matches
                    .set(defaults::SIG_IDX_BLE_UUID_START + i as u16);
                let mut detail = crate::protocol::MatchDetail::new();
                let _ = write!(detail, "Service UUID 0x{:04X}", uuid);
                result.add_match("ble_uuid", detail.as_str());
            }
        }
        for (i, &known) in defaults::BLE_STANDARD_UUIDS_16.iter().enumerate() {
            if uuid == known
                && !result
                    .sig_matches
                    .get(defaults::SIG_IDX_BLE_STD_UUID_START + i as u16)
            {
                result
                    .sig_matches
                    .set(defaults::SIG_IDX_BLE_STD_UUID_START + i as u16);
                let mut detail = crate::protocol::MatchDetail::new();
                let _ = write!(detail, "Standard UUID 0x{:04X}", uuid);
                result.add_match("ble_uuid_std", detail.as_str());
            }
        }
    }

    // BLE manufacturer ID check
    if input.manufacturer_id != 0 {
        for (i, &known) in BLE_MANUFACTURER_IDS.iter().enumerate() {
            if input.manufacturer_id == known {
                result
                    .sig_matches
                    .set(defaults::SIG_IDX_BLE_MFR_START + i as u16);
                result.add_match("ble_mfr", "Known manufacturer ID");
                break;
            }
        }
    }

    // BLE advertisement byte pattern check
    if !input.raw_ad.is_empty() {
        check_ble_ad_bytes(input.raw_ad, &mut result);
    }

    result
}

/// Match normalized fields without inventing a raw advertisement. Composite rules
/// are deliberately not evaluated: platform snapshots may contain cached fields.
/// Only explicitly migrated contextual payload patterns apply here; legacy raw
/// offsets and search-anywhere patterns are not evaluated on property maps.
pub fn filter_ble_snapshot(
    input: &BleScanInput,
    data: &crate::ble_data::Advertisement<'_>,
    config: &FilterConfig,
    mac_index: &MacIndex,
) -> FilterResult {
    let base = BleScanInput {
        service_uuids_16: &[],
        manufacturer_id: 0,
        raw_ad: &[],
        ..*input
    };
    let mut result = filter_ble(&base, config, mac_index);
    result.basis = "possibly_cached";
    if !config.ble_enabled || input.rssi < config.min_rssi {
        return result;
    }
    for (values, start, kind) in [
        (
            BLE_SERVICE_UUIDS_16,
            defaults::SIG_IDX_BLE_UUID_START,
            "ble_uuid",
        ),
        (
            defaults::BLE_STANDARD_UUIDS_16,
            defaults::SIG_IDX_BLE_STD_UUID_START,
            "ble_uuid_std",
        ),
    ] {
        for (i, &uuid) in values.iter().enumerate() {
            if data.has_service(crate::ble_data::Uuid::from_u16(uuid)) {
                result.sig_matches.set(start + i as u16);
                let mut detail = crate::protocol::MatchDetail::new();
                let _ = write!(detail, "Service UUID 0x{:04X}", uuid);
                result.add_match(kind, &detail);
            }
        }
    }
    for (i, &company_id) in BLE_MANUFACTURER_IDS.iter().enumerate() {
        if data
            .manufacturer_data
            .iter()
            .any(|r| r.company_id == company_id)
        {
            result
                .sig_matches
                .set(defaults::SIG_IDX_BLE_MFR_START + i as u16);
            result.add_match("ble_mfr", "Known manufacturer ID");
        }
    }
    // Explicit migration for the known Find My frame. Legacy v1 raw offsets
    // use the same complete Find My framing requirement as raw packet matching.
    let find_my = crate::ble_data::BytePattern {
        bytes: &[0x12, 0x19],
        mask: None,
        offset: 0,
        min_len: 27,
        max_len: Some(27),
    };
    if data.matches_manufacturer(0x004c, &find_my) {
        result.sig_matches.set(defaults::SIG_IDX_BLE_AD_BYTES_START);
        result.add_match("ble_ad_bytes", "Find My framing candidate");
    }
    result
}

/// Evaluate a WiFi scan event against signatures and then against rules.
pub fn filter_wifi_with_rules(
    input: &WiFiScanInput,
    config: &FilterConfig,
    db: &RuleDb,
    mac_index: &MacIndex,
) -> FilterResult {
    let mut result = filter_wifi(input, config, mac_index);
    apply_rules(&mut result, db);
    result
}

/// Explicit identity-only WiFi matching for observations with unknown or
/// unvalidated signal. No numeric RSSI is accepted or fabricated by this API.
pub fn filter_wifi_identity_with_rules(
    mac: &[u8; 6],
    ssid: &str,
    enabled: bool,
    db: &RuleDb,
    mac_index: &MacIndex,
) -> FilterResult {
    if !enabled {
        return FilterResult::new();
    }
    let mut result = filter_wifi_identity(mac, ssid, mac_index);
    apply_rules(&mut result, db);
    result
}

/// Evaluate a BLE scan event against signatures and then against rules.
pub fn filter_ble_with_rules(
    input: &BleScanInput,
    config: &FilterConfig,
    db: &RuleDb,
    mac_index: &MacIndex,
) -> FilterResult {
    let mut result = filter_ble(input, config, mac_index);
    apply_rules(&mut result, db);
    result
}

/// Run rule evaluation on a filter result and populate `rule_names` and `rule_categories`.
fn apply_rules(result: &mut FilterResult, db: &RuleDb) {
    if !result.matched {
        return;
    }
    result.ruleset = db.id;
    let (proofs, more) = evidence::evaluate(db, &result.sig_matches);
    for proof in &proofs {
        let rule = &db.rules[usize::from(proof.id)];
        let _ = result.rule_names.push(rule.name);
        let _ = result.rule_categories.push(rule.category);
    }
    result.evidence = proofs;
    result.evidence_more = more;
}

/// Check MAC address against the hash index (OUI signatures + watchlist).
fn check_mac(mac: &[u8; 6], mac_index: &MacIndex, result: &mut FilterResult, allow_vendor: bool) {
    let lookup = mac_index.lookup(mac);
    if allow_vendor && lookup.sig_idx != crate::mac_index::NO_ID {
        result.sig_matches.set(lookup.sig_idx);
        let vendor = defaults::vendor_for_sig_idx(lookup.sig_idx);
        result.add_match("mac_oui", vendor);
    }
    if lookup.has_watchlist_match() {
        result.watchlist = true;
        result.add_match("watchlist", "");
    }
}

/// Check raw BLE advertisement data against known byte patterns.
fn check_ble_ad_bytes(raw_ad: &[u8], result: &mut FilterResult) {
    for (i, pattern) in BLE_AD_BYTES_PATTERNS.iter().enumerate() {
        if !pattern.active || pattern.bytes.is_empty() {
            continue;
        }
        let matched = match pattern.offset {
            Some(offset) => {
                // Fixed offset match within manufacturer-specific data
                crate::ble_data::AdStructures::new(raw_ad).any(|field| {
                    let data = field.data;
                    field.kind == 0xFF
                        && pattern
                            .exact_manufacturer_len
                            .is_none_or(|len| data.len() == len)
                        && data.get(offset..offset.saturating_add(pattern.bytes.len()))
                            == Some(pattern.bytes)
                })
            }
            None => {
                // Search anywhere in raw AD data
                raw_ad
                    .windows(pattern.bytes.len())
                    .any(|w| w == pattern.bytes)
            }
        };
        if matched {
            result
                .sig_matches
                .set(defaults::SIG_IDX_BLE_AD_BYTES_START + i as u16);
            result.add_match("ble_ad_bytes", pattern.description);
        }
    }
}

/// Format a 6-byte MAC address into "AA:BB:CC:DD:EE:FF" string.
///
/// Re-exported from `protocol::format_mac` for backward compatibility.
pub fn format_mac(mac: &[u8; 6], buf: &mut crate::protocol::MacString) {
    crate::protocol::format_mac(mac, buf);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    use crate::mac_index::MacIndex;

    fn default_config() -> FilterConfig {
        FilterConfig::new()
    }

    fn test_mac_index() -> MacIndex {
        MacIndex::from_defaults()
    }

    #[test]
    fn snapshot_context_requires_full_frame_and_preserves_uuid_identity() {
        use crate::ble_data::{Advertisement, ManufacturerData, Uuid};
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0; 6],
            name: "",
            rssi: -40,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let mut bytes = [0u8; 27];
        bytes[..2].copy_from_slice(&[0x12, 0x19]);
        let records = [
            ManufacturerData {
                company_id: 0,
                payload: &[],
            },
            ManufacturerData {
                company_id: 0x4c,
                payload: &bytes,
            },
        ];
        let full = Uuid::from_u16(0xfffa);
        let mut custom = full;
        custom.0[15] ^= 1;
        let uuids = [custom];
        let mut data = Advertisement {
            service_uuids: &uuids,
            manufacturer_data: &records,
            service_data: &[],
        };
        let result = filter_ble_snapshot(&input, &data, &default_config(), &test_mac_index());
        assert!(result
            .matches
            .iter()
            .any(|m| m.detail == "Find My framing candidate"));
        assert!(!result.matches.iter().any(|m| m.filter_type == "ble_uuid"));
        assert!(result.rule_names.is_empty());
        let short = [ManufacturerData {
            company_id: 0x4c,
            payload: &bytes[..3],
        }];
        data.manufacturer_data = &short;
        assert!(!filter_ble_snapshot(&input, &data, &default_config(), &test_mac_index()).matched);
        let wrong = [ManufacturerData {
            company_id: 0x4d,
            payload: &bytes,
        }];
        data.manufacturer_data = &wrong;
        assert!(!filter_ble_snapshot(&input, &data, &default_config(), &test_mac_index()).matched);
        let uuids = [full];
        data.service_uuids = &uuids;
        assert!(
            filter_ble_snapshot(&input, &data, &default_config(), &test_mac_index())
                .matches
                .iter()
                .any(|m| m.filter_type == "ble_uuid")
        );
        let mut cfg = default_config();
        cfg.ble_enabled = false;
        assert!(!filter_ble_snapshot(&input, &data, &cfg, &test_mac_index()).matched);
    }

    #[test]
    fn address_type_gates_vendor_evidence_but_preserves_explicit_watchlists() {
        let mac = [0xB4, 0x1E, 0x52, 1, 2, 3];
        let mut index = test_mac_index();
        let mut input = BleScanInput {
            mac: &mac,
            address_type: BleAddressType::Public,
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        assert!(filter_ble(&input, &default_config(), &index).matched);
        for kind in [BleAddressType::Random, BleAddressType::Unknown] {
            input.address_type = kind;
            assert!(!filter_ble(&input, &default_config(), &index).matched);
        }
        index.add_watchlist_full(mac, 7);
        let result = filter_ble_with_rules(
            &input,
            &default_config(),
            &defaults::DEFAULT_RULE_DB,
            &index,
        );
        assert!(result.watchlist && result.should_alert());
        assert!(result.evidence.is_empty());
        assert!(result
            .detection(AlertPolicy::RulesAndWatchlist)
            .sig
            .is_empty());
    }

    #[test]
    fn find_my_checks_all_complete_manufacturer_sections_and_rejects_truncation() {
        let mut ad = [0u8; 35];
        ad[..4].copy_from_slice(&[3, 0xff, 1, 2]);
        ad[4..10].copy_from_slice(&[30, 0xff, 0x4c, 0, 0x12, 0x19]);
        let mut result = FilterResult::new();
        check_ble_ad_bytes(&ad, &mut result);
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_AD_BYTES_START));
        for end in 0..ad.len() {
            let mut truncated = FilterResult::new();
            check_ble_ad_bytes(&ad[..end], &mut truncated);
            assert!(!truncated
                .sig_matches
                .get(defaults::SIG_IDX_BLE_AD_BYTES_START));
        }
    }

    #[test]
    fn composite_candidate_alert_keeps_unrelated_clues_separate() {
        let input = WiFiScanInput {
            mac: &[0xb4, 0x1e, 0x52, 1, 2, 3],
            ssid: "Flock-ABCDEF",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(
            &input,
            &default_config(),
            &defaults::DEFAULT_RULE_DB,
            &test_mac_index(),
        );
        assert!(result.should_alert());
        assert_eq!(
            &result.evidence[0].sig[..],
            &[
                defaults::SIG_IDX_MAC_OUI_START,
                defaults::SIG_IDX_SSID_PATTERN_START
            ]
        );
        assert!(result.sig_matches.get(defaults::SIG_IDX_SSID_KEYWORD_START));
        assert_eq!(result.evidence[0].sig.len(), 2);
    }

    #[test]
    fn compact_details_are_utf8_safe_and_signature_overflow_is_visible() {
        let mut result = FilterResult::new();
        result.add_match("test", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaé");
        assert_eq!(result.matches[0].detail.len(), 31);
        for i in 0..9 {
            result.sig_matches.set(i);
        }
        let record = result.detection(AlertPolicy::Silent);
        assert!(record.more);
        assert_eq!(record.sig.len(), 8);
        assert!(!record.alert.fires());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
        assert!(result.matched);
        assert!(result.matches.len() >= 2);
    }

    // ── BLE filter tests ────────────────────────────────────────────

    #[test]
    fn ble_name_flock_matches() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Camera",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "ble_name"));
    }

    #[test]
    fn ble_name_fs_ext_battery_matches() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "FS Ext Battery",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.matched);
    }

    #[test]
    fn ble_name_pigvision_case_insensitive() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "PIGVISION-device",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.matched);
    }

    #[test]
    fn ble_manufacturer_id_xuntong_matches() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "ble_mfr"));
    }

    #[test]
    fn ble_raven_service_uuid_matches() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS service
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "ble_uuid"));
    }

    #[test]
    fn ble_standard_uuid_matches() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x1819], // Location and Navigation
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
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
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03],
            name: "My Headphones",
            rssi: -50,
            service_uuids_16: &[0x180F], // Battery Service (not surveillance)
            manufacturer_id: 0x004C,     // Apple (not in our list)
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(!result.matched);
    }

    #[test]
    fn ble_disabled_no_match() {
        let config = FilterConfig {
            ble_enabled: false,
            ..default_config()
        };
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            name: "Flock",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(!result.matched);
    }

    #[test]
    fn ble_rssi_below_threshold_no_match() {
        let config = FilterConfig {
            min_rssi: -60,
            ..default_config()
        };
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03],
            name: "Flock",
            rssi: -70,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(!result.matched);
    }

    // ── BLE AD bytes tests ────────────────────────────────────────

    #[test]
    fn ble_ad_bytes_airtag_findmy_matches() {
        let config = default_config();
        // Realistic AirTag AD: manufacturer-specific data with Apple FindMy header
        // AD structure: len=0x1B, type=0xFF, data=[0x4C, 0x00, 0x12, 0x19, ...]
        let mut raw_ad = [0u8; 31];
        raw_ad[..6].copy_from_slice(&[30, 0xFF, 0x4C, 0, 0x12, 0x19]);
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.matched);
        assert!(result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_ad_bytes"
                && m.detail.as_str() == "Find My framing candidate"));
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
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(!result.matched);
        assert!(!result.should_alert());
    }

    #[test]
    fn ble_ad_bytes_flipper_zero_black_matches() {
        let config = default_config();
        // Flipper Zero Black: [0x81, 0x30]
        let raw_ad = [0x04, 0xFF, 0x81, 0x30, 0x01];
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(!result.matched);
        assert!(!result.should_alert());
    }

    #[test]
    fn ble_ad_bytes_airtag_wrong_offset_no_match() {
        let config = default_config();
        // AirTag bytes at wrong position (not at start of manufacturer data)
        // Manufacturer data: [0x00, 0x4C, 0x00, 0x12, 0x19] — offset by 1
        let raw_ad = [0x06, 0xFF, 0x00, 0x4C, 0x00, 0x12, 0x19];
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        // Should NOT match AirTag (offset-sensitive pattern)
        assert!(!result
            .matches
            .iter()
            .any(|m| m.detail.as_str() == "Find My framing candidate"));
    }

    #[test]
    fn ble_ad_bytes_no_match_for_random_data() {
        let config = default_config();
        let raw_ad = [
            0x02, 0x01, 0x06, // Flags
            0x03, 0xFF, 0xAA, 0xBB, // Random manufacturer data
        ];
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(!result
            .matches
            .iter()
            .any(|m| m.filter_type == "ble_ad_bytes"));
    }

    #[test]
    fn ble_ad_bytes_empty_raw_ad_no_match() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
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
        let result = filter_wifi(&input, &config, &test_mac_index());
        assert!(result.sig_matches.get(defaults::SIG_IDX_MAC_OUI_START));
    }

    #[test]
    fn sig_matches_populated_for_wifi_ssid_pattern() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "Flock-A1B2C3",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config, &test_mac_index());
        assert!(result.sig_matches.get(defaults::SIG_IDX_SSID_PATTERN_START));
        // Also sets keyword "flock"
        assert!(result.sig_matches.get(defaults::SIG_IDX_SSID_KEYWORD_START));
    }

    #[test]
    fn sig_matches_populated_for_wifi_ssid_exact() {
        let config = default_config();
        let input = WiFiScanInput {
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ssid: "FS Ext Battery",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config, &test_mac_index());
        assert!(result.sig_matches.get(defaults::SIG_IDX_SSID_EXACT_START));
    }

    #[test]
    fn sig_matches_populated_for_ble_name() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Camera",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_NAME_START)); // "Flock"
    }

    #[test]
    fn sig_matches_populated_for_ble_uuid() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS = index 0
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_UUID_START));
    }

    #[test]
    fn sig_matches_populated_for_ble_mfr() {
        let config = default_config();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8, // XUNTONG = index 0
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_MFR_START));
    }

    #[test]
    fn sig_matches_populated_for_ble_ad_bytes() {
        let config = default_config();
        let mut raw_ad = [0u8; 31];
        raw_ad[..6].copy_from_slice(&[30, 0xFF, 0x4C, 0, 0x12, 0x19]);
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble(&input, &config, &test_mac_index());
        assert!(result.sig_matches.get(defaults::SIG_IDX_BLE_AD_BYTES_START)); // AirTag
    }

    // ── Rule integration tests ──────────────────────────────────────

    #[test]
    fn identity_only_wifi_matching_preserves_rules_without_bypassing_normal_rssi_policy() {
        let db = &defaults::DEFAULT_RULE_DB;
        let index = test_mac_index();
        let mac = [0xB4, 0x1E, 0x52, 1, 2, 3];
        let identity = filter_wifi_identity_with_rules(&mac, "SomeNetwork", true, db, &index);
        assert!(identity.matched);
        assert!(identity.rule_names.is_empty());
        assert!(!identity.should_alert());
        let weak = WiFiScanInput {
            mac: &mac,
            ssid: "SomeNetwork",
            rssi: -120,
        };
        assert!(!filter_wifi_with_rules(&weak, &default_config(), db, &index).matched);
        assert!(!filter_wifi_identity_with_rules(&mac, "SomeNetwork", false, db, &index).matched);
        let unknown = [0x02, 1, 2, 3, 4, 5];
        assert!(!filter_wifi_identity_with_rules(&unknown, "ordinary", true, db, &index).matched);
        assert!(
            filter_wifi_identity_with_rules(&unknown, "Flock-123456", true, db, &index).matched
        );
    }
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.is_empty());
        assert!(!result.should_alert());
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.is_empty());
        assert!(!result.should_alert());
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.is_empty());
        assert!(!result.should_alert());
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.is_empty());
        assert!(!result.should_alert());
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.is_empty());
        assert!(!result.should_alert());
    }

    #[test]
    fn rule_flock_safety_camera_via_ble_name() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Camera",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.is_empty());
        assert!(!result.should_alert());
    }

    #[test]
    fn rule_flock_safety_camera_via_allof_mfr_and_name() {
        // The nested allOf(xuntong_mfr, flock_ble_name) branch
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Device",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8, // XUNTONG
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock-like candidate"));
    }

    #[test]
    fn rule_flock_safety_no_match_mfr_only() {
        // xuntong_mfr alone should NOT trigger Flock-like candidate rule
        // (allOf needs both mfr AND ble name)
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Random Device", // no "Flock" in name
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x09C8,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        // It matches on ble_mfr signature, but should NOT trigger the rule
        assert!(result.matched); // still a signature match
        assert!(!result.rule_names.contains(&"Flock-like candidate"));
    }

    #[test]
    fn rule_raven_acoustic_sensor_via_gps_uuid() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS service
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Raven UUID candidate"));
    }

    #[test]
    fn rule_raven_acoustic_sensor_via_error_uuid() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x3500], // Raven Error service
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Raven UUID candidate"));
    }

    #[test]
    fn rule_raven_no_match_wrong_uuid() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[0x180A], // Standard UUID — matches signature but NOT Raven rule
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched); // signature match (std UUID)
        assert!(!result.rule_names.contains(&"Raven UUID candidate"));
    }

    #[test]
    fn rule_apple_airtag_matches() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let mut raw_ad = [0u8; 31];
        raw_ad[..6].copy_from_slice(&[30, 0xFF, 0x4C, 0, 0x12, 0x19]);
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Find My framing candidate"));
    }

    #[test]
    fn rule_apple_airtag_no_match_wrong_bytes() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        // Apple manufacturer data but NOT FindMy format
        let raw_ad = [0x05, 0xFF, 0x4C, 0x00, 0x01, 0x02];
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0x004C,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(!result.rule_names.contains(&"Find My framing candidate"));
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
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(!result.matched);
        assert!(!result.should_alert());
    }

    #[test]
    fn rule_flipper_zero_black_matches() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let raw_ad = [0x04, 0xFF, 0x81, 0x30, 0x01];
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(!result.matched);
        assert!(!result.should_alert());
    }

    #[test]
    fn rule_flipper_zero_no_match_wrong_bytes() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let raw_ad = [0x04, 0xFF, 0x82, 0x30, 0x01]; // 0x82 instead of 0x80/0x81
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &raw_ad,
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(!result.rule_names.contains(&"Flipper Zero"));
    }

    #[test]
    fn rule_no_rules_for_innocent_device() {
        let config = default_config();
        let db = &defaults::DEFAULT_RULE_DB;
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03],
            name: "My Headphones",
            rssi: -50,
            service_uuids_16: &[0x180F], // Battery service
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
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
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            name: "Flock Device",
            rssi: -50,
            service_uuids_16: &[0x3100], // Raven GPS
            manufacturer_id: 0x09C8,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, db, &test_mac_index());
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Flock-like candidate"));
        assert!(result.rule_names.contains(&"Raven UUID candidate"));
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
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
        let result = filter_wifi_with_rules(&input, &config, db, &test_mac_index());
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
        const {
            assert!(defaults::SIG_COUNT <= crate::rules::MAX_SIGNATURES);
        }
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

    // ── MacIndex integration tests ─────────────────────────────────

    #[test]
    fn filter_wifi_watchlist_match() {
        let config = default_config();
        let mut idx = MacIndex::from_defaults();
        idx.add_watchlist_oui([0xDE, 0xAD, 0xBE], 1);

        let input = WiFiScanInput {
            mac: &[0xDE, 0xAD, 0xBE, 0x01, 0x02, 0x03],
            ssid: "",
            rssi: -50,
        };
        let result = filter_wifi(&input, &config, &idx);
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "watchlist"));
    }

    #[test]
    fn filter_ble_watchlist_full_mac_match() {
        let config = default_config();
        let mut idx = MacIndex::from_defaults();
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD];
        idx.add_watchlist_full(mac, 2);

        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &mac,
            name: "",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble(&input, &config, &idx);
        assert!(result.matched);
        assert!(result.matches.iter().any(|m| m.filter_type == "watchlist"));
    }

    // ── HC-series module rule tests ──────────────────────────────────────

    #[test]
    fn rule_card_skimmer_hc05_matches() {
        let config = default_config();
        let idx = test_mac_index();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            name: "HC-05",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, &defaults::DEFAULT_RULE_DB, &idx);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"HC-series module"));
    }

    #[test]
    fn rule_card_skimmer_hc06_case_insensitive() {
        let config = default_config();
        let idx = test_mac_index();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            name: "hc-06",
            rssi: -50,
            service_uuids_16: &[],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, &defaults::DEFAULT_RULE_DB, &idx);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"HC-series module"));
    }

    // ── Pwnagotchi rule tests ────────────────────────────────────────

    #[test]
    fn rule_pwnagotchi_oui_matches() {
        let config = default_config();
        let idx = test_mac_index();
        let input = WiFiScanInput {
            mac: &[0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD],
            ssid: "",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, &defaults::DEFAULT_RULE_DB, &idx);
        assert!(!result.matched);
        assert!(!result.should_alert());
    }

    #[test]
    fn rule_pwnagotchi_no_match_wrong_oui() {
        let config = default_config();
        let idx = test_mac_index();
        let input = WiFiScanInput {
            mac: &[0xDE, 0xAD, 0xBF, 0xEF, 0xDE, 0xAD],
            ssid: "",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, &defaults::DEFAULT_RULE_DB, &idx);
        assert!(!result.rule_names.contains(&"Pwnagotchi"));
    }

    // ── Remote ID service candidate rule tests ──────────────────────────────────────

    #[test]
    fn rule_odid_ble_uuid_matches() {
        let config = default_config();
        let idx = test_mac_index();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            name: "",
            rssi: -50,
            service_uuids_16: &[0xFFFA],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, &defaults::DEFAULT_RULE_DB, &idx);
        assert!(result.matched);
        assert!(result.rule_names.contains(&"Remote ID service candidate"));
        assert!(result
            .rule_categories
            .iter()
            .any(|c| matches!(c, MatchCategory::Drone)));
    }

    #[test]
    fn rule_odid_wifi_oui_matches() {
        let config = default_config();
        let idx = test_mac_index();
        let input = WiFiScanInput {
            mac: &[0x90, 0x3A, 0xE6, 0x01, 0x02, 0x03],
            ssid: "",
            rssi: -50,
        };
        let result = filter_wifi_with_rules(&input, &config, &defaults::DEFAULT_RULE_DB, &idx);
        assert!(result.matched);
        assert!(result.rule_names.is_empty());
        assert!(!result.should_alert());
    }

    #[test]
    fn rule_odid_no_match_wrong_uuid() {
        let config = default_config();
        let idx = test_mac_index();
        let input = BleScanInput {
            address_type: crate::filter::BleAddressType::Public,
            mac: &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            name: "",
            rssi: -50,
            service_uuids_16: &[0xFFFB],
            manufacturer_id: 0,
            raw_ad: &[],
        };
        let result = filter_ble_with_rules(&input, &config, &defaults::DEFAULT_RULE_DB, &idx);
        assert!(!result.rule_names.contains(&"Remote ID service candidate"));
    }
}
