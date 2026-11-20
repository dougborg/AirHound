# External receivers and drivers

[Project home](../README.md) · [Documentation index](../docs/README.md) · [Desktop integration](../hostd/README.md#live-external-receiver)

AirHound separates hardware acquisition from observation matching. Native OS
adapters live in `hostd`; the independent receiver below runs as a child process
and supplies bounded, versioned observations.

| Receiver | Status | Start here |
| --- | --- | --- |
| ALFA AWUS036ACHM / MT7610U | Experimental passive userspace WiFi receiver; live macOS evidence and remaining release gates documented | [Build, operation and limitations](mt7610u/README.md), [test procedure](mt7610u/TESTING.md) |
| CSR USB Bluetooth / raw HCI | Investigation and offline replay tooling; not a supported replacement macOS Bluetooth driver | [HCI investigation](../docs/usb-hci-receiver-investigation.md) |
| CatSniffer v3.x | Capture/import integration investigation; no end-to-end AirHound support claim | [Integration plan](../docs/catsniffer-integration.md) |
| AWOK original Dual ESP32 v3 | Hardware/firmware identification and passive source integration investigation | [Integration plan](../docs/awok-integration.md) |

The MT7610U project is GPL-2.0-only and is not linked into MIT AirHound. Its firmware
has separate terms; read its [licensing section](mt7610u/README.md#licensing-and-provenance).
It is a passive receiver experiment, not an OS network interface for internet access.

## Data and validation

- [External WiFi observation file format](../docs/external-wifi-v1.md)
- [Live scanner process protocol](../docs/external-scanner-stream-v1.md)
- [MT7610U channel support](../docs/mt7610u-channels.md)
- [Receiver release gates](../docs/receiver-release-gates.md)
- [Validation and packaging tools](../scripts/README.md)

Receiver claims must distinguish decoded fixtures, USB communication and actual
RF reception. Preserve missing RSSI, identity scope, channel/PHY support and loss
information instead of filling unavailable fields with invented values.
