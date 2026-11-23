//! Host-side AHFB lifecycle validation. No transport or radio side effects.
//! Acknowledgments only accept commands; UART evidence and packet framing remain
//! separate. Any loss poisons this app session instead of splicing a new stream.
use airhound_flipper::bridge::{self, Binding, Body, Command, Header, Message, State, Status};
use std::io;

pub struct Session {
    nonce: [u8; 16],
    binding: Binding,
    next_request: u32,
    pending: Option<(u32, Command)>,
    lifecycle: Option<(u32, Command)>,
    state: Option<State>,
    status: Option<Status>,
    last_reported_status: Option<Status>,
    status_request: u32,
    stop_boundary: Option<u32>,
    generation: u32,
    parser: Option<crate::marauder_stream::Decoder>,
    last_device_timestamp: Option<u32>,
    failed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub generation: u32,
    /// FAP UART receipt uptime, not an AWOK clock or wall timestamp.
    pub app_uptime_ms: u32,
    pub device_micros32: u32,
    /// A decrease cannot distinguish micros() wrap from a radio reset.
    pub clock_discontinuity: bool,
    pub frame: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Hello {
        app_instance: u64,
        version: String,
        revision: String,
        modified: u8,
    },
    Accepted(Command),
    Status(Status),
    Raw {
        packets: Vec<Packet>,
        ignored_preflight_bytes: usize,
    },
}

impl Session {
    pub fn new(nonce: [u8; 16]) -> io::Result<Self> {
        Ok(Self {
            nonce,
            binding: Binding::new(nonce, 1).map_err(|_| invalid("Invalid bridge nonce"))?,
            next_request: 0,
            pending: None,
            lifecycle: None,
            state: None,
            status: None,
            last_reported_status: None,
            status_request: 0,
            stop_boundary: None,
            generation: 0,
            parser: None,
            last_device_timestamp: None,
            failed: false,
        })
    }

    pub fn state(&self) -> Option<State> {
        self.state
    }
    pub fn instance(&self) -> u64 {
        self.binding.instance()
    }
    /// Last wire-valid, session-bound producer status, including failure/loss.
    /// It is diagnostic evidence, not proof that the lifecycle was accepted.
    pub fn last_reported_status(&self) -> Option<Status> {
        self.last_reported_status
    }
    pub fn awaiting_ack(&self) -> bool {
        self.pending.is_some()
    }

    pub fn command(&mut self, command: Command) -> io::Result<Vec<u8>> {
        if self.failed || self.pending.is_some() {
            return Err(invalid("Bridge failed or acknowledgment pending"));
        }
        let allowed = match command {
            Command::Hello => self.next_request == 0 && self.state.is_none(),
            Command::StartBeacon => {
                matches!(self.state, Some(State::Ready | State::Paused))
                    && !matches!(self.lifecycle, Some((_, Command::StartBeacon)))
            }
            Command::Pause => {
                matches!(self.state, Some(State::Preflight | State::Capturing))
                    && !matches!(self.lifecycle, Some((_, Command::Pause | Command::Stop)))
            }
            Command::Stop => {
                self.state.is_some()
                    && !matches!(self.state, Some(State::Stopped | State::Failed))
                    && !matches!(self.lifecycle, Some((_, Command::Stop)))
            }
            Command::Heartbeat => {
                self.state.is_some() && !matches!(self.state, Some(State::Stopped | State::Failed))
            }
        };
        if !allowed {
            return Err(invalid("Invalid bridge command state"));
        }
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or_else(|| invalid("Bridge request IDs exhausted"))?;
        let mut bytes = [0; bridge::MAX_MESSAGE];
        let length = bridge::encode(
            Message {
                header: Header {
                    request_id: self.next_request,
                    nonce: self.nonce,
                    app_instance: self.binding.instance(),
                    sequence: 0,
                },
                body: Body::Command(command),
            },
            &mut bytes,
        )
        .map_err(|_| invalid("Invalid outgoing bridge message"))?;
        self.pending = Some((self.next_request, command));
        Ok(bytes[..length].to_vec())
    }

