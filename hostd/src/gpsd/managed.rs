//! A session-owned, foreground, read-only gpsd. Never adopts another daemon.
use super::{Options, Source};
use airhound_hostd::serial_lease::acquire as lease;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::watch;
#[cfg(test)]
#[derive(Clone)]
pub(super) struct TestHooks {
    listener: std::sync::Arc<tokio::net::UnixListener>,
    controls: tokio::sync::mpsc::Sender<tokio::io::BufReader<tokio::net::UnixStream>>,
}
#[cfg(all(test, target_os = "macos"))]
struct ProbeWitness {
    consumed: tokio::sync::oneshot::Sender<()>,
    timer: crate::test_support::TimerWitness,
    reaped: crate::owned_child::Reaped,
}

fn command(executable: &Path, device: &str, port: u16) -> Command {
    let mut command = Command::new(executable);
    // No shell, config file, global listener, hotplug control socket, daemonizing,
    // or receiver configuration. gpsd handles serial framing/exclusive open.
    command
        .args(["-N", "-b", "-n", "-S", &port.to_string(), device])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

// Verify this exact child owns the IPv4 listener before sending WATCH or
// consuming data. A bind probe alone has a race and can adopt a different gpsd.
#[cfg(target_os = "macos")]
async fn owns_listener(pid: u32, port: u16, probe_timeout: Duration) -> io::Result<bool> {
    let mut command = Command::new("/usr/sbin/lsof");
    command
        .args([
            "-nP",
            "-a",
            "-p",
            &pid.to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = ownership_output(
        command,
        probe_timeout,
        #[cfg(test)]
        None,
    )
    .await?;
    let owns = output.stdout.len() <= 16_384
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == format!("n127.0.0.1:{port}"));
    #[cfg(test)]
    if !owns {
        eprintln!(
            "gpsd listener probe pid={pid} port={port} status={} stdout={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(16_384)])
        );
    }
    Ok(owns)
}
// Keep the same owned-child timeout/drop path for production and fixture probes.
#[cfg(target_os = "macos")]
async fn ownership_output(
    mut command: Command,
    probe_timeout: Duration,
    #[cfg(test)] witness: Option<ProbeWitness>,
) -> io::Result<std::process::Output> {
    use tokio::io::AsyncReadExt;
    command.stdout(Stdio::piped()).kill_on_drop(true);
    #[cfg(test)]
    let (consumed, timer, reaped) = match witness {
        Some(witness) => (
            Some(witness.consumed),
            Some(witness.timer),
            Some(witness.reaped),
        ),
        None => (None, None, None),
    };
    let mut child = crate::owned_child::OwnedChild::new(
        command.spawn()?,
        #[cfg(test)]
        reaped,
    )?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("ownership probe stdout absent"))?;
    #[cfg(not(test))]
    let mut stdout = stdout;
    #[cfg(test)]
    let mut stdout = crate::test_support::ReadWitness {
        inner: stdout,
        consumed,
    };
    let deadline = tokio::time::Instant::now() + probe_timeout;
    let waiting = tokio::time::timeout_at(deadline, async {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).await?;
        let status = child.wait().await?;
        Ok::<_, io::Error>(std::process::Output {
            status,
            stdout: output,
            stderr: Vec::new(),
        })
    });
    #[cfg(test)]
    let waiting = async {
        if let Some(timer) = timer {
            timer.observe(deadline, waiting, Result::is_err).await
        } else {
            waiting.await
        }
    };
    match waiting.await {
        Ok(result) => result,
        Err(_) => {
            child.finish().await?;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "gpsd listener ownership check timed out",
            ))
        }
    }
}
#[cfg(target_os = "linux")]
async fn owns_listener(pid: u32, port: u16, _probe_timeout: Duration) -> io::Result<bool> {
    let mut sockets = std::collections::HashSet::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
        let target = std::fs::read_link(entry?.path())?;
        if let Some(inode) = target
            .to_str()
            .and_then(|s| s.strip_prefix("socket:["))
            .and_then(|s| s.strip_suffix(']'))
        {
            sockets.insert(inode.to_string());
        }
    }
    let address = format!("0100007F:{port:04X}");
    Ok(std::fs::read_to_string(format!("/proc/{pid}/net/tcp"))?
        .lines()
        .skip(1)
        .any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.len() > 9
                && fields[1] == address
                && fields[3] == "0A"
                && sockets.contains(fields[9])
        }))
}

