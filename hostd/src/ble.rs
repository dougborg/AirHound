//! BLE scanner using btleplug — converts peripheral discoveries into AirHound BleEvents.

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use futures_lite::StreamExt;
use tokio::sync::mpsc;

use crate::ScanEvent;
use airhound::scanner::BleEvent;

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
    control: crate::control::ScanControl,
) -> Result<(), btleplug::Error> {
    #[cfg(all(target_os = "macos", feature = "macos-private-addresses"))]
    let resolver = &crate::address_macos::AddressResolver::new();
    let mut state = control.subscribe();
    while crate::control::wait_running(&mut state, &tx).await {
        let mut events = adapter.events().await?;
        adapter.start_scan(ScanFilter::default()).await?;
        log::info!("BLE scan started");
        let finished = forward_events(
            &mut events,
            tx.clone(),
            control.clone(),
            |event| async {
                use btleplug::api::CentralEvent;
                match event {
                    CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => {
                        let peripheral = adapter.peripheral(&id).await.ok()?;
                        let props = peripheral.properties().await.ok().flatten()?;
                        let id = id.to_string();
                        Some((id, props))
                    }
                    _ => None,
                }
            },
            |id| async move {
                #[cfg(all(target_os = "macos", feature = "macos-private-addresses"))]
                {
                    resolver.lookup(&id).await
                }
                #[cfg(not(all(target_os = "macos", feature = "macos-private-addresses")))]
                {
                    let _ = id;
                    None
                }
            },
        )
        .await;
        // Explicitly stop CoreBluetooth/BlueZ before dropping the event receiver.
        let stopped = adapter.stop_scan().await;
        log::info!("BLE scan stopped");
        stopped?;
        if finished {
            break;
        }
    }
    Ok(())
}

// Keep enrichment bounded independently of btleplug's small discovery buffer.
// A full queue loses optional address enrichment, never the observation itself.
const ENRICHMENT_CAPACITY: usize = 8;

async fn forward_events<S, E, F, Fut, A, Address>(
    mut events: S,
    tx: mpsc::Sender<ScanEvent>,
    control: crate::control::ScanControl,
    mut properties: F,
    mut address: A,
) -> bool
where
    S: futures_lite::Stream<Item = E> + Unpin,
    F: FnMut(E) -> Fut,
    Fut: std::future::Future<Output = Option<(String, btleplug::api::PeripheralProperties)>>,
    A: FnMut(String) -> Address,
    Address: std::future::Future<Output = Option<[u8; 6]>>,
{
    let mut ingest_state = control.subscribe();
    let mut enrich_state = control.subscribe();
    let initial = *ingest_state.borrow_and_update();
    if !initial.enabled || initial.shutdown {
        return false;
    }
    let (pending, mut enrichment) = mpsc::channel(ENRICHMENT_CAPACITY);
    let output = tx.clone();
    let ingest = async move {
        loop {
            if ingest_state.borrow().generation != initial.generation {
                return false;
            }
            let event = tokio::select! {
                _ = tx.closed() => return true,
                _ = ingest_state.changed() => return false,
                event = events.next() => match event { Some(event) => event, None => return true },
            };
            let props = tokio::select! {
                _ = tx.closed() => return true,
                _ = ingest_state.changed() => return false,
                props = properties(event) => props,
            };
            if let Some(props) = props {
                match pending.try_send(props) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full((id, props))) => {
                        if !send_in_generation(
                            &tx,
                            &mut ingest_state,
                            initial.generation,
                            observation(&id, &props, None),
                        )
                        .await
                        {
                            return false;
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return false,
                }
            }
        }
    };
    let enrich = async move {
        loop {
            let current = *enrich_state.borrow_and_update();
            // A pause invalidates queued properties, even if start followed it
            // before this task was polled. Shutdown instead drains them below.
            if !same_scan_generation(current, initial.generation) {
                break;
            }
            let (id, props) = tokio::select! {
                _ = output.closed() => break,
                _ = enrich_state.changed() => continue,
                props = enrichment.recv() => match props { Some(props) => props, None => break },
            };
            let resolved = if current.shutdown {
                None
            } else {
                tokio::select! {
                    _ = output.closed() => break,
                    _ = enrich_state.changed() => {
                        let current = *enrich_state.borrow();
                        if !current.shutdown || !same_scan_generation(current, initial.generation) { break; }
                        None
                    },
                    resolved = address(id.clone()) => resolved,
                }
            };
            if !send_in_generation(
                &output,
                &mut enrich_state,
                initial.generation,
                observation(&id, &props, resolved),
            )
            .await
            {
                break;
            }
        }
    };
    // Both futures share task ownership. Shutdown bypasses unfinished lookups
    // and drains admitted observations; pause drops old-generation enrichment.
    let (finished, ()) = tokio::join!(ingest, enrich);
    finished
}

