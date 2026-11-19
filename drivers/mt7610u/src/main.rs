use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use mt7610u_lab::{
    firmware::{self, Firmware},
    transport::{RegisterIo, UsbDevice},
};
use mt7610u_lab::{radio, rx};
use std::path::PathBuf;
use std::{
    collections::{BTreeMap, HashSet},
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(about = "Experimental MT7610U userspace receive-only driver")]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
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

fn new_private_file(path: PathBuf) -> Result<BufWriter<File>> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(BufWriter::new(options.open(&path).with_context(|| {
        format!("creating {} (never overwrite)", path.display())
    })?))
}

fn capture(
    firmware_path: PathBuf,
    output: PathBuf,
    raw_usb: Option<PathBuf>,
    seconds: u16,
    channels: Vec<u8>,
    dwell_ms: u16,
) -> Result<()> {
    validate_capture_plan(&channels, raw_usb.is_some())?;
    let session_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let bytes = std::fs::read(firmware_path)?;
    let fw = Firmware::parse(&bytes)?;
    fw.require_reference()?;
    let mut output = new_private_file(output)?;
    let mut raw_output = raw_usb.map(new_private_file).transpose()?;
    let stop = cancellation_flag()?;
    let mut device = UsbDevice::open()?;
    let asic = device.read32(0)?;
    ensure!(
        matches!(asic >> 16, 0x7610 | 0x7650),
        "Unexpected ASIC {asic:#010x}"
    );
    let result = (|| -> Result<serde_json::Value> {
        check_cancelled(&stop)?;
        let uploaded = firmware::load(&mut device, &fw).context("firmware bootstrap")?;
        check_cancelled(&stop)?;
        let setup =
            radio::initialize_channel(&mut device, channels[0]).context("initial channel setup")?;
        radio::retune_with_drain(&mut device, &setup.calibration, channels[0], drain_packets)?;
        check_cancelled(&stop)?;
        writeln!(
            std::io::stderr().lock(),
            "Channel {} configured; omitted calibrations: {}",
            setup.channel,
            setup.omitted_calibrations
        )?;
        let started = Instant::now();
        let mut buffer = vec![0u8; 65536];
        let (mut transfers, mut frames, mut beacons, mut malformed, mut total_bytes) =
            (0u64, 0u64, 0u64, 0u64, 0u64);
        let mut bssids = HashSet::new();
        let mut observation_bytes = 0u64;
        let mut channel_index = 0usize;
        let mut tuned_at = Instant::now();
        let dwell = Duration::from_millis(dwell_ms as u64);
        let mut channel_beacons: BTreeMap<u8, u64> = channels.iter().map(|&c| (c, 0)).collect();
        let mut channel_listen_ms: BTreeMap<u8, u128> = channels.iter().map(|&c| (c, 0)).collect();
        let mut hops = 0u64;
        let mut retune_ms = 0u128;
        while started.elapsed() < Duration::from_secs(seconds as u64)
            && !stop.load(Ordering::Relaxed)
        {
            if channels.len() > 1 && tuned_at.elapsed() >= dwell {
                *channel_listen_ms.get_mut(&channels[channel_index]).unwrap() +=
                    tuned_at.elapsed().as_millis();
                channel_index = (channel_index + 1) % channels.len();
                let retune_started = Instant::now();
                radio::retune_with_drain(
                    &mut device,
                    &setup.calibration,
                    channels[channel_index],
                    drain_packets,
                )?;
                retune_ms += retune_started.elapsed().as_millis();
                hops += 1;
                tuned_at = Instant::now();
                continue;
            }
            let remaining = Duration::from_secs(seconds as u64).saturating_sub(started.elapsed());
            let channel_remaining = if channels.len() > 1 {
                dwell.saturating_sub(tuned_at.elapsed())
            } else {
                remaining
            };
            let timeout = remaining
                .min(channel_remaining)
                .min(Duration::from_millis(200));
            if timeout < Duration::from_millis(1) {
                continue;
            }
            let n = device.read_packets(&mut buffer, timeout)?;
            if n == 0 {
                continue;
            }
            transfers += 1;
            total_bytes += n as u64;
            check_capture_budget(total_bytes, transfers, raw_output.is_some())?;
            if let Some(out) = &mut raw_output {
                out.write_all(&(n as u32).to_le_bytes())?;
                out.write_all(&buffer[..n])?;
            }
            let decoded = match rx::parse_usb_transfer(&buffer[..n]) {
                Ok(v) => v,
                Err(error) => {
                    malformed += 1;
                    if malformed <= 3 {
                        writeln!(std::io::stderr().lock(), "RX decode: {error}")?;
                    }
                    continue;
                }
            };
            frames += decoded.len() as u64;
            for frame in decoded {
                if let Some(beacon) = frame.beacon {
                    beacons += 1;
                    *channel_beacons.get_mut(&channels[channel_index]).unwrap() += 1;
                    bssids.insert(beacon.bssid);
                    let bssid = beacon
                        .bssid
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(":");
                    let value = serde_json::json!({"schema_version":1,"type":"beacon","source":"mt7610u-lab","session_id":session_id,"sequence":beacons-1,"observed_at_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),"elapsed_ms":started.elapsed().as_millis(),"bssid":bssid,"ssid_bytes":beacon.ssid,"advertised_channel":beacon.channel,"receive_channel":channels[channel_index],"channel_epoch":hops,"dwell_elapsed_ms":tuned_at.elapsed().as_millis(),"rssi_dbm":frame.rssi_dbm,"estimated_rssi_dbm":mt7610u_lab::signal::estimate_rssi_for_channel(frame.raw_rssi,frame.rx_info,&setup.calibration,channels[channel_index]),"rssi_estimate_basis":"mt76_eeprom_formula","rssi_validated":false,"raw_rssi":frame.raw_rssi,"full_calibration":false});
                    write_observation(&mut output, &value, &mut observation_bytes)?;
                }
            }
        }
        *channel_listen_ms.get_mut(&channels[channel_index]).unwrap() +=
            tuned_at.elapsed().as_millis();
        output.flush()?;
        if let Some(out) = &mut raw_output {
            out.flush()?;
        }
        ensure!(
            beacons > 0,
            "No valid beacons received; radio capture has NOT been validated"
        );
        Ok(
            serde_json::json!({"duration_s":started.elapsed().as_secs_f64(),"transfers":transfers,"bytes":total_bytes,"frames":frames,"beacons":beacons,"distinct_bssids":bssids.len(),"malformed_transfers":malformed,"channels":channels,"dwell_ms":dwell_ms,"hops":hops,"retune_ms":retune_ms,"channel_beacons":channel_beacons,"channel_listen_ms":channel_listen_ms,"interrupted":stop.load(Ordering::Relaxed),"full_calibration":false,"supplied_firmware_sha256":fw.sha256,"firmware_uploaded":uploaded}),
        )
    })();
    let cleanup = radio::disable(&mut device).and_then(|()| {
        ensure!(
            device.read32(0x1004)? & 0xc == 0,
            "MAC TX/RX remained enabled after cleanup"
        );
        ensure!(
            device.read32(0x1404)? & 1 == 0,
            "Automatic ACK remained enabled after cleanup"
        );
        ensure!(
            device.read32(0x1114)? & (1 << 20) == 0,
            "Beacon TX remained enabled after cleanup"
        );
        Ok(())
    });
    let mut summary =
        finish_with_cleanup(result, cleanup.context("disabling receiver after capture"))?;
    summary["cleanup_verified"] = serde_json::json!(true);
    write_json(&mut std::io::stdout().lock(), &summary)
}

fn supported_channel(channel: u8) -> bool {
    matches!(channel, 1..=11 | 36 | 40 | 44 | 48)
}

fn parse_channel(text: &str) -> std::result::Result<u8, String> {
    let channel = text
        .parse::<u8>()
        .map_err(|_| "Channel must be a number".to_string())?;
    if supported_channel(channel) {
        Ok(channel)
    } else {
        Err("Supported channels: 1..11, 36, 40, 44, 48".into())
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
        !channels.is_empty() && channels.len() <= 15,
        "Choose one to fifteen supported channels"
    );
    ensure!(
        channels.iter().all(|&c| supported_channel(c)),
        "Supported channels: 1..11, 36, 40, 44, 48"
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

// Drop queued old-channel frames only while the radio has disabled RX. Refuse
// to continue if the endpoint cannot become quiet within a bounded drain.
fn drain_packets(device: &mut UsbDevice) -> Result<()> {
    let started = Instant::now();
    let mut buffer = [0u8; 65536];
    for _ in 0..128 {
        ensure!(
            started.elapsed() < Duration::from_millis(500),
            "RX endpoint did not become quiet before retune"
        );
        if device.read_packets(&mut buffer, Duration::from_millis(20))? == 0 {
            return Ok(());
        }
    }
    anyhow::bail!("RX endpoint exceeded the pre-retune drain budget")
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

fn check_cancelled(stop: &AtomicBool) -> Result<()> {
    ensure!(
        !stop.load(Ordering::Relaxed),
        "Operation cancelled before capture; radio cleanup will be attempted"
    );
    Ok(())
}

#[cfg(test)]
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
        assert!(validate_capture_plan(&[1, 6, 11], false).is_ok());
        assert!(validate_capture_plan(&[6], true).is_ok());
        assert!(validate_capture_plan(&[1, 36, 40, 44, 48], false).is_ok());
        assert_eq!(parse_channel("36").unwrap(), 36);
        assert!(parse_channel("52").is_err());
        assert!(parse_channel("not-a-channel").is_err());
        for channels in [vec![], vec![0], vec![12], vec![1, 1]] {
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
            "12"
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
