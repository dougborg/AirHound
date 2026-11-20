# Firmware observation streaming

Both firmware variants now attempt to emit every parsed WiFi/BLE observation
while scanning is enabled. Repeated sightings remain repeated events; this is
not a deduplicated inventory or a raw packet capture. Parser/radio limitations
still apply (including the current short BLE advertising buffer).

Ordinary observations use the existing `wifi` and `ble` records with `match: []`
and no `rule`. Matched observations carry reasons and the first matched rule.
Only matching observations increment signature-hit counters or trigger alerts.
The configured RSSI threshold controls matching/alerts, not observation export.
Disabling a radio in configuration suppresses its observations; stopping scanning
suppresses processing of newly consumed scan events. Already queued output may
still drain. Decoded drone records supplement the original observation; consumers
must not count these two representations as two distinct devices.

This expands the v1 schema to allow empty match arrays and adds `stream_stats`.
Existing clients that assume every WiFi/BLE message is an alert must check match
metadata instead. Strict validators need the updated schema. Desktop hostd's own
scanner output is unchanged; connecting it to embedded sensors remains separate
work. Examples are in `schemas/examples/*-observation.device.json`.

## Delivery and loss

Radio callbacks and event processing never wait for a slow consumer. Existing
bounded queues are retained; no framebuffer, larger heap, or growing history is
introduced. A `stream_stats` record is attempted every 30 seconds:

- `scan_drops`: parsed observations rejected by the scan queue.
- `output_drops`: serialized records rejected by the shared output queue.
- `ble_output_drops`: records rejected by the BLE output queue; no_std also
  counts records discarded at disconnect and records whose notification fails.
- `serialize_errors`: observation/drone records that could not fit the wire buffer.

Counters are cumulative wrapping u32 values since boot. Subtract successive
samples modulo 2^32 within one boot; reset the baseline after reboot. Stats are
also best-effort and may be lost during saturation. They do not count frames
missed by the radio, unsupported/malformed input rejected by parsers, or prove
that a remote client received a BLE notification. ESP-IDF notification delivery
errors are not measurable through these queue counters.

No connected client means BLE forwarding is inactive, not a reported drop.
Serial still emits records at 115200 baud, wrapped by diagnostic log prefixes;
a USB reader must strip log framing and ignore non-JSON diagnostics. BLE records
are split into 20-byte notifications with newline padding at the end. Reassemble
through the terminating newline and ignore empty lines; discard partial records
when the connection ends. A slow BLE subscriber or busy RF environment can exceed
transport capacity. This is broader visibility, not lossless RF capture.

Raw observations include nearby identifiers. Keep captures private; publish
aggregate validation results only.
