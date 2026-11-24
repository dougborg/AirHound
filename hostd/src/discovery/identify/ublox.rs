//! Explicit MON-VER only (u-blox GPS.G7-SW-12001-B1 §§29,38.8).
//! No CFG messages, baud selection, modem-line changes or automatic inventory I/O.
use super::{Identification, Request};
use crate::serial_lease::{self, Lease};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
const POLL: [u8; 8] = [0xb5, 0x62, 0x0a, 0x04, 0, 0, 0x0e, 0x34];
const DEADLINE: Duration = Duration::from_secs(3);
const MAX_PAYLOAD: usize = 1000;
const MAX_BYTES: usize = 65536;
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn cancelled(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "u-blox identification cancelled",
        ))
    } else {
        Ok(())
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Version {
    software: String,
    hardware: String,
    firmware: Option<String>,
    protocol: Option<String>,
    model: Option<String>,
}
fn checksum(bytes: &[u8]) -> [u8; 2] {
    let (mut a, mut b) = (0u8, 0u8);
    for &v in bytes {
        a = a.wrapping_add(v);
        b = b.wrapping_add(a)
    }
    [a, b]
}
fn field(bytes: &[u8]) -> io::Result<String> {
    let end = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| invalid("unterminated MON-VER string"))?;
    if !bytes[..end].iter().all(|b| (32..127).contains(b)) {
        return Err(invalid("non-printable MON-VER string"));
    }
    String::from_utf8(bytes[..end].to_vec()).map_err(|_| invalid("invalid MON-VER string"))
}
fn decode(bytes: &[u8]) -> io::Result<Version> {
    if bytes.len() < 40 || bytes.len() > MAX_PAYLOAD || !(bytes.len() - 40).is_multiple_of(30) {
        return Err(invalid("invalid MON-VER payload length"));
    }
    let software = field(&bytes[..30])?;
    let hardware = field(&bytes[30..40])?;
    if software.is_empty() || hardware.is_empty() {
        return Err(invalid("empty MON-VER version"));
    }
    let mut result = Version {
        software,
        hardware,
        firmware: None,
        protocol: None,
        model: None,
    };
    for extension in bytes[40..].as_chunks::<30>().0 {
        let text = field(extension)?;
        for (prefix, value) in [
            ("FWVER=", &mut result.firmware),
            ("PROTVER=", &mut result.protocol),
            ("MOD=", &mut result.model),
        ] {
            if let Some(v) = text.strip_prefix(prefix) {
                if v.is_empty() || value.as_ref().is_some_and(|old| old != v) {
                    return Err(invalid("empty or conflicting MON-VER extension"));
                }
                *value = Some(v.to_owned());
            }
        }
    }
    Ok(result)
}
#[derive(Default)]
struct Parser {
    buffer: Vec<u8>,
    total: usize,
}
impl Parser {
    fn feed(&mut self, data: &[u8]) -> io::Result<Option<Version>> {
        self.total = self
            .total
            .checked_add(data.len())
            .ok_or_else(|| invalid("MON-VER byte budget exceeded"))?;
        if self.total > MAX_BYTES || data.len() > 4096 {
            return Err(invalid("MON-VER byte budget exceeded"));
        }
        self.buffer.extend_from_slice(data);
        loop {
            let Some(index) = self.buffer.windows(2).position(|p| p == [0xb5, 0x62]) else {
                let trailing = self.buffer.last() == Some(&0xb5);
                self.buffer.clear();
                if trailing {
                    self.buffer.push(0xb5)
                }
                return Ok(None);
            };
            self.buffer.drain(..index);
            if self.buffer.len() < 6 {
                return Ok(None);
            }
            let length = u16::from_le_bytes([self.buffer[4], self.buffer[5]]) as usize;
            if length > MAX_PAYLOAD {
                return Err(invalid("oversized UBX response"));
            }
            let end = length + 8;
            if self.buffer.len() < end {
                return Ok(None);
            }
            if checksum(&self.buffer[2..end - 2]) != self.buffer[end - 2..end] {
                return Err(invalid("UBX response checksum mismatch"));
            }
            if self.buffer[2..4] == [0x0a, 0x04] {
                return decode(&self.buffer[6..end - 2]).map(Some);
            }
            self.buffer.drain(..end);
        }
    }
}

