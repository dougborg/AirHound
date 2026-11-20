//! Decode one normalized Remote ID service-data record, never a fabricated AD.
//! OS snapshots can be cached: a decoded frame is not a fresh RF packet claim.
use std::io::{self, Write};

const REMOTE_ID_UUID: [u8; 16] = [
    0, 0, 0xff, 0xfa, 0, 0, 0x10, 0, 0x80, 0, 0, 0x80, 0x5f, 0x9b, 0x34, 0xfb,
];

pub struct Context<'a> {
    pub peripheral_id: &'a str,
    pub identity_scope: &'a str,
    pub address: Option<&'a str>,
    pub address_type: &'a str,
    pub receiver_id: Option<&'a str>,
    pub rssi: Option<i8>,
    pub ts: u32,
}

#[derive(serde::Serialize)]
struct Observation<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    schema_version: u8,
    peripheral_id: &'a str,
    identity_scope: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<&'a str>,
    address_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    receiver_id: Option<&'a str>,
    source: &'static str,
    provenance: &'static str,
    freshness: &'static str,
    packet_completeness: &'static str,
    message_counter: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    rssi: Option<i8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uas_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ua_type: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lon: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operator_id: Option<&'a str>,
    ts: u32,
}

/// `payload` excludes the UUID and starts with application code 0x0D, then
/// the message counter and Remote ID message/pack. Reject locally truncated
/// records. Unknown OS packet completeness remains explicit in the output.
/// The caller applies BLE-enabled/RSSI policy, independently of signature hits.
pub fn emit(
    output: &mut impl Write,
    context: &Context<'_>,
    service_uuid: &[u8; 16],
    payload: &[u8],
    truncated: bool,
) -> io::Result<bool> {
    if truncated || service_uuid != &REMOTE_ID_UUID || payload.first() != Some(&0x0d) {
        return Ok(false);
    }
    let Some(frame) = airhound::odid::parse_odid_service_info(&payload[1..]) else {
        return Ok(false);
    };
    let observation = Observation {
        kind: "ble_remote_id",
        schema_version: 1,
        peripheral_id: context.peripheral_id,
        identity_scope: context.identity_scope,
        address: context.address,
        address_type: context.address_type,
        receiver_id: context.receiver_id,
        source: "ble_service_data",
        provenance: "os_property_snapshot",
        freshness: "possibly_cached",
        packet_completeness: "unknown",
        message_counter: payload[1],
        rssi: context.rssi,
        uas_id: frame.basic_id.as_ref().map(|id| id.uas_id.as_str()),
        ua_type: frame.basic_id.as_ref().map(|id| id.ua_type as u8),
        lat: frame.location.as_ref().map(|l| l.latitude),
        lon: frame.location.as_ref().map(|l| l.longitude),
        alt: frame.location.as_ref().map(|l| l.altitude_geo),
        speed: frame.location.as_ref().map(|l| l.speed_horizontal),
        operator_id: frame.operator_id.as_ref().map(|id| id.operator_id.as_str()),
        ts: context.ts,
    };
    serde_json::to_writer(&mut *output, &observation).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    Ok(true)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    fn context() -> Context<'static> {
        Context {
            peripheral_id: "test-peripheral",
            identity_scope: "corebluetooth",
            address: None,
            address_type: "unknown",
            receiver_id: Some("test-receiver"),
            rssi: Some(-45),
            ts: 123,
        }
    }
    fn basic_id(counter: u8) -> Vec<u8> {
        let mut data = vec![0; 27];
        data[0] = 0x0d;
        data[1] = counter;
        data[2] = 0x02;
        data[3] = 0x12;
        data[4..8].copy_from_slice(b"TEST");
        data
    }
    #[test]
    fn legacy_and_changing_records_preserve_opaque_identity_without_mac() {
        let mut out = Vec::new();
        for counter in [1, 1, 2] {
            assert!(emit(
                &mut out,
                &context(),
                &REMOTE_ID_UUID,
                &basic_id(counter),
                false
            )
            .unwrap());
        }
        let values: Vec<serde_json::Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(values.len(), 3);
        assert_eq!(values[2]["message_counter"], 2);
        assert_eq!(values[0]["uas_id"], "TEST");
        assert_eq!(values[0]["peripheral_id"], "test-peripheral");
        assert_eq!(values[0]["receiver_id"], "test-receiver");
        assert!(values[0].get("address").is_none());
        assert!(values[0].get("mac").is_none());
        assert_eq!(values[0]["packet_completeness"], "unknown");
        assert_eq!(values[0]["freshness"], "possibly_cached");
    }
    #[test]
    fn unavailable_signal_is_not_fabricated_as_zero_dbm() {
        let mut context = context();
        context.rssi = None;
        let mut out = Vec::new();
        assert!(emit(&mut out, &context, &REMOTE_ID_UUID, &basic_id(1), false).unwrap());
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(value.get("rssi").is_none());
    }

    #[test]
    fn wrong_context_and_truncation_do_not_decode() {
        let mut out = Vec::new();
        assert!(!emit(&mut out, &context(), &[0; 16], &basic_id(1), false).unwrap());
        assert!(!emit(&mut out, &context(), &REMOTE_ID_UUID, &basic_id(1), true).unwrap());
        let mut wrong_app = basic_id(1);
        wrong_app[0] = 0;
        assert!(!emit(&mut out, &context(), &REMOTE_ID_UUID, &wrong_app, false).unwrap());
        for end in 0..27 {
            assert!(!emit(
                &mut out,
                &context(),
                &REMOTE_ID_UUID,
                &basic_id(1)[..end],
                false
            )
            .unwrap());
        }
        assert!(out.is_empty());
    }
    // Public Open Drone ID odid_bt5_lr_sample.pcapng fixture, Apache-2.0.
    // Same source as core parser test; UUID/AD framing removed, no local capture.
    #[test]
    fn complete_long_range_fixture_decodes_without_the_firmware_buffer() {
        let hex = include_str!("../tests/fixtures/remote-id-service-data.hex").trim();
        let data: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert!(data.len() > 62);
        let mut out = Vec::new();
        assert!(emit(&mut out, &context(), &REMOTE_ID_UUID, &data, false).unwrap());
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["uas_id"], "SSEVTFG93700070");
        assert_eq!(value["source"], "ble_service_data");
        assert!(value.get("lat").is_some());
        assert!(!emit(&mut Vec::new(), &context(), &REMOTE_ID_UUID, &data, true).unwrap());
    }

    #[test]
    fn writer_failure_is_returned() {
        struct Fails;
        impl Write for Fails {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("failed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(emit(&mut Fails, &context(), &REMOTE_ID_UUID, &basic_id(1), false).is_err());
    }
}
