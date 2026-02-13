# AirHound

RF wardriving companion device built in Rust for ESP32. Scans WiFi and BLE, filters against known surveillance device signatures, and relays matched results to a companion app over BLE GATT or serial.

## Design Philosophy

AirHound is a **thin sensor/relay**. It scans, filters, and emits. The companion app (DeFlock or similar) handles analysis, scoring, alerting, GPS tagging, and storage.

## Supported Hardware

| Board | Chip | Target | Feature Flag | Extras |
|-------|------|--------|-------------|--------|
| Seeed XIAO ESP32-S3 | ESP32-S3 | `xtensa-esp32s3-none-elf` | `xiao` | PSRAM, GPS header |
| M5StickC Plus2 | ESP32 (PICO-V3) | `xtensa-esp32-none-elf` | `m5stickc` | TFT display, buzzer |

### M5StickC Plus2 Features

- **135x240 TFT display** (ST7789V2) — status screen with match counts, uptime, and last detection
- **Passive buzzer** (GPIO2) — short alert beep on surveillance device match, togglable via BLE command

## Tech Stack

- **Language:** Rust (`no_std`)
- **HAL:** esp-hal (Espressif official, git main)
- **Radio:** esp-radio (WiFi sniffer + BLE + coex)
- **BLE Host:** TrouBLE (trouble-host, Embassy ecosystem)
- **Async Runtime:** Embassy (via esp-rtos)
- **Display:** mipidsi + embedded-graphics (M5StickC only)
- **JSON:** serde + serde-json-core (no-alloc)

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

- **108 MAC OUI prefixes** -- Flock Safety, Silicon Labs, Axis, Hanwha, FLIR, Mobotix, and other surveillance vendors
- **SSID patterns** -- `Flock-XXXXXX`, `Penguin-XXXXXXXXXX`, `FS Ext Battery`
- **BLE name patterns** -- Flock, Penguin, FS Ext Battery, Pigvision
- **Raven BLE service UUIDs** -- 0x3100-0x3500 (custom), 0x180A/0x1809/0x1819 (standard)
- **Manufacturer IDs** -- 0x09C8 (XUNTONG / Flock Safety)

## License

MIT
