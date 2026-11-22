//! Toolkit-independent configuration for concurrent, explicitly selected receivers.
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    pub version: u8,
    /// Signature alert policy for this session; absence preserves the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert_policy: Option<airhound::evidence::AlertPolicy>,
    pub receivers: Vec<ReceiverConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverConfig {
    pub id: String,
    pub source: Source,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    NativeBle {
        adapter: Option<String>,
    },
    NativeWifi {},
    LinuxWifi {
        interface: String,
    },
    /// Explicit attachment to firmware that already emits AirHound NDJSON.
    /// Opening a UART bridge may reset its board; never selected by passive discovery.
    AirhoundSerial {
        port: String,
        baud: u32,
    },
    Sniffle {
        port: String,
        python: PathBuf,
        helper: PathBuf,
        module_path: Option<PathBuf>,
        channel: Option<u8>,
        #[serde(default)]
        coded: bool,
    },
    ExternalWifi {
        #[serde(default)]
        usb_selector: Option<String>,
        executable: PathBuf,
        firmware: PathBuf,
        channels: Vec<u8>,
        dwell_ms: u16,
    },
    BlePcap {
        path: PathBuf,
    },
    WifiPcap {
        path: PathBuf,
    },
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

// macOS exposes callout and dial-in nodes for the same UART. Canonicalization
// alone cannot identify these aliases because both are device nodes.
fn serial_resource(path: &std::path::Path) -> String {
    let name = path.to_string_lossy();
    let serial = name
        .strip_prefix("/dev/cu.")
        .or_else(|| name.strip_prefix("/dev/tty."));
    match serial {
        Some(serial) => format!("serial:macos:{serial}"),
        None => format!("serial:{name}"),
    }
}

impl SessionConfig {
    /// Explain incomplete discovery hints before typed deserialization obscures the field.
    pub fn from_json(value: serde_json::Value) -> io::Result<Self> {
        if let Some(receivers) = value.get("receivers").and_then(|v| v.as_array()) {
            for (index, receiver) in receivers.iter().enumerate() {
                let source = &receiver["source"];
                let fields: &[&str] = match source["kind"].as_str() {
                    Some("sniffle") => &["port", "python", "helper"],
                    Some("airhound_serial") => &["port"],
                    Some("external_wifi") => &["executable", "firmware"],
                    Some("wifi_pcap" | "ble_pcap") => &["path"],
                    _ => &[],
                };
                let missing: Vec<_> = fields
                    .iter()
                    .filter(|field| {
                        source.get(**field).is_none_or(|v| {
                            v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty())
                        })
                    })
                    .copied()
                    .collect();
                if !missing.is_empty() {
                    return Err(invalid(&format!(
                        "Receiver {} ({}) setup incomplete: configure {} before validating or starting capture.",
                        index + 1, source["kind"].as_str().unwrap_or("unknown"), missing.join(", ")
                    )));
                }
            }
        }
        serde_json::from_value(value).map_err(io::Error::other)
    }

    /// Validate the entire plan before any radio starts. Unpinned Sniffle sources
    /// use the least-loaded primary advertising channel (37 first on ties),
    /// counting all pinned receivers before assigning automatic ones in list order.
    /// Pins may intentionally overlap; more than three receivers also overlap.
    /// This is a requested fixed assignment, not a promise of gap-free RF coverage.
    pub fn resolve(mut self) -> io::Result<Self> {
        if self.version != 1 || self.receivers.is_empty() || self.receivers.len() > 16 {
            return Err(invalid("session version must be 1 with 1..=16 receivers"));
        }
        let mut ids = HashSet::new();
        let mut resources = HashSet::new();
        let mut external_count = 0;
        let mut default_external = false;
        let mut channel_load = [0usize; 3];
        for receiver in &self.receivers {
            if let Source::Sniffle {
                channel: Some(channel),
                ..
            } = &receiver.source
            {
                if !(37..=39).contains(channel) {
                    return Err(invalid("Sniffle primary channel must be 37..=39"));
                }
                channel_load[usize::from(*channel - 37)] += 1;
            }
        }
        let mut default_ble = false;
        let mut native_ble_count = 0;
        for receiver in &mut self.receivers {
            if receiver.id.is_empty()
                || receiver.id.len() > 64
                || !receiver
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                || !ids.insert(receiver.id.clone())
            {
                return Err(invalid(
                    "receiver IDs must be unique ASCII names (1..=64 letters, digits, -_.)",
                ));
            }
            let resource = match &mut receiver.source {
                Source::NativeBle { adapter } => {
                    native_ble_count += 1;
                    default_ble |= adapter.is_none();
                    if adapter.as_ref().is_some_and(|id| id.trim().is_empty()) {
                        return Err(invalid("empty BLE adapter selection"));
                    }
                    format!("ble:{}", adapter.as_deref().unwrap_or("default"))
                }
                Source::NativeWifi {} => "native_wifi".into(),
                Source::AirhoundSerial { port, baud } => {
                    if port.is_empty()
                        || port.len() > 1024
                        || ![115200, 460800, 921600].contains(baud)
                    {
                        return Err(invalid(
                            "AirHound serial requires a port and baud 115200, 460800 or 921600",
                        ));
                    }
                    let canonical =
                        std::fs::canonicalize(&*port).unwrap_or_else(|_| PathBuf::from(&*port));
                    serial_resource(&canonical)
                }
                Source::LinuxWifi { interface } => {
                    if interface.is_empty()
                        || interface.len() > 15
                        || interface == "."
                        || interface == ".."
                        || !interface
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    {
                        return Err(invalid("invalid monitor interface"));
                    }
                    format!("interface:{interface}")
                }
                Source::Sniffle {
                    port,
                    python,
                    helper,
                    channel,
                    ..
                } => {
                    if port.is_empty()
                        || python.as_os_str().is_empty()
                        || helper.as_os_str().is_empty()
                    {
                        return Err(invalid("Sniffle port, Python and helper are required"));
                    }
                    if channel.is_none() {
                        let index = (0..3).min_by_key(|&i| channel_load[i]).unwrap();
                        *channel = Some(37 + index as u8);
                        channel_load[index] += 1;
                    }
                    if !matches!(channel, Some(37..=39)) {
                        return Err(invalid("Sniffle primary channel must be 37..=39"));
                    }
                    // Resolve existing aliases, so two symlinks cannot open one port.
                    let canonical =
                        std::fs::canonicalize(&*port).unwrap_or_else(|_| PathBuf::from(&*port));
                    serial_resource(&canonical)
                }
                Source::ExternalWifi {
                    usb_selector,
                    executable,
                    firmware,
                    channels,
                    dwell_ms,
                } => {
                    external_count += 1;
                    default_external |= usb_selector.is_none();
                    if usb_selector
                        .as_deref()
                        .is_some_and(|s| !crate::external_wifi::valid_usb_selector(s))
                    {
                        return Err(invalid("invalid canonical USB selector"));
                    }
                    if executable.as_os_str().is_empty()
                        || firmware.as_os_str().is_empty()
                        || !(100..=10_000).contains(dwell_ms)
                        || channels.is_empty()
                        || channels.len() > 38
                        || channels
                            .iter()
                            .any(|c| !crate::external_wifi::supported_receive_channel(*c))
                        || channels.iter().collect::<HashSet<_>>().len() != channels.len()
                    {
                        return Err(invalid(
                            "invalid external WiFi executable, firmware, channel plan or dwell",
                        ));
                    }
                    format!(
                        "external_mt7610u:{}",
                        usb_selector.as_deref().unwrap_or("default")
                    )
                }
                Source::BlePcap { path } | Source::WifiPcap { path } => {
                    if path.as_os_str().is_empty() {
                        return Err(invalid("empty replay path"));
                    }
                    format!("replay:{}", path.display())
                }
            };
            if !resources.insert(resource) {
                return Err(invalid("a receiver resource is selected more than once"));
            }
        }
        if external_count > 1 && default_external {
            return Err(invalid(
                "multiple external WiFi receivers require explicit distinct USB selectors",
            ));
        }
        if native_ble_count > 1 && default_ble {
            return Err(invalid(
                "multiple native BLE receivers require explicit distinct adapter IDs",
            ));
        }
        Ok(self)
    }
}

