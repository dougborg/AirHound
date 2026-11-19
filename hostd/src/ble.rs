//! BLE scanner using btleplug — converts peripheral discoveries into AirHound BleEvents.

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use futures_lite::StreamExt;
use tokio::sync::mpsc;

use airhound::scanner::{BleEvent, ScanEvent};

/// Discover the first available BLE adapter.
pub async fn get_adapter() -> Result<Adapter, btleplug::Error> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    adapters
        .into_iter()
        .next()
        .ok_or(btleplug::Error::DeviceNotFound)
}

/// Run the BLE scan loop, converting btleplug events into AirHound `BleEvent`s.
///
/// Sends discovered events on `tx`. Runs until the channel is closed or an error occurs.
pub async fn scan_loop(
    adapter: Adapter,
    tx: mpsc::Sender<ScanEvent>,
) -> Result<(), btleplug::Error> {
    adapter.start_scan(ScanFilter::default()).await?;
    log::info!("BLE scan started");

    let events = adapter.events().await?;
    forward_events(events, tx, |event| async {
        use btleplug::api::CentralEvent;
        match event {
            CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => {
                let peripheral = adapter.peripheral(&id).await.ok()?;
                peripheral.properties().await.ok().flatten()
            }
            _ => None,
        }
    })
    .await;
    Ok(())
}

async fn forward_events<S, E, F, Fut>(mut events: S, tx: mpsc::Sender<ScanEvent>, mut properties: F)
where
    S: futures_lite::Stream<Item = E> + Unpin,
    F: FnMut(E) -> Fut,
    Fut: std::future::Future<Output = Option<btleplug::api::PeripheralProperties>>,
{
    loop {
        let event = tokio::select! {
            _ = tx.closed() => break,
            event = events.next() => match event { Some(event) => event, None => break },
        };
        let props = tokio::select! {
            _ = tx.closed() => break,
            props = properties(event) => props,
        };
        if let Some(props) = props {
            if tx
                .send(ScanEvent::Ble(convert_properties(&props)))
                .await
                .is_err()
            {
                break;
            }
        }
    }
}

/// Convert btleplug `PeripheralProperties` into an AirHound `BleEvent`.
fn convert_properties(props: &btleplug::api::PeripheralProperties) -> BleEvent {
    let mac = address_to_bytes(&props.address);

    let mut name = heapless::String::new();
    if let Some(ref local_name) = props.local_name {
        // Truncate to 33 bytes (NameString capacity), respecting UTF-8 char boundaries
        let end = if local_name.len() <= 33 {
            local_name.len()
        } else {
            local_name.floor_char_boundary(33)
        };
        let _ = name.push_str(&local_name[..end]);
    }

    let rssi = props
        .rssi
        .unwrap_or(0)
        .clamp(i8::MIN as i16, i8::MAX as i16) as i8;

    // Extract 16-bit service UUIDs
    let mut service_uuids_16 = heapless::Vec::new();
    for uuid in &props.services {
        // Check if this is a 16-bit UUID (Bluetooth Base UUID pattern)
        let uuid_bytes = uuid.as_bytes();
        // 16-bit UUIDs have the form 0000XXXX-0000-1000-8000-00805f9b34fb
        if uuid_bytes[4..]
            == [
                0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb,
            ]
            && uuid_bytes[0..2] == [0x00, 0x00]
        {
            let short = u16::from_be_bytes([uuid_bytes[2], uuid_bytes[3]]);
            let _ = service_uuids_16.push(short);
        }
    }

    // Extract manufacturer ID (first key if any)
    let manufacturer_id = props.manufacturer_data.keys().next().copied().unwrap_or(0);

    // raw_ad is not available from btleplug on macOS (CoreBluetooth abstracts it)
    let raw_ad = heapless::Vec::new();

    BleEvent {
        mac,
        name,
        rssi,
        service_uuids_16,
        manufacturer_id,
        raw_ad,
    }
}

