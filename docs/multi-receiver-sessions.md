# Multi-receiver sessions

`airhound-hostd --session-config session.json` runs explicitly selected receivers
concurrently through the shared observation and detection pipeline. Each receiver
has its own health state and ingress queue. NDJSON records retain the receiver
that supplied them, even when several receivers observe the same address.

This is source-build functionality. The published 0.2.0 preview archives do not
include it. Build with `just build-hostd`; on macOS rebuild the application bundle
with `just bundle-hostd` to retain permission attribution.

## Start a session

For the macOS native WiFi and system-managed BLE receivers, save this as
`session.json`:

```json
{
  "version": 1,
  "receivers": [
    { "id": "mac-wifi", "source": { "kind": "native_wifi" } },
    { "id": "mac-ble", "source": { "kind": "native_ble" } }
  ]
}
```

Then run from the repository root:

```sh
hostd/AirHound.app/Contents/MacOS/airhound-hostd \
  --session-config session.json \
  --output observations.csv > observations.ndjson
```

Use `hostd/target/release/airhound-hostd` on Linux, with Linux-compatible sources
in the file. The [Linux example](examples/session-linux.json) combines two
explicit BlueZ controllers with a monitor-mode WiFi interface. The
[macOS/CatSniffer example](examples/session-macos-sniffle.json) combines native
WiFi, native BLE, and a separate Sniffle receiver. **Edit adapter names, serial
ports and paths before running either example.** Sources are not auto-discovered
or flashed. Relative paths resolve from the process working directory, not the
configuration file's directory; shell `~` and environment variables are not
expanded inside JSON strings.

Session files are limited to 64 KiB, version `1`, and 1–16 receivers. Receiver IDs
must be unique, 1–64 ASCII letters, digits, hyphens, underscores or periods.
Unknown fields and unsupported source kinds are rejected. A configuration error
rejects the plan before starting radios; a later hardware or permission failure
is reported for the affected receiver.

Do not combine `--session-config` with individual source-selection flags such as
`--ble-adapter`, `--sniffle-port`, `--external-scanner`, `--wifi-interface`,
`--no-ble` or `--no-wifi`. Select all sources in the file. Session-wide options
such as `--output`, `--rssi`, verbosity and `--gpsd` still apply. Existing files
remain protected unless `--output` and `--overwrite` are both supplied.

## Receiver kinds

