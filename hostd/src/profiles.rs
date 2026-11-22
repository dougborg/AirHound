//! Backend-owned saved configurations. Loading never opens a capture transport.
use crate::{discovery::Inventory, session::SessionConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 128 * 1024;
const MAX_PROFILES: usize = 64;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub receiver_id: String,
    pub source_kind: String,
    pub selector: Option<String>,
    pub device_ids: Vec<String>,
    pub identity_scope: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u8,
    pub id: String,
    pub name: String,
    pub saved_at_ms: u64,
    pub config: Value,
    pub bindings: Vec<Binding>,
    /// A saved configuration is not evidence that RF reception was tested.
    pub validation_scope: String,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn identifier(id: &str) -> io::Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err(invalid(
            "profile ID must contain 1..64 ASCII letters, digits, - or _",
        ));
    }
    Ok(())
}
fn name_valid(name: &str) -> io::Result<()> {
    if name.trim().is_empty() || name.len() > 160 || name.chars().any(char::is_control) {
        return Err(invalid(
            "profile name must contain 1..160 bytes without control characters",
        ));
    }
    Ok(())
}
fn checked_config(config: &Value) -> io::Result<()> {
    if serde_json::to_vec(config).map_err(io::Error::other)?.len() > airhound_api::MAX_CONFIG_BYTES
    {
        return Err(invalid("profile configuration exceeds 64 KiB"));
    }
    SessionConfig::from_json(config.clone())?.resolve()?;
    Ok(())
}
fn private_metadata(metadata: &fs::Metadata, directory: bool) -> io::Result<()> {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid
        || metadata.mode() & 0o777 != if directory { 0o700 } else { 0o600 }
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
        || metadata.file_type().is_symlink()
    {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "profiles require owned, nonsymlink private directories (0700) and regular files (0600)"));
    }
    Ok(())
}

