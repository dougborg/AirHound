// SPDX-License-Identifier: GPL-2.0-only
// Derived from mt76 mt76x0/{init,phy,eeprom}.c at commit
// 22f7ff9be59890cb10cde16cc25a30f78b504f23 (https://github.com/openwrt/mt76).
// Copyright 2002-2010 Ralink Technology, Inc.; 2014 Felix Fietkau;
// 2015 Jakub Kicinski; 2018 Stanislaw Gruszka.
// Register definitions, efuse reads and MCU message layouts also derive from
// mt76x02_{regs,eeprom,mcu}.{h,c}: BSD-3-Clause-Clear, Copyright 2016 Felix Fietkau
// and 2018 Lorenzo Bianconi. See this crate's third-party license notices.

//! Experimental channel-one, 20MHz receive-only setup, after firmware bootstrap.
//!
//! This is deliberately not the complete Linux radio initialization sequence:
//! FULL, LC, TSSI and all TX calibration commands are omitted. Successful register
//! programming is not proof of calibrated RX or a supported driver. Capture must
//! separately demonstrate valid over-the-air frames. No EEPROM programming,
//! beacon generation, automatic ACKs or MAC TX enable is exposed here.

use crate::transport::RegisterIo;
use anyhow::{bail, ensure, Context, Result};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[path = "radio_tables.rs"]
mod tables;

