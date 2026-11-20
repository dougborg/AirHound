//! Compiled defaults generated from signatures/builtin.v2.json.
//! Edit the canonical pack, then run scripts/compile-signatures.py.

use crate::rules::{ExprNode, MatchCategory, Rule, RuleDb, SigIdx};

/// A BLE advertisement byte pattern signature.
pub struct BleAdBytesPattern {
    pub active: bool,
    pub exact_manufacturer_len: Option<usize>,
    /// Byte sequence to match
    pub bytes: &'static [u8],
    /// Match position: `None` = search anywhere in raw AD data,
    /// `Some(n)` = match at manufacturer-specific data offset `n`
    pub offset: Option<usize>,
    /// Human-readable description
    pub description: &'static str,
}

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

include!("generated_signatures.rs");

/// Look up the vendor name for a MAC OUI signature index.
/// Returns empty string if the index is out of range or not a MAC OUI signature.
pub fn vendor_for_sig_idx(sig_idx: u16) -> &'static str {
    let offset = sig_idx.wrapping_sub(SIG_IDX_MAC_OUI_START) as usize;
    if offset < MAC_PREFIXES.len() {
        MAC_PREFIXES[offset].1
    } else {
        ""
    }
}
