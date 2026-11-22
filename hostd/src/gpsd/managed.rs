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
async fn owns_listener(pid: u32, port: u16) -> io::Result<bool> {
    let output = tokio::time::timeout(
        Duration::from_secs(1),
        Command::new("/usr/sbin/lsof")
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
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "gpsd listener ownership check timed out",
        )
    })??;
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
#[cfg(target_os = "linux")]
async fn owns_listener(pid: u32, port: u16) -> io::Result<bool> {
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

async fn ready(child: &mut Child, port: u16) -> io::Result<()> {
    for _ in 0..30 {
        if child.try_wait()?.is_some() {
            return Err(io::Error::other("managed gpsd exited during startup"));
        }
        if owns_listener(
            child
                .id()
                .ok_or_else(|| io::Error::other("gpsd child exited"))?,
            port,
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

async fn owned_session(options: &Options, source: &Source, pid: u32, port: u16) -> io::Result<()> {
    let socket = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&options.endpoint),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "managed gpsd connection timeout"))??;
    // Recheck after connecting and before any WATCH or data ingestion. Never
    // delegate retries to the external-server client: ownership can change.
    if !owns_listener(pid, port).await? {
        return Err(io::Error::other("managed gpsd no longer owns its listener"));
    }
    super::consume_socket(socket, source).await
}

pub(super) async fn run(
    options: Options,
    source: Source,
    mut state: watch::Receiver<crate::control::State>,
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
            result = ready(&mut child, endpoint.port()) => result,
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
            result = owned_session(&options, &source, pid, endpoint.port()) => {
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
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(exit: bool) -> Self {
            let root = std::env::temp_dir().join(format!(
                "ah-gpsd-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            let script = format!(
                r#"#!/usr/bin/python3
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
    async fn until(source: &Source, expected: &str, fixture: &Fixture) {
        tokio::time::timeout(Duration::from_secs(12), async {
            while source.health()["state"] != expected {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let files: Vec<_> = ["args", "pids", "stderr", "trace"]
                .into_iter()
                .map(|name| (name, std::fs::read_to_string(fixture.0.join(name))))
                .collect();
            panic!(
                "expected {expected}, got {}; fixture evidence: {files:?}",
                source.health()
            );
        });
    }
    // One test serializes /dev/null leases; no real serial device is opened.
    #[tokio::test]
    async fn owned_helper_lifecycle_refuses_conflicts_and_clears_fixes() {
        let _ = env_logger::Builder::new()
            .filter_level(log::LevelFilter::Warn)
            .is_test(true)
            .try_init();
        let fixture = Fixture::new(false);
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
        let listen_port = port();
        let task = tokio::spawn(run(
            fixture.options(listen_port, "/dev/null"),
            source.clone(),
            control.subscribe(),
        ));
        until(&source, "fix", &fixture).await;
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

        let fixture = Fixture::new(false);
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
        let listen_port = port();
        let source = Source::new(Some("/dev/null".into()));
        let control = crate::control::ScanControl::new(true);
        let task = tokio::spawn(run(
            fixture.options(listen_port, "/dev/null"),
            source.clone(),
            control.subscribe(),
        ));
        until(&source, "fix", &fixture).await;
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

        let fixture = Fixture::new(true);
        let source = Source::new(Some("/dev/null".into()));
        let control = crate::control::ScanControl::new(true);
        let task = tokio::spawn(run(
            fixture.options(port(), "/dev/null"),
            source.clone(),
            control.subscribe(),
        ));
        until(&source, "fix", &fixture).await;
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
    }
}
