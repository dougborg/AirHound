// SPDX-License-Identifier: GPL-2.0-only
// Formula and descriptor validity rules derived from mt76x02_mac.c and
// mt76x0/eeprom.c at mt76 revision 22f7ff9be59890cb10cde16cc25a30f78b504f23.
// Copyright (C) 2016 Felix Fietkau <nbd@nbd.name>
// Copyright (C) 2018 Stanislaw Gruszka <stf_xl@wp.pl>
// See THIRD_PARTY_NOTICES for upstream terms and additional attributions.

//! EEPROM-derived signal estimates, not independently validated physical dBm.
//!
//! MT7610U uses chain zero. Other descriptor bytes must not be combined with it.
//! A zero raw byte is conservatively treated as unavailable here; upstream does
//! not explicitly reject zero. This policy avoids presenting an absent sample
//! as a strong signal. Ordinary beacons need no RSSI-valid flag; for A-MPDU
//! subframes the flag identifies the subframe with valid RSSI information.

use crate::radio::RxCalibration;

const RXINFO_RSSI: u32 = 1 << 13;
const RXINFO_AMPDU: u32 = 1 << 15;

/// Compatibility estimator using the 2.4 GHz EEPROM offsets.
///
/// Use `estimate_rssi_for_channel` for capture that can tune either band.
/// Arithmetic is widened before conversion: overflow returns `None`, never a
/// wrapped or clamped signal. Full radio calibration remains a separate status.
pub fn estimate_rssi(raw: [u8; 4], rx_info: u32, cal: &RxCalibration) -> Option<i8> {
    estimate_with_gains(raw, rx_info, cal.lna_gain, cal.rssi_offset)
}

/// Apply the configured receive band's EEPROM correction; reject unsupported
/// channels/bands. This remains an unvalidated estimate, not calibrated dBm.
pub fn estimate_rssi_for_channel(
    raw: [u8; 4],
    rx_info: u32,
    cal: &RxCalibration,
    channel: u8,
) -> Option<i8> {
    let (gain, offset) = cal.gains_for_channel(channel).ok()?;
    estimate_with_gains(raw, rx_info, gain, offset)
}

fn estimate_with_gains(raw: [u8; 4], rx_info: u32, lna_gain: i8, rssi_offset: i8) -> Option<i8> {
    if raw[0] == 0 || (rx_info & RXINFO_AMPDU != 0 && rx_info & RXINFO_RSSI == 0) {
        return None;
    }
    let estimate = i16::from(raw[0] as i8) + i16::from(rssi_offset) - i16::from(lna_gain);
    i8::try_from(estimate).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibration(offset: i8, gain: i8) -> RxCalibration {
        RxCalibration {
            frequency_offset: 0,
            lna_gain: gain,
            rssi_offset: offset,
            antenna_config: 0,
            config1: 0,
            nic_config2: 0,
            has_5ghz: false,
            has_2ghz: true,
            lna_gain_5ghz: 0,
            rssi_offset_5ghz: 0,
        }
    }

    #[test]
    fn channel_estimate_selects_band_and_rejects_unsupported_channels() {
        let mut cal = calibration(-2, 12);
        cal.has_5ghz = true;
        cal.lna_gain_5ghz = 7;
        cal.rssi_offset_5ghz = 3;
        let raw = [0xba, 0, 0, 0];
        assert_eq!(estimate_rssi_for_channel(raw, 0, &cal, 1), Some(-84));
        for channel in [36, 40, 44, 48] {
            assert_eq!(estimate_rssi_for_channel(raw, 0, &cal, channel), Some(-74));
        }
        assert_eq!(estimate_rssi_for_channel(raw, 0, &cal, 52), None);
        cal.has_5ghz = false;
        assert_eq!(estimate_rssi_for_channel(raw, 0, &cal, 36), None);
    }

    #[test]
    fn signed_chain_zero_and_eeprom_corrections() {
        // 0xba is -70, not an unsigned magnitude; -70 - 2 - 12 = -84.
        assert_eq!(
            estimate_rssi([0xba, 0xff, 1, 127], 0, &calibration(-2, 12)),
            Some(-84)
        );
        assert_eq!(
            estimate_rssi([0xba, 0, 0, 0], 0, &calibration(3, 12)),
            Some(-79)
        );
    }

    #[test]
    fn rejects_overflow_instead_of_wrapping() {
        assert_eq!(
            estimate_rssi([128, 0, 0, 0], 0, &calibration(-10, 12)),
            None
        );
        assert_eq!(estimate_rssi([127, 0, 0, 0], 0, &calibration(10, 0)), None);
        assert_eq!(
            estimate_rssi([128, 0, 0, 0], 0, &calibration(0, 0)),
            Some(-128)
        );
    }

    #[test]
    fn ampdu_requires_valid_flag_but_ordinary_frames_do_not() {
        let raw = [0xba, 0, 0, 0];
        let cal = calibration(0, 0);
        assert_eq!(estimate_rssi(raw, RXINFO_AMPDU, &cal), None);
        assert_eq!(
            estimate_rssi(raw, RXINFO_AMPDU | RXINFO_RSSI, &cal),
            Some(-70)
        );
        assert_eq!(estimate_rssi(raw, 0, &cal), Some(-70));
    }

    #[test]
    fn zero_chain_zero_is_unavailable_even_if_other_chains_or_flag_are_set() {
        assert_eq!(
            estimate_rssi([0, 0xba, 0xbc, 0xbd], RXINFO_RSSI, &calibration(-2, 12)),
            None
        );
    }
}
