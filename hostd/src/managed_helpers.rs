//! Explicit local helper installation. Inventory only reads metadata; it never executes helpers.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};
const MAX_MANIFEST: u64 = 512 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 128 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub executable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sniffle {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helper_capabilities: Vec<String>,
    pub python: String,
    pub helper: String,
    pub module_path: String,
    pub runtime_path: PathBuf,
    pub runtime_sha256: String,
    pub runtime_version: String,
    pub runtime_base_prefix: String,
    pub source_revision: String,
    pub version: String,
    pub pyserial_version: String,
    pub licenses: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alfa {
    pub executable: String,
    pub firmware: String,
    pub source_revision: String,
    pub source_match: String,
    pub licenses: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u8,
    pub platform: String,
    pub architecture: String,
    pub files: Vec<Entry>,
    pub sniffle: Option<Sniffle>,
    pub alfa: Option<Alfa>,
}
fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn relative(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 1024
        || value.chars().any(char::is_control)
        || Path::new(value).components().count() > 32
        || Path::new(value)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("bundle paths must be relative normal components"));
    }
    Ok(())
}
fn regular(path: &Path) -> io::Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(invalid("expected a regular file"));
    }
    Ok(f)
}
fn check_tree(root: &Path, relative_path: &str) -> io::Result<PathBuf> {
    relative(relative_path)?;
    let mut p = root.to_path_buf();
    for part in Path::new(relative_path).components() {
        p.push(part);
        if fs::symlink_metadata(&p)?.file_type().is_symlink() {
            return Err(invalid("bundle symlinks are not allowed"));
        }
    }
    Ok(p)
}
fn private_dir(path: &Path, create: bool) -> io::Result<()> {
    if create {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
    }
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.mode() & 0o777 != 0o700
        || m.uid() != unsafe { libc::geteuid() }
    {
        return Err(invalid(
            "helper store requires owned nonsymlink 0700 directories",
        ));
    }
    Ok(())
}
fn read_bounded(path: &Path, maximum: u64) -> io::Result<Vec<u8>> {
    let f = regular(path)?;
    if f.metadata()?.len() > maximum {
        return Err(invalid("helper metadata exceeds limit"));
    }
    let mut bytes = Vec::new();
    f.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(invalid("helper metadata exceeds limit"));
    }
    Ok(bytes)
}
fn digest_file(path: &Path) -> io::Result<String> {
    let mut f = regular(path)?;
    if f.metadata()?.len() > MAX_FILE {
        return Err(invalid("helper file exceeds limit"));
    }
    let mut h = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut count = 0;
    loop {
        let n = f.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > MAX_FILE {
            return Err(invalid("helper file exceeds limit"));
        }
        h.update(&buffer[..n]);
    }
    Ok(hex_bytes(&h.finalize()))
}
impl Manifest {
    fn validate(&self) -> io::Result<()> {
        if self.schema_version != 1
            || self.platform != std::env::consts::OS
            || self.architecture != std::env::consts::ARCH
        {
            return Err(invalid("unsupported helper schema/platform/architecture"));
        }
        if self.files.is_empty()
            || self.files.len() > 2048
            || self.sniffle.is_none() && self.alfa.is_none()
        {
            return Err(invalid("empty or oversized helper bundle"));
        }
        let mut paths = BTreeSet::new();
        let mut total = 0u64;
        for file in &self.files {
            relative(&file.path)?;
            if !paths.insert(file.path.as_str())
                || file.path == "manifest.json"
                || !hex(&file.sha256, 64)
                || file.bytes > MAX_FILE
            {
                return Err(invalid("invalid helper entry"));
            }
            total = total
                .checked_add(file.bytes)
                .ok_or_else(|| invalid("bundle size overflow"))?;
        }
        if total > MAX_TOTAL {
            return Err(invalid("helper bundle exceeds 128 MiB"));
        }
        let contains = |p: &str| -> io::Result<()> {
            relative(p)?;
            if !paths.contains(p) {
                return Err(invalid("component refers to an unlisted file"));
            }
            Ok(())
        };
        let executable = |p: &str| -> io::Result<()> {
            contains(p)?;
            if !self.files.iter().any(|f| f.path == p && f.executable) {
                return Err(invalid("component executable lacks executable mode"));
            }
            Ok(())
        };
        if let Some(s) = &self.sniffle {
            executable(&s.python)?;
            contains(&s.helper)?;
            relative(&s.module_path)?;
            contains(&format!("{}/sniffle/sniffle_hw.py", s.module_path))?;
            contains(&format!("{}/serial/__init__.py", s.module_path))?;
            if s.helper_capabilities.len() > 16
                || s.helper_capabilities.iter().any(|v| {
                    v.is_empty()
                        || v.len() > 64
                        || !v
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                })
            {
                return Err(invalid("invalid helper capability identifiers"));
            }
            if s.version != "1.11.0"
                || s.pyserial_version != "3.5"
                || s.source_revision != "18252f2c5a05f9ad906865b776652e31c61471c4"
                || !hex(&s.runtime_sha256, 64)
                || !s.runtime_path.is_absolute()
                || s.licenses.len() < 2
            {
                return Err(invalid("unsupported Sniffle runtime/provenance"));
            }
            if [
                "/tmp",
                "/private/tmp",
                "/var/folders",
                "/private/var/folders",
            ]
            .iter()
            .any(|p| s.runtime_path.starts_with(p))
            {
                return Err(invalid(
                    "Python runtime must not depend on a temporary directory",
                ));
            }
            for p in &s.licenses {
                contains(p)?;
            }
        }
        if let Some(a) = &self.alfa {
            executable(&a.executable)?;
            contains(&a.firmware)?;
            if !hex(&a.source_revision, 40)
                || a.source_match != "local_snapshot_not_reproducibly_verified"
                || a.licenses.len() < 3
            {
                return Err(invalid("ALFA source and license provenance required"));
            }
            for p in &a.licenses {
                contains(p)?;
            }
            contains("alfa/source/Cargo.toml")?;
            contains("alfa/source/src/main.rs")?;
            if !self.files.iter().any(|f| {
                f.path == a.firmware
                    && f.sha256
                        == "538d08c08f8fb5aa05cacf34e49aeb307e8b2dc094407fc655fbf3580c893953"
            }) {
                return Err(invalid("unrecognized ALFA reference firmware hash"));
            }
        }
        Ok(())
    }
}
pub fn default_root() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| invalid("HOME is not set"))?;
    Ok(if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Application Support/AirHound/helpers")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".local/share"))
            .join("airhound/helpers")
    })
}
fn load(root: &Path) -> io::Result<(PathBuf, Manifest, String)> {
    private_dir(root, false)?;
    let pointer = read_bounded(&root.join("current"), 128)?;
    let id = std::str::from_utf8(&pointer)
        .map_err(io::Error::other)?
        .trim();
    if !hex(id, 64) {
        return Err(invalid("invalid helper bundle identifier"));
    }
    let bundle = root.join("bundles").join(id);
    private_dir(&root.join("bundles"), false)?;
    private_dir(&bundle, false)?;
    let bytes = read_bounded(&bundle.join("manifest.json"), MAX_MANIFEST)?;
    if hex_bytes(&Sha256::digest(&bytes)) != id {
        return Err(invalid("helper manifest hash mismatch"));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    manifest.validate()?;
    Ok((bundle, manifest, id.into()))
}
fn defaults(bundle: &Path, m: &Manifest) -> Value {
    json!({"sniffle":m.sniffle.as_ref().map(|s|json!({"python":bundle.join(&s.python),"helper":bundle.join(&s.helper),"module_path":bundle.join(&s.module_path)})),"external_wifi":m.alfa.as_ref().map(|a|json!({"executable":bundle.join(&a.executable),"firmware":bundle.join(&a.firmware)}))})
}
pub fn status(root: &Path) -> Value {
    match load(root) {
        Ok((bundle, m, id)) => {
            json!({"state":"configured","bundle_id":id,"defaults":defaults(&bundle,&m),"provenance":{"sniffle":m.sniffle,"alfa":m.alfa,"files":m.files.len()},"validation_scope":"manifest_only","runtime_verified":false,"rf_verified":false})
        }
        Err(e) => {
            json!({"state":if e.kind()==io::ErrorKind::NotFound{"not_installed"}else{"invalid"},"message":e.to_string(),"defaults":null})
        }
    }
}
pub fn default_status() -> Value {
    match default_root() {
        Ok(root) => status(&root),
        Err(e) => json!({"state":"invalid","message":e.to_string()}),
    }
}
pub fn discovery_options(all: bool) -> crate::discovery::Options {
    let state = default_status();
    let d = &state["defaults"];
    let path = |component: &str, name: &str| d[component][name].as_str().map(PathBuf::from);
    crate::discovery::Options {
        all,
        sniffle_python: path("sniffle", "python"),
        sniffle_helper: path("sniffle", "helper"),
        sniffle_module_path: path("sniffle", "module_path"),
        external_scanner: path("external_wifi", "executable"),
        external_firmware: path("external_wifi", "firmware"),
    }
}
/// Hash checks are explicit setup work. This does not execute code or open radios.
fn verify_layout(bundle: &Path, m: &Manifest) -> io::Result<()> {
    let mut allowed_files: BTreeSet<PathBuf> =
        m.files.iter().map(|f| PathBuf::from(&f.path)).collect();
    allowed_files.insert("manifest.json".into());
    let mut allowed_dirs = BTreeSet::new();
    for file in &allowed_files {
        for parent in file.ancestors().skip(1) {
            if !parent.as_os_str().is_empty() {
                allowed_dirs.insert(parent.to_path_buf());
            }
        }
    }
    let limit = allowed_files.len() + allowed_dirs.len();
    let mut seen = 0;
    let mut pending = vec![bundle.to_path_buf()];
    while let Some(directory) = pending.pop() {
        private_dir(&directory, false)?;
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            seen += 1;
            if seen > limit {
                return Err(invalid("unexpected installed bundle entries"));
            }
            let relative = path.strip_prefix(bundle).map_err(io::Error::other)?;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() && allowed_dirs.contains(relative) {
                pending.push(path);
            } else if !metadata.is_file()
                || !allowed_files.contains(relative)
                || metadata.uid() != unsafe { libc::geteuid() }
            {
                return Err(invalid(
                    "unexpected file, symlink or special node in installed bundle",
                ));
            }
        }
    }
    if seen != limit {
        return Err(invalid("installed bundle is incomplete"));
    }
    let manifest = fs::symlink_metadata(bundle.join("manifest.json"))?;
    if manifest.mode() & 0o777 != 0o600 {
        return Err(invalid("installed manifest must be private"));
    }
    Ok(())
}
fn verify_bundle(bundle: &Path, m: &Manifest) -> io::Result<()> {
    verify_layout(bundle, m)?;
    for f in &m.files {
        let path = check_tree(bundle, &f.path)?;
        let meta = fs::symlink_metadata(&path)?;
        if meta.uid() != unsafe { libc::geteuid() }
            || meta.len() != f.bytes
            || meta.mode() & 0o777 != if f.executable { 0o700 } else { 0o600 }
            || digest_file(&path)? != f.sha256
        {
            return Err(invalid("installed helper integrity mismatch"));
        }
    }
    if let Some(s) = &m.sniffle {
        if fs::canonicalize(&s.runtime_path)? != s.runtime_path {
            return Err(invalid(
                "external Python must use its canonical stable path",
            ));
        }
        if digest_file(&s.runtime_path)? != s.runtime_sha256 {
            return Err(invalid(
                "external Python runtime has changed; restage explicitly",
            ));
        }
    }
    Ok(())
}
pub fn verify(root: &Path) -> io::Result<Value> {
    let (bundle, m, id) = load(root)?;
    verify_bundle(&bundle, &m)?;
    Ok(
        json!({"bundle_id":id,"integrity":"verified","rf_verified":false,"defaults":defaults(&bundle,&m)}),
    )
}
/// Import local user-selected code. No downloads, execution, radio access, or automatic replacement.
pub fn install(root: &Path, source: &Path, replace: bool) -> io::Result<Value> {
    if !root.is_absolute() || !source.is_absolute() {
        return Err(invalid("install paths must be absolute"));
    }
    private_dir(
        root.parent()
            .ok_or_else(|| invalid("helper store needs parent"))?,
        false,
    )?;
    private_dir(root, true)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join(".lock"))?;
    if !lock.metadata()?.is_file()
        || lock.metadata()?.uid() != unsafe { libc::geteuid() }
        || lock.metadata()?.mode() & 0o777 != 0o600
    {
        return Err(invalid("unsafe helper lock"));
    }
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes = read_bounded(&source.join("manifest.json"), MAX_MANIFEST)?;
    let m: Manifest = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    m.validate()?;
    let id = hex_bytes(&Sha256::digest(&bytes));
    if fs::symlink_metadata(root.join("current")).is_ok() && !replace {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "helpers already installed; explicit --replace required",
        ));
    }
    private_dir(&root.join("bundles"), true)?;
    let final_dir = root.join("bundles").join(&id);
    let pending = root.join(format!(".pending-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&pending)?;
    let result = (|| {
        for entry in &m.files {
            let from = check_tree(source, &entry.path)?;
            let input = regular(&from)?;
            if input.metadata()?.len() != entry.bytes {
                return Err(invalid("source size mismatch"));
            }
            let to = pending.join(&entry.path);
            let parent = to.parent().ok_or_else(|| invalid("missing entry parent"))?;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if entry.executable { 0o700 } else { 0o600 })
                .open(&to)?;
            let count = io::copy(&mut input.take(entry.bytes + 1), &mut out)?;
            out.sync_all()?;
            // Persist directory entries before publishing the bundle pointer.
            for directory in parent.ancestors().take_while(|p| p.starts_with(&pending)) {
                File::open(directory)?.sync_all()?;
            }
            if count != entry.bytes || digest_file(&to)? != entry.sha256 {
                return Err(invalid("source helper hash/size mismatch"));
            }
        }
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(pending.join("manifest.json"))?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        File::open(&pending)?.sync_all()?;
        verify_bundle(&pending, &m)?;
        if final_dir.exists() {
            private_dir(&final_dir, false)?;
            verify_bundle(&final_dir, &m)?;
            if read_bounded(&final_dir.join("manifest.json"), MAX_MANIFEST)? != bytes {
                return Err(invalid("existing bundle manifest mismatch"));
            }
        } else {
            fs::rename(&pending, &final_dir)?;
        }
        File::open(root.join("bundles"))?.sync_all()?;
        let pointer = root.join(format!(".current-{}", std::process::id()));
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pointer)?;
        out.write_all(id.as_bytes())?;
        out.sync_all()?;
        fs::rename(pointer, root.join("current"))?;
        File::open(root)?.sync_all()?;
        Ok(
            json!({"bundle_id":id,"integrity":"verified","rf_verified":false,"defaults":defaults(&final_dir,&m)}),
        )
    })();
    if pending.exists() {
        let _ = fs::remove_dir_all(&pending);
    }
    result
}
#[cfg(test)]
mod tests;
