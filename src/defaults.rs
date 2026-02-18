/// Default filter data compiled into the firmware.
///
/// MAC OUI prefixes merged from FlockOff (~88 entries), FlockSquawk (20 entries),
/// and flock-you. SSID patterns, BLE name patterns, Raven UUIDs, and manufacturer
/// IDs from FlockSquawk and flock-you.

/// Known MAC OUI prefixes (3-byte prefix, vendor name).
///
/// Sources: FlockOff defaultTargets.h, FlockSquawk DeviceSignatures.h, flock-you main.cpp
pub static MAC_PREFIXES: &[([u8; 3], &str)] = &[
    // === Flock Safety ===
    ([0xB4, 0x1E, 0x52], "Flock Safety"),
    // === Silicon Labs OUI (FlockSquawk / flock-you) ===
    ([0x58, 0x8E, 0x81], "Silicon Labs"),
    ([0xCC, 0xCC, 0xCC], "Silicon Labs"),
    ([0xEC, 0x1B, 0xBD], "Silicon Labs"),
    ([0x90, 0x35, 0xEA], "Silicon Labs"),
    ([0x04, 0x0D, 0x84], "Silicon Labs"),
    ([0xF0, 0x82, 0xC0], "Silicon Labs"),
    ([0x1C, 0x34, 0xF1], "Silicon Labs"),
    ([0x38, 0x5B, 0x44], "Silicon Labs"),
    ([0x94, 0x34, 0x69], "Silicon Labs"),
    ([0xB4, 0xE3, 0xF9], "Silicon Labs"),
    ([0x70, 0xC9, 0x4E], "Silicon Labs"),
    ([0x3C, 0x91, 0x80], "Silicon Labs"),
    ([0xD8, 0xF3, 0xBC], "Silicon Labs"),
    ([0x80, 0x30, 0x49], "Silicon Labs"),
    ([0x14, 0x5A, 0xFC], "Silicon Labs"),
    ([0x74, 0x4C, 0xA1], "Silicon Labs"),
    ([0x08, 0x3A, 0x88], "Silicon Labs"),
    ([0x9C, 0x2F, 0x9D], "Silicon Labs"),
    ([0x94, 0x08, 0x53], "Silicon Labs"),
    ([0xE4, 0xAA, 0xEA], "Silicon Labs"),
    // === Avigilon Alta ===
    ([0x70, 0x1A, 0xD5], "Avigilon Alta"),
    // === Axis Communications AB ===
    ([0x00, 0x40, 0x8C], "Axis Communications"),
    ([0xAC, 0xCC, 0x8E], "Axis Communications"),
    ([0xB8, 0xA4, 0x4F], "Axis Communications"),
    ([0xE8, 0x27, 0x25], "Axis Communications"),
    // === China Dragon Technology ===
    ([0x1C, 0x79, 0x2D], "China Dragon Technology"),
    ([0x3C, 0x3B, 0xAD], "China Dragon Technology"),
    ([0x40, 0x9C, 0xA7], "China Dragon Technology"),
    ([0x54, 0xAE, 0xBC], "China Dragon Technology"),
    ([0x5C, 0x8A, 0xAE], "China Dragon Technology"),
    ([0x6C, 0x05, 0xD3], "China Dragon Technology"),
    ([0xA4, 0x6B, 0x40], "China Dragon Technology"),
    ([0xA8, 0x4F, 0xA4], "China Dragon Technology"),
    ([0xA8, 0xA0, 0x92], "China Dragon Technology"),
    ([0xB0, 0xAC, 0x82], "China Dragon Technology"),
    ([0xBC, 0x2B, 0x02], "China Dragon Technology"),
    ([0xC0, 0xE3, 0x50], "China Dragon Technology"),
    ([0xC8, 0x26, 0xE2], "China Dragon Technology"),
    ([0xC8, 0x8A, 0xD8], "China Dragon Technology"),
    ([0x00, 0x7E, 0x56], "China Dragon Technology"),
    ([0x04, 0x39, 0x26], "China Dragon Technology"),
    ([0x24, 0xB7, 0x2A], "China Dragon Technology"),
    ([0x3C, 0x7A, 0xAA], "China Dragon Technology"),
    ([0x40, 0xAA, 0x56], "China Dragon Technology"),
    ([0x44, 0xEF, 0xBF], "China Dragon Technology"),
    ([0x78, 0x8A, 0x86], "China Dragon Technology"),
    ([0x94, 0xE0, 0xD6], "China Dragon Technology"),
    ([0xA0, 0x67, 0x20], "China Dragon Technology"),
    ([0xA0, 0x9D, 0xC1], "China Dragon Technology"),
    ([0xA8, 0x43, 0xA4], "China Dragon Technology"),
    ([0xD0, 0xA4, 0x6F], "China Dragon Technology"),
    ([0xE0, 0x51, 0xD8], "China Dragon Technology"),
    ([0xE0, 0x75, 0x26], "China Dragon Technology"),
    // === FLIR ===
    ([0x00, 0x13, 0x56], "FLIR Radiation"),
    ([0x00, 0x40, 0x7F], "FLIR Systems"),
    ([0x00, 0x1B, 0xD8], "FLIR Systems"),
    // === GeoVision ===
    ([0x00, 0x13, 0xE2], "GeoVision"),
    // === Hanwha Vision ===
    ([0x44, 0xB4, 0x23], "Hanwha Vision"),
    ([0x8C, 0x1D, 0x55], "Hanwha Vision"),
    ([0xE4, 0x30, 0x22], "Hanwha Vision"),
    // === March Networks ===
    ([0x00, 0x10, 0xBE], "March Networks"),
    ([0x00, 0x12, 0x81], "March Networks"),
    // === Meta Platforms ===
    ([0x48, 0x05, 0x60], "Meta Platforms"),
    ([0x50, 0x99, 0x03], "Meta Platforms"),
    ([0x78, 0xC4, 0xFA], "Meta Platforms"),
    ([0x80, 0xF3, 0xEF], "Meta Platforms"),
    ([0x84, 0x57, 0xF7], "Meta Platforms"),
    ([0x88, 0x25, 0x08], "Meta Platforms"),
    ([0x94, 0xF9, 0x29], "Meta Platforms"),
    ([0xB4, 0x17, 0xA8], "Meta Platforms"),
    ([0xC0, 0xDD, 0x8A], "Meta Platforms"),
    ([0xCC, 0xA1, 0x74], "Meta Platforms"),
    ([0xD0, 0xB3, 0xC2], "Meta Platforms"),
    ([0xD4, 0xD6, 0x59], "Meta Platforms"),
    // === Mobotix ===
    ([0x00, 0x03, 0xC5], "Mobotix"),
    // === Shenzhen Bilian Electronic ===
    ([0x08, 0xEA, 0x40], "Shenzhen Bilian"),
    ([0x0C, 0x8C, 0x24], "Shenzhen Bilian"),
    ([0x0C, 0xCF, 0x89], "Shenzhen Bilian"),
    ([0x10, 0xA4, 0xBE], "Shenzhen Bilian"),
    ([0x14, 0x5D, 0x34], "Shenzhen Bilian"),
    ([0x14, 0x6B, 0x9C], "Shenzhen Bilian"),
    ([0x20, 0x32, 0x33], "Shenzhen Bilian"),
    ([0x2C, 0xC3, 0xE6], "Shenzhen Bilian"),
    ([0x30, 0x7B, 0xC9], "Shenzhen Bilian"),
    ([0x34, 0x7D, 0xE4], "Shenzhen Bilian"),
    ([0x38, 0x01, 0x46], "Shenzhen Bilian"),
    ([0x38, 0x7A, 0xCC], "Shenzhen Bilian"),
    ([0x44, 0x01, 0xBB], "Shenzhen Bilian"),
    ([0x54, 0xEF, 0x33], "Shenzhen Bilian"),
    ([0x60, 0xFB, 0x00], "Shenzhen Bilian"),
    ([0x6C, 0xD5, 0x52], "Shenzhen Bilian"),
    ([0x74, 0xEE, 0x2A], "Shenzhen Bilian"),
    ([0x78, 0x22, 0x88], "Shenzhen Bilian"),
    ([0x7C, 0xA7, 0xB0], "Shenzhen Bilian"),
    ([0x84, 0xFC, 0x14], "Shenzhen Bilian"),
    ([0x88, 0x49, 0x2D], "Shenzhen Bilian"),
    ([0x94, 0xBA, 0x06], "Shenzhen Bilian"),
    ([0x98, 0x03, 0xCF], "Shenzhen Bilian"),
    ([0xA0, 0x9F, 0x10], "Shenzhen Bilian"),
    ([0xA8, 0xB5, 0x8E], "Shenzhen Bilian"),
    ([0xB4, 0x6D, 0xC2], "Shenzhen Bilian"),
    ([0xC4, 0x3C, 0xB0], "Shenzhen Bilian"),
    ([0xC8, 0xFE, 0x0F], "Shenzhen Bilian"),
    ([0xCC, 0x64, 0x1A], "Shenzhen Bilian"),
    ([0xE0, 0xB9, 0x4D], "Shenzhen Bilian"),
    ([0xEC, 0x3D, 0xFD], "Shenzhen Bilian"),
    ([0xF0, 0xC8, 0x14], "Shenzhen Bilian"),
    ([0xFC, 0x23, 0xCD], "Shenzhen Bilian"),
    ([0x20, 0xF4, 0x1B], "Shenzhen Bilian"),
    ([0x28, 0xF3, 0x66], "Shenzhen Bilian"),
    ([0x3C, 0x33, 0x00], "Shenzhen Bilian"),
    ([0x44, 0x33, 0x4C], "Shenzhen Bilian"),
    ([0xAC, 0xA2, 0x13], "Shenzhen Bilian"),
    // === Sunell Electronics ===
    ([0x00, 0x1C, 0x27], "Sunell Electronics"),
];

