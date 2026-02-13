# AirHound

RF wardriving companion device built in Rust for ESP32. Scans WiFi and BLE, filters against known surveillance device signatures, and relays matched results to a companion app over BLE GATT or serial.

## Design Philosophy

AirHound is a **thin sensor/relay**. It scans, filters, and emits. The companion app (DeFlock or similar) handles analysis, scoring, alerting, GPS tagging, and storage.

## Supported Hardware

| Board | Chip | Target | Feature Flag |
|-------|------|--------|-------------|
| Seeed XIAO ESP32-S3 | ESP32-S3 | `xtensa-esp32s3-none-elf` | `xiao` |
| M5StickC Plus2 | ESP32 (PICO-V3) | `xtensa-esp32-none-elf` | `m5stickc` |

## Tech Stack

- **Language:** Rust (`no_std`)
- **HAL:** esp-hal 1.0 (Espressif official)
- **Radio:** esp-radio (WiFi sniffer + BLE + coex)
- **BLE Host:** TrouBLE (trouble-host, Embassy ecosystem)
- **Async Runtime:** Embassy (via esp-rtos)
- **JSON:** serde + serde-json-core (no-alloc)

## Building

### Prerequisites

```bash
# Install Rust ESP32 toolchain
cargo install espup --locked
espup install

# Install flash tool
cargo install espflash --locked

# Source the ESP environment (add to shell profile)
. ~/export-esp.sh
```

### Build Commands

```bash
# XIAO ESP32-S3 (default)
cargo build --features xiao --release --target xtensa-esp32s3-none-elf

# M5StickC Plus2
cargo build --features m5stickc --release --target xtensa-esp32-none-elf
```

### Flash and Monitor

```bash
# XIAO ESP32-S3
cargo run --features xiao --release --target xtensa-esp32s3-none-elf

# M5StickC Plus2
cargo run --features m5stickc --release --target xtensa-esp32-none-elf
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
```

### BLE GATT Service

| Attribute | UUID | Properties |
|-----------|------|-----------|
| Service | `4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d` | — |
| TX (results) | `4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Notify |
| RX (commands) | `4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Write |

## Architecture

```
┌─────────────────┐    ┌──────────────────┐
│  WiFi Sniffer   │    │   BLE Scanner    │
│  (22-ch hop)    │    │  (periodic scan) │
└────────┬────────┘    └────────┬─────────┘
         │ ScanEvent            │ ScanEvent
         └──────────┬───────────┘
                    ▼
          ┌─────────────────┐
          │  Filter Engine   │
          │ MAC/SSID/BLE/UUID│
          └────────┬────────┘
                   │ NDJSON
         ┌─────────┴─────────┐
         ▼                   ▼
┌─────────────────┐  ┌──────────────┐
│  BLE GATT TX    │  │  Serial TX   │
│  (notify)       │  │  (115200)    │
└─────────────────┘  └──────────────┘
```

## Filter Data

Compiled-in filter data merged from multiple open-source surveillance detection projects:

- **108 MAC OUI prefixes** — Flock Safety, Silicon Labs, Axis, Hanwha, FLIR, Mobotix, and other surveillance vendors
- **SSID patterns** — `Flock-XXXXXX`, `Penguin-XXXXXXXXXX`, `FS Ext Battery`
- **BLE name patterns** — Flock, Penguin, FS Ext Battery, Pigvision
- **Raven BLE service UUIDs** — 0x3100-0x3500 (custom), 0x180A/0x1809/0x1819 (standard)
- **Manufacturer IDs** — 0x09C8 (XUNTONG / Flock Safety)

## License

MIT