pub struct Store {
    directory: PathBuf,
    _lock: File,
}
impl Store {
    /// Lock a private store. The service's socket parent is the persistence scope.
    pub fn open(directory: &Path) -> io::Result<Self> {
        if !directory.is_absolute() {
            return Err(invalid("profile directory must be absolute"));
        }
        private_metadata(
            &fs::symlink_metadata(
                directory
                    .parent()
                    .ok_or_else(|| invalid("profile directory needs a parent"))?,
            )?,
            true,
        )?;
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        private_metadata(&fs::symlink_metadata(directory)?, true)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join(".lock"))?;
        private_metadata(&lock.metadata()?, false)?;
        // SAFETY: the owned descriptor stays open for the lifetime of this store.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "profile store is busy; retry the operation",
            ));
        }
        // A killed writer can leave its private staging file. No live writer can
        // hold this lock now; remove only our bounded, validated staging names.
        for (index, entry) in fs::read_dir(directory)?.enumerate() {
            if index > MAX_PROFILES * 2 {
                return Err(invalid("profile directory contains too many entries"));
            }
            let entry = entry?;
            let name = entry.file_name();
            let Some(suffix) = name.to_str().and_then(|n| n.strip_prefix(".pending-")) else {
                continue;
            };
            let parts: Vec<_> = suffix.split('-').collect();
            if parts.len() != 2
                || parts
                    .iter()
                    .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(invalid("unexpected staging file in profile store"));
            }
            private_metadata(&fs::symlink_metadata(entry.path())?, false)?;
            fs::remove_file(entry.path())?;
        }
        Ok(Self {
            directory: directory.into(),
            _lock: lock,
        })
    }
    fn path(&self, id: &str) -> io::Result<PathBuf> {
        identifier(id)?;
        Ok(self.directory.join(format!("{id}.json")))
    }
    pub fn load(&self, id: &str) -> io::Result<Profile> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path(id)?)?;
        private_metadata(&file.metadata()?, false)?;
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("profile exceeds 128 KiB"));
        }
        let profile: Profile = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if profile.schema_version != 1 {
            return Err(invalid(
                "unsupported profile schema; no automatic migration is available",
            ));
        }
        if profile.id != id {
            return Err(invalid("profile ID does not match its filename"));
        }
        name_valid(&profile.name)?;
        checked_config(&profile.config)?;
        if profile.validation_scope != "configuration_only" {
            return Err(invalid("profile cannot assert RF verification"));
        }
        // Binding records are advisory evidence, not authority to select a new endpoint.
        if profile.bindings.len() > 16 {
            return Err(invalid("too many profile bindings"));
        }
        Ok(profile)
    }
    pub fn list(&self) -> io::Result<Value> {
        let mut profiles = Vec::new();
        for entry in fs::read_dir(&self.directory)?.take(MAX_PROFILES + 3) {
            let entry = entry?;
            if entry.file_name() == ".lock" {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                return Err(invalid("unexpected file in profile store"));
            }
            let id = path
                .file_stem()
                .and_then(|v| v.to_str())
                .ok_or_else(|| invalid("invalid profile filename"))?;
            let profile = self.load(id)?;
            profiles.push(json!({"id":profile.id,"name":profile.name,"saved_at_ms":profile.saved_at_ms,"receiver_count":profile.bindings.len(),"schema_version":profile.schema_version,"validation_scope":profile.validation_scope}));
            if profiles.len() > MAX_PROFILES {
                return Err(invalid("profile store exceeds 64 profiles"));
            }
        }
        profiles.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(json!({"schema_version":1,"profiles":profiles}))
    }
    /// Caller performs local-resource preview before this operation. No external
    /// command, serial endpoint, radio, or path in the plan is opened here.
    pub fn save(
        &self,
        id: String,
        name: String,
        config: Value,
        inventory: &Inventory,
        replace: bool,
    ) -> io::Result<Profile> {
        let destination = self.path(&id)?;
        name_valid(&name)?;
        checked_config(&config)?;
        if check_resources(&config)["valid"] != true {
            return Err(invalid(
                "profile helper or replay paths are missing or invalid",
            ));
        }
        let exists = match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                private_metadata(&metadata, false)?;
                true
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        if exists && !replace {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "profile exists; explicit replace is required",
            ));
        }
        // Listing also validates existing files and ensures a corrupt/foreign file
        // is not silently overwritten. Unknown schemas require a future migration.
        let listed = self.list()?;
        if !exists && listed["profiles"].as_array().unwrap().len() >= MAX_PROFILES {
            return Err(invalid("profile store is full (64 profiles)"));
        }
        let profile = Profile {
            schema_version: 1,
            id,
            name,
            saved_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            bindings: bindings(&config, inventory),
            config,
            validation_scope: "configuration_only".into(),
        };
        let bytes = serde_json::to_vec_pretty(&profile).map_err(io::Error::other)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("profile exceeds 128 KiB"));
        }
        let temporary = self.directory.join(format!(
            ".pending-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            if replace && exists {
                fs::rename(&temporary, &destination)?;
            } else {
                fs::hard_link(&temporary, &destination)?;
                fs::remove_file(&temporary)?;
            }
            File::open(&self.directory)?.sync_all()?;
            Ok(profile)
        })();
        let _ = fs::remove_file(&temporary);
        result
    }
    pub fn delete(&self, id: &str) -> io::Result<Value> {
        let path = self.path(id)?;
        private_metadata(&fs::symlink_metadata(&path)?, false)?;
        // Deletion permits unsupported/corrupt schema files, but never follows them.
        fs::remove_file(path)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(json!({"schema_version":1,"deleted":id}))
    }
}