/// Convert a btleplug `BDAddr` to a 6-byte array.
#[inline]
fn address_to_bytes(addr: &btleplug::api::BDAddr) -> [u8; 6] {
    addr.into_inner()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use btleplug::api::{BDAddr, PeripheralProperties};
    use std::collections::HashMap;
    use uuid::Uuid;

    fn make_props() -> PeripheralProperties {
        PeripheralProperties {
            address: BDAddr::from([0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33]),
            ..Default::default()
        }
    }

    #[test]
    fn convert_empty_properties() {
        let props = make_props();
        let event = convert_properties(&props);
        assert_eq!(event.mac, [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33]);
        assert!(event.name.is_empty());
        assert_eq!(event.rssi, 0);
        assert!(event.service_uuids_16.is_empty());
        assert_eq!(event.manufacturer_id, 0);
        assert!(event.raw_ad.is_empty());
    }

    #[test]
    fn convert_local_name() {
        let props = PeripheralProperties {
            local_name: Some("FS Ext Battery".to_string()),
            ..make_props()
        };
        let event = convert_properties(&props);
        assert_eq!(event.name.as_str(), "FS Ext Battery");
    }

    #[test]
    fn convert_long_name_truncated() {
        let long_name = "A".repeat(50);
        let props = PeripheralProperties {
            local_name: Some(long_name),
            ..make_props()
        };
        let event = convert_properties(&props);
        assert_eq!(event.name.len(), 33);
    }

    #[test]
    fn convert_multibyte_name_truncated_at_char_boundary() {
        // 'é' is 2 bytes in UTF-8; 16 × 2 = 32 bytes, adding one more 'é' would be 34
        let name = "é".repeat(17); // 34 bytes
        let props = PeripheralProperties {
            local_name: Some(name),
            ..make_props()
        };
        let event = convert_properties(&props);
        // Should truncate to 32 bytes (16 chars), not panic at byte 33
        assert_eq!(event.name.len(), 32);
        assert!(core::str::from_utf8(event.name.as_bytes()).is_ok());
    }

    #[test]
    fn convert_rssi() {
        let props = PeripheralProperties {
            rssi: Some(-65),
            ..make_props()
        };
        let event = convert_properties(&props);
        assert_eq!(event.rssi, -65);
    }

    #[test]
    fn convert_16bit_service_uuid() {
        // 0x3100 as a full 128-bit Bluetooth Base UUID
        let uuid = Uuid::from_u128(0x00003100_0000_1000_8000_00805f9b34fb);
        let props = PeripheralProperties {
            services: vec![uuid],
            ..make_props()
        };
        let event = convert_properties(&props);
        assert_eq!(event.service_uuids_16.len(), 1);
        assert_eq!(event.service_uuids_16[0], 0x3100);
    }

    #[test]
    fn convert_non_16bit_uuid_skipped() {
        // A custom 128-bit UUID (not from the Bluetooth Base range)
        let uuid = Uuid::from_u128(0x4a690001_1c4a_4e3c_b5d8_f47b2e1c0a9d);
        let props = PeripheralProperties {
            services: vec![uuid],
            ..make_props()
        };
        let event = convert_properties(&props);
        assert!(event.service_uuids_16.is_empty());
    }

    #[test]
    fn convert_manufacturer_data() {
        let mut mfr = HashMap::new();
        mfr.insert(0x09C8u16, vec![0x01, 0x02]);
        let props = PeripheralProperties {
            manufacturer_data: mfr,
            ..make_props()
        };
        let event = convert_properties(&props);
        assert_eq!(event.manufacturer_id, 0x09C8);
    }

    #[test]
    fn convert_no_manufacturer_data() {
        let props = make_props();
        let event = convert_properties(&props);
        assert_eq!(event.manufacturer_id, 0);
    }

    #[test]
    fn convert_address_roundtrip() {
        let addr = BDAddr::from([0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]);
        assert_eq!(
            address_to_bytes(&addr),
            [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]
        );
    }
    #[tokio::test]
    async fn event_stream_skips_missing_properties_and_forwards_discoveries() {
        let (tx, mut rx) = mpsc::channel(1);
        let props = make_props();
        let task = tokio::spawn(forward_events(
            futures_lite::stream::iter([None, Some(props)]),
            tx,
            std::future::ready,
        ));
        let ScanEvent::Ble(event) = rx.recv().await.unwrap() else {
            panic!("expected BLE")
        };
        assert_eq!(event.mac, [0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]);
        assert!(rx.recv().await.is_none());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn consumer_close_cancels_idle_property_fetch_and_backpressure() {
        for mode in 0..3 {
            let (tx, rx) = mpsc::channel(1);
            let task = tokio::spawn(async move {
                if mode == 0 {
                    forward_events(futures_lite::stream::pending::<()>(), tx, |_| {
                        std::future::ready(None)
                    })
                    .await;
                } else {
                    forward_events(futures_lite::stream::repeat(()), tx, move |_| async move {
                        if mode == 1 {
                            std::future::pending().await
                        } else {
                            Some(make_props())
                        }
                    })
                    .await;
                }
            });
            tokio::task::yield_now().await;
            drop(rx);
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
