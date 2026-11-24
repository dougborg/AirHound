//! Selected-device ADB forwarding only. No shell, modem or server-control API.
//! A detached ownership task completes allocation and cleanup even if the
//! acquiring source future is cancelled. Normal shutdown awaits its receipt.
use airhound_hostd::session::RayhunterAdb;
use std::{io, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::oneshot,
    task::JoinHandle,
    time::{timeout, Instant},
};

const LIMIT: u64 = 16 * 1024;
const ACQUIRE: Duration = Duration::from_secs(8);
const CLEANUP: Duration = Duration::from_secs(6);

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

struct Reply {
    stdout: Vec<u8>,
    success: bool,
}
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    command: Option<CommandProbe>,
    completed: Option<tokio::sync::mpsc::Sender<bool>>,
}
#[cfg(test)]
struct CommandProbe {
    consumed: oneshot::Sender<()>,
    timer: crate::test_support::TimerWitness,
}
#[cfg(test)]
struct ReadWitness<R> {
    inner: R,
    consumed: Option<oneshot::Sender<()>>,
}
#[cfg(test)]
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ReadWitness<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(result, std::task::Poll::Ready(Ok(())))
            && buf.filled()[before..].contains(&b'\n')
        {
            if let Some(witness) = self.consumed.take() {
                let _ = witness.send(());
            }
        }
        result
    }
}
async fn command(config: &RayhunterAdb, args: &[&str], end: Instant) -> io::Result<Reply> {
    command_inner(
        config,
        args,
        end,
        #[cfg(test)]
        None,
    )
    .await
}
async fn command_inner(
    config: &RayhunterAdb,
    args: &[&str],
    end: Instant,
    #[cfg(test)] probe: Option<CommandProbe>,
) -> io::Result<Reply> {
    let remaining = end
        .checked_duration_since(Instant::now())
        .ok_or_else(|| invalid("ADB operation deadline"))?;
    let mut child = Command::new(&config.executable)
        .args(["-H", "127.0.0.1", "-P", "5037", "-s", &config.serial])
        .args(args)
        .env_remove("ADB_SERVER_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("ADB stdout absent"))?
        .take(LIMIT + 1);
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid("ADB stderr absent"))?
        .take(LIMIT + 1);
    #[cfg(not(test))]
    let mut stdout = stdout;
    #[cfg(test)]
    let (consumed, timer) = match probe {
        Some(probe) => (Some(probe.consumed), Some(probe.timer)),
        None => (None, None),
    };
    #[cfg(test)]
    let mut stdout = ReadWitness {
        inner: stdout,
        consumed,
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let deadline = Instant::now() + remaining.min(Duration::from_secs(2));
    let waiting = tokio::time::timeout_at(deadline, async {
        tokio::try_join!(
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            child.wait()
        )
    });
    #[cfg(test)]
    let waiting = async {
        if let Some(witness) = timer {
            witness.observe(deadline, waiting, Result::is_err).await
        } else {
            waiting.await
        }
    };
    let result = waiting.await;
    let success = match result {
        Ok(Ok((_, _, status))) => status.success(),
        _ => {
            let _ = child.start_kill();
            timeout(Duration::from_secs(1), child.wait())
                .await
                .map_err(|_| invalid("ADB command reap deadline"))??;
            false
        }
    };
    if out.len() as u64 > LIMIT || err.len() as u64 > LIMIT {
        return Err(invalid("ADB output exceeds bound"));
    }
    // Retain stdout even after failure/timeout: a complete allocation response
    // can establish ownership for cleanup. No response means unknown, not none.
    Ok(Reply {
        stdout: out,
        success,
    })
}

fn text(reply: Reply) -> io::Result<String> {
    if !reply.success {
        return Err(invalid("ADB command failed or timed out"));
    }
    String::from_utf8(reply.stdout).map_err(|_| invalid("ADB response is not UTF-8"))
}
fn allocated_port(bytes: &[u8]) -> io::Result<u16> {
    if !bytes.ends_with(b"\n") {
        return Err(invalid(
            "ADB allocation outcome unknown: incomplete response; no inferred cleanup",
        ));
    }
    let value = std::str::from_utf8(bytes)
        .map_err(|_| invalid("ADB allocation outcome unknown"))?
        .trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid(
            "ADB allocation outcome unknown: invalid response; no inferred cleanup",
        ));
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| invalid("ADB allocation outcome unknown: invalid port"))
}
fn binding_state(list: &str, serial: &str, port: u16) -> io::Result<bool> {
    let local = format!("tcp:{port}");
    let mut matched = false;
    for line in list.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3 {
            return Err(invalid("Unknown ADB forward inventory"));
        }
        if fields[1] == local {
            if matched || fields[0] != serial || fields[2] != "tcp:8082" {
                return Err(invalid(
                    "ADB forward replaced or ambiguous; refusing removal",
                ));
            }
            matched = true;
        }
    }
    Ok(matched)
}

