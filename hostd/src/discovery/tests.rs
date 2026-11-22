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