pub(super) async fn query(request: &Request) -> Identification {
    let mut result = Identification::new(request, "unknown", "No u-blox version response returned");
    let attempt: io::Result<Version> = async {
        let lease = serial_lease::acquire(&request.port)?;
        check_owner(Path::new(&request.port)).await?;
        // Lease spans fresh binding validation and every open/read/restore/close path.
        let inventory = crate::discovery::discover(&crate::discovery::Options::default()).await;
        request.validate(&inventory)?;
        let before_binding = binding(&inventory, request)?;
        let port = PathBuf::from(&request.port);
        let (version, _lease) = run_probe(port, lease, DEADLINE).await?;
        let after = crate::discovery::discover(&crate::discovery::Options::default()).await;
        validate_completion_binding(&after, request, &before_binding)?;
        Ok(version)
    }
    .await;
    match attempt {
        Ok(v) => {
            result = Identification::new(
                request,
                "identified",
                "Receiver version observed; compatibility and reception remain untested",
            );
            result.application = Some("u-blox");
            result.software_version = Some(v.software);
            result.hardware_version = Some(v.hardware);
            result.firmware_version = v.firmware;
            result.protocol_version = v.protocol;
            result.model = v.model;
            result.message="Receiver reported u-blox version fields; module model is only reported when supplied explicitly. Compatibility and reception have not been tested".into();
        }
        Err(e) => {
            result.state = match e.kind() {
                io::ErrorKind::WouldBlock => "busy",
                io::ErrorKind::PermissionDenied => "permission_denied",
                io::ErrorKind::TimedOut => "timed_out",
                _ => "failed",
            };
            result.message = format!("u-blox identification: {e}");
        }
    }
    result
}

fn binding(
    inventory: &crate::discovery::Inventory,
    request: &Request,
) -> io::Result<serde_json::Value> {
    let device = inventory
        .devices
        .iter()
        .find(|d| d.id == request.device_id)
        .ok_or_else(|| invalid("GNSS attachment disappeared"))?;
    Ok(serde_json::json!({"id":device.id,"usb":device.usb,"endpoints":device.endpoints}))
}

fn validate_completion_binding(
    inventory: &crate::discovery::Inventory,
    request: &Request,
    before: &serde_json::Value,
) -> io::Result<()> {
    request.validate(inventory)?;
    if binding(inventory, request)? != *before {
        return Err(invalid("GNSS attachment changed during identification"));
    }
    Ok(())
}

async fn run_probe(
    port: PathBuf,
    lease: Lease,
    duration: Duration,
) -> io::Result<(Version, Lease)> {
    run_probe_with_clock(
        port,
        lease,
        duration,
        SystemProbeClock,
        #[cfg(test)]
        None,
    )
    .await
}

#[cfg(test)]
struct Completed(Option<tokio::sync::oneshot::Sender<()>>);
#[cfg(test)]
impl Drop for Completed {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            let _ = done.send(());
        }
    }
}
// Fields drop in declaration order. If the async caller is cancelled, the
// blocking result releases its lease before the test completion witness fires.
struct ProbeResult {
    observed: io::Result<Version>,
    lease: Lease,
    #[cfg(test)]
    _completed: Completed,
}
async fn run_probe_with_clock<C: ProbeClock + Send + 'static>(
    port: PathBuf,
    lease: Lease,
    duration: Duration,
    clock: C,
    #[cfg(test)] completed: Option<tokio::sync::oneshot::Sender<()>>,
) -> io::Result<(Version, Lease)> {
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_guard = CancelOnDrop(cancel.clone());
    let result = tokio::task::spawn_blocking(move || {
        let observed = probe(&port, &lease, &cancel, duration, &clock);
        ProbeResult {
            observed,
            lease,
            #[cfg(test)]
            _completed: Completed(completed),
        }
    })
    .await
    .map_err(|_| io::Error::other("u-blox identification worker failed"))?;
    Ok((result.observed?, result.lease))
}

