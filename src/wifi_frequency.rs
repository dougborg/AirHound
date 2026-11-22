//! Receive-frequency interpretation, not a regulatory or tuning allowlist.
//! Channel numbers overlap across bands. Never infer a band from a bare channel.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Ghz2_4,
    Ghz5,
    Ghz6,
}

impl Band {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ghz2_4 => "2.4GHz",
            Self::Ghz5 => "5GHz",
            Self::Ghz6 => "6GHz",
        }
    }
}

/// Interpret a reported center frequency on the WiFi channel raster.
/// Includes the 2484 MHz and 5935 MHz special channels. Width and regulatory
/// availability are not implied by this conversion.
pub fn channel(frequency_mhz: u16) -> Option<(Band, u8)> {
    let (band, base, number) = match frequency_mhz {
        2484 => return Some((Band::Ghz2_4, 14)),
        5935 => return Some((Band::Ghz6, 2)),
        2412..=2472 => (Band::Ghz2_4, 2407, frequency_mhz),
        4910..=4980 => (Band::Ghz5, 4000, frequency_mhz),
        5005..=5895 => (Band::Ghz5, 5000, frequency_mhz),
        5955..=7115 => (Band::Ghz6, 5950, frequency_mhz),
        _ => return None,
    };
    let offset = number - base;
    if !offset.is_multiple_of(5) {
        return None;
    }
    Some((band, (offset / 5) as u8))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn overlapping_channels_and_special_frequencies() {
        for (mhz, band, ch) in [
            (2412, Band::Ghz2_4, 1),
            (2472, Band::Ghz2_4, 13),
            (2484, Band::Ghz2_4, 14),
            (4910, Band::Ghz5, 182),
            (4980, Band::Ghz5, 196),
            (5180, Band::Ghz5, 36),
            (5885, Band::Ghz5, 177),
            (5935, Band::Ghz6, 2),
            (5955, Band::Ghz6, 1),
            (7115, Band::Ghz6, 233),
        ] {
            assert_eq!(channel(mhz), Some((band, ch)));
        }
        assert_eq!(Band::Ghz2_4.as_str(), "2.4GHz");
        assert_eq!(Band::Ghz5.as_str(), "5GHz");
        assert_eq!(Band::Ghz6.as_str(), "6GHz");
        for mhz in [
            0,
            2411,
            2413,
            2483,
            2485,
            4909,
            4981,
            5004,
            5896,
            5930,
            5950,
            5956,
            7116,
            u16::MAX,
        ] {
            assert_eq!(channel(mhz), None, "{mhz}");
        }
    }
}
