//! Bounded, dedicated SQLite writer. Enqueue is nonblocking; finish is a durable
//! barrier and must be called outside an async executor thread.
use super::{
    invalid, json, RecordInput, RecordingMode, Store, MAX_BATCH_RECORDS, MAX_RECORD_BYTES,
};
use serde_json::Value;
use std::{
    io,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
};

/// At most 128 queued records, plus one transaction batch, each bounded to 128 KiB.
const QUEUE_RECORDS: usize = 128;
enum Command {
    Record(RecordInput),
    Transition {
        paused: bool,
        ack: mpsc::SyncSender<io::Result<()>>,
    },
    Finish(bool),
}
type Failure = Arc<Mutex<Option<String>>>;

pub struct Writer {
    pub run_id: String,
    sender: Option<mpsc::SyncSender<Command>>,
    worker: Option<JoinHandle<io::Result<()>>>,
    failure: Failure,
}

impl Writer {
    /// Queue a lifecycle barrier without blocking ingestion. The returned
    /// acknowledgement is sent only after the transition transaction commits.
    pub fn transition(&self, paused: bool) -> io::Result<mpsc::Receiver<io::Result<()>>> {
        check(&self.failure)?;
        let (ack, received) = mpsc::sync_channel(1);
        let result = self
            .sender
            .as_ref()
            .expect("live writer")
            .try_send(Command::Transition { paused, ack })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "recording queue full during transition",
                ),
                mpsc::TrySendError::Disconnected(_) => io::Error::other("recording writer stopped"),
            });
        if let Err(error) = &result {
            self.fail(&error.to_string());
        }
        result?;
        Ok(received)
    }
    pub(crate) fn fail(&self, message: &str) {
        remember(&self.failure, message.to_owned());
    }
    /// Returns only after the run and its source metadata are committed. No
    /// receiver should be started before this succeeds.
    pub fn start(
        path: PathBuf,
        mode: RecordingMode,
        config: Value,
        sources: Vec<(String, Value)>,
    ) -> io::Result<Self> {
        Self::start_request(path, mode, config, sources, None)
    }
    pub fn start_request(
        path: PathBuf,
        mode: RecordingMode,
        config: Value,
        sources: Vec<(String, Value)>,
        request: Option<(String, Value)>,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_RECORDS);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let failure: Failure = Arc::default();
        let worker_failure = failure.clone();
        let worker = thread::Builder::new()
            .name("airhound-sqlite".into())
            .spawn(move || {
                let result = (|| {
                    let mut store = Store::open(&path)?;
                    // Validate metadata before creating a run so bad input cannot
                    // leave a partially configured recording behind.
                    let mut ids = std::collections::HashSet::new();
                    for (id, metadata) in &sources {
                        super::identifier(id)?;
                        json(metadata, 65536)?;
                        if !ids.insert(id) {
                            return Err(invalid("duplicate recording source"));
                        }
                    }
                    let run = store.start_request(
                        mode,
                        &config,
                        request.as_ref().map(|(id, value)| (id.as_str(), value)),
                    )?;
                    for (id, metadata) in sources {
                        if let Err(error) = store.add_source(&run, &id, &metadata) {
                            let _ =
                                store.transition(&run, "failed", "source initialization failed");
                            return Err(error);
                        }
                    }
                    if ready_tx.send(run.clone()).is_err() {
                        return Ok(());
                    }
                    let result = consume(&mut store, &run, receiver, &worker_failure);
                    if let Err(error) = &result {
                        // Full storage can prevent even the terminal transaction.
                        // The capture actor retries while idle; restart recovery
                        // marks an unknown unclosed run interrupted, never finished.
                        let _ = store.transition(
                            &run,
                            "failed",
                            &super::failure_reason(&error.to_string()),
                        );
                    }
                    result
                })();
                if let Err(error) = &result {
                    remember(&worker_failure, error.to_string());
                }
                result
            })?;
        let run_id = match ready_rx.recv() {
            Ok(id) => id,
            Err(_) => {
                return match worker.join() {
                    Ok(Err(error)) => Err(error),
                    _ => Err(io::Error::other("recording writer failed during startup")),
                };
            }
        };
        Ok(Self {
            run_id,
            sender: Some(sender),
            worker: Some(worker),
            failure,
        })
    }

    /// Acceptance means queued, not committed. Any rejection permanently fails
    /// this recording, so a caller cannot ignore a gap and later report success.
    pub fn append(&self, record: RecordInput) -> io::Result<()> {
        let result = (|| {
            check(&self.failure)?;
            super::identifier(&record.source_id)?;
            json(&record.payload, MAX_RECORD_BYTES)?;
            if record.received_at_us < 0
                || !record
                    .payload
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| !kind.is_empty() && kind.len() <= 128)
            {
                return Err(invalid("invalid recording timestamp or type"));
            }
            self.sender
                .as_ref()
                .expect("live writer")
                .try_send(Command::Record(record))
                .map_err(|error| match error {
                    mpsc::TrySendError::Full(_) => io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "recording queue full; capture must stop",
                    ),
                    mpsc::TrySendError::Disconnected(_) => {
                        io::Error::other("recording writer stopped")
                    }
                })
        })();
        if let Err(error) = &result {
            remember(&self.failure, error.to_string());
        }
        result
    }

    /// Call after receivers have stopped and their output is drained. A failed
    /// capture still drains accepted records, but is never marked finished.
    pub fn finish(mut self, capture_succeeded: bool) -> io::Result<()> {
        let sender = self.sender.take().expect("live writer");
        let sent = sender.send(Command::Finish(capture_succeeded));
        drop(sender);
        self.worker
            .take()
            .expect("live worker")
            .join()
            .map_err(|_| io::Error::other("recording writer panicked"))??;
        sent.map_err(|_| io::Error::other("recording writer stopped before finish"))?;
        check(&self.failure)
    }
}

