//! Bounded, single-owner Flipper RPC session engine. The caller owns the serial
//! lease and must configure transport read/write timeouts <=50ms. Callbacks must
//! not block: consumer backpressure must never delay heartbeat or stop control.
use crate::{
    flipper_bridge::{Event, Session},
    flipper_rpc_transport::{self as rpc, Received},
};
use airhound_flipper::bridge::{Command, State};
use flipper_rpc::proto::{self, app, main::Content};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read, Write},
    time::{Duration, Instant},
};

pub const APPLICATION: &str = "/ext/apps/GPIO/airhound_bridge.fap";
const ACK_TIMEOUT: Duration = Duration::from_millis(750);
const TRANSITION_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Copy)]
pub struct Desired {
    pub enabled: bool,
    pub shutdown: bool,
}
pub enum Output {
    DeviceInfo(BTreeMap<String, String>),
    Bridge(Event),
    FailureStatus {
        status: airhound_flipper::bridge::Status,
        stop_complete: bool,
    },
}

trait Clock {
    fn now(&self) -> Instant;
}
struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

struct Link<'a, T> {
    clock: &'a dyn Clock,
    io: &'a mut T,
    decoder: rpc::Decoder,
    correlation: rpc::Correlation,
    pending_messages: VecDeque<proto::Main>,
}
impl<T: Read + Write> Link<'_, T> {
    fn send(&mut self, content: Content) -> io::Result<u32> {
        let request = self.correlation.request(content)?;
        write_bounded(
            self.io,
            self.clock,
            &rpc::encode(&request)?,
            self.clock.now() + ACK_TIMEOUT,
        )?;
        Ok(request.command_id)
    }
    fn pump(&mut self, visit: impl FnMut(Received) -> io::Result<()>) -> io::Result<()> {
        if !self.pending_messages.is_empty() {
            return self.visit_pending(visit);
        }
        let mut bytes = [0; rpc::MAX_READ];
        let n = match self.io.read(&mut bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Flipper USB disconnected",
                ))
            }
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(())
            }
            Err(e) => return Err(e),
        };
        self.pending_messages
            .extend(self.decoder.feed(&bytes[..n])?);
        // One <=512-byte read plus the decoder's bounded partial frame is the
        // only producer of this queue. Never read again until it is drained.
        self.visit_pending(visit)
    }
    fn visit_pending(
        &mut self,
        mut visit: impl FnMut(Received) -> io::Result<()>,
    ) -> io::Result<()> {
        while let Some(message) = self.pending_messages.pop_front() {
            visit(self.correlation.receive(message)?)?;
        }
        Ok(())
    }
    fn exchange(
        &mut self,
        content: Content,
        mut visit: impl FnMut(Received) -> io::Result<()>,
    ) -> io::Result<()> {
        let id = self.send(content)?;
        let deadline = self.clock.now() + ACK_TIMEOUT;
        let mut ack = false;
        while !ack {
            if self.clock.now() >= deadline {
                return Err(timeout("Flipper RPC acknowledgment timed out"));
            }
            self.pump(|received| match received {
                Received::TransportAck { command } if command == id => {
                    ack = true;
                    Ok(())
                }
                other => visit(other),
            })?;
        }
        Ok(())
    }
}

fn bridge_event(
    received: Received,
    session: &mut Session,
    output: &mut impl FnMut(Output) -> io::Result<()>,
) -> io::Result<()> {
    match received {
        Received::ApplicationData(bytes) => match session.receive(&bytes) {
            Ok(event) => output(Output::Bridge(event)),
            Err(error) => {
                if let Some(status) = session.last_reported_status() {
                    output(Output::FailureStatus {
                        status,
                        stop_complete: session.diagnostic_stop_complete(),
                    })?;
                }
                Err(error)
            }
        },
        Received::ApplicationState(1) => Ok(()),
        Received::ApplicationState(0) => Err(invalid("Owned Flipper application closed")),
        _ => Err(invalid("Unexpected Flipper application event")),
    }
}
fn command<T: Read + Write>(
    link: &mut Link<'_, T>,
    session: &mut Session,
    c: Command,
    output: &mut impl FnMut(Output) -> io::Result<()>,
) -> io::Result<()> {
    let bytes = session.command(c)?;
    link.exchange(
        Content::AppDataExchangeRequest(app::DataExchangeRequest { data: bytes }),
        |event| bridge_event(event, session, output),
    )?;
    let deadline = link.clock.now() + ACK_TIMEOUT;
    while session.awaiting_ack() {
        if link.clock.now() >= deadline {
            return Err(timeout("Flipper bridge acknowledgment timed out"));
        }
        link.pump(|event| bridge_event(event, session, output))?;
    }
    Ok(())
}

/// Run one explicitly requested recording attachment. No automatic reconnect or
/// RF restart follows a transport/protocol error. Closing RPC lets the FAP's
/// independent local stop/watchdog handle failure; the host still returns error.
pub fn run<T: Read + Write>(
    io: &mut T,
    nonce: [u8; 16],
    desired: impl FnMut() -> Desired,
    output: impl FnMut(Output) -> io::Result<()>,
) -> io::Result<()> {
    run_with_clock(io, &SystemClock, nonce, desired, output)
}

