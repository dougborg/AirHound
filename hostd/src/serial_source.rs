//! Explicit firmware UART owner. Pause drains/discards observations while retaining
//! the port; it does not stop the device's autonomous radios or toggle its pins.
use crate::{
    control::ScanControl,
    health::{Phase, Scanner},
    observation::ScanEvent,
    receiver::{Feature, Receiver},
};
use airhound_hostd::device_stream::{self, Kind, MAX_LINE};
use serde_json::{json, Value};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
};
use tokio_serial::SerialPortBuilderExt;

#[derive(Default)]
struct Stats {
    records: u64,
    host_drops: u64,
    malformed: u64,
    oversized: u64,
    paused_discarded: u64,
    reconnects: u64,
    connection: u64,
    boot: u64,
    uptime: Option<u32>,
    producer: Option<Value>,
}
impl Stats {
    fn status(&self, state: &str) -> ScanEvent {
        ScanEvent::FirmwareStatus(json!({
            "type":"airhound_serial_status", "schema_version":1, "state":state,
            "records":self.records, "host_dropped_records":self.host_drops,
            "malformed_records":self.malformed,"oversized_lines":self.oversized,
            "paused_discarded_records":self.paused_discarded,"reconnect_attempts":self.reconnects,
            "connection_generation":self.connection,"boot_generation":self.boot,
            "producer_counters":self.producer,"radio_loss":"unknown",
            "pause_scope":"host_ingestion_only", "device_radios_controlled":false
        }))
    }
}
fn health(control: &ScanControl, phase: Phase, reason: &'static str) {
    control.set_health(Scanner::Wifi, phase, reason);
    control.set_health(Scanner::Ble, phase, reason);
}

pub async fn scan(
    port: &str,
    baud: u32,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    // Coordinate with every updated AirHound serial source before opening UART.
    // Keep the guard across pause/recovery; the transport closes before release.
    let _lease = airhound_hostd::serial_lease::acquire(port).inspect_err(|_| {
        health(&control, Phase::Unavailable, "serial_endpoint_unavailable");
    })?;
    let mut state = control.subscribe();
    let mut stats = Stats::default();
    let mut failures = 0;
    loop {
        if state.borrow().shutdown || tx.is_closed() {
            break;
        }
        // This explicit source may reset some UART-bridged boards on open.
        // No bootloader handshake, DTR/RTS manipulation, scan or sound command.
        let opened = tokio_serial::new(port, baud).open_native_async();
        let result = match opened {
            Ok(mut serial) => {
                // serialport opens exclusively by default (TIOCEXCL + advisory lock).
                stats.connection += 1;
                stats.boot += 1;
                stats.uptime = None;
                stats.producer = None;
                control.set_receiver(Receiver {
                    firmware: None,
                    producer_identity: None,
                    external_identity: None,
                    bridge_identity: None,
                    bridge_status: None,
                    backend: "airhound_serial",
                    adapter_id: port.into(),
                    adapter_info: "AirHound firmware identity pending status".into(),
                    controller_address: None,
                    selection: "explicit",
                    availability: "initializing",
                    scan_configuration: "device_managed",
                    extended_advertising: Feature::default(),
                    coded_rx: Feature::default(),
                    controller_phy: None,
                    capability_query: "firmware_reported",
                });
                let before = stats.records;
                let result = consume(&mut serial, port, &tx, &control, &mut stats).await;
                if stats.records > before {
                    failures = 0;
                }
                result
            }
            Err(error) => Err(io::Error::other(error)),
        };
        if state.borrow().shutdown || tx.is_closed() {
            break;
        }
        failures += 1;
        if failures >= 10 || stats.connection == 0 {
            control.clear_producer_identity();
            health(&control, Phase::Unavailable, "serial_transport_failed");
            control.set_receiver_availability("unavailable");
            let _ = tx.send(stats.status("failed")).await;
            return Err(result.err().unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "serial stream ended")
            }));
        }
        control.clear_producer_identity();
        stats.reconnects += 1;
        health(&control, Phase::Unavailable, "serial_reconnecting");
        control.set_receiver_availability("reconnecting");
        let _ = tx.send(stats.status("reconnecting")).await;
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(2))=>{},_=state.changed()=>{},_=tx.closed()=>break}
    }
    control.clear_producer_identity();
    health(
        &control,
        Phase::Stopped,
        "serial_transport_closed_device_may_continue",
    );
    control.set_receiver_availability("stopped");
    let _ = tx.send(stats.status("stopped")).await;
    Ok(())
}

