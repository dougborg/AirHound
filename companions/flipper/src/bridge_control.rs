//! Pure live-bridge controller. UART and RPC I/O belong to separate bounded loops.
//! Every returned UART action requires publishing its current status first. If
//! publication fails, the caller must fail the session and must not send Start.
use crate::{
    bridge::{Command, Header, State, Status, WATCHDOG_MS},
    diagnostic::Witness,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartAction {
    StartBeacon,
    Stop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub hello: bool,
    pub uart: Option<UartAction>,
}

pub struct Controller {
    nonce: Option<[u8; 16]>,
    instance: u64,
    last_request: u32,
    lifecycle_request: u32,
    heartbeat: u32,
    since: u32,
    last_rx: u32,
    pub state: State,
    pub cause: u8,
    pub generation: u32,
    pub evidence: u16,
    start_sent: bool,
    stop_baseline: u32,
    stop_attempts: u8,
    pause_target: bool,
    witness: Witness,
}
impl Controller {
    pub fn new(instance: u64, now: u32) -> Self {
        assert_ne!(instance, 0);
        Self {
            nonce: None,
            instance,
            last_request: 0,
            lifecycle_request: 0,
            heartbeat: now,
            since: now,
            last_rx: now,
            state: State::Ready,
            cause: 0,
            generation: 0,
            evidence: 0,
            start_sent: false,
            stop_baseline: 0,
            stop_attempts: 0,
            pause_target: false,
            witness: Witness::new(),
        }
    }
    pub fn nonce(&self) -> Option<[u8; 16]> {
        self.nonce
    }
    pub fn request_id(&self) -> u32 {
        self.lifecycle_request
    }
    pub fn request(&mut self, h: Header, c: Command, now: u32) -> Result<Accepted, u8> {
        if h.sequence != 0 || h.request_id == 0 || h.nonce == [0; 16] {
            return Err(2);
        }
        if c == Command::Hello {
            if self.nonce.is_some() || h.app_instance != 0 {
                return Err(1);
            }
            self.nonce = Some(h.nonce);
            self.last_request = h.request_id;
            self.heartbeat = now;
            return Ok(Accepted {
                hello: true,
                uart: None,
            });
        }
        if self.nonce != Some(h.nonce) || self.instance != h.app_instance {
            return Err(2);
        }
        if h.request_id <= self.last_request {
            return Err(3);
        }
        // Even rejected controls consume their ID; they never renew the watchdog.
        self.last_request = h.request_id;
        let uart = match c {
            Command::Hello => unreachable!(),
            Command::Heartbeat => {
                if matches!(self.state, State::Stopped | State::Failed) {
                    return Err(1);
                }
                None
            }
            Command::StartBeacon => {
                if !matches!(self.state, State::Ready | State::Paused) {
                    return Err(1);
                }
                self.generation = self.generation.checked_add(1).ok_or(4u8)?;
                self.state = State::Preflight;
                self.cause = 0;
                self.evidence = 0;
                self.start_sent = false;
                self.witness = Witness::new();
                self.since = now;
                self.last_rx = now;
                self.lifecycle_request = h.request_id;
                Some(UartAction::Stop)
            }
            Command::Pause => {
                if !matches!(self.state, State::Preflight | State::Capturing) {
                    return Err(1);
                }
                self.lifecycle_request = h.request_id;
                self.begin_stop(now, 1, true)
            }
            Command::Stop => {
                if matches!(self.state, State::Stopped | State::Failed)
                    || (self.state == State::Stopping && self.cause != 1)
                {
                    return Err(1);
                }
                self.lifecycle_request = h.request_id;
                let old_deadline = (self.state == State::Stopping).then_some(self.since);
                let action = self.begin_stop(now, 2, false);
                if let Some(since) = old_deadline {
                    self.since = since;
                }
                action
            }
        };
        self.heartbeat = now;
        Ok(Accepted { hello: false, uart })
    }
    fn begin_stop(&mut self, now: u32, cause: u8, pause: bool) -> Option<UartAction> {
        self.cause = cause;
        self.pause_target = pause;
        if self.generation == 0 {
            self.state = if cause == 2 {
                State::Stopped
            } else {
                State::Failed
            };
            return None;
        }
        self.state = State::Stopping;
        self.since = now;
        self.stop_baseline = self.witness.stops;
        self.stop_attempts = 1;
        self.evidence &= !4;
        Some(UartAction::Stop)
    }
    /// Transport/queue/UART failures cannot renew a session or restart RF.
    pub fn fail(&mut self, now: u32, cause: u8) -> Option<UartAction> {
        self.lifecycle_request = 0;
        if self.state == State::Stopped {
            // Final ISR/queue counters may reveal loss after the last poll.
            self.state = State::Failed;
            self.cause = cause;
            return None;
        }
        if self.state == State::Failed {
            return None;
        }
        if self.state == State::Stopping {
            self.pause_target = false;
            self.cause = cause;
            return None;
        }
        self.begin_stop(now, cause, false)
    }
    pub fn feed(&mut self, now: u32, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.last_rx = now;
        }
        for &b in bytes {
            self.witness.feed(b)
        }
    }
    pub fn poll(&mut self, now: u32) -> Option<UartAction> {
        if self.nonce.is_some()
            && !matches!(self.state, State::Stopped | State::Failed | State::Stopping)
            && now.wrapping_sub(self.heartbeat) >= WATCHDOG_MS
        {
            return self.fail(now, 3);
        }
        if self.witness.malformed
            && !matches!(self.state, State::Stopped | State::Failed | State::Stopping)
        {
            return self.fail(now, 6);
        }
        match self.state {
            State::Preflight if !self.start_sent => {
                if now.wrapping_sub(self.since) >= 1500 {
                    return self.fail(now, 7);
                }
                if self.witness.stops > 0
                    && self.witness.outside()
                    && now.wrapping_sub(self.last_rx) >= 50
                {
                    self.evidence |= 1;
                    self.start_sent = true;
                    self.since = now;
                    self.witness = Witness::new();
                    return Some(UartAction::StartBeacon);
                }
            }
            State::Preflight => {
                if self.witness.starts > 0 {
                    self.evidence |= 2;
                    self.state = State::Capturing;
                } else if now.wrapping_sub(self.since) >= 1500 {
                    return self.fail(now, 7);
                }
            }
            State::Stopping => {
                if self.witness.stops > self.stop_baseline {
                    self.evidence |= 4;
                }
                let acknowledged = self.evidence & 4 != 0
                    && self.witness.outside()
                    && now.wrapping_sub(self.last_rx) >= 50
                    && now.wrapping_sub(self.since) >= 100;
                if acknowledged || now.wrapping_sub(self.since) >= 1000 {
                    self.state =
                        if acknowledged && !self.witness.malformed && matches!(self.cause, 1 | 2) {
                            if self.pause_target {
                                State::Paused
                            } else {
                                State::Stopped
                            }
                        } else {
                            State::Failed
                        };
                } else if self.evidence & 4 == 0
                    && self.stop_attempts == 1
                    && now.wrapping_sub(self.since) >= 500
                {
                    self.stop_attempts = 2;
                    return Some(UartAction::Stop);
                }
            }
            _ => {}
        }
        None
    }
    pub fn status(&self, now: u32) -> Status {
        Status {
            state: self.state,
            cause: self.cause,
            evidence: self.evidence,
            generation: self.generation,
            app_uptime_ms: now,
            uart_rx_bytes: 0,
            uart_dropped_bytes: 0,
            uart_error_events: 0,
            rpc_dropped_messages: 0,
            unread_ring_bytes: 0,
        }
    }
}
