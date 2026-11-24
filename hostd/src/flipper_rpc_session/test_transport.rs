//! Causal byte transport for the protocol peer. Only explicit peer quiescence or
//! a requested fragment delay advances virtual time. The wall-clock receive
//! guard diagnoses a broken fixture; it never determines a successful result.
use super::*;
use std::{cell::Cell, rc::Rc, sync::mpsc};

const GUARD: Duration = Duration::from_secs(30);
const STEP: Duration = Duration::from_millis(25);
type Reply<T> = mpsc::SyncSender<io::Result<T>>;

#[derive(Clone)]
pub(super) struct ManualClock(Rc<Cell<Instant>>);
impl ManualClock {
    pub(super) fn new() -> Self {
        Self(Rc::new(Cell::new(Instant::now())))
    }
    pub(super) fn advance(&self, amount: Duration) {
        self.0.set(self.now() + amount);
    }
}
impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.0.get()
    }
}

enum Action {
    Read(usize, Reply<Vec<u8>>),
    Write(Vec<u8>, Reply<()>),
    Delay(Duration, Reply<()>),
    Probe(Reply<()>),
}

pub(super) trait PeerTransport: Read + Write + Send + 'static {
    fn delay(&mut self, duration: Duration) -> io::Result<()>;
    fn probe_empty(&mut self) -> io::Result<()>;
}

pub(super) struct Peer {
    actions: mpsc::Sender<Action>,
}
impl Peer {
    fn request<T>(&self, make: impl FnOnce(Reply<T>) -> Action) -> io::Result<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.actions
            .send(make(tx))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        match rx.recv_timeout(GUARD) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::ErrorKind::BrokenPipe.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("fixture host did not service an explicit peer action")
            }
        }
    }
}
impl Read for Peer {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let value = self.request(|tx| Action::Read(bytes.len(), tx))?;
        bytes[..value.len()].copy_from_slice(&value);
        Ok(value.len())
    }
}
impl Write for Peer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.request(|tx| Action::Write(bytes.to_vec(), tx))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl PeerTransport for Peer {
    fn delay(&mut self, duration: Duration) -> io::Result<()> {
        self.request(|tx| Action::Delay(duration, tx))
    }
    fn probe_empty(&mut self) -> io::Result<()> {
        self.request(Action::Probe)
    }
}
// The separate real UnixStream smoke exercises byte framing and ownership. It
// has no injected fault delay or negative-time assertion.
impl PeerTransport for std::os::unix::net::UnixStream {
    fn delay(&mut self, _: Duration) -> io::Result<()> {
        panic!("real socket smoke must not use timed faults")
    }
    fn probe_empty(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) struct Host {
    pub(super) clock: ManualClock,
    actions: mpsc::Receiver<Action>,
    incoming: VecDeque<u8>,
    outgoing: VecDeque<u8>,
    pending_read: Option<(usize, Reply<Vec<u8>>)>,
    pending_delay: Option<(Instant, Reply<()>)>,
}
impl Host {
    pub(super) fn pair() -> (Self, Peer) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                clock: ManualClock::new(),
                actions: rx,
                incoming: VecDeque::new(),
                outgoing: VecDeque::new(),
                pending_read: None,
                pending_delay: None,
            },
            Peer { actions: tx },
        )
    }
    fn service_read(&mut self) {
        if !self.outgoing.is_empty() {
            if let Some((limit, reply)) = self.pending_read.take() {
                let n = limit.min(self.outgoing.len());
                let _ = reply.send(Ok(self.outgoing.drain(..n).collect()));
            }
        }
    }
}
impl Read for Host {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.incoming.is_empty() {
                let n = bytes.len().min(self.incoming.len());
                for byte in &mut bytes[..n] {
                    *byte = self.incoming.pop_front().unwrap();
                }
                return Ok(n);
            }
            if let Some((until, _)) = self.pending_delay.as_ref() {
                if self.clock.now() < *until {
                    self.clock
                        .advance(STEP.min(until.duration_since(self.clock.now())));
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let (_, reply) = self.pending_delay.take().unwrap();
                let _ = reply.send(Ok(()));
            }
            if self.pending_read.is_some() {
                self.service_read();
                if self.pending_read.is_some() {
                    self.clock.advance(STEP);
                    return Err(io::ErrorKind::WouldBlock.into());
                }
            }
            match self.actions.recv_timeout(GUARD) {
                Ok(Action::Read(limit, reply)) => {
                    self.pending_read = Some((limit, reply));
                }
                Ok(Action::Write(value, reply)) => {
                    self.incoming.extend(value);
                    let _ = reply.send(Ok(()));
                }
                Ok(Action::Delay(duration, reply)) => {
                    self.pending_delay = Some((self.clock.now() + duration, reply));
                }
                Ok(Action::Probe(reply)) => {
                    assert!(
                        self.outgoing.is_empty(),
                        "host sent a command before AppStarted"
                    );
                    let _ = reply.send(Ok(()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("fixture peer did not reach its next causal boundary")
                }
            }
        }
    }
}
impl Write for Host {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.outgoing.extend(bytes);
        self.service_read();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some((_, reply)) = self.pending_read.take() {
            let _ = reply.send(Ok(Vec::new()));
        }
        if let Some((_, reply)) = self.pending_delay.take() {
            let _ = reply.send(Err(io::ErrorKind::BrokenPipe.into()));
        }
    }
}
