# External scanner process protocol, version 1

The MIT host launches an explicitly configured receiver executable directly,
without a shell. The GPL receiver stays a separate process. Its stdout is a
bounded NDJSON protocol; stderr is drained separately and is never interpreted
as observations. Host stdin remains available for AirHound commands.

The child sends `{"type":"hello","protocol_version":1}` before any observations.
It then sends `status` with state `running`, `paused` or `recovering`, and
[beacon-v1 records](external-wifi-v1.md) while running. Every record, including
its newline, fits in 16 KiB. Controls sent on child stdin fit in 512 bytes:

```json
{"cmd":"pause","request_id":1}
{"cmd":"resume","request_id":2}
{"cmd":"status","request_id":3}
{"cmd":"stop","request_id":4}
```

Request IDs are unsigned integers or strings. A command acknowledgment echoes
its ID. Periodic statuses without that ID cannot acknowledge the command.
Unknown commands, oversized/truncated records, invalid IDs and stdin I/O errors
cause cleanup and unsuccessful exit; clean stdin EOF requests orderly stop.

Pause closes and reads back receive/transmit/ACK/beacon/DMA gates before reporting
`state:"paused",cleanup_verified:true`. Resume initializes hardware, discards
old queued packets and starts a fresh observation session with sequence zero
before reporting running. Coalesced host stop/start changes must still execute
this pause/resume barrier. Queued events carry the host control generation so
an observation accepted before pause cannot be forwarded after resume.

`status` includes cumulative counters for transfers, decoded frames, beacons,
malformed transfers, dropped observations and reconnect attempts. The producer
uses bounded queues; observations can be dropped under backpressure and those
losses are counted. Reserved queue space is used for controls, not unlimited
buffering. Host queue drops are counted separately. Neither status nor unknown
RSSI is converted into a synthetic signal measurement.

On stop, the child attempts every cleanup gate and emits a final `stopped`
record with the correlated request ID, final counters and `cleanup_verified`.
Success requires verified cleanup, successful exit and child reaping. Unplugged
hardware cannot produce verified register readbacks. The host has bounded
startup/acknowledgment/shutdown deadlines; forced termination is an error, not
verified radio cleanup. A malformed stream also enters the cleanup/reap path.

USB removal enters bounded recovery with backoff. Reconnection starts a new
session; exhausted attempts or an unverified stop fail. This state machine has
simulated tests; physical disconnect/reconnect and sleep/wake remain separate
hardware release gates.

MCU command timeouts also enter recovery, but only after verified radio shutdown.
At most two such recoveries are permitted over the whole process lifetime; a
third timeout terminates the receiver. Status and stopped records include the
cumulative `mcu_timeouts` counter. Recovery does not establish firmware health:
the soak harness fails acceptance if this counter is nonzero, even when the
receiver resumes. Persistent startup timeouts require operator investigation
and may require an unplug/replug; the process does not retry indefinitely.

## Host observations and export policy

The host emits `external_wifi_observation`, retaining the original nullable RSSI,
estimate basis, capture timestamp, SSID bytes and channel metadata. It adds
identity-based MAC/SSID matches, rules, host control generation and
`filter_policy:"identity_only_unvalidated"`. The RSSI threshold does not apply
to this explicitly labeled identity-only path. Non-UTF-8 SSIDs remain bytes;
they are not lossy-decoded into potentially misleading signature matches.

These observations do not enter the numeric-RSSI firmware protocol or WiGLE
CSV. `wigle_exported:false` records that decision. Native WiFi/BLE behavior and
their signal thresholds are unchanged. A future estimated-signal export policy
must be explicit and preserve provenance; this prototype has no such opt-in.

## Example

From `hostd`, using absolute paths for the independently built receiver and
externally supplied firmware:

```sh
cargo run --locked -- --no-ble \
  --external-scanner /absolute/path/to/mt7610u-lab \
  --external-firmware /absolute/path/to/mt7610e.bin \
  --external-channels 1,6,11,48 --external-dwell-ms 500 \
  --output /new/path/native-observations.csv
```

The external backend replaces native WiFi scanning. With BLE disabled, the CSV
contains its header only; external observations and scanner health are on
stdout. AirHound `stop`/`start` commands on stdin control receiver pause/resume.
SIGINT/SIGTERM shut down and reap the receiver. Raw stdout contains nearby
network identifiers and should be stored privately.

## Physical USB selection

The helper accepts `--usb-selector usb:<bus>:<port>[.<port>...]` for live commands.
`list-devices` enumerates matching descriptors and port topology without opening
handles. Bus numbers are 0–255, ports 1–255 with at most seven levels; canonical
notation forbids leading zeros. USB addresses are diagnostic only and may change
on reconnect. Missing or ambiguous targets fail; another adapter is never chosen
as a fallback. An implicit single-device selection is pinned for subsequent opens
within that helper process.

Status/stopped messages add `usb_device` (nullable before first open), with
`selector`, `bus`, `ports`, `address`, and
`identity_scope: "usb_attachment_topology"`. Probe/bootstrap/capture summaries
also report this identity. Hostd exposes the confirmed `usb_selector` in
`external_scanner_status` and rejects a mismatch or an identity change during a
session. Once an identity has been learned, every later status and stop acknowledgement
must confirm it, including pause/recovery barriers. An explicitly requested
selector must be confirmed before running. Before any device has opened, null
identity is allowed for non-running states. Older identity-less helpers remain
compatible with unselected single-device sessions only.

A port selector is not hardware authentication. Another matching adapter placed
in the same port can satisfy it; USB bus numbering may change after reboot.
Separate processes rely on OS interface-claim exclusion. Hostd rejects duplicate
selectors within its session, but does not claim global ownership coordination.
