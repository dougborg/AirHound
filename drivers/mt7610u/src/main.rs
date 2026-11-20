#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
mod capture;
use capture::capture;
mod recording;
mod rotation;
mod stream;
use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use mt7610u_lab::receiver::{check_cancelled, drain_packets};
use mt7610u_lab::{
    firmware::{self, Firmware},
    transport::{RegisterIo, UsbDevice},
};
use std::path::PathBuf;
use std::{
    collections::HashSet,
    fs::File,
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[derive(Parser)]
#[command(about = "Experimental MT7610U userspace receive-only driver")]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Bounded live child protocol with pause/resume/stop commands on stdin.
    Stream {
        #[arg(long)]
        firmware: PathBuf,
        #[arg(long, value_delimiter = ',', default_value = "1", value_parser = parse_channel)]
        channels: Vec<u8>,
        #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u16).range(100..=10000))]
        dwell_ms: u16,
        #[arg(long, default_value_t = 32400, value_parser = clap::value_parser!(u32).range(1..=86400))]
        max_seconds: u32,
        /// New private directory for bounded rotating observation recordings.
        #[arg(long)]
        output_dir: Option<PathBuf>,
        #[arg(long, default_value_t=16, value_parser=clap::value_parser!(u32).range(1..=64))]
        rotate_mib: u32,
        #[arg(long, default_value_t=8, value_parser=clap::value_parser!(u16).range(1..=128))]
        keep_files: u16,
    },
    /// Read chip registers and endpoint layout; no register writes.
    Probe,
    /// Validate a firmware file without opening USB.
    InspectFirmware { firmware: PathBuf },
    /// Upload the pinned firmware into volatile MCU RAM; no EEPROM programming.
    Bootstrap { firmware: PathBuf },
    /// Replay a length-prefixed USB recording without accessing hardware.
    Replay { input: PathBuf },
    /// Capture passive 2.4/5GHz beacons; optionally hop supported channels.
    Capture {
        #[arg(long)]
        firmware: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        raw_usb: Option<PathBuf>,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=300))]
        seconds: u16,
        #[arg(long, value_delimiter = ',', default_value = "1", value_parser = parse_channel)]
        channels: Vec<u8>,
        #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u16).range(100..=10000))]
        dwell_ms: u16,
    },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Stream {
            firmware,
            channels,
            dwell_ms,
            max_seconds,
            output_dir,
            rotate_mib,
            keep_files,
        } => {
            let rotation = output_dir
                .map(|path| {
                    rotation::Rotation::new(
                        path,
                        u64::from(rotate_mib) * 1024 * 1024,
                        usize::from(keep_files),
                    )
                })
                .transpose()?;
            stream::run(firmware, channels, dwell_ms, max_seconds, rotation)?;
        }
        Command::InspectFirmware { firmware } => {
            let bytes = std::fs::read(firmware)?;
            let fw = Firmware::parse(&bytes)?;
            write_json(
                &mut std::io::stdout().lock(),
                &serde_json::json!({"ilm_bytes":fw.ilm.len(),"ivb_bytes":fw.ivb.len(),"dlm_bytes":fw.dlm.len(),"version":fw.version,"build_version":fw.build_version,"build_time":fw.build_time,"sha256":fw.sha256,"is_reference":fw.sha256==firmware::REFERENCE_SHA256}),
            )?;
        }
        Command::Replay { input } => {
            let summary = mt7610u_lab::replay::replay(File::open(input)?)?;
            write_json(
                &mut std::io::stdout().lock(),
                &serde_json::to_value(summary)?,
            )?;
        }
        Command::Probe => {
            let mut device = UsbDevice::open()?;
            let asic = device.read32(0x0000)?;
            let mac = device.read32(0x1000)?;
            let mcu = device.read32(0x0730)?;
            let wlan = device.read32(0x0080)?;
            write_json(
                &mut std::io::stdout().lock(),
                &serde_json::json!({"usb_id":"0e8d:7610","asic":format!("{asic:#010x}"),"mac":format!("{mac:#010x}"),"mcu":format!("{mcu:#010x}"),"wlan":format!("{wlan:#010x}"),"bulk_in":device.input_endpoints,"bulk_out":device.output_endpoints}),
            )?;
        }
        Command::Bootstrap { firmware } => {
            let bytes = std::fs::read(firmware).context("reading firmware")?;
            let fw = Firmware::parse(&bytes)?;
            fw.require_reference()?;
            let mut device = UsbDevice::open()?;
            let asic = device.read32(0x0000)?;
            ensure!(
                matches!(asic >> 16, 0x7610 | 0x7650),
                "Unexpected ASIC {asic:#010x}"
            );
            let stop = cancellation_flag()?;
            check_cancelled(&stop)?;
            let result = firmware::load(&mut device, &fw);
            // Even a failed bootstrap must attempt to leave the MAC transmitter off.
            let cleanup = device.rmw32(0x1004, 0xc, 0);
            let uploaded = finish_with_cleanup(
                result,
                cleanup.map(|_| ()).context("disabling MAC after bootstrap"),
            )?;
            write_json(
                &mut std::io::stdout().lock(),
                &serde_json::json!({"firmware_ready":true,"mcu":device.read32(0x0730)?,"mac_system_control":device.read32(0x1004)?,"supplied_firmware_sha256":fw.sha256,"firmware_uploaded":uploaded,"receive_ready":false}),
            )?;
        }
        Command::Capture {
            firmware,
            output,
            raw_usb,
            seconds,
            channels,
            dwell_ms,
        } => capture(firmware, output, raw_usb, seconds, channels, dwell_ms)?,
    }
    Ok(())
}

