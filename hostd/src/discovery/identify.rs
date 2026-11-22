//! Explicit, bounded Sniffle identity query. Passive inventory never calls this.
use super::{discover, Inventory, Options};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::PathBuf,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, process::Command};

const OUTPUT_LIMIT: u64 = 4096;
const DEADLINE: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol: String,
    pub device_id: String,
    pub port: String,
    pub python: PathBuf,
    pub helper: PathBuf,
    pub module_path: Option<PathBuf>,
}

/// Values actually reported by the helper's firmware query, never expected versions.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Firmware {
    #[serde(rename = "type")]
    kind: String,
    schema_version: u8,
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
    pub api_level: u16,
}
impl Firmware {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > OUTPUT_LIMIT as usize {
            return Err(invalid("identity response exceeds 4 KiB"));
        }
        let value: Self = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        if value.kind != "sniffle_identity" || value.schema_version != 1 {
            return Err(invalid("unrecognized Sniffle identity response"));
        }
        Ok(value)
    }
    pub fn compatible(&self) -> bool {
        (self.major, self.minor, self.patch, self.api_level) == (1, 11, 0, 0)
    }
    pub fn version(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Identification {
    pub schema_version: u8,
    pub device_id: String,
    pub endpoint: String,
    pub observed_at_ms: u64,
    pub provenance: &'static str,
    pub state: &'static str,
    pub message: String,
    pub application: Option<&'static str>,
    pub firmware_version: Option<String>,
    pub protocol_api: Option<u16>,
    pub compatible: Option<bool>,
    pub reception: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<serde_json::Value>,
}
impl Identification {
    pub fn new(request: &Request, state: &'static str, message: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            device_id: request.device_id.clone(),
            endpoint: request.port.clone(),
            observed_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            provenance: "explicit_sniffle_query",
            state,
            message: message.into(),
            application: None,
            firmware_version: None,
            protocol_api: None,
            compatible: None,
            reception: "not_tested",
            owner: None,
        }
    }
    /// Reuse only the current run's owner snapshot. This does no I/O and makes
    /// no historical-cache or RF readiness claim.
    pub fn from_active_owner(request: &Request, status: &serde_json::Value) -> Self {
        let mut result = Self::new(
            request,
            "busy",
            "Capture is active; no extra device endpoint was opened",
        );
        result.provenance = "active_owner_metadata";
        if status["active"] != true {
            return result;
        }
        let owner = status["receiver_health"]["receivers"]
            .as_array()
            .and_then(|receivers| {
                receivers.iter().find(|owner| {
                    owner["receiver"]["adapter_id"] == format!("sniffle:{}", request.port)
                })
            });
        if let Some(owner) = owner {
            result.state = "in_use";
            result.message =
                "Using metadata from the active receiver; no new firmware query was performed"
                    .into();
            if let Ok(bytes) = serde_json::to_vec(&owner["receiver"]["firmware"]) {
                if let Ok(firmware) = Firmware::parse(&bytes) {
                    result.apply_firmware(&firmware);
                }
            }
            result.owner = Some(serde_json::json!({"run_id":status["run_id"],"receiver":owner}));
        }
        result
    }
    pub fn apply_firmware(&mut self, firmware: &Firmware) {
        self.application = Some("Sniffle");
        self.firmware_version = Some(firmware.version());
        self.protocol_api = Some(firmware.api_level);
        self.compatible = Some(firmware.compatible());
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl Request {
    pub fn validate(&self, inventory: &Inventory) -> io::Result<()> {
        if self.protocol != "sniffle" {
            return Err(invalid(
                "only explicit Sniffle identification is implemented",
            ));
        }
        if self.device_id.is_empty()
            || self.device_id.len() > 512
            || self.port.is_empty()
            || self.port.len() > 4096
            || self.python.as_os_str().is_empty()
            || self.helper.as_os_str().is_empty()
        {
            return Err(invalid(
                "identity query needs a device, endpoint, Python and helper",
            ));
        }
        let device = inventory
            .devices
            .iter()
            .find(|device| device.id == self.device_id)
            .ok_or_else(|| {
                invalid("device is no longer in the current inventory; refresh discovery")
            })?;
        if !device
            .candidates
            .iter()
            .any(|candidate| candidate.kind == "sniffle")
        {
            return Err(invalid("Sniffle identity query requires a recognized Sniffle candidate; generic UARTs are not probed"));
        }
        if !device
            .endpoints
            .iter()
            .any(|endpoint| endpoint.kind == "serial" && endpoint.path == self.port)
        {
            return Err(invalid(
                "selected serial endpoint does not belong to this device",
            ));
        }
        Ok(())
    }
}

/// The caller must serialize this operation against capture start/stop/other probes.
/// Helpers and Python paths are explicitly user-configured executables, like capture helpers.
pub async fn query(request: &Request) -> Identification {
    let inventory = discover(&Options::default()).await;
    if let Err(error) = request.validate(&inventory) {
        return Identification::new(request, "unavailable", error.to_string());
    }
    let mut command = Command::new(&request.python);
    command
        .arg(&request.helper)
        .arg("--identify-only")
        .arg("--port")
        .arg(&request.port);
    if let Some(path) = &request.module_path {
        command.arg("--module-path").arg(path);
    }
    let mut result = Identification::new(request, "unknown", "No firmware identity returned");
    match read_identity(command, DEADLINE).await {
        Ok(firmware) => {
            result.state = if firmware.compatible() {
                "identified"
            } else {
                "incompatible"
            };
            result.message = if firmware.compatible() {
                "Firmware/API matches the supported Sniffle contract; reception has not been tested"
            } else {
                "Firmware was identified but this source requires Sniffle 1.11.0 / API0"
            }
            .into();
            result.apply_firmware(&firmware);
        }
        Err(error) => {
            result.state = match error.kind() {
                io::ErrorKind::PermissionDenied => "permission_denied",
                io::ErrorKind::TimedOut => "timed_out",
                _ => "failed",
            };
            result.message = error.to_string();
        }
    }
    result
}

async fn read_identity(mut command: Command, deadline: Duration) -> io::Result<Firmware> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let result = tokio::time::timeout(deadline, async {
        let mut bytes = Vec::new();
        child.stdout.take().expect("piped stdout").take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes).await?;
        if bytes.len() > OUTPUT_LIMIT as usize { return Err(invalid("identity response exceeds 4 KiB")); }
        if !child.wait().await?.success() {
            return Err(io::Error::other("Sniffle identity helper failed; check device ownership, permissions, protocol and Python dependencies"));
        }
        Firmware::parse(&bytes)
    }).await.unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "Sniffle identity query timed out")));
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn firmware_versions_are_reported_not_assumed_compatible() {
        for (minor, api, compatible) in [(11, 0, true), (10, 0, false), (11, 1, false)] {
            let data = format!(
                r#"{{"type":"sniffle_identity","schema_version":1,"major":1,"minor":{minor},"patch":0,"api_level":{api}}}"#
            );
            let firmware = Firmware::parse(data.as_bytes()).unwrap();
            assert_eq!(firmware.compatible(), compatible);
            assert_eq!(firmware.version(), format!("1.{minor}.0"));
        }
        for bad in [
            b"{}".as_slice(),
            b"null",
            b"{\"type\":\"status\"}",
            &[b' '; 4097],
        ] {
            assert!(Firmware::parse(bad).is_err());
        }
    }
    #[tokio::test]
    async fn helper_timeout_output_bounds_exit_and_malformed_output_are_rejected() {
        for (script, timeout, kind) in [
            (
                "sleep 1",
                Duration::from_millis(10),
                io::ErrorKind::TimedOut,
            ),
            ("printf '{}'", DEADLINE, io::ErrorKind::Other),
            ("exit 2", DEADLINE, io::ErrorKind::Other),
            ("yes abc", DEADLINE, io::ErrorKind::InvalidInput),
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            assert_eq!(
                read_identity(command, timeout).await.unwrap_err().kind(),
                kind
            );
        }
    }
}
