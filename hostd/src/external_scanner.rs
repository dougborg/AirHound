// SPDX-License-Identifier: MIT
//! Own a companion process through graceful radio shutdown and process reaping.
//! Only an explicitly selected executable is run, directly without a shell.
use crate::{
    control::ScanControl,
    external_wifi::{SessionValidator, MAX_LINE_BYTES},
    observation::ScanEvent,
};
use std::{io, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
    time::{timeout, Instant},
};

pub struct Options {
    pub usb_selector: Option<String>,
    pub executable: PathBuf,
    pub firmware: PathBuf,
    pub channels: Vec<u8>,
    pub dwell_ms: u16,
    #[cfg(test)]
    pub fixture_script: Option<PathBuf>,
}

fn publish_identity(
    control: &ScanControl,
    identity: &airhound_hostd::external_identity::Identity,
    selected: Option<&str>,
    state: &str,
) {
    control.set_receiver(crate::receiver::Receiver {
        backend: "mt7610u_companion",
        adapter_id: selected.unwrap_or("unreported").into(),
        adapter_info: identity.description(),
        firmware: None,
        controller_address: None,
        selection: "explicit_helper_reported_attachment",
        availability: match state {
            "running" => "running",
            "paused" => "paused",
            "recovering" => "recovering",
            "stopped" => "stopped",
            _ => "failed",
        },
        scan_configuration: "channel_plan",
        extended_advertising: crate::receiver::Feature::default(),
        coded_rx: crate::receiver::Feature::default(),
        controller_phy: None,
        capability_query: "not_applicable_wifi",
        producer_identity: None,
        external_identity: Some(identity.clone()),
    });
}

fn error(message: &str) -> io::Error {
    io::Error::other(message)
}

fn selected_identity(
    message: &serde_json::Value,
    requested: Option<&str>,
    selected: &mut Option<String>,
) -> io::Result<()> {
    let reported = message
        .get("usb_device")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.get("selector")
                .and_then(|v| v.as_str())
                .filter(|s| crate::external_wifi::valid_usb_selector(s))
                .ok_or_else(|| error("invalid companion USB identity"))
        })
        .transpose()?;
    let expected = selected.as_deref().or(requested);
    // Legacy helpers may omit identity only until one has been learned. New
    // helpers retain it through pause/recovery/stop, so every later barrier
    // must identify the same attachment before we send further commands.
    if expected.is_some()
        && (selected.is_some() || message["state"] == "running")
        && reported != expected
    {
        return Err(error("companion did not confirm pinned USB selector"));
    }
    if let Some(id) = reported {
        if requested.is_some_and(|r| r != id) || selected.as_deref().is_some_and(|r| r != id) {
            return Err(error("companion changed USB identity"));
        }
        *selected = Some(id.to_owned());
    }
    Ok(())
}

/// Cancellation-safe bounded framing: the pending line survives select! races.
struct Lines<R> {
    reader: BufReader<R>,
    pending: Vec<u8>,
    discarding: bool,
}
impl<R: AsyncRead + Unpin> Lines<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            pending: Vec::new(),
            discarding: false,
        }
    }
    async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let data = self.reader.fill_buf().await?;
            if data.is_empty() {
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    self.pending.clear();
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "companion truncated a record",
                    ))
                };
            }
            let newline = data.iter().position(|&b| b == b'\n');
            let count = newline.map_or(data.len(), |n| n + 1);
            if self.discarding {
                self.reader.consume(count);
                if newline.is_some() {
                    self.discarding = false;
                }
                continue;
            }
            if self.pending.len() + count > MAX_LINE_BYTES {
                self.reader.consume(count);
                self.pending.clear();
                self.discarding = newline.is_none();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "companion record exceeds 16KiB",
                ));
            }
            self.pending.extend_from_slice(&data[..count]);
            self.reader.consume(count);
            if newline.is_some() {
                self.pending.pop();
                return Ok(Some(std::mem::take(&mut self.pending)));
            }
        }
    }
}

async fn command(input: &mut ChildStdin, command: &str, request_id: u64) -> io::Result<()> {
    timeout(Duration::from_secs(1), async {
        input
            .write_all(
                format!("{{\"cmd\":\"{command}\",\"request_id\":{request_id}}}\n").as_bytes(),
            )
            .await?;
        input.flush().await
    })
    .await
    .map_err(|_| error("companion command pipe timed out"))?
}

fn json(line: &[u8]) -> io::Result<serde_json::Value> {
    serde_json::from_slice(line).map_err(|_| error("companion emitted invalid JSON"))
}

