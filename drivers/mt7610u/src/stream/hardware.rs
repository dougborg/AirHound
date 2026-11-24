//! USB-backed receiver lifecycle and channel dwell scheduling.
use super::ReceiverHardware;
use anyhow::{ensure, Context, Result};
use mt7610u_lab::{
    firmware::Firmware,
    radio, rx,
    timing::Clock,
    transport::{UsbDevice, UsbIo},
};
use serde_json::{json, Value};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
pub(super) struct Hardware<'a, T: UsbIo, F: FnMut() -> Result<UsbDevice<T>>> {
    open: F,
    firmware: Firmware<'a>,
    device: Option<UsbDevice<T>>,
    calibration: Option<radio::RxCalibration>,
    channels: Vec<u8>,
    dwell: Duration,
    channel_index: usize,
    tuned: Duration,
    clock: Clock,
    epoch: u64,
    buffer: Vec<u8>,
    stop: Arc<AtomicBool>,
    cleanup_unverified: bool,
    identity: Option<mt7610u_lab::selection::Identity>,
    firmware_uploaded: Option<bool>,
    transfers: u64,
    frames: u64,
    beacons: u64,
    malformed: u64,
}
impl<T: UsbIo, F: FnMut() -> Result<UsbDevice<T>>> ReceiverHardware for Hardware<'_, T, F> {
    fn clock(&self) -> Clock {
        self.clock.clone()
    }
    fn metrics(&self) -> Value {
        json!({"usb_device":self.identity,"transfers":self.transfers,"frames":self.frames,"beacons":self.beacons,"malformed_transfers":self.malformed,
            "firmware": {"supplied_image":self.firmware.image_metadata(),
              "load_state":match self.firmware_uploaded {Some(true)=>"uploaded_this_initialization",Some(false)=>"running_image_unverified",None=>"not_initialized"},
              "uploaded_image":self.firmware_uploaded.filter(|uploaded| *uploaded).map(|_| self.firmware.image_metadata()),
              "evidence":"companion_bootstrap"}})
    }
    fn start(&mut self) -> Result<()> {
        self.firmware_uploaded = None;
        self.device = Some((self.open)()?.with_clock(self.clock.clone()));
        self.identity = self.device.as_ref().and_then(|d| d.identity.clone());
        self.cleanup_unverified = true;
        let device = self.device.as_mut().unwrap();
        let (setup, uploaded) =
            mt7610u_lab::receiver::prepare(device, &self.firmware, self.channels[0], &self.stop)?;
        self.firmware_uploaded = Some(uploaded);
        self.calibration = Some(setup.calibration);
        self.channel_index = 0;
        self.epoch = 0;
        self.tuned = self.clock.now();
        Ok(())
    }
    fn shutdown(&mut self) -> Result<()> {
        self.firmware_uploaded = None;
        let Some(mut device) = self.device.take() else {
            ensure!(
                !self.cleanup_unverified,
                "previous device cleanup could not be verified"
            );
            return Ok(());
        };
        self.calibration = None;
        mt7610u_lab::receiver::shutdown(&mut device)?;
        self.cleanup_unverified = false;
        Ok(())
    }
    fn poll(&mut self) -> Result<Vec<Value>> {
        let device = self
            .device
            .as_mut()
            .context("receiver is not initialized")?;
        let calibration = self.calibration.as_ref().context("missing calibration")?;
        if self.channels.len() > 1 && self.clock.elapsed(self.tuned) >= self.dwell {
            self.channel_index = (self.channel_index + 1) % self.channels.len();
            radio::retune_with_drain(
                device,
                calibration,
                self.channels[self.channel_index],
                crate::drain_packets,
            )?;
            self.tuned = self.clock.now();
            self.epoch += 1;
        }
        let timeout = if self.channels.len() > 1 {
            self.dwell
                .saturating_sub(self.clock.elapsed(self.tuned))
                .min(Duration::from_millis(100))
        } else {
            Duration::from_millis(100)
        };
        if timeout < Duration::from_millis(1) {
            return Ok(Vec::new());
        }
        let n = device.read_packets(&mut self.buffer, timeout)?;
        if n == 0 {
            return Ok(Vec::new());
        }
        self.transfers += 1;
        let frames = match rx::parse_usb_transfer(&self.buffer[..n]) {
            Ok(v) => v,
            Err(_) => {
                self.malformed += 1;
                return Ok(Vec::new());
            }
        };
        self.frames += frames.len() as u64;
        let mut records = Vec::new();
        for frame in frames {
            if let Some(beacon) = frame.beacon {
                self.beacons += 1;
                let bssid = beacon
                    .bssid
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(":");
                records.push(json!({"schema_version":1,"type":"beacon","source":"mt7610u-lab","observed_at_unix_ms":self.clock.unix_time()?.as_millis(),"bssid":bssid,"ssid_bytes":beacon.ssid,"advertised_channel":beacon.channel,"receive_channel":self.channels[self.channel_index],"channel_epoch":self.epoch,"dwell_elapsed_ms":self.clock.elapsed(self.tuned).as_millis(),"rssi_dbm":frame.rssi_dbm,"estimated_rssi_dbm":mt7610u_lab::signal::estimate_rssi_for_channel(frame.raw_rssi,frame.rx_info,calibration,self.channels[self.channel_index]),"rssi_estimate_basis":"mt76_eeprom_formula","rssi_validated":false,"raw_rssi":frame.raw_rssi,"full_calibration":false}));
            }
        }
        Ok(records)
    }
}

