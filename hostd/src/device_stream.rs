//! Bounded ingestion of the existing AirHound firmware NDJSON contract.
//! Device uptime and producer evidence are never presented as host capture time
//! or attached to another sighting by a non-unique (address, uptime) pair.
use airhound::{
    defaults,
    filter::{self, BleAddressType, BleScanInput, FilterConfig, WiFiScanInput},
    mac_index::MacIndex,
};
use serde_json::{json, Value};
use std::io;

pub const MAX_LINE: usize = 4096;

#[derive(Clone, Debug)]
pub struct Record {
    pub value: Value,
    pub kind: Kind,
    pub connection_generation: u64,
    pub boot_generation: u64,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Wifi {
        mac: [u8; 6],
        ssid: String,
        rssi: i8,
    },
    Ble {
        mac: [u8; 6],
        name: String,
        rssi: i8,
        manufacturer: u16,
        uuid16: Option<u16>,
    },
    Status {
        board: String,
        version: String,
        uptime: u32,
        scanning: bool,
    },
    Stats,
    Evidence,
}

impl Record {
    pub fn observation(&self) -> bool {
        matches!(self.kind, Kind::Wifi { .. } | Kind::Ble { .. })
    }

    pub fn output(&self, config: &FilterConfig, index: &MacIndex) -> Value {
        let mut value = self.value.clone();
        value["producer_record"] = self.value.clone();
        value["connection_generation"] = json!(self.connection_generation);
        value["boot_generation"] = json!(self.boot_generation);
        value["transport"] = json!("airhound_serial");
        value["timestamp_basis"] = json!("device_uptime_ms_not_utc");
        value["wigle_exported"] = json!(false);
        // Producer hints are retained inside producer_record, separately from
        // host evaluation with the current signature pack and policy.
        value.as_object_mut().unwrap().remove("match");
        value.as_object_mut().unwrap().remove("rule");
        match &self.kind {
            Kind::Wifi { mac, ssid, rssi } => {
                let result = filter::filter_wifi_with_rules(
                    &WiFiScanInput {
                        mac,
                        ssid,
                        rssi: *rssi,
                    },
                    config,
                    &defaults::DEFAULT_RULE_DB,
                    index,
                );
                value["type"] = json!("wifi_observation");
                value["detection"] = serde_json::to_value(result.detection(config.alert_policy))
                    .expect("serializable detection");
                value["frequency_mhz"] = Value::Null;
                value["band"] = Value::Null;
                value["security"] = Value::Null;
            }
            Kind::Ble {
                mac,
                name,
                rssi,
                manufacturer,
                uuid16,
            } => {
                let uuids: Vec<u16> = uuid16.iter().copied().collect();
                let result = filter::filter_ble_with_rules(
                    &BleScanInput {
                        address_type: BleAddressType::Unknown,
                        mac,
                        name,
                        rssi: *rssi,
                        service_uuids_16: &uuids,
                        manufacturer_id: *manufacturer,
                        raw_ad: &[],
                    },
                    config,
                    &defaults::DEFAULT_RULE_DB,
                    index,
                );
                value["type"] = json!("ble_observation");
                value["address_type"] = json!("unknown");
                value["identity_scope"] = json!("device_reported_ble_address");
                value["payload_completeness"] = json!("firmware_summary_only");
                value["detection"] = serde_json::to_value(result.detection(config.alert_policy))
                    .expect("serializable detection");
            }
            Kind::Status { .. } => {
                value["type"] = json!("airhound_device_status");
                value["timestamp_basis"] = json!("device_uptime_seconds_not_utc");
            }
            Kind::Stats => {
                value["type"] = json!("airhound_device_stream_stats");
                value["counter_scope"] = json!("producer_since_boot");
                value["timestamp_basis"] = json!("not_reported");
            }
            Kind::Evidence => {
                value["type"] = json!("airhound_device_detection");
                value["association"] = json!("unassociated_producer_evidence");
            }
        }
        value
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid AirHound firmware record",
    )
}
fn text<'a>(v: &'a Value, key: &str, max: usize) -> io::Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| s.len() <= max)
        .ok_or_else(invalid)
}
fn uint(v: &Value, key: &str, max: u64) -> io::Result<u64> {
    v[key].as_u64().filter(|n| *n <= max).ok_or_else(invalid)
}
fn address(v: &Value) -> io::Result<[u8; 6]> {
    let s = text(v, "mac", 17)?;
    let mut bytes = [0; 6];
    let mut parts = s.split(':');
    for b in &mut bytes {
        let part = parts.next().filter(|s| s.len() == 2).ok_or_else(invalid)?;
        *b = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
    }
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok(bytes)
}

