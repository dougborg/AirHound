//! Optional, bounded gpsd JSON input. Receiver protocol and NMEA checksums stay
//! with gpsd; this client sends WATCH only, never receiver configuration.
use chrono::{DateTime, Utc};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

const MAX_LINE: usize = 16 * 1024;
const MAX_AGE: Duration = Duration::from_secs(5);
const FUTURE_TOLERANCE_MS: i64 = 2_000;
const RETRY: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct Options {
    pub endpoint: String,
    pub device: Option<String>,
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
}
impl Source {
    pub fn new(device: Option<String>) -> Self {
        Self(Arc::new(Mutex::new(Tracker {
            device,
            fix: None,
            last_time: None,
            reason: "connecting",
            protocol_ok: false,
        })))
    }
    pub fn location(&self) -> Option<crate::wigle::Location> {
        self.location_at(Utc::now(), Instant::now())
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
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let reason = match &state.fix {
            Some(fix) if !fresh(fix, Utc::now(), Instant::now()) => "stale",
            _ => state.reason,
        };
        serde_json::json!({"source":"gpsd", "state":reason, "device":state.device,
            "max_age_seconds":MAX_AGE.as_secs(),
            "fix_time":state.fix.as_ref().map(|fix| fix.time.to_rfc3339()),
            "altitude_datum":"mean_sea_level",
            "accuracy_source":"gpsd_eph_meters_confidence_unspecified", "association":"processing_time"})
    }
    fn disconnected(&self, reason: &'static str) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.fix = None;
        state.reason = reason;
        state.protocol_ok = false;
        // Keep device binding and last fix time across reconnect: never switch
        // sources or refresh a replayed cached fix just because TCP restarted.
    }
    fn line(&self, bytes: &[u8], now: DateTime<Utc>, received: Instant) -> io::Result<()> {
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
    let mut socket = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&options.endpoint),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "gpsd connection timeout"))??;
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
                    source.line(&pending, Utc::now(), Instant::now())?;
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

pub async fn run(
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
        tokio::select! {
            _ = tokio::time::sleep(RETRY) => {},
            changed = state.changed() => if changed.is_err() || state.borrow().shutdown { break },
        }
    }
    source.disconnected("stopped");
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    fn ready() -> Source {
        let source = Source::new(None);
        feed(
            &source,
            serde_json::json!({"class":"VERSION","proto_major":16}),
        );
        source
    }
    fn feed(source: &Source, value: serde_json::Value) {
        source
            .line(
                &serde_json::to_vec(&value).unwrap(),
                Utc::now(),
                Instant::now(),
            )
            .unwrap();
    }
    fn tpv(time: DateTime<Utc>) -> serde_json::Value {
        serde_json::json!({"class":"TPV","device":"synthetic","mode":3,
            "time":time.to_rfc3339(),"lat":10.,"lon":20.,"altMSL":15.,"eph":2.})
    }
    #[test]
    fn valid_fix_expires_by_both_clocks_and_preserves_units() {
        let source = ready();
        let now = Utc::now();
        let mono = Instant::now();
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
                serde_json::json!((Utc::now() - chrono::Duration::seconds(10)).to_rfc3339()),
            ),
            (
                "time",
                serde_json::json!((Utc::now() + chrono::Duration::seconds(10)).to_rfc3339()),
            ),
        ] {
            let source = ready();
            feed(&source, tpv(Utc::now()));
            let mut invalid = tpv(Utc::now());
            invalid[key] = value;
            feed(&source, invalid);
            assert!(source.location().is_none(), "accepted invalid {key}");
        }
    }
    #[test]
    fn optional_fields_are_not_invented_and_zero_is_a_real_coordinate() {
        let source = ready();
        let mut value = tpv(Utc::now());
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
        let now = Utc::now();
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
            let source = Source::new(None);
            feed(
                &source,
                serde_json::json!({"class":"VERSION", "proto_major":version}),
            );
            feed(&source, tpv(Utc::now()));
            assert!(source.location().is_some());
        }
    }

    #[test]
    fn malformed_protocol_and_partial_fixes_fail_closed() {
        let source = ready();
        assert!(source.line(b"bad", Utc::now(), Instant::now()).is_err());
        assert!(source
            .line(
                br#"{"class":"VERSION","proto_major":4}"#,
                Utc::now(),
                Instant::now()
            )
            .is_err());
        assert!(source
            .line(br#"{"class":"ERROR"}"#, Utc::now(), Instant::now())
            .is_err());
        let source = ready();
        feed(&source, tpv(Utc::now()));
        feed(
            &source,
            serde_json::json!({"class":"TPV","device":"synthetic","mode":3,"eph":1}),
        );
        assert!(source.location().is_none());
        let selected = Source::new(Some("wanted".into()));
        feed(
            &selected,
            serde_json::json!({"class":"VERSION","proto_major":16}),
        );
        feed(&selected, tpv(Utc::now()));
        assert!(selected.location().is_none());
    }
    #[tokio::test]
    async fn bounded_transport_and_orderly_shutdown() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let options = Options {
            endpoint: listener.local_addr().unwrap().to_string(),
            device: None,
        };
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 35];
            let n = socket.read(&mut request).await.unwrap();
            assert!(std::str::from_utf8(&request[..n])
                .unwrap()
                .starts_with("?WATCH="));
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
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.health()["state"], "stopped");
    }
    #[tokio::test]
    async fn reconnects_after_eof_and_cancels_an_idle_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let options = Options {
            endpoint: listener.local_addr().unwrap().to_string(),
            device: None,
        };
        let control = crate::control::ScanControl::new(true);
        let source = ready();
        let task = tokio::spawn(run(options, source.clone(), control.subscribe()));
        let (first, _) = listener.accept().await.unwrap();
        drop(first);
        let (mut second, _) = tokio::time::timeout(Duration::from_secs(4), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut stream = b"{\"class\":\"VERSION\",\"proto_major\":16}\n".to_vec();
        stream.extend(serde_json::to_vec(&tpv(Utc::now())).unwrap());
        stream.push(b'\n');
        second.write_all(&stream).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while source.location().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(control.external_location().is_none());
        control.set_location_source(source.clone());
        assert!(control.external_location().unwrap().is_some());
        assert_eq!(control.health_message()["location"]["source"], "gpsd");
        control.shutdown();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(control.external_location().unwrap().is_none());
    }
}
