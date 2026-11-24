//! Pure, bounded read-time projection shared by live and saved record pages.
//! Payloads and their raw evidence are never rewritten by this module.
use serde_json::{json, Value};

pub const MAX_PROJECTION_BYTES: usize = 64 * 1024;

pub fn project(record: &Value) -> Option<Value> {
    let kind = record.get("type")?.as_str()?;
    if !matches!(
        kind,
        "wifi_observation"
            | "ble_observation"
            | "external_wifi_observation"
            | "ble_packet_observation"
            | "wifi_packet_observation"
            | "cellular_observation"
    ) {
        return None;
    }
    let mut out = json!({"schema_version":1,"projection_basis":"stored_source_evidence",
        "frequency_mhz":null,"band":null,"phy":null,"frequency_basis":null,
        "phy_basis":null,"receive_time_validated":false});
    if matches!(
        kind,
        "wifi_observation" | "external_wifi_observation" | "wifi_packet_observation"
    ) {
        if let Some(frequency) = record["frequency_mhz"]
            .as_u64()
            .and_then(|v| u16::try_from(v).ok())
        {
            if let Some((band, _)) = airhound::wifi_frequency::channel(frequency) {
                out["frequency_mhz"] = json!(frequency);
                out["band"] = json!(band.as_str());
                out["frequency_basis"] = json!("source_reported_wifi_frequency");
            }
        }
    }
    if kind == "ble_packet_observation" {
        out["band"] = json!("2.4GHz");
        // Older records lack this pseudo-header flag: retain uncertainty rather
        // than assuming that a possibly aliased RF channel was measured.
        if record["rf_channel_aliased"] == false {
            if let Some(rf) = record["rf_channel"].as_u64().filter(|v| *v <= 39) {
                out["frequency_mhz"] = json!(2402 + 2 * rf);
                out["frequency_basis"] =
                    json!("derived_from_nonaliased_ble_pseudoheader_rf_channel");
            }
        }
        if let Some(phy) = record["phy"]
            .as_str()
            .filter(|v| matches!(*v, "1m" | "2m" | "coded_s2" | "coded_s8"))
        {
            out["phy"] = json!(phy);
            out["phy_basis"] = json!("receive_packet_pseudoheader");
        }
        out["channel_basis"] = json!("BLE_logical_channel_not_RF_index");
        out["rf_channel_aliased"] = record
            .get("rf_channel_aliased")
            .cloned()
            .unwrap_or(Value::Null);
    }
    if kind == "external_wifi_observation"
        && record["source"] == "mt7610u-lab"
        && record["schema_version"] == 1
    {
        if let Some(ch) = record["receive_channel"]
            .as_u64()
            .and_then(|v| u8::try_from(v).ok())
            .filter(|v| crate::external_wifi::supported_receive_channel(*v))
        {
            let frequency = if ch <= 13 {
                2407 + 5 * u16::from(ch)
            } else {
                5000 + 5 * u16::from(ch)
            };
            out["frequency_mhz"] = json!(frequency);
            out["band"] = json!(if ch <= 13 { "2.4GHz" } else { "5GHz" });
            out["frequency_basis"] = json!("derived_from_mt7610u_supported_receive_channel");
        }
        out["signal_basis"] = json!(if record["rssi_validated"] == true {
            "source_validated"
        } else {
            "uncalibrated_estimate_not_measured_RSSI"
        });
    }
    if kind == "wifi_packet_observation" {
        if let Some(raw) = record["frame_bytes"]
            .as_array()
            .filter(|v| v.len() <= 16 * 1024)
        {
            if let Some(bytes) = raw
                .iter()
                .map(|v| v.as_u64().and_then(|v| u8::try_from(v).ok()))
                .collect::<Option<Vec<_>>>()
            {
                out["wifi"] = crate::wifi_metadata::frame_projection(&bytes).unwrap_or(Value::Null);
            }
        }
    }
    if kind == "wifi_observation" {
        if let Some(wifi) = record
            .get("wifi_metadata")
            .filter(|v| v["schema_version"] == 1 && v["source"] == "corewlan_scan_snapshot")
        {
            out["wifi"] = wifi.clone();
            if out["frequency_mhz"].is_number() {
                out["frequency_basis"] = wifi
                    .get("frequency_basis")
                    .cloned()
                    .unwrap_or(out["frequency_basis"].clone());
            }
        }
    }
    if let Some(advertisement) = record.get("advertisement").filter(|v| v.is_object()) {
        out["advertisement_completeness"] = advertisement
            .get("completeness")
            .cloned()
            .unwrap_or(Value::Null);
        out["advertisement_freshness"] = advertisement
            .get("freshness")
            .cloned()
            .unwrap_or(Value::Null);
    }
    if serde_json::to_vec(&out).ok()?.len() > MAX_PROJECTION_BYTES {
        return Some(
            json!({"schema_version":1,"projection_state":"omitted_size_limit","raw_payload_retained":true}),
        );
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radio_context_is_required_and_raw_values_do_not_change() {
        let mut cat = json!({"type":"ble_packet_observation","rf_channel":12,"channel":38,"phy":"1m","pdu":[1,2,3]});
        assert!(
            project(&cat).unwrap()["frequency_mhz"].is_null(),
            "older alias flag absent cannot be inferred"
        );
        cat["rf_channel_aliased"] = true.into();
        assert!(project(&cat).unwrap()["frequency_mhz"].is_null());
        cat["rf_channel_aliased"] = false.into();
        let before = cat.clone();
        let out = project(&cat).unwrap();
        assert_eq!(out["frequency_mhz"], 2426);
        assert_eq!(out["phy"], "1m");
        assert_eq!(cat, before);
        let bare = json!({"type":"wifi_observation","ch":1});
        assert!(project(&bare).unwrap()["frequency_mhz"].is_null());
        let mut alfa = json!({"type":"external_wifi_observation","source":"mt7610u-lab","schema_version":1,"receive_channel":36,"rssi_validated":false,"estimated_rssi_dbm":-50});
        assert_eq!(project(&alfa).unwrap()["frequency_mhz"], 5180);
        alfa["source"] = "unknown".into();
        assert!(project(&alfa).unwrap()["frequency_mhz"].is_null());
    }
    #[test]
    fn source_specific_metadata_preserves_partial_identity_and_unknowns() {
        let cell = json!({"type":"cellular_observation","observation":{"pci":1,"earfcn":100}});
        let before = cell.clone();
        let p = project(&cell).unwrap();
        assert!(p["frequency_mhz"].is_null());
        assert_eq!(cell, before);
        let ble = json!({"type":"ble_observation","peripheral_id":"fixture","advertisement":{"completeness":"possibly_cached","freshness":"mixed"}});
        let p = project(&ble).unwrap();
        assert_eq!(p["advertisement_completeness"], "possibly_cached");
        assert!(p.get("address").is_none());
        assert!(project(&json!({"type":"scanner_health"})).is_none());
    }
}
