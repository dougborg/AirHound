//! Desktop-only identity. CoreBluetooth identifiers are not hardware MACs.
#[cfg(any(target_os = "linux", test))]
use airhound::scanner::OdidEvent;
use airhound::scanner::{BleEvent, WiFiEvent};

#[derive(Clone)]
pub enum ScanEvent {
    // Legacy synthetic inputs remain for regression fixtures; live adapters use BleSnapshot.
    #[cfg_attr(not(test), allow(dead_code))]
    Ble(BleEvent),
    BleSnapshot {
        event: BleEvent,
        id: String,
        address: Option<[u8; 6]>,
        snapshot: Box<crate::ble_payload::Snapshot>,
    },
    #[cfg_attr(not(test), allow(dead_code))]
    BlePeripheral {
        event: BleEvent,
        id: String,
        address: Option<[u8; 6]>,
    },
    WiFi(WiFiEvent),
    ExternalWiFi {
        event: crate::external_wifi::Beacon,
        generation: u64,
    },
    ExternalStatus {
        state: String,
        dropped: u64,
        producer_dropped: u64,
        reconnect_attempts: u64,
    },
    #[cfg(any(target_os = "linux", test))]
    Odid(OdidEvent),
}

/// A CoreBluetooth observation, retained even when it does not match a signature.
/// Kept separate from the firmware protocol and WiGLE's hardware-address records.
#[derive(serde::Serialize)]
pub struct BleObservation<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub peripheral_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address_source: Option<&'static str>,
    pub address_type: &'static str,
    pub identity_scope: &'static str,
    pub name: &'a str,
    pub rssi: i8,
    pub service_uuids: &'a [u16],
    pub manufacturer_id: u16,
    #[serde(rename = "match")]
    pub matches: &'a [airhound::protocol::MatchReason],
    pub ts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advertisement: Option<&'a crate::ble_payload::Snapshot>,
}

pub fn write(out: &mut impl std::io::Write, message: &BleObservation<'_>) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    out.write_all(&bytes)?;
    out.flush()
}

/// External observations retain signal uncertainty and source capture timestamps.
/// Identity matching deliberately bypasses the RSSI policy; no estimates enter
/// the core numeric-RSSI protocol or WiGLE export.
pub fn write_external(
    out: &mut impl std::io::Write,
    event: &crate::external_wifi::Beacon,
    index: &airhound::mac_index::MacIndex,
) -> std::io::Result<()> {
    let mac = crate::external_wifi::bssid(&event.bssid)
        .ok_or_else(|| std::io::Error::other("invalid external identity"))?;
    let ssid = event
        .ssid_bytes
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .unwrap_or("");
    let result = airhound::filter::filter_wifi_identity_with_rules(
        &mac,
        ssid,
        true,
        &airhound::defaults::DEFAULT_RULE_DB,
        index,
    );
    let mut value = serde_json::to_value(event).map_err(std::io::Error::other)?;
    value["type"] = serde_json::json!("external_wifi_observation");
    value["match"] = serde_json::to_value(&result.matches).map_err(std::io::Error::other)?;
    value["rules"] = serde_json::to_value(&result.rule_names).map_err(std::io::Error::other)?;
    value["filter_policy"] = serde_json::json!("identity_only_unvalidated");
    value["ssid_text_valid"] = serde_json::json!(event
        .ssid_bytes
        .as_deref()
        .is_some_and(|v| std::str::from_utf8(v).is_ok()));
    value["wigle_exported"] = serde_json::json!(false);
    write_json(out, &value)
}

/// Serialize a complete record before writing: command responses share stdout.
pub fn write_json(out: &mut impl std::io::Write, value: &serde_json::Value) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    out.write_all(&bytes)?;
    out.flush()
}
