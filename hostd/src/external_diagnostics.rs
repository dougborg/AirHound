//! Optional bounded companion diagnostics. This MIT protocol decoder does not link driver code.
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_BYTES: usize = 2048;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Diagnostics {
    pub last_failure: Option<Failure>,
    pub last_cleanup_failure: Option<Failure>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Failure {
    operation: Operation,
    kind: Kind,
    preparation_stage: Option<Stage>,
    firmware_uploaded_in_failed_preparation: Option<bool>,
    usb_error: Option<UsbError>,
    mcu_command: Option<u8>,
    mcu_sequence: Option<u8>,
    mcu: Option<Exchange>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Initialize,
    Receive,
    PauseShutdown,
    RecoveryShutdown,
    StopShutdown,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    McuTimeout,
    Usb,
    McuExchange,
    Other,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    FirmwareLoad,
    InitializeChannel,
    InitialRetune,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum UsbError {
    Io,
    InvalidParam,
    Access,
    NoDevice,
    NotFound,
    Busy,
    Timeout,
    Overflow,
    Pipe,
    Interrupted,
    NoMem,
    NotSupported,
    BadDescriptor,
    Other,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Exchange {
    command: u8,
    sequence: u8,
    output_endpoint: u8,
    input_endpoint: u8,
    write_completed: bool,
    read_attempts: u32,
    read_timeouts: u32,
    replies: u32,
    reply_bytes: u64,
    stale_sequences: u32,
    short_replies: u32,
    error_events: u32,
    last_reply_sequence: Option<u8>,
    last_reply_event: Option<u8>,
}
impl Diagnostics {
    /// Unknown fields are discarded by typed decoding; never forward opaque JSON.
    pub fn parse(value: &Value) -> Option<Self> {
        if serde_json::to_vec(value).ok()?.len() > MAX_BYTES {
            return None;
        }
        let result: Self = serde_json::from_value(value.clone()).ok()?;
        for failure in [&result.last_failure, &result.last_cleanup_failure]
            .into_iter()
            .flatten()
        {
            if failure.mcu_command.is_some_and(|c| c >= 128)
                || failure.mcu_sequence.is_some_and(|s| s >= 16)
            {
                return None;
            }
            if let Some(m) = &failure.mcu {
                if m.command >= 128
                    || m.sequence >= 16
                    || !(1..=15).contains(&m.output_endpoint)
                    || !(0x81..=0x8f).contains(&m.input_endpoint)
                    || m.last_reply_sequence.is_some_and(|s| s >= 16)
                    || m.last_reply_event.is_some_and(|e| e >= 16)
                    || m.read_timeouts.saturating_add(m.replies) > m.read_attempts
                    || m.stale_sequences > m.replies
                    || m.short_replies > m.replies
                    || m.error_events > m.replies
                    || m.reply_bytes > u64::from(m.replies) * 1024
                    || failure.mcu_command != Some(m.command)
                    || failure.mcu_sequence != Some(m.sequence)
                {
                    return None;
                }
            }
        }
        Some(result)
    }
}

/// A malformed optional extension never bypasses process cleanup or hides its primary error.
#[derive(Default)]
pub struct Snapshot {
    pub value: Option<Box<Diagnostics>>,
    pub invalid: bool,
}
impl Snapshot {
    pub fn observe(&mut self, message: &Value) {
        if let Some(value) = message.get("diagnostics_v1") {
            match Diagnostics::parse(value) {
                Some(value) => self.value = Some(Box::new(value)),
                None => self.invalid = true,
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    fn sample() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/external-diagnostics-v1.json"
        ))
        .unwrap()
    }
    #[test]
    fn optional_diagnostics_are_bounded_typed_and_never_forward_opaque_fields() {
        let mut value = sample();
        value["raw_reply"] = serde_json::json!("private");
        let result = Diagnostics::parse(&value).unwrap();
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(serialized, sample());
        assert!(serialized.get("raw_reply").is_none());
        for (pointer, bad) in [
            ("/last_failure/operation", serde_json::json!("arbitrary")),
            ("/last_failure/mcu_command", serde_json::json!(256)),
            ("/last_failure/mcu/input_endpoint", serde_json::json!(2)),
            ("/last_failure/mcu/read_timeouts", serde_json::json!(7)),
            ("/last_failure/mcu/reply_bytes", serde_json::json!(1025)),
            (
                "/last_failure/mcu/last_reply_sequence",
                serde_json::json!(16),
            ),
        ] {
            let mut value = sample();
            *value.pointer_mut(pointer).unwrap() = bad;
            assert!(Diagnostics::parse(&value).is_none(), "{pointer}");
        }
        let mut saturated = sample();
        saturated["last_failure"]["mcu"]["read_attempts"] = serde_json::json!(u32::MAX);
        saturated["last_failure"]["mcu"]["read_timeouts"] = serde_json::json!(u32::MAX);
        assert!(Diagnostics::parse(&saturated).is_some());
        let mut value = sample();
        value["padding"] = serde_json::json!("x".repeat(MAX_BYTES));
        assert!(Diagnostics::parse(&value).is_none());
    }
    #[test]
    fn missing_optional_metadata_is_compatible_and_malformed_evidence_is_explicit() {
        let mut snapshot = Snapshot::default();
        snapshot.observe(&serde_json::json!({"state":"running"}));
        assert!(snapshot.value.is_none());
        assert!(!snapshot.invalid);
        snapshot.observe(&serde_json::json!({"diagnostics_v1":sample()}));
        snapshot.observe(&serde_json::json!({"diagnostics_v1":"bad"}));
        assert!(snapshot.invalid);
        assert!(snapshot.value.is_some());
        snapshot.observe(&serde_json::json!({"type":"stopped"}));
        assert!(snapshot.invalid);
        assert!(snapshot.value.is_some());
    }
}
