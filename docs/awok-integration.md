# AWOK original Dual ESP32 v3.0: AirHound integration assessment

Research date: 2026-11-20. Documentation/source research only; no hardware access, firmware changes, or RF activity. Tracking: [#181](https://github.com/dougborg/AirHound/issues/181).

## Identify the actual board first

The requested name identifies the original dual classic-ESP32 family, but not whether it is **Dual Touch v3** or **Dual Mini v3**. Both manufacturer listings specify two ESP32-WROOM chips, two external 2.4 GHz antennas, and GPS selectable by rear DIP switches for either or both chips. One chip handles the screen; the second supports CLI/GPIO. The listings say firmware is not included. The GPS module's exact model is not established by those pages. Sources: [official Touch v3](https://awokdynamics.com/products/dual-touch-v3), [official Mini v3](https://awokdynamics.com/products/dual-mini-v3).

These are not the newer Dual C5 boards: do not promise 5 GHz, sub-GHz, IEEE 802.15.4, or BLE extended/Coded-PHY reception from the original v3 designation. Classic ESP32 radio capabilities are documented in the [Espressif datasheet](https://documentation.espressif.com/esp32_datasheet_en.pdf); scanner output capabilities additionally depend on firmware.

## Firmware and hardware evidence

[Marauder](https://github.com/justcallmekoko/ESP32Marauder) supports serial CLI at 115200, with USB/UART paths depending on board. `info` and `help` establish actual installed firmware/commands. CLI text should not be assumed to be a stable structured observation protocol. [CLI documentation](https://github.com/justcallmekoko/ESP32Marauder/wiki/cli).

Marauder wardriving writes WiGLE-compatible SD records, but valid GPS fix is required for wardrive records. Missing records therefore do not establish absence of radio observations. [Wardrive documentation](https://github.com/justcallmekoko/ESP32Marauder/wiki/wardrive). Newer firmware also documents Recon mission directories containing `session.json`, `obs.rlog`, and optional probes, relations, and PCAP, with a desktop converter. This is an additional import candidate only after checking the installed version and whether this hardware supports that mode. [Recon converter](https://github.com/justcallmekoko/ESP32Marauder/wiki/Recon-Desktop-Tool). Raw GPS NMEA streaming exists, but a sentence is not itself a valid fix. [NMEA stream](https://github.com/justcallmekoko/ESP32Marauder/wiki/NMEA-Stream).

An alternative, [AWOKxDAG](https://github.com/dagnazty/awokxdag), has original classic-board profiles. These explicitly remain experimental and are implementation references, not verified schematics for the user's unit:

| Profile | Display/input | SD CS | GPS UART2 RX/TX/default baud |
|---|---|---|---|
| Touch v3 | ILI9341, XPT2046 touch | GPIO14 | GPIO4/13, 115200 |
| Mini v3 | 128×128 ST7735, five buttons | GPIO4 | GPIO21/22, 9600 |

Both profile documents select classic ESP32, 4 MB flash, and PSRAM disabled. Both use SPI GPIO18/19/23; display CS/DC/reset 17/16/5; backlight 32, with opposite polarity. Touch touchscreen CS is 21. Physical flash, PSRAM, SD, GPS baud, and input/display behavior need verification before using these assumptions. Sources: [Touch profile](https://github.com/dagnazty/awokxdag/blob/main/docs/dual-esp32-touch.md), [Mini profile](https://github.com/dagnazty/awokxdag/blob/main/docs/dual-esp32-mini.md).

AWOKxDAG also describes headless bridge profiles, BLE control and ESP-NOW forwarding. Its classic profiles have small bounded result tables and Wi-Fi-only combined views; this is not evidence of lossless dual-radio capture. A radio relay introduces its own scheduling and transmission behavior, so direct USB is the better first AirHound integration. [Project implementation documentation](https://github.com/dagnazty/awokxdag).

Both projects declare MIT for their main code: [Marauder license](https://github.com/justcallmekoko/ESP32Marauder/blob/master/LICENSE), [AWOKxDAG license](https://github.com/dagnazty/awokxdag/blob/main/LICENSE). Reused code still needs attribution and its actual dependency/subcomponent licenses checked. Firmware licensing does not establish an open hardware design license.

## Recommended sequence

1. **Identify and capture fixtures:** determine Touch/Mini, module markings, both USB serial identities, installed firmware/version, actual GPIO routing, SD layout and GPS module/baud. Photograph board labels and preserve existing firmware/configuration before any later flashing. Verify each chip separately; do not infer the second chip's output from the screen chip.
2. **External observations first:** use the generic detector-device ingestion work in [#175](https://github.com/dougborg/AirHound/issues/175). Start with saved passive capture fixtures; select a versioned CSV/PCAP/Recon import only where the available record schema is understood. Then implement an explicit serial adapter for the actual firmware. Preserve sensor/chip identity, source version, timestamp meaning, GPS fix quality, and dropped/truncated-data counters. Human-readable scan lists and WiGLE rows cannot supply missing manufacturer payloads/UUIDs: match only evidence actually retained.
3. **Native firmware second:** add a compact headless classic-ESP32 AirHound board profile on one chip, reuse existing scanning/core evidence machinery and the #175 stream contract, and test bounded internal-memory operation. Do not copy the M5 PSRAM assumptions. Add GPS/SD and display/input only after the minimal stream works reliably.
4. **Two-chip scheduling last:** assigning one chip to Wi-Fi and one to BLE could reduce each chip's coexistence load; alternatively split Wi-Fi channels. This is a design opportunity, not stock-firmware behavior or guaranteed throughput improvement. Validate channel coverage, loss, timestamp alignment and GPS provenance under concurrent load before claiming the benefit.

External antennas, portable GPS/SD capture and two independently programmable radios are the main potential benefits. They add capture coverage and deployment flexibility; they do not independently improve signature specificity.

## Backlog proposal

Board-specific tracking issue #181: **“AWOK original Dual ESP32 v3: identify hardware and add passive observation-source support.”** Link #175 for transport and #79/#98 for GPS/source provenance and aggregation. Acceptance: exact variant recorded; both chips/USB identities inventoried; licensed/pinned upstream references; passive fixture set; source capability matrix; bounded decoder/adapter with malformed, reconnect and truncation tests; per-chip provenance; documented capture limitations. Keep its initial scope external-source bring-up. Add a native board-target follow-up only after identification, with memory budget, pin verification and two-radio validation gates.

Do not duplicate generic ingestion (#175), payload evidence (#35), UUID preservation (#36), confidence/alert policy (#154), or source/geotag aggregation. Hardware support should consume the shared signature evidence contract, not invent device-specific matching policy.

## Offline importer implementation

The experimental [Marauder capture importer](marauder-import.md) now provides a
bounded, fixture-tested v1.21.0 PCAP/binary serial profile and shared identity-only
matching. This is source-build offline support, not a hardware acceptance result
or validation of the unit’s previously observed v1.10.2 firmware. Live USB, BLE,
reconnect handling and real capture fixtures remain follow-up work.