#[derive(Clone, Copy)]
enum Pending {
    Initial,
    Pause(u64),
    Resume(u64),
    Recovery,
}

async fn issue(input: &mut ChildStdin, command_name: &str, next: &mut u64) -> io::Result<u64> {
    let id = *next;
    *next = next
        .checked_add(1)
        .ok_or_else(|| error("companion request ID overflow"))?;
    command(input, command_name, id).await?;
    Ok(id)
}

fn read_metrics(
    message: &serde_json::Value,
    dropped: &mut u64,
    reconnects: &mut u64,
) -> io::Result<()> {
    if let Some(value) = message.get("dropped_observations") {
        *dropped = value
            .as_u64()
            .ok_or_else(|| error("invalid companion drop counter"))?;
    }
    if let Some(value) = message.get("reconnect_attempts") {
        *reconnects = value
            .as_u64()
            .ok_or_else(|| error("invalid companion reconnect counter"))?;
    }
    Ok(())
}

async fn stderr_drain(mut input: impl AsyncRead + Unpin) -> io::Result<u64> {
    // Diagnostics may include sensitive data; discard contents, retain only a count.
    let mut bytes = [0; 4096];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut bytes).await?;
        if n == 0 {
            return Ok(total);
        }
        total = total.saturating_add(n as u64);
        tokio::task::yield_now().await;
    }
}

async fn publish_status(tx: &mpsc::Sender<ScanEvent>, event: ScanEvent) {
    // Give lifecycle updates a bounded chance to follow queued observations;
    // a full observation queue must not silently swallow a pause acknowledgement.
    if timeout(Duration::from_millis(250), tx.send(event))
        .await
        .is_err()
    {
        log::warn!("External scanner lifecycle update dropped: output remains backpressured");
    }
}

pub async fn scan_loop(
    options: Options,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    scan_loop_with_timeouts(
        options,
        tx,
        control,
        Duration::from_secs(10),
        Duration::from_secs(15),
    )
    .await
}

