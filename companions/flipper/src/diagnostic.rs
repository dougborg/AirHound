//! Bounded, allocation-free control for the explicitly selected UART diagnostic.
//! Text acknowledgments are observations, not authenticated proof of RF state.

pub const START: &[u8] = b"sniffbeacon -serial\r\n";
pub const STOP: &[u8] = b"stopscan\r\n";
pub const CAPACITY: usize = 48 * 1024;
const BEGIN: &[u8; 11] = b"[BUF/BEGIN]";
const CLOSE: &[u8; 11] = b"[BUF/CLOSE]";
const START_ACK: &[u8] = b"StartingBeacon sniff. Stop with stopscan";
const STOP_ACK: &[u8] = b"Stopping WiFi tran/recv";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preflight,
    Capture,
    Stopping,
    Done,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start,
    Stop,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Evidence {
    pub preflight_stop_text_ack: bool,
    pub start_sent: bool,
    pub start_text_ack: bool,
    pub stop_text_ack: bool,
    pub cancelled: bool,
    pub overflow: bool,
    pub malformed: bool,
    pub stop_attempts: u8,
    pub stop_after_ms: u32,
}

/// Recognize console lines only outside complete framed PCAP records. In-band
/// strings inside a packet never become control acknowledgments. Console text
/// itself can contain untrusted SSIDs; these observations remain unauthenticated.
pub struct Witness {
    line: [u8; 128],
    line_len: usize,
    long_line: bool,
    marker: usize,
    mode: u8,
    header: [u8; 24],
    used: usize,
    remaining: u32,
    global_seen: bool,
    snaplen: u32,
    pub malformed: bool,
    pub starts: u32,
    pub stops: u32,
    pub packets: u32,
}
impl Default for Witness {
    fn default() -> Self {
        Self::new()
    }
}
impl Witness {
    pub const fn new() -> Self {
        Self {
            line: [0; 128],
            line_len: 0,
            long_line: false,
            marker: 0,
            mode: 0,
            header: [0; 24],
            used: 0,
            remaining: 0,
            global_seen: false,
            snaplen: 0,
            malformed: false,
            starts: 0,
            stops: 0,
            packets: 0,
        }
    }
    pub fn outside(&self) -> bool {
        self.mode == 0 && self.marker == 0
    }
    fn console(&mut self, b: u8) {
        if b == b'\n' {
            let n = self.line_len.saturating_sub(usize::from(
                self.line_len > 0 && self.line[self.line_len - 1] == b'\r',
            ));
            if !self.long_line {
                if &self.line[..n] == START_ACK {
                    self.starts = self.starts.saturating_add(1);
                }
                if &self.line[..n] == STOP_ACK {
                    self.stops = self.stops.saturating_add(1);
                }
            }
            self.line_len = 0;
            self.long_line = false;
        } else if self.line_len < self.line.len() {
            self.line[self.line_len] = b;
            self.line_len += 1;
        } else {
            self.long_line = true;
        }
    }
    pub fn feed(&mut self, b: u8) {
        if self.malformed {
            return;
        }
        match self.mode {
            0 => {
                if b == BEGIN[self.marker] {
                    self.marker += 1;
                    if self.marker == BEGIN.len() {
                        self.marker = 0;
                        self.line_len = 0;
                        self.long_line = false;
                        self.used = 0;
                        self.mode = if self.global_seen { 2 } else { 1 };
                    }
                } else {
                    for &byte in BEGIN.iter().take(self.marker) {
                        self.console(byte);
                    }
                    self.marker = 0;
                    if b == BEGIN[0] {
                        self.marker = 1;
                    } else {
                        self.console(b);
                    }
                }
            }
            1 => {
                self.header[self.used] = b;
                self.used += 1;
                if self.used == 24 {
                    let h = &self.header;
                    let snap = u32::from_le_bytes(h[16..20].try_into().unwrap());
                    if h[..8] != [0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]
                        || h[8..16] != [0; 8]
                        || ![1514, 2324, 4096].contains(&snap)
                        || h[20..24] != [105, 0, 0, 0]
                    {
                        self.malformed = true;
                        return;
                    }
                    self.snaplen = snap;
                    self.global_seen = true;
                    self.mode = 2;
                    self.used = 0;
                }
            }
            2 => {
                self.header[self.used] = b;
                self.used += 1;
                if self.used == 11 && &self.header[..11] == CLOSE {
                    self.mode = 0;
                    self.used = 0;
                    return;
                }
                if self.used == 16 {
                    let micro = u32::from_le_bytes(self.header[4..8].try_into().unwrap());
                    let n = u32::from_le_bytes(self.header[8..12].try_into().unwrap());
                    let original = u32::from_le_bytes(self.header[12..16].try_into().unwrap());
                    if micro >= 1_000_000 || n == 0 || n > self.snaplen || n != original {
                        self.malformed = true;
                        return;
                    }
                    self.remaining = n;
                    self.mode = 3;
                    self.used = 0;
                }
            }
            3 => {
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.packets = self.packets.saturating_add(1);
                    self.mode = 2;
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Millisecond clock may wrap. All intervals are much shorter than half its range.
/// Caller sends initial STOP before receiving; then executes only returned commands.
/// No I/O, allocation, sleeping, UI work or filesystem operations happen here.
pub struct Run {
    pub phase: Phase,
    pub evidence: Evidence,
    pub witness: Witness,
    since: u32,
    started: u32,
    last_rx: u32,
    baseline_stops: u32,
    allow_start: bool,
}
impl Run {
    pub fn new(now: u32) -> Self {
        Self {
            phase: Phase::Preflight,
            evidence: Evidence::default(),
            witness: Witness::new(),
            since: now,
            started: now,
            last_rx: now,
            baseline_stops: 0,
            allow_start: true,
        }
    }
    /// Stop-only diagnostic: this instance can never emit Command::Start.
    pub fn preflight(now: u32) -> Self {
        Self {
            allow_start: false,
            ..Self::new(now)
        }
    }
    pub fn preflight_ready(&self, dropped: u32, errors: u32, unread: usize) -> bool {
        !self.allow_start
            && self.phase == Phase::Done
            && self.evidence.preflight_stop_text_ack
            && !self.evidence.start_sent
            && !self.evidence.cancelled
            && !self.evidence.overflow
            && !self.evidence.malformed
            && dropped == 0
            && errors == 0
            && unread == 0
            && self.witness.outside()
    }
    pub fn feed(&mut self, now: u32, bytes: &[u8]) {
        self.last_rx = now;
        for &b in bytes {
            self.witness.feed(b);
        }
    }
    /// Evaluate only after ISR shutdown, with final counters (including events
    /// after the last poll). This is transport completeness, not RF acceptance.
    pub fn raw_ready(&self, dropped: u32, errors: u32, unread_ring_bytes: usize) -> bool {
        self.phase == Phase::Done
            && self.evidence.start_sent
            && self.evidence.start_text_ack
            && self.evidence.stop_text_ack
            && !self.evidence.cancelled
            && !self.evidence.overflow
            && !self.evidence.malformed
            && dropped == 0
            && errors == 0
            && unread_ring_bytes == 0
            && self.witness.outside()
            && self.witness.packets > 0
    }
    pub fn retain_bytes(&self) -> bool {
        matches!(
            self.phase,
            Phase::Preflight | Phase::Capture | Phase::Stopping
        )
    }
    pub fn poll(&mut self, now: u32, cancel: bool, overflow: bool) -> Option<Command> {
        self.evidence.cancelled |= cancel;
        self.evidence.overflow |= overflow;
        self.evidence.malformed |= self.witness.malformed;
        match self.phase {
            Phase::Preflight => {
                self.evidence.preflight_stop_text_ack |= self.witness.stops > 0;
                if cancel
                    || overflow
                    || self.witness.malformed
                    || now.wrapping_sub(self.since) >= 1500
                {
                    self.phase = Phase::Done;
                } else if self.witness.stops > 0
                    && self.witness.outside()
                    && now.wrapping_sub(self.last_rx) >= 50
                {
                    self.evidence.preflight_stop_text_ack = true;
                    if !self.allow_start {
                        self.phase = Phase::Done;
                        return None;
                    }
                    self.phase = Phase::Capture;
                    self.since = now;
                    self.started = now;
                    self.witness = Witness::new();
                    self.evidence.start_sent = true;
                    return Some(Command::Start);
                }
            }
            Phase::Capture => {
                self.evidence.start_text_ack = self.witness.starts > 0;
                if cancel
                    || overflow
                    || self.witness.malformed
                    || now.wrapping_sub(self.since) >= 2000
                {
                    self.phase = Phase::Stopping;
                    self.since = now;
                    self.baseline_stops = self.witness.stops;
                    self.evidence.stop_after_ms = now.wrapping_sub(self.started);
                    self.evidence.stop_attempts = 1;
                    return Some(Command::Stop);
                }
            }
            Phase::Stopping => {
                self.evidence.stop_text_ack = self.witness.stops > self.baseline_stops;
                if (self.evidence.stop_text_ack
                    && self.witness.outside()
                    && now.wrapping_sub(self.last_rx) >= 50
                    && now.wrapping_sub(self.since) >= 100)
                    || now.wrapping_sub(self.since) >= 1000
                {
                    self.phase = Phase::Done;
                } else if !self.evidence.stop_text_ack
                    && self.evidence.stop_attempts == 1
                    && now.wrapping_sub(self.since) >= 500
                {
                    self.evidence.stop_attempts = 2;
                    return Some(Command::Stop);
                }
            }
            Phase::Done => {}
        }
        None
    }
}

/// No silent short writes. Caller still closes the file and retains failed evidence.
pub fn write_complete(mut bytes: &[u8], mut write: impl FnMut(&[u8]) -> usize) -> bool {
    while !bytes.is_empty() {
        let n = write(bytes);
        if n == 0 || n > bytes.len() {
            return false;
        }
        bytes = &bytes[n..];
    }
    true
}

/// Storage adapters must attempt close even after a failed/short write. The
/// caller retains both outcomes; neither makes capture acceptance automatic.
pub trait Output {
    fn write(&mut self, bytes: &[u8]) -> usize;
    fn close(&mut self) -> bool;
}
#[derive(Debug, PartialEq, Eq)]
pub struct Persisted {
    pub written: bool,
    pub closed: bool,
}
pub fn persist(bytes: &[u8], output: &mut impl Output) -> Persisted {
    let written = write_complete(bytes, |part| output.write(part));
    let closed = output.close();
    Persisted { written, closed }
}

/// Entirely synthetic engine replay for the UART-free device load/exit gate.
/// Four arbitrary payload bytes exercise framing; this is not an 802.11 fixture
/// or an observation/capture acceptance claim.
pub fn synthetic_self_test() -> bool {
    let mut run = Run::new(0);
    run.feed(1, b"Stopping WiFi tran/recv\r\n");
    if run.poll(60, false, false) != Some(Command::Start) {
        return false;
    }
    run.feed(
        61,
        b"StartingBeacon sniff. Stop with stopscan\r\n[BUF/BEGIN]",
    );
    run.feed(
        62,
        &[
            0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 105, 0, 0, 0,
        ],
    );
    run.feed(
        63,
        &[0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 4, 0, 0, 0, 1, 2, 3, 4],
    );
    run.feed(64, b"[BUF/CLOSE]");
    if run.poll(2060, false, false) != Some(Command::Stop) {
        return false;
    }
    run.feed(2070, b"Stopping WiFi tran/recv\r\n");
    run.poll(2160, false, false);
    run.raw_ready(0, 0, 0) && run.witness.packets == 1
}
