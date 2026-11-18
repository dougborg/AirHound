# AirHound

Open surveillance detection toolkit: a portable Rust library, [standardized signature format](schemas/signatures.v1.schema.json), and [documented companion event protocol](schemas/device-message.v1.schema.json) for WiFi and BLE surveillance device detection. ESP32 firmware provides a ready-to-flash reference implementation; the same library and formats support Linux daemons, Kismet integrations, and any tool that adopts the schemas.

## Why AirHound

Surveillance cameras, ALPR systems, and related RF devices are proliferating in public spaces. Several community projects have built tools to detect them (see [Related Projects](#related-projects) below), each combining scanning, analysis, alerting, and UI into a single firmware.

AirHound takes a different approach: **separation of concerns**, built around three portable deliverables:

1. **Signature database** — the most complete open-source collection of surveillance device identifiers (MAC OUI, SSID, BLE name/UUID/manufacturer), in a [standard JSON format](schemas/signatures.v1.schema.json) any tool can import ([#11](https://github.com/dougborg/AirHound/issues/11))
2. **Companion event protocol** — a documented NDJSON wire format with [JSON Schemas](schemas/) for detection events and host commands, transport-agnostic ([#9](https://github.com/dougborg/AirHound/issues/9))
3. **Detection library** — portable Rust crate (`no_std`, no platform dependencies) that connects signatures to scan data, testable on any host

Each layer is independently useful. The ESP32 firmware wires all three together with real radio hardware, but a Linux daemon ([#13](https://github.com/dougborg/AirHound/issues/13)), Kismet companion ([#12](https://github.com/dougborg/AirHound/issues/12)), or any other tool can consume the same signatures and speak the same protocol. Analysis, scoring, GPS tagging, and storage live in companion apps — the detection layer stays thin and focused. See [#17](https://github.com/dougborg/AirHound/issues/17) for the full architecture vision.

The goal is a community-maintained signature database and open standards that work across multiple tools, hardware platforms, and companion apps ([#16](https://github.com/dougborg/AirHound/issues/16)).

## Related Projects

AirHound builds on work by the surveillance detection community. These projects pioneered the techniques and published the device signatures that AirHound's signature database draws from:

| Project | Platform | WiFi | BLE | Output | Notes |
|---------|----------|------|-----|--------|-------|
| [Flock-You](https://github.com/colonelpanichacks/flock-you) | ESP32-S3 | — | Scan | Web UI, JSON/CSV/KML | Largest community, GPS via browser |
| [OUI-Spy](https://github.com/colonelpanichacks/oui-spy) | ESP32-S3 (custom PCB) | — | Scan | Web UI | Multi-mode: surveillance, drone, proximity |
| [FlockSquawk](https://github.com/f1yaw4y/FlockSquawk) | ESP32 (5 variants) | Promisc | Scan | Serial JSON, audio | First dual WiFi+BLE detection |
| [FlockBack](https://github.com/NSM-Barii/flock-back) | Linux (Python) | Monitor | Scan | CLI | No dedicated hardware needed |
| [ESP32Marauder](https://github.com/justcallmekoko/ESP32Marauder) | ESP32 (many) | Promisc | Scan | Serial, WiGLE CSV | Flock detection within broader toolkit |
| [DeFlock](https://deflock.org) | iOS | — | — | — | Companion app |

AirHound's approach is complementary: a portable detection library and standardized formats that separate scanning from analysis. The ESP32 firmware uses BLE GATT relay so the phone keeps its own connectivity; a Linux daemon or Kismet companion can use the same library with different radios. AirHound's signature database builds on research from these projects — the goal is a shared, community-maintained signature format that benefits everyone ([#11](https://github.com/dougborg/AirHound/issues/11)).

## Design Philosophy

- **Standard formats** — Surveillance device signatures, detection events, and host commands all have formal [JSON Schemas](schemas/). Any tool can validate, import, or produce conforming data without depending on AirHound's code.
- **Library-first** — Core detection logic in a portable `no_std` Rust library with no platform dependencies. Platform code (ESP32 firmware, Linux daemon, Kismet companion) is a thin consumer layer.
- **App-agnostic** — Open NDJSON companion protocol over any transport (BLE GATT, serial, TCP). Sensors and companion apps are interchangeable.
- **Multi-platform** — ESP32 firmware today. Linux daemon ([#13](https://github.com/dougborg/AirHound/issues/13)) and Kismet companion ([#12](https://github.com/dougborg/AirHound/issues/12)) planned, sharing the same library and formats.
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
- **Device message** — NDJSON output from sensor (WiFi match, BLE match, or status).
- **Host command** — NDJSON input to sensor (start, stop, configure).
- **Companion event protocol** — the transport-agnostic NDJSON wire format for device messages and host commands.

**WIDS (planned, [#32](https://github.com/dougborg/AirHound/issues/32)):**
- **Fingerprint alert** — single-frame security anomaly detection (e.g., malformed IE, zero WPA NONCE).
- **Behavioral alert** — multi-frame temporal detection (e.g., deauth flood, evil twin). Implemented as code modules with configurable thresholds, not declarative rules.

## Standard Formats

AirHound defines three JSON Schemas ([draft 2020-12](https://json-schema.org/draft/2020-12)) in the [`schemas/`](schemas/) directory. These are designed for cross-tool use — any surveillance detection project can adopt them independently of AirHound's Rust code.

| Schema | Description | Used by |
|--------|-------------|---------|
| [`signatures.v1.schema.json`](schemas/signatures.v1.schema.json) | Portable signature database: MAC OUI prefixes, SSID patterns, BLE names/UUIDs/manufacturer IDs, with boolean rule composition (`anyOf`/`allOf`/`not`) | Signature contributors, tool importers, runtime loaders ([#14](https://github.com/dougborg/AirHound/issues/14)) |
| [`device-message.v1.schema.json`](schemas/device-message.v1.schema.json) | Detection events emitted by sensors: WiFi matches, BLE matches, status reports | Companion apps, log analysis, data pipelines |
| [`host-command.v1.schema.json`](schemas/host-command.v1.schema.json) | Commands sent to sensors: start/stop scanning, set RSSI threshold, configure buzzer | Companion apps, CLI tools |

An [example signature file](schemas/examples/flock-raven-airtag.sigs.json) demonstrates the format with Flock Safety, Raven, and AirTag signatures.

## ESP32 Firmware

### Supported Hardware

| Board | Chip | Target | Feature Flag | Extras |
|-------|------|--------|-------------|--------|
| Seeed XIAO ESP32-S3 | ESP32-S3 | `xtensa-esp32s3-none-elf` | `xiao` | PSRAM, GPS header |
| M5StickC Plus2 | ESP32 (PICO-V3) | `xtensa-esp32-none-elf` | `m5stickc` | TFT display, buzzer |

#### M5StickC Plus2 Features

- **135x240 TFT display** (ST7789V2) — status screen with match counts, uptime, and last detection
- **Passive buzzer** (GPIO2) — short alert beep on surveillance device match, togglable via BLE command

#### Remote ID support and memory

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
page before downloading: v0.1.1 has no firmware assets. Build from source below
when no binaries are attached. Future release binaries produced by this workflow
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
espflash monitor --speed 115200
```

AirHound starts scanning immediately on boot. Matched devices appear as NDJSON on both BLE GATT notifications and serial.

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
just docker-clean            # Clean (required after dependency changes)
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

**WiFi scan result:**
```json
{"type":"wifi","mac":"B4:1E:52:XX:XX:XX","ssid":"Flock-A1B2C3","rssi":-65,"ch":6,"frame":"beacon","match":[{"type":"mac_oui","detail":"Flock Safety"},{"type":"ssid_pattern","detail":"Flock Safety camera WiFi"}],"ts":12345}
```

**BLE scan result:**
```json
{"type":"ble","mac":"58:8E:81:XX:XX:XX","name":"FS Ext Battery","rssi":-72,"mfr":2504,"match":[{"type":"ble_name","detail":"FS Ext Battery"},{"type":"ble_mfr","detail":"Known manufacturer ID"}],"ts":12346}
```

**Status report:**
```json
{"type":"status","scanning":true,"uptime":3600,"heap_free":45000,"ble_clients":1,"board":"xiao_esp32s3","version":"0.1.0"}
```

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

The library is organized in two code layers. Layer 1 is implemented; Layer 2 modules are planned.

```
                    ┌──────────────────────────────────┐
                    │          Platform Binaries        │
                    ├───────────┬───────────┬───────────┤
                    │  ESP32    │  Linux    │  Kismet   │
                    │ Firmware  │  Daemon   │ Companion │
                    │           │  (#13)    │  (#12)    │
                    └─────┬─────┴─────┬─────┴─────┬─────┘
                          │           │           │
┌─────────────────────────┴───────────┴───────────┴──────┐
│  Layer 2 (Planned) — feature-gated modules             │
│  gps · tracker · channel · export · wids               │
│  (#28)  (#29)    (#30)    (#31)    (#32)               │
├────────────────────────────────────────────────────────┤
│  Layer 1 (Implemented) — no_std, no dependencies       │
│  scanner · filter · defaults · protocol · comm · board │
└────────────────────────────────────────────────────────┘
```

### Firmware Pipeline (ESP32)

```
┌─────────────────┐    ┌──────────────────┐
│  WiFi Sniffer   │    │   BLE Scanner    │
│  (13-ch hop)    │    │  (periodic scan) │
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
│ (notify) │ │ (115200) │ │ (M5StickC)   │
└──────────┘ └──────────┘ └──────────────┘
```

## Signature Database

The signature database is defined by a formal [JSON Schema](schemas/signatures.v1.schema.json) and designed for cross-tool sharing ([#11](https://github.com/dougborg/AirHound/issues/11)). The current default signatures include data merged from multiple open-source surveillance detection projects:

- **115 MAC OUI prefixes** — Flock Safety, Silicon Labs, Axis, Hanwha, FLIR, Mobotix, and other surveillance vendors
- **SSID patterns** — `Flock-XXXXXX`, `Penguin-XXXXXXXXXX`, `FS Ext Battery`
- **BLE name patterns** — Flock, Penguin, FS Ext Battery, Pigvision
- **Raven BLE service UUIDs** — 0x3100-0x3500 (custom), 0x180A/0x1809/0x1819 (standard)
- **Manufacturer IDs** — 0x09C8 (XUNTONG / Flock Safety)

The schema supports boolean rule composition (`anyOf`/`allOf`/`not`) for complex device detections. See the [example signature file](schemas/examples/flock-raven-airtag.sigs.json) for the format, and the [signature contribution guide](CONTRIBUTING.md#adding-device-signatures) to add new devices.

## Roadmap

Directions, not promises. See [#17](https://github.com/dougborg/AirHound/issues/17) for the full architecture vision.

### Layer 2 Library Modules

| Module | Feature Gate | Description | Issue |
|--------|-------------|-------------|-------|
| `gps.rs` | `gps` | NMEA parsing, position types, geofence helpers | [#28](https://github.com/dougborg/AirHound/issues/28) |
| `tracker.rs` | `tracker` | Device history, re-identification, follow detection | [#29](https://github.com/dougborg/AirHound/issues/29) |
| `channel.rs` | `channel` | WiFi channel scheduling, dwell optimization | [#30](https://github.com/dougborg/AirHound/issues/30) |
| `export.rs` | `std` | WiGLE CSV, KML, pcapng export | [#31](https://github.com/dougborg/AirHound/issues/31) |
| `wids.rs` | `wids` | Rogue AP detection, deauth monitoring | [#32](https://github.com/dougborg/AirHound/issues/32) |

### Platform Targets

| Platform | Status | Issue |
|----------|--------|-------|
| ESP32 firmware (XIAO, M5StickC) | Implemented | — |
| Flipper Zero companion app | Planned | [#34](https://github.com/dougborg/AirHound/issues/34) |
| Linux daemon (Raspberry Pi / laptop) | Planned | [#13](https://github.com/dougborg/AirHound/issues/13) |
| Kismet companion process | Planned | [#12](https://github.com/dougborg/AirHound/issues/12) |

### Other Directions

- Cargo workspace extraction for multi-platform library reuse ([#33](https://github.com/dougborg/AirHound/issues/33))
- Flipper Zero BLE serial relay (upstream `flipperzero-rs` contribution)
- Runtime signature loading and updates ([#14](https://github.com/dougborg/AirHound/issues/14))
- GPS enrichment for boards with GPS modules ([#15](https://github.com/dougborg/AirHound/issues/15))
- Protocol v2 — versioning and expanded command set ([#9](https://github.com/dougborg/AirHound/issues/9))
- Cross-tool interoperability with OUI-Spy, FlockSquawk, Flock-You, and others ([#16](https://github.com/dougborg/AirHound/issues/16))
- More ESP32 variants (C3, C6 RISC-V) and other MCU families

## Contributing

Contributions are welcome — especially new device signatures. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on submitting signatures, adding board support, and development setup.

## License

MIT

## Testing and analysis

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

The initial production-line baselines are 89.44% for the core, 48.22% for Linux
hostd and 37.52% for macOS hostd. Enforced floors are 89%, 48% and 37%
respectively, recorded in
[`.github/coverage-thresholds.json`](.github/coverage-thresholds.json). Linux and
macOS desktop coverage run separately against their own measured floors.
These are regression guards, not quality targets. Startup, OS radio adapters,
permissions and hardware-specific paths remain major gaps; branch coverage is
not measured. New behavior should add tests rather than lower these floors.

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
