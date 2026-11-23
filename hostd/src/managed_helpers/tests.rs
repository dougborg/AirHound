use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "airhound-helpers-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(dir: &Path) -> Manifest {
    let runtime = std::env::current_exe().unwrap();
    let content = b"fixture";
    let names = [
        "sniffle/python",
        "sniffle/helper.py",
        "sniffle/modules/sniffle/sniffle_hw.py",
        "sniffle/modules/serial/__init__.py",
        "license1",
        "license2",
    ];
    let files = names
        .iter()
        .map(|name| {
            let p = dir.join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
            Entry {
                path: (*name).into(),
                bytes: content.len() as u64,
                sha256: hex_bytes(&Sha256::digest(content)),
                executable: *name == "sniffle/python",
            }
        })
        .collect();
    Manifest {
        schema_version: 1,
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        files,
        sniffle: Some(Sniffle {
            helper_capabilities: Vec::new(),
            python: "sniffle/python".into(),
            helper: "sniffle/helper.py".into(),
            module_path: "sniffle/modules".into(),
            runtime_path: runtime,
            runtime_sha256: "0".repeat(64),
            runtime_version: "test".into(),
            runtime_base_prefix: "test".into(),
            source_revision: "18252f2c5a05f9ad906865b776652e31c61471c4".into(),
            version: "1.11.0".into(),
            pyserial_version: "3.5".into(),
            licenses: vec!["license1".into(), "license2".into()],
        }),
        alfa: None,
    }
}
#[test]
fn rejects_paths_versions_sizes_and_missing_licenses() {
    let t = Temp::new();
    let mut m = fixture(&t.0);
    m.sniffle.as_mut().unwrap().runtime_path = "/usr/bin/python3".into();
    assert!(m.validate().is_ok());
    for path in ["../escape", "/absolute", "a/../b", ""] {
        let mut bad = m.clone();
        bad.files[0].path = path.into();
        assert!(bad.validate().is_err());
    }
    let mut bad = m.clone();
    bad.schema_version = 2;
    assert!(bad.validate().is_err());
    let mut bad = m.clone();
    bad.files[0].bytes = MAX_FILE + 1;
    assert!(bad.validate().is_err());
    let mut bad = m;
    bad.sniffle.as_mut().unwrap().licenses.clear();
    assert!(bad.validate().is_err());
}
#[test]
fn passive_missing_store_does_not_create_it() {
    let t = Temp::new();
    let root = t.0.join("missing");
    assert_eq!(status(&root)["state"], "not_installed");
    assert!(!root.exists());
}
#[test]
fn no_follow_and_nonblocking_reject_symlink_fifo() {
    let t = Temp::new();
    fs::write(t.0.join("file"), b"x").unwrap();
    std::os::unix::fs::symlink(t.0.join("file"), t.0.join("link")).unwrap();
    assert!(regular(&t.0.join("link")).is_err());
    let path = std::ffi::CString::new(t.0.join("fifo").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(regular(&t.0.join("fifo")).is_err());
    assert!(check_tree(&t.0, "link").is_err());
}
#[test]
fn failed_copy_does_not_publish_defaults() {
    let t = Temp::new();
    let source = t.0.join("source");
    fs::create_dir(&source).unwrap();
    let mut m = fixture(&source);
    m.sniffle.as_mut().unwrap().runtime_path = "/usr/bin/python3".into();
    m.files[0].sha256 = "0".repeat(64);
    fs::write(
        source.join("manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    let root = t.0.join("helpers");
    assert!(install(&root, &source, false).is_err());
    assert!(!root.join("current").exists());
    assert_eq!(
        fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".pending"))
            .count(),
        0
    );
}
fn write_fixture(source: &Path) -> Manifest {
    fs::create_dir(source).unwrap();
    let mut m = fixture(source);
    let s = m.sniffle.as_mut().unwrap();
    s.runtime_path = fs::canonicalize("/usr/bin/true").unwrap();
    s.runtime_sha256 = digest_file(&s.runtime_path).unwrap();
    write_manifest(source, &m);
    m
}
fn write_manifest(source: &Path, m: &Manifest) {
    fs::write(source.join("manifest.json"), serde_json::to_vec(m).unwrap()).unwrap();
}
#[test]
fn import_is_private_immutable_and_requires_explicit_replace() {
    let t = Temp::new();
    let source = t.0.join("source");
    let mut m = write_fixture(&source);
    let root = t.0.join("helpers");
    let first = install(&root, &source, false).unwrap();
    assert_eq!(verify(&root).unwrap()["integrity"], "verified");
    assert_eq!(status(&root)["validation_scope"], "manifest_only");
    assert!(install(&root, &source, false).is_err());
    let old_path = PathBuf::from(first["defaults"]["sniffle"]["helper"].as_str().unwrap());
    assert_eq!(fs::metadata(&old_path).unwrap().mode() & 0o777, 0o600);
    m.sniffle.as_mut().unwrap().runtime_version = "updated description".into();
    write_manifest(&source, &m);
    let second = install(&root, &source, true).unwrap();
    assert_ne!(first["bundle_id"], second["bundle_id"]);
    assert!(old_path.exists());
    let current = PathBuf::from(second["defaults"]["sniffle"]["helper"].as_str().unwrap());
    fs::write(current, b"tampered").unwrap();
    assert!(verify(&root).is_err());
}
#[test]
fn failed_runtime_check_preserves_old_pointer() {
    let t = Temp::new();
    let source = t.0.join("source");
    let mut m = write_fixture(&source);
    let root = t.0.join("helpers");
    let first = install(&root, &source, false).unwrap();
    m.sniffle.as_mut().unwrap().runtime_sha256 = "0".repeat(64);
    write_manifest(&source, &m);
    assert!(install(&root, &source, true).is_err());
    assert_eq!(status(&root)["bundle_id"], first["bundle_id"]);
    assert!(verify(&root).is_ok());
}

#[test]
fn unexpected_importable_code_and_nodes_fail_integrity_and_reuse() {
    let t = Temp::new();
    let source = t.0.join("source");
    write_fixture(&source);
    let root = t.0.join("helpers");
    let first = install(&root, &source, false).unwrap();
    let modules = PathBuf::from(
        first["defaults"]["sniffle"]["module_path"]
            .as_str()
            .unwrap(),
    );
    let injected = modules.join("extra.py");
    fs::write(&injected, b"unexpected code").unwrap();
    assert!(verify(&root).is_err());
    assert!(install(&root, &source, true).is_err());
    fs::remove_file(&injected).unwrap();
    std::os::unix::fs::symlink("missing", &injected).unwrap();
    assert!(verify(&root).is_err());
    fs::remove_file(&injected).unwrap();
    assert!(verify(&root).is_ok());
}
#[test]
fn dangling_current_pointer_requires_explicit_replacement() {
    let t = Temp::new();
    let source = t.0.join("source");
    write_fixture(&source);
    let root = t.0.join("helpers");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    std::os::unix::fs::symlink("missing", root.join("current")).unwrap();
    assert_eq!(
        install(&root, &source, false).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    install(&root, &source, true).expect("explicit replacement of dangling pointer");
}

#[test]
fn install_lock_releases_ownership_before_inherited_descriptor_closes() {
    let t = Temp::new();
    let owned = acquire_install_lock(&t.0).unwrap();
    // dup and fork share the same open-file description and flock ownership.
    // Keep that description alive after the owning scope exits.
    let inherited = owned.0.try_clone().unwrap();
    let competing = acquire_install_lock(&t.0).err().unwrap();
    assert_eq!(competing.kind(), io::ErrorKind::WouldBlock);
    drop(owned);
    let replacement = acquire_install_lock(&t.0).unwrap();
    drop(inherited);
    // Closing the inherited descriptor must not unlock the new owner's flock.
    assert_eq!(
        acquire_install_lock(&t.0).err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(replacement);
    assert!(acquire_install_lock(&t.0).is_ok());
}

#[test]
fn helper_capabilities_are_optional_bounded_and_never_inferred_from_firmware() {
    let t = Temp::new();
    let mut manifest = fixture(&t.0);
    manifest.sniffle.as_mut().unwrap().runtime_path = "/usr/bin/python3".into();
    let legacy = serde_json::to_value(&manifest).unwrap();
    assert!(legacy["sniffle"].get("helper_capabilities").is_none());
    let old: Manifest = serde_json::from_value(legacy).unwrap();
    assert!(old.sniffle.unwrap().helper_capabilities.is_empty());
    for values in [
        vec!["x".repeat(65)],
        vec!["valid".into(); 17],
        vec!["bad/name".into()],
    ] {
        manifest.sniffle.as_mut().unwrap().helper_capabilities = values;
        assert!(manifest.validate().is_err());
    }
    manifest.sniffle.as_mut().unwrap().helper_capabilities = vec![
        "borrowed_serial_lease_v1".into(),
        "structured_errors_v1".into(),
    ];
    assert!(manifest.validate().is_ok());
}
