//! Record the same complete NDJSON emitted by the canonical capture pipeline.
use super::{now_us, writer::Writer, RecordInput, MAX_RECORD_BYTES};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

/// Session-level records have no physical receiver. This is a logical source,
/// never an inferred device or radio identity.
pub const SESSION_SOURCE: &str = "__airhound_session__";
type Shared = Arc<Mutex<Option<Writer>>>;
pub struct Completion(Shared);
pub struct Output<W> {
    downstream: W,
    writer: Shared,
    pending: Vec<u8>,
}
pub fn attach<W: Write>(downstream: W, writer: Writer) -> (Output<W>, Completion) {
    let shared = Arc::new(Mutex::new(Some(writer)));
    (
        Output {
            downstream,
            writer: shared.clone(),
            pending: Vec::new(),
        },
        Completion(shared),
    )
}
impl Completion {
    /// Blocking durable barrier; call only after the producer has stopped.
    pub fn finish(self, succeeded: bool) -> io::Result<()> {
        let writer = self
            .0
            .lock()
            .map_err(|_| io::Error::other("recording lock poisoned"))?
            .take()
            .ok_or_else(|| io::Error::other("recording already closed"))?;
        writer.finish(succeeded)
    }
}
impl<W> Output<W> {
    fn fail(&self) {
        if let Ok(guard) = self.writer.lock() {
            if let Some(writer) = guard.as_ref() {
                writer.fail("recording output failed or ended with an incomplete record");
            }
        }
    }
}
impl<W: Write> Write for Output<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = (|| {
            for chunk in bytes.split_inclusive(|byte| *byte == b'\n') {
                if self.pending.len() + chunk.len() > MAX_RECORD_BYTES + 1 {
                    return Err(io::Error::other("recording line exceeds size limit"));
                }
                self.pending.extend_from_slice(chunk);
                if chunk.last() == Some(&b'\n') {
                    let payload: serde_json::Value =
                        serde_json::from_slice(&self.pending).map_err(io::Error::other)?;
                    let source = match payload.get("session_source") {
                        None => SESSION_SOURCE,
                        Some(value) => value
                            .get("receiver_id")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| io::Error::other("invalid session source identity"))?,
                    }
                    .to_owned();
                    let record = RecordInput {
                        source_id: source,
                        received_at_us: now_us()?,
                        payload,
                    };
                    {
                        let guard = self
                            .writer
                            .lock()
                            .map_err(|_| io::Error::other("recording lock poisoned"))?;
                        guard
                            .as_ref()
                            .ok_or_else(|| io::Error::other("recording already closed"))?
                            .append(record)?;
                    }
                    self.downstream.write_all(&self.pending)?;
                    self.pending.clear();
                }
            }
            Ok(bytes.len())
        })();
        if result.is_err() {
            self.fail();
        }
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        let result = if self.pending.is_empty() {
            self.downstream.flush()
        } else {
            Err(io::Error::other("incomplete recording line"))
        };
        if result.is_err() {
            self.fail();
        }
        result
    }
}
impl<W> Drop for Output<W> {
    fn drop(&mut self) {
        if !self.pending.is_empty() {
            self.fail();
        }
    }
}
