//! A session-owned, foreground, read-only gpsd. Never adopts another daemon.
use super::{Options, Source};
use airhound_hostd::serial_lease::acquire as lease;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::watch;

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
    let output = ownership_output(command.output(), probe_timeout).await?;
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
    output: impl std::future::Future<Output = io::Result<std::process::Output>>,
    probe_timeout: Duration,
) -> io::Result<std::process::Output> {
    tokio::time::timeout(probe_timeout, output)
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "gpsd listener ownership check timed out",
            )
        })?
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

async fn ready(child: &mut Child, port: u16, probe_timeout: Duration) -> io::Result<()> {
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
            result = ready(&mut child, endpoint.port(), probe_timeout) => result,
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
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => (),
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
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        async fn new(exit: bool) -> Self {
            let root = std::env::temp_dir().join(format!(
                "ah-gpsd-test-{}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            // Resolve and initialize the test interpreter before measuring the
            // native daemon's production readiness budget. On macOS /usr/bin/python3
            // is an Xcode launcher, not necessarily the interpreter used by CI.
            let began = std::time::Instant::now();
            let output = tokio::time::timeout(
                Duration::from_secs(10),
                Command::new("python3")
                    .args([
                        "-I",
                        "-c",
                        "import sys,socket,json,datetime,pathlib; print(json.dumps({'executable':sys.executable,'version':sys.version}))",
                    ])
                    .stdin(Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .expect("fixture Python initialization exceeded 10 seconds")
            .expect("fixture requires a working python3 on PATH");
            assert!(output.status.success(), "fixture Python failed: {output:?}");
            let interpreter: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let executable = interpreter["executable"].as_str().unwrap();
            assert!(Path::new(executable).is_absolute());
            assert!(
                !executable.chars().any(char::is_whitespace),
                "fixture interpreter path must fit an unambiguous shebang"
            );
            eprintln!(
                "managed gpsd fixture interpreter {interpreter}; initialization {:?}",
                began.elapsed()
            );
            std::fs::write(root.join("interpreter"), output.stdout.as_slice()).unwrap();
            let script = format!(
                r#"#!{executable} -I
import sys,time,os
root_path=os.path.dirname(__file__)
sys.stderr=open(os.path.join(root_path,'stderr'),'a',buffering=1)
def trace(stage):
    with open(os.path.join(root_path,'trace'),'a') as f:
        f.write(str(time.monotonic())+' '+stage+' pid='+str(os.getpid())+' executable='+sys.executable+'\n')
trace('python_entered')
import socket,json,datetime
from pathlib import Path
root=Path(__file__).parent
trace('imports_complete')
with (root/'args').open('a') as f: f.write(json.dumps(sys.argv[1:])+'\n')
with (root/'pids').open('a') as f: f.write(str(os.getpid())+'\n')
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
trace('before_bind')
s.bind(('127.0.0.1',int(sys.argv[sys.argv.index('-S')+1]))); s.listen()
trace('listening')
c,_=s.accept(); c.recv(4096)
c.sendall((json.dumps({{'class':'VERSION','proto_major':16,'release':'fixture'}})+'\n').encode())
c.sendall((json.dumps({{'class':'TPV','device':sys.argv[-1],'mode':3,'time':datetime.datetime.now(datetime.timezone.utc).isoformat(),'lat':10,'lon':20}})+'\n').encode())
time.sleep({})
"#,
                if exit { "0.3" } else { "30" }
            );
            std::fs::write(root.join("gpsd"), script).unwrap();
            std::fs::set_permissions(root.join("gpsd"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            Self(root)
        }
        fn options(&self, port: u16, device: &str) -> Options {
            Options {
                endpoint: format!("127.0.0.1:{port}"),
                device: Some(device.into()),
                managed_executable: Some(self.0.join("gpsd")),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if std::thread::panicking() {
                for name in ["args", "pids", "stderr"] {
                    if let Ok(text) = std::fs::read_to_string(self.0.join(name)) {
                        eprintln!("managed gpsd fixture {name}: {text}");
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
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
            let source = Source::new(Some("/dev/null".into()));
            let control = crate::control::ScanControl::new(true);
            let options = fixture.options(port, "/dev/null");
            let owned_source = source.clone();
            let state = control.subscribe();
            let mut task = tokio::spawn(async move {
                drop(reservation);
                // A real competing listener makes the race deterministic in
                // the fixture regression; ordinary lifecycle calls pass zero.
                let conflict = (attempt <= injected_conflicts).then(|| {
                    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).unwrap()
                });
                run_with_probe_timeout(options, owned_source, state, Duration::from_secs(10)).await;
                drop(conflict);
            });
            let outcome = tokio::time::timeout(Duration::from_secs(12), async {
                loop {
                    if task.is_finished() {
                        return false;
                    }
                    if source.health()["state"] == "fix" {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
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
                    .all(|name| !fixture.0.join(name).exists());
            control.shutdown();
            let joined = tokio::time::timeout(Duration::from_secs(4), &mut task).await;
            if joined.is_err() {
                task.abort();
                let _ = task.await;
                return Err(format!("attempt {attempt}: fixture task failed to stop"));
            }
            if !matches!(joined, Ok(Ok(()))) || !pre_spawn_busy || attempt == 3 {
                return Err(format!(
                    "attempt {attempt}: expected fix, got {}; pre-spawn busy={pre_spawn_busy}",
                    source.health()
                ));
            }
        }
        unreachable!()
    }

    async fn until(source: &Source, expected: &str, fixture: &Fixture) {
        tokio::time::timeout(Duration::from_secs(12), async {
            while source.health()["state"] != expected {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let files: Vec<_> = ["interpreter", "args", "pids", "stderr", "trace"]
                .into_iter()
                .map(|name| (name, std::fs::read_to_string(fixture.0.join(name))))
                .collect();
            panic!(
                "expected {expected}, got {}; fixture evidence: {files:?}",
                source.health()
            );
        });
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn ownership_probe_timeout_refuses_and_reaps_its_child() {
        // Spawn first so interpreter/scheduler startup cannot be confused with
        // the deliberately short probe-output deadline. wait_with_output uses
        // the same Child ownership/drop path as Command::output in production.
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap() as libc::pid_t;
        let began = std::time::Instant::now();
        let error = ownership_output(child.wait_with_output(), Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // Tokio's kill-on-drop path reaps asynchronously. Do not reap the child
        // from this test: ESRCH establishes that the runtime already did so.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if unsafe { libc::kill(pid, 0) } == -1 {
                    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed-out probe child must be killed and reaped");
        assert!(began.elapsed() < Duration::from_secs(6));
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    // One test serializes /dev/null leases; no real serial device is opened.
    #[tokio::test]
    async fn owned_helper_lifecycle_refuses_conflicts_and_clears_fixes() {
        let _ = env_logger::Builder::new()
            .filter_level(log::LevelFilter::Warn)
            .is_test(true)
            .try_init();
        let fixture = Fixture::new(false).await;
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let control = crate::control::ScanControl::new(true);
        let source = Source::new(Some("/dev/null".into()));
        run(
            fixture.options(occupied.local_addr().unwrap().port(), "/dev/null"),
            source.clone(),
            control.subscribe(),
        )
        .await;
        assert_eq!(source.health()["state"], "managed_endpoint_busy");
        assert!(!fixture.0.join("args").exists());
        assert!(std::net::TcpStream::connect(occupied.local_addr().unwrap()).is_ok());
        let held = lease("/dev/null").unwrap();
        run(
            fixture.options(port(), "/dev/null"),
            source.clone(),
            control.subscribe(),
        )
        .await;
        assert_eq!(source.health()["state"], "managed_device_busy");
        assert!(!fixture.0.join("args").exists());
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
            std::fs::read_to_string(fixture.0.join("args"))
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
        tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap();
        assert!(source.location().is_none());
        assert_eq!(source.health()["state"], "stopped");
        assert!(
            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, listen_port)).is_err()
        );
        assert!(std::net::TcpStream::connect(occupied.local_addr().unwrap()).is_ok());

        let fixture = Fixture::new(false).await;
        // Keep the established stream alive until the replacement listener is
        // bound. Closing it first lets the supervisor reap this child before it
        // writes its marker, or restart before the test can claim the endpoint.
        let takeover = r#"s.close()
(root/'listener_released').write_text('yes')
deadline=time.monotonic()+5
while not (root/'release_stream').exists():
    if time.monotonic() >= deadline: raise TimeoutError('test did not bind replacement')
    time.sleep(0.01)
c.close()
time.sleep(30)"#;
        let script = std::fs::read_to_string(fixture.0.join("gpsd"))
            .unwrap()
            .replace("time.sleep(30)", takeover);
        std::fs::write(fixture.0.join("gpsd"), script).unwrap();
        let Started {
            port: listen_port,
            source,
            control: _takeover_control,
            task,
            ..
        } = start_fixture(&fixture, 0).await.unwrap();
        assert_eq!(source.health()["state"], "fix");
        tokio::time::timeout(Duration::from_secs(3), async {
            while !fixture.0.join("listener_released").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let replacement =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, listen_port))
                .await
                .unwrap();
        std::fs::write(fixture.0.join("release_stream"), "yes").unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.health()["state"], "managed_endpoint_busy");
        assert!(source.location().is_none());
        assert!(source.health()["daemon"].is_null());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), replacement.accept())
                .await
                .is_err(),
            "replacement daemon must receive no connection or WATCH"
        );
        drop(replacement);

        let fixture = Fixture::new(true).await;
        let Started {
            source,
            control: _control,
            task,
            ..
        } = start_fixture(&fixture, 0).await.unwrap();
        assert_eq!(source.health()["state"], "fix");
        until(&source, "managed_child_exited", &fixture).await;
        assert!(source.location().is_none());
        assert!(source.health()["daemon"].is_null());
        tokio::time::timeout(Duration::from_secs(12), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.health()["state"], "managed_restart_limit_reached");
        assert_eq!(
            std::fs::read_to_string(fixture.0.join("args"))
                .unwrap()
                .lines()
                .count(),
            3
        );
        for pid in std::fs::read_to_string(fixture.0.join("pids"))
            .unwrap()
            .lines()
        {
            assert_eq!(unsafe { libc::kill(pid.parse::<i32>().unwrap(), 0) }, -1);
        }

        // A competing listener before spawn is the only retryable precondition.
        let fixture = Fixture::new(false).await;
        let started = start_fixture(&fixture, 1).await.unwrap();
        assert_eq!(started.attempts, 2);
        assert_eq!(started.source.health()["state"], "fix");
        assert_eq!(
            std::fs::read_to_string(fixture.0.join("pids"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        started.control.shutdown();
        tokio::time::timeout(Duration::from_secs(4), started.task)
            .await
            .unwrap()
            .unwrap();
        // Real helper evidence from the preceding run prevents a blind retry,
        // even when a subsequent attempt returns endpoint busy before spawning.
        let error = start_fixture(&fixture, 1)
            .await
            .err()
            .expect("started helper must not retry");
        assert!(error.contains("attempt 1:") && error.contains("pre-spawn busy=false"));
        let fixture = Fixture::new(false).await;
        let error = start_fixture(&fixture, 3)
            .await
            .err()
            .expect("three conflicts must fail");
        assert!(error.contains("attempt 3:") && error.contains("pre-spawn busy=true"));
        assert!(!fixture.0.join("args").exists());
        let fixture = Fixture::new(false).await;
        let script = std::fs::read_to_string(fixture.0.join("gpsd"))
            .unwrap()
            .replace(
                "trace('before_bind')",
                "raise SystemExit(2)\ntrace('before_bind')",
            );
        std::fs::write(fixture.0.join("gpsd"), script).unwrap();
        let error = start_fixture(&fixture, 0)
            .await
            .err()
            .expect("helper startup failure must not retry");
        assert!(error.contains("attempt 1:") && error.contains("managed_start_failed"));
        assert_eq!(
            std::fs::read_to_string(fixture.0.join("pids"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }
}
