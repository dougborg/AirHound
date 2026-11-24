//! CoreWLAN scan snapshots with bounded, explicitly attributed native metadata.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::health::{Phase, Scanner};
use crate::ScanEvent;
use airhound::comm;
use airhound::scanner::{FrameType, WiFiEvent};
use airhound_hostd::wifi_metadata::{BoundedBytes, Metadata, MAX_IE_BYTES, MAX_SSID_BYTES};
use objc2_core_wlan::{
    CWChannelBand, CWChannelWidth, CWNetwork, CWPHYMode, CWSecurity, CWWiFiClient,
};
use wifi_scan::WifiSecurity;

/// Owned host-only snapshot; no Objective-C objects cross the worker boundary.
#[derive(Default)]
struct Network {
    mac: String,
    ssid: String,
    channel: u32,
    signal_level: i32,
    security: Vec<WifiSecurity>,
    frequency_mhz: Option<u16>,
    legacy_ies: heapless::Vec<u8, 64>,
    metadata: Metadata,
}

/// Preserve only complete elements for the legacy small ODID parser. Full bounded
/// bytes and truncation evidence are independently retained in the host metadata.
fn legacy_ies(bytes: &[u8]) -> heapless::Vec<u8, 64> {
    let mut out = heapless::Vec::new();
    let mut offset = 0;
    while bytes.len().saturating_sub(offset) >= 2 {
        let end = offset + 2 + usize::from(bytes[offset + 1]);
        if end > bytes.len() || end > out.capacity() {
            break;
        }
        let _ = out.extend_from_slice(&bytes[offset..end]);
        offset = end;
    }
    out
}

/// Channel-raster conversion requires an explicit API-reported band. This is
/// an operating-channel frequency, not per-frame radiotap receive metadata.
fn frequency(band: CWChannelBand, channel: u32) -> Option<u16> {
    let mhz = match (band, channel) {
        (CWChannelBand::Band2GHz, 1..=13) => 2407 + 5 * channel,
        (CWChannelBand::Band2GHz, 14) => 2484,
        (CWChannelBand::Band5GHz, 182..=196) => 4000 + 5 * channel,
        (CWChannelBand::Band5GHz, 1..=179) => 5000 + 5 * channel,
        (CWChannelBand::Band6GHz, 2) => 5935,
        (CWChannelBand::Band6GHz, 1..=233) => 5950 + 5 * channel,
        _ => return None,
    };
    u16::try_from(mhz).ok()
}

fn native_scan() -> Result<Vec<Network>, wifi_scan::Error> {
    // CoreWLAN uses autoreleased Foundation collections. Retain all framework
    // objects in this dedicated synchronous worker and copy bounded data out.
    objc2::rc::autoreleasepool(|_| unsafe {
        let client = CWWiFiClient::sharedWiFiClient();
        let interface = client
            .interface()
            .ok_or_else(|| wifi_scan::Error::ScanFailed("No WiFi interface found".into()))?;
        let networks = interface
            .scanForNetworksWithName_error(None)
            .map_err(|_| wifi_scan::Error::ScanFailed("CoreWLAN scan failed".into()))?;
        Ok(networks.allObjects().iter().map(|n| snapshot(&n)).collect())
    })
}

