#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
use mt7610u_lab::transport;
#[path = "../src/test_support.rs"]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_support;

#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::test_support;
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "airhound-driver-cli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn command() -> Command {
        Command::new(env!("CARGO_BIN_EXE_mt7610u-lab"))
    }
    #[test]
    fn offline_inspection_and_replay_report_only_aggregate_data() {
        let dir = Directory::new();
        let fw = dir.0.join("synthetic.bin");
        fs::write(&fw, test_support::image()).unwrap();
        let output = command().arg("inspect-firmware").arg(&fw).output().unwrap();
        assert!(output.status.success());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["is_reference"], false);
        let packet = test_support::beacon_transfer();
        let raw = dir.0.join("capture.raw");
        let mut bytes = (packet.len() as u32).to_le_bytes().to_vec();
        bytes.extend(packet);
        fs::write(&raw, bytes).unwrap();
        let output = command().arg("replay").arg(&raw).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("02:01"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Test"));
        fs::write(&raw, [1, 2]).unwrap();
        assert!(!command()
            .arg("replay")
            .arg(&raw)
            .output()
            .unwrap()
            .status
            .success());
    }
    #[test]
    fn untrusted_firmware_is_rejected_before_usb_or_capture_creation() {
        let dir = Directory::new();
        let fw = dir.0.join("synthetic.bin");
        fs::write(&fw, test_support::image()).unwrap();
        let target = dir.0.join("must-not-exist.ndjson");
        for subcommand in ["bootstrap", "stream", "capture"] {
            let mut cmd = command();
            cmd.arg(subcommand);
            if subcommand != "bootstrap" {
                cmd.arg("--firmware");
            }
            cmd.arg(&fw);
            if subcommand == "capture" {
                cmd.arg("--output").arg(&target);
            }
            let out = cmd.output().unwrap();
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains("Unvalidated firmware SHA256"));
            assert!(!target.exists());
        }
        assert!(command().arg("--help").output().unwrap().status.success());
        assert!(!command()
            .args(["capture", "--channels", "169"])
            .output()
            .unwrap()
            .status
            .success());
    }
}