/// WiFi SSID exact-prefix patterns.
/// Match if SSID starts with the prefix and remaining chars match the given format.
pub static SSID_PATTERNS: &[SsidPattern] = &[
    SsidPattern {
        prefix: "Flock-",
        suffix_len: 6,
        suffix_kind: SuffixKind::HexChars,
        description: "Flock Safety camera WiFi",
    },
    SsidPattern {
        prefix: "Penguin-",
        suffix_len: 10,
        suffix_kind: SuffixKind::DecimalDigits,
        description: "Penguin device WiFi",
    },
];

/// WiFi SSID exact-match names.
pub static SSID_EXACT: &[&str] = &["FS Ext Battery"];

/// WiFi SSID substring keywords (case-insensitive).
pub static SSID_KEYWORDS: &[&str] = &["flock", "penguin", "pigvision"];

/// WiFi SSID name keyword from FlockOff (matches partial name in beacon/probe).
pub static WIFI_NAME_KEYWORDS: &[&str] = &["flock"];

/// BLE device name patterns (case-insensitive substring match).
pub static BLE_NAME_PATTERNS: &[&str] = &["Flock", "Penguin", "FS Ext Battery", "Pigvision"];

/// Raven custom BLE service UUIDs (16-bit short IDs).
/// Full UUID: 0000XXXX-0000-1000-8000-00805f9b34fb
pub static BLE_SERVICE_UUIDS_16: &[u16] = &[
    0x3100, // Raven GPS service
    0x3200, // Raven Power service
    0x3300, // Raven Network service
    0x3400, // Raven Upload service
    0x3500, // Raven Error service
];