struct Binding {
    config: RayhunterAdb,
    port: u16,
}
impl Binding {
    async fn present(&self, end: Instant) -> io::Result<bool> {
        let list = text(command(&self.config, &["forward", "--list"], end).await?)?;
        binding_state(&list, &self.config.serial, self.port)
    }
    async fn close(&self) -> io::Result<()> {
        let end = Instant::now() + CLEANUP;
        if !self.present(end).await? {
            return Ok(());
        } // already expired; do not remove a guessed tuple
        text(
            command(
                &self.config,
                &["forward", "--remove", &format!("tcp:{}", self.port)],
                end,
            )
            .await?,
        )?;
        if self.present(end).await? {
            return Err(invalid("Owned ADB forward remains after removal"));
        }
        Ok(())
    }
}

/// Dropping sends cancellation; close().await additionally requires successful
/// owned cleanup. Process crash/runtime destruction cannot promise remote cleanup.
#[derive(Debug)]
pub(crate) struct Forward {
    pub endpoint: String,
    pub provenance: serde_json::Value,
    close: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<io::Result<()>>>,
}
impl Forward {
    pub async fn open(config: RayhunterAdb, expected_hash: String) -> io::Result<Self> {
        Self::open_inner(
            config,
            expected_hash,
            #[cfg(test)]
            TestHooks::default(),
        )
        .await
    }
    #[cfg(test)]
    pub(crate) async fn open_observed(
        config: RayhunterAdb,
        expected_hash: String,
        completed: Option<tokio::sync::mpsc::Sender<bool>>,
    ) -> io::Result<Self> {
        Self::open_inner(
            config,
            expected_hash,
            TestHooks {
                completed,
                ..Default::default()
            },
        )
        .await
    }
    async fn open_inner(
        config: RayhunterAdb,
        expected_hash: String,
        #[cfg(test)] mut hooks: TestHooks,
    ) -> io::Result<Self> {
        config.validate()?;
        let (ready_tx, ready_rx) = oneshot::channel();
        let (close_tx, close_rx) = oneshot::channel();
        let worker = tokio::spawn(async move {
            let result = own(
                config,
                expected_hash,
                ready_tx,
                close_rx,
                #[cfg(test)]
                hooks.command.take(),
            )
            .await;
            #[cfg(test)]
            if let Some(completed) = hooks.completed {
                let _ = completed.try_send(result.is_ok());
            }
            if let Err(error) = &result {
                log::error!("Rayhunter ADB ownership: {error}");
            }
            result
        });
        let mut owner = Self {
            endpoint: String::new(),
            provenance: serde_json::Value::Null,
            close: Some(close_tx),
            worker: Some(worker),
        };
        match ready_rx.await {
            Ok(Ok((endpoint, provenance))) => {
                owner.endpoint = endpoint;
                owner.provenance = provenance;
                Ok(owner)
            }
            Ok(Err(message)) => {
                let _ = owner.close().await;
                Err(invalid(message))
            }
            Err(_) => {
                let _ = owner.close().await;
                Err(invalid("ADB owner terminated before readiness"))
            }
        }
    }
    pub async fn close(mut self) -> io::Result<()> {
        self.close.take();
        if let Some(worker) = self.worker.take() {
            worker
                .await
                .map_err(|_| invalid("ADB ownership task failed"))??;
        }
        Ok(())
    }
}
impl Drop for Forward {
    fn drop(&mut self) {
        self.close.take();
    }
}