#[cfg(target_os = "macos")]
async fn check_owner(port: &Path) -> io::Result<()> {
    check_owner_with_timeout(port, Duration::from_secs(1)).await
}

#[cfg(target_os = "macos")]
async fn check_owner_with_timeout(port: &Path, duration: Duration) -> io::Result<()> {
    use std::process::Stdio;
    use tokio::process::Command;
    let alias = port
        .to_str()
        .and_then(|p| p.strip_prefix("/dev/cu."))
        .map(|p| format!("/dev/tty.{p}"));
    let mut command = Command::new("/usr/sbin/lsof");
    command.args(["-nP", "-t", "--"]).arg(port);
    if let Some(alias) = alias {
        command.arg(alias);
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    ownership_result(child, duration).await
}

#[cfg(target_os = "macos")]
async fn ownership_result(child: tokio::process::Child, duration: Duration) -> io::Result<()> {
    ownership_result_inner(
        child,
        duration,
        #[cfg(test)]
        None,
    )
    .await
}
#[cfg(target_os = "macos")]
async fn ownership_result_inner(
    mut child: tokio::process::Child,
    duration: Duration,
    #[cfg(test)] witness: Option<crate::test_support::TimerWitness>,
) -> io::Result<()> {
    use tokio::io::AsyncReadExt;
    let deadline = tokio::time::Instant::now() + duration;
    let outcome = tokio::time::timeout_at(deadline, async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = child.stdout.take().expect("piped").take(4097);
        let mut stderr = child.stderr.take().expect("piped").take(4097);
        tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))?;
        let status = child.wait().await?;
        if !out.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "GNSS endpoint has an existing owner",
            ));
        }
        if status.code() != Some(1) || !err.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "could not establish GNSS endpoint ownership",
            ));
        }
        Ok(())
    });
    #[cfg(test)]
    let outcome = async {
        match witness {
            Some(witness) => witness.observe(deadline, outcome, Result::is_err).await,
            None => outcome.await,
        }
    };
    let outcome = outcome.await.unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "GNSS ownership check timed out",
        ))
    });
    if outcome.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    outcome
}
#[cfg(not(target_os = "macos"))]
async fn check_owner(_port: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported,
        "explicit u-blox identification requires macOS ownership inspection; gpsd capture is unchanged"))
}

