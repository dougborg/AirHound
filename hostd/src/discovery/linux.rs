//! Read-only sysfs inventory. Device nodes are never opened or configured.
use super::{Diagnostic, Endpoint, RawDevice};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const ENTRY_LIMIT: usize = 4096;
const ATTRIBUTE_LIMIT: u64 = 1024;

#[cfg(target_os = "linux")]
pub(super) fn collect() -> (Vec<RawDevice>, Vec<Diagnostic>) {
    collect_from(Path::new("/sys"))
}

fn diagnostic(diagnostics: &mut Vec<Diagnostic>, message: String) {
    diagnostics.push(Diagnostic {
        backend: "linux_sysfs".into(),
        message,
    });
}

fn entries(path: &Path, optional: bool, diagnostics: &mut Vec<Diagnostic>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if optional && error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            diagnostic(
                diagnostics,
                format!("Cannot enumerate {}: {error}", path.display()),
            );
            return Vec::new();
        }
    };
    let mut paths = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index == ENTRY_LIMIT {
            diagnostic(
                diagnostics,
                format!(
                    "Inventory truncated at {ENTRY_LIMIT} entries in {}",
                    path.display()
                ),
            );
            break;
        }
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(error) => diagnostic(
                diagnostics,
                format!("Cannot read entry in {}: {error}", path.display()),
            ),
        }
    }
    paths.sort();
    paths
}

fn attribute(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(ATTRIBUTE_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > ATTRIBUTE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sysfs attribute exceeds bound",
        ));
    }
    let value = String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(value.trim().to_owned())
}

fn hex_attribute(path: &Path) -> io::Result<u16> {
    u16::from_str_radix(&attribute(path)?, 16)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    (!name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)))
    .then(|| name.to_owned())
}

fn canonical(path: &Path, root: &Path, diagnostics: &mut Vec<Diagnostic>) -> Option<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) if path.starts_with(root) => Some(path),
        Ok(_) => {
            diagnostic(
                diagnostics,
                format!(
                    "Ignoring sysfs link outside inventory root: {}",
                    path.display()
                ),
            );
            None
        }
        Err(error) => {
            diagnostic(
                diagnostics,
                format!("Cannot resolve {}: {error}", path.display()),
            );
            None
        }
    }
}

fn usb_owner<'a>(path: &Path, usb: &'a BTreeMap<PathBuf, String>) -> Option<&'a String> {
    path.ancestors().find_map(|parent| usb.get(parent))
}

fn attach(
    devices: &mut BTreeMap<String, RawDevice>,
    id: String,
    name: String,
    kind: &str,
    path: String,
) {
    let device = devices.entry(id.clone()).or_insert_with(|| RawDevice {
        id,
        name,
        vid: None,
        pid: None,
        endpoints: Vec::new(),
    });
    if !device
        .endpoints
        .iter()
        .any(|e| e.kind == kind && e.path == path)
    {
        device.endpoints.push(Endpoint {
            kind: kind.into(),
            path,
        });
    }
}

