//! WiGLE CSV v1.6 writer.
//!
//! Observation-based: each row is one sighting at a point in time and space.
//! The same BSSID seen from different locations/times produces multiple rows.
//!
//! Uses the `csv` crate for correct field escaping and column alignment.

use std::io::{self, Write};

/// A GPS fix for WiGLE CSV rows. Platform-agnostic — populated by
/// `location_macos` on macOS, `None` elsewhere.
#[derive(Debug, Clone, Copy)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
    pub altitude: f64,
}

/// WiGLE CSV v1.6 writer.
///
/// The first header row is non-standard metadata (not a valid CSV record),
/// so we write it raw and use `csv::Writer` for the column header + data rows.
pub struct WigleWriter<W: Write> {
    writer: csv::Writer<W>,
}

impl<W: Write> WigleWriter<W> {
    /// Create a new writer and emit the two WiGLE header rows.
    pub fn new(mut inner: W) -> io::Result<Self> {
        // Row 1: WiGLE metadata (non-standard, written raw)
        writeln!(
            inner,
            "WigleWifi-1.6,appRelease=AirHound-{version},model=host,release={version},device=AirHound,display=,board=,brand=",
            version = env!("CARGO_PKG_VERSION"),
        )?;
        inner.flush()?;

        // Row 2: column headers (written via csv::Writer so field count is locked in)
        let mut writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(inner);
        writer.write_record(COLUMNS).map_err(io::Error::other)?;
        writer.flush().map_err(io::Error::other)?;
        Ok(Self { writer })
    }

    /// Write a WiFi observation row.
    #[allow(clippy::too_many_arguments)]
    pub fn write_wifi(
        &mut self,
        mac: &str,
        ssid: &str,
        auth: &str,
        seen: &str,
        channel: u8,
        frequency: u32,
        rssi: i8,
        location: Option<&Location>,
    ) -> io::Result<()> {
        let ch = channel.to_string();
        let freq = frequency.to_string();
        let rssi_s = rssi.to_string();
        let (lat, lon, alt, acc) = format_location(location);
        self.writer
            .write_record([
                mac, ssid, auth, seen, &ch, &freq, &rssi_s, &lat, &lon, &alt, &acc, "", "", "WIFI",
            ])
            .map_err(io::Error::other)
    }

    /// Write a BLE observation row.
    pub fn write_ble(
        &mut self,
        mac: &str,
        name: &str,
        seen: &str,
        rssi: i8,
        mfr_id: u16,
        location: Option<&Location>,
    ) -> io::Result<()> {
        let rssi_s = rssi.to_string();
        let (lat, lon, alt, acc) = format_location(location);
        let mfr_str = if mfr_id != 0 {
            format!("0x{mfr_id:04X}")
        } else {
            String::new()
        };
        self.writer
            .write_record([
                mac, name, "[LE]", seen, "0", "0", &rssi_s, &lat, &lon, &alt, &acc, "", &mfr_str,
                "BLE",
            ])
            .map_err(io::Error::other)
    }

    /// Flush buffered writes to the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush().map_err(io::Error::other)
    }
}

/// The 14 WiGLE CSV v1.6 column names.
const COLUMNS: &[&str] = &[
    "MAC",
    "SSID",
    "AuthMode",
    "FirstSeen",
    "Channel",
    "Frequency",
    "RSSI",
    "CurrentLatitude",
    "CurrentLongitude",
    "AltitudeMeters",
    "AccuracyMeters",
    "RCOIs",
    "MfgrId",
    "Type",
];

/// Format GPS location fields, returning empty strings when unavailable.
fn format_location(location: Option<&Location>) -> (String, String, String, String) {
    match location {
        Some(loc) => (
            loc.latitude.to_string(),
            loc.longitude.to_string(),
            loc.altitude.to_string(),
            loc.accuracy.to_string(),
        ),
        None => (String::new(), String::new(), String::new(), String::new()),
    }
}