async fn scan_loop_with_timeouts(
    options: Options,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
    hello_timeout: Duration,
    stop_timeout: Duration,
) -> io::Result<()> {
    let mut state = control.subscribe();
    if !crate::control::wait_running(&mut state, &tx).await {
        return Ok(());
    }
    if options
        .usb_selector
        .as_deref()
        .is_some_and(|s| !crate::external_wifi::valid_usb_selector(s))
    {
        return Err(error("invalid canonical USB selector"));
    }
    let mut command = Command::new(&options.executable);
    // Test fixtures are mutable temporary scripts. Read them through a stable
    // interpreter rather than exec'ing their inode: Linux rejects execution
    // while any process still holds a writable descriptor (ETXTBSY), including
    // a descriptor briefly inherited by a concurrently spawning test child.
    // Production still executes only the explicitly selected receiver directly.
    #[cfg(test)]
    if let Some(script) = &options.fixture_script {
        command.arg(script);
    }
    command.arg("stream");
    if let Some(selector) = &options.usb_selector {
        command.args(["--usb-selector", selector]);
    }
    let mut child = command
        .arg("--firmware")
        .arg(&options.firmware)
        .arg("--channels")
        .arg(
            options
                .channels
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .arg("--dwell-ms")
        .arg(options.dwell_ms.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = child.stdin.take().expect("piped stdin");
    let mut lines = Lines::new(child.stdout.take().expect("piped stdout"));
    let stderr = tokio::spawn(stderr_drain(child.stderr.take().expect("piped stderr")));
    let mut stopped: Option<serde_json::Value> = None;
    let mut drops = 0u64;
    let mut selected_usb = None;
    let mut identity = airhound_hostd::external_identity::Identity::default();
    let mut producer_dropped = 0u64;
    let mut reconnect_attempts = 0u64;
    let mut next_request = 1u64;
    // Capture the generation before awaiting the handshake: changes during
    // startup must also cross the verified pause/resume barrier.
    let mut generation = state.borrow().generation;
    let result = async {
        let hello = timeout(hello_timeout, lines.next()).await
            .map_err(|_| error("companion hello timed out"))??.ok_or_else(|| error("companion exited before hello"))?;
        let hello = json(&hello)?;
        if hello["type"] != "hello" || hello["protocol_version"] != 1 { return Err(error("unsupported companion handshake")); }
        identity = airhound_hostd::external_identity::Identity::from_hello(&hello)?;
        let mut validator = SessionValidator::default();
        let mut running = false;
        let mut forwarding = false;
        let mut pending = Some(Pending::Initial);
        let mut deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let current = *state.borrow_and_update();
            if current.shutdown { return Ok(()); }
            if current.generation != generation {
                generation = current.generation;
                forwarding = false;
                // Even false->true coalesced into one watch update needs a
                // hardware pause barrier; latest enabled=true is not proof of it.
                if !matches!(pending, Some(Pending::Pause(_))) {
                    let id = issue(&mut input, "pause", &mut next_request).await?;
                    pending = Some(Pending::Pause(id));
                    deadline = Instant::now() + Duration::from_secs(30);
                }
            }
            tokio::select! {
                _ = tx.closed() => return Ok(()),
                status = child.wait() => { status?; return Err(error("companion exited unexpectedly")); },
                changed = state.changed() => { if changed.is_err() { return Ok(()); } },
                _ = tokio::time::sleep_until(deadline), if pending.is_some() => return Err(error("companion state acknowledgement timed out")),
                line = lines.next() => {
                    let line = line?.ok_or_else(|| error("companion stdout closed without verified stop"))?;
                    let message = json(&line)?;
                    match message["type"].as_str() {
                        Some("beacon") => {
                            let latest = *state.borrow();
                            // Old queued records may arrive while crossing a
                            // generation barrier. They do not belong to the next
                            // validated session and must never advance its sequence.
                            if matches!(pending, Some(Pending::Pause(_) | Pending::Resume(_) | Pending::Recovery))
                                || latest.generation != generation { continue; }
                            if !running { return Err(error("companion observation outside a running session")); }
                            let observation = validator.accept(&line)?;
                            if forwarding && latest.enabled && !latest.shutdown && latest.generation == generation {
                                match tx.try_send(ScanEvent::ExternalWiFi { event: observation, generation }) {
                                    Ok(()) => {},
                                    Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                                    Err(mpsc::error::TrySendError::Full(_)) => drops = drops.saturating_add(1),
                                }
                            }
                        }
                        Some("status") => {
                            selected_identity(&message, options.usb_selector.as_deref(), &mut selected_usb)?;
                            let status = message["state"].as_str().ok_or_else(|| error("missing companion state"))?;
                            let acknowledgement = message.get("request_id").and_then(serde_json::Value::as_u64);
                            match status {
                                "running" => {
                                    let accepted = matches!(pending, Some(Pending::Initial | Pending::Recovery))
                                        || matches!(pending, Some(Pending::Resume(id)) if acknowledgement == Some(id));
                                    if accepted {
                                        if !running { validator = SessionValidator::default(); }
                                        running = true;
                                        pending = None;
                                        let latest = *state.borrow();
                                        forwarding = latest.enabled && !latest.shutdown && latest.generation == generation;
                                    } else if pending.is_some() || !running { continue; }
                                }
                                "paused" => {
                                    if message["cleanup_verified"] != true { return Err(error("companion pause did not verify radio shutdown")); }
                                    let accepted = matches!(pending, Some(Pending::Pause(id)) if acknowledgement == Some(id));
                                    if !accepted && (pending.is_some() || running) { continue; }
                                    running = false; forwarding = false;
                                    if accepted {
                                        pending = None;
                                        let latest = *state.borrow_and_update();
                                        if latest.shutdown { return Ok(()); }
                                        generation = latest.generation;
                                        if latest.enabled {
                                            let id = issue(&mut input, "resume", &mut next_request).await?;
                                            pending = Some(Pending::Resume(id));
                                            deadline = Instant::now() + Duration::from_secs(30);
                                        }
                                    }
                                }
                                "recovering" => {
                                    running = false; forwarding = false;
                                    if pending.is_none() || matches!(pending, Some(Pending::Initial)) {
                                        pending = Some(Pending::Recovery);
                                        deadline = Instant::now() + Duration::from_secs(90);
                                    }
                                }
                                _ => return Err(error("unknown companion state")),
                            }
                            identity.status(&message)?;
                            publish_identity(&control, &identity, selected_usb.as_deref(), status);
                            read_metrics(&message, &mut producer_dropped, &mut reconnect_attempts)?;
                            publish_status(&tx, ScanEvent::ExternalStatus { state: status.to_owned(), dropped: drops, producer_dropped, reconnect_attempts, usb_selector: selected_usb.clone(), identity:identity.clone() }).await;
                        }
                        Some("stopped") => { stopped = Some(message); return Err(error("companion stopped unexpectedly")); },
                        _ => return Err(error("unexpected companion message")),
                    }
                }
            }
        }
    }.await;
    let cleanup = shutdown_child(
        &mut child,
        &mut input,
        &mut lines,
        stopped,
        stop_timeout,
        next_request,
    )
    .await
    .and_then(|message| {
        selected_identity(&message, options.usb_selector.as_deref(), &mut selected_usb)?;
        read_metrics(&message, &mut producer_dropped, &mut reconnect_attempts)
    });
    // An inherited pipe in a broken companion descendant must not hang teardown.
    let mut stderr = stderr;
    let stderr_result = timeout(Duration::from_secs(1), &mut stderr).await;
    if stderr_result.is_err() {
        stderr.abort();
        let _ = stderr.await;
    }
    identity.firmware = None;
    publish_identity(
        &control,
        &identity,
        selected_usb.as_deref(),
        if cleanup.is_ok() && result.is_ok() {
            "stopped"
        } else {
            "failed"
        },
    );
    log::info!("External WiFi companion finished; host queue dropped {drops} observations");
    publish_status(
        &tx,
        ScanEvent::ExternalStatus {
            usb_selector: selected_usb,
            identity,
            state: if cleanup.is_ok() && result.is_ok() {
                "stopped"
            } else {
                "failed"
            }
            .into(),
            dropped: drops,
            producer_dropped,
            reconnect_attempts,
        },
    )
    .await;
    match (result, cleanup) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(e), Ok(_)) | (Ok(()), Err(e)) => Err(e),
        (Err(e), Err(cleanup)) => Err(error(&format!(
            "{e}; companion cleanup also failed: {cleanup}"
        ))),
    }
}

