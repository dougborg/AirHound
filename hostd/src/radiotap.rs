//! Bounded extraction of the metadata needed by AirHound.
//! Layout/alignment: https://www.radiotap.org/ and its defined fields.
//! Later fields are skipped using the declared header length.

pub fn events(packet: &[u8]) -> impl Iterator<Item = airhound::scanner::ScanEvent> {
    use airhound::scanner::{self, ScanEvent};
    let events = decode(packet)
        .map(|(frame, rssi, channel)| {
            [
                scanner::try_parse_odid_wifi(frame, rssi, channel).map(ScanEvent::Odid),
                scanner::parse_wifi_frame(frame, rssi, channel).map(ScanEvent::WiFi),
            ]
        })
        .unwrap_or([None, None]);
    events.into_iter().flatten()
}

pub fn decode(packet: &[u8]) -> Option<(&[u8], i8, u8)> {
    if packet.first().copied()? != 0 {
        return None;
    }
    let len = u16::from_le_bytes(packet.get(2..4)?.try_into().ok()?) as usize;
    let header = packet.get(..len)?;
    let present = u32::from_le_bytes(header.get(4..8)?.try_into().ok()?);
    let mut extension = present;
    let mut offset = 8;
    while extension & (1 << 31) != 0 {
        extension = u32::from_le_bytes(header.get(offset..offset + 4)?.try_into().ok()?);
        offset += 4;
    }
    let mut flags = 0;
    let mut channel = None;
    let mut signal = None;
    for (bit, (alignment, size)) in [(8, 8), (1, 1), (1, 1), (2, 4), (2, 2), (1, 1)]
        .into_iter()
        .enumerate()
    {
        if present & (1 << bit) == 0 {
            continue;
        }
        offset = (offset + alignment - 1) & !(alignment - 1);
        let field = header.get(offset..offset + size)?;
        match bit {
            1 => flags = field[0],
            3 => channel = frequency_to_channel(u16::from_le_bytes([field[0], field[1]])),
            5 => signal = Some(field[0] as i8),
            _ => {}
        }
        offset += size;
    }
    // Reject failed CRC and padded payloads rather than misparse their contents.
    if flags & 0x60 != 0 {
        return None;
    }
    let mut frame = packet.get(len..)?;
    if flags & 0x10 != 0 {
        frame = frame.get(..frame.len().checked_sub(4)?)?;
    }
    Some((frame, signal?, channel?))
}

fn frequency_to_channel(freq: u16) -> Option<u8> {
    match freq {
        2484 => Some(14),
        2412..=2472 if (freq - 2407).is_multiple_of(5) => Some(((freq - 2407) / 5) as u8),
        5180..=5885 if (freq - 5000).is_multiple_of(5) => Some(((freq - 5000) / 5) as u8),
        _ => None, // 6 GHz needs an explicit band in the shared event model.
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    fn packet(flags: u8) -> Vec<u8> {
        // TSFT, flags, rate, channel, FHSS, signal; extended presence word.
        let mut p = vec![0, 0, 33, 0, 0x3f, 0, 0, 0x80, 0, 0, 0, 0];
        p.resize(24, 0); // 4 bytes alignment + 8 bytes TSFT
        p.extend_from_slice(&[flags, 12, 0x6c, 0x09, 0x80, 0, 0, 0, (-47i8) as u8]);
        p.extend_from_slice(&[0x80, 0, 1, 2]);
        p
    }
    #[test]
    fn alignment_extensions_signal_and_fcs() {
        let mut p = packet(0x10);
        p.extend_from_slice(&[9; 4]);
        assert_eq!(decode(&p), Some((&[0x80, 0, 1, 2][..], -47, 1)));
        assert_eq!(frequency_to_channel(5180), Some(36));
        assert_eq!(frequency_to_channel(2484), Some(14));
        assert_eq!(frequency_to_channel(5955), None);
    }
    #[test]
    fn captured_remote_id_reaches_shared_event_pipeline() {
        // Same public opendroneid/wireshark-dissector NAN capture as scanner.rs.
        let hex = concat!(
            "d0000000516f9a01000084cca8604324506f9a01017950060409506f9a130327",
            "008869199d92090100101d22f0190150004742522d4f502d3132334142434400",
            "00000000000000000e040001000222",
        );
        let mut p = packet(0x10);
        p.truncate(33);
        p.extend(
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()),
        );
        p.extend([0; 4]);
        let mut output = events(&p);
        let airhound::scanner::ScanEvent::Odid(event) = output.next().unwrap() else {
            panic!("missing Remote ID event")
        };
        assert_eq!(event.rssi, -47);
        assert_eq!(event.mac, [0x84, 0xcc, 0xa8, 0x60, 0x43, 0x24]);
        let frame = airhound::odid::parse_odid_service_info(&event.raw_data).unwrap();
        assert_eq!(
            frame.operator_id.unwrap().operator_id.as_str(),
            "GBR-OP-123ABCD"
        );
        assert!(matches!(
            output.next(),
            Some(airhound::scanner::ScanEvent::WiFi(_))
        ));
        assert!(output.next().is_none());
    }

    #[test]
    fn malformed_or_incomplete_metadata_is_rejected() {
        let p = packet(0);
        for len in 0..33 {
            assert!(decode(&p[..len]).is_none(), "length {len}");
        }
        for flags in [0x20, 0x40] {
            assert!(decode(&packet(flags)).is_none());
        }
        let mut missing = p.clone();
        missing[4] &= !(1 << 5);
        assert!(decode(&missing).is_none());
        let mut version = p;
        version[0] = 1;
        assert!(decode(&version).is_none());
    }
}