type Ready = oneshot::Sender<Result<(String, serde_json::Value), String>>;
async fn own(
    config: RayhunterAdb,
    expected_hash: String,
    ready: Ready,
    close: oneshot::Receiver<()>,
    #[cfg(test)] probe: Option<CommandProbe>,
) -> io::Result<()> {
    let acquired = async {
        let hash = crate::rayhunter_source::decoder_hash(&config.executable).await?;
        if hash != expected_hash { return Err(invalid("ADB executable changed during recording")); }
        let lease = airhound_hostd::serial_lease::acquire_adb(&config.serial)?;
        let end = Instant::now() + ACQUIRE;
        if text(command(&config, &["get-state"], end).await?)?.trim() != "device" { return Err(invalid("Selected ADB device is not authorized and online")); }
        if text(command(&config, &["get-serialno"], end).await?)?.trim() != config.serial { return Err(invalid("Selected ADB serial changed")); }
        let path = text(command(&config, &["get-devpath"], end).await?)?;
        let path = path.trim();
        if !path.starts_with("usb:") || path.len() > 256 || path.chars().any(char::is_control) { return Err(invalid("Selected ADB transport is not explicit USB")); }
        let reply = command_inner(&config, &["forward", "tcp:0", "tcp:8082"], end, #[cfg(test)] probe).await
            .map_err(|error| invalid(format!("ADB allocation outcome unknown: {error}; no inferred cleanup")))?;
        let port = allocated_port(&reply.stdout)?;
        let binding = Binding { config: config.clone(), port };
        let verified = if reply.success { binding.present(end).await } else { Err(invalid("ADB allocation command failed after returned identity")) };
        if !matches!(verified, Ok(true)) {
            binding.close().await?;
            return Err(invalid("ADB allocation failed verification; owned cleanup completed"));
        }
        let provenance = serde_json::json!({"kind":"managed_adb_usb","serial":config.serial,"usb_path":path,"adb_sha256":hash,"local_port":port,"remote_port":8082,"ownership":"returned_tuple_and_cooperative_uid_lease","global_diag_exclusivity":"unknown"});
        Ok((binding, lease, provenance))
    }.await;
    match acquired {
        Ok((binding, _lease, provenance)) => {
            if ready
                .send(Ok((
                    format!("ws://127.0.0.1:{}/raw-diag", binding.port),
                    provenance,
                )))
                .is_ok()
            {
                let _ = close.await;
            }
            binding.close().await
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        config: RayhunterAdb,
        events: tokio::net::UnixListener,
    }
    impl Fixture {
        fn new(mode: &str) -> Self {
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // A macOS TMPDIR may exceed the Unix-domain socket path limit.
            let root = PathBuf::from("/tmp")
                .join(format!("airhound-adb-{}-{sequence}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            let events = tokio::net::UnixListener::bind(root.join("events")).unwrap();
            let executable = root.join("fake-adb");
            let serial = format!("fixture-{}-{sequence}", std::process::id());
            std::fs::write(root.join("mode"), mode).unwrap();
            std::fs::write(
                &executable,
                r#"#!/usr/bin/env python3
import sys,pathlib,json,os,signal,socket
p=pathlib.Path(__file__).parent
assert sys.argv[1:6]==['-H','127.0.0.1','-P','5037','-s']
serial=sys.argv[6];args=sys.argv[7:]
with (p/'calls').open('a') as f:f.write(json.dumps(args)+'\n')
mode=(p/'mode').read_text()
def notify(event):
 with socket.socket(socket.AF_UNIX) as peer:
  peer.connect(str(p/'events'));peer.sendall((event+'\n').encode())
if args==['get-state']:print('offline' if mode=='offline' else 'device')
elif args==['get-serialno']:print('wrong' if mode=='wrongserial' else serial)
elif args==['get-devpath']:print('network:host' if mode=='network' else 'usb:fixture')
elif args==['forward','tcp:0','tcp:8082']:
 (p/'forward').write_text(serial+' tcp:49123 tcp:8082\n')
 if mode=='delayed':
  gate=socket.socket(socket.AF_UNIX);gate.bind(str(p/'release'));gate.listen(1)
 (p/'allocated').write_text(str(os.getpid()))
 notify('allocated')
 if mode=='lost':sys.exit(0)
 if mode=='delayed':
  peer,_=gate.accept();assert peer.recv(1)==b'!';peer.close();gate.close()
 print('49123',flush=True)
 (p/'emitted').write_text('ready')
 notify('emitted')
 if mode=='hang':signal.pause()
elif args==['forward','--list']:
 if (p/'forward').exists():print((p/'forward').read_text(),end='')
elif args==['forward','--remove','tcp:49123']:
 (p/'forward').unlink()
 notify('removed')
else:raise AssertionError(args)
"#,
            )
            .unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                root,
                config: RayhunterAdb { executable, serial },
                events,
            }
        }
        fn hash(&self) -> String {
            use sha2::{Digest, Sha256};
            Sha256::digest(std::fs::read(&self.config.executable).unwrap())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }
        fn calls(&self) -> Vec<Vec<String>> {
            std::fs::read_to_string(self.root.join("calls"))
                .unwrap_or_default()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect()
        }
        async fn event(&self, expected: &str) {
            use tokio::io::{AsyncBufReadExt, BufReader};
            loop {
                let (stream, _) = self.events.accept().await.unwrap();
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line).await.unwrap();
                if line.trim() == expected {
                    return;
                }
            }
        }
        async fn wait_allocated(&self) {
            self.event("allocated").await;
        }
        async fn wait_removed(&self) {
            self.event("removed").await;
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn owns_exact_selected_tuple_and_releases_lease_for_second_start() {
        crate::test_support::manual_time(async {
            let f = Fixture::new("normal");
            let first = Forward::open(f.config.clone(), f.hash()).await.unwrap();
            assert_eq!(first.endpoint, "ws://127.0.0.1:49123/raw-diag");
            assert_eq!(first.provenance["serial"], f.config.serial);
            assert!(airhound_hostd::serial_lease::acquire_adb(&f.config.serial).is_err());
            first.close().await.unwrap();
            assert!(!f.root.join("forward").exists());
            Forward::open(f.config.clone(), f.hash())
                .await
                .unwrap()
                .close()
                .await
                .unwrap();
            assert_eq!(
                f.calls()
                    .iter()
                    .filter(|a| a.as_slice() == ["forward", "--remove", "tcp:49123"])
                    .count(),
                2
            );
        })
        .await;
    }
    #[tokio::test]
    async fn cancelled_acquisition_waits_for_response_then_removes_owned_forward() {
        crate::test_support::manual_time(async {
            let f = Fixture::new("delayed");
            let config = f.config.clone();
            let hash = f.hash();
            let (completed_tx, mut completed_rx) = tokio::sync::mpsc::channel(1);
            let task = tokio::spawn(async move {
                Forward::open_observed(config, hash, Some(completed_tx)).await
            });
            f.wait_allocated().await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            // The ownership task remains blocked until this explicit response gate.
            use std::io::Write;
            std::os::unix::net::UnixStream::connect(f.root.join("release"))
                .unwrap()
                .write_all(b"!")
                .unwrap();
            f.wait_removed().await;
            assert_eq!(completed_rx.recv().await, Some(true));
            let lease = airhound_hostd::serial_lease::acquire_adb(&f.config.serial).unwrap();
            drop(lease);
            assert!(f
                .calls()
                .iter()
                .any(|a| a.as_slice() == ["forward", "--remove", "tcp:49123"]));
        })
        .await;
    }
    #[tokio::test]
    async fn lost_response_is_unknown_and_never_guessed_from_inventory() {
        crate::test_support::manual_time(async {
            let f = Fixture::new("lost");
            let error = match Forward::open(f.config.clone(), f.hash()).await {
                Ok(_) => panic!("unexpected ready"),
                Err(e) => e,
            };
            assert!(error.to_string().contains("outcome unknown"));
            assert!(f.root.join("forward").exists());
            assert!(!f.calls().iter().any(|a| a.iter().any(|s| s == "--remove")));
        })
        .await;
    }
    #[tokio::test]
    async fn complete_response_survives_command_timeout_and_child_is_reaped() {
        crate::test_support::manual_time(async {
            let f = Fixture::new("hang");
            let config = f.config.clone();
            let hash = f.hash();
            let (consumed_tx, consumed_rx) = oneshot::channel();
            let (timer, mut timer_rx) = crate::test_support::TimerWitness::channel();
            let task = tokio::spawn(async move {
                Forward::open_inner(
                    config,
                    hash,
                    TestHooks {
                        command: Some(CommandProbe {
                            consumed: consumed_tx,
                            timer,
                        }),
                        ..Default::default()
                    },
                )
                .await
            });
            consumed_rx.await.unwrap();
            crate::test_support::advance_registered_timer(&mut timer_rx).await;
            assert!(task.await.unwrap().is_err());
            assert!(!f.root.join("forward").exists());
            let pid = std::fs::read_to_string(f.root.join("allocated"))
                .unwrap()
                .parse::<i32>()
                .unwrap();
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        })
        .await;
    }
    #[tokio::test]
    async fn replaced_tuple_is_not_removed_and_cleanup_failure_propagates() {
        crate::test_support::manual_time(async {
            let f = Fixture::new("normal");
            let owner = Forward::open(f.config.clone(), f.hash()).await.unwrap();
            std::fs::write(
                f.root.join("forward"),
                "another-device tcp:49123 tcp:8082\n",
            )
            .unwrap();
            assert!(owner
                .close()
                .await
                .unwrap_err()
                .to_string()
                .contains("replaced"));
            assert!(!f.calls().iter().any(|a| a.iter().any(|s| s == "--remove")));
        })
        .await;
    }
    #[tokio::test]
    async fn missing_tuple_is_expired_not_permission_to_remove() {
        crate::test_support::manual_time(async {
            let f = Fixture::new("normal");
            let owner = Forward::open(f.config.clone(), f.hash()).await.unwrap();
            std::fs::remove_file(f.root.join("forward")).unwrap();
            owner.close().await.unwrap();
            assert!(!f.calls().iter().any(|a| a.iter().any(|s| s == "--remove")));
        })
        .await;
    }
    #[tokio::test]
    async fn changed_executable_is_rejected_before_any_device_command() {
        crate::test_support::manual_time(async {
            let f = Fixture::new("normal");
            let expected = f.hash();
            let mut bytes = std::fs::read(&f.config.executable).unwrap();
            bytes.extend_from_slice(b"\n# changed\n");
            std::fs::write(&f.config.executable, bytes).unwrap();
            assert!(Forward::open(f.config.clone(), expected)
                .await
                .unwrap_err()
                .to_string()
                .contains("executable changed"));
            assert!(f.calls().is_empty());
        })
        .await;
    }
    #[tokio::test]
    async fn wrong_unauthorized_or_non_usb_device_never_allocates() {
        crate::test_support::manual_time(async {
            for mode in ["offline", "wrongserial", "network"] {
                let f = Fixture::new(mode);
                assert!(Forward::open(f.config.clone(), f.hash()).await.is_err());
                assert!(!f.root.join("allocated").exists());
            }
        })
        .await;
    }
    #[test]
    fn strict_port_and_tuple_parsing_never_accepts_ambiguity() {
        for input in [b"".as_slice(), b"123", b"0\n", b"65536\n", b"123\n456\n"] {
            assert!(allocated_port(input).is_err());
        }
        assert!(binding_state("a tcp:9 tcp:8082\na tcp:9 tcp:8082\n", "a", 9).is_err());
        assert!(binding_state("a tcp:9 tcp:8080\n", "a", 9).is_err());
        assert!(!binding_state("b tcp:8 tcp:8082\n", "a", 9).unwrap());
    }

    async fn decoder_event(
        rx: &mut tokio::sync::mpsc::Receiver<crate::observation::ScanEvent>,
    ) -> (i32, u64) {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(crate::observation::ScanEvent::Cellular { record, .. }) =
                    rx.recv().await
                {
                    if let Some(pid) = record["event"]["pid"].as_i64() {
                        assert_eq!(record["host_transport"]["kind"], "managed_adb_usb");
                        return (
                            pid as i32,
                            record["connection_generation"].as_u64().unwrap(),
                        );
                    }
                }
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn actual_transport_eof_releases_forward_before_bounded_reconnect() {
        crate::test_support::manual_time(async {
        let f = Fixture::new("normal");
        let decoder = f.root.join("decoder");
        std::fs::write(&decoder, r#"#!/usr/bin/env python3
import json,os,signal,sys,pathlib
assert sys.argv[1:]==['--endpoint','ws://127.0.0.1:49123/raw-diag']
p=pathlib.Path(__file__).parent
n=int((p/'connections').read_text())+1 if (p/'connections').exists() else 1
(p/'connections').write_text(str(n))
print(json.dumps({'ipc_version':1,'kind':'hello','stream':{'version':1,'replay':False,'format':'u32le_json_length+json+exact_device_read','stream_epoch':'fixture','next_sequence':0}}),flush=True)
print(json.dumps({'ipc_version':1,'kind':'status','pid':os.getpid()}),flush=True)
if n>1:signal.pause()
"#).unwrap();
        std::fs::set_permissions(&decoder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let control = crate::control::ScanControl::new(true);
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let (retry, mut retry_rx) = crate::test_support::TimerWitness::channel();
        let task = tokio::spawn(crate::rayhunter_source::scan_adb_test(
            f.config.clone(),
            decoder,
            tx,
            control.clone(),
            crate::rayhunter_source::TestHooks { retry: Some(retry), ..Default::default() },
        ));
        let (first, _) = decoder_event(&mut rx).await;
        timeout(Duration::from_secs(4), async {
            loop {
                if let Some(crate::observation::ScanEvent::Cellular { record, .. }) =
                    rx.recv().await
                {
                    if record["reason"] == "transport_disconnect" {
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        crate::test_support::advance_registered_timer(&mut retry_rx).await;
        let (second, _) = decoder_event(&mut rx).await;
        assert_ne!(first, second);
        control.shutdown();
        timeout(Duration::from_secs(8), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let calls = f.calls();
        let allocations: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_slice() == ["forward", "tcp:0", "tcp:8082"])
            .map(|(i, _)| i)
            .collect();
        let removals: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_slice() == ["forward", "--remove", "tcp:49123"])
            .map(|(i, _)| i)
            .collect();
        assert_eq!(allocations.len(), 2);
        assert_eq!(removals.len(), 2);
        assert!(
            allocations[0] < removals[0]
                && removals[0] < allocations[1]
                && allocations[1] < removals[1]
        );
        assert!(!f.root.join("forward").exists());
        }).await;
    }
    #[tokio::test]
    async fn actual_source_pause_resume_finish_and_cancel_release_forward_and_decoder() {
        crate::test_support::manual_time(async {
        use std::sync::atomic::Ordering;
        for cancel in [false, true] {
            let f = Fixture::new("normal");
            let decoder = f.root.join("decoder");
            std::fs::write(&decoder,r#"#!/usr/bin/env python3
import json,os,signal,sys
assert sys.argv[1:]==['--endpoint','ws://127.0.0.1:49123/raw-diag']
print(json.dumps({'ipc_version':1,'kind':'hello','stream':{'version':1,'replay':False,'format':'u32le_json_length+json+exact_device_read','stream_epoch':'fixture','next_sequence':0}}),flush=True)
for tick in range(4):
 print(json.dumps({'ipc_version':1,'kind':'status','pid':os.getpid(),'tick':tick}),flush=True)
signal.pause()
"#).unwrap();
            std::fs::set_permissions(&decoder, std::fs::Permissions::from_mode(0o700)).unwrap();
            let control = crate::control::ScanControl::new(true);
            let (tx, mut rx) = tokio::sync::mpsc::channel(2);
            let (witness_tx, mut witness_rx) = tokio::sync::mpsc::channel(8);
            let (reaped_tx, mut reaped_rx) = tokio::sync::mpsc::channel(8);
            let (forward_tx, mut forward_rx) = tokio::sync::mpsc::channel(8);
            let task = tokio::spawn(crate::rayhunter_source::scan_adb_test(
                f.config.clone(),
                decoder,
                tx,
                control.clone(),
                crate::rayhunter_source::TestHooks {
                    ingress: Some(witness_tx), reaped: Some(reaped_tx),
                    forward_completed: Some(forward_tx), ..Default::default()
                },
            ));
            let (first, generation) = decoder_event(&mut rx).await;
            let last = if cancel {
                timeout(Duration::from_secs(4), async {
                    while witness_rx.recv().await.unwrap() != 3 {}
                })
                .await
                .unwrap();
                assert_eq!(rx.len(), 2, "tick 3 faces the full source queue");
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                first
            } else {
                control.store(false, Ordering::Relaxed);
                f.wait_removed().await;
                assert_eq!(forward_rx.recv().await, Some(true));
                assert_eq!(reaped_rx.recv().await, Some((first as u32, true)));
                while rx.try_recv().is_ok() {}
                control.store(true, Ordering::Relaxed);
                let (second, next) = decoder_event(&mut rx).await;
                assert_ne!(first, second);
                assert!(next > generation);
                assert_eq!(unsafe { libc::kill(first, 0) }, -1);
                control.shutdown();
                timeout(Duration::from_secs(8), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                second
            };
            f.wait_removed().await;
            assert_eq!(forward_rx.recv().await, Some(true));
            assert_eq!(reaped_rx.recv().await, Some((last as u32, true)));
            assert_eq!(unsafe { libc::kill(last, 0) }, -1);
            assert!(!f.root.join("forward").exists());
        }
        }).await;
    }
}
