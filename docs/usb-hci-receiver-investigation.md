# Direct USB Bluetooth receiver investigation

Status: offline feasibility prototype, 2026-11-20. No USB interface was opened,
claimed or detached; no HCI command, reset, scan, NVRAM change, VM launch or USB
passthrough was performed. This work does not add a production driver or hostd
receiver backend.

## Recommendation

Use an isolated receiver process, like the external ALFA receiver, but define an
external **BLE** contract rather than reuse the WiFi event format. First evaluate
Bumble as the transport/controller owner on Linux. Keep pure HCI-to-observation
logic replay-testable and independent of its transport. Prefer the ordinary
Linux BlueZ path for an initial baseline before taking exclusive HCI ownership.

The connected device reports CSR8510 A10 (`0a12:0001`, revision `8891`). Those
identifiers do not authenticate the chip; CSR-compatible clones exist. Qualcomm's
[product brief](https://docs.qualcomm.com/doc/80-CT903-1/80-CT903-1_REV_AC_CSR8510_A10_Product_Brief.pdf)
specifies Bluetooth 4.0 for the genuine device. It is a useful potential legacy
BLE test receiver, not an extended-advertisement/Coded-PHY upgrade. Software cannot
add those radio features. Linux's [btusb implementation](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btusb.c)
contains CSR handling and clone workarounds; this is not validation of this unit.

| Route | Useful properties | Cost / limits | Source and license evidence |
| --- | --- | --- | --- |
| Linux BlueZ / existing hostd backend | Existing OS driver, explicit adapter selection, ordinary BLE baseline | OS property aggregation and API exposure remain; not direct report capture | [Linux btusb](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btusb.c); existing AirHound backend |
| Bumble sidecar, USB HCI or Linux HCI transport | Existing command/event machinery, controller support, Python prototyping; USB can identify a particular port/device | Needs exclusive ownership where applicable, transport recovery and a bounded subprocess protocol; not yet integrated | [USB transport](https://google.github.io/bumble/transports/usb.html), [HCI socket transport](https://google.github.io/bumble/transports/hci_socket.html), [Apache-2.0 license](https://github.com/google/bumble/blob/main/LICENSE) |
| BTstack sidecar | Mature C host stack, CSR support, published Linux/UTM CSR experience | Checked repository license restricts commercial use; evaluate commercial terms before adopting or redistributing | [libusb port](https://github.com/bluekitchen/btstack/blob/master/port/libusb/README.md), [license](https://github.com/bluekitchen/btstack/blob/master/LICENSE) |
| Minimal Rust USB/HCI receiver | Could share Rust types and limit scope to passive scanning | Still needs USB ownership, HCI credits/completion/timeouts, event masks, scan setup, cancellation/recovery, controller quirks and validated cleanup; larger maintenance burden than reusing a stack | Original implementation could use published [Bluetooth HCI specification](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-60/out/en/host-controller-interface/host-controller-interface-functional-specification.html); no upstream implementation copied here |

The prototype below imports no third-party runtime or code. Its original code
uses the repository MIT license. The sources above inform the investigation;
they are not a license to copy all code or embedded data into the MIT core.
Pin and review actual dependency versions/licenses before a live sidecar is added.

## What direct HCI gives us

Controller advertising reports carry address type and report bytes, avoiding
CoreBluetooth's opaque identity and cached-property abstraction for this path.
Legacy advertisements and scan responses remain separate observations. Extended
reports expose PHY codes, advertising SID and controller data-status flags when
the controller supports them. This is more useful evidence for contextual
matching and Remote ID than synthesizing AD bytes from OS maps.

HCI is still **controller output**, not promiscuous raw RF or a WiFi-style monitor
mode. Firmware scanning/deduplication/filtering, radio scheduling and controller
buffers can lose traffic. It does not expose every radio PDU, arbitrary connected
traffic, RF channel/time metadata, decrypted payloads or calibrated distance.
A host receive timestamp is not an on-air timestamp. Public, random, resolved
identity and anonymous address types must remain distinct.

## macOS ownership constraint

[libusb #1762](https://github.com/libusb/libusb/issues/1762) reports a kernel panic
while claiming a Bluetooth interface on macOS 26.3.1. The
[BTstack port notes](https://github.com/bluekitchen/btstack/blob/master/port/libusb/README.md)
also document the macOS 26 issue and report CSR8510 operation through an Ubuntu
VM with UTM. [Bumble's macOS instructions](https://google.github.io/bumble/platforms/macos.html)
describe experimental USB HCI access and the need to keep the controller out of
the OS stack.

This host reports macOS 27.2. The older report proves neither a crash nor safety
on this version. IORegistry inventory and CoreBluetooth feature queries do not
exercise interface ownership. Do not hide a USB claim inside enumeration or
an automatic startup fallback. A native macOS ownership experiment needs a
separately reviewed procedure, saved work, explicit target identity and recovery
plan. Changing the transport library alone cannot establish that an OS kernel
failure is fixed.

## Offline prototype

[`scripts/replay-hci-ble.py`](../scripts/replay-hci-ble.py) reads one contiguous
hexadecimal H4 **Event** packet per input line. It accepts LE legacy advertising
reports (`0x02`) and extended reports (`0x0D`); other event kinds are counted and
ignored. It has no USB import, device argument, HCI command sender or live mode.

```sh
# Synthetic fixture: one random-address legacy report, three AD bytes, -45 dBm.
printf '%s\n' '043e0f0201000166554433221103020106d3' | \
  python3 scripts/replay-hci-ble.py --receiver-id synthetic --session-id replay-1
python3 -m unittest discover -s scripts/tests -p test_hci_replay.py -v
```

The experimental output type is `experimental_ble_hci_report`, schema version
**0**, not a stable hostd or external-WiFi protocol. It records explicit replay
provenance, receiver/session identifiers, event/report sequence numbers, optional
address and RSSI, raw report data, raw PHY codes and controller data status. It
never invents a capture timestamp or claims live reception validation. Raw PHY
codes are preserved without assuming optional host coding-selection features.

Bounds: each H4 event is at most 258 bytes, a hex input line at most 518 bytes,
and a decoded batch at most 25 legacy or 10 extended reports. Length, event-count,
reserved event/address/status fields and signal values are validated. A malformed
report rejects the whole event before any of that event's records are emitted.
No histories or fragment buffers grow with the input; output backpressure stops
replay rather than silently dropping records. Counts on stderr describe replay,
not radio losses. Keep real captures and redirected output private.

No AD fields are decoded and no fragment reassembly is attempted. In particular,
`controller_data_status: complete` can denote the final fragment of a previously
split extended advertisement. `reassembled: false` prevents that report from
being mistaken for a complete reconstructed advertisement. Scan responses are
not concatenated to earlier advertisements. A production consumer must handle
these boundaries before invoking parsers that require complete AD structures.

## External BLE process contract to implement next

Use a new versioned contract with `hello`, receiver capabilities/configuration,
report, status/error and bounded control messages. Freeze backend/device identity
and a new session ID at receiver startup. Report separate source-delivery counts,
queue losses, malformed HCI events, incomplete/truncated reports and reconnects.
No fake MACs, cached-snapshot provenance, or RF-loss inference from queue counters.

For complete controller reports, expose typed borrowed manufacturer/service-data
views to the detector. For fragments, bound reassembly by bytes, number of sources
and time; key by receiver/session, address type/address, SID, scan-response flag,
and relevant PHY metadata. Drop/mark incomplete assemblies on reconnect or gaps.
Anonymous reports cannot be safely grouped using a fabricated address. Preserve
controller status even if a higher-level protocol payload happens to decode.

Keep controller initialization, scan mode, duplicate filtering and supported/
selected/validated features explicit. Normal BLE scanning does not validate raw
report completeness or Coded PHY. Confirm cancellation disables the requested
scan and releases only the chosen device; restart must start a fresh session.

## Linux-first live acceptance plan (not performed)

1. Save a pinned source/dependency manifest and identify the CSR unit by descriptors
   and USB port; do not choose the first Bluetooth device by accident. Keep the
   Mac's built-in controller and ALFA/M5 devices out of passthrough.
2. Prepare an isolated Linux/UTM guest with a known kernel/BlueZ version. Explicitly
   pass through only the intended dongle. Record what the guest's standard btusb
   driver reports before selecting a userspace ownership path.
3. Establish legacy 1M reception using a known configurable advertiser and the
   existing BlueZ scanner. Record HCI version/features without inferring capability
   from the product string. Do not attempt extended commands on this CSR baseline.
4. Evaluate a pinned Bumble receiver in the guest with explicit transport/device
   selection. Choose either kernel HCI access or direct USB ownership deliberately;
   do not run competing owners. Start with passive scanning, no pairing or device
   connections, and bounded report output. Preserve raw evidence locally.
5. Compare known payload changes/address types against replay and hostd results.
   Exercise malformed fixtures offline, then output backpressure, termination,
   unplug/replug and recovery. Define numeric loss/latency tolerances from the
   controlled baseline before the load run. An unknown RF transmit denominator
   cannot support a percentage packet-loss claim.
6. Validate any newer controller's extended/PHY modes separately, including fragment
   completeness. Only then plan native macOS USB ownership against current OS and
   libusb evidence; retain Linux as a supported external receiver alternative.

Open work: live transport binding, pinned dependency packaging, stable external-BLE
schema/hostd ingestion, bounded reassembly, controller cleanup/recovery, hardware
coverage and platform release gates. The replay tests do not close those items.
