//! Bounded, allocation-free address counts since boot (not physical-device IDs).
use crate::scanner::FrameType;

const USED: u8 = 1;
const AP: u8 = 2;
const BLE: u8 = 4;
const HIT: u8 = 8;

#[derive(Clone, Copy)]
struct Entry {
    address: [u8; 6],
    flags: u8,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Counts {
    pub aps: usize,
    pub wifi_other: usize,
    pub ble: usize,
    pub wifi_hits: usize,
    pub ble_hits: usize,
    pub limited: bool,
}

/// Keeps the first N distinct transport/address pairs; never evicts or recounts.
/// After overflow, counts are lower bounds. Existing entries still update.
/// Randomized addresses can represent the same physical device multiple times.
pub struct Observations<const N: usize> {
    entries: [Entry; N],
    limited: bool,
    len: usize,
}

impl<const N: usize> Default for Observations<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Observations<N> {
    pub const fn new() -> Self {
        Self {
            entries: [Entry {
                address: [0; 6],
                flags: 0,
            }; N],
            limited: false,
            len: 0,
        }
    }

    /// Initialize directly in caller-provided storage without a large stack temporary.
    pub fn init_in(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        // SAFETY: Entry consists only of byte arrays and u8 flags, and limited
        // is a bool and len is usize. All-zero bytes are a valid empty table, identical to new().
        // The exclusive storage borrow also bounds the returned reference.
        unsafe {
            storage.as_mut_ptr().write_bytes(0, 1);
            storage.assume_init_mut()
        }
    }

    pub fn wifi(&mut self, address: [u8; 6], frame: FrameType, matched: bool) {
        // Unknown/control frames do not reliably establish a transmitter identity.
        if address == [0; 6] || address[0] & 1 != 0 || frame == FrameType::Other {
            return;
        }
        let ap = matches!(frame, FrameType::Beacon | FrameType::ProbeResponse);
        self.record(address, if ap { AP } else { 0 }, matched);
    }

    pub fn ble(&mut self, address: [u8; 6], matched: bool) {
        if address != [0; 6] {
            self.record(address, BLE, matched);
        }
    }

    fn record(&mut self, address: [u8; 6], flags: u8, matched: bool) {
        let existing = self.entries[..self.len].iter().position(|e| {
            e.flags & USED != 0 && e.address == address && e.flags & BLE == flags & BLE
        });
        let index = existing.or_else(|| (self.len < N).then_some(self.len));
        if let Some(index) = index {
            if index == self.len {
                self.len += 1;
            }
            let entry = &mut self.entries[index];
            entry.address = address;
            entry.flags |= USED | flags | if matched { HIT } else { 0 };
        } else {
            self.limited = true;
        }
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts {
            limited: self.limited,
            ..Counts::default()
        };
        for entry in &self.entries[..self.len] {
            let flags = entry.flags;
            if flags & USED == 0 {
                continue;
            }
            if flags & BLE != 0 {
                counts.ble += 1;
                counts.ble_hits += usize::from(flags & HIT != 0);
            } else {
                if flags & AP != 0 {
                    counts.aps += 1;
                } else {
                    counts.wifi_other += 1;
                }
                counts.wifi_hits += usize::from(flags & HIT != 0);
            }
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: [u8; 6] = [2, 1, 2, 3, 4, 5];
    #[test]
    fn initializes_in_place_and_deduplicates() {
        let mut storage = core::mem::MaybeUninit::<Observations<4096>>::uninit();
        let table = Observations::init_in(&mut storage);
        assert_eq!(table.counts(), Counts::default());
        table.wifi(A, FrameType::Beacon, true);
        table.wifi(A, FrameType::Beacon, true);
        assert_eq!(table.counts().aps, 1);
        assert_eq!(table.counts().wifi_hits, 1);
    }

    #[test]
    fn repeats_promotions_and_matches_are_distinct() {
        let mut seen = Observations::<4>::new();
        seen.wifi(A, FrameType::Data, false);
        seen.wifi(A, FrameType::ProbeRequest, false);
        assert_eq!(seen.counts().wifi_other, 1);
        seen.wifi(A, FrameType::Beacon, true);
        seen.wifi(A, FrameType::Data, false);
        seen.ble(A, true);
        seen.ble(A, true);
        assert_eq!(
            seen.counts(),
            Counts {
                aps: 1,
                ble: 1,
                wifi_hits: 1,
                ble_hits: 1,
                ..Counts::default()
            }
        );
    }
    #[test]
    fn overflow_never_recounts_and_existing_entries_still_update() {
        let mut seen = Observations::<1>::new();
        seen.wifi(A, FrameType::ProbeRequest, false);
        seen.ble(A, true);
        seen.wifi(A, FrameType::ProbeResponse, true);
        assert_eq!(
            seen.counts(),
            Counts {
                aps: 1,
                wifi_hits: 1,
                limited: true,
                ..Counts::default()
            }
        );
        let mut empty = Observations::<0>::new();
        empty.ble(A, false);
        assert!(empty.counts().limited);
    }
    #[test]
    fn rejects_invalid_wifi_sources_but_accepts_ble_random_addresses() {
        let mut seen = Observations::<4>::new();
        for address in [[0; 6], [255; 6], [1; 6]] {
            seen.wifi(address, FrameType::Beacon, true);
        }
        seen.wifi(A, FrameType::Other, true);
        seen.ble([0; 6], true);
        assert_eq!(seen.counts(), Counts::default());
        seen.ble([1; 6], false);
        assert_eq!(seen.counts().ble, 1);
    }
}
