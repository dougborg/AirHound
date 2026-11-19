// SPDX-License-Identifier: GPL-2.0-only
// Adapted from mt76x0/usb_mcu.c and mt76x02_usb_mcu.c (BSD-3-Clause-Clear),
// Copyright (C) 2018 Lorenzo Bianconi; chip power sequence from mt76x0/init.c
// (GPL-2.0-only), Ralink Technology 2002-2010, Felix Fietkau 2014,
// Jakub Kicinski 2015, Stanislaw Gruszka 2018. See THIRD_PARTY_NOTICES.
use crate::transport::{RegisterIo, UsbDevice};
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    thread::sleep,
    time::{Duration, Instant},
};

pub const REFERENCE_SHA256: &str =
    "538d08c08f8fb5aa05cacf34e49aeb307e8b2dc094407fc655fbf3580c893953";
const MAX_CHUNK: usize = 0x38f0;

pub struct Firmware<'a> {
    pub ivb: &'a [u8],
    pub ilm: &'a [u8],
    pub dlm: &'a [u8],
    pub build_version: u16,
    pub version: u16,
    pub build_time: String,
    pub sha256: String,
}

impl<'a> Firmware<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        ensure!(bytes.len() >= 32, "Firmware header shorter than 32 bytes");
        let ilm_len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let dlm_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        ensure!(ilm_len > 64, "ILM must include a 64-byte IVB and code");
        let end_ilm = 32usize
            .checked_add(ilm_len)
            .context("ILM length overflow")?;
        let total = end_ilm
            .checked_add(dlm_len)
            .context("DLM length overflow")?;
        ensure!(
            total == bytes.len(),
            "Firmware lengths require {total} bytes, got {}",
            bytes.len()
        );
        // Prevent writes outside the MT7610 memory regions before touching hardware.
        ensure!(
            ilm_len < 0x80000 && dlm_len <= 0x80000,
            "Firmware memory ranges exceed supported MT7610 layout"
        );
        Ok(Self {
            ivb: &bytes[32..96],
            ilm: &bytes[96..end_ilm],
            dlm: &bytes[end_ilm..],
            build_version: u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
            version: u16::from_le_bytes(bytes[10..12].try_into().unwrap()),
            build_time: String::from_utf8_lossy(&bytes[16..32]).into_owned(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }

    pub fn require_reference(&self) -> Result<()> {
        ensure!(self.sha256 == REFERENCE_SHA256, "Unvalidated firmware SHA256 {}; this experiment accepts only the pinned reference image", self.sha256);
        Ok(())
    }
}

pub fn chunk_packet(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_CHUNK,
        "Invalid firmware chunk size"
    );
    let aligned = (bytes.len() + 3) & !3;
    let mut packet = vec![0; aligned + 8];
    packet[..4].copy_from_slice(&(0x50000000 | bytes.len() as u32).to_le_bytes());
    packet[4..4 + bytes.len()].copy_from_slice(bytes);
    Ok(packet)
}

pub fn poll(
    io: &mut impl RegisterIo,
    reg: u32,
    mask: u32,
    expected: u32,
    timeout: Duration,
) -> Result<u32> {
    let until = Instant::now() + timeout;
    loop {
        let value = io.read32(reg)?;
        if value & mask == expected {
            return Ok(value);
        }
        if Instant::now() >= until {
            bail!("Register {reg:#x} timed out: {value:#010x}, expected {expected:#x} under mask {mask:#x}");
        }
        sleep(Duration::from_millis(5));
    }
}

pub fn enable_chip(io: &mut impl RegisterIo) -> Result<()> {
    let mut value = io.read32(0x0080)?;
    // Disable WLAN but retain its clock, as required for subsequent USB reads.
    io.write32(0x0080, value & !1)?;
    sleep(Duration::from_micros(20));
    value = io.read32(0x0080)?;
    value = (value | 0xff000000) & !0x20;
    if value & 1 != 0 {
        io.write32(0x0080, value | 0xc)?;
        sleep(Duration::from_micros(20));
        value &= !0xc;
    }
    io.write32(0x0080, value | 3)?;
    poll(io, 0x0020, 0x00c00000, 0x00c00000, Duration::from_secs(2))?;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let mac = io.read32(0x1000)?;
        if mac != 0 && mac != u32::MAX {
            break;
        }
        ensure!(Instant::now() < until, "MAC did not become accessible");
        sleep(Duration::from_millis(5));
    }
    // MAC transmit disabled; USB MCU OUT transfers are not WLAN frames.
    io.rmw32(0x1004, 0xc, 0)?;
    Ok(())
}