/// Unknown records/logs are ignored. Recognized but malformed records are errors.
pub fn parse(line: &[u8]) -> io::Result<Option<Record>> {
    if line.len() > MAX_LINE {
        return Err(invalid());
    }
    // Strip ANSI terminal codes used by firmware log prefixes. Keep UTF-8 bytes.
    let mut clean = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] == 0x1b && line.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < line.len() && !(0x40..=0x7e).contains(&line[i]) {
                i += 1;
            }
            i += usize::from(i < line.len());
        } else {
            clean.push(line[i]);
            i += 1;
        }
    }
    let Some(start) = clean.iter().position(|b| *b == b'{') else {
        return Ok(None);
    };
    if start > 128 {
        return Err(invalid());
    }
    let value: Value = serde_json::from_slice(&clean[start..]).map_err(|_| invalid())?;
    let kind = match value["type"].as_str() {
        Some("wifi" | "ble") => {
            let mac = address(&value)?;
            let rssi = value["rssi"]
                .as_i64()
                .and_then(|n| i8::try_from(n).ok())
                .ok_or_else(invalid)?;
            uint(&value, "ts", u32::MAX.into())?;
            if value["type"] == "wifi" {
                uint(&value, "ch", u8::MAX.into())?;
                if !["beacon", "probe_req", "probe_resp", "data", "other"]
                    .contains(&text(&value, "frame", 16)?)
                {
                    return Err(invalid());
                }
                Kind::Wifi {
                    mac,
                    ssid: text(&value, "ssid", 32)?.into(),
                    rssi,
                }
            } else {
                let uuid = value
                    .get("uuid")
                    .filter(|v| !v.is_null())
                    .map(|_| text(&value, "uuid", 36))
                    .transpose()?;
                let uuid16 = uuid
                    .and_then(|s| s.strip_prefix("0x"))
                    .filter(|s| s.len() == 4)
                    .and_then(|s| u16::from_str_radix(s, 16).ok());
                Kind::Ble {
                    mac,
                    name: text(&value, "name", 32)?.into(),
                    rssi,
                    manufacturer: uint(&value, "mfr", u16::MAX.into())? as u16,
                    uuid16,
                }
            }
        }
        Some("status") => {
            uint(&value, "heap_free", u32::MAX.into())?;
            if text(&value, "board", 64)?.is_empty() || text(&value, "version", 64)?.is_empty() {
                return Err(invalid());
            }
            Kind::Status {
                board: text(&value, "board", 64)?.into(),
                version: text(&value, "version", 64)?.into(),
                uptime: uint(&value, "uptime", u32::MAX.into())? as u32,
                scanning: value["scanning"].as_bool().ok_or_else(invalid)?,
            }
        }
        Some("stream_stats") => {
            for key in [
                "scan_drops",
                "output_drops",
                "ble_output_drops",
                "serialize_errors",
            ] {
                uint(&value, key, u32::MAX.into())?;
            }
            Kind::Stats
        }
        Some("detection") => {
            text(&value, "id", 64)?;
            text(&value, "scope", 32)?;
            uint(&value, "ts", u32::MAX.into())?;
            if !value["detection"].is_object() {
                return Err(invalid());
            }
            Kind::Evidence
        }
        _ => return Ok(None),
    };
    Ok(Some(Record {
        value,
        kind,
        connection_generation: 0,
        boot_generation: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_prefixed_stream_validates_and_preserves_provenance() {
        let raw=br#"INFO - {"type":"wifi","mac":"02:00:00:00:00:01","ssid":"test","rssi":-48,"ch":6,"frame":"beacon","ts":42,"match":[]}"#;
        let record = parse(raw).unwrap().unwrap();
        let output = record.output(&FilterConfig::default(), &MacIndex::from_defaults());
        assert_eq!(output["type"], "wifi_observation");
        assert_eq!(output["ts"], 42);
        assert_eq!(output["producer_record"]["type"], "wifi");
        assert!(output["producer_record"].get("boot_generation").is_none());
        assert_eq!(output["timestamp_basis"], "device_uptime_ms_not_utc");
        assert!(output.get("detection").is_some());
        assert!(parse(&vec![b'x'; MAX_LINE + 1]).is_err());
        assert!(parse(b"INFO - scanning started").unwrap().is_none());
        assert!(parse(br#"{"type":"wifi","mac":"invalid"}"#).is_err());
        assert!(parse(br#"{"type":"future","x":1}"#).unwrap().is_none());
    }
    #[test]
    fn producer_evidence_is_not_attached_by_address_and_uptime() {
        let r=parse(br#"{"type":"detection","id":"02:00:00:00:00:01","scope":"ble_address","ts":1,"detection":{"sig":[1]}}"#).unwrap().unwrap();
        assert!(!r.observation());
        assert_eq!(
            r.output(&FilterConfig::default(), &MacIndex::from_defaults())["association"],
            "unassociated_producer_evidence"
        );
    }
    #[test]
    fn summaries_do_not_invent_raw_advertisements_or_address_types() {
        let r=parse(br#"{"type":"ble","mac":"02:00:00:00:00:01","name":"sensor","rssi":-60,"mfr":0,"uuid":"0xFEAA","ts":55,"match":[]}"#).unwrap().unwrap();
        let o = r.output(&FilterConfig::default(), &MacIndex::from_defaults());
        assert_eq!(o["type"], "ble_observation");
        assert_eq!(o["address_type"], "unknown");
        assert_eq!(o["payload_completeness"], "firmware_summary_only");
        assert!(o.get("advertisement").is_none());
    }
}
