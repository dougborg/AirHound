# Desktop BLE receivers and capability evidence

`airhound-hostd --list-ble-adapters` emits one `ble_adapters` JSON record and
exits without starting a scan or creating a CSV file. Enumeration can still
require OS Bluetooth permission. It has a ten-second deadline. An empty adapter
list is explicit; permission/initialization errors produce a nonzero exit.

On Linux, choose an identifier from that list:

```sh
airhound-hostd --list-ble-adapters
airhound-hostd --ble-adapter hci1 --output observations.csv
```

`hciN` is the BlueZ backend identifier, stable for that controller's current
registration. It is not a permanent physical-device identifier: indices may
change after removal/replug. Diagnostics also retain the backend's adapter info
and controller address when exposed. An absent explicit choice fails startup;
it never falls back to another radio. With no choice, the existing first-adapter
behavior is retained. This implementation does not automatically reconnect to a
replacement adapter.

macOS CoreBluetooth manages adapter selection. `--ble-adapter` fails clearly on
macOS; plugging in a USB adapter does not make it selectable through this API.
The listed `system` receiver is a backend identity, not a USB bus address or an
assertion about which physical controller the OS selected.

## What the diagnostics mean

The active receiver is included as an additive `receiver` object in
`scanner_health`. Its identity is separate from observed peripheral identities.
Power/off/unknown events update availability, and scanner failures mark it
unavailable. A powered-on notification does not prove scanning resumed. Receiver
capability queries are initialization snapshots, not continuous controller
monitoring. No scan settings or system-wide PHY policy are changed.

Each extended-advertising/Coded-RX feature has four independent states:
`controller`, `api`, `selected_scan`, and `validated_reception`, each of which is
`supported`, `unsupported`, or `unknown`. Unknown is not unsupported. Current
software never reports validated reception from a feature flag.

- **Linux:** a read-only Bluetooth MGMT `Get PHY Configuration` query (command
  `0x0044`) obtains supported/configurable/system-selected masks. It uses a
  nonblocking management socket with a 400 ms deadline per adapter. Missing
  `CAP_NET_ADMIN`, unavailable commands, malformed replies and timeouts leave
  capabilities unknown with a diagnostic reason; normal scanning can continue.
  The supported LE Coded RX bit establishes only controller-reported support.
  `system_selected_mask` describes controller defaults, **not proof of the PHY
  used by this btleplug scan**. Extended-advertising controller/API exposure and
  selected scan PHY remain unknown on this path. See the
  [BlueZ MGMT specification](https://bluez.readthedocs.io/en/latest/mgmt-api/#get-phy-configuration-command).
- **macOS:** the public `CBCentralManager.supportsFeatures` extended-scan check
  populates API support when that selector exists. Controller details, selected
  scan PHY and Coded RX remain unknown. Apple's
  [extended scan feature](https://developer.apple.com/documentation/corebluetooth/cbcentralmanager/feature/extendedscanandconnect)
  does not establish Coded-PHY reception or arbitrary USB adapter support.

btleplug's default `ScanFilter` exposes service filtering rather than explicit
PHY selection or raw extended-report fragments. Typed payload preservation is
separate (#35/#36). Whether a lower-level Linux scan backend is needed remains
subject to the reception tests in #172; this diagnostic work does not settle it.

## Validation and local hardware notes

Software tests cover default/multiple/missing/stale selections, platform
restrictions, unknown versus unsupported states, controller-state changes,
malformed/wrong-controller/wrong-command MGMT responses, and CLI errors without
radios. The MGMT parser's supported and selected PHY masks are deliberately
independent in fixtures. A Linux container build is not a radio test.

The locally attached USB device reports `CSR8510 A10`, VID `0a12`, PID `0001`,
revision `8891`. Its descriptor alone does not authenticate the silicon or prove
support. Qualcomm's [CSR8510 A10 product brief](https://docs.qualcomm.com/doc/80-CT903-1/80-CT903-1_REV_AC_CSR8510_A10_Product_Brief.pdf)
describes Bluetooth 4.0 for the genuine chip; it is not an extended/Coded test
receiver candidate. The host inventory still identified the active macOS
controller as the PCIe BCM4378 after insertion. No reception validation of the
USB device is claimed.

Read-only enumeration on 2026-11-20 (macOS 27.2, capability implementation
`d0a0559`) reported one powered-on, system-managed CoreBluetooth receiver.
`extended_advertising.api` was `unsupported` from the public feature query;
controller, selected-scan and validated-reception fields remained `unknown`.
All Coded-RX fields were `unknown`. This measures API exposure, not RF reception.

Remaining #171/#172 acceptance includes identified Linux hardware/firmware and
BlueZ versions, measured legacy/extended/PHY combinations, controller removal
under load, and OS-specific payload completeness. Keep those results separate
from parser fixtures and feature queries.

## Direct USB HCI receiver investigation

A separate receiver process could own a USB Bluetooth controller through HCI and
feed AirHound typed BLE observations, analogous to the external WiFi receiver.
[Bumble's USB transport](https://github.com/google/bumble/blob/main/docs/mkdocs/src/transports/usb.md)
is one existing implementation to evaluate. This would bypass CoreBluetooth's
property abstraction, but would not add PHY/radio features absent in the dongle.
It also requires an external-BLE ingest contract; the current WiFi receiver
protocol must not be silently repurposed as BLE.

Before live macOS USB ownership, account for the documented kernel-panic issue:
[libusb #1762](https://github.com/libusb/libusb/issues/1762) reports a crash when
claiming a Bluetooth interface on macOS 26.3.1, and
[BTstack's libusb port notes](https://github.com/bluekitchen/btstack/blob/master/port/libusb/README.md)
describe the issue and report a Linux/UTM CSR8510 path working. These reports do
not establish either failure or safety on this host's macOS 27.2. Read-only
IORegistry enumeration and CoreBluetooth feature queries do not exercise that
interface-claim path. No direct USB claim, driver detachment, NVRAM change or
controller reset was performed in the local inventory check.

Suggested sequence: replay-tested HCI-to-observation adapter, then an isolated
Linux receiver/VM with explicit USB passthrough, followed by a separately planned
macOS ownership test. Preserve HCI address types, receiver/session provenance,
report completeness and bounded queues; do not reuse cached-OS snapshot labels
for direct controller reports. The CSR8510 is a legacy-path test target, not
extended/Coded validation hardware.

## External raw-packet sources

`--sniffle-port` selects live Sniffle USB capture; `--ble-pcap` selects a saved
DLT256 capture. These replace native BLE for that session and are not listed by
`--list-ble-adapters`, which enumerates native OS adapters. See the
[Sniffle source guide](sniffle-source.md) for metadata, fixed-channel/PHY selection,
reconnect behavior and unvalidated extended/Coded modes.
