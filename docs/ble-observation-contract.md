# BLE observation contract

Implementation agreement for #35, #36, #171 and #172; confidence and alert policy
remain #154. This document specifies the shared boundaries. It is not a claim
that a backend receives extended advertisements or Coded PHY.

## Identity and receiver

An observation has a backend-scoped peripheral identifier and an optional
hardware address with explicit public/random/unknown address type and provenance.
An address recovered by a private platform API does not establish public address
type. Never synthesize a MAC for a UUID-only peripheral. Receiver identity and
scan-session identity are distinct from advertiser identity.

Receiver diagnostics describe backend, selected adapter identifier, and separate
controller-advertised, API-exposed, selected-configuration and experimentally
validated capabilities. Every unavailable fact is unknown, not implicitly false
or supported. Preserve normal default scanning. Explicit unsupported selections
fail clearly rather than selecting a different receiver. CoreBluetooth adapter
selection is system-managed.

## Normalized advertisement fields

- UUID identity is a canonical full 128-bit value. Expand 16/32-bit Bluetooth
  aliases using the Bluetooth base UUID; abbreviate only exact base matches.
  Raw AD little-endian UUIDs and platform canonical UUIDs use explicit conversion
  functions. A custom UUID sharing a short suffix is never the same service.
- Manufacturer records pair a `u16` company ID with payload bytes **after** the
  company ID. Zero is a valid company ID. Missing data is represented separately.
- Service-data records pair a full UUID with payload bytes **after** the UUID.
  Preserve UUIDs in advertised lists and service-data keys without conflating a
  service-data record with membership in an advertised service list.
- Preserve every available record within explicit bounds; matching must examine
  the appropriate records rather than choosing a map's arbitrary first entry.
  Deterministic ordering is required for serialization and capacity selection.
- Names are fields, not concatenations of AD sections. A complete name takes
  precedence over a shortened name within a raw report.

Core parsing/matching uses borrowed, allocation-free views over typed records.
Desktop storage can own bounded vectors; firmware need not enlarge every queued
`BleEvent` to the desktop layout. Retain the existing M5 storage until its linked
stack, queue footprint and live behavior are checked. Backend normalization must
not construct a fictitious original AD packet from OS property maps.

Desktop limits are policy bounds, not radio capability claims. Implementations
must declare limits for record counts, individual payload bytes and total bytes,
retain deterministic subsets, and report capacity truncation. Raw-report parsing
must distinguish malformed lengths, incomplete input and local capacity loss.
Do not label an OS snapshot complete merely because it fits these bounds.

## Freshness and completeness

Raw radio reports and OS property snapshots have different evidence semantics.
Mark btleplug property snapshots as potentially aggregated/cached with unknown
packet completeness and cross-field contemporaneity. Receiving a properties
update does not establish that every property changed in that callback. Preserve
available manufacturer/service update events; never invent missing timestamps,
packet order, PHY, advertising-set identity or fragmentation metadata.

This first contract does not enable corroborated same-packet alerts from cached
fields. Decoders may process a specific available service-data record with its
source/freshness limits; temporal deduplication and reassembly are separate work.
Any future reassembly must be bounded by source/session/set, bytes and time.

## Matching and output compatibility

New byte patterns explicitly identify manufacturer/company or service/UUID
context, payload-relative offset, length requirements and optional masks. Legacy
raw-AD and company-inclusive offsets retain their documented behavior until an
explicit migration. No silent reinterpretation of signature-v1 offsets.

Existing v1 fields remain compatible. Add full UUID/payload/provenance fields or
a distinct versioned desktop record; do not change a numeric UUID array into a
string array in place. Shared matching views must support both raw AD and decoded
backend fields without making their provenance equivalent. Classification and
alert decisions remain separate; this work does not enable research candidates.

## Ownership and validation

Payload/UUID work owns shared pure types/parsers, desktop property normalization,
contextual matching and compatible observation output (#35/#36). Adapter work
owns CLI selection, initialization, receiver diagnostics and capability reporting
(#171). Both may touch `hostd/src/ble.rs` and `hostd/src/main.rs`; implement in
isolated branches, keep helper modules separate, and resolve integration centrally.

Required fixtures cover full/custom UUIDs and endianness, multiple and empty
records (including company ID zero), wrong context, malformed/truncated input,
capacity overflow, UUID-only identity, snapshot freshness, deterministic output,
and existing behavior. Adapter fixtures cover absent/multiple/stale selections,
unknown/unsupported capabilities and disconnects. Test through production
conversion/matching/output where possible, not a separate lookalike evaluator.

#172 follows these software interfaces: route normalized Remote ID service data
through decoding and desktop output without a generic signature prerequisite or
fabricated MAC. Hardware validation then separately establishes legacy, extended
and PHY-specific reception using an identified receiver and controlled transmitter.

## First payload/UUID implementation

`ble_data` supplies canonical UUIDs, borrowed AD/manufacturer/service views and
contextual byte masks/offset/length checks. The desktop `advertisement` extension
preserves up to 64 advertised UUIDs, 16 manufacturer records and 16 service
records, at most 1,024 bytes per record and 4,096 payload bytes in total. These are
application limits, not Bluetooth limits. Lowest sorted keys win; manufacturer
payloads consume the budget before service payloads. Empty records and company
ID zero remain representable. Omitted entry counts and per-record truncation are
explicit; truncated payloads are excluded from matching. Normalization occurs
before the bounded address-enrichment queue. OS-owned properties can still be
larger before normalization and the upstream library owns its own event buffers.

Desktop output retains the numeric `service_uuids` compatibility array while
`advertisement.advertised_services` and service-data keys carry full UUID strings.
Linux also retains legacy matched `type: ble` records while emitting the new
all-observation `ble_observation` records; consumers must select a stream rather
than count both as independent observations. Full payload output is private
capture data and follows the same handling as existing identifiers.

Update-only manufacturer/service events are retained even when property lookup
fails. `advertisement.rssi_available=false` means the legacy outer `rssi=0` is
an unavailable-value placeholder, not a measurement. Such records are excluded
from RSSI-dependent matching and WiGLE export. Names and addresses may be absent.
Receiver identity is an integration hook populated by the adapter work (#171).

Normalized snapshots evaluate primitive clues but do not run composite rules:
cached fields cannot establish same-packet corroboration. The migrated Find My
pattern requires company `004C`, payload type `12`, declared length `19`, and
exactly 27 payload bytes after the company ID. It is labeled a framing candidate,
not a model identification. Legacy raw-AD search-anywhere patterns are not applied
to OS maps. Existing raw-AD/v1 offset semantics are unchanged.

The raw AD iterator reports incomplete envelopes separately from malformed typed
UUID/company/service fields. Full UUID borrowed accessors and base-alias matching
use retained embedded bytes; owned `BleEvent` storage has not grown. Embedded
capacity/provenance reporting, primary UUID output, full configurable UUID rule
loading and signature schema migration remain follow-up work. The existing
62-byte bound still prevents a general extended-advertisement support claim.

Reference for AD field boundaries:
[Bluetooth Supplement, Data Types](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/CSS_v11/out/en/supplement-to-the-bluetooth-core-specification/data-types-specification.html).
