//! One serial-owner thread survives async cancellation long enough to perform
//! bounded RPC teardown. No auto reconnect, generic app launch or arbitrary UART.
use crate::{
    control::ScanControl,
    health::{Phase, Scanner},
    observation::ScanEvent,
    receiver::{Feature, Receiver},
};
use airhound_flipper::bridge::State;
use airhound_hostd::{
    flipper_bridge::Event,
    flipper_rpc_session::{self, Desired, Output},
};
use serde_json::json;
use std::{
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tokio_serial::SerialPort as _;

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub async fn scan(
    port: String,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    scan_with(port, tx, control, |port| {
        let mut serial = tokio_serial::new(port, 115200)
            .timeout(Duration::from_millis(50))
            .open_native()
            .map_err(io::Error::other)?;
        serial
            .write_request_to_send(false)
            .map_err(io::Error::other)?;
        serial
            .write_data_terminal_ready(true)
            .map_err(io::Error::other)?;
        Ok(serial)
    })
    .await
}
async fn scan_with<T: Read + Write + 'static, F: FnOnce(&str) -> io::Result<T> + Send + 'static>(
    port: String,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
    open: F,
) -> io::Result<()> {
    scan_with_completion(
        port,
        tx,
        control,
        open,
        #[cfg(test)]
        None,
    )
    .await
}
async fn scan_with_completion<
    T: Read + Write + 'static,
    F: FnOnce(&str) -> io::Result<T> + Send + 'static,
>(
    port: String,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
    open: F,
    #[cfg(test)] completed: Option<oneshot::Sender<()>>,
) -> io::Result<()> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel = Cancel(cancelled.clone());
    let (done, finished) = oneshot::channel();
    std::thread::Builder::new()
        .name("airhound-flipper".into())
        .spawn(move || {
            let result = worker(&port, &tx, &control, &cancelled, open);
            control.set_bridge_identity(None);
            control.set_receiver_availability(if result.is_ok() {
                "stopped"
            } else {
                "unavailable"
            });
            control.set_health(
                Scanner::Wifi,
                if result.is_ok() {
                    Phase::Stopped
                } else {
                    Phase::Unavailable
                },
                if result.is_ok() {
                    "flipper_session_closed"
                } else {
                    "flipper_session_failed_stop_unverified"
                },
            );
            // worker's serial descriptor and lease have dropped before completion.
            let _ = done.send(result);
            #[cfg(test)]
            if let Some(completed) = completed {
                let _ = completed.send(());
            }
        })?;
    finished
        .await
        .map_err(|_| io::Error::other("Flipper source worker stopped unexpectedly"))?
}
fn worker<T: Read + Write, F: FnOnce(&str) -> io::Result<T>>(
    port: &str,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
    cancelled: &AtomicBool,
    open: F,
) -> io::Result<()> {
    let state = control.subscribe();
    if state.borrow().shutdown || cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let _lease = airhound_hostd::serial_lease::acquire(port)?;
    // The factory runs only after shared ownership is acquired. The transport
    // closes before this stack's lease guard, including cancellation cleanup.
    let mut serial = open(port)?;
    control.set_receiver(receiver(port));
    let mut nonce = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let mut identity = json!({"transport":"flipper_usb_rpc","bridge_protocol_version":1,"awok_firmware":null,"radio_loss":"unknown"});
    let mut packets = 0u64;
    let mut dropped = 0u64;
    let mut instance = 0u64;
    let mut last_packet_generation = None;
    let mut latest_status = serde_json::Value::Null;
    let result = flipper_rpc_session::run(
        &mut serial,
        nonce,
        || {
            let s = *state.borrow();
            Desired {
                enabled: s.enabled,
                shutdown: s.shutdown || cancelled.load(Ordering::Acquire) || tx.is_closed(),
            }
        },
        |event| {
            match event {
                Output::FailureStatus {
                    status,
                    stop_complete,
                } => {
                    latest_status = retain_failure_status(control, status, stop_complete);
                }
                Output::DeviceInfo(info) => {
                    identity["flipper_firmware"] =
                        json!({"evidence":"usb_cli_device_info","fields":info});
                    control.set_bridge_identity(Some(identity.clone()));
                }
                Output::Bridge(Event::Hello {
                    app_instance,
                    version,
                    revision,
                    modified,
                }) => {
                    instance = app_instance;
                    identity["app_instance"] = format!("{app_instance:016x}").into();
                    identity["application"] = json!({"name":"AirHound Flipper bridge","version":version,"source_revision":revision,"source_modified":modified,"compiled_firmware_abi":"87.1","evidence":"ahfb_hello"});
                    control.set_bridge_identity(Some(identity.clone()));
                    control.set_receiver_availability("available");
                }
                Output::Bridge(Event::Accepted(_)) => (),
                Output::Bridge(Event::Status(status)) => {
                    let name = match status.state {
                        State::Ready => "ready",
                        State::Preflight => "preflight",
                        State::Capturing => "capturing",
                        State::Stopping => "stopping",
                        State::Paused => "paused",
                        State::Stopped => "stopped",
                        State::Failed => "failed",
                    };
                    latest_status = json!({"state":name,"cause":status.cause,"uart_text_evidence":status.evidence,"stop_complete":reported_stop_complete(&status),"capture_generation":status.generation,"app_uptime_ms":status.app_uptime_ms,"uart_rx_bytes":status.uart_rx_bytes,"uart_dropped_bytes":status.uart_dropped_bytes,"uart_error_events":status.uart_error_events,"rpc_dropped_messages":status.rpc_dropped_messages,"unread_ring_bytes":status.unread_ring_bytes});
                    set_reported_health(
                        control,
                        status.state,
                        last_packet_generation == Some(status.generation),
                    );
                    let _=tx.try_send(ScanEvent::FirmwareStatus(json!({"type":"flipper_bridge_status","schema_version":1,"producer":latest_status,"host_dropped_observations":dropped,"radio_loss":"unknown"})));
                }
                Output::Bridge(Event::Raw {
                    packets: observations,
                    ..
                }) => {
                    for packet in observations {
                        last_packet_generation = Some(packet.generation);
                        let mut counts = (packets, dropped);
                        forward_packet(
                            packet,
                            instance,
                            tx,
                            control,
                            cancelled,
                            &latest_status,
                            &mut counts,
                        )?;
                        (packets, dropped) = counts;
                    }
                }
            }
            control.set_bridge_status(json!({"producer_last_report":latest_status,"host_dropped_observations":dropped,"emitted_packets":packets,"radio_loss":"unknown"}));
            Ok(())
        },
    );
    let _=tx.try_send(ScanEvent::FirmwareStatus(json!({"type":"flipper_bridge_status","schema_version":1,"state":if result.is_ok(){"finished"}else{"failed"},"producer_last_report":latest_status,"host_dropped_observations":dropped,"emitted_packets":packets,"radio_stop":if result.is_ok(){"see_uart_evidence"}else{"unverified"},"radio_loss":"unknown"})));
    control.set_bridge_status(json!({"state":if result.is_ok(){"finished"}else{"failed"},"producer_last_report":latest_status,"host_dropped_observations":dropped,"emitted_packets":packets,"radio_stop":if result.is_ok(){"see_uart_evidence"}else{"unverified"},"radio_loss":"unknown"}));
    result
}

