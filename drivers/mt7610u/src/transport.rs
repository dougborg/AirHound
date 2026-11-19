// SPDX-License-Identifier: GPL-2.0-only
// USB protocol adapted from mt76 usb.c / mt76x02_usb_mcu.c / mt76x02_usb_core.c,
// Copyright (C) 2018 Lorenzo Bianconi. Those files are BSD-3-Clause-Clear;
// see THIRD_PARTY_NOTICES for retained terms and the pinned source revision.
use anyhow::{bail, ensure, Context, Result};
use rusb::{Context as UsbContext, DeviceHandle, Direction, TransferType, UsbContext as _};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_millis(300);

fn retry_control<T>(mut operation: impl FnMut() -> rusb::Result<T>) -> rusb::Result<T> {
    for attempt in 0..10 {
        match operation() {
            Err(rusb::Error::Timeout) if attempt < 9 => {
                std::thread::sleep(Duration::from_millis(6))
            }
            result => return result,
        }
    }
    unreachable!("last attempt always returns")
}

pub trait RegisterIo {
    fn read32(&mut self, reg: u32) -> Result<u32>;
    fn write32(&mut self, reg: u32, value: u32) -> Result<()>;
    fn rmw32(&mut self, reg: u32, mask: u32, value: u32) -> Result<u32> {
        let next = (self.read32(reg)? & !mask) | value;
        self.write32(reg, next)?;
        Ok(next)
    }
    fn mcu_command(&mut self, _command: u8, _payload: &[u8], _wait: bool) -> Result<()> {
        bail!("MCU commands unavailable on this transport")
    }
    fn mcu_query(&mut self, _command: u8, _payload: &[u8]) -> Result<Vec<u8>> {
        bail!("MCU queries unavailable on this transport")
    }
}

pub struct UsbDevice {
    handle: DeviceHandle<UsbContext>,
    pub input_endpoints: Vec<u8>,
    pub output_endpoints: Vec<u8>,
    sequence: u8,
}

impl UsbDevice {
    pub fn open() -> Result<Self> {
        let context = UsbContext::new()?;
        let devices = context.devices()?;
        let matching: Vec<_> = devices
            .iter()
            .filter(|d| {
                d.device_descriptor()
                    .is_ok_and(|v| v.vendor_id() == 0x0e8d && v.product_id() == 0x7610)
            })
            .collect();
        ensure!(
            matching.len() == 1,
            "Expected exactly one USB 0e8d:7610, found {}",
            matching.len()
        );
        let device = &matching[0];
        let handle = device
            .open()
            .context("opening MT7610U (no driver detachment attempted)")?;
        ensure!(
            handle.active_configuration()? == 1,
            "Expected active configuration 1; refusing to change it implicitly"
        );
        let config = device.active_config_descriptor()?;
        let descriptor = config
            .interfaces()
            .find(|i| i.number() == 0)
            .and_then(|i| i.descriptors().find(|d| d.setting_number() == 0))
            .context("missing interface 0 alternate 0")?;
        let mut input_endpoints = Vec::new();
        let mut output_endpoints = Vec::new();
        for ep in descriptor.endpoint_descriptors() {
            if ep.transfer_type() != TransferType::Bulk {
                continue;
            }
            match ep.direction() {
                Direction::In => input_endpoints.push(ep.address()),
                Direction::Out => output_endpoints.push(ep.address()),
            }
        }
        ensure!(input_endpoints.len() == 2 && output_endpoints.len() == 6,
            "Unexpected MT7610U bulk endpoint layout: in={input_endpoints:x?}, out={output_endpoints:x?}");
        handle.claim_interface(0).context("claiming interface 0")?;
        Ok(Self {
            handle,
            input_endpoints,
            output_endpoints,
            sequence: 0,
        })
    }