fn run_with_clock<T: Read + Write>(
    io: &mut T,
    clock: &dyn Clock,
    nonce: [u8; 16],
    mut desired: impl FnMut() -> Desired,
    mut output: impl FnMut(Output) -> io::Result<()>,
) -> io::Result<()> {
    let initial = desired();
    if initial.shutdown {
        return Ok(());
    }
    read_until(
        io,
        clock,
        b">: ",
        4096,
        clock.now() + Duration::from_secs(3),
    )?;
    if desired().shutdown {
        return Ok(());
    }
    write_bounded(io, clock, b"device_info\r", clock.now() + ACK_TIMEOUT)?;
    let info = read_until(
        io,
        clock,
        b">: ",
        8192,
        clock.now() + Duration::from_secs(3),
    )?;
    let metadata = parse_device_info(&info)?;
    output(Output::DeviceInfo(metadata))?;
    if desired().shutdown {
        return Ok(());
    }
    write_bounded(io, clock, b"loader info\r", clock.now() + ACK_TIMEOUT)?;
    let loader = read_until(
        io,
        clock,
        b">: ",
        1024,
        clock.now() + Duration::from_secs(2),
    )?;
    if !loader
        .windows(b"No application is running".len())
        .any(|w| w == b"No application is running")
    {
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            "Flipper already has an active application",
        ));
    }
    if desired().shutdown {
        return Ok(());
    }
    write_bounded(io, clock, b"start_rpc_session\r", clock.now() + ACK_TIMEOUT)?;
    let echo = read_until(io, clock, b"\n", 128, clock.now() + Duration::from_secs(2))?;
    if echo.strip_suffix(b"\r").unwrap_or(&echo) != b"start_rpc_session" {
        return Err(invalid("Unexpected Flipper CLI/RPC transition"));
    }
    let mut link = Link {
        io,
        clock,
        decoder: rpc::Decoder::default(),
        correlation: rpc::Correlation::default(),
        pending_messages: VecDeque::new(),
    };
    let mut session = Session::new(nonce)?;
    // Fixed app and RPC argument: this source is not a generic app launcher.
    let result = (|| {
        if desired().shutdown {
            return Ok(());
        }
        let mut app_started = false;
        link.exchange(
            Content::AppStartRequest(app::StartRequest {
                name: APPLICATION.into(),
                args: "RPC".into(),
            }),
            |event| match event {
                Received::ApplicationState(1) => {
                    app_started = true;
                    Ok(())
                }
                _ => Err(invalid("Unexpected app startup event")),
            },
        )?;
        // AppStart's transport ACK only acknowledges loader acceptance. The FAP
        // installs its callback before emitting AppStarted; Hello must wait.
        let started_deadline = clock.now() + TRANSITION_TIMEOUT;
        while !app_started {
            if desired().shutdown {
                return Ok(());
            }
            if clock.now() >= started_deadline {
                return Err(timeout("Flipper application startup timed out"));
            }
            link.pump(|event| match event {
                Received::ApplicationState(1) => {
                    app_started = true;
                    Ok(())
                }
                _ => Err(invalid("Unexpected app startup event")),
            })?;
        }
        command(&mut link, &mut session, Command::Hello, &mut output)?;
        let mut heartbeat = clock.now();
        let mut transition: Option<(Command, Instant)> = None;
        loop {
            let want = desired();
            let state = session
                .state()
                .ok_or_else(|| invalid("Bridge lost Hello state"))?;
            if state == State::Stopped {
                return Ok(());
            }
            if state == State::Failed {
                return Err(invalid("Flipper bridge failed"));
            }
            if transition.is_some_and(|(_, deadline)| clock.now() >= deadline) {
                return Err(timeout("Flipper lifecycle transition timed out"));
            }
            if matches!(
                (transition, state),
                (Some((Command::StartBeacon, _)), State::Capturing)
                    | (Some((Command::Pause, _)), State::Paused)
            ) {
                transition = None;
            }
            let next = if want.shutdown {
                if matches!(transition, Some((Command::Stop, _))) {
                    None
                } else {
                    Some(Command::Stop)
                }
            } else if !want.enabled && matches!(state, State::Preflight | State::Capturing) {
                if matches!(transition, Some((Command::Pause | Command::Stop, _))) {
                    None
                } else {
                    Some(Command::Pause)
                }
            } else if want.enabled
                && matches!(state, State::Ready | State::Paused)
                && transition.is_none()
            {
                Some(Command::StartBeacon)
            } else {
                None
            };
            if let Some(next) = next {
                command(&mut link, &mut session, next, &mut output)?;
                heartbeat = clock.now();
                transition = Some((next, clock.now() + TRANSITION_TIMEOUT));
                continue;
            }
            if clock.now().duration_since(heartbeat) >= Duration::from_secs(1)
                && !matches!(transition, Some((Command::Stop, _)))
            {
                command(&mut link, &mut session, Command::Heartbeat, &mut output)?;
                heartbeat = clock.now();
            }
            link.pump(|event| bridge_event(event, &mut session, &mut output))?;
        }
    })();
    if result.is_err() && session.diagnostic_pending() {
        // Never renew the device lease or send a recovery control. This fixed
        // deadline is set once at the first observed failure, not per status.
        // 250ms accommodates wire/scheduling latency beyond the device 3s cap;
        // it changes neither that producer deadline nor its heartbeat lease.
        if let Err(error) = drain_failure(
            &mut link,
            &mut session,
            &mut output,
            clock.now() + Duration::from_millis(3250),
        ) {
            log::warn!(
                "Flipper failure_drain failed ({:?}): {error}; stop completion is unverified",
                error.kind()
            );
            invalidate_failure_proof(&mut session, &mut output);
        }
    }
    // SessionClose, not AppExit of an arbitrary current application. An error
    // remains an error even if this transport shutdown is acknowledged.
    let closed =
        link.exchange(
            Content::StopSession(proto::StopSession {}),
            |received| match received {
                Received::ApplicationState(_) => Ok(()),
                Received::ApplicationData(bytes) if session.diagnostic_available() => {
                    if let Some(status) = session.receive_diagnostic(&bytes)? {
                        output(Output::FailureStatus {
                            status,
                            stop_complete: session.diagnostic_stop_complete(),
                        })?;
                    }
                    Ok(())
                }
                received @ Received::ApplicationData(_) => {
                    bridge_event(received, &mut session, &mut output)
                }
                _ => Err(invalid("Unexpected closing RPC response")),
            },
        );
    if let Err(error) = &closed {
        log::warn!(
            "Flipper session_close failed ({:?}): {error}; stop completion is unverified",
            error.kind()
        );
        invalidate_failure_proof(&mut session, &mut output);
    }
    result.and(closed)
}

fn invalidate_failure_proof(
    session: &mut Session,
    output: &mut impl FnMut(Output) -> io::Result<()>,
) {
    session.invalidate_diagnostics();
    if let Some(status) = session.last_reported_status() {
        let _ = output(Output::FailureStatus {
            status,
            stop_complete: false,
        });
    }
}

fn drain_failure<T: Read + Write>(
    link: &mut Link<'_, T>,
    session: &mut Session,
    output: &mut impl FnMut(Output) -> io::Result<()>,
    deadline: Instant,
) -> io::Result<()> {
    // An autonomous terminal can precede the RPC and AHFB acknowledgments
    // of a heartbeat submitted before the failure was observed. Consume only
    // those already-correlated replies before issuing StopSession, within the
    // same original deadline; send no new bridge command or lease renewal.
    while session.diagnostic_pending() || session.awaiting_ack() || link.correlation.awaiting_ack()
    {
        if link.clock.now() >= deadline {
            return Err(timeout("Flipper failure diagnostic drain timed out"));
        }
        link.pump(|received| match received {
            Received::ApplicationData(bytes) => {
                if let Some(status) = session.receive_diagnostic(&bytes)? {
                    output(Output::FailureStatus {
                        status,
                        stop_complete: session.diagnostic_stop_complete(),
                    })?;
                }
                Ok(())
            }
            // Only a previously pending request can yield this correlated ACK.
            Received::TransportAck { .. } => Ok(()),
            _ => Err(invalid("Unexpected failure diagnostic RPC event")),
        })?;
    }
    Ok(())
}

