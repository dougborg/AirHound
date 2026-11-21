//! Strict offline profile for Marauder v1.21.0 (d9197cf0) bare WiFi PCAP.
//! Serial delimiters are recognized only between complete PCAP records.
use std::io;

pub const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONSOLE_BYTES: usize = 64 * 1024;
const BEGIN: &[u8] = b"[BUF/BEGIN]";
const CLOSE: &[u8] = b"[BUF/CLOSE]";

#[derive(Debug, Clone, Copy)]
pub struct Record<'a> {
    /// Device micros(), not wall time. Wraps/resets cannot be reconstructed.
    pub timestamp_micros32: u32,
    pub frame: &'a [u8],
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn take<'a>(data: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    if data.len() < n {
        return Err(invalid("truncated Marauder capture"));
    }
    let (head, tail) = data.split_at(n);
    *data = tail;
    Ok(head)
}
fn u32le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().expect("fixed length field"))
}

/// Validate the entire bounded capture before calling the visitor. A malformed
/// suffix therefore never emits a misleading successful partial import.
pub fn decode(
    data: &[u8],
    serial: bool,
    mut visit: impl FnMut(Record<'_>) -> io::Result<()>,
) -> io::Result<usize> {
    if data.len() > MAX_CAPTURE_BYTES {
        return Err(invalid(
            "capture exceeds 64 MiB; split at complete sessions",
        ));
    }
    walk(data, serial, &mut |_| Ok(()))?;
    walk(data, serial, &mut visit)
}

fn walk(
    mut data: &[u8],
    serial: bool,
    visit: &mut impl FnMut(Record<'_>) -> io::Result<()>,
) -> io::Result<usize> {
    let mut snaplen = None;
    let mut count = 0;
    loop {
        if serial {
            let offset = data.windows(BEGIN.len()).position(|w| w == BEGIN);
            let Some(offset) = offset else {
                if snaplen.is_none() || data.len() > MAX_CONSOLE_BYTES {
                    return Err(invalid("missing capture start or excessive console data"));
                }
                // An incomplete delimiter at EOF is a truncated next chunk.
                if (1..BEGIN.len()).any(|n| data.ends_with(&BEGIN[..n])) {
                    return Err(invalid("truncated serial chunk start"));
                }
                return Ok(count);
            };
            if offset > MAX_CONSOLE_BYTES {
                return Err(invalid("excessive console data between capture chunks"));
            }
            take(&mut data, offset + BEGIN.len())?;
        }
        if snaplen.is_none() {
            let header = take(&mut data, 24)?;
            if header[..16] != [0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                || u32le(&header[20..]) != 105
            {
                return Err(invalid("expected Marauder little-endian PCAP 2.4, DLT105"));
            }
            let limit = u32le(&header[16..]);
            if ![1514, 2324, 4096].contains(&limit) {
                return Err(invalid("unsupported Marauder snapshot profile"));
            }
            snaplen = Some(limit as usize);
        }
        loop {
            if serial && data.starts_with(CLOSE) {
                take(&mut data, CLOSE.len())?;
                break;
            }
            if !serial && data.is_empty() {
                return Ok(count);
            }
            let header = take(&mut data, 16)?;
            let seconds = u32le(header);
            let micros = u32le(&header[4..]);
            let included = u32le(&header[8..]) as usize;
            let original = u32le(&header[12..]) as usize;
            let timestamp = seconds as u64 * 1_000_000 + micros as u64;
            if micros >= 1_000_000 || timestamp > u32::MAX as u64 {
                return Err(invalid("invalid boot-relative micros32 timestamp"));
            }
            if included == 0 || included != original || included > snaplen.unwrap() {
                return Err(invalid("invalid or truncated packet length"));
            }
            let frame = take(&mut data, included)?;
            visit(Record {
                timestamp_micros32: timestamp as u32,
                frame,
            })?;
            count += 1;
        }
    }
}

/// Match only supported management-frame identities. Every raw packet remains
/// available to callers; control/data frames are not guessed into AP identities.
pub fn observation(
    record: Record<'_>,
    source_id: &str,
    index: &airhound::mac_index::MacIndex,
) -> serde_json::Value {
    let identity = record
        .frame
        .first()
        .filter(|fc| matches!(**fc, 0x80 | 0x40 | 0x50))
        .and_then(|_| airhound::scanner::parse_wifi_identity(record.frame))
        .filter(|i| {
            matches!(
                i.frame_type,
                airhound::scanner::FrameType::Beacon
                    | airhound::scanner::FrameType::ProbeRequest
                    | airhound::scanner::FrameType::ProbeResponse
            )
        });
    let mut value = serde_json::json!({
        "type": "marauder_wifi_packet", "schema_version": 1,
        "source": "marauder", "source_id": source_id, "replay": true,
        "decoder_profile": "marauder-v1.21.0-dlt105",
        "firmware_version_verified": false,
        "timestamp_micros32": record.timestamp_micros32,
        "timestamp_clock": "device_boot_micros32_wrap_or_reset_unknown",
        "rssi_dbm": null, "receive_channel": null, "loss_count": null,
        "gps": null, "wigle_exported": false, "frame_bytes": record.frame,
        "filter_policy": "identity_only_unvalidated", "identity": null,
    });
    if let Some(identity) = identity {
        let result = airhound::filter::filter_wifi_identity_with_rules(
            &identity.mac,
            &identity.ssid,
            true,
            &airhound::defaults::DEFAULT_RULE_DB,
            index,
        );
        value["identity"] = serde_json::json!({"transmitter": identity.mac, "ssid": identity.ssid.as_str(), "frame_type": identity.frame_type.as_str()});
        value["detection"] =
            serde_json::to_value(result.detection(airhound::evidence::AlertPolicy::Silent))
                .expect("serializable evidence");
    }
    value
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    fn header() -> Vec<u8> {
        let mut v = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0];
        for n in [0u32, 0, 1514, 105] {
            v.extend(n.to_le_bytes());
        }
        v
    }
    fn packet(time: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        for n in [
            time / 1_000_000,
            time % 1_000_000,
            payload.len() as u32,
            payload.len() as u32,
        ] {
            v.extend(n.to_le_bytes());
        }
        v.extend(payload);
        v
    }
    fn chunk(bytes: &[u8]) -> Vec<u8> {
        [BEGIN, bytes, CLOSE].concat()
    }
    #[test]
    fn serial_markers_in_payload_and_console_between_chunks() {
        let payload = [b"abc".as_slice(), BEGIN, CLOSE, b"xyz"].concat();
        let mut first = header();
        first.extend(packet(u32::MAX, &payload));
        let serial = [
            b"boot\r\n".as_slice(),
            &chunk(&first),
            b"scan status\n",
            &chunk(&packet(7, b"second")),
            b"stopped\n",
        ]
        .concat();
        let mut found = Vec::new();
        assert_eq!(
            decode(&serial, true, |r| {
                found.push((r.timestamp_micros32, r.frame.to_vec()));
                Ok(())
            })
            .unwrap(),
            2
        );
        assert_eq!(found, [(u32::MAX, payload), (7, b"second".to_vec())]);
    }
    #[test]
    fn pcap_and_serial_have_identical_records() {
        for snap in [1514u32, 2324, 4096] {
            let mut bytes = header();
            bytes[16..20].copy_from_slice(&snap.to_le_bytes());
            bytes.extend(packet(91_000_000, &[0xaa; 24])); // first timestamp byte '[' is not framing
            for serial in [false, true] {
                let input = if serial { chunk(&bytes) } else { bytes.clone() };
                assert_eq!(
                    decode(&input, serial, |r| {
                        assert_eq!(r.frame, &[0xaa; 24]);
                        Ok(())
                    })
                    .unwrap(),
                    1
                );
            }
        }
    }
    #[test]
    fn truncation_fails_before_any_output() {
        let mut bytes = header();
        bytes.extend(packet(10, b"payload"));
        let serial = chunk(&bytes);
        // A PCAP ending exactly after its global header is a valid empty capture.
        for end in 0..bytes.len() {
            if end == 24 {
                continue;
            }
            let mut calls = 0;
            assert!(
                decode(&bytes[..end], false, |_| {
                    calls += 1;
                    Ok(())
                })
                .is_err(),
                "PCAP {end}"
            );
            assert_eq!(calls, 0);
        }
        for end in 0..serial.len() {
            assert!(
                decode(&serial[..end], true, |_| panic!("partial output")).is_err(),
                "serial {end}"
            );
        }
    }
    #[test]
    fn malformed_profiles_lengths_and_timestamps_fail_closed() {
        let mut good = header();
        good.extend(packet(10, b"packet"));
        for (offset, value) in [
            (0, 0),
            (4, 3),
            (8, 1),
            (16, 0),
            (20, 127),
            (24, u32::MAX),
            (28, 1_000_000),
            (32, 4097),
            (36, 5),
        ] {
            let mut bad = good.clone();
            bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                decode(&bad, false, |_| panic!("partial output")).is_err(),
                "offset {offset}"
            );
        }
        assert!(decode(
            &[chunk(&good), chunk(&header())].concat(),
            true,
            |_| panic!("partial output")
        )
        .is_err());
        let large_console = vec![b'x'; MAX_CONSOLE_BYTES + 1];
        assert!(decode(&[large_console, chunk(&good)].concat(), true, |_| Ok(())).is_err());
        assert!(decode(&vec![0; MAX_CAPTURE_BYTES + 1], false, |_| Ok(())).is_err());
        assert!(
            decode(&[chunk(&good), b"[BUF/BE".to_vec()].concat(), true, |_| Ok(
                ()
            ))
            .is_err()
        );
    }
    #[test]
    fn empty_captures_and_output_error() {
        assert_eq!(decode(&header(), false, |_| unreachable!()).unwrap(), 0);
        assert_eq!(
            decode(&chunk(&header()), true, |_| unreachable!()).unwrap(),
            0
        );
        let bytes = [header(), packet(0, b"frame")].concat();
        assert_eq!(
            decode(&bytes, false, |_| Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "closed"
            )))
            .unwrap_err()
            .kind(),
            io::ErrorKind::BrokenPipe
        );
    }
    #[test]
    fn observations_preserve_bytes_without_radio_or_location_claims() {
        let mut frame = vec![0u8; 36];
        frame[0] = 0x80;
        frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame.extend([0, 12]);
        frame.extend(b"Flock-ABCDEF");
        let index = airhound::mac_index::MacIndex::from_defaults();
        let value = observation(
            Record {
                timestamp_micros32: 42,
                frame: &frame,
            },
            "awok-cli",
            &index,
        );
        assert_eq!(value["identity"]["ssid"], "Flock-ABCDEF");
        assert_eq!(value["frame_bytes"], serde_json::json!(frame));
        for field in ["rssi_dbm", "receive_channel", "gps", "loss_count"] {
            assert!(value[field].is_null());
        }
        assert_eq!(value["firmware_version_verified"], false);
        assert!(!value["detection"]["sig"].as_array().unwrap().is_empty());
        assert_eq!(value["detection"]["policy"], "silent");
        let unknown = observation(
            Record {
                timestamp_micros32: 0,
                frame: &[0xff; 24],
            },
            "awok-cli",
            &index,
        );
        assert!(unknown["identity"].is_null());
        assert!(unknown.get("detection").is_none());
        let malformed = observation(
            Record {
                timestamp_micros32: 0,
                frame: &[0x80; 16],
            },
            "awok-cli",
            &index,
        );
        assert!(malformed["identity"].is_null());
    }
}