/// Convert WiFi channel number to frequency in MHz.
pub fn channel_to_frequency(ch: u8) -> u32 {
    match ch as u32 {
        1..=13 => 2407 + ch as u32 * 5,
        14 => 2484,
        36..=177 => 5000 + ch as u32 * 5,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_to_freq_2ghz() {
        assert_eq!(channel_to_frequency(1), 2412);
        assert_eq!(channel_to_frequency(6), 2437);
        assert_eq!(channel_to_frequency(11), 2462);
        assert_eq!(channel_to_frequency(13), 2472);
    }

    #[test]
    fn channel_to_freq_5ghz() {
        assert_eq!(channel_to_frequency(36), 5180);
        assert_eq!(channel_to_frequency(149), 5745);
    }

    #[test]
    fn channel_to_freq_14() {
        assert_eq!(channel_to_frequency(14), 2484);
    }

    #[test]
    fn channel_to_freq_unknown() {
        assert_eq!(channel_to_frequency(0), 0);
        assert_eq!(channel_to_frequency(200), 0);
    }

    #[test]
    fn wigle_header_format() {
        let mut buf = Vec::new();
        {
            let _writer = WigleWriter::new(&mut buf).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.starts_with("WigleWifi-1.6,"));
        assert!(output.contains("MAC,SSID,AuthMode,FirstSeen,Channel,Frequency,RSSI,"));
    }

    #[test]
    fn write_wifi_with_location() {
        let mut buf = Vec::new();
        {
            let mut w = WigleWriter::new(&mut buf).unwrap();
            let loc = Location {
                latitude: 37.7749,
                longitude: -122.4194,
                altitude: 10.0,
                accuracy: 5.0,
            };
            w.write_wifi(
                "AA:BB:CC:DD:EE:FF",
                "TestNet",
                "[WPA2-PSK]",
                "2024-01-01 12:00:00",
                6,
                2437,
                -65,
                Some(&loc),
            )
            .unwrap();
            w.flush().unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("AA:BB:CC:DD:EE:FF,TestNet,[WPA2-PSK],"));
        assert!(output.contains("37.7749"));
        assert!(output.contains(",WIFI\n"));
    }

    #[test]
    fn write_wifi_without_location() {
        let mut buf = Vec::new();
        {
            let mut w = WigleWriter::new(&mut buf).unwrap();
            w.write_wifi(
                "AA:BB:CC:DD:EE:FF",
                "TestNet",
                "[WPA2-PSK]",
                "2024-01-01 12:00:00",
                6,
                2437,
                -65,
                None,
            )
            .unwrap();
            w.flush().unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains(",WIFI\n"));
    }

    #[test]
    fn write_ble_with_mfr() {
        let mut buf = Vec::new();
        {
            let mut w = WigleWriter::new(&mut buf).unwrap();
            w.write_ble(
                "11:22:33:44:55:66",
                "TestBLE",
                "2024-01-01 12:00:00",
                -72,
                0x004C,
                None,
            )
            .unwrap();
            w.flush().unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("0x004C,BLE\n"));
    }

    #[test]
    fn csv_escapes_ssid_with_comma() {
        let mut buf = Vec::new();
        {
            let mut w = WigleWriter::new(&mut buf).unwrap();
            w.write_wifi(
                "AA:BB:CC:DD:EE:FF",
                "Net,work",
                "[OPEN]",
                "2024-01-01 12:00:00",
                1,
                2412,
                -50,
                None,
            )
            .unwrap();
            w.flush().unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        // csv crate wraps the SSID in quotes
        assert!(output.contains("\"Net,work\""));
    }

    /// Verify all row variants produce exactly 14 CSV fields (matching the header).
    #[test]
    fn all_rows_have_14_fields() {
        let loc = Location {
            latitude: 1.0,
            longitude: 2.0,
            altitude: 3.0,
            accuracy: 4.0,
        };

        let cases: Vec<(&str, Box<dyn Fn(&mut WigleWriter<&mut Vec<u8>>)>)> = vec![
            (
                "wifi+loc",
                Box::new(|w| {
                    w.write_wifi("M", "S", "A", "T", 6, 2437, -50, Some(&loc))
                        .unwrap();
                }),
            ),
            (
                "wifi-no-loc",
                Box::new(|w| {
                    w.write_wifi("M", "S", "A", "T", 6, 2437, -50, None)
                        .unwrap();
                }),
            ),
            (
                "ble+loc+mfr",
                Box::new(|w| {
                    w.write_ble("M", "N", "T", -50, 0x004C, Some(&loc)).unwrap();
                }),
            ),
            (
                "ble+loc-no-mfr",
                Box::new(|w| {
                    w.write_ble("M", "N", "T", -50, 0, Some(&loc)).unwrap();
                }),
            ),
            (
                "ble-no-loc+mfr",
                Box::new(|w| {
                    w.write_ble("M", "N", "T", -50, 0x004C, None).unwrap();
                }),
            ),
            (
                "ble-no-loc-no-mfr",
                Box::new(|w| {
                    w.write_ble("M", "N", "T", -50, 0, None).unwrap();
                }),
            ),
        ];

        for (label, write_fn) in &cases {
            let mut buf = Vec::new();
            {
                let mut w = WigleWriter::new(&mut buf).unwrap();
                write_fn(&mut w);
                w.flush().unwrap();
            }
            let output = String::from_utf8(buf).unwrap();
            let lines: Vec<&str> = output.lines().collect();
            // Header (2 lines) + data (1 line)
            assert_eq!(lines.len(), 3, "{label}: expected 3 lines");
            let header_fields = lines[1].split(',').count();
            let data_fields = lines[2].split(',').count();
            assert_eq!(
                header_fields, data_fields,
                "{label}: header has {header_fields} fields but data row has {data_fields}: {:?}",
                lines[2]
            );
        }
    }
}
