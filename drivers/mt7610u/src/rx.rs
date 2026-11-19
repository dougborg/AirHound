// SPDX-License-Identifier: GPL-2.0-only
// USB segment decoding adapted from Linux mt76x0/dma.c and dma.h (v4.19):
// Copyright (C) 2014 Felix Fietkau <nbd@openwrt.org>
// Copyright (C) 2015 Jakub Kicinski <kubakici@wp.pl>
// Copyright (C) 2018 Stanislaw Gruszka <stf_xl@wp.pl>
// Current mt76x02 descriptor/PN handling also credits:
// Copyright (C) 2016 Felix Fietkau <nbd@nbd.name>
// Copyright (C) 2018 Stanislaw Gruszka <stf_xl@wp.pl>
// Rust implementation and validation Copyright (C) 2026 AirHound contributors.
//
// Sources:
// https://github.com/torvalds/linux/blob/v4.19/drivers/net/wireless/mediatek/mt76/mt76x0/dma.c
// https://github.com/torvalds/linux/blob/v4.19/drivers/net/wireless/mediatek/mt76/mt76x0/dma.h
// https://github.com/openwrt/mt76/blob/22f7ff9be59890cb10cde16cc25a30f78b504f23/mt76x02_mac.h
// https://github.com/openwrt/mt76/blob/22f7ff9be59890cb10cde16cc25a30f78b504f23/mt76x02_mac.c
//
//! Decode packet-endpoint USB transfers; this does not handle MCU responses.
//!
//! Each segment is a 4-byte DMA header, `dma_len` bytes of RXWI + padded MPDU,
//! and a 4-byte FCE trailer. `dma_len` is the header's little-endian low u16,
//! must be a multiple of four, and excludes both outer words. RXWI occupies
//! 32 bytes, including its initial RXINFO word; the MPDU starts at offset 36
//! from the segment start. Repeated segments form an aggregate. Current mt76x0
//! disables USB aggregation, but the older driver's framing supports both.
//!
//! RXWI's MPDU length excludes alignment padding and optional two-byte L2
//! padding immediately after the 802.11 header. Non-fragmented hardware PN
//! fields are removed like mt76x02_mac_process_rx. No extra four bytes are
//! guessed to be an 802.11 FCS: upstream passes this MPDU length to mac80211
//! without RX_INCLUDES_FCS. The trailing USB FCE word is NOT an 802.11 FCS.
//! Structurally valid CRC-error frames remain available as bytes but never
//! produce beacon metadata. CRC-corrupt headers/lengths still fail validation.
//!
//! RSSI bytes require EEPROM/band/LNA calibration, so `rssi_dbm` is deliberately
//! absent. A beacon's advertised channel is not the receiver's tuned channel.

use std::fmt;

const DMA_HEADER_LEN: usize = 4;
const FCE_LEN: usize = 4;
const RXWI_LEN: usize = 32;
const RXINFO_CRC_ERROR: u32 = 1 << 8;
const RXINFO_FRAGMENT: u32 = 1 << 3;
const RXINFO_L2PAD: u32 = 1 << 14;
const RXINFO_PN_LEN_MASK: u32 = 7 << 19;
const RXWI_MPDU_LEN_MASK: u32 = 0x3fff << 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFrame {
    /// MPDU after removing descriptor/alignment/L2/stripped-PN padding.
    /// FCS bytes are not synthesized or heuristically removed.
    pub bytes: Vec<u8>,
    pub rx_info: u32,
    /// Descriptor bytes, NOT calibrated dBm values (nor necessarily all valid).
    pub raw_rssi: [u8; 4],
    /// Requires radio calibration context that this parser does not have.
    pub rssi_dbm: Option<i8>,
    pub fcs_error: bool,
    /// Present only for a structurally valid, unfragmented, unprotected beacon.
    pub beacon: Option<Beacon>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    pub bssid: [u8; 6],
    /// None means no SSID IE; an empty vector is a hidden SSID. Bytes may not
    /// be UTF-8 and must be escaped/lossily displayed only at the UI boundary.
    pub ssid: Option<Vec<u8>>,
    /// Advertised primary channel from consistent DS/HT Operation elements.
    /// None means absent, invalid, or conflicting; never a tuned-channel guess.
    pub channel: Option<u8>,
    pub beacon_interval_tu: u16,
    pub capability_info: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset of the invalid USB segment (or trailing bytes).
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid MT7610U RX segment at byte {}: {}",
            self.offset, self.reason
        )
    }
}

