//! Independent bounded decoder for LINKTYPE_BLUETOOTH_LE_LL_WITH_PHDR (256).
//! No cached-field merging or speculative extended-chain identity association.
use crate::ble_payload::{Capacity, ManufacturerRecord, ServiceRecord, Snapshot};
use airhound::ble_data::AdStructures;
use serde::Serialize;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_CAPTURE: usize = 4096;
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub struct Reader<R> {
    input: R,
    little: bool,
    nanos: bool,
    snaplen: u32,
}
impl<R: AsyncRead + Unpin> Reader<R> {
    pub async fn new(mut input: R) -> io::Result<Self> {
        let mut b = [0; 24];
        input.read_exact(&mut b).await?;
        let (little, nanos) = match &b[..4] {
            [0xd4, 0xc3, 0xb2, 0xa1] => (true, false),
            [0xa1, 0xb2, 0xc3, 0xd4] => (false, false),
            [0x4d, 0x3c, 0xb2, 0xa1] => (true, true),
            [0xa1, 0xb2, 0x3c, 0x4d] => (false, true),
            _ => return Err(invalid("expected classic PCAP, not PCAPNG")),
        };
        let u16at = |o| {
            if little {
                u16::from_le_bytes([b[o], b[o + 1]])
            } else {
                u16::from_be_bytes([b[o], b[o + 1]])
            }
        };
        let u32at = |o| number(&b[o..o + 4], little);
        if u16at(4) != 2 || u16at(6) != 4 || u32at(20) != 256 {
            return Err(invalid("expected PCAP 2.4 link type 256"));
        }
        let snaplen = u32at(16);
        if snaplen == 0 || snaplen > 16 * 1024 * 1024 {
            return Err(invalid("invalid PCAP snaplen"));
        }
        Ok(Self {
            input,
            little,
            nanos,
            snaplen,
        })
    }
    pub async fn next(&mut self) -> io::Result<Option<Packet>> {
        let mut h = [0; 16];
        if self.input.read(&mut h[..1]).await? == 0 {
            return Ok(None);
        }
        self.input.read_exact(&mut h[1..]).await?;
        let secs = number(&h[..4], self.little);
        let sub = number(&h[4..8], self.little);
        let cap = number(&h[8..12], self.little);
        let orig = number(&h[12..], self.little);
        if cap as usize > MAX_CAPTURE
            || cap > self.snaplen
            || cap > orig
            || sub >= if self.nanos { 1_000_000_000 } else { 1_000_000 }
        {
            return Err(invalid("invalid or oversized PCAP record"));
        }
        let mut data = vec![0; cap as usize];
        self.input.read_exact(&mut data).await?;
        let stamp =
            u64::from(secs) * 1_000_000 + u64::from(if self.nanos { sub / 1000 } else { sub });
        Ok(Some(Packet::decode(stamp, orig, &data)?))
    }
}
fn number(b: &[u8], little: bool) -> u32 {
    let a = b.try_into().expect("four bytes");
    if little {
        u32::from_le_bytes(a)
    } else {
        u32::from_be_bytes(a)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Packet {
    pub source_id: String,
    pub generation: u64,
    pub sequence: u64,
    pub replay: bool,
    #[serde(skip)]
    pub ingestion_generation: u64,
    pub captured_at_us: u64,
    pub captured_len: usize,
    pub original_len: u32,
    pub rf_channel: u8,
    pub channel: u8,
    pub phy: &'static str,
    pub rssi: Option<i8>,
    pub crc: &'static str,
    pub packet_completeness: &'static str,
    pub access_address: u32,
    pub pdu_type: u8,
    pub pdu: Vec<u8>,
    pub address: Option<String>,
    pub address_type: &'static str,
    pub name: String,
    pub advertisement: Option<Snapshot>,
    #[serde(skip)]
    pub mac: Option<[u8; 6]>,
}
impl Packet {
    fn decode(stamp: u64, original_len: u32, b: &[u8]) -> io::Result<Self> {
        if b.len() < 14 {
            return Err(invalid("short BLE pseudo-header"));
        }
        let flags = u16::from_le_bytes([b[8], b[9]]);
        let phybits = flags >> 14;
        let aa = number(&b[10..14], true);
        let ci = if phybits == 2 { 1 } else { 0 };
        if b.len() < 14 + ci + 2 + 3 || b[0] > 39 || phybits == 3 {
            return Err(invalid("invalid BLE packet header"));
        }
        if ci == 1 && b[14] > 1 {
            return Err(invalid("invalid coded PHY indicator"));
        }
        let pdu = b[14 + ci..b.len() - 3].to_vec();
        let complete = b.len() == original_len as usize
            && flags & 1 != 0
            && pdu.len() == usize::from(pdu[1]) + 2;
        let crc = if flags & 0x400 == 0 {
            "unknown"
        } else if flags & 0x800 == 0 {
            "invalid"
        } else {
            "valid"
        };
        let mut packet = Self {
            source_id: String::new(),
            generation: 0,
            sequence: 0,
            replay: false,
            ingestion_generation: 0,
            captured_at_us: stamp,
            captured_len: b.len(),
            original_len,
            rf_channel: b[0],
            channel: match b[0] {
                0 => 37,
                12 => 38,
                39 => 39,
                1..=11 => b[0] - 1,
                _ => b[0] - 2,
            },
            phy: match phybits {
                0 => "1m",
                1 => "2m",
                _ => {
                    if b[14] == 0 {
                        "coded_s8"
                    } else {
                        "coded_s2"
                    }
                }
            },
            rssi: (flags & 2 != 0).then_some(b[1] as i8),
            crc,
            packet_completeness: if complete {
                "complete_packet"
            } else {
                "incomplete_or_not_dewhitened"
            },
            access_address: aa,
            pdu_type: pdu[0] & 15,
            pdu,
            address: None,
            address_type: "unknown",
            name: String::new(),
            advertisement: None,
            mac: None,
        };
        // Advertising channel PDUs only; data/scan requests are packet records, not advertiser observations.
        let category = (flags >> 7) & 7;
        if !complete || crc != "valid" || aa != 0x8e89bed6 || !matches!(category, 0..=2) {
            return Ok(packet);
        }
        let payload = &packet.pdu[2..];
        let (address, ad, fragment) = match packet.pdu_type {
            0 | 2 | 4 | 6 if payload.len() >= 6 && payload.len() <= 37 => {
                (&payload[..6], &payload[6..], false)
            }
            1 if payload.len() == 12 => (&payload[..6], &payload[12..], false),
            7 if !payload.is_empty() => {
                let len = usize::from(payload[0] & 0x3f);
                if len == 0 || payload.len() < len + 1 {
                    packet.packet_completeness = "malformed_extended_header";
                    return Ok(packet);
                }
                let flags = payload[1];
                let mut end = 2;
                for (mask, width) in [(1, 6), (2, 6), (4, 1), (8, 2), (16, 3), (32, 18), (64, 1)] {
                    if flags & mask != 0 {
                        end += width;
                    }
                }
                if flags & 0x80 != 0 || end > len + 1 {
                    packet.packet_completeness = "malformed_extended_header";
                    return Ok(packet);
                }
                if flags & 1 == 0 {
                    packet.packet_completeness = "anonymous_or_unassociated_fragment";
                    return Ok(packet);
                }
                (&payload[2..8], &payload[len + 1..], flags & 16 != 0)
            }
            _ => return Ok(packet),
        };
        let mut mac: [u8; 6] = address
            .try_into()
            .map_err(|_| invalid("short advertiser address"))?;
        mac.reverse();
        if mac == [0; 6] || mac == [255; 6] {
            packet.packet_completeness = "invalid_advertiser_address";
            return Ok(packet);
        }
        packet.mac = Some(mac);
        packet.address = Some(
            mac.iter()
                .map(|n| format!("{n:02X}"))
                .collect::<Vec<_>>()
                .join(":"),
        );
        packet.address_type = if packet.pdu[0] & 0x40 != 0 {
            "random"
        } else {
            "public"
        };
        let (mut snapshot, name) = snapshot(ad, packet.address_type, packet.rssi.is_some());
        if fragment {
            snapshot.completeness = "incomplete";
        }
        packet.name = name;
        packet.packet_completeness = if fragment {
            "extended_chain_incomplete"
        } else if snapshot.completeness != "complete" {
            "malformed_advertisement"
        } else {
            "complete_advertisement"
        };
        packet.advertisement = Some(snapshot);
        Ok(packet)
    }
    pub fn matchable(&self) -> bool {
        self.packet_completeness == "complete_advertisement" && self.rssi.is_some()
    }
}
fn snapshot(ad: &[u8], address_type: &'static str, rssi_available: bool) -> (Snapshot, String) {
    let mut s = Snapshot {
        provenance: "raw_ble_packet",
        freshness: "single_packet",
        completeness: "complete",
        rssi_available,
        address_type,
        receiver_id: None,
        advertised_services: vec![],
        manufacturer_data: vec![],
        service_data: vec![],
        capacity: Capacity::default(),
    };
    let mut name = String::new();
    let mut complete_name = false;
    let mut fields = AdStructures::new(ad);
    for field in fields.by_ref() {
        for uuid in field.uuids() {
            if !s.advertised_services.contains(&uuid) {
                if s.advertised_services.len() < crate::ble_payload::MAX_UUIDS {
                    s.advertised_services.push(uuid)
                } else {
                    s.capacity.omitted_uuids += 1;
                }
            }
        }
        if let Some(m) = field.manufacturer() {
            if s.manufacturer_data.len() < crate::ble_payload::MAX_RECORDS {
                s.manufacturer_data.push(ManufacturerRecord {
                    company_id: m.company_id,
                    payload: m.payload.to_vec(),
                    original_len: m.payload.len(),
                    truncated: false,
                })
            } else {
                s.capacity.omitted_manufacturer_records += 1;
            }
        }
        if let Some(m) = field.service() {
            if s.service_data.len() < crate::ble_payload::MAX_RECORDS {
                s.service_data.push(ServiceRecord {
                    uuid: m.uuid,
                    payload: m.payload.to_vec(),
                    original_len: m.payload.len(),
                    truncated: false,
                })
            } else {
                s.capacity.omitted_service_records += 1;
            }
        }
        if (field.kind == 9 && !complete_name) || (field.kind == 8 && name.is_empty()) {
            if let Ok(value) = std::str::from_utf8(field.data) {
                name = value.to_owned();
                complete_name = field.kind == 9;
            }
        }
    }
    if fields.malformed()
        || fields.incomplete()
        || s.capacity.omitted_uuids
            + s.capacity.omitted_manufacturer_records
            + s.capacity.omitted_service_records
            > 0
    {
        s.completeness = "incomplete";
    }
    (s, name)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;
    pub fn frame(header: u8, ad: &[u8]) -> Vec<u8> {
        let mut b = vec![
            0,
            (-45i8) as u8,
            128,
            0,
            0xd6,
            0xbe,
            0x89,
            0x8e,
            0x13,
            0x0c,
            0xd6,
            0xbe,
            0x89,
            0x8e,
            header,
            (6 + ad.len()) as u8,
        ];
        b.extend([6, 5, 4, 3, 2, 0]);
        b.extend(ad);
        b.extend([0; 3]);
        b
    }
    pub fn pcap(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        b.extend(65535u32.to_le_bytes());
        b.extend(256u32.to_le_bytes());
        for f in frames {
            for n in [1_700_000_000u32, 123456, f.len() as u32, f.len() as u32] {
                b.extend(n.to_le_bytes());
            }
            b.extend(f);
        }
        b
    }
    pub fn packet(ad: &[u8]) -> Packet {
        let f = frame(0, ad);
        Packet::decode(1_700_000_000_123_456, f.len() as u32, &f).unwrap()
    }
    #[tokio::test]
    async fn replay_preserves_order_times_and_eof() {
        let bytes = pcap(&[frame(0, &[2, 1, 6]), frame(0x40, &[])]);
        let mut r = Reader::new(bytes.as_slice()).await.unwrap();
        let p = r.next().await.unwrap().unwrap();
        assert_eq!(p.address.as_deref(), Some("00:02:03:04:05:06"));
        assert_eq!(p.captured_at_us, 1_700_000_000_123_456);
        assert_eq!(p.address_type, "public");
        assert!(p.matchable());
        assert_eq!(r.next().await.unwrap().unwrap().address_type, "random");
        assert!(r.next().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn endian_and_nanosecond_variants() {
        for little in [false, true] {
            for nanos in [false, true] {
                let mut b = pcap(&[frame(0, &[])]);
                if nanos {
                    b[..4].copy_from_slice(&[0x4d, 0x3c, 0xb2, 0xa1]);
                    b[28..32].copy_from_slice(&123456789u32.to_le_bytes());
                }
                if !little {
                    b[..4].reverse();
                    b[4..6].reverse();
                    b[6..8].reverse();
                    for at in [8, 12, 16, 20, 24, 28, 32, 36] {
                        b[at..at + 4].reverse();
                    }
                }
                let mut r = Reader::new(b.as_slice()).await.unwrap();
                assert_eq!(
                    r.next().await.unwrap().unwrap().captured_at_us,
                    1_700_000_000_123_456
                );
            }
        }
    }
    #[tokio::test]
    async fn rejects_bad_headers_lengths_and_partial_records() {
        for at in [0, 4, 6, 20] {
            let mut b = pcap(&[]);
            b[at] ^= 0xff;
            assert!(Reader::new(b.as_slice()).await.is_err());
        }
        for size in [0u32, 16 * 1024 * 1024 + 1] {
            let mut b = pcap(&[]);
            b[16..20].copy_from_slice(&size.to_le_bytes());
            assert!(Reader::new(b.as_slice()).await.is_err());
        }
        for (at, value) in [(28, 1_000_000u32), (32, 4097), (32, 65536), (36, 0)] {
            let mut b = pcap(&[frame(0, &[])]);
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
            let mut r = Reader::new(b.as_slice()).await.unwrap();
            assert!(r.next().await.is_err());
        }
        let full = pcap(&[frame(0, &[])]);
        for len in [0, 23, 25, 39, full.len() - 1] {
            let b = &full[..len];
            if len < 24 {
                assert!(Reader::new(b).await.is_err());
            } else {
                let mut r = Reader::new(b).await.unwrap();
                assert!(r.next().await.is_err());
            }
        }
    }
    #[test]
    fn typed_fields_keep_company_zero_duplicates_and_full_uuids() {
        let mut ad = vec![
            4, 9, b'O', b'n', b'e', 4, 9, b'T', b'w', b'o', 3, 0xff, 0, 0, 4, 0xff, 0, 0, 7, 3, 3,
            0x00, 0x31, 5, 0x20, 0x78, 0x56, 0x34, 0x12,
        ];
        let (s, name) = snapshot(&ad, "public", true);
        assert_eq!(name, "One");
        assert_eq!(s.manufacturer_data.len(), 2);
        assert_eq!(s.manufacturer_data[0].company_id, 0);
        assert_eq!(
            s.service_data[0].uuid,
            airhound::ble_data::Uuid::from_u32(0x12345678)
        );
        assert_eq!(s.advertised_services[0].as_u16(), Some(0x3100));
        ad.extend([5, 9, 1]);
        assert_eq!(snapshot(&ad, "public", true).0.completeness, "incomplete");
        assert!(!packet(&[2, 0xff, 1]).matchable());
        let (s, _) = snapshot(&[3, 8, b'H', b'i', 2, 9, 0xff], "random", false);
        assert!(!s.rssi_available);
        let excessive = [3, 0xff, 1, 0].repeat(17);
        assert_eq!(
            snapshot(&excessive, "public", true)
                .0
                .capacity
                .omitted_manufacturer_records,
            1
        );
    }
    #[test]
    fn rejects_false_advertiser_evidence_and_retains_raw_packets() {
        for flag in [0x0013u16, 0x0413, 0x0c12] {
            let mut b = frame(0, &[]);
            b[8..10].copy_from_slice(&flag.to_le_bytes());
            let p = Packet::decode(0, b.len() as u32, &b).unwrap();
            assert!(!p.matchable());
            assert!(p.advertisement.is_none());
        }
        let mut b = frame(0, &[]);
        b[8] &= !2;
        let p = Packet::decode(0, b.len() as u32, &b).unwrap();
        assert!(p.rssi.is_none());
        assert!(!p.matchable());
        for kind in [3, 5, 8, 15] {
            let b = frame(kind, &[]);
            let p = Packet::decode(0, b.len() as u32, &b).unwrap();
            assert!(p.mac.is_none());
        }
        let mut b = frame(0, &[]);
        b[10] = 0;
        assert!(Packet::decode(0, b.len() as u32, &b).unwrap().mac.is_none());
        let mut b = frame(0, &[]);
        b[15] += 1;
        assert!(!Packet::decode(0, b.len() as u32, &b).unwrap().matchable());
        let b = frame(0, &[]);
        assert!(!Packet::decode(0, b.len() as u32 + 1, &b)
            .unwrap()
            .matchable());
        for len in [0, 13, 15, 18] {
            assert!(Packet::decode(0, len, &b[..len as usize]).is_err());
        }
    }
    #[test]
    fn extended_fragments_coded_and_malformed_headers() {
        let make = |payload: &[u8], coded: bool| {
            let mut b = frame(7, &[]);
            b.truncate(14);
            if coded {
                b[9] |= 0x80;
                b.push(0);
            }
            b.extend([7, payload.len() as u8]);
            b.extend(payload);
            b.extend([0; 3]);
            b
        };
        let complete = [7, 1, 6, 5, 4, 3, 2, 0, 2, 1, 6];
        let b = make(&complete, true);
        let p = Packet::decode(0, b.len() as u32, &b).unwrap();
        assert_eq!(p.phy, "coded_s8");
        assert!(p.matchable());
        let b = make(&[10, 17, 6, 5, 4, 3, 2, 0, 0, 0, 0, 2, 1, 6], false);
        assert_eq!(
            Packet::decode(0, b.len() as u32, &b)
                .unwrap()
                .packet_completeness,
            "extended_chain_incomplete"
        );
        for payload in [&[0u8][..], &[1, 1], &[1, 128]] {
            let b = make(payload, false);
            assert_eq!(
                Packet::decode(0, b.len() as u32, &b)
                    .unwrap()
                    .packet_completeness,
                "malformed_extended_header"
            );
        }
        let b = make(&[1, 0], false);
        assert_eq!(
            Packet::decode(0, b.len() as u32, &b)
                .unwrap()
                .packet_completeness,
            "anonymous_or_unassociated_fragment"
        );
        let mut b = make(&complete, true);
        b[14] = 2;
        assert!(Packet::decode(0, b.len() as u32, &b).is_err());
    }
}
