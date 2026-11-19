use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use mt7610u_lab::{
    firmware::{self, Firmware},
    transport::{RegisterIo, UsbDevice},
};
use mt7610u_lab::{radio, rx};
use std::path::PathBuf;
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
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
    /// Attempt passive channel-one reception; writes beacon NDJSON locally.
    Capture {
        #[arg(long)]
        firmware: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        raw_usb: Option<PathBuf>,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=300))]
        seconds: u16,
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
        } => capture(firmware, output, raw_usb, seconds)?,
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
) -> Result<()> {
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
            radio::initialize_channel_one(&mut device).context("channel-one initialization")?;
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
        while started.elapsed() < Duration::from_secs(seconds as u64)
            && !stop.load(Ordering::Relaxed)
        {
            let n = device.read_packets(&mut buffer, Duration::from_millis(200))?;
            if n == 0 {
                continue;
            }
            transfers += 1;
            total_bytes += n as u64;
            ensure!(
                total_bytes <= 64 * 1024 * 1024,
                "Capture reached the 64MiB transfer budget"
            );
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
                    bssids.insert(beacon.bssid);
                    let bssid = beacon
                        .bssid
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(":");
                    let value = serde_json::json!({"type":"beacon","elapsed_ms":started.elapsed().as_millis(),"bssid":bssid,"ssid_bytes":beacon.ssid,"advertised_channel":beacon.channel,"receive_channel":setup.channel,"rssi_dbm":frame.rssi_dbm,"raw_rssi":frame.raw_rssi,"full_calibration":false});
                    serde_json::to_writer(&mut output, &value)?;
                    output.write_all(b"\n")?;
                }
            }
        }
        output.flush()?;
        if let Some(out) = &mut raw_output {
            out.flush()?;
        }
        ensure!(
            beacons > 0,
            "No valid beacons received; radio capture has NOT been validated"
        );
        Ok(
            serde_json::json!({"duration_s":started.elapsed().as_secs_f64(),"transfers":transfers,"bytes":total_bytes,"frames":frames,"beacons":beacons,"distinct_bssids":bssids.len(),"malformed_transfers":malformed,"receive_channel":1,"interrupted":stop.load(Ordering::Relaxed),"full_calibration":false,"supplied_firmware_sha256":fw.sha256,"firmware_uploaded":uploaded}),
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
