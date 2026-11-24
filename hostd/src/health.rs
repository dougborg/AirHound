//! Adapter health is distinct from the user's requested scanning state.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Disabled,
    Initializing,
    Running,
    Paused,
    #[cfg(any(target_os = "macos", test))]
    Blocked,
    Unavailable,
    Stopped,
}
#[derive(Clone, Copy)]
pub enum Scanner {
    Cellular,
    Ble,
    Wifi,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Health {
    pub state: Phase,
    pub reason: &'static str,
}
impl Default for Health {
    fn default() -> Self {
        Self {
            state: Phase::Initializing,
            reason: "not_started",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub cellular: Health,
    pub ble: Health,
    pub wifi: Health,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            ble: Health::default(),
            wifi: Health::default(),
            cellular: Health {
                state: Phase::Disabled,
                reason: "not_assigned",
            },
        }
    }
}
