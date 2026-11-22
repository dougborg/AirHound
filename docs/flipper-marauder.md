# Flipper Zero and AWOK Marauder setup

This guide covers the **original AWOK Dual Touch v3**, its two classic ESP32
processors, and control from a Flipper Zero. It records the setup tested on
2026-11-21; it is not a guide for the newer Dual C5, Dual Mini, or other boards.

## What is available

| Component | Current state |
| --- | --- |
| AWOK stock firmware | Official Marauder v1.21.0 installed on both processors; boot and CLI verified |
| Flipper firmware | Momentum mntm-012, API 87.1, retained unchanged |
| Marauder companion | Existing third-party WiFi Marauder app installed; GPIO command/scan path tested separately with a bounded Flipper-side UART script |
| AirHound capture importer | Experimental offline PCAP/serial-file import; real AWOK capture acceptance remains pending |
| AirHound Flipper companion | Bounded receiver/parser and synthetic FAP prototype; separate fixed-memory UART diagnostic in development, not hardware accepted |
| Flipper sub-GHz source | Planned; the UART test does not validate sub-GHz reception or host streaming |

The WiFi Marauder app is the
[upstream Marauder companion](https://github.com/0xchocolate/flipperzero-wifi-marauder),
not an AirHound app. Our [Flipper prototype](../companions/flipper/README.md)
and [native AWOK firmware project](https://github.com/dougborg/AirHound/issues/210)
are separate work.

## Two processors, two USB ports

Each processor has its own firmware and flash. Updating one does not update the
other, and a working touchscreen does not establish the Flipper side's state.

| Role | Verified Marauder target | v1.21.0 release asset suffix |
| --- | --- | --- |
| Screen processor | Marauder v6.1 | `_v6_1.bin` |
| Flipper-controlled processor | Flipper Zero Dev Board Pro | `_marauder_dev_board_pro.bin` |

Both processors were identified as ESP32-D0WD-V3 revision 3.1 with **16 MB flash**.
PSRAM is not verified; flash capacity is not RAM capacity. No SD card is installed
in the tested setup. Board GPS wiring, fix quality and SD recording remain untested.

On the tested Mac, either port appears as a **CP2102 USB to UART Bridge Controller**
(`10c4:ea60`), commonly `/dev/cu.usbserial-0001`. That name is not a reliable
processor identifier. Connect one AWOK USB port at a time and use the 115200-baud
`info` response to distinguish the targets. Keep the Flipper's own USB CDC port
separate. USB connections from both devices to a computer do not connect their UARTs.

The original board remains a 2.4 GHz Wi-Fi/Bluetooth platform. Installing newer
firmware does not add 5 GHz, BLE Coded PHY, or sub-GHz hardware. See the
[board assessment](awok-integration.md) for capability and pin-map caveats.

## Connect for Flipper control

1. Power off the devices before attaching or removing the AWOK GPIO header.
2. Seat the board in the intended orientation, with the header aligned correctly.
3. Power the setup normally. Connect the Flipper's USB to the computer if using
   desktop diagnostics; the successful GPIO test used no AWOK USB connection.
4. Open **Apps → GPIO → ESP → WiFi Marauder** on the tested Momentum installation.
   Other firmware builds may place the app elsewhere.
5. Request `info` first. The responding processor should identify the Dev Board Pro
   target and expected firmware version. A visible app menu alone is not proof
   that the board answered.

The verified command link uses the Flipper's USART at **115200 baud**. Only one
application or diagnostic should own it at a time. Exit the Marauder app before
running an AirHound/JavaScript UART diagnostic, and release the UART afterward.

A short passive command sequence is `info`, `sniffbeacon`, `stopscan`, then `info`.
Always stop a test scan; opening a different menu is not a reliable stop check.
This sequence is a connectivity smoke test, not a recording/export workflow.

## Updating and recovering the AWOK

Use the [official release assets](https://github.com/justcallmekoko/ESP32Marauder/releases)
and [upstream update guide](https://github.com/justcallmekoko/ESP32Marauder/wiki/Update-Firmware).
Match the exact board and processor; the `_flipper.bin` target is not the verified
Dev Board Pro target for this unit.

Our update preserved separate full-flash backups of both chips, partition metadata,
and original hashes privately. The verified layout placed `app0` at `0x10000`, with
an application slot of `0x1e0000` bytes. Only that application partition was updated;
bootloader, partition table and configuration partitions were preserved. Do not
apply those offsets to an unverified board or substitute the other chip's backup.
Release SHA-256 digests and esptool flash hashes were checked, followed by live
`info` responses. Full-backup restoration has not been tested.

Bootloader handshakes failed while the board was GPIO-attached, before any erase
or write. Updating the Flipper-controlled processor over its own USB while detached
succeeded. This is an observed workaround, not a proven electrical root cause.
Finish flashing and boot verification before reattaching for GPIO control.

The screen previously ran a temporary calibration experiment. Official v1.21.0
has replaced it; that experiment's RAM-only calibration was not a persistent fix.
In the tested Marauder menu, touch behaves as three navigation zones: upper area
moves up, lower area moves down, middle area selects on release. It does not work
like directly tapping each visible menu item. The planned AirHound touchscreen UI
is tracked separately in [#212](https://github.com/dougborg/AirHound/issues/212).

## What the live test established

Through the Flipper GPIO UART, the Dev Board Pro processor answered `info`, ran a
short passive channel-1 beacon scan, returned **62 beacon observations from 10
BSSIDs**, acknowledged `stopscan`, and answered `info` again. No panic was observed
during that short run. Repeated observations are not unique devices; BSSIDs are
radio identifiers, not necessarily separate physical access points.

`list -a` returned no retained AP rows in that run. The successful evidence is the
live beacon output, not a populated saved list. The existing companion app was
opened afterward; comprehensive menu-by-menu companion acceptance is still pending.

This does not establish sustained stability, channel hopping, simultaneous two-chip
capture, complete packet reception, Bluetooth scanning, GPS, SD logging, or sub-GHz
support. Longer scans, reconnects, capture transfer and import remain
[#181](https://github.com/dougborg/AirHound/issues/181).

## Current binary-capture limitation

The newer acceptance runs in the [hardware ledger](attached-hardware-validation.md#flipper-and-awok-follow-up)
failed to produce a complete binary capture. The accumulating mJS approach led to
an out-of-memory screen. The installed v0.7.6 companion later displayed beacon
reception but crashed with a NULL-pointer screen, leaving empty files. The precise
native fault remains unknown. Final stop-only recovery observed two firmware-text
stop acknowledgments; neither firmware nor the original companion was replaced.

Marauder 1.21.0 `info` changes its scan state. End any preflight information query
with an acknowledged `stopscan` before attempting a scan; an `info` response alone
does not mean the scanner is idle. Console acknowledgment text is not authenticated
and does not prove complete RF reception or a hardware-level stop.

The isolated [AirHound diagnostic](../companions/flipper/README.md#bounded-uart-diagnostic)
uses fixed memory and device-local timing. It must pass build, UART-free load/exit,
headroom and synthetic replay checks before a separately reviewed live test.
This is a diagnostic application, not a full firmware rewrite or a demonstrated
fix for the third-party companion crash.

A simpler fallback is Momentum's built-in **GPIO → USB-UART Bridge**, configured
for USART pins 13/14 at 115200 baud and **USB channel 1** so channel 0 can retain
the CLI. Entry defaults to channel 0 and can disrupt the management connection;
initial selection therefore requires the user's physical UI assistance. Do not
switch USB modes blindly over RPC. This path has not been accepted with AirHound;
its bounded RX queue may discard bytes without reporting every loss, so complete
host framing is necessary and still cannot establish lossless RF capture.

## Using captures with AirHound

Follow the [Marauder capture import guide](marauder-import.md) for supported binary
PCAP and serial framing, commands, timestamp semantics and missing metadata.
The importer is source-build tooling, not part of the published 0.2.0 desktop
archives. Human-readable beacon logs are not binary PCAP captures.

The v1.21.0 source path supports serial PCAP without an SD card, but requires
`SavePCAP` enabled; this path still needs live acceptance on this board. Do not
infer lossless capture from a successfully parsed file. Keep captures, identifiers,
GPS tracks and flash backups private.

## Related work

- [AirHound Flipper prototype and build/API checks](../companions/flipper/README.md)
- [USB CDC library contribution](https://github.com/flipperzero-rs/flipperzero-rs/pull/257)
  and [BLE serial contribution](https://github.com/flipperzero-rs/flipperzero-rs/pull/256):
  hardware-tested library building blocks, still separate from the AirHound companion.
- [Multi-receiver sessions](https://github.com/dougborg/AirHound/issues/229): eventual
  independent source identities and scan assignments; current GPIO control is not
  yet a live AirHound source adapter.
- [Flipper sub-GHz reception](https://github.com/dougborg/AirHound/issues/42): a separate
  receiver role, with concurrency and transport still to validate.