/// Standard BLE service UUIDs also associated with Raven devices.
pub static BLE_STANDARD_UUIDS_16: &[u16] = &[
    0x180A, // Device Information
    0x1809, // Health Thermometer
    0x1819, // Location and Navigation
];

/// BLE manufacturer company IDs.
pub static BLE_MANUFACTURER_IDS: &[u16] = &[
    0x09C8, // XUNTONG (associated with Flock Safety)
];

/// A BLE advertisement byte pattern signature.
pub struct BleAdBytesPattern {
    /// Byte sequence to match
    pub bytes: &'static [u8],
    /// Match position: `None` = search anywhere in raw AD data,
    /// `Some(n)` = match at manufacturer-specific data offset `n`
    pub offset: Option<usize>,
    /// Human-readable description
    pub description: &'static str,
}

/// Known BLE advertisement byte patterns.
///
/// Patterns for tracker and multi-tool devices that can be identified
/// by specific byte sequences in their advertisement data.
pub static BLE_AD_BYTES_PATTERNS: &[BleAdBytesPattern] = &[
    // Apple AirTag / FindMy: manufacturer data starts with Apple company ID (0x004C)
    // followed by FindMy type byte 0x12 and length 0x19
    BleAdBytesPattern {
        bytes: &[0x4C, 0x00, 0x12, 0x19],
        offset: Some(0),
        description: "Apple AirTag",
    },
    // Flipper Zero White: [0x80, 0x30] in manufacturer-specific data
    BleAdBytesPattern {
        bytes: &[0x80, 0x30],
        offset: None,
        description: "Flipper Zero",
    },
    // Flipper Zero Black: [0x81, 0x30] in manufacturer-specific data
    BleAdBytesPattern {
        bytes: &[0x81, 0x30],
        offset: None,
        description: "Flipper Zero",
    },
];