/// All accessors are documented public CoreWLAN snapshot properties; no private
/// selectors, monitor mode, association changes, or permission bypasses.
unsafe fn snapshot(network: &CWNetwork) -> Network {
    let mut result = Network {
        mac: network.bssid().map(|v| v.to_string()).unwrap_or_default(),
        ssid: network.ssid().map(|v| v.to_string()).unwrap_or_default(),
        signal_level: network
            .rssiValue()
            .clamp(i32::MIN as isize, i32::MAX as isize) as i32,
        metadata: Metadata {
            source: "corewlan_scan_snapshot",
            noise_dbm: i16::try_from(network.noiseMeasurement()).ok(),
            beacon_interval_ms: u32::try_from(network.beaconInterval()).ok(),
            country_code: network.countryCode().map(|v| v.to_string()),
            ibss: Some(network.ibss()),
            ..Metadata::default()
        },
        ..Network::default()
    };
    if let Some(channel) = network.wlanChannel() {
        let band = channel.channelBand();
        result.channel = u32::try_from(channel.channelNumber()).unwrap_or(0);
        result.frequency_mhz = frequency(band, result.channel);
        result.metadata.band = match band {
            CWChannelBand::Band2GHz => Some("2.4GHz"),
            CWChannelBand::Band5GHz => Some("5GHz"),
            CWChannelBand::Band6GHz => Some("6GHz"),
            _ => None,
        };
        result.metadata.frequency_provenance = result
            .frequency_mhz
            .map(|_| "derived_from_corewlan_band_channel");
        result.metadata.channel_width_mhz = match channel.channelWidth() {
            CWChannelWidth::Width20MHz => Some(20),
            CWChannelWidth::Width40MHz => Some(40),
            CWChannelWidth::Width80MHz => Some(80),
            CWChannelWidth::Width160MHz => Some(160),
            _ => None,
        };
    }
    // NSData is held alive, never mutated here, and copied before leaving this
    // worker. Bound retained bytes before allocation, not after a full to_vec.
    if let Some(data) = network.ssidData() {
        result.metadata.ssid_bytes =
            Some(BoundedBytes::new(data.as_bytes_unchecked(), MAX_SSID_BYTES));
    }
    if let Some(data) = network.informationElementData() {
        let bytes = data.as_bytes_unchecked();
        result.legacy_ies = legacy_ies(bytes);
        result.metadata.information_elements = Some(BoundedBytes::new(bytes, MAX_IE_BYTES));
    }
    for (mode, name) in [
        (CWPHYMode::Mode11a, "802.11a"),
        (CWPHYMode::Mode11b, "802.11b"),
        (CWPHYMode::Mode11g, "802.11g"),
        (CWPHYMode::Mode11n, "802.11n"),
        (CWPHYMode::Mode11ac, "802.11ac"),
        (CWPHYMode::Mode11ax, "802.11ax"),
    ] {
        if network.supportsPHYMode(mode) {
            result.metadata.advertised_phy_modes.push(name);
        }
    }
    (result.security, result.metadata.security_modes) =
        security_modes(|mode| network.supportsSecurity(mode));
    result
}

