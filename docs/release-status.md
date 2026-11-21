## Preview status and installation

This is an **engineering preview**, not a completed hardware acceptance release.
It includes the desktop scanner, shared signature/evidence system, M5 observation
streaming and UI fixes, optional gpsd input, and CatSniffer/Sniffle packet input.
Signature matches are evidence, not confirmed device identity; research candidates
remain disabled.

### Downloads

- `airhound-m5stickc.bin`: merged no_std image for **M5StickC Plus2** (ESP32).
- `airhound-xiao.bin`: merged no_std image for **Seeed XIAO ESP32-S3**.
- `SHA256SUMS`: checksums for both images. Verify before flashing.

These images include the bootloader/partition layout and are written at address
zero. Back up your board before replacing firmware. Do not use the Plus2 image
on another M5 model. See the [firmware guide](https://github.com/dougborg/AirHound/blob/main/docs/firmware.md)
for checksum, flashing, serial baud and source-build instructions.

Desktop macOS/Linux and ESP-IDF firmware are **source builds** in this release;
there are no signed/notarized desktop installers or ESP-IDF release binaries.
See the [desktop guide](https://github.com/dougborg/AirHound/blob/main/hostd/README.md).
The macOS default includes private Bluetooth address lookup; a public-API-only
build is available. Sniffle requires separately installed compatible upstream
tools and the explicit helper/serial-port configuration in the
[Sniffle guide](https://github.com/dougborg/AirHound/blob/main/docs/sniffle-source.md).
The separate Sniffle helper is GPL-3.0-only; upstream radio firmware and tools
are not bundled in the MIT Rust application or these firmware images.

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

Release Please updates the root crate and its copies in the desktop, ESP-IDF and
fuzz lockfiles. Keep locked builds enabled. Review the version PR and require its
CI checks before merging. The release workflow creates a draft, builds both
no_std images, checks the required asset set, generates checksums, then publishes
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
