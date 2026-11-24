//! Optional, bounded gpsd JSON input. Receiver protocol and NMEA checksums stay
//! with gpsd; this client sends WATCH only, never receiver configuration.
use chrono::{DateTime, Utc};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

mod managed;

const MAX_LINE: usize = 16 * 1024;
const MAX_AGE: Duration = Duration::from_secs(5);
const FUTURE_TOLERANCE_MS: i64 = 2_000;
const RETRY: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct Options {
    pub endpoint: String,
    pub device: Option<String>,
    pub managed_executable: Option<std::path::PathBuf>,
}
impl From<&airhound_hostd::session::LocationConfig> for Options {
    fn from(value: &airhound_hostd::session::LocationConfig) -> Self {
        match value {
            airhound_hostd::session::LocationConfig::Gpsd { endpoint, device } => Self {
                endpoint: endpoint.clone(),
                device: device.clone(),
                managed_executable: None,
            },
            airhound_hostd::session::LocationConfig::ManagedGpsd {
                endpoint,
                device,
                executable,
            } => Self {
                endpoint: endpoint.clone(),
                device: Some(device.clone()),
                managed_executable: Some(executable.clone()),
            },
        }
    }
}
#[derive(Clone)]
pub struct Source(Arc<Mutex<Tracker>>);
struct Fix {
    location: crate::wigle::Location,
    time: DateTime<Utc>,
    received: Instant,
}
struct Tracker {
    device: Option<String>,
    fix: Option<Fix>,
    last_time: Option<DateTime<Utc>>,
    reason: &'static str,
    protocol_ok: bool,
    daemon: Option<serde_json::Value>,
    #[cfg(test)]
    clock: Option<(DateTime<Utc>, Instant)>,
    #[cfg(test)]
    updates: Option<Arc<tokio::sync::Notify>>,
    #[cfg(test)]
    retry: Option<crate::test_support::TimerWitness>,
    #[cfg(test)]
    managed: Option<managed::TestHooks>,
}
impl Source {
    pub fn new(device: Option<String>) -> Self {
        Self(Arc::new(Mutex::new(Tracker {
            device,
            fix: None,
            last_time: None,
            reason: "connecting",
            protocol_ok: false,
            daemon: None,
            #[cfg(test)]
            clock: None,
            #[cfg(test)]
            updates: None,
            #[cfg(test)]
            retry: None,
            #[cfg(test)]
            managed: None,
        })))
    }
    fn now(&self) -> (DateTime<Utc>, Instant) {
        #[cfg(test)]
        if let Some(now) = self.0.lock().unwrap_or_else(|e| e.into_inner()).clock {
            return now;
        }
        (Utc::now(), Instant::now())
    }
    pub fn location(&self) -> Option<crate::wigle::Location> {
        let (wall, monotonic) = self.now();
        self.location_at(wall, monotonic)
    }
    fn location_at(
        &self,
        now: DateTime<Utc>,
        monotonic: Instant,
    ) -> Option<crate::wigle::Location> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state
            .fix
            .as_ref()
            .filter(|fix| fresh(fix, now, monotonic))
            .map(|fix| fix.location)
    }
    pub fn health(&self) -> serde_json::Value {
        let (wall, monotonic) = self.now();
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let reason = match &state.fix {
            Some(fix) if !fresh(fix, wall, monotonic) => "stale",
            _ => state.reason,
        };
        serde_json::json!({"source":"gpsd", "state":reason, "device":state.device,
            "daemon":state.daemon,
            "max_age_seconds":MAX_AGE.as_secs(),
            "fix_time":state.fix.as_ref().map(|fix| fix.time.to_rfc3339()),
            "altitude_datum":"mean_sea_level",
            "accuracy_source":"gpsd_eph_meters_confidence_unspecified", "association":"processing_time"})
    }
    /// One fresh snapshot, associated when the host processes the observation.
    /// Serial buffering and unsynchronized producer clocks prevent a capture-time claim.
    pub fn evidence(&self) -> Option<serde_json::Value> {
        let (now, received) = self.now();
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let fix = state.fix.as_ref().filter(|fix| fresh(fix, now, received))?;
        Some(serde_json::json!({
            "schema_version":1,"source":"gpsd","device":state.device,
            "position_of":"receiver_rig","association":"host_processing_time",
            "associated_at":now.to_rfc3339(),"fix_time":fix.time.to_rfc3339(),
            "fix_age_ms":now.signed_duration_since(fix.time).num_milliseconds(),
            "host_fix_age_ms":received.saturating_duration_since(fix.received).as_millis(),
            "max_age_ms":MAX_AGE.as_millis(),"capture_time_synchronized":false,
            "latitude":fix.location.latitude,"longitude":fix.location.longitude,
            "horizontal_datum":"WGS84","coordinate_units":"degrees",
            "altitude_msl_m":fix.location.altitude,"horizontal_accuracy_m":fix.location.accuracy,
            "accuracy_source":"gpsd_eph_meters_confidence_unspecified",
        }))
    }
    fn disconnected(&self, reason: &'static str) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.fix = None;
        state.reason = reason;
        state.protocol_ok = false;
        state.daemon = None;
        // Keep device binding and last fix time across reconnect: never switch
        // sources or refresh a replayed cached fix just because TCP restarted.
        #[cfg(test)]
        if let Some(updates) = &state.updates {
            updates.notify_one();
        }
    }
    fn line(&self, bytes: &[u8], now: DateTime<Utc>, received: Instant) -> io::Result<()> {
        let result = self.accept_line(bytes, now, received);
        #[cfg(test)]
        if let Some(updates) = &self.0.lock().unwrap_or_else(|e| e.into_inner()).updates {
            updates.notify_one();
        }
        result
    }
    fn accept_line(&self, bytes: &[u8], now: DateTime<Utc>, received: Instant) -> io::Result<()> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid gpsd JSON"))?;
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match value["class"].as_str() {
            Some("VERSION") => {
                state.protocol_ok = matches!(value["proto_major"].as_u64(), Some(3 | 16));
                if !state.protocol_ok {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unsupported gpsd protocol",
                    ));
                }
                state.daemon = Some(
                    serde_json::json!({"application":"gpsd","version":value["release"].as_str().filter(|version| version.len() <= 128 && !version.chars().any(char::is_control)),"protocol_major":value["proto_major"].as_u64(),"protocol_minor":value["proto_minor"].as_u64(),"evidence":"gpsd_version_message","receiver_firmware_version":null}),
                );
                state.reason = "waiting_for_fix";
            }
            Some("TPV") if state.protocol_ok => {
                let Some(device) = value["device"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 1024)
                else {
                    return Ok(());
                };
                if state
                    .device
                    .as_deref()
                    .is_some_and(|selected| selected != device)
                {
                    return Ok(());
                }
                if state.device.is_none() {
                    state.device = Some(device.to_owned());
                }
                let mode = value["mode"].as_u64().unwrap_or(0);
                if !matches!(mode, 2 | 3) {
                    state.fix = None;
                    state.reason = "no_fix";
                    return Ok(());
                }
                // Time-only, simulated and dead-reckoned positions are not a GNSS fix.
                if matches!(value["status"].as_u64(), Some(5 | 7 | 8)) {
                    state.fix = None;
                    state.reason = "non_gnss_fix";
                    return Ok(());
                }
                let parsed = parse_fix(&value, now, received);
                match parsed {
                    Some(fix) => {
                        if state.last_time.is_some_and(|last| fix.time <= last) {
                            return Ok(());
                        }
                        state.last_time = Some(fix.time);
                        state.fix = Some(fix);
                        state.reason = "fix";
                    }
                    None => {
                        state.fix = None;
                        state.reason = "invalid_or_stale_fix";
                    }
                }
            }
            Some("DEVICE") if value["activated"].as_f64() == Some(0.) => {
                if value["path"].as_str() == state.device.as_deref() {
                    state.fix = None;
                    state.reason = "device_disconnected";
                }
            }
            Some("ERROR") => return Err(io::Error::other("gpsd rejected request")),
            _ => {}
        }
        Ok(())
    }
}
fn fresh(fix: &Fix, now: DateTime<Utc>, received: Instant) -> bool {
    let age = now.signed_duration_since(fix.time).num_milliseconds();
    (-FUTURE_TOLERANCE_MS..=MAX_AGE.as_millis() as i64).contains(&age)
        && received.saturating_duration_since(fix.received) <= MAX_AGE
}
fn parse_fix(value: &serde_json::Value, now: DateTime<Utc>, received: Instant) -> Option<Fix> {
    let time = DateTime::parse_from_rfc3339(value["time"].as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let latitude = value["lat"].as_f64()?;
    let longitude = value["lon"].as_f64()?;
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return None;
    }
    let accuracy = value["eph"].as_f64().filter(|v| v.is_finite() && *v >= 0.);
    // WiGLE altitude uses MSL. Do not silently substitute ellipsoid height.
    let altitude = (value["mode"].as_u64() == Some(3))
        .then(|| value["altMSL"].as_f64())
        .flatten()
        .filter(|v| v.is_finite());
    let fix = Fix {
        location: crate::wigle::Location {
            latitude,
            longitude,
            accuracy,
            altitude,
        },
        time,
        received,
    };
    fresh(&fix, now, received).then_some(fix)
}

