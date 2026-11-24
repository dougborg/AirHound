//! Bounded output queue; blocking writes never hold the lifecycle lock.
use anyhow::{ensure, Result};
use serde_json::Value;
use std::{
    collections::VecDeque,
    io::Write,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::Duration,
};
pub(super) const QUEUE_CAPACITY: usize = 64;
const OBSERVATION_CAPACITY: usize = 48;
const MAX_LINE: usize = 16 * 1024;
pub(super) struct Queued {
    pub(super) bytes: Vec<u8>,
    observation: bool,
}
#[derive(Default)]
pub(super) struct Queue {
    pub(super) lines: VecDeque<Queued>,
    closing: bool,
    failed: bool,
    done: bool,
}
type FinishWait = for<'a> fn(&Condvar, MutexGuard<'a, Queue>, Duration) -> MutexGuard<'a, Queue>;

fn timed_wait<'a>(
    wake: &Condvar,
    queue: MutexGuard<'a, Queue>,
    timeout: Duration,
) -> MutexGuard<'a, Queue> {
    wake.wait_timeout_while(queue, timeout, |q| !q.done)
        .unwrap()
        .0
}

#[cfg(test)]
pub(super) fn completed_wait<'a>(
    wake: &Condvar,
    queue: MutexGuard<'a, Queue>,
    _: Duration,
) -> MutexGuard<'a, Queue> {
    wake.wait_while(queue, |q| !q.done).unwrap()
}

pub(super) struct Output {
    pub(super) finish_wait: FinishWait,
    pub(super) queue: Arc<(Mutex<Queue>, Condvar)>,
}
impl Output {
    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(super) fn new(writer: impl Write + Send + 'static) -> Self {
        let mut output = Self::with_rotation(writer, None);
        output.finish_wait = completed_wait;
        output
    }
    pub(super) fn with_rotation(
        writer: impl Write + Send + 'static,
        rotation: Option<crate::rotation::Rotation>,
    ) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker = queue.clone();
        thread::spawn(move || write_output(writer, worker, rotation));
        Self {
            queue,
            finish_wait: timed_wait,
        }
    }
    pub(super) fn send(&self, value: &Value, observation: bool) -> Result<bool> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_LINE, "stream record exceeds 16 KiB");
        let (lock, wake) = &*self.queue;
        let mut q = lock.lock().unwrap();
        ensure!(!q.failed && !q.closing, "stream output closed");
        if observation && q.lines.len() >= OBSERVATION_CAPACITY {
            return Ok(false);
        }
        ensure!(
            q.lines.len() < QUEUE_CAPACITY,
            "stream control queue exhausted"
        );
        q.lines.push_back(Queued { bytes, observation });
        wake.notify_one();
        Ok(true)
    }
    pub(super) fn discard_observations(&self) -> u64 {
        let mut q = self.queue.0.lock().unwrap();
        let before = q.lines.len();
        q.lines.retain(|line| !line.observation);
        (before - q.lines.len()) as u64
    }
    pub(super) fn healthy(&self) -> bool {
        !self.queue.0.lock().unwrap().failed
    }
    pub(super) fn finish(&self, timeout: Duration) -> Result<()> {
        self.finish_with_wait(timeout, self.finish_wait)
    }
    pub(super) fn finish_with_wait(&self, timeout: Duration, wait: FinishWait) -> Result<()> {
        let (lock, wake) = &*self.queue;
        let mut q = lock.lock().unwrap();
        q.closing = true;
        wake.notify_one();
        let q = wait(wake, q, timeout);
        ensure!(
            q.done && !q.failed,
            "stream output did not finish within bounded shutdown deadline"
        );
        Ok(())
    }
}
fn write_output(
    mut writer: impl Write,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    mut rotation: Option<crate::rotation::Rotation>,
) {
    let (lock, wake) = &*queue;
    loop {
        let line = {
            let mut q = lock.lock().unwrap();
            while q.lines.is_empty() && !q.closing {
                q = wake.wait(q).unwrap();
            }
            match q.lines.pop_front() {
                Some(line) => line,
                None => {
                    drop(q);
                    let failed = rotation.as_mut().is_some_and(|r| r.finish().is_err());
                    let mut q = lock.lock().unwrap();
                    q.failed |= failed;
                    q.done = true;
                    wake.notify_all();
                    return;
                }
            }
        };
        // Never hold the queue lock across a potentially blocking write.
        let recording = if line.observation {
            rotation
                .as_mut()
                .map_or(Ok(()), |r| r.observation(&line.bytes))
        } else {
            Ok(())
        };
        if recording.is_err()
            || writer
                .write_all(&line.bytes)
                .and_then(|()| writer.flush())
                .is_err()
        {
            let mut q = lock.lock().unwrap();
            q.failed = true;
            q.done = true;
            q.lines.clear();
            wake.notify_all();
            return;
        }
    }
}
