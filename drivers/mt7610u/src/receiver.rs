//! Shared preparation, bounded draining and verified passive shutdown.
use crate::{
    firmware::{self, Firmware},
    radio,
    transport::{RegisterIo, UsbDevice, UsbIo},
};
use anyhow::{ensure, Context, Result};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Historical preparation evidence, separate from the current firmware state.
#[derive(Clone, Copy, Debug)]
pub struct PreparationFailure {
    pub stage: &'static str,
    pub firmware_uploaded: Option<bool>,
}
impl std::fmt::Display for PreparationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "receiver preparation failed during {}", self.stage)
    }
}
impl std::error::Error for PreparationFailure {}

pub fn prepare<T: UsbIo>(
    device: &mut UsbDevice<T>,
    firmware: &Firmware<'_>,
    channel: u8,
    stop: &AtomicBool,
) -> Result<(radio::RadioSetup, bool)> {
    check_cancelled(stop)?;
    let asic = device.read32(0)?;
    ensure!(
        matches!(asic >> 16, 0x7610 | 0x7650),
        "Unexpected ASIC {asic:#010x}"
    );
    let uploaded = firmware::load(device, firmware).context(PreparationFailure {
        stage: "firmware_load",
        firmware_uploaded: None,
    })?;
    check_cancelled(stop)?;
    let setup = radio::initialize_channel(device, channel).context(PreparationFailure {
        stage: "initialize_channel",
        firmware_uploaded: Some(uploaded),
    })?;
    check_cancelled(stop)?;
    radio::retune_with_drain(device, &setup.calibration, channel, drain_packets).context(
        PreparationFailure {
            stage: "initial_retune",
            firmware_uploaded: Some(uploaded),
        },
    )?;
    check_cancelled(stop)?;
    Ok((setup, uploaded))
}

pub fn shutdown(device: &mut impl RegisterIo) -> Result<()> {
    radio::disable(device)?;
    for (reg, mask) in [
        (0x1004, 0xc),
        (0x1404, 1),
        (0x1114, (1 << 20) | (1 << 19)),
        (0x208, 5),
    ] {
        ensure!(
            device.read32(reg)? & mask == 0,
            "receiver cleanup gate {reg:#x} was not closed"
        );
    }
    Ok(())
}

// Drop queued old-channel frames only while the radio has disabled RX. Refuse
// to continue if the endpoint cannot become quiet within a bounded drain.
pub fn drain_packets<T: UsbIo>(device: &mut UsbDevice<T>) -> Result<()> {
    let clock = device.clock();
    let started = clock.now();
    let mut buffer = [0u8; 65536];
    for _ in 0..128 {
        ensure!(
            clock.elapsed(started) < Duration::from_millis(500),
            "RX endpoint did not become quiet before retune"
        );
        if device.read_packets(&mut buffer, Duration::from_millis(20))? == 0 {
            return Ok(());
        }
    }
    anyhow::bail!("RX endpoint exceeded the pre-retune drain budget")
}

pub fn check_cancelled(stop: &AtomicBool) -> Result<()> {
    ensure!(
        !stop.load(Ordering::Relaxed),
        "Operation cancelled before capture; radio cleanup will be attempted"
    );
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::{
        test_support::{image, Endpoint},
        transport::{McuExchangeEvidence, McuTimeout},
    };
    #[test]
    fn preparation_context_retains_actual_first_command_failure() {
        for uploaded in [false, true] {
            let endpoint = Endpoint::default();
            endpoint.0.lock().unwrap().suppress_mcu_replies = true;
            endpoint
                .0
                .lock()
                .unwrap()
                .registers
                .insert(0x730, if uploaded { 0 } else { 1 });
            let bytes = image();
            let mut firmware = Firmware::parse(&bytes).unwrap();
            firmware.sha256 = crate::firmware::REFERENCE_SHA256.into();
            let mut device = endpoint.device();
            let error = prepare(&mut device, &firmware, 1, &AtomicBool::new(false))
                .expect_err("initialization failure");
            let stage = error.downcast_ref::<PreparationFailure>().unwrap();
            assert_eq!(stage.stage, "initialize_channel");
            assert_eq!(stage.firmware_uploaded, Some(uploaded));
            assert!(error.is::<McuTimeout>());
            assert_eq!(
                error.downcast_ref::<McuExchangeEvidence>().unwrap().command,
                12
            );
            shutdown(&mut device).unwrap();
            assert_eq!(device.read32(0x1004).unwrap() & 0xc, 0);
        }
    }
    #[test]
    fn preparation_context_distinguishes_load_and_initial_retune_failures() {
        let bytes = image();
        let mut firmware = Firmware::parse(&bytes).unwrap();
        let endpoint = Endpoint::default();
        let mut device = endpoint.device();
        let error = prepare(&mut device, &firmware, 1, &AtomicBool::new(false))
            .expect_err("untrusted firmware");
        let stage = error.downcast_ref::<PreparationFailure>().unwrap();
        assert_eq!(stage.stage, "firmware_load");
        assert_eq!(stage.firmware_uploaded, None);
        firmware.sha256 = crate::firmware::REFERENCE_SHA256.into();
        endpoint
            .0
            .lock()
            .unwrap()
            .packets
            .push_back(Err(rusb::Error::NoDevice));
        let error = prepare(&mut device, &firmware, 1, &AtomicBool::new(false))
            .expect_err("retune drain disconnected");
        let stage = error.downcast_ref::<PreparationFailure>().unwrap();
        assert_eq!(stage.stage, "initial_retune");
        assert_eq!(stage.firmware_uploaded, Some(false));
        assert_eq!(
            error.downcast_ref::<rusb::Error>(),
            Some(&rusb::Error::NoDevice)
        );
    }
}
