//! Scripted endpoint model: no USB hardware, firmware file, or network required.
#![allow(dead_code)]
use crate::transport::{UsbDevice, UsbIo};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

pub struct Model {
    pub registers: BTreeMap<u32, u32>,
    pub rf: BTreeMap<u32, u32>,
    pub eeprom: [u8; 512],
    pub efuse_offset: usize,
    pub responses: VecDeque<rusb::Result<Vec<u8>>>,
    pub packets: VecDeque<rusb::Result<Vec<u8>>>,
    pub writes: Vec<(u8, u8, u16, u16, Vec<u8>)>,
    pub bulk_writes: Vec<Vec<u8>>,
    pub short_write: bool,
    pub short_read: bool,
    pub error: Option<rusb::Error>,
    pub released: usize,
    pub upload: bool,
}
impl Default for Model {
    fn default() -> Self {
        let mut eeprom = [0; 512];
        eeprom[..2].copy_from_slice(&0x7610u16.to_le_bytes());
        eeprom[0x34..0x36].copy_from_slice(&0x0111u16.to_le_bytes());
        eeprom[0x44] = 12;
        Self {
            registers: BTreeMap::from([
                (0, 0x76100002),
                (0x1000, 0x76502000),
                (0x20, 0xc00000),
                (0x2000, 0x7610),
                (0x730, 1),
            ]),
            rf: BTreeMap::new(),
            eeprom,
            efuse_offset: 0,
            responses: VecDeque::new(),
            packets: VecDeque::new(),
            writes: vec![],
            bulk_writes: vec![],
            short_write: false,
            short_read: false,
            error: None,
            released: 0,
            upload: false,
        }
    }
}
#[derive(Clone, Default)]
pub struct Endpoint(pub Arc<Mutex<Model>>);
impl Endpoint {
    pub fn device(&self) -> UsbDevice<Self> {
        UsbDevice::from_io(self.clone(), vec![0x81, 0x82], vec![1, 2, 3, 4, 5, 6]).unwrap()
    }
}
fn word(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}
impl UsbIo for Endpoint {
    fn write_control(
        &mut self,
        t: u8,
        r: u8,
        v: u16,
        i: u16,
        d: &[u8],
        _: Duration,
    ) -> rusb::Result<usize> {
        let mut m = self.0.lock().unwrap();
        if let Some(e) = m.error {
            return Err(e);
        }
        m.writes.push((t, r, v, i, d.to_vec()));
        if m.short_write {
            return Ok(d.len().saturating_sub(1));
        }
        if r == 6 {
            let reg = ((v as u32) << 16) | i as u32;
            let value = word(d);
            match reg {
                0x1004 => assert_eq!(value & 4, 0, "MAC TX enabled"),
                0x1404 => assert_eq!(value & 1, 0, "automatic ACK enabled"),
                0x1114 => assert_eq!(value & (1 << 20), 0, "beacon TX enabled"),
                _ => {}
            }
            if reg == 0x24 {
                assert_eq!(value & 0xc0, 0, "EEPROM write mode forbidden");
                m.efuse_offset = ((value >> 16) & 0x3ff) as usize;
                m.registers.insert(reg, value & !(1 << 30));
            } else {
                m.registers.insert(reg, value);
            }
        }
        if r == 1 && v == 1 {
            m.upload = true;
        }
        if r == 1 && v == 0x12 {
            m.upload = false;
            m.registers.insert(0x730, 1);
        }
        Ok(d.len())
    }
    fn read_control(
        &mut self,
        t: u8,
        r: u8,
        v: u16,
        i: u16,
        d: &mut [u8],
        _: Duration,
    ) -> rusb::Result<usize> {
        assert_eq!((t, r), (0xc0, 7));
        let m = self.0.lock().unwrap();
        if let Some(e) = m.error {
            return Err(e);
        }
        if m.short_read {
            return Ok(2);
        }
        let reg = ((v as u32) << 16) | i as u32;
        let value = if (0x28..=0x34).contains(&reg) && reg.is_multiple_of(4) {
            word(&m.eeprom[m.efuse_offset + (reg - 0x28) as usize..])
        } else {
            *m.registers.get(&reg).unwrap_or(&0)
        };
        d[..4].copy_from_slice(&value.to_le_bytes());
        Ok(4)
    }
    fn write_bulk(&mut self, e: u8, d: &[u8], _: Duration) -> rusb::Result<usize> {
        assert_eq!(e, 1, "WLAN TX endpoint used");
        let mut m = self.0.lock().unwrap();
        if let Some(e) = m.error {
            return Err(e);
        }
        m.bulk_writes.push(d.to_vec());
        if m.short_write {
            return Ok(d.len() - 1);
        }
        if m.upload {
            return Ok(d.len());
        }
        let header = word(d);
        let command = (header >> 20) & 0x7f;
        let seq = (header >> 16) & 15;
        let payload = &d[4..d.len() - 4];
        let mut response = vec![];
        if command == 12 {
            for p in payload.as_chunks::<8>().0 {
                m.rf.insert(word(p), word(&p[4..]));
            }
        } else if command == 10 {
            let address = word(payload);
            response.extend_from_slice(&address.to_le_bytes());
            response.extend_from_slice(&m.rf.get(&address).copied().unwrap_or(0).to_le_bytes());
        } else if command == 31 {
            assert!(
                [1, 2, 12].contains(&word(payload)),
                "TX-inclusive calibration"
            );
        }
        if seq != 0 {
            let mut reply = (seq << 16).to_le_bytes().to_vec();
            reply.extend(response);
            reply.extend([0; 4]);
            m.responses.push_back(Ok(reply));
        }
        Ok(d.len())
    }
    fn read_bulk(&mut self, e: u8, d: &mut [u8], _: Duration) -> rusb::Result<usize> {
        let mut m = self.0.lock().unwrap();
        if let Some(e) = m.error {
            return Err(e);
        }
        let result = if e == 0x82 {
            m.responses.pop_front()
        } else {
            assert_eq!(e, 0x81);
            m.packets.pop_front()
        };
        match result.unwrap_or(Err(rusb::Error::Timeout)) {
            Ok(bytes) => {
                d[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            Err(e) => Err(e),
        }
    }
    fn release(&mut self) {
        self.0.lock().unwrap().released += 1;
    }
}

pub fn image() -> Vec<u8> {
    let mut b = vec![0; 32 + 68 + 5];
    b[..4].copy_from_slice(&68u32.to_le_bytes());
    b[4..8].copy_from_slice(&5u32.to_le_bytes());
    b[32..].fill(0x11);
    b
}

pub fn beacon_transfer() -> Vec<u8> {
    let mut frame = vec![0; 36];
    frame[0] = 0x80;
    frame[4..10].fill(255);
    frame[10..16].copy_from_slice(&[2, 1, 2, 3, 4, 5]);
    frame[16..22].copy_from_slice(&[2, 1, 2, 3, 4, 5]);
    frame[32..34].copy_from_slice(&100u16.to_le_bytes());
    frame.extend_from_slice(&[0, 4, b'T', b'e', b's', b't', 3, 1, 6]);
    let dma = (32 + frame.len() + 3) & !3;
    let mut bytes = vec![0; dma + 8];
    bytes[..4].copy_from_slice(&(dma as u32).to_le_bytes());
    bytes[8..12].copy_from_slice(&((frame.len() as u32) << 16).to_le_bytes());
    bytes[16] = 0xba;
    bytes[36..36 + frame.len()].copy_from_slice(&frame);
    bytes[dma + 4..].copy_from_slice(&0x18080010u32.to_le_bytes());
    bytes
}
