# AirHound desktop scanner

[Project home](../README.md) · [Documentation index](../docs/README.md) · [Command protocol](../docs/protocol.md)

`airhound-hostd` scans using the host OS, records address-bearing observations in
WiGLE CSV, and writes NDJSON to stdout. It has a CLI interface; the macOS `.app`
bundle provides application identity and permission descriptions, not a GUI.

Preview downloads include macOS Apple Silicon and Linux x86-64 archives. Check
the [release asset inventory](../docs/release-status.md) for OS/runtime requirements,
checksum verification, signing limitations, and separate receiver/source packages.
Extract the archive and run `AirHound.app/Contents/MacOS/airhound-hostd --help`
on macOS or `bin/airhound-hostd --help` on Linux from a terminal.

## Build and run

Run the recipes below from the repository root with stable Rust and `just` installed.
The recipes enter `hostd/`, which selects its own stable toolchain:

```sh
just build-hostd             # macOS or Linux
just bundle-hostd            # macOS only
hostd/AirHound.app/Contents/MacOS/airhound-hostd --output observations.csv
```

On macOS, allow Bluetooth and Location Services. If permission attribution fails
when launching from a terminal, launch the actual app through Launch Services:

```sh
open -n hostd/AirHound.app --stdout /tmp/airhound-events.ndjson \
  --stderr /tmp/airhound.log --args --output /tmp/airhound-observations.csv
```

The bundle contains a copied executable, not a symlink. Rebuild it after changing
code. Check System Settings → Privacy & Security → Location Services if WiFi
addresses are redacted. Location fields are empty when no valid recent fix exists.

Linux needs BlueZ, D-Bus development headers and libpcap headers (Ubuntu:
`libdbus-1-dev libpcap-dev pkg-config`). WiFi is opt-in with `--wifi-interface wlan0mon` on an
externally configured monitor-mode interface with capture permission. Linux
location is available through the optional gpsd provider. The default Linux configuration scans BLE only.

## Multiple receivers

Use `--session-config session.json` to run several explicitly selected native,
Sniffle, Linux monitor, external MT7610U, or replay sources together. Each has its
own health and bounded ingress queue; NDJSON records retain source identity.
The [multi-receiver guide](../docs/multi-receiver-sessions.md) includes configuration
examples, channel-assignment behavior, lifecycle and loss limits. This mode is
source-build functionality and is not in the published 0.2.0 preview archives.

## Receiving detector-device data

`hostd` currently scans native host radios or supervises the external receiver
process described below. It has no M5/XIAO serial-port or BLE GATT client mode.
Its stdin accepts commands for its own scanning session, not firmware events.

Embedded firmware does emit ordinary and matched observations over serial/BLE.
Use a separate consumer following the [firmware streaming contract](../docs/firmware-streaming.md),
or `scripts/monitor-firmware.py` for bounded serial validation. That script writes
private captures and aggregate diagnostics; it does not feed hostd or export WiGLE.
For no_std M5 firmware select `--baud 460800`; other firmware builds use 115200.

## Saved Marauder / AWOK Wi-Fi captures

The experimental source-build `airhound-marauder-import` tool imports saved
v1.21.0-profile PCAP or binary serial captures through shared identity rules.
It preserves raw packets and unknown radio metadata; hardware acceptance and live
USB ingestion remain pending. See [usage, framing and limits](../docs/marauder-import.md).

## CatSniffer / Sniffle packet source

Use `--sniffle-port` with the separate `scripts/sniffle-source.py` helper for live
passive BLE capture, or `--ble-pcap` to replay classic DLT256 captures. These
single-source flags replace native BLE in the session and feed typed observations and the
shared signature/evidence engine. See [setup, output and limits](../docs/sniffle-source.md).
With `--session-config`, Sniffle and native BLE can run as separate receivers.
Complete advertiser packets enter CSV; all captured packets remain in NDJSON.
Extended-chain reassembly and live Coded-PHY validation remain follow-ups.

## External GNSS location

Use `--gpsd 127.0.0.1:2947` to geotag WiGLE rows from an external receiver on
macOS or Linux. Optional `--gpsd-device` selects one gpsd device. Fresh valid fixes
are required; unavailable positions remain blank without CoreLocation fallback.
See [GNSS setup, validity, and wardriving limits](../docs/gnss-wardriving.md).

## Observation identity and export

CoreBluetooth supplies a host-scoped peripheral UUID, not a public MAC API.
The standard macOS build additionally attempts address lookup through
`corebluetooth-address` and Apple's private `retrieveAddressForPeripheral:`
selector. Discovery ingestion runs independently of address enrichment. If the
bounded enrichment queue fills, observations are emitted immediately with their
UUIDs instead of waiting for an address. A single native worker uses a per-call
autorelease pool and a 25 ms response budget. Missing addresses, unsupported
selectors, errors and timeouts also retain UUID-only records. Addresses are
looked up again on subsequent observations rather than cached indefinitely.
Enrichment can change delivery order; resolving an address does not emit a
second copy of an observation.