const MAC_SYS_CTRL: u32 = 0x1004;
const MAC_TX: u32 = 1 << 2;
const MAC_RX: u32 = 1 << 3;
const AUTO_RSP_CFG: u32 = 0x1404;
const BEACON_TIME_CFG: u32 = 0x1114;
const BEACON_TX: u32 = 1 << 20;
const WPDMA_GLO_CFG: u32 = 0x0208;
const USB_DMA_CFG: u32 = 0x0238;
const EFUSE_CTRL: u32 = 0x0024;
const EFUSE_KICK: u32 = 1 << 30;
const EFUSE_MODE: u32 = 0xc0;
const EFUSE_AIN: u32 = 0x03ff_0000;
const RF_MEMMAP: u32 = 0x8000_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RxCalibration {
    pub frequency_offset: u8,
    pub lna_gain: i8,
    pub rssi_offset: i8,
    pub antenna_config: u16,
    pub config1: u16,
    pub nic_config2: u16,
    pub has_5ghz: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioSetup {
    pub channel: u8,
    pub frequency_mhz: u16,
    pub bandwidth_mhz: u8,
    pub full_calibration_performed: bool,
    pub omitted_calibrations: &'static str,
    pub calibration: RxCalibration,
}

fn rmw(io: &mut impl RegisterIo, reg: u32, mask: u32, value: u32) -> Result<()> {
    let current = io.read32(reg)?;
    ensure!(
        current != u32::MAX,
        "device unavailable while updating register {reg:#06x}"
    );
    io.write32(reg, (current & !mask) | value)
}

fn poll_clear(io: &mut impl RegisterIo, reg: u32, mask: u32, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    loop {
        let value = io.read32(reg)?;
        ensure!(
            value != u32::MAX,
            "device unavailable while polling register {reg:#06x}"
        );
        if value & mask == 0 {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            bail!("register {reg:#06x} remained busy (mask {mask:#010x})");
        }
        sleep(Duration::from_millis(1));
    }
}

/// Read logical efuse-backed EEPROM. The only writes are READ-command triggers
/// to EFUSE_CTRL with MODE=0; EFUSE_DATA and programming modes are never written.
pub fn read_eeprom(io: &mut impl RegisterIo) -> Result<[u8; 512]> {
    let mut bytes = [0xff; 512];
    for offset in (0..512).step_by(16) {
        poll_clear(io, EFUSE_CTRL, EFUSE_KICK, Duration::from_secs(1))?;
        let control = io.read32(EFUSE_CTRL)?;
        ensure!(control != u32::MAX, "device unavailable during efuse read");
        let command = (control & !(EFUSE_AIN | EFUSE_MODE)) | ((offset as u32) << 16) | EFUSE_KICK;
        io.write32(EFUSE_CTRL, command)?;
        poll_clear(io, EFUSE_CTRL, EFUSE_KICK, Duration::from_secs(1))?;
        sleep(Duration::from_micros(2));
        if io.read32(EFUSE_CTRL)? & 0x3f == 0x3f {
            continue;
        }
        for word in 0..4 {
            let value = io.read32(0x0028 + 4 * word as u32)?;
            bytes[offset + word * 4..offset + word * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

pub fn parse_calibration(bytes: &[u8; 512]) -> Result<RxCalibration> {
    let word = |offset| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    ensure!(
        word(0) == 0x7610,
        "expected MT7610 EEPROM, found chip ID {:#06x}",
        word(0)
    );
    let nic0 = word(0x34);
    ensure!(
        (nic0 >> 12) & 3 != 2,
        "EEPROM describes a 5GHz-only board; channel one is unsupported"
    );
    ensure!(
        nic0 & 0xf <= 1,
        "multiple RX chains are not supported by this experimental driver"
    );
    let valid = |v: u8| if v == 0xff { 0 } else { v };
    let base = valid(bytes[0x3a]);
    // mt76's frequency adjustment uses sign/magnitude with bit7=positive,
    // not two's complement. Match its u8 arithmetic and 0xbf hardware clamp.
    let adjustment = valid(bytes[0xdb]);
    let magnitude = (adjustment & 0x7f) as i16;
    let adjustment = if adjustment & 0x80 != 0 {
        magnitude
    } else {
        -magnitude
    };
    let rssi_offset = bytes[0x46] as i8;
    Ok(RxCalibration {
        frequency_offset: base.wrapping_sub(adjustment as u8).min(0xbf),
        lna_gain: valid(bytes[0x44]) as i8,
        rssi_offset: if (-10..=10).contains(&rssi_offset) {
            rssi_offset
        } else {
            0
        },
        antenna_config: word(0x22),
        config1: word(0x24),
        nic_config2: word(0x42),
        has_5ghz: (nic0 >> 12) & 3 != 1,
    })
}

fn mcu_pair(io: &mut impl RegisterIo, command: u8, id: u32, value: u32, wait: bool) -> Result<()> {
    let mut payload = [0; 8];
    payload[..4].copy_from_slice(&id.to_le_bytes());
    payload[4..].copy_from_slice(&value.to_le_bytes());
    io.mcu_command(command, &payload, wait)
}

fn rf_address(bank: u8, reg: u8) -> Result<u32> {
    ensure!(
        bank <= 7 && reg <= 127,
        "invalid RF bank/register {bank}:{reg}"
    );
    Ok(RF_MEMMAP | ((bank as u32) << 16) | reg as u32)
}

pub fn read_rf(io: &mut impl RegisterIo, bank: u8, reg: u8) -> Result<u8> {
    let address = rf_address(bank, reg)?;
    let mut payload = [0; 8];
    payload[..4].copy_from_slice(&address.to_le_bytes());
    let reply = io.mcu_query(10, &payload)?;
    ensure!(
        reply.len() == 8,
        "RF read returned {} bytes, expected one address/value pair",
        reply.len()
    );
    let echoed = u32::from_le_bytes(reply[..4].try_into().unwrap());
    ensure!(
        echoed == address,
        "RF read address mismatch: {echoed:#010x} != {address:#010x}"
    );
    let value = u32::from_le_bytes(reply[4..].try_into().unwrap());
    ensure!(value <= 255, "RF register value exceeds one byte");
    Ok(value as u8)
}

pub fn write_rf(io: &mut impl RegisterIo, bank: u8, reg: u8, value: u8) -> Result<()> {
    mcu_pair(io, 12, rf_address(bank, reg)?, value as u32, true)
}

fn rf_rmw(io: &mut impl RegisterIo, reg: u8, mask: u8, value: u8) -> Result<()> {
    let current = read_rf(io, 0, reg)?;
    write_rf(io, 0, reg, (current & !mask) | value)
}

fn write_rf_table(io: &mut impl RegisterIo, entries: &[(u32, u32)]) -> Result<()> {
    // Small acknowledged batches remain below the upstream 192-byte in-band limit.
    for chunk in entries.chunks(16) {
        let mut payload = Vec::with_capacity(chunk.len() * 8);
        for &(offset, value) in chunk {
            let address = rf_address((offset >> 16) as u8, (offset & 0xff) as u8)?;
            ensure!(value <= 255, "RF table value exceeds one byte");
            payload.extend_from_slice(&address.to_le_bytes());
            payload.extend_from_slice(&value.to_le_bytes());
        }
        io.mcu_command(12, &payload, true)?;
    }
    Ok(())
}

fn write_mac_table(io: &mut impl RegisterIo, entries: &[(u32, u32)]) -> Result<()> {
    for &(reg, value) in entries {
        // Upstream normal-client initialization enables automatic ACKs. Keep
        // these gates closed even transiently while applying its static tables.
        let value = match reg {
            MAC_SYS_CTRL => value & !(MAC_TX | MAC_RX),
            AUTO_RSP_CFG => 0,
            BEACON_TIME_CFG => value & !BEACON_TX,
            WPDMA_GLO_CFG => value & !1,
            _ => value,
        };
        io.write32(reg, value)?;
    }
    Ok(())
}

/// Disable packet reception and every exposed autonomous transmit gate.
/// Called before initialization and by the owner during teardown/error cleanup.
pub fn disable(io: &mut impl RegisterIo) -> Result<()> {
    // A failed USB transfer must not prevent attempts to close the remaining
    // independent gates. Evaluate every operation before returning diagnostics.
    let attempts = [
        ("MAC TX/RX", rmw(io, MAC_SYS_CTRL, MAC_TX | MAC_RX, 0)),
        ("automatic responses", io.write32(AUTO_RSP_CFG, 0)),
        (
            "beacon/TBTT",
            rmw(io, BEACON_TIME_CFG, BEACON_TX | (1 << 19), 0),
        ),
        ("packet DMA", rmw(io, WPDMA_GLO_CFG, 1 | (1 << 2), 0)),
    ];
    let failures: Vec<_> = attempts
        .into_iter()
        .filter_map(|(gate, result)| result.err().map(|error| format!("{gate}: {error:#}")))
        .collect();
    ensure!(
        failures.is_empty(),
        "radio shutdown incomplete: {}",
        failures.join("; ")
    );
    Ok(())
}

fn configure_antenna(io: &mut impl RegisterIo, cal: &RxCalibration) -> Result<()> {
    let mut antenna = cal.antenna_config & !((1 << 14) | (1 << 12));
    let mut wlan = io.read32(0x0080)? & !((1 << 6) | (1 << 5));
    let mut coexist = io.read32(0x004c)? & !0x3c;
    if antenna & (1 << 15) != 0 {
        let diversity = cal.nic_config2 & (1 << 3) == 0 && cal.nic_config2 & (1 << 4) != 0;
        if diversity {
            antenna |= 1 << 12;
        } else {
            coexist |= 1 << 4;
        }
        coexist |= 1 << 3;
        wlan |= 1 << 6;
    } else if cal.has_5ghz {
        coexist |= (1 << 3) | (1 << 4);
    } else {
        wlan |= 1 << 6;
        coexist |= 1 << 1;
    }
    io.write32(0x0080, wlan)?;
    rmw(io, 0x0020, 0xffff, antenna as u32)?;
    // CSR_EE_CFG1 is a volatile configuration register, not EEPROM storage.
    rmw(io, 0x0104, 0xffff, cal.config1 as u32)?;
    rmw(io, 0x0040, 1 << 2, 0)?;
    io.write32(0x004c, coexist)
}

fn validate_channel(channel: u8) -> Result<()> {
    ensure!(
        (1..=11).contains(&channel),
        "supported receive channels are 1..11, got {channel}"
    );
    Ok(())
}

fn configure_channel(io: &mut impl RegisterIo, cal: &RxCalibration, channel: u8) -> Result<()> {
    let [r35, r33, r32, r31, pll_n, xo_div] = tables::CHANNEL_PLL[usize::from(channel - 1)];
    mcu_pair(io, 1, 2, 0, true).context("MCU BW_SETTING 20MHz")?;
    rmw(io, 0x2004, 0x18 | 0x20, 0)?; // CORE1 bandwidth/Japan channel14 filter
    rmw(io, 0x2300, 0x7000 | 0x300, 0x1000)?; // AGC0 BW20, primary channel0
    rmw(io, 0x2700, 3, 0)?;
    rmw(io, 0x132c, 7, 4)?; // 2.4GHz, lower primary
    rmw(io, 0x141c, 0xfff, 0x1e4)?; // primary20 CCA mapping [0,1,2,3], mask1
    write_rf_table(io, tables::RF_2G)?;
    write_rf(io, 5, 0, 0x45)?;
    write_rf(io, 6, 0, 0x44)?;
    // Channels 1..11 use the non-SDM frequency plan.
    for (reg, value) in [(37, 0x02), (36, 0x3f), (35, r35), (34, 0xdd), (33, r33)] {
        write_rf(io, 0, reg, value)?;
    }
    for (reg, mask, value) in [
        (32, 0xe0, 0x40),
        (32, 0x1f, r32),
        (31, 0xe0, 0x40),
        (31, 0x1f, r31),
        (30, 0x80, 0),
        (30, 0x7c, 0),
        (30, 2, 2),
    ] {
        rf_rmw(io, reg, mask, value)?;
    }
    write_rf(io, 0, 29, pll_n)?;
    rf_rmw(io, 30, 1, 0)?;
    for (mask, value) in [(0xc0, 0), (0x30, 0x30), (0x0c, 0)] {
        rf_rmw(io, 28, mask, value)?;
    }
    write_rf(io, 0, 26, 0)?;
    write_rf(io, 0, 27, 0)?;
    rf_rmw(io, 28, 3, 0)?;
    rf_rmw(io, 24, 3, xo_div)?;
    write_rf_table(io, tables::RF_BW_20)?;
    write_rf_table(io, tables::RF_BAND_2G)?;
    // External PA enable is intentionally left off for receive-only operation.
    rmw(io, 0x0518, 0x0c, 0)?;
    for &(reg, value) in tables::BBP_2G_20 {
        let value = if reg == 0x2320 {
            let gain =
                ((value >> 8) as u8 & 0x7f).wrapping_sub((cal.lna_gain as u8).wrapping_mul(2));
            (value & !0x7f00) | (((gain as u32) & 0x7f) << 8)
        } else {
            value
        };
        io.write32(reg, value)?;
    }
    rf_rmw(io, 4, 0, 0x80)?; // Internal VCO calibration trigger, no MAC TX.
    Ok(())
}

/// Configure volatile hardware for channel1 and enable passive RX. Caller must
/// own the USB interface, have booted firmware, and arrange `disable` on exit.
/// This deliberately reports incomplete full calibration rather than claiming
/// the omitted TX-inclusive firmware calibration is safe for a passive tool.
pub fn initialize_channel_one(io: &mut impl RegisterIo) -> Result<RadioSetup> {
    initialize_channel(io, 1)
}

/// Initialize passive 20 MHz reception on a supported 2.4 GHz channel.
/// Invalid channels are rejected before touching hardware.
pub fn initialize_channel(io: &mut impl RegisterIo, channel: u8) -> Result<RadioSetup> {
    validate_channel(channel)?;
    disable(io)?;
    match initialize_inner(io, channel) {
        Ok(setup) => Ok(setup),
        Err(error) => match disable(io) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(format!(
                "radio initialization failed; cleanup also failed: {cleanup:#}"
            ))),
        },
    }
}

fn initialize_inner(io: &mut impl RegisterIo, channel: u8) -> Result<RadioSetup> {
    let calibration = parse_calibration(&read_eeprom(io)?)?;
    poll_clear(
        io,
        WPDMA_GLO_CFG,
        (1 << 1) | (1 << 3),
        Duration::from_secs(1),
    )?;
    io.write32(MAC_SYS_CTRL, 3)?;
    sleep(Duration::from_millis(200));
    io.write32(MAC_SYS_CTRL, 0)?;
    mcu_pair(io, 1, 1, 1, false).context("MCU Q_SELECT")?;
    write_mac_table(io, tables::COMMON_MAC)?;
    write_mac_table(io, tables::MAC)?;
    rmw(io, 0x141c, 0, 0xf000)?;
    rmw(io, 0x080c, 1 << 4, 0)?;
    rmw(io, 0x0230, 0x3ff, 0x201)?;
    poll_clear(io, 0x1200, 3, Duration::from_secs(1))?;
    let mut ready = false;
    for _ in 0..20 {
        let revision = io.read32(0x2000)?;
        if revision != 0 && revision != u32::MAX {
            ready = true;
            break;
        }
        sleep(Duration::from_millis(1));
    }
    ensure!(ready, "BBP is not ready after reset");
    write_mac_table(io, tables::BBP)?;
    write_mac_table(io, tables::BBP_2G_20)?;
    write_mac_table(io, tables::DCOC)?;
    configure_antenna(io, &calibration)?;
    write_rf_table(io, tables::RF_CENTRAL)?;
    write_rf_table(io, tables::RF_2G)?;
    write_rf_table(io, tables::RF_5G)?;
    write_rf_table(io, tables::RF_VGA)?;
    write_rf_table(io, tables::RF_BW_20)?;
    write_rf_table(io, tables::RF_BAND_2G)?;
    write_rf(io, 0, 22, calibration.frequency_offset)?;
    ensure!(
        read_rf(io, 0, 22)? == calibration.frequency_offset,
        "RF frequency offset readback mismatch"
    );
    rf_rmw(io, 73, 0, 0x80)?;
    rf_rmw(io, 73, 0x80, 0)?;
    rf_rmw(io, 73, 0, 0x80)?;
    rmw(io, 0x2300, (1 << 4) | (1 << 3), 0)?; // one RX chain
    configure_channel(io, &calibration, channel)?;
    // Only resistance, VCO and receive DC-offset calibration. FULL (0xff),
    // TXIQ, TXDCOC, DPD, TSSI and LC sequences are deliberately absent.
    mcu_pair(io, 31, 1, 0, true).context("MCU resistance calibration")?;
    mcu_pair(io, 31, 12, u32::from(channel), true).context("MCU VCO calibration")?;
    mcu_pair(io, 31, 2, 1, true).context("MCU RX DC-offset calibration")?;
    enable_passive_rx(io)?;
    Ok(RadioSetup {
        channel, frequency_mhz: 2412 + 5 * u16::from(channel - 1), bandwidth_mhz: 20,
        full_calibration_performed: false,
        omitted_calibrations: "FULL, LC, TSSI, TXIQ, TXDCOC, TX group-delay and DPD; RX sensitivity remains experimental",
        calibration,
    })
}

fn enable_passive_rx(io: &mut impl RegisterIo) -> Result<()> {
    // No frame filters except invalid PHY/version frames; accept other BSS,
    // multicast, broadcast and control frames. Bit0 remains clear to retain FCS errors.
    io.write32(0x1400, (1 << 1) | (1 << 4))?;
    io.write32(AUTO_RSP_CFG, 0)?;
    rmw(io, BEACON_TIME_CFG, BEACON_TX | (1 << 19) | (3 << 17), 0)?;
    rmw(io, USB_DMA_CFG, 1 << 21, 1 << 22)?; // single-frame RX USB transfers
    rmw(io, WPDMA_GLO_CFG, 1, 1 << 2)?;
    rmw(io, MAC_SYS_CTRL, MAC_TX | 3, MAC_RX)?;
    ensure!(
        io.read32(MAC_SYS_CTRL)? & (MAC_TX | MAC_RX) == MAC_RX,
        "MAC RX/TX gates do not match passive reception"
    );
    ensure!(
        io.read32(AUTO_RSP_CFG)? & 1 == 0,
        "automatic ACK unexpectedly enabled"
    );
    ensure!(
        io.read32(BEACON_TIME_CFG)? & BEACON_TX == 0,
        "beacon TX unexpectedly enabled"
    );
    ensure!(
        io.read32(WPDMA_GLO_CFG)? & 5 == 4,
        "packet DMA gates do not match passive reception"
    );
    Ok(())
}

/// Stop reception, discard pending old-channel USB packets, then retune.
/// The callback must drain the packet endpoint until quiet with a bounded deadline.
/// It runs with MAC RX/TX off and RX DMA on, so queued frames can reach USB.
/// On failure all receive/transmit gates are closed best-effort; resume requires
/// another successful retune. FULL/TX calibrations remain intentionally omitted.
pub fn retune_with_drain<I: RegisterIo>(
    io: &mut I,
    calibration: &RxCalibration,
    channel: u8,
    drain: impl FnOnce(&mut I) -> Result<()>,
) -> Result<()> {
    validate_channel(channel)?;
    let result = (|| {
        rmw(io, MAC_SYS_CTRL, MAC_TX | MAC_RX, 0)?;
        io.write32(AUTO_RSP_CFG, 0)?;
        rmw(io, BEACON_TIME_CFG, BEACON_TX | (1 << 19), 0)?;
        rmw(io, WPDMA_GLO_CFG, 1, 1 << 2)?;
        poll_clear(io, 0x1200, 3, Duration::from_secs(1))?;
        drain(io).context("draining old-channel packets")?;
        rmw(io, WPDMA_GLO_CFG, 1 | (1 << 2), 0)?;
        poll_clear(
            io,
            WPDMA_GLO_CFG,
            (1 << 1) | (1 << 3),
            Duration::from_secs(1),
        )?;
        configure_channel(io, calibration, channel)?;
        mcu_pair(io, 31, 12, u32::from(channel), true).context("MCU VCO calibration")?;
        mcu_pair(io, 31, 2, 1, true).context("MCU RX DC-offset calibration")?;
        enable_passive_rx(io)
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => match disable(io) {
            Ok(()) => Err(error),
            Err(cleanup) => {
                Err(error.context(format!("retune failed; cleanup also failed: {cleanup:#}")))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct SimulatedDevice {
        registers: BTreeMap<u32, u32>,
        rf: BTreeMap<u32, u32>,
        eeprom: [u8; 512],
        efuse_offset: usize,
        efuse_triggers: Vec<u32>,
        calibrations: Vec<u32>,
        calibration_parameters: Vec<u32>,
        fail_calibration: bool,
        bbp_ready: bool,
        bad_rf_response: bool,
    }

    impl SimulatedDevice {
        fn new() -> Self {
            let mut eeprom = [0; 512];
            eeprom[..2].copy_from_slice(&0x7610u16.to_le_bytes());
            eeprom[0x34..0x36].copy_from_slice(&0x0111u16.to_le_bytes());
            eeprom[0x3a] = 0x25;
            eeprom[0xdb] = 0x83;
            eeprom[0x44] = 12;
            eeprom[0x46] = (-2i8) as u8;
            Self {
                registers: BTreeMap::new(),
                rf: BTreeMap::new(),
                eeprom,
                efuse_offset: 0,
                efuse_triggers: Vec::new(),
                calibrations: Vec::new(),
                calibration_parameters: Vec::new(),
                fail_calibration: false,
                bbp_ready: true,
                bad_rf_response: false,
            }
        }
    }

    impl RegisterIo for SimulatedDevice {
        fn read32(&mut self, reg: u32) -> Result<u32> {
            if reg == 0x2000 {
                return Ok(if self.bbp_ready { 0x7610 } else { 0 });
            }
            if (0x28..=0x34).contains(&reg) && reg.is_multiple_of(4) {
                let offset = self.efuse_offset + (reg - 0x28) as usize;
                return Ok(u32::from_le_bytes(
                    self.eeprom[offset..offset + 4].try_into().unwrap(),
                ));
            }
            Ok(*self.registers.get(&reg).unwrap_or(&0))
        }
        fn write32(&mut self, reg: u32, value: u32) -> Result<()> {
            if reg == MAC_SYS_CTRL {
                assert_eq!(value & MAC_TX, 0, "MAC TX must never be enabled");
            }
            if reg == AUTO_RSP_CFG {
                assert_eq!(value, 0, "automatic ACK must remain disabled");
            }
            if reg == BEACON_TIME_CFG {
                assert_eq!(value & BEACON_TX, 0, "beacon TX must remain disabled");
            }
            assert!(
                !(0x28..=0x34).contains(&reg),
                "efuse DATA registers must never be written"
            );
            if reg == EFUSE_CTRL {
                assert_eq!(value & EFUSE_MODE, 0, "only logical READ mode is permitted");
                assert_ne!(value & EFUSE_KICK, 0);
                self.efuse_offset = ((value & EFUSE_AIN) >> 16) as usize;
                self.efuse_triggers.push(value);
                self.registers.insert(reg, value & !EFUSE_KICK);
            } else {
                self.registers.insert(reg, value);
            }
            Ok(())
        }
        fn mcu_command(&mut self, command: u8, payload: &[u8], _wait: bool) -> Result<()> {
            assert!(payload.len() <= 192);
            match command {
                12 => {
                    assert_eq!(payload.len() % 8, 0);
                    for pair in payload.as_chunks::<8>().0 {
                        let address = u32::from_le_bytes(pair[..4].try_into().unwrap());
                        let value = u32::from_le_bytes(pair[4..].try_into().unwrap());
                        assert_eq!(address & 0x8000_0000, RF_MEMMAP);
                        assert!(value <= 255);
                        self.rf.insert(address, value);
                    }
                }
                31 => {
                    let calibration = u32::from_le_bytes(payload[..4].try_into().unwrap());
                    assert!(
                        [1, 2, 12].contains(&calibration),
                        "TX-inclusive calibration is forbidden"
                    );
                    self.calibrations.push(calibration);
                    self.calibration_parameters
                        .push(u32::from_le_bytes(payload[4..8].try_into().unwrap()));
                    ensure!(!self.fail_calibration, "injected calibration failure");
                }
                1 => {}
                _ => panic!("unexpected MCU command {command}"),
            }
            Ok(())
        }
        fn mcu_query(&mut self, command: u8, payload: &[u8]) -> Result<Vec<u8>> {
            assert_eq!(command, 10);
            assert_eq!(payload.len(), 8);
            if self.bad_rf_response {
                return Ok(vec![0; 7]);
            }
            let address = u32::from_le_bytes(payload[..4].try_into().unwrap());
            let mut response = payload.to_vec();
            response[4..]
                .copy_from_slice(&self.rf.get(&address).copied().unwrap_or(0).to_le_bytes());
            Ok(response)
        }
    }

    #[test]
    fn hopping_programs_pll_and_drains_before_tuning() {
        let mut io = SimulatedDevice::new();
        let setup = initialize_channel(&mut io, 6).unwrap();
        assert_eq!((setup.channel, setup.frequency_mhz), (6, 2437));
        for (channel, r35, r33, r32, r31, n, divider) in [
            (1, 0x28, 0xe2, 0x42, 0x42, 0x28, 3),
            (6, 0x3c, 0xd4, 0x47, 0x47, 0xa2, 1),
            (11, 0x3c, 0xd4, 0x47, 0x42, 0xa4, 1),
        ] {
            let old_pll = io.rf[&(RF_MEMMAP | 29)];
            let mut drained = false;
            retune_with_drain(&mut io, &setup.calibration, channel, |io| {
                assert_eq!(io.registers[&MAC_SYS_CTRL] & (MAC_TX | MAC_RX), 0);
                assert_eq!(io.registers[&WPDMA_GLO_CFG] & 5, 4);
                assert_eq!(io.rf[&(RF_MEMMAP | 29)], old_pll);
                drained = true;
                Ok(())
            })
            .unwrap();
            assert!(drained);
            for (reg, value) in [(35, r35), (33, r33), (32, r32), (31, r31), (29, n)] {
                assert_eq!(
                    io.rf[&(RF_MEMMAP | reg)],
                    value,
                    "channel {channel}, RF {reg}"
                );
            }
            assert_eq!(io.rf[&(RF_MEMMAP | 24)] & 3, divider);
            assert_eq!(io.registers[&MAC_SYS_CTRL] & (MAC_TX | MAC_RX), MAC_RX);
            assert_eq!(io.registers[&WPDMA_GLO_CFG] & 5, 4);
            assert_eq!(&io.calibrations[io.calibrations.len() - 2..], &[12, 2]);
            assert_eq!(
                &io.calibration_parameters[io.calibration_parameters.len() - 2..],
                &[u32::from(channel), 1]
            );
        }
    }

    #[test]
    fn invalid_channels_never_touch_hardware() {
        struct NoIo;
        impl RegisterIo for NoIo {
            fn read32(&mut self, _: u32) -> Result<u32> {
                panic!("unexpected read")
            }
            fn write32(&mut self, _: u32, _: u32) -> Result<()> {
                panic!("unexpected write")
            }
        }
        let cal = parse_calibration(&SimulatedDevice::new().eeprom).unwrap();
        for channel in [0, 12, 13, 14, 36, 255] {
            assert!(initialize_channel(&mut NoIo, channel).is_err());
            assert!(
                retune_with_drain(&mut NoIo, &cal, channel, |_| panic!("unexpected drain"))
                    .is_err()
            );
        }
    }

    #[test]
    fn drain_and_calibration_failures_close_all_gates() {
        let mut io = SimulatedDevice::new();
        let setup = initialize_channel_one(&mut io).unwrap();
        let error = retune_with_drain(&mut io, &setup.calibration, 6, |_| {
            anyhow::bail!("endpoint never became quiet")
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("endpoint never became quiet"));
        assert_eq!(io.rf[&(RF_MEMMAP | 29)], 0x28);
        assert_eq!(io.registers[&MAC_SYS_CTRL] & (MAC_TX | MAC_RX), 0);
        assert_eq!(io.registers[&WPDMA_GLO_CFG] & 5, 0);
        io.fail_calibration = true;
        let error = retune_with_drain(&mut io, &setup.calibration, 11, |_| Ok(())).unwrap_err();
        assert!(format!("{error:#}").contains("injected calibration failure"));
        assert_eq!(io.registers[&MAC_SYS_CTRL] & (MAC_TX | MAC_RX), 0);
        assert_eq!(io.registers[&WPDMA_GLO_CFG] & 5, 0);
        assert_eq!(io.registers[&AUTO_RSP_CFG], 0);
        assert_eq!(io.registers[&BEACON_TIME_CFG] & BEACON_TX, 0);
    }

    #[test]
    fn logical_efuse_reads_preserve_data_and_never_select_programming() {
        let mut io = SimulatedDevice::new();
        let read = read_eeprom(&mut io).unwrap();
        assert_eq!(read, io.eeprom);
        assert_eq!(io.efuse_triggers.len(), 32);
        let calibration = parse_calibration(&read).unwrap();
        assert_eq!(calibration.frequency_offset, 0x22);
        assert_eq!(calibration.lna_gain, 12);
        assert_eq!(calibration.rssi_offset, -2);
        io.eeprom[0xdb] = 3;
        assert_eq!(
            parse_calibration(&io.eeprom).unwrap().frequency_offset,
            0x28
        );
        io.eeprom[0xdb] = 0xff;
        io.eeprom[0x3a] = 0xff;
        io.eeprom[0x44] = 0xff;
        io.eeprom[0x46] = 32;
        let calibration = parse_calibration(&io.eeprom).unwrap();
        assert_eq!(
            (
                calibration.frequency_offset,
                calibration.lna_gain,
                calibration.rssi_offset
            ),
            (0, 0, 0)
        );
        io.eeprom[0x35] = 0x20;
        assert!(
            parse_calibration(&io.eeprom).is_err(),
            "5GHz-only hardware must not tune channel1"
        );
    }

    #[test]
    fn initialization_tunes_channel_one_without_enabling_transmit() {
        let mut io = SimulatedDevice::new();
        let setup = initialize_channel_one(&mut io).unwrap();
        assert_eq!(
            (setup.channel, setup.frequency_mhz, setup.bandwidth_mhz),
            (1, 2412, 20)
        );
        assert!(!setup.full_calibration_performed);
        assert_eq!(io.registers[&MAC_SYS_CTRL] & (MAC_TX | MAC_RX), MAC_RX);
        assert_eq!(io.registers[&WPDMA_GLO_CFG] & 5, 4);
        assert_eq!(io.rf[&(RF_MEMMAP | 29)], 0x28);
        assert_eq!(io.rf[&(RF_MEMMAP | 37)], 2);
        assert_eq!(io.rf[&(RF_MEMMAP | 22)], 0x22);
        assert_eq!(io.calibrations, [1, 12, 2]);
        disable(&mut io).unwrap();
        assert_eq!(io.registers[&MAC_SYS_CTRL] & (MAC_TX | MAC_RX), 0);
    }

    #[test]
    fn malformed_rf_response_or_unready_bbp_fails_closed() {
        let mut io = SimulatedDevice::new();
        io.bad_rf_response = true;
        assert!(read_rf(&mut io, 0, 22).is_err());
        assert!(write_rf(&mut io, 8, 0, 1).is_err());
        assert!(write_rf(&mut io, 0, 128, 1).is_err());
        io.bad_rf_response = false;
        io.bbp_ready = false;
        assert!(initialize_channel_one(&mut io).is_err());
        assert_eq!(io.registers[&MAC_SYS_CTRL] & (MAC_TX | MAC_RX), 0);
        assert!(io.calibrations.is_empty());
    }
    #[test]
    fn cleanup_attempts_every_gate_after_read_and_write_failures() {
        struct FaultyCleanup {
            reads: Vec<u32>,
            writes: Vec<(u32, u32)>,
        }
        impl RegisterIo for FaultyCleanup {
            fn read32(&mut self, reg: u32) -> Result<u32> {
                self.reads.push(reg);
                if reg == MAC_SYS_CTRL {
                    bail!("injected MAC read timeout");
                }
                Ok(if reg == BEACON_TIME_CFG {
                    BEACON_TX | (1 << 19)
                } else {
                    5
                })
            }
            fn write32(&mut self, reg: u32, value: u32) -> Result<()> {
                self.writes.push((reg, value));
                if reg == AUTO_RSP_CFG {
                    bail!("injected automatic-response write timeout");
                }
                Ok(())
            }
        }
        let mut io = FaultyCleanup {
            reads: Vec::new(),
            writes: Vec::new(),
        };
        let error = disable(&mut io).unwrap_err().to_string();
        assert!(error.contains("MAC TX/RX: injected MAC read timeout"));
        assert!(error.contains("automatic responses: injected automatic-response write timeout"));
        assert_eq!(io.reads, [MAC_SYS_CTRL, BEACON_TIME_CFG, WPDMA_GLO_CFG]);
        assert_eq!(
            io.writes,
            [(AUTO_RSP_CFG, 0), (BEACON_TIME_CFG, 0), (WPDMA_GLO_CFG, 0)]
        );
    }
    #[test]
    fn initialization_error_keeps_cleanup_failures_in_its_diagnostics() {
        struct FailingInit {
            inner: SimulatedDevice,
            mac_writes: usize,
        }
        impl RegisterIo for FailingInit {
            fn read32(&mut self, reg: u32) -> Result<u32> {
                self.inner.read32(reg)
            }
            fn write32(&mut self, reg: u32, value: u32) -> Result<()> {
                if reg == MAC_SYS_CTRL {
                    self.mac_writes += 1;
                    if self.mac_writes == 2 {
                        bail!("injected cleanup write failure");
                    }
                }
                self.inner.write32(reg, value)
            }
        }
        let mut io = FailingInit {
            inner: SimulatedDevice::new(),
            mac_writes: 0,
        };
        io.inner.eeprom[0] = 0;
        let error = format!("{:#}", initialize_channel_one(&mut io).unwrap_err());
        assert!(error.contains("expected MT7610 EEPROM"));
        assert!(error.contains("cleanup also failed"));
        assert!(error.contains("injected cleanup write failure"));
        assert_eq!(io.inner.registers[&AUTO_RSP_CFG], 0);
        assert_eq!(io.inner.registers[&WPDMA_GLO_CFG] & 5, 0);
    }
}
