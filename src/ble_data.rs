//! Borrowed BLE fields. OS property maps are not reconstructed radio packets.
use core::fmt;

/// Canonical (network-order) 128-bit UUID, including expanded Bluetooth aliases.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Uuid(pub [u8; 16]);
impl Uuid {
    pub const fn from_u16(value: u16) -> Self {
        Self::from_u32(value as u32)
    }
    pub const fn from_u32(value: u32) -> Self {
        let v = value.to_be_bytes();
        Self([
            v[0], v[1], v[2], v[3], 0, 0, 0x10, 0, 0x80, 0, 0, 0x80, 0x5f, 0x9b, 0x34, 0xfb,
        ])
    }
    pub fn from_ad_le(bytes: &[u8]) -> Option<Self> {
        match bytes.len() {
            2 => Some(Self::from_u16(u16::from_le_bytes(bytes.try_into().ok()?))),
            4 => Some(Self::from_u32(u32::from_le_bytes(bytes.try_into().ok()?))),
            16 => {
                let mut value: [u8; 16] = bytes.try_into().ok()?;
                value.reverse();
                Some(Self(value))
            }
            _ => None,
        }
    }
    pub fn as_u16(self) -> Option<u16> {
        let value = u16::from_be_bytes([self.0[2], self.0[3]]);
        (self == Self::from_u16(value)).then_some(value)
    }
}
impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}
impl serde::Serialize for Uuid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = heapless::String::<36>::new();
        use fmt::Write;
        write!(value, "{self}").map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManufacturerData<'a> {
    pub company_id: u16,
    pub payload: &'a [u8],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceData<'a> {
    pub uuid: Uuid,
    pub payload: &'a [u8],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdStructure<'a> {
    pub kind: u8,
    pub data: &'a [u8],
}
impl<'a> AdStructure<'a> {
    pub fn manufacturer(self) -> Option<ManufacturerData<'a>> {
        if self.kind != 0xff || self.data.len() < 2 {
            return None;
        }
        Some(ManufacturerData {
            company_id: u16::from_le_bytes([self.data[0], self.data[1]]),
            payload: &self.data[2..],
        })
    }
    pub fn service(self) -> Option<ServiceData<'a>> {
        let width = match self.kind {
            0x16 => 2,
            0x20 => 4,
            0x21 => 16,
            _ => return None,
        };
        Some(ServiceData {
            uuid: Uuid::from_ad_le(self.data.get(..width)?)?,
            payload: &self.data[width..],
        })
    }
    pub fn uuids(self) -> impl Iterator<Item = Uuid> + 'a {
        let width = match self.kind {
            0x02 | 0x03 => 2,
            0x04 | 0x05 => 4,
            0x06 | 0x07 => 16,
            _ => 0,
        };
        let data = if width != 0 && self.data.len().is_multiple_of(width) {
            self.data
        } else {
            &[]
        };
        data.chunks_exact(width.max(1)).filter_map(Uuid::from_ad_le)
    }
}
/// A malformed final length stops iteration; `malformed()` is meaningful after draining.
pub struct AdStructures<'a> {
    remaining: &'a [u8],
    malformed: bool,
    incomplete: bool,
}
impl<'a> AdStructures<'a> {
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self {
            remaining: bytes,
            malformed: false,
            incomplete: false,
        }
    }
    pub const fn malformed(&self) -> bool {
        self.malformed
    }
    pub const fn incomplete(&self) -> bool {
        self.incomplete
    }
}
impl<'a> Iterator for AdStructures<'a> {
    type Item = AdStructure<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let len = *self.remaining.first()? as usize;
        if len == 0 {
            self.remaining = &[];
            return None;
        }
        if len >= self.remaining.len() {
            self.incomplete = true;
            self.remaining = &[];
            return None;
        }
        let value = AdStructure {
            kind: self.remaining[1],
            data: &self.remaining[2..len + 1],
        };
        self.remaining = &self.remaining[len + 1..];
        let bad = match value.kind {
            0x02 | 0x03 => !value.data.len().is_multiple_of(2),
            0x04 | 0x05 => !value.data.len().is_multiple_of(4),
            0x06 | 0x07 => !value.data.len().is_multiple_of(16),
            0xff | 0x16 => value.data.len() < 2,
            0x20 => value.data.len() < 4,
            0x21 => value.data.len() < 16,
            _ => false,
        };
        self.malformed |= bad;
        Some(value)
    }
}

/// Explicit payload-relative constraints. A mask byte of zero is a wildcard.
pub struct BytePattern<'a> {
    pub bytes: &'a [u8],
    pub mask: Option<&'a [u8]>,
    pub offset: usize,
    pub min_len: usize,
    pub max_len: Option<usize>,
}
impl BytePattern<'_> {
    pub fn matches(&self, payload: &[u8]) -> bool {
        if self.bytes.is_empty()
            || payload.len() < self.min_len
            || self.max_len.is_some_and(|n| payload.len() > n)
            || self.mask.is_some_and(|m| m.len() != self.bytes.len())
        {
            return false;
        }
        let Some(end) = self.offset.checked_add(self.bytes.len()) else {
            return false;
        };
        let Some(candidate) = payload.get(self.offset..end) else {
            return false;
        };
        candidate
            .iter()
            .zip(self.bytes)
            .enumerate()
            .all(|(i, (actual, expected))| {
                let mask = self.mask.map_or(0xff, |m| m[i]);
                actual & mask == expected & mask
            })
    }
    pub fn manufacturer_matches(&self, record: ManufacturerData<'_>, company_id: u16) -> bool {
        record.company_id == company_id && self.matches(record.payload)
    }
    pub fn service_matches(&self, record: ServiceData<'_>, uuid: Uuid) -> bool {
        record.uuid == uuid && self.matches(record.payload)
    }
}

