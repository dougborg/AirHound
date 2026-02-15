# AirHound

Open-protocol RF wardriving companion for ESP32. Scans WiFi and BLE, filters against known surveillance device signatures, and relays matched results over BLE GATT or serial using a documented NDJSON protocol that any companion app can integrate.

## Why AirHound

Surveillance cameras, ALPR systems, and related RF devices are proliferating in public spaces. Several community projects have built tools to detect them (see [Related Projects](#related-projects) below), each combining scanning, analysis, alerting, and UI into a single firmware.

AirHound takes a different approach: **separation of concerns**. The firmware is a thin, dumb scanning dongle. It scans, filters against a compiled-in signature database, and emits matched results as structured NDJSON. All the smarts — GPS tagging, scoring, alerting, maps, storage — live in a companion app. This keeps the firmware small, portable across hardware, and app-agnostic: any app that speaks the protocol can use it.

The goal is a community-maintained signature database and an open protocol that work across multiple hardware platforms and companion apps.

## Related Projects

AirHound builds on work by the surveillance detection community. These projects pioneered the techniques and published the device signatures that AirHound's filter database draws from:

| Project | Platform | WiFi | BLE | Output | Notes |
|---------|----------|------|-----|--------|-------|
| [Flock-You](https://github.com/colonelpanichacks/flock-you) | ESP32-S3 | — | Scan | Web UI, JSON/CSV/KML | Largest community, GPS via browser |
| [OUI-Spy](https://github.com/colonelpanichacks/oui-spy) | ESP32-S3 (custom PCB) | — | Scan | Web UI | Multi-mode: surveillance, drone, proximity |
| [FlockSquawk](https://github.com/f1yaw4y/FlockSquawk) | ESP32 (5 variants) | Promisc | Scan | Serial JSON, audio | First dual WiFi+BLE detection |
| [FlockBack](https://github.com/NSM-Barii/flock-back) | Linux (Python) | Monitor | Scan | CLI | No dedicated hardware needed |
| [ESP32Marauder](https://github.com/justcallmekoko/ESP32Marauder) | ESP32 (many) | Promisc | Scan | Serial, WiGLE CSV | Flock detection within broader toolkit |
| [DeFlock](https://deflock.org) | iOS | — | — | — | Companion app |

AirHound's approach is complementary: a thin sensor that delegates analysis, GPS tagging, scoring, and storage to a companion app, written in Rust with a testable library crate, using BLE GATT relay so the phone keeps its own connectivity. AirHound's signature database builds on research from these projects — the goal is a shared, community-maintained signature format that benefits everyone ([#11](https://github.com/dougborg/AirHound/issues/11)).

## Design Philosophy

- **Thin sensor/relay** — AirHound scans, filters, and emits. The companion app handles analysis, scoring, alerting, GPS tagging, and storage.
- **App-agnostic** — Open NDJSON protocol over BLE GATT and serial. Any companion app that speaks the protocol can integrate.
- **Multi-platform** — ESP32 today, with a path toward other MCUs and a Linux daemon for laptop/Raspberry Pi wardriving.
- **Community signatures** — The filter database is designed to grow through contributions. See [Contributing](CONTRIBUTING.md).

## Supported Hardware

| Board | Chip | Target | Feature Flag | Extras |
|-------|------|--------|-------------|--------|
| Seeed XIAO ESP32-S3 | ESP32-S3 | `xtensa-esp32s3-none-elf` | `xiao` | PSRAM, GPS header |
| M5StickC Plus2 | ESP32 (PICO-V3) | `xtensa-esp32-none-elf` | `m5stickc` | TFT display, buzzer |

### M5StickC Plus2 Features

- **135x240 TFT display** (ST7789V2) — status screen with match counts, uptime, and last detection
- **Passive buzzer** (GPIO2) — short alert beep on surveillance device match, togglable via BLE command

## Quick Start

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

## Building

### Docker (recommended)

No local ESP toolchain needed. Requires Docker and [`just`](https://github.com/casey/just).

```bash
cargo install just

just docker-build            # Both targets
just docker-build-xiao       # XIAO ESP32-S3 only
just docker-build-m5stickc   # M5StickC Plus2 only
just docker-clean            # Clean (required after dependency changes)
```

### Native

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

### Flash

```bash
# Flash pre-built binaries (espflash auto-detects serial port)
espflash flash --chip esp32s3 target/xtensa-esp32s3-none-elf/release/airhound
espflash flash --chip esp32 target/xtensa-esp32-none-elf/release/airhound

# Or build + flash + monitor in one step (native only)
just flash-xiao
just flash-m5stickc
```

## Protocol

AirHound communicates using newline-delimited JSON (NDJSON) over BLE GATT notifications and serial (115200 baud).

### Device Messages (device -> companion)

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

### Host Commands (companion -> device)

```json
{"cmd":"start"}
{"cmd":"stop"}
{"cmd":"status"}
{"cmd":"set_rssi","min_rssi":-80}
{"cmd":"set_buzzer","enabled":false}
```

### BLE GATT Service

| Attribute | UUID | Properties |
|-----------|------|-----------|
| Service | `4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d` | -- |
| TX (results) | `4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Notify |
| RX (commands) | `4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Write |

## Architecture

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

## Filter Data

Compiled-in filter data merged from multiple open-source surveillance detection projects:

- **115 MAC OUI prefixes** — Flock Safety, Silicon Labs, Axis, Hanwha, FLIR, Mobotix, and other surveillance vendors
- **SSID patterns** — `Flock-XXXXXX`, `Penguin-XXXXXXXXXX`, `FS Ext Battery`
- **BLE name patterns** — Flock, Penguin, FS Ext Battery, Pigvision
- **Raven BLE service UUIDs** — 0x3100-0x3500 (custom), 0x180A/0x1809/0x1819 (standard)
- **Manufacturer IDs** — 0x09C8 (XUNTONG / Flock Safety)

Know of a device that should be detected? See the [signature contribution guide](CONTRIBUTING.md#adding-device-signatures).

## Roadmap

Directions, not promises:

- More ESP32 variants (C3, C6 RISC-V) and other MCU families (nRF52, RP2040)
- Linux daemon for Raspberry Pi / laptop wardriving with monitor-mode WiFi
- Over-the-air signature updates from companion app
- Formalized signature format spec for cross-tool interoperability
- Protocol versioning and expanded command set ([#9](https://github.com/dougborg/AirHound/issues/9))

## Contributing

Contributions are welcome — especially new device signatures. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on submitting signatures, adding board support, and development setup.

## License

MIT