async fn stop(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    if let Some(pid) = child.id() {
        // Child has not been reaped; its PID cannot have been reused.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

async fn ready(
    child: &mut Child,
    port: u16,
    probe_timeout: Duration,
    #[cfg(test)] source: &Source,
) -> io::Result<()> {
    #[cfg(test)]
    {
        use tokio::io::AsyncBufReadExt;
        let hooks = source.0.lock().unwrap().managed.clone();
        if let Some(hooks) = hooks {
            let pid = child
                .id()
                .ok_or_else(|| io::Error::other("fixture child exited"))?;
            let readiness = async {
                let (socket, _) = hooks.listener.accept().await?;
                let mut reader = tokio::io::BufReader::new(socket);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await? == 0 {
                        return Err(io::Error::other("fixture exited before listener readiness"));
                    }
                    let event: serde_json::Value = serde_json::from_str(&line)?;
                    if event["pid"].as_u64() != Some(u64::from(pid)) {
                        return Err(io::Error::other(
                            "fixture readiness belongs to another child",
                        ));
                    }
                    if event["stage"] == "listening" {
                        break;
                    }
                }
                hooks
                    .controls
                    .send(reader)
                    .await
                    .map_err(|_| io::Error::other("fixture control owner closed"))
            };
            tokio::select! {
                result = readiness => result?,
                status = child.wait() => {
                    status?;
                    return Err(io::Error::other("fixture exited before listener readiness"));
                }
            }
        }
    }
    for _ in 0..30 {
        if child.try_wait()?.is_some() {
            return Err(io::Error::other("managed gpsd exited during startup"));
        }
        if owns_listener(
            child
                .id()
                .ok_or_else(|| io::Error::other("gpsd child exited"))?,
            port,
            probe_timeout,
        )
        .await?
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "managed gpsd did not establish its own listener",
    ))
}

async fn owned_session(
    options: &Options,
    source: &Source,
    pid: u32,
    port: u16,
    probe_timeout: Duration,
) -> io::Result<()> {
    let socket = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&options.endpoint),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "managed gpsd connection timeout"))??;
    // Recheck after connecting and before any WATCH or data ingestion. Never
    // delegate retries to the external-server client: ownership can change.
    if !owns_listener(pid, port, probe_timeout).await? {
        return Err(io::Error::other("managed gpsd no longer owns its listener"));
    }
    super::consume_socket(socket, source).await
}

pub(super) async fn run(
    options: Options,
    source: Source,
    state: watch::Receiver<crate::control::State>,
) {
    run_with_probe_timeout(options, source, state, Duration::from_secs(1)).await;
}

