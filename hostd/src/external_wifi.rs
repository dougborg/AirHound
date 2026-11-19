// SPDX-License-Identifier: MIT
//! Bounded validation of external scanner observations. This module does not
//! load a driver, operate hardware, interpret estimates as measured RSSI, or
//! export observations. BSSIDs require uppercase colon-separated hexadecimal. Unknown fields are accepted for forward extensions.
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::HashSet,
    io::{self, Read},
};

pub const MAX_LINE_BYTES: usize = 16 * 1024;
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

// Unlike a plain Option field, this requires the key even when its value is null.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}

#[derive(Deserialize)]
struct Beacon {
    schema_version: u32,
    #[serde(rename = "type")]
    kind: String,
    source: String,
    session_id: String,
    sequence: u64,
    bssid: String,
    #[serde(deserialize_with = "nullable")]
    ssid_bytes: Option<Vec<u8>>,
    #[serde(deserialize_with = "nullable")]
    advertised_channel: Option<u8>,
    receive_channel: u8,
    elapsed_ms: u64,
    observed_at_unix_ms: u64,
    dwell_elapsed_ms: u64,
    channel_epoch: u64,
    #[serde(deserialize_with = "nullable")]
    rssi_dbm: Option<i8>,
    #[serde(deserialize_with = "nullable")]
    estimated_rssi_dbm: Option<i8>,
    rssi_validated: bool,
    rssi_estimate_basis: String,
    raw_rssi: [u8; 4],
    full_calibration: bool,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub schema_version: u32,
    pub records: u64,
    pub bytes: u64,
    pub distinct_bssids: usize,
    pub distinct_receive_channels: usize,
    pub estimated_signal_records: u64,
    pub missing_signal_records: u64,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn bssid(text: &str) -> Option<[u8; 6]> {
    if text.len() != 17 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut mac = [0; 6];
    for (i, octet) in mac.iter_mut().enumerate() {
        if i > 0 && bytes[i * 3 - 1] != b':' {
            return None;
        }
        let pair = &bytes[i * 3..i * 3 + 2];
        if !pair
            .iter()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b))
        {
            return None;
        }
        *octet = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    (mac != [0; 6] && mac[0] & 1 == 0).then_some(mac)
}

fn advertised_channel(channel: u8) -> bool {
    (1..=14).contains(&channel)
        || ((36..=64).contains(&channel) && channel.is_multiple_of(4))
        || ((100..=144).contains(&channel) && channel.is_multiple_of(4))
        || ((149..=177).contains(&channel) && channel % 4 == 1)
}

/// Require complete newline-terminated records; never include captured data in
/// diagnostics or summaries. The file limit includes delimiters and blank lines
/// are invalid. Input reads and storage remain bounded even without a newline.
pub fn validate(reader: impl Read) -> io::Result<Summary> {
    validate_with_limits(reader, MAX_LINE_BYTES, MAX_FILE_BYTES)
}

