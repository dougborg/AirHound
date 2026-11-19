//! Per-session scanner state shared by commands and radio adapters.
use std::sync::atomic::Ordering;
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub enabled: bool,
    pub shutdown: bool,
    pub generation: u64,
}

#[derive(Clone)]
pub struct ScanControl(watch::Sender<State>);

impl ScanControl {
    pub fn new(enabled: bool) -> Self {
        Self(
            watch::channel(State {
                enabled,
                shutdown: false,
                generation: 0,
            })
            .0,
        )
    }
    pub fn load(&self, _: Ordering) -> bool {
        self.0.borrow().enabled
    }
    pub fn store(&self, enabled: bool, _: Ordering) {
        self.0.send_if_modified(|state| {
            if state.shutdown || state.enabled == enabled {
                return false;
            }
            state.enabled = enabled;
            state.generation = state.generation.wrapping_add(1);
            true
        });
    }
    pub fn shutdown(&self) {
        self.0.send_modify(|state| {
            state.shutdown = true;
            state.generation = state.generation.wrapping_add(1);
        });
    }
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.0.subscribe()
    }
}

/// Wait without polling while paused; closed output or shutdown terminates scanning.
pub async fn wait_running<T>(state: &mut watch::Receiver<State>, tx: &mpsc::Sender<T>) -> bool {
    loop {
        let current = *state.borrow_and_update();
        if current.shutdown || tx.is_closed() {
            return false;
        }
        if current.enabled {
            return true;
        }
        tokio::select! {
            _ = tx.closed() => return false,
            result = state.changed() => if result.is_err() { return false; },
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pause_resume_shutdown_and_consumer_close() {
        let control = ScanControl::new(false);
        let (tx, rx) = mpsc::channel::<()>(1);
        let mut state = control.subscribe();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            wait_running(&mut state, &tx)
        )
        .await
        .is_err());
        control.store(true, Ordering::Relaxed);
        assert!(wait_running(&mut state, &tx).await);
        let generation = state.borrow().generation;
        control.store(true, Ordering::Relaxed);
        assert_eq!(state.borrow().generation, generation);
        control.shutdown();
        control.store(false, Ordering::Relaxed);
        assert!(!wait_running(&mut state, &tx).await);
        let control = ScanControl::new(true);
        drop(rx);
        assert!(!wait_running(&mut control.subscribe(), &tx).await);
    }
}