/// Public API query list, separate from advertised RSN parsing. Generic group
/// results stay unspecified rather than inventing an AKM/cipher or key length.
fn security_modes(
    mut supported: impl FnMut(CWSecurity) -> bool,
) -> (Vec<WifiSecurity>, Vec<&'static str>) {
    let mut security = Vec::new();
    let mut names = Vec::new();
    for (mode, legacy, name) in [
        (CWSecurity::None, WifiSecurity::Open, "open"),
        (CWSecurity::WEP, WifiSecurity::Wep, "wep"),
        (CWSecurity::DynamicWEP, WifiSecurity::Wep, "dynamic_wep"),
        (
            CWSecurity::WPAPersonal,
            WifiSecurity::WpaPersonalPsk,
            "wpa_personal",
        ),
        (
            CWSecurity::WPA2Personal,
            WifiSecurity::Wpa2PersonalPsk,
            "wpa2_personal",
        ),
        (
            CWSecurity::WPA3Personal,
            WifiSecurity::Wpa3PersonalSae,
            "wpa3_personal",
        ),
        (
            CWSecurity::WPAEnterprise,
            WifiSecurity::WpaEnterpriseEap,
            "wpa_enterprise",
        ),
        (
            CWSecurity::WPA2Enterprise,
            WifiSecurity::Wpa2EnterpriseEap,
            "wpa2_enterprise",
        ),
        (
            CWSecurity::WPA3Enterprise,
            WifiSecurity::Wpa3EnterpriseEap,
            "wpa3_enterprise",
        ),
    ] {
        if supported(mode) {
            if !security.contains(&legacy) {
                security.push(legacy);
            }
            names.push(name);
        }
    }
    // Mixed-mode queries may be true independently of the individual queries.
    for (mode, name, modes) in [
        (
            CWSecurity::WPAPersonalMixed,
            "wpa_wpa2_personal",
            vec![WifiSecurity::WpaPersonalPsk, WifiSecurity::Wpa2PersonalPsk],
        ),
        (
            CWSecurity::WPA3Transition,
            "wpa2_wpa3_personal",
            vec![WifiSecurity::Wpa2PersonalPsk, WifiSecurity::Wpa3PersonalSae],
        ),
        (
            CWSecurity::WPAEnterpriseMixed,
            "wpa_wpa2_enterprise",
            vec![
                WifiSecurity::WpaEnterpriseEap,
                WifiSecurity::Wpa2EnterpriseEap,
            ],
        ),
    ] {
        if supported(mode) {
            names.push(name);
            for legacy in modes {
                if !security.contains(&legacy) {
                    security.push(legacy);
                }
            }
        }
    }
    // These public CWSecurity cases are present in the locked objc2 binding.
    // Apple headers do not declare a separate WPA3 Enterprise 192-bit case.
    for (mode, name, legacy) in [
        (CWSecurity::OWE, "owe", Some("OWE")),
        (
            CWSecurity::OWETransition,
            "owe_transition",
            Some("OWE-TRANSITION"),
        ),
        (CWSecurity::Personal, "personal_unspecified", None),
        (CWSecurity::Enterprise, "enterprise_unspecified", None),
        (CWSecurity::Unknown, "unknown", None),
    ] {
        if supported(mode) {
            names.push(name);
            if let Some(tag) = legacy {
                security.push(WifiSecurity::Other(tag.into()));
            }
        }
    }
    (security, names)
}

/// Polling interval between WiFi scans.
/// CoreWLAN controls scan duration and may return cached snapshots.
const SCAN_INTERVAL: Duration = Duration::from_secs(1);

/// Run the WiFi scan loop, periodically scanning via CoreWLAN and sending WiFiEvents.
///
/// Sends discovered events on `tx`. Runs until the channel is closed or an error occurs.
pub async fn scan_loop(
    tx: mpsc::Sender<ScanEvent>,
    control: crate::control::ScanControl,
) -> Result<(), ScanError> {
    scan_loop_with(tx, control, || blocking_scan(native_scan)).await
}

async fn blocking_scan<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ScanError> {
    // A synchronous CoreWLAN call cannot be canceled. A dedicated OS thread
    // keeps its completion independent of Tokio runtime shutdown.
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("airhound-wifi".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .map_err(|_| ScanError::TaskPanicked);
            let _ = send.send(result);
        })
        .map_err(|_| ScanError::WorkerUnavailable)?;
    receive.await.map_err(|_| ScanError::TaskPanicked)?
}

