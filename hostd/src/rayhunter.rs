//! MIT IPC validation only. Diagnostic/RRC decoding runs in a separate GPL process.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io;

pub const LINE_LIMIT: usize = 96 * 1024;
pub const CHUNK_BYTES: usize = 32 * 1024;
pub const FRAME_LIMIT: usize = 256 * 1024;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid Rayhunter helper IPC")
}
fn number(v: &Value, key: &str) -> io::Result<u64> {
    v[key].as_u64().ok_or_else(invalid)
}
fn bounded_text<'a>(v: &'a Value, key: &str, max: usize) -> io::Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
        .ok_or_else(invalid)
}
pub fn validate_endpoint(endpoint: &str) -> io::Result<()> {
    let address = endpoint
        .strip_prefix("ws://")
        .and_then(|s| s.strip_suffix("/raw-diag"))
        .and_then(|s| s.parse::<std::net::SocketAddr>().ok());
    if !address.is_some_and(|a| a.ip().is_loopback() && a.port() != 0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Rayhunter requires ws://<loopback IP>:<port>/raw-diag via an explicitly selected forward"));
    }
    Ok(())
}
struct Frame {
    metadata: Value,
    sequence: u64,
    size: usize,
    offset: usize,
    hash: Sha256,
    expected: String,
}
/// Per-connection validation. A derived cell cannot precede its complete raw frame.
#[derive(Default)]
pub struct Reader {
    epoch: Option<String>,
    next: u64,
    frame: Option<Frame>,
    complete: Option<(u64, String, Value)>,
}
impl Reader {
    pub fn accept(&mut self, line: &[u8]) -> io::Result<Value> {
        if line.len() > LINE_LIMIT {
            return Err(invalid());
        }
        let v: Value = serde_json::from_slice(line).map_err(|_| invalid())?;
        if v["ipc_version"] != 1 {
            return Err(invalid());
        }
        let kind = bounded_text(&v, "kind", 32)?;
        let mut out = match kind {
            "hello" if self.epoch.is_none() => {
                let stream = &v["stream"];
                if stream["version"] != 1
                    || stream["replay"] != false
                    || stream["format"] != "u32le_json_length+json+exact_device_read"
                {
                    return Err(invalid());
                }
                self.epoch = Some(bounded_text(stream, "stream_epoch", 128)?.to_owned());
                self.next = number(stream, "next_sequence")?;
                json!({"type":"cellular_source_hello","stream":stream})
            }
            _ if self.epoch.is_none() => return Err(invalid()),
            "raw" => {
                let meta = &v["frame"];
                let sequence = number(meta, "sequence")?;
                let size = usize::try_from(number(meta, "raw_length")?).map_err(|_| invalid())?;
                let offset = usize::try_from(number(&v, "offset")?).map_err(|_| invalid())?;
                let hex = bounded_text(&v, "hex", CHUNK_BYTES * 2)?;
                let expected = bounded_text(&v, "sha256", 64)?;
                if expected.len() != 64
                    || !expected.bytes().all(|b| b.is_ascii_hexdigit())
                    || size == 0
                    || size > FRAME_LIMIT
                    || hex.len() % 2 != 0
                {
                    return Err(invalid());
                }
                number(meta, "read_monotonic_ns")?;
                if !(meta["read_system_unix_ns"].is_null()
                    || meta["read_system_unix_ns"].as_str().is_some_and(|s| {
                        s.len() <= 40 && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
                    }))
                {
                    return Err(invalid());
                }
                if !(meta["recording_id"].is_null()
                    || bounded_text(meta, "recording_id", 256).is_ok())
                {
                    return Err(invalid());
                }
                if offset == 0 {
                    if self.frame.is_some() || sequence != self.next {
                        return Err(invalid());
                    }
                    self.complete = None;
                    self.frame = Some(Frame {
                        metadata: meta.clone(),
                        sequence,
                        size,
                        offset: 0,
                        hash: Sha256::new(),
                        expected: expected.into(),
                    });
                }
                let f = self.frame.as_mut().ok_or_else(invalid)?;
                if f.metadata != *meta
                    || f.sequence != sequence
                    || f.offset != offset
                    || f.expected != expected
                    || offset + hex.len() / 2 > size
                {
                    return Err(invalid());
                }
                let bytes = hex
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| {
                        let hi = (b[0] as char).to_digit(16).ok_or_else(invalid)?;
                        let lo = (b[1] as char).to_digit(16).ok_or_else(invalid)?;
                        Ok((hi * 16 + lo) as u8)
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                f.hash.update(&bytes);
                f.offset += bytes.len();
                if f.offset == f.size {
                    let f = self.frame.take().unwrap();
                    let actual = f
                        .hash
                        .finalize()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    if actual != f.expected {
                        return Err(invalid());
                    }
                    self.complete = Some((sequence, actual, f.metadata));
                    self.next = sequence.checked_add(1).ok_or_else(invalid)?;
                }
                json!({"type":"cellular_raw_chunk","frame":meta,"offset":offset,"hex":hex,"sha256":expected,"frame_complete":self.frame.is_none()})
            }
            "cell" => {
                let sequence = number(&v, "sequence")?;
                let (last, hash, read_metadata) = self
                    .complete
                    .as_ref()
                    .filter(|(n, _, _)| *n == sequence)
                    .ok_or_else(invalid)?;
                let index = number(&v, "message_index")?;
                if index >= 4096 {
                    return Err(invalid());
                }
                let o = &v["observation"];
                if o["rat"] != "lte"
                    || !matches!(
                        o["kind"].as_str(),
                        Some("serving_measurement" | "neighbor_measurement" | "cell_identity")
                    )
                    || number(o, "earfcn")? > u32::MAX as u64
                    || number(o, "pci")? > 503
                {
                    return Err(invalid());
                }
                number(&v, "modem_timestamp")?;
                if o["kind"] == "cell_identity" {
                    for (key, len) in [("mcc", 3), ("mnc", 3)] {
                        let s = bounded_text(o, key, len)?;
                        if !s.bytes().all(|b| b.is_ascii_digit())
                            || (key == "mcc" && s.len() != 3)
                            || (key == "mnc" && s.len() < 2)
                        {
                            return Err(invalid());
                        }
                    }
                    if number(o, "tac")? > 65535 || number(o, "eci")? > 0x0fff_ffff {
                        return Err(invalid());
                    }
                }
                for key in ["rsrp_dbm", "rsrq_db", "rssi_dbm"] {
                    if let Some(value) = o.get(key) {
                        if !value.is_null()
                            && !value
                                .as_f64()
                                .is_some_and(|n| n.is_finite() && (-300.0..=100.0).contains(&n))
                        {
                            return Err(invalid());
                        }
                    }
                }
                json!({"type":"cellular_observation","observation":o,"raw_reference":{"sequence":last,"sha256":hash,"message_index":index},"source_read":read_metadata,"modem_timestamp":v["modem_timestamp"],"capture_time":null,"capture_clock_quality":"unvalidated","transmitter_location":null,"identity_completeness":if o["kind"]=="cell_identity" {"broadcast_plmn_tac_eci"} else {"earfcn_pci_only"},"wigle_exported":false})
            }
            "gap" => {
                let from = number(&v, "from_sequence")?;
                let to = number(&v, "to_sequence_exclusive")?;
                if self.frame.is_some() || from != self.next || to <= from {
                    return Err(invalid());
                }
                self.next = to;
                self.complete = None;
                json!({"type":"cellular_source_gap","from_sequence":from,"to_sequence_exclusive":to,"reason":"upstream_not_retained"})
            }
            "status" | "decode_error" => {
                if self.frame.is_some() {
                    return Err(invalid());
                }
                if kind == "decode_error"
                    && self
                        .complete
                        .as_ref()
                        .is_none_or(|(n, _, _)| Some(*n) != v["sequence"].as_u64())
                {
                    return Err(invalid());
                }
                json!({"type":"cellular_source_status","event":v})
            }
            _ => return Err(invalid()),
        };
        out["schema_version"] = 1.into();
        out["stream_epoch"] = self.epoch.clone().into();
        out["rf_loss"] = json!("unknown");
        out["replay"] = false.into();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hello() -> Value {
        json!({"ipc_version":1,"kind":"hello","stream":{"version":1,"replay":false,"format":"u32le_json_length+json+exact_device_read","stream_epoch":"epoch1","next_sequence":4}})
    }
    fn raw(bytes: &[u8], offset: usize, part: &[u8]) -> Value {
        json!({"ipc_version":1,"kind":"raw","frame":{"sequence":4,"raw_length":bytes.len(),"read_monotonic_ns":15,"read_system_unix_ns":"1234567890123456789","recording_id":"capture"},"offset":offset,"hex":part.iter().map(|b|format!("{b:02x}")).collect::<String>(),"sha256":Sha256::digest(bytes).iter().map(|b|format!("{b:02x}")).collect::<String>()})
    }
    fn cell() -> Value {
        json!({"ipc_version":1,"kind":"cell","sequence":4,"message_index":0,"modem_timestamp":123,"observation":{"rat":"lte","kind":"cell_identity","earfcn":70000,"pci":503,"mcc":"001","mnc":"01","tac":12,"eci":0xabc}})
    }
    fn accept(r: &mut Reader, v: Value) -> io::Result<Value> {
        r.accept(&serde_json::to_vec(&v).unwrap())
    }
    fn started() -> Reader {
        let mut r = Reader::default();
        accept(&mut r, hello()).unwrap();
        r
    }
    #[test]
    fn raw_chunks_require_complete_hash_before_cell_and_preserve_full_earfcn_and_mnc() {
        let mut r = started();
        let b = b"exact diagnostic bytes";
        let first = accept(&mut r, raw(b, 0, &b[..6])).unwrap();
        assert_eq!(first["hex"], "657861637420");
        assert!(accept(&mut r, cell()).is_err());
        accept(&mut r, raw(b, 6, &b[6..])).unwrap();
        let out = accept(&mut r, cell()).unwrap();
        assert_eq!(out["observation"]["earfcn"], 70000);
        assert_eq!(out["observation"]["mnc"], "01");
        assert!(out["capture_time"].is_null());
        assert!(out["transmitter_location"].is_null());
        assert_eq!(out["stream_epoch"], "epoch1");
    }
    #[test]
    fn corrupt_raw_reorder_replay_and_missing_chunks_are_rejected() {
        for mutation in [0, 1, 2, 3] {
            let mut r = started();
            let mut v = raw(b"ab", 0, b"ab");
            match mutation {
                0 => v["hex"] = "ffff".into(),
                1 => v["offset"] = 1.into(),
                2 => v["frame"]["sequence"] = 3.into(),
                _ => v["frame"]["raw_length"] = (FRAME_LIMIT + 1).into(),
            }
            assert!(accept(&mut r, v).is_err());
        }
        let mut r = started();
        accept(&mut r, raw(b"ab", 0, b"ab")).unwrap();
        assert!(accept(&mut r, raw(b"ab", 0, b"ab")).is_err());
        let mut r = started();
        accept(&mut r, raw(b"ab", 0, b"a")).unwrap();
        assert!(accept(&mut r, raw(b"ab", 0, b"ab")).is_err());
    }
    #[test]
    fn upstream_gap_advances_only_exact_cursor_and_invalidates_cell_reference() {
        let mut r = started();
        accept(&mut r, raw(b"ab", 0, b"ab")).unwrap();
        assert!(accept(
            &mut r,
            json!({"ipc_version":1,"kind":"gap","from_sequence":4,"to_sequence_exclusive":9})
        )
        .is_err());
        accept(
            &mut r,
            json!({"ipc_version":1,"kind":"gap","from_sequence":5,"to_sequence_exclusive":9}),
        )
        .unwrap();
        assert!(accept(&mut r, cell()).is_err());
    }
    #[test]
    fn no_prehello_no_second_hello_unknown_ipc_or_forged_identity() {
        assert!(accept(&mut Reader::default(), cell()).is_err());
        let mut r = started();
        assert!(accept(&mut r, hello()).is_err());
        let mut r = started();
        accept(&mut r, raw(b"ab", 0, b"ab")).unwrap();
        for (key, value) in [
            ("mnc", json!("1")),
            ("pci", json!(504)),
            ("eci", json!(0x10000000u64)),
            ("rsrp_dbm", json!(500)),
        ] {
            let mut v = cell();
            v["observation"][key] = value;
            assert!(accept(&mut r, v).is_err());
        }
        assert!(r.accept(&vec![b'x'; LINE_LIMIT + 1]).is_err());
    }
    #[test]
    fn endpoint_is_explicit_loopback_only() {
        for e in ["ws://127.0.0.1:9000/raw-diag", "ws://[::1]:9000/raw-diag"] {
            assert!(validate_endpoint(e).is_ok());
        }
        for e in [
            "ws://localhost:9000/raw-diag",
            "ws://192.0.2.1:9000/raw-diag",
            "ws://127.0.0.1:0/raw-diag",
            "ws://127.0.0.1:9000/raw-diag?control=start",
        ] {
            assert!(validate_endpoint(e).is_err());
        }
    }
}