// Dropping without finish closes the queue. The worker drains accepted records
// but leaves the run open for interruption recovery; Drop cannot certify finish.
fn remember(failure: &Failure, message: String) {
    failure
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_or_insert(message);
}
fn check(failure: &Failure) -> io::Result<()> {
    match &*failure.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(message) => Err(io::Error::other(message.clone())),
        None => Ok(()),
    }
}
fn consume(
    store: &mut Store,
    run: &str,
    receiver: mpsc::Receiver<Command>,
    failure: &Failure,
) -> io::Result<()> {
    let mut pending = None;
    loop {
        let command = pending.take().or_else(|| receiver.recv().ok());
        match command {
            Some(Command::Record(first)) => {
                let mut batch = vec![first];
                while batch.len() < MAX_BATCH_RECORDS {
                    match receiver.try_recv() {
                        Ok(Command::Record(record)) => batch.push(record),
                        Ok(other) => {
                            pending = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                store.append(run, &batch)?;
            }
            Some(Command::Finish(succeeded)) => {
                let healthy = check(failure);
                let state = if succeeded && healthy.is_ok() {
                    "finished"
                } else {
                    "failed"
                };
                store.transition(run, state, "capture stopped; accepted records committed")?;
                return healthy;
            }
            Some(Command::Transition { paused, ack }) => {
                let result = store.transition(
                    run,
                    if paused { "paused" } else { "recording" },
                    if paused {
                        "pause requested; receiver stop acknowledgement is separate"
                    } else {
                        "resume requested; receiver readiness is separate"
                    },
                );
                let reply = result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| io::Error::new(error.kind(), error.to_string()));
                let _ = ack.send(reply);
                result?;
            }
            None => return Ok(()),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn queue_overflow_is_permanent_even_after_capacity_returns() {
        // Keep the consumer stopped to test saturation without scheduler timing.
        let (sender, receiver) = mpsc::sync_channel(1);
        let writer = Writer {
            run_id: "test".into(),
            sender: Some(sender),
            worker: None,
            failure: Arc::default(),
        };
        let record = || RecordInput {
            source_id: "a".into(),
            received_at_us: 0,
            payload: serde_json::json!({"type":"observation"}),
        };
        writer.append(record()).unwrap();
        assert_eq!(
            writer.append(record()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(matches!(receiver.recv().unwrap(), Command::Record(_)));
        assert!(writer.append(record()).is_err());
        assert!(receiver.try_recv().is_err());
    }
}
