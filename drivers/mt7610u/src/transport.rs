// SPDX-License-Identifier: GPL-2.0-only
// USB protocol adapted from mt76 usb.c / mt76x02_usb_mcu.c / mt76x02_usb_core.c,
// Copyright (C) 2018 Lorenzo Bianconi. Those files are BSD-3-Clause-Clear;
// see THIRD_PARTY_NOTICES for retained terms and the pinned source revision.
pub use crate::timing::{Clock, TimeSource};
use anyhow::{bail, ensure, Context, Result};
use rusb::{Context as UsbContext, DeviceHandle, Direction, TransferType};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(300);

fn retry_control<T>(
    clock: &Clock,
    mut operation: impl FnMut() -> rusb::Result<T>,
) -> rusb::Result<T> {
    for attempt in 0..10 {
        match operation() {
            Err(rusb::Error::Timeout) if attempt < 9 => clock.sleep(Duration::from_millis(6)),
            result => return result,
        }
    }
    unreachable!("last attempt always returns")
}

/// A command was sent, but no matching MCU acknowledgment arrived in time.
#[derive(Debug)]
pub struct McuTimeout {
    pub command: u8,
    pub sequence: u8,
}
impl std::fmt::Display for McuTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MCU command {} sequence {} timed out",
            self.command, self.sequence
        )
    }
}
impl std::error::Error for McuTimeout {}

pub trait RegisterIo {
    fn clock(&self) -> Clock {
        Clock::default()
    }
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

/// Only endpoint I/O is abstracted; production framing, retries and validation
/// run unchanged with either libusb or a scripted transport in tests.
pub trait UsbIo {
    fn clock(&self) -> Clock {
        Clock::default()
    }
    fn write_control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
        timeout: Duration,
    ) -> rusb::Result<usize>;
    fn read_control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &mut [u8],
        timeout: Duration,
    ) -> rusb::Result<usize>;
    fn write_bulk(&mut self, endpoint: u8, data: &[u8], timeout: Duration) -> rusb::Result<usize>;
    fn read_bulk(
        &mut self,
        endpoint: u8,
        data: &mut [u8],
        timeout: Duration,
    ) -> rusb::Result<usize>;
    fn release(&mut self);
}
impl UsbIo for DeviceHandle<UsbContext> {
    fn write_control(
        &mut self,
        t: u8,
        r: u8,
        v: u16,
        i: u16,
        d: &[u8],
        timeout: Duration,
    ) -> rusb::Result<usize> {
        DeviceHandle::write_control(self, t, r, v, i, d, timeout)
    }
    fn read_control(
        &mut self,
        t: u8,
        r: u8,
        v: u16,
        i: u16,
        d: &mut [u8],
        timeout: Duration,
    ) -> rusb::Result<usize> {
        DeviceHandle::read_control(self, t, r, v, i, d, timeout)
    }
    fn write_bulk(&mut self, e: u8, d: &[u8], timeout: Duration) -> rusb::Result<usize> {
        DeviceHandle::write_bulk(self, e, d, timeout)
    }
    fn read_bulk(&mut self, e: u8, d: &mut [u8], timeout: Duration) -> rusb::Result<usize> {
        DeviceHandle::read_bulk(self, e, d, timeout)
    }
    fn release(&mut self) {
        let _ = self.release_interface(0);
    }
}

pub struct UsbDevice<T: UsbIo = DeviceHandle<UsbContext>> {
    handle: T,
    clock: Clock,
    pub input_endpoints: Vec<u8>,
    pub output_endpoints: Vec<u8>,
    sequence: u8,
    pub identity: Option<crate::selection::Identity>,
}

impl UsbDevice {
    pub fn open() -> Result<Self> {
        crate::selection::Selection::default().open()
    }
    pub fn open_selected(selector: Option<crate::selection::Selector>) -> Result<Self> {
        crate::selection::Selection::new(selector).open()
    }
    pub(crate) fn open_device(
        device: rusb::Device<UsbContext>,
        identity: crate::selection::Identity,
    ) -> Result<Self> {
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
        let mut transport = Self::from_io(handle, input_endpoints, output_endpoints)?;
        transport.identity = Some(identity);
        Ok(transport)
    }
}

