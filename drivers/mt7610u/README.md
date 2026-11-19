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
a sensitivity benchmark. The adapter's physical label confirms AWUS036ACHM.

Subsequent hopping checks used a 500 ms requested dwell:

| Channel plan | Capture time | Beacons | Distinct BSSIDs | Hops | Retune time |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1, 6, 11 | 15 s | 1,051 | 48 | 27 | 1,140 ms |
| 1–11 | 15 s | 544 | 61 | 27 | 1,181 ms |

The all-channel run received at least one beacon while tuned to every supported
channel. Both runs verified disabled shutdown gates. These are opportunistic
captures of a changing RF environment, not controlled comparisons of antenna
range or sensitivity. At about 42–44 ms per retune, a three-channel 500 ms dwell
cycle takes roughly 1.63 seconds; eleven channels take roughly 5.97 seconds.
A SIGTERM test while hopping stopped after about two seconds and three hops,
with all shutdown gate readbacks verified.

Lower 5 GHz support was subsequently exercised on the same adapter:

| Channel plan | Capture time | Beacons | Distinct BSSIDs | Hops | Retune time |
| --- | ---: | ---: | ---: | ---: | ---: |
| 36, 40, 44, 48 | 15 s | 300 | 14 | 27 | 1,127 ms |
| 1, 36, 6, 40, 11, 44, 48 | 15 s | 598 | 60 | 27 | 1,167 ms |

The mixed-band run received beacons advertising channels 40/44/48 while tuned
to those channels, as well as 2.4 GHz beacons after repeated band transitions.
Channel 36 produced no valid beacons in either run; correct register programming
and simulated tests do not independently establish its receive sensitivity.
A SIGTERM run alternating 48 and 1 received 158 beacons before verified shutdown.
The independent MIT AirHound capture checker accepted all three versioned files
(300, 598 and 158 records), including their unvalidated signal provenance.

Some APs were heard while configured far from their advertised channel. Their
EEPROM-derived signal estimates were substantially weaker off-channel, and
sightings occurred throughout dwells, including before the first hop. This
favors out-of-band reception over a short stale-packet burst, but does not prove
the hardware cause. `receive_channel` records the configured channel;
`advertised_channel` records the beacon's own primary-channel information.
Neither is substituted for the other. `channel_epoch` and `dwell_elapsed_ms`
allow transition timing to be investigated without guessing from arrival order.

## Why support this adapter?

The dedicated USB radio can receive beacon BSSIDs directly without relying on
macOS Wi-Fi scan results, and can scan independently of the Mac's network
connection. Channel control and per-beacon observations make it useful for
passive Wi-Fi surveying; neither throughput ratings nor transmitter power alone
establish receive range.

