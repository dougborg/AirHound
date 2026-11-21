# AirHound

[Preview downloads](https://github.com/dougborg/AirHound/releases) ·
[Package inventory and validation status](docs/release-status.md)

AirHound is a Rust toolkit for observing nearby WiFi and Bluetooth devices and
matching their advertisements against device signatures. It includes ESP32
firmware, a macOS/Linux desktop CLI, a portable `no_std` detection library, and
versioned observation and signature formats.

**A signature match is a lead, not confirmed device identity.** Vendor prefixes,
standard BLE services and names can belong to unrelated devices. Observed address
counts are separate from signature hits; rotating addresses can represent the
same physical device.

## Choose a starting point

| Goal | Guide | Current scope |
| --- | --- | --- |
| Run a portable detector | [ESP32 firmware](docs/firmware.md) | M5StickC Plus2 and XIAO ESP32-S3; [M5 controls](docs/m5-controls.md) |
| Scan from macOS or Linux | [Desktop CLI](hostd/README.md) | Native BLE/WiFi, WiGLE CSV, NDJSON and optional gpsd location |
| Use an external receiver | [Receivers and drivers](drivers/README.md) | CatSniffer/Sniffle BLE capture and experimental ALFA MT7610U integration |
| Develop the Flipper companion | [Flipper prototype](companions/flipper/README.md) | Host-tested receiver and synthetic API87.1 FAP; no live UART yet |
| Work on detection logic | [Shared core and source layout](src/README.md) | Parsing, signatures, rule evidence and alert policy |
| Improve signatures | [Signature packs](signatures/README.md) | Canonical production pack, research candidates and admission policy |
| Consume observations or send commands | [Schemas](schemas/README.md) | Firmware/desktop contracts, examples and transport limits |
| Build the ESP-IDF alternative | [ESP-IDF firmware](firmware-std/README.md) | Separate runtime and build configuration |
| Develop, benchmark or validate | [Contributing](CONTRIBUTING.md), [scripts](scripts/README.md) | Tests, coverage, fuzzing, packaging and live validation |

The [documentation index](docs/README.md) groups the detailed guides by task.
The [repository map](docs/repository-layout.md) explains component build roots,
toolchains, generated files and licensing boundaries.
There is no desktop GUI yet: the macOS `.app` supplies permission identity for the
CLI. `hostd` does not yet ingest M5/XIAO serial or BLE GATT streams. A separate
consumer can use the [firmware streaming contract](docs/firmware-streaming.md).

<a id="desktop-scanner-configuration"></a>
<a id="linux-wifi-capture"></a>

## Desktop quick start

Install stable Rust and [`just`](https://github.com/casey/just). From the repository root:

```sh
# macOS: build the CLI and its permission bundle
just bundle-hostd
open -n hostd/AirHound.app --stdout /tmp/airhound-events.ndjson \
  --stderr /tmp/airhound.log --args --output /tmp/airhound-observations.csv
```

Allow Bluetooth and Location Services when prompted. Choose a new CSV path for
another run; existing files are protected. The standard build attempts private
BLE address lookup with UUID-only fallback. `just bundle-hostd-public-api` omits
that lookup; it is not by itself an App Store-ready distribution.

On Linux, install BlueZ, `libdbus-1-dev`, `libpcap-dev` and `pkg-config`
(Debian/Ubuntu names), then:

```sh
just build-hostd
hostd/target/release/airhound-hostd --output observations.csv
```

Linux defaults to BLE only. WiFi requires `--wifi-interface mon0`, an externally
configured monitor interface and capture permission. See the [desktop guide](hostd/README.md)
for radio selection, scan cadence, GNSS, exports and platform limits.

<a id="supported-hardware"></a>
<a id="m5stickc-plus2-features"></a>
<a id="remote-id-support-and-memory"></a>
<a id="quick-start"></a>
<a id="building"></a>
<a id="docker-recommended"></a>
<a id="native"></a>
<a id="flash"></a>

## ESP32 firmware

Use the [firmware guide](docs/firmware.md) for board selection, build/flash commands,
M5 PSRAM and Remote ID limits. Start with the [M5 bring-up procedure](docs/m5stickc-bringup.md)
when backing up or flashing a board. Compilation and downloadable images alone
do not establish hardware reliability.

<a id="why-airhound"></a>
<a id="design-philosophy"></a>
<a id="terminology"></a>
<a id="library-modules"></a>
<a id="firmware-pipeline-esp32"></a>

## Architecture

Radio backends supply observations to the shared detection library. Matching
attaches rule-specific evidence; alert policy is applied separately. Firmware
streams ordinary observations as well as matches. Desktop OS snapshots expose
different metadata from raw packets, and the contracts retain that distinction.
See the [core guide](src/README.md) and [evidence pipeline](docs/detection-evidence.md).

## Signature database

The [canonical v2 pack](signatures/builtin.v2.json) generates the Rust defaults
used by firmware and desktop. Its current compiled profile preserves existing
behavior; migrated entries are explicitly unvalidated. Research candidates are
not automatically enabled. See [authoring and admission](signatures/README.md).

## Standard formats

The [schema catalog](schemas/README.md) distinguishes production signatures,
legacy examples, firmware messages and richer desktop observations. It includes
validation commands and links to the versioned contracts.

<a id="device-messages-sensor---host"></a>
<a id="host-commands-host---sensor"></a>
<a id="ble-gatt-service-esp32-firmware"></a>
<a id="command-results-and-runtime-support"></a>

## Protocol

See the [protocol guide](docs/protocol.md) for message examples, GATT UUIDs and the
runtime command-support matrix, and [firmware streaming](docs/firmware-streaming.md)
for framing, loss counters and transport limits.

## Testing and analysis

The [testing guide](docs/testing.md) covers CI, coverage floors, fuzzing and
hardware-validation boundaries. [Desktop benchmarks](hostd/README.md#tests-and-performance)
measure pipeline processing, not radio scan frequency.

## Roadmap

The [roadmap](docs/roadmap.md) and [tracking issue #160](https://github.com/dougborg/AirHound/issues/160)
record remaining work and release gates. [CatSniffer/Sniffle](docs/sniffle-source.md)
feeds desktop observations; broader CatSniffer protocols and
[AWOK Dual ESP32](docs/awok-integration.md) remain integration investigations.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, validation and contribution
requirements. Signature contributions need sources, explicit limitations and
positive/negative evidence rather than stronger labels inferred from generic IDs.

## Related projects

Community work includes [Flock-You](https://github.com/colonelpanichacks/flock-you),
[OUI-Spy](https://github.com/colonelpanichacks/oui-spy),
[FlockSquawk](https://github.com/f1yaw4y/FlockSquawk),
[FlockBack](https://github.com/NSM-Barii/flock-back),
[ESP32Marauder](https://github.com/justcallmekoko/ESP32Marauder), and
[DeFlock](https://deflock.org). See [signature research](docs/signature-research.md)
for sources and evaluation; consult upstream projects for their current capabilities.

## License

AirHound is MIT licensed. The independent [MT7610U receiver](drivers/mt7610u/README.md)
is GPL-2.0-only and runs as a separate process; it is not linked into AirHound.
Its README documents firmware terms and hardware-validation limits.

The separate [Sniffle helper](scripts/sniffle-source.py) is GPL-3.0-only and uses
user-installed Sniffle tools; see [source setup and licensing](docs/sniffle-source.md).
