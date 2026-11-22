//! Finite capture orchestration and private recording ownership.
use crate::recording::new_private_file;
use crate::{
    cancellation_flag, check_capture_budget, drain_packets, finish_with_cleanup,
    validate_capture_plan, write_json, write_observation,
};
use anyhow::{ensure, Context, Result};
use mt7610u_lab::{
    firmware::Firmware,
    radio, rx,
    transport::{UsbDevice, UsbIo},
};
use std::{
    collections::{BTreeMap, HashSet},
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub(super) fn capture(
    firmware_path: PathBuf,
    output: PathBuf,
    raw_usb: Option<PathBuf>,
    seconds: u16,
    channels: Vec<u8>,
    dwell_ms: u16,
    selector: Option<mt7610u_lab::selection::Selector>,
) -> Result<()> {
    validate_capture_plan(&channels, raw_usb.is_some())?;
    let bytes = std::fs::read(firmware_path)?;
    let fw = Firmware::parse(&bytes)?;
    fw.require_reference()?;
    let mut output = new_private_file(output)?;
    let mut raw_output = raw_usb.map(new_private_file).transpose()?;
    let stop = cancellation_flag()?;
    let mut device = UsbDevice::open_selected(selector)?;
    let mut summary = capture_device(
        &mut device,
        &fw,
        &mut output,
        raw_output.as_mut().map(|w| w as &mut dyn Write),
        &CapturePlan {
            seconds,
            channels,
            dwell_ms,
        },
        &stop,
    )?;
    summary["usb_device"] = serde_json::to_value(&device.identity)?;
    write_json(&mut std::io::stdout().lock(), &summary)
}

pub(super) struct CapturePlan {
    pub(super) seconds: u16,
    pub(super) channels: Vec<u8>,
    pub(super) dwell_ms: u16,
}

fn capture_device<T: UsbIo>(
    device: &mut UsbDevice<T>,
    fw: &Firmware<'_>,
    output: &mut impl Write,
    mut raw_output: Option<&mut dyn Write>,
    plan: &CapturePlan,
    stop: &AtomicBool,
) -> Result<serde_json::Value> {
    let seconds = plan.seconds;
    let channels = &plan.channels;
    let dwell_ms = plan.dwell_ms;
    let session_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let result = (|| -> Result<serde_json::Value> {
        let (setup, uploaded) = mt7610u_lab::receiver::prepare(device, fw, channels[0], stop)?;
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
                    device,
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
                    write_observation(&mut *output, &value, &mut observation_bytes)?;
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
    let cleanup = mt7610u_lab::receiver::shutdown(device);
    let mut summary =
        finish_with_cleanup(result, cleanup.context("disabling receiver after capture"))?;
    summary["cleanup_verified"] = serde_json::json!(true);
    Ok(summary)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::{beacon_transfer, image, Endpoint};
    fn gates_closed(endpoint: &Endpoint) {
        let m = endpoint.0.lock().unwrap();
        assert_eq!(m.registers.get(&0x1004).unwrap_or(&0) & 0xc, 0);
        assert_eq!(m.registers.get(&0x1404).unwrap_or(&0) & 1, 0);
    }
    #[test]
    fn capture_records_real_decode_and_hops_with_verified_shutdown() {
        for channels in [vec![6], vec![6, 153]] {
            let endpoint = Endpoint::default();
            let image = image();
            let mut fw = Firmware::parse(&image).unwrap();
            fw.sha256 = mt7610u_lab::firmware::REFERENCE_SHA256.into();
            let transfer = beacon_transfer();
            endpoint.0.lock().unwrap().packets.extend([
                Err(rusb::Error::Timeout), // initial pre-retune drain
                Ok(vec![1, 2, 3]),
                Ok(transfer.clone()),
            ]);
            let mut output = vec![];
            let mut raw = vec![];
            let summary = capture_device(
                &mut endpoint.device(),
                &fw,
                &mut output,
                Some(&mut raw),
                &CapturePlan {
                    seconds: 1,
                    channels: channels.clone(),
                    dwell_ms: 20,
                },
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(summary["beacons"], 1);
            assert_eq!(summary["malformed_transfers"], 1);
            assert_eq!(summary["cleanup_verified"], true);
            if channels.len() > 1 {
                assert!(summary["hops"].as_u64().unwrap() > 0);
            }
            let record: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(record["sequence"], 0);
            assert_eq!(record["ssid_bytes"], serde_json::json!([84, 101, 115, 116]));
            assert_eq!(raw.len(), 4 + 3 + 4 + transfer.len());
            gates_closed(&endpoint);
        }
    }
    #[test]
    fn cancellation_quiet_capture_and_output_failure_all_cleanup() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let endpoint = Endpoint::default();
        let image = image();
        let mut fw = Firmware::parse(&image).unwrap();
        fw.sha256 = mt7610u_lab::firmware::REFERENCE_SHA256.into();
        let mut device = endpoint.device();
        let plan = CapturePlan {
            seconds: 0,
            channels: vec![1],
            dwell_ms: 500,
        };
        assert!(capture_device(
            &mut device,
            &fw,
            &mut vec![],
            None,
            &plan,
            &AtomicBool::new(true)
        )
        .is_err());
        gates_closed(&endpoint);
        let err = capture_device(
            &mut device,
            &fw,
            &mut vec![],
            None,
            &plan,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("No valid beacons"));
        gates_closed(&endpoint);
        endpoint
            .0
            .lock()
            .unwrap()
            .packets
            .extend([Err(rusb::Error::Timeout), Ok(beacon_transfer())]);
        assert!(capture_device(
            &mut device,
            &fw,
            &mut Broken,
            None,
            &CapturePlan { seconds: 1, ..plan },
            &AtomicBool::new(false)
        )
        .is_err());
        gates_closed(&endpoint);
    }
}
