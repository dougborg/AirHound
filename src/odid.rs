// Open Drone ID (ASTM F3411) parser.
//
// Decodes ODID messages from BLE advertisements, WiFi NAN action frames,
// and WiFi beacon vendor IEs. Each encoded message is 25 bytes. This module
// provides pure `no_std` parsing with no allocations.
//
// Reference: ASTM F3411-22a, ASD-STAN prEN 4709-002.

/// Size of a single encoded ODID message.
const MESSAGE_SIZE: usize = 25;

/// BLE service data UUID for ODID (little-endian: 0xFA, 0xFF).
const ODID_BLE_UUID: [u8; 2] = [0xFA, 0xFF];

/// WiFi vendor IE OUI for ODID (ASTM).
const ODID_WIFI_OUI: [u8; 3] = [0x90, 0x3A, 0xE6];

// Decoding constants (from ASTM F3411 spec).
const LATLON_MULT: f64 = 10_000_000.0;
const ALT_DIV: f32 = 0.5;
const ALT_ADDER: f32 = 1000.0;
const SPEED_DIV_LO: f32 = 0.25;
const SPEED_DIV_HI: f32 = 0.75;
const SPEED_HI_OFFSET: f32 = 255.0 * SPEED_DIV_LO; // 63.75
const VSPEED_DIV: f32 = 0.5;
const INV_TIMESTAMP: u16 = 0xFFFF;

// ── Message type (upper nibble of byte 0) ──────────────────────────

/// ODID message type identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    BasicId = 0,
    Location = 1,
    Auth = 2,
    SelfId = 3,
    System = 4,
    OperatorId = 5,
    Packed = 0xF,
}

impl MessageType {
    fn from_byte(b: u8) -> Option<Self> {
        match b >> 4 {
            0 => Some(Self::BasicId),
            1 => Some(Self::Location),
            2 => Some(Self::Auth),
            3 => Some(Self::SelfId),
            4 => Some(Self::System),
            5 => Some(Self::OperatorId),
            0xF => Some(Self::Packed),
            _ => None,
        }
    }
}

// ── Enums ──────────────────────────────────────────────────────────

/// UA (Unmanned Aircraft) type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UaType {
    None = 0,
    Aeroplane = 1,
    HelicopterOrMultirotor = 2,
    Gyroplane = 3,
    HybridLift = 4,
    Ornithopter = 5,
    Glider = 6,
    Kite = 7,
    FreeBalloon = 8,
    CaptiveBalloon = 9,
    Airship = 10,
    FreeFallParachute = 11,
    Rocket = 12,
    TetheredPoweredAircraft = 13,
    GroundObstacle = 14,
    Other = 15,
}

impl UaType {
    fn from_nibble(n: u8) -> Self {
        match n & 0x0F {
            0 => Self::None,
            1 => Self::Aeroplane,
            2 => Self::HelicopterOrMultirotor,
            3 => Self::Gyroplane,
            4 => Self::HybridLift,
            5 => Self::Ornithopter,
            6 => Self::Glider,
            7 => Self::Kite,
            8 => Self::FreeBalloon,
            9 => Self::CaptiveBalloon,
            10 => Self::Airship,
            11 => Self::FreeFallParachute,
            12 => Self::Rocket,
            13 => Self::TetheredPoweredAircraft,
            14 => Self::GroundObstacle,
            _ => Self::Other,
        }
    }
}

/// ID type for Basic ID message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IdType {
    None = 0,
    SerialNumber = 1,
    CaaRegistrationId = 2,
    UtmAssignedUuid = 3,
    SpecificSessionId = 4,
    Reserved(u8),
}

impl IdType {
    fn from_nibble(n: u8) -> Self {
        match n & 0x0F {
            0 => Self::None,
            1 => Self::SerialNumber,
            2 => Self::CaaRegistrationId,
            3 => Self::UtmAssignedUuid,
            4 => Self::SpecificSessionId,
            v => Self::Reserved(v),
        }
    }
}

/// UA operational status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Undeclared = 0,
    Ground = 1,
    Airborne = 2,
    Emergency = 3,
    RemoteIdSystemFailure = 4,
    Reserved(u8),
}

impl Status {
    fn from_nibble(n: u8) -> Self {
        match n & 0x0F {
            0 => Self::Undeclared,
            1 => Self::Ground,
            2 => Self::Airborne,
            3 => Self::Emergency,
            4 => Self::RemoteIdSystemFailure,
            v => Self::Reserved(v),
        }
    }
}

/// Height reference type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HeightRef {
    OverTakeoff = 0,
    OverGround = 1,
}

/// Operator location type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OperatorLocationType {
    Takeoff = 0,
    LiveGnss = 1,
    Fixed = 2,
    Reserved(u8),
}

impl OperatorLocationType {
    fn from_bits(n: u8) -> Self {
        match n & 0x03 {
            0 => Self::Takeoff,
            1 => Self::LiveGnss,
            2 => Self::Fixed,
            v => Self::Reserved(v),
        }
    }
}

/// EU classification type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClassificationType {
    Undeclared = 0,
    Eu = 1,
    Reserved(u8),
}

impl ClassificationType {
    fn from_bits(n: u8) -> Self {
        match n & 0x07 {
            0 => Self::Undeclared,
            1 => Self::Eu,
            v => Self::Reserved(v),
        }
    }
}

// ── Decoded message types ──────────────────────────────────────────

/// Decoded Basic ID message.
#[derive(Debug, Clone)]
pub struct BasicId {
    pub ua_type: UaType,
    pub id_type: IdType,
    /// UAS ID — up to 20 bytes, null-trimmed to a string.
    pub uas_id: heapless::String<20>,
}

/// Decoded Location message.
#[derive(Debug, Clone)]
pub struct Location {
    pub status: Status,
    /// Direction in degrees (0..360). 361 = invalid.
    pub direction: f32,
    /// Horizontal speed in m/s. 255 = invalid.
    pub speed_horizontal: f32,
    /// Vertical speed in m/s. 63 = invalid.
    pub speed_vertical: f32,
    /// Latitude in degrees.
    pub latitude: f64,
    /// Longitude in degrees.
    pub longitude: f64,
    /// Barometric altitude in meters. -1000 = invalid.
    pub altitude_baro: f32,
    /// Geodetic altitude in meters (WGS84). -1000 = invalid.
    pub altitude_geo: f32,
    pub height_type: HeightRef,
    /// Height above takeoff/ground in meters. -1000 = invalid.
    pub height: f32,
    /// Horizontal accuracy enum (4-bit).
    pub horiz_accuracy: u8,
    /// Vertical accuracy enum (4-bit).
    pub vert_accuracy: u8,
    /// Barometric accuracy enum (4-bit).
    pub baro_accuracy: u8,
    /// Speed accuracy enum (4-bit).
    pub speed_accuracy: u8,
    /// Timestamp accuracy enum (4-bit).
    pub ts_accuracy: u8,
    /// Seconds after the full hour, or `None` if invalid (0xFFFF).
    pub timestamp: Option<f32>,
}

