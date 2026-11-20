# Canonical signature packs

The production source of truth is [builtin.v2.json](../signatures/builtin.v2.json).
It contains all 144 existing primitive slots and seven rule slots, including
inactive placeholders. [signatures.v2.schema.json](../schemas/signatures.v2.schema.json)
defines its compiled profile. The compiler generates checked-in Rust data in
[src/generated_signatures.rs](../src/generated_signatures.rs); defaults.rs
contains types and helpers. Firmware builds need neither Python nor a JSON
parser. Desktop and both firmware variants consume the same generated tables.

This migration preserves every existing numeric ID, label, rule expression,
category and enable/alert flag. The pack/ruleset identifiers remain builtin-2
and builtin-rules-2 because their interpretation is unchanged. Stable textual
IDs in JSON let a consumer resolve compact evidence without parsing Rust.
Schema version 2 and pack revision names are separate concepts.

## Edit and validate

Use the existing schema-development environment (install check-jsonschema,
which also supplies jsonschema), then:

~~~sh
just generate-signatures
just check-signatures
cargo test --lib --no-default-features
~~~

Use a host Rust toolchain for the last command; the repository's firmware
toolchain override may require selecting stable explicitly.

The compiler checks schema structure, duplicate JSON keys/IDs/predicates,
contiguous indices, group ordering, referenced signatures and provenance,
enabled/alert policy, expression nesting/evaluation-stack limits, maximum
alert-proof size and the compact firmware message budget. It never silently
approximates an unsupported predicate. Regeneration must reproduce the checked-in
file exactly; CI rejects drift and runs rejection fixtures.

The schema job also uploads the catalog, schema and revision registry as
the canonical-signature-pack artifact. This is a review/download artifact,
not an automatic runtime update or installation mechanism.

## IDs and revisions

An emitted numeric signature ID resolves through pack_id and index; a rule ID
resolves through ruleset_id and index. id is the stable textual name. Indices
are explicit, contiguous and grouped by the current firmware table layout.
Retired byte-pattern slots stay inactive rather than shifting later IDs.

The append-only [revision registry](../signatures/revisions.json) fingerprints
the matching data and rule policy. Behavior, labels or index meaning changes
require a new pack and/or ruleset revision and a new registry entry. Ruleset
fingerprints include pack identity and engine semantics. CI compares the registry
with the PR's base commit and rejects rewriting/removing an existing revision.
Review/provenance notes can improve without changing numeric meaning.

This registry prevents accidental revision reuse; it is not a signed update
channel or an immutable archive of every historical pack. Consumers should retain
the exact catalog used for a capture. Distribution, authenticated updates and
runtime loading remain #14.

## Explicit compiled profile

The initial v2 profile represents current behavior, rather than promising the
entire future matcher:

- MAC prefixes require a universal/unicast WiFi address or known public BLE
  address; explicit watchlists remain independent.
- SSIDs support exact, ASCII-insensitive substring, and structured suffix
  predicates. BLE names use ASCII-insensitive substring matching.
- Service UUID predicates currently use exact Bluetooth 16-bit aliases.
  Arbitrary 128-bit UUID predicates are rejected pending #36.
- Manufacturer payload predicates explicitly name company ID, payload-relative
  offset zero, bytes and exact payload length. The compiler adds the company
  prefix only for the legacy raw-AD representation. Raw packet and normalized
  desktop matching read this same generated predicate.
- The old unscoped two-byte patterns are representable only as disabled legacy
  placeholders. They cannot be activated through this profile.
- Rules use positive anyOf/allOf expressions with explicit enabled and alert
  flags. NOT/absence, regex, masks and arbitrary offsets are rejected until
  their runtime semantics and completeness guarantees are implemented.

Company-ID-zero standalone predicates remain rejected because the embedded
legacy parser currently uses zero as absence. Payload context can still name
company zero. Capacity/truncation and cached-snapshot limitations remain as
documented in [detection evidence](detection-evidence.md); the schema does not
upgrade a receiver's capabilities. Generic typed BLE storage/parity is next
under #35/#36.

## Provenance is not validation

Each signature and rule has explicit review status, source references and
limitations. All migrated entries initially say legacy_unvalidated, pointing
to the exact previous AirHound source revision. This records where the data came
from; it does not independently verify every upstream OUI, device claim or
license. Per-entry primary-source and device-capture audit remains #154/#16.
A device_validated status requires a device-capture source, and still needs human
review of that evidence.

The v1 example/research files remain legacy interchange demonstrations, not
production input. They are not auto-imported or promoted. No glasses, equipment
or other research candidate is enabled by this migration. The shared contract
allows future device receivers to supply evidence to one matcher.
