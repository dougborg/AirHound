//! Bounded stdin framing and correlated control commands.
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::BufRead,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
};
const MAX_COMMAND: usize = 512;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Command {
    pub(super) cmd: Action,
    #[serde(default)]
    pub(super) request_id: Option<Value>,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Action {
    Pause,
    Resume,
    Stop,
    Status,
}

#[derive(Default)]
pub(super) struct InputOutcome {
    pub(super) stop_request_id: Option<Value>,
    pub(super) error: Option<&'static str>,
}
impl InputOutcome {
    pub(super) fn result(&self) -> Result<()> {
        if let Some(error) = self.error {
            anyhow::bail!(error);
        }
        Ok(())
    }
}
pub(super) fn read_commands(
    mut input: impl BufRead,
    tx: mpsc::SyncSender<Command>,
    stop: Arc<AtomicBool>,
    outcome: Arc<Mutex<InputOutcome>>,
) {
    let terminate = |error, request_id| {
        *outcome.lock().unwrap() = InputOutcome {
            error,
            stop_request_id: request_id,
        };
        stop.store(true, Ordering::Relaxed);
    };
    loop {
        // take() bounds even a stream with no newline; no unbounded read_line.
        let mut line = Vec::new();
        let n = match std::io::Read::take(&mut input, (MAX_COMMAND + 1) as u64)
            .read_until(b'\n', &mut line)
        {
            Ok(n) => n,
            Err(_) => {
                terminate(Some("stream control input read failed"), None);
                return;
            }
        };
        if n == 0 {
            terminate(None, None);
            return;
        }
        if n > MAX_COMMAND {
            terminate(Some("stream control command exceeds 512 bytes"), None);
            return;
        }
        if !line.ends_with(b"\n") {
            terminate(Some("stream control command is truncated"), None);
            return;
        }
        let command = match serde_json::from_slice::<Command>(&line) {
            Ok(c)
                if c.request_id
                    .as_ref()
                    .is_none_or(|v| v.is_string() || v.is_u64()) =>
            {
                c
            }
            _ => {
                terminate(Some("invalid stream control command"), None);
                return;
            }
        };
        if matches!(command.cmd, Action::Stop) {
            terminate(None, command.request_id);
            return;
        }
        if tx.try_send(command).is_err() {
            terminate(Some("stream control queue exhausted or closed"), None);
            return;
        }
    }
}
