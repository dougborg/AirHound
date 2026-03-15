//! WiFi scanner for macOS using `wifi_scan` crate (CoreWLAN wrapper).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;

use airhound::comm;
use airhound::scanner::{FrameType, ScanEvent, WiFiEvent};
use wifi_scan::WifiSecurity;

/// Polling interval between WiFi scans.
/// CoreWLAN active scans take ~3-5s, so effective cycle is ~4-6s.
const SCAN_INTERVAL: Duration = Duration::from_secs(1);

/// Whether we've already warned about missing Location Services.
static LOCATION_WARNING_LOGGED: AtomicBool = AtomicBool::new(false);

/// Run the WiFi scan loop, periodically scanning via CoreWLAN and sending WiFiEvents.
///
/// Sends discovered events on `tx`. Runs until the channel is closed or an error occurs.
pub async fn scan_loop(tx: mpsc::Sender<ScanEvent>) -> Result<(), ScanError> {
    log::info!("WiFi scan started (CoreWLAN via wifi_scan, interval {SCAN_INTERVAL:?})");

    loop {
        let networks = tokio::task::spawn_blocking(wifi_scan::scan)
            .await
            .map_err(|_| ScanError::TaskPanicked)?;

        match networks {
            Ok(wifis) => {
                // Check for Location Services on first scan
                if !wifis.is_empty()
                    && !LOCATION_WARNING_LOGGED.load(Ordering::Relaxed)
                    && wifis.iter().all(|w| w.mac.is_empty())
                {
                    log::warn!(
                        "WiFi scan: all BSSIDs empty — grant Location Services permission \
                         to the terminal app in System Settings → Privacy & Security → Location Services"
                    );
                    LOCATION_WARNING_LOGGED.store(true, Ordering::Relaxed);
                }

                let mut count = 0u32;
                for wifi in &wifis {
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

        tokio::time::sleep(SCAN_INTERVAL).await;
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
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::TaskPanicked => write!(f, "scan task panicked"),
        }
    }
}

// MAC parsing tests live in the library (airhound::comm::parse_mac_string).
