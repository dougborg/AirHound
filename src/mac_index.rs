/// O(1) MAC address lookup via open-addressing hash tables.
///
/// Two hash tables: `OuiIndex` for 3-byte OUI prefix lookups and
/// `FullMacIndex` for 6-byte exact MAC lookups. Combined in `MacIndex`
/// which provides a unified `lookup()` returning all matching info.
///
/// Uses FNV-1a hashing with linear probing. Sentinel-based empty/tombstone
/// slots avoid enum overhead per slot.

/// FNV-1a hash for byte slices (32-bit).
#[inline]
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Empty sentinel for OUI slots (broadcast OUI — never a valid unicast OUI).
const OUI_EMPTY: [u8; 3] = [0xFF, 0xFF, 0xFF];
/// Tombstone sentinel for OUI slots.
const OUI_TOMBSTONE: [u8; 3] = [0xFF, 0xFF, 0xFE];

/// Empty sentinel for full MAC slots (broadcast MAC — never a valid unicast address).
const MAC_EMPTY: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
/// Tombstone sentinel for full MAC slots.
const MAC_TOMBSTONE: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE];

/// Sentinel value indicating "no ID" for signature indices and watchlist IDs.
pub const NO_ID: u16 = u16::MAX;

/// Value stored alongside an OUI key.
#[derive(Debug, Clone, Copy)]
pub struct OuiValue {
    /// Index into `MAC_PREFIXES` array, or [`NO_ID`] if this is watchlist-only.
    pub sig_idx: u16,
    /// Vendor name from the signature database, or empty if watchlist-only.
    pub vendor: &'static str,
    /// Watchlist entry ID, or [`NO_ID`] if not a watchlist entry.
    pub watchlist_id: u16,
}

/// Value stored alongside a full MAC key.
#[derive(Debug, Clone, Copy)]
pub struct FullMacValue {
    /// Watchlist entry ID.
    pub watchlist_id: u16,
}

/// Open-addressing hash table for 3-byte OUI prefix lookup.
///
/// **Limitation:** The all-zeros OUI `[0, 0, 0]` is used as the empty sentinel
/// and cannot be stored or looked up. This OUI is not assigned to any real
/// hardware vendor (IEEE OUI `00:00:00` is reserved).
pub struct OuiIndex<const N: usize> {
    keys: [[u8; 3]; N],
    vals: [OuiValue; N],
    len: usize,
}

impl<const N: usize> OuiIndex<N> {
    pub const fn new() -> Self {
        Self {
            keys: [OUI_EMPTY; N],
            vals: [OuiValue {
                sig_idx: NO_ID,
                vendor: "",
                watchlist_id: NO_ID,
            }; N],
            len: 0,
        }
    }

    /// Number of entries currently stored.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn slot_idx(&self, key: &[u8; 3]) -> usize {
        fnv1a(key) as usize % N
    }

    /// Insert or update an OUI entry. Returns `false` if the table is full.
    pub fn insert(&mut self, key: [u8; 3], val: OuiValue) -> bool {
        if key == OUI_EMPTY || key == OUI_TOMBSTONE {
            return false; // reserved sentinels
        }
        if self.len >= N {
            return false;
        }

        let mut idx = self.slot_idx(&key);
        let mut tombstone_idx: Option<usize> = None;

        for _ in 0..N {
            if self.keys[idx] == key {
                // Key exists — merge: update sig_idx/vendor if they were unset,
                // update watchlist_id if new value has one
                if val.sig_idx != NO_ID {
                    self.vals[idx].sig_idx = val.sig_idx;
                    self.vals[idx].vendor = val.vendor;
                }
                if val.watchlist_id != NO_ID {
                    self.vals[idx].watchlist_id = val.watchlist_id;
                }
                return true;
            }
            if self.keys[idx] == OUI_EMPTY {
                let target = tombstone_idx.unwrap_or(idx);
                self.keys[target] = key;
                self.vals[target] = val;
                self.len += 1;
                return true;
            }
            if self.keys[idx] == OUI_TOMBSTONE && tombstone_idx.is_none() {
                tombstone_idx = Some(idx);
            }
            idx = (idx + 1) % N;
        }

        // Table scanned fully without finding empty — shouldn't happen if len < N
        false
    }

    /// Look up by OUI prefix. Returns `None` if not found.
    pub fn get(&self, key: &[u8; 3]) -> Option<&OuiValue> {
        if *key == OUI_EMPTY || *key == OUI_TOMBSTONE {
            return None;
        }
        let mut idx = self.slot_idx(key);
        for _ in 0..N {
            if self.keys[idx] == *key {
                return Some(&self.vals[idx]);
            }
            if self.keys[idx] == OUI_EMPTY {
                return None; // chain broken
            }
            idx = (idx + 1) % N;
        }
        None
    }

    /// Get mutable reference to a value by OUI prefix.
    pub fn get_mut(&mut self, key: &[u8; 3]) -> Option<&mut OuiValue> {
        if *key == OUI_EMPTY || *key == OUI_TOMBSTONE {
            return None;
        }
        let mut idx = self.slot_idx(key);
        for _ in 0..N {
            if self.keys[idx] == *key {
                return Some(&mut self.vals[idx]);
            }
            if self.keys[idx] == OUI_EMPTY {
                return None;
            }
            idx = (idx + 1) % N;
        }
        None
    }

    /// Remove an OUI entry by key. Returns the removed value if found.
    pub fn remove(&mut self, key: &[u8; 3]) -> Option<OuiValue> {
        if *key == OUI_EMPTY || *key == OUI_TOMBSTONE {
            return None;
        }
        let mut idx = self.slot_idx(key);
        for _ in 0..N {
            if self.keys[idx] == *key {
                let val = self.vals[idx];
                self.keys[idx] = OUI_TOMBSTONE;
                self.len -= 1;
                return Some(val);
            }
            if self.keys[idx] == OUI_EMPTY {
                return None;
            }
            idx = (idx + 1) % N;
        }
        None
    }
}

