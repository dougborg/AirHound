use super::*;
use crate::discovery::{Device, Endpoint};
use std::os::unix::fs::{symlink, PermissionsExt};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "airhound-profiles-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> Store {
        Store::open(&self.0.join("profiles")).unwrap()
    }
    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(content).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn inventory() -> Inventory {
    Inventory {
        schema_version: 1,
        mode: "passive",
        platform: "fixture".into(),
        devices: vec![],
        diagnostics: vec![],
    }
}
fn config() -> Value {
    json!({"version":1,"receivers":[{"id":"ble","source":{"kind":"native_ble"}}]})
}
fn save(store: &Store) -> Profile {
    store
        .save(
            "test".into(),
            "My setup".into(),
            config(),
            &inventory(),
            false,
        )
        .unwrap()
}

#[test]
fn round_trip_private_atomic_replace_and_delete() {
    let temp = Temp::new();
    let store = temp.store();
    let original = save(&store);
    assert_eq!(store.load("test").unwrap().config, original.config);
    assert_eq!(store.list().unwrap()["profiles"][0]["name"], "My setup");
    assert_eq!(
        fs::metadata(temp.0.join("profiles/test.json"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        store
            .save(
                "test".into(),
                "Changed".into(),
                config(),
                &inventory(),
                false
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(store.load("test").unwrap().name, "My setup");
    store
        .save(
            "test".into(),
            "Changed".into(),
            config(),
            &inventory(),
            true,
        )
        .unwrap();
    assert_eq!(store.load("test").unwrap().name, "Changed");
    assert_eq!(store.delete("test").unwrap()["deleted"], "test");
    assert!(store.list().unwrap()["profiles"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn reject_traversal_bad_names_symlinks_modes_and_fifos() {
    let temp = Temp::new();
    let store = temp.store();
    for id in ["../out", "/tmp/out", "", ".", "..", "a/b", "with space"] {
        assert!(store.load(id).is_err());
        assert!(store.delete(id).is_err());
    }
    for name in ["", "\nname", "  "] {
        assert!(store
            .save("test".into(), name.into(), config(), &inventory(), false)
            .is_err());
    }
    let outside = temp.file("outside", b"unchanged");
    symlink(&outside, temp.0.join("profiles/test.json")).unwrap();
    assert!(store.load("test").is_err());
    assert!(store.delete("test").is_err());
    assert!(store
        .save("test".into(), "Test".into(), config(), &inventory(), true)
        .is_err());
    assert_eq!(fs::read(outside).unwrap(), b"unchanged");
    fs::remove_file(temp.0.join("profiles/test.json")).unwrap();
    save(&store);
    fs::set_permissions(
        temp.0.join("profiles/test.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(store.load("test").is_err());
    fs::remove_file(temp.0.join("profiles/test.json")).unwrap();
    let fifo = std::ffi::CString::new(temp.0.join("profiles/test.json").to_str().unwrap()).unwrap();
    // SAFETY: CString is a valid nul-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(store.load("test").is_err()); // O_NONBLOCK prevents a hanging read.
    assert!(store.delete("test").is_err());
    drop(store);
    fs::set_permissions(temp.0.join("profiles"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::open(&temp.0.join("profiles")).is_err());
}

#[test]
fn reject_untrusted_schema_false_verification_and_oversized_files() {
    let temp = Temp::new();
    let store = temp.store();
    let profile = save(&store);
    let path = temp.0.join("profiles/test.json");
    let base = serde_json::to_value(profile).unwrap();
    for (key, value) in [
        ("schema_version", json!(9)),
        ("id", json!("different")),
        ("validation_scope", json!("rf_verified")),
    ] {
        let mut broken = base.clone();
        broken[key] = value;
        fs::write(&path, serde_json::to_vec(&broken).unwrap()).unwrap();
        assert!(store.load("test").is_err());
    }
    fs::write(&path, vec![b' '; MAX_BYTES + 1]).unwrap();
    assert!(store.load("test").is_err());
    // Explicit deletion remains possible when a schema is unsupported.
    store.delete("test").unwrap();
}

#[test]
fn contention_capacity_and_interrupted_writer_recovery() {
    let temp = Temp::new();
    let store = temp.store();
    assert!(
        matches!(Store::open(&temp.0.join("profiles")),Err(e) if e.kind()==io::ErrorKind::WouldBlock)
    );
    for i in 0..MAX_PROFILES {
        store
            .save(
                format!("p{i}"),
                "Plan".into(),
                config(),
                &inventory(),
                false,
            )
            .unwrap();
    }
    assert!(store
        .save("extra".into(), "Plan".into(), config(), &inventory(), false)
        .is_err());
    let mut pending = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp.0.join("profiles/.pending-123-1"))
        .unwrap();
    pending.write_all(b"partial").unwrap();
    drop(pending);
    drop(store);
    let store = temp.store();
    assert!(!temp.0.join("profiles/.pending-123-1").exists());
    assert_eq!(
        store.list().unwrap()["profiles"].as_array().unwrap().len(),
        MAX_PROFILES
    );
}

#[test]
fn resources_are_metadata_only_and_rechecked_after_save() {
    let temp = Temp::new();
    let store = temp.store();
    let file = temp.file(
        "capture.pcap",
        b"not opened or parsed by profile validation",
    );
    let plan =
        json!({"version":1,"receivers":[{"id":"replay","source":{"kind":"ble_pcap","path":file}}]});
    let saved = store
        .save("replay".into(), "Replay".into(), plan, &inventory(), false)
        .unwrap();
    assert_eq!(check_resources(&saved.config)["valid"], true);
    fs::remove_file(file).unwrap();
    let loaded = store.load("replay").unwrap();
    assert_eq!(
        check_resources(&loaded.config)["checks"][0]["state"],
        "missing"
    );
    let plan = json!({"version":1,"receivers":[{"id":"replay","source":{"kind":"ble_pcap","path":"relative.pcap"}}]});
    assert_eq!(
        check_resources(&plan)["checks"][0]["state"],
        "absolute_path_required"
    );
    assert!(store
        .save("bad".into(), "Bad".into(), plan, &inventory(), false)
        .is_err());
    let fifo = temp.0.join("fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: CString is valid and the path belongs to this test.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let plan =
        json!({"version":1,"receivers":[{"id":"replay","source":{"kind":"ble_pcap","path":fifo}}]});
    assert_eq!(
        check_resources(&plan)["checks"][0]["state"],
        "wrong_type_or_mode"
    );
}

#[test]
fn endpoint_reuse_does_not_certify_identity_or_rebind_devices() {
    let mut current = inventory();
    current.devices.push(Device {
        id: "usb:port1".into(),
        identity_scope: "usb_topology",
        name: "fixture".into(),
        usb: None,
        endpoints: vec![Endpoint {
            kind: "serial".into(),
            path: "/dev/cu.fixture".into(),
        }],
        candidates: vec![],
    });
    let plan = json!({"version":1,"receivers":[{"id":"sniffer","source":{"kind":"sniffle","port":"/dev/cu.fixture","python":"/unused/python","helper":"/unused/helper"}}]});
    let profile = Profile {
        schema_version: 1,
        id: "fixture".into(),
        name: "Fixture".into(),
        config: plan.clone(),
        bindings: bindings(&plan, &current),
        saved_at_ms: 0,
        validation_scope: "configuration_only".into(),
        origin: None,
    };
    let checks = check_bindings(&profile, &current);
    assert_eq!(
        checks["checks"][0]["state"],
        "present_requires_identity_review"
    );
    assert_eq!(checks["checks"][0]["physical_identity_verified"], false);
    assert_eq!(checks["automatic_rebinding"], false);
    current.devices[0].id = "usb:replacement".into();
    assert_eq!(
        check_bindings(&profile, &current)["checks"][0]["state"],
        "changed_requires_identity_review"
    );
    current.devices.push(current.devices[0].clone());
    current.devices[1].id = "usb:ambiguous".into();
    assert_eq!(
        check_bindings(&profile, &current)["checks"][0]["state"],
        "ambiguous"
    );
    current.devices.clear();
    assert_eq!(
        check_bindings(&profile, &current)["checks"][0]["state"],
        "missing"
    );
}

#[test]
fn helper_paths_are_not_executed_and_permissions_are_revalidated() {
    let temp = Temp::new();
    let python = temp.file("python", b"this is deliberately not executable code");
    let helper = temp.file("helper.py", b"raise RuntimeError('must never execute')");
    let modules = temp.0.join("modules");
    fs::create_dir(&modules).unwrap();
    let plan = json!({"version":1,"receivers":[{"id":"sniffer","source":{"kind":"sniffle","port":"/dev/missing","python":python,"helper":helper,"module_path":modules}}]});
    assert_eq!(
        check_resources(&plan)["checks"][0]["state"],
        "wrong_type_or_mode"
    );
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(check_resources(&plan)["valid"], true);
    let store = temp.store();
    store
        .save(
            "sniffer".into(),
            "Sniffer".into(),
            plan.clone(),
            &inventory(),
            false,
        )
        .unwrap();
    assert_eq!(store.load("sniffer").unwrap().config, plan);
    fs::remove_dir(&modules).unwrap();
    assert_eq!(check_resources(&plan)["checks"][2]["state"], "missing");
    assert_eq!(
        check_bindings(&store.load("sniffer").unwrap(), &inventory())["checks"][0]["state"],
        "missing"
    );
}

#[test]
fn store_and_lock_symlinks_never_follow_external_files() {
    let temp = Temp::new();
    let target = temp.0.join("target");
    fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
    symlink(&target, temp.0.join("profiles")).unwrap();
    assert!(Store::open(&temp.0.join("profiles")).is_err());
    fs::remove_file(temp.0.join("profiles")).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(temp.0.join("profiles"))
        .unwrap();
    let outside = temp.file("outside", b"untouched");
    symlink(&outside, temp.0.join("profiles/.lock")).unwrap();
    assert!(Store::open(&temp.0.join("profiles")).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}

#[test]
fn origin_survives_reopen_and_copied_profile_is_advisory_foreign_scope() {
    let first = Temp::new();
    let second = Temp::new();
    let store = first.store();
    let profile = save(&store);
    let origin = profile.origin.clone().unwrap();
    assert_eq!(origin.platform, std::env::consts::OS);
    assert_eq!(store.origin_check(&profile)["state"], "same_store");
    drop(store);
    let store = first.store();
    assert_eq!(store.load("test").unwrap().origin.as_ref(), Some(&origin));
    assert_eq!(store.origin_check(&profile)["state"], "same_store");
    let other = second.store();
    fs::copy(
        first.0.join("profiles/test.json"),
        second.0.join("profiles/test.json"),
    )
    .unwrap();
    assert_eq!(
        other.origin_check(&other.load("test").unwrap())["state"],
        "different_store"
    );
    let mut different = profile.clone();
    different.origin.as_mut().unwrap().platform = "another-platform".into();
    assert_eq!(
        store.origin_check(&different)["state"],
        "different_platform"
    );
    let mut legacy = serde_json::to_value(profile).unwrap();
    legacy.as_object_mut().unwrap().remove("origin");
    fs::write(
        first.0.join("profiles/test.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    assert_eq!(
        store.origin_check(&store.load("test").unwrap())["state"],
        "unknown"
    );
    assert_eq!(
        store.origin_check(&different)["physical_identity_verified"],
        false
    );
}

#[test]
fn alias_change_keeps_a_saved_profile_editable_but_invalidates_current_resolution() {
    let temp = Temp::new();
    let store = temp.store();
    let one = temp.file("one-port", b"");
    let two = temp.file("two-port", b"");
    let alias = temp.0.join("alias-port");
    symlink(&two, &alias).unwrap();
    let python = temp.file("python", b"not executed");
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    let helper = temp.file("helper", b"not executed");
    let source =
        |port: &Path| json!({"kind":"sniffle","port":port,"python":python,"helper":helper});
    let plan = json!({"version":1,"receivers":[{"id":"one","source":source(&one)},{"id":"two","source":source(&alias)}]});
    store
        .save(
            "two".into(),
            "Two receivers".into(),
            plan.clone(),
            &inventory(),
            false,
        )
        .unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(&one, &alias).unwrap();
    let loaded = store.load("two").unwrap();
    assert_eq!(loaded.config, plan);
    assert!(SessionConfig::from_json(loaded.config)
        .unwrap()
        .resolve()
        .is_err());
    assert!(store
        .save(
            "two".into(),
            "Still conflicts".into(),
            plan,
            &inventory(),
            true
        )
        .is_err());
    // Semantic readiness changed, but the last valid saved profile is unchanged.
    assert_eq!(store.load("two").unwrap().name, "Two receivers");
    let mut broken = serde_json::to_value(store.load("two").unwrap()).unwrap();
    broken["config"]["version"] = json!(99);
    fs::write(
        temp.0.join("profiles/two.json"),
        serde_json::to_vec(&broken).unwrap(),
    )
    .unwrap();
    assert!(store.load("two").is_err());
}

#[test]
fn m5_and_gps_profile_round_trip_preserves_full_configuration_and_ownership() {
    let temp = Temp::new();
    let store = temp.store();
    let plan = json!({
        "version": 1, "alert_policy": "silent",
        "location": {"kind":"gpsd", "endpoint":"127.0.0.1:2947", "device":"/dev/cu.gps-fixture"},
        "receivers": [{"id":"m5", "source":{"kind":"airhound_serial", "port":"/dev/cu.m5-fixture", "baud":460800}}]
    });
    let saved = store
        .save(
            "m5-gps".into(),
            "M5 with GPS".into(),
            plan.clone(),
            &inventory(),
            false,
        )
        .unwrap();
    let loaded = store.load("m5-gps").unwrap();
    assert_eq!(loaded.config, plan);
    assert_eq!(saved.bindings[0].source_kind, "airhound_serial");
    assert_eq!(
        loaded.bindings[0].selector.as_deref(),
        Some("/dev/cu.m5-fixture")
    );
    assert_eq!(
        check_bindings(&loaded, &inventory())["checks"][0]["state"],
        "missing"
    );
    assert_eq!(loaded.validation_scope, "configuration_only");

    let mut collision = plan;
    collision["receivers"][0]["source"]["port"] = json!("/dev/tty.gps-fixture");
    assert!(store
        .save(
            "collision".into(),
            "Shared UART".into(),
            collision,
            &inventory(),
            false
        )
        .is_err());
    assert_eq!(store.load("m5-gps").unwrap().config, loaded.config);
}

#[test]
fn flipper_profile_retains_explicit_port_and_requires_current_binding_review() {
    let temp = Temp::new();
    let store = temp.store();
    let plan = json!({"version":1,"alert_policy":"silent","receivers":[{"id":"bridge","source":{"kind":"flipper_marauder","port":"/dev/cu.flipper-fixture"}}]});
    let saved = store
        .save(
            "flipper".into(),
            "Flipper bridge".into(),
            plan.clone(),
            &inventory(),
            false,
        )
        .unwrap();
    let loaded = store.load("flipper").unwrap();
    assert_eq!(loaded.config, plan);
    assert_eq!(saved.bindings[0].source_kind, "flipper_marauder");
    assert_eq!(
        saved.bindings[0].selector.as_deref(),
        Some("/dev/cu.flipper-fixture")
    );
    assert_eq!(
        check_bindings(&loaded, &inventory())["checks"][0]["state"],
        "missing"
    );
    assert_eq!(loaded.validation_scope, "configuration_only");
}

#[test]
fn store_drop_releases_lock_even_while_duplicate_descriptor_survives() {
    let temp = Temp::new();
    let store = temp.store();
    save(&store);
    let duplicate = store._lock.0.try_clone().unwrap();
    assert!(
        matches!(Store::open(&temp.0.join("profiles")), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
    );
    drop(store);
    let reopened = temp.store();
    assert_eq!(reopened.load("test").unwrap().config, config());
    drop(duplicate);
    // The old duplicate's close must not release a different Store owner's lock.
    assert!(
        matches!(Store::open(&temp.0.join("profiles")), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
    );
    drop(reopened);
    let _last = temp.store();
}

#[test]
fn acquired_lock_guard_releases_on_constructor_error_with_duplicate_alive() {
    let temp = Temp::new();
    drop(temp.store());
    let path = temp.0.join("profiles/.lock");
    let mut duplicate = None;
    let construct = |duplicate: &mut Option<File>| -> io::Result<()> {
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let lock = StoreLock::acquire(file)?;
        *duplicate = Some(lock.0.try_clone()?);
        // Same error-propagation path as origin/staging validation after acquire.
        Err(invalid("synthetic construction failure"))
    };
    assert_eq!(
        construct(&mut duplicate).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    let _reopened = temp.store();
    drop(duplicate);
}
