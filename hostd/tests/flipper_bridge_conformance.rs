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
                    capabilities: bridge::CAPABILITIES,
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
    assert_eq!(p.host.last_reported_status().unwrap().evidence, 15);
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

#[test]
fn paired_retry_reply_delays_pause_until_all_commands_are_acknowledged() {
    let mut p = Pair::new();
    p.start(10);
    p.command(Command::Pause, 2300);
    p.drain().unwrap();
    assert_eq!(p.poll(2800), Some(UartAction::Stop));
    p.drain().unwrap();
    p.feed(3036, b"Stopping WiFi tran/recv\r\n");
    p.drain().unwrap();
    p.poll(3036);
    p.drain().unwrap();
    p.feed(3436, b"console tail\r\n");
    p.drain().unwrap();
    p.poll(3486);
    p.drain().unwrap();
    assert_eq!(p.host.state(), Some(State::Stopping));
    // Synthetic second command reply: the actual failed run's late bytes
    // were not retained, so this does not claim to identify their contents.
    p.feed(3492, b"#stopscan\r\nStopping WiFi ");
    p.drain().unwrap();
    p.poll(3542);
    p.drain().unwrap();
    assert_eq!(p.host.state(), Some(State::Stopping));
    p.feed(3550, b"tran/recv\r\n> ");
    p.drain().unwrap();
    p.poll(3600);
    p.drain().unwrap();
    assert_eq!(p.host.state(), Some(State::Paused));
    assert_eq!(p.host.last_reported_status().unwrap().evidence, 15);
    p.start(3700);
    assert_eq!(p.producer.generation, 2);
    p.stopped(3900, Command::Stop, State::Stopped);
}

#[test]
fn paired_missing_retry_reply_preserves_specific_failed_diagnostic() {
    let mut p = Pair::new();
    p.start(10);
    p.command(Command::Pause, 100);
    p.drain().unwrap();
    assert_eq!(p.poll(600), Some(UartAction::Stop));
    p.drain().unwrap();
    p.feed(725, b"Stopping WiFi tran/recv\r\n");
    p.drain().unwrap();
    p.poll(725);
    p.drain().unwrap();
    p.poll(775);
    p.drain().unwrap();
    assert_eq!(p.host.state(), Some(State::Stopping));
    p.command(Command::Heartbeat, 2700);
    p.drain().unwrap();
    p.poll(2725);
    let error = p.drain().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Bridge stop deadline expired with outstanding acknowledgments"
    );
    let report = p.host.last_reported_status().unwrap();
    assert_eq!(report, p.producer.status(2725));
    assert_eq!(
        (report.cause, report.evidence, report.app_uptime_ms),
        (14, 7, 2725)
    );
    assert_eq!(p.host.state(), Some(State::Failed));
    assert!(p.host.command(Command::StartBeacon).is_err());
}

#[test]
fn paired_late_paused_input_is_distinct_from_malformed_framing() {
    let mut p = Pair::new();
    p.start(10);
    p.stopped(100, Command::Pause, State::Paused);
    // bridge_io raises this explicit cause before retaining any late Raw.
    assert_eq!(p.producer.fail(206, 13), Some(UartAction::Stop));
    p.status(206);
    let error = p.drain().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Bridge received unexpected UART input while paused"
    );
    let report = p.host.last_reported_status().unwrap();
    assert_eq!(report, p.producer.status(206));
    assert_eq!(
        (report.state, report.cause, report.evidence),
        (State::Stopping, 13, 3)
    );
    assert_eq!(p.host.state(), Some(State::Failed));
    assert!(p.host.command(Command::StartBeacon).is_err());
}

#[test]
fn paired_watchdog_completion_distinguishes_deadline_and_retains_final_loss_revocation() {
    for missing_retry in [false, true] {
        let mut p = Pair::new();
        p.start(10);
        assert_eq!(p.poll(3010), Some(UartAction::Stop));
        assert!(p.drain().is_err());
        assert_eq!(p.host.last_reported_status().unwrap().evidence, 3);
        if missing_retry {
            assert_eq!(p.poll(3510), Some(UartAction::Stop));
            while let Some(bytes) = p.queued.pop_front() {
                p.host.receive_diagnostic(&bytes).unwrap();
            }
        }
        let ack_at = if missing_retry { 3600 } else { 3011 };
        p.producer.feed(ack_at, b"Stopping WiFi tran/recv\r\n");
        p.poll(ack_at);
        while let Some(bytes) = p.queued.pop_front() {
            p.host.receive_diagnostic(&bytes).unwrap();
        }
        let terminal_at = if missing_retry { 5600 } else { 3110 };
        p.poll(terminal_at);
        while let Some(bytes) = p.queued.pop_front() {
            p.host.receive_diagnostic(&bytes).unwrap();
        }
        let terminal = p.host.last_reported_status().unwrap();
        assert_eq!(terminal.state, State::Failed);
        assert_eq!(terminal.cause, 3);
        assert_eq!(terminal.evidence & 4, 4);
        assert_eq!(
            terminal.evidence & bridge::STOP_COMPLETE != 0,
            !missing_retry
        );
        assert_eq!(p.host.state(), Some(State::Failed));
        assert!(p.host.command(Command::StartBeacon).is_err());
        if !missing_retry {
            // Final producer I/O checks can revoke completion after a controller
            // terminal transition. Keep the latest counters, not stale proof.
            p.producer.fail(3111, 5);
            let mut final_loss = p.producer.status(3111);
            final_loss.uart_dropped_bytes = 1;
            assert_eq!(final_loss.evidence, 7);
            p.queue(0, Body::Status(final_loss));
            let bytes = p.queued.pop_front().unwrap();
            assert_eq!(p.host.receive_diagnostic(&bytes).unwrap(), Some(final_loss));
            assert_eq!(p.host.last_reported_status(), Some(final_loss));
            assert!(!p.host.diagnostic_stop_complete());
        }
    }
}

#[test]
fn paired_normal_pause_without_explicit_completion_is_rejected() {
    let mut p = Pair::new();
    p.start(10);
    p.command(Command::Pause, 100);
    p.drain().unwrap();
    let mut incomplete = p.producer.status(200);
    incomplete.state = State::Paused;
    incomplete.evidence = 7;
    p.queue(p.producer.request_id(), Body::Status(incomplete));
    assert!(p.drain().is_err());
    assert_eq!(p.host.state(), Some(State::Failed));
}

#[test]
fn paired_stale_pause_ack_cannot_validate_autonomous_completion() {
    let mut p = Pair::new();
    p.start(10);
    p.stopped(100, Command::Pause, State::Paused);
    let mut stale = p.producer.status(300);
    stale.state = State::Stopping;
    stale.cause = 3;
    stale.evidence = 7; // The previous Pause text ACK was never cleared.
    p.queue(0, Body::Status(stale));
    assert!(p.drain().is_err());
    assert!(!p.host.diagnostic_stop_complete());
    stale.state = State::Failed;
    stale.evidence = 15;
    stale.app_uptime_ms = 400;
    p.queue(0, Body::Status(stale));
    let bytes = p.queued.pop_front().unwrap();
    assert_eq!(p.host.receive_diagnostic(&bytes).unwrap(), Some(stale));
    assert!(!p.host.diagnostic_stop_complete());
    assert!(p.host.command(Command::StartBeacon).is_err());
}