impl<T: UsbIo> UsbDevice<T> {
    /// Bind orchestration and transport waits to one clock before initialization.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// Construct a transport from claimed endpoint I/O. The first OUT endpoint
    /// is the MCU endpoint; WLAN transmit endpoints are never used.
    pub fn from_io(handle: T, input_endpoints: Vec<u8>, output_endpoints: Vec<u8>) -> Result<Self> {
        ensure!(
            input_endpoints.len() == 2 && output_endpoints.len() == 6,
            "expected two input and six output endpoints"
        );
        ensure!(
            input_endpoints.iter().all(|e| e & 0x80 != 0)
                && output_endpoints.iter().all(|e| e & 0x80 == 0 && *e != 0),
            "invalid endpoint direction"
        );
        Ok(Self {
            clock: handle.clock(),
            handle,
            input_endpoints,
            output_endpoints,
            sequence: 0,
            identity: None,
        })
    }

    pub fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        let mut write = || {
            self.handle
                .write_control(0x40, request, value, index, data, TIMEOUT)
        };
        let count = (if matches!(request, 0x06 | 0x42) {
            retry_control(&self.clock, write)
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
        let deadline = self.clock.now() + Duration::from_millis(1500);
        let mut reply = [0u8; 1024];
        while self.clock.now() < deadline {
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
        Err(McuTimeout {
            command,
            sequence: seq,
        }
        .into())
    }
}

impl<T: UsbIo> RegisterIo for UsbDevice<T> {
    fn clock(&self) -> Clock {
        self.clock.clone()
    }
    fn read32(&mut self, reg: u32) -> Result<u32> {
        ensure!(
            reg & 0xc0000000 == 0,
            "Unsupported special register namespace {reg:#x}"
        );
        let mut data = [0u8; 4];
        let count = retry_control(&self.clock, || {
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

impl<T: UsbIo> Drop for UsbDevice<T> {
    fn drop(&mut self) {
        self.handle.release();
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn register_io_and_mcu_exchange_validate_real_wire_protocol() {
        let endpoint = crate::test_support::Endpoint::default();
        let mut device = endpoint.device();
        device.write32(0x123456, 0xaabbccdd).unwrap();
        assert_eq!(device.read32(0x123456).unwrap(), 0xaabbccdd);
        assert_eq!(device.rmw32(0x123456, 0xff, 0x12).unwrap(), 0xaabbcc12);
        device.fce_write(0x230, 0x12345678).unwrap();
        assert!(device.fce_write(u16::MAX, 0).is_err());
        assert!(device.read32(0xc0000000).is_err());
        assert!(device.write32(0xc0000000, 0).is_err());
        device.mcu_command(1, &[0; 8], false).unwrap();
        for _ in 0..17 {
            device.mcu_command(1, &[0; 8], true).unwrap();
        }
        assert_eq!(device.sequence, 2);
        let address = 0x80000001u32;
        let mut payload = address.to_le_bytes().to_vec();
        payload.extend(0x34u32.to_le_bytes());
        device.mcu_command(12, &payload, true).unwrap();
        assert_eq!(device.mcu_query(10, &payload).unwrap(), payload);
        assert!(device.mcu_command(128, &[], true).is_err());
        let writes = &endpoint.0.lock().unwrap().writes;
        assert!(writes
            .iter()
            .any(|w| (w.1, w.2, w.3) == (0x42, 0x5678, 0x230)));
        assert!(writes
            .iter()
            .any(|w| (w.1, w.2, w.3) == (0x42, 0x1234, 0x232)));
    }

    #[test]
    fn transport_short_io_disconnect_and_reply_validation() {
        let endpoint = crate::test_support::Endpoint::default();
        let mut d = endpoint.device();
        endpoint.0.lock().unwrap().short_read = true;
        assert!(d.read32(0).unwrap_err().to_string().contains("Short"));
        endpoint.0.lock().unwrap().short_read = false;
        endpoint.0.lock().unwrap().short_write = true;
        assert!(d.write32(0, 1).is_err());
        assert!(d.write_mcu_bulk(&[0; 8]).is_err());
        endpoint.0.lock().unwrap().short_write = false;
        let mut buf = [0; 32];
        assert_eq!(d.read_packets(&mut buf, Duration::ZERO).unwrap(), 0);
        endpoint
            .0
            .lock()
            .unwrap()
            .packets
            .push_back(Ok(vec![1, 2, 3]));
        assert_eq!(d.read_packets(&mut buf, Duration::ZERO).unwrap(), 3);
        for error in [rusb::Error::NoDevice, rusb::Error::Pipe] {
            endpoint.0.lock().unwrap().error = Some(error);
            assert!(d.read_packets(&mut buf, Duration::ZERO).is_err());
            assert!(d.write32(0, 0).is_err());
            assert!(d.read32(0).is_err());
        }
        endpoint.0.lock().unwrap().error = None;
        for reply in [Ok(vec![0; 3]), Err(rusb::Error::NoDevice)] {
            endpoint.0.lock().unwrap().responses.clear();
            endpoint.0.lock().unwrap().responses.push_back(reply);
            assert!(d.mcu_query(1, &[]).is_err());
        }
        endpoint.0.lock().unwrap().responses.clear();
        let seq = d.sequence % 15 + 1;
        let mut reply = (((seq as u32) << 16) | (1 << 20)).to_le_bytes().to_vec();
        reply.extend([0; 4]);
        endpoint.0.lock().unwrap().responses.push_back(Ok(reply));
        assert!(d
            .mcu_query(1, &[])
            .unwrap_err()
            .to_string()
            .contains("event"));
        endpoint.0.lock().unwrap().responses.clear();
        endpoint
            .0
            .lock()
            .unwrap()
            .responses
            .extend([Err(rusb::Error::Timeout), Ok(vec![0; 8])]);
        assert!(d.mcu_query(1, &[]).is_ok()); // timeout and stale sequence skipped
        drop(d);
        assert_eq!(endpoint.0.lock().unwrap().released, 1);
        assert!(UsbDevice::from_io(endpoint.clone(), vec![], vec![]).is_err());
        assert!(UsbDevice::from_io(endpoint, vec![1, 2], vec![1, 2, 3, 4, 5, 6]).is_err());
    }

    #[test]
    fn control_retries_timeouts_but_stops_on_disconnect() {
        let time = crate::test_support::ManualTime::default();
        let clock = time.clock();
        let mut attempts = 0;
        let value = retry_control(&clock, || {
            attempts += 1;
            if attempts < 3 {
                Err(rusb::Error::Timeout)
            } else {
                Ok(4)
            }
        })
        .unwrap();
        assert_eq!((attempts, value), (3, 4));
        assert_eq!(clock.now(), Duration::from_millis(12));
        attempts = 0;
        let result: rusb::Result<()> = retry_control(&clock, || {
            attempts += 1;
            Err(rusb::Error::NoDevice)
        });
        assert_eq!(result, Err(rusb::Error::NoDevice));
        assert_eq!(attempts, 1);
        assert_eq!(clock.now(), Duration::from_millis(12));
        attempts = 0;
        let result: rusb::Result<()> = retry_control(&clock, || {
            attempts += 1;
            Err(rusb::Error::Timeout)
        });
        assert_eq!(result, Err(rusb::Error::Timeout));
        assert_eq!(attempts, 10);
        assert_eq!(clock.now(), Duration::from_millis(66));
    }
    #[test]
    fn mcu_deadline_refuses_late_ack_without_waiting_for_wall_time() {
        let endpoint = crate::test_support::Endpoint::default();
        let mut device = endpoint.device();
        let clock = device.clock();
        endpoint
            .0
            .lock()
            .unwrap()
            .responses
            .extend(vec![Err(rusb::Error::Timeout); 5]);
        // The normal scripted write appends a valid ACK after the five reads.
        let error = device.mcu_command(1, &[0; 8], true).unwrap_err();
        assert!(error.is::<McuTimeout>());
        assert_eq!(clock.now(), Duration::from_millis(1500));
        assert_eq!(endpoint.0.lock().unwrap().responses.len(), 1);
        drop(device);
        assert_eq!(endpoint.0.lock().unwrap().released, 1);
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
