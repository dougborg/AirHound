# CatSniffer / Sniffle BLE source

AirHound accepts live Sniffle USB capture and saved classic PCAP files with
LINKTYPE_BLUETOOTH_LE_LL_WITH_PHDR (256). Both feed the same bounded Rust parser,
typed BLE fields, signature matcher, evidence policy and WiGLE exporter.
The single-source flags replace the native BLE backend for that session; WiFi and
gpsd can still run alongside it. Use a [multi-receiver session](multi-receiver-sessions.md)
to run Sniffle alongside native BLE or additional Sniffle receivers. It does not
turn CatSniffer into a CoreBluetooth adapter.

## Live capture

Prerequisites:

- CatSniffer v3 CC1352P7 with official **Sniffle 1.11.0 / API0**, specifically
  `sniffle_cc1352p7_1M.hex`, and a compatible USB bridge at **921600 baud**.
- User-installed [Sniffle host tools at v1.11.0](https://github.com/nccgroup/Sniffle/tree/v1.11.0).
  Keep the host tools and firmware versions matched. The helper checks the radio
  version before emitting a PCAP header. AirHound never installs firmware.
- A Python environment with `pyserial` and Sniffle's Python dependencies. The
  `--sniffle-module-path` directory contains the upstream `sniffle/` package.
- An explicit serial port, identified using unplug/replug or verified USB
  topology. Generic RaspberryPi Pico USB descriptors are not unique to CatSniffer.

From the repository root, after building with `just build-hostd`:

```sh
hostd/target/release/airhound-hostd \
  --sniffle-port /dev/cu.usbmodem112301 \
  --sniffle-helper scripts/sniffle-source.py \
  --sniffle-python /path/to/venv/bin/python \
  --sniffle-module-path /path/to/Sniffle/python_cli \
  --sniffle-channel 37 --no-wifi \
  --output capture.csv > capture.ndjson
```

On Linux select the actual `/dev/ttyACM*` device and arrange serial-device access.
The example port is local to the tested Mac, not an automatic detection rule.
The helper is a separate process; its path must remain available after moving or
packaging the host binary. Python defaults to `python3` if omitted.

Reception is **passive**, with connection following disabled and CRC validation
requested. No scan requests, connections or firmware changes are requested.
Primary reception is fixed on channel 37, 38 or 39; default 37. Extended secondary
following is enabled in Sniffle. `--sniffle-coded` selects Coded PHY for primary
reception instead of 1M: it does not listen to both simultaneously. Do not infer
all-channel coverage or better range from the option alone.

## Replay

```sh
hostd/target/release/airhound-hostd --ble-pcap capture.pcap \
  --no-wifi --output replay.csv > replay.ndjson
```

Replay uses captured timestamps, not replay wall time, and never attaches a
current GPS fix to historical rows. EOF closes the BLE source; with WiFi disabled
the session exits after draining output. With another source active the session
continues. Replay requires a regular file; FIFOs and device nodes are rejected. PCAPNG and
other link types are rejected explicitly. The parser supports
both byte orders and microsecond/nanosecond classic-PCAP timestamps, bounded to
4096 captured bytes per record. It rejects malformed framing before allocating
an unbounded record. Replay backpressures rather than discarding input records.

## Output and matching

Every accepted capture record emits `ble_packet_observation`, including packets
that do not identify an advertiser. See the [schema](../schemas/desktop-ble-packet.v1.schema.json).

- `source_id`, `generation`, `sequence` identify source records within a host
  session. Generation changes on live helper restart; these are not globally
  unique IDs across executions. `replay` distinguishes saved input.
- `captured_at_us` is capture epoch time. `channel` is BLE logical channel;
  `rf_channel` is the DLT256 frequency index (0 is 2402 MHz). PHY, RSSI availability,
  capture lengths and producer-reported CRC status remain explicit. CRC metadata
  is not independent packet authentication or a verified device identity.
- `pdu` retains the captured link-layer PDU without access address, coding
  indicator or CRC bytes. `access_address` is the link-layer access address,
  **not** an advertiser MAC.
- Advertiser-bearing packets expose their observed address and public/random
  type. Random addresses are not permanent identities. Scan requests and data
  packets remain packet records; they do not fabricate advertiser observations.
- Typed fields preserve full UUIDs, repeated manufacturer/service records and
  company ID zero. AD fields are bounded, and malformed or capacity-limited
  advertisements are not used for rule evidence.
- Only complete, CRC-reported-valid, addressed advertisements with known RSSI
  enter matching and CSV. Same-packet fields can produce composite evidence.
  Scan responses are independent observations; cached fields are never merged.
  Detection filters affect matching, not ordinary packet output or CSV export.
- Complete Remote ID service data uses the shared decoder, with raw-packet
  provenance. Protocol decoding does not authenticate a drone's identity.

Extended headers are parsed conservatively. Packets with an AuxPtr remain
`extended_chain_incomplete`; anonymous/unassociated fragments remain packet-only.
**Multi-packet extended-chain reassembly is not implemented.** These packets
remain available in NDJSON, but partial chains cannot trigger complete-payload
rules. Coded decoding has synthetic fixtures; live Coded/extended reception still
needs a suitable transmitter and the validation matrix in #172.

## Lifecycle and loss

`ble_source_status` reports packet count, host-queue drops, source generation,
reconnect attempts and explicit unknown RF loss. Live input drains the capture
pipe independently of matching; a full host queue drops packets and increments
`host_dropped_packets`. Status summaries are best effort during pressure, with a
final summary at orderly shutdown. Counts do not prove absence of radio, UART or
USB losses upstream. Helper decoder errors are reported to stderr; repeated errors
terminate the helper so the host can restart it.

The host retries a disconnected/exited helper after two seconds. Three consecutive
sessions without receiving a packet fail the selected source and the process;
there is no silent fallback to native BLE. Initial process-spawn failure is fatal.
A ten-second header deadline catches a helper that never becomes ready; a quiet
RF environment after readiness does not cause a reconnect.

Stop/start commands pause ingestion and restart the helper on resume. Queued live
records from an old control generation are discarded rather than replayed after
resume. Replay retains its reader position and queued records. Shutdown reaps the child and closes
the serial handle. The independently powered Sniffle radio can remain in passive
receive mode after the host closes it; host stop is not a radio-power-off command.

## Licensing and validation

The Rust parser, observation normalization and matcher are MIT code. The small
[`sniffle-source.py`](../scripts/sniffle-source.py) bridge is GPL-3.0-only and runs
as a separate process using user-installed GPL Sniffle tools; see its
[license text](../scripts/licenses/GPL-3.0.txt). Upstream firmware/tools are not
bundled in the AirHound executable. Distribution needs to retain the helper's
license and source obligations; this boundary does not relicense the Rust core.

Tests exercise PCAP framing/bounds, endian/time variants, malformed AD, public and
random addresses, repeated typed records, fragments, Coded metadata, conservative
matching, EOF/error propagation, partial reads across status ticks, pause/resume,
backpressure, child retry and helper passive-mode/version checks.

The connected macOS CatSniffer passed saved-capture replay (3931 packets / 2101
advertiser observations) and a 20-second live run (1799 / 1007), both with zero
reported host drops and clean exits. This is a short functional check, not a dense
RF soak, physical unplug test, Linux hardware validation or an extended/Coded test.
Raw ambient captures and the original radio firmware backup remain private.
