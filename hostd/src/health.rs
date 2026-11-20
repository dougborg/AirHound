//! Adapter health is distinct from the user's requested scanning state.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Disabled,
    Initializing,
    Running,
    Paused,
    Blocked,
    Unavailable,
    Stopped,
}
#[derive(Clone, Copy)]
pub enum Scanner {
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub ble: Health,
    pub wifi: Health,
}