async fn shutdown_child<R: AsyncRead + Unpin>(
    child: &mut Child,
    input: &mut ChildStdin,
    lines: &mut Lines<R>,
    mut stopped: Option<serde_json::Value>,
    stop_timeout: Duration,
    request_id: u64,
) -> io::Result<serde_json::Value> {
    let graceful = timeout(stop_timeout, async {
        if stopped.is_none() {
            command(input, "stop", request_id).await?;
            loop {
                // A malformed observation is fatal to the scan but must not
                // prevent a later verified shutdown acknowledgement. Resync
                // only at bounded line boundaries; the overall grace applies.
                let line = match lines.next().await {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let Ok(message) = json(&line) else {
                    continue;
                };
                if message["type"] == "stopped"
                    && message["request_id"].as_u64() == Some(request_id)
                {
                    stopped = Some(message);
                    break;
                }
            }
        }
        let stopped = stopped
            .take()
            .filter(|message| message["cleanup_verified"] == true)
            .ok_or_else(|| error("companion did not verify radio cleanup"))?;
        let status = child.wait().await?;
        if !status.success() {
            return Err(error("companion exited unsuccessfully"));
        }
        Ok(stopped)
    })
    .await;
    match graceful {
        Ok(Ok(stopped)) => Ok(stopped),
        failed => {
            let reason = match failed {
                Ok(Err(e)) => e.to_string(),
                _ => "companion graceful shutdown timed out".into(),
            };
            // kill() requests termination AND reaps the child; bounded even for a
            // companion that ignores its command pipe. Never claim verified cleanup.
            timeout(Duration::from_secs(2), child.kill())
                .await
                .map_err(|_| error("companion kill/reap timed out; radio state unverified"))??;
            Err(error(&format!(
                "{reason}; forced termination, radio cleanup unverified"
            )))
        }
    }
}

#[cfg(all(test, unix))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Fake {
        directory: PathBuf,
        executable: PathBuf,
    }
    impl Fake {
        fn new(body: &str) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "airhound-child-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory).unwrap();
            let executable = directory.join("receiver");
            let mut file = std::fs::File::create(&executable).unwrap();
            writeln!(
                file,
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'/pid\n{body}",
                directory.display()
            )
            .unwrap();
            file.set_permissions(std::fs::Permissions::from_mode(0o700))
                .unwrap();
            Self {
                directory,
                executable,
            }
        }
        fn options(&self) -> Options {
            Options {
                usb_selector: None,
                executable: PathBuf::from("/bin/sh"),
                fixture_script: Some(self.executable.clone()),
                firmware: self.directory.join("unused"),
                channels: vec![1],
                dwell_ms: 500,
            }
        }
        fn assert_exited(&self) {
            let pid = match std::fs::read_to_string(self.directory.join("pid")) {
                Ok(pid) => pid,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return, // killed before script exec
                Err(e) => panic!("reading fake child PID: {e}"),
            };
            assert!(!std::process::Command::new("/bin/kill")
                .args(["-0", &pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success());
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    #[test]
    fn reported_usb_identity_is_validated_and_cannot_change_on_reconnect() {
        let mut selected = None;
        let running =
            |id: &str| serde_json::json!({"state":"running","usb_device":{"selector":id}});
        assert!(selected_identity(
            &serde_json::json!({"state":"running"}),
            Some("usb:1:2"),
            &mut selected
        )
        .is_err());
        assert!(selected_identity(&running("usb:1:3"), Some("usb:1:2"), &mut selected).is_err());
        selected_identity(&running("usb:1:2"), Some("usb:1:2"), &mut selected).unwrap();
        assert_eq!(selected.as_deref(), Some("usb:1:2"));
        assert!(
            selected_identity(&serde_json::json!({"state":"running"}), None, &mut selected)
                .is_err()
        );
        assert!(selected_identity(
            &serde_json::json!({"type":"stopped","usb_device":{"selector":"usb:1:3"}}),
            None,
            &mut selected
        )
        .is_err());
        assert!(
            selected_identity(&serde_json::json!({"type":"stopped"}), None, &mut selected).is_err()
        );
        selected_identity(
            &serde_json::json!({"type":"stopped","usb_device":{"selector":"usb:1:2"}}),
            None,
            &mut selected,
        )
        .unwrap();

        selected_identity(&running("usb:1:2"), Some("usb:1:2"), &mut selected).unwrap();
        assert!(selected_identity(&running("usb:1:3"), None, &mut selected).is_err());
        assert!(selected_identity(&running("usb:01:2"), None, &mut selected).is_err());
        assert!(selected_identity(
            &serde_json::json!({"state":"recovering"}),
            Some("usb:1:2"),
            &mut selected
        )
        .is_err());
        selected_identity(
            &serde_json::json!({"state":"recovering","usb_device":{"selector":"usb:1:2"}}),
            Some("usb:1:2"),
            &mut selected,
        )
        .unwrap();
        selected_identity(
            &serde_json::json!({"type":"stopped","usb_device":null}),
            Some("usb:1:2"),
            &mut None,
        )
        .unwrap();
        selected_identity(&serde_json::json!({"state":"running"}), None, &mut None).unwrap();
    }

    #[tokio::test]
    async fn selected_usb_is_forwarded_and_reported_without_shell_interpolation() {
        let body = LIFECYCLE
            .replace(
                "\"type\":\"status\"",
                "\"type\":\"status\",\"usb_device\":{\"selector\":\"usb:1:2.3\"}",
            )
            .replace(
                "\"type\":\"stopped\"",
                "\"type\":\"stopped\",\"usb_device\":{\"selector\":\"usb:1:2.3\"}",
            );
        let fake = Fake::new(&format!("printf '%s\\n' \"$@\" > \"$0.args\"\n{body}"));
        let mut options = fake.options();
        options.usb_selector = Some("usb:1:2.3".into());
        let control = ScanControl::new(true);
        let (tx, mut rx) = mpsc::channel(8);
        let worker = tokio::spawn(scan_loop(options, tx, control.clone()));
        let status = timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(status, ScanEvent::ExternalStatus { usb_selector: Some(id), state, .. } if id == "usb:1:2.3" && state == "running")
        );
        control.shutdown();
        timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let arguments = std::fs::read_to_string(fake.executable.with_extension("args")).unwrap();
        let args: Vec<_> = arguments.lines().collect();
        assert!(args
            .windows(2)
            .any(|a| a == ["--usb-selector", "usb:1:2.3"]));
        fake.assert_exited();
    }

    #[tokio::test]
    async fn mismatched_pause_identity_never_triggers_resume() {
        let body = LIFECYCLE
            .replace(
                "\"state\":\"running\"",
                "\"state\":\"running\",\"usb_device\":{\"selector\":\"usb:1:2\"}",
            )
            .replace(
                "\"state\":\"paused\"",
                "\"state\":\"paused\",\"usb_device\":{\"selector\":\"usb:1:3\"}",
            )
            .replace(
                "\"type\":\"stopped\"",
                "\"type\":\"stopped\",\"usb_device\":{\"selector\":\"usb:1:2\"}",
            )
            .replace(
                "case \"$command\" in",
                "printf '%s\\n' \"$command\" >> \"$0.commands\"\ncase \"$command\" in",
            );
        let fake = Fake::new(&body);
        let control = ScanControl::new(true);
        let (tx, mut rx) = mpsc::channel(8);
        let worker = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        control.store(false, std::sync::atomic::Ordering::Relaxed);
        control.store(true, std::sync::atomic::Ordering::Relaxed);
        let error = timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            error.to_string().contains("USB selector")
                || error.to_string().contains("USB identity"),
            "{error}"
        );
        let commands = std::fs::read_to_string(fake.executable.with_extension("commands")).unwrap();
        assert!(commands.contains("pause"));
        assert!(!commands.contains("resume"));
        fake.assert_exited();
    }

    #[tokio::test]
    async fn mismatched_stopped_identity_is_not_a_verified_selected_stop() {
        let body = LIFECYCLE
            .replace(
                "\"state\":\"running\"",
                "\"state\":\"running\",\"usb_device\":{\"selector\":\"usb:1:2\"}",
            )
            .replace(
                "\"type\":\"stopped\"",
                "\"type\":\"stopped\",\"usb_device\":{\"selector\":\"usb:1:3\"}",
            );
        let fake = Fake::new(&body);
        let control = ScanControl::new(true);
        let (tx, mut rx) = mpsc::channel(8);
        let worker = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        control.shutdown();
        assert!(timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        let status = rx.recv().await.unwrap();
        assert!(
            matches!(status, ScanEvent::ExternalStatus {state, usb_selector:Some(id), ..} if state == "failed" && id == "usb:1:2")
        );
        fake.assert_exited();
    }

    const LIFECYCLE: &str = r#"
printf '%s\n' '{"type":"hello","protocol_version":1}' '{"type":"status","state":"running"}'
while IFS= read -r command; do
id=${command##*:}; id=${id%?}
case "$command" in
*pause*) printf '{"type":"status","state":"paused","cleanup_verified":true,"request_id":%s}\n' "$id";;
*resume*) printf '{"type":"status","state":"running","request_id":%s}\n' "$id";;
*stop*) printf '{"type":"stopped","cleanup_verified":true,"request_id":%s,"dropped_observations":17,"reconnect_attempts":2}\n' "$id"; exit 0;;
esac
done
exit 3
"#;
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn writable_fixture_descriptor_does_not_prevent_receiver_lifecycle() {
        let fake = Fake::new(LIFECYCLE);
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .open(&fake.executable)
            .unwrap();
        // Reproduce the exact CI error deterministically, without timing or
        // relaxing supervision/handshake deadlines.
        let error = std::process::Command::new(&fake.executable)
            .spawn()
            .expect_err("Linux must reject exec of a writable script inode");
        assert_eq!(error.raw_os_error(), Some(26)); // ETXTBSY
        let (tx, mut rx) = mpsc::channel(4);
        let control = ScanControl::new(true);
        let mut task = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        status(&mut rx, "running", &mut task).await;
        control.shutdown();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fake.assert_exited();
        drop(writer);
    }
    async fn status(
        rx: &mut mpsc::Receiver<ScanEvent>,
        expected: &str,
        task: &mut tokio::task::JoinHandle<io::Result<()>>,
    ) {
        timeout(Duration::from_secs(3), async {
            loop {
                let event = match rx.recv().await {
                    Some(event) => event,
                    None => panic!(
                        "receiver exited while awaiting {expected}: {:?}",
                        task.await
                    ),
                };
                match event {
                    ScanEvent::ExternalStatus { state, .. } if state == expected => break,
                    ScanEvent::ExternalStatus { state, .. } => assert_ne!(state, "failed"),
                    _ => panic!("unexpected observation"),
                }
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn helper_build_survives_as_current_owner_metadata_but_firmware_does_not() {
        let body = LIFECYCLE.replace("\"protocol_version\":1", "\"protocol_version\":1,\"helper_build\":{\"component\":\"mt7610u-lab\",\"package_version\":\"0.1.0\",\"display_version\":\"git fixture\",\"git_revision\":null,\"source_tag\":null,\"tracked_source_modified\":null}");
        let fake = Fake::new(&body);
        let (tx, mut rx) = mpsc::channel(4);
        let control = ScanControl::new(true);
        let mut task = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        status(&mut rx, "running", &mut task).await;
        let health = control.health_message();
        assert_eq!(
            health["receiver"]["external_identity"]["helper_build"]["display_version"],
            "git fixture"
        );
        assert!(health["receiver"]["external_identity"]["firmware"].is_null());
        control.shutdown();
        task.await.unwrap().unwrap();
        assert_eq!(
            control.health_message()["receiver"]["availability"],
            "stopped"
        );
        fake.assert_exited();
    }

    #[tokio::test]
    async fn explicit_child_pauses_resumes_stops_and_is_reaped() {
        let fake = Fake::new(LIFECYCLE);
        let (tx, mut rx) = mpsc::channel(4);
        let control = ScanControl::new(true);
        let mut task = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        status(&mut rx, "running", &mut task).await;
        control.store(false, Ordering::Relaxed);
        status(&mut rx, "paused", &mut task).await;
        control.store(true, Ordering::Relaxed);
        status(&mut rx, "running", &mut task).await;
        control.shutdown();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fake.assert_exited();
        let final_status = rx.recv().await.unwrap();
        assert!(
            matches!(final_status, ScanEvent::ExternalStatus {state,producer_dropped:17,reconnect_attempts:2,..} if state == "stopped")
        );
    }
    #[tokio::test]
    async fn malformed_handshake_and_unexpected_eof_fail_and_reap() {
        for body in ["printf '%s\\n' '{\"type\":\"hello\",\"protocol_version\":2}'; while read -r line; do exit 0; done", "exit 0"] {
            let fake = Fake::new(body);
            let (tx, _rx) = mpsc::channel(4);
            assert!(timeout(Duration::from_secs(3), scan_loop(fake.options(), tx, ScanControl::new(true))).await.unwrap().is_err());
            fake.assert_exited();
        }
    }
    #[tokio::test]
    async fn stderr_flood_does_not_block_handshake_or_shutdown() {
        let fake = Fake::new(&format!("head -c 131072 /dev/zero >&2\n{LIFECYCLE}"));
        let (tx, mut rx) = mpsc::channel(4);
        let control = ScanControl::new(true);
        let mut task = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        status(&mut rx, "running", &mut task).await;
        control.shutdown();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fake.assert_exited();
    }
    #[tokio::test]
    async fn framing_survives_cancellation_and_enforces_limits_and_eof() {
        let (mut writer, reader) = tokio::io::duplex(65536);
        let mut lines = Lines::new(reader);
        writer.write_all(b"{\"partial\":").await.unwrap();
        assert!(timeout(Duration::from_millis(5), lines.next())
            .await
            .is_err());
        writer.write_all(b"1}\n").await.unwrap();
        assert_eq!(lines.next().await.unwrap().unwrap(), b"{\"partial\":1}");
        writer
            .write_all(&vec![b'x'; MAX_LINE_BYTES + 1])
            .await
            .unwrap();
        assert!(lines.next().await.is_err());
        let mut truncated = Lines::new(&b"unfinished"[..]);
        assert!(truncated.next().await.is_err());
    }
    #[tokio::test]
    async fn hello_timeout_and_uncooperative_stop_force_reap_and_report_failure() {
        let fake = Fake::new("while IFS= read -r line; do :; done");
        let (tx, _rx) = mpsc::channel(4);
        let error = timeout(
            Duration::from_secs(4),
            scan_loop_with_timeouts(
                fake.options(),
                tx,
                ScanControl::new(true),
                Duration::from_millis(250),
                Duration::from_millis(250),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("hello timed out"));
        assert!(error.to_string().contains("forced termination"));
        fake.assert_exited();
    }

    #[tokio::test]
    async fn closed_consumer_still_requests_verified_cleanup() {
        let fake = Fake::new(LIFECYCLE);
        let (tx, mut rx) = mpsc::channel(4);
        let mut task = tokio::spawn(scan_loop(fake.options(), tx, ScanControl::new(true)));
        status(&mut rx, "running", &mut task).await;
        drop(rx);
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fake.assert_exited();
    }

    #[tokio::test]
    async fn saturated_observation_queue_drops_data_without_blocking_pause_or_stop() {
        let beacon = |sequence| serde_json::json!({"schema_version":1,"type":"beacon","source":"mt7610u-lab","session_id":"burst","sequence":sequence,"bssid":"02:00:00:00:00:01","ssid_bytes":[255],"advertised_channel":1,"receive_channel":1,"elapsed_ms":sequence,"observed_at_unix_ms":1,"dwell_elapsed_ms":sequence,"channel_epoch":0,"rssi_dbm":null,"estimated_rssi_dbm":-80,"rssi_validated":false,"rssi_estimate_basis":"mt76_eeprom_formula","raw_rssi":[186,0,0,0],"full_calibration":false});
        let mut body = String::from("printf '%s\\n' '{\"type\":\"hello\",\"protocol_version\":1}' '{\"type\":\"status\",\"state\":\"running\"}'\n");
        for sequence in 0..200 {
            body.push_str(&format!("printf '%s\\n' '{}'\n", beacon(sequence)));
        }
        body.push_str("printf '%s\\n' '{\"type\":\"status\",\"state\":\"running\",\"dropped_observations\":7}'\n");
        body.push_str(
            LIFECYCLE
                .split("while IFS=")
                .nth(1)
                .map(|tail| format!("while IFS={tail}"))
                .unwrap()
                .as_str(),
        );
        // A marker after the burst replaces a scheduler-dependent fixed sleep.
        body = body.replacen(
            "while IFS=",
            ": > \"${0%/*}/burst-complete\"\nwhile IFS=",
            1,
        );
        let fake = Fake::new(&body);
        let (tx, mut rx) = mpsc::channel(1);
        let control = ScanControl::new(true);
        let mut task = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        // Keep the initial status queued while the burst is emitted. Process
        // startup on a loaded runner is not part of the pause/stop latency
        // assertion below; allow the production handshake budget plus burst I/O.
        timeout(Duration::from_secs(15), async {
            while !fake.directory.join("burst-complete").exists() {
                if task.is_finished() {
                    panic!(
                        "supervisor exited before the burst completed: {:?}",
                        (&mut task).await
                    );
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        status(&mut rx, "running", &mut task).await;
        control.store(false, Ordering::Relaxed);
        let drops = timeout(Duration::from_secs(3), async {
            loop {
                if let Some(ScanEvent::ExternalStatus { state, dropped, .. }) = rx.recv().await {
                    if state == "paused" {
                        break dropped;
                    }
                }
            }
        })
        .await
        .unwrap();
        assert!(drops > 0);
        control.shutdown();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fake.assert_exited();
    }
    #[tokio::test]
    async fn malformed_observation_still_allows_verified_graceful_shutdown() {
        let body = LIFECYCLE.replace("while IFS=", "printf '%s\\n' 'not-json'\nwhile IFS=");
        let fake = Fake::new(&body);
        let (tx, _rx) = mpsc::channel(4);
        let failure = timeout(
            Duration::from_secs(3),
            scan_loop(fake.options(), tx, ScanControl::new(true)),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(failure.to_string().contains("invalid JSON"));
        assert!(!failure.to_string().contains("forced termination"));
        fake.assert_exited();
    }
    #[tokio::test]
    async fn coalesced_pause_resume_requires_correlated_barrier_and_discards_stale_records() {
        let beacon = |session: &str, sequence| serde_json::json!({"schema_version":1,"type":"beacon","source":"mt7610u-lab","session_id":session,"sequence":sequence,"bssid":"02:00:00:00:00:01","ssid_bytes":[],"advertised_channel":1,"receive_channel":1,"elapsed_ms":sequence,"observed_at_unix_ms":1,"dwell_elapsed_ms":sequence,"channel_epoch":0,"rssi_dbm":null,"estimated_rssi_dbm":null,"rssi_validated":false,"rssi_estimate_basis":"mt76_eeprom_formula","raw_rssi":[0,0,0,0],"full_calibration":false});
        let old0 = beacon("old", 0);
        let old1 = beacon("old", 1);
        let fresh = beacon("new", 0);
        let body = format!(
            r#"
printf '%s\n' '{{"type":"hello","protocol_version":1}}' '{{"type":"status","state":"running"}}'
while IFS= read -r command; do
id=${{command##*:}}; id=${{id%?}}
case "$command" in
*pause*) printf '%s\n' '{{"type":"status","state":"running","request_id":999}}' '{old0}'; printf '{{"type":"status","state":"paused","cleanup_verified":true,"request_id":%s}}\n' "$id";;
*resume*) printf '%s\n' '{{"type":"status","state":"running","request_id":0}}' '{old1}'; printf '{{"type":"status","state":"running","request_id":%s}}\n' "$id"; printf '%s\n' '{fresh}';;
*stop*) printf '{{"type":"stopped","cleanup_verified":true,"request_id":%s}}\n' "$id"; exit 0;;
esac
done
"#
        );
        let fake = Fake::new(&body);
        let (tx, mut rx) = mpsc::channel(16);
        let control = ScanControl::new(true);
        let mut task = tokio::spawn(scan_loop(fake.options(), tx, control.clone()));
        status(&mut rx, "running", &mut task).await;
        // No yield: watch coalesces these transitions into latest enabled=true.
        control.store(false, Ordering::Relaxed);
        control.store(true, Ordering::Relaxed);
        let event = timeout(Duration::from_secs(3), async {
            loop {
                match rx.recv().await.unwrap() {
                    ScanEvent::ExternalWiFi { event, generation } => break (event, generation),
                    ScanEvent::ExternalStatus { state, .. } => assert_ne!(state, "failed"),
                    _ => panic!("unexpected event"),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(event.0.session_id, "new");
        assert_eq!(event.0.sequence, 0);
        assert_eq!(event.1, 2);
        control.shutdown();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fake.assert_exited();
    }
}
