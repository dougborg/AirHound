# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AirHound is a `no_std` Rust firmware for ESP32 devices that acts as an RF wardriving companion. It scans WiFi (promiscuous mode) and BLE advertisements, filters results against compiled-in surveillance device signatures (MAC OUI prefixes, SSID patterns, BLE names/UUIDs/manufacturer IDs), and emits matched results as NDJSON over BLE GATT notifications and serial. AirHound is a **thin sensor/relay** — the companion app (DeFlock) handles analysis, scoring, GPS tagging, and storage.

## Build Commands

The project builds inside Docker (no local ESP toolchain needed). Install `just` (`cargo install just`).

```bash
just docker-build            # Both targets
just docker-build-xiao       # XIAO ESP32-S3 only
just docker-build-m5stickc   # M5StickC Plus2 only
just docker-check            # Type-check both targets
just docker-clean            # Clean artifacts (REQUIRED after dependency changes)
```

To flash (requires `espflash` on host, device connected via USB):
```bash
espflash flash --chip esp32s3 target/xtensa-esp32s3-none-elf/release/airhound
espflash flash --chip esp32 target/xtensa-esp32-none-elf/release/airhound
```

Tests and formatting:
```bash
just docker-test             # Run unit tests (in container)
just test                    # Run unit tests (requires nightly on host)
cargo fmt --check            # Check formatting (requires nightly on host)
just setup-hooks             # Configure git pre-commit + commit-msg hooks
```

The project uses `build-std = ["core", "alloc"]` (passed via justfile, not `.cargo/config.toml`). Commits must follow [Conventional Commits](https://www.conventionalcommits.org/) format.

## Feature Flags

Board features (`xiao`, `m5stickc`) select chip features (`esp32s3`, `esp32`), which enable chip-specific crate features across all `esp-*` dependencies. **Only one board feature should be active at a time.** `xiao` is the default.

The `m5stickc` feature additionally enables display (`mipidsi`, `embedded-graphics`, `embedded-hal-bus`) and buzzer modules.

## Architecture

The firmware runs on the Embassy async executor (`esp-rtos`). All tasks are single-threaded cooperative (no preemption). Tasks communicate through static `embassy_sync::Channel`s defined in `main.rs`:

- **SCAN_CHANNEL** (capacity 16) — WiFi sniffer ISR and BLE scan task push raw `ScanEvent`s
- **OUTPUT_CHANNEL** (capacity 8) — Serialized NDJSON `MsgBuffer`s ready for transmission
- **CMD_CHANNEL** (capacity 4) — Parsed `HostCommand`s from BLE or serial input
- **BLE_OUTPUT_CHANNEL** (capacity 4) — Cloned output messages forwarded as BLE GATT notifications
- **BUZZER_SIGNAL** (capacity 1, m5stickc only) — Coalescing trigger for buzzer beeps

Pipeline: `WiFi Sniffer / BLE Scanner → SCAN_CHANNEL → filter_task → OUTPUT_CHANNEL → output_serial_task / BLE GATT TX`

Shared state uses atomics (`SCANNING`, `BLE_CLIENTS`, `WIFI_MATCH_COUNT`, `BLE_MATCH_COUNT`, `BUZZER_ENABLED`) and `critical_section::Mutex<Cell<T>>` for larger types (`FILTER_CONFIG`, `LAST_MATCH`).

### Crate Structure

The project is split into a library crate (`src/lib.rs`) and a binary crate (`src/main.rs`). The library contains all pure-logic modules testable on host (`cargo test --lib --no-default-features`). The binary contains ESP-specific code (ISR callbacks, embassy tasks, hardware init). Library uses `#![cfg_attr(not(test), no_std)]` — `no_std` for firmware, `std` for tests.

### Module Responsibilities

**Library modules** (`src/lib.rs` re-exports):
- **`scanner.rs`** — WiFi/BLE event types, 802.11 frame parsing (`parse_wifi_frame()`), BLE advertisement parsing (`BleAdvParser`). Pure functions — ISR callbacks and channel hop task live in `main.rs`.
- **`filter.rs`** — Stateless filter engine. `filter_wifi()` and `filter_ble()` evaluate inputs against compiled-in defaults plus runtime `FilterConfig`. Returns up to 4 `MatchReason`s per result.
- **`defaults.rs`** — All compiled-in filter data: MAC OUI prefixes, SSID patterns, BLE names, service UUIDs, manufacturer IDs.
- **`protocol.rs`** — Serde-based NDJSON message types using `heapless` strings. `DeviceMessage` (wifi/ble/status) and `HostCommand` (start/stop/status/set_rssi/set_buzzer).
- **`comm.rs`** — JSON serialization/deserialization, `LineReader` NDJSON accumulator, command handler. BLE GATT service definition and channel type aliases live in `main.rs`.
- **`board.rs`** — Compile-time hardware constants per board (pin assignments, capabilities).

**Binary modules** (`src/main.rs`):
- Entry point, heap setup, peripheral init, task spawning, WiFi sniffer callback, channel hop task, BLE scan task, BLE GATT server, serial output task. Owns all static channels, shared state, and ESP-specific types.
- **`display.rs`** (m5stickc only) — ST7789V2 display driver. `Screen` renderer with `row!` and `centered!` macros.
- **`buzzer.rs`** (m5stickc only) — LEDC-driven passive buzzer.

## Key Constraints

- **`no_std` / `no_alloc` for application code**: Uses `heapless` collections with fixed capacities. `alloc` is only for the WiFi/BLE radio stacks.
- **Heap budget is tight**: ESP32 (M5StickC) uses 64KB heap — reduced from 72KB to leave DRAM for stack. ESP32-S3 (XIAO) uses 128KB. Cannot go below ~60KB on ESP32 or WiFi/BLE coex allocation fails.
- **Stack overflow risk on ESP32**: Embassy task futures are stored in static BSS. Large generic types (e.g., mipidsi Display with nested SPI generics) consume significant DRAM. Use `StaticCell` for large buffers instead of task-stack allocation.
- **All string types have fixed max lengths**: `MacString` (18), `NameString` (33), `MatchDetail` (32), `MsgBuffer` (512 bytes). Be mindful of truncation.
- **ISR context for WiFi sniffer callback**: The sniffer callback runs in interrupt context — must use `try_send` (non-blocking) on the channel, not `.await`.
- **BLE must init before WiFi** for coexistence to work (assertion failure otherwise on ESP32-S3).

## M5StickC Plus2 Hardware

- **GPIO4 = POWER_HOLD** — must drive HIGH to keep device powered (especially on battery)
- **GPIO27 = backlight** — active HIGH
- **GPIO12 = display RST** — manual hardware reset required before mipidsi init
- Display: ST7789V2, 135×240, SPI2 at 40MHz, offset(52,40), color inversion ON, BGR
- Buzzer: passive on GPIO2, driven by LEDC PWM

## Dependencies

All `esp-*` crates are from **git main branch** (`https://github.com/esp-rs/esp-hal.git`). Docker named volumes cache the cargo registry/git — run `just docker-clean` after switching dependency sources. `trouble-host 0.6.0` is the BLE host stack.
