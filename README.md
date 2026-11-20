# AirHound

AirHound is a Rust toolkit for observing nearby WiFi and Bluetooth devices and
matching their advertisements against surveillance-device signatures. It includes
ESP32 firmware, a macOS/Linux desktop scanner, a portable `no_std` detection
library, and documented JSON formats.

**A signature match is a lead, not confirmed device identity.** Vendor OUIs,
standard BLE services and names can also belong to unrelated devices. Observed
address counts are separate from signature-hit counts, and changing addresses
can represent the same physical device.

## Choose a starting point

| Use case | Current implementation | Start here |
| --- | --- | --- |
| Portable detector | M5StickC Plus2 and XIAO ESP32-S3 firmware; WiFi/BLE observations, signature alerts, serial and BLE output | [Firmware setup](#esp32-firmware) |
| Scan from a Mac | CLI with CoreWLAN WiFi, CoreBluetooth BLE and optional location | [Desktop quick start](#desktop-quick-start) |
| Scan from Linux | CLI with BlueZ BLE; WiFi requires an existing monitor-mode interface | [Desktop guide](hostd/README.md) |
| External ALFA receiver | Experimental AWUS036ACHM/MT7610U passive receiver and hostd process integration; not a general WiFi driver | [Receiver guide](drivers/mt7610u/README.md) |
| Consume embedded observations | Firmware emits all parsed observations, including `match: []`; a custom serial/BLE consumer is currently required | [Streaming contract](docs/firmware-streaming.md) |
| Reuse detection logic or formats | Portable library, compiled defaults and JSON Schemas | [Architecture](#architecture), [schemas](#standard-formats) |

There is no desktop GUI yet. The macOS `.app` bundle supplies permission identity
for the CLI. `hostd` scans local radios or supervises the external receiver; it
does **not** yet connect to an M5/XIAO over serial or BLE. See the
[roadmap](docs/roadmap.md) for implementation and hardware-validation gaps.

## Desktop quick start

Install stable Rust and [`just`](https://github.com/casey/just), then run from the
repository root. Desktop recipes use `hostd`'s stable toolchain; the ESP toolchain
is only needed for firmware.

**macOS:**

```sh
just bundle-hostd
open -n hostd/AirHound.app --stdout /tmp/airhound-events.ndjson \
  --stderr /tmp/airhound.log --args --output /tmp/airhound-observations.csv
```

Allow Bluetooth and Location Services when prompted. Output files are protected
against overwriting; choose a new CSV path for another run. The standard build
attempts private-API BLE address lookup, with UUID-only fallback. Use
`just bundle-hostd-public-api` to omit that lookup; this does not by itself make
an App Store-ready application.

**Linux:** install BlueZ, `libdbus-1-dev`, `libpcap-dev` and `pkg-config`
(Debian/Ubuntu names), then:

```sh
just build-hostd
hostd/target/release/airhound-hostd --output observations.csv
# WiFi as well, with a monitor interface you have configured externally:
hostd/target/release/airhound-hostd --wifi-interface mon0 --output capture.csv
```

Linux defaults to BLE only. The process needs capture permission for WiFi;
AirHound does not configure monitor mode or hop the Linux interface's channel.
See the [desktop guide](hostd/README.md) for output semantics, permissions,
scan cadence, external receivers, benchmarks and live profiling.

Desktop BLE now retains bounded manufacturer/service payloads and full UUIDs.
Use `airhound-hostd --list-ble-adapters` for receiver diagnostics, and
`--ble-adapter hciN` for explicit Linux selection. See
[receiver capabilities](docs/ble-receivers.md), the
[observation contract](docs/ble-observation-contract.md), and
[desktop Remote ID](docs/desktop-remote-id.md). Decoding a software fixture does
not establish extended-advertisement or Coded-PHY reception.

## Why AirHound

Surveillance cameras, ALPR systems, and related RF devices are proliferating in public spaces. Community projects have developed several approaches to detecting and documenting them (see [Related Projects](#related-projects)).

AirHound takes a different approach: **separation of concerns**, built around three portable deliverables:

1. **Signature database** — a collection of community-sourced device identifiers (MAC OUI, SSID, BLE name/UUID/manufacturer), in a [standard JSON format](schemas/signatures.v1.schema.json) any tool can import ([#11](https://github.com/dougborg/AirHound/issues/11))
2. **Companion event protocol** — a documented NDJSON wire format with [JSON Schemas](schemas/) for detection events and host commands, transport-agnostic ([#9](https://github.com/dougborg/AirHound/issues/9))
3. **Detection library** — portable Rust crate (`no_std`, no platform dependencies) that connects signatures to scan data, testable on any host

Each layer is independently useful. The ESP32 firmware wires all three together with real radio hardware, and the desktop CLI uses the same library. A Kismet companion ([#12](https://github.com/dougborg/AirHound/issues/12)) remains planned. Analysis, scoring, GPS tagging, and storage live in companion apps — the detection layer stays thin and focused. See [#17](https://github.com/dougborg/AirHound/issues/17) for the full architecture vision.

The goal is a community-maintained signature database and open standards that work across multiple tools, hardware platforms, and companion apps ([#16](https://github.com/dougborg/AirHound/issues/16)).

## Related Projects

AirHound builds on work by the surveillance detection community. These projects pioneered the techniques and published the device signatures that AirHound's signature database draws from:

Related community projects include [Flock-You](https://github.com/colonelpanichacks/flock-you),
[OUI-Spy](https://github.com/colonelpanichacks/oui-spy),
[FlockSquawk](https://github.com/f1yaw4y/FlockSquawk),
[FlockBack](https://github.com/NSM-Barii/flock-back),
[ESP32Marauder](https://github.com/justcallmekoko/ESP32Marauder), and
[DeFlock](https://deflock.org). Consult their own documentation for current
hardware support and capabilities.

AirHound's approach is complementary: a portable detection library and standardized formats that separate scanning from analysis. The ESP32 firmware uses BLE GATT relay so the phone keeps its own connectivity; a Linux daemon or Kismet companion can use the same library with different radios. AirHound's signature database builds on research from these projects — the goal is a shared, community-maintained signature format that benefits everyone ([#11](https://github.com/dougborg/AirHound/issues/11)).

## Design Philosophy

- **Standard formats** — Surveillance device signatures, detection events, and host commands all have formal [JSON Schemas](schemas/). Any tool can validate, import, or produce conforming data without depending on AirHound's code.
- **Library-first** — Core detection logic in a portable `no_std` Rust library with no platform dependencies. Platform code (ESP32 firmware, Linux daemon, Kismet companion) is a thin consumer layer.
- **App-agnostic** — Open NDJSON companion protocol over any transport (BLE GATT, serial, TCP). Sensors and companion apps are interchangeable.
- **Multi-platform** — ESP32 firmware and a macOS/Linux desktop CLI share detection logic. Kismet integration remains planned.
- **Community signatures** — The signature database is designed to grow through contributions and be shared across projects ([#11](https://github.com/dougborg/AirHound/issues/11), [#16](https://github.com/dougborg/AirHound/issues/16)). See [Contributing](CONTRIBUTING.md).

## Terminology

**Data:**

- **Signature** — an atomic, stateless matching criterion (MAC OUI, SSID pattern, BLE name, raw byte pattern, etc.). Evaluated against a single frame in isolation.
- **Rule** — a named device detection that composes signatures with boolean logic (`anyOf`/`allOf`/`not`). Still stateless — combines per-frame signature matches, not temporal patterns.
- **Signature database** — portable JSON file of signatures and rules ([`signatures.v1.schema.json`](schemas/signatures.v1.schema.json)).

**Processing:**

- **Scan event** — a parsed WiFi frame or BLE advertisement, before any evaluation.
- **Filter engine** — stateless code that evaluates scan events against signatures.
- **Match** — positive result: a scan event matched one or more signatures. Contains match reasons.
- **Match reason** — a single explanation: which signature type triggered and a human-readable detail.

**Communication:**

- **Device message** — NDJSON output from a sensor: observations with optional match metadata, drone data, status, command results or stream counters.
- **Host command** — NDJSON input to sensor (start, stop, configure).
- **Companion event protocol** — the transport-agnostic NDJSON wire format for device messages and host commands.

**WIDS (planned, [#32](https://github.com/dougborg/AirHound/issues/32)):**

- **Fingerprint alert** — single-frame security anomaly detection (e.g., malformed IE, zero WPA NONCE).
- **Behavioral alert** — multi-frame temporal detection (e.g., deauth flood, evil twin). Implemented as code modules with configurable thresholds, not declarative rules.

## Standard Formats

AirHound defines JSON Schemas ([draft 2020-12](https://json-schema.org/draft/2020-12)) in the [`schemas/`](schemas/) directory. These are designed for cross-tool use — any surveillance detection project can adopt them independently of AirHound's Rust code.

| Schema | Description | Used by |
|--------|-------------|---------|
| [`signatures.v1.schema.json`](schemas/signatures.v1.schema.json) | Portable signature database: MAC OUI prefixes, SSID patterns, BLE names/UUIDs/manufacturer IDs, with boolean rule composition (`anyOf`/`allOf`/`not`) | Signature contributors, tool importers, runtime loaders ([#14](https://github.com/dougborg/AirHound/issues/14)) |
| [`device-message.v1.schema.json`](schemas/device-message.v1.schema.json) | Firmware observations, match metadata, drone data, status, command results and stream counters | Companion apps, log analysis, data pipelines |
| [`host-command.v1.schema.json`](schemas/host-command.v1.schema.json) | Commands sent to sensors: start/stop scanning, set RSSI threshold, configure buzzer | Companion apps, CLI tools |
| [`desktop-ble.v1.schema.json`](schemas/desktop-ble.v1.schema.json) | Desktop BLE observations with platform UUIDs and optional addresses | Desktop consumers |
| [`desktop-remote-id.v1.schema.json`](schemas/desktop-remote-id.v1.schema.json) | Decoded BLE Remote ID with optional hardware address and explicit snapshot provenance |

An [example signature file](schemas/examples/flock-raven-airtag.sigs.json) demonstrates conservative Flock-like and Raven UUID candidate rules. The production source of truth is now the [canonical v2 pack](signatures/builtin.v2.json), which generates the shared Rust defaults. See [pack authoring, validation and revision rules](docs/signature-packs.md).

The [shared evidence pipeline](docs/detection-evidence.md) records which signatures support each rule and applies alert policy separately. Weak clues remain visible; generic vendor/name matches do not automatically sound the buzzer.

## ESP32 Firmware

### Supported Hardware

| Board | Chip | Target | Feature Flag | Extras |
|-------|------|--------|-------------|--------|
| Seeed XIAO ESP32-S3 | ESP32-S3 | `xtensa-esp32s3-none-elf` | `xiao` | PSRAM, GPS header |
| M5StickC Plus2 | ESP32 (PICO-V3-02) | `xtensa-esp32-none-elf` | `m5stickc` | TFT display, buzzer, 2 MB PSRAM |

#### M5StickC Plus2 Features

The no_std build provides:

- **TFT display** (240×135 landscape) — Overview separates AP, other WiFi and BLE address counts from signature hits; Details shows memory and WiFi queue statistics.
- **Front button A** — switches Overview / Details.
- **Side button B** — toggles detection sound; companion buzzer commands update the same state.
- **Passive buzzer** (GPIO2) — short alert for explicit watchlists and alert-enabled rules. Quiet signature clues still update counts and the display. Settings reset at reboot.

See the [controls guide](docs/m5-controls.md) for debounce and validation details.
These controls describe the no_std runtime; do not assume ESP-IDF UI parity.
USB uses a CH9102 USB-to-UART bridge. The application console is **460800 baud**
for no_std M5 builds, **115200** for other builds. Boot-ROM output remains
115200; a different USB protocol cannot bypass this board's physical UART.

See [M5StickC Plus2 bring-up and backup procedure](docs/m5stickc-bringup.md)
for the local hardware validation and serial-monitor workflow.

#### Remote ID support and memory

The Plus2 has 8 MB flash and 2 MB PSRAM, per the
[M5Stack specification](https://docs.m5stack.com/en/core/StickC-Plus2). Our current
M5StickC `no_std` firmware tests and initializes PSRAM, then uses a separate
external-memory allocator for 4,096 observed addresses. WiFi/BLE, the RTOS,
and the scan queue retain internal RAM; the internal heap remains 64 KiB. Status
`heap_free` continues to mean internal free bytes, with external free memory
reported separately in the display and periodic memory diagnostics.

The `no_std` M5StickC firmware excludes WiFi Remote ID events to limit static
RAM use; legacy BLE Remote ID decoding remains available. The XIAO `no_std`
firmware and both ESP-IDF (`firmware-std`) builds include WiFi Remote ID events.
NAN, beacon, and probe-response payloads use a bounded 256-byte event buffer so
complete message packs survive the scan queue without relying on transmitter
MAC prefixes.

Switching to ESP-IDF does not remove the hardware's RAM limit. The `no_std`
firmware stores its scan queue statically; ESP-IDF uses a heap-backed queue and
separate thread stacks. Both need memory headroom while WiFi and BLE are active.

Extended/Coded PHY BLE Remote ID reception is tracked in
[#132](https://github.com/dougborg/AirHound/issues/132). The decoder accepts full
extended-advertisement captures, but the firmware's generic BLE event currently
retains only 62 bytes. Decoder tests alone do not establish end-to-end
long-range reception.

### Quick Start

Check the assets on the [Releases](https://github.com/dougborg/AirHound/releases)
page before downloading; a release tag alone does not imply flashable binaries.
Build from source below when no binaries are attached. Future release binaries produced by this workflow
are merged no_std images containing the bootloader, partition table and application.
ESP-IDF builds are currently CI previews, not release assets.

Download the binary for your board and `SHA256SUMS` from the same release.
Verify its checksum (`shasum -a 256 airhound-xiao.bin` on macOS or
`sha256sum airhound-xiao.bin` on Linux) against the corresponding manifest entry.
Then flash the merged binary at address zero:

```bash
# 1. Install espflash
cargo install espflash --locked

# 2. Connect your board via USB and flash
espflash write-bin --chip esp32s3 0x0 airhound-xiao.bin             # XIAO ESP32-S3
espflash write-bin --chip esp32 0x0 airhound-m5stickc.bin           # M5StickC Plus2

# 3. Connect from a companion app over BLE, or monitor serial output
espflash monitor --monitor-baud 460800 # no_std M5; other builds: 115200
```

AirHound starts scanning immediately on boot. The firmware attempts to emit every parsed WiFi/BLE observation from enabled radios as NDJSON on BLE GATT notifications and serial; signature matches are metadata, not a condition for streaming. See [firmware streaming](docs/firmware-streaming.md) for loss counters and transport limits.

Live radio coexistence, runtime memory headroom, buzzer behavior and board
flashing remain tracked in [#141](https://github.com/dougborg/AirHound/issues/141).
Successful compilation and generated images do not establish those results.

### Building

#### Docker (recommended)

No local ESP toolchain needed. Requires Docker and [`just`](https://github.com/casey/just).

```bash
cargo install just

just docker-build            # Both targets
just docker-build-xiao       # XIAO ESP32-S3 only
just docker-build-m5stickc   # M5StickC Plus2 only
just docker-clean           # Clean stale artifacts when needed
```

#### Native

Requires the ESP Rust toolchain (`espup install`) and `espflash`.

```bash
# Install toolchain
cargo install espup --locked && espup install
cargo install espflash --locked
. ~/export-esp.sh

# Build
just build-xiao
just build-m5stickc
```

#### Flash

```bash
# Flash locally built ELF files (espflash auto-detects serial port)
espflash flash --chip esp32s3 target/xtensa-esp32s3-none-elf/release/airhound
espflash flash --chip esp32 target/xtensa-esp32-none-elf/release/airhound

# Or build + flash + monitor in one step (native only)
just flash-xiao
just flash-m5stickc
```

## Protocol

AirHound uses newline-delimited JSON (NDJSON) for all communication. The format is transport-agnostic — the same messages work over BLE GATT, serial, TCP, or any stream transport. Full JSON Schemas are in [`schemas/`](schemas/).

### Device Messages (sensor -> host)

**Matched WiFi observation:**
```json
{"type":"wifi","mac":"B4:1E:52:XX:XX:XX","ssid":"Flock-A1B2C3","rssi":-65,"ch":6,"frame":"beacon","match":[{"type":"mac_oui","detail":"Flock Safety"},{"type":"ssid_pattern","detail":"Flock Safety camera WiFi"}],"ts":12345}
```

**Matched BLE observation:**
```json
{"type":"ble","mac":"58:8E:81:XX:XX:XX","name":"FS Ext Battery","rssi":-72,"mfr":2504,"match":[{"type":"ble_name","detail":"FS Ext Battery"},{"type":"ble_mfr","detail":"Known manufacturer ID"}],"ts":12346}
```

**Status report:**
```json
{"type":"status","scanning":true,"uptime":3600,"heap_free":45000,"ble_clients":1,"board":"xiao_esp32s3","version":"0.1.0"}
```

Ordinary firmware observations have the same fields with `"match": []` and no
`rule`. Check match metadata before alerting. Serial output also contains log
prefixes and diagnostics; it is not a clean bidirectional NDJSON port. Firmware
commands are received over BLE GATT, whereas hostd receives its own commands on
stdin. [Stream counters and framing](docs/firmware-streaming.md) explain bounded,
best-effort delivery and BLE reassembly.

### Host Commands (host -> sensor)

```json
{"cmd":"start"}
{"cmd":"stop"}
{"cmd":"status"}
{"cmd":"set_rssi","min_rssi":-80}
{"cmd":"set_buzzer","enabled":false}
```

### BLE GATT Service (ESP32 firmware)

| Attribute | UUID | Properties |
|-----------|------|-----------|
| Service | `4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d` | -- |
| TX (results) | `4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Notify |
| RX (commands) | `4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Write |

## Architecture

AirHound's value lives in three separable, portable layers ([#17](https://github.com/dougborg/AirHound/issues/17)):

| Layer | Description | Status |
|-------|-------------|--------|
| **Signature Database** | Portable JSON format for surveillance device signatures | v1 schema committed ([`signatures.v1.schema.json`](schemas/signatures.v1.schema.json)) |
| **Companion Event Protocol** | NDJSON wire format for detection events and host commands | v1 schema committed ([`device-message.v1`](schemas/device-message.v1.schema.json), [`host-command.v1`](schemas/host-command.v1.schema.json)) |
| **Detection Library** | Rust crate: parsing, filtering, protocol types | Implemented (`src/lib.rs`, `no_std`) |

### Library Modules

The portable library currently includes frame parsing (`scanner`, `odid`),
signature/rule evaluation (`filter`, `rules`, `evidence`, `defaults`), bounded observation and
address tracking (`observations`, `mac_index`), watchlist/proximity primitives,
button state handling, and protocol serialization (`protocol`, `comm`). Module
availability does not imply every runtime exposes every feature as a command.

| Component | Responsibility |
| --- | --- |
| [`src/lib.rs`](src/lib.rs) | Shared `no_std` detection and protocol code |
| [`src/main.rs`](src/main.rs) | Embassy/esp-hal no_std firmware |
| [`firmware-std/`](firmware-std/) | ESP-IDF firmware alternative |
| [`hostd/`](hostd/) | macOS/Linux scanning, lifecycle, location and WiGLE export |
| [`drivers/mt7610u/`](drivers/mt7610u/) | Separate GPL receiver process; not linked into AirHound |

Broader history, adaptive channel management, WIDS, Kismet integration and a
native desktop GUI remain [roadmap work](docs/roadmap.md).

### Firmware Pipeline (ESP32)

```
┌─────────────────┐    ┌──────────────────┐
│  WiFi Sniffer   │    │   BLE Scanner    │
│  (13-ch hop)    │    │  (passive scan)  │
└────────┬────────┘    └────────┬─────────┘
         │ ScanEvent            │ ScanEvent
         └──────────┬───────────┘
                    ▼
          ┌─────────────────┐
          │  Filter Engine   │
          │ MAC/SSID/BLE/UUID│
          └────────┬────────┘
                   │ NDJSON
         ┌─────────┼─────────┐
         ▼         ▼         ▼
┌──────────┐ ┌──────────┐ ┌──────────────┐
│ BLE GATT │ │ Serial   │ │ Display/Buzz │
│ (notify) │ │ (UART)   │ │ (M5StickC)   │
└──────────┘ └──────────┘ └──────────────┘
```

Serial/BLE carry observations; local signature alerts and display counters have
separate semantics. See [streaming](docs/firmware-streaming.md) and
[M5 throughput experiments](docs/m5-stream-performance.md) for queue limits,
measured behavior and remaining validation.

## Signature Database

The signature database is defined by a formal [JSON Schema](schemas/signatures.v1.schema.json) and designed for cross-tool sharing ([#11](https://github.com/dougborg/AirHound/issues/11)). The current default signatures include data merged from multiple open-source surveillance detection projects:

- **115 MAC OUI prefixes** — Flock Safety, Silicon Labs, Axis, Hanwha, FLIR, Mobotix, and other surveillance vendors
- **SSID patterns** — `Flock-XXXXXX`, `Penguin-XXXXXXXXXX`, `FS Ext Battery`
- **BLE name patterns** — Flock, Penguin, FS Ext Battery, Pigvision
- **Raven BLE service UUIDs** — 0x3100-0x3500 (custom), 0x180A/0x1809/0x1819 (standard)
- **Manufacturer IDs** — 0x09C8 (XUNTONG / Flock Safety)

The schema supports boolean rule composition (`anyOf`/`allOf`/`not`) for complex device detections. See the [example signature file](schemas/examples/flock-raven-airtag.sigs.json) for the format, and the [signature contribution guide](CONTRIBUTING.md#adding-device-signatures) to add new devices.

## Roadmap

See the [prioritized roadmap](docs/roadmap.md) and its published
[tracking issue #160](https://github.com/dougborg/AirHound/issues/160) for the
complete open-issue inventory, dependencies, evidence and release gates.

1. Integrate the locally validated firmware/receiver work and open PRs (#152),
   including M5 controls and observation streaming. Source availability on a
   branch is not a published hardware release.
2. Address firmware memory/stack/drop limits (#153), signature confidence (#154),
   and macOS live capture/permission failures (#159).
3. Validate receiver endurance/recovery (#155), channel/RF performance (#156),
   and distribution (#157).
4. Improve typed observations/health (#71) and sensor UI (#8), then evaluate a
   native desktop GUI (#158) before expanding storage and integrations.

Desktop CLI, Linux monitor-interface WiFi and command outcomes are already
merged, along with macOS lifecycle/address enrichment (#150) and the
experimental external receiver (#151). M5Stick WiFi startup, PSRAM/counting
fixes and a startup-stack CI guard are merged in #162. Hardware release
validation remains [#141](https://github.com/dougborg/AirHound/issues/141).

Directions are not delivery promises. The roadmap distinguishes implemented,
locally validated, open-review and future work; the architecture umbrella
remains [#17](https://github.com/dougborg/AirHound/issues/17).

## Contributing

Contributions are welcome — especially new device signatures. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on submitting signatures, adding board support, and development setup.

## License

MIT for AirHound. The independent [MT7610U userspace driver experiment](drivers/mt7610u/README.md)
is GPL-2.0-only and is not linked into AirHound. It has demonstrated passive
beacon reception and channel hopping across 2.4 GHz and lower 5 GHz from an ALFA AWUS036ACHM on Apple Silicon macOS;
see its README for hardware evidence, limitations and separate firmware terms.

## Testing and analysis

Desktop pipeline benchmarks run on Linux and macOS in the Desktop performance
workflow. Use `just bench-hostd` locally, or `just profile-hostd` for a macOS live
radio profile with pause/resume and shutdown measurements. See the
[desktop guide](hostd/README.md#tests-and-performance) for methodology,
permissions, and the default macOS BLE address lookup and public-API-only build.

CI runs the portable core tests, desktop tests on Linux and macOS, strict Clippy
(including core test code), formatting, schema checks, dependency audits for all
four lockfiles, workflow/shell linting, and all four firmware builds. Dependabot
covers the core, both runtimes, fuzz harness, Actions and development container.
Firmware compilation is not hardware execution. Wokwi is explicitly skipped when
its token is unavailable; live board, radio and memory checks remain in
[#141](https://github.com/dougborg/AirHound/issues/141).

Coverage uses `cargo-llvm-cov 0.9.1` and `nightly-2026-11-18`, with test bodies
excluded via `coverage(off)`. Run `bash .github/scripts/coverage.sh core` or
`bash .github/scripts/coverage.sh hostd` with that toolchain selected and
`llvm-tools-preview` installed. CI uploads HTML and JSON reports and publishes
line/function/region totals in its job summary. Coverage directories are separate
from ordinary builds; do not mix compiler versions in them.

The core production-line floor is 89%; both desktop platforms enforce 81%,
recorded in [`.github/coverage-thresholds.json`](.github/coverage-thresholds.json).
Linux and macOS run independently. Desktop tests exercise session startup,
scanner selection, event filtering and serialization, command framing, output
failures, cancellation and draining, plus adapter conversion and offline capture.
CLI subprocess tests verify argument handling and file protection before adapters
are initialized. Test bodies are excluded; OS adapter code remains in the totals.
Permission prompts, live radio scans and location delivery still require hardware
validation. Branch coverage is not measured. New behavior should add tests rather
than lower these floors.

Parser fuzzing uses `cargo-fuzz 0.13.2` with the same nightly toolchain and address
sanitizer. The `commands` target checks bounded line framing and recovery as well
as JSON parsing. The `frames` target exercises BLE, WiFi and Remote ID parsing,
seeded with valid advertisements and the public NAN capture already used by our
regression tests. To run locally:

```bash
cargo +nightly-2026-11-18 fuzz run commands -- -max_total_time=60 -max_len=4096
cargo +nightly-2026-11-18 fuzz run frames -- -max_total_time=60 -max_len=4096
```

CI runs each target for 30 seconds per PR/push, and for 10 minutes weekly.
Crashes and timeouts fail the job and their reproducer artifacts are uploaded.
Only named seed inputs are checked in; promote findings into deterministic
regression tests. A clean fuzz run does not prove absence of parser bugs.

Desktop integration tests exercise chunked command input through state changes
and status output, oversized-line rejection, output errors, and shutdown while
scanner tasks are idle or blocked on a full queue. They use in-memory streams and
scanner tasks, so they do not establish driver or physical-radio behavior.

### Command results and runtime support

Each command dequeued by a runtime emits a `command_result` after applying its
supported state changes. A `status` request also emits the existing status
message. Malformed or unknown input that reaches the command queue returns
`invalid_command` with `cmd: "unknown"`.

```json
{"type":"command_result","cmd":"start","ok":true}
{"type":"command_result","cmd":"export_wigle","ok":false,"error":"unsupported"}
```

| Command | no_std firmware | ESP-IDF firmware | hostd |
|---|---|---|---|
| start / stop / status / set_rssi | Supported | Supported | Supported |
| set_buzzer | Supported on configured buzzer boards | M5StickC only | Unsupported |
| add_watchlist / remove_watchlist | Supported, volatile | Supported, volatile | Unsupported |
| set_mode / proximity commands / enable_category / export_wigle | Unsupported | Unsupported | Unsupported |

Firmware command ingestion uses BLE GATT; hostd accepts NDJSON on stdin.
Stopping pauses detection/output; it does not promise that the radio hardware
is powered off. hostd already writes WiGLE CSV continuously, but does not
implement an on-demand export command.

Error codes are `invalid_command`, `invalid_argument`, `unsupported`,
`duplicate`, `capacity`, and `not_found`. Watchlist changes update the list and
lookup index together, reject duplicate IDs/targets, and roll back if the index
cannot accept an entry. RSSI must be between -128 and 0 dBm.

Responses have no request IDs; clients should send one command at a time and
wait for its result. BLE ingress/output queues and notifications remain bounded
and best-effort: an overlong line, full ingress queue, or disconnected client
can still lose a request/response. A missing response is not confirmation of
success. These additive messages extend the v1 schema; consumers should ignore
unknown message types when using newer firmware.

### Desktop scanner configuration

Run `cargo +stable run --manifest-path hostd/Cargo.toml -- --help` for options.
`--no-ble` and `--no-wifi` disable the corresponding scanner; at least one
must remain enabled. `--rssi -70` sets the detection threshold (default -90 dBm,
range -128 through 0); WiGLE CSV continues to record all observed networks.
`-v` enables debug logging and `-vv` trace logging on stderr; `RUST_LOG`
overrides this default. NDJSON remains on stdout.

`--output observations.csv` selects the CSV destination. Existing files are
refused unless `--overwrite` is also supplied. Without `--output`, a timestamped
filename is created with the same protection against overwriting existing data.

### Linux WiFi capture

Install `libpcap-dev`, `libdbus-1-dev`, and `pkg-config` before building hostd
(Debian/Ubuntu package names). Supply an existing monitor-mode interface:

```bash
cargo +stable run --manifest-path hostd/Cargo.toml -- \
  --wifi-interface mon0 --no-ble --output capture.csv
```

The process needs permission to capture packets on that interface. Configure
monitor mode and the channel externally using your distribution's network tools;
AirHound does not change interface mode, disconnect NetworkManager, hop channels,
or transmit packets. Without `--wifi-interface`, Linux runs BLE only.

The adapter requires radiotap headers with channel and dBm signal fields. It
supports 2.4 GHz and 5 GHz channels 36–177; 6 GHz awaits explicit band information
in the shared event model. Truncated packets, failed CRCs, padded payloads and
missing/unsupported metadata are discarded. Capture and the bounded event queue
are best effort under load. Parsed WiFi observations use the shared signature
filter and WiGLE writer; Remote ID NAN/beacon/probe-response payloads also emit
`drone` messages. Linux currently leaves CSV security and GPS fields unknown.
Hardware reception, driver compatibility and packet loss require live validation
tracked in [#141](https://github.com/dougborg/AirHound/issues/141).

Hardware receiver investigations: [CatSniffer v3.x](docs/catsniffer-integration.md)
and [AWOK Dual ESP32 v3](docs/awok-integration.md). These are integration plans,
not validated device support. Both should feed the shared observation/evidence
pipeline.
