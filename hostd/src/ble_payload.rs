//! Bounded desktop snapshots; OS properties may aggregate multiple advertisements.
use airhound::ble_data::{Advertisement, ManufacturerData, ServiceData, Uuid};
use btleplug::api::{AddressType, PeripheralProperties};
use serde::Serialize;

pub const MAX_UUIDS: usize = 64;
pub const MAX_RECORDS: usize = 16;
pub const MAX_RECORD_BYTES: usize = 1024;
pub const MAX_TOTAL_BYTES: usize = 4096;

#[derive(Clone, Debug, Serialize)]
pub struct ManufacturerRecord {
    pub company_id: u16,
    pub payload: Vec<u8>,
    pub original_len: usize,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct ServiceRecord {
    pub uuid: Uuid,
    pub payload: Vec<u8>,
    pub original_len: usize,
    pub truncated: bool,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Capacity {
    pub omitted_uuids: usize,
    pub omitted_manufacturer_records: usize,
    pub omitted_service_records: usize,
    pub truncated_payloads: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub provenance: &'static str,
    pub freshness: &'static str,
    pub completeness: &'static str,
    pub rssi_available: bool,
    pub address_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver_id: Option<String>,
    pub advertised_services: Vec<Uuid>,
    pub manufacturer_data: Vec<ManufacturerRecord>,
    pub service_data: Vec<ServiceRecord>,
    pub capacity: Capacity,
}

// Select the lowest keys deterministically without copying an unbounded map.
fn lowest<T: Ord + Copy>(items: impl Iterator<Item = T>, limit: usize) -> Vec<T> {
    let mut result = Vec::with_capacity(limit);
    for item in items {
        match result.binary_search(&item) {
            Ok(_) => {}
            Err(index) if index < limit => {
                if result.len() == limit {
                    result.pop();
                }
                result.insert(index, item);
            }
            _ => {}
        }
    }
    result
}
impl Snapshot {
    pub fn from_properties(props: &PeripheralProperties) -> Self {
        let advertised_services = lowest(
            props.services.iter().map(|uuid| Uuid(*uuid.as_bytes())),
            MAX_UUIDS,
        );
        let manufacturer_keys = lowest(props.manufacturer_data.keys().copied(), MAX_RECORDS);
        let service_keys = lowest(props.service_data.keys().copied(), MAX_RECORDS);
        let mut capacity = Capacity {
            // Count source entries whose UUID was excluded; deduplication is not loss.
            omitted_uuids: props
                .services
                .iter()
                .filter(|uuid| !advertised_services.contains(&Uuid(*uuid.as_bytes())))
                .count(),
            omitted_manufacturer_records: props
                .manufacturer_data
                .len()
                .saturating_sub(manufacturer_keys.len()),
            omitted_service_records: props.service_data.len().saturating_sub(service_keys.len()),
            truncated_payloads: 0,
        };
        let mut budget = MAX_TOTAL_BYTES;
        let mut payload = |bytes: &[u8]| {
            let count = bytes.len().min(MAX_RECORD_BYTES).min(budget);
            budget -= count;
            let truncated = count < bytes.len();
            if truncated {
                capacity.truncated_payloads += 1;
            }
            (bytes[..count].to_vec(), truncated)
        };
        let manufacturer_data = manufacturer_keys
            .into_iter()
            .map(|company_id| {
                let bytes = &props.manufacturer_data[&company_id];
                let (payload, truncated) = payload(bytes);
                ManufacturerRecord {
                    company_id,
                    payload,
                    original_len: bytes.len(),
                    truncated,
                }
            })
            .collect();
        let service_data = service_keys
            .into_iter()
            .map(|uuid| {
                let bytes = &props.service_data[&uuid];
                let (payload, truncated) = payload(bytes);
                ServiceRecord {
                    uuid: Uuid(*uuid.as_bytes()),
                    payload,
                    original_len: bytes.len(),
                    truncated,
                }
            })
            .collect();
        let address_type = if cfg!(target_os = "macos") {
            "unknown"
        } else {
            match props.address_type {
                Some(AddressType::Public) => "public",
                Some(AddressType::Random) => "random",
                None => "unknown",
            }
        };
        Self {
            provenance: "os_property_snapshot",
            freshness: "possibly_cached",
            completeness: "unknown",
            address_type,
            rssi_available: props.rssi.is_some(),
            receiver_id: None,
            advertised_services,
            manufacturer_data,
            service_data,
            capacity,
        }
    }
    pub fn with_view<T>(&self, consume: impl FnOnce(&Advertisement<'_>) -> T) -> T {
        // Exclude locally truncated payloads from matching; retained bytes remain visible.
        let manufacturer_data: heapless::Vec<_, MAX_RECORDS> = self
            .manufacturer_data
            .iter()
            .filter(|r| !r.truncated)
            .map(|r| ManufacturerData {
                company_id: r.company_id,
                payload: &r.payload,
            })
            .collect();
        let service_data: heapless::Vec<_, MAX_RECORDS> = self
            .service_data
            .iter()
            .filter(|r| !r.truncated)
            .map(|r| ServiceData {
                uuid: r.uuid,
                payload: &r.payload,
            })
            .collect();
        consume(&Advertisement {
            service_uuids: &self.advertised_services,
            manufacturer_data: &manufacturer_data,
            service_data: &service_data,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn multiple_records_company_zero_full_uuids_and_capacity_are_preserved() {
        let mut props = PeripheralProperties::default();
        let custom = uuid::Uuid::parse_str("1234fffa-1122-3344-5566-778899aabbcc").unwrap();
        props.services = vec![custom, uuid::Uuid::from_bytes(Uuid::from_u16(0xfffa).0)];
        props.manufacturer_data.insert(0, vec![]);
        props.manufacturer_data.insert(0x4c, vec![0x12, 0x19]);
        props.service_data.insert(custom, vec![7, 8]);
        let snapshot = Snapshot::from_properties(&props);
        assert_eq!(snapshot.manufacturer_data[0].company_id, 0);
        assert_eq!(snapshot.manufacturer_data[1].payload, [0x12, 0x19]);
        assert_eq!(snapshot.service_data[0].uuid.0, *custom.as_bytes());
        assert_eq!(snapshot.completeness, "unknown");
        assert!(snapshot.with_view(|v| v.has_service(Uuid(*custom.as_bytes()))));
        for id in 0..32 {
            props
                .manufacturer_data
                .insert(id, vec![0xaa; MAX_RECORD_BYTES + 1]);
        }
        let snapshot = Snapshot::from_properties(&props);
        assert_eq!(snapshot.manufacturer_data.len(), MAX_RECORDS);
        assert_eq!(snapshot.capacity.omitted_manufacturer_records, 17);
        assert!(
            snapshot
                .manufacturer_data
                .iter()
                .map(|r| r.payload.len())
                .sum::<usize>()
                <= MAX_TOTAL_BYTES
        );
        assert!(snapshot.capacity.truncated_payloads > 0);
        assert_eq!(snapshot.with_view(|v| v.manufacturer_data.len()), 0);
        let text = serde_json::to_string(&snapshot).unwrap();
        assert!(text.contains("possibly_cached"));
    }
    #[test]
    fn duplicate_uuids_are_not_capacity_loss_and_unknown_signal_is_explicit() {
        let props = PeripheralProperties {
            services: vec![uuid::Uuid::from_bytes(Uuid::from_u16(7).0); 100],
            ..Default::default()
        };
        let snapshot = Snapshot::from_properties(&props);
        assert_eq!(snapshot.capacity.omitted_uuids, 0);
        assert!(!snapshot.rssi_available);
        assert_eq!(snapshot.advertised_services.len(), 1);
    }

    #[test]
    fn deterministic_subset_and_no_short_uuid_collision() {
        let mut a = PeripheralProperties::default();
        let mut b = PeripheralProperties::default();
        for n in 0..80 {
            a.services.push(uuid::Uuid::from_bytes(Uuid::from_u32(n).0));
        }
        b.services = a.services.iter().rev().copied().collect();
        assert_eq!(
            serde_json::to_value(Snapshot::from_properties(&a)).unwrap(),
            serde_json::to_value(Snapshot::from_properties(&b)).unwrap()
        );
        assert_eq!(Snapshot::from_properties(&a).capacity.omitted_uuids, 16);
        let custom = Uuid([
            0, 0, 0xff, 0xfa, 1, 0, 0x10, 0, 0x80, 0, 0, 0x80, 0x5f, 0x9b, 0x34, 0xfb,
        ]);
        a.services = vec![uuid::Uuid::from_bytes(custom.0)];
        assert!(!Snapshot::from_properties(&a).with_view(|v| v.has_service(Uuid::from_u16(0xfffa))));
    }
}
