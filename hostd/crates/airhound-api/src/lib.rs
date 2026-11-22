//! Shared presentation-independent control contract. No radios or policy logic.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_CONFIG_BYTES: usize = 65_536;
pub const METHODS: &[&str] = &[
    "hello",
    "status",
    "devices.list",
    "session.validate",
    "recordings.list",
    "recordings.records",
    "capture",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Command {
    #[serde(rename = "capture")]
    Capture { action: CaptureAction },
    #[serde(rename = "hello")]
    Hello {},
    #[serde(rename = "status")]
    Status {},
    #[serde(rename = "devices.list")]
    DevicesList {
        #[serde(default)]
        all: bool,
    },
    #[serde(rename = "session.validate")]
    SessionValidate { config: Value },
    #[serde(rename = "recordings.list")]
    RecordingsList { before: Option<String>, limit: u16 },
    #[serde(rename = "recordings.records")]
    RecordingRecords {
        run_id: String,
        after: i64,
        limit: u16,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureAction {
    Start { request_id: String, config: Value },
    Pause { run_id: String },
    Resume { run_id: String },
    Finish { run_id: String },
    Status,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    /// Nonzero, strictly increasing within a connection; zero is reserved for uncorrelated errors.
    pub request_id: u64,
    pub command: Command,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    IncompatibleVersion,
    HandshakeRequired,
    UnsupportedMethod,
    InvalidSession,
    Timeout,
    Internal,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}
impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
        }
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for ApiError {}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u16,
    pub request_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}
impl Response {
    pub fn success(request_id: u64, result: Value) -> Self {
        Self {
            version: VERSION,
            request_id,
            result: Some(result),
            error: None,
        }
    }
    pub fn failure(request_id: u64, error: ApiError) -> Self {
        Self {
            version: VERSION,
            request_id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub version: u16,
    pub instance_id: String,
    pub build_version: String,
    pub max_frame_bytes: usize,
    pub methods: Vec<String>,
    pub mode: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_without_radio_dependencies() {
        for command in [
            Command::Hello {},
            Command::Status {},
            Command::DevicesList { all: true },
            Command::SessionValidate {
                config: serde_json::json!({"version":1}),
            },
        ] {
            let request = Request {
                version: VERSION,
                request_id: 1,
                command,
            };
            let encoded = serde_json::to_vec(&request).unwrap();
            assert_eq!(
                serde_json::from_slice::<Request>(&encoded).unwrap(),
                request
            );
        }
    }
    #[test]
    fn unknown_commands_and_fields_are_not_ignored() {
        assert!(serde_json::from_value::<Command>(
            serde_json::json!({"method":"session.start","params":{}})
        )
        .is_err());
        assert!(serde_json::from_value::<Command>(
            serde_json::json!({"method":"devices.list","params":{"all":false,"start":true}})
        )
        .is_err());
        assert!(serde_json::from_value::<Request>(serde_json::json!({"version":1,"request_id":1,"command":{"method":"hello","params":{}},"unknown":true})).is_err());
    }
    #[test]
    fn result_and_error_constructors_are_exclusive() {
        let ok = Response::success(7, serde_json::json!({}));
        assert!(ok.result.is_some() && ok.error.is_none());
        let error = Response::failure(8, ApiError::new(ErrorCode::InvalidSession, "invalid"));
        assert!(error.result.is_none() && error.error.is_some());
        assert_eq!(
            serde_json::from_value::<Response>(serde_json::to_value(&error).unwrap()).unwrap(),
            error
        );
    }
}
