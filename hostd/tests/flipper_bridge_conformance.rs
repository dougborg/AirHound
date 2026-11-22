//! Paired producer/host bytes, not independent mocks of either peer's contract.
use airhound_flipper::{
    bridge::{self, Body, Command, Header, Message, State},
    bridge_control::{Controller, UartAction},
};
use airhound_hostd::flipper_bridge::{Event, Session};
use std::{collections::VecDeque, io};

struct Pair {
    host: Session,
    producer: Controller,
    sequence: u32,
    queued: VecDeque<Vec<u8>>,
}
impl Pair {
    fn new() -> Self {
        let mut pair = Self {
            host: Session::new([7; 16]).unwrap(),
            producer: Controller::new(42, 0),
            sequence: 0,
            queued: VecDeque::new(),
        };
        assert_eq!(pair.command(Command::Hello, 0), None);
        pair.drain().unwrap();
        pair
    }
    fn queue(&mut self, request: u32, body: Body<'_>) {
        self.sequence += 1;
        let mut bytes = [0; bridge::MAX_MESSAGE];
        let n = bridge::encode(
            Message {
                header: Header {
                    request_id: request,
                    nonce: [7; 16],
                    app_instance: 42,
                    sequence: self.sequence,
                },
                body,
            },
            &mut bytes,
        )
        .unwrap();
        assert!(self.queued.len() < 16, "bounded mock sender");
        self.queued.push_back(bytes[..n].to_vec());
    }
    fn status(&mut self, now: u32) {
        self.queue(
            self.producer.request_id(),
            Body::Status(self.producer.status(now)),
        );
    }
    fn command(&mut self, command: Command, now: u32) -> Option<UartAction> {
        let bytes = self.host.command(command).unwrap();
        let m = bridge::decode(&bytes).unwrap();
        let Body::Command(c) = m.body else { panic!() };
        let accepted = self.producer.request(m.header, c, now).unwrap();
        if accepted.hello {
            self.queue(
                m.header.request_id,
                Body::Hello(bridge::Hello {
                    capabilities: 7,
                    api_major: 87,
                    api_minor: 1,
                    uart_baud: 115200,
                    watchdog_ms: 3000,
                    app_version: "0.1.0",
                    source_revision: "",
                    source_modified: 2,
                }),
            );
        } else {
            self.queue(m.header.request_id, Body::Ack { command, result: 0 });
        }
        if command != Command::Heartbeat {
            self.status(now);
        }
        accepted.uart
    }
    fn drain(&mut self) -> io::Result<Vec<Event>> {
        let mut events = vec![];
        while let Some(bytes) = self.queued.pop_front() {
            events.push(self.host.receive(&bytes)?);
        }
        Ok(events)
    }
    fn feed(&mut self, now: u32, bytes: &[u8]) {
        self.producer.feed(now, bytes);
        self.queue(
            0,
            Body::Raw {
                generation: self.producer.generation,
                app_uptime_ms: now,
                bytes,
            },
        );
    }
    fn poll(&mut self, now: u32) -> Option<UartAction> {
        let action = self.producer.poll(now);
        self.status(now);
        action
    }
    fn start(&mut self, now: u32) {
        assert_eq!(
            self.command(Command::StartBeacon, now),
            Some(UartAction::Stop)
        );
        self.drain().unwrap();
        self.feed(now + 1, b"Stopping WiFi tran/recv\r\n");
        self.drain().unwrap();
        assert_eq!(self.poll(now + 51), Some(UartAction::StartBeacon));
        self.drain().unwrap();
        self.feed(now + 52, b"StartingBeacon sniff. Stop with stopscan\r\n");
        self.drain().unwrap();
        self.poll(now + 52);
        self.drain().unwrap();
        assert_eq!(self.host.state(), Some(State::Capturing));
        let bytes = pcap();
        self.feed(now + 53, &bytes);
        let events = self.drain().unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event,Event::Raw {packets,..} if packets.len()==1)));
    }
    fn stopped(&mut self, now: u32, command: Command, target: State) {
        assert_eq!(self.command(command, now), Some(UartAction::Stop));
        self.drain().unwrap();
        self.feed(now + 1, b"Stopping WiFi tran/recv\r\n");
        self.drain().unwrap();
        self.poll(now + 100);
        self.drain().unwrap();
        assert_eq!(self.host.state(), Some(target));
    }
}
fn pcap() -> Vec<u8> {
    let mut b = b"[BUF/BEGIN]".to_vec();
    b.extend([0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]);
    for n in [0u32, 0, 4096, 105, 0, 123, 1, 1] {
        b.extend(n.to_le_bytes());
    }
    b.push(0x80);
    b.extend(b"[BUF/CLOSE]");
    b
}
#[test]
fn paired_capture_pause_finish_uses_fresh_stop_evidence() {
    let mut p = Pair::new();
    p.start(10);
    p.stopped(100, Command::Pause, State::Paused);
    p.stopped(300, Command::Stop, State::Stopped);
}
#[test]
fn paired_resume_and_prior_status_backlog_keep_generation_and_request_identity() {
    let mut p = Pair::new();
    p.start(10);
    p.status(99); // Old Capturing status was queued before Pause is received.
    p.stopped(100, Command::Pause, State::Paused);
    p.status(299); // Old Paused status precedes next accepted Start.
    p.start(300);
    assert_eq!(p.producer.generation, 2);
    p.command(Command::Heartbeat, 400);
    p.drain().unwrap();
    p.stopped(500, Command::Stop, State::Stopped);
}
#[test]
fn paired_unopened_stop_does_not_claim_uart_stop_and_watchdog_never_resumes() {
    let mut p = Pair::new();
    assert_eq!(p.command(Command::Stop, 1), None);
    p.drain().unwrap();
    assert_eq!(p.host.state(), Some(State::Stopped));
    assert_eq!(p.host.last_reported_status().unwrap().evidence, 0);
    let mut p = Pair::new();
    p.start(10);
    assert_eq!(p.poll(3010), Some(UartAction::Stop));
    assert!(p.drain().is_err());
    p.producer.feed(3011, b"Stopping WiFi tran/recv\r\n");
    p.producer.poll(3110);
    assert_eq!(p.producer.state, State::Failed);
    assert!(p.host.command(Command::StartBeacon).is_err());
}
#[test]
fn paired_stale_identity_and_sequence_loss_are_terminal() {
    for bad_nonce in [false, true] {
        let mut p = Pair::new();
        p.start(10);
        p.status(100);
        let mut bytes = p.queued.pop_front().unwrap();
        if bad_nonce {
            bytes[12] ^= 1;
        } else {
            bytes[36] = bytes[36].wrapping_add(1);
        }
        assert!(p.host.receive(&bytes).is_err());
        assert!(p.host.command(Command::Heartbeat).is_err());
    }
}

