# Repository guide for coding assistants

Use the [README](README.md) for supported entry points, the
[documentation index](docs/README.md) for component guides,
[CONTRIBUTING.md](CONTRIBUTING.md) for development conventions, and the
[roadmap](docs/roadmap.md) for open work and hardware-release gates. Verify claims
against the current branch; a local build or short capture is not a release.

## Repository layout

- `src/lib.rs`: portable `no_std` detection library. Includes parsing, rules,
  signatures, bounded address/observation tracking, watchlist/proximity primitives,
  buttons and protocol serialization. It has library dependencies but no required
  ESP platform dependencies when built with `--no-default-features`.
- `src/main.rs`: Embassy/esp-hal firmware. Owns radio initialization, callbacks,
  static channels, tasks and output; `display.rs`, `buzzer.rs` and `memory.rs`
  provide board-specific functionality.
- `firmware-std/`: ESP-IDF runtime alternative with a separate lockfile.
- `hostd/`: macOS/Linux CLI with its own stable toolchain and lockfile. Native
  radio capture, WiGLE output, lifecycle handling and external receiver supervision.
- `drivers/mt7610u/`: separate GPL-2.0-only passive userspace receiver experiment.
  MIT hostd communicates with it through a process protocol, without linking it.
- `schemas/`: signature, firmware message, command and desktop BLE contracts.
- `scripts/` and `.github/`: validation, packaging, benchmarking and CI workflows.

The desktop CLI exists today; `apps/macos/` adds a native SwiftUI recording browser
and passive inventory UI. Its live capture control and Kismet integration remain
planned. Rust owns shared session, detection and control logic; frontends present
backend state.
The macOS `.app` supplies permission identity, not a graphical interface. Hostd
cannot yet ingest an embedded detector's serial/BLE stream. Its stdin is for
commands, not observations.

## Commands

Run from the repository root unless stated otherwise. Install `just` first.

```sh
# Portable logic; no ESP toolchain or device required
cargo +stable test --locked --lib --no-default-features
cargo +stable fmt --check
cargo +stable fmt --manifest-path firmware-std/Cargo.toml --check
python3 -B -m unittest discover -s scripts/tests -v
just check-json

# Desktop (hostd selects stable Rust)
just test-hostd
just clippy-hostd
just fmt-hostd
just build-hostd
just bench-hostd

# Firmware: Docker avoids a local ESP toolchain
just docker-build-xiao
just docker-build-m5stickc
just docker-build-std-xiao
just docker-build-std-m5stickc
```

Native firmware builds require the `esp` toolchain installed by `espup` and its
export environment. Use `just build-xiao` / `just build-m5stickc`. `build-std` is
passed by the recipes, not globally in `.cargo/config.toml`, so host tests work.
Select only one board feature: `xiao` (default) or `m5stickc`.

`just flash-xiao-host` / `just flash-m5stickc-host` flash an existing ELF and
monitor it. M5 no_std application output is 460800 baud; other builds use 115200.
Boot-ROM output stays 115200. See the [bring-up guide](docs/m5stickc-bringup.md)
for backup, image offsets and hardware validation before flashing.

Coverage uses the pinned toolchain and floors in `.github/`; follow [the testing guide](docs/testing.md)
rather than substituting toolchain versions or lowering thresholds. Synthetic
benchmarks exclude radio acquisition. Live macOS profiling requires permissions
and should not run concurrently with builds for performance comparisons.

## Runtime and memory constraints

Firmware tasks cooperate on an Embassy executor. WiFi callback delivery must
remain non-blocking. The no_std queues currently are:

| Queue | M5 | XIAO |
| --- | --- | --- |
| Scan events | 20 | 16 |
| Serialized output | 4 | 8 |
| Commands | 4 | 4 |
| BLE output | 4 | 4 |

The pipeline is radio → scan queue → parsing/matching → output queue → serial
and optional BLE forwarding. Both firmware runtimes attempt to stream ordinary
and matching observations; only signature matches trigger signature alerts.
`stream_stats` reports bounded-queue/serialization losses, not all RF loss or
proof of subscriber delivery. See [streaming](docs/firmware-streaming.md).

M5 has a 64 KiB internal heap and an isolated 2 MiB PSRAM allocator used for a
4,096-entry observed-address tracker. Radios, RTOS and queues still need internal
RAM. XIAO's internal heap is 128 KiB; the M5 PSRAM work does not enable XIAO PSRAM.
Do not infer free internal memory from available external RAM.

Embassy futures occupy static RAM. Keeping large buffers live across `.await`
can consume startup stack headroom even when compilation succeeds. The M5 linked
startup-stack gate is 7 KiB; run `scripts/check-firmware-memory.py` against its ELF
when changing static storage, queues, task futures or heap allocations. This is
a layout guard, not a measured runtime stack high-water mark. Build both boards
after changing shared event types.

M5 no_std excludes WiFi Remote ID events for static-memory reasons. XIAO no_std
and both ESP-IDF builds include them. The generic BLE event keeps only 62 bytes;
decoder tests do not establish extended/Coded PHY reception.

## Board and platform details

M5StickC Plus2: GPIO4 power hold must be high; GPIO27 backlight; GPIO12 display
reset; ST7789V2 display; GPIO2 passive buzzer; front button A GPIO37 and side
button B GPIO39. Buttons use external pull-ups. See [controls](docs/m5-controls.md).
Its CH9102 USB bridge carries UART; native USB cannot be enabled in software.

Linux WiFi requires an externally configured monitor interface. Hostd does not
change interface mode or hop its channels. macOS CoreBluetooth provides UUIDs;
standard builds attempt private address lookup with UUID fallback. Public-API
builds omit that lookup. Neither path guarantees stable physical-device identity.
The ALFA receiver is an experimental passive scanner, not an OS network driver.

## Change discipline

- Use Conventional Commits and focused PRs; run checks appropriate to the change.
- Keep protocol examples and schemas consistent with serialization and runtime
  command support. A parsed command is not necessarily implemented everywhere.
- Distinguish observed addresses, signature hits and confirmed identity. Broad
  OUI/name/service signatures can produce false positives.
- Keep firmware/driver build evidence separate from live radio, permissions,
  fault recovery and endurance evidence.
- Keep raw identifiers, captures and flash backups local and ignored. Publish
  aggregate validation results only.
- Read dependency versions and pins from manifests/lockfiles. ESP dependencies
  must be upgraded as a tested set; Docker images are pinned in `justfile`.