fn parse_device_info(bytes: &[u8]) -> io::Result<BTreeMap<String, String>> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("Invalid Flipper device-info encoding"))?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if matches!(
            key,
            "firmware_version"
                | "firmware_commit"
                | "firmware_commit_dirty"
                | "firmware_build_date"
                | "firmware_api_major"
                | "firmware_api_minor"
                | "firmware_origin_fork"
        ) && (value.len() > 128
            || value.chars().any(char::is_control)
            || fields.insert(key.into(), value.into()).is_some())
        {
            return Err(invalid("Invalid or duplicate firmware identity field"));
        }
    }
    if fields.get("firmware_api_major").map(String::as_str) != Some("87")
        || fields.get("firmware_api_minor").map(String::as_str) != Some("1")
    {
        return Err(invalid("Flipper firmware ABI unsupported; expected 87.1"));
    }
    Ok(fields)
}
fn read_until<T: Read>(
    io: &mut T,
    clock: &dyn Clock,
    marker: &[u8],
    limit: usize,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while clock.now() < deadline {
        let mut byte = [0];
        match io.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Flipper serial endpoint closed",
                ))
            }
            Ok(_) => {
                bytes.push(byte[0]);
                if bytes.ends_with(marker) {
                    bytes.truncate(bytes.len() - marker.len());
                    return Ok(bytes);
                }
                if bytes.len() >= limit {
                    return Err(invalid("Flipper CLI response exceeded limit"));
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Err(timeout("Flipper CLI response timed out"))
}
fn write_bounded<T: Write>(
    io: &mut T,
    clock: &dyn Clock,
    mut bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if clock.now() >= deadline {
            return Err(timeout("Flipper serial write timed out"));
        }
        match io.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Flipper serial write closed",
                ))
            }
            Ok(n) => bytes = &bytes[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn timeout(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, message)
}

