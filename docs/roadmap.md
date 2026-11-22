# AirHound roadmap

Published tracking index: [#160](https://github.com/dougborg/AirHound/issues/160).

Updated 2026-11-22. Issue bodies own acceptance criteria; this index owns ordering.
Older evidence and inventory below are retained as dated context, not current
release or implementation claims.

## Recommended order

1. Finish the local shared-client integration: Rust service/CLI (#238/#243),
   TUI (#239) and MCP (#240). Native SwiftUI (#158) remains presentation; live
   service-owned controls are the next slice. Preserve desktop coverage floors.
2. **P1: durable runs and SQLite recording.** Implement contracts (#54),
   service-owned Start/Pause/Resume/Finish and recovery (#56), SQLite (#57), and
   evidence history (#55) together. See the [storage plan](storage-plan.md).
   Deliver bounded writes/queries, migrations, visible failures and minimal
   history before extensive GUI history or mobile synchronization. Retention #59
   starts with explicit limits and error accounting; advanced aggregation follows.
3. **Validate sources incrementally:** replay fixtures, one known receiver, two
   receivers, mixed WiFi/BLE/GPS, then additional discovered candidates. At each
   stage verify source identity, timing/location provenance, counts, reported drops,
   channel assignment acknowledgements, reconnect and clean shutdown. Relevant
   work: #229, #175, #171, #155 and #156. Enumeration is not capture acceptance.
4. Build on that foundation with minimal native/terminal/API history and controls,
   then phone roles (#246): location source, capture source, controller or gateway
   for attached receivers. iPhone BLE/GATT is the first proposed gateway path;
   Android USB host and channel tuning depend on actual endpoint capabilities.
   Authenticated networking, offline phone sync and vehicle displays (#247) follow.
5. **Prepare a release preview after the storage/multi-source milestone:** verify
   artifacts and clean-install behavior, write versioned release notes, and update
   the tested hardware matrix. Carry untested/experimental capabilities and
   unresolved hardware acceptance forward explicitly; do not infer RF support
   from parser tests or packaging success.

Keep signature quality/provenance (#154/#14/#16), constrained firmware memory
(#153), permission lifecycle (#159), and hardware fault/endurance/distribution
(#155/#156/#157) visible throughout. This ordering does not waive those gates.
SQLite, remote phone roles and service-owned live controls are still planned;
NDJSON/CSV remain current recording/export paths.

## Historical integration evidence — earlier baseline

- #173 preserves bounded desktop BLE manufacturer/service payloads and full UUIDs,
  reports adapter capabilities without inferring RF support, and decodes normalized
  Remote ID data. Linux coverage was repaired from 80.13% to 81.04% without lowering
  the gate. Extended/Coded reception remains unvalidated.
- #177 adds optional gpsd geotagging with stale/disconnected fixes cleared and no
  silent source fallback. The combined tree has 82.37% local macOS production
  coverage, excluding test bodies. A granted-permission live run recorded 3,286
  rows: 2,539 geotagged, nine startup rows blank and 738 rows after gpsd stopped
  blank. Raw identifiers and coordinates remain local.
- #174 is offline HCI replay and a receiver investigation, not a live USB driver.
  The CSR dongle has not been claimed or reset on macOS. #176 owns live acceptance.
- #175 explicitly tracks the missing host mode for running M5/XIAO devices;
  hostd stdin remains a command channel, not a firmware-event ingestion channel.
- #170's smart-glasses/equipment and broader device research stays offline until
  labeled captures, ordinary-device negatives and provenance support promotion.
  Vendor/service identifiers do not establish model, ownership or surveillance.

The following platform table records the earlier baseline, not newer final-head
coverage or release acceptance. Consult the linked issues and PR checks for updates.

## Earlier platform baseline

| Area | State and evidence |
| --- | --- |
| Desktop CLI + Linux WiFi | Merged #144/#145: scanner/RSSI options, safe CSV output, Linux monitor-interface capture, desktop production-line coverage floors of 81%. #110/#105 closed as completed implementation deliverables; hardware validation remains #141 and channel management #30. |
| Protocol outcomes | #143 merged explicit supported/unsupported/error responses. Actual mode/category/proximity/export effects remain #72 and its feature children. |
| macOS lifecycle, BLE address lookup, benchmarks | #150 merged. Standard build includes private address lookup; public-API-only build is optional. UUID-only fallback is retained. Final live capture/permissions need #159; profiler success alone is not reception evidence. |
| External ALFA receiver | #151 merged (reviewed head 923dc3a). Separate GPL-2.0-only passive userspace receiver; MIT host uses a process protocol. Includes live supervision, 38 passive HT20 channels, refactoring, 75 driver tests and 85.52% local driver line coverage (81% gates). Linux/macOS coverage, fuzzing, performance, driver checks and clean-source candidate packaging passed. Not production ready. |
| M5StickC Plus2 | #162 merged: missing WiFi start fixed, AP/other WiFi/BLE counts separated from signature hits, 4,096-entry tracker in isolated 2 MiB PSRAM. Full PSRAM self-test and 180-second coexistence run pass; 149 addresses, 1,106 WiFi packets, 42 initial drops, zero hop errors. Internal free 3,672 B; external free 2,068,472 B. Combined-tree CI, all four firmware builds, preview packaging, coverage, fuzzing and CodeQL passed. |
| Quality baseline | Combined-tree CI: core 89.76%, host Linux 85.23%, host macOS 84.64%, receiver macOS 85.52% production-line coverage; all coverage floors and fuzzing pass. Local combined checks pass 313 core and 38 Python tests. Linked startup stack is 7,420 B (6,144 B minimum). Hardware acceptance remains separate. |
| Release | #123 remains open. Newly published assets, clean-host installs and live support matrix still need #141/#157. No eight-hour receiver soak or calibrated RSSI claim. |

## Decisions and limits carried forward

- Count observed addresses separately from suspect signature hits. A MAC/UUID is not proof of a unique physical device; randomized addresses, address type, table capacity and dropped observations must remain visible (#29/#71/#154).
- PSRAM does not eliminate internal radio memory or startup-stack requirements. Retain the 64 KiB internal heap; a 68 KiB trial linked but stalled radio startup with only 3,324 stack bytes. The working build reserves 7,420 stack bytes (#153). XIAO PSRAM is not enabled by the M5Stick work.
- Keep no_std versus ESP-IDF an evidence-based support decision (#18), and extended/Coded BLE a controller/transport capability issue (#132), not something inferred from parser tests or available RAM.
- macOS standard builds retain optional private address enrichment by default, with a public-only distribution option. Neither path guarantees stable MACs or App Store readiness (#150/#159/#158).
- Use the confirmed AWUS036ACHM/MT7610U hardware identity, not a different similarly named ALFA chipset. The current driver is a passive scanner, not an OS WiFi interface. Wider channels, TX or full NIC support are separate future decisions (#156/#157).
- Preserve GPL receiver separation and external firmware provenance; validate actual source/binary packaging rather than treating a process boundary as a blanket licensing answer (#157).
- Benchmark synthetic pipelines separately from live acquisition; publish only aggregate evidence. No raw MACs/SSIDs, captures or flash backups in issues or artifacts (#152/#155/#156/#159).

## Complete open-issue inventory

The original implementation/research inventory follows. New desktop issues are listed first; their acceptance criteria live in GitHub. The roadmap tracking issue itself is an index, not an additional deliverable. Later/research entries remain visible without implying current priority.

### Added desktop and integration work

| Issue | Work | Remaining scope |
| --- | --- | --- |
| [#171](https://github.com/dougborg/AirHound/issues/171) | BLE adapters/capabilities | Hardware matrix, durable identity and actual scan/API capability validation |
| [#172](https://github.com/dougborg/AirHound/issues/172) | Desktop Remote ID | Controlled legacy/extended/Coded reception and completeness validation |
| [#175](https://github.com/dougborg/AirHound/issues/175) | Detector-device host ingestion | Serial first, then bounded/reconnect-tested BLE GATT |
| [#176](https://github.com/dougborg/AirHound/issues/176) | Live USB HCI receiver | Linux/UTM experiment, host ingestion, separate macOS ownership assessment |

### Now — integrate and stabilize

| Issue | Work | Status / scope |
| --- | --- | --- |
| [#152](https://github.com/dougborg/AirHound/issues/152) | integrate the validated firmware and receiver work into reviewable PRs | #150/#151/#162 merged; dependency/release follow-up remains |
| [#141](https://github.com/dougborg/AirHound/issues/141) | test: validate firmware and desktop scanners before the next release | Partial hardware evidence; release gate remains open |
| [#153](https://github.com/dougborg/AirHound/issues/153) | enforce firmware memory/stack budgets and reduce scan-event loss | PSRAM and linked startup-stack guard merged in #162; runtime heap/stack/drop work remains |
| [#154](https://github.com/dougborg/AirHound/issues/154) | distinguish signature evidence from confirmed device identification | New; audit broad signatures and confidence |
| [#159](https://github.com/dougborg/AirHound/issues/159) | resolve macOS live scanner permission and startup regressions | Known live macOS capture/startup failures still need resolution |
| [#155](https://github.com/dougborg/AirHound/issues/155) | complete MT7610U endurance, disconnect and sleep/wake acceptance | Short runs pass; eight-hour/fault acceptance absent |
| [#156](https://github.com/dougborg/AirHound/issues/156) | validate MT7610U channel reception, calibration and Linux parity | 38-channel implementation merged; controlled RF/RSSI validation absent |
| [#1](https://github.com/dougborg/AirHound/issues/1) | deps: migrate ESP crates to a tested published-version set | Locked git dependencies; published-set migration remains |

### Next — usable sensors and desktop experience

| Issue | Work | Status / scope |
| --- | --- | --- |
| [#157](https://github.com/dougborg/AirHound/issues/157) | package and validate distribution of the standalone GPL receiver | Unsigned clean-source candidate packaging passed; signed distribution not validated |
| [#158](https://github.com/dougborg/AirHound/issues/158) | evaluate a native cross-platform desktop GUI for AirHound | Evaluation/prototype; no toolkit selected |
| [#132](https://github.com/dougborg/AirHound/issues/132) | feat(ble): support extended and Coded PHY Remote ID advertisements end to end | 62-byte scanner limit; extended/Coded reception unverified |
| [#35](https://github.com/dougborg/AirHound/issues/35) | fix: align BLE byte-pattern semantics and preserve desktop payloads | Exact bytes work; wildcard/offset and desktop payload gaps remain |
| [#36](https://github.com/dougborg/AirHound/issues/36) | feat: parse BLE 128-bit service UUIDs (AD types 0x06/0x07) | 128-bit parsing/matching and embedded UUID emission incomplete |
| [#71](https://github.com/dougborg/AirHound/issues/71) | feat(protocol): define observation/catalog and scanner-health messages | Catalog/typed health contract unresolved; current counters are screen/log only |
| [#72](https://github.com/dougborg/AirHound/issues/72) | feat(protocol): complete command effects and acknowledgments | Acknowledgments merged; feature effects remain scoped to children |
| [#8](https://github.com/dougborg/AirHound/issues/8) | feat(m5stickc): multi-screen UI with button navigation | Observed/suspect and memory counts merged; buttons/pages unfinished |
| [#7](https://github.com/dougborg/AirHound/issues/7) | feat(m5stickc): get buzzer working on match detection | Driver initializes; audible behavior unverified |
| [#19](https://github.com/dougborg/AirHound/issues/19) | feat: BLE duty cycling and batched notifications for power and throughput | Active continuous BLE; batching/duty-cycle evaluation remains |
| [#30](https://github.com/dougborg/AirHound/issues/30) | lib: channel strategy module (channel.rs) | Basic hopping exists; measured/adaptive/shared scheduling remains |
| [#18](https://github.com/dougborg/AirHound/issues/18) | arch: compare shipped no_std and ESP-IDF firmware and decide support policy | Both runtimes build; support-policy decision needs live evidence |
| [#13](https://github.com/dougborg/AirHound/issues/13) | feat: Linux standalone daemon for direct WiFi/BLE capture | Runtime merged; deployment/service extensions remain |
| [#107](https://github.com/dougborg/AirHound/issues/107) | hostd: TCP/WebSocket listener for companion protocol | Planned; local transport is not a network listener |
| [#108](https://github.com/dougborg/AirHound/issues/108) | hostd: JSON file logging for match history | Planned rotated history |
| [#109](https://github.com/dougborg/AirHound/issues/109) | hostd: desktop notifications on match | Planned; depends on confidence/rate-limit policy |

### Later — shared detection and platform architecture

| Issue | Work | Status / scope |
| --- | --- | --- |
| [#17](https://github.com/dougborg/AirHound/issues/17) | arch: multi-platform architecture — shared core, multiple runtimes | Architecture umbrella; not a second implementation |
| [#33](https://github.com/dougborg/AirHound/issues/33) | arch: extract airhound-core library into Cargo workspace | Open backlog; see issue acceptance criteria |
| [#90](https://github.com/dougborg/AirHound/issues/90) | Create airhound-handlers crate (DroneProcessor + ProximityTracker + output pipeline) | Open backlog; see issue acceptance criteria |
| [#9](https://github.com/dougborg/AirHound/issues/9) | spec: NDJSON wire protocol v1 schema and v2 roadmap | v1 schemas and outcomes exist; compatibility/v2 planning remains |
| [#11](https://github.com/dougborg/AirHound/issues/11) | feat: define a portable signature format for cross-tool interoperability | Schema exists; cross-tool/default alignment remains |
| [#14](https://github.com/dougborg/AirHound/issues/14) | feat: runtime signature loading and update protocol | Open backlog; see issue acceptance criteria |
| [#16](https://github.com/dougborg/AirHound/issues/16) | feat: interoperability with OUI-Spy, FlockSquawk, Flock-You, and other projects | Open backlog; see issue acceptance criteria |
| [#28](https://github.com/dougborg/AirHound/issues/28) | lib: GPS data types module (gps.rs) | Open backlog; see issue acceptance criteria |
| [#29](https://github.com/dougborg/AirHound/issues/29) | lib: device tracker module (tracker.rs) | Bounded address counter merged; history/expiry/address types remain |
| [#31](https://github.com/dougborg/AirHound/issues/31) | lib: extract and extend the existing hostd WiGLE writer | Hostd WiGLE writer exists; extraction/additional exports remain |
| [#32](https://github.com/dougborg/AirHound/issues/32) | lib: WIDS rules module (wids.rs) | Open backlog; see issue acceptance criteria |
| [#38](https://github.com/dougborg/AirHound/issues/38) | feat: extract frame subtype, BSSID, and IEs from WiFi frames | Open backlog; see issue acceptance criteria |
| [#39](https://github.com/dougborg/AirHound/issues/39) | feat: add WiFi frame-content signature types to signature schema | Open backlog; see issue acceptance criteria |
| [#49](https://github.com/dougborg/AirHound/issues/49) | feat: device presence watchlist with known MAC alerting | Firmware watchlist exists; cross-runtime effects/alerts remain |
| [#50](https://github.com/dougborg/AirHound/issues/50) | feat: overpowered signal and proximity anomaly alerting | Open backlog; see issue acceptance criteria |
| [#76](https://github.com/dougborg/AirHound/issues/76) | Implement DroneProcessor (ODID state accumulation) | Open backlog; see issue acceptance criteria |
| [#77](https://github.com/dougborg/AirHound/issues/77) | Implement ProximityTracker (multi-target RSSI tracking) | Open backlog; see issue acceptance criteria |
| [#79](https://github.com/dougborg/AirHound/issues/79) | Add GPS integration module | Open backlog; see issue acceptance criteria |
| [#85](https://github.com/dougborg/AirHound/issues/85) | Wire output pipeline with category-based routing | Partial routing exists; category controls/dedup pipeline incomplete |
| [#99](https://github.com/dougborg/AirHound/issues/99) | feat: OUI manufacturer name lookup for device messages | Open backlog; see issue acceptance criteria |

### P1 — durable desktop sessions and recording

The first five implementation issues below are promoted; embedded storage and
advanced retention remain separately scoped.

| Issue | Work | Status / scope |
| --- | --- | --- |
| [#54](https://github.com/dougborg/AirHound/issues/54) | Storage contracts and semantic fixtures | P1; preserve current evidence, identity, timestamps and binary fields |
| [#55](https://github.com/dougborg/AirHound/issues/55) | Detection evidence and queryable history | P1 with SQLite; preserve historical rule/policy results |
| [#56](https://github.com/dougborg/AirHound/issues/56) | Durable backend-owned run lifecycle | P1; pause retains identity, Finish commits, restart marks interrupted |
| [#57](https://github.com/dougborg/AirHound/issues/57) | SQLite recording for the shared Rust backend | P1 on macOS/Linux; schema, writer, migrations and bounded history |
| [#58](https://github.com/dougborg/AirHound/issues/58) | docs: storage schema guidance for companion app developers | Open backlog; see issue acceptance criteria |
| [#59](https://github.com/dougborg/AirHound/issues/59) | Explicit retention and storage limits | Basic failure/limit semantics P1; advanced purge/aggregation later |
| [#60](https://github.com/dougborg/AirHound/issues/60) | feat: ESP-IDF sensor storage — observation buffer, match log, and watchlist | Open backlog; see issue acceptance criteria |
| [#80](https://github.com/dougborg/AirHound/issues/80) | Add NVS persistence layer | Open backlog; see issue acceptance criteria |
| [#100](https://github.com/dougborg/AirHound/issues/100) | feat: double-buffer write pattern for local storage | Open backlog; see issue acceptance criteria |
| [#101](https://github.com/dougborg/AirHound/issues/101) | feat: SD card firmware update (OTA from removable media) | Open backlog; see issue acceptance criteria |

### Later — additional interfaces, hardware and project identity

| Issue | Work | Status / scope |
| --- | --- | --- |
| [#12](https://github.com/dougborg/AirHound/issues/12) | feat: Kismet companion process for surveillance detection | Open backlog; see issue acceptance criteria |
| [#15](https://github.com/dougborg/AirHound/issues/15) | feat: GPS integration and multi-sensor direction finding | Open backlog; see issue acceptance criteria |
| [#34](https://github.com/dougborg/AirHound/issues/34) | feat: Flipper Zero companion app for AirHound ESP32 sensors | Open backlog; see issue acceptance criteria |
| [#81](https://github.com/dougborg/AirHound/issues/81) | Create airhound-web crate with HTTP server | Open backlog; see issue acceptance criteria |
| [#82](https://github.com/dougborg/AirHound/issues/82) | Build web dashboard (Preact SPA) | Open backlog; see issue acceptance criteria |
| [#83](https://github.com/dougborg/AirHound/issues/83) | Add WebSocket event streaming | Open backlog; see issue acceptance criteria |
| [#84](https://github.com/dougborg/AirHound/issues/84) | Add 4 operating modes with NVS persistence | Open backlog; see issue acceptance criteria |
| [#86](https://github.com/dougborg/AirHound/issues/86) | Add WiFi AP + STA mode support | Open backlog; see issue acceptance criteria |
| [#87](https://github.com/dougborg/AirHound/issues/87) | Add NeoPixel LED driver | Open backlog; see issue acceptance criteria |
| [#88](https://github.com/dougborg/AirHound/issues/88) | Add scan-mode reboot for WiFi ODID | Reassess against current coexistence before adding reboot mode |
| [#89](https://github.com/dougborg/AirHound/issues/89) | Archive OUI-Spy C++ codebase | Maintenance/archival decision; do not archive automatically |
| [#97](https://github.com/dougborg/AirHound/issues/97) | feat: ESP32-C5 hardware target for dual-band (2.4 + 5 GHz) WiFi scanning | Open backlog; see issue acceptance criteria |
| [#98](https://github.com/dougborg/AirHound/issues/98) | feat: multi-sensor aggregation in companion protocol | Open backlog; see issue acceptance criteria |
| [#130](https://github.com/dougborg/AirHound/issues/130) | Create a logo for AirHound | Open backlog; see issue acceptance criteria |

### Research / deferred — additional radios and analysis

| Issue | Work | Status / scope |
| --- | --- | --- |
| [#40](https://github.com/dougborg/AirHound/issues/40) | feat: drone/UAV detection via DJI DroneID and WiFi fingerprinting | Open backlog; see issue acceptance criteria |
| [#41](https://github.com/dougborg/AirHound/issues/41) | feat: ADS-B aircraft surveillance detection | Open backlog; see issue acceptance criteria |
| [#42](https://github.com/dougborg/AirHound/issues/42) | feat: ISM band (433/915 MHz) surveillance device detection via rtl_433 | Open backlog; see issue acceptance criteria |
| [#43](https://github.com/dougborg/AirHound/issues/43) | feat: wireless HID (MouseJack/nRF24) device detection | Open backlog; see issue acceptance criteria |
| [#44](https://github.com/dougborg/AirHound/issues/44) | feat: Zigbee/802.15.4 device detection | Open backlog; see issue acceptance criteria |
| [#45](https://github.com/dougborg/AirHound/issues/45) | feat: Z-Wave device detection | Open backlog; see issue acceptance criteria |
| [#46](https://github.com/dougborg/AirHound/issues/46) | feat: MAC randomization defeat via device correlation | Open backlog; see issue acceptance criteria |
| [#47](https://github.com/dougborg/AirHound/issues/47) | feat: client probe request profiling for device fingerprinting | Open backlog; see issue acceptance criteria |
| [#48](https://github.com/dougborg/AirHound/issues/48) | feat: PCAP evidence capture for detected surveillance devices | Open backlog; see issue acceptance criteria |
| [#51](https://github.com/dougborg/AirHound/issues/51) | feat: DHCP hostname and OS fingerprinting from network traffic | Open backlog; see issue acceptance criteria |
| [#52](https://github.com/dougborg/AirHound/issues/52) | feat: WPA handshake and PMKID capture for security analysis | Open backlog; see issue acceptance criteria |
| [#53](https://github.com/dougborg/AirHound/issues/53) | feat: KismetDB historical query integration for pattern analysis | Open backlog; see issue acceptance criteria |

## Open pull-request queue

- #150/#151/#162 are merged; live/production acceptance remains #159/#155–#157.
- #149: btleplug update rebased locally; default/public-only host tests and strict Clippy pass. A 45-second live run acknowledged commands but captured zero observations and cancelled a stalled scanner at shutdown (#159); final remote checks remain.
- #161: grouped dependency update (supersedes #148); rebase and test.
- #163: receiver SHA-2 dependency update; digest-format compatibility fixed, 75 local tests/Clippy and clean-source candidate packaging pass; final CI pending.
- #136–#140: release/upload/checkout/install/simulator Actions updates; review compatibility and final workflows.
- #123: release automation PR; gated by evidence, assets and the documented support matrix.

## Local evidence to retain through integration

- `docs/m5stickc-bringup.md`: backup/flash provenance, WiFi correction, unique counters, failed stack trial and successful PSRAM validation.
- `docs/receiver-release-gates.md`, `drivers/mt7610u/TESTING.md`, `docs/mt7610u-channels.md`: receiver acceptance procedures and limits.
- `hostd/README.md`: permission bundles, public/private address builds, synthetic benchmarks and live profiler.
- Ignored local `target/hardware/` and driver validation directories: exact manifests and private raw evidence; do not publish these wholesale.

Update this index when issues close or integration changes status; do not mark locally tested work as released.
