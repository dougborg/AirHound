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

## Explicit live source (prototype)

The runtime source is `flipper_marauder` with one required `port`. It targets
firmware ABI **87.1** and the separately installed AirHound bridge at
`/ext/apps/GPIO/airhound_bridge.fap`. It does not install applications, update
firmware, select another application, reset a board, or reconnect automatically.
Hardware evidence and its exact build scope are recorded in
[attached hardware validation](attached-hardware-validation.md). Later protocol
changes require acceptance of the matching host and FAP artifacts.

```json
{
  "version": 1,
  "alert_policy": "silent",
  "receivers": [
    {
      "id": "flipper-bridge",
      "source": {
        "kind": "flipper_marauder",
        "port": "/dev/cu.REPLACE_WITH_SELECTED_FLIPPER"
      }
    }
  ]
}
```

The shared backend validates and saves this configuration like other receiver
setups. Duplicate serial endpoints, including macOS callout/dial-in aliases,
conflict with M5, Sniffle and local GPS sources. Validation and passive discovery
do not open the port or establish that the FAP is installed. The STM32 USB ID is
only a candidate hint; a current service-owned Hello identifies the AirHound
bridge, not the attached AWOK hardware or radio reception.

Starting a recording acquires the shared serial lease before opening CDC. It
uses the Flipper DTR handshake, reads bounded `device_info` and `loader info`,
requires the supported ABI and an idle loader, then opens the official RPC
session. Only the fixed AirHound bridge is launched, initially without UART/RF.
Explicit Start negotiates the bridge and requests passive beacon capture.
Channels remain device-managed and unknown; this source does not claim channel
assignment, receive RSSI, AWOK firmware identity or complete RF loss accounting.

Pause asks the bridge to stop and reports the actual observed UART acknowledgment
and drain state. Resume requests a fresh preflight and capture generation.
Finish requests a terminal stop before closing this RPC session. Acceptance of
an RPC or bridge command is not evidence that the radio stopped. A timeout,
disconnect, malformed stream or producer loss fails the source and leaves stop
unverified unless separately observed. The bridge's independent local watchdog
remains responsible for stopping after host/transport loss; it cannot guarantee
behavior across device power loss or a hung hardware layer.

The Rust owner thread retains the serial lease through transport cleanup even
when its async caller is cancelled. Observation output never blocks that thread:
queue overflow increments a counter retained in receiver health independently
of status-message delivery. Current firmware/app identity is cleared on close;
last reported diagnostic status and host drops remain marked as historical.

`bridge_identity` distinguishes actual Flipper CLI firmware fields, the FAP's
reported application version/revision/compiled ABI, and its hexadecimal launch
token. `bridge_status` retains lifecycle evidence and counters. Live packets use
the ordinary shared WiFi observation/detection/storage path, with original
Marauder clock values, explicit discontinuities, host receipt time and capture
generation. Fresh receiver-location associations follow the shared live-source
contract; saved Marauder replay never receives present-day GPS.

After a session-bound autonomous failure report, the owner may continue reading
that same RPC stream for at most 3.25 seconds from its first observed failure.
The 250 ms margin accommodates host/wire scheduling beyond the unchanged device
three-second stop cap. No heartbeat, capture control, recovery or ordinary packet
output occurs during this diagnostic drain. Identity, sequence, request, cause,
generation and nondecreasing counters remain checked; malformed or disconnected
transport ends the drain. Coalesced RPC frames survive the initial failure
exactly once in a bounded queue.

A terminal failure report can arrive before the transport and bridge acknowledgments
of a heartbeat submitted earlier. Both already-correlated replies are drained within
the same fixed deadline before SessionClose; no new heartbeat is sent. Missing or
mismatched replies invalidate completion. Secondary drain and SessionClose errors
are logged separately while the original receiver failure remains the result.

Later bound failure reports retain their device clock, loss and unread-byte
counters as `accepted_lifecycle: false`. They cannot make a failed run successful
or restore receiver availability. Terminal Failed state and a stop-text bit do
not alone prove complete quiet drain or receipt of every outstanding Stop reply.
Absent or incomplete terminal evidence remains unverified; the original source
error is returned even when later diagnostic evidence is received.

The current host requires Hello capability mask15, including explicit bounded
stop-completion evidence. Older capability7 peers are rejected before Start.
Evidence bit3 (value8) is separate from the stop-text acknowledgment (value4):
normal Paused/Stopped after UART ownership requires both, a fresh request-scoped
boundary, complete parser framing and zero reported loss/unread bytes. Gen0
no-UART Stop remains evidence0 and makes no radio-stop claim.

For autonomous failures, `stop_complete` is derived from the validated same-session
request, generation, cause, sequence, fresh cleared-stop boundary and terminal
completion evidence. It is not copied from an arbitrary rejected Status. Raw
reported bits/counters remain available with `accepted_lifecycle: false`, even
when this derived proof is false. Final loss or unread bytes permanently revoke completion for that failure drain,
even if a later raw report reasserts its completion bit; coalesced or
closing reports update the retained evidence, and a poisoned/failed continuation
clears derived proof while preserving the original source error. Raw bytes after
a terminal report also poison completion instead of silently being ignored. A valid failure
stop never restores receiver health or changes the recording to successful.