fn reported_stop_complete(status: &airhound_flipper::bridge::Status) -> bool {
    status.evidence & airhound_flipper::bridge::STOP_REQUEST_EVIDENCE
        == airhound_flipper::bridge::STOP_REQUEST_EVIDENCE
        && matches!(status.state, State::Paused | State::Stopped | State::Failed)
        && status.uart_dropped_bytes == 0
        && status.uart_error_events == 0
        && status.rpc_dropped_messages == 0
        && status.unread_ring_bytes == 0
}

fn retain_failure_status(
    control: &ScanControl,
    status: airhound_flipper::bridge::Status,
    stop_complete: bool,
) -> serde_json::Value {
    set_reported_health(control, State::Failed, false);
    diagnostic_status(status, stop_complete)
}

fn diagnostic_status(
    status: airhound_flipper::bridge::Status,
    stop_complete: bool,
) -> serde_json::Value {
    // Keep the original clock and every reported counter even when the lifecycle
    // validator rejects this status. This is diagnostic evidence, not readiness.
    json!({
        "accepted_lifecycle":false,
        "state":format!("{:?}",status.state),
        "cause":status.cause,
        "uart_text_evidence":status.evidence,"stop_complete":stop_complete && reported_stop_complete(&status),
        "capture_generation":status.generation,
        "app_uptime_ms":status.app_uptime_ms,
        "uart_rx_bytes":status.uart_rx_bytes,
        "uart_dropped_bytes":status.uart_dropped_bytes,
        "uart_error_events":status.uart_error_events,
        "rpc_dropped_messages":status.rpc_dropped_messages,
        "unread_ring_bytes":status.unread_ring_bytes,
    })
}