#[test]
fn paired_finish_escalates_pending_pause_without_reusing_evidence_or_extending_deadline() {
    let mut p = Pair::new();
    p.start(10);
    assert_eq!(p.command(Command::Pause, 100), Some(UartAction::Stop));
    p.drain().unwrap();
    p.feed(101, b"Stopping WiFi tran/recv\r\n");
    p.drain().unwrap();
    p.poll(102);
    p.drain().unwrap(); // acknowledgment seen, quiet/drain not yet complete
    assert_eq!(p.producer.evidence & 4, 4);
    assert_eq!(p.command(Command::Stop, 150), Some(UartAction::Stop));
    p.drain().unwrap();
    assert_eq!(p.producer.evidence & 4, 0);
    assert!(p.host.command(Command::Stop).is_err());
    // No fresh Stop response: old pause response cannot establish success.
    p.poll(1150); // Fresh Stop has its own 1s acknowledgment deadline.
    assert_eq!(p.producer.state, State::Failed);
    assert!(p.drain().is_err());
    assert_eq!(p.host.last_reported_status().unwrap().evidence & 4, 0);
    assert_eq!(p.host.last_reported_status().unwrap().cause, 10);
}

#[test]
fn paired_stop_failures_retain_specific_diagnostic_evidence_without_resuming() {
    for (tail, cause, message) in [
        (None, 10, "Bridge stop acknowledgment deadline expired"),
        (
            Some(b"[BUF/BEGIN]".as_slice()),
            11,
            "Bridge stop drain ended with an incomplete frame",
        ),
        (
            Some(b"console\r\n".as_slice()),
            12,
            "Bridge stop drain deadline expired before quiet completion",
        ),
    ] {
        let mut p = Pair::new();
        p.start(10);
        assert_eq!(p.command(Command::Pause, 100), Some(UartAction::Stop));
        p.drain().unwrap();
        let request = p.producer.request_id();
        let deadline = if let Some(tail) = tail {
            p.feed(725, b"Stopping WiFi tran/recv\r\n");
            p.drain().unwrap();
            p.poll(725);
            p.drain().unwrap();
            p.feed(800, tail);
            p.drain().unwrap();
            if cause == 12 {
                p.producer.pending_input(2725);
            }
            2725
        } else {
            1100
        };
        p.poll(deadline);
        assert_eq!(p.producer.request_id(), request);
        let error = p.drain().unwrap_err();
        assert_eq!(error.to_string(), message);
        let reported = p.host.last_reported_status().unwrap();
        assert_eq!(reported, p.producer.status(deadline));
        assert_eq!(reported.cause, cause);
        assert_eq!(reported.app_uptime_ms, deadline);
        assert_eq!(reported.state, State::Failed);
        assert_eq!(p.host.state(), Some(State::Failed));
        assert!(p.host.command(Command::StartBeacon).is_err());
    }
}

