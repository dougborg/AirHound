//! Desktop-only identity. CoreBluetooth identifiers are not hardware MACs.
#[cfg(any(target_os = "linux", test))]
use airhound::scanner::OdidEvent;
use airhound::scanner::{BleEvent, WiFiEvent};

#[derive(Clone)]
pub enum ScanEvent {
    Ble(BleEvent),
    BlePeripheral {
        event: BleEvent,
        id: String,
        address: Option<[u8; 6]>,
    },
    WiFi(WiFiEvent),
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
}

pub fn write(out: &mut impl std::io::Write, message: &BleObservation<'_>) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    out.write_all(&bytes)?;
    out.flush()
}