fn collect_from(root: &Path) -> (Vec<RawDevice>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let root = match fs::canonicalize(root) {
        Ok(root) => root,
        Err(error) => {
            diagnostic(
                &mut diagnostics,
                format!("Cannot access sysfs inventory: {error}"),
            );
            return (Vec::new(), diagnostics);
        }
    };
    let mut devices = BTreeMap::new();
    let mut usb = BTreeMap::new();
    for path in entries(&root.join("bus/usb/devices"), false, &mut diagnostics) {
        let Some(node) = name(&path) else { continue };
        // USB interface nodes share their parent device's identity.
        if node.contains(':') {
            continue;
        }
        let Some(physical) = canonical(&path, &root, &mut diagnostics) else {
            continue;
        };
        let ids = hex_attribute(&physical.join("idVendor"))
            .and_then(|vid| hex_attribute(&physical.join("idProduct")).map(|pid| (vid, pid)));
        let (vid, pid) = match ids {
            Ok(ids) => ids,
            Err(error) => {
                diagnostic(
                    &mut diagnostics,
                    format!("Cannot read USB identity for {node}: {error}"),
                );
                continue;
            }
        };
        let id = format!("linux:usb:{node}");
        let label = attribute(&physical.join("product"))
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("USB {vid:04x}:{pid:04x}"));
        devices.insert(
            id.clone(),
            RawDevice {
                id: id.clone(),
                name: label,
                vid: Some(vid),
                pid: Some(pid),
                endpoints: vec![Endpoint {
                    kind: "usb".into(),
                    path: path.to_string_lossy().into_owned(),
                }],
            },
        );
        usb.insert(physical, id);
    }
    for path in entries(&root.join("class/tty"), true, &mut diagnostics) {
        let Some(node) = name(&path) else { continue };
        // Console/PTY nodes are not receiver candidates. USB ancestry is checked
        // for conventional UART device names without opening /dev nodes.
        if !["ttyUSB", "ttyACM", "ttyAMA", "ttyS", "rfcomm"]
            .iter()
            .any(|prefix| node.starts_with(prefix))
        {
            continue;
        }
        let Some(physical) = canonical(&path, &root, &mut diagnostics) else {
            continue;
        };
        let id = usb_owner(&physical, &usb)
            .cloned()
            .unwrap_or_else(|| format!("linux:serial:{node}"));
        attach(
            &mut devices,
            id,
            format!("Serial {node}"),
            "serial",
            format!("/dev/{node}"),
        );
    }
    for path in entries(&root.join("class/bluetooth"), true, &mut diagnostics) {
        let Some(node) = name(&path) else { continue };
        if !node
            .strip_prefix("hci")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let Some(physical) = canonical(&path, &root, &mut diagnostics) else {
            continue;
        };
        let id = usb_owner(&physical, &usb)
            .cloned()
            .unwrap_or_else(|| format!("linux:bluetooth:{node}"));
        attach(
            &mut devices,
            id,
            format!("Bluetooth {node}"),
            "native_ble",
            node,
        );
    }
    for path in entries(&root.join("class/net"), true, &mut diagnostics) {
        let Some(node) = name(&path) else { continue };
        let phy_link = path.join("phy80211");
        match fs::symlink_metadata(&phy_link) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                diagnostic(
                    &mut diagnostics,
                    format!("Cannot inspect WiFi radio {node}: {error}"),
                );
                continue;
            }
            Ok(_) => {}
        }
        let Some(phy) = canonical(&phy_link, &root, &mut diagnostics) else {
            continue;
        };
        let Some(physical) = canonical(&path, &root, &mut diagnostics) else {
            continue;
        };
        let Some(phy_name) = name(&phy) else { continue };
        let id = usb_owner(&physical, &usb)
            .or_else(|| usb_owner(&phy, &usb))
            .cloned()
            .unwrap_or_else(|| format!("linux:wifi:{phy_name}"));
        let kind = match attribute(&path.join("type")) {
            Ok(value) if value == "803" => "linux_wifi",
            Ok(_) => "native_wifi",
            Err(error) => {
                diagnostic(
                    &mut diagnostics,
                    format!("Cannot establish monitor mode for {node}: {error}"),
                );
                "native_wifi"
            }
        };
        attach(
            &mut devices,
            id.clone(),
            format!("WiFi {phy_name}"),
            kind,
            node,
        );
        attach(
            &mut devices,
            id,
            format!("WiFi {phy_name}"),
            "wifi_phy",
            phy_name,
        );
    }
    for device in devices.values_mut() {
        device
            .endpoints
            .sort_by(|a, b| (&a.kind, &a.path).cmp(&(&b.kind, &b.path)));
    }
    (devices.into_values().collect(), diagnostics)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    static NEXT_TREE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "airhound-discovery-linux-{}-{}",
                std::process::id(),
                NEXT_TREE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("bus/usb/devices")).unwrap();
            Self(path)
        }
        fn dir(&self, path: &str) -> PathBuf {
            let path = self.0.join(path);
            fs::create_dir_all(&path).unwrap();
            path
        }
        fn file(&self, path: &str, value: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        }
        fn link(&self, target: &str, link: &str) {
            let link = self.0.join(link);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(self.0.join(target), link).unwrap();
        }
        fn usb(&self, node: &str, vid: &str, pid: &str) {
            self.file(&format!("devices/{node}/idVendor"), vid);
            self.file(&format!("devices/{node}/idProduct"), pid);
            self.link(
                &format!("devices/{node}"),
                &format!("bus/usb/devices/{node}"),
            );
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn composite_usb_endpoints_group_under_physical_parent_without_serial_ids() {
        let tree = Tree::new();
        tree.usb("1-2", "0e8d\n", "7610\n");
        tree.file("devices/1-2/product", "Test receiver\n");
        tree.file("devices/1-2/serial", "DO_NOT_EXPOSE");
        tree.dir("devices/1-2/1-2:1.0/ttyUSB0/tty/ttyUSB0");
        tree.link(
            "devices/1-2/1-2:1.0/ttyUSB0/tty/ttyUSB0",
            "class/tty/ttyUSB0",
        );
        tree.dir("devices/1-2/1-2:1.1/bluetooth/hci0");
        tree.link("devices/1-2/1-2:1.1/bluetooth/hci0", "class/bluetooth/hci0");
        tree.dir("devices/1-2/1-2:1.2/ieee80211/phy0");
        tree.dir("devices/1-2/1-2:1.2/net/wlan0");
        tree.file("devices/1-2/1-2:1.2/net/wlan0/type", "803\n");
        tree.link(
            "devices/1-2/1-2:1.2/ieee80211/phy0",
            "devices/1-2/1-2:1.2/net/wlan0/phy80211",
        );
        tree.link("devices/1-2/1-2:1.2/net/wlan0", "class/net/wlan0");
        tree.link("devices/1-2/1-2:1.0", "bus/usb/devices/1-2:1.0");
        let (devices, diagnostics) = collect_from(&tree.0);
        assert!(diagnostics.is_empty());
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        assert_eq!(device.id, "linux:usb:1-2");
        assert_eq!(device.name, "Test receiver");
        assert_eq!((device.vid, device.pid), (Some(0x0e8d), Some(0x7610)));
        assert_eq!(device.endpoints.len(), 5);
        assert!(device
            .endpoints
            .iter()
            .any(|e| e.kind == "serial" && e.path == "/dev/ttyUSB0"));
        assert!(device
            .endpoints
            .iter()
            .any(|e| e.kind == "native_ble" && e.path == "hci0"));
        assert!(device
            .endpoints
            .iter()
            .any(|e| e.kind == "linux_wifi" && e.path == "wlan0"));
        assert!(!device.id.contains("DO_NOT_EXPOSE"));
    }

    #[test]
    fn native_interfaces_group_by_phy_and_ethernet_and_console_are_ignored() {
        let tree = Tree::new();
        tree.dir("devices/pci/wifi/ieee80211/phy1");
        for (interface, kind) in [("wlan0", "1"), ("mon0", "803")] {
            tree.dir(&format!("devices/pci/wifi/net/{interface}"));
            tree.file(&format!("devices/pci/wifi/net/{interface}/type"), kind);
            tree.link(
                "devices/pci/wifi/ieee80211/phy1",
                &format!("devices/pci/wifi/net/{interface}/phy80211"),
            );
            tree.link(
                &format!("devices/pci/wifi/net/{interface}"),
                &format!("class/net/{interface}"),
            );
        }
        tree.dir("class/net/eth0");
        tree.dir("class/tty/tty0");
        tree.dir("class/bluetooth/hci0:11");
        tree.dir("devices/platform/ttyAMA0");
        tree.link("devices/platform/ttyAMA0", "class/tty/ttyAMA0");
        tree.dir("devices/platform/hci1");
        tree.link("devices/platform/hci1", "class/bluetooth/hci1");
        let (devices, diagnostics) = collect_from(&tree.0);
        assert!(diagnostics.is_empty());
        assert_eq!(devices.len(), 3);
        let wifi = devices.iter().find(|d| d.id == "linux:wifi:phy1").unwrap();
        assert_eq!(wifi.endpoints.len(), 3);
        assert!(wifi
            .endpoints
            .iter()
            .any(|e| e.kind == "native_wifi" && e.path == "wlan0"));
        assert!(wifi
            .endpoints
            .iter()
            .any(|e| e.kind == "linux_wifi" && e.path == "mon0"));
    }

    #[test]
    fn disappearing_devices_and_bad_attributes_do_not_hide_other_receivers() {
        let tree = Tree::new();
        tree.usb("1-2", "0e8d", "7610");
        tree.usb("1-3", "nothex", "7610");
        tree.usb("1-4", "1546", "01a8");
        tree.file("devices/1-4/product", &"x".repeat(1025));
        tree.link("absent", "class/tty/ttyACM0");
        tree.dir("devices/pci/phy0");
        tree.dir("class/net/wlan0");
        tree.link("devices/pci/phy0", "class/net/wlan0/phy80211");
        let (devices, diagnostics) = collect_from(&tree.0);
        assert_eq!(devices.len(), 3);
        assert!(devices.iter().any(|d| d.name == "USB 1546:01a8"));
        assert!(diagnostics
            .iter()
            .any(|d| d.message.contains("USB identity for 1-3")));
        assert!(diagnostics.iter().any(|d| d.message.contains("ttyACM0")));
        assert!(diagnostics
            .iter()
            .any(|d| d.message.contains("monitor mode")));
    }

    #[test]
    fn missing_optional_classes_and_outside_links_have_explicit_semantics() {
        let tree = Tree::new();
        let (devices, diagnostics) = collect_from(&tree.0);
        assert!(devices.is_empty());
        assert!(diagnostics.is_empty());
        tree.dir("class/tty");
        symlink(std::env::temp_dir(), tree.0.join("class/tty/ttyUSB0")).unwrap();
        let (_, diagnostics) = collect_from(&tree.0);
        assert!(diagnostics
            .iter()
            .any(|d| d.message.contains("outside inventory root")));
        fs::remove_dir_all(tree.0.join("bus/usb/devices")).unwrap();
        let (_, diagnostics) = collect_from(&tree.0);
        assert!(diagnostics
            .iter()
            .any(|d| d.message.contains("Cannot enumerate")));
        let (devices, diagnostics) = collect_from(&tree.0.join("missing"));
        assert!(devices.is_empty());
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn helpers_bound_and_validate_untrusted_registry_content() {
        let tree = Tree::new();
        tree.file("value", "1234\n");
        assert_eq!(hex_attribute(&tree.0.join("value")).unwrap(), 0x1234);
        tree.file("value", &"f".repeat(1025));
        assert!(attribute(&tree.0.join("value")).is_err());
        fs::write(tree.0.join("value"), [0xff]).unwrap();
        assert!(attribute(&tree.0.join("value")).is_err());
        assert!(name(Path::new("bad name")).is_none());
        assert!(name(Path::new("..")).is_none());
        let mut devices = BTreeMap::new();
        attach(
            &mut devices,
            "id".into(),
            "name".into(),
            "serial",
            "/dev/test".into(),
        );
        attach(
            &mut devices,
            "id".into(),
            "name".into(),
            "serial",
            "/dev/test".into(),
        );
        assert_eq!(devices["id"].endpoints.len(), 1);
        let path = tree.dir("too-many");
        for i in 0..=ENTRY_LIMIT {
            fs::write(path.join(i.to_string()), "").unwrap();
        }
        let mut diagnostics = Vec::new();
        assert_eq!(
            entries(&path, false, &mut diagnostics).len(),
            ENTRY_LIMIT,
            "{:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert_eq!(diagnostics.len(), 1);
    }
}