impl<'a, T: UsbIo, F: FnMut() -> Result<UsbDevice<T>>> Hardware<'a, T, F> {
    pub(super) fn new(
        firmware: Firmware<'a>,
        channels: Vec<u8>,
        dwell: Duration,
        stop: Arc<AtomicBool>,
        clock: Clock,
        open: F,
    ) -> Self {
        Self {
            open,
            firmware,
            device: None,
            calibration: None,
            channels,
            dwell,
            channel_index: 0,
            tuned: Duration::ZERO,
            clock,
            epoch: 0,
            buffer: vec![0; 65536],
            stop,
            cleanup_unverified: false,
            identity: None,
            firmware_uploaded: None,
            transfers: 0,
            frames: 0,
            beacons: 0,
            malformed: 0,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::{beacon_transfer, image, Endpoint};
    use std::sync::atomic::Ordering;
    #[test]
    fn stream_loop_keeps_transport_clock_through_initialization_and_reopen() {
        let endpoint = Endpoint::default();
        let clock = endpoint.clock();
        let bytes = image();
        let mut fw = Firmware::parse(&bytes).unwrap();
        fw.sha256 = mt7610u_lab::firmware::REFERENCE_SHA256.into();
        let stop = Arc::new(AtomicBool::new(false));
        let mut hardware = Hardware::new(
            fw,
            vec![6],
            Duration::from_millis(500),
            stop.clone(),
            clock.clone(),
            || Ok(endpoint.device()),
        );
        hardware.start().unwrap();
        let first = clock.now();
        hardware.shutdown().unwrap();
        endpoint
            .0
            .lock()
            .unwrap()
            .time
            .advance(Duration::from_secs(2));
        hardware.start().unwrap();
        assert!(clock.now() > first + Duration::from_secs(2));
        assert_eq!(hardware.clock.now(), clock.now());
        hardware.shutdown().unwrap();
        let started = clock.now();
        let (_tx, rx) = std::sync::mpsc::sync_channel(1);
        super::super::drive(
            hardware,
            rx,
            stop.clone(),
            Arc::new(std::sync::Mutex::new(super::super::InputOutcome::default())),
            super::super::Output::new(std::io::sink()),
            super::super::LoopLimits {
                duration: Duration::from_secs(1),
                status_interval: Duration::from_secs(5),
            },
        )
        .unwrap();
        assert!(
            (Duration::from_secs(1)..Duration::from_millis(1100)).contains(&clock.elapsed(started))
        );
        let model = endpoint.0.lock().unwrap();
        assert_eq!(model.released, 3);
        assert_eq!(model.registers[&0x1004] & 0xc, 0);
        assert_eq!(model.registers[&0x1404] & 1, 0);
        assert!(!stop.load(Ordering::Relaxed));
    }

    #[test]
    fn hardware_lifecycle_decodes_packets_retunes_and_verifies_cleanup() {
        let endpoint = Endpoint::default();
        let image = image();
        let mut fw = Firmware::parse(&image).unwrap();
        fw.sha256 = mt7610u_lab::firmware::REFERENCE_SHA256.into();
        let stop = Arc::new(AtomicBool::new(false));
        let mut h = Hardware::new(
            fw,
            vec![6, 153],
            Duration::from_millis(500),
            stop.clone(),
            endpoint.clock(),
            || {
                let mut device = endpoint.device();
                device.identity = Some(mt7610u_lab::selection::Identity {
                    selector: "usb:1:2".into(),
                    bus: 1,
                    ports: vec![2],
                    address: 3,
                    identity_scope: "usb_attachment_topology",
                });
                Ok(device)
            },
        );
        assert!(h.poll().is_err());
        assert_eq!(h.metrics()["firmware"]["load_state"], "not_initialized");
        h.shutdown().unwrap();
        endpoint.0.lock().unwrap().registers.insert(0x730, 0);
        h.start().unwrap();
        assert_eq!(
            h.metrics()["firmware"]["load_state"],
            "uploaded_this_initialization"
        );
        assert_eq!(
            h.metrics()["firmware"]["uploaded_image"]["sha256"],
            mt7610u_lab::firmware::REFERENCE_SHA256
        );
        assert!(h.poll().unwrap().is_empty());
        endpoint
            .0
            .lock()
            .unwrap()
            .packets
            .push_back(Ok(vec![1, 2, 3]));
        assert!(h.poll().unwrap().is_empty());
        endpoint
            .0
            .lock()
            .unwrap()
            .packets
            .push_back(Ok(beacon_transfer()));
        let records = h.poll().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["receive_channel"], 6);
        assert_eq!(records[0]["rssi_validated"], false);
        assert_eq!(h.metrics()["malformed_transfers"], 1);
        assert_eq!(h.metrics()["beacons"], 1);
        endpoint
            .0
            .lock()
            .unwrap()
            .time
            .advance(Duration::from_secs(1));
        h.poll().unwrap();
        assert_eq!(h.epoch, 1);
        assert_eq!(h.channel_index, 1);
        h.shutdown().unwrap();
        assert!(!h.cleanup_unverified);
        assert!(h.metrics()["firmware"]["uploaded_image"].is_null());
        h.start().unwrap();
        assert_eq!(
            h.metrics()["firmware"]["load_state"],
            "running_image_unverified"
        );
        assert!(h.metrics()["firmware"]["uploaded_image"].is_null());
        h.shutdown().unwrap();
        assert_eq!(h.metrics()["usb_device"]["selector"], "usb:1:2");
        assert_eq!(endpoint.0.lock().unwrap().registers[&0x1004] & 0xc, 0);
        stop.store(true, Ordering::Relaxed);
        assert!(h.start().is_err());
        h.shutdown().unwrap();
    }
    #[test]
    fn disconnect_never_claims_verified_cleanup_and_reconnect_can_recover() {
        let endpoint = Endpoint::default();
        let image = image();
        let mut fw = Firmware::parse(&image).unwrap();
        fw.sha256 = mt7610u_lab::firmware::REFERENCE_SHA256.into();
        let mut h = Hardware::new(
            fw,
            vec![1],
            Duration::from_millis(500),
            Arc::new(AtomicBool::new(false)),
            endpoint.clock(),
            || Ok(endpoint.device()),
        );
        h.start().unwrap();
        endpoint.0.lock().unwrap().error = Some(rusb::Error::NoDevice);
        assert!(h.poll().is_err());
        assert!(h.shutdown().is_err());
        assert!(h.shutdown().is_err());
        assert!(h.cleanup_unverified);
        endpoint.0.lock().unwrap().error = None;
        h.start().unwrap();
        h.shutdown().unwrap();
        assert!(!h.cleanup_unverified);
    }
}
