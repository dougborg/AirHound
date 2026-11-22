# AirHound documentation

[Project home](../README.md) · [Contributing](../CONTRIBUTING.md) · [Roadmap](roadmap.md)

Start with a component README for setup and current support. Detailed contracts
and validation reports stay at their existing paths so external links continue
to work. Guides describe implemented behavior; investigations and measured runs
are labeled separately.

## Run or develop a component

| Component | Entry point |
| --- | --- |
| Shared detection library and no_std source | [src/README.md](../src/README.md) |
| ESP32 firmware: M5StickC Plus2 / XIAO ESP32-S3 | [Firmware setup](firmware.md) |
| Alternative ESP-IDF firmware | [firmware-std/README.md](../firmware-std/README.md) |
| macOS/Linux desktop CLI | [hostd/README.md](../hostd/README.md) |
| External receivers and independent driver | [drivers/README.md](../drivers/README.md) |
| Production signatures and research candidates | [signatures/README.md](../signatures/README.md) |
| JSON contracts and examples | [schemas/README.md](../schemas/README.md) |
| Development, packaging and measurement tools | [scripts/README.md](../scripts/README.md) |

## Detection and data contracts

- [Detection evidence and alert policy](detection-evidence.md)
- [Canonical signature authoring and revision rules](signature-packs.md)
- [Signature research and prior art](signature-research.md)
- [Observation/command protocol and GATT UUIDs](protocol.md)
- [Firmware streaming, framing and loss counters](firmware-streaming.md)
- [CatSniffer/Sniffle live capture and PCAP replay](sniffle-source.md)
- [Concurrent desktop receivers, configuration and source identity](multi-receiver-sessions.md)
- [Marauder / AWOK saved Wi-Fi capture import](marauder-import.md)
- [WiFi receive frequency and band](wifi-observation-contract.md)
- [Offline WiFi PCAP replay](wifi-pcap-replay.md): bounded radiotap replay, 6 GHz metadata and capture-time exports.
- [Typed BLE observation contract](ble-observation-contract.md)
- [Desktop Remote ID decoding and reception limits](desktop-remote-id.md)
- [External WiFi file format](external-wifi-v1.md) and [live process protocol](external-scanner-stream-v1.md)

- [Flipper companion parser and synthetic demo](../companions/flipper/README.md)

## Hardware setup and validation

- [Discover attached receivers and location devices](device-discovery.md)

- [Flipper Zero and AWOK Marauder setup, firmware and test status](flipper-marauder.md)

- [M5 backup, flashing and bring-up](m5stickc-bringup.md), [buttons and sound](m5-controls.md), [streaming experiments](m5-stream-performance.md)
- [Desktop scanner health](desktop-scanner-health.md) and [BLE receiver capabilities](ble-receivers.md)
- [GNSS setup and wardriving](gnss-wardriving.md)
- [MT7610U channel plan](mt7610u-channels.md) and [receiver release gates](receiver-release-gates.md)
- [MT7610U test procedure](../drivers/mt7610u/TESTING.md)

Receiver investigations: [USB HCI dongle](usb-hci-receiver-investigation.md),
[CatSniffer](catsniffer-integration.md), and [AWOK Dual ESP32](awok-integration.md).
These do not claim production integration or validated support for every listed mode.

## Development and project status

- [Repository layout and build roots](repository-layout.md)

- [Tests, coverage, fuzzing and performance](testing.md)
- [Preview release status, assets and publication procedure](release-status.md)
- [Contribution process](../CONTRIBUTING.md)
- [Priorities, issue inventory and release gates](roadmap.md)
- [Change history](../CHANGELOG.md)

## Native macOS interface

The [SwiftUI app](../apps/macos/README.md) browses hostd recordings and lists passive
device candidates. Capture control remains in the CLI for now.
