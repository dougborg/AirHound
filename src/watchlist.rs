/// Watchlist matching for user-defined MAC address targets.
///
/// Provides a managed collection of watchlist entries with add/remove/check
/// operations. Supports both 3-byte OUI prefix and 6-byte full MAC matching.

/// Default watchlist capacity.
/// ESP32: 32 entries (~2.5KB) to fit tighter DRAM budget.
/// ESP32-S3: 128 entries (~10KB).
#[cfg(feature = "esp32")]
pub const DEFAULT_WATCHLIST_CAPACITY: usize = 32;
#[cfg(not(feature = "esp32"))]
pub const DEFAULT_WATCHLIST_CAPACITY: usize = 128;

/// Maximum number of recent sightings to track per entry.
const SIGHTING_RING_SIZE: usize = 4;

/// A channel sighting record for a watchlist entry.
#[derive(Debug, Clone, Copy)]
pub struct WatchlistSighting {
    pub channel: u8,
    pub ts: u32,
}

/// A watchlist entry with a match criterion and user-assigned label.
#[derive(Debug)]
pub struct WatchlistEntry {
    pub id: u16,
    pub match_type: WatchlistMatch,
    pub label: heapless::String<32>,
    recent: [WatchlistSighting; SIGHTING_RING_SIZE],
    recent_len: u8,
    recent_idx: u8,
}

impl WatchlistEntry {
    pub fn new(id: u16, match_type: WatchlistMatch, label: &str) -> Self {
        let mut l = heapless::String::new();
        let _ = l.push_str(label);
        Self {
            id,
            match_type,
            label: l,
            recent: [WatchlistSighting { channel: 0, ts: 0 }; SIGHTING_RING_SIZE],
            recent_len: 0,
            recent_idx: 0,
        }
    }
}

/// How to match a watchlist entry against a MAC address.
#[derive(Debug)]
pub enum WatchlistMatch {
    /// Match the first 3 bytes (OUI prefix).
    MacPrefix([u8; 3]),
    /// Match all 6 bytes exactly.
    MacFull([u8; 6]),
}

/// A managed watchlist with fixed-capacity storage.
///
/// The capacity `N` is a const generic defaulting to [`DEFAULT_WATCHLIST_CAPACITY`].
/// Linear scan is used for lookup — fast enough for hundreds of entries on ESP32.
pub struct Watchlist<const N: usize = DEFAULT_WATCHLIST_CAPACITY> {
    entries: heapless::Vec<WatchlistEntry, N>,
}

impl<const N: usize> Watchlist<N> {
    pub const fn new() -> Self {
        Self {
            entries: heapless::Vec::new(),
        }
    }

    /// Add an entry. Returns `Err` with the entry back if the id already
    /// exists or the watchlist is full.
    pub fn add(&mut self, entry: WatchlistEntry) -> Result<(), WatchlistEntry> {
        if self.entries.iter().any(|e| e.id == entry.id) {
            return Err(entry);
        }
        self.entries.push(entry).map_err(|e| e)
    }