async fn scan_loop_with<F, Fut>(
    tx: mpsc::Sender<ScanEvent>,
    control: crate::control::ScanControl,
    mut scan: F,
) -> Result<(), ScanError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Result<Vec<Network>, wifi_scan::Error>, ScanError>>,
{
    let mut warned = false;
    let mut state = control.subscribe();
    loop {
        if !crate::control::wait_running(&mut state, &tx).await {
            return Ok(());
        }
        if crate::location_macos::permission_blocked() {
            control.set_health(
                Scanner::Wifi,
                Phase::Blocked,
                "location_permission_denied_or_disabled",
            );
            tokio::select! {
                _ = tx.closed() => return Ok(()),
                _ = state.changed() => {},
                _ = tokio::time::sleep(SCAN_INTERVAL) => {},
            }
            continue;
        }
        let generation = state.borrow().generation;
        log::debug!("WiFi scan requested");
        let scan = scan();
        tokio::pin!(scan);
        let networks = loop {
            tokio::select! {
                _ = tx.closed() => return Ok(()),
                changed = state.changed() => {
                    if changed.is_err() || state.borrow().shutdown { return Ok(()); }
                    // Finish this one OS scan, but discard it across pause/resume.
                }
                result = &mut scan => break result?,
            }
        };
        if state.borrow().generation != generation {
            continue;
        }

        match networks {
            Ok(wifis) => {
                // Redaction is observable even before authorization settles.
                let redacted = !wifis.is_empty() && wifis.iter().all(|w| w.mac.is_empty());
                control.set_health(
                    Scanner::Wifi,
                    if redacted {
                        Phase::Blocked
                    } else {
                        Phase::Running
                    },
                    if redacted {
                        "addresses_redacted_check_location_permission"
                    } else {
                        "scan_completed"
                    },
                );
                if redacted && !warned {
                    log::warn!(
                        "WiFi scan: all BSSIDs empty — grant Location Services permission \
                         to AirHound or the launching terminal in System Settings → Privacy & Security → Location Services"
                    );
                    warned = true;
                }

                if !redacted && warned {
                    log::info!("WiFi addresses are available again");
                    warned = false;
                }
                let mut count = 0u32;
                for wifi in &wifis {
                    if state.borrow().generation != generation {
                        break;
                    }
                    if let Some(event) = convert_network(wifi) {
                        count += 1;
                        if tx
                            .send(ScanEvent::WiFiDetailed {
                                event,
                                metadata: Box::new(wifi.metadata.clone()),
                            })
                            .await
                            .is_err()
                        {
                            log::debug!("WiFi scan channel closed, stopping");
                            return Ok(());
                        }
                    }
                }
                log::debug!("WiFi scan: {count} networks (of {} total)", wifis.len());
            }
            Err(e) => {
                control.set_health(Scanner::Wifi, Phase::Unavailable, "scan_failed_retrying");
                log::warn!("WiFi scan failed: {e}");
            }
        }

        tokio::select! {
            _ = tx.closed() => return Ok(()),
            _ = tokio::time::sleep(SCAN_INTERVAL) => {},
            _ = state.changed() => {}
        }
    }
}

/// Normalize a cached/aggregate CoreWLAN network; this is not a captured frame.
fn convert_network(network: &Network) -> Option<WiFiEvent> {
    // Skip networks with empty/unparseable BSSID (Location Services not granted)
    let mac = comm::parse_mac_string(&network.mac)?;

    let mut ssid = heapless::String::new();
    if !network.ssid.is_empty() {
        let s = &network.ssid;
        let end = if s.len() <= 33 {
            s.len()
        } else {
            s.floor_char_boundary(33)
        };
        let _ = ssid.push_str(&s[..end]);
    }

    let rssi = network.signal_level.clamp(i8::MIN as i32, i8::MAX as i32) as i8;

    // Band is supplied by CoreWLAN, never inferred from a bare channel.
    let channel = u8::try_from(network.channel).unwrap_or(0);

    let security = format_security(&network.security);

    Some(WiFiEvent {
        mac,
        ssid,
        rssi,
        channel,
        frequency_mhz: network.frequency_mhz,
        frame_type: FrameType::Beacon, // Legacy classification; metadata identifies snapshot evidence.
        raw_ies: network.legacy_ies.clone(),
        security,
    })
}

