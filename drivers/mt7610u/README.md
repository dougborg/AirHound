# MT7610U userspace receiver experiment

A standalone Rust/libusb driver for passive Wi-Fi reception with USB
`0e8d:7610` (MediaTek MT7610U). It runs directly on macOS without a kernel
extension or a replacement system network driver. It is an independent Cargo
workspace: AirHound does not build or link it.

## Hardware result

On 2026-11-19, an attached ALFA adapter on Apple Silicon macOS successfully:

- Opened USB interface 0 and identified ASIC `0x76100002`.
- Loaded the pinned firmware into MCU RAM and reported MCU ready.
- Read logical EEPROM calibration data, initialized the radio, and tuned
  channel 1 at 20 MHz.
- Captured **961 beacons from 20 distinct BSSIDs in 10 seconds**, with 2,416
  decoded frames from 2,449 USB transfers (609,032 bytes).

All 1,954 CRC-clean frames decoded successfully. There were also 495 CRC-error
frames: 462 were structurally decodable but excluded from beacon observations;
33 had malformed headers/payloads and were rejected. No parser framing defect
was found in the recorded transfers.
After cleanup hardening, a repeat five-second run received 557 beacons from
21 BSSIDs. A SIGTERM test received 233 beacons before stopping after about two
seconds. Both returned success and read back MAC TX/RX, automatic ACK and beacon
transmit gates as disabled (`cleanup_verified: true`).
This is evidence of working reception on one adapter, not a complete driver or
a sensitivity benchmark. Exact ALFA retail model is unconfirmed.

## Current limits

- Channel 1 only; no channel hopping or 5 GHz support yet.
- Full, LC, TSSI and TX-related calibration are omitted. Receive sensitivity
  remains experimental. `rssi_dbm` is `null`; raw RSSI bytes are retained without
  pretending they are calibrated dBm.
- No native macOS Wi-Fi interface, association, IP networking or packet injection.
- The host never enables MAC TX, automatic ACKs or beacon transmission, and
  exposes only the MCU bulk-output endpoint. This has not been independently
  verified with RF test equipment.
- No EEPROM/efuse programming or persistent firmware installation. Firmware
  lives in RAM; USB unplug/replug resets the device.
- No AirHound integration yet. The intended boundary is a separate process
  exporting observations through a documented protocol.
- Automated tests use simulated registers and packet fixtures. Linux CI checks
  portability; Linux hardware operation has not been validated.

## Build and run

Install stable Rust and libusb (`brew install libusb pkg-config` on macOS, or
`libusb-1.0-0-dev pkg-config` on Debian/Ubuntu). Run from this directory so the
independent `rust-toolchain.toml` is selected:

```sh
cd drivers/mt7610u
cargo build --locked
cargo run --locked -- probe
```

`probe` only reads registers and USB descriptors. Exactly one matching adapter
must be attached, with configuration 1/interface 0 available. The tool does not
detach an existing kernel driver or change USB configuration implicitly.

Firmware is **not bundled or downloaded automatically**. Supply an unmodified
`mt7610e.bin` image from [the pinned mt76 source](https://github.com/openwrt/mt76/tree/22f7ff9be59890cb10cde16cc25a30f78b504f23/firmware)
after reviewing [its separate license](FIRMWARE_LICENSE). Despite its filename,
upstream uses this image for MT7610U USB hardware too. Required SHA-256:

```text
538d08c08f8fb5aa05cacf34e49aeb307e8b2dc094407fc655fbf3580c893953
```

```sh
cargo run --locked -- inspect-firmware /path/to/mt7610e.bin
cargo run --locked -- bootstrap /path/to/mt7610e.bin
mkdir -p captures
cargo run --locked -- capture --firmware /path/to/mt7610e.bin \
  --output captures/beacons.ndjson --seconds 10
```

`inspect-firmware` does not open USB. `bootstrap` loads volatile firmware and
leaves MAC TX/RX disabled. If the MCU is already running, upload is skipped;
the supplied file's hash does **not** verify the image already in device RAM.
Unplug/replug before a run when firmware provenance needs to be established.

Capture writes beacon observations as NDJSON, including BSSID, SSID bytes,
advertised channel, receive channel and raw RSSI. Terminal output contains
aggregate counts. Output files are created with mode `0600`, never overwritten,
and ignored under `captures/`. Optional `--raw-usb captures/transfers.bin` records
each raw USB transfer prefixed by its little-endian `u32` byte length. These
files contain nearby network identifiers and possibly raw data frames; keep
them local rather than committing them.

Capture duration is 1–300 seconds, with a 64 MiB USB transfer budget. Ctrl-C,
SIGTERM and SIGHUP request orderly shutdown. Cancellation is checked between
initialization stages and between receive operations; a stage already running
finishes its bounded USB operations first. Normal completion and returned errors attempt to
disable reception/transmission before releasing USB. Forced process termination
cannot guarantee cleanup; unplug the adapter to reset it. A run without any
valid beacons exits unsuccessfully instead of claiming receiver validation.

## Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The 27 local tests pass. The dedicated GitHub workflow runs these checks on Linux and macOS and audits
the standalone lockfile. Dependabot tracks this crate separately. Tests cover
firmware/header boundaries, USB framing, malformed receive descriptors and
beacon elements, EEPROM read-only commands, and simulated initialization traces
that keep transmit/ACK/beacon gates disabled.

Next milestones are longer capture/shutdown testing, offline replay of raw
captures, receive calibration and RSSI validation, channel hopping, and a
versioned companion-process interface for AirHound. Hardware measurements must
remain separate from simulated-test claims.

## Licensing and provenance

Source is **GPL-2.0-only**, derived from the Linux mt76 implementation pinned at
`22f7ff9be59890cb10cde16cc25a30f78b504f23`. See [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES), including retained BSD-Clear terms.
This license is separate from AirHound's MIT license. Firmware has its own
terms; source licensing does not grant firmware redistribution rights.
