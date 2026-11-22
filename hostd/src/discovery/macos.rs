//! Read-only USB registry inventory. No device node is opened and no radio is started.
use super::{Diagnostic, Endpoint, RawDevice};
use plist::{Dictionary, Value};
use std::{collections::BTreeMap, io, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const NODE_LIMIT: usize = 16_384;
const DEPTH_LIMIT: usize = 128;

pub(super) async fn collect() -> (Vec<RawDevice>, Vec<Diagnostic>) {
    let mut command = Command::new("/usr/sbin/ioreg");
    command.args(["-a", "-l", "-r", "-c", "IOUSBHostDevice"]);
    match read_command(command, OUTPUT_LIMIT, Duration::from_secs(5)).await {
        Ok(bytes) => match parse(&bytes) {
            Ok(result) => result,
            Err(error) => (
                Vec::new(),
                vec![diagnostic(format!("invalid registry response: {error}"))],
            ),
        },
        Err(error) => (
            Vec::new(),
            vec![diagnostic(format!(
                "USB registry inventory unavailable: {error}"
            ))],
        ),
    }
}

async fn read_command(
    mut command: Command,
    limit: usize,
    deadline: Duration,
) -> io::Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let result = tokio::time::timeout(deadline, async {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .expect("piped stdout")
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > limit {
            return Err(io::Error::other("registry output exceeds inventory bound"));
        }
        if !child.wait().await?.success() {
            return Err(io::Error::other("registry command failed"));
        }
        Ok(bytes)
    })
    .await;
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        result => {
            let _ = child.kill().await;
            match result {
                Ok(Err(error)) => Err(error),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "registry command timed out",
                )),
                Ok(Ok(_)) => unreachable!(),
            }
        }
    }
}

fn diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        backend: "macos_ioreg".into(),
        message,
    }
}

fn number(dict: &Dictionary, key: &str) -> Option<u64> {
    dict.get(key)?.as_unsigned_integer()
}

fn text<'a>(dict: &'a Dictionary, key: &str) -> Option<&'a str> {
    dict.get(key)?.as_string()
}

fn parse(bytes: &[u8]) -> Result<(Vec<RawDevice>, Vec<Diagnostic>), plist::Error> {
    let value = Value::from_reader(std::io::Cursor::new(bytes))?;
    let mut inventory = Inventory::default();
    match value.as_array() {
        Some(roots) => {
            for root in roots {
                inventory.visit(root, None, 0);
            }
        }
        None => inventory
            .diagnostics
            .push(diagnostic("registry root is not an array".into())),
    }
    Ok((
        inventory.devices.into_values().collect(),
        inventory.diagnostics,
    ))
}

#[derive(Default)]
struct Inventory {
    devices: BTreeMap<String, RawDevice>,
    diagnostics: Vec<Diagnostic>,
    nodes: usize,
    truncated: bool,
}

