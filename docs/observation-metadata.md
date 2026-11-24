# Observation metadata and retained evidence

The desktop backend preserves source evidence and adds a bounded, read-time
`observation_metadata` projection to stored record pages. Live and saved views use
the same Rust projection. It does not update the original database payload, change
record cursors, or require a schema migration. Older clients can ignore the
additive field; newer clients also accept pages without it.

Frequency, receive PHY, advertised capability, and source clocks are distinct.
A missing value stays unknown. A derived frequency records its basis. An advertised
802.11 mode is not the mode used to receive a frame. CoreBluetooth identities are
OS-scoped identifiers, not invented hardware addresses.

| Source | Retained or projected evidence | Remaining limits |
| --- | --- | --- |
| Native macOS Wi-Fi | CoreWLAN band/channel-derived frequency; channel width, noise, beacon interval, country and IBSS when reported; supported PHY/security queries; binary SSID and information elements | OS scan snapshot with unknown beacon/probe subtype and capture time. Public queries cover PHY through ax and width through 160 MHz; be/320 are not queried, rather than reported unsupported. Raw IEs may contain newer capabilities. No per-packet receive PHY. |
| Native macOS BLE | Existing bounded advertisement snapshot, full UUIDs, service/manufacturer data, name and OS identity remain in the original record | Snapshot may combine cached fields. Existing composite-matching restrictions remain. No manufactured MAC, RF channel, or receive PHY. |
| M5 serial | Existing producer record and reported fields remain intact | Current firmware's BLE summary is not a full advertisement. Wi-Fi raw IEs/security/band and full BLE payloads require producer changes; the host cannot reconstruct them. |
| CatSniffer PCAP | Packet receive PHY; RF frequency from a non-aliased pseudoheader RF index; existing full advertisement snapshot and packet evidence | Historical records without the alias flag cannot prove frequency. Logical BLE channel and RF index differ. No automatic conversion into WiGLE's Bluetooth Frequency column. |
| ALFA companion | Receive frequency derived from the explicitly supported 2.4/5 GHz channel set; SSID bytes, advertised channel and existing raw/estimated RSSI | Estimates remain separate from validated RSSI. Current producer schema does not supply raw IEs, security, or receive PHY. |
| Flipper Wi-Fi | Bounded parse of existing plaintext, unfragmented management-frame bytes: binary SSID, advertised DS/HT channel, privacy bit, capability-element presence and RSN/WPA selectors | Advertised channel is not receive channel/frequency. No receive PHY or measured RSSI is invented. Encrypted, fragmented and unsupported frames remain raw evidence. |
| Orbic cellular | Existing partial cellular identity, measurements, raw decoder fields and clock provenance remain intact | This change does not join partial identities, validate modem/host clock alignment, or invent missing location. |

## Bounds and parser behavior

Native raw SSID retention is at most 32 bytes and IE retention at most 4,096 bytes.
Both include original length and truncation. Raw IE order, duplicates and unknown
IDs remain in those bytes. The small legacy ODID input still receives only complete
IEs that fit its existing 64-byte capacity; it is not the full retained IE evidence.

The shared IE parser distinguishes malformed/truncated tagged parameters,
unsupported security versions, unknown suite selectors and legally omitted RSN
fields. It does not infer an open network from missing or broken security IEs.
RSN optional-field handling follows the pinned `ieee80211` parser's field-boundary
semantics; absent default suites are not fabricated. Conflicting or incomplete
channel evidence does not become an authoritative advertised channel. HE/EHT
presence is explicitly unparsed, not a complete capability decoder.

A projection is limited to 64 KiB serialized. Oversize projections report omission
while retaining the original payload. A record page's 512 KiB limit includes the
payload, projection, record wrappers and page envelope. Swift's existing bounded
presentation window counts metadata bytes too; held inspection remains separate
from ongoing capture. Projection performs no per-record filesystem or device I/O.

## Matching and export boundaries

This change preserves existing matcher policy. CatSniffer already supplies its rich
advertisement view to BLE matching; empty legacy convenience fields were not a loss
of that packet evidence. Native cached snapshots and firmware summaries do not gain
unjustified complete-packet matching privileges. Security, frequency and GPS are not
new signature inputs in this change.

The desktop capture path still sends its legacy WiGLE writer to a sink. Enriched
SQLite records are not an implemented desktop export or an upload eligibility claim.
Issue #31 owns SQLite export and source-specific eligibility; #258 owns upload and
#259 its UI. A future export must distinguish real addresses from OS identities,
measured from estimated RSSI, receiver location from target location, and trustworthy
capture time from receipt time. Unknown values must not become invented defaults.

Producer follow-ups remain separate: bounded M5 full advertisement/IE reporting
with explicit completeness and memory/output-loss tests; and ALFA companion raw
IE/security extensions with versioned, bounded host retention. Neither is delivered
by host projection. Existing raw records remain the authoritative evidence for
future decoders rather than being rewritten by this feature.