/// Normalized borrowed fields, suitable for raw reports or platform records.
/// The caller owns freshness/completeness policy; this view makes no packet claim.
pub struct Advertisement<'a> {
    pub service_uuids: &'a [Uuid],
    pub manufacturer_data: &'a [ManufacturerData<'a>],
    pub service_data: &'a [ServiceData<'a>],
}
impl Advertisement<'_> {
    pub fn has_service(&self, uuid: Uuid) -> bool {
        self.service_uuids.contains(&uuid) || self.service_data.iter().any(|r| r.uuid == uuid)
    }
    pub fn matches_manufacturer(&self, company_id: u16, pattern: &BytePattern<'_>) -> bool {
        self.manufacturer_data
            .iter()
            .any(|&record| pattern.manufacturer_matches(record, company_id))
    }
    pub fn matches_service_data(&self, uuid: Uuid, pattern: &BytePattern<'_>) -> bool {
        self.service_data
            .iter()
            .any(|&record| pattern.service_matches(record, uuid))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn uuid_identity_and_endian_are_exact() {
        let id = Uuid::from_u16(0xfffa);
        assert_eq!(Uuid::from_ad_le(&[0xfa, 0xff]), Some(id));
        assert_eq!(Uuid::from_ad_le(&[0xfa, 0xff, 0, 0]), Some(id));
        assert_eq!(id.to_string(), "0000fffa-0000-1000-8000-00805f9b34fb");
        let mut raw = id.0;
        raw.reverse();
        assert_eq!(Uuid::from_ad_le(&raw), Some(id));
        let mut custom = id;
        custom.0[15] ^= 1;
        assert_eq!(custom.as_u16(), None);
        assert_eq!(Uuid::from_u32(0x1234fffa).as_u16(), None);
        assert_eq!(Uuid::from_ad_le(&[1, 2, 3]), None);
    }
    #[test]
    fn fields_keep_context_and_reject_partial_records() {
        let bytes = [
            3, 0xff, 0, 0, 4, 0x16, 0xfa, 0xff, 7, 3, 0xff, 0x4c, 0, 5, 0x21, 1,
        ];
        let mut iter = AdStructures::new(&bytes);
        assert_eq!(
            iter.next().unwrap().manufacturer(),
            Some(ManufacturerData {
                company_id: 0,
                payload: &[]
            })
        );
        let service = iter.next().unwrap().service().unwrap();
        assert_eq!(service.uuid, Uuid::from_u16(0xfffa));
        assert_eq!(service.payload, &[7]);
        assert_eq!(
            iter.next().unwrap().manufacturer().unwrap().company_id,
            0x4c
        );
        assert!(iter.next().is_none());
        assert!(iter.incomplete());
        assert!(!iter.malformed());
        let mut malformed = AdStructures::new(&[2, 0xff, 1, 4, 3, 1, 2, 3]);
        assert_eq!(malformed.by_ref().count(), 2);
        assert!(malformed.malformed());
        assert!(!malformed.incomplete());
        assert_eq!(
            AdStructure {
                kind: 3,
                data: &[1, 2, 3]
            }
            .uuids()
            .count(),
            0
        );
        assert_eq!(
            AdStructure {
                kind: 5,
                data: &[0xfa, 0xff, 0, 0]
            }
            .uuids()
            .next(),
            Some(Uuid::from_u16(0xfffa))
        );
        assert!(AdStructure {
            kind: 0x21,
            data: &[1; 15]
        }
        .service()
        .is_none());
        assert_eq!(AdStructures::new(&[0, 255]).count(), 0);
    }
    #[test]
    fn contextual_patterns_mask_and_bounds() {
        let mut p = BytePattern {
            bytes: &[0x10, 0x19],
            mask: Some(&[0xf0, 0xff]),
            offset: 1,
            min_len: 3,
            max_len: Some(4),
        };
        assert!(p.manufacturer_matches(
            ManufacturerData {
                company_id: 0,
                payload: &[0, 0x12, 0x19]
            },
            0
        ));
        assert!(!p.manufacturer_matches(
            ManufacturerData {
                company_id: 1,
                payload: &[0, 0x12, 0x19]
            },
            0
        ));
        assert!(!p.matches(&[0, 0x12]));
        assert!(!p.matches(&[0, 0x12, 0x19, 0, 0]));
        assert!(p.service_matches(
            ServiceData {
                uuid: Uuid::from_u16(0xfffa),
                payload: &[0, 0x12, 0x19]
            },
            Uuid::from_u16(0xfffa)
        ));
        p.offset = usize::MAX;
        assert!(!p.matches(&[0, 0x12, 0x19]));
        p.offset = 0;
        p.mask = Some(&[]);
        assert!(!p.matches(&[0x10, 0x19, 0]));
    }
}
