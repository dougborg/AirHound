//! Receiver identity and capability evidence; never a claim of validated reception.
use btleplug::api::{Central, CentralState, Manager as _};
use btleplug::platform::{Adapter, Manager};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    Unknown,
}
impl Support {
    pub fn from_bool(value: bool) -> Self {
        if value {
            Self::Supported
        } else {
            Self::Unsupported
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Feature {
    pub controller: Support,
    pub api: Support,
    pub selected_scan: Support,
    pub validated_reception: Support,
}
impl Default for Feature {
    fn default() -> Self {
        Self {
            controller: Support::Unknown,
            api: Support::Unknown,
            selected_scan: Support::Unknown,
            validated_reception: Support::Unknown,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ControllerPhy {
    pub supported_mask: u32,
    pub configurable_mask: u32,
    /// System/controller defaults, not evidence of this btleplug scan's PHY.
    pub system_selected_mask: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProducerIdentity {
    pub application: &'static str,
    pub version: String,
    pub board: String,
    pub evidence: &'static str,
    pub connection_generation: u64,
    pub boot_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Receiver {
    pub backend: &'static str,
    pub adapter_id: String,
    pub adapter_info: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<airhound_hostd::discovery::identify::Firmware>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_address: Option<String>,
    pub selection: &'static str,
    pub availability: &'static str,
    pub scan_configuration: &'static str,
    pub extended_advertising: Feature,
    pub coded_rx: Feature,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_phy: Option<ControllerPhy>,
    pub capability_query: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer_identity: Option<ProducerIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_identity: Option<airhound_hostd::external_identity::Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_identity: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_status: Option<serde_json::Value>,
}

pub fn backend() -> &'static str {
    if cfg!(target_os = "linux") {
        "bluez"
    } else if cfg!(target_os = "macos") {
        "corebluetooth"
    } else {
        "platform"
    }
}

pub fn validate_selection(
    requested: Option<&str>,
    system_managed: bool,
) -> Result<(), btleplug::Error> {
    if let Some(id) = requested {
        if system_managed {
            return Err(btleplug::Error::NotSupported(
                "CoreBluetooth adapter selection is system-managed; omit --ble-adapter".into(),
            ));
        }
        if controller_index(id).is_none() {
            return Err(btleplug::Error::RuntimeError(
                "--ble-adapter requires a listed BlueZ hci identifier, for example hci0".into(),
            ));
        }
    }
    Ok(())
}

pub fn controller_index(id: &str) -> Option<u16> {
    let number = id.strip_prefix("hci")?;
    if number.is_empty() || !number.bytes().all(|v| v.is_ascii_digit()) {
        return None;
    }
    let index: u16 = number.parse().ok()?;
    (index != u16::MAX && format!("hci{index}") == id).then_some(index)
}

fn descriptor(
    info: String,
    state: Option<CentralState>,
    platform: &'static str,
) -> Result<Receiver, btleplug::Error> {
    let adapter_id = if platform == "bluez" {
        let id = info.split_whitespace().next().unwrap_or("");
        if controller_index(id).is_none() {
            return Err(btleplug::Error::RuntimeError(
                "BlueZ returned an unrecognized adapter identifier".into(),
            ));
        }
        id.to_owned()
    } else {
        "system".to_owned()
    };
    Ok(Receiver {
        producer_identity: None,
        external_identity: None,
        bridge_identity: None,
        bridge_status: None,
        backend: platform,
        adapter_id,
        adapter_info: info,
        firmware: None,
        controller_address: None,
        selection: if platform == "corebluetooth" {
            "system_managed"
        } else {
            "backend_default"
        },
        availability: availability(state),
        scan_configuration: "backend_default_phy_unknown",
        extended_advertising: Feature::default(),
        coded_rx: Feature::default(),
        controller_phy: None,
        capability_query: "not_exposed_by_btleplug",
    })
}
fn availability(state: Option<CentralState>) -> &'static str {
    match state {
        Some(CentralState::PoweredOn) => "powered_on",
        Some(CentralState::PoweredOff) => "powered_off",
        _ => "unknown",
    }
}

pub fn select_index(
    receivers: &[Receiver],
    requested: Option<&str>,
) -> Result<usize, btleplug::Error> {
    match requested {
        Some(id) => receivers
            .iter()
            .position(|r| r.adapter_id == id)
            .ok_or_else(|| {
                btleplug::Error::RuntimeError(format!(
                    "Requested BLE adapter {id} is unavailable; use --list-ble-adapters"
                ))
            }),
        None if !receivers.is_empty() => Ok(0), // Preserve btleplug's existing default order.
        None => Err(btleplug::Error::DeviceNotFound),
    }
}

pub fn update_state(control: &crate::control::ScanControl, state: CentralState) {
    let reason = availability(Some(state.clone()));
    control.set_receiver_availability(reason);
    if state != CentralState::PoweredOn {
        control.set_health(
            crate::health::Scanner::Ble,
            crate::health::Phase::Unavailable,
            "adapter_off_or_unavailable",
        );
    } else if control.health().ble.state == crate::health::Phase::Unavailable {
        control.set_health(
            crate::health::Scanner::Ble,
            crate::health::Phase::Initializing,
            "adapter_powered_on_scan_unconfirmed",
        );
    }
}

pub async fn discover() -> Result<Vec<(Adapter, Receiver)>, btleplug::Error> {
    let manager = Manager::new().await?;
    let mut receivers = Vec::new();
    for adapter in manager.adapters().await? {
        let mut receiver = descriptor(
            adapter.adapter_info().await?,
            adapter.adapter_state().await.ok(),
            backend(),
        )?;
        receiver.controller_address = adapter
            .adapter_address()
            .await
            .ok()
            .flatten()
            .map(|address| address.to_string());
        probe(&mut receiver).await;
        receivers.push((adapter, receiver));
    }
    Ok(receivers)
}

pub async fn selected(requested: Option<&str>) -> Result<(Adapter, Receiver), btleplug::Error> {
    validate_selection(requested, cfg!(target_os = "macos"))?;
    let mut receivers = discover().await?;
    let descriptions: Vec<_> = receivers.iter().map(|(_, r)| r.clone()).collect();
    let index = select_index(&descriptions, requested)?;
    let (adapter, mut receiver) = receivers.remove(index);
    if requested.is_some() {
        receiver.selection = "explicit";
    }
    Ok((adapter, receiver))
}

async fn probe(receiver: &mut Receiver) {
    #[cfg(target_os = "macos")]
    {
        use objc2::{msg_send, sel, ClassType};
        use objc2_core_bluetooth::{CBCentralManager, CBCentralManagerFeature};
        let available: bool = unsafe {
            msg_send![CBCentralManager::class(), respondsToSelector: sel!(supportsFeatures:)]
        };
        if !available {
            receiver.capability_query = "corebluetooth_feature_api_unavailable";
            return;
        }
        // Public class feature query; no manager allocation, scan or PHY configuration.
        let supported = unsafe {
            CBCentralManager::supportsFeatures(CBCentralManagerFeature::ExtendedScanAndConnect)
        };
        receiver.extended_advertising.api = Support::from_bool(supported);
        receiver.capability_query = "corebluetooth_extended_scan_feature";
    }
    #[cfg(target_os = "linux")]
    {
        let index = controller_index(&receiver.adapter_id).expect("validated BlueZ identifier");
        match tokio::task::spawn_blocking(move || linux::read_phy(index)).await {
            Ok(Ok(phy)) => {
                receiver.coded_rx.controller =
                    Support::from_bool(phy.supported_mask & (1 << 14) != 0);
                receiver.controller_phy = Some(phy);
                receiver.capability_query = "linux_mgmt_get_phy_configuration";
            }
            Ok(Err(reason)) => receiver.capability_query = reason,
            Err(_) => receiver.capability_query = "capability_probe_failed",
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = receiver;
}

/// Decode only the matching MGMT Get PHY command response, ignoring unrelated events.
#[cfg(any(target_os = "linux", test))]
fn phy_response(packet: &[u8], index: u16) -> Result<Option<ControllerPhy>, &'static str> {
    if packet.len() < 6 {
        return Err("malformed_mgmt_response");
    }
    let u16_at = |i| u16::from_le_bytes([packet[i], packet[i + 1]]);
    let event = u16_at(0);
    if u16_at(2) != index || !matches!(event, 1 | 2) {
        return Ok(None);
    }
    if usize::from(u16_at(4)) != packet.len() - 6 || packet.len() < 9 {
        return Err("malformed_mgmt_response");
    }
    if u16_at(6) != 0x0044 {
        return Ok(None);
    }
    if packet[8] != 0 {
        return Err(match packet[8] {
            0x14 => "mgmt_permission_denied",
            0x01 | 0x0c => "mgmt_phy_query_unavailable",
            0x11 => "adapter_removed",
            _ => "mgmt_phy_query_failed",
        });
    }
    if event != 1 || packet.len() != 21 {
        return Err("malformed_mgmt_response");
    }
    let u32_at = |i| u32::from_le_bytes(packet[i..i + 4].try_into().unwrap());
    Ok(Some(ControllerPhy {
        supported_mask: u32_at(9),
        configurable_mask: u32_at(13),
        system_selected_mask: u32_at(17),
    }))
}

// Exercise the actual bounded socket receive path with local datagrams in tests;
// this does not require a Bluetooth controller or privileged MGMT socket.
#[cfg(target_os = "linux")]
fn receive_phy(
    fd: std::os::fd::RawFd,
    index: u16,
    timeout: std::time::Duration,
) -> Result<ControllerPhy, &'static str> {
    let start = std::time::Instant::now();
    receive_phy_with(
        fd,
        index,
        timeout,
        || start.elapsed(),
        |fd, remaining| {
            let mut pollfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            unsafe { libc::poll(&mut pollfd, 1, remaining.as_millis().max(1) as i32) > 0 }
        },
    )
}

// The elapsed and readability seams preserve real recv/parser/error behavior;
// fixtures drive deadline passage explicitly, without sleeping or radio access.
#[cfg(any(target_os = "linux", test))]
fn receive_phy_with(
    fd: std::os::fd::RawFd,
    index: u16,
    timeout: std::time::Duration,
    mut elapsed: impl FnMut() -> std::time::Duration,
    mut readable: impl FnMut(std::os::fd::RawFd, std::time::Duration) -> bool,
) -> Result<ControllerPhy, &'static str> {
    loop {
        let remaining = timeout.saturating_sub(elapsed());
        if remaining.is_zero() || !readable(fd, remaining) {
            return Err("mgmt_query_timeout");
        }
        let mut packet = [0u8; 1024];
        let size = unsafe { libc::recv(fd, packet.as_mut_ptr().cast(), packet.len(), 0) };
        if size < 0 {
            return Err("mgmt_receive_failed");
        }
        if let Some(phy) = phy_response(&packet[..size as usize], index)? {
            return Ok(phy);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    #[repr(C)]
    struct SockAddrHci {
        family: libc::sa_family_t,
        dev: u16,
        channel: u16,
    }
    pub fn read_phy(index: u16) -> Result<ControllerPhy, &'static str> {
        // Read-only MGMT command. No HCI scan or global PHY settings are changed.
        let fd = unsafe {
            libc::socket(
                libc::AF_BLUETOOTH,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                1,
            )
        };
        if fd < 0 {
            return Err(
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied {
                    "mgmt_permission_denied"
                } else {
                    "mgmt_socket_unavailable"
                },
            );
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let address = SockAddrHci {
            family: libc::AF_BLUETOOTH as _,
            dev: 0xffff,
            channel: 3,
        };
        if unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&address as *const SockAddrHci).cast(),
                std::mem::size_of_val(&address) as _,
            )
        } < 0
        {
            return Err("mgmt_bind_failed");
        }
        let [lo, hi] = index.to_le_bytes();
        let command = [0x44u8, 0, lo, hi, 0, 0];
        if unsafe { libc::send(fd.as_raw_fd(), command.as_ptr().cast(), command.len(), 0) }
            != command.len() as isize
        {
            return Err("mgmt_send_failed");
        }
        receive_phy(fd.as_raw_fd(), index, std::time::Duration::from_millis(400))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    fn fake(id: &str) -> Receiver {
        descriptor(
            format!("{id} (usb:v0000p0000)"),
            Some(CentralState::PoweredOn),
            "bluez",
        )
        .unwrap()
    }
    #[test]
    fn explicit_selection_never_falls_back_and_defaults_preserve_order() {
        let list = [fake("hci1"), fake("hci0")];
        assert_eq!(select_index(&list, None).unwrap(), 0);
        assert_eq!(select_index(&list, Some("hci0")).unwrap(), 1);
        assert!(select_index(&list, Some("hci2")).is_err());
        assert!(select_index(&[], None).is_err());
        assert!(select_index(&list[1..], Some("hci1")).is_err()); // removed/replaced receiver
        assert!(validate_selection(Some("hci0"), true).is_err());
        assert!(validate_selection(None, true).is_ok());
        for invalid in ["", "hci", "hci01", "hci65535", "/org/bluez/hci0", "system"] {
            assert!(validate_selection(Some(invalid), false).is_err());
        }
    }
    #[test]
    fn unknown_and_platform_feature_never_claim_validated_or_selected_phy() {
        let mut r = descriptor("CoreBluetooth".into(), None, "corebluetooth").unwrap();
        r.extended_advertising.api = Support::from_bool(true);
        assert_eq!(r.coded_rx.controller, Support::Unknown);
        assert_eq!(r.extended_advertising.validated_reception, Support::Unknown);
        assert_eq!(r.extended_advertising.selected_scan, Support::Unknown);
        assert_eq!(Support::from_bool(false), Support::Unsupported);
        assert_eq!(availability(Some(CentralState::PoweredOff)), "powered_off");
        assert_eq!(availability(Some(CentralState::Unknown)), "unknown");
        assert!(descriptor("surprise".into(), None, "bluez").is_err());
    }
    #[test]
    fn receiver_health_preserves_identity_on_disconnect_without_claiming_recovery() {
        let control = crate::control::ScanControl::new(true);
        control.set_receiver(fake("hci1"));
        control.set_health(
            crate::health::Scanner::Ble,
            crate::health::Phase::Running,
            "scan_started",
        );
        update_state(&control, CentralState::PoweredOn);
        assert_eq!(control.health().ble.state, crate::health::Phase::Running);
        update_state(&control, CentralState::PoweredOff);
        let health = control.health_message();
        assert_eq!(health["receiver"]["adapter_id"], "hci1");
        assert_eq!(health["receiver"]["availability"], "powered_off");
        assert_eq!(
            control.health().ble.state,
            crate::health::Phase::Unavailable
        );
        update_state(&control, CentralState::PoweredOn);
        assert_eq!(
            control.health().ble.state,
            crate::health::Phase::Initializing
        );
        assert_eq!(control.receiver_id().as_deref(), Some("hci1"));
        assert_eq!(
            health["receiver"]["coded_rx"]["validated_reception"],
            "unknown"
        );
    }

    #[test]
    fn management_transport_ignores_other_controllers_and_bounds_failure() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixDatagram;
        use std::time::Duration;
        let receive = |fd, index, timeout| {
            receive_phy_with(
                fd,
                index,
                timeout,
                || Duration::ZERO,
                |fd, _| {
                    let mut pollfd = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    unsafe { libc::poll(&mut pollfd, 1, 0) > 0 }
                },
            )
        };
        let (reader, writer) = UnixDatagram::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut response = vec![1, 0, 2, 0, 15, 0, 0x44, 0, 0];
        response.extend_from_slice(&(1u32 << 14).to_le_bytes());
        response.extend_from_slice(&0u32.to_le_bytes());
        response.extend_from_slice(&(1u32 << 10).to_le_bytes());
        let mut unrelated = response.clone();
        unrelated[2] = 3;
        writer.send(&unrelated).unwrap();
        writer.send(&response).unwrap();
        let phy = receive(reader.as_raw_fd(), 2, Duration::from_millis(50)).unwrap();
        assert_eq!(phy.supported_mask, 1 << 14);
        assert_eq!(phy.system_selected_mask, 1 << 10);
        assert_eq!(
            receive(reader.as_raw_fd(), 2, Duration::ZERO),
            Err("mgmt_query_timeout")
        );
        assert_eq!(
            receive(reader.as_raw_fd(), 2, Duration::from_millis(5)),
            Err("mgmt_query_timeout")
        );
        writer.send(&[1, 0]).unwrap();
        assert_eq!(
            receive(reader.as_raw_fd(), 2, Duration::from_millis(50)),
            Err("malformed_mgmt_response")
        );
        response[8] = 0x14;
        writer.send(&response).unwrap();
        assert_eq!(
            receive(reader.as_raw_fd(), 2, Duration::from_millis(50)),
            Err("mgmt_permission_denied")
        );
    }

    #[test]
    fn management_deadline_does_not_restart_after_unrelated_packet() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixDatagram;
        use std::time::Duration;
        let (reader, writer) = UnixDatagram::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        // A complete command response for another controller is ignored.
        writer.send(&[1, 0, 3, 0, 0, 0]).unwrap();
        writer.send(&[1, 0, 3, 0, 0, 0]).unwrap();
        let elapsed = std::cell::Cell::new(Duration::ZERO);
        let mut waits = Vec::new();
        assert_eq!(
            receive_phy_with(
                reader.as_raw_fd(),
                2,
                Duration::from_millis(50),
                || elapsed.get(),
                |_, remaining| {
                    waits.push(remaining);
                    elapsed.set(elapsed.get() + Duration::from_millis(25));
                    true
                }
            ),
            Err("mgmt_query_timeout")
        );
        assert_eq!(
            waits,
            [Duration::from_millis(50), Duration::from_millis(25)]
        );
        // A readiness race still reports recv failure instead of fabricated data.
        assert_eq!(
            receive_phy_with(
                reader.as_raw_fd(),
                2,
                Duration::from_millis(50),
                || Duration::ZERO,
                |_, _| true
            ),
            Err("mgmt_receive_failed")
        );
    }

    #[test]
    fn mgmt_parser_rejects_wrong_controller_opcode_truncation_and_status() {
        let mut p = vec![1, 0, 2, 0, 15, 0, 0x44, 0, 0];
        p.extend_from_slice(&(1u32 << 14).to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&(1u32 << 10).to_le_bytes());
        let phy = phy_response(&p, 2).unwrap().unwrap();
        assert_eq!(phy.supported_mask, 1 << 14);
        assert_eq!(phy.system_selected_mask, 1 << 10);
        assert_eq!(phy_response(&p, 1), Ok(None));
        p[6] = 0x45;
        assert_eq!(phy_response(&p, 2), Ok(None));
        p[6] = 0x44;
        for len in 0..p.len() {
            assert!(phy_response(&p[..len], 2).is_err());
        }
        p[8] = 0x14;
        assert_eq!(phy_response(&p, 2), Err("mgmt_permission_denied"));
        p[8] = 0x11;
        assert_eq!(phy_response(&p, 2), Err("adapter_removed"));
        p[8] = 0x0c;
        assert_eq!(phy_response(&p, 2), Err("mgmt_phy_query_unavailable"));
    }
}