struct Port {
    file: Option<File>,
    original: Option<libc::termios>,
    exclusive: bool,
}
impl Port {
    fn open(path: &Path) -> io::Result<Self> {
        let before = fs::metadata(path)?;
        if !before.file_type().is_char_device() {
            return Err(invalid("GNSS endpoint must be a character device"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let mut port = Self {
            file: Some(file),
            original: None,
            exclusive: false,
        };
        let after = port.file.as_ref().expect("open").metadata()?;
        if (before.dev(), before.ino(), before.rdev()) != (after.dev(), after.ino(), after.rdev()) {
            return Err(invalid("GNSS endpoint changed before open"));
        }
        let fd = port.fd();
        if unsafe { libc::ioctl(fd, libc::TIOCEXCL as _) } < 0 {
            return Err(io::Error::last_os_error());
        }
        port.exclusive = true;
        let mut old = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut old) } < 0 {
            return Err(io::Error::last_os_error());
        }
        port.original = Some(old);
        let mut raw = old;
        raw.c_iflag &= !(libc::BRKINT
            | libc::ICRNL
            | libc::INLCR
            | libc::IGNCR
            | libc::ISTRIP
            | libc::IXON
            | libc::IXOFF
            | libc::PARMRK);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        // No speed/cflag/modem-line changes; restore even if setting raw fails.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(port)
    }
    fn fd(&self) -> RawFd {
        self.file.as_ref().expect("open port").as_raw_fd()
    }
    fn close(&mut self) -> io::Result<()> {
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        let fd = file.as_raw_fd();
        let mut error = None;
        if let Some(original) = self.original.take() {
            let mut restored = unsafe { std::mem::zeroed::<libc::termios>() };
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &original) } < 0
                || unsafe { libc::tcgetattr(fd, &mut restored) } < 0
            {
                error = Some(io::Error::last_os_error())
            } else if !settings_equal(&original, &restored) {
                error = Some(io::Error::other(
                    "GNSS host terminal settings could not be restored",
                ))
            }
        }
        if self.exclusive && unsafe { libc::ioctl(fd, libc::TIOCNXCL as _) } < 0 && error.is_none()
        {
            error = Some(io::Error::last_os_error())
        }
        self.exclusive = false;
        drop(file);
        error.map_or(Ok(()), Err)
    }
}
impl Drop for Port {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn kernel_state_mask() -> libc::tcflag_t {
    #[cfg(target_os = "macos")]
    {
        libc::PENDIN
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}
fn settings_equal(a: &libc::termios, b: &libc::termios) -> bool {
    // Darwin may set PENDIN (input retyping state) during canonical restoration.
    a.c_iflag == b.c_iflag
        && a.c_oflag == b.c_oflag
        && a.c_cflag == b.c_cflag
        && (a.c_lflag & !kernel_state_mask()) == (b.c_lflag & !kernel_state_mask())
        && a.c_cc == b.c_cc
        && unsafe {
            libc::cfgetispeed(a) == libc::cfgetispeed(b)
                && libc::cfgetospeed(a) == libc::cfgetospeed(b)
        }
}
trait ProbeClock {
    fn now(&self) -> Instant;
    fn poll(
        &self,
        fd: RawFd,
        event: libc::c_short,
        duration: Duration,
    ) -> io::Result<libc::c_short>;
}
struct SystemProbeClock;
impl ProbeClock for SystemProbeClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn poll(
        &self,
        fd: RawFd,
        event: libc::c_short,
        duration: Duration,
    ) -> io::Result<libc::c_short> {
        let mut poll = libc::pollfd {
            fd,
            events: event,
            revents: 0,
        };
        let timeout = duration.as_millis().clamp(1, 100) as i32;
        if unsafe { libc::poll(&mut poll, 1, timeout) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(poll.revents)
    }
}

fn ready(
    fd: RawFd,
    event: libc::c_short,
    deadline: Instant,
    cancel: &AtomicBool,
    clock: &dyn ProbeClock,
) -> io::Result<()> {
    loop {
        cancelled(cancel)?;
        let now = clock.now();
        if now >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "MON-VER response timed out",
            ));
        }
        let events = match clock.poll(fd, event, deadline - now) {
            Ok(events) => events,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if events & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "GNSS transport closed",
            ));
        }
        if events & event != 0 {
            return Ok(());
        }
    }
}
fn probe(
    path: &Path,
    lease: &Lease,
    cancel: &AtomicBool,
    duration: Duration,
    clock: &dyn ProbeClock,
) -> io::Result<Version> {
    cancelled(cancel)?;
    if !lease.matches(path)? {
        return Err(invalid("GNSS endpoint changed after lease"));
    }
    let mut port = Port::open(path)?;
    let deadline = clock.now() + duration;
    let result = (|| {
        ready(port.fd(), libc::POLLOUT, deadline, cancel, clock)?;
        cancelled(cancel)?;
        // Exactly one informational write. A short write fails; it is never retried.
        if port.file.as_mut().expect("open").write(&POLL)? != POLL.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short MON-VER query write",
            ));
        }
        let mut parser = Parser::default();
        let mut buffer = [0; 4096];
        loop {
            ready(port.fd(), libc::POLLIN, deadline, cancel, clock)?;
            cancelled(cancel)?;
            match port.file.as_mut().expect("open").read(&mut buffer) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "GNSS transport closed",
                    ))
                }
                Ok(n) => {
                    if let Some(version) = parser.feed(&buffer[..n])? {
                        cancelled(cancel)?;
                        let current = fs::metadata(path)?;
                        let opened = port.file.as_ref().expect("open").metadata()?;
                        if !lease.matches(path)?
                            || (current.dev(), current.ino(), current.rdev())
                                != (opened.dev(), opened.ino(), opened.rdev())
                        {
                            return Err(invalid("GNSS endpoint changed during identification"));
                        }
                        return Ok(version);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) => return Err(e),
            }
        }
    })();
    let closed = port.close();
    closed?;
    result
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::{os::fd::FromRawFd, thread};
    fn payload(fields: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, text) in fields.iter().enumerate() {
            let n = if i == 1 { 10 } else { 30 };
            assert!(text.len() < n);
            out.extend(text.as_bytes());
            out.resize(out.len() + n - text.len(), 0);
        }
        out
    }
    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0xb5, 0x62, 0x0a, 0x04];
        out.extend((payload.len() as u16).to_le_bytes());
        out.extend(payload);
        out.extend(checksum(&out[2..]));
        out
    }
    fn version_frame() -> Vec<u8> {
        frame(&payload(&[
            "ROM CORE 3.01 (107888)",
            "00080000",
            "FWVER=SPG 3.01",
            "PROTVER=18.00",
            "GPS;GLO;GAL;BDS",
            "SBAS;IMES;QZSS",
        ]))
    }
    #[test]
    fn observed_version_layout_every_chunk_boundary_and_unknown_model() {
        assert_eq!(frame(&[]), POLL);
        let bytes = version_frame();
        assert_eq!(bytes.len(), 168);
        for chunk in 1..=bytes.len() {
            let mut parser = Parser::default();
            assert!(parser.feed(b"$TEST,not-location\r\n").unwrap().is_none());
            let mut found = None;
            for part in bytes.chunks(chunk) {
                found = parser.feed(part).unwrap().or(found)
            }
            let v = found.unwrap();
            assert_eq!(v.software, "ROM CORE 3.01 (107888)");
            assert_eq!(v.hardware, "00080000");
            assert_eq!(v.firmware.as_deref(), Some("SPG 3.01"));
            assert_eq!(v.protocol.as_deref(), Some("18.00"));
            assert_eq!(v.model, None);
        }
        let minimal = decode(&payload(&["ROM 1", "HW1"])).unwrap();
        assert_eq!(
            (minimal.firmware, minimal.protocol, minimal.model),
            (None, None, None)
        );
        assert_eq!(
            decode(&payload(&["ROM 1", "HW1", "MOD=EXPLICIT"]))
                .unwrap()
                .model
                .as_deref(),
            Some("EXPLICIT")
        );
    }
    #[test]
    fn parser_rejects_corruption_malformed_extensions_and_budgets() {
        let mut bad = version_frame();
        *bad.last_mut().unwrap() ^= 1;
        assert!(Parser::default().feed(&bad).is_err());
        for data in [
            vec![0xb5, 0x62, 0x0a, 0x04, 0xff, 0xff],
            frame(b"short"),
            frame(&[b'x'; 40]),
            frame(&payload(&["", "HW"])),
            frame(&payload(&["ROM", "HW", "MOD="])),
            frame(&payload(&["ROM", "HW", "PROTVER=1", "PROTVER=2"])),
            frame(&payload(&["ROM\x01", "HW"])),
        ] {
            assert!(Parser::default().feed(&data).is_err())
        }
        let mut parser = Parser::default();
        for _ in 0..16 {
            assert!(parser.feed(&[b'x'; 4096]).unwrap().is_none())
        }
        assert!(parser.feed(b"x").is_err());
        let bytes = version_frame();
        assert!(Parser::default()
            .feed(&bytes[..bytes.len() - 1])
            .unwrap()
            .is_none());
        let mut other = vec![0xb5, 0x62, 1, 1, 0, 0];
        other.extend(checksum(&other[2..]));
        assert!(Parser::default().feed(&other).unwrap().is_none());
    }
    struct Pty {
        master: File,
        slave: File,
        path: PathBuf,
    }
    impl Pty {
        fn new() -> Self {
            let (mut m, mut s) = (-1, -1);
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut m,
                        &mut s,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            for fd in [m, s] {
                assert_eq!(
                    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                    0
                )
            }
            let mut name = [0u8; 256];
            assert_eq!(
                unsafe { libc::ttyname_r(s, name.as_mut_ptr().cast(), name.len()) },
                0
            );
            let end = name.iter().position(|b| *b == 0).unwrap();
            Self {
                master: unsafe { File::from_raw_fd(m) },
                slave: unsafe { File::from_raw_fd(s) },
                path: PathBuf::from(std::str::from_utf8(&name[..end]).unwrap()),
            }
        }
        fn settings(&self) -> libc::termios {
            let mut t = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut t) },
                0
            );
            t
        }
    }
    fn fixture_ready(fd: RawFd, event: libc::c_short) -> libc::c_short {
        let mut poll = libc::pollfd {
            fd,
            events: event,
            revents: 0,
        };
        // Outer fixture hang guard only; never advances correctness time.
        assert!(
            unsafe { libc::poll(&mut poll, 1, 30_000) } > 0,
            "fixture readiness hang guard"
        );
        assert_eq!(
            poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL),
            0
        );
        poll.revents
    }
    type PollStep = Box<
        dyn FnMut(RawFd, libc::c_short, Duration, &std::sync::Mutex<Instant>) -> libc::c_short
            + Send,
    >;
    struct TestClock {
        time: std::sync::Mutex<Instant>,
        step: std::sync::Mutex<PollStep>,
    }
    impl TestClock {
        fn new(
            step: impl FnMut(RawFd, libc::c_short, Duration, &std::sync::Mutex<Instant>) -> libc::c_short
                + Send
                + 'static,
        ) -> Self {
            Self {
                time: std::sync::Mutex::new(Instant::now()),
                step: std::sync::Mutex::new(Box::new(step)),
            }
        }
        fn frozen() -> Self {
            Self::new(|fd, event, _, _| fixture_ready(fd, event))
        }
    }
    impl ProbeClock for TestClock {
        fn now(&self) -> Instant {
            *self.time.lock().unwrap()
        }
        fn poll(
            &self,
            fd: RawFd,
            event: libc::c_short,
            duration: Duration,
        ) -> io::Result<libc::c_short> {
            Ok(self.step.lock().unwrap()(fd, event, duration, &self.time))
        }
    }
    fn read_poll(master: &mut File) {
        fixture_ready(master.as_raw_fd(), libc::POLLIN);
        let mut bytes = [0; 8];
        master.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, POLL);
    }
    #[tokio::test]
    async fn private_pty_one_poll_lease_and_terminal_restore() {
        crate::test_support::manual_time(async {
            let pty = Pty::new();
            let before = pty.settings();
            let lease = serial_lease::acquire(&pty.path).unwrap();
            assert!(serial_lease::acquire(&pty.path).is_err());
            let mut master = pty.master.try_clone().unwrap();
            let peer = thread::spawn(move || {
                read_poll(&mut master);
                for part in version_frame().chunks(7) {
                    master.write_all(part).unwrap();
                }
            });
            let (v, lease) = run_probe_with_clock(
                pty.path.clone(),
                lease,
                Duration::from_secs(2),
                TestClock::frozen(),
                None,
            )
            .await
            .unwrap();
            peer.join().unwrap();
            assert_eq!(v.firmware.as_deref(), Some("SPG 3.01"));
            assert!(settings_equal(&before, &pty.settings()));
            assert!(serial_lease::acquire(&pty.path).is_err());
            drop(lease);
            assert!(serial_lease::acquire(&pty.path).is_ok());
            let _reopened = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
                .open(&pty.path)
                .unwrap();
        })
        .await;
    }
    #[tokio::test]
    async fn timeout_and_incomplete_reply_restore_before_lease_release() {
        crate::test_support::manual_time(async {
            for reply in [vec![], version_frame()[..20].to_vec()] {
                let pty = Pty::new();
                let before = pty.settings();
                let mut master = pty.master.try_clone().unwrap();
                let has_reply = !reply.is_empty();
                let (sent, received) = std::sync::mpsc::sync_channel(1);
                let peer = thread::spawn(move || {
                    read_poll(&mut master);
                    master.write_all(&reply).unwrap();
                    sent.send(()).unwrap();
                });
                let mut first = true;
                let clock = TestClock::new(move |fd, event, remaining, time| {
                    if event == libc::POLLOUT {
                        return fixture_ready(fd, event);
                    }
                    if first {
                        received
                            .recv_timeout(Duration::from_secs(30))
                            .expect("peer reply hang guard");
                        first = false;
                        if has_reply {
                            return fixture_ready(fd, event);
                        }
                    }
                    // The peer completed its chosen reply and the real parser has
                    // requested more input. Expire this exact remaining budget.
                    *time.lock().unwrap() += remaining;
                    0
                });
                let lease = serial_lease::acquire(&pty.path).unwrap();
                let error = run_probe_with_clock(
                    pty.path.clone(),
                    lease,
                    Duration::from_millis(150),
                    clock,
                    None,
                )
                .await
                .unwrap_err();
                peer.join().unwrap();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert!(settings_equal(&before, &pty.settings()));
                assert!(serial_lease::acquire(&pty.path).is_ok());
            }
        })
        .await;
    }

    #[tokio::test]
    async fn cancellation_keeps_lease_until_worker_restores_and_closes() {
        crate::test_support::manual_time(async {
            let pty = Pty::new();
            let before = pty.settings();
            let lease = serial_lease::acquire(&pty.path).unwrap();
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::sync_channel(1);
            let (finished, done) = tokio::sync::oneshot::channel();
            let mut entered = Some(entered);
            let clock = TestClock::new(move |fd, event, _, _| {
                if event == libc::POLLOUT {
                    return fixture_ready(fd, event);
                }
                entered
                    .take()
                    .expect("only one blocked readiness call")
                    .send(())
                    .unwrap();
                wait.recv_timeout(Duration::from_secs(30))
                    .expect("fixture release hang guard");
                0
            });
            let task = tokio::spawn(run_probe_with_clock(
                pty.path.clone(),
                lease,
                Duration::from_secs(10),
                clock,
                Some(finished),
            ));
            ready.await.unwrap();
            let mut master = pty.master.try_clone().unwrap();
            tokio::task::spawn_blocking(move || read_poll(&mut master))
                .await
                .unwrap();
            assert!(serial_lease::acquire(&pty.path).is_err());
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            // Worker still owns its open port until this explicit poll release.
            assert!(serial_lease::acquire(&pty.path).is_err());
            release.send(()).unwrap();
            done.await.expect("cancelled worker completion");
            assert!(settings_equal(&before, &pty.settings()));
            let lease = serial_lease::acquire(&pty.path).unwrap();
            let _port = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
                .open(&pty.path)
                .unwrap();
            drop(lease);
        })
        .await;
    }

    #[test]
    fn post_query_binding_change_discards_the_observed_reply() {
        use crate::discovery::{assemble, Endpoint, Options, RawDevice};
        let raw = RawDevice {
            id: "fixture-gps".into(),
            name: "GPS".into(),
            vid: Some(0x1546),
            pid: Some(0x01a8),
            endpoints: vec![Endpoint {
                kind: "serial".into(),
                path: "/dev/fixture-gps".into(),
            }],
        };
        let inventory = assemble(vec![raw], vec![], "macos", &Options::default());
        let request:Request=serde_json::from_value(serde_json::json!({"protocol":"ublox","device_id":"fixture-gps","port":"/dev/fixture-gps"})).unwrap();
        let before = binding(&inventory, &request).unwrap();
        if !super::super::supports_ublox_identification() {
            assert_eq!(
                validate_completion_binding(&inventory, &request, &before)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::Unsupported
            );
            return;
        }
        validate_completion_binding(&inventory, &request, &before).unwrap();
        let mut removed = inventory.clone();
        removed.devices.clear();
        assert!(validate_completion_binding(&removed, &request, &before).is_err());
        let mut replaced = inventory.clone();
        replaced.devices[0].usb.as_mut().unwrap().pid = "ffff".into();
        assert!(validate_completion_binding(&replaced, &request, &before).is_err());
        let mut changed = inventory.clone();
        changed.devices[0].endpoints.push(Endpoint {
            kind: "serial".into(),
            path: "/dev/new-role".into(),
        });
        assert!(validate_completion_binding(&changed, &request, &before).is_err());
    }

    #[test]
    fn cancelled_or_changed_endpoint_never_sends_poll() {
        let pty = Pty::new();
        let other = Pty::new();
        let lease = serial_lease::acquire(&pty.path).unwrap();
        assert_eq!(
            probe(
                &pty.path,
                &lease,
                &AtomicBool::new(true),
                DEADLINE,
                &TestClock::frozen()
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Interrupted
        );
        assert!(probe(
            &other.path,
            &lease,
            &AtomicBool::new(false),
            DEADLINE,
            &TestClock::frozen()
        )
        .is_err());
        assert!(serial_lease::acquire(&pty.path).is_err());
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn existing_external_pty_owner_is_refused() {
        crate::test_support::manual_time(async {
            let pty = Pty::new();
            // This integration test checks lsof's actual ownership result, not its
            // speed under concurrent coverage instrumentation. Production still
            // bounds the lsof result wait to one second and refuses on timeout.
            let error = check_owner_with_timeout(&pty.path, Duration::from_secs(10))
                .await
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        })
        .await;
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn ownership_timeout_refuses_and_reaps_the_owned_process() {
        crate::test_support::manual_time(async {
            use std::process::Stdio;
            let child = tokio::process::Command::new("/bin/sh")
                .args(["-c", "read value"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let pid = child.id().unwrap() as libc::pid_t;
            let (witness, mut events) = crate::test_support::TimerWitness::channel();
            let task = tokio::spawn(ownership_result_inner(
                child,
                Duration::from_millis(50),
                Some(witness),
            ));
            crate::test_support::advance_registered_timer(&mut events).await;
            let error = task.await.unwrap().unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            // Already reaped, rather than merely signalled or left as a zombie.
            let mut status = 0;
            assert_eq!(
                unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
                -1
            );
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        })
        .await;
    }
    #[test]
    fn readiness_deadline_is_checked_before_each_poll_at_exact_virtual_boundary() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let clock = TestClock::new(move |_, _, remaining, time| {
            let call = observed.fetch_add(1, Ordering::SeqCst);
            let step = if call == 0 {
                remaining - Duration::from_nanos(1)
            } else {
                assert_eq!(call, 1, "must not poll again at expired deadline");
                assert_eq!(remaining, Duration::from_nanos(1));
                remaining
            };
            *time.lock().unwrap() += step;
            0
        });
        let deadline = clock.now() + DEADLINE;
        let error = ready(-1, libc::POLLIN, deadline, &AtomicBool::new(false), &clock).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(clock.now(), deadline);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