#[test]
fn paired_queued_tail_remains_parseable_after_old_stop_deadline() {
    let mut p = Pair::new();
    p.start(10);
    p.command(Command::Pause, 100);
    p.drain().unwrap();
    p.feed(725, b"Stopping WiFi tran/recv\r\n");
    p.drain().unwrap();
    p.poll(725);
    p.drain().unwrap();
    let mut bytes = b"[BUF/BEGIN]".to_vec();
    // The scan's global PCAP header was already received by start().
    // A finite ~16 KiB stress tail, not a claim about the attached board's RAM.
    for _ in 0..8 {
        for n in [0u32, 123, 2000, 2000] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend([0x80; 2000]);
    }
    bytes.extend(b"[BUF/CLOSE]");
    let mut packets = 0;
    let mut now = 725;
    for chunk in bytes.chunks(115) {
        now += 10; // <= 115200 baud, 8N1.
        p.feed(now, chunk);
        for event in p.drain().unwrap() {
            if let Event::Raw { packets: batch, .. } = event {
                packets += batch.len();
            }
        }
        p.poll(now);
        p.drain().unwrap();
        assert_eq!(p.host.state(), Some(State::Stopping));
    }
    assert!(now > 1100 && now < 3100);
    p.poll(now + 50);
    p.drain().unwrap();
    assert_eq!(packets, 8);
    assert_eq!(p.host.state(), Some(State::Paused));
    assert_eq!(p.host.last_reported_status().unwrap().evidence, 7);
    p.stopped(now + 100, Command::Stop, State::Stopped);
}

#[test]
fn paired_transport_cancellation_stops_locally_despite_queued_host_events() {
    let mut p = Pair::new();
    p.start(10);
    for now in 100..110 {
        p.status(now);
    }
    // Host transport cancellation cannot require draining the outbound queue.
    assert_eq!(p.producer.fail(110, 4), Some(UartAction::Stop));
    p.producer.feed(111, b"Stopping WiFi tran/recv\r\n");
    p.producer.poll(210);
    assert_eq!(p.producer.state, State::Failed);
    assert_eq!(p.producer.evidence & 4, 4);
    p.status(210);
    assert!(p.drain().is_err());
    assert_eq!(p.host.last_reported_status().unwrap().cause, 4);
    assert!(p.host.command(Command::StartBeacon).is_err());
}
