# ESP32 firmware

[Project home](../README.md) · [Documentation index](README.md) · [Source and shared core](../src/README.md) · [ESP-IDF alternative](../firmware-std/README.md)

Commands below run from the repository root. The primary runtime is Embassy/esp-hal in `src/main.rs`; the ESP-IDF runtime has a separate package and build instructions.

### Supported Hardware

| Board | Chip | Target | Feature Flag | Hardware extras |
|-------|------|--------|-------------|--------|
| Seeed XIAO ESP32-S3 | ESP32-S3 | `xtensa-esp32s3-none-elf` | `xiao` | PSRAM hardware, GPS header; XIAO PSRAM not enabled by this firmware |
| M5StickC Plus2 | ESP32 (PICO-V3-02) | `xtensa-esp32-none-elf` | `m5stickc` | TFT display, buzzer, 2 MB PSRAM |

#### M5StickC Plus2 Features

The no_std build provides:

- **TFT display** (240×135 landscape) — Overview separates AP, other WiFi and BLE address counts from signature hits; Details shows memory and WiFi queue statistics.
- **Front button A** — switches Overview / Details.
- **Side button B** — toggles detection sound; companion buzzer commands update the same state.
- **Passive buzzer** (GPIO2) — short alert for explicit watchlists and alert-enabled rules. Quiet signature clues still update counts and the display. Settings reset at reboot.

See the [controls guide](../docs/m5-controls.md) for debounce and validation details.
These controls describe the no_std runtime; do not assume ESP-IDF UI parity.
USB uses a CH9102 USB-to-UART bridge. The application console is **460800 baud**
for no_std M5 builds, **115200** for other builds. Boot-ROM output remains
115200; a different USB protocol cannot bypass this board's physical UART.

See [M5StickC Plus2 bring-up and backup procedure](../docs/m5stickc-bringup.md)
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

AirHound starts scanning automatically after its consumers and initial display are ready. The firmware attempts to emit every parsed WiFi/BLE observation from enabled radios as NDJSON on BLE GATT notifications and serial; signature matches are metadata, not a condition for streaming. See [firmware streaming](../docs/firmware-streaming.md) for loss counters and transport limits.

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
# Flash locally built ELF files; select your board explicitly when several USB devices are connected
espflash flash --chip esp32s3 target/xtensa-esp32s3-none-elf/release/airhound
espflash flash --chip esp32 target/xtensa-esp32-none-elf/release/airhound

# Or build + flash + monitor in one step (native only)
just flash-xiao
just flash-m5stickc
```