    pub fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        let write = || {
            self.handle
                .write_control(0x40, request, value, index, data, TIMEOUT)
        };
        let count = (if matches!(request, 0x06 | 0x42) {
            retry_control(write)
        } else {
            write()
        })
        .with_context(|| format!("USB control OUT request={request:#x} index={index:#x}"))?;
        ensure!(
            count == data.len(),
            "Short control write {count}/{}",
            data.len()
        );
        Ok(())
    }

    pub fn fce_write(&mut self, offset: u16, value: u32) -> Result<()> {
        self.control_out(0x42, value as u16, offset, &[])?;
        self.control_out(
            0x42,
            (value >> 16) as u16,
            offset.checked_add(2).context("FCE address overflow")?,
            &[],
        )
    }

    // Expose only the MCU endpoint, never the WLAN transmit endpoints.
    pub fn write_mcu_bulk(&mut self, bytes: &[u8]) -> Result<()> {
        let count =
            self.handle
                .write_bulk(self.output_endpoints[0], bytes, Duration::from_secs(1))?;
        ensure!(
            count == bytes.len(),
            "Short MCU bulk write {count}/{}",
            bytes.len()
        );
        Ok(())
    }

    pub fn read_packets(&mut self, bytes: &mut [u8], timeout: Duration) -> Result<usize> {
        match self
            .handle
            .read_bulk(self.input_endpoints[0], bytes, timeout)
        {
            Ok(n) => Ok(n),
            Err(rusb::Error::Timeout) => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    fn exchange(&mut self, command: u8, payload: &[u8], wait: bool) -> Result<Vec<u8>> {
        ensure!(command < 128, "MCU command out of range");
        let seq = if wait {
            self.sequence = self.sequence % 15 + 1;
            self.sequence
        } else {
            0
        };
        let packet = mcu_packet(command, seq, payload)?;
        self.write_mcu_bulk(&packet)?;
        if !wait {
            return Ok(Vec::new());
        }
        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut reply = [0u8; 1024];
        while Instant::now() < deadline {
            let n = match self.handle.read_bulk(
                self.input_endpoints[1],
                &mut reply,
                Duration::from_millis(300),
            ) {
                Ok(n) => n,
                Err(rusb::Error::Timeout) => continue,
                Err(e) => return Err(e.into()),
            };
            ensure!(n >= 8, "Short MCU reply ({n} bytes)");
            let header = u32::from_le_bytes(reply[..4].try_into().unwrap());
            let got_seq = ((header >> 16) & 15) as u8;
            let event = (header >> 20) & 15;
            if got_seq != seq {
                continue;
            }
            ensure!(
                event == 0,
                "MCU command {command} failed with event {event}"
            );
            return Ok(reply[4..n - 4].to_vec());
        }
        bail!("MCU command {command} sequence {seq} timed out")
    }
}

impl RegisterIo for UsbDevice {
    fn read32(&mut self, reg: u32) -> Result<u32> {
        ensure!(
            reg & 0xc0000000 == 0,
            "Unsupported special register namespace {reg:#x}"
        );
        let mut data = [0u8; 4];
        let count = retry_control(|| {
            self.handle.read_control(
                0xc0,
                0x07,
                (reg >> 16) as u16,
                reg as u16,
                &mut data,
                TIMEOUT,
            )
        })
        .with_context(|| format!("register read {reg:#x}"))?;
        ensure!(count == 4, "Short register read {reg:#x}: {count} bytes");
        Ok(u32::from_le_bytes(data))
    }
    fn write32(&mut self, reg: u32, value: u32) -> Result<()> {
        ensure!(
            reg & 0xc0000000 == 0,
            "Unsupported special register namespace {reg:#x}"
        );
        self.control_out(0x06, (reg >> 16) as u16, reg as u16, &value.to_le_bytes())
    }
    fn mcu_command(&mut self, command: u8, payload: &[u8], wait: bool) -> Result<()> {
        self.exchange(command, payload, wait).map(|_| ())
    }
    fn mcu_query(&mut self, command: u8, payload: &[u8]) -> Result<Vec<u8>> {
        self.exchange(command, payload, true)
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(0);
    }
}

pub fn mcu_packet(command: u8, sequence: u8, payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(command < 128 && sequence < 16, "Invalid MCU header fields");
    let aligned = payload.len().checked_add(3).context("length overflow")? & !3;
    ensure!(aligned <= 192, "MCU payload exceeds 192-byte inband limit");
    let mut packet = vec![0; aligned + 8];
    let info = 0x50000000 | ((command as u32) << 20) | ((sequence as u32) << 16) | aligned as u32;
    packet[..4].copy_from_slice(&info.to_le_bytes());
    packet[4..4 + payload.len()].copy_from_slice(payload);
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_retries_timeouts_but_stops_on_disconnect() {
        let mut attempts = 0;
        let value = retry_control(|| {
            attempts += 1;
            if attempts < 3 {
                Err(rusb::Error::Timeout)
            } else {
                Ok(4)
            }
        })
        .unwrap();
        assert_eq!((attempts, value), (3, 4));
        attempts = 0;
        let result: rusb::Result<()> = retry_control(|| {
            attempts += 1;
            Err(rusb::Error::NoDevice)
        });
        assert_eq!(result, Err(rusb::Error::NoDevice));
        assert_eq!(attempts, 1);
        attempts = 0;
        let result: rusb::Result<()> = retry_control(|| {
            attempts += 1;
            Err(rusb::Error::Timeout)
        });
        assert_eq!(result, Err(rusb::Error::Timeout));
        assert_eq!(attempts, 10);
    }
    #[test]
    fn mcu_wire_format_and_bounds() {
        let packet = mcu_packet(12, 3, &[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(&packet[..4], &0x50c30008u32.to_le_bytes());
        assert_eq!(&packet[4..9], &[1, 2, 3, 4, 5]);
        assert_eq!(&packet[9..], &[0; 7]);
        assert!(mcu_packet(128, 0, &[]).is_err());
        assert!(mcu_packet(1, 16, &[]).is_err());
        assert!(mcu_packet(1, 0, &vec![0; 65535]).is_err());
    }
}
