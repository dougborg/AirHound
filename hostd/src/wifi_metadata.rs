//! Desktop observation metadata. Raw evidence is bounded, and advertised
//! capabilities never stand in for the PHY used to receive a packet.
use serde::Serialize;
use serde_json::{json, Value};

pub const MAX_IE_BYTES: usize = 4096;
pub const MAX_SSID_BYTES: usize = 32;

#[derive(Clone, Debug, Serialize)]
pub struct BoundedBytes {
    pub bytes: Vec<u8>,
    pub original_length: usize,
    pub truncated: bool,
}
impl BoundedBytes {
    pub fn new(bytes: &[u8], limit: usize) -> Self {
        let limit = limit.min(MAX_IE_BYTES);
        Self {
            bytes: bytes[..bytes.len().min(limit)].to_vec(),
            original_length: bytes.len(),
            truncated: bytes.len() > limit,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Metadata {
    pub source: &'static str,
    pub ssid_bytes: Option<BoundedBytes>,
    pub information_elements: Option<BoundedBytes>,
    pub band: Option<&'static str>,
    pub frequency_provenance: Option<&'static str>,
    pub channel_width_mhz: Option<u16>,
    pub noise_dbm: Option<i16>,
    pub beacon_interval_ms: Option<u32>,
    pub country_code: Option<String>,
    pub ibss: Option<bool>,
    pub advertised_phy_modes: Vec<&'static str>,
    pub security_modes: Vec<&'static str>,
}
impl Metadata {
    pub fn projection(&self) -> Value {
        let mut value = json!({"schema_version":1,"source":self.source,
            "ssid":self.ssid_bytes,"information_elements":self.information_elements,
            "band":self.band,"frequency_basis":self.frequency_provenance,
            "channel_width_mhz":self.channel_width_mhz,"noise_dbm":self.noise_dbm,
            "beacon_interval_ms":self.beacon_interval_ms,"country_code":self.country_code,
            "ibss":self.ibss,"advertised_phy_modes":self.advertised_phy_modes,
            "security_modes":self.security_modes,"receive_phy":null,
            "timestamp_basis":"OS_scan_snapshot_capture_time_unknown",
            "frame_basis":"os_scan_snapshot_subtype_unknown",
            "query_scope":["phy_modes_through_80211ax","channel_widths_through_160mhz","security_public_corewlan_enum_including_owe"],
            "unqueried_capabilities":["80211be","320MHz"]});
        if let Some(ies) = &self.information_elements {
            value["advertised"] = information_elements(&ies.bytes, ies.truncated);
        }
        value
    }
}

/// Bounded tagged-parameter projection; incomplete raw bytes remain evidence.
/// Security fields are advertised information, not a successful connection.
pub fn information_elements(bytes: &[u8], truncated: bool) -> Value {
    let limited = &bytes[..bytes.len().min(MAX_IE_BYTES)];
    let truncated = truncated || limited.len() != bytes.len();
    let mut offset = 0;
    let mut malformed = false;
    let mut ssids = Vec::new();
    let mut channels = Vec::new();
    let mut security = Vec::new();
    let mut capabilities = Vec::new();
    let mut ids = Vec::new();
    let mut security_omitted = 0;
    let mut channel_evidence_omitted = 0;
    while offset < limited.len() {
        let Some(header) = limited.get(offset..offset + 2) else {
            malformed = true;
            break;
        };
        let end = offset + 2 + usize::from(header[1]);
        let Some(body) = limited.get(offset + 2..end) else {
            malformed = true;
            break;
        };
        let id = header[0];
        // At most 2048 empty IEs fit the byte bound, but retain only the useful
        // distinct element IDs and one bounded parsed summary per security IE.
        if !ids.contains(&id) {
            ids.push(id);
        }
        match id {
            0 if body.len() <= MAX_SSID_BYTES => ssids.push(body.to_vec()),
            0 => malformed = true,
            3 if body.len() != 1 => malformed = true,
            61 if body.len() != 22 => malformed = true,
            45 if body.len() != 26 => malformed = true,
            191 if body.len() != 12 => malformed = true,
            3 if body.len() == 1 => {
                if channels.len() < 32 {
                    channels.push(("DS_parameter_set", body[0]));
                } else {
                    channel_evidence_omitted += 1;
                }
            }
            61 if body.len() >= 22 => {
                if channels.len() < 32 {
                    channels.push(("HT_operation", body[0]));
                } else {
                    channel_evidence_omitted += 1;
                }
            }
            45 if body.len() == 26 => capabilities.push("HT_capability_element"),
            191 if body.len() == 12 => capabilities.push("VHT_capability_element"),
            255 if body.first() == Some(&35) => capabilities.push("HE_capability_element_unparsed"),
            255 if body.first() == Some(&108) => {
                capabilities.push("EHT_capability_element_unparsed")
            }
            48 if security.len() < 16 => security.push(security_element(body, false)),
            221 if body.starts_with(&[0x00, 0x50, 0xf2, 1]) && security.len() < 16 => {
                security.push(security_element(&body[4..], true))
            }
            68 if security.len() < 16 => security.push(
                json!({"protocol":"WAPI","parse_status":"unsupported","body_length":body.len()}),
            ),
            48 | 68 => security_omitted += 1,
            221 if body.starts_with(&[0x00, 0x50, 0xf2, 1]) => security_omitted += 1,
            _ => (),
        }
        offset = end;
    }
    ssids.sort();
    ssids.dedup();
    capabilities.sort();
    capabilities.dedup();
    let unique_channels: std::collections::BTreeSet<_> =
        channels.iter().map(|(_, ch)| *ch).collect();
    let advertised_channel =
        (unique_channels.len() == 1 && channel_evidence_omitted == 0 && !malformed && !truncated)
            .then(|| *unique_channels.first().unwrap());
    json!({"parse_status":if truncated { "truncated" } else if malformed { "malformed" } else { "complete" },
        "parsed_bytes":offset,"element_ids":ids,
        "ssid_bytes":if ssids.len()==1 {Some(&ssids[0])}else{None},"ssid_conflict":ssids.len()>1,
        "advertised_channel":advertised_channel,"advertised_channel_conflict":unique_channels.len()>1,
        "channel_evidence":channels,"capability_elements":capabilities,
        "security_elements":security,"security_elements_omitted":security_omitted,"channel_evidence_omitted":channel_evidence_omitted,"security_basis":"advertised_information_elements_not_receive_mode"})
}

fn selector(bytes: &[u8], akm: bool) -> Value {
    let standard = bytes.starts_with(&[0x00, 0x0f, 0xac]);
    let wpa = bytes.starts_with(&[0x00, 0x50, 0xf2]);
    let label = if standard || wpa {
        match (akm, bytes[3]) {
            (false, 0) => "use_group",
            (false, 1) => "WEP40",
            (false, 2) => "TKIP",
            (false, 4) => "CCMP-128",
            (false, 5) => "WEP104",
            (false, 6) if standard => "BIP-CMAC-128",
            (false, 8) if standard => "GCMP-128",
            (false, 9) if standard => "GCMP-256",
            (false, 10) if standard => "CCMP-256",
            (true, 1) => "802.1X",
            (true, 2) => "PSK",
            (true, 3) if standard => "FT-802.1X",
            (true, 4) if standard => "FT-PSK",
            (true, 5) if standard => "802.1X-SHA256",
            (true, 6) if standard => "PSK-SHA256",
            (true, 8) if standard => "SAE",
            (true, 9) if standard => "FT-SAE",
            (true, 18) if standard => "OWE",
            _ => "unknown",
        }
    } else {
        "unknown"
    };
    json!({"selector":bytes,"name":label})
}
fn security_element(body: &[u8], wpa: bool) -> Value {
    let mut out = json!({"protocol":if wpa {"WPA"}else{"RSN"},"parse_status":"malformed"});
    let mut at = 0;
    fn take<'a>(body: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
        let bytes = body.get(*at..at.checked_add(n)?)?;
        *at += n;
        Some(bytes)
    }
    let parsed = (|| -> Option<()> {
        let version = u16::from_le_bytes(take(body, &mut at, 2)?.try_into().ok()?);
        out["version"] = json!(version);
        if version != 1 {
            out["parse_status"] = json!("unsupported_version");
            return Some(());
        }
        // RSN permits omission of later fields at complete field boundaries.
        // Preserve absence; do not invent the specification's default suites.
        if !wpa && at == body.len() {
            out["parse_status"] = json!("fields_omitted");
            return Some(());
        }
        out["group_cipher"] = selector(take(body, &mut at, 4)?, false);
        for (field, akm) in [("pairwise_ciphers", false), ("akm_suites", true)] {
            if !wpa && at == body.len() {
                out["parse_status"] = json!("fields_omitted");
                return Some(());
            }
            let count = usize::from(u16::from_le_bytes(take(body, &mut at, 2)?.try_into().ok()?));
            if count == 0 || count > 64 {
                return None;
            }
            let raw = take(body, &mut at, count.checked_mul(4)?)?;
            out[field] = Value::Array(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|v| selector(v, akm))
                    .collect(),
            );
        }
        if at < body.len() {
            let caps = u16::from_le_bytes(take(body, &mut at, 2)?.try_into().ok()?);
            out["capabilities_bits"] = json!(caps);
            if !wpa {
                out["pmf_required"] = json!(caps & (1 << 6) != 0);
                out["pmf_capable"] = json!(caps & (1 << 7) != 0);
            }
        }
        if !wpa && at < body.len() {
            let count = usize::from(u16::from_le_bytes(take(body, &mut at, 2)?.try_into().ok()?));
            take(body, &mut at, count.checked_mul(16)?)?;
            out["pmkid_count"] = json!(count);
            if at < body.len() {
                out["group_management_cipher"] = selector(take(body, &mut at, 4)?, false);
            }
        }
        out["parse_status"] = json!(if at == body.len() {
            "complete"
        } else {
            "unsupported_tail"
        });
        Some(())
    })();
    if parsed.is_none() {
        out["parse_status"] = json!("malformed");
    }
    out
}

/// Only plaintext management tagged parameters are interpreted. Raw frames stay
/// in their original record; the derived metadata has its own parser provenance.
pub fn frame_projection(frame: &[u8]) -> Option<Value> {
    let fc = u16::from_le_bytes(frame.get(..2)?.try_into().ok()?);
    if fc & 0x000f != 0 || fc & (0x4000 | 0x8000 | 0x0400) != 0 {
        return None;
    }
    let subtype = (fc >> 4) & 15;
    let start = match subtype {
        8 | 5 => 36,
        4 => 24,
        _ => return None,
    };
    let ies = frame.get(start..)?;
    if frame[22] & 0x0f != 0 {
        return None;
    }
    let bounded = BoundedBytes::new(ies, MAX_IE_BYTES);
    let privacy = if start == 36 {
        Some(u16::from_le_bytes(frame[34..36].try_into().ok()?) & 0x10 != 0)
    } else {
        None
    };
    Some(
        json!({"schema_version":1,"source":"raw_80211_management_frame",
        "information_elements":bounded,"advertised":information_elements(&bounded.bytes,bounded.truncated),
        "privacy_bit":privacy,"bssid":&frame[16..22],"transmitter":&frame[10..16],"receive_phy":null,"receive_frequency_mhz":null}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rsn() -> Vec<u8> {
        vec![
            1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 2, 0, 0, 15, 172, 2, 0, 15, 172, 8, 0xc0, 0,
        ]
    }
    #[test]
    fn raw_unknown_ies_non_utf8_ssid_and_security_are_independent() {
        let body = rsn();
        let mut bytes = vec![0, 3, 0xff, 0, 0x80, 222, 3, 1, 2, 3, 48, body.len() as u8];
        bytes.extend(&body);
        let raw = BoundedBytes::new(&bytes, MAX_IE_BYTES);
        let parsed = information_elements(&raw.bytes, raw.truncated);
        assert_eq!(raw.bytes, bytes);
        assert_eq!(parsed["parse_status"], "complete");
        assert_eq!(parsed["ssid_bytes"], json!([255, 0, 128]));
        assert!(parsed["element_ids"]
            .as_array()
            .unwrap()
            .contains(&json!(222)));
        let security = &parsed["security_elements"][0];
        assert_eq!(security["parse_status"], "complete");
        assert_eq!(security["akm_suites"][1]["name"], "SAE");
        assert_eq!(security["pmf_required"], true);
    }
    #[test]
    fn malformed_counts_unknown_suites_and_truncation_never_become_open() {
        let mut body = rsn();
        body[6] = 255;
        assert_eq!(security_element(&body, false)["parse_status"], "malformed");
        let mut body = rsn();
        body[2] = 0xaa;
        assert_eq!(
            security_element(&body, false)["group_cipher"]["name"],
            "unknown"
        );
        for (bytes, truncated, state) in [
            (vec![48, 4, 1], false, "malformed"),
            (vec![0, 0], true, "truncated"),
            (vec![], false, "complete"),
        ] {
            let p = information_elements(&bytes, truncated);
            assert_eq!(p["parse_status"], state);
            assert!(p.get("security").is_none());
        }
        let bytes = vec![0u8; MAX_IE_BYTES + 10];
        let raw = BoundedBytes::new(&bytes, MAX_IE_BYTES);
        assert_eq!(raw.original_length, MAX_IE_BYTES + 10);
        assert!(raw.truncated);
        assert_eq!(raw.bytes.len(), MAX_IE_BYTES);
    }
    #[test]
    fn optional_rsn_field_boundaries_are_distinct_from_partial_fields() {
        let full = rsn();
        for end in [2, 6, 12] {
            let parsed = security_element(&full[..end], false);
            assert_eq!(parsed["parse_status"], "fields_omitted", "boundary {end}");
        }
        for end in [0, 1, 3, 5, 7, 9, 11, 13, 15, 21, 23] {
            assert_eq!(
                security_element(&full[..end], false)["parse_status"],
                "malformed",
                "partial {end}"
            );
        }
        assert_eq!(
            security_element(&full[..22], false)["parse_status"],
            "complete"
        );
        let malformed = information_elements(&[3, 1, 6, 61, 1, 6], false);
        assert_eq!(malformed["parse_status"], "malformed");
        assert!(malformed["advertised_channel"].is_null());
        assert!(information_elements(&[3, 1, 6], true)["advertised_channel"].is_null());
    }
    #[test]
    fn conflicting_advertised_channels_and_fragmented_frames_are_not_receive_metadata() {
        let ies = vec![3, 1, 6, 3, 1, 11];
        let p = information_elements(&ies, false);
        assert!(p["advertised_channel"].is_null());
        assert_eq!(p["advertised_channel_conflict"], true);
        let mut frame = vec![0; 36];
        frame[0] = 0x80;
        frame[34] = 0x10;
        frame.extend([3, 1, 6]);
        let p = frame_projection(&frame).unwrap();
        assert_eq!(p["advertised"]["advertised_channel"], 6);
        assert!(p["receive_frequency_mhz"].is_null());
        assert!(p["receive_phy"].is_null());
        assert_eq!(p["privacy_bit"], true);
        frame[1] = 0x40;
        assert!(frame_projection(&frame).is_none());
        frame[1] = 0;
        frame[22] = 1;
        assert!(frame_projection(&frame).is_none());
    }
}
