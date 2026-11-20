# Driver structure and validation

The standalone GPL-2.0-only receiver communicates with the MIT AirHound host
through NDJSON. It does not link the host or share GPL implementation code with
it. Firmware is supplied separately and is not a test fixture or bundled asset.

## Module responsibilities

| Module | Responsibility |
| --- | --- |
| `main.rs` | CLI dispatch, argument checks, process cancellation, output limits |
| `capture.rs` | Finite capture orchestration and aggregate reporting |
| `receiver.rs` | Shared initialization, bounded old-channel draining, verified shutdown |
| `stream.rs` | Lifecycle state machine, recovery budgets, command scheduling |
| `stream/protocol.rs` | Bounded control-input framing and request IDs |
| `stream/output.rs` | Bounded output queue and independently owned blocking writer |
| `stream/hardware.rs` | USB receiver adapter, channel dwell, observations and counters |
| `transport.rs` | libusb adapter plus register/MCU framing, retries and validation |
| `firmware.rs` | Image validation, volatile upload, ready-MCU reuse |
| `radio.rs`, `channel_tables.rs`, `radio_tables.rs` | Radio programming and pinned Linux settings |
| `rx.rs`, `signal.rs` | Frame decoding and explicitly unvalidated signal estimates |
| `recording.rs`, `rotation.rs`, `replay.rs` | Private files, bounded rotation and offline replay |

Finite capture and streaming use the same preparation and shutdown routines.
The `UsbIo` boundary abstracts endpoint I/O, so tests execute the production
packet framing, upload, register programming and receive lifecycle. The native
libusb implementation remains in the measured code. `ReceiverHardware` provides
a separate seam for command ordering, backpressure and reconnection tests.

`test_support.rs` is compiled only for tests. Its synthetic endpoint model
checks that MAC TX, automatic ACK, beacon generation and EEPROM programming are
not enabled. The upload tests use synthetic images and explicitly supply the
expected digest in the test object; separate tests verify that real CLI inputs
with an untrusted digest fail before USB access. No firmware bytes are bundled.

## Automated checks

From the repository root:

```sh
cargo fmt --manifest-path drivers/mt7610u/Cargo.toml --check
cargo test --locked --manifest-path drivers/mt7610u/Cargo.toml
cargo clippy --locked --manifest-path drivers/mt7610u/Cargo.toml --all-targets -- -D warnings
RUSTUP_TOOLCHAIN=nightly-2026-11-18 bash .github/scripts/coverage.sh driver
```

Coverage requires `cargo-llvm-cov` 0.9.1 and the toolchain's `llvm-tools-preview`
component. CI runs driver coverage on both Linux and macOS with an **81% line
floor** configured in `.github/coverage-thresholds.json`. It uploads JSON and
HTML reports. Only test code is excluded, including integration test helpers;
production firmware, USB and hardware-facing code remain in the denominator.
This measures executable code, not static table entries. The independent
[Linux table checker and published channel checks](../../docs/mt7610u-channels.md)
cover that separate concern.

The local macOS measurement on 2026-11-19 was **85.52% lines**, **85.43%
functions**, and **79.84% regions** with 75 passing tests. Regions are not a
branch-coverage measurement. Notable line coverage: firmware 92.37%, radio
93.80%, frame parsing 98.20%, streaming hardware adapter 95.96%, lifecycle
83.02%, and finite capture 84.62%. Native transport/discovery remains 64.66% and
CLI dispatch 68.84%; these gaps are retained rather than excluded. The Linux
job is configured but was not run locally for this change.

Tests include short transfers, timeout retries, disconnect propagation,
stale/error MCU replies, sequence wrap, firmware chunk offsets, invalid image
rejection, cancellation, failed/quiet captures, broken output, pause/resume
ordering, command EOF, input failures, bounded queues and recovery exhaustion.

## Hardware evidence and remaining gates

The refactored driver completed a local 20-second, nine-channel capture with
946 decoded beacons, 36 retunes and verified disabled shutdown gates. The host
validator accepted every observation. There were 65 malformed/unsupported USB
transfers, rejected and counted by the decoder. Raw captures remain in ignored
local validation directories.

A separate 30-second stream check passed with pause/resume, six acknowledged
commands, 1,420 beacons, zero dropped observations, no file-descriptor growth,
and verified shutdown. Its 76 malformed/unsupported transfers were counted;
no physical disconnect occurred, so this is not reconnect evidence.

Simulation and a short hardware check do not establish eight-hour stability,
physical reconnect/sleep-wake behavior, receive sensitivity, calibrated RSSI,
or reception on every supported channel. Follow the
[hardware release gates](../../docs/receiver-release-gates.md) before treating
this as a production driver.