impl Source {
    pub fn is_ble(&self) -> bool {
        matches!(
            self,
            Self::NativeBle { .. } | Self::Sniffle { .. } | Self::BlePcap { .. }
        )
    }
    pub fn shutdown_budget(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if matches!(self, Self::ExternalWifi { .. }) {
            40
        } else {
            5
        })
    }
    /// Requested configuration only. Actual radio state comes from backend health.
    pub fn assignment(&self) -> serde_json::Value {
        match self {
            Self::NativeBle { .. } | Self::NativeWifi {} => {
                serde_json::json!({"mode":"os_managed"})
            }
            Self::LinuxWifi { .. } => {
                serde_json::json!({"mode":"externally_configured","channel":null})
            }
            Self::AirhoundSerial { baud, .. } => serde_json::json!({
                "mode":"device_managed", "media":["wifi","ble"], "baud":baud,
                "pause":"host_ingestion_only", "stop":"close_transport_only",
                "port_open_may_reset_board":true
            }),
            Self::Sniffle { channel, coded, .. } => {
                serde_json::json!({"mode":"fixed_primary","channel":channel,"phy":if *coded {"coded"} else {"1m"},"secondary_following":true})
            }
            Self::ExternalWifi {
                channels,
                dwell_ms,
                usb_selector,
                ..
            } => {
                serde_json::json!({"mode":"channel_plan","channels":channels,"dwell_ms":dwell_ms,"usb_selector":usb_selector})
            }
            Self::BlePcap { .. } | Self::WifiPcap { .. } => serde_json::json!({"mode":"replay"}),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(sources: Vec<Source>) -> SessionConfig {
        SessionConfig {
            version: 1,
            alert_policy: None,
            receivers: sources
                .into_iter()
                .enumerate()
                .map(|(i, source)| ReceiverConfig {
                    id: format!("receiver-{i}"),
                    source,
                })
                .collect(),
        }
    }
    fn sniffle(port: &str, channel: Option<u8>) -> Source {
        Source::Sniffle {
            port: port.into(),
            python: "python3".into(),
            helper: "helper.py".into(),
            module_path: None,
            channel,
            coded: false,
        }
    }
    fn external() -> Source {
        Source::ExternalWifi {
            usb_selector: None,
            executable: "driver".into(),
            firmware: "firmware.bin".into(),
            channels: vec![1, 6, 11],
            dwell_ms: 100,
        }
    }
    fn invalid_plan(config: SessionConfig) {
        assert_eq!(
            config.resolve().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn multiple_external_receivers_require_distinct_canonical_selectors() {
        let selected = |selector: &str, channel| {
            let mut source = external();
            if let Source::ExternalWifi {
                usb_selector,
                channels,
                ..
            } = &mut source
            {
                *usb_selector = Some(selector.into());
                *channels = vec![channel];
            }
            source
        };
        let resolved = plan(vec![selected("usb:0:1.2", 1), selected("usb:0:1.3", 6)])
            .resolve()
            .unwrap();
        assert_eq!(
            resolved.receivers[0].source.assignment()["channels"],
            serde_json::json!([1])
        );
        assert_eq!(
            resolved.receivers[1].source.assignment()["usb_selector"],
            "usb:0:1.3"
        );
        invalid_plan(plan(vec![external(), selected("usb:1:2", 6)]));
        invalid_plan(plan(vec![selected("usb:1:2", 1), selected("usb:1:2", 6)]));
        for selector in [
            "",
            "usb:01:2",
            "usb:1:02",
            "usb:1:0",
            "usb:1:1.2.3.4.5.6.7.8",
            "bus:1:2",
            "usb:256:1",
        ] {
            invalid_plan(plan(vec![selected(selector, 1)]));
        }
    }

    #[test]
    fn documented_session_examples_resolve() {
        for example in [
            include_str!("../../docs/examples/session-linux.json"),
            include_str!("../../docs/examples/session-alfa-multi.json"),
            include_str!("../../docs/examples/session-macos-sniffle.json"),
        ] {
            let config: SessionConfig = serde_json::from_str(example).unwrap();
            assert!(config.resolve().is_ok());
        }
    }

    #[test]
    fn automatic_channels_account_for_pins_even_when_pins_are_last() {
        let resolved = plan(vec![
            sniffle("auto-a", None),
            sniffle("auto-b", None),
            sniffle("pin", Some(37)),
            sniffle("auto-c", None),
        ])
        .resolve()
        .unwrap();
        let channels: Vec<_> = resolved
            .receivers
            .iter()
            .map(|r| r.source.assignment()["channel"].as_u64().unwrap())
            .collect();
        assert_eq!(channels, [38, 39, 37, 37]);
        let serialized = serde_json::to_value(&resolved).unwrap();
        assert_eq!(
            serde_json::to_value(resolved.resolve().unwrap()).unwrap(),
            serialized
        );
    }

    #[test]
    fn intentional_overlap_is_preserved_and_auto_receivers_balance() {
        let resolved = plan(vec![
            sniffle("pin-a", Some(39)),
            sniffle("pin-b", Some(39)),
            sniffle("auto-a", None),
            sniffle("auto-b", None),
            sniffle("auto-c", None),
            sniffle("auto-d", None),
        ])
        .resolve()
        .unwrap();
        let channels: Vec<_> = resolved
            .receivers
            .iter()
            .map(|r| r.source.assignment()["channel"].as_u64().unwrap())
            .collect();
        assert_eq!(channels, [39, 39, 37, 38, 37, 38]);
    }

    #[test]
    fn version_receiver_count_and_id_bounds() {
        invalid_plan(plan(vec![]));
        let mut wrong_version = plan(vec![Source::NativeWifi {}]);
        wrong_version.version = 2;
        invalid_plan(wrong_version);
        let replay_sources = |n| {
            (0..n)
                .map(|i| Source::BlePcap {
                    path: format!("{i}.pcap").into(),
                })
                .collect()
        };
        assert!(plan(replay_sources(16)).resolve().is_ok());
        invalid_plan(plan(replay_sources(17)));
        for id in [
            "".to_owned(),
            "a".repeat(65),
            "has space".into(),
            "ü".into(),
            "slash/id".into(),
        ] {
            let mut config = plan(vec![Source::NativeWifi {}]);
            config.receivers[0].id = id;
            invalid_plan(config);
        }
        let mut config = plan(vec![Source::NativeWifi {}]);
        config.receivers[0].id = format!("A0-_.{}", "x".repeat(59));
        assert!(config.resolve().is_ok());
        let mut duplicate = plan(vec![
            Source::NativeWifi {},
            Source::NativeBle { adapter: None },
        ]);
        duplicate.receivers[1].id = duplicate.receivers[0].id.clone();
        invalid_plan(duplicate);
    }

    #[test]
    fn duplicate_resources_are_rejected_independently_of_receiver_names() {
        for source in [
            Source::NativeWifi {},
            Source::NativeBle {
                adapter: Some("hci0".into()),
            },
            Source::LinuxWifi {
                interface: "mon0".into(),
            },
            sniffle("same-port", None),
            external(),
            Source::BlePcap {
                path: "same.pcap".into(),
            },
        ] {
            invalid_plan(plan(vec![source.clone(), source]));
        }
        invalid_plan(plan(vec![
            Source::NativeBle { adapter: None },
            Source::NativeBle {
                adapter: Some("hci1".into()),
            },
        ]));
        assert!(plan(vec![
            Source::NativeBle {
                adapter: Some("hci0".into())
            },
            Source::NativeBle {
                adapter: Some("hci1".into())
            }
        ])
        .resolve()
        .is_ok());
    }

    #[test]
    fn empty_or_invalid_source_parameters_are_rejected() {
        for adapter in ["", "  "] {
            invalid_plan(plan(vec![Source::NativeBle {
                adapter: Some(adapter.into()),
            }]));
        }
        for interface in [
            String::new(),
            "x".repeat(16),
            ".".into(),
            "..".into(),
            "../../mon0".into(),
            "white space".into(),
        ] {
            invalid_plan(plan(vec![Source::LinuxWifi { interface }]));
        }
        invalid_plan(plan(vec![Source::BlePcap {
            path: PathBuf::new(),
        }]));
        for field in 0..3 {
            let mut source = sniffle("port", None);
            if let Source::Sniffle {
                port,
                python,
                helper,
                ..
            } = &mut source
            {
                match field {
                    0 => port.clear(),
                    1 => *python = PathBuf::new(),
                    _ => *helper = PathBuf::new(),
                }
            }
            invalid_plan(plan(vec![source]));
        }
        for channel in [0, 36, 40, 255] {
            invalid_plan(plan(vec![sniffle("port", Some(channel))]));
        }
    }

    #[test]
    fn external_plan_validates_channels_paths_and_dwell_bounds() {
        for field in 0..7 {
            let mut source = external();
            if let Source::ExternalWifi {
                executable,
                firmware,
                channels,
                dwell_ms,
                ..
            } = &mut source
            {
                match field {
                    0 => *executable = PathBuf::new(),
                    1 => *firmware = PathBuf::new(),
                    2 => channels.clear(),
                    3 => *channels = vec![1, 1],
                    4 => *channels = vec![0],
                    5 => *channels = vec![1; 39],
                    _ => *dwell_ms = 99,
                }
            }
            invalid_plan(plan(vec![source]));
        }
        for dwell in [100, 10_000, 10_001] {
            let mut source = external();
            if let Source::ExternalWifi { dwell_ms, .. } = &mut source {
                *dwell_ms = dwell;
            }
            assert_eq!(plan(vec![source]).resolve().is_ok(), dwell <= 10_000);
        }
    }

    #[test]
    fn source_assignments_express_requests_without_inventing_capabilities() {
        let sources = [
            Source::NativeBle { adapter: None },
            Source::NativeWifi {},
            Source::LinuxWifi {
                interface: "mon0".into(),
            },
            sniffle("port", Some(38)),
            external(),
            Source::BlePcap {
                path: "file.pcap".into(),
            },
        ];
        for (i, source) in sources.iter().enumerate() {
            assert_eq!(source.is_ble(), [true, false, false, true, false, true][i]);
            assert_eq!(
                source.shutdown_budget().as_secs(),
                if i == 4 { 40 } else { 5 }
            );
        }
        assert_eq!(sources[0].assignment(), json!({"mode":"os_managed"}));
        assert_eq!(sources[1].assignment(), json!({"mode":"os_managed"}));
        assert_eq!(
            sources[2].assignment(),
            json!({"mode":"externally_configured","channel":null})
        );
        assert_eq!(
            sources[3].assignment(),
            json!({"mode":"fixed_primary","channel":38,"phy":"1m","secondary_following":true})
        );
        assert_eq!(
            sources[4].assignment(),
            json!({"mode":"channel_plan","channels":[1,6,11],"dwell_ms":100,"usb_selector":null})
        );
        assert_eq!(sources[5].assignment(), json!({"mode":"replay"}));
        let mut coded = sniffle("coded-port", Some(39));
        if let Source::Sniffle { coded, .. } = &mut coded {
            *coded = true;
        }
        assert_eq!(coded.assignment()["phy"], "coded");
    }

    #[test]
    fn serde_rejects_unknown_fields_and_unsupported_sources() {
        for value in [
            json!({"version":1,"receivers":[],"typo":true}),
            json!({"version":1,"receivers":[{"id":"r","source":{"kind":"native_wifi"},"typo":true}]}),
            json!({"version":1,"receivers":[{"id":"r","source":{"kind":"native_ble","typo":true}}]}),
            json!({"version":1,"receivers":[{"id":"r","source":{"kind":"native_wifi","typo":true}}]}),
            json!({"version":1,"receivers":[{"id":"r","source":{"kind":"subghz"}}]}),
        ] {
            assert!(serde_json::from_value::<SessionConfig>(value).is_err());
        }
        let config: SessionConfig = serde_json::from_value(json!({"version":1,"receivers":[{"id":"r","source":{"kind":"sniffle","port":"port","python":"python3","helper":"helper.py"}}]})).unwrap();
        let resolved = config.resolve().unwrap();
        assert_eq!(resolved.receivers[0].source.assignment()["phy"], "1m");
        let encoded = serde_json::to_value(&resolved).unwrap();
        let decoded: SessionConfig = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded.resolve().unwrap()).unwrap(),
            encoded
        );
    }

    #[test]
    fn airhound_serial_is_explicit_mixed_media_and_rejects_aliases_and_bad_baud() {
        let serial = |port: &str, baud| Source::AirhoundSerial {
            port: port.into(),
            baud,
        };
        let resolved = plan(vec![serial("/dev/cu.fixture", 460800)])
            .resolve()
            .unwrap();
        let assignment = resolved.receivers[0].source.assignment();
        assert_eq!(assignment["media"], json!(["wifi", "ble"]));
        assert_eq!(assignment["pause"], "host_ingestion_only");
        assert_eq!(assignment["port_open_may_reset_board"], true);
        invalid_plan(plan(vec![
            serial("/dev/cu.fixture", 460800),
            serial("/dev/tty.fixture", 460800),
        ]));
        invalid_plan(plan(vec![
            serial("/dev/cu.fixture", 460800),
            sniffle("/dev/tty.fixture", None),
        ]));
        invalid_plan(plan(vec![serial("/dev/cu.fixture", 9600)]));
        invalid_plan(plan(vec![serial("", 460800)]));
        let encoded = serde_json::to_value(&resolved).unwrap();
        assert_eq!(
            serde_json::to_value(SessionConfig::from_json(encoded.clone()).unwrap()).unwrap(),
            encoded
        );
    }

    #[test]
    fn callout_and_dialin_nodes_are_one_serial_resource() {
        invalid_plan(plan(vec![
            sniffle("/dev/cu.airhound-test", None),
            sniffle("/dev/tty.airhound-test", None),
        ]));
        assert!(plan(vec![
            sniffle("/dev/cu.airhound-test-a", None),
            sniffle("/dev/cu.airhound-test-b", None),
        ])
        .resolve()
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn serial_symlink_aliases_cannot_open_the_same_resource_twice() {
        let dir = std::env::temp_dir().join(format!(
            "airhound-session-alias-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let port = dir.join("port");
        let alias = dir.join("alias");
        std::fs::write(&port, []).unwrap();
        std::os::unix::fs::symlink(&port, &alias).unwrap();
        let result = plan(vec![
            sniffle(port.to_str().unwrap(), None),
            sniffle(alias.to_str().unwrap(), None),
        ])
        .resolve();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}

#[cfg(test)]
mod incomplete_setup_tests {
    use super::*;
    #[test]
    fn incomplete_sniffle_hint_names_missing_fields() {
        let value = serde_json::json!({"version":1,"receivers":[{"id":"sniffer","source":{"kind":"sniffle","port":"/dev/test","python":null,"helper":null,"module_path":null}}]});
        let error = SessionConfig::from_json(value).unwrap_err().to_string();
        assert!(error.contains("Receiver 1 (sniffle) setup incomplete"));
        assert!(error.contains("python, helper"));
    }
    #[test]
    fn optional_sniffle_module_path_remains_optional() {
        let value = serde_json::json!({"version":1,"receivers":[{"id":"sniffer","source":{"kind":"sniffle","port":"/dev/test","python":"python3","helper":"helper.py","module_path":null}}]});
        assert!(SessionConfig::from_json(value).is_ok());
    }
}