async fn run_with_probe_timeout(
    options: Options,
    source: Source,
    mut state: watch::Receiver<crate::control::State>,
    probe_timeout: Duration,
) {
    let config = airhound_hostd::session::LocationConfig::ManagedGpsd {
        endpoint: options.endpoint.clone(),
        device: options.device.clone().unwrap_or_default(),
        executable: options.managed_executable.clone().unwrap_or_default(),
    };
    if let Err(error) = config.validate() {
        log::warn!("managed gpsd validation: {error}");
        source.disconnected("managed_configuration_invalid");
        return;
    }
    let device = options.device.as_deref().expect("validated device");
    let _lease = match lease(device) {
        Ok(lease) => lease,
        Err(error) => {
            log::warn!("managed gpsd lease: {error}");
            source.disconnected("managed_device_busy");
            return;
        }
    };
    let endpoint: std::net::SocketAddr = options.endpoint.parse().expect("validated endpoint");
    for attempt in 0..3 {
        if state.borrow().shutdown {
            source.disconnected("stopped");
            return;
        }
        source.disconnected(if attempt == 0 {
            "managed_starting"
        } else {
            "managed_restarting"
        });
        // Fail explicitly if anything is already listening. Never attach to or
        // terminate it. Ownership is checked again after our child starts.
        let reservation = match std::net::TcpListener::bind(endpoint) {
            Ok(listener) => listener,
            Err(error) => {
                log::warn!("managed gpsd endpoint: {error}");
                source.disconnected("managed_endpoint_busy");
                return;
            }
        };
        drop(reservation);
        let mut child = match command(
            options.managed_executable.as_ref().unwrap(),
            device,
            endpoint.port(),
        )
        .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                log::warn!("managed gpsd spawn: {error}");
                source.disconnected("managed_spawn_failed");
                return;
            }
        };
        let started = tokio::select! {
            result = ready(&mut child, endpoint.port(), probe_timeout, #[cfg(test)] &source) => result,
            _ = async { loop { if state.changed().await.is_err() || state.borrow().shutdown { break; } } } => {
                stop(&mut child).await; source.disconnected("stopped"); return;
            }
        };
        if let Err(error) = started {
            log::warn!("managed gpsd startup: {error}");
            stop(&mut child).await;
            source.disconnected("managed_start_failed");
            return;
        }
        let pid = child.id().expect("ready child has not been reaped");
        tokio::select! {
            biased;
            _ = child.wait() => source.disconnected("managed_child_exited"),
            result = owned_session(&options, &source, pid, endpoint.port(), probe_timeout) => {
                if let Err(error) = result { log::warn!("managed gpsd connection: {error}"); }
                source.disconnected("managed_connection_lost");
            }
            _ = async { loop { if state.changed().await.is_err() || state.borrow().shutdown { break; } } } => {
                stop(&mut child).await; source.disconnected("stopped"); return;
            }
        }
        // Reap/stop before releasing the serial lease or starting another child.
        stop(&mut child).await;
        source.disconnected("managed_child_exited");
        let retry = tokio::time::sleep(Duration::from_secs(2));
        #[cfg(test)]
        let retry = async {
            let witness = source.0.lock().unwrap().retry.clone();
            if let Some(witness) = witness {
                witness.observe(retry.deadline(), retry, |_| true).await;
            } else {
                retry.await;
            }
        };
        tokio::select! {
            _ = retry => (),
            _ = async { loop { if state.changed().await.is_err() || state.borrow().shutdown { break; } } } => {
                source.disconnected("stopped"); return;
            }
        }
    }
    source.disconnected("managed_restart_limit_reached");
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Fixture {
        root: std::path::PathBuf,
        hooks: TestHooks,
        controls: tokio::sync::Mutex<
            tokio::sync::mpsc::Receiver<tokio::io::BufReader<tokio::net::UnixStream>>,
        >,
        retry: crate::test_support::TimerWitness,
        retry_events: tokio::sync::Mutex<
            tokio::sync::mpsc::UnboundedReceiver<crate::test_support::TimerEvent>,
        >,
    }
    impl Fixture {
        async fn new() -> Self {
            let root = std::path::PathBuf::from("/tmp").join(format!(
                "ah-gpsd-test-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ));
            std::fs::create_dir(&root).unwrap();
            let listener =
                std::sync::Arc::new(tokio::net::UnixListener::bind(root.join("events")).unwrap());
            let (controls, received) = tokio::sync::mpsc::channel(8);
            let (retry, retry_events) = crate::test_support::TimerWitness::channel();
            let output = Command::new("python3").args([
                "-I", "-c", "import sys,json; print(json.dumps({'executable':sys.executable,'version':sys.version}))",
            ]).stdin(Stdio::null()).kill_on_drop(true).output().await.unwrap();
            assert!(output.status.success());
            let interpreter: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let executable = interpreter["executable"].as_str().unwrap();
            assert!(Path::new(executable).is_absolute());
            assert!(!executable.chars().any(char::is_whitespace));
            std::fs::write(root.join("interpreter"), &output.stdout).unwrap();
            let body = r#"import sys,os,socket,json,signal
from pathlib import Path
from datetime import datetime,timedelta,timezone
root=Path(__file__).parent
sys.stderr=open(root/'stderr','a',buffering=1)
events=socket.socket(socket.AF_UNIX);events.connect(str(root/'events'))
def trace(stage):
 with (root/'trace').open('a') as f:f.write(stage+' pid='+str(os.getpid())+'\n')
 events.sendall((json.dumps({'pid':os.getpid(),'stage':stage})+'\n').encode())
trace('python_entered')
with (root/'args').open('a') as f:f.write(json.dumps(sys.argv[1:])+'\n')
with (root/'pids').open('a') as f:f.write(str(os.getpid())+'\n')
generation=len((root/'pids').read_text().splitlines())-1
fix_time=(datetime(2026,1,1,tzinfo=timezone.utc)+timedelta(seconds=generation)).isoformat()
s=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
trace('before_bind')
s.bind(('127.0.0.1',int(sys.argv[sys.argv.index('-S')+1])));s.listen()
trace('listening')
c,_=s.accept()
watch=c.makefile('rb').readline(4097)
assert watch==b'?WATCH={"enable":true,"json":true};\n',watch
c.sendall((json.dumps({'class':'VERSION','proto_major':16,'release':'fixture'})+'\n').encode())
c.sendall((json.dumps({'class':'TPV','device':sys.argv[-1],'mode':3,'time':fix_time,'lat':10,'lon':20})+'\n').encode())
commands=events.makefile('r')
command=commands.readline().strip()
if command=='exit':
 c.close();sys.exit(0)
assert command=='release_listener',command
s.close();trace('listener_released')
assert commands.readline().strip()=='release_stream'
c.close();signal.pause()
"#;
            std::fs::write(root.join("gpsd"), format!("#!{executable} -I\n{body}")).unwrap();
            std::fs::set_permissions(root.join("gpsd"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            Self {
                root,
                hooks: TestHooks { listener, controls },
                controls: tokio::sync::Mutex::new(received),
                retry,
                retry_events: tokio::sync::Mutex::new(retry_events),
            }
        }
        fn source(&self) -> Source {
            let source = super::super::tests::controlled_source(Some("/dev/null".into()));
            {
                let mut state = source.0.lock().unwrap();
                state.updates = Some(std::sync::Arc::new(tokio::sync::Notify::new()));
                state.managed = Some(self.hooks.clone());
                state.retry = Some(self.retry.clone());
            }
            source
        }
        async fn child_control(&self) -> tokio::io::BufReader<tokio::net::UnixStream> {
            self.controls.lock().await.recv().await.unwrap()
        }
        async fn advance_retry(&self) {
            crate::test_support::advance_registered_timer(&mut *self.retry_events.lock().await)
                .await;
        }
        fn options(&self, port: u16, device: &str) -> Options {
            Options {
                endpoint: format!("127.0.0.1:{port}"),
                device: Some(device.into()),
                managed_executable: Some(self.root.join("gpsd")),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if std::thread::panicking() {
                for name in ["args", "pids", "stderr"] {
                    if let Ok(text) = std::fs::read_to_string(self.root.join(name)) {
                        eprintln!("managed gpsd fixture {name}: {text}");
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    fn retain_fixture_conflict(
        reservation: std::net::TcpListener,
        inject_conflict: bool,
    ) -> Option<std::net::TcpListener> {
        if inject_conflict {
            Some(reservation)
        } else {
            drop(reservation);
            None
        }
    }

    #[test]
    fn injected_conflict_keeps_the_original_listener_owned() {
        use std::os::fd::AsRawFd;
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reservation.local_addr().unwrap();
        let descriptor = reservation.as_raw_fd();
        let conflict = retain_fixture_conflict(reservation, true).unwrap();
        assert_eq!(conflict.as_raw_fd(), descriptor);
        assert_eq!(conflict.local_addr().unwrap(), address);
        assert_eq!(
            std::net::TcpListener::bind(address).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
        assert!(retain_fixture_conflict(conflict, false).is_none());
    }

    struct Started {
        port: u16,
        source: Source,
        control: crate::control::ScanControl,
        task: tokio::task::JoinHandle<()>,
        attempts: usize,
    }

    // Keep the reservation through scheduling. No port reservation can span
    // gpsd's own bind; retry only proven pre-spawn fixture conflicts, never a
    // started helper or production ownership/health assertion.
    async fn start_fixture(
        fixture: &Fixture,
        injected_conflicts: usize,
    ) -> Result<Started, String> {
        for attempt in 1..=3 {
            let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = reservation.local_addr().unwrap().port();
            let source = fixture.source();
            let control = crate::control::ScanControl::new(true);
            let options = fixture.options(port, "/dev/null");
            let owned_source = source.clone();
            let state = control.subscribe();
            let mut task = tokio::spawn(async move {
                // Retain the actual reservation for injected conflicts. Closing
                // and rebinding it would create a second, unintended port race.
                let conflict = retain_fixture_conflict(reservation, attempt <= injected_conflicts);
                run_with_probe_timeout(options, owned_source, state, Duration::from_secs(10)).await;
                drop(conflict);
            });
            let updates = source.0.lock().unwrap().updates.clone().unwrap();
            let mut joined_early = false;
            let outcome: Result<bool, ()> = Ok(loop {
                if source.health()["state"] == "fix" {
                    break true;
                }
                tokio::select! {
                    _ = updates.notified() => {},
                    result = &mut task => { result.unwrap(); joined_early = true; break false; },
                }
            });
            if matches!(outcome, Ok(true)) {
                return Ok(Started {
                    port,
                    source,
                    control,
                    task,
                    attempts: attempt,
                });
            }
            let pre_spawn_busy = matches!(outcome, Ok(false))
                && source.health()["state"] == "managed_endpoint_busy"
                && ["args", "pids", "trace"]
                    .iter()
                    .all(|name| !fixture.root.join(name).exists());
            control.shutdown();
            let joined = if joined_early { Ok(()) } else { task.await };
            if joined.is_err() || !pre_spawn_busy || attempt == 3 {
                return Err(format!(
                    "attempt {attempt}: expected fix, got {}; pre-spawn busy={pre_spawn_busy}",
                    source.health()
                ));
            }
        }
        unreachable!()
    }

    async fn until(source: &Source, expected: &str, _fixture: &Fixture) {
        let updates = source.0.lock().unwrap().updates.clone().unwrap();
        while source.health()["state"] != expected {
            updates.notified().await;
        }
    }
    async fn send_control(
        reader: &mut tokio::io::BufReader<tokio::net::UnixStream>,
        command: &str,
    ) {
        use tokio::io::AsyncWriteExt;
        reader
            .get_mut()
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn control_event(
        reader: &mut tokio::io::BufReader<tokio::net::UnixStream>,
        expected: &str,
    ) {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["stage"], expected);
    }
    #[tokio::test]
    async fn readiness_observes_exit_before_fixture_connection() {
        crate::test_support::manual_time(async {
            let fixture = Fixture::new().await;
            let source = fixture.source();
            let mut child = Command::new("python3")
                .args(["-c", "raise SystemExit(2)"])
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let pid = child.id().unwrap();
            let error = ready(&mut child, port(), Duration::from_secs(1), &source)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("before listener readiness"));
            assert_eq!(child.wait().await.unwrap().code(), Some(2));
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        })
        .await;
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn ownership_probe_timeout_refuses_and_reaps_its_child() {
        crate::test_support::manual_time(async {
            let mut command = Command::new("python3");
            command
                .args([
                    "-c",
                    "import os,signal;print(os.getpid(),flush=True);signal.pause()",
                ])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let (consumed, seen) = tokio::sync::oneshot::channel();
            let (timer, mut timers) = crate::test_support::TimerWitness::channel();
            let (reaped, mut completions) = tokio::sync::mpsc::channel(1);
            let task = tokio::spawn(ownership_output(
                command,
                Duration::from_secs(1),
                Some(ProbeWitness {
                    consumed,
                    timer,
                    reaped,
                }),
            ));
            seen.await.unwrap();
            crate::test_support::advance_registered_timer(&mut timers).await;
            let error = task.await.unwrap().unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            let (pid, confirmed) = completions.recv().await.unwrap();
            assert!(confirmed);
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        })
        .await;
    }

    // One test serializes /dev/null leases; no real serial device is opened.
    #[tokio::test]
    async fn owned_helper_lifecycle_refuses_conflicts_and_clears_fixes() {
        crate::test_support::manual_time(async {
            let _ = env_logger::Builder::new()
                .filter_level(log::LevelFilter::Warn)
                .is_test(true)
                .try_init();
            let fixture = Fixture::new().await;
            let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let control = crate::control::ScanControl::new(true);
            let source = fixture.source();
            run(
                fixture.options(occupied.local_addr().unwrap().port(), "/dev/null"),
                source.clone(),
                control.subscribe(),
            )
            .await;
            assert_eq!(source.health()["state"], "managed_endpoint_busy");
            assert!(!fixture.root.join("args").exists());
            assert!(std::net::TcpStream::connect(occupied.local_addr().unwrap()).is_ok());
            let held = lease("/dev/null").unwrap();
            run(
                fixture.options(port(), "/dev/null"),
                source.clone(),
                control.subscribe(),
            )
            .await;
            assert_eq!(source.health()["state"], "managed_device_busy");
            assert!(!fixture.root.join("args").exists());
            drop(held);
            let Started {
                port: listen_port,
                source,
                control,
                task,
                ..
            } = start_fixture(&fixture, 0).await.unwrap();
            assert_eq!(source.health()["state"], "fix");
            let args: serde_json::Value = serde_json::from_str(
                std::fs::read_to_string(fixture.root.join("args"))
                    .unwrap()
                    .trim(),
            )
            .unwrap();
            assert_eq!(
                args,
                serde_json::json!(["-N", "-b", "-n", "-S", listen_port.to_string(), "/dev/null"])
            );
            assert_eq!(source.health()["daemon"]["version"], "fixture");
            assert!(source.health()["daemon"]["receiver_firmware_version"].is_null());
            control.shutdown();
            task.await.unwrap();
            assert!(source.location().is_none());
            assert_eq!(source.health()["state"], "stopped");
            assert!(
                std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, listen_port)).is_err()
            );
            assert!(std::net::TcpStream::connect(occupied.local_addr().unwrap()).is_ok());

            let fixture = Fixture::new().await;
            // The fixture releases its listener and stream only on explicit
            // commands; the replacement is bound before EOF is delivered.
            let Started {
                port: listen_port,
                source,
                control: _takeover_control,
                task,
                ..
            } = start_fixture(&fixture, 0).await.unwrap();
            assert_eq!(source.health()["state"], "fix");
            let mut child_control = fixture.child_control().await;
            send_control(&mut child_control, "release_listener").await;
            control_event(&mut child_control, "listener_released").await;
            let replacement =
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, listen_port))
                    .await
                    .unwrap();
            send_control(&mut child_control, "release_stream").await;
            until(&source, "managed_child_exited", &fixture).await;
            fixture.advance_retry().await;
            task.await.unwrap();
            assert_eq!(source.health()["state"], "managed_endpoint_busy");
            assert!(source.location().is_none());
            assert!(source.health()["daemon"].is_null());
            // The source task has completed: one actual accept poll proves no
            // queued connection, without waiting an arbitrary quiet interval.
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    replacement.poll_accept(cx).is_pending()
                ))
                .await,
                "replacement daemon must receive no connection or WATCH"
            );
            drop(replacement);

            let fixture = Fixture::new().await;
            let Started {
                source,
                control: _control,
                task,
                ..
            } = start_fixture(&fixture, 0).await.unwrap();
            assert_eq!(source.health()["state"], "fix");
            for attempt in 0..3 {
                let mut child_control = fixture.child_control().await;
                send_control(&mut child_control, "exit").await;
                until(&source, "managed_child_exited", &fixture).await;
                assert!(source.location().is_none());
                assert!(source.health()["daemon"].is_null());
                // The next child emits this explicit fixture generation time;
                // cached timestamps remain rejected by the unchanged tracker.
                let (wall, monotonic) = source.now();
                super::super::tests::set_clock(
                    &source,
                    wall + chrono::Duration::seconds(1),
                    monotonic + Duration::from_secs(1),
                );
                fixture.advance_retry().await;
                if attempt < 2 {
                    until(&source, "fix", &fixture).await;
                }
            }
            task.await.unwrap();
            assert_eq!(source.health()["state"], "managed_restart_limit_reached");
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("args"))
                    .unwrap()
                    .lines()
                    .count(),
                3
            );
            for pid in std::fs::read_to_string(fixture.root.join("pids"))
                .unwrap()
                .lines()
            {
                assert_eq!(unsafe { libc::kill(pid.parse::<i32>().unwrap(), 0) }, -1);
            }

            // A competing listener before spawn is the only retryable precondition.
            let fixture = Fixture::new().await;
            let started = start_fixture(&fixture, 1).await.unwrap();
            assert_eq!(started.attempts, 2);
            assert_eq!(started.source.health()["state"], "fix");
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("pids"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
            started.control.shutdown();
            started.task.await.unwrap();
            // Real helper evidence from the preceding run prevents a blind retry,
            // even when a subsequent attempt returns endpoint busy before spawning.
            let error = start_fixture(&fixture, 1)
                .await
                .err()
                .expect("started helper must not retry");
            assert!(error.contains("attempt 1:") && error.contains("pre-spawn busy=false"));
            let fixture = Fixture::new().await;
            let error = start_fixture(&fixture, 3)
                .await
                .err()
                .expect("three conflicts must fail");
            assert!(error.contains("attempt 3:") && error.contains("pre-spawn busy=true"));
            assert!(!fixture.root.join("args").exists());
            let fixture = Fixture::new().await;
            let script = std::fs::read_to_string(fixture.root.join("gpsd"))
                .unwrap()
                .replace(
                    "trace('before_bind')",
                    "raise SystemExit(2)\ntrace('before_bind')",
                );
            std::fs::write(fixture.root.join("gpsd"), script).unwrap();
            let error = start_fixture(&fixture, 0)
                .await
                .err()
                .expect("helper startup failure must not retry");
            assert!(error.contains("attempt 1:") && error.contains("managed_start_failed"));
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("pids"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
        })
        .await;
    }
}