fn set_reported_health(control: &ScanControl, state: State, packet_verified: bool) {
    let availability = match state {
        State::Ready | State::Preflight | State::Capturing => "available",
        State::Stopping => "stopping",
        State::Paused => "paused",
        State::Stopped => "stopped",
        State::Failed => "unavailable",
    };
    let phase = match state {
        State::Paused => Phase::Paused,
        State::Stopped => Phase::Stopped,
        State::Failed => Phase::Unavailable,
        State::Capturing if packet_verified => Phase::Running,
        _ => Phase::Initializing,
    };
    // Inventory consults availability independently of scanner phase. Publish
    // failure/stop immediately, before bounded RPC teardown completes.
    control.set_receiver_availability(availability);
    control.set_health(Scanner::Wifi, phase, "flipper_bridge_reported_state");
}

fn receiver(port: &str) -> Receiver {
    Receiver {
        backend: "flipper_marauder",
        adapter_id: port.into(),
        adapter_info: "Flipper bridge identity pending".into(),
        firmware: None,
        controller_address: None,
        selection: "explicit",
        availability: "initializing",
        scan_configuration: "passive_beacon_device_managed",
        extended_advertising: Feature::default(),
        coded_rx: Feature::default(),
        controller_phy: None,
        capability_query: "rpc_bridge_reported",
        producer_identity: None,
        external_identity: None,
        bridge_identity: None,
        bridge_status: None,
    }
}