async fn consume<R: AsyncRead + Unpin>(
    input: &mut R,
    port: &str,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
    stats: &mut Stats,
) -> io::Result<()> {
    let mut state = control.subscribe();
    // A reconnect can begin while already paused; watch::changed does not emit
    // the subscription's initial value, so publish that state before reading.
    let enabled = state.borrow().enabled;
    health(
        control,
        if enabled {
            Phase::Initializing
        } else {
            Phase::Paused
        },
        if enabled {
            "firmware_status_pending"
        } else {
            "serial_ingestion_paused_device_continues"
        },
    );
    control.set_receiver_availability(if enabled { "initializing" } else { "paused" });
    let mut line = Vec::with_capacity(MAX_LINE);
    let mut discard = false;
    let mut bytes = [0u8; 1024];
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let idle = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(idle);
    loop {
        let count = tokio::select! {
            _=tx.closed()=>return Ok(()),
            _=state.changed()=>{
                let current=*state.borrow_and_update();
                if current.shutdown{return Ok(());}
                line.clear();discard=true;
                health(control,if current.enabled{Phase::Initializing}else{Phase::Paused},if current.enabled{"serial_waiting_for_records"}else{"serial_ingestion_paused_device_continues"});
                control.set_receiver_availability(if current.enabled{"initializing"}else{"paused"});
                let _=tx.send(stats.status(if current.enabled{"resuming"}else{"paused"})).await;
                continue;
            },
            _=&mut idle=>return Err(io::Error::new(io::ErrorKind::TimedOut,"no AirHound firmware records for 60 seconds; check port, baud and firmware")),
            _=tick.tick()=>{let _=tx.try_send(stats.status(if state.borrow().enabled{"receiving"}else{"paused"}));continue;},
            result=input.read(&mut bytes)=>result?,
        };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AirHound serial disconnected",
            ));
        }
        for byte in &bytes[..count] {
            if *byte != b'\n' {
                if !discard {
                    if line.len() == MAX_LINE {
                        line.clear();
                        discard = true;
                        stats.oversized += 1;
                    } else {
                        line.push(*byte);
                    }
                }
                continue;
            }
            if discard {
                discard = false;
                line.clear();
                continue;
            }
            let parsed = device_stream::parse(&line);
            line.clear();
            let mut record = match parsed {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => {
                    stats.malformed += 1;
                    continue;
                }
            };
            idle.as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_secs(60));
            stats.records += 1;
            let current = *state.borrow();
            match &record.kind {
                Kind::Status {
                    board,
                    version,
                    uptime,
                    scanning,
                } => {
                    if stats.uptime.is_some_and(|old| *uptime < old) {
                        stats.boot += 1;
                        stats.producer = None;
                    }
                    stats.uptime = Some(*uptime);
                    if !scanning && current.enabled {
                        health(control, Phase::Paused, "firmware_scanning_disabled");
                    }
                    control.set_receiver(Receiver {
                        firmware: None,
                        external_identity: None,
                        bridge_identity: None,
                        bridge_status: None,
                        producer_identity: Some(crate::receiver::ProducerIdentity {
                            application: "AirHound",
                            version: version.clone(),
                            board: board.clone(),
                            evidence: "device_status",
                            connection_generation: stats.connection,
                            boot_generation: stats.boot,
                        }),
                        backend: "airhound_serial",
                        adapter_id: port.into(),
                        adapter_info: format!("AirHound {version} on {board} (reported)"),
                        controller_address: None,
                        selection: "explicit",
                        availability: if current.enabled {
                            "available"
                        } else {
                            "paused"
                        },
                        scan_configuration: "device_managed",
                        extended_advertising: Feature::default(),
                        coded_rx: Feature::default(),
                        controller_phy: None,
                        capability_query: "firmware_reported",
                    });
                }
                Kind::Stats => stats.producer = Some(record.value.clone()),
                _ => {}
            }
            record.connection_generation = stats.connection;
            record.boot_generation = stats.boot;
            if record.observation() && !current.enabled {
                stats.paused_discarded += 1;
                continue;
            }
            if record.observation() {
                control.set_health(
                    if matches!(record.kind, Kind::Wifi { .. }) {
                        Scanner::Wifi
                    } else {
                        Scanner::Ble
                    },
                    Phase::Running,
                    "serial_observations_received_device_managed",
                );
                control.set_receiver_availability("available");
                let event = ScanEvent::Firmware {
                    record,
                    generation: current.generation,
                };
                if let Err(error) = tx.try_send(event) {
                    if matches!(error, mpsc::error::TrySendError::Closed(_)) {
                        return Ok(());
                    }
                    stats.host_drops += 1;
                }
            } else {
                // Producer health/evidence remains visible during ingestion pause;
                // it is namespaced and never interpreted as current host control.
                let output = record.output(
                    &airhound::filter::FilterConfig::default(),
                    &airhound::mac_index::MacIndex::new(),
                );
                if let Err(error) = tx.try_send(ScanEvent::FirmwareStatus(output)) {
                    if matches!(error, mpsc::error::TrySendError::Closed(_)) {
                        return Ok(());
                    }
                    stats.host_drops += 1;
                }
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    const WIFI:&[u8]=b"{\"type\":\"wifi\",\"mac\":\"02:00:00:00:00:01\",\"ssid\":\"test\",\"rssi\":-40,\"ch\":1,\"frame\":\"beacon\",\"ts\":1}\n";
    #[tokio::test]
    async fn shared_lease_denies_reader_before_serial_open() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let (mut master, mut slave) = (-1, -1);
        let mut name = [0i8; 128];
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    name.as_mut_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let (_master, _slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        let port = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
            .to_str()
            .unwrap();
        let lease = airhound_hostd::serial_lease::acquire(port).unwrap();
        let control = ScanControl::new(true);
        let (tx, mut rx) = mpsc::channel(16);
        let error = scan(port, 115200, tx, control.clone()).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(control.health().wifi.state, Phase::Unavailable);
        assert_eq!(control.health().ble.state, Phase::Unavailable);
        assert!(
            rx.recv().await.is_none(),
            "denied source must emit no producer records"
        );
        assert!(airhound_hostd::serial_lease::acquire(port).is_err());
        drop(lease);
        assert!(airhound_hostd::serial_lease::acquire(port).is_ok());
    }

    #[tokio::test]
    async fn reconnect_while_paused_preserves_both_health_states() {
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let (tx, mut rx) = mpsc::channel(16);
        let control = ScanControl::new(false);
        health(&control, Phase::Initializing, "fixture_reconnecting");
        let c = control.clone();
        let task = tokio::spawn(async move {
            let mut stats = Stats::default();
            let result = consume(&mut reader, "fixture", &tx, &c, &mut stats).await;
            (stats, result)
        });
        writer.write_all(b"{\"type\":\"status\",\"board\":\"fixture\",\"version\":\"0.1.1\",\"uptime\":1,\"scanning\":true,\"heap_free\":3000}\n").await.unwrap();
        writer.write_all(WIFI).await.unwrap();
        drop(writer);
        let (stats, result) = task.await.unwrap();
        assert!(result.is_err());
        assert_eq!(stats.paused_discarded, 1);
        assert_eq!(control.health().wifi.state, Phase::Paused);
        assert_eq!(control.health().ble.state, Phase::Paused);
        assert_eq!(
            control.health_message()["receiver"]["availability"],
            "paused"
        );
        while let Some(event) = rx.recv().await {
            assert!(!matches!(event, ScanEvent::Firmware { .. }));
        }
    }

    #[tokio::test]
    async fn framing_recovers_from_oversized_and_malformed_lines() {
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let (tx, mut rx) = mpsc::channel(16);
        let control = ScanControl::new(true);
        let task = tokio::spawn(async move {
            let mut s = Stats::default();
            let result = consume(&mut reader, "fixture", &tx, &control, &mut s).await;
            (s, result)
        });
        writer.write_all(&vec![b'x'; MAX_LINE + 20]).await.unwrap();
        writer.write_all(b"\n{malformed}\n").await.unwrap();
        writer.write_all(WIFI).await.unwrap();
        drop(writer);
        let (mut s, result) = task.await.unwrap();
        assert!(result.is_err());
        assert_eq!(s.oversized, 1);
        assert_eq!(s.malformed, 1);
        assert_eq!(s.records, 1);
        let mut observations = 0;
        while let Some(e) = rx.recv().await {
            if matches!(e, ScanEvent::Firmware { .. }) {
                observations += 1;
            }
        }
        assert_eq!(observations, 1);
        s.boot = 2;
        assert!(matches!(s.status("stopped"), ScanEvent::FirmwareStatus(_)));
    }
    #[tokio::test]
    async fn reset_and_producer_counters_are_distinct_from_host_loss() {
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let (tx, mut rx) = mpsc::channel(16);
        let control = ScanControl::new(true);
        let c = control.clone();
        let task = tokio::spawn(async move {
            let mut s = Stats {
                connection: 1,
                boot: 1,
                ..Stats::default()
            };
            let result = consume(&mut reader, "fixture", &tx, &c, &mut s).await;
            (s, result)
        });
        for uptime in [70, 5] {
            let value = json!({"type":"status","board":"fixture","version":"0.1.1","uptime":uptime,"scanning":true,"heap_free":3000});
            writer
                .write_all(format!("{value}\n").as_bytes())
                .await
                .unwrap();
        }
        writer.write_all(b"{\"type\":\"stream_stats\",\"scan_drops\":3,\"output_drops\":4,\"ble_output_drops\":5,\"serialize_errors\":0}\n").await.unwrap();
        writer.write_all(WIFI).await.unwrap();
        drop(writer);
        let (s, r) = task.await.unwrap();
        assert!(r.is_err());
        assert_eq!(s.boot, 2);
        assert_eq!(s.host_drops, 0);
        assert_eq!(s.producer.unwrap()["scan_drops"], 3);
        let identity = control.health_message()["receiver"]["producer_identity"].clone();
        assert_eq!(identity["version"], "0.1.1");
        assert_eq!(identity["boot_generation"], 2);
        assert_eq!(control.health().wifi.state, Phase::Running);
        assert_ne!(control.health().ble.state, Phase::Running);
        let mut generations = vec![];
        while let Some(event) = rx.recv().await {
            if let ScanEvent::FirmwareStatus(v) = event {
                if v["type"] == "airhound_device_status" {
                    generations.push(v["boot_generation"].clone());
                }
            }
        }
        assert_eq!(generations, vec![json!(1), json!(2)]);
        control.clear_producer_identity();
        assert!(control.health_message()["receiver"]["producer_identity"].is_null());
    }

    #[tokio::test]
    async fn full_queue_counts_host_drop_and_disconnection_ends_reader() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(ScanEvent::FirmwareStatus(json!({}))).unwrap();
        writer.write_all(WIFI).await.unwrap();
        drop(writer);
        let mut stats = Stats::default();
        assert!(consume(
            &mut reader,
            "fixture",
            &tx,
            &ScanControl::new(true),
            &mut stats
        )
        .await
        .is_err());
        assert_eq!(stats.host_drops, 1);
        assert_eq!(stats.records, 1);
    }

    #[tokio::test]
    async fn pause_keeps_transport_open_and_discards_without_replaying() {
        crate::test_support::manual_time(async {
            async fn pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
                assert!(
                    std::future::poll_fn(|cx| std::task::Poll::Ready(
                        future.as_mut().poll(cx).is_pending()
                    ))
                    .await
                );
            }
            let (mut writer, mut input) = tokio::io::duplex(8192);
            let (tx, mut rx) = mpsc::channel(16);
            let control = ScanControl::new(true);
            let mut stats = Stats::default();
            {
                let mut reader =
                    Box::pin(consume(&mut input, "fixture", &tx, &control, &mut stats));
                writer.write_all(WIFI).await.unwrap();
                pending(reader.as_mut()).await;
                assert!(std::iter::from_fn(|| rx.try_recv().ok())
                    .any(|event| matches!(event, ScanEvent::Firmware { .. })));
                control.store(false, std::sync::atomic::Ordering::Relaxed);
                pending(reader.as_mut()).await;
                assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(
                    |event| matches!(event, ScanEvent::FirmwareStatus(v) if v["state"]=="paused")
                ));
                writer.write_all(b"\n").await.unwrap();
                writer.write_all(WIFI).await.unwrap();
                // This poll consumes the complete paused input and returns only
                // at the next empty read. Resume cannot overtake the discard.
                pending(reader.as_mut()).await;
                assert!(!std::iter::from_fn(|| rx.try_recv().ok())
                    .any(|event| matches!(event, ScanEvent::Firmware { .. })));
                control.store(true, std::sync::atomic::Ordering::Relaxed);
                pending(reader.as_mut()).await;
                assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(
                    |event| matches!(event, ScanEvent::FirmwareStatus(v) if v["state"]=="resuming")
                ));
                writer.write_all(b"\n").await.unwrap();
                writer.write_all(WIFI).await.unwrap();
                pending(reader.as_mut()).await;
                let generation =
                    std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
                        ScanEvent::Firmware { generation, .. } => Some(generation),
                        _ => None,
                    });
                assert_eq!(generation, Some(2));
                control.shutdown();
                reader.await.unwrap();
            }
            assert_eq!(stats.paused_discarded, 1);
            assert_eq!(stats.host_drops, 0);
        })
        .await;
    }
}
