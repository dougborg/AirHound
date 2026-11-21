use airhound_flipper::{Event, MAX_LINE, ObservationKind, Receiver, Rejection};
fn record(r: &mut Receiver, bytes: &[u8]) -> Vec<Event> {
    bytes
        .iter()
        .copied()
        .chain(*b"\n")
        .filter_map(|b| r.feed(b))
        .collect()
}
fn compact(fixture: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::from_str::<serde_json::Value>(fixture).unwrap()).unwrap()
}
const WIFI: &str = include_str!("../../../schemas/examples/wifi-observation.device.json");
const BLE: &str = include_str!("../../../schemas/examples/ble-observation.device.json");
const EVIDENCE: &str = include_str!("../../../schemas/examples/evidence.device.json");
#[test]
fn shared_fixtures_preserve_ordinary_observations_and_unassociated_evidence() {
    let mut r = Receiver::new();
    r.begin_session();
    assert!(
        matches!(record(&mut r, &compact(WIFI)).as_slice(), [Event::Observation(o)] if o.kind == ObservationKind::AccessPoint && !o.signature_match)
    );
    assert!(
        matches!(record(&mut r, &compact(BLE)).as_slice(), [Event::Observation(o)] if o.kind == ObservationKind::Ble && !o.signature_match)
    );
    assert!(
        matches!(record(&mut r, &compact(EVIDENCE)).as_slice(), [Event::Evidence { detection, .. }] if detection.sig.as_slice() == [0,117,120,123])
    );
    assert_eq!(r.counts.access_point_observations, 1);
    assert_eq!(r.counts.ble_observations, 1);
    assert_eq!(r.counts.signature_observations, 0);
    assert_eq!(r.counts.evidence_records, 1);
}
#[test]
fn arbitrary_chunking_logs_crlf_and_escaped_strings() {
    let mut r = Receiver::new();
    r.begin_session();
    assert!(record(&mut r, b"INFO radio ready").is_empty());
    let mut wifi: serde_json::Value = serde_json::from_str(WIFI).unwrap();
    wifi["ssid"] = "quote\" and \\ and é".into();
    wifi["frame"] = "probe_req".into();
    wifi["future"] = serde_json::json!({"nested":[true, null, 1.25]});
    let mut line = b"INFO (123) sensor: ".to_vec();
    line.extend(serde_json::to_vec(&wifi).unwrap());
    line.extend(b"\r\n");
    let mut events = Vec::new();
    for chunk in line.chunks(7) {
        for b in chunk {
            if let Some(e) = r.feed(*b) {
                events.push(e);
            }
        }
    }
    assert!(
        matches!(&events[..], [Event::Observation(o)] if o.name == "quote\" and \\ and é" && o.kind == ObservationKind::OtherWifi)
    );
}
#[test]
fn oversize_discards_through_newline_and_then_recovers() {
    let mut r = Receiver::new();
    r.begin_session();
    let mut bad = vec![b'x'; MAX_LINE + 1];
    bad.extend(compact(WIFI));
    assert_eq!(
        record(&mut r, &bad),
        [Event::Rejected(Rejection::Oversized)]
    );
    assert_eq!(r.counts.access_point_observations, 0);
    assert_eq!(r.counts.oversized_records, 1);
    assert!(matches!(
        record(&mut r, &compact(WIFI)).as_slice(),
        [Event::Observation(_)]
    ));
    let too_large = serde_json::json!({"type":"future","padding":"x".repeat(510)});
    assert_eq!(
        record(&mut r, &serde_json::to_vec(&too_large).unwrap()),
        [Event::Rejected(Rejection::Oversized)]
    );
}
#[test]
fn malformed_known_unknown_and_deep_records_recover() {
    let mut r = Receiver::new();
    r.begin_session();
    for bad in [
        r#"{"type":"wifi"}"#.to_owned(),
        r#"{"type":"future","extra":nonsense}"#.to_owned(),
        r#"{"type":"future","extra":01}"#.to_owned(),
        r#"{"type":"future","extra":1.}"#.to_owned(),
        r#"{"type":"future","extra":"\ud800"}"#.to_owned(),
        r#"{"type":"future","extra":true,}"#.to_owned(),
        r#"{"type":"future"} trailing"#.to_owned(),
        format!(
            "{{\"type\":\"future\",\"extra\":{}0{}}}",
            "[".repeat(20),
            "]".repeat(20)
        ),
    ] {
        assert_eq!(
            record(&mut r, bad.as_bytes()),
            [Event::Rejected(Rejection::MalformedOrUnsupported)],
            "{bad}"
        );
    }
    assert_eq!(
        record(&mut r, br#"{"type":"future","new":{"data":[1,true,null]}}"#),
        [Event::Unknown]
    );
    assert!(matches!(
        record(&mut r, &compact(BLE)).as_slice(),
        [Event::Observation(_)]
    ));
}
#[test]
fn reconnect_never_splices_partial_record_or_reuses_loss_baseline() {
    let mut r = Receiver::new();
    assert!(record(&mut r, &compact(WIFI)).is_empty());
    r.begin_session();
    record(&mut r, br#"{"type":"stream_stats","scan_drops":3,"output_drops":4,"ble_output_drops":5,"serialize_errors":6}"#);
    assert_eq!(r.latest_stats.unwrap().scan_drops, 3);
    for b in b"{\"type\":\"wi" {
        assert!(r.feed(*b).is_none());
    }
    r.disconnect();
    assert!(!r.connected());
    assert_eq!(r.counts.partial_lines_discarded, 1);
    r.begin_session();
    assert!(r.latest_stats.is_none());
    assert!(matches!(
        record(&mut r, &compact(WIFI)).as_slice(),
        [Event::Observation(_)]
    ));
    assert_eq!(r.counts.access_point_observations, 1);
}
#[test]
fn repeated_sightings_and_evidence_are_not_unique_device_counts() {
    let mut r = Receiver::new();
    r.begin_session();
    let mut wifi: serde_json::Value = serde_json::from_str(WIFI).unwrap();
    wifi["match"] = serde_json::json!([{"type":"ssid_keyword","detail":"candidate"}]);
    for _ in 0..2 {
        record(&mut r, &serde_json::to_vec(&wifi).unwrap());
        record(&mut r, &compact(EVIDENCE));
    }
    assert_eq!(r.counts.access_point_observations, 2);
    assert_eq!(r.counts.signature_observations, 2);
    assert_eq!(r.counts.evidence_records, 2);
}

#[test]
fn production_serializer_maximum_field_records_fit_receiver() {
    use airhound::{
        evidence::{AlertDecision, AlertPolicy, Detection, RuleEvidence},
        protocol::{DeviceMessage, MatchReason},
    };
    let mac = "FF:FF:FF:FF:FF:FF".try_into().unwrap();
    let name = "n".repeat(33).as_str().try_into().unwrap();
    let mut reasons = heapless::Vec::new();
    for _ in 0..4 {
        reasons
            .push(MatchReason {
                filter_type: "ssid_keyword",
                detail: "x".repeat(32).as_str().try_into().unwrap(),
            })
            .unwrap();
    }
    let rule_name = "r".repeat(32);
    let observation = DeviceMessage::WiFiScan {
        mac: &mac,
        ssid: &name,
        rssi: -128,
        ch: 255,
        frame: "probe_resp",
        matches: &reasons,
        rule: Some(&rule_name),
        ts: u32::MAX,
    };
    let uuid = "0000fffa-0000-1000-8000-00805f9b34fb".try_into().unwrap();
    let ble = DeviceMessage::BleScan {
        mac: &mac,
        name: &name,
        rssi: -128,
        uuid: Some(&uuid),
        mfr: u16::MAX,
        matches: &reasons,
        rule: Some(&rule_name),
        ts: u32::MAX,
    };
    let rules: Vec<_> = (0..4)
        .map(|_| RuleEvidence {
            id: 6,
            sig: [252, 253, 254, 255].into_iter().collect(),
            more: false,
            alert_eligible: false,
        })
        .collect();
    let evidence = DeviceMessage::Detection {
        id: &mac,
        scope: "wifi_address",
        ts: u32::MAX,
        detection: Detection {
            engine: "evidence-1",
            policy: AlertPolicy::RulesAndWatchlist,
            pack: "builtin-2",
            ruleset: "builtin-rules-2",
            sig: (248..256).collect(),
            rules: &rules,
            more: false,
            basis: "possibly_cached",
            alert: AlertDecision::Suppressed,
        },
    };
    let mut receiver = Receiver::new();
    receiver.begin_session();
    for message in [observation, ble, evidence] {
        let mut buffer = [0u8; 512];
        let n = serde_json_core::to_slice(&message, &mut buffer).expect("production buffer bound");
        assert!(n < 512, "newline must also fit");
        assert!(!matches!(
            record(&mut receiver, &buffer[..n]).as_slice(),
            [Event::Rejected(_)]
        ));
    }
    assert_eq!(receiver.counts.signature_observations, 2);
    assert_eq!(receiver.counts.evidence_records, 1);
}

#[test]
fn receiver_storage_and_unsupported_capacity_are_explicit() {
    assert!(core::mem::size_of::<Receiver>() <= 768);
    assert!(core::mem::size_of::<Event>() <= 640);
    let mut r = Receiver::new();
    r.begin_session();
    let mut wifi: serde_json::Value = serde_json::from_str(WIFI).unwrap();
    // Five reasons exceed the sensor's bounded four-reason contract.
    wifi["match"] = serde_json::Value::Array(
        (0..5)
            .map(|_| serde_json::json!({"type":"x","detail":""}))
            .collect(),
    );
    assert_eq!(
        record(&mut r, &serde_json::to_vec(&wifi).unwrap()),
        [Event::Rejected(Rejection::MalformedOrUnsupported)]
    );
    assert_eq!(r.counts.signature_observations, 0);
    assert_eq!(r.counts.rejected_records, 1);
}

#[test]
fn accepted_unknown_records_are_strict_json_despite_ignored_fields() {
    let base =
        br#"{"type":"future","nested":{"array":[-1.25e+2,true,false,null,"escape\n\u0061"]}}"#;
    let mut r = Receiver::new();
    r.begin_session();
    for pos in 1..base.len() {
        for byte in [
            0, b' ', b'"', b'\\', b']', b'}', b',', b':', b'-', b'0', b'x', 0xff,
        ] {
            let mut changed = base.to_vec();
            changed[pos] = byte;
            let events = record(&mut r, &changed);
            if events == [Event::Unknown] {
                assert!(
                    serde_json::from_slice::<serde_json::Value>(&changed).is_ok(),
                    "{changed:?}"
                );
            }
        }
    }
    assert_eq!(
        record(&mut r, br#"{"type":"future","mac":{"future":"shape"}}"#),
        [Event::Unknown]
    );
}

#[test]
fn invalid_known_record_fields_never_become_ordinary_observations() {
    let wifi: serde_json::Value = serde_json::from_str(WIFI).unwrap();
    let mut r = Receiver::new();
    r.begin_session();
    for (field, value) in [
        ("mac", serde_json::json!("not-a-mac")),
        ("ssid", serde_json::json!("x".repeat(34))),
        ("rssi", serde_json::json!(-129)),
        ("frame", serde_json::json!("future_frame")),
        ("match", serde_json::Value::Null),
        ("ch", serde_json::json!(256)),
        ("ts", serde_json::json!(-1)),
    ] {
        let mut changed = wifi.clone();
        changed[field] = value;
        assert_eq!(
            record(&mut r, &serde_json::to_vec(&changed).unwrap()),
            [Event::Rejected(Rejection::MalformedOrUnsupported)]
        );
    }
    let mut evidence: serde_json::Value = serde_json::from_str(EVIDENCE).unwrap();
    evidence["scope"] = "different_source".into();
    assert_eq!(
        record(&mut r, &serde_json::to_vec(&evidence).unwrap()),
        [Event::Rejected(Rejection::MalformedOrUnsupported)]
    );
    assert_eq!(r.counts.access_point_observations, 0);
    assert_eq!(r.counts.signature_observations, 0);
    assert_eq!(r.counts.evidence_records, 0);
    assert_eq!(r.counts.rejected_records, 8);
}