fn forward_packet(
    packet: airhound_hostd::flipper_bridge::Packet,
    instance: u64,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
    cancelled: &AtomicBool,
    latest_status: &serde_json::Value,
    counts: &mut (u64, u64),
) -> io::Result<()> {
    let state = control.subscribe();
    let current = state.borrow();
    if !current.enabled || current.shutdown || cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let event = ScanEvent::MarauderLive {
        packet,
        app_instance: instance,
        ingestion_generation: current.generation,
        received_at: chrono::Utc::now().to_rfc3339(),
    };
    match tx.try_send(event) {
        Ok(()) => {
            counts.0 += 1;
            control.set_health(
                Scanner::Wifi,
                Phase::Running,
                "flipper_marauder_packet_received",
            );
        }
        Err(mpsc::error::TrySendError::Full(_)) => counts.1 += 1,
        Err(mpsc::error::TrySendError::Closed(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Observation consumer closed",
            ))
        }
    }
    control.set_bridge_status(json!({"producer_last_report":latest_status,"host_dropped_observations":counts.1,"emitted_packets":counts.0,"radio_loss":"unknown"}));
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    struct Endpoint {
        _master: std::fs::File,
        _slave: std::fs::File,
        path: String,
    }
    impl Endpoint {
        fn new() -> Self {
            use std::os::fd::{AsRawFd, FromRawFd};
            let (mut master, mut slave) = (0, 0);
            let mut name = [0i8; 1024];
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
            let master = unsafe { std::fs::File::from_raw_fd(master) };
            let slave = unsafe { std::fs::File::from_raw_fd(slave) };
            for file in [&master, &slave] {
                assert_eq!(
                    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
                    0
                );
            }
            Self {
                _master: master,
                _slave: slave,
                path: unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                    .to_str()
                    .unwrap()
                    .into(),
            }
        }
    }
    #[tokio::test]
    async fn shared_owner_is_checked_before_open_and_pre_cancel_opens_nothing() {
        let endpoint = Endpoint::new();
        let _lease = airhound_hostd::serial_lease::acquire(&endpoint.path).unwrap();
        let (tx, _rx) = mpsc::channel(1);
        let control = ScanControl::new(true);
        assert!(scan_with(
            endpoint.path.clone(),
            tx.clone(),
            control.clone(),
            |_| -> io::Result<std::io::Cursor<Vec<u8>>> {
                panic!("competing source opened endpoint")
            }
        )
        .await
        .is_err());
        assert!(control.health_message()["receiver"].is_null());
        let control = ScanControl::new(true);
        control.shutdown();
        scan_with(
            endpoint.path,
            tx,
            control,
            |_| -> io::Result<std::io::Cursor<Vec<u8>>> {
                panic!("cancelled source opened endpoint")
            },
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn cancelled_future_retains_lease_until_owner_transport_is_closed() {
        crate::test_support::manual_time(async {
            struct Gate {
                entered: Option<oneshot::Sender<()>>,
                release: std::sync::mpsc::Receiver<()>,
                closed: Arc<AtomicBool>,
            }
            impl Read for Gate {
                fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                    if let Some(entered) = self.entered.take() {
                        let _ = entered.send(());
                    }
                    // A real worker-owned read remains blocked until the test
                    // releases it; no polling or arbitrary expiry opens this gate.
                    let _ = self.release.recv();
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "mock disconnect",
                    ))
                }
            }
            impl Write for Gate {
                fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                    panic!("no prompt received")
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            impl Drop for Gate {
                fn drop(&mut self) {
                    self.closed.store(true, Ordering::Release);
                }
            }
            let endpoint = Endpoint::new();
            let (entered, ready) = oneshot::channel();
            let (release, blocked) = std::sync::mpsc::channel();
            let (completed, done) = oneshot::channel();
            let closed = Arc::new(AtomicBool::new(false));
            let transport = Gate {
                entered: Some(entered),
                release: blocked,
                closed: closed.clone(),
            };
            let (tx, _rx) = mpsc::channel(1);
            let path = endpoint.path.clone();
            let task = tokio::spawn(scan_with_completion(
                path,
                tx,
                ScanControl::new(true),
                move |_| Ok(transport),
                Some(completed),
            ));
            ready.await.unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert!(!closed.load(Ordering::Acquire));
            assert!(airhound_hostd::serial_lease::acquire(&endpoint.path).is_err());
            release.send(()).unwrap();
            // This witness is emitted by the actual worker thread only after
            // both its transport and lease guards have dropped.
            done.await.unwrap();
            let _lease = airhound_hostd::serial_lease::acquire(&endpoint.path).unwrap();
            assert!(closed.load(Ordering::Acquire));
        })
        .await;
    }

    #[test]
    fn saturated_output_preserves_loss_in_health_and_finish_preempts_packets() {
        let control = ScanControl::new(true);
        control.set_receiver(receiver("/dev/test-fixture"));
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(ScanEvent::FirmwareStatus(json!({"fixture":true})))
            .unwrap();
        let cancelled = AtomicBool::new(false);
        let mut counts = (0, 0);
        let packet = || airhound_hostd::flipper_bridge::Packet {
            generation: 1,
            app_uptime_ms: 2,
            device_micros32: 3,
            clock_discontinuity: false,
            frame: vec![0x80],
        };
        let producer = diagnostic_status(
            airhound_flipper::bridge::Status {
                state: State::Failed,
                cause: 5,
                evidence: 7,
                generation: 1,
                app_uptime_ms: 9876,
                uart_rx_bytes: 12345,
                uart_dropped_bytes: 17,
                uart_error_events: 2,
                rpc_dropped_messages: 3,
                unread_ring_bytes: 4,
            },
            false,
        );
        assert_eq!(producer["app_uptime_ms"], 9876);
        assert_eq!(producer["accepted_lifecycle"], false);
        assert_eq!(producer["stop_complete"], false);
        assert_eq!(producer["uart_rx_bytes"], 12345);
        assert_eq!(producer["uart_error_events"], 2);
        assert_eq!(producer["rpc_dropped_messages"], 3);
        assert_eq!(producer["unread_ring_bytes"], 4);
        for _ in 0..8 {
            forward_packet(
                packet(),
                42,
                &tx,
                &control,
                &cancelled,
                &producer,
                &mut counts,
            )
            .unwrap();
        }
        assert_eq!(counts, (0, 8));
        let health = control.health_message();
        assert_eq!(
            health["receiver"]["bridge_status"]["host_dropped_observations"],
            8
        );
        assert_eq!(
            health["receiver"]["bridge_status"]["producer_last_report"],
            producer
        );
        control.shutdown();
        rx.try_recv().unwrap();
        forward_packet(
            packet(),
            42,
            &tx,
            &control,
            &cancelled,
            &producer,
            &mut counts,
        )
        .unwrap();
        assert!(rx.try_recv().is_err());
        assert_eq!(counts, (0, 8));
    }
    #[test]
    fn reported_completion_requires_both_bits_and_zero_final_loss() {
        let mut status = airhound_flipper::bridge::Status {
            state: State::Failed,
            cause: 3,
            evidence: 15,
            generation: 1,
            app_uptime_ms: 100,
            uart_rx_bytes: 100,
            uart_dropped_bytes: 0,
            uart_error_events: 0,
            rpc_dropped_messages: 0,
            unread_ring_bytes: 0,
        };
        assert_eq!(diagnostic_status(status, true)["stop_complete"], true);
        for evidence in [3, 7, 11] {
            status.evidence = evidence;
            assert_eq!(diagnostic_status(status, true)["stop_complete"], false);
        }
        status.evidence = 15;
        status.unread_ring_bytes = 1;
        assert_eq!(diagnostic_status(status, true)["stop_complete"], false);
        status.unread_ring_bytes = 0;
        status.uart_dropped_bytes = 1;
        assert_eq!(diagnostic_status(status, true)["stop_complete"], false);
        assert_eq!(diagnostic_status(status, true)["accepted_lifecycle"], false);
    }

    #[test]
    fn rejected_raw_completion_cannot_promote_receiver_health() {
        let control = ScanControl::new(true);
        control.set_receiver(receiver("/dev/fixture"));
        for state in [State::Paused, State::Failed] {
            let raw = airhound_flipper::bridge::Status {
                state,
                cause: 3,
                evidence: 15,
                generation: 99,
                app_uptime_ms: 100,
                uart_rx_bytes: 100,
                uart_dropped_bytes: 0,
                uart_error_events: 0,
                rpc_dropped_messages: 0,
                unread_ring_bytes: 0,
            };
            let Output::FailureStatus {
                status,
                stop_complete,
            } = (Output::FailureStatus {
                status: raw,
                stop_complete: false,
            })
            else {
                panic!()
            };
            let reported = retain_failure_status(&control, status, stop_complete);
            control.set_bridge_status(
                json!({"producer_last_report":reported,"radio_stop":"unverified"}),
            );
            let health = control.health_message();
            assert_eq!(health["receiver"]["availability"], "unavailable");
            assert_eq!(
                health["receiver"]["bridge_status"]["producer_last_report"]["uart_text_evidence"],
                15
            );
            assert_eq!(
                health["receiver"]["bridge_status"]["producer_last_report"]["stop_complete"],
                false
            );
            assert_eq!(
                health["receiver"]["bridge_status"]["producer_last_report"]["accepted_lifecycle"],
                false
            );
        }
    }

    #[test]
    fn lifecycle_availability_changes_before_transport_teardown_without_erasing_evidence() {
        let control = ScanControl::new(true);
        control.set_receiver(receiver("/dev/fixture"));
        control.set_bridge_identity(Some(json!({"application":{"evidence":"ahfb_hello"}})));
        control.set_bridge_status(
            json!({"host_dropped_observations":8,"producer_last_report":{"uart_dropped_bytes":17}}),
        );
        for (state, availability, phase) in [
            (State::Ready, "available", "initializing"),
            (State::Capturing, "available", "running"),
            (State::Stopping, "stopping", "initializing"),
            (State::Paused, "paused", "paused"),
            (State::Stopped, "stopped", "stopped"),
            (State::Failed, "unavailable", "unavailable"),
        ] {
            set_reported_health(&control, state, true);
            let health = control.health_message();
            assert_eq!(health["receiver"]["availability"], availability);
            assert_eq!(health["scanners"]["wifi"]["state"], phase);
            assert_eq!(
                health["receiver"]["bridge_status"]["host_dropped_observations"],
                8
            );
            assert_eq!(
                health["receiver"]["bridge_status"]["producer_last_report"]["uart_dropped_bytes"],
                17
            );
        }
        set_reported_health(&control, State::Capturing, false);
        assert_eq!(
            control.health_message()["scanners"]["wifi"]["state"],
            "initializing"
        );
    }
}
