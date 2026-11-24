//! Causal byte transport for the protocol peer. Only explicit peer quiescence or
//! a requested fragment delay advances virtual time. The wall-clock receive
//! guard diagnoses a broken fixture; it never determines a successful result.
use super::*;
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};

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
    /// Only after the protocol run has returned, complete already queued fixture
    /// actions so a peer between its finished check and write can exit and join.
    /// These bytes are discarded, never delivered to the protocol or its clock.
    fn finish_peer(&mut self, finished: &AtomicBool) -> io::Result<()> {
        if !finished.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "protocol still owns transport",
            ));
        }
        if let Some((_, reply)) = self.pending_read.take() {
            let _ = reply.send(Ok(Vec::new()));
        }
        if let Some((_, reply)) = self.pending_delay.take() {
            let _ = reply.send(Ok(()));
        }
        // Fixture scripts are finite. This independent action cap also catches
        // a peer that ignores cancellation without an unbounded drain loop.
        for _ in 0..64 {
            match self.actions.recv_timeout(GUARD) {
                Ok(Action::Read(_, reply)) => {
                    let _ = reply.send(Ok(Vec::new()));
                }
                Ok(Action::Write(_, reply) | Action::Delay(_, reply) | Action::Probe(reply)) => {
                    let _ = reply.send(Ok(()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "fixture peer did not finish owned teardown",
                    ))
                }
            }
        }
        Err(io::Error::other(
            "fixture peer exceeded teardown action bound",
        ))
    }
    fn disconnect_peer(&mut self) {
        let (_, disconnected) = mpsc::channel();
        self.actions = disconnected;
        self.pending_read = None;
        self.pending_delay = None;
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

/// Own the scripted peer and its transport together. Completion and assertion
/// unwinding both cancel, hand off pending actions, and join the actual thread.
pub(super) struct ScriptedPeer<T> {
    pub(super) io: Host,
    pub(super) thread: Option<std::thread::JoinHandle<T>>,
    pub(super) finished: Arc<AtomicBool>,
}
impl<T> ScriptedPeer<T> {
    pub(super) fn finish(&mut self) -> io::Result<T> {
        self.finished.store(true, Ordering::Release);
        let handoff = self.io.finish_peer(&self.finished);
        if handoff.is_err() {
            self.io.disconnect_peer();
        }
        // Always join even when the handoff reports a fixture failure.
        let result = self.thread.take().expect("peer joined once").join();
        handoff?;
        result.map_err(|_| io::Error::other("fixture peer panicked during owned teardown"))
    }
}
impl<T> Drop for ScriptedPeer<T> {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.finish();
        }
    }
}

#[test]
fn completed_run_hands_off_a_write_that_passed_the_finished_check() {
    let (io, mut other) = Host::pair();
    let finished = Arc::new(AtomicBool::new(false));
    let peer_finished = finished.clone();
    let (checked, check) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        assert!(!peer_finished.load(Ordering::Acquire));
        checked.send(()).unwrap();
        released.recv_timeout(GUARD).unwrap();
        // The write deliberately occurs after the owner finishes, despite the
        // earlier flag check. This was the failing full-suite interleaving.
        other.write_all(b"late fixture status").unwrap();
        assert!(peer_finished.load(Ordering::Acquire));
        other.delay(Duration::from_secs(1)).unwrap();
    });
    let mut peer = ScriptedPeer {
        io,
        thread: Some(thread),
        finished,
    };
    check.recv_timeout(GUARD).unwrap();
    let now = peer.io.clock.now();
    peer.finished.store(true, Ordering::Release);
    release.send(()).unwrap();
    peer.finish().unwrap();
    assert_eq!(peer.io.clock.now(), now);
    assert!(peer.io.incoming.is_empty());
    assert!(peer.io.outgoing.is_empty());
    assert!(peer.thread.is_none());
}

#[test]
fn premature_transport_close_remains_an_error() {
    let (mut io, mut other) = Host::pair();
    assert_eq!(
        io.finish_peer(&AtomicBool::new(false)).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    drop(io);
    assert_eq!(
        other
            .write_all(b"required protocol frame")
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[test]
fn assertion_unwind_still_finishes_and_joins_the_scripted_peer() {
    let (io, mut other) = Host::pair();
    let finished = Arc::new(AtomicBool::new(false));
    let (done, completed) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let mut byte = [0];
        assert_eq!(other.read(&mut byte).unwrap(), 0);
        done.send(()).unwrap();
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _peer = ScriptedPeer {
            io,
            thread: Some(thread),
            finished,
        };
        panic!("injected fixture assertion");
    }));
    assert!(result.is_err());
    // No wait: successful Drop already joined the thread that sent this event.
    assert_eq!(completed.try_recv(), Ok(()));
}

/// Socket timeouts are hang guards only. Translate expiry into a terminal
/// fixture error so the production retry loop cannot hide it behind frozen time.
pub(super) struct GuardedIo<T>(pub(super) T);
fn terminal_guard(error: io::Error) -> io::Error {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        io::Error::other(format!("fixture socket hang guard expired: {error}"))
    } else {
        error
    }
}
impl<T: Read> Read for GuardedIo<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.read(bytes).map_err(terminal_guard)
    }
}
impl<T: Write> Write for GuardedIo<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes).map_err(terminal_guard)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush().map_err(terminal_guard)
    }
}
impl PeerTransport for GuardedIo<std::os::unix::net::UnixStream> {
    fn delay(&mut self, _: Duration) -> io::Result<()> {
        panic!("real socket smoke must not use timed faults")
    }
    fn probe_empty(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Every exit, including assertion panic, closes the fixture connection and
/// joins its only peer. The abort clone refers only to this test-owned socket.
pub(super) struct SocketPeer<T> {
    pub(super) abort: std::os::unix::net::UnixStream,
    pub(super) thread: Option<std::thread::JoinHandle<T>>,
}
impl<T> SocketPeer<T> {
    pub(super) fn join(mut self) -> std::thread::Result<T> {
        let _ = self.abort.shutdown(std::net::Shutdown::Both);
        self.thread.take().unwrap().join()
    }
}
impl<T> Drop for SocketPeer<T> {
    fn drop(&mut self) {
        let _ = self.abort.shutdown(std::net::Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