/// Format `wifi_scan::WifiSecurity` variants into WiGLE AuthMode bracket notation.
fn format_security(secs: &[WifiSecurity]) -> heapless::String<64> {
    let mut s = heapless::String::new();
    if secs.is_empty() || secs.iter().all(|s| matches!(s, WifiSecurity::Unknown)) {
        let _ = s.push_str("[?]");
        return s;
    }
    for sec in secs {
        let tag = match sec {
            WifiSecurity::Open => "[OPEN]",
            WifiSecurity::Wep => "[WEP]",
            WifiSecurity::WpaPersonalPsk => "[WPA-PSK]",
            WifiSecurity::Wpa2PersonalPsk => "[WPA2-PSK]",
            WifiSecurity::Wpa3PersonalSae => "[WPA3-SAE]",
            WifiSecurity::Wpa2EnterpriseEap => "[WPA2-EAP]",
            WifiSecurity::Wpa2EnterpriseEap256 => "[WPA2-EAP256]",
            WifiSecurity::Wpa3EnterpriseEap => "[WPA3-EAP]",
            WifiSecurity::Wpa3EnterpriseEap256 => "[WPA3-EAP256]",
            WifiSecurity::Wpa3EnterpriseSuiteBEap256 => "[WPA3-SUITEB]",
            WifiSecurity::Wpa2EnterpriseEapFt => "[WPA2-EAP-FT]",
            WifiSecurity::Wpa2PersonalPsk256 => "[WPA2-PSK256]",
            WifiSecurity::Wpa2PersonalPskFt => "[WPA2-PSK-FT]",
            WifiSecurity::Wpa3PersonalSaeFt => "[WPA3-SAE-FT]",
            WifiSecurity::WpaEnterpriseEap => "[WPA-EAP]",
            WifiSecurity::TunneledDirectLinkSetup => "[TDLS]",
            WifiSecurity::Other(value) if value == "OWE" => "[OWE]",
            WifiSecurity::Other(value) if value == "OWE-TRANSITION" => "[OWE-TRANSITION]",
            WifiSecurity::Unknown => continue,
            _ => continue,
        };
        let _ = s.push_str(tag);
    }
    if s.is_empty() {
        let _ = s.push_str("[?]");
    }
    s
}

#[derive(Debug)]
pub enum ScanError {
    TaskPanicked,
    WorkerUnavailable,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::TaskPanicked => write!(f, "scan task panicked"),
            ScanError::WorkerUnavailable => write!(f, "could not start WiFi worker"),
        }
    }
}

// MAC parsing tests live in the library (airhound::comm::parse_mac_string).