/// Open-addressing hash table for 6-byte full MAC lookup.
pub struct FullMacIndex<const N: usize> {
    keys: [[u8; 6]; N],
    vals: [FullMacValue; N],
    len: usize,
}

impl<const N: usize> FullMacIndex<N> {
    pub const fn new() -> Self {
        Self {
            keys: [MAC_EMPTY; N],
            vals: [FullMacValue {
                watchlist_id: NO_ID,
            }; N],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn slot_idx(&self, key: &[u8; 6]) -> usize {
        fnv1a(key) as usize % N
    }

    fn is_sentinel(key: &[u8; 6]) -> bool {
        *key == MAC_EMPTY || *key == MAC_TOMBSTONE
    }

    /// Insert a full MAC entry. Returns `false` if the table is full.
    pub fn insert(&mut self, key: [u8; 6], val: FullMacValue) -> bool {
        if Self::is_sentinel(&key) {
            return false;
        }
        if self.len >= N {
            return false;
        }

        let mut idx = self.slot_idx(&key);
        let mut tombstone_idx: Option<usize> = None;

        for _ in 0..N {
            if self.keys[idx] == key {
                self.vals[idx] = val; // update
                return true;
            }
            if self.keys[idx] == MAC_EMPTY {
                let target = tombstone_idx.unwrap_or(idx);
                self.keys[target] = key;
                self.vals[target] = val;
                self.len += 1;
                return true;
            }
            if self.keys[idx] == MAC_TOMBSTONE && tombstone_idx.is_none() {
                tombstone_idx = Some(idx);
            }
            idx = (idx + 1) % N;
        }
        false
    }

    /// Look up by full MAC. Returns `None` if not found.
    pub fn get(&self, key: &[u8; 6]) -> Option<&FullMacValue> {
        if Self::is_sentinel(key) {
            return None;
        }
        let mut idx = self.slot_idx(key);
        for _ in 0..N {
            if self.keys[idx] == *key {
                return Some(&self.vals[idx]);
            }
            if self.keys[idx] == MAC_EMPTY {
                return None;
            }
            idx = (idx + 1) % N;
        }
        None
    }

    /// Remove a full MAC entry. Returns the removed value if found.
    pub fn remove(&mut self, key: &[u8; 6]) -> Option<FullMacValue> {
        if Self::is_sentinel(key) {
            return None;
        }
        let mut idx = self.slot_idx(key);
        for _ in 0..N {
            if self.keys[idx] == *key {
                let val = self.vals[idx];
                self.keys[idx] = MAC_TOMBSTONE;
                self.len -= 1;
                return Some(val);
            }
            if self.keys[idx] == MAC_EMPTY {
                return None;
            }
            idx = (idx + 1) % N;
        }
        None
    }
}

/// Combined result from `MacIndex::lookup()`.
#[derive(Debug, Clone, Copy)]
pub struct MacLookupResult {
    /// Signature index from OUI match, or `NO_ID` if none.
    pub sig_idx: u16,
    /// Vendor name from OUI match, or empty string.
    pub vendor: &'static str,
    /// Watchlist ID from OUI prefix match, or `NO_ID` if none.
    pub oui_watchlist_id: u16,
    /// Watchlist ID from full MAC match, or `NO_ID` if none.
    pub full_watchlist_id: u16,
}

impl MacLookupResult {
    /// Whether any match was found (signature or watchlist).
    pub fn has_match(&self) -> bool {
        self.sig_idx != NO_ID || self.oui_watchlist_id != NO_ID || self.full_watchlist_id != NO_ID
    }

    /// Whether this is a watchlist match (OUI or full).
    pub fn has_watchlist_match(&self) -> bool {
        self.oui_watchlist_id != NO_ID || self.full_watchlist_id != NO_ID
    }

    /// Return the first watchlist ID found (full MAC preferred over OUI).
    pub fn watchlist_id(&self) -> Option<u16> {
        if self.full_watchlist_id != NO_ID {
            Some(self.full_watchlist_id)
        } else if self.oui_watchlist_id != NO_ID {
            Some(self.oui_watchlist_id)
        } else {
            None
        }
    }
}

/// Default OUI hash table capacity.
/// ESP32: 128 slots (~3.2KB) — sufficient for ~50 OUI entries at 40% load.
/// ESP32-S3: 256 slots (~6.4KB) — sufficient for ~100 OUI entries.
#[cfg(feature = "esp32")]
pub const DEFAULT_OUI_CAP: usize = 128;
#[cfg(not(feature = "esp32"))]
pub const DEFAULT_OUI_CAP: usize = 256;

/// Default full MAC hash table capacity.
/// ESP32: 32 slots (~0.3KB).
/// ESP32-S3: 64 slots (~0.5KB).
#[cfg(feature = "esp32")]
pub const DEFAULT_MAC_CAP: usize = 32;
#[cfg(not(feature = "esp32"))]
pub const DEFAULT_MAC_CAP: usize = 64;

/// Combined MAC index: OUI prefix table + full MAC table.
///
/// OUI_CAP should be ≥ 2× the number of OUI entries for good load factor.
/// MAC_CAP should be ≥ 2× the expected watchlist full-MAC entries.
pub struct MacIndex<
    const OUI_CAP: usize = { DEFAULT_OUI_CAP },
    const MAC_CAP: usize = { DEFAULT_MAC_CAP },
> {
    oui: OuiIndex<OUI_CAP>,
    full: FullMacIndex<MAC_CAP>,
}

impl<const OUI_CAP: usize, const MAC_CAP: usize> MacIndex<OUI_CAP, MAC_CAP> {
    pub const fn new() -> Self {
        Self {
            oui: OuiIndex::new(),
            full: FullMacIndex::new(),
        }
    }

    /// Build the index from the default MAC_PREFIXES signature array.
    pub fn from_defaults() -> Self {
        let mut idx = Self::new();
        for (i, &(ref prefix, vendor)) in crate::defaults::MAC_PREFIXES.iter().enumerate() {
            idx.oui.insert(
                *prefix,
                OuiValue {
                    sig_idx: crate::defaults::SIG_IDX_MAC_OUI_START + i as u16,
                    vendor,
                    watchlist_id: NO_ID,
                },
            );
        }
        idx
    }

    /// Look up a 6-byte MAC address against both tables.
    pub fn lookup(&self, mac: &[u8; 6]) -> MacLookupResult {
        let oui = [mac[0], mac[1], mac[2]];
        let oui_result = self.oui.get(&oui);
        let full_result = self.full.get(mac);

        MacLookupResult {
            sig_idx: oui_result.map_or(NO_ID, |v| v.sig_idx),
            vendor: oui_result.map_or("", |v| v.vendor),
            oui_watchlist_id: oui_result.map_or(NO_ID, |v| v.watchlist_id),
            full_watchlist_id: full_result.map_or(NO_ID, |v| v.watchlist_id),
        }
    }

    /// Add a watchlist OUI prefix entry.
    pub fn add_watchlist_oui(&mut self, prefix: [u8; 3], watchlist_id: u16) -> bool {
        self.oui.insert(
            prefix,
            OuiValue {
                sig_idx: NO_ID,
                vendor: "",
                watchlist_id,
            },
        )
    }

    /// Add a watchlist full MAC entry.
    pub fn add_watchlist_full(&mut self, mac: [u8; 6], watchlist_id: u16) -> bool {
        self.full.insert(mac, FullMacValue { watchlist_id })
    }

    /// Remove a watchlist OUI prefix entry. If the OUI also has a signature,
    /// only clears the watchlist_id rather than removing the slot.
    pub fn remove_watchlist_oui(&mut self, prefix: &[u8; 3]) {
        if let Some(val) = self.oui.get_mut(prefix) {
            if val.sig_idx != NO_ID {
                // OUI is also a signature — just clear watchlist
                val.watchlist_id = NO_ID;
            } else {
                // Watchlist-only — remove entirely
                self.oui.remove(prefix);
            }
        }
    }

    /// Remove a watchlist full MAC entry.
    pub fn remove_watchlist_full(&mut self, mac: &[u8; 6]) {
        self.full.remove(mac);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── OuiIndex tests ──────────────────────────────────────────────

    #[test]
    fn oui_empty_returns_none() {
        let idx: OuiIndex<32> = OuiIndex::new();
        assert!(idx.get(&[0xAA, 0xBB, 0xCC]).is_none());
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn oui_insert_and_get() {
        let mut idx: OuiIndex<32> = OuiIndex::new();
        assert!(idx.insert(
            [0xB4, 0x1E, 0x52],
            OuiValue {
                sig_idx: 0,
                vendor: "Flock Safety",
                watchlist_id: NO_ID,
            }
        ));
        assert_eq!(idx.len(), 1);

        let val = idx.get(&[0xB4, 0x1E, 0x52]).unwrap();
        assert_eq!(val.sig_idx, 0);
        assert_eq!(val.vendor, "Flock Safety");
        assert_eq!(val.watchlist_id, NO_ID);
    }

    #[test]
    fn oui_miss_returns_none() {
        let mut idx: OuiIndex<32> = OuiIndex::new();
        idx.insert(
            [0xB4, 0x1E, 0x52],
            OuiValue {
                sig_idx: 0,
                vendor: "Test",
                watchlist_id: NO_ID,
            },
        );
        assert!(idx.get(&[0xAA, 0xBB, 0xCC]).is_none());
    }

    #[test]
    fn oui_collision_handling() {
        // Insert enough entries to force collisions in a small table
        let mut idx: OuiIndex<8> = OuiIndex::new();
        let prefixes = [
            [0x01, 0x02, 0x03],
            [0x04, 0x05, 0x06],
            [0x07, 0x08, 0x09],
            [0x0A, 0x0B, 0x0C],
        ];
        for (i, &p) in prefixes.iter().enumerate() {
            assert!(idx.insert(
                p,
                OuiValue {
                    sig_idx: i as u16,
                    vendor: "Test",
                    watchlist_id: NO_ID,
                }
            ));
        }
        // All should be findable
        for (i, &p) in prefixes.iter().enumerate() {
            let val = idx.get(&p).unwrap();
            assert_eq!(val.sig_idx, i as u16);
        }
    }

    #[test]
    fn oui_update_existing_key() {
        let mut idx: OuiIndex<32> = OuiIndex::new();
        idx.insert(
            [0xAA, 0xBB, 0xCC],
            OuiValue {
                sig_idx: 5,
                vendor: "Old",
                watchlist_id: NO_ID,
            },
        );
        // Insert same key with watchlist_id — should merge
        idx.insert(
            [0xAA, 0xBB, 0xCC],
            OuiValue {
                sig_idx: NO_ID,
                vendor: "",
                watchlist_id: 42,
            },
        );
        assert_eq!(idx.len(), 1); // no new entry
        let val = idx.get(&[0xAA, 0xBB, 0xCC]).unwrap();
        assert_eq!(val.sig_idx, 5); // preserved
        assert_eq!(val.vendor, "Old"); // preserved
        assert_eq!(val.watchlist_id, 42); // merged
    }

    #[test]
    fn oui_remove_and_tombstone() {
        let mut idx: OuiIndex<32> = OuiIndex::new();
        idx.insert(
            [0xAA, 0xBB, 0xCC],
            OuiValue {
                sig_idx: 0,
                vendor: "Test",
                watchlist_id: NO_ID,
            },
        );
        let removed = idx.remove(&[0xAA, 0xBB, 0xCC]);
        assert!(removed.is_some());
        assert_eq!(idx.len(), 0);
        assert!(idx.get(&[0xAA, 0xBB, 0xCC]).is_none());
    }

    #[test]
    fn oui_remove_nonexistent() {
        let mut idx: OuiIndex<32> = OuiIndex::new();
        assert!(idx.remove(&[0xAA, 0xBB, 0xCC]).is_none());
    }

    #[test]
    fn oui_insert_after_remove_reuses_tombstone() {
        let mut idx: OuiIndex<8> = OuiIndex::new();
        idx.insert(
            [0xAA, 0xBB, 0xCC],
            OuiValue {
                sig_idx: 0,
                vendor: "A",
                watchlist_id: NO_ID,
            },
        );
        idx.remove(&[0xAA, 0xBB, 0xCC]);
        idx.insert(
            [0xDD, 0xEE, 0xFF],
            OuiValue {
                sig_idx: 1,
                vendor: "B",
                watchlist_id: NO_ID,
            },
        );
        assert!(idx.get(&[0xDD, 0xEE, 0xFF]).is_some());
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn oui_sentinel_keys_rejected() {
        let mut idx: OuiIndex<32> = OuiIndex::new();
        // Empty sentinel
        assert!(!idx.insert(
            OUI_EMPTY,
            OuiValue {
                sig_idx: 0,
                vendor: "",
                watchlist_id: NO_ID,
            }
        ));
        // Tombstone sentinel
        assert!(!idx.insert(
            OUI_TOMBSTONE,
            OuiValue {
                sig_idx: 0,
                vendor: "",
                watchlist_id: NO_ID,
            }
        ));
        assert!(idx.get(&OUI_EMPTY).is_none());
        assert!(idx.get(&OUI_TOMBSTONE).is_none());
    }

    #[test]
    fn oui_find_after_tombstone_in_chain() {
        // Insert two keys that hash to the same slot, remove the first,
        // then verify the second is still findable across the tombstone.
        let mut idx: OuiIndex<4> = OuiIndex::new();
        let a = [0x01, 0x02, 0x03];
        let b = [0x05, 0x06, 0x07];
        idx.insert(
            a,
            OuiValue {
                sig_idx: 0,
                vendor: "A",
                watchlist_id: NO_ID,
            },
        );
        idx.insert(
            b,
            OuiValue {
                sig_idx: 1,
                vendor: "B",
                watchlist_id: NO_ID,
            },
        );
        // Remove first — creates tombstone
        idx.remove(&a);
        // Second should still be reachable
        assert!(idx.get(&b).is_some());
    }

    // ── FullMacIndex tests ──────────────────────────────────────────

    #[test]
    fn full_mac_insert_and_get() {
        let mut idx: FullMacIndex<16> = FullMacIndex::new();
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD];
        assert!(idx.insert(mac, FullMacValue { watchlist_id: 1 }));
        let val = idx.get(&mac).unwrap();
        assert_eq!(val.watchlist_id, 1);
    }

    #[test]
    fn full_mac_miss() {
        let idx: FullMacIndex<16> = FullMacIndex::new();
        assert!(idx.get(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]).is_none());
    }

    #[test]
    fn full_mac_remove() {
        let mut idx: FullMacIndex<16> = FullMacIndex::new();
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD];
        idx.insert(mac, FullMacValue { watchlist_id: 1 });
        let removed = idx.remove(&mac);
        assert!(removed.is_some());
        assert!(idx.get(&mac).is_none());
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn full_mac_sentinel_rejected() {
        let mut idx: FullMacIndex<16> = FullMacIndex::new();
        assert!(!idx.insert(MAC_EMPTY, FullMacValue { watchlist_id: 1 }));
        assert!(!idx.insert(MAC_TOMBSTONE, FullMacValue { watchlist_id: 1 }));
    }

    // ── MacIndex combined tests ─────────────────────────────────────

    #[test]
    fn mac_index_from_defaults_has_entries() {
        let idx = MacIndex::<256, 64>::from_defaults();
        assert_eq!(idx.oui.len(), crate::defaults::MAC_PREFIXES.len());
        // Spot-check Flock Safety OUI
        let val = idx.oui.get(&[0xB4, 0x1E, 0x52]).unwrap();
        assert_eq!(val.sig_idx, crate::defaults::SIG_IDX_MAC_OUI_START);
        assert_eq!(val.vendor, "Flock Safety");
    }

    #[test]
    fn mac_index_lookup_oui_match() {
        let idx = MacIndex::<256, 64>::from_defaults();
        let result = idx.lookup(&[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03]);
        assert!(result.has_match());
        assert_eq!(result.sig_idx, crate::defaults::SIG_IDX_MAC_OUI_START);
        assert_eq!(result.vendor, "Flock Safety");
        assert!(!result.has_watchlist_match());
    }

    #[test]
    fn mac_index_lookup_no_match() {
        let idx = MacIndex::<256, 64>::from_defaults();
        let result = idx.lookup(&[0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03]);
        assert!(!result.has_match());
        assert_eq!(result.sig_idx, NO_ID);
        assert_eq!(result.vendor, "");
    }

    #[test]
    fn mac_index_watchlist_oui() {
        let mut idx = MacIndex::<256, 64>::new();
        assert!(idx.add_watchlist_oui([0xAA, 0xBB, 0xCC], 42));
        let result = idx.lookup(&[0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33]);
        assert!(result.has_watchlist_match());
        assert_eq!(result.oui_watchlist_id, 42);
        assert_eq!(result.watchlist_id(), Some(42));
    }

    #[test]
    fn mac_index_watchlist_full_mac() {
        let mut idx = MacIndex::<256, 64>::new();
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD];
        assert!(idx.add_watchlist_full(mac, 7));
        let result = idx.lookup(&mac);
        assert!(result.has_watchlist_match());
        assert_eq!(result.full_watchlist_id, 7);
        assert_eq!(result.watchlist_id(), Some(7));
    }

    #[test]
    fn mac_index_watchlist_full_preferred_over_oui() {
        let mut idx = MacIndex::<256, 64>::new();
        let mac = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        idx.add_watchlist_oui([0xAA, 0xBB, 0xCC], 10);
        idx.add_watchlist_full(mac, 20);
        let result = idx.lookup(&mac);
        assert_eq!(result.oui_watchlist_id, 10);
        assert_eq!(result.full_watchlist_id, 20);
        // Full MAC preferred
        assert_eq!(result.watchlist_id(), Some(20));
    }

    #[test]
    fn mac_index_oui_overlap_sig_and_watchlist() {
        let mut idx = MacIndex::<256, 64>::from_defaults();
        // Add watchlist entry for Flock Safety OUI (which is also a signature)
        idx.add_watchlist_oui([0xB4, 0x1E, 0x52], 99);
        let result = idx.lookup(&[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03]);
        assert!(result.has_match());
        assert_eq!(result.sig_idx, crate::defaults::SIG_IDX_MAC_OUI_START);
        assert_eq!(result.vendor, "Flock Safety");
        assert_eq!(result.oui_watchlist_id, 99);
    }

    #[test]
    fn mac_index_remove_watchlist_oui_keeps_signature() {
        let mut idx = MacIndex::<256, 64>::from_defaults();
        idx.add_watchlist_oui([0xB4, 0x1E, 0x52], 99);
        idx.remove_watchlist_oui(&[0xB4, 0x1E, 0x52]);
        // Signature should still be there
        let result = idx.lookup(&[0xB4, 0x1E, 0x52, 0x01, 0x02, 0x03]);
        assert_eq!(result.sig_idx, crate::defaults::SIG_IDX_MAC_OUI_START);
        assert_eq!(result.oui_watchlist_id, NO_ID); // watchlist cleared
    }

    #[test]
    fn mac_index_remove_watchlist_oui_removes_watchlist_only() {
        let mut idx = MacIndex::<256, 64>::new();
        idx.add_watchlist_oui([0xAA, 0xBB, 0xCC], 42);
        idx.remove_watchlist_oui(&[0xAA, 0xBB, 0xCC]);
        let result = idx.lookup(&[0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33]);
        assert!(!result.has_match());
    }

    #[test]
    fn mac_index_remove_watchlist_full() {
        let mut idx = MacIndex::<256, 64>::new();
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD];
        idx.add_watchlist_full(mac, 7);
        idx.remove_watchlist_full(&mac);
        let result = idx.lookup(&mac);
        assert!(!result.has_watchlist_match());
    }

    #[test]
    fn mac_index_all_default_ouis_findable() {
        let idx = MacIndex::<256, 64>::from_defaults();
        for (i, &(ref prefix, vendor)) in crate::defaults::MAC_PREFIXES.iter().enumerate() {
            let val = idx.oui.get(prefix).unwrap_or_else(|| {
                panic!(
                    "OUI {:02X}:{:02X}:{:02X} ({}) not found in index",
                    prefix[0], prefix[1], prefix[2], vendor
                )
            });
            assert_eq!(
                val.sig_idx,
                crate::defaults::SIG_IDX_MAC_OUI_START + i as u16
            );
        }
    }
}