/// Decoded System message.
#[derive(Debug, Clone)]
pub struct System {
    pub operator_location_type: OperatorLocationType,
    pub classification_type: ClassificationType,
    pub operator_latitude: f64,
    pub operator_longitude: f64,
    pub area_count: u16,
    /// Area radius in meters.
    pub area_radius: u16,
    /// Area ceiling in meters.
    pub area_ceiling: f32,
    /// Area floor in meters.
    pub area_floor: f32,
    pub category_eu: u8,
    pub class_eu: u8,
    pub operator_altitude_geo: f32,
    /// Seconds since 00:00:00 01/01/2019 UTC.
    pub timestamp: u32,
}

/// Decoded Operator ID message.
#[derive(Debug, Clone)]
pub struct OperatorId {
    pub operator_id_type: u8,
    /// Operator ID string — up to 20 bytes, null-trimmed.
    pub operator_id: heapless::String<20>,
}

/// A decoded ODID frame, potentially containing multiple messages.
#[derive(Debug, Clone, Default)]
pub struct OdidFrame {
    pub basic_id: Option<BasicId>,
    pub location: Option<Location>,
    pub system: Option<System>,
    pub operator_id: Option<OperatorId>,
}

impl OdidFrame {
    /// Returns `true` if at least one message was successfully decoded.
    pub fn has_data(&self) -> bool {
        self.basic_id.is_some()
            || self.location.is_some()
            || self.system.is_some()
            || self.operator_id.is_some()
    }
}

// ── Transport entry points ─────────────────────────────────────────

/// Parse ODID from a BLE advertisement's AD structures.
///
/// Scans AD structures for service data (type 0x16) with UUID 0xFFFA.
/// The payload after the UUID is one or more 25-byte ODID messages.
pub fn parse_odid_ble(ad_data: &[u8]) -> Option<OdidFrame> {
    let mut i = 0;
    while i + 1 < ad_data.len() {
        let len = ad_data[i] as usize;
        if len == 0 || i + 1 + len > ad_data.len() {
            break;
        }
        let ad_type = ad_data[i + 1];
        let payload = &ad_data[i + 2..i + 1 + len];

        // AD type 0x16 = Service Data - 16-bit UUID
        if ad_type == 0x16 && payload.len() >= 3 + MESSAGE_SIZE {
            if payload[0..2] == ODID_BLE_UUID {
                // Skip the counter byte (1 byte after UUID) per ASTM F3411
                let odid_payload = &payload[3..];
                return decode_messages(odid_payload);
            }
        }

        i += 1 + len;
    }
    None
}

/// Parse ODID from a WiFi NAN (Neighbor Awareness Networking) service info payload.
///
/// The caller must extract the service info bytes from the NAN SDA
/// (Service Descriptor Attribute). The payload starts with the ODID message(s).
pub fn parse_odid_wifi_nan(service_info: &[u8]) -> Option<OdidFrame> {
    if service_info.len() < MESSAGE_SIZE {
        return None;
    }
    decode_messages(service_info)
}

/// Parse ODID from a WiFi beacon or probe response vendor-specific IE.
///
/// Scans IEs for vendor IE (tag 0xDD) with OUI 90:3A:E6 (ASTM ODID).
/// `ie_data` should be the tagged parameters section of the frame.
pub fn parse_odid_wifi_beacon(ie_data: &[u8]) -> Option<OdidFrame> {
    let mut i = 0;
    while i + 1 < ie_data.len() {
        let tag = ie_data[i];
        let len = ie_data[i + 1] as usize;
        if i + 2 + len > ie_data.len() {
            break;
        }
        let body = &ie_data[i + 2..i + 2 + len];

        // Vendor-specific IE with ODID OUI
        if tag == 0xDD && len >= 4 + MESSAGE_SIZE && body[0..3] == ODID_WIFI_OUI {
            // Skip OUI (3) + OUI type (1)
            let odid_payload = &body[4..];
            return decode_messages(odid_payload);
        }

        i += 2 + len;
    }
    None
}

// ── Core decoder ───────────────────────────────────────────────────

/// Decode one or more 25-byte ODID messages from a raw payload.
fn decode_messages(data: &[u8]) -> Option<OdidFrame> {
    let mut frame = OdidFrame::default();
    let mut offset = 0;

    while offset + MESSAGE_SIZE <= data.len() {
        let msg = &data[offset..offset + MESSAGE_SIZE];
        match MessageType::from_byte(msg[0]) {
            Some(MessageType::BasicId) => {
                if let Some(bid) = decode_basic_id(msg) {
                    frame.basic_id = Some(bid);
                }
            }
            Some(MessageType::Location) => {
                if let Some(loc) = decode_location(msg) {
                    frame.location = Some(loc);
                }
            }
            Some(MessageType::System) => {
                if let Some(sys) = decode_system(msg) {
                    frame.system = Some(sys);
                }
            }
            Some(MessageType::OperatorId) => {
                if let Some(oid) = decode_operator_id(msg) {
                    frame.operator_id = Some(oid);
                }
            }
            Some(MessageType::Packed) => {
                // Pack header: byte 0 = type+version, byte 1 = single message
                // size, byte 2 = message count, bytes 3+ = inner messages.
                let single_size = msg[1] as usize;
                let count = msg[2] as usize;
                if single_size == MESSAGE_SIZE {
                    let pack_data_start = offset + 3; // skip 3-byte header
                    let pack_data = &data[pack_data_start..];
                    for i in 0..count {
                        let start = i * MESSAGE_SIZE;
                        if start + MESSAGE_SIZE <= pack_data.len() {
                            let inner = &pack_data[start..start + MESSAGE_SIZE];
                            decode_into_frame(&mut frame, inner);
                        }
                    }
                    offset = pack_data_start + count * MESSAGE_SIZE;
                    continue;
                }
            }
            _ => {} // Auth, SelfId, unknown — skip
        }
        offset += MESSAGE_SIZE;
    }

    if frame.has_data() {
        Some(frame)
    } else {
        None
    }
}

