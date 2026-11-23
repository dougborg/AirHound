//! AHFB v1 allocation-free wire codec shared by the host and Flipper bridge.
//! RPC transport acceptance is distinct from observed UART start/stop evidence.

pub const HEADER_BYTES: usize = 44;
pub const MAX_PAYLOAD: usize = 256;
pub const MAX_MESSAGE: usize = HEADER_BYTES + MAX_PAYLOAD;
pub const WATCHDOG_MS: u32 = 3000;
/// Bit3 requires explicit bounded stop-completion evidence before acceptance.
pub const CAPABILITIES: u32 = 15;
pub const STOP_COMPLETE: u16 = 8;
pub const STOP_REQUEST_EVIDENCE: u16 = 4 | STOP_COMPLETE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Length,
    Version,
    Reserved,
    Type,
    Value,
    Sequence,
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    Hello = 1,
    StartBeacon = 2,
    Pause = 3,
    Stop = 4,
    Heartbeat = 5,
}
impl Command {
    pub fn from_byte(n: u8) -> Result<Self, Error> {
        match n {
            1 => Ok(Self::Hello),
            2 => Ok(Self::StartBeacon),
            3 => Ok(Self::Pause),
            4 => Ok(Self::Stop),
            5 => Ok(Self::Heartbeat),
            _ => Err(Error::Type),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub request_id: u32,
    pub nonce: [u8; 16],
    /// Random launch token, never a firmware boot identifier.
    pub app_instance: u64,
    pub sequence: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hello<'a> {
    pub capabilities: u32,
    /// Compiled Flipper C ABI, not RPC wire version or device firmware identity.
    pub api_major: u16,
    pub api_minor: u16,
    pub uart_baud: u32,
    pub watchdog_ms: u32,
    pub app_version: &'a str,
    pub source_revision: &'a str,
    /// 0 clean, 1 modified, 2 unknown.
    pub source_modified: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Ready = 0,
    Preflight = 1,
    Capturing = 2,
    Stopping = 3,
    Paused = 4,
    Stopped = 5,
    Failed = 6,
}
impl State {
    fn from_byte(n: u8) -> Result<Self, Error> {
        match n {
            0 => Ok(Self::Ready),
            1 => Ok(Self::Preflight),
            2 => Ok(Self::Capturing),
            3 => Ok(Self::Stopping),
            4 => Ok(Self::Paused),
            5 => Ok(Self::Stopped),
            6 => Ok(Self::Failed),
            _ => Err(Error::Value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// 0 none, 1 pause, 2 stop, 3 watchdog, 4 transport, 5 UART loss,
    /// 6 framing, 7 preflight failure, 8 queue loss, 9 cancellation.
    pub cause: u8,
    /// Bits 0/1/2: observed preflight-stop/start/stop text; not physical RF proof.
    pub evidence: u16,
    pub generation: u32,
    pub app_uptime_ms: u32,
    pub uart_rx_bytes: u64,
    pub uart_dropped_bytes: u64,
    pub uart_error_events: u64,
    pub rpc_dropped_messages: u64,
    pub unread_ring_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body<'a> {
    Command(Command),
    Hello(Hello<'a>),
    /// Result 0 accepted, 1 invalid state, 2 payload, 3 stale, 4 unavailable.
    Ack {
        command: Command,
        result: u8,
    },
    Status(Status),
    Raw {
        generation: u32,
        app_uptime_ms: u32,
        bytes: &'a [u8],
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message<'a> {
    pub header: Header,
    pub body: Body<'a>,
}

fn u16le(b: &[u8], n: usize) -> u16 {
    u16::from_le_bytes([b[n], b[n + 1]])
}
fn u32le(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes(b[n..n + 4].try_into().unwrap())
}
fn u64le(b: &[u8], n: usize) -> u64 {
    u64::from_le_bytes(b[n..n + 8].try_into().unwrap())
}

pub fn decode(data: &[u8]) -> Result<Message<'_>, Error> {
    if data.len() < HEADER_BYTES || data.len() > MAX_MESSAGE {
        return Err(Error::Length);
    }
    if &data[..4] != b"AHFB" || data[4] != 1 {
        return Err(Error::Version);
    }
    if u16le(data, 6) != 0 || u16le(data, 42) != 0 {
        return Err(Error::Reserved);
    }
    let len = usize::from(u16le(data, 40));
    if len > MAX_PAYLOAD || data.len() != HEADER_BYTES + len {
        return Err(Error::Length);
    }
    let header = Header {
        request_id: u32le(data, 8),
        nonce: data[12..28].try_into().unwrap(),
        app_instance: u64le(data, 28),
        sequence: u32le(data, 36),
    };
    if header.nonce == [0; 16] {
        return Err(Error::Session);
    }
    let p = &data[HEADER_BYTES..];
    let body = match data[5] {
        1..=5 => {
            if len != 0 {
                return Err(Error::Length);
            }
            let command = Command::from_byte(data[5])?;
            if header.request_id == 0
                || header.sequence != 0
                || ((command == Command::Hello) != (header.app_instance == 0))
            {
                return Err(Error::Session);
            }
            Body::Command(command)
        }
        0x81 => {
            if len < 19 {
                return Err(Error::Length);
            }
            let version_len = usize::from(p[16]);
            if version_len == 0 || version_len > 32 || 18 + version_len > len {
                return Err(Error::Length);
            }
            let rev_len = usize::from(p[17 + version_len]);
            if !(rev_len == 0 || rev_len == 40) || len != 19 + version_len + rev_len {
                return Err(Error::Length);
            }
            let version = &p[17..17 + version_len];
            let revision = &p[18 + version_len..18 + version_len + rev_len];
            if !version
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || b".+_-".contains(b))
                || !revision.iter().all(u8::is_ascii_hexdigit)
            {
                return Err(Error::Value);
            }
            let hello = Hello {
                capabilities: u32le(p, 0),
                api_major: u16le(p, 4),
                api_minor: u16le(p, 6),
                uart_baud: u32le(p, 8),
                watchdog_ms: u32le(p, 12),
                app_version: core::str::from_utf8(version).map_err(|_| Error::Value)?,
                source_revision: core::str::from_utf8(revision).map_err(|_| Error::Value)?,
                source_modified: p[len - 1],
            };
            if hello.capabilities & !CAPABILITIES != 0
                || hello.source_modified > 2
                || hello.watchdog_ms != WATCHDOG_MS
                || hello.uart_baud != 115200
            {
                return Err(Error::Value);
            }
            if header.request_id == 0 {
                return Err(Error::Session);
            }
            Body::Hello(hello)
        }
        0x82 => {
            if len != 2 {
                return Err(Error::Length);
            }
            if header.request_id == 0 || p[1] > 4 {
                return Err(Error::Value);
            }
            Body::Ack {
                command: Command::from_byte(p[0])?,
                result: p[1],
            }
        }
        0x83 => {
            if len != 48 {
                return Err(Error::Length);
            }
            let status = Status {
                state: State::from_byte(p[0])?,
                cause: p[1],
                evidence: u16le(p, 2),
                generation: u32le(p, 4),
                app_uptime_ms: u32le(p, 8),
                uart_rx_bytes: u64le(p, 12),
                uart_dropped_bytes: u64le(p, 20),
                uart_error_events: u64le(p, 28),
                rpc_dropped_messages: u64le(p, 36),
                unread_ring_bytes: u32le(p, 44),
            };
            if status.cause > 14 || status.evidence & !15 != 0 {
                return Err(Error::Value);
            }
            Body::Status(status)
        }
        0x84 => {
            if !(9..=MAX_PAYLOAD).contains(&len) || u32le(p, 0) == 0 || header.request_id != 0 {
                return Err(Error::Length);
            }
            Body::Raw {
                generation: u32le(p, 0),
                app_uptime_ms: u32le(p, 4),
                bytes: &p[8..],
            }
        }
        _ => return Err(Error::Type),
    };
    if !matches!(body, Body::Command(_)) && (header.app_instance == 0 || header.sequence == 0) {
        return Err(Error::Session);
    }
    Ok(Message { header, body })
}

pub fn encode(message: Message<'_>, out: &mut [u8; MAX_MESSAGE]) -> Result<usize, Error> {
    out.fill(0);
    out[..4].copy_from_slice(b"AHFB");
    out[4] = 1;
    out[8..12].copy_from_slice(&message.header.request_id.to_le_bytes());
    out[12..28].copy_from_slice(&message.header.nonce);
    out[28..36].copy_from_slice(&message.header.app_instance.to_le_bytes());
    out[36..40].copy_from_slice(&message.header.sequence.to_le_bytes());
    let p = &mut out[HEADER_BYTES..];
    let (kind, len) = match message.body {
        Body::Command(c) => (c as u8, 0),
        Body::Hello(h) => {
            if h.app_version.len() > 32 || h.source_revision.len() > 40 {
                return Err(Error::Length);
            }
            p[0..4].copy_from_slice(&h.capabilities.to_le_bytes());
            p[4..6].copy_from_slice(&h.api_major.to_le_bytes());
            p[6..8].copy_from_slice(&h.api_minor.to_le_bytes());
            p[8..12].copy_from_slice(&h.uart_baud.to_le_bytes());
            p[12..16].copy_from_slice(&h.watchdog_ms.to_le_bytes());
            p[16] = h.app_version.len() as u8;
            p[17..17 + h.app_version.len()].copy_from_slice(h.app_version.as_bytes());
            let n = 17 + h.app_version.len();
            p[n] = h.source_revision.len() as u8;
            p[n + 1..n + 1 + h.source_revision.len()].copy_from_slice(h.source_revision.as_bytes());
            p[n + 1 + h.source_revision.len()] = h.source_modified;
            (0x81, n + 2 + h.source_revision.len())
        }
        Body::Ack { command, result } => {
            p[0] = command as u8;
            p[1] = result;
            (0x82, 2)
        }
        Body::Status(s) => {
            p[0] = s.state as u8;
            p[1] = s.cause;
            p[2..4].copy_from_slice(&s.evidence.to_le_bytes());
            p[4..8].copy_from_slice(&s.generation.to_le_bytes());
            p[8..12].copy_from_slice(&s.app_uptime_ms.to_le_bytes());
            p[12..20].copy_from_slice(&s.uart_rx_bytes.to_le_bytes());
            p[20..28].copy_from_slice(&s.uart_dropped_bytes.to_le_bytes());
            p[28..36].copy_from_slice(&s.uart_error_events.to_le_bytes());
            p[36..44].copy_from_slice(&s.rpc_dropped_messages.to_le_bytes());
            p[44..48].copy_from_slice(&s.unread_ring_bytes.to_le_bytes());
            (0x83, 48)
        }
        Body::Raw {
            generation,
            app_uptime_ms,
            bytes,
        } => {
            if bytes.is_empty() || bytes.len() > 248 {
                return Err(Error::Length);
            }
            p[..4].copy_from_slice(&generation.to_le_bytes());
            p[4..8].copy_from_slice(&app_uptime_ms.to_le_bytes());
            p[8..8 + bytes.len()].copy_from_slice(bytes);
            (0x84, 8 + bytes.len())
        }
    };
    out[5] = kind;
    out[40..42].copy_from_slice(&(len as u16).to_le_bytes());
    decode(&out[..HEADER_BYTES + len])?;
    Ok(HEADER_BYTES + len)
}

/// Host-side binding: stale peers/gaps never reset a live stream implicitly.
pub struct Binding {
    nonce: [u8; 16],
    instance: u64,
    sequence: u32,
    hello_request: u32,
    failed: bool,
}
impl Binding {
    pub fn new(nonce: [u8; 16], hello_request: u32) -> Result<Self, Error> {
        if nonce == [0; 16] || hello_request == 0 {
            return Err(Error::Session);
        }
        Ok(Self {
            nonce,
            instance: 0,
            sequence: 0,
            hello_request,
            failed: false,
        })
    }
    fn accept_inner(&mut self, m: Message<'_>) -> Result<(), Error> {
        if matches!(m.body, Body::Command(_))
            || m.header.nonce != self.nonce
            || m.header.app_instance == 0
        {
            return Err(Error::Session);
        }
        if m.header.sequence != self.sequence.checked_add(1).ok_or(Error::Sequence)? {
            return Err(Error::Sequence);
        }
        if self.instance == 0 {
            if !matches!(m.body, Body::Hello(_)) || m.header.request_id != self.hello_request {
                return Err(Error::Session);
            }
        } else if m.header.app_instance != self.instance || matches!(m.body, Body::Hello(_)) {
            return Err(Error::Session);
        }
        self.instance = m.header.app_instance;
        self.sequence = m.header.sequence;
        Ok(())
    }
    pub fn accept(&mut self, m: Message<'_>) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Session);
        }
        let result = self.accept_inner(m);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn instance(&self) -> u64 {
        self.instance
    }
}
