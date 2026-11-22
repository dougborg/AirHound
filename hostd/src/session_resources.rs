//! Physical Linux radio identity checks before any source starts.
use airhound_hostd::session::SessionConfig;
#[cfg(any(target_os = "linux", test))]
use airhound_hostd::session::Source;
use std::io;
#[cfg(any(target_os = "linux", test))]
use std::{collections::HashMap, path::Path};

pub(crate) fn validate(plan: &SessionConfig) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        validate_linux(plan, Path::new("/sys/class/net"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = plan;
        Ok(())
    }
}

#[cfg(any(target_os = "linux", test))]
fn validate_linux(plan: &SessionConfig, net_root: &Path) -> io::Result<()> {
    let mut radios = HashMap::new();
    for receiver in &plan.receivers {
        if let Source::LinuxWifi { interface } = &receiver.source {
            // SessionConfig::resolve rejects path separators and invalid interface
            // names. Keep this guard so this preflight is safe as a standalone call.
            if interface.is_empty()
                || interface.len() > 15
                || interface == "."
                || interface == ".."
                || !interface
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid Linux WiFi interface name",
                ));
            }
            let phy = std::fs::canonicalize(net_root.join(interface).join("phy80211")).map_err(
                |error| {
                    io::Error::new(
                        error.kind(),
                        format!("cannot establish physical WiFi radio for {interface}: {error}"),
                    )
                },
            )?;
            if let Some(other) = radios.insert(phy, &receiver.id) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "receivers {other} and {} select interfaces on the same WiFi radio",
                        receiver.id
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use airhound_hostd::session::ReceiverConfig;

    fn plan(names: &[&str]) -> SessionConfig {
        SessionConfig {
            version: 1,
            alert_policy: None,
            location: None,
            receivers: names
                .iter()
                .enumerate()
                .map(|(i, name)| ReceiverConfig {
                    id: format!("radio-{i}"),
                    source: Source::LinuxWifi {
                        interface: (*name).into(),
                    },
                })
                .collect(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn physical_radio_identity_rejects_multiple_interfaces_on_one_phy() {
        let root = std::env::temp_dir().join(format!(
            "airhound-session-phy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("phy0")).unwrap();
        std::fs::create_dir_all(root.join("phy1")).unwrap();
        for (interface, phy) in [("mon0", "phy0"), ("mon1", "phy0"), ("mon2", "phy1")] {
            std::fs::create_dir_all(root.join("net").join(interface)).unwrap();
            std::os::unix::fs::symlink(
                root.join(phy),
                root.join("net").join(interface).join("phy80211"),
            )
            .unwrap();
        }
        let net = root.join("net");
        assert!(validate_linux(&plan(&["mon0"]), &net).is_ok());
        assert!(validate_linux(&plan(&["mon0", "mon2"]), &net).is_ok());
        let shared = validate_linux(&plan(&["mon0", "mon1"]), &net).unwrap_err();
        assert_eq!(shared.kind(), io::ErrorKind::InvalidInput);
        assert!(shared.to_string().contains("radio-0 and radio-1"));
        assert_eq!(
            validate_linux(&plan(&["missing"]), &net)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preflight_rejects_unsafe_interface_paths_and_ignores_non_wifi_sources() {
        for name in [
            "",
            ".",
            "..",
            "../mon0",
            "/dev/mon0",
            "mon 0",
            "1234567890123456",
        ] {
            assert_eq!(
                validate_linux(&plan(&[name]), Path::new("/nonexistent"))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        let config = SessionConfig {
            version: 1,
            alert_policy: None,
            location: None,
            receivers: vec![ReceiverConfig {
                id: "ble".into(),
                source: Source::NativeBle { adapter: None },
            }],
        };
        assert!(validate_linux(&config, Path::new("/nonexistent")).is_ok());
        assert!(validate(&config).is_ok());
    }
}
