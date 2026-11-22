//! Bounded incremental framing for an explicitly started Marauder serial PCAP.
//! A lost UART chunk invalidates the stream; only a new scan/global header can
//! restart it. Markers inside length-delimited packets are never interpreted.
use crate::marauder::{self, Record};
use std::io;

const BEGIN: &[u8] = b"[BUF/BEGIN]";
const CLOSE: &[u8] = b"[BUF/CLOSE]";
pub const MAX_CHUNK: usize = 256;
const MAX_PACKET: usize = 4096;
const MAX_CONSOLE: usize = 64 * 1024;

#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    global: Option<Vec<u8>>,
    in_chunk: bool,
    console_bytes: usize,
    failed: bool,
}

impl Decoder {
    /// Caller must create a new decoder only at an explicit scan boundary, not
    /// after guessing where packet framing might resume following a gap.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate(&mut self) {
        self.failed = true;
        self.buffer.clear();
    }

    pub fn feed(
        &mut self,
        chunk: &[u8],
        mut visit: impl FnMut(Record<'_>) -> io::Result<()>,
    ) -> io::Result<usize> {
        if self.failed {
            return Err(invalid("Marauder stream requires a fresh scan boundary"));
        }
        let result = self.consume(chunk, &mut visit);
        if result.is_err() {
            self.invalidate();
        }
        result
    }

    fn consume(
        &mut self,
        chunk: &[u8],
        visit: &mut impl FnMut(Record<'_>) -> io::Result<()>,
    ) -> io::Result<usize> {
        if chunk.len() > MAX_CHUNK || self.buffer.len() + chunk.len() > MAX_PACKET + 16 + MAX_CHUNK
        {
            return Err(invalid("Marauder stream buffer limit exceeded"));
        }
        self.buffer.extend_from_slice(chunk);
        let mut emitted = 0;
        loop {
            if !self.in_chunk {
                match self.buffer.windows(BEGIN.len()).position(|w| w == BEGIN) {
                    Some(offset) => {
                        self.console_bytes += offset;
                        if self.console_bytes > MAX_CONSOLE {
                            return Err(invalid("Excessive Marauder console bytes"));
                        }
                        self.buffer.drain(..offset + BEGIN.len());
                        self.console_bytes = 0;
                        self.in_chunk = true;
                    }
                    None => {
                        let discard = self.buffer.len().saturating_sub(BEGIN.len() - 1);
                        self.console_bytes += discard;
                        self.buffer.drain(..discard);
                        if self.console_bytes > MAX_CONSOLE {
                            return Err(invalid("Excessive Marauder console bytes"));
                        }
                        break;
                    }
                }
            }
            if self.global.is_none() {
                if self.buffer.len() < 24 {
                    break;
                }
                let header: Vec<_> = self.buffer.drain(..24).collect();
                // Use the same strict pinned PCAP profile as offline import.
                marauder::decode(&header, false, |_| Ok(()))?;
                self.global = Some(header);
            }
            if self.buffer.starts_with(CLOSE) {
                self.buffer.drain(..CLOSE.len());
                self.in_chunk = false;
                continue;
            }
            if self.buffer.len() < 16 {
                break;
            }
            let included = u32::from_le_bytes(self.buffer[8..12].try_into().unwrap()) as usize;
            if included == 0 || included > MAX_PACKET {
                return Err(invalid("Invalid Marauder packet length"));
            }
            if self.buffer.len() < included + 16 {
                break;
            }
            let mut record = self.global.as_ref().unwrap().clone();
            record.extend(self.buffer.drain(..included + 16));
            emitted += marauder::decode(&record, false, &mut *visit)?;
        }
        Ok(emitted)
    }

    /// Stop does not silently promote a truncated last frame into success.
    pub fn finish(&self) -> io::Result<()> {
        if self.failed || self.global.is_none() || self.in_chunk {
            return Err(invalid("Incomplete Marauder capture stream"));
        }
        if (1..BEGIN.len()).any(|n| self.buffer.ends_with(&BEGIN[..n])) {
            return Err(invalid("Truncated Marauder chunk marker"));
        }
        Ok(())
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut out = b"boot\r\n[BUF/BEGIN]".to_vec();
        out.extend([0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]);
        for n in [0u32, 0, 4096, 105] {
            out.extend(n.to_le_bytes());
        }
        // Delimiters in raw frame bytes cannot create a fake record boundary.
        let frame = b"[BUF/CLOSE][BUF/BEGIN]";
        for n in [4294u32, 967295, frame.len() as u32, frame.len() as u32] {
            out.extend(n.to_le_bytes());
        }
        out.extend(frame);
        out.extend(CLOSE);
        out.extend(b"console\n[BUF/BEGIN]");
        for n in [0u32, 3, 1, 1] {
            out.extend(n.to_le_bytes());
        }
        out.push(0x80);
        out.extend(CLOSE);
        out.extend(b"stopped\r\n");
        out
    }

    #[test]
    fn every_fragment_size_retains_packets_markers_and_original_wrapping_clock() {
        for width in 1..=MAX_CHUNK {
            let mut decoder = Decoder::new();
            let mut records = Vec::new();
            for chunk in fixture().chunks(width) {
                decoder
                    .feed(chunk, |r| {
                        records.push((r.timestamp_micros32, r.frame.to_vec()));
                        Ok(())
                    })
                    .unwrap();
            }
            decoder.finish().unwrap();
            assert_eq!(
                records,
                [
                    (u32::MAX, b"[BUF/CLOSE][BUF/BEGIN]".to_vec()),
                    (3, vec![0x80])
                ]
            );
        }
    }

    #[test]
    fn gaps_invalid_headers_lengths_and_truncation_do_not_resynchronize() {
        let bytes = fixture();
        for cut in [1, 20, 45, 60] {
            let mut decoder = Decoder::new();
            decoder.feed(&bytes[..cut], |_| Ok(())).unwrap();
            decoder.invalidate();
            assert!(decoder
                .feed(&bytes, |_| panic!("packet after loss"))
                .is_err());
            assert!(decoder.finish().is_err());
        }
        let mut decoder = Decoder::new();
        assert!(decoder.feed(&vec![0; MAX_CHUNK + 1], |_| Ok(())).is_err());
        let mut bad = bytes.clone();
        bad[6 + BEGIN.len()] = 0;
        assert!(Decoder::new()
            .feed(&bad, |_| panic!("bad global header"))
            .is_err());
        let mut decoder = Decoder::new();
        decoder
            .feed(&bytes[..bytes.len() - 20], |_| Ok(()))
            .unwrap();
        assert!(decoder.finish().is_err());
    }
}