| `source.kind` | Configuration | Scope |
| --- | --- | --- |
| `native_ble` | Optional `adapter` | Linux BlueZ adapter ID such as `hci0`; omit on macOS, where CoreBluetooth manages selection |
| `native_wifi` | No additional fields | macOS CoreWLAN scans; OS controls channels and scan timing |
| `linux_wifi` | `interface` | Existing Linux monitor-mode interface; configure mode and channel externally |
| `sniffle` | `port`, `python`, `helper`; optional `module_path`, `channel`, `coded` | Separate Sniffle process and serial receiver; see [firmware/host prerequisites](sniffle-source.md) |
| `external_wifi` | `executable`, `firmware`, `channels`, `dwell_ms` | Standalone MT7610U receiver process; see [protocol and limitations](external-scanner-stream-v1.md) |
| `ble_pcap` | `path` | Finite classic DLT256 BLE PCAP replay; see [supported framing](sniffle-source.md#replay) |

For Sniffle, `python` and `helper` are required in a session file, even though the
single-source CLI provides a default Python executable. `coded` defaults to false.
An omitted `module_path` uses the Python environment's normal package lookup.

Multiple native BLE receivers require explicit distinct adapter IDs. The macOS
system-managed receiver cannot be selected multiple times. Duplicate native WiFi,
monitor-interface, BLE-selector and Sniffle-port selections are rejected; existing
serial symlinks resolve to a common path for this check. macOS `/dev/cu.*` and
`/dev/tty.*` names for the same serial device are also treated as aliases rather
than separate receivers.

Before starting Linux sources, AirHound resolves each monitor interface's
`/sys/class/net/<interface>/phy80211` link to identify its physical WiFi radio.
The plan is rejected if two interfaces resolve to the same radio or if the
physical identity cannot be established. Interface names are limited to 15 ASCII
letters, digits, hyphens, underscores or periods; `.` and `..` are rejected.
These checks do not inventory shared antennas, composite-device RF constraints,
or every possible USB/serial alias. Check hardware topology before treating two
entries as independent radios.

Only one `external_wifi` entry is accepted: the current MT7610U process protocol
has no physical-device selector. Two copies could otherwise claim the same radio.
Its channel list must contain distinct supported receive channels, with
`dwell_ms` between 100 and 10000. Existing regulatory and hardware validation
limits remain in the [MT7610U channel guide](mt7610u-channels.md).

## Channel assignments

Sniffle receivers can pin primary advertising channel `37`, `38` or `39`. With
`channel` omitted, the plan distributes automatic assignments among those three
channels, accounting for pinned assignments. Overlap is possible once all three
channels are assigned; intentional manual overlap is also accepted.

Each Sniffle receiver listens to either 1M or Coded primary advertisements, not
both at once. Secondary following remains enabled and can take that receiver
away from its primary channel. Live Coded-PHY acceptance and extended-chain
reassembly remain separate follow-ups. Three receivers therefore do not establish
gap-free coverage of every BLE transmission.

Health reports expose `requested_assignment`. It is a requested plan, not an
acknowledged measurement of the current RF channel. OS-managed sources remain
OS-managed, and Linux monitor channels remain externally configured. This first
implementation does not redistribute WiFi channels, retune receivers after a
failure, or schedule shared antennas. Capability-aware scheduling is tracked in
[#229](https://github.com/dougborg/AirHound/issues/229).

## Observation identity and output

The stream begins with `session_started`, including `schema_version: 1`, a
`session_id` and receiver count. Receiver-derived NDJSON objects retain their
existing type and payload, with an additive field:

```json
{
  "session_source": {
    "session_id": "1234-1795310000000000000",
    "receiver_id": "catsniffer",
    "ingress_sequence": 42
  }
}
```

`ingress_sequence` starts at zero for each configured receiver. It orders host
ingress events, not RF transmissions. One ingress event may produce multiple
records, including an observation and detection evidence; those share the same
sequence. Some ingress events produce no output, so gaps alone do not measure
packet loss. Existing backend sequence, generation, capture timestamps and source
fields remain intact. Cross-receiver delivery order is not a synchronized clock.

Repeated and cross-receiver observations are retained. Matching an address across
receivers does not establish permanent physical-device identity, triangulation,
or independent evidence. Address rotation and backend cache behavior still apply.
The shared filter configuration applies to all receivers; matching policy does
not upgrade incomplete payloads or unvalidated radio metadata. Native WiFi emits
`wifi_observation` for ordinary observations as well as matches; existing matched
`wifi` and detection-evidence records remain additional outputs for compatibility.
Do not count an observation and its associated match records as separate captures.

WiGLE CSV remains the existing combined address-bearing export, without a receiver
column. Keep NDJSON alongside it when source attribution matters. Existing export
rules still exclude UUID-only BLE and unvalidated external WiFi signal estimates.
Replay preserves historical capture time and never receives a current GPS fix.
Live receivers share the session location provider; use that only for receivers
co-located with that provider.

## Health, pause and failure

The existing stdin `status`, `stop` and `start` commands apply to the whole
session. `scanner_health.receivers` exposes each receiver's scanner state,
receiver metadata when available, requested assignment and queue policy.
`receivers[].scanners` is authoritative; multi-receiver health omits the old
top-level `scanners` aggregate rather than collapsing different radio states. A
start/stop acknowledgment reports the request, not proof that every radio has
completed the transition. Backend-specific pause behavior remains documented in
the [desktop guide](../hostd/README.md#scan-cadence-and-lifecycle).

Each receiver has a 32-event ingress queue, followed by a shared 32-event queue.
The relay backpressures on the shared queue. Backend ingestion policies differ:
some await capacity, while others report drops or retain only available events.
That does not make RF acquisition lossless: native capture callbacks, OS buffers,
USB/serial transport and helper processes can lose observations while downstream
output is slow. Backend loss counters remain separate; unknown radio loss remains
unknown. A slow output sink can eventually backpressure every receiver.

A source error is retained and reported while the other sources continue.
Replay EOF finishes that source without restarting the file. When every source
finishes, the session drains output and exits. Reconnection remains each backend's
existing behavior; the supervisor does not universally restart failed receivers.
Output failure ends the session and requests cleanup of all receivers. A session
with a recorded source error returns failure when it finishes, even if other
receivers produced useful observations.

Ctrl-C requests cooperative shutdown. The session permits up to five seconds for
receiver cleanup, or forty seconds when an external MT7610U receiver is present.
An expired cleanup deadline is an error and explicitly leaves radio cleanup
unverified; it is not reported as a successful stop.

## Application integration

The public Rust library exposes `airhound_hostd::session` configuration types and
plan validation. Receiver execution, supervision and observation processing still
live in the daemon executable; they are not an embeddable GUI engine API. A client
can launch the daemon with a session file, read NDJSON stdout and send protocol
commands on stdin. Extracting a reusable execution API remains future work.

## Remaining source integrations

This mode does not yet ingest M5/XIAO firmware serial or GATT streams, control the
Flipper/Marauder companion, or add Classic Bluetooth and sub-GHz reception.
Saved Marauder capture import remains a separate tool. Consult the
[Flipper prototype](../companions/flipper/README.md),
[firmware streaming contract](firmware-streaming.md) and
[Marauder importer](marauder-import.md) for their current boundaries.
