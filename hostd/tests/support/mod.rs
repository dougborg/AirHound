//! Test-only OS event witnesses. Wall time is only an outer fixture hang guard.
#![allow(dead_code)]
#[cfg(target_os = "macos")]
use std::fs::File;
use std::{
    collections::BTreeMap,
    io::{self, Read},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output},
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
use std::{ffi::CString, os::unix::ffi::OsStrExt};
const OUTER_GUARD: Duration = Duration::from_secs(120);

fn poll_event(fd: i32, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fixture outer hang guard",
            ));
        }
        let millis = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        let mut event = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: event is a valid single pollfd, and the caller owns fd.
        let result = unsafe { libc::poll(&mut event, 1, millis) };
        if result > 0 {
            if event.revents & (libc::POLLNVAL | libc::POLLERR) != 0 {
                return Err(io::Error::other("fixture event descriptor failed"));
            }
            return Ok(());
        }
        if result == 0 {
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

struct Watched {
    identity: (u64, u64),
    #[cfg(target_os = "macos")]
    file: File,
    #[cfg(target_os = "linux")]
    descriptor: i32,
}
/// Watches exact paths plus their parents. Re-registers replaced files before
/// checking the predicate, including when a parent event precedes old-file removal.
pub struct FileEvents {
    fd: OwnedFd,
    paths: Vec<PathBuf>,
    watched: BTreeMap<PathBuf, Watched>,
}
impl FileEvents {
    pub fn new(paths: &[&Path]) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        // SAFETY: kqueue has no pointer arguments.
        let raw = unsafe { libc::kqueue() };
        #[cfg(target_os = "linux")]
        // SAFETY: inotify_init1 has no pointer arguments.
        let raw = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful creation returned a new owned descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut all = Vec::new();
        for path in paths {
            for parent in path.ancestors().skip(1) {
                all.push(parent.to_owned());
                if parent.exists() {
                    break;
                }
            }
            all.push((*path).to_owned());
        }
        all.sort();
        all.dedup();
        let mut this = Self {
            fd,
            paths: all,
            watched: BTreeMap::new(),
        };
        this.refresh()?;
        Ok(this)
    }
    fn refresh(&mut self) -> io::Result<()> {
        for path in self.paths.clone() {
            let metadata = match std::fs::metadata(&path) {
                Ok(value) => value,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    self.remove(&path);
                    continue;
                }
                Err(e) => return Err(e),
            };
            let identity = (metadata.dev(), metadata.ino());
            if self
                .watched
                .get(&path)
                .is_some_and(|old| old.identity == identity)
            {
                continue;
            }
            self.remove(&path);
            #[cfg(target_os = "macos")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let file = match std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_EVTONLY | libc::O_CLOEXEC)
                    .open(&path)
                {
                    Ok(file) => file,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                let actual = file.metadata()?;
                let event = libc::kevent {
                    ident: file.as_raw_fd() as _,
                    filter: libc::EVFILT_VNODE,
                    flags: libc::EV_ADD | libc::EV_CLEAR,
                    fflags: libc::NOTE_WRITE
                        | libc::NOTE_EXTEND
                        | libc::NOTE_ATTRIB
                        | libc::NOTE_DELETE
                        | libc::NOTE_RENAME,
                    data: 0,
                    udata: std::ptr::null_mut(),
                };
                // SAFETY: event and owned descriptor remain valid for this call.
                if unsafe {
                    libc::kevent(
                        self.fd.as_raw_fd(),
                        &event,
                        1,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null(),
                    )
                } < 0
                {
                    return Err(io::Error::last_os_error());
                }
                self.watched.insert(
                    path.clone(),
                    Watched {
                        identity: (actual.dev(), actual.ino()),
                        file,
                    },
                );
            }
            #[cfg(target_os = "linux")]
            {
                let name = CString::new(path.as_os_str().as_bytes())?;
                // SAFETY: name is terminated and fd is a live inotify descriptor.
                let descriptor = unsafe {
                    libc::inotify_add_watch(
                        self.fd.as_raw_fd(),
                        name.as_ptr(),
                        libc::IN_MODIFY
                            | libc::IN_ATTRIB
                            | libc::IN_CREATE
                            | libc::IN_DELETE
                            | libc::IN_MOVED_FROM
                            | libc::IN_MOVED_TO
                            | libc::IN_DELETE_SELF
                            | libc::IN_MOVE_SELF,
                    )
                };
                if descriptor < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::NotFound {
                        continue;
                    }
                    return Err(error);
                }
                self.watched.insert(
                    path.clone(),
                    Watched {
                        identity,
                        descriptor,
                    },
                );
            }
        }
        Ok(())
    }
    fn remove(&mut self, path: &Path) {
        if let Some(old) = self.watched.remove(path) {
            #[cfg(target_os = "linux")]
            // SAFETY: descriptor belongs to this inotify instance; already-removed
            // watches may return EINVAL and need no further cleanup.
            unsafe {
                libc::inotify_rm_watch(self.fd.as_raw_fd(), old.descriptor);
            }
            #[cfg(target_os = "macos")]
            drop(old.file); // Closing unregisters the old vnode automatically.
        }
    }
    pub fn until<T>(
        &mut self,
        mut condition: impl FnMut() -> io::Result<Option<T>>,
    ) -> io::Result<T> {
        let deadline = Instant::now() + OUTER_GUARD;
        loop {
            self.refresh()?;
            if let Some(value) = condition()? {
                return Ok(value);
            }
            poll_event(self.fd.as_raw_fd(), deadline)?;
            #[cfg(target_os = "macos")]
            {
                // SAFETY: initialized event output and zero timeout are valid.
                let mut event: libc::kevent = unsafe { std::mem::zeroed() };
                let zero = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                let count = unsafe {
                    libc::kevent(
                        self.fd.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        &zero,
                    )
                };
                if count < 0 {
                    return Err(io::Error::last_os_error());
                }
                if count > 0 && event.flags & libc::EV_ERROR != 0 {
                    return Err(io::Error::other("vnode event failed"));
                }
            }
            #[cfg(target_os = "linux")]
            {
                let mut bytes = [0u8; 8192];
                // SAFETY: bytes is a live writable buffer; fd is owned.
                let count = unsafe {
                    libc::read(self.fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len())
                };
                if count < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::WouldBlock {
                        return Err(error);
                    }
                } else {
                    let mut offset = 0;
                    let size = std::mem::size_of::<libc::inotify_event>();
                    while offset < count as usize {
                        if count as usize - offset < size {
                            return Err(io::Error::other("truncated inotify event"));
                        }
                        // SAFETY: the complete fixed header is inside bytes;
                        // read_unaligned does not assume byte-buffer alignment.
                        let event = unsafe {
                            std::ptr::read_unaligned(
                                bytes.as_ptr().add(offset).cast::<libc::inotify_event>(),
                            )
                        };
                        let end = offset + size + event.len as usize;
                        if end > count as usize || event.mask & libc::IN_Q_OVERFLOW != 0 {
                            return Err(io::Error::other("incomplete inotify witness"));
                        }
                        if event.mask & libc::IN_IGNORED != 0 {
                            // An automatically removed watch must be rearmed even
                            // if the filesystem subsequently reuses its inode.
                            self.watched
                                .retain(|_, watched| watched.descriptor != event.wd);
                        }
                        offset = end;
                    }
                }
            }
        }
    }
}