impl Inventory {
    fn visit(&mut self, node: &Value, parent: Option<&str>, depth: usize) {
        if self.nodes >= NODE_LIMIT || depth > DEPTH_LIMIT {
            if !self.truncated {
                self.diagnostics.push(diagnostic(
                    "registry traversal limit reached; inventory is incomplete".into(),
                ));
                self.truncated = true;
            }
            return;
        }
        self.nodes += 1;
        let Some(dict) = node.as_dictionary() else {
            return;
        };
        let is_device = matches!(
            text(dict, "IOObjectClass"),
            Some("IOUSBHostDevice" | "IOUSBDevice")
        );
        let identity = if is_device {
            // Location IDs describe this USB topology, not permanent device identity.
            // A registry fallback is only valid during this attachment/boot.
            let id = number(dict, "locationID")
                .filter(|id| *id != 0)
                .map(|id| format!("macos:usb:location:{id:08x}"))
                .or_else(|| {
                    number(dict, "IORegistryEntryID").map(|id| format!("macos:usb:registry:{id:x}"))
                });
            if let Some(id) = id.as_ref() {
                self.devices.entry(id.clone()).or_insert_with(|| RawDevice {
                    id: id.clone(),
                    name: text(dict, "USB Product Name")
                        .or_else(|| text(dict, "IORegistryEntryName"))
                        .unwrap_or("USB device")
                        .chars()
                        .take(256)
                        .collect(),
                    vid: number(dict, "idVendor").and_then(|n| u16::try_from(n).ok()),
                    pid: number(dict, "idProduct").and_then(|n| u16::try_from(n).ok()),
                    endpoints: vec![Endpoint {
                        kind: "usb".into(),
                        path: id.clone(),
                    }],
                });
            } else {
                self.diagnostics.push(diagnostic(
                    "USB registry entry has no usable identity; skipped".into(),
                ));
            }
            id
        } else {
            parent.map(str::to_owned)
        };
        if text(dict, "IOObjectClass") == Some("IOSerialBSDClient") {
            if let Some(device) = identity.as_ref().and_then(|id| self.devices.get_mut(id)) {
                for (key, kind, prefix) in [
                    ("IOCalloutDevice", "serial", "/dev/cu."),
                    ("IODialinDevice", "serial_alias", "/dev/tty."),
                ] {
                    if let Some(path) = text(dict, key).filter(|s| {
                        s.starts_with(prefix) && s.len() <= 1024 && !s.chars().any(char::is_control)
                    }) {
                        if !device
                            .endpoints
                            .iter()
                            .any(|e| e.kind == kind && e.path == path)
                        {
                            device.endpoints.push(Endpoint {
                                kind: kind.into(),
                                path: path.into(),
                            });
                        }
                    }
                }
            }
        }
        if let Some(children) = dict
            .get("IORegistryEntryChildren")
            .and_then(Value::as_array)
        {
            for child in children {
                self.visit(child, identity.as_deref(), depth + 1);
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn node(class: &str, fields: &[(&str, Value)], children: Vec<Value>) -> Value {
        let mut dict = Dictionary::new();
        dict.insert("IOObjectClass".into(), class.into());
        for (key, value) in fields {
            dict.insert((*key).into(), value.clone());
        }
        dict.insert("IORegistryEntryChildren".into(), Value::Array(children));
        Value::Dictionary(dict)
    }
    fn device(location: u64, children: Vec<Value>) -> Value {
        node(
            "IOUSBHostDevice",
            &[
                ("locationID", location.into()),
                ("idVendor", 0x2e8au64.into()),
                ("idProduct", 0xc0u64.into()),
                ("USB Product Name", "Pico".into()),
            ],
            children,
        )
    }
    fn serial() -> Value {
        node(
            "IOSerialBSDClient",
            &[
                ("IOCalloutDevice", "/dev/cu.fixture".into()),
                ("IODialinDevice", "/dev/tty.fixture".into()),
            ],
            vec![],
        )
    }
    fn encoded(value: Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes).unwrap();
        bytes
    }
    #[test]
    fn groups_serial_aliases_under_nearest_device_and_deduplicates_roots() {
        let child = device(
            2,
            vec![node("IOUSBHostInterface", &[], vec![serial(), serial()])],
        );
        let parent = device(1, vec![child.clone()]);
        let (devices, diagnostics) = parse(&encoded(Value::Array(vec![parent, child]))).unwrap();
        assert!(diagnostics.is_empty());
        assert_eq!(devices.len(), 2);
        assert_eq!(
            devices[0].endpoints.len(),
            1,
            "hub must not own child serial ports"
        );
        assert_eq!(devices[1].endpoints.len(), 3);
        assert_eq!(devices[1].vid, Some(0x2e8a));
        assert_eq!(devices[1].pid, Some(0xc0));
        assert_eq!(devices[1].endpoints[1].kind, "serial");
        assert_eq!(devices[1].endpoints[2].kind, "serial_alias");
    }
    #[test]
    fn fallback_identity_missing_fields_and_binary_properties_are_safe() {
        let fallback = node(
            "IOUSBHostDevice",
            &[
                ("IORegistryEntryID", 42u64.into()),
                ("idVendor", u64::MAX.into()),
                ("USB Serial Number", "do-not-expose".into()),
                ("binary", Value::Data(vec![0, 1, 2])),
            ],
            vec![serial()],
        );
        let missing = node("IOUSBHostDevice", &[], vec![serial()]);
        let (devices, diagnostics) =
            parse(&encoded(Value::Array(vec![fallback, missing]))).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "macos:usb:registry:2a");
        assert_eq!(devices[0].name, "USB device");
        assert_eq!(devices[0].vid, None);
        assert_eq!(diagnostics.len(), 1);
        assert!(!devices[0].name.contains("do-not-expose"));
    }
    #[test]
    fn malformed_and_wrong_root_are_reported() {
        assert!(parse(b"broken").is_err());
        let (devices, diagnostics) = parse(&encoded(Value::String("wrong".into()))).unwrap();
        assert!(devices.is_empty());
        assert_eq!(diagnostics.len(), 1);
    }
    #[test]
    fn traversal_is_bounded_and_serial_paths_are_validated() {
        let bad = node(
            "IOSerialBSDClient",
            &[
                ("IOCalloutDevice", "/dev/cu.bad\npath".into()),
                ("IODialinDevice", "/tmp/not-a-port".into()),
            ],
            vec![],
        );
        let (devices, _) = parse(&encoded(Value::Array(vec![device(1, vec![bad])]))).unwrap();
        assert_eq!(devices[0].endpoints.len(), 1);
        let mut inventory = Inventory::default();
        inventory.visit(&device(1, vec![]), None, DEPTH_LIMIT + 1);
        inventory.nodes = NODE_LIMIT;
        inventory.visit(&Value::String("ignored".into()), None, 0);
        assert!(inventory.devices.is_empty());
        assert_eq!(inventory.diagnostics.len(), 1);
    }
    #[tokio::test]
    async fn subprocess_reader_limits_time_size_and_failures() {
        let mut cmd = Command::new("/usr/bin/printf");
        cmd.arg("valid");
        assert_eq!(
            read_command(cmd, 10, Duration::from_secs(1)).await.unwrap(),
            b"valid"
        );
        let mut cmd = Command::new("/usr/bin/printf");
        cmd.arg("too-long");
        assert!(read_command(cmd, 2, Duration::from_secs(1))
            .await
            .unwrap_err()
            .to_string()
            .contains("bound"));
        assert!(read_command(
            Command::new("/nonexistent/airhound-test"),
            10,
            Duration::from_secs(1)
        )
        .await
        .is_err());
        assert!(
            read_command(Command::new("/usr/bin/false"), 10, Duration::from_secs(1))
                .await
                .unwrap_err()
                .to_string()
                .contains("failed")
        );
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("5");
        assert_eq!(
            read_command(cmd, 10, Duration::from_millis(20))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
