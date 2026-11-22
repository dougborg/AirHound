use airhound_flipper::{
    bridge::{Command, Header, State},
    bridge_control::{Controller, UartAction},
};
fn h(id: u32, instance: u64) -> Header {
    Header {
        request_id: id,
        nonce: [7; 16],
        app_instance: instance,
        sequence: 0,
    }
}
fn ready(now: u32) -> Controller {
    let mut c = Controller::new(42, now);
    assert!(c.request(h(1, 0), Command::Hello, now).unwrap().hello);
    c
}
fn running(now: u32) -> Controller {
    let mut c = ready(now);
    assert_eq!(
        c.request(h(2, 42), Command::StartBeacon, now).unwrap().uart,
        Some(UartAction::Stop)
    );
    c.feed(now.wrapping_add(1), b"Stopping WiFi tran/recv\r\n");
    assert_eq!(c.poll(now.wrapping_add(51)), Some(UartAction::StartBeacon));
    assert_eq!(c.state, State::Preflight);
    assert_eq!(c.evidence, 1);
    c.feed(
        now.wrapping_add(52),
        b"StartingBeacon sniff. Stop with stopscan\r\n",
    );
    c.poll(now.wrapping_add(52));
    assert_eq!(c.state, State::Capturing);
    c
}
#[test]
fn hello_and_unbound_controls_never_start_uart() {
    let mut c = Controller::new(42, 0);
    assert!(c.request(h(1, 42), Command::StartBeacon, 0).is_err());
    assert_eq!(c.state, State::Ready);
    assert_eq!(c.request(h(1, 0), Command::Hello, 0).unwrap().uart, None);
    assert_eq!(c.request(h(2, 42), Command::Stop, 1).unwrap().uart, None);
    assert_eq!(c.state, State::Stopped);
    assert!(c.request(h(3, 42), Command::StartBeacon, 2).is_err());
}
#[test]
fn preflight_needs_exact_ack_quiet_and_complete_frame() {
    let mut c = ready(0);
    c.request(h(2, 42), Command::StartBeacon, 0).unwrap();
    c.feed(1, b"#\xf3stopscan\r\n> ");
    assert_eq!(c.poll(100), None);
    assert_eq!(c.evidence, 0);
    c.feed(110, b"\r\nStopping WiFi tran/recv\r\n[BUF/");
    assert_eq!(c.poll(500), None);
    assert_eq!(c.poll(1500), Some(UartAction::Stop));
    assert_eq!(c.cause, 7);
    assert_eq!(c.evidence & 2, 0);
    c.poll(2500);
    assert_eq!(c.state, State::Failed);
}
#[test]
fn duplicate_or_invalid_controls_cannot_refresh_watchdog() {
    let mut c = running(0);
    assert!(c.request(h(2, 42), Command::StartBeacon, 2900).is_err());
    let mut bad = h(3, 42);
    bad.nonce = [8; 16];
    assert!(c.request(bad, Command::Heartbeat, 2999).is_err());
    assert_eq!(c.poll(3000), Some(UartAction::Stop));
    assert_eq!(c.cause, 3);
    assert_eq!(c.request_id(), 0);
    c.feed(3001, b"Stopping WiFi tran/recv\r\n");
    c.poll(3100);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.evidence & 4, 4);
    assert!(c.request(h(4, 42), Command::StartBeacon, 3200).is_err());
}
#[test]
fn heartbeat_does_not_replace_lifecycle_request_and_clock_wraps() {
    let now = u32::MAX - 100;
    let mut c = running(now);
    assert_eq!(
        c.request(h(3, 42), Command::Heartbeat, now.wrapping_add(2500))
            .unwrap()
            .uart,
        None
    );
    assert_eq!(c.request_id(), 2);
    assert_eq!(c.poll(now.wrapping_add(5499)), None);
    assert_eq!(c.poll(now.wrapping_add(5500)), Some(UartAction::Stop));
}
#[test]
fn pause_resume_requires_new_preflight_generation_and_stop_ack() {
    let mut c = running(0);
    assert_eq!(
        c.request(h(3, 42), Command::Pause, 100).unwrap().uart,
        Some(UartAction::Stop)
    );
    assert_eq!(c.request_id(), 3);
    assert!(c.request(h(4, 42), Command::StartBeacon, 101).is_err());
    c.feed(102, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Paused);
    assert_eq!(
        c.request(h(5, 42), Command::StartBeacon, 201).unwrap().uart,
        Some(UartAction::Stop)
    );
    assert_eq!(c.generation, 2);
    assert_eq!(c.evidence, 0);
    assert_eq!(c.poll(300), None);
    c.feed(301, b"Stopping WiFi tran/recv\r\n");
    assert_eq!(c.poll(351), Some(UartAction::StartBeacon));
}
#[test]
fn queue_failure_before_start_never_requires_sending_start() {
    let mut c = ready(0);
    c.request(h(2, 42), Command::StartBeacon, 0).unwrap();
    c.feed(1, b"Stopping WiFi tran/recv\r\n");
    assert_eq!(c.poll(51), Some(UartAction::StartBeacon)); // caller cannot enqueue pre-action status
    assert_eq!(c.fail(51, 8), Some(UartAction::Stop));
    assert_eq!(c.state, State::Stopping);
    assert_eq!(c.evidence & 2, 0);
    c.feed(52, b"Stopping WiFi tran/recv\r\n");
    c.poll(151);
    assert_eq!(c.state, State::Failed);
}
#[test]
fn loss_during_pause_teardown_cannot_become_clean_paused() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Pause, 100).unwrap();
    assert_eq!(c.fail(101, 5), None);
    c.feed(102, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.cause, 5);
}
#[test]
fn missing_ack_has_one_retry_then_bounded_unknown_failure() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Stop, 100).unwrap();
    assert_eq!(c.poll(599), None);
    assert_eq!(c.poll(600), Some(UartAction::Stop));
    assert_eq!(c.poll(601), None);
    c.poll(1100);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.evidence & 4, 0);
}
#[test]
fn final_transport_loss_after_stop_cannot_retain_clean_success() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Stop, 100).unwrap();
    c.feed(101, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Stopped);
    assert_eq!(c.fail(201, 5), None);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.evidence & 4, 4);
}
