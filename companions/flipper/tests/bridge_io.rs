use airhound_flipper::{
    bridge::{Command, Header, State},
    bridge_control::{Controller, UartAction},
    bridge_io::{self, Output},
};
#[derive(Debug, PartialEq)]
enum Event {
    Status(State, u16),
    Raw(Vec<u8>),
    Uart(UartAction),
}
#[derive(Default)]
struct Fake {
    events: Vec<Event>,
    status_fails: bool,
    raw_fails: bool,
    uart_fails: bool,
}
impl Output for Fake {
    fn status(&mut self, c: &Controller) -> bool {
        self.events.push(Event::Status(c.state, c.evidence));
        !self.status_fails
    }
    fn raw(&mut self, _: &Controller, b: &[u8]) -> bool {
        self.events.push(Event::Raw(b.to_vec()));
        !self.raw_fails
    }
    fn uart(&mut self, _: &Controller, a: UartAction) -> bool {
        self.events.push(Event::Uart(a));
        !self.uart_fails
    }
}
fn preflight() -> Controller {
    let mut c = Controller::new(42, 0);
    let mut h = Header {
        nonce: [7; 16],
        request_id: 1,
        app_instance: 0,
        sequence: 0,
    };
    c.request(h, Command::Hello, 0).unwrap();
    h.request_id = 2;
    h.app_instance = 42;
    c.request(h, Command::StartBeacon, 0).unwrap();
    c
}
#[test]
fn old_preflight_data_precedes_ready_boundary_and_start() {
    let mut c = preflight();
    let mut out = Fake::default();
    let mut b = b"Stopping WiFi tran/recv\r\n[BUF/BEGIN]".to_vec();
    b.extend([0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]);
    b.extend([0; 8]);
    b.extend(4096u32.to_le_bytes());
    b.extend(105u32.to_le_bytes());
    for n in [1u32, 0, 1, 1] {
        b.extend(n.to_le_bytes())
    }
    b.push(0);
    b.extend(b"[BUF/CLOSE]");
    bridge_io::received(&mut c, 1, &b, &mut out);
    bridge_io::poll(&mut c, 51, false, &mut out);
    assert_eq!(
        out.events,
        vec![
            Event::Raw(b),
            Event::Status(State::Preflight, 1),
            Event::Uart(UartAction::StartBeacon)
        ]
    );
}
#[test]
fn pending_isr_ring_blocks_quiet_but_never_suppresses_timeout() {
    let mut c = preflight();
    let mut out = Fake::default();
    c.feed(1, b"Stopping WiFi tran/recv\r\n");
    bridge_io::poll(&mut c, 100, true, &mut out);
    assert!(out.events.is_empty());
    bridge_io::poll(&mut c, 1499, true, &mut out);
    assert!(out.events.is_empty());
    bridge_io::poll(&mut c, 1500, true, &mut out);
    assert!(out.events.contains(&Event::Uart(UartAction::Stop)));
    assert_eq!(c.cause, 7);
}
#[test]
fn failed_boundary_enqueue_never_sends_start_and_still_sends_stop() {
    let mut c = preflight();
    let mut out = Fake {
        status_fails: true,
        ..Fake::default()
    };
    c.feed(1, b"Stopping WiFi tran/recv\r\n");
    bridge_io::poll(&mut c, 51, false, &mut out);
    assert_eq!(
        out.events,
        vec![
            Event::Status(State::Preflight, 1),
            Event::Uart(UartAction::Stop)
        ]
    );
    assert_eq!(c.cause, 8);
}
#[test]
fn raw_queue_loss_and_stop_status_loss_still_stop_uart() {
    let mut c = preflight();
    let mut out = Fake {
        status_fails: true,
        raw_fails: true,
        ..Fake::default()
    };
    bridge_io::received(&mut c, 1, b"old raw", &mut out);
    assert_eq!(out.events.last(), Some(&Event::Uart(UartAction::Stop)));
    assert_eq!(c.state, State::Stopping);
}
#[test]
fn unavailable_uart_never_becomes_successful_stop() {
    let mut c = preflight();
    let mut out = Fake {
        uart_fails: true,
        ..Fake::default()
    };
    bridge_io::dispatch(&mut c, 1, UartAction::Stop, &mut out);
    assert_eq!(c.cause, 5);
    bridge_io::poll(&mut c, 1001, false, &mut out);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.evidence & 4, 0);
}
#[test]
fn fresh_isr_arrival_between_drain_and_poll_defers_start() {
    let mut c = preflight();
    let mut out = Fake::default();
    c.feed(1, b"Stopping WiFi tran/recv\r\n");
    // Ring looked empty at drain; ISR receipt/sequence advanced before polling.
    let pending = bridge_io::pending_rx(10, 11, false, 0);
    assert!(pending);
    bridge_io::poll(&mut c, 100, pending, &mut out);
    assert!(out.events.is_empty());
    assert!(bridge_io::pending_rx(11, 11, false, 49));
    assert!(!bridge_io::pending_rx(11, 11, false, 50));
    bridge_io::poll(&mut c, 150, false, &mut out);
    assert_eq!(
        out.events.last(),
        Some(&Event::Uart(UartAction::StartBeacon))
    );
}
#[test]
fn late_paused_input_requests_stop_and_fails_before_emitting_raw() {
    let mut c = preflight();
    let mut out = Fake::default();
    c.feed(1, b"Stopping WiFi tran/recv\r\n");
    c.poll(51);
    c.feed(52, b"StartingBeacon sniff. Stop with stopscan\r\n");
    c.poll(52);
    c.request(
        Header {
            request_id: 3,
            nonce: [7; 16],
            app_instance: 42,
            sequence: 0,
        },
        Command::Pause,
        100,
    )
    .unwrap();
    c.feed(101, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Paused);
    bridge_io::received(&mut c, 300, b"unexpected late input", &mut out);
    assert_eq!(out.events[0], Event::Status(State::Stopping, 3));
    assert_eq!(out.events[1], Event::Uart(UartAction::Stop));
    assert!(matches!(out.events[2], Event::Raw(_)));
    assert_eq!(c.cause, 6);
    assert_eq!(c.request_id(), 0);
    c.feed(301, b"\r\nStopping WiFi tran/recv\r\n");
    c.poll(400);
    assert_eq!(c.state, State::Failed);
}