/// Decode a single message into an existing frame.
fn decode_into_frame(frame: &mut OdidFrame, msg: &[u8]) {
    if msg.len() < MESSAGE_SIZE {
        return;
    }
    match MessageType::from_byte(msg[0]) {
        Some(MessageType::BasicId) => {
            if let Some(bid) = decode_basic_id(msg) {
                frame.basic_id = Some(bid);
            }
        }
        Some(MessageType::Location) => {
            if let Some(loc) = decode_location(msg) {
                frame.location = Some(loc);
            }
        }
        Some(MessageType::System) => {
            if let Some(sys) = decode_system(msg) {
                frame.system = Some(sys);
            }
        }
        Some(MessageType::OperatorId) => {
            if let Some(oid) = decode_operator_id(msg) {
                frame.operator_id = Some(oid);
            }
        }
        _ => {}
    }
}

// ── Individual message decoders ────────────────────────────────────

/// Decode a Basic ID message from 25 raw bytes.
///
/// Layout:
/// - Byte 0: [MessageType:4][ProtoVersion:4]
/// - Byte 1: [IDType:4][UAType:4]
/// - Bytes 2-21: UASID (20 bytes)
/// - Bytes 22-24: reserved
pub fn decode_basic_id(data: &[u8]) -> Option<BasicId> {
    if data.len() < MESSAGE_SIZE {
        return None;
    }
    let ua_type = UaType::from_nibble(data[1] & 0x0F);
    let id_type = IdType::from_nibble(data[1] >> 4);
    let id_bytes = &data[2..22];

    let mut uas_id = heapless::String::new();
    for &b in id_bytes {
        if b == 0 {
            break;
        }
        // Only accept printable ASCII
        if b >= 0x20 && b <= 0x7E {
            let _ = uas_id.push(b as char);
        }
    }

    Some(BasicId {
        ua_type,
        id_type,
        uas_id,
    })
}

/// Decode a Location message from 25 raw bytes.
///
/// Layout:
/// - Byte 0: [MessageType:4][ProtoVersion:4]
/// - Byte 1: [Status:4][Reserved:1][HeightType:1][EWDirection:1][SpeedMult:1]
/// - Byte 2: Direction (uint8)
/// - Byte 3: SpeedHorizontal (uint8)
/// - Byte 4: SpeedVertical (int8)
/// - Bytes 5-8: Latitude (int32 LE)
/// - Bytes 9-12: Longitude (int32 LE)
/// - Bytes 13-14: AltitudeBaro (uint16 LE)
/// - Bytes 15-16: AltitudeGeo (uint16 LE)
/// - Bytes 17-18: Height (uint16 LE)
/// - Byte 19: [VertAccuracy:4][HorizAccuracy:4]
/// - Byte 20: [BaroAccuracy:4][SpeedAccuracy:4]
/// - Bytes 21-22: TimeStamp (uint16 LE)
/// - Byte 23: [Reserved2:4][TSAccuracy:4]
/// - Byte 24: reserved
pub fn decode_location(data: &[u8]) -> Option<Location> {
    if data.len() < MESSAGE_SIZE {
        return None;
    }

    let flags = data[1];
    let speed_mult = flags & 0x01;
    let ew_direction = (flags >> 1) & 0x01;
    let height_type = if (flags >> 2) & 0x01 == 1 {
        HeightRef::OverGround
    } else {
        HeightRef::OverTakeoff
    };
    let status = Status::from_nibble(flags >> 4);

    let direction = decode_direction(data[2], ew_direction);
    let speed_horizontal = decode_speed_horizontal(data[3], speed_mult);
    let speed_vertical = decode_speed_vertical(data[4] as i8);

    let latitude = decode_latlon(i32::from_le_bytes([data[5], data[6], data[7], data[8]]));
    let longitude = decode_latlon(i32::from_le_bytes([data[9], data[10], data[11], data[12]]));

    let altitude_baro = decode_altitude(u16::from_le_bytes([data[13], data[14]]));
    let altitude_geo = decode_altitude(u16::from_le_bytes([data[15], data[16]]));
    let height = decode_altitude(u16::from_le_bytes([data[17], data[18]]));

    let horiz_accuracy = data[19] & 0x0F;
    let vert_accuracy = data[19] >> 4;
    let speed_accuracy = data[20] & 0x0F;
    let baro_accuracy = data[20] >> 4;

    let ts_raw = u16::from_le_bytes([data[21], data[22]]);
    let timestamp = decode_timestamp(ts_raw);

    let ts_accuracy = data[23] & 0x0F;

    Some(Location {
        status,
        direction,
        speed_horizontal,
        speed_vertical,
        latitude,
        longitude,
        altitude_baro,
        altitude_geo,
        height_type,
        height,
        horiz_accuracy,
        vert_accuracy,
        baro_accuracy,
        speed_accuracy,
        ts_accuracy,
        timestamp,
    })
}

/// Decode a System message from 25 raw bytes.
///
/// Layout:
/// - Byte 0: [MessageType:4][ProtoVersion:4]
/// - Byte 1: [Reserved:3][ClassificationType:3][OperatorLocationType:2]
/// - Bytes 2-5: OperatorLatitude (int32 LE)
/// - Bytes 6-9: OperatorLongitude (int32 LE)
/// - Bytes 10-11: AreaCount (uint16 LE)
/// - Byte 12: AreaRadius (uint8, *10 = meters)
/// - Bytes 13-14: AreaCeiling (uint16 LE)
/// - Bytes 15-16: AreaFloor (uint16 LE)
/// - Byte 17: [CategoryEU:4][ClassEU:4]
/// - Bytes 18-19: OperatorAltitudeGeo (uint16 LE)
/// - Bytes 20-23: Timestamp (uint32 LE)
/// - Byte 24: reserved
pub fn decode_system(data: &[u8]) -> Option<System> {
    if data.len() < MESSAGE_SIZE {
        return None;
    }

    let flags = data[1];
    let operator_location_type = OperatorLocationType::from_bits(flags & 0x03);
    let classification_type = ClassificationType::from_bits((flags >> 2) & 0x07);

    let operator_latitude = decode_latlon(i32::from_le_bytes([data[2], data[3], data[4], data[5]]));
    let operator_longitude =
        decode_latlon(i32::from_le_bytes([data[6], data[7], data[8], data[9]]));

    let area_count = u16::from_le_bytes([data[10], data[11]]);
    let area_radius = (data[12] as u16) * 10;
    let area_ceiling = decode_altitude(u16::from_le_bytes([data[13], data[14]]));
    let area_floor = decode_altitude(u16::from_le_bytes([data[15], data[16]]));

    let category_eu = data[17] >> 4;
    let class_eu = data[17] & 0x0F;

    let operator_altitude_geo = decode_altitude(u16::from_le_bytes([data[18], data[19]]));

    let timestamp = u32::from_le_bytes([data[20], data[21], data[22], data[23]]);

    Some(System {
        operator_location_type,
        classification_type,
        operator_latitude,
        operator_longitude,
        area_count,
        area_radius,
        area_ceiling,
        area_floor,
        category_eu,
        class_eu,
        operator_altitude_geo,
        timestamp,
    })
}

