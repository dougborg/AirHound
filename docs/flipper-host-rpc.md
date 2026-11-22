# Flipper host RPC integration

The live bridge is being developed as an explicit receiver source. Its host
transport, lifecycle and hardware acceptance are not yet complete. The existing
Marauder file importer remains available independently.

AirHound owns the serial endpoint, bounded message framing, request correlation,
bridge lifecycle, packet parsing, detection and storage. The Flipper application
owns its GPIO UART, passive command allowlist and local watchdog. The shared
[AHFB contract](flipper-bridge-protocol.md) distinguishes command acceptance,
observed UART responses and actual packet reception.

## Generated protobuf messages

The host pins `flipper-rpc = 0.9.5` with default features disabled and only its
`proto` feature. This reuses generated Prost message types without importing the
crate's automatic serial connection or management operations. The crate contains
an MIT LICENSE (copyright 2025 flipper-rpc contributors); desktop packaging's
existing Cargo dependency license collection includes that file.

Published crate SHA-256:
`9fd9a12d00f8216c09473f3bc7d1deffcf9e9f9982c18065e7367549bb7c2b52`.
Its Cargo VCS revision is `fbbee8c82a63b377727c5692c4c694ce4eac7ba6`.
The crate attributes the official Flipper schema but does not pin an exact schema
revision. AirHound therefore does not claim every generated message matches the
attached firmware.

The messages AirHound uses are tested against synthetic goldens generated from
[Flipper's official protobuf repository](https://github.com/flipperdevices/flipperzero-protobuf)
at `ea4f185f5eaa265955c520eae2832887ee6aa5e4`, the schema used by our matched
Momentum RPC tools. Tests cover Start/Exit, AppDataExchange, AppState, Empty ACK,
StopSession and Ping/Pong in both encoding directions. These establish wire
compatibility for those messages, not firmware behavior or radio support.

Regenerate fixtures with `protoc`, Python's protobuf runtime and a clean checkout
of that exact schema:

```sh
python scripts/generate-flipper-rpc-goldens.py \
  --schema /path/to/flipperzero-protobuf \
  --output hostd/tests/fixtures/flipper-rpc
```

The generator opens no device. Its fixtures contain synthetic data only.

## Prior art considered

- [flipper-rpc](https://github.com/elijah629/flipper-rpc) provides reusable generated
  messages and a broader synchronous management client. Only its message types
  fit this source's bounded, cancellable ownership requirements.
- [flipper-companion-protocol](https://docs.rs/crate/flipper-companion-protocol/0.8.1)
  supplies a larger actor/client stack under AGPL; it is not used here.
- [flipperzero-rs](https://github.com/flipperzero-rs/flipperzero-rs) supplies
  device-side USB/BLE and Flipper application APIs. Those are useful to the FAP,
  but are not a host-side serial RPC receiver.

## Trust and clocks

The bridge's app version/source revision are self-reported. Its compiled C ABI
is distinct from firmware identity. A capture generation is local to one FAP
launch and is not an AWOK boot identifier. PCAP `micros()` values are retained;
a decrease is marked as an ambiguous wrap or reset, never converted into a
fabricated wall timestamp. App UART receipt uptime and host receipt time are
separate. RF loss remains unknown even when transport counters report zero.

Sequence gaps, malformed PCAP or reported producer drops invalidate the stream.
No decoder searches arbitrary packet bytes for a replacement global header.
A new explicit capture is required after a failed session.
