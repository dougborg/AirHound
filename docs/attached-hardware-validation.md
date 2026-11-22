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

The later 20-second ALFA metadata test used host source `8180cabc` and helper
source `f292753` (the difference between those source snapshots is documentation).
The tested helper reported package 0.1.0 and clean source revision `f292753`;
its executable SHA-256 was
`84a8d071145d46c8d5a3ad13a45a3cfd150d3b9e67ef3a9bd78ba8e4ce1e8561`.
The supplied firmware hash remained the pinned value above. That run reused an
already running MCU, so its firmware load state was `running_image_unverified`:
the supplied file's header/hash is not proof of the running MCU image.
The host executable hash was
`dd5dfd6f601bec4d9d79ea1cc7fcfb67a7a097208a5f6cf8c35e6b9c1ea6b1f0`.

Managed GPS acceptance used the session-owned, foreground `gpsd -N -b -n`
lifecycle: this is a read-only host daemon, not firmware flashed onto the GPS.
The earlier individual RF tests predate shared-serial-lease hardening. The later
external combined acceptance below used clean assembled source `d7be542`, backend
SHA-256 `97e144c1d98f84dda6cfc1913d6fc63193327f860c0cff80a4772ec1aac507bf`,
and immutable helper bundle
`63df7145713d571b77cd8e6de60f5b5124ae452eb3829f766b26ae40e2524189`.
The bundle retains the ALFA executable above and the shared-lease Sniffle helper.
This was a private external-only service, not the installed GUI or native radios.

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
| ALFA AWUS036ACHM | MT7610U `0e8d:7610`; explicit USB topology selector; ASIC `0x76100002` | First capture hit MCU command 12 timeout. After physical reconnect: 10 seconds on channel 1, 740 beacons, 1,600 frames/1,607 transfers, 7 malformed, cleanup verified. Later 20-second metadata run retained 1,259 observations, all attributed to its configured source; clean finish and zero reported host/producer drops or reconnects | Actual helper build reported; supplied image retained separately. Five running statuses correctly said running image unverified because the MCU was reused; stopped status cleared firmware. Short success is not an endurance claim; signal calibration remains limited |
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
| CatSniffer + managed GPSD, 20 seconds | 1,356 BLE packets; 1,346 had fresh receiver/rig location, ten startup packets correctly unlocated; finished | Setup save/load and owned daemon startup/finish passed. gpsd 3.27.5/protocol 16.1 and Sniffle 1.11.0/API 0 reported; daemon/private service exited and endpoints released. Native form remains to be checked in the actual final GUI |
| Assembled M5 + CatSniffer + ALFA + managed GPS, 65.72 seconds | 6,191 observations: M5 467 WiFi + 26 BLE, Cat 2,693, ALFA 3,005; 6,062 fresh receiver-location associations | Pause/resume, M5 connection/boot continuity, one owned Cat helper failure and bounded recovery, and clean finish passed. All configured ALFA channels 1/6/11 received observations |
| Same saved setup after backend restart, 30.09 seconds | 3,475 observations: M5 263 WiFi + 15 BLE, Cat 1,646, ALFA 1,551; 3,326 fresh receiver-location associations | Saved plan reloaded unchanged, actual producer identities verified again, all three ALFA channels received observations, clean finish |
| Native BLE + CatSniffer, 45 seconds | 2,536 native BLE  + 2,961 Cat observations; 5,513 total records, finished | Concurrent BLE sources, attribution and orderly finish worked; Cat host drops/reconnects 0 |
| Native WiFi + native BLE + CatSniffer + ALFA, 45 seconds, separately permitted backend app | 987 native WiFi  + 3,193 Cat  + 2,497 ALFA observations; 6,706 total records; **failed** terminal state | The three receiving sources worked concurrently, but native BLE initialization timed out. Do not call this four-receiver acceptance |

ALFA's combined assignment was channels 1/6/11 with 500 ms dwell. Its reported
producer drops, host drops and reconnects were 0; Cat host drops/reconnects were 0.
Those counters do not measure all missed RF traffic. Original source IDs and
per-source ingress order are retained in each durable observation.

The two assembled external runs retained 9,666 observations in total. Every
observation had the expected configured receiver attribution; receiver-location
associations had zero invalid source/device/freshness relationships. SQLite
integrity checks passed. Startup observations without a fresh GPS fix remained
unlocated instead of inheriting a guessed location.

During the eight-second pause, no observations were stored after the two-second
drain guard. M5 retained its connection and boot generation and counted 70
intentionally discarded host-ingestion records. The device's radios continued.
The deliberately terminated Cat helper recovered through one supervised reconnect;
the other sources continued. Host-drop counters were zero for all three receivers;
ALFA's reported producer-drop counter was zero. M5 reported 40 cumulative scan
drops, unchanged across its three sampled producer reports; output drops and
serialization errors were zero. This does not establish zero RF loss or exclude
loss before the first producer report.

