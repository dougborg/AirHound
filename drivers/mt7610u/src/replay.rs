// SPDX-License-Identifier: GPL-2.0-only
//! Offline summaries of capture's little-endian u32-length-prefixed USB records.
//! No USB device is opened; identifiers are retained only in an internal set and
//! are never returned in the summary. CRC errors count decoded frames only:
//! corrupt descriptors/headers belong to `malformed`, not a guessed CRC count.

use crate::rx;
use std::collections::HashSet;
use std::io::{self, Read};

pub const MAX_TRANSFER_BYTES: usize = 65_536;
/// Includes every four-byte length prefix, so zero-length records are bounded too.
pub const MAX_RECORDING_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReplaySummary {
    pub transfers: u64,
    /// USB payload bytes, excluding the recording's four-byte length prefixes.
    pub bytes: u64,
    /// Successfully decoded frames; malformed transfers contribute no frames.
    pub frames: u64,
    pub beacons: u64,
    /// CRC-error flags among successfully decoded frames, not malformed transfers.
    pub crc_errors: u64,
    /// Complete USB records rejected by the RX parser; truncated files are fatal.
    pub malformed: u64,
    pub distinct_bssids: u64,
}

/// Summarize a length-prefixed recording using at most one 64KiB transfer buffer.
///
/// Clean EOF between records is success. Partial lengths/payloads return
/// `UnexpectedEof`, oversized inputs return `InvalidData`, and other reader
/// errors propagate with their original kind. Malformed USB records are counted
/// and skipped using their recording length, never by scanning for a signature.
/// The 64MiB input limit includes length prefixes; one extra byte may be read to
/// distinguish exact-limit EOF from an over-budget file.
pub fn replay(reader: impl Read) -> io::Result<ReplaySummary> {
    replay_with_limit(reader, MAX_RECORDING_BYTES)
}