// Only an immediate shutdown can drain this generation. A pause followed by
// resume/shutdown must not resurrect observations admitted before the pause.
fn same_scan_generation(current: crate::control::State, generation: u64) -> bool {
    current.generation == generation
        || (current.shutdown && current.generation == generation.wrapping_add(1))
}

// Wait for output capacity without pinning old-generation observations across
// a pause. Shutdown still drains observations already admitted to this task.
async fn send_in_generation(
    tx: &mpsc::Sender<ScanEvent>,
    state: &mut tokio::sync::watch::Receiver<crate::control::State>,
    generation: u64,
    event: ScanEvent,
) -> bool {
    loop {
        let current = *state.borrow_and_update();
        if !same_scan_generation(current, generation) {
            return false;
        }
        tokio::select! {
            _ = state.changed() => {},
            permit = tx.reserve() => {
                let Ok(permit) = permit else { return false; };
                let current = *state.borrow();
                if !same_scan_generation(current, generation) { return false; }
                permit.send(event);
                return true;
            },
        }
    }
}

fn observation(
    id: &str,
    props: &btleplug::api::PeripheralProperties,
    address: Option<[u8; 6]>,
) -> ScanEvent {
    let event = convert_properties(props);
    if cfg!(target_os = "macos") || event.mac == [0; 6] {
        ScanEvent::BlePeripheral {
            event,
            id: id.to_owned(),
            address,
        }
    } else {
        ScanEvent::Ble(event)
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
            futures_lite::stream::iter([None, Some(("test-id".into(), props))]),
            tx,
            crate::control::ScanControl::new(true),
            std::future::ready,
            |_| std::future::ready(None),
        ));
        let event = rx.recv().await.unwrap();
        let event = match event {
            ScanEvent::Ble(event) | ScanEvent::BlePeripheral { event, .. } => event,
            _ => panic!("expected BLE"),
        };
        assert_eq!(event.mac, [0xaa, 0xbb, 0xcc, 0x11, 0x22, 0x33]);
        assert!(rx.recv().await.is_none());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn consumer_close_cancels_idle_property_fetch_and_backpressure() {
        for mode in 0..4 {
            let (tx, rx) = mpsc::channel(1);
            let task = tokio::spawn(async move {
                if mode == 0 {
                    forward_events(
                        futures_lite::stream::pending::<()>(),
                        tx,
                        crate::control::ScanControl::new(true),
                        |_| std::future::ready(None),
                        move |_| async move {
                            if mode == 3 {
                                std::future::pending().await
                            } else {
                                None
                            }
                        },
                    )
                    .await;
                } else {
                    forward_events(
                        futures_lite::stream::repeat(()),
                        tx,
                        crate::control::ScanControl::new(true),
                        move |_| async move {
                            if mode == 1 {
                                std::future::pending().await
                            } else {
                                Some(("test-id".into(), make_props()))
                            }
                        },
                        move |_| async move {
                            if mode == 3 {
                                std::future::pending().await
                            } else {
                                None
                            }
                        },
                    )
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
    // Mirrors btleplug's 16-slot broadcast stream, including skipped lag errors.
    fn discovery_stream(
        receive: tokio::sync::broadcast::Receiver<usize>,
    ) -> std::pin::Pin<Box<impl futures_lite::Stream<Item = usize>>> {
        Box::pin(futures_lite::stream::unfold(
            receive,
            |mut receive| async move {
                loop {
                    match receive.recv().await {
                        Ok(id) => return Some((id, receive)),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        ))
    }

    #[tokio::test(start_paused = true)]
    async fn slow_enrichment_preserves_burst_discoveries_exactly_once() {
        for delay in [0, 20] {
            let (send, receive) = tokio::sync::broadcast::channel(16);
            let (output, mut observations) = mpsc::channel(128);
            let task = tokio::spawn(forward_events(
                discovery_stream(receive),
                output,
                crate::control::ScanControl::new(true),
                |id| std::future::ready(Some((id.to_string(), PeripheralProperties::default()))),
                move |_| async move {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    Some([1; 6])
                },
            ));
            for id in 0..100 {
                send.send(id).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            drop(send);
            assert!(task.await.unwrap());
            let mut ids = std::collections::HashSet::new();
            let mut enriched = 0;
            while let Some(event) = observations.recv().await {
                let ScanEvent::BlePeripheral { id, address, .. } = event else {
                    panic!("expected UUID observation");
                };
                assert!(ids.insert(id), "observation must be emitted exactly once");
                enriched += usize::from(address.is_some());
            }
            assert_eq!(ids.len(), 100, "lookup delay must not lose discoveries");
            assert!(enriched > 0);
            if delay > 0 {
                assert!(
                    enriched < 100,
                    "saturated enrichment must fall back to UUID"
                );
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_enrichment_flushes_on_shutdown_but_is_discarded_across_pause() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        for transition in 0..3 {
            let control = crate::control::ScanControl::new(true);
            let (send, receive) = tokio::sync::broadcast::channel(16);
            let (output, mut observations) = mpsc::channel(128);
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let task = tokio::spawn(forward_events(
                discovery_stream(receive),
                output,
                control.clone(),
                |id| std::future::ready(Some((id.to_string(), PeripheralProperties::default()))),
                move |_| {
                    count.fetch_add(1, Ordering::Relaxed);
                    std::future::pending()
                },
            ));
            for id in 0..100 {
                send.send(id).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            // Only one active lookup plus eight queued observations are retained.
            assert_eq!(observations.len(), 100 - ENRICHMENT_CAPACITY - 1);
            if transition == 1 {
                control.shutdown();
            } else {
                control.store(false, Ordering::Relaxed);
                control.store(true, Ordering::Relaxed);
                if transition == 2 {
                    control.shutdown();
                }
            }
            assert!(
                !tokio::time::timeout(std::time::Duration::from_millis(1), task)
                    .await
                    .unwrap()
                    .unwrap()
            );
            let mut ids = std::collections::HashSet::new();
            while let Some(event) = observations.recv().await {
                let ScanEvent::BlePeripheral { id, address, .. } = event else {
                    panic!("expected UUID observation");
                };
                assert!(address.is_none());
                assert!(ids.insert(id));
            }
            assert_eq!(
                ids.len(),
                if transition == 1 {
                    100
                } else {
                    100 - ENRICHMENT_CAPACITY - 1
                }
            );
            drop(send);
        }
    }
    #[tokio::test]
    async fn pause_cancels_backpressured_output_and_does_not_finish_the_session() {
        for iteration in 0..20 {
            let control = crate::control::ScanControl::new(true);
            let (tx, mut rx) = mpsc::channel(1);
            let task = tokio::spawn(forward_events(
                futures_lite::stream::repeat(()),
                tx,
                control.clone(),
                |_| std::future::ready(Some(("id".into(), PeripheralProperties::default()))),
                |_| std::future::ready(None),
            ));
            tokio::task::yield_now().await;
            assert_eq!(rx.len(), 1, "exercise pause while output is full");
            control.store(false, std::sync::atomic::Ordering::Relaxed);
            control.store(true, std::sync::atomic::Ordering::Relaxed);
            if iteration % 2 == 0 {
                control.shutdown();
            }
            assert!(
                !tokio::time::timeout(std::time::Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap(),
                "pause must allow the scan loop to restart"
            );
            assert!(rx.recv().await.is_some());
            assert!(
                rx.recv().await.is_none(),
                "old-generation output must not appear after resume"
            );
        }
        let control = crate::control::ScanControl::new(false);
        let (tx, _rx) = mpsc::channel(1);
        assert!(
            !forward_events(
                futures_lite::stream::repeat(()),
                tx,
                control,
                |_| async { panic!("paused scanner must not read properties") },
                |_| async { panic!("paused scanner must not enrich") },
            )
            .await
        );
    }
}
