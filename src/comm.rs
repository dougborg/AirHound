/// Communication layer — BLE GATT server and serial NDJSON transport.
///
/// The device streams filtered scan results as newline-delimited JSON
/// over both BLE notifications and serial. Commands can be received
/// from either transport.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver};

use trouble_host::prelude::*;

use crate::filter::FilterConfig;
use crate::protocol::{DeviceMessage, HostCommand, MsgBuffer, MAX_MSG_LEN};

/// Output channel for filtered scan results to be sent to companion
pub type OutputChannel = Channel<CriticalSectionRawMutex, MsgBuffer, 8>;
pub type OutputReceiver<'a> = Receiver<'a, CriticalSectionRawMutex, MsgBuffer, 8>;

/// BLE output channel — receives cloned messages from the serial output task
/// for forwarding as BLE GATT notifications.
pub type BleOutputChannel = Channel<CriticalSectionRawMutex, MsgBuffer, 4>;

/// Command channel for host commands received via BLE or serial
pub type CommandChannel = Channel<CriticalSectionRawMutex, HostCommand, 4>;

/// BLE GATT service UUIDs for AirHound.
///
/// These duplicate the string literals in the `#[gatt_service]` and `#[characteristic]`
/// proc macro attributes below — Rust proc macros require string literals, so we can't
/// reference these constants there. Kept here as the canonical source of truth.
#[allow(dead_code)]
pub mod ble_uuids {
    /// AirHound primary service UUID
    pub const SERVICE: &str = "4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d";
    /// TX characteristic — scan results, notify
    pub const TX_CHAR: &str = "4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d";
    /// RX characteristic — commands, write
    pub const RX_CHAR: &str = "4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d";
}

/// BLE advertising name
pub const BLE_ADV_NAME: &str = "AirHound";

/// Serial baud rate
pub const SERIAL_BAUD: u32 = 115200;

/// Maximum BLE notification payload (MTU-3)
pub const BLE_MAX_NOTIFY: usize = 20;

// ── GATT server definition (trouble-host proc macros) ──────────────────

/// AirHound BLE GATT service.
///
/// TX: scan result notifications (device → companion)
/// RX: host command writes (companion → device)
#[gatt_service(uuid = "4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d")]
pub struct AirHoundGattService {
    /// TX — filtered scan results, notify-only.
    /// Messages are chunked into BLE_MAX_NOTIFY-sized pieces.
    /// The companion accumulates until it sees '\n' (NDJSON delimiter).
    #[characteristic(uuid = "4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d", notify)]
    pub tx: [u8; 20],

    /// RX — host commands, write-only.
    /// Companion sends NDJSON commands which are accumulated via LineReader.
    #[characteristic(uuid = "4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d", write)]
    pub rx: [u8; 20],
}

/// Top-level AirHound GATT server.
#[gatt_server]
pub struct AirHoundServer {
    pub airhound_service: AirHoundGattService,
}

// ── Serialization helpers ──────────────────────────────────────────────

/// Serialize a DeviceMessage to JSON bytes and write to the output buffer.
/// Returns the number of bytes written, or None if serialization failed.
pub fn serialize_message(msg: &DeviceMessage, buf: &mut [u8]) -> Option<usize> {
    match serde_json_core::to_slice(msg, buf) {
        Ok(len) => {
            // Append newline for NDJSON
            if len < buf.len() {
                buf[len] = b'\n';
                Some(len + 1)
            } else {
                Some(len)
            }
        }
        Err(_) => None,
    }
}

/// Deserialize a HostCommand from a JSON byte slice.
pub fn parse_command(data: &[u8]) -> Option<HostCommand> {
    // Strip trailing newline/whitespace
    let trimmed = trim_trailing_whitespace(data);
    if trimmed.is_empty() {
        return None;
    }
    serde_json_core::from_slice::<HostCommand>(trimmed).ok().map(|(cmd, _)| cmd)
}

/// Process a received host command and update state accordingly.
pub fn handle_command(cmd: HostCommand, config: &mut FilterConfig, scanning: &mut bool) -> Option<DeviceMessage<'static>> {
    match cmd {
        HostCommand::Start => {
            *scanning = true;
            log::info!("Scanning started by host command");
            None
        }
        HostCommand::Stop => {
            *scanning = false;
            log::info!("Scanning stopped by host command");
            None
        }
        HostCommand::GetStatus => {
            // Status message will be constructed by the caller with real uptime/heap data
            None
        }
        HostCommand::SetRssi { min_rssi } => {
            config.min_rssi = min_rssi;
            log::info!("RSSI threshold set to {}", min_rssi);
            None
        }
    }
}

// ── Serial NDJSON reader ───────────────────────────────────────────────

/// Serial NDJSON reader state machine.
/// Accumulates bytes until a newline is found, then yields the line.
pub struct LineReader {
    buf: [u8; MAX_MSG_LEN],
    pos: usize,
}

impl LineReader {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_MSG_LEN],
            pos: 0,
        }
    }

    /// Feed a byte into the reader. Returns a complete line (without newline)
    /// when one is detected.
    pub fn feed(&mut self, byte: u8) -> Option<&[u8]> {
        if byte == b'\n' || byte == b'\r' {
            if self.pos > 0 {
                let line = &self.buf[..self.pos];
                self.pos = 0;
                Some(line)
            } else {
                None
            }
        } else if self.pos < self.buf.len() {
            self.buf[self.pos] = byte;
            self.pos += 1;
            None
        } else {
            // Overflow — discard and reset
            self.pos = 0;
            None
        }
    }
}

fn trim_trailing_whitespace(data: &[u8]) -> &[u8] {
    let mut end = data.len();
    while end > 0 && (data[end - 1] == b' ' || data[end - 1] == b'\n' || data[end - 1] == b'\r' || data[end - 1] == b'\t') {
        end -= 1;
    }
    &data[..end]
}
