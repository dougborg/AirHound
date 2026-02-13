# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AirHound is a `no_std` Rust firmware for ESP32 devices that acts as an RF wardriving companion. It scans WiFi (promiscuous mode) and BLE advertisements, filters results against compiled-in surveillance device signatures (MAC OUI prefixes, SSID patterns, BLE names/UUIDs/manufacturer IDs), and emits matched results as NDJSON over BLE GATT notifications and serial. AirHound is a **thin sensor/relay** — the companion app (DeFlock) handles analysis, scoring, GPS tagging, and storage.

## Build Commands

Requires the ESP Rust toolchain (`espup install`) and `espflash`.

```bash
# Build for XIAO ESP32-S3 (default feature)
cargo build --features xiao --release --target xtensa-esp32s3-none-elf

# Build for M5StickC Plus2
cargo build --features m5stickc --release --target xtensa-esp32-none-elf

# Flash and monitor (espflash runner configured in .cargo/config.toml)
cargo run --features xiao --release --target xtensa-esp32s3-none-elf
cargo run --features m5stickc --release --target xtensa-esp32-none-elf
```

There are no tests or linter configured. The project uses `build-std = ["core", "alloc"]` (set in `.cargo/config.toml`).

## Feature Flag Hierarchy

Board features (`xiao`, `m5stickc`) select chip features (`esp32s3`, `esp32`), which in turn enable the correct chip-specific crate features across all `esp-*` dependencies. `xiao` is the default. Only one board feature should be active at a time.

## Architecture

The firmware runs on the Embassy async executor (`esp-rtos`). Tasks communicate through static `embassy_sync::Channel`s defined in `main.rs`:

- **SCAN_CHANNEL** (capacity 16) — WiFi sniffer ISR and BLE scan task push raw `ScanEvent`s here
- **OUTPUT_CHANNEL** (capacity 8) — Serialized NDJSON `MsgBuffer`s ready for transmission
- **CMD_CHANNEL** (capacity 4) — Parsed `HostCommand`s from BLE or serial input

Task pipeline: `WiFi Sniffer / BLE Scanner → filter_task → output_serial_task / BLE GATT TX`

### Module Responsibilities

- **`main.rs`** — Entry point, task spawning, filter task that bridges scan events to output. Owns the static channels and `FilterConfig`.
- **`scanner.rs`** — WiFi/BLE event types, 802.11 frame parsing (`parse_frame_type`, `extract_ssid`, `extract_src_mac`), BLE AD structure parsing (`BleAdvParser`), channel hop loop. Radio initialization is still TODO.
- **`filter.rs`** — Stateless filter engine. `filter_wifi()` and `filter_ble()` evaluate scan inputs against compiled-in defaults from `defaults.rs` plus runtime `FilterConfig` (RSSI threshold, enable/disable toggles). Returns up to 4 `MatchReason`s per result.
- **`defaults.rs`** — All compiled-in filter data: 108 MAC OUI prefixes, SSID patterns/exact/keywords, BLE name patterns, Raven service UUIDs, manufacturer IDs. The `SsidPattern` struct handles structured matching (prefix + typed suffix).
- **`protocol.rs`** — Serde-based NDJSON message types using `heapless` strings. `DeviceMessage` (tagged enum: wifi/ble/status) and `HostCommand` (tagged enum: start/stop/status/set_rssi). All string types are fixed-capacity `heapless::String`.
- **`comm.rs`** — BLE GATT UUIDs, serial config, JSON serialization/deserialization helpers, `LineReader` byte-by-byte NDJSON accumulator, command handler. BLE GATT server is placeholder.
- **`board.rs`** — Compile-time hardware constants (pin assignments, capabilities) selected by board feature flags. Each board defines a `mod hw` with constants like `BOARD_NAME`, `LED_PIN`, `HAS_DISPLAY`.

## Key Constraints

- **`no_std` / `no_alloc` for most things**: Uses `heapless` collections with fixed capacities. `alloc` is only for the BLE stack (72KB heap via `esp-alloc`).
- **All string types have fixed max lengths**: `MacString` (18), `NameString` (33), `MatchDetail` (32), `MsgBuffer` (512 bytes). Be mindful of truncation.
- **Single-threaded cooperative**: Embassy executor is single-threaded with no preemption. Shared state (like `FilterConfig`) relies on this rather than locks.
- **ISR context for WiFi sniffer callback**: The sniffer callback runs in interrupt context — must use `try_send` (non-blocking) on the channel, not `.await`.
