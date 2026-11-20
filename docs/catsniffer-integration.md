# CatSniffer 3.2: AirHound integration assessment

Live Sniffle and classic-PCAP ingestion are implemented; see the
[AirHound source guide](sniffle-source.md) for commands, validation and limits.

Research and hardware validation date: 2026-11-20. USB identity, firmware migration and passive legacy BLE capture have now been validated on the connected board; AirHound live and replay ingestion now pass local functional tests. Tracking: [#180](https://github.com/dougborg/AirHound/issues/180). Repo context: AirHound README, detection-evidence.md, signature-research.md, and issues #42/#44 reviewed.

## Local hardware result

The unplug/replug comparison identifies this unit as the generic
`RaspberryPi Pico` USB device (`2e8a:00c0`), using the legacy single-port bridge.
The original radio reported `TI Packet Sniffer ver.1.10.0`. Its complete 704 KiB
flash was backed up and CRC-verified before installing official Sniffle 1.11.0
`sniffle_cc1352p7_1M.hex`; the new image's flash CRC and Sniffle API version were
verified. The RP2040 bridge was not reflashed. The working serial baud is 921600.
Private backups and captures remain local.

Three passive 15-second captures on advertising channels 37, 38 and 39 yielded
1,793, 1,185 and 953 packets respectively (3,931 total), with zero decode
exceptions. All observed packets used 1M PHY. Counts include scan requests and
responses transmitted by other devices; our receiver used passive scan mode
with connection following disabled. Address counts overlap between channels
and are not counts of unique physical devices.

This establishes basic BLE RF reception, not simultaneous three-channel
coverage, extended/Coded-PHY reception, loss-free operation or endurance.
AirHound now ingests DLT256 files and live Sniffle USB capture; extended-chain
reassembly and the broader receiver matrix remain under [#180](https://github.com/dougborg/AirHound/issues/180). The USB firmware descriptor
does not independently verify the PCB's exact 3.2 revision.


## Recommendation

Treat CatSniffer as an external **packet receiver feeding host-side normalization and the shared signature engine**. Start with saved Sniffle BLE capture ingestion, then passive live capture. It offers useful access to Bluetooth packet addresses, advertising payloads and potentially extended/Coded-PHY packets that CoreBluetooth does not expose. It also opens 802.15.4 and selected sub-GHz receive paths. It is not a Wi-Fi receiver or a general-purpose SDR.

Do not start by porting AirHound firmware to its MCUs or writing a macOS Bluetooth driver. Existing capture firmware and host tools provide a shorter path. CatSniffer serial packet capture is distinct from the CSR dongle’s USB HCI work (#176).

## Hardware identity: v3 family verified; exact 3.2 pending

Electronic Cats’ current product page specifies RP2040 USB/controller, TI CC1352P7 multiprotocol radio MCU, and Semtech SX1262 sub-GHz radio. The firmware repository explicitly distinguishes v3.x (RP2040/P7) from v1/v2 (SAMD21/P1). However, the hardware repository README enumerates revisions only through **3.1**, and its tree does not expose an independently named 3.2 directory. Do not label the user’s exact 3.2 board validated yet. Read its silkscreen and installed firmware identification before selecting images; legacy generic documentation still includes P1-era descriptions.

Sources: [product](https://electroniccats.com/store/catsniffer-v3/), [hardware repository](https://github.com/ElectronicCats/CatSniffer), [v3 firmware](https://github.com/ElectronicCats/CatSniffer-Firmware/tree/v3.x).

Silicon coverage is not board RF validation: TI publishes CC1352P7 ranges 431–527, 861–1054, 1069–1329 and 2360–2500 MHz; SX1262 silicon covers 150–960 MHz. Exact usable bands also depend on the board matching network, switch and antenna. Confirm these for revision 3.2 before claiming broad sub-GHz support. Existing tooling has LoRa and FSK capture; chip marketing support for other protocols is not proof of a working capture/decoder path.

Sources: [TI CC1352P7](https://www.ti.com/product/CC1352P7), [Semtech SX1262](https://www.semtech.com/products/wireless-rf/lora-connect/sx1262).

## Existing host and capture paths

Current Catnip documentation describes a composite USB device, VID:PID 1209:babb, exposing three CDC interfaces: Cat-Bridge for the CC1352 firmware’s binary protocol, Cat-LoRa for SX1262 text output, and Cat-Shell for control/identity. Older firmware enumerates differently; discover interfaces and validate identity instead of relying on VID/PID alone. The shell supports firmware/version/status queries. Sniffle uses its own serial protocol; it is neither the TI sniffer framing nor HCI.

There is **one switched antenna path**: current docs select band1 for CC1352 2.4 GHz and band3 for SX1262 sub-GHz. Do not promise simultaneous effective BLE and LoRa reception. TI capture framing and LoRa/FSK records differ; preserve source, protocol and configuration. LoRa records provide payload/RSSI/SNR; FSK records expose payload/RSSI/length. Report truncation and unavailable metadata explicitly.

Source: [Catnip protocol specification](https://github.com/ElectronicCats/CatSniffer-Tools/blob/main/catnip/docs/protocol.md).

| Path | Upstream available now | AirHound work / limits |
| --- | --- | --- |
| BLE / Sniffle | CatSniffer v3 supported; BLE 1M/2M/Coded PHY, extended advertising, PCAP and Wireshark extcap | Import packets, parse AD structures, preserve address type, reassemble bounded extended chains, validate coverage on actual hardware. Random addresses remain random, not permanent identities. |
| Zigbee / Thread | CC1352 TI sniffer image and fixed-channel captures, channels 11–26 | Normalize 802.15.4 evidence; implement protocol-specific parsing. PAN/short address alone is not a product signature. Encryption limits application identification. |
| LoRa / FSK | RP2040 controls SX1262; current Catnip captures and writes files | Match receive profiles/frequency to target signals. Preserve raw/decoded distinction. No blanket rtl_433 compatibility, wideband IQ capture, or automatic encrypted payload decoding. |

Sources: [Sniffle](https://github.com/nccgroup/Sniffle), [Catnip capture commands](https://github.com/ElectronicCats/CatSniffer-Tools/blob/main/catnip/docs/commands/sniff.md).

The CC1352 stores one protocol image at a time. SX1262 has no separately installed capture firmware; its driver runs on RP2040. Current Catnip can automatically flash CC1352 when a capture command selects another protocol. Thus `catnip sniff` is **not** a harmless discovery command. AirHound must separate identity/probing from explicit firmware installation, and use passive BLE mode rather than active scanning. Baud rates and bridge control-line behavior must follow the selected image; a blanket serial default is insufficient.

Source: [Catnip firmware guide](https://github.com/ElectronicCats/CatSniffer-Tools/blob/main/catnip/docs/firmware.md).

macOS Intel/Apple Silicon binaries are available; upstream automated tests exercise Linux, not systematic macOS hardware operation. VHCI is Linux-only, so it does not make CatSniffer a CoreBluetooth adapter. A fixed live Wireshark FIFO limits concurrent captures. Upstream `verify --test-all` transmits LoRa and changes configuration: exclude it from passive discovery/testing.

Sources: [Catnip README](https://github.com/ElectronicCats/CatSniffer-Tools/blob/main/catnip/README.md), [known limitations](https://github.com/ElectronicCats/CatSniffer-Tools/blob/main/catnip/docs/limitations.md).

## Licensing and implementation boundary

Catnip and Sniffle identify as GPLv3. Electronic Cats describes firmware as AGPLv3 and hardware as CERN OHL 1.2, but the CatSniffer-Firmware v3.x root LICENSE.md itself contains CERN OHL 1.2. Consequently, audit the particular image/source component before redistribution or derivation; do not assume one uniform firmware license. Prefer user-installed capture tools and independently implemented file ingestion while keeping upstream code out of AirHound’s MIT core. This is an integration recommendation, not a conclusion about every possible distribution arrangement.

Sources: [Sniffle license](https://github.com/nccgroup/Sniffle/blob/master/LICENSE), [Catnip README/license declaration](https://github.com/ElectronicCats/CatSniffer-Tools/blob/main/catnip/README.md), [firmware root license](https://github.com/ElectronicCats/CatSniffer-Firmware/blob/v3.x/LICENSE.md), [hardware README declarations](https://github.com/ElectronicCats/CatSniffer/blob/master/README.md).

## Staged work and backlog placement

1. **Identity and fixture acquisition:** inspect the actual board/installed versions using read-only commands; record exact revision, interfaces, antenna/profile, firmware hashes, OS and capture-tool versions. Acquire owned-device passive BLE captures, including negative/malformed/truncated fixtures and extended/Coded cases where supported. This stage needs the user's hardware; software importer development can start with redistributable upstream fixtures.
2. **Offline BLE ingestion first:** preserve observed address and address kind, full manufacturer/service data and UUIDs, timestamps, available RSSI/channel/PHY/CRC, original versus captured length, and receiver provenance. Feed the same observation/evidence contract as existing radios. Require bounded parsing/reassembly and explicit unavailable fields. Use #172 for the extended/Coded validation matrix and #171 for receiver capabilities.
3. **Live external receiver:** sidecar or explicit serial protocol adapter; no implicit flash. Test reconnect, timeout, backpressure, loss counters, interface selection and passive behavior on macOS/Linux. Link host ingestion #175. Single-radio/channel and shared-antenna constraints must appear in capabilities, not be silently hidden.
4. **802.15.4 next:** extend **existing #44**, which already names CatSniffer v3, with actual capture fixtures and integration tasks. Do not create a duplicate generic Zigbee issue.
5. **Sub-GHz after the shared signature foundation:** extend **existing #42** with a CatSniffer LoRa/FSK receiver path alongside Flipper/rtl_433, without assuming interchangeability. Keep profile selection and raw packet observation distinct from a validated device signature.

Device integration tracking issue #180: **“CatSniffer v3.x passive receiver integration: validate revision 3.2, capture fixtures, and host ingestion.”** Acceptance: exact board/firmware matrix; safe read-only discovery; pinned BLE PCAP fixtures/import; macOS/Linux passive live validation; explicit antenna/channel/PHY limits; licensing audit before bundled artifacts. Cross-link #175/#171/#172/#44/#42 and keep protocol features in those existing issues. Tracked in #180.

## Evidence pins

Hardware tree inspected: f7af615e3b0ca536b1702b3f2f26acada651d687. Catnip file blob hashes: README c22a00cfd02b046108af7f2ae5f5fe0c0df87227; protocol 56d01ec62cc49bf7d9f5d2aabc8fc70f91514d75; firmware dd4348c71b8d7df8e887eb2320334c80eaea15a4; limitations f008c424ab24d6055c8d672df1f5f23f1ac2cf83; sniff commands ea217597f13ed9ec0c180d262fb78b6cb2acb5ab. These are documentation snapshots, not claims of tested hardware functionality.