/// Wait for the actual owned child exit notification, then reap. No poll sleeps.
/// Linux requires pidfd support; unsupported kernels fail explicitly.
pub fn wait_child(child: &mut Child) -> io::Result<ExitStatus> {
    wait_child_until(child, Instant::now() + OUTER_GUARD)
}
pub fn wait_child_until(child: &mut Child, deadline: Instant) -> io::Result<ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    #[cfg(target_os = "macos")]
    // SAFETY: no pointer arguments.
    let raw = unsafe { libc::kqueue() };
    #[cfg(target_os = "linux")]
    // SAFETY: the PID is this unreaped owned child; flags are zero.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id(), 0) as i32 };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw was freshly created above.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    #[cfg(target_os = "macos")]
    {
        let event = libc::kevent {
            ident: child.id() as _,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: event is valid and child's PID remains owned until reaped.
        if unsafe {
            libc::kevent(
                fd.as_raw_fd(),
                &event,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            return Err(io::Error::last_os_error());
        }
    }
    poll_event(fd.as_raw_fd(), deadline)?;
    child.wait()
}

pub struct OwnedChild(pub Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
/// Drain output while the child runs; any wait failure still kills/reaps it and
/// joins readers before returning. Fixtures do not create pipe-owning descendants.
pub fn output(command: &mut Command) -> io::Result<Output> {
    use std::process::Stdio;
    let mut child = OwnedChild(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let read = |mut pipe: Box<dyn Read + Send>| -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        pipe.read_to_end(&mut out)?;
        Ok(out)
    };
    let out = std::thread::spawn(move || read(Box::new(stdout)));
    let err = std::thread::spawn(move || read(Box::new(stderr)));
    let status = wait_child(&mut child.0);
    if status.is_err() {
        let _ = child.0.kill();
        let _ = child.0.wait();
    }
    // Join both before propagating either error; no reader is detached when
    // its sibling fails to read or panics.
    let stdout = out.join();
    let stderr = err.join();
    let stdout = stdout.map_err(|_| io::Error::other("stdout reader panicked"))??;
    let stderr = stderr.map_err(|_| io::Error::other("stderr reader panicked"))??;
    Ok(Output {
        status: status?,
        stdout,
        stderr,
    })
}

/// Actual executable readiness comes from the operational post-listen diagnostic,
/// not from an earlier socket directory entry. Diagnostics stay bounded and drained.
pub struct ServiceChild {
    pub child: Child,
    log: Option<std::thread::JoinHandle<io::Result<Vec<u8>>>>,
}
impl ServiceChild {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        use std::io::BufRead;
        use std::process::Stdio;
        let mut child = command
            .env("RUST_LOG", "airhound_hostd=debug")
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child.stderr.take().unwrap();
        let (send, received) = std::sync::mpsc::channel();
        let reader = std::thread::Builder::new().spawn(move || {
            let mut reader = io::BufReader::new(stderr);
            let mut all = Vec::new();
            let mut announced = false;
            let result = (|| {
                loop {
                    let mut line = Vec::new();
                    let count = reader.by_ref().take(8193).read_until(b'\n', &mut line)?;
                    if count == 0 {
                        break;
                    }
                    if count > 8192 || all.len() + count > 256 * 1024 {
                        return Err(io::Error::other(
                            "service fixture diagnostic bound exceeded",
                        ));
                    }
                    if !line.ends_with(b"\n") {
                        return Err(io::Error::other("unterminated service diagnostic line"));
                    }
                    if !announced
                        && String::from_utf8_lossy(&line)
                            .trim_end()
                            .ends_with("AirHound control socket listening")
                    {
                        announced = true;
                        let _ = send.send(Ok(()));
                    }
                    all.extend_from_slice(&line);
                }
                if !announced {
                    return Err(io::Error::other(
                        "service exited before listening diagnostic",
                    ));
                }
                Ok(all)
            })();
            if !announced {
                let _ = send.send(Err(result.as_ref().err().unwrap().to_string()));
            }
            result
        });
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let owned = Self {
            child,
            log: Some(reader),
        };
        received
            .recv_timeout(OUTER_GUARD)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("service readiness outer guard: {error}"),
                )
            })?
            .map_err(io::Error::other)?;
        Ok(owned)
    }
    pub fn diagnostics(&mut self) -> io::Result<Vec<u8>> {
        self.log
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::other("service log reader panicked"))?
    }
}
impl Drop for ServiceChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.log.take() {
            let _ = reader.join();
        }
    }
}