async fn session(options: &Options, source: &Source) -> io::Result<()> {
    let socket = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&options.endpoint),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "gpsd connection timeout"))??;
    consume_socket(socket, source).await
}
async fn consume_socket(mut socket: tokio::net::TcpStream, source: &Source) -> io::Result<()> {
    // Device filtering is local, avoiding request length/escaping ambiguity.
    socket
        .write_all(b"?WATCH={\"enable\":true,\"json\":true};\n")
        .await?;
    let mut pending = Vec::with_capacity(1024);
    let mut chunk = [0; 1024];
    loop {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "gpsd disconnected",
            ));
        }
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                if !pending.is_empty() {
                    let (wall, monotonic) = source.now();
                    source.line(&pending, wall, monotonic)?;
                }
                pending.clear();
            } else {
                if pending.len() == MAX_LINE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "gpsd response exceeds bound",
                    ));
                }
                pending.push(byte);
            }
        }
    }
}

pub async fn run(options: Options, source: Source, state: watch::Receiver<crate::control::State>) {
    if options.managed_executable.is_some() {
        managed::run(options, source, state).await;
    } else {
        run_external(options, source, state).await;
    }
}
async fn run_external(
    options: Options,
    source: Source,
    mut state: watch::Receiver<crate::control::State>,
) {
    loop {
        if state.borrow().shutdown {
            break;
        }
        let connected = session(&options, &source);
        tokio::pin!(connected);
        loop {
            tokio::select! {
                result = &mut connected => {
                    if let Err(error) = &result { log::warn!("gpsd connection ended: {error}"); }
                    source.disconnected(if result.is_err() { "disconnected" } else { "stopped" });
                    break;
                }
                changed = state.changed() => {
                    if changed.is_err() || state.borrow().shutdown { source.disconnected("stopped"); return }
                }
            }
        }
        let retry = tokio::time::sleep(RETRY);
        #[cfg(test)]
        let retry = async {
            let witness = source
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retry
                .clone();
            if let Some(witness) = witness {
                witness.observe(retry.deadline(), retry, |_| true).await;
            } else {
                retry.await;
            }
        };
        tokio::select! {
            _ = retry => {},
            changed = state.changed() => if changed.is_err() || state.borrow().shutdown { break },
        }
    }
    source.disconnected("stopped");
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn evidence_identifies_rig_processing_time_and_expires_with_the_fix() {
        let source = ready();
        feed(
            &source,
            serde_json::json!({"class":"VERSION","proto_major":16,"proto_minor":1,"release":"3.27.5"}),
        );
        feed(&source, tpv(wall()));
        let evidence = source.evidence().unwrap();
        assert_eq!(evidence["position_of"], "receiver_rig");
        assert_eq!(evidence["association"], "host_processing_time");
        assert_eq!(evidence["capture_time_synchronized"], false);
        assert_eq!(evidence["latitude"], 10.);
        assert_eq!(evidence["altitude_msl_m"], 15.);
        assert_eq!(evidence["horizontal_accuracy_m"], 2.);
        assert!(evidence["fix_time"].is_string());
        assert_eq!(source.health()["daemon"]["version"], "3.27.5");
        assert!(source.health()["daemon"]["receiver_firmware_version"].is_null());
        let (wall, mono) = source.now();
        set_clock(&source, wall, mono + Duration::from_secs(6));
        assert!(source.evidence().is_none());
        source.disconnected("disconnected");
        assert!(source.evidence().is_none());
        assert!(source.health()["daemon"].is_null());
    }
    fn wall() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
    pub(super) fn controlled_source(device: Option<String>) -> Source {
        let source = Source::new(device);
        // This instant is an opaque origin; tests advance their local pair
        // explicitly, never infer freshness from execution speed.
        set_clock(&source, wall(), Instant::now());
        source
    }
    pub(crate) fn ready() -> Source {
        let source = controlled_source(None);
        feed(
            &source,
            serde_json::json!({"class":"VERSION","proto_major":16}),
        );
        source
    }
    pub(crate) fn observe(source: &Source, updates: Arc<tokio::sync::Notify>) {
        source.0.lock().unwrap().updates = Some(updates);
    }
    pub(crate) fn set_clock(source: &Source, wall: DateTime<Utc>, monotonic: Instant) {
        source.0.lock().unwrap().clock = Some((wall, monotonic));
    }
    pub(crate) fn feed(source: &Source, value: serde_json::Value) {
        let (wall, monotonic) = source.now();
        source
            .line(&serde_json::to_vec(&value).unwrap(), wall, monotonic)
            .unwrap();
    }
    pub(crate) fn tpv(time: DateTime<Utc>) -> serde_json::Value {
        serde_json::json!({"class":"TPV","device":"synthetic","mode":3,
            "time":time.to_rfc3339(),"lat":10.,"lon":20.,"altMSL":15.,"eph":2.})
    }
    #[test]
    fn valid_fix_expires_by_both_clocks_and_preserves_units() {
        let source = ready();
        let now = wall();
        let mono = source.now().1;
        source
            .line(&serde_json::to_vec(&tpv(now)).unwrap(), now, mono)
            .unwrap();
        let fix = source.location_at(now, mono).unwrap();
        assert_eq!(
            (fix.latitude, fix.longitude, fix.altitude, fix.accuracy),
            (10., 20., Some(15.), Some(2.))
        );
        assert!(source
            .location_at(now + chrono::Duration::seconds(6), mono)
            .is_none());
        assert!(source
            .location_at(now, mono + Duration::from_secs(6))
            .is_none());
        assert!(source
            .location_at(now - chrono::Duration::seconds(3), mono)
            .is_none());
        assert!(!source.health().to_string().contains("latitude"));
    }
    #[test]
    fn rejects_invalid_missing_stale_and_non_gnss_fixes() {
        for (key, value) in [
            ("mode", serde_json::json!(1)),
            ("status", serde_json::json!(5)),
            ("status", serde_json::json!(7)),
            ("status", serde_json::json!(8)),
            ("lat", serde_json::json!(91)),
            ("lon", serde_json::json!(-181)),
            ("lat", serde_json::Value::Null),
            ("time", serde_json::json!("bad")),
            (
                "time",
                serde_json::json!((wall() - chrono::Duration::seconds(10)).to_rfc3339()),
            ),
            (
                "time",
                serde_json::json!((wall() + chrono::Duration::seconds(10)).to_rfc3339()),
            ),
        ] {
            let source = ready();
            feed(&source, tpv(wall()));
            let mut invalid = tpv(wall());
            invalid[key] = value;
            feed(&source, invalid);
            assert!(source.location().is_none(), "accepted invalid {key}");
        }
    }
    #[test]
    fn optional_fields_are_not_invented_and_zero_is_a_real_coordinate() {
        let source = ready();
        let mut value = tpv(wall());
        value["mode"] = 2.into();
        value["lat"] = 0.into();
        value["lon"] = 0.into();
        value["eph"] = (-1).into();
        value["hdop"] = 0.8.into();
        value["altHAE"] = 100.into();
        feed(&source, value);
        let fix = source.location().unwrap();
        assert_eq!(
            (fix.latitude, fix.longitude, fix.altitude, fix.accuracy),
            (0., 0., None, None)
        );
        let mut output = Vec::new();
        {
            let mut writer = crate::wigle::WigleWriter::new(&mut output).unwrap();
            writer
                .write_wifi("synthetic", "test", "", "test", 1, 2412, -50, Some(&fix))
                .unwrap();
            writer
                .write_wifi("synthetic", "test", "", "test", 1, 2412, -50, None)
                .unwrap();
            writer.flush().unwrap();
        }
        let text = String::from_utf8(output).unwrap();
        let rows: Vec<_> = text
            .lines()
            .skip(2)
            .map(|line| line.split(',').collect::<Vec<_>>())
            .collect();
        assert_eq!(&rows[0][9..11], &["", ""]);
        assert_eq!(&rows[1][7..11], &["", "", "", ""]);
    }
    #[test]
    fn device_binding_reconnect_and_time_ordering_prevent_cached_refresh() {
        let source = ready();
        let now = wall();
        feed(&source, tpv(now));
        let mut other = tpv(now + chrono::Duration::milliseconds(1));
        other["device"] = "other".into();
        other["lat"] = 30.into();
        feed(&source, other);
        assert_eq!(source.location().unwrap().latitude, 10.);
        source.disconnected("disconnected");
        feed(&source, tpv(now)); // VERSION required on each connection
        assert!(source.location().is_none());
        feed(
            &source,
            serde_json::json!({"class":"VERSION","proto_major":16}),
        );
        feed(&source, tpv(now));
        feed(&source, tpv(now - chrono::Duration::milliseconds(1)));
        assert!(source.location().is_none());
        feed(&source, tpv(now + chrono::Duration::milliseconds(1)));
        assert!(source.location().is_some());
        feed(
            &source,
            serde_json::json!({"class":"DEVICE","path":"synthetic","activated":0}),
        );
        assert!(source.location().is_none());
        assert_eq!(source.health()["state"], "device_disconnected");
    }
    #[test]
    fn legacy_and_current_protocol_versions_accept_fixes() {
        for version in [3, 16] {
            let source = controlled_source(None);
            feed(
                &source,
                serde_json::json!({"class":"VERSION", "proto_major":version}),
            );
            feed(&source, tpv(wall()));
            assert!(source.location().is_some());
        }
    }

    #[test]
    fn malformed_protocol_and_partial_fixes_fail_closed() {
        let source = ready();
        assert!(source.line(b"bad", wall(), source.now().1).is_err());
        assert!(source
            .line(
                br#"{"class":"VERSION","proto_major":4}"#,
                wall(),
                source.now().1
            )
            .is_err());
        assert!(source
            .line(br#"{"class":"ERROR"}"#, wall(), source.now().1)
            .is_err());
        let source = ready();
        feed(&source, tpv(wall()));
        feed(
            &source,
            serde_json::json!({"class":"TPV","device":"synthetic","mode":3,"eph":1}),
        );
        assert!(source.location().is_none());
        let selected = controlled_source(Some("wanted".into()));
        feed(
            &selected,
            serde_json::json!({"class":"VERSION","proto_major":16}),
        );
        feed(&selected, tpv(wall()));
        assert!(selected.location().is_none());
    }
    #[tokio::test]
    async fn bounded_transport_and_orderly_shutdown() {
        crate::test_support::manual_time(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let options = Options {
                managed_executable: None,
                endpoint: listener.local_addr().unwrap().to_string(),
                device: None,
            };
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let expected = b"?WATCH={\"enable\":true,\"json\":true};\n";
                let mut request = vec![0; expected.len()];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(request, expected);
                socket.write_all(&vec![b'x'; MAX_LINE + 1]).await.unwrap();
            });
            assert_eq!(
                session(&options, &ready()).await.unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            server.await.unwrap();
            let control = crate::control::ScanControl::new(true);
            let source = ready();
            let task = tokio::spawn(run(options, source.clone(), control.subscribe()));
            control.shutdown();
            task.await.unwrap();
            assert_eq!(source.health()["state"], "stopped");
        })
        .await;
    }
    #[tokio::test]
    async fn reconnects_after_eof_and_cancels_an_idle_connection() {
        crate::test_support::manual_time(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let options = Options {
                managed_executable: None,
                endpoint: listener.local_addr().unwrap().to_string(),
                device: None,
            };
            let control = crate::control::ScanControl::new(true);
            let source = ready();
            let (retry, mut retry_rx) = crate::test_support::TimerWitness::channel();
            source.0.lock().unwrap().retry = Some(retry);
            let updates = Arc::new(tokio::sync::Notify::new());
            source.0.lock().unwrap().updates = Some(updates.clone());
            let task = tokio::spawn(run(options, source.clone(), control.subscribe()));
            let (first, _) = listener.accept().await.unwrap();
            drop(first);
            crate::test_support::advance_registered_timer(&mut retry_rx).await;
            let (mut second, _) = listener.accept().await.unwrap();
            let mut stream = b"{\"class\":\"VERSION\",\"proto_major\":16}\n".to_vec();
            stream.extend(serde_json::to_vec(&tpv(wall())).unwrap());
            stream.push(b'\n');
            second.write_all(&stream).await.unwrap();
            // manual_time supplies the real failure watchdog; freshness
            // and reconnect behavior use only the controlled clocks/events.
            while source.location().is_none() {
                updates.notified().await;
            }
            assert!(control.external_location().is_none());
            control.set_location_source(source.clone());
            assert!(control.external_location().unwrap().is_some());
            assert_eq!(control.health_message()["location"]["source"], "gpsd");
            control.shutdown();
            task.await.unwrap();
            assert!(control.external_location().unwrap().is_none());
        })
        .await;
    }
}