#[cfg(test)]
mod test_transport;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::test_transport::{Host, PeerTransport};
    use super::*;
    use airhound_flipper::bridge::{self, Body, Header, Message, Status};
    use std::{
        os::unix::net::UnixStream,
        sync::{
            atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
            Arc,
        },
    };

    #[derive(Clone, Copy, Debug)]
    enum Fault {
        None,
        Loss,
        MissingHello,
        MalformedHello,
        StateFirst,
        MissingAppStarted,
        WatchdogTerminal,
        WatchdogMissing,
        WatchdogBadNonce,
        WatchdogReplay,
        WatchdogEof,
        WatchdogLoss,
        WatchdogRepeated,
        WatchdogWrongRequest,
        WatchdogWrongGeneration,
        WatchdogWrongCause,
        WatchdogCounterDecrease,
        WatchdogUnread,
        WatchdogRevoked,
        WatchdogInitialWrongGeneration,
        WatchdogInitialWrongRequest,
        WatchdogMissingBoundary,
        WatchdogBadTail,
        WatchdogRawTail,
        WatchdogPendingRpc,
        WatchdogPendingBridge,
        WatchdogMissingRpcAck,
        WatchdogMissingBridgeAck,
        WatchdogWrongRpcAck,
        WatchdogWrongBridgeAck,
    }

    impl Fault {
        fn delayed_heartbeat(self) -> bool {
            matches!(
                self,
                Self::WatchdogPendingRpc
                    | Self::WatchdogPendingBridge
                    | Self::WatchdogMissingRpcAck
                    | Self::WatchdogMissingBridgeAck
                    | Self::WatchdogWrongRpcAck
                    | Self::WatchdogWrongBridgeAck
            )
        }
    }

    #[derive(Default)]
    struct PeerControl {
        finished: Arc<AtomicBool>,
        repeated_pairs: AtomicUsize,
    }

    fn peer<T: PeerTransport>(io: T, fault: Fault) -> std::thread::JoinHandle<Vec<Command>> {
        peer_with_control(io, fault, Arc::new(PeerControl::default()))
    }

    fn peer_with_control<T: PeerTransport>(
        mut io: T,
        fault: Fault,
        control: Arc<PeerControl>,
    ) -> std::thread::JoinHandle<Vec<Command>> {
        std::thread::spawn(move || {
            io.write_all(b">: ").unwrap();
            for (command,response) in [
                (b"device_info".as_slice(),b"device_info\r\nfirmware_version: fixture\r\nfirmware_api_major: 87\r\nfirmware_api_minor: 1\r\n>: ".as_slice()),
                (b"loader info",b"loader info\r\nNo application is running\r\n>: "),
                (b"start_rpc_session",b"start_rpc_session\r\n"),
            ] {
                let mut actual = Vec::new();
                loop {
                    let mut byte = [0];
                    io.read_exact(&mut byte).unwrap();
                    if byte[0] == b'\r' { break; }
                    actual.push(byte[0]);
                    assert!(actual.len() < 8192);
                }
                assert_eq!(actual,command);
                io.write_all(response).unwrap();
            }
            let mut decoder = rpc::Decoder::default();
            let mut sequence = 0u32;
            let mut generation = 0u32;
            let mut commands = vec![];
            let mut ready = true;
            while !control.finished.load(Ordering::Acquire) {
                let mut bytes = [0; rpc::MAX_READ];
                let n = match io.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e)
                        if e.kind() == io::ErrorKind::TimedOut
                            || e.kind() == io::ErrorKind::WouldBlock =>
                    {
                        continue
                    }
                    Err(_) => break,
                };
                for request in decoder.feed(&bytes[..n]).unwrap() {
                    if matches!(fault, Fault::StateFirst)
                        && matches!(&request.content, Some(Content::AppStartRequest(_)))
                    {
                        let started = proto::Main {
                            content: Some(Content::AppStateResponse(app::AppStateResponse {
                                state: 1,
                            })),
                            ..Default::default()
                        };
                        io.write_all(&rpc::encode(&started).unwrap()).unwrap();
                    }
                    let response = proto::Main {
                        command_id: request.command_id,
                        content: Some(Content::Empty(proto::Empty {})),
                        ..Default::default()
                    };
                    let delayed_heartbeat = fault.delayed_heartbeat()
                        && matches!(&request.content, Some(Content::AppDataExchangeRequest(data)) if matches!(bridge::decode(&data.data).unwrap().body, Body::Command(Command::Heartbeat)));
                    if !delayed_heartbeat {
                        if let Err(error) = io.write_all(&rpc::encode(&response).unwrap()) {
                            // A peer rejecting queued invalid diagnostics may close
                            // immediately after its best-effort StopSession. Only
                            // that teardown ACK may race the intentional close.
                            if matches!(request.content, Some(Content::StopSession(_)))
                                && matches!(
                                    fault,
                                    Fault::WatchdogBadNonce
                                        | Fault::WatchdogReplay
                                        | Fault::WatchdogWrongRequest
                                        | Fault::WatchdogWrongGeneration
                                        | Fault::WatchdogWrongCause
                                        | Fault::WatchdogCounterDecrease
                                        | Fault::WatchdogInitialWrongGeneration
                                        | Fault::WatchdogInitialWrongRequest
                                        | Fault::WatchdogMissingBoundary
                                        | Fault::WatchdogBadTail
                                        | Fault::WatchdogRawTail
                                )
                                && matches!(
                                    error.kind(),
                                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                                )
                            {
                                return commands;
                            }
                            panic!(
                                "fixture ACK failed for {fault:?} request {}: {error}",
                                request.command_id
                            );
                        }
                    }
                    match request.content.unwrap() {
                        Content::AppStartRequest(app) => {
                            assert_eq!(app.name, APPLICATION);
                            assert_eq!(app.args, "RPC");
                            if matches!(fault, Fault::StateFirst | Fault::MissingAppStarted) {
                                continue;
                            }
                            // ACK has already been sent. Before AppStarted the
                            // host must not send Hello or any other RPC command.
                            io.probe_empty().unwrap();
                            let started = proto::Main {
                                content: Some(Content::AppStateResponse(app::AppStateResponse {
                                    state: 1,
                                })),
                                ..Default::default()
                            };
                            io.write_all(&rpc::encode(&started).unwrap()).unwrap();
                        }
                        Content::StopSession(_) => return commands,
                        Content::AppDataExchangeRequest(data) => {
                            let message = bridge::decode(&data.data).unwrap();
                            let Body::Command(command) = message.body else {
                                panic!()
                            };
                            commands.push(command);
                            let mut send = |io: &mut T, request_id: u32, body: Body<'_>| {
                                sequence += 1;
                                let mut frame = [0; bridge::MAX_MESSAGE];
                                let terminal = matches!(body, Body::Status(status) if status.state == State::Failed && status.cause == 3);
                                let mut nonce = message.header.nonce;
                                if terminal
                                    && (matches!(fault, Fault::WatchdogBadNonce)
                                        || (matches!(fault, Fault::WatchdogBadTail)
                                            && matches!(body, Body::Status(s) if s.evidence==7)))
                                {
                                    nonce[0] ^= 1;
                                }
                                let wire_sequence = sequence
                                    - u32::from(terminal && matches!(fault, Fault::WatchdogReplay));
                                let n = bridge::encode(
                                    Message {
                                        header: Header {
                                            request_id,
                                            nonce,
                                            app_instance: 42,
                                            sequence: wire_sequence,
                                        },
                                        body,
                                    },
                                    &mut frame,
                                )
                                .unwrap();
                                let event = proto::Main {
                                    content: Some(Content::AppDataExchangeRequest(
                                        app::DataExchangeRequest {
                                            data: frame[..n].to_vec(),
                                        },
                                    )),
                                    ..Default::default()
                                };
                                io.write_all(&rpc::encode(&event).unwrap()).unwrap_or_else(|error| {
                                    panic!("fixture event failed for {fault:?}, command {command:?}, request {request_id}, sequence {sequence}, body {body:?}: {error}")
                                });
                            };
                            if command == Command::Hello {
                                if matches!(fault, Fault::MissingHello) {
                                    continue;
                                }
                                if matches!(fault, Fault::MalformedHello) {
                                    let malformed = proto::Main {
                                        content: Some(Content::AppDataExchangeRequest(
                                            app::DataExchangeRequest { data: vec![0; 44] },
                                        )),
                                        ..Default::default()
                                    };
                                    io.write_all(&rpc::encode(&malformed).unwrap()).unwrap();
                                    continue;
                                }
                                send(
                                    &mut io,
                                    message.header.request_id,
                                    Body::Hello(bridge::Hello {
                                        capabilities: bridge::CAPABILITIES,
                                        api_major: 87,
                                        api_minor: 1,
                                        uart_baud: 115200,
                                        watchdog_ms: 3000,
                                        app_version: "fixture",
                                        source_revision: "",
                                        source_modified: 2,
                                    }),
                                );
                                continue;
                            }
                            if !delayed_heartbeat {
                                send(
                                    &mut io,
                                    message.header.request_id,
                                    Body::Ack { command, result: 0 },
                                );
                            }
                            let status = |state, cause, evidence, generation| Status {
                                state,
                                cause,
                                evidence,
                                generation,
                                app_uptime_ms: 1,
                                uart_rx_bytes: 0,
                                uart_dropped_bytes: 0,
                                uart_error_events: 0,
                                rpc_dropped_messages: 0,
                                unread_ring_bytes: 0,
                            };
                            match command {
                                Command::StartBeacon => {
                                    ready = false;
                                    generation += 1;
                                    send(
                                        &mut io,
                                        message.header.request_id,
                                        Body::Status(status(State::Preflight, 0, 0, generation)),
                                    );
                                    send(
                                        &mut io,
                                        message.header.request_id,
                                        Body::Status(status(State::Preflight, 0, 1, generation)),
                                    );
                                    let mut capture = b"[BUF/BEGIN]".to_vec();
                                    capture.extend([0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]);
                                    for n in [0u32, 0, 4096, 105, 0, 123, 1, 1] {
                                        capture.extend(n.to_le_bytes());
                                    }
                                    capture.push(0x80);
                                    capture.extend(b"[BUF/CLOSE]");
                                    send(
                                        &mut io,
                                        0,
                                        Body::Raw {
                                            generation,
                                            app_uptime_ms: 2,
                                            bytes: &capture,
                                        },
                                    );
                                    let mut current = status(State::Capturing, 0, 3, generation);
                                    if matches!(fault, Fault::Loss) {
                                        current.uart_dropped_bytes = 17;
                                        current.cause = 5;
                                        current.state = State::Failed;
                                    }
                                    send(&mut io, message.header.request_id, Body::Status(current));
                                    if matches!(
                                        fault,
                                        Fault::WatchdogTerminal
                                            | Fault::WatchdogMissing
                                            | Fault::WatchdogBadNonce
                                            | Fault::WatchdogReplay
                                            | Fault::WatchdogEof
                                            | Fault::WatchdogLoss
                                            | Fault::WatchdogRepeated
                                            | Fault::WatchdogWrongRequest
                                            | Fault::WatchdogWrongGeneration
                                            | Fault::WatchdogWrongCause
                                            | Fault::WatchdogCounterDecrease
                                            | Fault::WatchdogUnread
                                            | Fault::WatchdogRevoked
                                            | Fault::WatchdogInitialWrongGeneration
                                            | Fault::WatchdogInitialWrongRequest
                                            | Fault::WatchdogMissingBoundary
                                            | Fault::WatchdogBadTail
                                            | Fault::WatchdogRawTail
                                    ) {
                                        let mut failed = status(State::Stopping, 3, 3, generation);
                                        failed.app_uptime_ms = 3001;
                                        failed.uart_rx_bytes = 10;
                                        if matches!(fault, Fault::WatchdogInitialWrongGeneration) {
                                            failed.generation += 1;
                                        }
                                        if matches!(fault, Fault::WatchdogMissingBoundary) {
                                            failed.state = State::Failed;
                                            failed.evidence = 15;
                                        }
                                        send(
                                            &mut io,
                                            if matches!(fault, Fault::WatchdogInitialWrongRequest) {
                                                99
                                            } else {
                                                0
                                            },
                                            Body::Status(failed),
                                        );
                                        if matches!(fault, Fault::WatchdogEof) {
                                            return commands;
                                        }
                                        if matches!(fault, Fault::WatchdogRepeated) {
                                            for step in 1..=60 {
                                                // Keep offering input beyond the host's fixed
                                                // drain deadline, even without scheduler delay.
                                                if io.delay(Duration::from_millis(75)).is_err() {
                                                    return commands;
                                                }
                                                if control.finished.load(Ordering::Acquire) {
                                                    return commands;
                                                }
                                                failed.app_uptime_ms = 3001 + step * 25;
                                                send(
                                                    &mut io,
                                                    0,
                                                    Body::Raw {
                                                        generation,
                                                        app_uptime_ms: failed.app_uptime_ms,
                                                        bytes: b"not an observation",
                                                    },
                                                );
                                                if control.finished.load(Ordering::Acquire) {
                                                    return commands;
                                                }
                                                send(&mut io, 0, Body::Status(failed));
                                                control
                                                    .repeated_pairs
                                                    .fetch_add(1, Ordering::Release);
                                            }
                                        }
                                        if !matches!(
                                            fault,
                                            Fault::WatchdogMissing
                                                | Fault::WatchdogRepeated
                                                | Fault::WatchdogInitialWrongGeneration
                                                | Fault::WatchdogInitialWrongRequest
                                                | Fault::WatchdogMissingBoundary
                                        ) {
                                            failed.state = State::Failed;
                                            failed.evidence = 15;
                                            failed.app_uptime_ms = 3101;
                                            if matches!(fault, Fault::WatchdogLoss) {
                                                failed.uart_dropped_bytes = 1;
                                                failed.evidence = 7;
                                            }
                                            if matches!(fault, Fault::WatchdogWrongGeneration) {
                                                failed.generation += 1;
                                            }
                                            if matches!(fault, Fault::WatchdogWrongCause) {
                                                failed.cause = 4;
                                            }
                                            if matches!(fault, Fault::WatchdogCounterDecrease) {
                                                failed.uart_rx_bytes = 9;
                                            }
                                            if matches!(fault, Fault::WatchdogUnread) {
                                                failed.unread_ring_bytes = 1;
                                                failed.evidence = 7;
                                            }
                                            send(
                                                &mut io,
                                                if matches!(fault, Fault::WatchdogWrongRequest) {
                                                    99
                                                } else {
                                                    0
                                                },
                                                Body::Status(failed),
                                            );
                                            if matches!(
                                                fault,
                                                Fault::WatchdogRevoked | Fault::WatchdogBadTail
                                            ) {
                                                failed.app_uptime_ms += 1;
                                                failed.evidence &= !bridge::STOP_COMPLETE;
                                                failed.uart_dropped_bytes = 1;
                                                send(&mut io, 0, Body::Status(failed));
                                            }
                                            if matches!(fault, Fault::WatchdogRawTail) {
                                                send(
                                                    &mut io,
                                                    0,
                                                    Body::Raw {
                                                        generation,
                                                        app_uptime_ms: 3102,
                                                        bytes: b"late input",
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                                Command::Pause | Command::Stop => {
                                    let cause = if command == Command::Pause { 1 } else { 2 };
                                    let target = if command == Command::Pause {
                                        State::Paused
                                    } else {
                                        State::Stopped
                                    };
                                    if ready {
                                        send(
                                            &mut io,
                                            message.header.request_id,
                                            Body::Status(status(target, cause, 0, 0)),
                                        );
                                    } else {
                                        send(
                                            &mut io,
                                            message.header.request_id,
                                            Body::Status(status(
                                                State::Stopping,
                                                cause,
                                                3,
                                                generation,
                                            )),
                                        );
                                        send(
                                            &mut io,
                                            0,
                                            Body::Raw {
                                                generation,
                                                app_uptime_ms: 3,
                                                bytes: b"Stopping WiFi tran/recv\r\n",
                                            },
                                        );
                                        send(
                                            &mut io,
                                            message.header.request_id,
                                            Body::Status(status(target, cause, 15, generation)),
                                        );
                                    }
                                }
                                Command::Heartbeat if delayed_heartbeat => {
                                    let mut stopping = status(State::Stopping, 3, 3, generation);
                                    stopping.app_uptime_ms = 3001;
                                    send(&mut io, 0, Body::Status(stopping));
                                    let terminal = Status {
                                        state: State::Failed,
                                        evidence: 15,
                                        app_uptime_ms: 3101,
                                        ..stopping
                                    };
                                    send(&mut io, 0, Body::Status(terminal));
                                    // The real producer can repeat the final state before
                                    // answering an already-submitted heartbeat.
                                    send(&mut io, 0, Body::Status(terminal));
                                    let mut response = response;
                                    if matches!(fault, Fault::WatchdogWrongRpcAck) {
                                        response.command_id += 1;
                                    }
                                    let ack = rpc::encode(&response).unwrap();
                                    sequence += 1;
                                    let mut frame = [0; bridge::MAX_MESSAGE];
                                    let n = bridge::encode(
                                        Message {
                                            header: Header {
                                                request_id: message.header.request_id
                                                    + u32::from(matches!(
                                                        fault,
                                                        Fault::WatchdogWrongBridgeAck
                                                    )),
                                                nonce: message.header.nonce,
                                                app_instance: 42,
                                                sequence,
                                            },
                                            body: Body::Ack { command, result: 1 },
                                        },
                                        &mut frame,
                                    )
                                    .unwrap();
                                    let bridge_ack = rpc::encode(&proto::Main {
                                        content: Some(Content::AppDataExchangeRequest(
                                            app::DataExchangeRequest {
                                                data: frame[..n].to_vec(),
                                            },
                                        )),
                                        ..Default::default()
                                    })
                                    .unwrap();
                                    if matches!(
                                        fault,
                                        Fault::WatchdogMissingRpcAck | Fault::WatchdogWrongRpcAck
                                    ) {
                                        if io.delay(Duration::from_millis(100)).is_err() {
                                            return commands;
                                        }
                                        if io.write_all(&bridge_ack).is_err() {
                                            return commands;
                                        }
                                        if matches!(fault, Fault::WatchdogWrongRpcAck) {
                                            let _ = io.write_all(&ack);
                                        }
                                        continue;
                                    }
                                    if matches!(
                                        fault,
                                        Fault::WatchdogMissingBridgeAck
                                            | Fault::WatchdogWrongBridgeAck
                                    ) {
                                        if io.delay(Duration::from_millis(100)).is_err() {
                                            return commands;
                                        }
                                        if io.write_all(&ack).is_err() {
                                            return commands;
                                        }
                                        if matches!(fault, Fault::WatchdogWrongBridgeAck) {
                                            let _ = io.write_all(&bridge_ack);
                                        }
                                        continue;
                                    }
                                    let (first, second) =
                                        if matches!(fault, Fault::WatchdogPendingRpc) {
                                            (&bridge_ack, &ack)
                                        } else {
                                            (&ack, &bridge_ack)
                                        };
                                    // Force separate reads so terminal cannot accidentally
                                    // consume both pending acknowledgments in one batch.
                                    if io.delay(Duration::from_millis(100)).is_err() {
                                        return commands;
                                    }
                                    if io.write_all(first).is_err() {
                                        return commands;
                                    }
                                    if io.delay(Duration::from_millis(100)).is_err() {
                                        return commands;
                                    }
                                    if io.write_all(second).is_err() {
                                        return commands;
                                    }
                                }
                                Command::Heartbeat => (),
                                Command::Hello => unreachable!(),
                            }
                        }
                        other => panic!("unapproved RPC request: {other:?}"),
                    }
                }
            }
            commands
        })
    }
    fn transport() -> (Host, test_transport::Peer) {
        Host::pair()
    }
    #[test]
    fn autonomous_failure_drain_retains_terminal_evidence_but_never_recovers() {
        for fault in [
            Fault::WatchdogTerminal,
            Fault::WatchdogMissing,
            Fault::WatchdogBadNonce,
            Fault::WatchdogReplay,
            Fault::WatchdogEof,
            Fault::WatchdogLoss,
            Fault::WatchdogRepeated,
            Fault::WatchdogWrongRequest,
            Fault::WatchdogWrongGeneration,
            Fault::WatchdogWrongCause,
            Fault::WatchdogCounterDecrease,
            Fault::WatchdogUnread,
            Fault::WatchdogRevoked,
            Fault::WatchdogInitialWrongGeneration,
            Fault::WatchdogInitialWrongRequest,
            Fault::WatchdogMissingBoundary,
            Fault::WatchdogBadTail,
            Fault::WatchdogRawTail,
        ] {
            let (io, other) = transport();
            let clock = io.clock.clone();
            let control = Arc::new(PeerControl::default());
            let thread = peer_with_control(other, fault, control.clone());
            let mut peer = test_transport::ScriptedPeer {
                io,
                thread: Some(thread),
                finished: control.finished.clone(),
            };
            let started = clock.now();
            let mut diagnostic = vec![];
            let mut completion = vec![];
            let mut accepted_terminal = false;
            let mut packet_count = 0;
            let error = run_with_clock(
                &mut peer.io,
                &clock,
                [7; 16],
                || Desired {
                    enabled: true,
                    shutdown: false,
                },
                |event| {
                    match event {
                        Output::FailureStatus {
                            status,
                            stop_complete,
                        } => {
                            completion.push(stop_complete);
                            diagnostic.push(status);
                        }
                        Output::Bridge(Event::Raw { packets, .. }) => packet_count += packets.len(),
                        Output::Bridge(Event::Status(s))
                            if matches!(s.state, State::Failed | State::Stopped) =>
                        {
                            accepted_terminal = true
                        }
                        _ => (),
                    }
                    Ok(())
                },
            )
            .unwrap_err();
            // Protocol completion precedes the fixture-only handoff. No further
            // bytes enter run_with_clock and the clock is never advanced here.
            let commands = peer.finish().unwrap();
            assert_eq!(commands, [Command::Hello, Command::StartBeacon]);
            assert_eq!(
                error.to_string(),
                "Bridge reported autonomous stop or failure"
            );
            assert!(!accepted_terminal);
            if matches!(fault, Fault::WatchdogTerminal) {
                assert_eq!(completion, [false, true]);
            } else if matches!(
                fault,
                Fault::WatchdogRevoked | Fault::WatchdogBadTail | Fault::WatchdogRawTail
            ) {
                assert!(completion.starts_with(&[false, true, false]));
                assert!(completion[2..].iter().all(|proof| !proof));
            } else {
                assert!(completion.iter().all(|complete| !complete));
            }
            assert_eq!(
                packet_count, 1,
                "diagnostic Raw must never become observations"
            );
            assert_eq!(
                diagnostic[0].state,
                if matches!(fault, Fault::WatchdogMissingBoundary) {
                    State::Failed
                } else {
                    State::Stopping
                }
            );
            assert_eq!(diagnostic[0].cause, 3);
            if matches!(
                fault,
                Fault::WatchdogTerminal | Fault::WatchdogLoss | Fault::WatchdogUnread
            ) {
                assert_eq!(diagnostic.len(), 2);
                assert_eq!(
                    (
                        diagnostic[1].state,
                        diagnostic[1].cause,
                        diagnostic[1].evidence
                    ),
                    (
                        State::Failed,
                        3,
                        if matches!(fault, Fault::WatchdogLoss | Fault::WatchdogUnread) {
                            7
                        } else {
                            15
                        }
                    )
                );
                assert_eq!(diagnostic[1].app_uptime_ms, 3101);
                assert_eq!(
                    diagnostic[1].uart_dropped_bytes,
                    u64::from(matches!(fault, Fault::WatchdogLoss))
                );
                assert_eq!(
                    diagnostic[1].unread_ring_bytes,
                    u32::from(matches!(fault, Fault::WatchdogUnread))
                );
            } else if matches!(fault, Fault::WatchdogRevoked) {
                assert_eq!(diagnostic.len(), 3);
                assert_eq!(diagnostic[1].evidence, 15);
                assert_eq!(diagnostic[2].evidence, 7);
                assert_eq!(diagnostic[2].uart_dropped_bytes, 1);
                assert_eq!(diagnostic[2].state, State::Failed);
            } else if matches!(fault, Fault::WatchdogBadTail | Fault::WatchdogRawTail) {
                assert!(diagnostic.len() >= 3);
                assert_eq!(diagnostic[1].evidence, 15);
                assert_eq!(diagnostic.last().unwrap(), &diagnostic[1]);
            } else if matches!(fault, Fault::WatchdogRepeated) {
                let pairs = control.repeated_pairs.load(Ordering::Acquire);
                assert!(
                    (1..60).contains(&pairs),
                    "host must time out before the noisy peer finishes"
                );
                assert!(diagnostic.len() >= 2);
                assert!(diagnostic.iter().all(|s| s.state == State::Stopping));
            } else {
                assert!(diagnostic.iter().all(|report| report == &diagnostic[0]));
            }
            // A noisy peer cannot renew the fixed diagnostic drain. Its next
            // 75ms fragment arrives during best-effort RPC close, at3300ms.
            let expected = match fault {
                Fault::WatchdogMissing => Duration::from_millis(3250),
                Fault::WatchdogRepeated => Duration::from_millis(3300),
                _ => Duration::ZERO,
            };
            assert_eq!(clock.now().duration_since(started), expected, "{fault:?}");
        }
    }

    #[test]
    fn watchdog_terminal_waits_for_both_already_submitted_acknowledgments() {
        for fault in [
            Fault::WatchdogPendingRpc,
            Fault::WatchdogPendingBridge,
            Fault::WatchdogMissingRpcAck,
            Fault::WatchdogMissingBridgeAck,
            Fault::WatchdogWrongRpcAck,
            Fault::WatchdogWrongBridgeAck,
        ] {
            let (mut io, other) = transport();
            let clock = io.clock.clone();
            let peer = peer(other, fault);
            let mut proof = Vec::new();
            let started = clock.now();
            let error = run_with_clock(
                &mut io,
                &clock,
                [7; 16],
                || Desired {
                    enabled: true,
                    shutdown: false,
                },
                |event| {
                    if let Output::FailureStatus { stop_complete, .. } = event {
                        proof.push(stop_complete);
                    }
                    Ok(())
                },
            )
            .unwrap_err();
            drop(io);
            let commands = peer.join().unwrap();
            assert_eq!(
                commands,
                [Command::Hello, Command::StartBeacon, Command::Heartbeat]
            );
            assert_eq!(
                error.to_string(),
                "Bridge reported autonomous stop or failure"
            );
            let missing = matches!(
                fault,
                Fault::WatchdogMissingRpcAck | Fault::WatchdogMissingBridgeAck
            );
            let valid = matches!(
                fault,
                Fault::WatchdogPendingRpc | Fault::WatchdogPendingBridge
            );
            assert_eq!(proof.last(), Some(&valid), "pending {fault:?}: {proof:?}");
            assert!(
                proof.contains(&true),
                "terminal was validated before pending-ACK cleanup"
            );
            if missing {
                assert_eq!(
                    clock.now().duration_since(started),
                    Duration::from_millis(4250)
                );
            }
        }
    }

    #[test]
    fn coalesced_rpc_messages_survive_visitor_failure_exactly_once() {
        let frame = |state| {
            rpc::encode(&proto::Main {
                content: Some(Content::AppStateResponse(app::AppStateResponse { state })),
                ..Default::default()
            })
            .unwrap()
        };
        let mut bytes = frame(1);
        bytes.extend(frame(0));
        let mut io = io::Cursor::new(bytes);
        let mut link = Link {
            io: &mut io,
            clock: &SystemClock,
            decoder: rpc::Decoder::default(),
            correlation: rpc::Correlation::default(),
            pending_messages: VecDeque::new(),
        };
        let mut seen = vec![];
        assert!(link
            .pump(|event| {
                seen.push(event);
                Err(invalid("initial failure"))
            })
            .is_err());
        assert_eq!(link.pending_messages.len(), 1);
        link.pump(|event| {
            seen.push(event);
            Ok(())
        })
        .unwrap();
        assert!(link.pending_messages.is_empty());
        assert_eq!(
            seen,
            [Received::ApplicationState(1), Received::ApplicationState(0)]
        );
        assert!(link.pump(|_| panic!("no replay")).is_err());
    }

    #[test]
    fn actual_rpc_bytes_drive_pause_resume_finish_without_implicit_reconnect() {
        let (mut io, other) = transport();
        let clock = io.clock.clone();
        let peer = peer(other, Fault::None);
        let wanted = Arc::new(AtomicU8::new(1));
        let read = wanted.clone();
        let mut packets = 0;
        run_with_clock(
            &mut io,
            &clock,
            [7; 16],
            || Desired {
                enabled: read.load(Ordering::Acquire) == 1,
                shutdown: read.load(Ordering::Acquire) == 2,
            },
            |event| {
                match event {
                    Output::Bridge(Event::Raw {
                        packets: values, ..
                    }) if !values.is_empty() => {
                        packets += values.len();
                        wanted.store(if packets == 1 { 0 } else { 2 }, Ordering::Release);
                    }
                    Output::Bridge(Event::Status(s)) if s.state == State::Paused => {
                        wanted.store(1, Ordering::Release);
                    }
                    _ => (),
                }
                Ok(())
            },
        )
        .unwrap();
        drop(io);
        assert_eq!(packets, 2);
        assert_eq!(
            peer.join().unwrap(),
            [
                Command::Hello,
                Command::StartBeacon,
                Command::Pause,
                Command::StartBeacon,
                Command::Stop
            ]
        );
    }
    #[test]
    fn bound_failure_retains_producer_counters_without_clean_success() {
        let (mut io, other) = transport();
        let clock = io.clock.clone();
        let peer = peer(other, Fault::Loss);
        let mut reported = None;
        assert!(run_with_clock(
            &mut io,
            &clock,
            [7; 16],
            || Desired {
                enabled: true,
                shutdown: false
            },
            |event| {
                if let Output::FailureStatus {
                    status,
                    stop_complete,
                } = event
                {
                    assert!(!stop_complete);
                    reported = Some(status);
                }
                Ok(())
            }
        )
        .is_err());
        drop(io);
        peer.join().unwrap();
        let status = reported.unwrap();
        assert_eq!(status.uart_dropped_bytes, 17);
        assert_eq!(status.cause, 5);
        assert_eq!(status.state, State::Failed);
    }
    #[test]
    fn disabled_attachment_can_identify_and_stop_without_starting_uart() {
        for fault in [Fault::None, Fault::StateFirst] {
            let (mut io, other) = transport();
            let clock = io.clock.clone();
            let peer = peer(other, fault);
            let stop = Arc::new(AtomicU8::new(0));
            let wanted = stop.clone();
            let mut terminal = None;
            run_with_clock(
                &mut io,
                &clock,
                [9; 16],
                || Desired {
                    enabled: false,
                    shutdown: wanted.load(Ordering::Acquire) != 0,
                },
                |event| {
                    match event {
                        Output::Bridge(Event::Hello { .. }) => {
                            stop.store(1, Ordering::Release);
                        }
                        Output::Bridge(Event::Status(s)) if s.state == State::Stopped => {
                            terminal = Some(s);
                        }
                        Output::Bridge(Event::Raw { .. }) => {
                            panic!("unopened bridge emitted UART bytes")
                        }
                        _ => (),
                    }
                    Ok(())
                },
            )
            .unwrap();
            drop(io);
            assert_eq!(peer.join().unwrap(), [Command::Hello, Command::Stop]);
            let terminal = terminal.unwrap();
            assert_eq!(terminal.generation, 0);
            assert_eq!(terminal.evidence, 0);
            assert_eq!(terminal.uart_rx_bytes, 0);
        }
    }

    #[test]
    fn missing_app_started_times_out_without_hello_or_rf() {
        let (mut io, other) = transport();
        let clock = io.clock.clone();
        let peer = peer(other, Fault::MissingAppStarted);
        let before = clock.now();
        assert!(run_with_clock(
            &mut io,
            &clock,
            [8; 16],
            || Desired {
                enabled: true,
                shutdown: false
            },
            |_| Ok(())
        )
        .is_err());
        assert_eq!(clock.now().duration_since(before), TRANSITION_TIMEOUT);
        drop(io);
        assert!(peer.join().unwrap().is_empty());
    }

    #[test]
    fn missing_or_malformed_hello_never_starts_or_retries_and_closes_owned_rpc() {
        for fault in [Fault::MissingHello, Fault::MalformedHello] {
            let (mut io, other) = transport();
            let clock = io.clock.clone();
            let peer = peer(other, fault);
            let started = clock.now();
            assert!(run_with_clock(
                &mut io,
                &clock,
                [8; 16],
                || Desired {
                    enabled: true,
                    shutdown: false
                },
                |_| Ok(())
            )
            .is_err());
            assert_eq!(
                clock.now().duration_since(started),
                if matches!(fault, Fault::MissingHello) {
                    ACK_TIMEOUT
                } else {
                    Duration::ZERO
                }
            );
            drop(io);
            assert_eq!(peer.join().unwrap(), [Command::Hello]);
        }
    }
    #[test]
    fn real_unix_stream_bytes_complete_without_clock_progress() {
        let (io, other) = UnixStream::pair().unwrap();
        // Only a broken-fixture guard. Protocol correctness uses the frozen
        // injected clock and completed peer I/O, never this wall-clock limit.
        io.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        io.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
        other
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        other
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let peer = test_transport::SocketPeer {
            abort: io.try_clone().unwrap(),
            thread: Some(peer(test_transport::GuardedIo(other), Fault::None)),
        };
        let mut io = test_transport::GuardedIo(io);
        let clock = test_transport::ManualClock::new();
        let start = clock.now();
        let stop = std::cell::Cell::new(false);
        run_with_clock(
            &mut io,
            &clock,
            [9; 16],
            || Desired {
                enabled: false,
                shutdown: stop.get(),
            },
            |event| {
                if matches!(event, Output::Bridge(Event::Hello { .. })) {
                    stop.set(true);
                }
                Ok(())
            },
        )
        .unwrap();
        drop(io);
        assert_eq!(peer.join().unwrap(), [Command::Hello, Command::Stop]);
        assert_eq!(clock.now(), start);
    }
    #[test]
    fn cli_and_write_deadlines_expire_at_the_exact_injected_boundary() {
        struct Blocked<'a> {
            clock: &'a test_transport::ManualClock,
            steps: VecDeque<Duration>,
            calls: usize,
        }
        impl Blocked<'_> {
            fn step(&mut self) -> io::Error {
                self.calls += 1;
                self.clock.advance(
                    self.steps
                        .pop_front()
                        .expect("I/O called at expired deadline"),
                );
                io::ErrorKind::WouldBlock.into()
            }
        }
        impl Read for Blocked<'_> {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(self.step())
            }
        }
        impl Write for Blocked<'_> {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(self.step())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for writing in [false, true] {
            let clock = test_transport::ManualClock::new();
            let deadline = clock.now() + ACK_TIMEOUT;
            let mut io = Blocked {
                clock: &clock,
                steps: [
                    ACK_TIMEOUT - Duration::from_nanos(1),
                    Duration::from_nanos(1),
                ]
                .into(),
                calls: 0,
            };
            let error = if writing {
                write_bounded(&mut io, &clock, b"command", deadline).unwrap_err()
            } else {
                read_until(&mut io, &clock, b"prompt", 128, deadline).unwrap_err()
            };
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert_eq!(clock.now(), deadline);
            assert_eq!(
                io.calls, 2,
                "one read just before deadline, none at deadline"
            );
        }
    }
    #[test]
    fn real_socket_guard_expiry_is_terminal_with_frozen_correctness_time() {
        struct Expired;
        impl Read for Expired {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::TimedOut.into())
            }
        }
        impl Write for Expired {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::WouldBlock.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let clock = test_transport::ManualClock::new();
        let start = clock.now();
        let mut io = test_transport::GuardedIo(Expired);
        assert_eq!(
            read_until(&mut io, &clock, b"prompt", 128, start + ACK_TIMEOUT)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Other
        );
        assert_eq!(
            write_bounded(&mut io, &clock, b"command", start + ACK_TIMEOUT)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Other
        );
        assert_eq!(clock.now(), start);
    }

    #[test]
    fn real_socket_peer_is_joined_after_assertion_unwind() {
        let (io, mut other) = UnixStream::pair().unwrap();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (finished, done) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            entered.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(other.read(&mut byte).unwrap(), 0);
            finished.send(()).unwrap();
        });
        ready
            .recv_timeout(Duration::from_secs(30))
            .expect("peer startup hang guard");
        let owned = test_transport::SocketPeer {
            abort: io.try_clone().unwrap(),
            thread: Some(thread),
        };
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owned = owned;
            panic!("injected fixture assertion");
        }));
        assert!(caught.is_err());
        // Drop completed join, so this is one immediate completion assertion.
        done.try_recv()
            .expect("peer must be reaped before unwind completes");
    }
}