fn bindings(config: &Value, inventory: &Inventory) -> Vec<Binding> {
    config["receivers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|receiver| {
            let source = &receiver["source"];
            let kind = source["kind"].as_str().unwrap_or("unknown");
            let selector = match kind {
                "sniffle" | "airhound_serial" => source["port"].as_str(),
                "linux_wifi" => source["interface"].as_str(),
                "external_wifi" => source["usb_selector"].as_str(),
                "native_ble" => source["adapter"].as_str(),
                _ => None,
            }
            .map(str::to_owned);
            let mut device_ids: Vec<String> = inventory
                .devices
                .iter()
                .filter(|device| match kind {
                    "sniffle" | "airhound_serial" => device
                        .endpoints
                        .iter()
                        .any(|ep| ep.kind == "serial" && Some(&ep.path) == selector.as_ref()),
                    "linux_wifi" => device
                        .endpoints
                        .iter()
                        .any(|ep| ep.kind == "linux_wifi" && Some(&ep.path) == selector.as_ref()),
                    "native_ble" | "native_wifi" if selector.is_none() => {
                        device.candidates.iter().any(|c| c.kind == kind)
                    }
                    _ => false,
                })
                .map(|device| device.id.clone())
                .collect();
            device_ids.sort();
            device_ids.dedup();
            Binding {
                receiver_id: receiver["id"].as_str().unwrap_or_default().into(),
                source_kind: kind.into(),
                selector,
                device_ids,
                identity_scope: if kind.starts_with("native_") {
                    "system_managed"
                } else {
                    "endpoint_only"
                }
                .into(),
            }
        })
        .collect()
}

/// Presence checks use inventory only. The same endpoint/topology can be reused
/// by a replacement device, so a successful match is never physical identity.
pub fn check_bindings(profile: &Profile, inventory: &Inventory) -> Value {
    let checks: Vec<_> = bindings(&profile.config, inventory).into_iter().map(|current| {
        let previous = profile.bindings.iter().find(|b| b.receiver_id == current.receiver_id);
        let state = match current.source_kind.as_str() {
            "ble_pcap" | "wifi_pcap" => "not_a_device",
            "external_wifi" => "selector_requires_review",
            "native_ble" if current.selector.is_some() => "selector_requires_review",
            _ if current.device_ids.is_empty() => "missing",
            _ if current.device_ids.len() > 1 => "ambiguous",
            _ if current.identity_scope == "system_managed" => "system_managed",
            _ if previous.is_none_or(|p| p.device_ids != current.device_ids || p.selector != current.selector) => "changed_requires_identity_review",
            _ => "present_requires_identity_review",
        };
        json!({"receiver_id":current.receiver_id,"state":state,"binding":current,"physical_identity_verified":false})
    }).collect();
    json!({"checks":checks,"automatic_rebinding":false,"message":"Saved endpoints reuse settings only. A matching port or USB topology does not prove the same physical device. Review receiver selection before starting."})
}

/// Inspect only metadata; never open a configured file, FIFO, device or helper.
pub fn check_resources(config: &Value) -> Value {
    let mut checks = Vec::new();
    for receiver in config["receivers"].as_array().into_iter().flatten() {
        let source = &receiver["source"];
        let fields: &[(&str, &str)] = match source["kind"].as_str() {
            Some("sniffle") => &[
                ("python", "executable"),
                ("helper", "file"),
                ("module_path", "optional_directory"),
            ],
            Some("external_wifi") => &[("executable", "executable"), ("firmware", "file")],
            Some("ble_pcap" | "wifi_pcap") => &[("path", "file")],
            _ => &[],
        };
        for (field, expected) in fields {
            if source[*field].is_null() && *expected == "optional_directory" {
                continue;
            }
            let path = source[*field].as_str().unwrap_or_default();
            let state = if !Path::new(path).is_absolute() {
                "absolute_path_required"
            } else {
                match fs::metadata(path) {
                    Ok(m) if *expected == "optional_directory" && m.is_dir() => "present",
                    Ok(m) if *expected == "executable" && m.is_file() && m.mode() & 0o111 != 0 => {
                        "present"
                    }
                    Ok(m) if *expected == "file" && m.is_file() => "present",
                    Ok(_) => "wrong_type_or_mode",
                    Err(e) if e.kind() == io::ErrorKind::NotFound => "missing",
                    Err(_) => "unavailable",
                }
            };
            checks.push(
                json!({"receiver_id":receiver["id"],"field":field,"path":path,"state":state}),
            );
        }
    }
    json!({"valid":checks.iter().all(|check| check["state"] == "present"),"checks":checks,"scope":"local_resource_metadata_only"})
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