fn supported_channel(channel: u8) -> bool {
    mt7610u_lab::radio::SUPPORTED_CHANNELS.contains(&channel)
}

fn parse_channel(text: &str) -> std::result::Result<u8, String> {
    let channel = text
        .parse::<u8>()
        .map_err(|_| "Channel must be a number".to_string())?;
    if supported_channel(channel) {
        Ok(channel)
    } else {
        Err(
            "Supported HT20 channels: 1..13, 36..64 step 4, 100..144 step 4, 149..165 step 4"
                .into(),
        )
    }
}

fn write_observation(
    out: &mut impl Write,
    record: &serde_json::Value,
    bytes: &mut u64,
) -> Result<()> {
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    ensure!(
        line.len() <= 16 * 1024,
        "Observation exceeds companion protocol line limit"
    );
    let total = bytes
        .checked_add(line.len() as u64)
        .context("Observation file length overflow")?;
    ensure!(
        total <= 64 * 1024 * 1024,
        "Observation file reached companion protocol 64MiB limit"
    );
    out.write_all(&line)?;
    *bytes = total;
    Ok(())
}

fn validate_capture_plan(channels: &[u8], raw_usb: bool) -> Result<()> {
    ensure!(
        !channels.is_empty() && channels.len() <= mt7610u_lab::radio::SUPPORTED_CHANNELS.len(),
        "Choose one to 38 supported channels"
    );
    ensure!(
        channels.iter().all(|&c| supported_channel(c)),
        "Supported HT20 channels: 1..13, 36..64 step 4, 100..144 step 4, 149..165 step 4"
    );
    ensure!(
        channels.iter().copied().collect::<HashSet<_>>().len() == channels.len(),
        "Duplicate channels would bias dwell scheduling"
    );
    ensure!(!raw_usb || channels.len() == 1, "Raw USB recording currently requires a single channel (the format has no channel metadata)");
    Ok(())
}

fn check_capture_budget(payload_bytes: u64, transfers: u64, recording: bool) -> Result<()> {
    let prefix_bytes = if recording {
        transfers
            .checked_mul(4)
            .context("recording size overflow")?
    } else {
        0
    };
    let total = payload_bytes
        .checked_add(prefix_bytes)
        .context("capture size overflow")?;
    ensure!(
        total <= mt7610u_lab::replay::MAX_RECORDING_BYTES,
        "Capture reached the 64MiB budget (including raw recording prefixes)"
    );
    Ok(())
}