    /// Remove an entry by id. Returns `true` if found and removed.
    pub fn remove(&mut self, id: u16) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            self.entries.swap_remove(pos);
            true
        } else {
            false
        }
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of entries currently stored.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the watchlist is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Check a MAC address against all entries.
    ///
    /// Returns a reference to the first matching entry, or `None`.
    pub fn check(&self, mac: &[u8; 6]) -> Option<&WatchlistEntry> {
        for entry in &self.entries {
            let matched = match &entry.match_type {
                WatchlistMatch::MacPrefix(prefix) => mac[..3] == prefix[..],
                WatchlistMatch::MacFull(full) => mac == full,
            };
            if matched {
                return Some(entry);
            }
        }
        None
    }

    /// Record a channel sighting for a watchlist entry by id.
    pub fn record_sighting(&mut self, id: u16, channel: u8, ts: u32) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            let idx = entry.recent_idx as usize;
            entry.recent[idx] = WatchlistSighting { channel, ts };
            entry.recent_idx = ((idx + 1) % SIGHTING_RING_SIZE) as u8;
            if (entry.recent_len as usize) < SIGHTING_RING_SIZE {
                entry.recent_len += 1;
            }
        }
    }

    /// Get all distinct channels seen by any watchlist entry since `since_ms`.
    pub fn active_channels(&self, since_ms: u32, now_ms: u32) -> heapless::Vec<u8, 13> {
        let mut channels = heapless::Vec::<u8, 13>::new();
        for entry in &self.entries {
            for i in 0..entry.recent_len as usize {
                let s = &entry.recent[i];
                let age = now_ms.wrapping_sub(s.ts);
                if age <= since_ms && !channels.contains(&s.channel) {
                    let _ = channels.push(s.channel);
                }
            }
        }
        channels
    }

    /// Check if a watchlist entry is channel hopping (seen on multiple
    /// distinct channels within `window_ms`).
    pub fn is_channel_hopping(&self, id: u16, window_ms: u32, now_ms: u32) -> bool {
        if let Some(entry) = self.entries.iter().find(|e| e.id == id) {
            let mut seen_channels = heapless::Vec::<u8, SIGHTING_RING_SIZE>::new();
            for i in 0..entry.recent_len as usize {
                let s = &entry.recent[i];
                let age = now_ms.wrapping_sub(s.ts);
                if age <= window_ms && !seen_channels.contains(&s.channel) {
                    let _ = seen_channels.push(s.channel);
                }
            }
            seen_channels.len() >= 2
        } else {
            false
        }
    }

    /// Get the label for a watchlist entry by id.
    pub fn get_label(&self, id: u16) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.label.as_str())
    }

    /// Iterate over all entries.
    pub fn entries(&self) -> &[WatchlistEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestWatchlist = Watchlist<16>;

    fn make_entry(id: u16, match_type: WatchlistMatch, label: &str) -> WatchlistEntry {
        WatchlistEntry::new(id, match_type, label)
    }

    #[test]
    fn empty_watchlist_returns_none() {
        let wl = TestWatchlist::new();
        let mac = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        assert!(wl.check(&mac).is_none());
        assert!(wl.is_empty());
        assert_eq!(wl.len(), 0);
    }

    #[test]
    fn add_and_prefix_match() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Target A",
        ))
        .unwrap();
        let mac = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let entry = wl.check(&mac).unwrap();
        assert_eq!(entry.id, 1);
        assert_eq!(entry.label.as_str(), "Target A");
        assert_eq!(wl.len(), 1);
    }

    #[test]
    fn add_and_full_mac_match() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            2,
            WatchlistMatch::MacFull([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]),
            "Target B",
        ))
        .unwrap();
        let mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let entry = wl.check(&mac).unwrap();
        assert_eq!(entry.id, 2);
    }

    #[test]
    fn no_match() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "A",
        ))
        .unwrap();
        wl.add(make_entry(
            2,
            WatchlistMatch::MacFull([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]),
            "B",
        ))
        .unwrap();
        let mac = [0xDD, 0xEE, 0xFF, 0x00, 0x00, 0x00];
        assert!(wl.check(&mac).is_none());
    }

    #[test]
    fn first_match_priority() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "First",
        ))
        .unwrap();
        wl.add(make_entry(
            2,
            WatchlistMatch::MacFull([0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33]),
            "Second",
        ))
        .unwrap();
        let mac = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let entry = wl.check(&mac).unwrap();
        assert_eq!(entry.label.as_str(), "First");
    }

    #[test]
    fn skips_non_matching_entries() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0x11, 0x22, 0x33]),
            "Miss",
        ))
        .unwrap();
        wl.add(make_entry(
            2,
            WatchlistMatch::MacFull([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
            "Miss",
        ))
        .unwrap();
        wl.add(make_entry(
            3,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Hit",
        ))
        .unwrap();
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let entry = wl.check(&mac).unwrap();
        assert_eq!(entry.id, 3);
        assert_eq!(entry.label.as_str(), "Hit");
    }

    #[test]
    fn remove_by_id() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "A",
        ))
        .unwrap();
        wl.add(make_entry(
            2,
            WatchlistMatch::MacPrefix([0x11, 0x22, 0x33]),
            "B",
        ))
        .unwrap();
        assert_eq!(wl.len(), 2);

        assert!(wl.remove(1));
        assert_eq!(wl.len(), 1);

        // Entry 1 gone, entry 2 still there
        let mac = [0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x00];
        assert!(wl.check(&mac).is_none());
        let mac = [0x11, 0x22, 0x33, 0x00, 0x00, 0x00];
        assert!(wl.check(&mac).is_some());
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut wl = TestWatchlist::new();
        assert!(!wl.remove(99));
    }

    #[test]
    fn clear_removes_all() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "A",
        ))
        .unwrap();
        wl.add(make_entry(
            2,
            WatchlistMatch::MacPrefix([0x11, 0x22, 0x33]),
            "B",
        ))
        .unwrap();
        assert_eq!(wl.len(), 2);

        wl.clear();
        assert!(wl.is_empty());
        assert_eq!(wl.len(), 0);
    }

    #[test]
    fn add_duplicate_id_rejected() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "A",
        ))
        .unwrap();
        let result = wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0x11, 0x22, 0x33]),
            "B",
        ));
        assert!(result.is_err());
        assert_eq!(wl.len(), 1);
    }

    #[test]
    fn add_after_remove_reuses_id() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Original",
        ))
        .unwrap();
        wl.remove(1);
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0x11, 0x22, 0x33]),
            "Replacement",
        ))
        .unwrap();
        let mac = [0x11, 0x22, 0x33, 0x00, 0x00, 0x00];
        let entry = wl.check(&mac).unwrap();
        assert_eq!(entry.label.as_str(), "Replacement");
    }

    // ── Channel tracking tests ──────────────────────────────────────

    #[test]
    fn record_sighting_stores_channel() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Target",
        ))
        .unwrap();

        wl.record_sighting(1, 6, 1000);
        let channels = wl.active_channels(5000, 1000);
        assert!(channels.contains(&6));
    }

    #[test]
    fn record_sighting_nonexistent_id_no_panic() {
        let mut wl = TestWatchlist::new();
        wl.record_sighting(99, 6, 1000); // should be a no-op
    }

    #[test]
    fn sighting_ring_buffer_wraps() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Target",
        ))
        .unwrap();

        // Fill ring buffer (size 4) and overflow
        wl.record_sighting(1, 1, 100);
        wl.record_sighting(1, 2, 200);
        wl.record_sighting(1, 3, 300);
        wl.record_sighting(1, 4, 400);
        wl.record_sighting(1, 5, 500); // wraps, overwrites slot 0

        // Channel 1 should be gone, channels 2-5 should remain
        let channels = wl.active_channels(1000, 500);
        assert!(!channels.contains(&1));
        assert!(channels.contains(&2));
        assert!(channels.contains(&5));
    }

    #[test]
    fn active_channels_filters_by_time() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Target",
        ))
        .unwrap();

        wl.record_sighting(1, 6, 1000);
        wl.record_sighting(1, 11, 5000);

        // Only sightings within last 2000ms from now=5000
        let channels = wl.active_channels(2000, 5000);
        assert!(channels.contains(&11));
        assert!(!channels.contains(&6)); // too old
    }

    #[test]
    fn active_channels_deduplicates() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "A",
        ))
        .unwrap();
        wl.add(make_entry(
            2,
            WatchlistMatch::MacPrefix([0x11, 0x22, 0x33]),
            "B",
        ))
        .unwrap();

        // Both entries seen on channel 6
        wl.record_sighting(1, 6, 1000);
        wl.record_sighting(2, 6, 1000);

        let channels = wl.active_channels(5000, 1000);
        assert_eq!(channels.len(), 1);
        assert!(channels.contains(&6));
    }

    #[test]
    fn is_channel_hopping_true() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Target",
        ))
        .unwrap();

        wl.record_sighting(1, 1, 1000);
        wl.record_sighting(1, 6, 2000);

        assert!(wl.is_channel_hopping(1, 5000, 2000));
    }

    #[test]
    fn is_channel_hopping_false_same_channel() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Target",
        ))
        .unwrap();

        wl.record_sighting(1, 6, 1000);
        wl.record_sighting(1, 6, 2000);

        assert!(!wl.is_channel_hopping(1, 5000, 2000));
    }

    #[test]
    fn is_channel_hopping_false_outside_window() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "Target",
        ))
        .unwrap();

        wl.record_sighting(1, 1, 1000);
        wl.record_sighting(1, 6, 5000);

        // Window of 1000ms from now=5000: only the second sighting is in range
        assert!(!wl.is_channel_hopping(1, 1000, 5000));
    }

    #[test]
    fn is_channel_hopping_nonexistent_id() {
        let wl = TestWatchlist::new();
        assert!(!wl.is_channel_hopping(99, 5000, 5000));
    }

    #[test]
    fn get_label_found() {
        let mut wl = TestWatchlist::new();
        wl.add(make_entry(
            1,
            WatchlistMatch::MacPrefix([0xAA, 0xBB, 0xCC]),
            "My Target",
        ))
        .unwrap();
        assert_eq!(wl.get_label(1), Some("My Target"));
    }

    #[test]
    fn get_label_not_found() {
        let wl = TestWatchlist::new();
        assert_eq!(wl.get_label(99), None);
    }
}
