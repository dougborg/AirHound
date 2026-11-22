# Discover attached devices

AirHound can inventory candidate receivers and location devices before starting a
session. Run from a source build:

```sh
hostd/target/release/airhound-hostd devices list
hostd/target/release/airhound-hostd devices list --json
```

On macOS, the same commands work with
`hostd/AirHound.app/Contents/MacOS/airhound-hostd`. This command is not included in
the published 0.2.0 preview archives.

By default, the command shows devices with candidate AirHound roles. Add `--all`
to include other enumerated devices, useful when investigating an unrecognized
adapter:

```sh
hostd/target/release/airhound-hostd devices list --all --json
```

The text view is for inspection; JSON is the integration boundary for scripts and
a future GUI. The inventory is a snapshot of operating-system descriptors and
interfaces. It does not start a scanner or choose devices for a session.

## What discovery establishes

On macOS, discovery reads IORegistry. On Linux, it reads sysfs. Related serial,
Bluetooth and network endpoints are grouped with their physical device where the
OS exposes that relationship. Native, system-managed interfaces are listed
separately when they cannot be assigned to a specific physical USB receiver.

An entry can identify a supported backend, a possible firmware/board family, or a
recognized device that AirHound cannot yet ingest. Read the entry's explanation
and requirements before adding it to a session. Device presence alone does not
establish permission, exclusive ownership, installed firmware, working RF
reception or readiness to scan.

The command does not open serial ports, toggle control lines, claim USB
interfaces, reset controllers, change network modes or channels, install firmware,
or start Bluetooth/WiFi scans. That distinction matters for ESP32 boards whose
USB-UART control lines can reset the processor and for multi-protocol tools that
can install a different radio image when a capture command starts.

## JSON output and readiness

The JSON document has `schema_version: 1`, `mode: "passive"`, `platform`, a
`devices` array and `diagnostics`. Each device includes:

- `id`, `identity_scope`, and descriptor-derived `name`;
- `usb.vid` and `usb.pid` when USB identity is available;
- `endpoints`, with a `kind` and `path` for each OS-visible interface;
- `candidates`, describing possible backend roles rather than asserting that a
  device is ready for all of them.

Each candidate has `kind`, `label`, `support`, `state`, `evidence`,
`requirements` and `capabilities`; a `configuration_hint` is included where
applicable. Requirement
entries identify their `name`, `state` and explanatory `detail`. A missing optional
Sniffle module path leaves Python package discovery unknown; it does not imply
that an explicit module path is required when the package is already installed.

`capabilities.scope: "backend_declared_not_device_verified"` describes what an
AirHound backend can request or represent, such as BLE packets, WiFi beacons or
fixed primary channels. It does not establish that the attached device, installed
firmware or current OS supports every listed mode. Unknown roles use
`capabilities.scope: "unknown"`; native BLE PHY/reception claims remain unknown.

Support and readiness are separate:

| Field | Meaning |
| --- | --- |
| `support: available` | An implemented AirHound path exists; this does not establish this device's readiness. |
| `support: experimental` | An implemented path has experimental acceptance limits. |
| `support: research` | Related investigation exists; a working live source is not implied. |
| `support: candidate` | Descriptor evidence suggests a possible role that needs identification. |
| `state: unverified` | Passive discovery cannot establish runtime readiness. |
| `state: needs_configuration` | The proposed path needs explicit settings or prerequisites. |
| `state: needs_identification` | Generic descriptors cannot determine the board or protocol. |
| `state: unsupported` | This candidate has no usable current AirHound path on the platform. |

The passive command does not label a receiver ready, busy or permission-blocked
without evidence. Runtime [scanner health](desktop-scanner-health.md) remains the
source of actual scan state. A device can have more than one candidate role;
these are alternatives or capabilities to investigate, not independent radios.

## Candidate identities

USB vendor/product IDs help identify candidates, but several boards use generic
bridges. They are not hardware or firmware authentication.

| Descriptor evidence | Interpretation and limits |
| --- | --- |
| `0e8d:7610` | MT7610U chipset candidate for the external WiFi receiver. This is the ID used by the tested ALFA AWUS036ACHM, but it does not prove the product brand or RF acceptance. |
| `2e8a:00c0` | Generic RP2040/Pico bridge. The tested CatSniffer uses this ID; another board with this ID is not thereby a CatSniffer or a Sniffle receiver. |
| `1209:babb` | Documented Catnip/CatSniffer composite identity. Inspect the exposed interfaces and installed firmware before selecting a capture protocol. |
| `10c4:ea60` | Generic CP210x UART bridge. Both processors of the tested AWOK board use this identity; the ID cannot distinguish the screen processor, second processor, an M5 board, or another ESP32 product. |
| `1a86:55d4` | WCH CH9102-family UART bridge, not a unique M5 identity. |
| `0483:5740` | STM32 CDC identity also used by Flipper. This alone does not prove Flipper firmware or that an AirHound companion is running. |
| `1546:01a8` | The u-blox USB GNSS identity observed in the tested setup. A visible endpoint does not establish a fix or a running gpsd service. |
| `1915:522a` | Nordic sniffer candidate observed in the local hardware inventory. Its capture protocol is not the Sniffle protocol. |
| `0a12:0001` | CSR8510-family Bluetooth candidate. OS exposure and direct USB ownership are separate questions; it is not evidence of extended/Coded-PHY support. |