fn write_json(writer: &mut impl Write, value: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn finish_with_cleanup<T>(operation: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (operation, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("Cleanup also failed: {cleanup:#}")))
        }
    }
}

fn cancellation_flag() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    Ok(stop)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn observation_records_are_complete_bounded_lines() {
        let mut out = Vec::new();
        let mut count = 0;
        write_observation(&mut out, &serde_json::json!({"sequence":0}), &mut count).unwrap();
        assert!(out.ends_with(b"\n"));
        assert_eq!(count, out.len() as u64);
        let previous = out.clone();
        let mut count = 64 * 1024 * 1024;
        assert!(write_observation(&mut out, &serde_json::json!({}), &mut count).is_err());
        assert_eq!(out, previous);
        assert!(
            write_observation(&mut out, &serde_json::json!("x".repeat(16 * 1024)), &mut 0).is_err()
        );
        assert_eq!(out, previous);
    }

    #[test]
    fn capture_budget_keeps_raw_output_within_replay_limit() {
        let cap = mt7610u_lab::replay::MAX_RECORDING_BYTES;
        assert!(check_capture_budget(cap - 4, 1, true).is_ok());
        assert!(check_capture_budget(cap - 3, 1, true).is_err());
        assert!(check_capture_budget(cap, 1, false).is_ok());
        assert!(check_capture_budget(0, u64::MAX, true).is_err());
    }

    #[test]
    fn channel_plans_reject_ambiguous_or_unsupported_capture_before_usb() {
        assert!(validate_capture_plan(mt7610u_lab::radio::SUPPORTED_CHANNELS, false).is_ok());
        assert!(validate_capture_plan(&[1, 6, 11], false).is_ok());
        assert!(validate_capture_plan(&[6], true).is_ok());
        assert!(validate_capture_plan(&[1, 36, 40, 44, 48], false).is_ok());
        assert_eq!(parse_channel("36").unwrap(), 36);
        assert!(parse_channel("169").is_err());
        assert!(parse_channel("not-a-channel").is_err());
        for channels in [vec![], vec![0], vec![14], vec![1, 1]] {
            assert!(validate_capture_plan(&channels, false).is_err());
        }
        assert!(validate_capture_plan(&[1, 6, 11], true).is_err());
        assert!(Args::try_parse_from([
            "lab",
            "capture",
            "--firmware",
            "a",
            "--output",
            "b",
            "--channels",
            "14"
        ])
        .is_err());
        assert!(Args::try_parse_from([
            "lab",
            "capture",
            "--firmware",
            "a",
            "--output",
            "b",
            "--dwell-ms",
            "0"
        ])
        .is_err());
    }

    struct BrokenPipe;
    impl Write for BrokenPipe {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn broken_output_returns_error_without_panicking() {
        assert!(write_json(&mut BrokenPipe, &serde_json::json!({"beacons": 1})).is_err());
    }

    #[test]
    fn shutdown_failure_preserves_both_errors_and_prevents_success() {
        let result: Result<()> = finish_with_cleanup(
            Err(anyhow::anyhow!("capture failed")),
            Err(anyhow::anyhow!("USB shutdown failed")),
        );
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("capture failed"));
        assert!(message.contains("USB shutdown failed"));
        assert!(finish_with_cleanup(Ok(1), Err(anyhow::anyhow!("shutdown failed"))).is_err());
        assert_eq!(finish_with_cleanup(Ok(1), Ok(())).unwrap(), 1);
    }

    #[test]
    fn pending_cancellation_stops_next_initialization_stage() {
        assert!(check_cancelled(&AtomicBool::new(true)).is_err());
        assert!(check_cancelled(&AtomicBool::new(false)).is_ok());
    }
}

#[cfg(test)]
use mt7610u_lab::transport;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_support;