    pub fn receive(&mut self, bytes: &[u8]) -> io::Result<Event> {
        if self.failed {
            return Err(invalid("Bridge session already failed"));
        }
        let result = self.receive_inner(bytes);
        if result.is_err() {
            self.failed = true;
            self.parser = None;
            self.state = Some(State::Failed);
        }
        result
    }

    fn receive_inner(&mut self, bytes: &[u8]) -> io::Result<Event> {
        let message = bridge::decode(bytes).map_err(|_| invalid("Invalid bridge wire message"))?;
        self.binding
            .accept(message)
            .map_err(|_| invalid("Bridge identity or sequence mismatch"))?;
        match message.body {
            Body::Hello(hello) => {
                if self.pending != Some((message.header.request_id, Command::Hello))
                    || hello.capabilities != bridge::CAPABILITIES
                    || (hello.api_major, hello.api_minor) != (87, 1)
                {
                    return Err(invalid("Unsupported bridge capabilities or compiled ABI"));
                }
                self.pending = None;
                self.state = Some(State::Ready);
                Ok(Event::Hello {
                    app_instance: message.header.app_instance,
                    version: hello.app_version.into(),
                    revision: hello.source_revision.into(),
                    modified: hello.source_modified,
                })
            }
            Body::Ack { command, result } => {
                if self.pending != Some((message.header.request_id, command)) || result != 0 {
                    return Err(invalid(
                        "Bridge rejected command or acknowledgment mismatched",
                    ));
                }
                self.pending = None;
                if matches!(
                    command,
                    Command::StartBeacon | Command::Pause | Command::Stop
                ) {
                    self.lifecycle = Some((message.header.request_id, command));
                }
                Ok(Event::Accepted(command))
            }
            Body::Status(status) => self.accept_status(message.header.request_id, status),
            Body::Raw {
                generation,
                app_uptime_ms,
                bytes,
            } => {
                if generation != self.generation
                    || !matches!(
                        self.state,
                        Some(State::Preflight | State::Capturing | State::Stopping)
                    )
                {
                    return Err(invalid(
                        "Raw bridge bytes outside current capture generation",
                    ));
                }
                let Some(parser) = &mut self.parser else {
                    return Ok(Event::Raw {
                        packets: vec![],
                        ignored_preflight_bytes: bytes.len(),
                    });
                };
                let mut packets = Vec::new();
                let last = &mut self.last_device_timestamp;
                parser.feed(bytes, |record| {
                    let discontinuity =
                        last.is_some_and(|previous| record.timestamp_micros32 < previous);
                    *last = Some(record.timestamp_micros32);
                    packets.push(Packet {
                        generation,
                        app_uptime_ms,
                        device_micros32: record.timestamp_micros32,
                        clock_discontinuity: discontinuity,
                        frame: record.frame.to_vec(),
                    });
                    Ok(())
                })?;
                Ok(Event::Raw {
                    packets,
                    ignored_preflight_bytes: 0,
                })
            }
            Body::Command(_) => Err(invalid("Unexpected bridge host command")),
        }
    }