Every such desktop BLE observation is emitted as `type: "ble_observation"`, with
`peripheral_id`, `identity_scope`, name, RSSI, service UUIDs, manufacturer ID,
match reasons and uptime. Successful lookups add `address` and `address_source`.
See [the schema](../schemas/desktop-ble.v1.schema.json) and
[example](../schemas/examples/ble-address.desktop.json).

The observed address may rotate. Its public/random type is unknown, so these
records explicitly report `address_type: "unknown"`; they are not used for MAC/OUI
matching. Name, service and manufacturer signatures still apply. The peripheral
UUID itself is not a permanent cross-machine device identity.

WiGLE CSV receives only observations with an actual address. UUID-only BLE records
remain in NDJSON; no all-zero or fabricated MAC is exported. Thus, capture stdout
as well as CSV when complete desktop BLE history is required. Existing `wifi`,
`ble`, `drone`, `status` and `command_result` protocol messages retain their schema.
Native address-bearing BLE observations on Linux retain the existing `ble` match
messages and CSV behavior.

Private API use can break on OS updates and is unsuitable for a Mac App Store
build. Exclude it with:

```sh
just bundle-hostd-public-api
# Equivalent build: cargo +stable build --manifest-path hostd/Cargo.toml --release --locked --no-default-features
```

That build keeps UUID-based BLE records and WiFi scanning. It omits the resolver
dependency; this switch alone does not establish App Store readiness.

## Structured BLE observations and receiver selection

`--list-ble-adapters` lists receiver diagnostics without scanning or creating a
CSV. On Linux, `--ble-adapter hciN` selects a listed controller and fails if it is
absent; CoreBluetooth selection is system-managed. See the
[receiver guide](../docs/ble-receivers.md) for capability evidence and limits.

Every production BLE snapshot now carries bounded full service UUIDs and
company/service-keyed payload records in the additive `advertisement` object.
Records expose capacity truncation, `possibly_cached` freshness and unknown
packet completeness. Existing numeric `service_uuids` and legacy Linux matched
`ble` messages remain compatible. When an update arrives without available
RSSI, `advertisement.rssi_available` is false; the legacy outer zero is a
placeholder, not a measured signal, and no CSV or signature hit is generated.

Available Remote ID service data is independently decoded into `ble_remote_id`,
including UUID-only peripherals and update-only records without measured RSSI.
These records omit unavailable addresses/RSSI. See the
[Remote ID guide](../docs/desktop-remote-id.md) and
[shared contract](../docs/ble-observation-contract.md). These software paths do
not enable extended or Coded-PHY reception on unsupported hardware/APIs.

## Scan cadence and lifecycle

BLE discovery is continuous, with duplicate advertisements delivered by the OS.
On macOS, WiFi scan requests are sequential with a one-second wait after each
result. CoreWLAN controls scan duration and may return repeated snapshots; CSV
rows do not prove fresh RF sweeps. Linux capture listens continuously on the
externally selected channel and drops events if its bounded queue fills.