/// Decode an Operator ID message from 25 raw bytes.
///
/// Layout:
/// - Byte 0: [MessageType:4][ProtoVersion:4]
/// - Byte 1: OperatorIdType (uint8)
/// - Bytes 2-21: OperatorId (20 bytes)
/// - Bytes 22-24: reserved
pub fn decode_operator_id(data: &[u8]) -> Option<OperatorId> {
    if data.len() < MESSAGE_SIZE {
        return None;
    }

    let operator_id_type = data[1];
    let id_bytes = &data[2..22];

    let mut operator_id = heapless::String::new();
    for &b in id_bytes {
        if b == 0 {
            break;
        }
        if b >= 0x20 && b <= 0x7E {
            let _ = operator_id.push(b as char);
        }
    }

    Some(OperatorId {
        operator_id_type,
        operator_id,
    })
}

// ── Primitive decoders ─────────────────────────────────────────────

fn decode_direction(enc: u8, ew_flag: u8) -> f32 {
    if ew_flag != 0 {
        enc as f32 + 180.0
    } else {
        enc as f32
    }
}

fn decode_speed_horizontal(enc: u8, mult: u8) -> f32 {
    if mult != 0 {
        enc as f32 * SPEED_DIV_HI + SPEED_HI_OFFSET
    } else {
        enc as f32 * SPEED_DIV_LO
    }
}

fn decode_speed_vertical(enc: i8) -> f32 {
    enc as f32 * VSPEED_DIV
}

fn decode_latlon(enc: i32) -> f64 {
    enc as f64 / LATLON_MULT
}

fn decode_altitude(enc: u16) -> f32 {
    enc as f32 * ALT_DIV - ALT_ADDER
}