impl std::error::Error for ScanError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    async fn pending_once<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
        assert!(
            std::future::poll_fn(|cx| {
                std::task::Poll::Ready(future.as_mut().poll(cx).is_pending())
            })
            .await
        );
    }
    fn network() -> Network {
        Network {
            mac: "12:34:56:78:9A:BC".into(),
            ssid: "Flock-test".into(),
            channel: 6,
            signal_level: -55,
            security: vec![WifiSecurity::Wpa2PersonalPsk],
            metadata: Metadata {
                source: "corewlan_scan_snapshot",
                ssid_bytes: Some(BoundedBytes::new(b"Flock-test", MAX_SSID_BYTES)),
                advertised_phy_modes: vec!["802.11ax"],
                ..Metadata::default()
            },
            ..Network::default()
        }
    }
    #[test]
    fn network_metadata_and_bounds() {
        let event = convert_network(&network()).unwrap();
        assert_eq!(event.mac, [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]);
        assert_eq!(event.ssid.as_str(), "Flock-test");
        assert_eq!(event.security.as_str(), "[WPA2-PSK]");
        assert_eq!((event.channel, event.rssi), (6, -55));
        assert_eq!(event.frequency_mhz, None);
        assert!(convert_network(&Network::default()).is_none());
        let mut n = network();
        n.ssid = "é".repeat(20);
        n.signal_level = -300;
        let event = convert_network(&n).unwrap();
        assert_eq!(event.ssid.len(), 32);
        assert_eq!(event.rssi, -128);
        n.ssid.clear();
        n.signal_level = 300;
        n.channel = 65537;
        let event = convert_network(&n).unwrap();
        assert!(event.ssid.is_empty());
        assert_eq!(event.rssi, 127);
        assert_eq!(event.channel, 0);
        assert_eq!(event.frequency_mhz, None);
    }
    #[test]
    fn security_modes_are_preserved_in_csv_tags() {
        use WifiSecurity::*;
        for (sec, expected) in [
            (Open, "[OPEN]"),
            (Wep, "[WEP]"),
            (WpaPersonalPsk, "[WPA-PSK]"),
            (Wpa2PersonalPsk, "[WPA2-PSK]"),
            (Wpa3PersonalSae, "[WPA3-SAE]"),
            (Wpa2EnterpriseEap, "[WPA2-EAP]"),
            (Wpa2EnterpriseEap256, "[WPA2-EAP256]"),
            (Wpa3EnterpriseEap, "[WPA3-EAP]"),
            (Wpa3EnterpriseEap256, "[WPA3-EAP256]"),
            (Wpa3EnterpriseSuiteBEap256, "[WPA3-SUITEB]"),
            (Wpa2EnterpriseEapFt, "[WPA2-EAP-FT]"),
            (Wpa2PersonalPsk256, "[WPA2-PSK256]"),
            (Wpa2PersonalPskFt, "[WPA2-PSK-FT]"),
            (Wpa3PersonalSaeFt, "[WPA3-SAE-FT]"),
            (WpaEnterpriseEap, "[WPA-EAP]"),
            (TunneledDirectLinkSetup, "[TDLS]"),
            (Unknown, "[?]"),
            (Other("vendor".into()), "[?]"),
        ] {
            assert_eq!(format_security(&[sec]).as_str(), expected);
        }
        assert_eq!(format_security(&[]).as_str(), "[?]");
        assert_eq!(
            format_security(&[Unknown, Wpa2PersonalPsk, Wpa3PersonalSae]).as_str(),
            "[WPA2-PSK][WPA3-SAE]"
        );
        assert!(format_security(&vec![Wpa3EnterpriseSuiteBEap256; 100]).len() <= 64);
    }
    #[test]
    fn public_security_query_retains_owe_and_mixed_modes_without_guessing_ciphers() {
        let mut queried = Vec::new();
        let (legacy, names) = security_modes(|mode| {
            queried.push(mode);
            matches!(
                mode,
                CWSecurity::OWE
                    | CWSecurity::OWETransition
                    | CWSecurity::WPAEnterpriseMixed
                    | CWSecurity::Enterprise
            )
        });
        assert_eq!(queried.len(), 17);
        assert!(names.contains(&"owe"));
        assert!(names.contains(&"owe_transition"));
        assert!(names.contains(&"enterprise_unspecified"));
        assert_eq!(
            format_security(&legacy).as_str(),
            "[WPA-EAP][WPA2-EAP][OWE][OWE-TRANSITION]"
        );
        let (legacy, names) = security_modes(|_| false);
        assert_eq!(format_security(&legacy).as_str(), "[?]");
        assert!(names.is_empty());
    }

    #[test]
    fn frequency_requires_reported_band_and_handles_overlapping_channels() {
        for (band, channel, expected) in [
            (CWChannelBand::Band2GHz, 1, Some(2412)),
            (CWChannelBand::Band2GHz, 14, Some(2484)),
            (CWChannelBand::Band5GHz, 36, Some(5180)),
            (CWChannelBand::Band5GHz, 182, Some(4910)),
            (CWChannelBand::Band6GHz, 1, Some(5955)),
            (CWChannelBand::Band6GHz, 2, Some(5935)),
            (CWChannelBand::Band6GHz, 233, Some(7115)),
            (CWChannelBand::BandUnknown, 1, None),
            (CWChannelBand(100), 1, None),
            (CWChannelBand::Band2GHz, 36, None),
            (CWChannelBand::Band5GHz, 180, None),
            (CWChannelBand::Band6GHz, 234, None),
            (CWChannelBand::Band6GHz, u32::MAX, None),
        ] {
            assert_eq!(frequency(band, channel), expected);
            assert_eq!(frequency(band, 0), None);
        }
    }

    #[test]
    fn ie_and_binary_ssid_evidence_survive_legacy_capacity_and_text_limits() {
        // An IE spanning the small firmware buffer must not become a fake partial
        // element. The host copy independently retains it and explicit truncation.
        let mut ies = vec![0, 3, b'a', b'b', b'c', 221, 80];
        ies.extend([0xAA; 80]);
        assert_eq!(legacy_ies(&ies).as_slice(), &[0, 3, b'a', b'b', b'c']);
        let full = BoundedBytes::new(&ies, 4096);
        assert_eq!(full.bytes, ies);
        assert!(!full.truncated);
        let excess = BoundedBytes::new(&vec![0xAB; 5000], 4096);
        assert_eq!(
            (excess.original_length, excess.bytes.len(), excess.truncated),
            (5000, 4096, true)
        );
        let mut n = network();
        n.ssid.clear();
        n.metadata.ssid_bytes = Some(BoundedBytes::new(&[0xff, 0, 0x80], 32));
        n.metadata.information_elements = Some(full);
        n.legacy_ies = legacy_ies(&ies);
        n.frequency_mhz = frequency(CWChannelBand::Band6GHz, 1);
        let e = convert_network(&n).unwrap();
        assert!(e.ssid.is_empty());
        assert_eq!(n.metadata.ssid_bytes.unwrap().bytes, [0xff, 0, 0x80]);
        assert_eq!(e.raw_ies.as_slice(), &[0, 3, b'a', b'b', b'c']);
        assert_eq!(e.frequency_mhz, Some(5955));
        assert!(legacy_ies(&[221, 7, 1]).is_empty());
        assert!(legacy_ies(&[221]).is_empty());
    }

    #[tokio::test]
    async fn scan_recovers_from_failure_skips_redacted_results_and_stops_on_close() {
        crate::test_support::manual_time(async {
            let (tx, mut rx) = mpsc::channel(1);
            let mut scans = std::collections::VecDeque::from([
                Err(wifi_scan::Error::ScanFailed("temporary".into())),
                Ok(vec![Network::default()]),
                Ok(vec![network(), network()]),
            ]);
            let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = calls.clone();
            let task = scan_loop_with(tx, crate::control::ScanControl::new(true), move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                std::future::ready(Ok(scans.pop_front().unwrap_or(Ok(vec![]))))
            });
            tokio::pin!(task);
            pending_once(task.as_mut()).await;
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
            // Each poll processes a result and registers the next interval.
            // Assert before expiry, then cross Tokio's next millisecond timer tick.
            for expected in 2..=3 {
                tokio::time::advance(SCAN_INTERVAL - Duration::from_millis(1)).await;
                pending_once(task.as_mut()).await;
                assert_eq!(
                    calls.load(std::sync::atomic::Ordering::Relaxed),
                    expected - 1
                );
                tokio::time::advance(Duration::from_millis(2)).await;
                pending_once(task.as_mut()).await;
                assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), expected);
            }
            let ScanEvent::WiFiDetailed { event, metadata } = rx.recv().await.unwrap() else {
                panic!("expected WiFi");
            };
            assert_eq!(event.ssid.as_str(), "Flock-test");
            assert_eq!(metadata.source, "corewlan_scan_snapshot");
            assert_eq!(metadata.ssid_bytes.as_ref().unwrap().bytes, b"Flock-test");
            assert_eq!(metadata.advertised_phy_modes, ["802.11ax"]);
            assert!(metadata.projection()["receive_phy"].is_null());
            drop(rx);
            task.await.unwrap();
        })
        .await;
    }
    #[tokio::test]
    async fn closed_output_and_scan_task_failure_are_observable() {
        crate::test_support::manual_time(async {
            let (tx, rx) = mpsc::channel(1);
            drop(rx);
            scan_loop_with(tx, crate::control::ScanControl::new(true), || async {
                panic!("must not scan after receiver closes")
            })
            .await
            .unwrap();
            let (tx, _rx) = mpsc::channel(1);
            let err = scan_loop_with(tx, crate::control::ScanControl::new(true), || {
                std::future::ready(Err(ScanError::TaskPanicked))
            })
            .await
            .unwrap_err();
            assert_eq!(err.to_string(), "scan task panicked");
        })
        .await;
    }
    #[tokio::test]
    async fn closed_public_scanner_does_not_start_a_hardware_scan() {
        crate::test_support::manual_time(async {
            let (tx, rx) = mpsc::channel(1);
            drop(rx);
            scan_loop(tx, crate::control::ScanControl::new(true))
                .await
                .unwrap();
        })
        .await;
    }
    #[tokio::test]
    async fn blocking_worker_returns_results_and_reports_panics() {
        crate::test_support::manual_time(async {
            assert_eq!(blocking_scan(|| 42).await.unwrap(), 42);
            assert!(matches!(
                blocking_scan(|| panic!("test worker failure")).await,
                Err(ScanError::TaskPanicked)
            ));
            assert_eq!(
                ScanError::WorkerUnavailable.to_string(),
                "could not start WiFi worker"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn shutdown_does_not_wait_for_a_blocked_native_scan() {
        crate::test_support::manual_time(async {
            let (started, ready) = tokio::sync::oneshot::channel();
            let (release, blocked) = std::sync::mpsc::channel();
            struct ReleaseOnDrop(Option<std::sync::mpsc::Sender<()>>);
            impl Drop for ReleaseOnDrop {
                fn drop(&mut self) {
                    if let Some(release) = self.0.take() {
                        let _ = release.send(());
                    }
                }
            }
            let mut release = ReleaseOnDrop(Some(release));
            let (finished, done) = tokio::sync::oneshot::channel();
            let control = crate::control::ScanControl::new(true);
            let (tx, _rx) = mpsc::channel(1);
            let mut request = Some((started, blocked, finished));
            let task = tokio::spawn(scan_loop_with(tx, control.clone(), move || {
                let (started, blocked, finished) = request.take().unwrap();
                blocking_scan(move || {
                    let _ = started.send(());
                    let _ = blocked.recv();
                    let _ = finished.send(());
                    Ok(vec![network()])
                })
            }));
            ready.await.unwrap();
            control.shutdown();
            task.await.unwrap().unwrap();
            release.0.take().unwrap().send(()).unwrap();
            done.await.unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn pause_blocks_new_scans_and_discards_pre_pause_results() {
        crate::test_support::manual_time(async {
            use std::sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            };
            let calls = Arc::new(AtomicUsize::new(0));
            let counter = calls.clone();
            let control = crate::control::ScanControl::new(false);
            let (tx, mut rx) = mpsc::channel(8);
            let (first_result, first) = tokio::sync::oneshot::channel();
            let (second_result, second) = tokio::sync::oneshot::channel();
            let mut results = std::collections::VecDeque::from([first, second]);
            let task = scan_loop_with(tx, control.clone(), move || {
                counter.fetch_add(1, Ordering::Relaxed);
                let result = results
                    .pop_front()
                    .expect("only two scans authorized by fixture");
                async move { result.await.unwrap() }
            });
            tokio::pin!(task);
            pending_once(task.as_mut()).await;
            assert_eq!(calls.load(Ordering::Relaxed), 0);
            control.store(true, Ordering::Relaxed);
            pending_once(task.as_mut()).await;
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            control.store(false, Ordering::Relaxed);
            pending_once(task.as_mut()).await;
            first_result.send(Ok(Ok(vec![network()]))).unwrap();
            pending_once(task.as_mut()).await;
            assert!(matches!(
                rx.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            control.store(true, Ordering::Relaxed);
            pending_once(task.as_mut()).await;
            assert_eq!(calls.load(Ordering::Relaxed), 2);
            second_result.send(Ok(Ok(vec![network()]))).unwrap();
            pending_once(task.as_mut()).await;
            assert!(matches!(
                rx.recv().await,
                Some(ScanEvent::WiFiDetailed { .. })
            ));
            control.shutdown();
            task.await.unwrap();
        })
        .await;
    }
}