The M5 uses a UART bridge, so discovery cannot determine whether it runs AirHound,
a bootloader or another application. Similarly, connecting a Flipper does not
identify an AWOK board on its GPIO header. Each AWOK USB port exposes a processor
separately; the two ports are not automatically inferred to belong to one PCB.

Descriptor provenance is recorded in the existing
[ALFA receiver guide](../drivers/mt7610u/README.md),
[CatSniffer assessment](catsniffer-integration.md),
[Flipper/AWOK setup](flipper-marauder.md),
[GNSS setup](gnss-wardriving.md), and
[USB Bluetooth investigation](usb-hci-receiver-investigation.md).
The [FreeBSD CH9102 USB-ID update](https://lists.freebsd.org/archives/dev-commits-src-main/2025-June/033075.html)
and [Flipper CDC descriptor](https://github.com/flipperdevices/flipperzero-firmware/blob/dev/targets/f7/furi_hal/furi_hal_usb_cdc.c)
support the generic bridge/CDC distinctions above.

## Identity and incomplete inventories

Entries with `identity_scope: "attachment_topology"` describe OS-visible devices
at their current topology or registry location. They are not permanent identifiers: changing USB ports, rebooting,
reconnecting, switching firmware or switching OS can change them. Device serial
numbers are not required to list a receiver, and endpoint paths should not be
interpreted as unique product identities. The synthetic macOS CoreBluetooth and
CoreWLAN entries use `identity_scope: "system_backend"`: their IDs identify OS
services, not a particular USB dongle or a proven physical radio.

Multiple endpoints in one entry do not imply multiple independent radios. A
composite device may share an antenna or radio between roles; two Linux network
interfaces may share one physical WiFi radio. The
[multi-receiver planner](multi-receiver-sessions.md) performs its own duplicate
resource checks before starting a session.

Enumeration failures and limits belong to the inventory's diagnostics. A partial
inventory is not proof that a missing device is absent. A successfully serialized
partial inventory can still return exit status zero; scripts must inspect
`diagnostics`, candidate states and requirements. Re-run after attaching a
device or changing OS permissions; discovery does not currently watch hotplug
events continuously.

## From inventory to a session

Use the suggested backend and endpoint as a starting point, then supply explicit
configuration in a [session file](multi-receiver-sessions.md). Discovery does not
run helper executables to test them, load firmware, or automatically activate an
attached receiver.

You can supply prerequisite paths to the listing command with
`--external-scanner`, `--external-firmware`, `--sniffle-python`,
`--sniffle-helper` and `--sniffle-module-path`. These allow passive path checks and
more complete configuration hints. A found file is not proof that an executable
runs, Python dependencies import, a firmware image is compatible, or the caller
can claim the USB device. Without the required inputs, hints remain incomplete;
do not run them as a ready-made session.

- **Native macOS WiFi:** scanning still requires Location Services permission.
  An inventory entry does not confirm that CoreWLAN will expose BSSIDs.
- **Native BLE:** `--list-ble-adapters` remains the separate OS-backed capability
  query. The passive USB inventory is not a substitute for BlueZ/CoreBluetooth
  visibility and permission checks.
- **ALFA/MT7610U:** configure the separate receiver executable and pinned firmware.
  Its direct USB path is independent of native CoreWLAN's location permission.
  The current receiver has no physical-device selector: an inventory ID does not
  tell its helper which of several attached MT7610U adapters to claim. The driver
  currently refuses to open a device unless exactly one matches. Sessions
  currently accept only one external MT7610U receiver. With several attached,
  each candidate reports an unsupported `physical_device_selection` requirement;
  its configuration hint cannot target that inventory ID.
- **Sniffle:** confirm the board and installed protocol, then provide the serial
  endpoint, helper, Python environment and upstream module path. A generic Pico
  descriptor cannot establish these prerequisites. For composite Catnip devices,
  identify the capture endpoint explicitly; Cat-Bridge, Cat-LoRa and Cat-Shell
  are not interchangeable just because each exposes a serial port. If several
  serial endpoints are present, `serial_endpoint` reports `needs_selection` and
  the hint leaves `port` null rather than selecting the first port. An absent
  serial endpoint reports `missing`.
- **GNSS:** configure gpsd separately and select it with `--gpsd`; a USB GPS entry
  is a location-source candidate, not an observation receiver.
- **M5, Flipper, Marauder and Nordic sniffer:** discovery does not add missing live
  ingestion. Follow each component's current integration guide.

Active protocol identification, firmware/version checks and ownership checks
need a separate explicit probe operation with device-specific behavior. They
must not be inferred from a successful passive inventory. AirHound does not yet
provide a general active `devices probe` command.
