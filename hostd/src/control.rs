//! Per-session scanner state shared by commands and radio adapters.
use crate::health::{Health, Phase, Scanner, Snapshot};
use std::sync::{atomic::Ordering, Arc, Mutex};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub enabled: bool,
    pub shutdown: bool,
    pub generation: u64,
}

#[derive(Clone)]
pub struct ScanControl(watch::Sender<State>, Arc<Mutex<Snapshot>>);

impl ScanControl {
    pub fn new(enabled: bool) -> Self {
        Self(
            watch::channel(State {
                enabled,
                shutdown: false,
                generation: 0,
            })
            .0,
            Arc::new(Mutex::new(Snapshot::default())),
        )
    }
    pub fn health(&self) -> Snapshot {
        *self.1.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn set_health(&self, scanner: Scanner, state: Phase, reason: &'static str) {
        let mut snapshot = self.1.lock().unwrap_or_else(|e| e.into_inner());
        let health = Health { state, reason };
        match scanner {
            Scanner::Ble => snapshot.ble = health,
            Scanner::Wifi => snapshot.wifi = health,
        }
    }
    pub fn health_message(&self) -> serde_json::Value {
        serde_json::json!({"type":"scanner_health", "schema_version":1,
            "scan_requested":self.load(Ordering::Relaxed), "scanners":self.health()})
    }
    pub fn load(&self, _: Ordering) -> bool {
        self.0.borrow().enabled
    }
    pub fn store(&self, enabled: bool, _: Ordering) {
        self.0.send_if_modified(|state| {
            if state.shutdown || state.enabled == enabled {
                return false;
            }
            let mut health = self.1.lock().unwrap_or_else(|e| e.into_inner());
            let health = &mut *health;
            for scanner in [&mut health.ble, &mut health.wifi] {
                if !enabled && matches!(scanner.state, Phase::Running | Phase::Initializing) {
                    *scanner = Health {
                        state: Phase::Paused,
                        reason: "pause_requested",
                    };
                }
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
    #[test]
    fn requested_state_does_not_hide_permission_or_adapter_failures() {
        let control = ScanControl::new(true);
        control.set_health(
            Scanner::Wifi,
            Phase::Blocked,
            "location_permission_denied_or_disabled",
        );
        control.set_health(Scanner::Ble, Phase::Running, "scan_started");
        control.store(false, Ordering::Relaxed);
        assert_eq!(control.health().wifi.state, Phase::Blocked);
        assert_eq!(control.health().ble.state, Phase::Paused);
        control.store(true, Ordering::Relaxed);
        assert_eq!(control.health().wifi.state, Phase::Blocked);
        // Only the adapter can confirm recovery, independently of user intent.
        control.set_health(Scanner::Wifi, Phase::Running, "scan_completed");
        let message = control.health_message();
        assert_eq!(message["scan_requested"], true);
        assert_eq!(message["scanners"]["wifi"]["state"], "running");
        control.set_health(Scanner::Ble, Phase::Unavailable, "adapter_failed");
        control.store(false, Ordering::Relaxed);
        assert_eq!(control.health().ble.state, Phase::Unavailable);
    }

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
