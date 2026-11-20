#![no_main]
use airhound::{ble_data, odid, scanner};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let ble = scanner::BleAdvParser::parse(&[0x12; 6], -65, data);
    assert!(ble.raw_ad.len() <= 62);
    let mut fields = ble_data::AdStructures::new(data);
    for field in fields.by_ref() {
        std::hint::black_box(field.manufacturer());
        std::hint::black_box(field.service());
        for uuid in field.uuids() {
            std::hint::black_box(uuid.as_u16());
        }
    }
    std::hint::black_box((fields.malformed(), fields.incomplete()));
    let _ = scanner::parse_wifi_frame(data, -65, 6);
    if let Some(event) = scanner::try_parse_odid_wifi(data, -65, 6) {
        let _ = odid::parse_odid_service_info(&event.raw_data);
    }
    let _ = odid::parse_odid_ble(data);
    let _ = odid::parse_odid_wifi_beacon(data);
    let _ = odid::parse_odid_service_info(data);
});