impl std::error::Error for ParseError {}

fn error(offset: usize, reason: &'static str) -> ParseError {
    ParseError { offset, reason }
}

/// Parse one completed USB transfer, including multiple concatenated segments.
///
/// Empty transfers and one optional zero padding word after the last complete
/// segment are accepted. Malformed/truncated segments fail the whole transfer;
/// this parser never searches arbitrary bytes for a plausible next descriptor.
pub fn parse_usb_transfer(transfer: &[u8]) -> Result<Vec<ReceivedFrame>, ParseError> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < transfer.len() {
        let remaining = &transfer[offset..];
        if !frames.is_empty() && remaining == [0; 4] {
            break;
        }
        if remaining.len() < DMA_HEADER_LEN {
            return Err(error(offset, "truncated DMA header"));
        }
        let dma_len = usize::from(u16::from_le_bytes([remaining[0], remaining[1]]));
        if dma_len < RXWI_LEN || dma_len % 4 != 0 {
            return Err(error(
                offset,
                "DMA length is too small or not four-byte aligned",
            ));
        }
        // dma_len is u16, so this addition cannot overflow usize.
        let segment_len = DMA_HEADER_LEN + dma_len + FCE_LEN;
        if remaining.len() < segment_len {
            return Err(error(offset, "truncated DMA payload or FCE trailer"));
        }
        let fce = le32(&remaining[DMA_HEADER_LEN + dma_len..segment_len]);
        if fce >> 30 != 0 {
            return Err(error(offset, "non-packet FCE type on packet endpoint"));
        }
        let descriptor = &remaining[DMA_HEADER_LEN..DMA_HEADER_LEN + RXWI_LEN];
        let rx_info = le32(descriptor);
        let control = le32(&descriptor[4..]);
        let mpdu_len = ((control & RXWI_MPDU_LEN_MASK) >> 16) as usize;
        let payload = &remaining[DMA_HEADER_LEN + RXWI_LEN..DMA_HEADER_LEN + dma_len];
        let l2pad = if rx_info & RXINFO_L2PAD != 0 { 2 } else { 0 };
        if mpdu_len < 10 || mpdu_len + l2pad > payload.len() {
            return Err(error(
                offset,
                "MPDU length exceeds payload or is shorter than a MAC header",
            ));
        }
        let header_len = header_length(payload)
            .ok_or_else(|| error(offset, "invalid or unsupported 802.11 header"))?;
        if header_len > mpdu_len {
            return Err(error(
                offset,
                "MPDU does not contain its complete MAC header",
            ));
        }
        let pn_len = ((rx_info & RXINFO_PN_LEN_MASK) >> 19) as usize * 4;
        if pn_len != 0 && (header_len + 8 > mpdu_len || header_len + pn_len > mpdu_len) {
            return Err(error(offset, "truncated hardware PN field"));
        }
        let stripped_pn = if rx_info & RXINFO_FRAGMENT == 0 {
            pn_len
        } else {
            0
        };
        if header_len + stripped_pn > mpdu_len {
            return Err(error(offset, "hardware PN length exceeds MPDU body"));
        }
        let mut bytes = Vec::with_capacity(mpdu_len - stripped_pn);
        bytes.extend_from_slice(&payload[..header_len]);
        bytes.extend_from_slice(&payload[header_len + l2pad + stripped_pn..mpdu_len + l2pad]);
        let fcs_error = rx_info & RXINFO_CRC_ERROR != 0;
        let beacon = if fcs_error || rx_info & RXINFO_FRAGMENT != 0 || pn_len != 0 {
            None
        } else {
            parse_beacon(&bytes)
        };
        frames.push(ReceivedFrame {
            bytes,
            rx_info,
            raw_rssi: descriptor[12..16]
                .try_into()
                .expect("fixed RXWI length checked"),
            rssi_dbm: None,
            fcs_error,
            beacon,
        });
        offset += segment_len;
    }
    Ok(frames)
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Header lengths for protocol-version-zero management, control, and data.
/// Reject extension frames rather than guessing where their L2 padding lies.
fn header_length(bytes: &[u8]) -> Option<usize> {
    let fc = u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]);
    if fc & 3 != 0 {
        return None;
    }
    let len = match fc & 0x0c {
        0 => 24,
        4 => {
            if fc & 0x00e0 == 0x00c0 {
                10
            } else {
                16
            }
        }
        8 => {
            let mut len = if fc & 0x0300 == 0x0300 { 30 } else { 24 };
            if fc & 0x0080 != 0 {
                len += 2;
                if fc & 0x8000 != 0 {
                    len += 4;
                }
            }
            len
        }
        _ => return None,
    };
    (bytes.len() >= len).then_some(len)
}