Both recordings finished, both private service processes exited zero without
forced cleanup, the GPS/helper children were gone, and all selected serial ports
were released. Actual identities were AirHound 0.1.1 on M5StickC Plus2,
Sniffle 1.11.0/API 0, ALFA helper `f292753`, and the reported gpsd host software.
The ALFA supplied-versus-running-image distinction was checked in both runs.

A preliminary 36-second attempt stopped on an overly strict harness expectation:
ALFA uses the healthy availability value `running`, while BLE/serial use
`available`. It finished and cleaned up normally. That failed-harness result is
retained separately and excluded from the accepted totals above; the corrected
check still requires every enabled scanner phase to be running.

The CoreBluetooth failure and CoreWLAN redaction occurred under different
permission identities. The user's permission panel showed the standalone test
backend enabled and three AirHound entries disabled; subsequent GUI permission
work must be evaluated using the actual running binary, not inferred from that
first matrix. Keep the active user's recording intact while validating fixes.

## Adversarial review and fault evidence

These are isolated software/PTY checks, not additional RF reception claims. No
physical receiver, installed preview or default recording was altered for them.

| Area | Reviewed source / evidence | Result |
| --- | --- | --- |
| GPS session rebase | #276 `0896c50`; range-diff against previously reviewed `1275063` | All three feature patches unchanged after the second rebase. The earlier additive conflict correctly retained both backend build metadata and location-provider capabilities |
| Managed helper installation | #280 `08f5a67` | Independent installer replay rejects unlisted Python files, symlinks, FIFOs and reuse of altered bundles. Clean bundle verifies. Dangling current pointer requires explicit replacement. Source review confirms manifest directory sync before atomic publication |
| Managed GPS daemon ownership | `ea145e9` | Original fake-child test reproduced adoption of an unrelated replacement listener. Fixed service sends no WATCH to it, clears daemon identity/fix and reports endpoint busy; clean finish |
| Shared serial ownership | #281 `77d01e3`, bounded test harness `46ec9b6` | One canonical endpoint/cu–tty alias namespace is shared by Rust and Python. Lease precedes upstream Sniffle construction, M5 UART open and managed GPS child spawn; stale lock files are retained safely rather than unlinked |
| M5 ownership wiring | `98b56fb`, on M5 source `f61f8a1` | Actual private M5 source is denied by a Sniffle-held PTY lease before UART open; original Sniffle owner remains alive. Six reader regressions include pre-open denial and already-paused reconnect health. The macOS PTY does not support this driver's baud ioctl; the subsequent assembled physical-device runs above verify successful M5 reception, pause continuity and restart under the new guard |
| GPS/Sniffle ownership wiring | `e8a972f` | Actual managed service tested both orders on a PTY. Sniffle-first prevents GPS child spawn. GPS-first denies Sniffle before its constructor writes, including a symlink alias. Finish reaps the GPS child, releases ownership and permits a subsequent identification query |
| Native setup actions and identity | #277 `480449a` | Save/Delete intent survives background polling and retains submitted plan/ID; real private-backend persistence regression passes. Source identity must match the stored active run as well as receiver ID; disconnected cached identity is hidden |
| GPS status presentation | `8f52ff8` | Disconnected cached status is explicitly historical and says current fix unknown; it cannot continue claiming a current fix |

The shared lease coordinates updated AirHound sources for the same user. It does
not retroactively change old helper binaries or establish ownership over unrelated
software. Install the reviewed helper revision when testing the assembled build.
OS serial exclusivity remains an additional transport safeguard, not the shared
ownership contract by itself.

## Remaining acceptance

- Re-establish the default GUI service connection through the actual Start Local
  Service control, then validate a combined session under that same authorized
  identity, including M5 and explicit GPS where compatible. At this checkpoint the
  default control socket is absent and the requested user click is still pending;
  private service tests do not establish that GUI path.
- Extend the passed external combined/restart/helper-recovery checks to native
  radios under the authorized app identity. Physical unplug/replug, sleep/wake
  and prolonged load remain untested in this assembled build. Synthetic
  malformed framing, queue overflow, EOF, boot-boundary and pause tests already
  cover the M5 reader; they do not replace hardware fault injection.
- Retry Flipper after the requested USB reconnect, then exercise the existing
  passive AWOK control path and import a genuine resulting capture. No fresh
  Flipper/AWOK live acceptance is claimed while that user reconnect is pending.
- Exercise saved GPS/provider setup through the native UI. Backend session
  configuration and provenance are implemented and live-tested; location must
  remain an explicit association and is not evidence of a transmitter location.
- Keep Nordic/CSR marked recognized but unsupported until implemented and tested.
  Their recognition must not enable a capture toggle or invent firmware versions.

History-only connection to an absent recording store is tracked separately in
[#283](https://github.com/dougborg/AirHound/issues/283). Capture-enabled startup
initializes the store; the follow-up must preserve read-only history semantics and
not hide corruption or permission errors.

See [serial source semantics](airhound-serial-source.md),
[Flipper/AWOK setup](flipper-marauder.md), [GNSS setup](gnss-wardriving.md), and
[receiver release gates](receiver-release-gates.md) for repeatable procedures.