// ── Signature index constants ────────────────────────────────────────
//
// Each compiled-in signature array gets a starting index. The global index
// of signature `i` within array `A` is `SIG_IDX_A_START + i`. These map
// every compiled-in signature to a unique `SigIdx` for the rule engine.

use crate::rules::{ExprNode, Rule, RuleDb, SigIdx};

pub const SIG_IDX_MAC_OUI_START: SigIdx = 0;
pub const SIG_IDX_SSID_PATTERN_START: SigIdx = MAC_PREFIXES.len() as SigIdx;
pub const SIG_IDX_SSID_EXACT_START: SigIdx = SIG_IDX_SSID_PATTERN_START + SSID_PATTERNS.len() as SigIdx;
pub const SIG_IDX_SSID_KEYWORD_START: SigIdx = SIG_IDX_SSID_EXACT_START + SSID_EXACT.len() as SigIdx;
pub const SIG_IDX_WIFI_NAME_START: SigIdx = SIG_IDX_SSID_KEYWORD_START + SSID_KEYWORDS.len() as SigIdx;
pub const SIG_IDX_BLE_NAME_START: SigIdx = SIG_IDX_WIFI_NAME_START + WIFI_NAME_KEYWORDS.len() as SigIdx;
pub const SIG_IDX_BLE_UUID_START: SigIdx = SIG_IDX_BLE_NAME_START + BLE_NAME_PATTERNS.len() as SigIdx;
pub const SIG_IDX_BLE_STD_UUID_START: SigIdx = SIG_IDX_BLE_UUID_START + BLE_SERVICE_UUIDS_16.len() as SigIdx;
pub const SIG_IDX_BLE_MFR_START: SigIdx = SIG_IDX_BLE_STD_UUID_START + BLE_STANDARD_UUIDS_16.len() as SigIdx;
pub const SIG_IDX_BLE_AD_BYTES_START: SigIdx = SIG_IDX_BLE_MFR_START + BLE_MANUFACTURER_IDS.len() as SigIdx;

/// Total number of compiled-in signatures.
pub const SIG_COUNT: usize = SIG_IDX_BLE_AD_BYTES_START as usize + BLE_AD_BYTES_PATTERNS.len();

// ── Named signature indices (for readability in rule definitions) ────

