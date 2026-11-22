//! Passive receiver candidates. Enumeration never opens radio or serial transports.
use serde::Serialize;

pub mod identify;
use serde_json::{json, Value};
use std::{
    io,
    path::{Path, PathBuf},
};
#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Endpoint {
    pub kind: String,
    pub path: String,
}
#[derive(Clone, Debug)]
pub(super) struct RawDevice {
    pub id: String,
    pub name: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub endpoints: Vec<Endpoint>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub backend: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct UsbIdentity {
    pub vid: String,
    pub pid: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Requirement {
    pub name: String,
    pub state: &'static str,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub kind: &'static str,
    /// Backend role, not a claim of verified hardware capability.
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_issue: Option<&'static str>,
    pub label: &'static str,
    pub support: &'static str,
    pub state: &'static str,
    pub evidence: Vec<String>,
    pub requirements: Vec<Requirement>,
    pub capabilities: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identification: Option<identify::Identification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_identity: Option<crate::external_identity::Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_hint: Option<Value>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub identity_scope: &'static str,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb: Option<UsbIdentity>,
    pub endpoints: Vec<Endpoint>,
    pub candidates: Vec<Candidate>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Inventory {
    pub schema_version: u8,
    pub mode: &'static str,
    pub platform: String,
    pub devices: Vec<Device>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub all: bool,
    pub external_scanner: Option<PathBuf>,
    pub external_firmware: Option<PathBuf>,
    pub sniffle_python: Option<PathBuf>,
    pub sniffle_helper: Option<PathBuf>,
    pub sniffle_module_path: Option<PathBuf>,
}

pub async fn discover(options: &Options) -> Inventory {
    #[cfg(target_os = "macos")]
    let (mut raw, diagnostics) = macos::collect().await;
    #[cfg(target_os = "macos")]
    for (kind, name) in [
        ("native_ble", "System-managed CoreBluetooth"),
        ("native_wifi", "System-managed CoreWLAN"),
    ] {
        raw.push(RawDevice {
            id: format!("macos:{kind}"),
            name: name.into(),
            vid: None,
            pid: None,
            endpoints: vec![Endpoint {
                kind: kind.into(),
                path: "system_managed".into(),
            }],
        });
    }
    #[cfg(target_os = "linux")]
    let (raw, diagnostics) = linux::collect();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let (raw, diagnostics) = (
        Vec::new(),
        vec![Diagnostic {
            backend: "platform".into(),
            message: "Passive inventory is implemented for macOS and Linux only".into(),
        }],
    );
    assemble(raw, diagnostics, std::env::consts::OS, options)
}

impl Inventory {
    /// Attach metadata from the service-owned current run only; never open devices
    /// or resurrect identity from historical recordings or an earlier inventory.
    pub fn reuse_active_metadata(&mut self, status: &Value) {
        if status["active"] != true {
            return;
        }
        let Some(receivers) = status["receiver_health"]["receivers"].as_array() else {
            return;
        };
        for device in &mut self.devices {
            for candidate in &mut device.candidates {
                if candidate.kind == "serial_unknown" {
                    if let Some(identified) = active_airhound_candidate(
                        device.id.as_str(),
                        &device.endpoints,
                        receivers,
                        status,
                    ) {
                        *candidate = identified;
                        self.mode = "passive_with_active_metadata";
                    }
                    continue;
                }
                if candidate.kind == "external_wifi" {
                    // OS inventory IDs and libusb selectors are different namespaces.
                    // Reuse only an already established exact selector; never infer it
                    // from VID/PID, number of attached adapters, or a display label.
                    if let Some(selector) = candidate
                        .configuration_hint
                        .as_ref()
                        .and_then(|hint| hint["usb_selector"].as_str())
                    {
                        if let Some(owner) = receivers.iter().find(|owner| {
                            owner["receiver"]["backend"] == "mt7610u_companion"
                                && owner["receiver"]["adapter_id"] == selector
                                && matches!(
                                    owner["receiver"]["availability"].as_str(),
                                    Some("running" | "paused")
                                )
                        }) {
                            if let Ok(identity) =
                                serde_json::from_value::<crate::external_identity::Identity>(
                                    owner["receiver"]["external_identity"].clone(),
                                )
                            {
                                candidate.evidence.push(identity.description());
                                candidate.external_identity = Some(identity);
                                candidate.state = "in_use";
                                self.mode = "passive_with_active_metadata";
                            }
                        }
                    }
                    continue;
                }
                if candidate.kind != "sniffle" {
                    continue;
                }
                let Some((endpoint, _owner)) = device
                    .endpoints
                    .iter()
                    .filter(|endpoint| endpoint.kind == "serial")
                    .find_map(|endpoint| {
                        receivers
                            .iter()
                            .find(|owner| {
                                owner["receiver"]["adapter_id"]
                                    == format!("sniffle:{}", endpoint.path)
                            })
                            .map(|owner| (endpoint, owner))
                    })
                else {
                    continue;
                };
                let request = identify::Request {
                    protocol: "sniffle".into(),
                    device_id: device.id.clone(),
                    port: endpoint.path.clone(),
                    python: PathBuf::new(),
                    helper: PathBuf::new(),
                    module_path: None,
                };
                let identity = identify::Identification::from_active_owner(&request, status);
                if let (Some(version), Some(api)) =
                    (&identity.firmware_version, identity.protocol_api)
                {
                    if identity.compatible == Some(true) {
                        candidate.support = "available";
                    }
                    candidate.label = "Sniffle BLE receiver";
                    candidate.evidence.push(format!("Active receiver reported Sniffle {version} / API{api}; board revision is still unknown"));
                }
                candidate.state = "in_use";
                candidate.identification = Some(identity);
                self.mode = "passive_with_active_metadata";
            }
        }
    }
}

// Runtime identity can enrich a passive inventory, but must never initiate UART
// access or treat a USB bridge's brand as firmware evidence.
fn active_airhound_candidate(
    device_id: &str,
    endpoints: &[Endpoint],
    owners: &[Value],
    status: &Value,
) -> Option<Candidate> {
    let (endpoint, owner) = endpoints
        .iter()
        .filter(|endpoint| endpoint.kind == "serial")
        .find_map(|endpoint| {
            owners
                .iter()
                .find(|owner| {
                    let receiver = &owner["receiver"];
                    receiver["backend"] == "airhound_serial"
                        && matches!(
                            receiver["availability"].as_str(),
                            Some("available" | "paused" | "initializing")
                        )
                        && receiver["adapter_id"]
                            .as_str()
                            .is_some_and(|path| serial_alias(path) == serial_alias(&endpoint.path))
                })
                .map(|owner| (endpoint, owner))
        })?;
    let identity = &owner["receiver"]["producer_identity"];
    if identity["application"] != "AirHound" || identity["evidence"] != "device_status" {
        return None;
    }
    let bounded = |key: &str| {
        identity[key]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 64)
    };
    let version = bounded("version")?;
    let board = bounded("board")?;
    identity["connection_generation"].as_u64()?;
    identity["boot_generation"].as_u64()?;
    let mut candidate = candidate("airhound_serial", "AirHound WiFi/BLE sensor", "available", "in_use",
        "Firmware identity comes from the existing capture owner; discovery did not open or reset the serial port");
    candidate.evidence.push(format!("Device reports AirHound {version} on {board}; RF readiness remains in current receiver health"));
    candidate.capabilities = json!({"scope":"firmware_summary_contract", "media":["wifi","ble"], "channel_control":"device_managed", "pause":"host_ingestion_only", "stop":"close_transport_only", "extended_advertising":"unknown", "coded_phy":"unknown"});
    let request = identify::Request {
        protocol: "airhound_serial".into(),
        device_id: device_id.into(),
        port: endpoint.path.clone(),
        python: PathBuf::new(),
        helper: PathBuf::new(),
        module_path: None,
    };
    let mut identified = identify::Identification::new(
        &request,
        "in_use",
        "Identity reused from actual AirHound device status; no additional endpoint opened",
    );
    identified.provenance = "active_capture_owner";
    identified.application = Some("AirHound");
    identified.firmware_version = Some(version.into());
    identified.board = Some(board.into());
    // The firmware reports no protocol version, and identity alone is not RF evidence.
    identified.owner = Some(json!({"run_id":status["run_id"],"receiver":owner}));
    candidate.identification = Some(identified);
    if let Some(baud) = owner["requested_assignment"]["baud"]
        .as_u64()
        .filter(|baud| [115200, 460800, 921600].contains(baud))
    {
        candidate.configuration_hint = Some(
            json!({"kind":"airhound_serial","port":owner["receiver"]["adapter_id"],"baud":baud}),
        );
    }
    Some(candidate)
}

fn serial_alias(path: &str) -> String {
    match path
        .strip_prefix("/dev/cu.")
        .or_else(|| path.strip_prefix("/dev/tty."))
    {
        Some(port) => format!("macos_serial:{port}"),
        None => format!("serial:{path}"),
    }
}

fn candidate(
    kind: &'static str,
    label: &'static str,
    support: &'static str,
    state: &'static str,
    evidence: &str,
) -> Candidate {
    Candidate {
        kind,
        role: match kind {
            "gps" => "location_source",
            "flipper" => "controller_or_companion",
            "serial_unknown" => "unknown",
            _ => "capture_receiver",
        },
        support_issue: match kind {
            "mt76_wifi" => Some("https://github.com/dougborg/AirHound/issues/235"),
            "sniffle" => Some("https://github.com/dougborg/AirHound/issues/180"),
            "external_wifi" => Some("https://github.com/dougborg/AirHound/issues/155"),
            "gps" => Some("https://github.com/dougborg/AirHound/issues/79"),
            "flipper" => Some("https://github.com/dougborg/AirHound/issues/181"),
            "usb_hci" => Some("https://github.com/dougborg/AirHound/issues/176"),
            "nrf_sniffer" => Some("https://github.com/dougborg/AirHound/issues/188"),
            "serial_unknown" => Some("https://github.com/dougborg/AirHound/issues/261"),
            _ => None,
        },
        label,
        support,
        state,
        evidence: vec![evidence.into()],
        requirements: vec![],
        capabilities: match kind {
            "native_ble" => {
                json!({"scope":"backend_declared_not_device_verified","observations":"ble_advertisements","channel_control":"os_managed","coded_phy":"unknown","extended_advertising":"unknown"})
            }
            "native_wifi" => {
                json!({"scope":"backend_declared_not_device_verified","observations":"wifi_access_points","channel_control":"os_managed"})
            }
            "linux_wifi" => {
                json!({"scope":"backend_declared_not_device_verified","observations":"wifi_management_frames","channel_control":"externally_configured"})
            }
            "external_wifi" => {
                json!({"scope":"backend_declared_not_device_verified","observations":"wifi_beacons","bands_ghz":[2.4,5],"channel_control":"configured_plan","receive_only":true})
            }
            "sniffle" => {
                json!({"scope":"backend_declared_not_device_verified","observations":"ble_packets","primary_channels":[37,38,39],"channel_control":"fixed_primary","coded_phy":"firmware_dependent"})
            }
            "gps" => {
                json!({"scope":"backend_declared_not_device_verified","observations":"location_via_gpsd"})
            }
            _ => json!({"scope":"unknown"}),
        },
        identification: None,
        external_identity: None,
        configuration_hint: None,
    }
}
fn prerequisite(name: &str, path: Option<&Path>, directory: bool) -> Requirement {
    let (state, detail) = match path {
        None => ("missing", "Supply an explicit path".into()),
        Some(path) => match path.metadata() {
            Ok(m) if (directory && m.is_dir()) || (!directory && m.is_file()) => (
                "present",
                format!(
                    "{} (existence only; not executed or validated)",
                    path.display()
                ),
            ),
            Ok(_) => (
                "invalid",
                format!(
                    "{} is not a {}",
                    path.display(),
                    if directory { "directory" } else { "file" }
                ),
            ),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                ("permission_blocked", e.to_string())
            }
            Err(e) => ("missing", format!("{}: {e}", path.display())),
        },
    };
    Requirement {
        name: name.into(),
        state,
        detail,
    }
}
fn configure(c: &mut Candidate, requirements: Vec<Requirement>) {
    c.state = if requirements.iter().all(|r| r.state == "present") {
        "unverified"
    } else {
        "needs_configuration"
    };
    c.requirements = requirements;
}
fn classify(raw: &RawDevice, platform: &str, options: &Options) -> Vec<Candidate> {
    let mut result = Vec::new();
    for endpoint in &raw.endpoints {
        match endpoint.kind.as_str() {
            "native_ble" => {
                let mut c = candidate(
                    "native_ble",
                    "OS Bluetooth receiver",
                    "available",
                    "unverified",
                    "OS receiver endpoint; power, permission and reception not probed",
                );
                c.configuration_hint = Some(if platform == "macos" {
                    json!({"kind":"native_ble"})
                } else {
                    json!({"kind":"native_ble","adapter":endpoint.path})
                });
                result.push(c);
            }
            "native_wifi" if platform == "macos" => {
                let mut c = candidate("native_wifi", "macOS WiFi scanner", "available", "unverified", "CoreWLAN backend; Location Services permission and radio availability not probed");
                c.configuration_hint = Some(json!({"kind":"native_wifi"}));
                result.push(c);
            }
            "native_wifi" | "linux_wifi" => {
                let mut c = candidate(
                    "linux_wifi",
                    "Linux WiFi interface",
                    "available",
                    "unverified",
                    "Wireless interface discovered in sysfs; capture permission not probed",
                );
                if endpoint.kind != "linux_wifi" {
                    c.state = "needs_configuration";
                    c.requirements.push(Requirement { name:"monitor_mode".into(), state:"missing", detail:"Configure a radiotap monitor interface externally; discovery never changes mode or channel".into() });
                }
                c.configuration_hint = Some(json!({"kind":"linux_wifi","interface":endpoint.path}));
                result.push(c);
            }
            _ => {}
        }
    }
    let ids = (raw.vid, raw.pid);
    let serial_count = raw.endpoints.iter().filter(|e| e.kind == "serial").count();
    let serial = raw.endpoints.iter().find(|e| e.kind == "serial");
    let selected_serial = serial.filter(|_| serial_count == 1);
    match ids {
        (Some(0x0e8d), Some(0x7961)) | (Some(0x0846), Some(0x9072)) => {
            let label = if raw.vid == Some(0x0e8d) { "MediaTek MT7921U / ALFA AWUS036AXML candidate" } else { "Netgear A9000 / MT7925U candidate" };
            let mut c = candidate("mt76_wifi", label, "experimental", "unverified", "Exact USB ID listed by Network-Weather/mt76-usb-macos; no local hardware validation or live AirHound backend yet (#235)");
            c.requirements.push(Requirement { name:"airhound_backend".into(), state:"unsupported", detail:"Live mt76 integration is pending; MT7610U helper configuration is not compatible. Do not load firmware during discovery".into() });
            result.push(c);
        }
        (Some(0x0e8d), Some(0x7610)) => {
            let mut c = candidate("external_wifi", "MediaTek MT7610U (including ALFA AWUS036ACHM)", "experimental", "needs_configuration", "USB VID/PID matches MT7610U; branding, firmware and exclusive access not verified");
            configure(&mut c, vec![prerequisite("external_scanner", options.external_scanner.as_deref(), false), prerequisite("external_firmware", options.external_firmware.as_deref(), false)]);
            c.configuration_hint = Some(json!({"kind":"external_wifi", "usb_selector":null, "executable":options.external_scanner, "firmware":options.external_firmware,"channels":[1],"dwell_ms":500})); result.push(c);
        }
        (Some(0x2e8a), Some(0x00c0)) | (Some(0x1209), Some(0xbabb)) => {
            let mut c = candidate("sniffle", "CatSniffer / Sniffle candidate", "candidate", "needs_identification", "USB bridge identity is compatible with CatSniffer; installed radio and Sniffle firmware must be confirmed explicitly");
            let mut required = vec![
                prerequisite("sniffle_python", options.sniffle_python.as_deref(), false),
                prerequisite("sniffle_helper", options.sniffle_helper.as_deref(), false),
                Requirement { name:"serial_endpoint".into(), state:if serial_count == 1 {"present"} else if serial_count == 0 {"missing"} else {"needs_selection"}, detail:"Select one validated capture endpoint; composite serial roles are not inferred and ports are not opened".into() },
            ];
            if options.sniffle_module_path.is_some() {
                required.push(prerequisite("sniffle_module_path", options.sniffle_module_path.as_deref(), true));
            }
            configure(&mut c, required);
            if options.sniffle_module_path.is_none() {
                c.requirements.push(Requirement { name:"sniffle_package".into(), state:"unknown", detail:"Optional module path omitted; Python package lookup is not executed during discovery".into() });
            }
            c.requirements.push(Requirement { name:"firmware_identity".into(), state:"unknown", detail:"No protocol probe performed; USB identity is not proof of installed Sniffle firmware".into() });
            if c.state == "unverified" { c.state = "needs_identification"; }
            c.configuration_hint = Some(json!({"kind":"sniffle","port":selected_serial.map(|e| &e.path),"python":options.sniffle_python,"helper":options.sniffle_helper,"module_path":options.sniffle_module_path,"channel":37,"coded":false})); result.push(c);
        }
        (Some(0x1546), Some(0x01a8)) => result.push(candidate("gps", "u-blox GNSS candidate", "available", "needs_configuration", "u-blox GNSS USB ID; use an explicitly configured gpsd server; direct protocol/fix not probed")),
        (Some(0x1915), Some(0x522a)) => result.push(candidate("nrf_sniffer", "Nordic BLE sniffer candidate", "research", "unsupported", "Recognized USB identity; no live Nordic source adapter in AirHound sessions")),
        (Some(0x0a12), Some(0x0001)) => {
            if !result.iter().any(|c| c.kind == "native_ble") {
                result.push(candidate("usb_hci", "CSR8510-compatible Bluetooth dongle", "research", "unsupported", "USB identity alone does not establish an OS-supported adapter; direct USB HCI integration remains experimental research"));
            }
        }
        (Some(0x0483), Some(0x5740)) => result.push(candidate("flipper", "STM32 CDC / Flipper candidate", "candidate", "needs_identification", "Shared STM32 USB CDC ID; not proof of Flipper hardware or compatible firmware. Live Flipper source integration is pending")),
        (Some(0x10c4), Some(0xea60)) | (Some(0x1a86), Some(0x55d4 | 0x7523)) | (Some(0x0403), Some(0x6001)) => result.push(candidate("serial_unknown", "USB UART (possible M5/AWOK or other board)", "candidate", "needs_identification", "Generic bridge identity cannot identify the board, processor role or firmware; no serial port was opened")),
        _ if serial.is_some() => result.push(candidate("serial_unknown", "Unidentified serial device", "candidate", "needs_identification", "Serial endpoint only; firmware protocol unknown")),
        _ => {}
    }
    result
}
fn assemble(
    raw: Vec<RawDevice>,
    diagnostics: Vec<Diagnostic>,
    platform: &str,
    options: &Options,
) -> Inventory {
    let mut devices: Vec<Device> = raw
        .into_iter()
        .filter_map(|mut r| {
            r.endpoints
                .sort_by(|a, b| (&a.kind, &a.path).cmp(&(&b.kind, &b.path)));
            r.endpoints.dedup();
            let candidates = classify(&r, platform, options);
            if candidates.is_empty() && !options.all {
                return None;
            }
            let identity_scope = if platform == "macos"
                && matches!(r.id.as_str(), "macos:native_ble" | "macos:native_wifi")
            {
                "system_backend"
            } else {
                "attachment_topology"
            };
            Some(Device {
                id: r.id,
                identity_scope,
                name: r.name,
                usb: r.vid.zip(r.pid).map(|(v, p)| UsbIdentity {
                    vid: format!("{v:04x}"),
                    pid: format!("{p:04x}"),
                }),
                endpoints: r.endpoints,
                candidates,
            })
        })
        .collect();
    if devices
        .iter()
        .filter(|d| d.candidates.iter().any(|c| c.kind == "external_wifi"))
        .count()
        > 1
    {
        for device in &mut devices {
            for c in &mut device.candidates {
                if c.kind == "external_wifi" {
                    c.state = "needs_configuration";
                    c.requirements.push(Requirement { name:"physical_device_selection".into(), state:"needs_selection", detail:"Multiple MT7610U devices attached; run the updated helper list-devices command and choose distinct usb_selector values. Inventory IDs are not helper selectors and no automatic mapping is performed".into() });
                }
            }
        }
    }
    devices.sort_by(|a, b| a.id.cmp(&b.id));
    Inventory {
        schema_version: 1,
        mode: "passive",
        platform: platform.into(),
        devices,
        diagnostics,
    }
}

/// Text output uses escaped labels so USB descriptors cannot inject terminal controls.
pub fn write_text(inventory: &Inventory, mut out: impl io::Write) -> io::Result<()> {
    writeln!(
        out,
        "AirHound devices — passive inventory ({})",
        inventory.platform
    )?;
    for d in &inventory.devices {
        writeln!(out, "{:?}  {:?}", d.id, d.name)?;
        for c in &d.candidates {
            writeln!(out, "  {}: {} [{}; {}]", c.kind, c.state, c.support, c.role)?;
            if let Some(issue) = c.support_issue {
                writeln!(out, "    support: {issue}")?;
            }
        }
        for e in &d.endpoints {
            writeln!(out, "  {}: {:?}", e.kind, e.path)?;
        }
        for c in &d.candidates {
            for r in &c.requirements {
                writeln!(out, "  {}: {} ({:?})", r.name, r.state, r.detail)?;
            }
        }
    }
    if inventory.devices.is_empty() {
        writeln!(out, "No candidate devices found.")?;
    }
    for d in &inventory.diagnostics {
        writeln!(out, "Discovery diagnostic [{}]: {:?}", d.backend, d.message)?;
    }
    writeln!(out,"No ports opened or scans started. Readiness, firmware, permissions and busy state require explicit validation.")
}
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
