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
                            let _ = finished.send(Ok(()));
                            break;
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
                drop(lease);
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
    use tokio::io::{AsyncBufReadExt, BufReader};
    #[tokio::test]
    async fn cancelled_caller_keeps_ownership_until_child_exit_and_reap() {
        let lease = Arc::new(airhound_hostd::serial_lease::acquire("/dev/null").unwrap());
        let mut command = Command::new("python3");
        command.args([
            "-c",
            "import os,time;print(os.getpid(),flush=True);time.sleep(30)",
        ]);
        let mut child = Child::spawn(command, lease.clone(), true).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(2),
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let pid: i32 = line.trim().parse().unwrap();
        assert!(airhound_hostd::serial_lease::acquire("/dev/null").is_err());
        drop(lease);
        drop(child); // Simulates a caller future being cancelled without Finish.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(guard) = airhound_hostd::serial_lease::acquire("/dev/null") {
                    assert_eq!(
                        unsafe { libc::kill(pid, 0) },
                        -1,
                        "ownership released before child exited"
                    );
                    drop(guard);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let lease = Arc::new(airhound_hostd::serial_lease::acquire("/dev/null").unwrap());
        let command = Command::new("/nonexistent/airhound-helper");
        assert!(Child::spawn(command, lease.clone(), true).await.is_err());
        drop(lease);
        assert!(airhound_hostd::serial_lease::acquire("/dev/null").is_ok());
    }
}
