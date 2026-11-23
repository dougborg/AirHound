use super::*;
fn raw(vid: u16, pid: u16) -> RawDevice {
    RawDevice {
        id: "usb:1".into(),
        name: "Test device".into(),
        vid: Some(vid),
        pid: Some(pid),
        endpoints: vec![],
    }
}
#[test]
fn known_usb_ids_are_candidates_not_firmware_or_readiness_proof() {
    for (vid, pid, kind, state) in [
        (0x0e8d, 0x7610, "external_wifi", "needs_configuration"),
        (0x2e8a, 0x00c0, "sniffle", "needs_configuration"),
        (0x1209, 0xbabb, "sniffle", "needs_configuration"),
        (0x1546, 0x01a8, "gps", "needs_configuration"),
        (0x1915, 0x522a, "nrf_sniffer", "unsupported"),
        (0x0a12, 1, "usb_hci", "unsupported"),
        (0x0483, 0x5740, "flipper", "needs_identification"),
        (0x10c4, 0xea60, "serial_unknown", "needs_identification"),
        (0x1a86, 0x55d4, "serial_unknown", "needs_identification"),
        (0x1a86, 0x7523, "serial_unknown", "needs_identification"),
        (0x0403, 0x6001, "serial_unknown", "needs_identification"),
    ] {
        let c = classify(&raw(vid, pid), "macos", &Options::default());
        assert_eq!(c.len(), 1, "{vid:04x}:{pid:04x}");
        assert_eq!(c[0].kind, kind);
        assert_eq!(c[0].state, state);
        assert!(!c[0].evidence.is_empty());
    }
}
#[test]
fn os_endpoints_override_usb_only_guess_without_selecting_mac_dongle() {
    let mut d = raw(0x0a12, 1);
    d.endpoints.push(Endpoint {
        kind: "native_ble".into(),
        path: "hci2".into(),
    });
    let c = classify(&d, "linux", &Options::default());
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].configuration_hint.as_ref().unwrap()["adapter"], "hci2");
    let c = classify(&d, "macos", &Options::default());
    assert!(c[0]
        .configuration_hint
        .as_ref()
        .unwrap()
        .get("adapter")
        .is_none());
    assert_eq!(c[0].state, "unverified");
}
#[test]
fn native_and_monitor_wifi_have_distinct_requirements() {
    let mut d = raw(1, 2);
    for (platform, kind, state) in [
        ("macos", "native_wifi", "unverified"),
        ("linux", "native_wifi", "needs_configuration"),
        ("linux", "linux_wifi", "unverified"),
    ] {
        d.endpoints = vec![Endpoint {
            kind: kind.into(),
            path: "wlan0".into(),
        }];
        let c = classify(&d, platform, &Options::default());
        assert_eq!(c[0].state, state);
        assert_eq!(
            c[0].kind,
            if platform == "macos" {
                "native_wifi"
            } else {
                "linux_wifi"
            }
        );
    }
}
#[test]
fn unknown_usb_hidden_by_default_serial_candidates_retained_and_endpoints_deduplicated() {
    let unknown = raw(1, 2);
    let mut serial = raw(3, 4);
    serial.id = "usb:2".into();
    let endpoint = Endpoint {
        kind: "serial".into(),
        path: "/dev/test".into(),
    };
    serial.endpoints = vec![endpoint.clone(), endpoint];
    let inventory = assemble(
        vec![serial.clone(), unknown.clone()],
        vec![],
        "linux",
        &Options::default(),
    );
    assert_eq!(inventory.devices.len(), 1);
    assert_eq!(inventory.devices[0].endpoints.len(), 1);
    assert_eq!(inventory.devices[0].candidates[0].kind, "serial_unknown");
    let inventory = assemble(
        vec![serial, unknown],
        vec![],
        "linux",
        &Options {
            all: true,
            ..Default::default()
        },
    );
    assert_eq!(inventory.devices.len(), 2);
    assert_eq!(inventory.devices[0].id, "usb:1");
    assert_eq!(inventory.devices[0].usb.as_ref().unwrap().vid, "0001");
    let value = serde_json::to_value(inventory).unwrap();
    assert_eq!(value["mode"], "passive");
    assert_eq!(value["schema_version"], 1);
}
#[test]
fn prerequisite_files_never_become_verified_firmware() {
    let dir = std::env::temp_dir().join(format!(
        "airhound-discovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("helper");
    std::fs::write(&file, b"not executable or valid firmware").unwrap();
    let options = Options {
        external_scanner: Some(file.clone()),
        external_firmware: Some(file.clone()),
        sniffle_python: Some(file.clone()),
        sniffle_helper: Some(file.clone()),
        sniffle_module_path: Some(dir.clone()),
        ..Default::default()
    };
    let alfa = classify(&raw(0x0e8d, 0x7610), "macos", &options);
    assert_eq!(alfa[0].state, "unverified");
    assert!(alfa[0].requirements.iter().all(|r| r.state == "present"));
    let mut radio = raw(0x2e8a, 0xc0);
    radio.endpoints.push(Endpoint {
        kind: "serial".into(),
        path: "/dev/cu.radio".into(),
    });
    let sniffle = classify(&radio, "macos", &options);
    assert_eq!(sniffle[0].state, "needs_identification");
    assert_eq!(
        sniffle[0].configuration_hint.as_ref().unwrap()["port"],
        "/dev/cu.radio"
    );
    assert_eq!(prerequisite("file", Some(&dir), false).state, "invalid");
    assert_eq!(prerequisite("dir", Some(&file), true).state, "invalid");
    assert_eq!(
        prerequisite("missing", Some(&dir.join("absent")), false).state,
        "missing"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn text_escapes_descriptors_and_surfaces_partial_discovery() {
    let mut d = raw(0x0e8d, 0x7610);
    d.name = "fake\nrow\x1b[2J".into();
    let inventory = assemble(
        vec![d],
        vec![Diagnostic {
            backend: "usb".into(),
            message: "permission denied\npartial".into(),
        }],
        "macos",
        &Options::default(),
    );
    let mut text = Vec::new();
    write_text(&inventory, &mut text).unwrap();
    let text = String::from_utf8(text).unwrap();
    assert!(!text.contains('\x1b'));
    assert!(text.contains("fake\\nrow"));
    assert!(text.contains("permission denied\\npartial"));
    assert!(text.contains("external_scanner: missing"));
    let mut text = Vec::new();
    write_text(
        &assemble(vec![], vec![], "linux", &Options::default()),
        &mut text,
    )
    .unwrap();
    assert!(String::from_utf8(text)
        .unwrap()
        .contains("No candidate devices"));
}

#[test]
fn composite_serial_roles_and_multiple_alfa_devices_are_not_auto_selected() {
    let mut d = raw(0x1209, 0xbabb);
    d.endpoints = vec![
        Endpoint {
            kind: "serial".into(),
            path: "/dev/a".into(),
        },
        Endpoint {
            kind: "serial".into(),
            path: "/dev/b".into(),
        },
    ];
    let c = classify(&d, "linux", &Options::default());
    assert!(c[0].configuration_hint.as_ref().unwrap()["port"].is_null());
    assert!(c[0]
        .requirements
        .iter()
        .any(|r| r.name == "serial_endpoint" && r.state == "needs_selection"));
    let a = raw(0x0e8d, 0x7610);
    let mut b = a.clone();
    b.id = "usb:2".into();
    let inventory = assemble(vec![a, b], vec![], "linux", &Options::default());
    for d in inventory.devices {
        assert!(d.candidates[0].configuration_hint.as_ref().unwrap()["usb_selector"].is_null());
        assert!(d.candidates[0]
            .requirements
            .iter()
            .any(|r| r.name == "physical_device_selection" && r.state == "needs_selection"));
    }
}

#[test]
fn ordered_mt76_devices_are_unverified_and_cannot_activate_old_helper() {
    for (vid, pid) in [(0x0e8d, 0x7961), (0x0846, 0x9072)] {
        for platform in ["macos", "linux"] {
            let candidates = classify(&raw(vid, pid), platform, &Options::default());
            assert_eq!(candidates.len(), 1);
            let c = &candidates[0];
            assert_eq!(c.kind, "mt76_wifi");
            assert_eq!(c.state, "unverified");
            assert_eq!(c.support, "experimental");
            assert!(c.configuration_hint.is_none());
            assert_eq!(c.capabilities["scope"], "unknown");
            assert_eq!(c.requirements[0].state, "unsupported");
        }
    }
    // No vendor-wide match, untested A8500, or MT7927 support inference.
    for (vid, pid) in [(0x0e8d, 0x6639), (0x0846, 0x9050), (0x0846, 0xffff)] {
        assert!(classify(&raw(vid, pid), "macos", &Options::default()).is_empty());
    }
}

#[test]
fn roles_distinguish_controllers_location_and_unknown_transport() {
    for (vid, pid, role) in [
        (0x1546, 0x01a8, "location_source"),
        (0x0483, 0x5740, "controller_or_companion"),
        (0x1a86, 0x55d4, "unknown"),
        (0x2e8a, 0xc0, "capture_receiver"),
    ] {
        let c = classify(&raw(vid, pid), "macos", &Options::default());
        assert_eq!(c[0].role, role);
        assert!(c[0].identification.is_none());
        assert!(c[0]
            .support_issue
            .unwrap()
            .starts_with("https://github.com/dougborg/AirHound/issues/"));
    }
}
#[test]
fn identification_requires_current_candidate_and_explicit_owned_endpoint() {
    let mut d = raw(0x2e8a, 0xc0);
    d.endpoints.push(Endpoint {
        kind: "serial".into(),
        path: "/dev/cu.test".into(),
    });
    let inventory = assemble(vec![d], vec![], "macos", &Options::default());
    let mut request = identify::Request {
        protocol: "sniffle".into(),
        device_id: "usb:1".into(),
        port: "/dev/cu.test".into(),
        python: "python3".into(),
        helper: "helper.py".into(),
        module_path: None,
    };
    request.validate(&inventory).unwrap();
    request.port = "/dev/cu.wrong".into();
    assert!(request.validate(&inventory).is_err());
    request.port = "/dev/cu.test".into();
    request.device_id = "removed".into();
    assert!(request.validate(&inventory).is_err());
    request.device_id = "usb:1".into();
    let mut generic = raw(0x1a86, 0x55d4);
    generic.endpoints = inventory.devices[0].endpoints.clone();
    let inventory = assemble(vec![generic], vec![], "macos", &Options::default());
    assert!(request.validate(&inventory).is_err());
}
#[test]
fn active_firmware_metadata_is_endpoint_scoped_and_never_cached_across_discovery() {
    let mut d = raw(0x2e8a, 0xc0);
    d.endpoints.push(Endpoint {
        kind: "serial".into(),
        path: "/dev/cu.test".into(),
    });
    let health = json!({"active":true,"run_id":"run","receiver_health":{"receivers":[{"id":"cat","receiver":{"adapter_id":"sniffle:/dev/cu.test","firmware":{"type":"sniffle_identity","schema_version":1,"major":1,"minor":11,"patch":0,"api_level":0}}}]}});
    let mut inventory = assemble(vec![d.clone()], vec![], "macos", &Options::default());
    inventory.reuse_active_metadata(&health);
    let c = &inventory.devices[0].candidates[0];
    assert_eq!(c.state, "in_use");
    assert_eq!(c.support, "available");
    let id = c.identification.as_ref().unwrap();
    assert_eq!(id.firmware_version.as_deref(), Some("1.11.0"));
    assert_eq!(id.reception, "not_tested");
    assert_eq!(inventory.mode, "passive_with_active_metadata");
    let mut fresh = assemble(vec![d.clone()], vec![], "macos", &Options::default());
    fresh.reuse_active_metadata(&json!({"active":false,"last":health}));
    assert!(fresh.devices[0].candidates[0].identification.is_none());
    d.endpoints[0].path = "/dev/cu.replaced".into();
    let mut replaced = assemble(vec![d], vec![], "macos", &Options::default());
    replaced.reuse_active_metadata(&health);
    assert!(replaced.devices[0].candidates[0].identification.is_none());
    assert_eq!(replaced.mode, "passive");
}

#[test]
fn airhound_identity_requires_current_serial_owner_and_actual_status() {
    let mut device = raw(0x1a86, 0x55d4);
    device.endpoints.push(Endpoint {
        kind: "serial".into(),
        path: "/dev/cu.sensor".into(),
    });
    let status = json!({"active":true,"run_id":"run", "receiver_health":{"receivers":[{
        "id":"m5","requested_assignment":{"baud":460800},
        "receiver":{"backend":"airhound_serial","adapter_id":"/dev/tty.sensor","availability":"available",
            "producer_identity":{"application":"AirHound","version":"0.1.1","board":"m5stickc_plus2","evidence":"device_status","connection_generation":1,"boot_generation":1}}
    }]}});
    let inventory_for = |status: &Value| {
        let mut inventory = assemble(vec![device.clone()], vec![], "macos", &Options::default());
        inventory.reuse_active_metadata(status);
        inventory
    };
    let inventory = inventory_for(&status);
    let candidate = &inventory.devices[0].candidates[0];
    assert_eq!(candidate.kind, "airhound_serial");
    assert_eq!(candidate.role, "capture_receiver");
    assert_eq!(candidate.capabilities["media"], json!(["wifi", "ble"]));
    assert_eq!(
        candidate.configuration_hint.as_ref().unwrap()["baud"],
        460800
    );
    let identity = candidate.identification.as_ref().unwrap();
    assert_eq!(identity.application, Some("AirHound"));
    assert_eq!(identity.firmware_version.as_deref(), Some("0.1.1"));
    assert_eq!(identity.board.as_deref(), Some("m5stickc_plus2"));
    assert_eq!(identity.provenance, "active_capture_owner");
    assert_eq!(identity.reception, "not_tested");
    assert!(identity.protocol_api.is_none());
    assert!(identity.compatible.is_none());
    for state in ["reconnecting", "unavailable", "stopped"] {
        let mut lost = status.clone();
        lost["receiver_health"]["receivers"][0]["receiver"]["availability"] = json!(state);
        assert_eq!(
            inventory_for(&lost).devices[0].candidates[0].kind,
            "serial_unknown"
        );
    }
    let mut paused = status.clone();
    paused["receiver_health"]["receivers"][0]["receiver"]["availability"] = json!("paused");
    assert_eq!(
        inventory_for(&paused).devices[0].candidates[0].kind,
        "airhound_serial"
    );
    for (key, value) in [
        ("producer_identity", Value::Null),
        ("backend", json!("sniffle")),
        ("adapter_id", json!("/dev/cu.other")),
    ] {
        let mut mismatch = status.clone();
        mismatch["receiver_health"]["receivers"][0]["receiver"][key] = value;
        let fresh = inventory_for(&mismatch);
        assert_eq!(fresh.devices[0].candidates[0].kind, "serial_unknown");
        assert!(fresh.devices[0].candidates[0].identification.is_none());
        assert_eq!(fresh.mode, "passive");
    }
    let ended = inventory_for(&json!({"active":false,"last":status}));
    assert_eq!(ended.devices[0].candidates[0].kind, "serial_unknown");
    assert!(ended.devices[0].candidates[0].configuration_hint.is_none());
}

#[test]
fn gnss_hint_is_a_location_provider_not_a_capture_source() {
    let mut device = raw(0x1546, 0x01a8);
    device.endpoints = vec![Endpoint {
        kind: "serial".into(),
        path: "/dev/gps-fixture".into(),
    }];
    let candidates = classify(&device, "linux", &Options::default());
    let candidate = &candidates[0];
    assert_eq!(candidate.role, "location_source");
    assert!(candidate.configuration_hint.is_none());
    assert_eq!(
        candidate.location_configuration_hint.as_ref().unwrap()["device"],
        "/dev/gps-fixture"
    );
    assert_eq!(candidate.state, "needs_configuration");
    device.endpoints.push(Endpoint {
        kind: "serial".into(),
        path: "/dev/gps-ambiguous".into(),
    });
    let candidates = classify(&device, "linux", &Options::default());
    assert!(candidates[0].location_configuration_hint.as_ref().unwrap()["device"].is_null());
}

#[test]
fn external_identity_requires_exact_selector_and_current_owner() {
    let identity=crate::external_identity::Identity::from_hello(&json!({"helper_build":{"component":"mt7610u-lab","package_version":"0.1.0","display_version":"git fixture","git_revision":null,"source_tag":null,"tracked_source_modified":null}})).unwrap();
    let health = json!({"active":true,"receiver_health":{"receivers":[{"id":"alfa","receiver":{"backend":"mt7610u_companion","adapter_id":"usb:1:2","availability":"running","external_identity":identity}}]}});
    let fresh = || {
        assemble(
            vec![raw(0x0e8d, 0x7610)],
            vec![],
            "macos",
            &Options::default(),
        )
    };
    let mut unmapped = fresh();
    unmapped.reuse_active_metadata(&health);
    assert!(unmapped.devices[0].candidates[0]
        .external_identity
        .is_none());
    let mapped = || {
        let mut v = fresh();
        v.devices[0].candidates[0]
            .configuration_hint
            .as_mut()
            .unwrap()["usb_selector"] = json!("usb:1:2");
        v
    };
    let mut exact = mapped();
    exact.reuse_active_metadata(&health);
    assert!(exact.devices[0].candidates[0].external_identity.is_some());
    assert_eq!(exact.devices[0].candidates[0].support, "experimental");
    for state in ["stopped", "failed", "recovering"] {
        let mut stopped = health.clone();
        stopped["receiver_health"]["receivers"][0]["receiver"]["availability"] = json!(state);
        let mut inventory = mapped();
        inventory.reuse_active_metadata(&stopped);
        assert!(inventory.devices[0].candidates[0]
            .external_identity
            .is_none());
    }
    let mut history = mapped();
    history.reuse_active_metadata(&json!({"active":false,"last":health}));
    assert!(history.devices[0].candidates[0].external_identity.is_none());
}

#[test]
fn active_flipper_hello_identifies_bridge_not_downstream_radio_and_never_stale_owner() {
    let mut device = raw(0x0483, 0x5740);
    device.endpoints.push(Endpoint {
        kind: "serial".into(),
        path: "/dev/cu.flipper-fixture".into(),
    });
    let inventory = || assemble(vec![device.clone()], vec![], "macos", &Options::default());
    let mut status = json!({"active":true,"receiver_health":{"receivers":[{"receiver":{"backend":"flipper_marauder","adapter_id":"/dev/tty.flipper-fixture","availability":"available","bridge_identity":{"application":{"evidence":"ahfb_hello","version":"fixture"},"awok_firmware":null}},"scanners":{"wifi":{"state":"initializing"}}}]}});
    let mut active = inventory();
    active.reuse_active_metadata(&status);
    let candidate = &active.devices[0].candidates[0];
    assert_eq!(candidate.label, "AirHound Flipper bridge");
    assert_eq!(candidate.support, "available");
    assert_eq!(
        candidate.bridge_identity.as_ref().unwrap()["awok_firmware"],
        Value::Null
    );
    assert!(!candidate
        .evidence
        .iter()
        .any(|s| s.contains("reception verified")));
    status["receiver_health"]["receivers"][0]["scanners"]["wifi"]["state"] = "running".into();
    let mut running = inventory();
    running.reuse_active_metadata(&status);
    assert_eq!(
        running.devices[0].candidates[0].label,
        "AirHound Flipper bridge"
    );
    for mode in ["failed", "stopped", "unavailable"] {
        status["receiver_health"]["receivers"][0]["receiver"]["availability"] = mode.into();
        let mut stale = inventory();
        stale.reuse_active_metadata(&status);
        assert!(stale.devices[0].candidates[0].bridge_identity.is_none());
    }
    status["active"] = false.into();
    status["receiver_health"]["receivers"][0]["receiver"]["availability"] = "available".into();
    let mut stale = inventory();
    stale.reuse_active_metadata(&status);
    assert!(stale.devices[0].candidates[0].bridge_identity.is_none());
}

#[test]
fn ublox_explicit_contract_requires_current_recognized_usb_and_selected_endpoint() {
    let mut raw = raw(0x1546, 0x01a8);
    raw.endpoints.push(Endpoint {
        kind: "serial".into(),
        path: "/dev/cu.gps-test".into(),
    });
    let inventory = assemble(vec![raw], vec![], "macos", &Options::default());
    let request: identify::Request = serde_json::from_value(
        json!({"protocol":"ublox","device_id":"usb:1","port":"/dev/cu.gps-test"}),
    )
    .unwrap();
    if !identify::supports_ublox_identification() {
        assert_eq!(
            request.validate(&inventory).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
        assert!(inventory.devices[0].candidates[0]
            .identification_protocol
            .is_none());
        return;
    }
    request.validate(&inventory).unwrap();
    assert_eq!(
        inventory.devices[0].candidates[0].identification_protocol,
        Some("ublox")
    );
    assert!(inventory.devices[0].candidates[0].identification.is_none());
    let mut wrong = request.clone();
    wrong.port = "/dev/cu.other".into();
    assert!(wrong.validate(&inventory).is_err());
    wrong = request.clone();
    wrong.device_id = "stale".into();
    assert!(wrong.validate(&inventory).is_err());
    wrong = request.clone();
    wrong.helper = "helper.py".into();
    assert!(wrong.validate(&inventory).is_err());
    let mut changed = inventory.clone();
    changed.devices[0].usb.as_mut().unwrap().vid = "1234".into();
    assert!(request.validate(&changed).is_err());
    changed = inventory.clone();
    changed.devices[0].endpoints.clear();
    assert!(request.validate(&changed).is_err());
    let busy = identify::Identification::from_active_owner(
        &request,
        &json!({"active":true,"run_id":"active","receiver_health":{"location":{"state":"fix","firmware":"unverified"}}}),
    );
    assert_eq!(busy.state, "busy");
    assert_eq!(busy.compatible, None);
    assert_eq!(busy.firmware_version, None);
    assert_eq!(busy.reception, "not_tested");
    let json = serde_json::to_value(busy).unwrap();
    for field in [
        "firmware_version",
        "software_version",
        "hardware_version",
        "protocol_version",
        "model",
    ] {
        assert!(json[field].is_null())
    }
}

#[test]
fn ublox_inventory_capability_does_not_probe_or_guess_missing_endpoint() {
    let mut gps = raw(0x1546, 0x01a8);
    assert_eq!(
        classify(&gps, "macos", &Options::default())[0].identification_protocol,
        None
    );
    gps.endpoints = vec![
        Endpoint {
            kind: "serial".into(),
            path: "/dev/one".into(),
        },
        Endpoint {
            kind: "serial".into(),
            path: "/dev/two".into(),
        },
    ];
    let candidate = classify(&gps, "macos", &Options::default()).remove(0);
    assert_eq!(
        candidate.identification_protocol,
        identify::supports_ublox_identification().then_some("ublox")
    );
    assert!(candidate.location_configuration_hint.unwrap()["device"].is_null());
    assert!(candidate.identification.is_none());
    gps.vid = Some(0x1a86);
    gps.pid = Some(0x55d4);
    assert!(classify(&gps, "macos", &Options::default())
        .iter()
        .all(|c| c.identification_protocol.is_none()));
}
