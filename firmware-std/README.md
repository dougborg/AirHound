# ESP-IDF firmware

[Project home](../README.md) · [Documentation index](../docs/README.md) · [Board setup](../docs/firmware.md)

This is AirHound's alternative ESP-IDF runtime. It shares the portable library
with the Embassy/esp-hal firmware but has its own Cargo manifest, lockfile,
toolchain configuration and SDK settings. It uses `esp-idf-svc` and ESP32 NimBLE.

## Build

From the repository root, install Docker and `just`, then select a board:

```sh
just docker-build-std-m5stickc
just docker-build-std-xiao
# Both:
just docker-build-std
```

The recipes choose the target, `MCU`, board feature and `build-std` options.
Artifacts are under `firmware-std/target/<target>/release/airhound-std`, where the
target is `xtensa-esp32-espidf` for M5 or `xtensa-esp32s3-espidf` for XIAO.
CI builds both variants. ESP-IDF images are CI previews, not the merged no_std
release binaries described in the [firmware guide](../docs/firmware.md).

## Runtime differences

- The serial application console is 115200 baud on both boards.
- Both variants include WiFi Remote ID events. The M5 no_std runtime excludes
  them to preserve static memory headroom.
- The event queue is heap-backed and threads have separate stacks. ESP-IDF does
  not remove the ESP32's memory limits or inherit the no_std PSRAM allocation policy.
- Do not assume no_std M5 display/button behavior is identical here. Consult the
  [command matrix](../docs/protocol.md#command-results-and-runtime-support) for supported commands.
- Both runtimes attempt to stream ordinary and matching observations; delivery
  is bounded and best effort. See [firmware streaming](../docs/firmware-streaming.md).

## Source and validation

[`src/main.rs`](src/main.rs) owns initialization, scanning, event processing and
output. [`src/display.rs`](src/display.rs) and [`src/buzzer.rs`](src/buzzer.rs)
implement M5 peripherals. Board-specific SDK settings are in
[`sdkconfig.defaults.esp32`](sdkconfig.defaults.esp32) and
[`sdkconfig.defaults.esp32s3`](sdkconfig.defaults.esp32s3).

```sh
# From the repository root:
cargo +stable fmt --manifest-path firmware-std/Cargo.toml --check
```

Shared logic is tested through the [core suite](../src/README.md#work-on-the-library).
Build success is not a substitute for flashing, RF, coexistence, memory, command
and output testing on each board. See the [roadmap](../docs/roadmap.md) for open
hardware-validation gates.
