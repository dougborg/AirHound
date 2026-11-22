# Attached hardware validation — 2026-11-22

This is bounded acceptance evidence for the local macOS setup, not a release-wide
hardware certification. Supported sources still need permissions and correct
firmware. USB product names alone establish neither firmware nor working reception.
The remaining integration work is tracked in [#269](https://github.com/dougborg/AirHound/issues/269).

## Provenance and privacy

Baseline desktop code: `67eab20`; executable was the embedded helper in the local
macOS app build. M5 ingestion used the accompanying `airhound_serial` change,
built with stable Rust. ALFA testing used the multi-adapter `mt7610u-lab` build
with canonical selector support and firmware SHA-256
`538d08c08f8fb5aa05cacf34e49aeb307e8b2dc094407fc655fbf3580c893953`.
The repository's older local ALFA executable lacked those commands and was not
used. CatSniffer used the documented Python Sniffle helper and Sniffle module.
The later GNSS/session integration tests used production source `018eea9` from
[#276](https://github.com/dougborg/AirHound/pull/276); private summaries record the
binary SHA-256, service configuration and database checks.

Raw observations, SQLite recordings, exact local endpoints, test plans, tool logs
and permission screenshots remain private. This report contains aggregate counts
and owned-device identities only. An example USB/serial path is not a portable
configuration; rediscover the current endpoint and select it explicitly.

## Individual devices

| Device / role | Observed identity and endpoint | Current evidence | Limit / next check |
| --- | --- | --- | --- |
| Native Bluetooth | CoreBluetooth, OS-managed | 30-second run: 1,816 observations, clean finish | OS determines adapter/PHY; USB Bluetooth dongles do not become separate CoreBluetooth receivers |
| Native WiFi | CoreWLAN, OS interface | CLI helper initially returned redacted addresses; a separately permitted backend app received 1,172 observations | Actual Location permission differs between app identities. Success is not proof that LaunchServices itself is the required fix. Confirm the running GUI/backend identity has permission |
| CatSniffer 3.2 | RP2040 `2e8a:00c0`, USB CDC; reported Sniffle 1.11.0/API 0 | 30-second run: 2,563 packet observations; clean finish, zero reported host drops/reconnects | Channel 37/1M test; no new Coded/extended acceptance claim. A later explicit identity probe and reuse of active-owner metadata also passed |
| M5StickC Plus2 | WCH `1a86:55d4`, UART at 460800 baud; actual AirHound 0.1.1 status | 66-second durable serial run: 407 WiFi  + 71 BLE observations, 512 total stored records; pause/resume/finish passed | Eight-second host pause discarded 47 observations and retained port/boot; zero observations recorded during pause. All host errors/drop counters and reported producer counters were 0. Autonomous radios continue while host ingestion is paused/stopped |
| ALFA AWUS036ACHM | MT7610U `0e8d:7610`; explicit USB topology selector; ASIC `0x76100002` | First capture hit MCU command 12 timeout. After physical reconnect: 10 seconds on channel 1, 740 beacons, 1,600 frames/1,607 transfers, 7 malformed, cleanup verified | Recovery required physical reconnect in this test. Short success is not an endurance claim; signal calibration remains limited |
| u-blox GNSS | `1546:01a8`, USB CDC; gpsd NMEA0183/9600 | 60-second CatSniffer session: all 4,520 BLE packets carried durable GPS associations; fix age 16–1,020ms, host age 0–1,004ms | Explicit GPSD session configuration implemented and live-tested in #276. gpsd 3.27.5/protocol 16.1 is host software, not receiver firmware. Receiver model/firmware remains unverified |
| Flipper Zero | STM32 `0483:5740`, USB CDC; Momentum `mntm-012` banner (`e1784e74`, built 2025-12-31) | Current USB connection returned firmware banner | Subsequent `device_info` commands did not respond, including after the user confirmed unlocked/main screen and AWOK seated. Waiting for requested USB reconnect before retry; older API 87.1 evidence is historical |
| AWOK Dual Touch v3 | No directly attached CP2102 endpoint in this inventory; possible Flipper GPIO route | Prior-day official Marauder 1.21.0 and bounded GPIO test are documented separately | User confirmed board attached via GPIO; current USB CLI still fails to respond, so current UART control/reception remains unverified. Existing offline importer is implemented; fresh real-capture acceptance and live host bridge remain gaps |
| Nordic BLE sniffer | `1915:522a`, USB CDC, nRF Sniffer product descriptor | Recognized candidate only | No current AirHound live-source integration; product descriptor is not a queried firmware version |
| CSR-compatible Bluetooth dongle | `0a12:0001`, CSR8510A10 product descriptor | Recognized prototype/research target only | No verified macOS live USB-HCI path; product name does not establish genuine silicon or capabilities |

The M5's internal free heap was about 3.6 KiB; its prior startup diagnostic confirmed
2 MiB PSRAM with about 2 MiB free external memory. Neither value describes total free
memory by itself. The serial test's reported uptime progressed 61→91→121 seconds
through pause/resume, with one connection and one boot generation. Earlier Python
monitor opens reset the device despite DTR/RTS being held false; do not promise
that opening the port is nondisruptive.

## Combined sessions

| Configuration | Result | Interpretation |
| --- | --- | --- |
| CatSniffer + GPSD, 60 seconds | 4,520 BLE packets, all 4,520 geotagged, finished; host drops/reconnects 0 | Explicit receiver-location association persisted; Sniffle 1.11.0/API 0 and gpsd 3.27.5/API 16.1 reported by their actual producers |
| CatSniffer + GPSD provider loss, 30 seconds | 2,441 BLE packets, 842 geotagged; 1,476 packets retained after the one-second disconnect guard, none geotagged | Health changed fix → disconnected and daemon identity cleared; no stale coordinate association after the guard, no invalid associations, clean finish |
| Native BLE + CatSniffer, 45 seconds | 2,536 native BLE  + 2,961 Cat observations; 5,513 total records, finished | Concurrent BLE sources, attribution and orderly finish worked; Cat host drops/reconnects 0 |
| Native WiFi + native BLE + CatSniffer + ALFA, 45 seconds, separately permitted backend app | 987 native WiFi  + 3,193 Cat  + 2,497 ALFA observations; 6,706 total records; **failed** terminal state | The three receiving sources worked concurrently, but native BLE initialization timed out. Do not call this four-receiver acceptance |

ALFA's combined assignment was channels 1/6/11 with 500 ms dwell. Its reported
producer drops, host drops and reconnects were 0; Cat host drops/reconnects were 0.
Those counters do not measure all missed RF traffic. Original source IDs and
per-source ingress order are retained in each durable observation.

The CoreBluetooth failure and CoreWLAN redaction occurred under different
permission identities. The user's permission panel showed the standalone test
backend enabled and three AirHound entries disabled; subsequent GUI permission
work must be evaluated using the actual running binary, not inferred from that
first matrix. Keep the active user's recording intact while validating fixes.

## Remaining acceptance

- Complete current GUI/backend permission validation and a combined session under
  that same identity, including the new M5 source where compatible.
- Test physical disconnect/reconnect, helper failure and prolonged load. Synthetic
  malformed framing, queue overflow, EOF, boot-boundary and pause tests already
  cover the M5 reader; they do not replace hardware fault injection.
- Retry Flipper after the requested USB reconnect, then exercise the existing
  passive AWOK control path and import a genuine resulting capture.
- Exercise saved GPS/provider setup through the native UI. Backend session
  configuration and provenance are implemented and live-tested; location must
  remain an explicit association and is not evidence of a transmitter location.
- Keep Nordic/CSR marked recognized but unsupported until implemented and tested.
  Their recognition must not enable a capture toggle or invent firmware versions.

See [serial source semantics](airhound-serial-source.md),
[Flipper/AWOK setup](flipper-marauder.md), [GNSS setup](gnss-wardriving.md), and
[receiver release gates](receiver-release-gates.md) for repeatable procedures.
