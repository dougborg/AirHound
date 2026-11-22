//! Allocation-free receive-only companion prototype. No radio or hardware access.
#![no_std]

use heapless::{String, Vec};
use serde::Deserialize;

pub mod bridge;
pub mod diagnostic;
mod syntax;

/// Current sensor serializers use a 512-byte output buffer (including newline).
/// Larger future/schema-valid records are explicitly rejected, not truncated.
pub const MAX_JSON: usize = 512;
pub const MAX_PREFIX: usize = 128;
pub const MAX_LINE: usize = MAX_JSON + MAX_PREFIX;

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct RuleEvidence {
    pub id: u16,
    pub sig: Vec<u16, 4>,
    pub more: bool,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct Detection {
    pub engine: String<32>,
    pub policy: String<24>,
    pub pack: String<64>,
    pub ruleset: String<64>,
    pub sig: Vec<u16, 8>,
    pub rules: Vec<RuleEvidence, 4>,
    pub more: bool,
    pub basis: String<24>,
    pub alert: String<16>,
}
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
pub struct StreamStats {
    pub scan_drops: u32,
    pub output_drops: u32,
    pub ble_output_drops: u32,
    pub serialize_errors: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Observation {
    pub id: String<18>,
    pub name: String<33>,
    pub ts: u32,
    pub rssi: i8,
    pub kind: ObservationKind,
    /// A clue, not confirmed identity or an instruction to alert.
    pub signature_match: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ObservationKind {
    AccessPoint,
    OtherWifi,
    Ble,
}
// Fixed-size returned value avoids heap allocation; no queue stores these values.
// The host test enforces the size budget.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Observation(Observation),
    /// Unassociated: (id, scope, ts) is not unique across repeated observations.
    /// Never attach to a previous sighting or count as another observation.
    Evidence {
        id: String<18>,
        scope: String<24>,
        ts: u32,
        detection: Detection,
    },
    Stats(StreamStats),
    Unknown,
    Rejected(Rejection),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    MalformedOrUnsupported,
    Oversized,
}
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Counts {
    pub access_point_observations: u32,
    pub other_wifi_observations: u32,
    pub ble_observations: u32,
    pub signature_observations: u32,
    pub evidence_records: u32,
    pub unknown_records: u32,
    pub rejected_records: u32,
    pub oversized_records: u32,
    pub partial_lines_discarded: u32,
}
#[derive(Deserialize)]
struct MatchReason {
    #[serde(rename = "type")]
    _kind: String<32>,
    #[serde(rename = "detail")]
    _detail: String<32>,
}
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: String<32>,
    mac: Option<String<18>>,
    id: Option<String<18>>,
    scope: Option<String<24>>,
    ssid: Option<String<33>>,
    name: Option<String<33>>,
    ts: Option<u32>,
    rssi: Option<i8>,
    frame: Option<String<16>>,
    ch: Option<u8>,
    mfr: Option<u16>,
    #[serde(rename = "uuid")]
    _uuid: Option<String<37>>,
    #[serde(rename = "match")]
    matches: Option<Vec<MatchReason, 4>>,
    rule: Option<String<32>>,
    detection: Option<Detection>,
    scan_drops: Option<u32>,
    output_drops: Option<u32>,
    ble_output_drops: Option<u32>,
    serialize_errors: Option<u32>,
}

/// One line in memory, no internal event queue. Consume each returned event
/// before feeding the next byte. Transport queue losses must be counted by the
/// future hardware adapter, separately from these parser rejection counters.
pub struct Receiver {
    line: Vec<u8, MAX_LINE>,
    discard: bool,
    connected: bool,
    pub counts: Counts,
    pub latest_stats: Option<StreamStats>,
}
impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}
impl Receiver {
    pub const fn new() -> Self {
        Self {
            line: Vec::new(),
            discard: false,
            connected: false,
            counts: Counts {
                access_point_observations: 0,
                other_wifi_observations: 0,
                ble_observations: 0,
                signature_observations: 0,
                evidence_records: 0,
                unknown_records: 0,
                rejected_records: 0,
                oversized_records: 0,
                partial_lines_discarded: 0,
            },
            latest_stats: None,
        }
    }
    /// Caller creates a new source/session boundary. No pre-reconnect fragments,
    /// counters or previous device-loss baseline survive this call.
    pub fn begin_session(&mut self) {
        *self = Self::new();
        self.connected = true;
    }
    pub fn disconnect(&mut self) {
        if !self.line.is_empty() {
            self.counts.partial_lines_discarded =
                self.counts.partial_lines_discarded.saturating_add(1);
        }
        self.connected = false;
        self.line.clear();
        self.discard = false;
    }
    pub fn connected(&self) -> bool {
        self.connected
    }
    pub fn feed(&mut self, byte: u8) -> Option<Event> {
        if !self.connected {
            return None;
        }
        if byte != b'\n' {
            if !self.discard && self.line.push(byte).is_err() {
                self.line.clear();
                self.discard = true;
                self.counts.oversized_records = self.counts.oversized_records.saturating_add(1);
                return Some(Event::Rejected(Rejection::Oversized));
            }
            return None;
        }
        if self.discard {
            self.discard = false;
            return None;
        }
        let event = parse_line(&self.line);
        self.line.clear();
        match &event {
            Some(Event::Observation(o)) => {
                let count = match o.kind {
                    ObservationKind::AccessPoint => &mut self.counts.access_point_observations,
                    ObservationKind::OtherWifi => &mut self.counts.other_wifi_observations,
                    ObservationKind::Ble => &mut self.counts.ble_observations,
                };
                *count = count.saturating_add(1);
                if o.signature_match {
                    self.counts.signature_observations =
                        self.counts.signature_observations.saturating_add(1);
                }
            }
            Some(Event::Evidence { .. }) => {
                self.counts.evidence_records = self.counts.evidence_records.saturating_add(1)
            }
            Some(Event::Stats(stats)) => self.latest_stats = Some(*stats),
            Some(Event::Unknown) => {
                self.counts.unknown_records = self.counts.unknown_records.saturating_add(1)
            }
            Some(Event::Rejected(Rejection::Oversized)) => {
                self.counts.oversized_records = self.counts.oversized_records.saturating_add(1)
            }
            Some(Event::Rejected(_)) => {
                self.counts.rejected_records = self.counts.rejected_records.saturating_add(1)
            }
            None => {}
        }
        event
    }
}
fn valid_mac(s: &str) -> bool {
    s.len() == 17
        && s.bytes().enumerate().all(|(i, b)| {
            if i % 3 == 2 {
                b == b':'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn parse_line(line: &[u8]) -> Option<Event> {
    let start = line.iter().position(|&b| b == b'{')?;
    let mut json = &line[start..];
    while json
        .last()
        .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
    {
        json = &json[..json.len() - 1];
    }
    if start > MAX_PREFIX || json.len() > MAX_JSON {
        return Some(Event::Rejected(Rejection::Oversized));
    }
    let rejected = || Event::Rejected(Rejection::MalformedOrUnsupported);
    if !syntax::valid(json) {
        return Some(rejected());
    }
    // Classify future message types without imposing today's unrelated fields.
    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "type")]
        kind: String<32>,
    }
    let mut unescape = [0u8; MAX_JSON];
    let Ok((header, _)) = serde_json_core::from_slice_escaped::<Header>(json, &mut unescape) else {
        return Some(rejected());
    };
    if !matches!(
        header.kind.as_str(),
        "wifi" | "ble" | "detection" | "stream_stats"
    ) {
        return Some(Event::Unknown);
    }
    // Decode escapes into a bounded scratch buffer; no String/Vec allocations.
    let Ok((r, used)) = serde_json_core::from_slice_escaped::<Record>(json, &mut unescape) else {
        return Some(rejected());
    };
    if used != json.len() {
        return Some(rejected());
    }
    Some(convert(r).unwrap_or_else(rejected))
}
fn convert(r: Record) -> Option<Event> {
    match r.kind.as_str() {
        "wifi" | "ble" => {
            let id = r.mac?;
            if !valid_mac(&id) {
                return None;
            }
            let kind = if r.kind == "ble" {
                r.mfr?;
                ObservationKind::Ble
            } else {
                r.ch?;
                match r.frame?.as_str() {
                    "beacon" | "probe_resp" => ObservationKind::AccessPoint,
                    "probe_req" | "data" | "other" => ObservationKind::OtherWifi,
                    _ => return None,
                }
            };
            Some(Event::Observation(Observation {
                id,
                name: if r.kind == "ble" { r.name? } else { r.ssid? },
                ts: r.ts?,
                rssi: r.rssi?,
                kind,
                signature_match: !r.matches?.is_empty() || r.rule.is_some_and(|s| !s.is_empty()),
            }))
        }
        "detection" => {
            let id = r.id?;
            let scope = r.scope?;
            if !valid_mac(&id) || !matches!(scope.as_str(), "wifi_address" | "ble_address") {
                return None;
            }
            Some(Event::Evidence {
                id,
                scope,
                ts: r.ts?,
                detection: r.detection?,
            })
        }
        "stream_stats" => Some(Event::Stats(StreamStats {
            scan_drops: r.scan_drops?,
            output_drops: r.output_drops?,
            ble_output_drops: r.ble_output_drops?,
            serialize_errors: r.serialize_errors?,
        })),
        _ => Some(Event::Unknown),
    }
}
