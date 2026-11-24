//! Test-only paused-clock guard for real I/O. Tokio 1.54 documents that a
//! running spawn_blocking task inhibits automatic time advancement. Hold one
//! until the fixture finishes, so OS readiness cannot race virtual deadlines.
//! No polling/yield count or elapsed wall time decides test correctness.
#[must_use]
pub struct ManualTime {
    release: Option<std::sync::mpsc::Sender<()>>,
    worker: Option<tokio::task::JoinHandle<()>>,
}
impl ManualTime {
    pub async fn hold() -> Self {
        let (release, wait) = std::sync::mpsc::channel();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            if matches!(
                wait.recv_timeout(std::time::Duration::from_secs(120)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                // Never let a stuck fixture resume auto-time and accidentally
                // satisfy a correctness deadline. This is an outer hang guard.
                eprintln!("ManualTime fixture exceeded its 120s wall-clock hang guard");
                std::process::abort();
            }
        });
        let guard = Self {
            release: Some(release),
            worker: Some(worker),
        };
        ready.await.expect("clock guard started");
        guard
    }
    pub async fn finish(mut self) {
        self.release.take();
        self.worker.take().unwrap().await.unwrap();
    }
}
impl Drop for ManualTime {
    fn drop(&mut self) {
        // Also unblock the worker if the fixture panics or is cancelled.
        self.release.take();
    }
}
