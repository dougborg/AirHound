//! Bounded child protocol. USB stays on the main thread; blocked pipe readers
//! cannot prevent that thread from shutting down the receive gates.
use anyhow::{ensure, Result};
use mt7610u_lab::{firmware::Firmware, transport::UsbDevice};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod hardware;
use hardware::Hardware;
mod output;
mod protocol;
use output::Output;
use protocol::{read_commands, Action, Command, InputOutcome};

const RECOVERY_BUDGET: Duration = Duration::from_secs(60);
const MAX_RECOVERY_ATTEMPTS: u32 = 8;

trait ReceiverHardware {
    fn start(&mut self) -> Result<()>;
    fn shutdown(&mut self) -> Result<()>;
    fn poll(&mut self) -> Result<Vec<Value>>;
    fn metrics(&self) -> Value {
        json!({"transfers":0,"frames":0,"beacons":0,"malformed_transfers":0})
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Running,
    Paused,
    Recovering,
}
struct Runtime<H> {
    hardware: H,
    state: State,
    session: String,
    sequence: u64,
    generation: u64,
    dropped: u64,
    reconnect_attempts: u64,
    mcu_timeouts: u64,
    recovery_attempts: u32,
    recovery_started: Instant,
    retry_at: Instant,
    session_started: Instant,
    cleanup_verified: bool,
    pending_resume: Option<Value>,
}
impl<H: ReceiverHardware> Runtime<H> {
    fn new(hardware: H) -> Self {
        let now = Instant::now();
        Self {
            hardware,
            state: State::Recovering,
            session: String::new(),
            sequence: 0,
            generation: 0,
            dropped: 0,
            reconnect_attempts: 0,
            mcu_timeouts: 0,
            recovery_attempts: 0,
            recovery_started: now,
            retry_at: now,
            session_started: now,
            cleanup_verified: true,
            pending_resume: None,
        }
    }
    fn status(&self, request_id: Option<Value>) -> Value {
        let state = match self.state {
            State::Running => "running",
            State::Paused => "paused",
            State::Recovering => "recovering",
        };
        let mut value = json!({"type":"status","state":state,"session_id":self.session,"dropped_observations":self.dropped,"reconnect_attempts":self.reconnect_attempts,"mcu_timeouts":self.mcu_timeouts,"cleanup_verified":self.cleanup_verified});
        if let Some(metrics) = self.hardware.metrics().as_object() {
            for (key, count) in metrics {
                value[key] = count.clone();
            }
        }
        if let Some(id) = request_id {
            value["request_id"] = id;
        }
        value
    }
    fn transition(&mut self, command: Command, out: &Output) -> Result<()> {
        match command.cmd {
            Action::Pause if self.state != State::Paused => {
                self.hardware.shutdown()?;
                self.cleanup_verified = true;
                self.dropped += out.discard_observations();
                self.state = State::Paused;
                self.pending_resume = None;
            }
            Action::Resume if self.state != State::Running => {
                if self.state == State::Paused {
                    self.recovery_started = Instant::now();
                    self.recovery_attempts = 0;
                }
                self.state = State::Recovering;
                self.retry_at = Instant::now();
                // Resume acknowledgement is emitted only after initialization succeeds.
                self.pending_resume = command.request_id;
                if let Err(error) = self.start(out) {
                    self.recover(error, out)?;
                }
                return Ok(());
            }
            _ => {}
        }
        out.send(&self.status(command.request_id), false)?;
        Ok(())
    }
    fn start(&mut self, out: &Output) -> Result<()> {
        self.cleanup_verified = false;
        self.hardware.start()?;
        self.generation += 1;
        self.session = format!(
            "{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            self.generation
        );
        self.sequence = 0;
        self.session_started = Instant::now();
        self.state = State::Running;
        self.recovery_attempts = 0;
        let request_id = self.pending_resume.take();
        out.send(&self.status(request_id), false)?;
        Ok(())
    }
    fn recover(&mut self, error: anyhow::Error, out: &Output) -> Result<()> {
        let mcu_timeout = error
            .chain()
            .any(|e| e.is::<mt7610u_lab::transport::McuTimeout>());
        let disconnected = error
            .chain()
            .any(|e| e.downcast_ref::<rusb::Error>() == Some(&rusb::Error::NoDevice));
        if !mcu_timeout && !disconnected {
            return Err(error);
        }
        self.cleanup_verified = self.hardware.shutdown().is_ok();
        if mcu_timeout {
            self.mcu_timeouts += 1;
            ensure!(
                self.cleanup_verified,
                "MCU timeout recovery requires verified radio shutdown"
            );
            ensure!(
                self.mcu_timeouts < 3,
                "MCU timeout recovery budget exhausted"
            );
        }
        self.dropped += out.discard_observations();
        if self.state != State::Recovering {
            self.recovery_started = Instant::now();
            self.recovery_attempts = 0;
        }
        self.state = State::Recovering;
        self.recovery_attempts += 1;
        self.reconnect_attempts += 1;
        ensure!(
            self.recovery_attempts < MAX_RECOVERY_ATTEMPTS
                && self.recovery_started.elapsed() < RECOVERY_BUDGET,
            "receiver reconnect budget exhausted"
        );
        self.retry_at = Instant::now()
            + Duration::from_millis((250u64 << (self.recovery_attempts - 1)).min(5000));
        out.send(&self.status(None), false)?;
        Ok(())
    }
    fn tick(&mut self, out: &Output) -> Result<()> {
        if self.state == State::Recovering {
            if Instant::now() >= self.retry_at {
                if let Err(error) = self.start(out) {
                    self.recover(error, out)?;
                }
            }
        } else if self.state == State::Running {
            match self.hardware.poll() {
                Ok(records) => {
                    for mut value in records {
                        value["session_id"] = json!(self.session);
                        value["sequence"] = json!(self.sequence);
                        value["elapsed_ms"] = json!(self.session_started.elapsed().as_millis());
                        if out.send(&value, true)? {
                            self.sequence += 1;
                        } else {
                            self.dropped += 1;
                        }
                    }
                }
                Err(error) => self.recover(error, out)?,
            }
        }
        Ok(())
    }
    fn stop(&mut self, out: &Output, request_id: Option<Value>) -> Result<()> {
        let result = self.hardware.shutdown();
        self.cleanup_verified = result.is_ok();
        self.dropped += out.discard_observations();
        let mut stopped = self.status(request_id);
        stopped["type"] = json!("stopped");
        stopped.as_object_mut().unwrap().remove("state");
        let send = out.send(&stopped, false);
        result?;
        send?;
        Ok(())
    }
}

pub(super) fn run(
    firmware_path: PathBuf,
    channels: Vec<u8>,
    dwell_ms: u16,
    max_seconds: u32,
    rotation: Option<super::rotation::Rotation>,
) -> Result<()> {
    super::validate_capture_plan(&channels, false)?;
    let bytes = std::fs::read(firmware_path)?;
    let firmware = Firmware::parse(&bytes)?;
    firmware.require_reference()?;
    let stop = super::cancellation_flag()?;
    let (commands, input_outcome) = command_reader(stop.clone());
    let out = Output::with_rotation(std::io::stdout(), rotation);
    out.send(&json!({"type":"hello","protocol_version":1}), false)?;
    let hardware = Hardware::new(
        firmware,
        channels,
        Duration::from_millis(u64::from(dwell_ms)),
        stop.clone(),
        UsbDevice::open,
    );
    drive(
        hardware,
        commands,
        stop,
        input_outcome,
        out,
        LoopLimits {
            duration: Duration::from_secs(u64::from(max_seconds)),
            status_interval: Duration::from_secs(5),
        },
    )
}

struct LoopLimits {
    duration: Duration,
    status_interval: Duration,
}

fn drive<H: ReceiverHardware>(
    hardware: H,
    commands: Receiver<Command>,
    stop: Arc<AtomicBool>,
    input_outcome: Arc<Mutex<InputOutcome>>,
    out: Output,
    limits: LoopLimits,
) -> Result<()> {
    let mut runtime = Runtime::new(hardware);
    let started = Instant::now();
    let mut last_status = Instant::now();
    let operation = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) && started.elapsed() < limits.duration {
            ensure!(out.healthy(), "stream consumer disconnected");
            // Bound command work per tick; stop/EOF uses an independent atomic flag.
            for _ in 0..16 {
                match commands.try_recv() {
                    Ok(command) => runtime.transition(command, &out)?,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            runtime.tick(&out)?;
            if last_status.elapsed() >= limits.status_interval {
                out.send(&runtime.status(None), false)?;
                last_status = Instant::now();
            }
            if runtime.state != State::Running {
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(())
    })();
    let operation = operation.and(input_outcome.lock().unwrap().result());
    let stop_request_id = input_outcome.lock().unwrap().stop_request_id.clone();
    let cleanup = runtime.stop(&out, stop_request_id);
    let flush = out.finish(Duration::from_secs(1));
    super::finish_with_cleanup(super::finish_with_cleanup(operation, cleanup), flush)
}
fn command_reader(stop: Arc<AtomicBool>) -> (Receiver<Command>, Arc<Mutex<InputOutcome>>) {
    let (tx, rx) = mpsc::sync_channel(16);
    let outcome = Arc::new(Mutex::new(InputOutcome::default()));
    let reader_outcome = outcome.clone();
    thread::spawn(move || read_commands(std::io::stdin().lock(), tx, stop, reader_outcome));
    (rx, outcome)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::output::{Queue, QUEUE_CAPACITY};
    use super::*;
    use std::{io::Write, sync::Condvar};
    #[derive(Default)]
    struct FakeHardware {
        events: Vec<&'static str>,
        records: usize,
        fail_start: bool,
        fail_shutdown: bool,
        fail_poll: bool,
        stop_after_poll: Option<Arc<AtomicBool>>,
    }
    impl ReceiverHardware for FakeHardware {
        fn start(&mut self) -> Result<()> {
            self.events.push("start");
            if self.fail_start {
                return Err(rusb::Error::NoDevice.into());
            }
            Ok(())
        }
        fn shutdown(&mut self) -> Result<()> {
            self.events.push("shutdown");
            ensure!(!self.fail_shutdown, "injected gate verification failure");
            Ok(())
        }
        fn poll(&mut self) -> Result<Vec<Value>> {
            if let Some(stop) = &self.stop_after_poll {
                stop.store(true, Ordering::Relaxed);
            }
            if self.fail_poll {
                return Err(rusb::Error::NoDevice.into());
            }
            Ok((0..std::mem::take(&mut self.records))
                .map(|_| json!({"type":"beacon"}))
                .collect())
        }
    }
    fn queued_output() -> Output {
        Output {
            queue: Arc::new((Mutex::new(Queue::default()), Condvar::new())),
        }
    }
    fn records(out: &Output) -> Vec<Value> {
        out.queue
            .0
            .lock()
            .unwrap()
            .lines
            .iter()
            .map(|l| serde_json::from_slice(&l.bytes).unwrap())
            .collect()
    }
    fn command(cmd: Action, id: u64) -> Command {
        Command {
            cmd,
            request_id: Some(json!(id)),
        }
    }
    #[test]
    fn mcu_timeout_recovery_requires_cleanup_and_has_a_session_wide_limit() {
        let out = Output::new(Vec::new());
        let mut runtime = Runtime::new(FakeHardware::default());
        runtime.start(&out).unwrap();
        let original = runtime.session.clone();
        let error = || {
            anyhow::Error::from(mt7610u_lab::transport::McuTimeout {
                command: 31,
                sequence: 7,
            })
        };
        runtime.recover(error(), &out).unwrap();
        assert!(runtime.cleanup_verified);
        assert_eq!(runtime.state, State::Recovering);
        runtime.start(&out).unwrap();
        assert_ne!(runtime.session, original);
        runtime.recover(error(), &out).unwrap();
        runtime.start(&out).unwrap();
        assert!(runtime
            .recover(error(), &out)
            .unwrap_err()
            .to_string()
            .contains("budget exhausted"));
        let mut runtime = Runtime::new(FakeHardware {
            fail_shutdown: true,
            ..Default::default()
        });
        assert!(runtime
            .recover(error(), &out)
            .unwrap_err()
            .to_string()
            .contains("verified radio shutdown"));
        out.finish(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn event_loop_processes_commands_cancellation_eof_and_input_failure() {
        for input_error in [None, Some("invalid control input")] {
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::sync_channel(16);
            tx.send(command(Action::Pause, 1)).unwrap();
            tx.send(command(Action::Resume, 2)).unwrap();
            tx.send(command(Action::Status, 3)).unwrap();
            let outcome = Arc::new(Mutex::new(InputOutcome {
                error: input_error,
                stop_request_id: Some(json!(4)),
            }));
            let hardware = FakeHardware {
                stop_after_poll: Some(stop.clone()),
                records: 1,
                ..Default::default()
            };
            let result = drive(
                hardware,
                rx,
                stop.clone(),
                outcome,
                Output::new(std::io::sink()),
                LoopLimits {
                    duration: Duration::from_secs(1),
                    status_interval: Duration::ZERO,
                },
            );
            assert_eq!(result.is_err(), input_error.is_some());
            assert!(stop.load(Ordering::Relaxed));
        }
        for duration in [Duration::ZERO, Duration::from_secs(1)] {
            let (tx, rx) = mpsc::sync_channel(1);
            drop(tx);
            drive(
                FakeHardware::default(),
                rx,
                Arc::new(AtomicBool::new(false)),
                Arc::new(Mutex::new(InputOutcome::default())),
                Output::new(std::io::sink()),
                LoopLimits {
                    duration,
                    status_interval: Duration::from_secs(5),
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn pause_verifies_gates_drops_old_queue_and_resume_starts_new_session() {
        let out = queued_output();
        let mut r = Runtime::new(FakeHardware::default());
        r.tick(&out).unwrap();
        let first = r.session.clone();
        r.hardware.records = 2;
        r.tick(&out).unwrap();
        assert_eq!(r.sequence, 2);
        r.transition(command(Action::Pause, 4), &out).unwrap();
        assert_eq!(r.hardware.events, ["start", "shutdown"]);
        assert_eq!(r.state, State::Paused);
        assert_eq!(r.dropped, 2);
        let messages = records(&out);
        assert!(!messages.iter().any(|v| v["type"] == "beacon"));
        assert_eq!(messages.last().unwrap()["cleanup_verified"], true);
        assert_eq!(messages.last().unwrap()["request_id"], 4);
        r.tick(&out).unwrap();
        assert_eq!(r.hardware.events.len(), 2);
        r.transition(command(Action::Resume, 5), &out).unwrap();
        assert_ne!(r.session, first);
        assert_eq!(r.sequence, 0);
        r.hardware.records = 1;
        r.tick(&out).unwrap();
        let messages = records(&out);
        let n = messages.len();
        assert_eq!(messages[n - 2]["state"], "running");
        assert_eq!(messages[n - 2]["request_id"], 5);
        assert_eq!(messages[n - 1]["sequence"], 0);
        r.stop(&out, None).unwrap();
        assert_eq!(records(&out).last().unwrap()["cleanup_verified"], true);
    }
    #[test]
    fn failed_pause_never_acknowledges_paused_and_failed_stop_reports_unverified() {
        let out = queued_output();
        let mut r = Runtime::new(FakeHardware::default());
        r.tick(&out).unwrap();
        r.hardware.fail_shutdown = true;
        assert!(r.transition(command(Action::Pause, 1), &out).is_err());
        assert!(!records(&out).iter().any(|v| v["state"] == "paused"));
        assert!(r.stop(&out, None).is_err());
        assert_eq!(records(&out).last().unwrap()["cleanup_verified"], false);
    }
    #[test]
    fn output_backpressure_has_bounded_memory_and_consecutive_accepted_sequences() {
        let out = queued_output();
        let mut r = Runtime::new(FakeHardware::default());
        r.tick(&out).unwrap();
        r.hardware.records = 100;
        r.tick(&out).unwrap();
        assert_eq!(r.sequence, 47);
        assert_eq!(r.dropped, 53);
        let obs: Vec<_> = records(&out)
            .into_iter()
            .filter(|v| v["type"] == "beacon")
            .collect();
        for (i, v) in obs.iter().enumerate() {
            assert_eq!(v["sequence"], i);
        }
        out.queue.0.lock().unwrap().lines.clear();
        r.hardware.records = 1;
        r.tick(&out).unwrap();
        assert_eq!(records(&out)[0]["sequence"], 47);
        for _ in 0..QUEUE_CAPACITY - 1 {
            out.send(&json!({"type":"status"}), false).unwrap();
        }
        assert!(out.send(&json!({}), false).is_err());
        assert_eq!(out.queue.0.lock().unwrap().lines.len(), QUEUE_CAPACITY);
    }
    #[test]
    fn unplug_recovers_with_new_session_and_bounded_attempts() {
        let out = queued_output();
        let mut r = Runtime::new(FakeHardware::default());
        r.tick(&out).unwrap();
        let first = r.session.clone();
        r.hardware.fail_poll = true;
        r.tick(&out).unwrap();
        assert_eq!(r.state, State::Recovering);
        r.hardware.fail_poll = false;
        r.retry_at = Instant::now();
        r.tick(&out).unwrap();
        assert_eq!(r.state, State::Running);
        assert_ne!(r.session, first);
        r.hardware.fail_poll = true;
        r.tick(&out).unwrap();
        r.hardware.fail_start = true;
        for _ in 1..MAX_RECOVERY_ATTEMPTS - 1 {
            r.retry_at = Instant::now();
            r.tick(&out).unwrap();
        }
        r.retry_at = Instant::now();
        assert!(r
            .tick(&out)
            .unwrap_err()
            .to_string()
            .contains("budget exhausted"));
    }
    #[test]
    fn commands_are_bounded_and_eof_stop_cannot_be_blocked_by_a_full_queue() {
        for input in [
            vec![b'x'; 10000],
            b"{\"cmd\":\"invalid\"}\n".to_vec(),
            b"{\"cmd\":\"pause\",\"request_id\":{}}\n".to_vec(),
        ] {
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, _rx) = mpsc::sync_channel(1);
            let mut cursor = std::io::Cursor::new(input);
            let outcome = Arc::new(Mutex::new(InputOutcome::default()));
            read_commands(&mut cursor, tx, stop.clone(), outcome.clone());
            assert!(outcome.lock().unwrap().result().is_err());
            assert!(stop.load(Ordering::Relaxed));
            assert!(cursor.position() <= 513);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(1);
        read_commands(
            std::io::Cursor::new(b"{\"cmd\":\"pause\"}\n{\"cmd\":\"resume\"}\n"),
            tx,
            stop.clone(),
            Arc::new(Mutex::new(InputOutcome::default())),
        );
        assert!(stop.load(Ordering::Relaxed));
        assert!(matches!(rx.recv().unwrap().cmd, Action::Pause));
    }
    #[test]
    fn stop_echoes_request_id_but_eof_has_no_error_or_identifier() {
        for (input, id) in [
            (
                b"{\"cmd\":\"stop\",\"request_id\":42}\n".as_slice(),
                Some(json!(42)),
            ),
            (b"".as_slice(), None),
        ] {
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, _rx) = mpsc::sync_channel(1);
            let outcome = Arc::new(Mutex::new(InputOutcome::default()));
            read_commands(
                std::io::Cursor::new(input),
                tx,
                stop.clone(),
                outcome.clone(),
            );
            assert!(stop.load(Ordering::Relaxed));
            let outcome = outcome.lock().unwrap();
            assert!(outcome.result().is_ok());
            assert_eq!(outcome.stop_request_id, id);
            let out = queued_output();
            let mut r = Runtime::new(FakeHardware::default());
            r.stop(&out, outcome.stop_request_id.clone()).unwrap();
            assert_eq!(records(&out).last().unwrap().get("request_id"), id.as_ref());
        }
    }
    #[test]
    fn control_read_failure_remains_failure_after_successful_cleanup() {
        struct BrokenInput;
        impl std::io::Read for BrokenInput {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let outcome = Arc::new(Mutex::new(InputOutcome::default()));
        let (tx, _rx) = mpsc::sync_channel(1);
        read_commands(
            std::io::BufReader::new(BrokenInput),
            tx,
            Arc::new(AtomicBool::new(false)),
            outcome.clone(),
        );
        let out = queued_output();
        let mut r = Runtime::new(FakeHardware::default());
        let cleanup = r.stop(&out, None);
        assert!(
            super::super::finish_with_cleanup(outcome.lock().unwrap().result(), cleanup).is_err()
        );
        assert_eq!(records(&out).last().unwrap()["cleanup_verified"], true);
    }
    struct BlockedWriter {
        gate: Arc<(Mutex<bool>, Condvar)>,
    }
    impl Write for BlockedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let (lock, wake) = &*self.gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = wake.wait(open).unwrap();
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn blocked_stdout_cannot_block_cleanup_and_shutdown_flush_is_bounded() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let out = Output::new(BlockedWriter { gate: gate.clone() });
        let mut r = Runtime::new(FakeHardware::default());
        r.tick(&out).unwrap();
        r.stop(&out, None).unwrap();
        assert_eq!(r.hardware.events, ["start", "shutdown"]);
        assert!(out.finish(Duration::from_millis(10)).is_err());
        *gate.0.lock().unwrap() = true;
        gate.1.notify_one();
        out.finish(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn broken_stdout_and_non_disconnect_errors_are_fatal() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let out = Output::new(Broken);
        out.send(&json!({}), false).unwrap();
        assert!(out.finish(Duration::from_secs(1)).is_err());
        let out = queued_output();
        let mut r = Runtime::new(FakeHardware::default());
        assert!(r.recover(rusb::Error::Access.into(), &out).is_err());
    }
}