/// Returns true when this invocation uploaded the supplied image. A ready MCU
/// is reused without making any claim about the identity of its running image.
pub fn load(device: &mut UsbDevice, firmware: &Firmware<'_>) -> Result<bool> {
    firmware.require_reference()?;
    enable_chip(device)?;
    device.write32(0x0238, 0x00c00000)?;
    if device.read32(0x0730)? == 1 {
        return Ok(false);
    }
    // Upstream uses 0x2c. Deliberately leave MAC TX (bit2) disabled here.
    device.write32(0x1004, 0x28)?;
    device.rmw32(0x0238, 0, 0x00c00020)?;
    device.control_out(0x01, 1, 0, &[])?;
    sleep(Duration::from_millis(6));
    for (reg, value) in [
        (0x0800, 1),
        (0x09a0, 0x00400230),
        (0x09a4, 1),
        (0x09c4, 0x44),
        (0x0a6c, 3),
    ] {
        device.write32(reg, value)?;
    }
    device.rmw32(0x0238, 0, 1 << 16)?;
    device.rmw32(0x0238, 1 << 16, 0)?;
    for (data, base) in [(firmware.ilm, 0x40u32), (firmware.dlm, 0x80000u32)] {
        for (index, chunk) in data.chunks(MAX_CHUNK).enumerate() {
            let offset = base + (index * MAX_CHUNK) as u32;
            device.fce_write(0x0230, offset)?;
            device.fce_write(0x0234, (((chunk.len() + 3) & !3) as u32) << 16)?;
            device.write_mcu_bulk(&chunk_packet(chunk)?)?;
            let next = device.read32(0x09a8)?.wrapping_add(1);
            device.write32(0x09a8, next)?;
            sleep(Duration::from_millis(6));
        }
    }
    device.control_out(0x01, 0x12, 0, firmware.ivb)?;
    poll(device, 0x0730, 1, 1, Duration::from_secs(1))?;
    device.write32(0x0800, 1)?;
    device.rmw32(0x1004, 0xc, 0)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Vec<u8> {
        let mut image = vec![0; 32 + 68 + 4];
        image[..4].copy_from_slice(&68u32.to_le_bytes());
        image[4..8].copy_from_slice(&4u32.to_le_bytes());
        image[32..96].fill(0x11);
        image[96..100].fill(0x22);
        image[100..].fill(0x33);
        image
    }
    #[test]
    fn rejects_bad_firmware_before_device_access() {
        for n in 0..32 {
            assert!(Firmware::parse(&vec![0; n]).is_err());
        }
        let image = sample();
        let fw = Firmware::parse(&image).unwrap();
        assert_eq!(fw.ivb, &[0x11; 64]);
        assert_eq!(fw.ilm, &[0x22; 4]);
        assert_eq!(fw.dlm, &[0x33; 4]);
        assert!(fw.require_reference().is_err());
        assert!(Firmware::parse(&image[..image.len() - 1]).is_err());
        let mut long = image.clone();
        long.push(0);
        assert!(Firmware::parse(&long).is_err());
        let mut invalid = image;
        invalid[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Firmware::parse(&invalid).is_err());
    }
    #[test]
    fn firmware_packet_uses_unrounded_length_and_zero_tail() {
        let packet = chunk_packet(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(&packet[..4], &0x50000005u32.to_le_bytes());
        assert_eq!(&packet[4..9], &[1, 2, 3, 4, 5]);
        assert_eq!(&packet[9..], &[0; 7]);
        assert!(chunk_packet(&[]).is_err());
        assert!(chunk_packet(&vec![0; MAX_CHUNK + 1]).is_err());
    }
}
