//! WiFi scanner for macOS using `wifi_scan` crate (CoreWLAN wrapper).

use std::time::Duration;

use tokio::sync::mpsc;

use crate::ScanEvent;
use airhound::comm;
use airhound::scanner::{FrameType, WiFiEvent};
use wifi_scan::WifiSecurity;

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
    scan_loop_with(tx, control, || blocking_scan(wifi_scan::scan)).await
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
    Fut: std::future::Future<
        Output = Result<Result<Vec<wifi_scan::Wifi>, wifi_scan::Error>, ScanError>,
    >,
{
    let mut warned = false;
    let mut state = control.subscribe();
    loop {
        if !crate::control::wait_running(&mut state, &tx).await {
            return Ok(());
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
                // Check for Location Services on first scan
                if !wifis.is_empty() && !warned && wifis.iter().all(|w| w.mac.is_empty()) {
                    log::warn!(
                        "WiFi scan: all BSSIDs empty — grant Location Services permission \
                         to AirHound or the launching terminal in System Settings → Privacy & Security → Location Services"
                    );
                    warned = true;
                }

                let mut count = 0u32;
                for wifi in &wifis {
                    if state.borrow().generation != generation {
                        break;
                    }
                    if let Some(event) = convert_network(wifi) {
                        count += 1;
                        if tx.send(ScanEvent::WiFi(event)).await.is_err() {
                            log::debug!("WiFi scan channel closed, stopping");
                            return Ok(());
                        }
                    }
                }
                log::debug!("WiFi scan: {count} networks (of {} total)", wifis.len());
            }
            Err(e) => {
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

/// Convert a `wifi_scan::Wifi` into an AirHound `WiFiEvent`.
fn convert_network(network: &wifi_scan::Wifi) -> Option<WiFiEvent> {
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

    let channel = network.channel as u8;

    let security = format_security(&network.security);

    Some(WiFiEvent {
        mac,
        ssid,
        rssi,
        channel,
        frame_type: FrameType::Beacon, // CoreWLAN only returns beacon-equivalent data
        raw_ies: heapless::Vec::new(),
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
            WifiSecurity::WpaPersonal => "[WPA-PSK]",
            WifiSecurity::Wpa2PersonalPsk => "[WPA2-PSK]",
            WifiSecurity::Wpa3PersonalSae => "[WPA3-SAE]",
            WifiSecurity::Wpa2EnterpriseEap => "[WPA2-EAP]",
            WifiSecurity::Wpa3EnterpriseEap256 => "[WPA3-EAP256]",
            WifiSecurity::Wpa3EnterpriseSuiteBEap256 => "[WPA3-SUITEB]",
            WifiSecurity::Wpa2EnterpriseEapFt => "[WPA2-EAP-FT]",
            WifiSecurity::Wpa3PersonalPsk256 => "[WPA3-PSK256]",
            WifiSecurity::Wpa2PersonalPskFt => "[WPA2-PSK-FT]",
            WifiSecurity::Wpa3PersonalSaeFt => "[WPA3-SAE-FT]",
            WifiSecurity::WpaEnterprise => "[WPA-EAP]",
            WifiSecurity::Personal => "[PSK]",
            WifiSecurity::Enterprise => "[EAP]",
            WifiSecurity::Tdls => "[TDLS]",
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
    fn network() -> wifi_scan::Wifi {
        wifi_scan::Wifi {
            mac: "12:34:56:78:9A:BC".into(),
            ssid: "Flock-test".into(),
            channel: 6,
            signal_level: -55,
            security: vec![WifiSecurity::Wpa2PersonalPsk],
        }
    }
    #[test]
    fn network_metadata_and_bounds() {
        let event = convert_network(&network()).unwrap();
        assert_eq!(event.mac, [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]);
        assert_eq!(event.ssid.as_str(), "Flock-test");
        assert_eq!(event.security.as_str(), "[WPA2-PSK]");
        assert_eq!((event.channel, event.rssi), (6, -55));
        assert!(convert_network(&wifi_scan::Wifi::default()).is_none());
        let mut n = network();
        n.ssid = "é".repeat(20);
        n.signal_level = -300;
        let event = convert_network(&n).unwrap();
        assert_eq!(event.ssid.len(), 32);
        assert_eq!(event.rssi, -128);
        n.ssid.clear();
        n.signal_level = 300;
        let event = convert_network(&n).unwrap();
        assert!(event.ssid.is_empty());
        assert_eq!(event.rssi, 127);
    }
    #[test]
    fn security_modes_are_preserved_in_csv_tags() {
        use WifiSecurity::*;
        for (sec, expected) in [
            (Open, "[OPEN]"),
            (Wep, "[WEP]"),
            (WpaPersonal, "[WPA-PSK]"),
            (Wpa2PersonalPsk, "[WPA2-PSK]"),
            (Wpa3PersonalSae, "[WPA3-SAE]"),
            (Wpa2EnterpriseEap, "[WPA2-EAP]"),
            (Wpa3EnterpriseEap256, "[WPA3-EAP256]"),
            (Wpa3EnterpriseSuiteBEap256, "[WPA3-SUITEB]"),
            (Wpa2EnterpriseEapFt, "[WPA2-EAP-FT]"),
            (Wpa3PersonalPsk256, "[WPA3-PSK256]"),
            (Wpa2PersonalPskFt, "[WPA2-PSK-FT]"),
            (Wpa3PersonalSaeFt, "[WPA3-SAE-FT]"),
            (WpaEnterprise, "[WPA-EAP]"),
            (Personal, "[PSK]"),
            (Enterprise, "[EAP]"),
            (Tdls, "[TDLS]"),
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
    #[tokio::test(start_paused = true)]
    async fn scan_recovers_from_failure_skips_redacted_results_and_stops_on_close() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut scans = std::collections::VecDeque::from([
            Err(wifi_scan::Error::ScanFailed("temporary".into())),
            Ok(vec![wifi_scan::Wifi::default()]),
            Ok(vec![network(), network()]),
        ]);
        let task = tokio::spawn(scan_loop_with(
            tx,
            crate::control::ScanControl::new(true),
            move || std::future::ready(Ok(scans.pop_front().unwrap_or(Ok(vec![])))),
        ));
        let ScanEvent::WiFi(event) = rx.recv().await.unwrap() else {
            panic!("expected WiFi");
        };
        assert_eq!(event.ssid.as_str(), "Flock-test");
        drop(rx);
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn closed_output_and_scan_task_failure_are_observable() {
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
    }
    #[tokio::test]
    async fn closed_public_scanner_does_not_start_a_hardware_scan() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        scan_loop(tx, crate::control::ScanControl::new(true))
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn blocking_worker_returns_results_and_reports_panics() {
        assert_eq!(blocking_scan(|| 42).await.unwrap(), 42);
        assert!(matches!(
            blocking_scan(|| panic!("test worker failure")).await,
            Err(ScanError::TaskPanicked)
        ));
        assert_eq!(
            ScanError::WorkerUnavailable.to_string(),
            "could not start WiFi worker"
        );
    }

    #[tokio::test]
    async fn shutdown_does_not_wait_for_a_blocked_native_scan() {
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
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
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
        done.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn pause_blocks_new_scans_and_discards_pre_pause_results() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let control = crate::control::ScanControl::new(false);
        let (tx, mut rx) = mpsc::channel(8);
        let task = tokio::spawn(scan_loop_with(tx, control.clone(), move || {
            counter.fetch_add(1, Ordering::Relaxed);
            async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(Ok(vec![network()]))
            }
        }));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        control.store(true, Ordering::Relaxed);
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        control.store(false, Ordering::Relaxed);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        control.store(true, Ordering::Relaxed);
        assert!(matches!(rx.recv().await, Some(ScanEvent::WiFi(_))));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        control.shutdown();
        task.await.unwrap().unwrap();
    }
}