// Flock Safety Camera signatures
/// MAC_PREFIXES[0] = Flock Safety OUI B4:1E:52
const SIG_FLOCK_SAFETY_OUI: SigIdx = SIG_IDX_MAC_OUI_START + 0;
/// MAC_PREFIXES[1] = Silicon Labs 58:8E:81
const SIG_SILABS_588E81: SigIdx = SIG_IDX_MAC_OUI_START + 1;
/// MAC_PREFIXES[2] = Silicon Labs CC:CC:CC
const SIG_SILABS_CCCCCC: SigIdx = SIG_IDX_MAC_OUI_START + 2;
/// SSID_PATTERNS[0] = Flock-XXXXXX
const SIG_FLOCK_SSID_PREFIX: SigIdx = SIG_IDX_SSID_PATTERN_START + 0;
/// SSID_EXACT[0] = "FS Ext Battery"
const SIG_FS_EXT_BATTERY_SSID: SigIdx = SIG_IDX_SSID_EXACT_START + 0;
/// SSID_KEYWORDS[0] = "flock"
const SIG_FLOCK_SSID_KEYWORD: SigIdx = SIG_IDX_SSID_KEYWORD_START + 0;
/// BLE_NAME_PATTERNS[0] = "Flock"
const SIG_FLOCK_BLE_NAME: SigIdx = SIG_IDX_BLE_NAME_START + 0;
/// BLE_NAME_PATTERNS[2] = "FS Ext Battery"
const SIG_FS_EXT_BATTERY_BLE: SigIdx = SIG_IDX_BLE_NAME_START + 2;
/// BLE_MANUFACTURER_IDS[0] = 0x09C8 (XUNTONG)
const SIG_XUNTONG_MFR: SigIdx = SIG_IDX_BLE_MFR_START + 0;

// Raven Acoustic Sensor signatures
/// BLE_SERVICE_UUIDS_16[0] = 0x3100
const SIG_RAVEN_GPS_UUID: SigIdx = SIG_IDX_BLE_UUID_START + 0;
/// BLE_SERVICE_UUIDS_16[1] = 0x3200
const SIG_RAVEN_POWER_UUID: SigIdx = SIG_IDX_BLE_UUID_START + 1;
/// BLE_SERVICE_UUIDS_16[2] = 0x3300
const SIG_RAVEN_NETWORK_UUID: SigIdx = SIG_IDX_BLE_UUID_START + 2;
/// BLE_SERVICE_UUIDS_16[3] = 0x3400
const SIG_RAVEN_UPLOAD_UUID: SigIdx = SIG_IDX_BLE_UUID_START + 3;
/// BLE_SERVICE_UUIDS_16[4] = 0x3500
const SIG_RAVEN_ERROR_UUID: SigIdx = SIG_IDX_BLE_UUID_START + 4;

// Apple AirTag signature
/// BLE_AD_BYTES_PATTERNS[0] = Apple FindMy header
const SIG_AIRTAG_FINDMY_AD: SigIdx = SIG_IDX_BLE_AD_BYTES_START + 0;

// Flipper Zero signatures
/// BLE_AD_BYTES_PATTERNS[1] = Flipper Zero White
const SIG_FLIPPER_ZERO_WHITE: SigIdx = SIG_IDX_BLE_AD_BYTES_START + 1;
/// BLE_AD_BYTES_PATTERNS[2] = Flipper Zero Black
const SIG_FLIPPER_ZERO_BLACK: SigIdx = SIG_IDX_BLE_AD_BYTES_START + 2;

// ── Compiled-in rule database ───────────────────────────────────────

