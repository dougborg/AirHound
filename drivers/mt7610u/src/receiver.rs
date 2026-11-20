//! Shared preparation, bounded draining and verified passive shutdown.
use crate::{
    firmware::{self, Firmware},
    radio,
    transport::{RegisterIo, UsbDevice, UsbIo},
};
use anyhow::{ensure, Result};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

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
    let uploaded = firmware::load(device, firmware)?;
    check_cancelled(stop)?;
    let setup = radio::initialize_channel(device, channel)?;
    check_cancelled(stop)?;
    radio::retune_with_drain(device, &setup.calibration, channel, drain_packets)?;
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

pub fn check_cancelled(stop: &AtomicBool) -> Result<()> {
    ensure!(
        !stop.load(Ordering::Relaxed),
        "Operation cancelled before capture; radio cleanup will be attempted"
    );
    Ok(())
}