    fn accept_status(&mut self, request: u32, status: Status) -> io::Result<Event> {
        self.last_reported_status = Some(status);
        let previous_state = self.state.ok_or_else(|| invalid("Status before Hello"))?;
        if let Some(previous) = self.status {
            if status.uart_rx_bytes < previous.uart_rx_bytes
                || status.uart_dropped_bytes < previous.uart_dropped_bytes
                || status.uart_error_events < previous.uart_error_events
                || status.rpc_dropped_messages < previous.rpc_dropped_messages
            {
                return Err(invalid("Bridge cumulative counters decreased"));
            }
        }
        if status.uart_dropped_bytes != 0
            || status.uart_error_events != 0
            || status.rpc_dropped_messages != 0
        {
            return Err(invalid("Bridge reports UART or transport loss"));
        }
        // Autonomous failures may report their final evidence, but cannot promote
        // the source to a successful paused/stopped state.
        if status.cause >= 3 || status.state == State::Failed {
            return Err(invalid(match status.cause {
                6 => "Bridge reported malformed UART framing",
                10 => "Bridge stop acknowledgment deadline expired",
                11 => "Bridge stop drain ended with an incomplete frame",
                12 => "Bridge stop drain deadline expired before quiet completion",
                13 => "Bridge received unexpected UART input while paused",
                14 => "Bridge stop deadline expired with outstanding acknowledgments",
                _ => "Bridge reported autonomous stop or failure",
            }));
        }
        let expected = match status.state {
            State::Ready => {
                if previous_state != State::Ready
                    || self.lifecycle.is_some()
                    || status.generation != 0
                    || status.evidence != 0
                    || status.cause != 0
                    || request != 0
                {
                    return Err(invalid("Unexpected bridge Ready status"));
                }
                None
            }
            State::Preflight => {
                if !matches!(
                    previous_state,
                    State::Ready | State::Paused | State::Preflight
                ) || status.cause != 0
                    || status.evidence & !1 != 0
                {
                    return Err(invalid("Invalid preflight transition"));
                }
                if previous_state != State::Preflight {
                    if status.generation
                        != self
                            .generation
                            .checked_add(1)
                            .ok_or_else(|| invalid("Bridge generation exhausted"))?
                    {
                        return Err(invalid("Unexpected bridge capture generation"));
                    }
                    self.generation = status.generation;
                    self.parser = None;
                    self.last_device_timestamp = None;
                    self.stop_boundary = None;
                }
                Some(Command::StartBeacon)
            }
            State::Capturing => {
                if !matches!(previous_state, State::Preflight | State::Capturing)
                    || status.cause != 0
                    || status.evidence & 3 != 3
                    || self.parser.is_none()
                {
                    return Err(invalid("Capture lacks explicit preflight/start evidence"));
                }
                Some(Command::StartBeacon)
            }
            State::Stopping => {
                if !matches!(
                    previous_state,
                    State::Ready
                        | State::Preflight
                        | State::Capturing
                        | State::Stopping
                        | State::Paused
                ) {
                    return Err(invalid("Invalid stopping transition"));
                }
                Some(match status.cause {
                    1 => Command::Pause,
                    2 => Command::Stop,
                    _ => return Err(invalid("Stopping lacks command cause")),
                })
            }
            State::Paused | State::Stopped => {
                // Stop before any UART generation only closes the app session;
                // evidence0 explicitly leaves the physical radio state unknown.
                let unopened_stop = status.state == State::Stopped
                    && previous_state == State::Ready
                    && self.generation == 0
                    && status.evidence == 0
                    && status.uart_rx_bytes == 0
                    && status.unread_ring_bytes == 0;
                if !unopened_stop
                    && (!(previous_state == State::Stopping || previous_state == status.state)
                        || status.evidence & 4 == 0
                        || self.stop_boundary != Some(request)
                        || status.unread_ring_bytes != 0)
                {
                    return Err(invalid("Stop lacks observed text or clean drain"));
                }
                let command = if status.state == State::Paused {
                    Command::Pause
                } else {
                    Command::Stop
                };
                if status.cause != if command == Command::Pause { 1 } else { 2 } {
                    return Err(invalid("Stop cause mismatched"));
                }
                if let Some(parser) = &self.parser {
                    parser.finish()?;
                }
                Some(command)
            }
            State::Failed => unreachable!(),
        };
        if status.generation != self.generation {
            return Err(invalid("Status generation mismatched"));
        }
        if let Some(command) = expected {
            if self.lifecycle != Some((request, command))
                || self.pending == Some((request, command))
            {
                return Err(invalid(
                    "Status lacks correlated accepted lifecycle command",
                ));
            }
        }
        if let Some(previous) = self.status {
            if previous.generation == status.generation {
                if status.evidence & previous.evidence & 3 != previous.evidence & 3 {
                    return Err(invalid("Bridge start/preflight evidence disappeared"));
                }
                if request == self.status_request
                    && status.evidence & previous.evidence & 4 != previous.evidence & 4
                {
                    return Err(invalid("Current stop evidence disappeared"));
                }
            }
        }
        if status.state == State::Stopping && self.stop_boundary != Some(request) {
            if status.evidence & 4 != 0 {
                return Err(invalid("New stop reused prior stop evidence"));
            }
            self.stop_boundary = Some(request);
        }
        self.status_request = request;
        if status.state == State::Preflight && status.evidence & 1 != 0 && self.parser.is_none() {
            self.parser = Some(crate::marauder_stream::Decoder::new());
        }
        if matches!(status.state, State::Paused | State::Stopped) {
            self.parser = None;
        }
        self.status = Some(status);
        self.state = Some(status.state);
        Ok(Event::Status(status))
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    struct Peer {
        session: Session,
        sequence: u32,
    }
    impl Peer {
        fn new() -> Self {
            let mut peer = Self {
                session: Session::new([7; 16]).unwrap(),
                sequence: 0,
            };
            assert_eq!(peer.command(Command::Hello), 1);
            peer.receive(
                1,
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
            )
            .unwrap();
            peer
        }
        fn command(&mut self, command: Command) -> u32 {
            bridge::decode(&self.session.command(command).unwrap())
                .unwrap()
                .header
                .request_id
        }
        fn receive(&mut self, request: u32, body: Body<'_>) -> io::Result<Event> {
            self.sequence += 1;
            let mut bytes = [0; bridge::MAX_MESSAGE];
            let n = bridge::encode(
                Message {
                    header: Header {
                        request_id: request,
                        nonce: [7; 16],
                        app_instance: 55,
                        sequence: self.sequence,
                    },
                    body,
                },
                &mut bytes,
            )
            .unwrap();
            self.session.receive(&bytes[..n])
        }
        fn ack(&mut self, id: u32, command: Command) {
            self.receive(id, Body::Ack { command, result: 0 }).unwrap();
        }
        fn status(
            &mut self,
            id: u32,
            state: State,
            cause: u8,
            evidence: u16,
            generation: u32,
        ) -> io::Result<Event> {
            self.receive(
                id,
                Body::Status(Status {
                    state,
                    cause,
                    evidence,
                    generation,
                    app_uptime_ms: 1,
                    uart_rx_bytes: 100,
                    uart_dropped_bytes: 0,
                    uart_error_events: 0,
                    rpc_dropped_messages: 0,
                    unread_ring_bytes: 0,
                }),
            )
        }
        fn start(&mut self) -> u32 {
            let id = self.command(Command::StartBeacon);
            self.ack(id, Command::StartBeacon);
            self.status(id, State::Preflight, 0, 0, 1).unwrap();
            id
        }
    }
    fn capture() -> Vec<u8> {
        let mut bytes = b"[BUF/BEGIN]".to_vec();
        bytes.extend([0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]);
        for n in [0u32, 0, 4096, 105] {
            bytes.extend(n.to_le_bytes());
        }
        for micros in [u32::MAX, 3] {
            for n in [micros / 1_000_000, micros % 1_000_000, 1, 1] {
                bytes.extend(n.to_le_bytes());
            }
            bytes.push(0x80);
        }
        bytes.extend(b"[BUF/CLOSE]");
        bytes
    }

    #[test]
    fn handshake_requires_capabilities_and_matching_ack_is_not_reception() {
        let mut peer = Peer::new();
        assert_eq!(peer.session.state(), Some(State::Ready));
        let id = peer.command(Command::StartBeacon);
        peer.ack(id, Command::StartBeacon);
        assert_eq!(peer.session.state(), Some(State::Ready));
        assert!(peer.session.command(Command::StartBeacon).is_err());
        assert!(peer.status(id, State::Capturing, 0, 3, 1).is_err());
        assert!(peer.session.command(Command::Heartbeat).is_err());
        for (caps, major, minor) in [(0, 87, 1), (3, 87, 1), (7, 88, 1), (7, 87, 2)] {
            let mut peer = Peer {
                session: Session::new([7; 16]).unwrap(),
                sequence: 0,
            };
            peer.command(Command::Hello);
            assert!(peer
                .receive(
                    1,
                    Body::Hello(bridge::Hello {
                        capabilities: caps,
                        api_major: major,
                        api_minor: minor,
                        uart_baud: 115200,
                        watchdog_ms: 3000,
                        app_version: "1",
                        source_revision: "",
                        source_modified: 2
                    })
                )
                .is_err());
        }
    }

    #[test]
    fn raw_preflight_boundary_packet_clock_pause_and_explicit_new_generation() {
        let mut peer = Peer::new();
        let id = peer.start();
        assert_eq!(
            peer.receive(
                0,
                Body::Raw {
                    generation: 1,
                    app_uptime_ms: 1,
                    bytes: &capture()
                }
            )
            .unwrap(),
            Event::Raw {
                packets: vec![],
                ignored_preflight_bytes: capture().len()
            }
        );
        peer.status(id, State::Preflight, 0, 1, 1).unwrap();
        let event = peer
            .receive(
                0,
                Body::Raw {
                    generation: 1,
                    app_uptime_ms: 2,
                    bytes: &capture(),
                },
            )
            .unwrap();
        let Event::Raw { packets, .. } = event else {
            panic!()
        };
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].device_micros32, u32::MAX);
        assert!(!packets[0].clock_discontinuity);
        assert_eq!(packets[1].device_micros32, 3);
        assert!(packets[1].clock_discontinuity);
        // Header/packet may arrive before start-text status; evidence stays separate.
        assert_eq!(peer.session.state(), Some(State::Preflight));
        peer.status(id, State::Capturing, 0, 3, 1).unwrap();
        let heartbeat = peer.command(Command::Heartbeat);
        peer.ack(heartbeat, Command::Heartbeat);
        peer.status(id, State::Capturing, 0, 3, 1).unwrap();
        let pause = peer.command(Command::Pause);
        peer.ack(pause, Command::Pause);
        peer.status(pause, State::Stopping, 1, 3, 1).unwrap();
        peer.status(pause, State::Paused, 1, 7, 1).unwrap();
        let next = peer.command(Command::StartBeacon);
        peer.ack(next, Command::StartBeacon);
        peer.status(next, State::Preflight, 0, 0, 2).unwrap();
        peer.status(next, State::Preflight, 0, 1, 2).unwrap();
        assert_eq!(peer.session.generation, 2);
        assert_eq!(peer.session.last_device_timestamp, None);
    }

    #[test]
    fn stop_before_uart_ownership_is_terminal_without_radio_stop_claim() {
        let mut peer = Peer::new();
        let id = peer.command(Command::Stop);
        peer.ack(id, Command::Stop);
        let status = Status {
            state: State::Stopped,
            cause: 2,
            evidence: 0,
            generation: 0,
            app_uptime_ms: 0,
            uart_rx_bytes: 0,
            uart_dropped_bytes: 0,
            uart_error_events: 0,
            rpc_dropped_messages: 0,
            unread_ring_bytes: 0,
        };
        assert_eq!(
            peer.receive(id, Body::Status(status)).unwrap(),
            Event::Status(status)
        );
        assert_eq!(peer.session.last_reported_status().unwrap().evidence, 0);
        assert!(peer.session.command(Command::StartBeacon).is_err());
    }

    #[test]
    fn accepted_pause_and_stop_are_not_resubmitted_before_first_status() {
        let mut peer = Peer::new();
        let start = peer.start();
        peer.status(start, State::Preflight, 0, 1, 1).unwrap();
        peer.status(start, State::Capturing, 0, 3, 1).unwrap();
        let pause = peer.command(Command::Pause);
        peer.ack(pause, Command::Pause);
        assert!(peer.session.command(Command::Pause).is_err());
        let stop = peer.command(Command::Stop);
        peer.ack(stop, Command::Stop);
        assert!(peer.session.command(Command::Stop).is_err());
        assert!(peer.session.command(Command::Pause).is_err());
    }

    #[test]
    fn each_stop_requires_a_fresh_zero_evidence_boundary_and_acknowledgment() {
        let mut peer = Peer::new();
        let start = peer.start();
        peer.status(start, State::Preflight, 0, 1, 1).unwrap();
        peer.receive(
            0,
            Body::Raw {
                generation: 1,
                app_uptime_ms: 2,
                bytes: &capture(),
            },
        )
        .unwrap();
        peer.status(start, State::Capturing, 0, 3, 1).unwrap();
        let pause = peer.command(Command::Pause);
        peer.ack(pause, Command::Pause);
        peer.status(pause, State::Stopping, 1, 3, 1).unwrap();
        peer.status(pause, State::Paused, 1, 7, 1).unwrap();
        let stop = peer.command(Command::Stop);
        peer.ack(stop, Command::Stop);
        peer.status(stop, State::Stopping, 2, 3, 1).unwrap();
        assert!(peer.session.command(Command::Stop).is_err());
        peer.status(stop, State::Stopped, 2, 7, 1).unwrap();
        let mut peer = Peer::new();
        peer.start();
        let stop = peer.command(Command::Stop);
        peer.ack(stop, Command::Stop);
        // A positive stale witness cannot replace the required new boundary.
        assert!(peer.status(stop, State::Stopping, 2, 4, 1).is_err());
    }

    #[test]
    fn queued_prior_status_is_valid_until_new_command_acknowledgment() {
        let mut peer = Peer::new();
        let start = peer.command(Command::StartBeacon);
        peer.status(0, State::Ready, 0, 0, 0).unwrap();
        peer.ack(start, Command::StartBeacon);
        peer.status(start, State::Preflight, 0, 0, 1).unwrap();
        peer.status(start, State::Preflight, 0, 1, 1).unwrap();
        peer.receive(
            0,
            Body::Raw {
                generation: 1,
                app_uptime_ms: 2,
                bytes: &capture(),
            },
        )
        .unwrap();
        peer.status(start, State::Capturing, 0, 3, 1).unwrap();
        let pause = peer.command(Command::Pause);
        peer.status(start, State::Capturing, 0, 3, 1).unwrap();
        peer.ack(pause, Command::Pause);
        peer.status(pause, State::Stopping, 1, 3, 1).unwrap();
        peer.status(pause, State::Paused, 1, 7, 1).unwrap();
        let next = peer.command(Command::StartBeacon);
        peer.status(pause, State::Paused, 1, 7, 1).unwrap();
        peer.ack(next, Command::StartBeacon);
        peer.status(next, State::Preflight, 0, 0, 2).unwrap();
    }

    #[test]
    fn stale_commands_loss_counter_regression_and_stop_without_evidence_fail_closed() {
        let mut peer = Peer::new();
        let id = peer.command(Command::Heartbeat);
        assert!(peer
            .receive(
                id + 1,
                Body::Ack {
                    command: Command::Heartbeat,
                    result: 0
                }
            )
            .is_err());
        let mut peer = Peer::new();
        let id = peer.start();
        let mut status = peer.session.status.unwrap();
        status.uart_dropped_bytes = 1;
        assert!(peer.receive(id, Body::Status(status)).is_err());
        assert_eq!(peer.session.last_reported_status(), Some(status));
        let mut peer = Peer::new();
        let id = peer.start();
        let mut status = peer.session.status.unwrap();
        status.uart_rx_bytes = 99;
        assert!(peer.receive(id, Body::Status(status)).is_err());
        let mut peer = Peer::new();
        peer.start();
        let stop = peer.command(Command::Stop);
        peer.ack(stop, Command::Stop);
        peer.status(stop, State::Stopping, 2, 0, 1).unwrap();
        assert!(peer.status(stop, State::Stopped, 2, 0, 1).is_err());
    }

    #[test]
    fn generation_request_sequence_and_packet_gaps_never_silently_restart() {
        let mut peer = Peer::new();
        let id = peer.start();
        assert!(peer.status(id, State::Preflight, 0, 1, 2).is_err());
        let mut peer = Peer::new();
        let id = peer.start();
        assert!(peer.status(id + 1, State::Preflight, 0, 1, 1).is_err());
        let mut peer = Peer::new();
        let id = peer.start();
        peer.status(id, State::Preflight, 0, 1, 1).unwrap();
        peer.sequence += 1;
        assert!(peer
            .receive(
                0,
                Body::Raw {
                    generation: 1,
                    app_uptime_ms: 2,
                    bytes: &capture()
                }
            )
            .is_err());
    }
}