fn validate_with_limits(
    mut reader: impl Read,
    line_limit: usize,
    file_limit: u64,
) -> io::Result<Summary> {
    let mut summary = Summary {
        schema_version: 1,
        ..Summary::default()
    };
    let mut line = Vec::new();
    let mut buffer = [0; 4096];
    let mut session: Option<String> = None;
    let mut last: Option<(u64, u64, u8, u64)> = None;
    let mut identifiers = HashSet::new();
    let mut channels = HashSet::new();
    loop {
        let remaining = file_limit.saturating_sub(summary.bytes);
        let read_limit = buffer.len().min(remaining.saturating_add(1) as usize);
        let n = match reader.read(&mut buffer[..read_limit]) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            break;
        }
        summary.bytes += n as u64;
        if summary.bytes > file_limit {
            return Err(invalid("capture exceeds file byte limit"));
        }
        for &byte in &buffer[..n] {
            if byte != b'\n' {
                if line.len() == line_limit {
                    return Err(invalid("capture record exceeds line byte limit"));
                }
                line.push(byte);
                continue;
            }
            if line.len() >= line_limit {
                return Err(invalid(
                    "capture record including newline exceeds line byte limit",
                ));
            }
            let record: Beacon = serde_json::from_slice(&line)
                .map_err(|_| invalid("invalid capture record fields or JSON"))?;
            line.clear();
            if record.schema_version != 1
                || record.kind != "beacon"
                || record.source != "mt7610u-lab"
            {
                return Err(invalid(
                    "unsupported capture schema, record type, or source",
                ));
            }
            if record.session_id.is_empty()
                || record.session_id.len() > 128
                || record.session_id.chars().any(char::is_control)
            {
                return Err(invalid("invalid capture session identifier"));
            }
            match &session {
                None => session = Some(record.session_id),
                Some(id) if id != &record.session_id => {
                    return Err(invalid("capture session changed"))
                }
                _ => {}
            }
            if record.sequence != summary.records {
                return Err(invalid("capture sequence is not consecutive from zero"));
            }
            let mac = bssid(&record.bssid).ok_or_else(|| invalid("invalid capture BSSID"))?;
            if record.ssid_bytes.as_ref().is_some_and(|v| v.len() > 32) {
                return Err(invalid("SSID exceeds 32 bytes"));
            }
            if !(1..=11).contains(&record.receive_channel)
                && ![36, 40, 44, 48].contains(&record.receive_channel)
            {
                return Err(invalid("unsupported receive channel"));
            }
            if record
                .advertised_channel
                .is_some_and(|c| !advertised_channel(c))
            {
                return Err(invalid("invalid advertised channel"));
            }
            if record.rssi_dbm.is_some()
                || record.rssi_validated
                || record.full_calibration
                || record.rssi_estimate_basis != "mt76_eeprom_formula"
            {
                return Err(invalid(
                    "unsupported signal provenance or calibration claim",
                ));
            }
            if let Some((elapsed, epoch, channel, dwell)) = last {
                if record.elapsed_ms < elapsed
                    || record.channel_epoch < epoch
                    || (record.channel_epoch == epoch
                        && (record.receive_channel != channel || record.dwell_elapsed_ms < dwell))
                {
                    return Err(invalid(
                        "capture time or channel epoch regressed or changed inconsistently",
                    ));
                }
            }
            last = Some((
                record.elapsed_ms,
                record.channel_epoch,
                record.receive_channel,
                record.dwell_elapsed_ms,
            ));
            // Shape/type checking above preserves raw samples without treating
            // them as measurements or recomputing absent EEPROM calibration.
            let _ = (record.raw_rssi, record.observed_at_unix_ms);
            summary.records += 1;
            summary.estimated_signal_records += u64::from(record.estimated_rssi_dbm.is_some());
            summary.missing_signal_records += u64::from(record.estimated_rssi_dbm.is_none());
            identifiers.insert(mac);
            channels.insert(record.receive_channel);
        }
    }
    if !line.is_empty() {
        return Err(invalid(
            "capture ends with an incomplete newline-delimited record",
        ));
    }
    if summary.records == 0 {
        return Err(invalid("capture is empty"));
    }
    summary.distinct_bssids = identifiers.len();
    summary.distinct_receive_channels = channels.len();
    Ok(summary)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn beacon() -> Value {
        json!({"schema_version":1,"type":"beacon","source":"mt7610u-lab","session_id":"test", "sequence":0,"bssid":"02:00:00:00:00:01","ssid_bytes":[255,0,128],"advertised_channel":13,"receive_channel":1,"elapsed_ms":1,"observed_at_unix_ms":1,"dwell_elapsed_ms":1,"channel_epoch":0,"rssi_dbm":null,"estimated_rssi_dbm":-80,"rssi_validated":false,"rssi_estimate_basis":"mt76_eeprom_formula","raw_rssi":[186,0,0,0],"full_calibration":false})
    }
    fn line(value: &Value) -> Vec<u8> {
        format!("{value}\n").into_bytes()
    }
    #[test]
    fn binary_ssid_unknown_extensions_and_summary_privacy() {
        let mut value = beacon();
        value["future"] = json!({"anything":true});
        let summary = validate(&line(&value)[..]).unwrap();
        assert_eq!(summary.records, 1);
        assert_eq!(summary.distinct_bssids, 1);
        let text = serde_json::to_string(&summary).unwrap();
        assert!(!text.contains("02:00"));
        assert!(!text.contains("test"));
        value["ssid_bytes"] = Value::Null;
        value["estimated_rssi_dbm"] = Value::Null;
        assert_eq!(
            validate(&line(&value)[..]).unwrap().missing_signal_records,
            1
        );
    }
    #[test]
    fn invalid_fields_and_provenance_fail_closed() {
        for (key, value) in [
            ("schema_version", json!(2)),
            ("sequence", json!(1)),
            ("rssi_dbm", json!(-50)),
            ("estimated_rssi_dbm", json!(-129)),
            ("rssi_validated", json!(true)),
            ("full_calibration", json!(true)),
            ("bssid", json!("01:00:00:00:00:01")),
            ("bssid", json!("00:00:00:00:00:00")),
            ("receive_channel", json!(12)),
            ("ssid_bytes", json!(vec![0; 33])),
        ] {
            let mut record = beacon();
            record[key] = value;
            assert!(validate(&line(&record)[..]).is_err(), "{key}");
        }
        let mut record = beacon();
        record.as_object_mut().unwrap().remove("rssi_dbm");
        assert!(validate(&line(&record)[..]).is_err());
    }
    #[test]
    fn sessions_sequence_and_channel_epochs_are_consistent() {
        let first = line(&beacon());
        let mut next = beacon();
        next["sequence"] = json!(1);
        let mut bytes = first.clone();
        bytes.extend(line(&next));
        assert_eq!(validate(&bytes[..]).unwrap().records, 2);
        for (key, value) in [
            ("sequence", json!(0)),
            ("session_id", json!("different")),
            ("receive_channel", json!(6)),
            ("elapsed_ms", json!(0)),
        ] {
            let mut bad = next.clone();
            bad[key] = value;
            let mut bytes = first.clone();
            bytes.extend(line(&bad));
            assert!(validate(&bytes[..]).is_err());
        }
    }
    #[test]
    fn bounded_lines_files_and_complete_records() {
        let bytes = line(&beacon());
        assert!(validate_with_limits(&bytes[..], bytes.len() - 1, bytes.len() as u64).is_err());
        assert!(validate_with_limits(&bytes[..], bytes.len(), bytes.len() as u64).is_ok());
        assert!(validate_with_limits(&bytes[..], 20, 65536).is_err());
        assert!(validate_with_limits(&bytes[..], 65536, bytes.len() as u64 - 1).is_err());
        assert!(validate(&bytes[..bytes.len() - 1]).is_err());
        assert!(validate(&b""[..]).is_err());
        assert!(validate(&b"\n"[..]).is_err());
    }
    #[test]
    fn fragmented_reads_and_interruptions_preserve_framing() {
        struct Fragments {
            bytes: io::Cursor<Vec<u8>>,
            interrupted: bool,
        }
        impl Read for Fragments {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.interrupted = !self.interrupted;
                if self.interrupted {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.bytes.read(&mut out[..1])
            }
        }
        assert_eq!(
            validate(Fragments {
                bytes: io::Cursor::new(line(&beacon())),
                interrupted: false
            })
            .unwrap()
            .records,
            1
        );
    }
}
