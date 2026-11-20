# AirHound desktop scanner

`airhound-hostd` scans using the host OS, records address-bearing observations in
WiGLE CSV, and writes NDJSON to stdout. It has a CLI interface; the macOS `.app`
bundle provides application identity and permission descriptions, not a GUI.

## Build and run

Use the host Rust toolchain (`RUSTUP_TOOLCHAIN=stable` when the repository's ESP
Rust override is active):

```sh
just test-hostd
just clippy-hostd
just bundle-hostd
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
`libdbus-1-dev libpcap-dev`). WiFi is opt-in with `--wifi-interface wlan0mon` on an
externally configured monitor-mode interface with capture permission. Linux
location is not implemented. The default Linux configuration scans BLE only.

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
# Equivalent build: cargo build --manifest-path hostd/Cargo.toml --release --locked --no-default-features
```

That build keeps UUID-based BLE records and WiFi scanning. It omits the resolver
dependency; this switch alone does not establish App Store readiness.

## Scan cadence and lifecycle

BLE discovery is continuous, with duplicate advertisements delivered by the OS.
On macOS, WiFi scan requests are sequential with a one-second wait after each
result. CoreWLAN controls scan duration and may return repeated snapshots; CSV
rows do not prove fresh RF sweeps. Linux capture listens continuously on the
externally selected channel and drops events if its bounded queue fills.

NDJSON commands are accepted on stdin: `{"cmd":"stop"}`, `{"cmd":"start"}`,
`{"cmd":"status"}` and the configuration commands documented in the root README.
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
signal estimates remain unvalidated; live integration needs a host observation
model that preserves this distinction. Ordinary `cargo run` still selects
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