NDJSON commands are accepted on stdin: `{"cmd":"stop"}`, `{"cmd":"start"}`,
`{"cmd":"status"}` and the configuration commands in the [protocol guide](../docs/protocol.md#command-results-and-runtime-support).
`stop` pauses processing, stops BLE discovery and prevents new WiFi scan requests.
An already-running macOS WiFi call cannot be canceled; its results are discarded
across pause/resume. Start/stop acknowledgments report the requested state, not
hardware completion. Ctrl-C requests cooperative scanner cleanup, allows up to
one second for scanner tasks, then cancels any task that has not finished and
drains/flushes queued output. Native WiFi/address workers do not hold runtime
shutdown open.

Use `--rssi`, `--no-ble`, `--no-wifi`, and `-v`/`-vv` to configure scanning and
logging. `RUST_LOG` overrides verbosity. Existing CSV files are protected unless
both `--output` and `--overwrite` are supplied. CSV flushes every five seconds and
on shutdown; repeated observations are retained. CoreWLAN and btleplug do not
expose complete radio frames. Linux monitor capture feeds raw WiFi and Remote ID
through the shared parsers. WiFi channel conversion covers 2.4/5 GHz, not 6 GHz.

## Tests and performance

Both desktop platforms enforce 81% production-line coverage. Tests cover session
lifecycle, command handling, output failures, adapter conversion and Linux offline
PCAP capture. The default macOS build and public-API-only build are tested in CI.
Live permissions, radio capture and location delivery require hardware checks.
Profiler regressions use a simulated process and clock; run them with
`python3 -B -m unittest discover -s scripts/tests -v`. CI runs the same suite.

```sh
just bench-hostd
# Alternate build:
python3 scripts/benchmark-hostd.py --no-default-features
```

The opt-in release benchmark writes `hostd/target/performance/benchmark.json`,
including commit, dirty-tree status, toolchain, OS, architecture, samples and
summary statistics. It runs three 100,000-event batches for each identity mode:
hardware MAC and platform UUID. Each batch contains 20% matching BLE and 80%
unmatched WiFi. It exercises the production bounded channel, filtering, timestamp
conversion, CSV and NDJSON output, then validates row counts outside the timer.
Temporary observation files are removed. Writes are buffered/cached, without
`fsync`; results exclude live radio acquisition, private address lookup and GUI
work. Compare repeated runs on the same hardware; do not treat events/s as scan
frequency or a battery measurement.

The Desktop performance workflow runs on relevant PRs, weekly and manually on
Linux/macOS, publishes job summaries, uploads JSON artifacts and verifies macOS
packaging. Timing is informational: there is no performance threshold on shared
runners. Benchmark failures and incorrect output counts still fail the job.
Raw live observations and local measurement artifacts belong under the ignored
`hostd/target/performance/` directory and must not be committed or uploaded to CI.

For a bounded live macOS run after granting permissions:

```sh
just profile-hostd
# Already built: python3 scripts/profile-macos.py --seconds 45 --mode all
# In-flight shutdown check: python3 scripts/profile-macos.py --seconds 1 --mode wifi
```

The profiler launches the bundle through Launch Services, samples process CPU/RSS,
sends status/stop/start commands at 0/15/25 seconds, then requests SIGINT shutdown.
It verifies the process identity before signaling it. Early process exit, missing
or failed scheduled command acknowledgments, and forced termination fail the
profile. Short runs validate only commands scheduled within their duration; a
quiet radio alone does not cause failure. If the process exits before SIGINT,
shutdown time is reported as unavailable rather than zero. The report includes
address-lookup success counts, location availability and shutdown time. Reply timing includes up to 5 ms of file
polling latency. Raw CSV, NDJSON and diagnostic logs stay in a private local run
directory under `hostd/target/performance`; the script prints aggregate results.
Permission prompts may affect startup. CPU is sampled process CPU, not system
power consumption. Do not profile concurrently with compilation for comparisons.

## External receiver capture validation

The independent [MT7610U receiver experiment](../drivers/mt7610u/README.md)
exports [version 1 Wi-Fi observation files](../docs/external-wifi-v1.md).
Validate a completed file without opening a radio or requesting location:

```sh
cd hostd
cargo run --locked --bin airhound-capture-check -- /path/to/beacons.ndjson
```

This MIT utility validates bounded records, session/sequence consistency and
signal provenance, then prints aggregate counts without network identifiers.
It does not link the GPL driver, launch a scanner, ingest observations into the
normal hostd pipeline, run detection matching or export WiGLE CSV. The receiver's
signal estimates remain unvalidated; the live receiver integration below preserves this distinction with its own
observation type. This file-checking utility remains a separate offline path. Ordinary `cargo run` still selects
`airhound-hostd`.

## Live external receiver

Use `--external-scanner /absolute/path/to/mt7610u-lab` with
`--external-firmware /absolute/path/to/mt7610e.bin` to replace native WiFi with
the standalone receiver. `--external-channels 1,6,11,48` and
`--external-dwell-ms 500` select its channel plan. The daemon supervises the
child directly, validates its bounded stream, and propagates stop/start and
shutdown through correlated acknowledgments and verified cleanup.

External observations retain their capture timestamp, binary SSID, nullable
RSSI and unvalidated estimate. They appear on stdout as
`external_wifi_observation` with `filter_policy: identity_only_unvalidated`,
MAC/SSID matches and explicit `wigle_exported: false`. The normal RSSI threshold
does not apply to this labeled identity-only path. No estimates are silently
inserted into WiGLE CSV or the firmware protocol. See the
[stream protocol and example](../docs/external-scanner-stream-v1.md).

## Linux WiFi capture

Supply an existing monitor-mode interface, with capture permission:

```sh
cargo +stable run --manifest-path hostd/Cargo.toml -- \
  --wifi-interface mon0 --no-ble --output capture.csv
```

Configure interface mode and channel externally. AirHound does not disconnect
NetworkManager, change interface mode, hop this interface's channel or transmit
WiFi packets. Without `--wifi-interface`, Linux runs BLE only.

The adapter requires radiotap channel and dBm signal fields. It supports 2.4 GHz
and 5 GHz channels 36–177; 6 GHz awaits explicit band metadata. Truncated packets,
failed CRCs, padded payloads and missing/unsupported metadata are discarded.
Capture and the bounded event queue are best effort under load. Parsed WiFi
observations use the shared filter and WiGLE writer; Remote ID
NAN/beacon/probe-response payloads also emit `drone` messages. Security fields
remain unknown; optional gpsd supplies valid location when configured.
Hardware/driver compatibility and RF loss still need live validation.