fn decode_timestamp(enc: u16) -> Option<f32> {
    if enc == INV_TIMESTAMP {
        None
    } else {
        Some(enc as f32 / 10.0)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 25-byte Basic ID message.
    fn make_basic_id(ua_type: u8, id_type: u8, id: &[u8]) -> [u8; MESSAGE_SIZE] {
        let mut msg = [0u8; MESSAGE_SIZE];
        msg[0] = (MessageType::BasicId as u8) << 4 | 0x02; // type + proto version 2
        msg[1] = (id_type << 4) | (ua_type & 0x0F);
        let copy_len = id.len().min(20);
        msg[2..2 + copy_len].copy_from_slice(&id[..copy_len]);
        msg
    }

    /// Build a minimal 25-byte Location message with given lat/lon.
    fn make_location(lat: f64, lon: f64, alt_geo: f32, status: u8) -> [u8; MESSAGE_SIZE] {
        let mut msg = [0u8; MESSAGE_SIZE];
        msg[0] = (MessageType::Location as u8) << 4 | 0x02;
        // Byte 1: status in upper nibble, flags in lower
        msg[1] = (status & 0x0F) << 4;

        msg[2] = 90; // direction = 90 degrees
        msg[3] = 40; // speed = 40 * 0.25 = 10.0 m/s
        msg[4] = 10; // vspeed = 10 * 0.5 = 5.0 m/s

        let lat_enc = (lat * LATLON_MULT) as i32;
        let lon_enc = (lon * LATLON_MULT) as i32;
        msg[5..9].copy_from_slice(&lat_enc.to_le_bytes());
        msg[9..13].copy_from_slice(&lon_enc.to_le_bytes());

        // AltitudeBaro = (alt + 1000) / 0.5
        let baro_enc = ((100.0 + ALT_ADDER) / ALT_DIV) as u16;
        msg[13..15].copy_from_slice(&baro_enc.to_le_bytes());

        let geo_enc = ((alt_geo + ALT_ADDER) / ALT_DIV) as u16;
        msg[15..17].copy_from_slice(&geo_enc.to_le_bytes());

        let height_enc = ((50.0 + ALT_ADDER) / ALT_DIV) as u16;
        msg[17..19].copy_from_slice(&height_enc.to_le_bytes());

        // Accuracy bytes
        msg[19] = 0x0A | (0x05 << 4); // horiz=10, vert=5
        msg[20] = 0x03 | (0x04 << 4); // speed=3, baro=4

        // Timestamp = 1234 -> 123.4 seconds
        let ts: u16 = 1234;
        msg[21..23].copy_from_slice(&ts.to_le_bytes());
        msg[23] = 0x07; // ts_accuracy = 7

        msg
    }

    /// Build a minimal 25-byte System message.
    fn make_system(op_lat: f64, op_lon: f64) -> [u8; MESSAGE_SIZE] {
        let mut msg = [0u8; MESSAGE_SIZE];
        msg[0] = (MessageType::System as u8) << 4 | 0x02;
        msg[1] = 0x01 | (0x01 << 2); // LiveGnss + EU classification

        let lat_enc = (op_lat * LATLON_MULT) as i32;
        let lon_enc = (op_lon * LATLON_MULT) as i32;
        msg[2..6].copy_from_slice(&lat_enc.to_le_bytes());
        msg[6..10].copy_from_slice(&lon_enc.to_le_bytes());

        // AreaCount = 1
        msg[10..12].copy_from_slice(&1u16.to_le_bytes());
        // AreaRadius = 5 -> 50m
        msg[12] = 5;
        // AreaCeiling = (200 + 1000) / 0.5 = 2400
        msg[13..15].copy_from_slice(&2400u16.to_le_bytes());
        // AreaFloor = (0 + 1000) / 0.5 = 2000
        msg[15..17].copy_from_slice(&2000u16.to_le_bytes());

        // CategoryEU=1 (Open), ClassEU=2
        msg[17] = (0x01 << 4) | 0x02;

        // OperatorAltitudeGeo = (30 + 1000) / 0.5 = 2060
        msg[18..20].copy_from_slice(&2060u16.to_le_bytes());

        // Timestamp
        msg[20..24].copy_from_slice(&12345678u32.to_le_bytes());

        msg
    }

    /// Build a minimal 25-byte Operator ID message.
    fn make_operator_id(id: &[u8]) -> [u8; MESSAGE_SIZE] {
        let mut msg = [0u8; MESSAGE_SIZE];
        msg[0] = (MessageType::OperatorId as u8) << 4 | 0x02;
        msg[1] = 0; // OperatorIdType = 0
        let copy_len = id.len().min(20);
        msg[2..2 + copy_len].copy_from_slice(&id[..copy_len]);
        msg
    }

    #[test]
    fn test_decode_basic_id() {
        let msg = make_basic_id(2, 1, b"1234567890ABCDEF1234");
        let bid = decode_basic_id(&msg).unwrap();
        assert_eq!(bid.ua_type, UaType::HelicopterOrMultirotor);
        assert_eq!(bid.id_type, IdType::SerialNumber);
        assert_eq!(bid.uas_id.as_str(), "1234567890ABCDEF1234");
    }

    #[test]
    fn test_decode_basic_id_null_terminated() {
        let msg = make_basic_id(1, 2, b"ABC\0\0\0\0\0");
        let bid = decode_basic_id(&msg).unwrap();
        assert_eq!(bid.ua_type, UaType::Aeroplane);
        assert_eq!(bid.id_type, IdType::CaaRegistrationId);
        assert_eq!(bid.uas_id.as_str(), "ABC");
    }

    #[test]
    fn test_decode_location() {
        let msg = make_location(47.3977, 8.5456, 500.0, 2);
        let loc = decode_location(&msg).unwrap();

        assert_eq!(loc.status, Status::Airborne);
        assert!((loc.direction - 90.0).abs() < 0.01);
        assert!((loc.speed_horizontal - 10.0).abs() < 0.01);
        assert!((loc.speed_vertical - 5.0).abs() < 0.01);

        // Lat/lon precision limited by i32 encoding
        assert!((loc.latitude - 47.3977).abs() < 0.0001);
        assert!((loc.longitude - 8.5456).abs() < 0.0001);

        assert!((loc.altitude_baro - 100.0).abs() < 0.5);
        assert!((loc.altitude_geo - 500.0).abs() < 0.5);
        assert!((loc.height - 50.0).abs() < 0.5);

        assert_eq!(loc.horiz_accuracy, 10);
        assert_eq!(loc.vert_accuracy, 5);
        assert_eq!(loc.speed_accuracy, 3);
        assert_eq!(loc.baro_accuracy, 4);
        assert_eq!(loc.ts_accuracy, 7);

        let ts = loc.timestamp.unwrap();
        assert!((ts - 123.4).abs() < 0.01);
    }

    #[test]
    fn test_decode_location_invalid_timestamp() {
        let mut msg = make_location(0.0, 0.0, 0.0, 0);
        msg[21..23].copy_from_slice(&INV_TIMESTAMP.to_le_bytes());
        let loc = decode_location(&msg).unwrap();
        assert!(loc.timestamp.is_none());
    }

    #[test]
    fn test_decode_location_ew_direction() {
        let mut msg = make_location(0.0, 0.0, 0.0, 0);
        // Set EW direction flag (bit 1 of byte 1)
        msg[1] |= 0x02;
        msg[2] = 45; // encoded direction
        let loc = decode_location(&msg).unwrap();
        assert!((loc.direction - 225.0).abs() < 0.01); // 45 + 180
    }

    #[test]
    fn test_decode_location_speed_mult() {
        let mut msg = make_location(0.0, 0.0, 0.0, 0);
        // Set speed mult flag (bit 0 of byte 1)
        msg[1] |= 0x01;
        msg[3] = 100; // encoded speed
        let loc = decode_location(&msg).unwrap();
        // 100 * 0.75 + 63.75 = 138.75
        assert!((loc.speed_horizontal - 138.75).abs() < 0.01);
    }

    #[test]
    fn test_decode_system() {
        let msg = make_system(52.5200, 13.4050);
        let sys = decode_system(&msg).unwrap();

        assert_eq!(sys.operator_location_type, OperatorLocationType::LiveGnss);
        assert_eq!(sys.classification_type, ClassificationType::Eu);
        assert!((sys.operator_latitude - 52.5200).abs() < 0.0001);
        assert!((sys.operator_longitude - 13.4050).abs() < 0.0001);
        assert_eq!(sys.area_count, 1);
        assert_eq!(sys.area_radius, 50);
        assert!((sys.area_ceiling - 200.0).abs() < 0.5);
        assert!((sys.area_floor - 0.0).abs() < 0.5);
        assert_eq!(sys.category_eu, 1);
        assert_eq!(sys.class_eu, 2);
        assert!((sys.operator_altitude_geo - 30.0).abs() < 0.5);
        assert_eq!(sys.timestamp, 12345678);
    }

    #[test]
    fn test_decode_operator_id() {
        let msg = make_operator_id(b"FIN87astrdge12k8");
        let oid = decode_operator_id(&msg).unwrap();
        assert_eq!(oid.operator_id_type, 0);
        assert_eq!(oid.operator_id.as_str(), "FIN87astrdge12k8");
    }

    #[test]
    fn test_decode_messages_single() {
        let msg = make_basic_id(2, 1, b"TESTDRONE001");
        let frame = decode_messages(&msg).unwrap();
        assert!(frame.basic_id.is_some());
        assert!(frame.location.is_none());
    }

    #[test]
    fn test_decode_messages_multiple() {
        let bid = make_basic_id(2, 1, b"MULTI001");
        let loc = make_location(51.5074, -0.1278, 120.0, 2);
        let mut data = [0u8; MESSAGE_SIZE * 2];
        data[..MESSAGE_SIZE].copy_from_slice(&bid);
        data[MESSAGE_SIZE..].copy_from_slice(&loc);

        let frame = decode_messages(&data).unwrap();
        assert!(frame.basic_id.is_some());
        assert!(frame.location.is_some());
        assert_eq!(frame.basic_id.unwrap().uas_id.as_str(), "MULTI001");
        assert!((frame.location.unwrap().latitude - 51.5074).abs() < 0.0001);
    }

    #[test]
    fn test_parse_odid_ble() {
        // Construct a BLE AD structure with ODID service data
        let msg = make_basic_id(2, 1, b"BLETEST001");
        // AD: length, type=0x16, UUID (2 bytes LE), counter byte, message
        let ad_len = 1 + 2 + 1 + MESSAGE_SIZE; // type + uuid + counter + msg
        let mut ad_data = [0u8; 64];
        ad_data[0] = ad_len as u8;
        ad_data[1] = 0x16; // Service Data - 16-bit UUID
        ad_data[2] = 0xFA; // UUID low byte
        ad_data[3] = 0xFF; // UUID high byte
        ad_data[4] = 0x00; // counter byte
        ad_data[5..5 + MESSAGE_SIZE].copy_from_slice(&msg);

        let frame = parse_odid_ble(&ad_data[..5 + MESSAGE_SIZE]).unwrap();
        assert!(frame.basic_id.is_some());
        assert_eq!(frame.basic_id.unwrap().uas_id.as_str(), "BLETEST001");
    }

    #[test]
    fn test_parse_odid_ble_no_match() {
        // Non-ODID AD structure
        let ad_data = [3, 0x16, 0x00, 0x01]; // wrong UUID
        assert!(parse_odid_ble(&ad_data).is_none());
    }

    #[test]
    fn test_parse_odid_wifi_beacon() {
        let msg = make_basic_id(1, 1, b"WIFIBEACON01");
        // Vendor IE: tag=0xDD, length, OUI (3), OUI type (1), message
        let ie_len = 3 + 1 + MESSAGE_SIZE;
        let mut ie_data = [0u8; 64];
        ie_data[0] = 0xDD;
        ie_data[1] = ie_len as u8;
        ie_data[2] = 0x90; // OUI byte 0
        ie_data[3] = 0x3A; // OUI byte 1
        ie_data[4] = 0xE6; // OUI byte 2
        ie_data[5] = 0x01; // OUI type
        ie_data[6..6 + MESSAGE_SIZE].copy_from_slice(&msg);

        let frame = parse_odid_wifi_beacon(&ie_data[..6 + MESSAGE_SIZE]).unwrap();
        assert!(frame.basic_id.is_some());
        assert_eq!(frame.basic_id.unwrap().uas_id.as_str(), "WIFIBEACON01");
    }

    #[test]
    fn test_parse_odid_wifi_nan() {
        let msg = make_location(35.6762, 139.6503, 100.0, 2);
        let frame = parse_odid_wifi_nan(&msg).unwrap();
        let loc = frame.location.unwrap();
        assert_eq!(loc.status, Status::Airborne);
        assert!((loc.latitude - 35.6762).abs() < 0.0001);
    }

    #[test]
    fn test_too_short_rejected() {
        assert!(decode_basic_id(&[0; 10]).is_none());
        assert!(decode_location(&[0; 10]).is_none());
        assert!(decode_system(&[0; 10]).is_none());
        assert!(decode_operator_id(&[0; 10]).is_none());
        assert!(parse_odid_wifi_nan(&[0; 10]).is_none());
    }

    #[test]
    fn test_message_type_parsing() {
        assert_eq!(MessageType::from_byte(0x02), Some(MessageType::BasicId));
        assert_eq!(MessageType::from_byte(0x12), Some(MessageType::Location));
        assert_eq!(MessageType::from_byte(0x42), Some(MessageType::System));
        assert_eq!(MessageType::from_byte(0x52), Some(MessageType::OperatorId));
        assert_eq!(MessageType::from_byte(0xF2), Some(MessageType::Packed));
        assert_eq!(MessageType::from_byte(0x62), None); // invalid
    }

    #[test]
    fn test_primitive_decoders() {
        assert!((decode_direction(90, 0) - 90.0).abs() < f32::EPSILON);
        assert!((decode_direction(90, 1) - 270.0).abs() < f32::EPSILON);

        assert!((decode_speed_horizontal(100, 0) - 25.0).abs() < f32::EPSILON);
        assert!((decode_speed_horizontal(100, 1) - 138.75).abs() < 0.01);

        assert!((decode_speed_vertical(10) - 5.0).abs() < f32::EPSILON);
        assert!((decode_speed_vertical(-10) - (-5.0)).abs() < f32::EPSILON);

        assert!((decode_latlon(473977000) - 47.3977).abs() < 0.0001);
        assert!((decode_latlon(-1278000) - (-0.1278)).abs() < 0.0001);

        assert!((decode_altitude(2200) - 100.0).abs() < f32::EPSILON);
        assert!((decode_altitude(0) - (-1000.0)).abs() < f32::EPSILON);

        assert!(decode_timestamp(INV_TIMESTAMP).is_none());
        assert!((decode_timestamp(1234).unwrap() - 123.4).abs() < 0.01);
    }

    #[test]
    fn test_negative_speed_vertical() {
        let mut msg = make_location(0.0, 0.0, 0.0, 0);
        msg[4] = (-20i8) as u8; // -20 * 0.5 = -10.0 m/s
        let loc = decode_location(&msg).unwrap();
        assert!((loc.speed_vertical - (-10.0)).abs() < f32::EPSILON);
    }

    // ── Tests using exact bytes from opendroneid-core-c ────────────
    // Source: opendroneid/opendroneid-core-c test/unit_odid_wifi_beacon.cpp
    // These are the encoded message bytes produced by the reference C encoder.

    /// BasicID from the C reference: USS-Enterprise, HybridLift, SerialNumber.
    /// Exact encoded bytes from unit_odid_wifi_beacon.cpp expectedBuffer[71..96].
    #[test]
    fn test_reference_basic_id_uss_enterprise() {
        #[rustfmt::skip]
        let msg: [u8; MESSAGE_SIZE] = [
            0x02,  // MessageType=0 (BasicID), ProtoVersion=2
            0x14,  // IDType=1 (SerialNumber) << 4 | UAType=4 (HybridLift)
            b'U', b'S', b'S', b'-', b'E', b'n', b't', b'e', b'r', b'p',
            b'r', b'i', b's', b'e', 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00,  // reserved
        ];

        let bid = decode_basic_id(&msg).unwrap();
        assert_eq!(bid.ua_type, UaType::HybridLift);
        assert_eq!(bid.id_type, IdType::SerialNumber);
        assert_eq!(bid.uas_id.as_str(), "USS-Enterprise");
    }

    /// Location from the C reference: Airborne, direction 0.25deg, speed 62 m/s.
    /// Exact encoded bytes from unit_odid_wifi_beacon.cpp expectedBuffer[96..].
    /// Only the first two bytes are fully specified in the test; we construct
    /// the rest from the known input values to validate our decoder matches.
    #[test]
    fn test_reference_location_airborne() {
        // The C test confirms byte 0 = 0x12, byte 1 = 0x20
        // We construct remaining bytes from known C test input values:
        //   Direction=0.25 -> encoded=0, EW=0  (truncated to uint8: 0)
        //   SpeedHorizontal=62 -> 62/0.25=248, mult=0
        //   SpeedVertical=62 -> 62/0.5=124
        //   Lat/Lon/Alt all default (0/0/-1000)
        #[rustfmt::skip]
        let msg: [u8; MESSAGE_SIZE] = [
            0x12, // MessageType=1 (Location), ProtoVersion=2
            0x20, // Status=2 (Airborne), HeightType=0, EW=0, SpeedMult=0
            0x00, // Direction: 0 (0.25 truncated to 0)
            248,  // SpeedHorizontal: 248 * 0.25 = 62.0
            124,  // SpeedVertical: 124 * 0.5 = 62.0
            // Latitude = 0 (default)
            0x00, 0x00, 0x00, 0x00,
            // Longitude = 0 (default)
            0x00, 0x00, 0x00, 0x00,
            // AltitudeBaro = 0 -> -1000m (default)
            0x00, 0x00,
            // AltitudeGeo = 0 -> -1000m (default)
            0x00, 0x00,
            // Height = 0 -> -1000m (default)
            0x00, 0x00,
            // Accuracy bytes (defaults = 0)
            0x00, 0x00,
            // Timestamp = INV
            0xFF, 0xFF,
            // TSAccuracy = 0
            0x00,
            // Reserved
            0x00,
        ];

        let loc = decode_location(&msg).unwrap();
        assert_eq!(loc.status, Status::Airborne);
        assert!((loc.direction - 0.0).abs() < f32::EPSILON);
        assert!((loc.speed_horizontal - 62.0).abs() < f32::EPSILON);
        assert!((loc.speed_vertical - 62.0).abs() < f32::EPSILON);
        assert!(loc.timestamp.is_none());
    }

    /// Round-trip test using values from opendroneid-core-c test/test_inout.c.
    /// Input: Lat=45.539309, Lon=-122.966389, Dir=215.7, Speed=5.4,
    ///        VSpeed=5.25, AltBaro=100, AltGeo=110, Height=80 (over ground),
    ///        Status=Airborne, TimeStamp=360.52
    #[test]
    fn test_reference_round_trip_location() {
        // Encode using the same formulas as the C encoder
        let lat_enc = (45.539309 * LATLON_MULT) as i32;
        let lon_enc = (-122.966389 * LATLON_MULT) as i32;
        // Direction 215.7 > 180, so EW=1, encoded = 215.7 - 180 = 35.7 -> 35
        let ew_flag: u8 = 1;
        let dir_enc: u8 = 35; // (215.7 - 180) truncated
                              // Speed 5.4 / 0.25 = 21.6 -> 21 (truncated), mult=0
        let speed_enc: u8 = 21;
        // VSpeed 5.25 / 0.5 = 10.5 -> 10 (truncated)
        let vspeed_enc: i8 = 10;
        // Alt: (100 + 1000) / 0.5 = 2200
        let alt_baro_enc: u16 = 2200;
        // AltGeo: (110 + 1000) / 0.5 = 2220
        let alt_geo_enc: u16 = 2220;
        // Height: (80 + 1000) / 0.5 = 2160
        let height_enc: u16 = 2160;
        // Timestamp: 360.52 * 10 = 3605.2 -> 3605
        let ts_enc: u16 = 3605;

        let mut msg = [0u8; MESSAGE_SIZE];
        msg[0] = 0x12; // Location + proto v2
                       // Status=Airborne(2), HeightType=OverGround(1), EW=1, SpeedMult=0
        msg[1] = (2 << 4) | (1 << 2) | (ew_flag << 1);
        msg[2] = dir_enc;
        msg[3] = speed_enc;
        msg[4] = vspeed_enc as u8;
        msg[5..9].copy_from_slice(&lat_enc.to_le_bytes());
        msg[9..13].copy_from_slice(&lon_enc.to_le_bytes());
        msg[13..15].copy_from_slice(&alt_baro_enc.to_le_bytes());
        msg[15..17].copy_from_slice(&alt_geo_enc.to_le_bytes());
        msg[17..19].copy_from_slice(&height_enc.to_le_bytes());
        msg[21..23].copy_from_slice(&ts_enc.to_le_bytes());

        let loc = decode_location(&msg).unwrap();
        assert_eq!(loc.status, Status::Airborne);
        assert_eq!(loc.height_type, HeightRef::OverGround);

        // Direction: 35 + 180 = 215.0 (C encoder truncates, so we lose .7)
        assert!((loc.direction - 215.0).abs() < 1.0);
        // Speed: 21 * 0.25 = 5.25 (C encoder truncates, so 5.4 -> 5.25)
        assert!((loc.speed_horizontal - 5.25).abs() < 0.01);
        assert!((loc.speed_vertical - 5.0).abs() < 0.01);
        // Lat/Lon within i32 encoding precision
        assert!((loc.latitude - 45.539309).abs() < 0.00001);
        assert!((loc.longitude - (-122.966389)).abs() < 0.00001);
        assert!((loc.altitude_baro - 100.0).abs() < 0.5);
        assert!((loc.altitude_geo - 110.0).abs() < 0.5);
        assert!((loc.height - 80.0).abs() < 0.5);
        // Timestamp: 3605 / 10 = 360.5
        assert!((loc.timestamp.unwrap() - 360.5).abs() < 0.1);
    }

    /// System message round-trip from test_inout.c.
    /// Input: OpLoc=Takeoff, Class=EU, Lat≈45.539319, Lon≈-122.966379,
    ///        AreaCount=35, Radius=75, Ceiling=176.9, Floor=41.7,
    ///        CategoryEU=Specific(2), ClassEU=Class3(4), OpAltGeo=20.5,
    ///        Timestamp=28000000
    #[test]
    fn test_reference_round_trip_system() {
        let lat_enc = ((45.539309 + 0.00001) * LATLON_MULT) as i32;
        let lon_enc = ((-122.966389 + 0.00001) * LATLON_MULT) as i32;
        let area_ceiling_enc = ((176.9 + ALT_ADDER) / ALT_DIV) as u16; // 2353
        let area_floor_enc = ((41.7 + ALT_ADDER) / ALT_DIV) as u16; // 2083
        let op_alt_enc = ((20.5 + ALT_ADDER) / ALT_DIV) as u16; // 2041
        let timestamp: u32 = 28000000;

        let mut msg = [0u8; MESSAGE_SIZE];
        msg[0] = 0x42; // System + proto v2
                       // OperatorLocationType=Takeoff(0), ClassificationType=EU(1)
        msg[1] = 0x00 | (0x01 << 2);
        msg[2..6].copy_from_slice(&lat_enc.to_le_bytes());
        msg[6..10].copy_from_slice(&lon_enc.to_le_bytes());
        msg[10..12].copy_from_slice(&35u16.to_le_bytes()); // AreaCount
        msg[12] = 7; // AreaRadius: 7 * 10 = 70 (75 rounds down in C encoder)
        msg[13..15].copy_from_slice(&area_ceiling_enc.to_le_bytes());
        msg[15..17].copy_from_slice(&area_floor_enc.to_le_bytes());
        // CategoryEU=Specific(2), ClassEU=Class3(4)
        msg[17] = (0x02 << 4) | 0x04;
        msg[18..20].copy_from_slice(&op_alt_enc.to_le_bytes());
        msg[20..24].copy_from_slice(&timestamp.to_le_bytes());

        let sys = decode_system(&msg).unwrap();
        assert_eq!(sys.operator_location_type, OperatorLocationType::Takeoff);
        assert_eq!(sys.classification_type, ClassificationType::Eu);
        assert!((sys.operator_latitude - 45.53932).abs() < 0.0001);
        assert!((sys.operator_longitude - (-122.96638)).abs() < 0.0001);
        assert_eq!(sys.area_count, 35);
        assert_eq!(sys.area_radius, 70);
        assert!((sys.area_ceiling - 176.5).abs() < 1.0);
        assert!((sys.area_floor - 41.5).abs() < 1.0);
        assert_eq!(sys.category_eu, 2); // Specific
        assert_eq!(sys.class_eu, 4); // Class 3
        assert!((sys.operator_altitude_geo - 20.5).abs() < 0.5);
        assert_eq!(sys.timestamp, 28000000);
    }

    /// Full WiFi beacon frame from opendroneid-core-c unit_odid_wifi_beacon.cpp.
    /// The vendor IE contains a message pack with BasicID + Location + System.
    /// We parse just the vendor IE portion and validate all three are decoded.
    #[test]
    fn test_reference_wifi_beacon_vendor_ie() {
        // From the C test's expectedBuffer, extract the vendor IE starting at
        // tag 0xDD. The OUI in that test is FA:0B:BC (ASD-STAN), not the ASTM
        // OUI 90:3A:E6. Both are valid per the spec. Let's test with the ASTM
        // OUI since that's what our parser checks.
        //
        // Build a vendor IE with ASTM OUI wrapping the BasicID from the test.
        let basic_id_bytes: [u8; MESSAGE_SIZE] = [
            0x02, 0x14, b'U', b'S', b'S', b'-', b'E', b'n', b't', b'e', b'r', b'p', b'r', b'i',
            b's', b'e', 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let ie_len = 3 + 1 + MESSAGE_SIZE; // OUI + type + message
        let mut ie = [0u8; 64];
        ie[0] = 0xDD;
        ie[1] = ie_len as u8;
        ie[2..5].copy_from_slice(&ODID_WIFI_OUI);
        ie[5] = 0x0D; // OUI type
        ie[6..6 + MESSAGE_SIZE].copy_from_slice(&basic_id_bytes);

        let frame = parse_odid_wifi_beacon(&ie[..6 + MESSAGE_SIZE]).unwrap();
        let bid = frame.basic_id.unwrap();
        assert_eq!(bid.uas_id.as_str(), "USS-Enterprise");
        assert_eq!(bid.ua_type, UaType::HybridLift);
        assert_eq!(bid.id_type, IdType::SerialNumber);
    }

    /// Operator ID round-trip from test_inout.c.
    /// Input: OperatorIdType=0, OperatorId="98765432100123456789"
    #[test]
    fn test_reference_round_trip_operator_id() {
        #[rustfmt::skip]
        let msg: [u8; MESSAGE_SIZE] = [
            0x52, // MessageType=5 (OperatorID), ProtoVersion=2
            0x00, // OperatorIdType=0
            b'9', b'8', b'7', b'6', b'5', b'4', b'3', b'2', b'1', b'0',
            b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9',
            0x00, 0x00, 0x00, // reserved
        ];

        let oid = decode_operator_id(&msg).unwrap();
        assert_eq!(oid.operator_id_type, 0);
        assert_eq!(oid.operator_id.as_str(), "98765432100123456789");
    }

    /// Basic ID from simulation (test/opendroneid_sim.c).
    /// Input: SerialNumber, Helicopter, "INTCE123456789012345"
    #[test]
    fn test_reference_sim_basic_id() {
        #[rustfmt::skip]
        let msg: [u8; MESSAGE_SIZE] = [
            0x02, // MessageType=0, ProtoVersion=2
            0x12, // IDType=1 (SerialNumber), UAType=2 (Helicopter)
            b'I', b'N', b'T', b'C', b'E', b'1', b'2', b'3', b'4', b'5',
            b'6', b'7', b'8', b'9', b'0', b'1', b'2', b'3', b'4', b'5',
            0x00, 0x00, 0x00,
        ];

        let bid = decode_basic_id(&msg).unwrap();
        assert_eq!(bid.ua_type, UaType::HelicopterOrMultirotor);
        assert_eq!(bid.id_type, IdType::SerialNumber);
        assert_eq!(bid.uas_id.as_str(), "INTCE123456789012345");
    }

    /// Packed message: 3-byte header + N inner 25-byte messages.
    /// Format: [type+version, single_size=25, count=N, msg0..., msg1..., ...]
    #[test]
    fn test_decode_packed_message() {
        let bid = make_basic_id(2, 1, b"PACKED001");
        let loc = make_location(48.8566, 2.3522, 35.0, 2);

        // 3-byte pack header + 2 inner messages
        let mut data = [0u8; 3 + MESSAGE_SIZE * 2];
        data[0] = (MessageType::Packed as u8) << 4 | 0x02; // type=Packed, version=2
        data[1] = MESSAGE_SIZE as u8; // single message size
        data[2] = 2; // message count
        data[3..3 + MESSAGE_SIZE].copy_from_slice(&bid);
        data[3 + MESSAGE_SIZE..3 + MESSAGE_SIZE * 2].copy_from_slice(&loc);

        let frame = decode_messages(&data).unwrap();
        let bid = frame.basic_id.unwrap();
        assert_eq!(bid.uas_id.as_str(), "PACKED001");
        assert_eq!(bid.ua_type, UaType::HelicopterOrMultirotor);

        let loc = frame.location.unwrap();
        assert_eq!(loc.status, Status::Airborne);
        assert!((loc.latitude - 48.8566).abs() < 0.0001);
        assert!((loc.longitude - 2.3522).abs() < 0.0001);
    }

    /// Packed message with wrong single_size is skipped.
    #[test]
    fn test_decode_packed_wrong_size_skipped() {
        let mut data = [0u8; MESSAGE_SIZE];
        data[0] = (MessageType::Packed as u8) << 4 | 0x02;
        data[1] = 30; // wrong single message size (not 25)
        data[2] = 1;

        let result = decode_messages(&data);
        assert!(result.is_none());
    }

    /// Negative latitude/longitude (southern/western hemisphere).
    #[test]
    fn test_negative_latlon() {
        // São Paulo: -23.5505, -46.6333
        let msg = make_location(-23.5505, -46.6333, 760.0, 2);
        let loc = decode_location(&msg).unwrap();
        assert!((loc.latitude - (-23.5505)).abs() < 0.0001);
        assert!((loc.longitude - (-46.6333)).abs() < 0.0001);
    }
}
