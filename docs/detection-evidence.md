# Signature evidence and alert policy

AirHound evaluates signatures and attaches evidence in the shared Rust core
(`filter` and `evidence`), used by both firmware variants and desktop. Backends
supply observations; the UI and transport consume the result. A candidate is a
lead, not verified product identity, ownership, intent, or proximity.

## Data flow

1. Normalize observed fields while retaining address type and source provenance.
2. Match primitive signatures and retain their pack-scoped IDs.
3. Evaluate enabled rules against those matches. Each rule gets its own proof.
4. Apply alert policy independently of observation retention and signature counts.
5. Emit the observation and evidence. Future desktop history can correlate them.

An OR expression retains the first satisfied branch's proof. An AND expression
unions its supporting IDs, removing duplicates. Unrelated matches never upgrade
a rule. Missing fields are unknown: NOT cannot turn missing, unavailable, or
truncated data into a positive claim. Absence predicates need a future
completeness contract; negative-only and double-negation-only proofs are
conservatively withheld. The older Boolean evaluator remains a compatibility
API, but production filtering uses the evidence evaluator.

OS BLE snapshots may combine cached fields. They produce primitive evidence with
`basis: "possibly_cached"`, and no composite rule evidence. The Linux compatibility
record uses this same result. It must not be counted as another independent
observation. Firmware BLE address type is currently unknown, so vendor-prefix
inference is disabled there until the scanner preserves that metadata. Explicit
watchlist matches still work; they represent user intent, not corroboration.

UUID-list fields must contain a whole number of UUIDs. The firmware scanner
and shared raw matcher reject an odd-length 16-bit list rather than retaining
its recognizable prefix. A separately well-formed field can still contribute
evidence; a truncated AD field contributes none. These framing checks do not
establish device identity or extended-advertisement support.

## Current defaults

- A Flock-like candidate requires a vendor clue **and** a corresponding name
  clue from the same observation. This heuristic is alert-enabled, not a
  calibrated confidence score.
- Silicon Labs OUIs, generic names, and standalone vendor or service IDs remain
  visible clues without automatically causing a signature buzzer alert.
- HC-03/05/06 names identify an HC-series module candidate, not a card skimmer.
- Find My framing requires the Apple manufacturer field, header, and full
  expected field length. It is not sufficient to identify an AirTag.
- Unverified two-byte Flipper patterns are inactive.
- A WiFi vendor IE's OUI is not a transmitter MAC prefix. Remote ID service
  advertising is a candidate; decoded Remote ID records remain a separate path.
- WiFi locally administered/multicast addresses and BLE random/unknown addresses
  do not provide vendor OUI evidence.

The compiled pack is `builtin-2`, ruleset `builtin-rules-2`, and engine
`evidence-1`. Change these identifiers when their indexed contents or semantics
change. Numeric signature/rule IDs are meaningful only with those identifiers.
Custom compiled rule databases must supply their own ruleset ID; they still
refer to the compiled primitive signature pack.

## Output and bounds

Firmware and legacy desktop `wifi`/`ble` records have an additive
`type: "detection"` companion for matches. Its `id`, `scope`, and `ts`
associate it with the preceding observation in that source stream. This tuple
is not globally unique: timestamps wrap and several observations can share a
millisecond. Receivers should preserve stream order and source identity.

Rich desktop BLE and external WiFi observations embed the same `detection`
object. It contains engine, pack, ruleset, policy, primitive IDs, rule-local IDs,
basis, and alert decision. External WiFi uses the silent policy while signal
validation remains pending. See the device and desktop BLE JSON schemas.

The core allocates no heap: at most 8 emitted primitive IDs, 4 rule proofs,
4 supporting IDs per rule, and 16 expression stack entries. `more` marks
omitted evidence; an incomplete rule proof cannot enable an alert. Malformed
or overdeep expressions fail closed. The fixed 512-byte firmware message buffer
and observation queue entries are unchanged. A worst-case built-in evidence
serialization test includes the newline. Custom pack/ruleset strings must also
fit the transport limit; serialization failures are counted.

A companion costs an additional queue entry and serial/BLE bandwidth per
matched observation. It is best effort and can drop independently, with existing
stream loss counters recording failures. It does not eliminate scan or transport
drops. Live throughput and M5 stack/headroom validation remain release checks.

## Alerts and remaining work

The default signature policy is `rules_and_watchlist`: explicit watchlists or
complete alert-enabled rules can trigger the M5 buzzer. Match counts and the
last-match display still include quiet clues. The existing buzzer enable control
continues to apply. Decoded Remote ID's existing alert path is separate.

The core also supports `watchlist_only`, `all_matches`, and `silent`; a runtime
selector is not exposed yet. No numerical confidence score is claimed.

The [canonical v2 pack](signature-packs.md) now generates compiled defaults with
stable IDs, explicit policy/context, revision checks and review metadata. Legacy
v1 examples remain demonstrations; their raw-offset semantics are not silently
reinterpreted. Runtime loading (#14), general contextual predicates/full-UUID
firmware parity (#35/#36), confidence calibration and policy configuration (#154),
and temporal correlation remain follow-up work. Research candidates remain
opt-in and unvalidated. Flipper/sub-GHz and other external receivers are future
observation sources for this same pipeline.