[ALFA documents mainline Linux support](https://docs.alfa.com.tw/Support/Linux/MT7610U/)
for this chipset, starting with kernel 4.19. The confirmed AWUS036ACHM's
[specifications](https://www.alfa.com.tw/products/awus036achm) include dual-band
2.4/5 GHz operation and a detachable 5 dBi RP-SMA antenna. USB identification
alone does not distinguish that retail model from Tube-UAC2; the physical label
was used to verify the model. Antenna placement and
external antenna options can help surveying, but this prototype has not measured
a sensitivity or range advantage over the Mac's built-in radio.

This is a Wi-Fi 5-era 2.4/5 GHz chipset, not a 6 GHz receiver. One radio listens
on one channel at a time; hopping necessarily misses frames on other channels.
GPS/time/location correlation and AirHound ingestion remain separate work.

## Current limits

- Passive 20 MHz channels 1–11 and lower 5 GHz channels 36/40/44/48 only.
  Other 5 GHz channels, wider receive bandwidths and 6 GHz are unsupported.
- Full, LC, TSSI and TX-related calibration are omitted. Receive sensitivity
  remains experimental. `rssi_dbm` is `null`. `estimated_rssi_dbm` applies mt76's
  signed chain-zero formula (raw + EEPROM offset − LNA gain), selecting separate
  EEPROM corrections for 2.4 GHz and low 5 GHz, with checked
  arithmetic. This estimate is explicitly unvalidated; raw bytes are retained.
  Zero samples and A-MPDU samples without the RSSI-valid flag have no estimate.
- No native macOS Wi-Fi interface, association, IP networking or packet injection.
- The host never enables MAC TX, automatic ACKs or beacon transmission, and
  exposes only the MCU bulk-output endpoint. This has not been independently
  verified with RF test equipment.
- No EEPROM/efuse programming or persistent firmware installation. Firmware
  lives in RAM; USB unplug/replug resets the device.
- The [versioned file format](../../docs/external-wifi-v1.md) and independent
  AirHound-side checker establish the data boundary. Live process supervision,
  observation ingestion, detection matching and WiGLE export remain unimplemented.
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

# Visit the common non-overlapping 2.4GHz channels in order.
cargo run --locked -- capture --firmware /path/to/mt7610e.bin \
  --output captures/hopping.ndjson --channels 1,6,11 --dwell-ms 500 --seconds 30

# Cover all supported channels, at the cost of less time on each one.
cargo run --locked -- capture --firmware /path/to/mt7610e.bin \
  --output captures/all-channels.ndjson --channels 1,2,3,4,5,6,7,8,9,10,11 \
  --dwell-ms 500 --seconds 30

# Alternate 2.4GHz and supported low 5GHz channels.
cargo run --locked -- capture --firmware /path/to/mt7610e.bin \
  --output captures/dual-band.ndjson --channels 1,36,6,40,11,44,48 \
  --dwell-ms 500 --seconds 30
```

`inspect-firmware` does not open USB. `bootstrap` loads volatile firmware and
leaves MAC TX/RX disabled. If the MCU is already running, upload is skipped;
the supplied file's hash does **not** verify the image already in device RAM.
Unplug/replug before a run when firmware provenance needs to be established.

Capture writes [version 1 beacon observations](../../docs/external-wifi-v1.md)
as NDJSON, including a session ID, consecutive sequence numbers, host observation
timestamps, BSSID, SSID bytes, advertised channel, receive channel and raw RSSI.
Observation files are bounded to 64 MiB with 16 KiB lines, matching the independent
checker. Terminal output contains
aggregate counts. Output files are created with mode `0600`, never overwritten,
and ignored under `captures/`. Optional `--raw-usb captures/transfers.bin` records
each raw USB transfer prefixed by its little-endian `u32` byte length. These
files contain nearby network identifiers and possibly raw data frames; keep
them local rather than committing them.

Raw USB recording currently requires a single channel because its file format
does not include channel metadata. Replay needs neither hardware nor firmware:

```sh
cargo run --locked -- replay captures/transfers.bin
```

Replay emits aggregate counts only, rejects truncated/oversized records and
bounds input to 64 MiB including length prefixes. Its CRC-error counter covers
structurally decoded frames; malformed transfers are counted separately.

Channel plans reject duplicates and unsupported channels before opening USB.
The receiver stops MAC reception, drains queued old-channel USB packets with a
bounded wait, then retunes before accepting new observations. Failure to drain
aborts capture. The requested dwell is listening time between retunes, not a
guaranteed whole-sweep period. `--seconds` bounds the capture loop, excluding
initialization; a retune already in progress can finish after that deadline.
Summaries include per-channel observation counts,
listening time and retune overhead. Advertised beacon channel remains distinct
from receive channel: neighboring channels can overlap.

Capture duration is 1–300 seconds, with a 64 MiB budget including length prefixes
when recording raw USB. Ctrl-C,
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

The dedicated GitHub workflow runs these checks on Linux and macOS and audits
the standalone lockfile. Dependabot tracks this crate separately. Tests cover
firmware/header boundaries, USB framing, malformed receive descriptors and
beacon elements, EEPROM read-only commands, and simulated initialization traces
that keep transmit/ACK/beacon gates disabled.

Next milestones are longer capture/shutdown testing, receive calibration and
RSSI validation, broader 5 GHz support, and supervised live companion-process
integration with an explicit policy for unvalidated signal estimates. Hardware measurements must
remain separate from simulated-test claims.

## Licensing and provenance

Source is **GPL-2.0-only**, derived from the Linux mt76 implementation pinned at
`22f7ff9be59890cb10cde16cc25a30f78b504f23`. See [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES), including retained BSD-Clear terms.
This license is separate from AirHound's MIT license. Firmware has its own
terms; source licensing does not grant firmware redistribution rights.
