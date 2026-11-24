//! A helper supervisor survives async cancellation and holds the serial lease
//! until its child is reaped. It never blocks an async runtime worker.
use airhound_hostd::serial_lease::Lease;
use std::{
    io,
    process::{Command, Stdio},
    sync::{mpsc, Arc},
    time::Duration,
};
use tokio::{
    process::{ChildStderr, ChildStdout},
    sync::oneshot,
};

pub struct Child {
    stop: Option<mpsc::Sender<()>>,
    done: oneshot::Receiver<io::Result<()>>,
    startup: Option<
        oneshot::Receiver<io::Result<(std::process::ChildStdout, std::process::ChildStderr)>>,
    >,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}
impl Child {
    pub fn start(mut command: Command, lease: Arc<Lease>, inherit: bool) -> io::Result<Self> {
        use std::os::unix::process::CommandExt;
        let fd = lease.raw_fd();
        if inherit {
            // Runs only in the forked child. The parent's descriptor stays CLOEXEC.
            unsafe {
                command.pre_exec(move || {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (stop, requests) = mpsc::channel();
        let (started, startup) = oneshot::channel();
        let (finished, done) = oneshot::channel();
        std::thread::Builder::new()
            .name("sniffle-child".into())
            .spawn(move || {
                // Thread construction fails before any process can be spawned.
                let lease = lease;
                if !matches!(requests.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                    // No child exists. Publish failure only after relinquishing
                    // the supervisor's ownership so ready() is a cleanup barrier.
                    drop(lease);
                    let _ = started.send(Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "helper start cancelled",
                    )));
                    let _ = finished.send(Ok(()));
                    return;
                }
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        drop(lease);
                        let _ = started.send(Err(error));
                        let _ = finished.send(Ok(()));
                        return;
                    }
                };
                let stdout = child.stdout.take().expect("piped stdout");
                let stderr = child.stderr.take().expect("piped stderr");
                let mut killing = started.send(Ok((stdout, stderr))).is_err();
                if killing {
                    let _ = child.kill();
                }
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            // Reaping precedes lease release, which precedes the
                            // completion notification observed by finish().
                            drop(lease);
                            let _ = finished.send(Ok(()));
                            return;
                        }
                        Err(error) => {
                            // Unknown reap state must never release ownership to another
                            // source while this process may still hold the transport.
                            std::mem::forget(lease);
                            let _ = finished.send(Err(error));
                            return;
                        }
                        Ok(None) => (),
                    }
                    if !killing && !matches!(requests.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        let _ = child.kill();
                        killing = true;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })?;
        Ok(Self {
            stop: Some(stop),
            done,
            startup: Some(startup),
            stdout: None,
            stderr: None,
        })
    }
    pub async fn ready(&mut self) -> io::Result<()> {
        let startup = self
            .startup
            .take()
            .ok_or_else(|| io::Error::other("helper startup already consumed"))?;
        let (stdout, stderr) = startup
            .await
            .map_err(|_| io::Error::other("helper supervisor stopped"))??;
        self.stdout = Some(ChildStdout::from_std(stdout)?);
        self.stderr = Some(ChildStderr::from_std(stderr)?);
        Ok(())
    }
    #[cfg(test)]
    pub async fn spawn(command: Command, lease: Arc<Lease>, inherit: bool) -> io::Result<Self> {
        let mut child = Self::start(command, lease, inherit)?;
        child.ready().await?;
        Ok(child)
    }
    /// Observe the production supervisor's reap-and-release barrier even when
    /// the caller is dropped. The replacement receiver is never awaited.
    #[cfg(test)]
    fn take_completion(&mut self) -> oneshot::Receiver<io::Result<()>> {
        let (_, replacement) = oneshot::channel();
        std::mem::replace(&mut self.done, replacement)
    }
    pub async fn finish(&mut self) -> io::Result<()> {
        self.stop.take();
        tokio::time::timeout(Duration::from_secs(5), &mut self.done)
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "helper cleanup pending; serial lease retained",
                )
            })?
            .map_err(|_| io::Error::other("helper supervisor stopped before cleanup"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    struct Endpoint {
        _master: std::fs::File,
        _slave: std::fs::File,
        path: String,
    }
    impl Endpoint {
        fn new() -> Self {
            use std::os::fd::{AsRawFd, FromRawFd};
            let (mut master, mut slave) = (0, 0);
            let mut name = [0i8; 1024];
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        name.as_mut_ptr(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            let master = unsafe { std::fs::File::from_raw_fd(master) };
            let slave = unsafe { std::fs::File::from_raw_fd(slave) };
            for file in [&master, &slave] {
                assert_eq!(
                    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
                    0
                );
            }
            Self {
                _master: master,
                _slave: slave,
                path: unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                    .to_str()
                    .unwrap()
                    .to_owned(),
            }
        }
        fn path(&self) -> &str {
            &self.path
        }
    }
    fn assert_reaped(pid: i32) {
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "child must be reaped");
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
    #[tokio::test]
    async fn cancelled_caller_keeps_ownership_until_child_exit_and_reap() {
        crate::test_support::manual_time(async {
            // This fake device is private to this test, including parallel GPS tests.
            // Helpers inherit only its lease; no physical serial device is opened.
            let endpoint = Endpoint::new();
            let lease = Arc::new(airhound_hostd::serial_lease::acquire(endpoint.path()).unwrap());
            let mut command = Command::new("python3");
            command.args([
                "-c",
                "import os,signal;print(os.getpid(),flush=True);signal.pause()",
            ]);
            let mut child = Child::spawn(command, lease.clone(), true).await.unwrap();
            let mut line = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .await
                .unwrap();
            let pid: i32 = line.trim().parse().unwrap();
            assert!(airhound_hostd::serial_lease::acquire(endpoint.path()).is_err());
            drop(lease);
            let completion = child.take_completion();
            drop(child); // Simulates a caller future being cancelled without Finish.
            completion.await.unwrap().unwrap();
            let guard = airhound_hostd::serial_lease::acquire(endpoint.path()).unwrap();
            assert_reaped(pid);
            drop(guard);
            let lease = Arc::new(airhound_hostd::serial_lease::acquire(endpoint.path()).unwrap());
            let command = Command::new("/nonexistent/airhound-helper");
            assert!(Child::spawn(command, lease.clone(), true).await.is_err());
            drop(lease);
            assert!(airhound_hostd::serial_lease::acquire(endpoint.path()).is_ok());
        })
        .await;
    }
    #[tokio::test]
    async fn startup_failure_and_finish_release_supervisor_ownership_before_notification() {
        crate::test_support::manual_time(async {
            // This fake device is private to this test, including parallel GPS tests.
            // Helpers inherit only its lease; no physical serial device is opened.
            let endpoint = Endpoint::new();
            // Neither completion path may require polling for a later thread-scope
            // destructor. Repeat to exercise notification scheduling on CI workers.
            for _ in 0..32 {
                let lease =
                    Arc::new(airhound_hostd::serial_lease::acquire(endpoint.path()).unwrap());
                let mut child =
                    Child::start(Command::new("/nonexistent/airhound-helper"), lease, true)
                        .unwrap();
                assert!(child.ready().await.is_err());
                let guard = airhound_hostd::serial_lease::acquire(endpoint.path())
                    .expect("failed startup must release ownership before ready returns");
                child.finish().await.unwrap();
                drop(guard);

                let lease =
                    Arc::new(airhound_hostd::serial_lease::acquire(endpoint.path()).unwrap());
                let mut command = Command::new("python3");
                command.args([
                    "-c",
                    "import os,signal;print(os.getpid(),flush=True);signal.pause()",
                ]);
                let mut child = Child::spawn(command, lease, true).await.unwrap();
                let mut line = String::new();
                BufReader::new(child.stdout.take().unwrap())
                    .read_line(&mut line)
                    .await
                    .unwrap();
                let pid: i32 = line.trim().parse().unwrap();
                child.finish().await.unwrap();
                let _guard = airhound_hostd::serial_lease::acquire(endpoint.path())
                    .expect("Finish must release ownership before returning");
                assert_reaped(pid);
            }
        })
        .await;
    }
    #[tokio::test]
    async fn cancellation_during_pending_exec_reaps_before_releasing_lease() {
        crate::test_support::manual_time(async {
            use std::os::{
                fd::AsRawFd,
                unix::{net::UnixStream, process::CommandExt},
            };
            // This fake device is private to this test, including parallel GPS tests.
            // Helpers inherit only its lease; no physical serial device is opened.
            let endpoint = Endpoint::new();
            let (parent, gate) = UnixStream::pair().unwrap();
            parent.set_nonblocking(true).unwrap();
            let mut parent = tokio::net::UnixStream::from_std(parent).unwrap();
            let gate_fd = gate.as_raw_fd();
            let lease = Arc::new(airhound_hostd::serial_lease::acquire(endpoint.path()).unwrap());
            let mut command = Command::new("python3");
            command.args(["-c", "import signal;signal.pause()"]);
            // The startup handshake cannot complete until this controlled pre-exec
            // gate opens. Only async-signal-safe libc calls run in the forked child.
            unsafe {
                command.pre_exec(move || {
                    let pid = libc::getpid().to_ne_bytes();
                    if libc::write(gate_fd, pid.as_ptr().cast(), pid.len()) != pid.len() as isize {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    let mut release = [0u8];
                    if libc::read(gate_fd, release.as_mut_ptr().cast(), 1) != 1 {
                        return Err(io::Error::from_raw_os_error(libc::ECANCELED));
                    }
                    Ok(())
                });
            }
            let mut child = Child::start(command, lease.clone(), true).unwrap();
            let mut bytes = [0u8; 4];
            parent.read_exact(&mut bytes).await.unwrap();
            let pid = i32::from_ne_bytes(bytes);
            let completion = child.take_completion();
            drop(child); // Cancel while Command::spawn is still waiting for exec.
            drop(lease);
            assert!(airhound_hostd::serial_lease::acquire(endpoint.path()).is_err());
            parent.write_all(b"x").await.unwrap();
            completion.await.unwrap().unwrap();
            let _guard = airhound_hostd::serial_lease::acquire(endpoint.path()).unwrap();
            assert_reaped(pid);
        })
        .await;
    }
}
