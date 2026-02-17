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

### Quick Start

Pre-built binaries are available on the [Releases](https://github.com/dougborg/AirHound/releases) page. To flash:

```bash
# 1. Install espflash
cargo install espflash --locked

# 2. Connect your board via USB and flash
espflash flash --chip esp32s3 airhound-xiao.bin             # XIAO ESP32-S3
espflash flash --chip esp32 airhound-m5stickc.bin           # M5StickC Plus2

# 3. Connect from a companion app over BLE, or monitor serial output
espflash monitor --speed 115200
```

AirHound starts scanning immediately on boot. Matched devices appear as NDJSON on both BLE GATT notifications and serial.

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
# Flash pre-built binaries (espflash auto-detects serial port)
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