/// Extract only trustworthy beacon metadata. Malformed IEs invalidate metadata,
/// not the enclosing USB descriptor; raw bytes remain in `ReceivedFrame`.
pub fn parse_beacon(bytes: &[u8]) -> Option<Beacon> {
    if bytes.len() < 36 {
        return None;
    }
    let fc = u16::from_le_bytes([bytes[0], bytes[1]]);
    // Beacon subtype, version 0, no DS bits, fragments, protection or HT control.
    if fc & 0x00ff != 0x0080 || fc & 0xc700 != 0 || bytes[22] & 0x0f != 0 {
        return None;
    }
    let bssid: [u8; 6] = bytes[16..22].try_into().ok()?;
    if bssid == [0; 6] || bssid[0] & 1 != 0 {
        return None;
    }
    let mut ssid = None;
    let mut advertised_channel = None;
    let mut channel_conflict = false;
    let mut ies = &bytes[36..];
    while !ies.is_empty() {
        if ies.len() < 2 {
            return None;
        }
        let id = ies[0];
        let len = usize::from(ies[1]);
        let value = ies.get(2..2 + len)?;
        match id {
            0 => {
                if len > 32 {
                    return None;
                }
                if ssid.as_deref().is_some_and(|previous| previous != value) {
                    return None;
                }
                ssid = Some(value.to_vec());
            }
            3 | 61 => {
                if (id == 3 && len != 1) || (id == 61 && len != 22) {
                    return None;
                }
                // MT7610U is a 2.4/5 GHz part; no 6 GHz channel inference.
                let channel = value[0];
                if channel == 0 || channel > 196 {
                    channel_conflict = true;
                }
                if advertised_channel.is_some_and(|previous| previous != channel) {
                    channel_conflict = true;
                }
                advertised_channel = Some(channel);
            }
            _ => {}
        }
        ies = &ies[2 + len..];
    }
    Some(Beacon {
        bssid,
        ssid,
        channel: if channel_conflict {
            None
        } else {
            advertised_channel
        },
        beacon_interval_tu: u16::from_le_bytes([bytes[32], bytes[33]]),
        capability_info: u16::from_le_bytes([bytes[34], bytes[35]]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beacon() -> Vec<u8> {
        let mut bytes = vec![0; 36];
        bytes[0] = 0x80;
        bytes[4..10].fill(0xff);
        bytes[10..16].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]);
        bytes[16..22].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]);
        bytes[32..34].copy_from_slice(&100u16.to_le_bytes());
        bytes[34..36].copy_from_slice(&0x0431u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 4, b'T', b'e', b's', b't', 3, 1, 6]);
        bytes
    }

    fn segment(mpdu: &[u8], rx_info: u32) -> Vec<u8> {
        let l2pad = usize::from(rx_info & RXINFO_L2PAD != 0) * 2;
        let header_len = header_length(mpdu).unwrap();
        let dma_len = (RXWI_LEN + mpdu.len() + l2pad + 3) & !3;
        let mut bytes = vec![0xa5; DMA_HEADER_LEN + dma_len + FCE_LEN];
        bytes[..4].copy_from_slice(&(dma_len as u32).to_le_bytes());
        bytes[4..36].fill(0);
        bytes[4..8].copy_from_slice(&rx_info.to_le_bytes());
        bytes[8..12].copy_from_slice(&((mpdu.len() as u32) << 16).to_le_bytes());
        bytes[16..20].copy_from_slice(&[0xba, 0xbc, 0xbd, 0xbe]);
        bytes[36..36 + header_len].copy_from_slice(&mpdu[..header_len]);
        bytes[36 + header_len + l2pad..36 + mpdu.len() + l2pad]
            .copy_from_slice(&mpdu[header_len..]);
        // Nonzero packet FCE makes accidental inclusion as an IE observable.
        bytes[DMA_HEADER_LEN + dma_len..].copy_from_slice(&0x1808_0010u32.to_le_bytes());
        bytes
    }

    #[test]
    fn known_beacon_strips_dma_padding_and_fce_without_inventing_rssi() {
        let mpdu = beacon();
        let frames = parse_usb_transfer(&segment(&mpdu, 0)).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].bytes, mpdu);
        assert_eq!(frames[0].raw_rssi, [0xba, 0xbc, 0xbd, 0xbe]);
        assert_eq!(frames[0].rssi_dbm, None);
        assert!(!frames[0].fcs_error);
        let parsed = frames[0].beacon.as_ref().unwrap();
        assert_eq!(parsed.bssid, [0x02, 1, 2, 3, 4, 5]);
        assert_eq!(parsed.ssid.as_deref(), Some(b"Test".as_slice()));
        assert_eq!(parsed.channel, Some(6));
        assert_eq!(parsed.beacon_interval_tu, 100);
        assert_eq!(parsed.capability_info, 0x0431);
    }

    #[test]
    fn aggregates_use_dma_length_plus_both_outer_words() {
        let first = segment(&beacon(), 0);
        let mut second_mpdu = beacon();
        second_mpdu[21] = 9;
        let mut bytes = first;
        bytes.extend_from_slice(&segment(&second_mpdu, RXINFO_L2PAD));
        bytes.extend_from_slice(&[0; 4]);
        let frames = parse_usb_transfer(&bytes).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].bytes, second_mpdu);
        assert_eq!(frames[1].beacon.as_ref().unwrap().bssid[5], 9);
        assert!(parse_usb_transfer(&[]).unwrap().is_empty());
    }

    #[test]
    fn every_truncated_segment_prefix_is_rejected() {
        let bytes = segment(&beacon(), RXINFO_L2PAD);
        for len in 1..bytes.len() {
            assert!(parse_usb_transfer(&bytes[..len]).is_err(), "prefix {len}");
        }
        let mut aggregate = bytes.clone();
        aggregate.extend_from_slice(&bytes[..bytes.len() - 1]);
        assert_eq!(
            parse_usb_transfer(&aggregate).unwrap_err().offset,
            bytes.len()
        );
    }

    #[test]
    fn invalid_dma_lengths_and_non_packet_trailers_are_rejected() {
        for len in [0u16, 1, 28, 33, u16::MAX] {
            let mut bytes = segment(&beacon(), 0);
            bytes[..2].copy_from_slice(&len.to_le_bytes());
            assert!(parse_usb_transfer(&bytes).is_err());
        }
        let mut bytes = segment(&beacon(), 0);
        let end = bytes.len();
        bytes[end - 4..].copy_from_slice(&(1u32 << 30).to_le_bytes());
        assert!(parse_usb_transfer(&bytes).is_err());
    }

    #[test]
    fn mpdu_cannot_extend_into_fce_or_claim_a_partial_header() {
        for len in [0u32, 9, 20, 0x3fff] {
            let mut bytes = segment(&beacon(), 0);
            bytes[8..12].copy_from_slice(&(len << 16).to_le_bytes());
            assert!(parse_usb_transfer(&bytes).is_err());
        }
        let mut bytes = segment(&beacon(), 0);
        bytes.push(1);
        assert!(parse_usb_transfer(&bytes).is_err());
    }

    #[test]
    fn crc_error_protected_or_fragmented_beacons_are_not_trusted() {
        for flag in [RXINFO_CRC_ERROR, RXINFO_FRAGMENT] {
            let frames = parse_usb_transfer(&segment(&beacon(), flag)).unwrap();
            assert!(frames[0].beacon.is_none());
            assert_eq!(frames[0].fcs_error, flag == RXINFO_CRC_ERROR);
        }
        for flag in [0x04, 0x40, 0x80] {
            let mut mpdu = beacon();
            mpdu[1] = flag;
            assert!(parse_beacon(&mpdu).is_none());
        }
        let mut mpdu = beacon();
        mpdu[22] = 1;
        assert!(parse_beacon(&mpdu).is_none());
    }

    #[test]
    fn hidden_binary_and_missing_ssids_are_distinct() {
        for ssid in [vec![], vec![0xff, 0, 0x80]] {
            let mut mpdu = beacon()[..36].to_vec();
            mpdu.extend_from_slice(&[0, ssid.len() as u8]);
            mpdu.extend_from_slice(&ssid);
            let parsed = parse_beacon(&mpdu).unwrap();
            assert_eq!(parsed.ssid, Some(ssid));
            assert_eq!(parsed.channel, None);
        }
        assert_eq!(parse_beacon(&beacon()[..36]).unwrap().ssid, None);
    }

    #[test]
    fn malformed_or_conflicting_ies_are_not_silently_accepted() {
        for suffix in [
            vec![0],
            vec![1, 3, 1],
            vec![0, 33],
            vec![3, 0],
            vec![61, 1, 6],
        ] {
            let mut mpdu = beacon();
            mpdu.extend_from_slice(&suffix);
            assert!(parse_beacon(&mpdu).is_none());
        }
        let mut mpdu = beacon();
        mpdu.extend_from_slice(&[0, 1, b'X']);
        assert!(parse_beacon(&mpdu).is_none());
        let mut mpdu = beacon();
        mpdu.extend_from_slice(&[61, 22, 11]);
        mpdu.extend_from_slice(&[0; 21]);
        assert_eq!(parse_beacon(&mpdu).unwrap().channel, None);
    }

    #[test]
    fn ht_primary_channel_is_used_only_when_consistent_and_in_range() {
        let mut mpdu = beacon()[..42].to_vec(); // SSID present, no DS element.
        mpdu.extend_from_slice(&[61, 22, 36]);
        mpdu.extend_from_slice(&[0; 21]);
        assert_eq!(parse_beacon(&mpdu).unwrap().channel, Some(36));
        mpdu[44] = 0;
        assert_eq!(parse_beacon(&mpdu).unwrap().channel, None);
    }

    #[test]
    fn control_and_four_address_qos_headers_preserve_l2_padding_boundaries() {
        let mut ack = vec![0; 10];
        ack[0] = 0xd4;
        assert_eq!(parse_usb_transfer(&segment(&ack, 0)).unwrap()[0].bytes, ack);
        let mut qos = vec![0; 36];
        qos[0] = 0x88;
        qos[1] = 0x83; // To/From DS, QoS + HT control => 36-byte header.
        qos.extend_from_slice(&[1, 2, 3, 4, 5]);
        let frames = parse_usb_transfer(&segment(&qos, RXINFO_L2PAD)).unwrap();
        assert_eq!(frames[0].bytes, qos);
        assert!(frames[0].beacon.is_none());
    }

    #[test]
    fn hardware_pn_is_removed_only_for_nonfragmented_frames() {
        let mut data = vec![0; 24];
        data[0] = 0x08;
        data.extend_from_slice(&[9; 8]);
        data.extend_from_slice(&[1, 2, 3]);
        let pn_and_pad = (2 << 19) | RXINFO_L2PAD;
        let frame = parse_usb_transfer(&segment(&data, pn_and_pad))
            .unwrap()
            .remove(0);
        assert_eq!(&frame.bytes[24..], &[1, 2, 3]);
        let frame = parse_usb_transfer(&segment(&data, pn_and_pad | RXINFO_FRAGMENT))
            .unwrap()
            .remove(0);
        assert_eq!(frame.bytes, data);
    }

    #[test]
    fn truncated_pn_fields_and_unknown_mac_headers_are_rejected() {
        let mut data = vec![0; 28];
        data[0] = 0x08;
        for flags in [2 << 19, (7 << 19) | RXINFO_FRAGMENT] {
            assert!(parse_usb_transfer(&segment(&data, flags)).is_err());
        }
        for invalid_fc in [0x81, 0x0c] {
            let mut bytes = segment(&beacon(), 0);
            bytes[36] = invalid_fc;
            assert!(parse_usb_transfer(&bytes).is_err());
        }
    }

    #[test]
    fn invalid_bssids_and_non_beacons_do_not_create_access_points() {
        for bssid in [[0; 6], [0xff; 6], [1, 2, 3, 4, 5, 6]] {
            let mut bytes = beacon();
            bytes[16..22].copy_from_slice(&bssid);
            assert!(parse_beacon(&bytes).is_none());
        }
        let mut bytes = beacon();
        bytes[0] = 0x50; // Probe response is not claimed as a beacon.
        assert!(parse_beacon(&bytes).is_none());
    }

    #[test]
    fn captured_crc_corruption_is_rejected_without_weakening_header_validation() {
        // Sanitized descriptor/header shapes from the first MT7610U capture:
        // 33 of 2449 transfers failed structural parsing, all with CRCERR.
        // Frame addresses and bodies are replaced with zeros; no radio identity
        // or SSID is embedded in this fixture. DMA header/FCE bit 19 is real.
        let captured = |rx_info: u32, fc: u16| {
            let mut bytes = vec![0; 56];
            bytes[..4].copy_from_slice(&0x0008_0030u32.to_le_bytes());
            bytes[4..8].copy_from_slice(&rx_info.to_le_bytes());
            bytes[8..12].copy_from_slice(&0x0010_a4ffu32.to_le_bytes());
            bytes[36..38].copy_from_slice(&fc.to_le_bytes());
            bytes[52..].copy_from_slice(&0x0008_0030u32.to_le_bytes());
            bytes
        };
        // Corrupted data header claims four addresses in a 16-byte MPDU.
        let err = parse_usb_transfer(&captured(0x0000_0122, 0x03b8)).unwrap_err();
        assert_eq!(err.reason, "invalid or unsupported 802.11 header");
        // Corrupted descriptor marks L2PAD, but physical payload is only 16B.
        let err = parse_usb_transfer(&captured(0x0000_612a, 0xb728)).unwrap_err();
        assert!(err.reason.starts_with("MPDU length exceeds payload"));
        // A structurally intact RTS is retained even with CRCERR, as raw bytes.
        let frame = parse_usb_transfer(&captured(0x0000_2100, 0x00b4))
            .unwrap()
            .remove(0);
        assert!(frame.fcs_error);
        assert_eq!(frame.bytes.len(), 16);
        assert!(frame.beacon.is_none());
    }

    #[test]
    fn captured_64_byte_boundary_zero_word_is_not_a_second_segment() {
        // Hardware appended a zero word on 24 captured transfers whose normal
        // DMA+FCE segment size was divisible by 64 (not an extra FCS word).
        let mut data = vec![0; 24];
        data[0] = 0x08;
        let mut transfer = segment(&data, 0);
        assert_eq!(transfer.len(), 64);
        transfer.extend_from_slice(&[0; 4]);
        let frames = parse_usb_transfer(&transfer).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].bytes, data);
    }
}
