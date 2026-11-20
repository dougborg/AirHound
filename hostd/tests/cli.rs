#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn help_and_argument_errors_do_not_require_radios() {
    let binary = env!("CARGO_BIN_EXE_airhound-hostd");
    let help = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for flag in [
        "--output",
        "--overwrite",
        "--rssi",
        "--no-ble",
        "--no-wifi",
        "--list-ble-adapters",
        "--ble-adapter",
        "--gpsd",
        "--gpsd-device",
    ] {
        assert!(help.contains(flag));
    }
    let missing_provider = std::process::Command::new(binary)
        .args(["--gpsd-device", "synthetic"])
        .output()
        .unwrap();
    assert!(!missing_provider.status.success());
    assert!(String::from_utf8(missing_provider.stderr)
        .unwrap()
        .contains("--gpsd"));
    let invalid = std::process::Command::new(binary)
        .args(["--rssi", "1"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8(invalid.stderr)
        .unwrap()
        .contains("invalid value"));
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn executable_preserves_existing_output_before_initializing_adapters() {
    let path = std::env::temp_dir().join(format!("airhound-cli-{}.csv", std::process::id()));
    std::fs::write(&path, b"previous session").unwrap();
    for verbosity in [None, Some("-v"), Some("-vv")] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_airhound-hostd"));
        command.args(["--output", path.to_str().unwrap()]);
        if let Some(flag) = verbosity {
            command.arg(flag);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("AlreadyExists"));
        assert_eq!(std::fs::read(&path).unwrap(), b"previous session");
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn invalid_adapter_selection_fails_before_radio_initialization() {
    let binary = env!("CARGO_BIN_EXE_airhound-hostd");
    for args in [
        vec!["--ble-adapter", "not-an-adapter"],
        vec!["--ble-adapter", "hci0", "--no-ble"],
        vec!["--list-ble-adapters", "--output", "ignored.csv"],
    ] {
        let output = std::process::Command::new(binary)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new(binary)
            .args(["--ble-adapter", "hci0"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("system-managed"));
    }
}