fn replay_with_limit(mut reader: impl Read, recording_limit: u64) -> io::Result<ReplaySummary> {
    let mut summary = ReplaySummary::default();
    let mut identifiers = HashSet::new();
    let mut recorded_bytes = 0u64;
    let mut transfer = Vec::new();
    loop {
        let mut length = [0; 4];
        // read_exact alone cannot distinguish clean EOF from a partial prefix.
        let has_record = loop {
            match reader.read(&mut length[..1]) {
                Ok(0) => break false,
                Ok(_) => break true,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        };
        if !has_record {
            summary.distinct_bssids = identifiers.len() as u64;
            return Ok(summary);
        }
        if recording_limit.saturating_sub(recorded_bytes) < 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "recording exceeds the byte budget",
            ));
        }
        reader.read_exact(&mut length[1..]).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("record {} length prefix: {err}", summary.transfers + 1),
            )
        })?;
        recorded_bytes += 4;
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_TRANSFER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "USB transfer exceeds 65536 bytes",
            ));
        }
        if length as u64 > recording_limit - recorded_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "recording exceeds the byte budget",
            ));
        }
        transfer.resize(length, 0);
        reader.read_exact(&mut transfer).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("record {} USB payload: {err}", summary.transfers + 1),
            )
        })?;
        recorded_bytes += length as u64;
        summary.transfers += 1;
        summary.bytes += length as u64;
        match rx::parse_usb_transfer(&transfer) {
            Ok(frames) => {
                summary.frames += frames.len() as u64;
                for frame in frames {
                    summary.crc_errors += u64::from(frame.fcs_error);
                    if let Some(beacon) = frame.beacon {
                        summary.beacons += 1;
                        identifiers.insert(beacon.bssid);
                    }
                }
            }
            Err(_) => summary.malformed += 1,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn record(transfer: &[u8]) -> Vec<u8> {
        let mut bytes = (transfer.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(transfer);
        bytes
    }

    fn beacon(last_address_byte: u8, crc_error: bool) -> Vec<u8> {
        // Synthetic beacon, not a captured identity or SSID.
        let mut frame = vec![0; 36];
        frame[0] = 0x80;
        frame[4..10].fill(0xff);
        frame[10..16].copy_from_slice(&[0x02, 0, 0, 0, 0, last_address_byte]);
        frame[16..22].copy_from_slice(&[0x02, 0, 0, 0, 0, last_address_byte]);
        frame.extend_from_slice(&[0, 4, b'T', b'e', b's', b't']);
        let dma_len = (32 + frame.len() + 3) & !3;
        let mut transfer = vec![0; dma_len + 8];
        transfer[..4].copy_from_slice(&(dma_len as u32).to_le_bytes());
        transfer[4..8].copy_from_slice(&(u32::from(crc_error) << 8).to_le_bytes());
        transfer[8..12].copy_from_slice(&((frame.len() as u32) << 16).to_le_bytes());
        transfer[36..36 + frame.len()].copy_from_slice(&frame);
        transfer
    }

    #[test]
    fn empty_file_and_empty_transfers_have_distinct_counts() {
        assert_eq!(replay(Cursor::new([])).unwrap(), ReplaySummary::default());
        let summary = replay(Cursor::new(record(&[]))).unwrap();
        assert_eq!(summary.transfers, 1);
        assert_eq!(summary.bytes, 0);
        assert_eq!(summary.frames, 0);
        assert_eq!(summary.malformed, 0);
    }

    #[test]
    fn counts_aggregates_crc_flags_and_distinct_beacons_without_identifiers() {
        let mut aggregate = beacon(1, false);
        aggregate.extend_from_slice(&beacon(2, false));
        let mut bytes = record(&aggregate);
        bytes.extend(record(&beacon(1, false))); // repeated BSSID, new observation
        bytes.extend(record(&beacon(3, true))); // corrupted beacon isn't trusted
        let summary = replay(Cursor::new(&bytes)).unwrap();
        assert_eq!(summary.transfers, 3);
        assert_eq!(summary.bytes, (bytes.len() - 3 * 4) as u64);
        assert_eq!(summary.frames, 4);
        assert_eq!(summary.beacons, 3);
        assert_eq!(summary.crc_errors, 1);
        assert_eq!(summary.malformed, 0);
        assert_eq!(summary.distinct_bssids, 2);
        let json = serde_json::to_value(&summary).unwrap();
        assert!(json
            .as_object()
            .unwrap()
            .values()
            .all(serde_json::Value::is_u64));
        assert_eq!(json.as_object().unwrap().len(), 7);
    }

    #[test]
    fn malformed_usb_does_not_prevent_replaying_later_records() {
        let mut malformed_crc = beacon(9, true);
        malformed_crc[36] = 0x81; // corrupt MAC protocol version
        let mut bytes = record(&malformed_crc);
        bytes.extend(record(&[1, 2, 3]));
        bytes.extend(record(&beacon(1, false)));
        let summary = replay(Cursor::new(bytes)).unwrap();
        assert_eq!(summary.transfers, 3);
        assert_eq!(summary.malformed, 2);
        assert_eq!(summary.frames, 1);
        assert_eq!(
            summary.crc_errors, 0,
            "CRC statistics count decoded frames only"
        );
        assert_eq!(summary.beacons, 1);
    }

    #[test]
    fn partial_length_or_payload_is_fatal_even_after_good_records() {
        let valid = record(&beacon(1, false));
        for truncated in [vec![1], vec![1, 0], vec![1, 0, 0], vec![2, 0, 0, 0, 1]] {
            let mut bytes = valid.clone();
            bytes.extend_from_slice(&truncated);
            assert_eq!(
                replay(Cursor::new(bytes)).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
    }

    #[test]
    fn oversized_transfer_is_rejected_before_allocating_or_reading_its_payload() {
        let mut reader = Cursor::new((MAX_TRANSFER_BYTES as u32 + 1).to_le_bytes());
        assert_eq!(
            replay(&mut reader).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.position(), 4);
        // The exact maximum is accepted as a record, albeit malformed USB data.
        let summary = replay(Cursor::new(record(&vec![0; MAX_TRANSFER_BYTES]))).unwrap();
        assert_eq!(summary.transfers, 1);
        assert_eq!(summary.malformed, 1);
    }

    #[test]
    fn total_budget_includes_prefixes_and_is_checked_before_payload_read() {
        let bytes = record(&beacon(1, false));
        let exact = bytes.len() as u64;
        assert_eq!(
            replay_with_limit(Cursor::new(&bytes), exact)
                .unwrap()
                .frames,
            1
        );
        let mut reader = Cursor::new(&bytes);
        assert_eq!(
            replay_with_limit(&mut reader, exact - 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.position(), 4, "over-budget payload must not be read");
        let mut reader = Cursor::new([0; 12]); // three empty records cannot evade the limit
        assert_eq!(
            replay_with_limit(&mut reader, 8).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            reader.position(),
            9,
            "only one EOF probe byte exceeds the limit"
        );
    }

    struct FragmentedReader {
        bytes: Cursor<Vec<u8>>,
        interrupted: bool,
    }
    impl Read for FragmentedReader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.interrupted = !self.interrupted;
            if self.interrupted {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = out.len().min(1);
            self.bytes.read(&mut out[..count])
        }
    }

    #[test]
    fn short_reads_and_interruptions_do_not_look_like_truncation() {
        let reader = FragmentedReader {
            bytes: Cursor::new(record(&beacon(1, false))),
            interrupted: false,
        };
        assert_eq!(replay(reader).unwrap().beacons, 1);
    }

    #[test]
    fn reader_failure_retains_its_error_kind() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::PermissionDenied.into())
            }
        }
        assert_eq!(
            replay(Broken).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
