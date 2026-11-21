## Preview status and installation

This is an **engineering preview**, not a completed hardware acceptance release.
It includes the desktop scanner, shared signature/evidence system, M5 observation
streaming and UI fixes, optional gpsd input, and CatSniffer/Sniffle packet input.
Signature matches are evidence, not confirmed device identity; research candidates
remain disabled.

### Downloads and subproject inventory

| Component | Release deliverable | Status / requirements |
| --- | --- | --- |
| M5StickC Plus2 / XIAO ESP32-S3 no_std | `airhound-m5stickc.bin`, `airhound-xiao.bin` | Merged images; Plus2 image is not for other M5 models |
| ESP-IDF alternative runtime | `airhound-m5stickc-std.bin`, `airhound-xiao-std.bin` | Experimental merged images using their built ESP-IDF bootloader and partition table; hardware matrix incomplete |
| Desktop macOS | `airhound-desktop-macos-arm64.tar.gz` | Apple Silicon, macOS 15+; CLI app bundle, ad-hoc signed, not Developer ID signed/notarized; default private Bluetooth address lookup enabled |
| Desktop Linux | `airhound-desktop-linux-x86_64.tar.gz` | Built on Ubuntu 22.04; glibc 2.35+, libdbus-1.so.3, libpcap.so.0.8; BlueZ for native BLE |
| ALFA AWUS036ACHM / MT7610U receiver | `mt7610u-lab-macos-arm64.tar.gz`, `mt7610u-lab-linux-x86_64.tar.gz` | Separate experimental GPL-2.0-only userspace receiver, not an OS network driver; explicit USB permissions/ownership and separately acquired firmware required |
| ALFA corresponding source | Matching `mt7610u-lab-*-source.tar.gz` archives | Exact driver and locked dependency sources, license notices and offline build instructions; each binary manifest names/hashes its source archive |
| Shared library, production signatures, schemas, tools, docs | `airhound-source.tar.gz` | Full tracked source at the release commit; includes canonical v2 pack, revision registry, compiler and tests; runtime pack loading is not implemented |
| CatSniffer / Sniffle | Desktop backend plus `scripts/sniffle-source.py` in source archive | Separate GPL-3.0-only helper; upstream Sniffle tools/firmware must be installed separately; no automatic flashing |
| GPS / GNSS | Integrated desktop `--gpsd` support | Install/run gpsd separately; no dedicated GPS driver package |
| CSR/raw Bluetooth HCI | Offline replay tool and investigation in source archive | No live USB dongle driver or desktop HCI source is being released |
| AWOK Dual ESP32 | Investigation/roadmap in source archive | No board firmware or live ingestion implementation yet |
| Flipper / sub-GHz | Roadmap and research in source archive | No companion application or sub-GHz decoder package yet |
| Benchmarks, profiling, fuzzing, hardware validation | Scripts, harnesses and procedures in source archive | Development tools, not capture-performance or production-readiness claims |

`SHA256SUMS` covers all eleven downloadable payloads. Verify it before using the
files. The firmware images include bootloader/partition layout and are written
at address zero. Back up your board first; use only the image for its exact board.
See the [firmware guide](https://github.com/dougborg/AirHound/blob/main/docs/firmware.md)
for flashing and baud settings (M5 no_std 460800; ESP-IDF 115200).

Extract desktop archives and run their executable from a terminal; the macOS app
is a permission-bearing CLI bundle, not a GUI. Linux runtime packages on Ubuntu
are `libdbus-1-3`, `libpcap0.8` and `bluez`. Other architectures require a source
build. macOS downloads may require the normal OS approval flow for software from
an unidentified developer; do not disable Gatekeeper globally.

The ALFA Linux binary is build/smoke-tested, not RF hardware validated. Prefer the
native Linux mt76 driver and AirHound's monitor-interface input for ordinary Linux
use. Do not let the kernel, VM and userspace receiver compete for the same adapter.
No ALFA firmware is bundled. Receiver binaries and corresponding source are separate
from MIT AirHound; upstream and dependency license notices are included.

Extract the source/tools archive to use Sniffle helpers, schema examples, signature
compiler and research tools with their relative file layout intact. Python tools
need their documented dependencies; research candidates remain disabled. The
standard macOS executable includes private address lookup; build with
`--no-default-features` for public APIs only. See the
[desktop guide](https://github.com/dougborg/AirHound/blob/main/hostd/README.md) and
[Sniffle guide](https://github.com/dougborg/AirHound/blob/main/docs/sniffle-source.md).

### Validation and known limits

- M5StickC Plus2 no_std: earlier development images were exercised on hardware;
  screen and both buttons were confirmed, and startup readiness fixes passed
  fresh-boot captures. New release images require their own post-flash check.
- XIAO hardware and both ESP-IDF variants: builds are checked in CI; the full
  live radio/GATT/control acceptance matrix remains unverified.
- macOS native WiFi/BLE, gpsd output and CatSniffer live/replay paths have local
  smoke-test evidence. These short tests do not establish long-term reliability,
  route accuracy, complete RF capture or Linux radio hardware acceptance.
- CatSniffer currently selects one primary advertising channel at a time.
  Extended-chain reassembly and controlled extended/Coded-PHY reception remain
  pending. Random BLE addresses do not become permanent device identifiers.
- The optional ALFA MT7610U receiver remains experimental: MCU calibration
  timeouts interrupted the attempted endurance run; sustained operation,
  unplug/sleep recovery, RF parity and RSSI calibration are not established.
- Firmware and desktop typed BLE matching still have parity work outstanding.
  M5 no_std intentionally excludes WiFi Remote ID. Known-transmitter Remote ID
  acceptance and broader signature provenance/false-positive audits remain open.
- Wokwi simulation is not counted as passed when skipped for unavailable tokens.

The outstanding hardware matrix is tracked in
[#141](https://github.com/dougborg/AirHound/issues/141), with receiver gates in
[#155](https://github.com/dougborg/AirHound/issues/155),
[#156](https://github.com/dougborg/AirHound/issues/156), and
[#157](https://github.com/dougborg/AirHound/issues/157). Preview publication does
not close those gates or promote experimental features to production support.

### Maintainer release procedure

Feature releases advance the minor version even before 1.0 (for example,
0.1.1 → 0.2.0); fixes alone advance the patch. Breaking changes before 1.0 also
advance the minor version. GitHub prerelease status is independent of that version
number and remains enabled while the hardware acceptance matrix is incomplete.

Release Please updates the root crate and its copies in the desktop, ESP-IDF and
fuzz lockfiles. Keep locked builds enabled. Review the version PR and require its
CI checks before merging. The release workflow creates a draft, builds all
firmware/desktop/receiver packages and source archives, checks the complete required
asset set and matching receiver source hashes, generates checksums, then publishes
as a GitHub prerelease without replacing the latest stable release. This status
document is appended to the generated changelog. Review it for every release.

The TOML extra-file selectors use `@.name.value`: Release Please's formatting-
preserving TOML parser wraps strings in tagged values. The selectors were checked
with Release Please 17.6.0 against all three real lockfiles: only the `airhound`
version changed, with dependency pins and all other bytes preserved. Recheck
this behavior when upgrading the release action; the release PR's locked desktop,
ESP-IDF and fuzz builds also guard against omitted updates.

Before changing to stable publication, complete or explicitly rescope the
acceptance matrix and update both `prerelease` settings (Release Please and the
publisher), the preview title, and the latest-release policy together.