/// Shared post-order expression node pool for all rules.
///
/// Layout:
///   [0..11]  Flock Safety Camera (12 nodes)
///   [12..17] Raven Acoustic Sensor (6 nodes)
///   [18]     Apple AirTag (1 node)
///   [19..21] Flipper Zero (3 nodes)
static DEFAULT_RULE_NODES: &[ExprNode] = &[
    // ── Flock Safety Camera ──
    // anyOf(
    //   flock-safety-oui, silabs-588e81, silabs-cccccc,
    //   flock-ssid-prefix, fs-ext-battery-ssid, flock-ssid-keyword,
    //   flock-ble-name, fs-ext-battery-ble,
    //   allOf(xuntong-mfr, flock-ble-name)
    // )
    ExprNode::Sig(SIG_FLOCK_SAFETY_OUI),    // [0]
    ExprNode::Sig(SIG_SILABS_588E81),        // [1]
    ExprNode::Sig(SIG_SILABS_CCCCCC),        // [2]
    ExprNode::Sig(SIG_FLOCK_SSID_PREFIX),    // [3]
    ExprNode::Sig(SIG_FS_EXT_BATTERY_SSID),  // [4]
    ExprNode::Sig(SIG_FLOCK_SSID_KEYWORD),   // [5]
    ExprNode::Sig(SIG_FLOCK_BLE_NAME),       // [6]
    ExprNode::Sig(SIG_FS_EXT_BATTERY_BLE),   // [7]
    // allOf(xuntong-mfr, flock-ble-name) — nested AND
    ExprNode::Sig(SIG_XUNTONG_MFR),          // [8]
    ExprNode::Sig(SIG_FLOCK_BLE_NAME),       // [9]  (same sig referenced again)
    ExprNode::AllOf { count: 2 },             // [10]
    // Top-level anyOf: 8 direct sigs + 1 allOf result = 9 children
    ExprNode::AnyOf { count: 9 },             // [11]

    // ── Raven Acoustic Sensor ──
    // anyOf(raven-gps, raven-power, raven-network, raven-upload, raven-error)
    ExprNode::Sig(SIG_RAVEN_GPS_UUID),        // [12]
    ExprNode::Sig(SIG_RAVEN_POWER_UUID),      // [13]
    ExprNode::Sig(SIG_RAVEN_NETWORK_UUID),    // [14]
    ExprNode::Sig(SIG_RAVEN_UPLOAD_UUID),     // [15]
    ExprNode::Sig(SIG_RAVEN_ERROR_UUID),      // [16]
    ExprNode::AnyOf { count: 5 },             // [17]

    // ── Apple AirTag ──
    // Single sig reference
    ExprNode::Sig(SIG_AIRTAG_FINDMY_AD),      // [18]

    // ── Flipper Zero ──
    // anyOf(flipper-white, flipper-black)
    ExprNode::Sig(SIG_FLIPPER_ZERO_WHITE),     // [19]
    ExprNode::Sig(SIG_FLIPPER_ZERO_BLACK),     // [20]
    ExprNode::AnyOf { count: 2 },              // [21]
];

static DEFAULT_RULES: &[Rule] = &[
    Rule {
        name: "Flock Safety Camera",
        expr_start: 0,
        expr_len: 12,
    },
    Rule {
        name: "Raven Acoustic Sensor",
        expr_start: 12,
        expr_len: 6,
    },
    Rule {
        name: "Apple AirTag",
        expr_start: 18,
        expr_len: 1,
    },
    Rule {
        name: "Flipper Zero",
        expr_start: 19,
        expr_len: 3,
    },
];

/// The default compiled-in rule database.
pub static DEFAULT_RULE_DB: RuleDb = RuleDb {
    nodes: DEFAULT_RULE_NODES,
    rules: DEFAULT_RULES,
};

/// SSID suffix format kind
#[derive(Debug, Clone, Copy)]
pub enum SuffixKind {
    /// Only hexadecimal characters (0-9, a-f, A-F)
    HexChars,
    /// Only decimal digits (0-9)
    DecimalDigits,
}

/// A structured SSID matching pattern
#[derive(Debug, Clone)]
pub struct SsidPattern {
    pub prefix: &'static str,
    pub suffix_len: usize,
    pub suffix_kind: SuffixKind,
    pub description: &'static str,
}

impl SsidPattern {
    /// Check if an SSID matches this pattern
    pub fn matches(&self, ssid: &str) -> bool {
        if let Some(suffix) = ssid.strip_prefix(self.prefix) {
            if suffix.len() != self.suffix_len {
                return false;
            }
            match self.suffix_kind {
                SuffixKind::HexChars => suffix.chars().all(|c| c.is_ascii_hexdigit()),
                SuffixKind::DecimalDigits => suffix.chars().all(|c| c.is_ascii_digit()),
            }
        } else {
            false
        }
    }
}
