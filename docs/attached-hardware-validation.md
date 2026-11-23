# Attached hardware validation — 2026-11-22

This is bounded acceptance evidence for the local macOS setup, not a release-wide
hardware certification. Supported sources still need permissions and correct
firmware. USB product names alone establish neither firmware nor working reception.
The remaining integration work is tracked in [#269](https://github.com/dougborg/AirHound/issues/269).

## Current acceptance status

The prior installed `33a5e93` backend received native WiFi and Bluetooth after the
user approved permissions: the unchanged service finished a 20.196328-second run
with 483 WiFi and 1,233 BLE observations. Existing recordings and saved setups
were preserved. This verifies backend-controlled reception, not native GUI
Start/Finish controls.

The completion-capable Flipper producer `c7e72ae` passed a private normal
Pause/Resume/Finish run with host `306d24a` (129 packets), then a paused-host
watchdog test with corrected host `58239fa`. The latter correctly left the
recording failed while retaining backend-validated stop completion. A subsequent
CatSniffer/ALFA/Flipper session with managed GPS finished with 3,106 observations
and 2,957 fresh receiver-location associations. Its original harness rejected a
valid Cat packet classification; independent saved-evidence revalidation passed
without recapturing. Earlier acquisition, drain and watchdog failures remain below.
The Flipper UI and live-source builds are now delivered in the installed `aa472a0`
preview described below. The earlier native reception result does not establish
permission or reception acceptance for this newly signed application.

The M5 remains absent from the latest passive inventory. Physical CatSniffer
unplug/replug, active-capture transport loss, actual GUI capture controls and
longer-duration recovery checks remain open. The matrices below
retain earlier successes and failures with their build and test scope; they do
not imply that every tested device is currently attached or enabled.

## Current preview delivery

The reviewed desktop preview is built from clean merged source `aa472a0` and
includes the host integration in
[#299](https://github.com/dougborg/AirHound/pull/299), compatible with the separately
installed companion from [#298](https://github.com/dougborg/AirHound/pull/298).
The acceptance ledger in [#305](https://github.com/dougborg/AirHound/pull/305) is
also merged. [#306](https://github.com/dougborg/AirHound/pull/306) subsequently added
private backend/Swift setup tests: at delivery, main `308596d` differed from the
preview source only by 185 lines in that test file. No production rebuild was
needed for that test-only change.

The preview rollout preserved existing recordings, saved setups and settings.
Reviewed checks covered signed application startup, saved-data compatibility and
restoration preparation. These checks establish delivery and preservation, not
new hardware reception or a live downgrade. The rollout did not flash device
firmware or start capture.

Actual GUI Validate/Start/Pause/Resume/Finish controls, observations presentation
and native permissions under the new signed identity remain separate acceptance
checks. Earlier `33a5e93` native reception and private Flipper/combined results
below retain their original artifact scope.

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

| Device / role | Observed identity and endpoint | Recorded evidence | Limit / next check |
| --- | --- | --- | --- |
| Native Bluetooth | CoreBluetooth, OS-managed | Prior `33a5e93` installed-backend permission retry: 1,233 observations alongside WiFi, clean finish | OS determines adapter/PHY; USB Bluetooth dongles do not become separate CoreBluetooth receivers |
| Native WiFi | CoreWLAN, OS interface | Prior unchanged `33a5e93` installed backend received 483 observations alongside BLE after user permission approval | Earlier redacted-address attempts remain failures. Permission is specific to the running app/backend identity; reception is not proof of GUI capture-control acceptance |
| CatSniffer 3.2 | RP2040 `2e8a:00c0`, USB CDC; reported Sniffle 1.11.0/API 0 | 30-second run: 2,563 packet observations; clean finish, zero reported host drops/reconnects | Channel 37/1M test; no new Coded/extended acceptance claim. A later explicit identity probe and reuse of active-owner metadata also passed |
| M5StickC Plus2 | WCH `1a86:55d4`, UART at 460800 baud; actual AirHound 0.1.1 status | Earlier 66-second durable serial run: 407 WiFi + 71 BLE observations, 512 total stored records; pause/resume/finish passed | Currently absent; reconnect and revalidate. The earlier eight-second host pause discarded 47 observations and retained port/boot; zero observations recorded during pause. All host errors/drop counters and reported producer counters were 0. Autonomous radios continue while host ingestion is paused/stopped |
| ALFA AWUS036ACHM | MT7610U `0e8d:7610`; explicit USB topology selector; ASIC `0x76100002` | First capture hit MCU command 12 timeout. After physical reconnect: 10 seconds on channel 1, 740 beacons, 1,600 frames/1,607 transfers, 7 malformed, cleanup verified. Later 20-second metadata run retained 1,259 observations, all attributed to its configured source; clean finish and zero reported host/producer drops or reconnects | Actual helper build reported; supplied image retained separately. Five running statuses correctly said running image unverified because the MCU was reused; stopped status cleared firmware. Short success is not an endurance claim; signal calibration remains limited |
| u-blox GNSS | `1546:01a8`, USB CDC; gpsd NMEA0183/9600 | 60-second CatSniffer session: all 4,520 BLE packets carried durable GPS associations; fix age 16–1,020ms, host age 0–1,004ms | Explicit GPSD session configuration implemented and live-tested in #276. gpsd 3.27.5/protocol 16.1 is host software, not receiver firmware. Receiver model/firmware remains unverified |
| Flipper Zero | STM32 `0483:5740`, USB CDC; Momentum `mntm-012`, commit `e1784e74`, API 87.1 | Completion-capable producer passed UART-free startup, normal Pause/Resume, one paused-host watchdog case, and concurrent Cat/ALFA/GPS reception | FAP build identity is separate from Flipper firmware and AWOK identity. Active transport loss, physical reconnect, endurance and actual GUI controls remain open. Earlier failures remain scoped to their artifacts |
| AWOK Dual Touch v3 | No directly attached CP2102 endpoint; Flipper GPIO USART route | Independent GPIO `info` reported Marauder 1.21.0/Dev Board Pro; diagnostic import retained 85 packets. Latest external combined run retained 728 live packets across two Flipper capture generations | Prior 63- and 65-packet attempts failed before Resume. The bridge Hello does not establish downstream firmware, assigned channel, RSSI or RF loss; literal stop acknowledgments do not prove physical RF silence |
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
| Actual GUI-launched service, bounded native WiFi + BLE attempt | Zero observations; **failed** terminal state | GUI Start Local Service launch and authenticated connection were verified. CoreWLAN reported redacted addresses; CoreBluetooth initialization timed out. The test recording was stopped, the service stayed running, and existing recordings/profiles were preserved |
| Same installed service after permission approval, native WiFi + BLE | 944 WiFi + 1,962 BLE observations; finished | Original harness rejected valid native metadata shapes; source-specific offline revalidation passed without rescanning |
| Same installed service, native WiFi + BLE + M5 + CatSniffer + ALFA + managed GPS, 45.27 seconds | 9,857 observations; 9,655 fresh receiver-location associations; finished | All five sources received; ALFA channels 1/6/11; zero invalid source/location associations. Backend-controlled acceptance; existing recordings/profiles preserved, external ports released, service remained running |
| Updated installed `33a5e93` service after permission approval, native WiFi + BLE, 20.196328 seconds | 483 WiFi + 1,233 BLE observations; 1,721 total records; finished | Both native scanners running, zero invalid source associations; nine prior recordings and saved setups preserved. Same signed backend and service throughout; no GUI capture-control claim |
| Private Flipper `c7e72ae` / host `306d24a`, normal lifecycle, 8.984887 seconds | 129 packets: 65 in generation 1, 64 in generation 2; 377 records; finished | Quiet Pause for 1.207 seconds, explicit Resume, fresh Finish evidence 15 → 3 → 15; cleanup and default-service preservation passed |
| Same Flipper / host `58239fa`, paused-host watchdog | 67 packets, 212 records; recording correctly **failed**; fault acceptance passed | Exact private host suspended for 6.559710 seconds after quiet Pause. No later packets; final cause 3, evidence 15 and backend-derived completion true, zero known loss/unread input. This covers paused-host heartbeat expiry only |
| Private CatSniffer + ALFA + Flipper + managed GPS, host `58239fa`, 27.544530 seconds | 3,106 observations: Cat 945, ALFA 1,433, Flipper 728; 4,183 records; 2,957 fresh receiver-location associations; finished | All sources quiet during Pause and received after Resume; ALFA 1/6/11; Flipper generations 370 + 358 with fresh Finish. Original harness false result retained; corrected offline validation and all-record shared-client pagination passed without recapture |
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
| GPS session rebase | #276 `e7cd268`, independently reviewed after merged M5 source | GPS feature/docs remain patch-equivalent. The final integration removes one duplicate CoreLocation initializer from a clean Git merge; only native Wi-Fi requests Location. Build and GPS capability fields are retained; all test targets compile |
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

## Installed GUI service launch

The reviewed `d7be542` preview was installed at `~/Applications/AirHound.app`
after confirming the default service was absent. The previous authorized app was
archived for rollback; the recording database and saved profiles were preserved.
The installed backend retained SHA-256
`97e144c1d98f84dda6cfc1913d6fc63193327f860c0cff80a4772ec1aac507bf`
and service CDHash `6dda3e160b2745abfc5756d5940f32e655a9a131`.

The actual visible bottom status control opened the connection popover, and
**Start Local Service** started the installed nested `tools.airhound.service`
application. The authenticated default socket reported clean source `d7be542`,
managed helper bundle `63df7145713d571b77cd8e6de60f5b5124ae452eb3829f766b26ae40e2524189`,
and both GPS modes. The GUI displayed **Service connected**, its backend source
version and existing recording history. One restored duplicate window was closed;
one GUI window and one idle service remained.

This used bounded visual interaction after confirming the exact frontmost process
and window frame; accessibility enumeration returned no windows. No capture was
started and no operating-system permission prompt was approved. This verifies the
installed GUI launch/connection path, not native-radio permission or reception.

The subsequent bounded native attempt failed while Bluetooth and Location
authorization prompts were still outstanding. Read-only diagnostics confirmed
both prompts existed for the installed launch. A prior successful service run
reported the same `FailedUnavailable` registration state before user approval,
then changed to `AllowedAlways` without an application lifecycle change. The
current evidence did not establish an AppKit lifecycle defect. The user approved
Bluetooth at 12:08:15 and Location at 12:08:16. The same installed binary and
service then received both native protocols, followed by the successful combined
run above; no runtime lifecycle change or service restart was needed.

After that run, **Attached receiver rig** was saved as a new setup with readable
source IDs, explicit attached endpoints and the immutable helper bundle. Saving
did not replace an existing setup. Loading reproduced the exact configuration
and validated local resources without applying it or starting capture. Endpoint
identity review remains necessary after moving or replacing hardware.

The GUI-only update in [#289](https://github.com/dougborg/AirHound/pull/289)
was then installed to preserve capture actions submitted during background polling
and explain disabled Start controls. The installed backend and its recording store
were preserved. Private synthetic-backend tests covered queued controls, duplicate
Start rejection and stale Finish confirmation targets; user retry of the installed
GUI controls remains pending. These tests do not replace an actual GUI-driven
hardware recording.

## Earlier installed recovery update

[#292](https://github.com/dougborg/AirHound/pull/292) and
[#294](https://github.com/dougborg/AirHound/pull/294) are merged. That earlier installed
application combined GUI source `41f704a` (including the reviewed capture
controls) with backend source `33a5e93`. These are the actual component builds,
not a claim that an earlier artifact was rebuilt from the merge commit. The #294
merged tree differs from this composition only in a test-only GPS fixture fix.
That signed backend SHA-256 was
`20cf48cfb1930b7c3e7b889dd7d2091c257420b08f6699d9fa1d2e7bdb5e4365`;
its unsigned input was separately recorded. All 14 file-backed Mach-O sections
matched before and after signing. Strict signatures and private LaunchServices
startup checks passed independently.

The compatible immutable helper bundle is
`848dad314647df61667eff49b4e4e613f97442fcb16231a08fd8edfcc6689925`.
Its Sniffle helper advertises the supervised borrowed-lease contract. A real,
private CatSniffer-only run using these exact artifacts lasted 21.34 seconds and
persisted 1,069 channel-37 BLE packets with Sniffle 1.11.0/API 0 identity and zero
reported host drops. This used actual 921600-baud serial transport, without the
PTY baud shim used by the earlier synthetic test. Pause reaped the owned helper,
cleared its firmware identity and retained the serial lease; no observations
were stored during the settled pause. Resume used a new helper/generation and received
more packets. Finish released the port and lease, and the private service exited
zero. SQLite integrity and per-source attribution passed independent checks.
There was **no physical unplug/replug** in this test.

Installation occurred only after verifying that the default service was idle.
The complete previous signed application, helper pointer, original profiles and a
consistent recording database backup were preserved. A private restoration
rehearsal verified the old signed application and original pointer bytes; no live
downgrade was performed. The initial installer invocation refused to replace an
existing pointer; the transition stopped before launching the new service. The
explicitly reviewed helper-only replacement then activated the new pointer while
retaining both immutable bundles. No profile replacement option was used.

Independent before/after checks found exact equality across all six recording
database tables: 38,813 records, eight runs and their source/receipt/transition
rows were preserved. The original **Attached receiver rig** setup remained
unchanged. A create-only sibling, **Attached receiver rig (USB recovery)**,
changes only CatSniffer's Python, helper and module paths. Loading it validates
configuration and local resources without applying it or starting capture. The
M5 endpoint is currently missing; CatSniffer endpoint and ALFA topology checks
still require identity review. Earlier successful M5 reception does not prove
that it is attached now. The updated GUI was reopened with the service idle.

A separate, receipt-owned 20-second native WiFi/BLE check on the updated service
then produced zero observations and ended in a **failed** terminal state. The
screen was locked. OS diagnostics recorded a Bluetooth prompt for
`tools.airhound.desktop`, requested by the installed `tools.airhound.service`,
and a Location prompt for that same nested service. Location remained
`NotDetermined`; WiFi reported redacted addresses and Bluetooth initialization
failed. Only the test run was finished; the service stayed running and all prior
recordings/profiles were preserved. No privacy setting was reset and no prompt
was approved automatically.

After the user approved the outstanding permissions, the same signed binary and
service passed the bounded retry: 483 WiFi and 1,233 BLE observations over
20.196328 seconds, with 1,721 total durable records and a finished live run.
Both native scanners reported running; CoreBluetooth reported powered on. All
1,716 observations retained their exact configured source attribution. SQLite
integrity and foreign-key checks passed. Nine pre-existing run metadata sets and
saved profiles remained unchanged, and the service stayed running. This closes
native reception acceptance for this installed binary without a rebuild,
restart or privacy reset; it does not establish native GUI Start/Finish acceptance.

At the reviewed #294 head, all 19 required checks and the relevant native,
packaging, performance, website and CodeQL workflows passed. Desktop line
coverage was 84.54% on macOS and 84.38% on Linux; function coverage was 81.98% and
81.95%, respectively. Test bodies were excluded and no physical hardware was
exercised by coverage. Linux region coverage was 79.98%, so these results do not
mean every coverage metric exceeds 80%.

Physical CatSniffer reconnect remains tracked in
[#290](https://github.com/dougborg/AirHound/issues/290). The full hardware milestone
[#269](https://github.com/dougborg/AirHound/issues/269), native GUI Start/Finish
acceptance and reattached M5 verification remain open.

## Flipper and AWOK follow-up

The following sequence preserves failures from earlier acquisition paths. The
latest live-source evidence and remaining limits appear at the end of this section.

After USB reconnect, the Flipper's fresh identity and GPIO `info` exchange succeeded.
A bounded settings-only binary transfer returned 681 bytes with a matching device
hash. The following mJS passive beacon capture lost USB connectivity and produced
a zero-byte file. Its first cleanup had no acknowledgment; a later stop recovery
returned two stop acknowledgments, fresh Marauder information and a prompt. The
372-byte recovery result matched the device hash. A subsequent read-only screen
snapshot displayed **out of memory**. Pinned Momentum mJS retains `readBytes`
ArrayBuffers in an arena until interpreter destruction, so this accumulating
binary-capture approach was abandoned. The displayed reason is confirmed; it does
not provide a stack trace or establish an exact allocation that failed.

The installed third-party companion is app version 7.6 (display `v0.7.6`), API87.1,
108,256 bytes, SHA-256
`fc9f2ae606722ac63866c120fe108e62a6e1c85df0e45a1708867a591376d680`.
Its downloaded backup matches the device hash and remains preserved; save-PCAP and
save-log were enabled. Official v0.7.6 source at
`e77f2754b3ca97c14dcc27b5d8a19091b7bbfa62` was audited, but matching version does
**not** establish byte-equivalent source provenance for this installed artifact.

Screen-verified navigation reached the actual **Sniff / beacon** selection before
each of two native attempts. The first returned to the menu without rebooting but
saved a zero-byte PCAP and a 62-byte log containing the command echo and stop
acknowledgment, without the mandatory start message. Source inspection of official
Marauder 1.21.0 (`d9197cf063cede379104ca8673c2d952d66575a3`) found that `info`
leaves `SHOW_INFO` active (`CommandLine.cpp:1860`); `scanning()` treats it as active
(`WiFiScan.cpp:2780`) and the sniff-command guard then declines to start
(`CommandLine.cpp:844`). The earlier recovery ended in `info`. This is a supported
causal explanation, not direct telemetry of that attempt's internal state.

A corrected stop-only preflight ended in two observed stop acknowledgments and
**no subsequent AWOK `info`**. The second native attempt displayed received beacon
text, but the host stop path timed out and the companion disappeared. A subsequent
screen showed **NULL pointer dereference**, a different crash from the mJS OOM.
The planned three-second stop budget was not met; actual RF duration is unknown.
Both new PCAP and log files were zero bytes and matched their device hashes.
No attempt was repaired, trimmed or counted as successful capture/import.

Final recovery (`stop-v4`) ran only after fresh identity and idle-loader checks.
It sent two `stopscan` commands, retained a hash-verified 3,691-byte result with two
firmware-text stop acknowledgments, ended with `stopscan`, and closed transport.
The NULL fault screen was preserved. The native fault's precise instruction,
thread and cause remain unknown: this Momentum build prints registers to the live
log, retains the reason through reboot, and clears the register buffer at startup.
A sequential 100,000-chunk ASan/UBSan replay of the matched-source text callback
found no bounds failure; it does not emulate concurrent GUI/RX behavior or prove
that the installed companion is safe.

The subsequent isolated implementation was the [AirHound UART diagnostic](../companions/flipper/README.md#bounded-uart-diagnostic):
fixed memory, one UART command owner, device-local stop timing, immutable UI text,
and storage only after RX shutdown. It is not a fix for the unknown native fault and
does not replace or flash either firmware.
Its default launch is UART-free. The unique `de00984` build passed actual synthetic
load/selftest/exit with 53,048 bytes free heap while loaded and 127,752 after exit;
its original companion hash stayed unchanged. One capture-mode attempt then
aborted in preflight: `start_sent: false`, no scan command issued, closed empty raw
file, zero reported UART drops/errors/unread bytes. Because that build did not
retain preflight bytes, silence versus line mismatch remains unknown. A subsequent
stop-only recovery returned two stop acknowledgments in 85 hash-verified bytes,
ending in `stopscan` with idle loader and released transport. No RF retry occurred.
The `aace44e` follow-up passed UART-free synthetic load/exit (52,392 bytes free
heap loaded; 127,824 after exit). Its explicit stop-only mode cannot emit Start.
That mode timed out after 1,500 ms with 18 hash-verified bytes: five `0xf3` bytes
preceded the echoed `stopscan` on the same line, followed by a prompt, without
the required acknowledgment. It reported zero drops/errors/unread bytes and
OTG disabled; the owned dialog closed and loader returned idle. This establishes
received console data, not the cause of the prefix bytes. The following narrow change
added the same leading CRLF as the working JS stop path to terminate stale input,
while retaining exact acknowledgment matching. No baud, power, firmware, or
capture-duration change was made.

Revision `b0c26fa` (FAP SHA-256
`5bc5d4778cfaa195d27edee4b485b2eb8c8e9fc2aab0fdb466e6c1715afe4045`)
passed UART-free load/exit and the stop-only gate: 48 hash-verified bytes contained
the exact acknowledgment, UART ownership lasted 117 ms, and no scan started.
One subsequent bounded passive acquisition produced 28,990 hash-verified bytes
and 85 complete packets. Its local stop command occurred at 2,002 ms; UART
ownership lasted 2,989 ms. Preflight/start/stop text acknowledgments were observed;
all reported UART drop/error/unread and framing-error counters were zero. Files
closed, the loader returned idle, USB was released and the original companion
hash remained unchanged. No reset or crash was observed in this attempt.

Strict import accepted 85 packets without repair. Private service/SQLite/shared
client replay returned 85 Wi-Fi packet observations with correct source identity,
replay and producer boot-clock provenance, and unknown RSSI/channel/GPS/wall time.
SQLite `quick_check` passed; the private service exited 0 and removed its socket.
This accepts the bounded diagnostic acquisition/import path only. RF loss,
endurance and concurrent live-receiver operation are not established by that test.

### Private live receiver acceptance and failed follow-up

The reviewed host source `b2db65a` and live FAP source `f457474` then passed one
private shared-backend capture/Pause/Finish run. The backend artifact SHA-256 was
`38f8b3b61c253b0b38193b6127157b43341ec4b3d7c6b7754bf03de551b16ad2`;
the FAP SHA-256 was
`40652e454a05d28a63dec7f94d77b8bb10d9095a4c12d317abb466dbfd63d976`.
SQLite retained 187 records, including 65 real WiFi packet observations, with the
configured source, bound app-instance token and capture generation 1. Original
device boot-relative timestamps remained distinct from host receipt time;
channel, RSSI and receiver location were unreported. The bound Hello identifies
the FAP build and its compiled firmware ABI, not AWOK firmware or hardware boot.

Pause was requested 2.002654 seconds after the host observed Capturing. The device
Capturing-to-Paused report span was 3.016 seconds; neither number is an exact RF
duration or a hard two-second device limit. Durable evidence showed Paused
(cause 1/evidence 7), a fresh Finish Stopping boundary (cause 2/evidence 3), then
Stopped (cause 2/evidence 7). Known UART/RPC/host loss counters were zero; RF loss
remained unknown. The recording finished, the private service exited zero,
CLI/idle-loader and original companion hashes were verified, and the serial port
and lease were released. The installed AirHound service and its data were preserved.

The next bounded test intended to exercise quiet Pause and one explicit Resume,
but failed during the first Pause. It retained 63 packets in 176 records and ended
failed; **Resume was never dispatched**. A fresh stop-text acknowledgment was
observed while UART bytes continued arriving, but the final report did not prove
clean drain completion. The earlier failure DTO omitted its app uptime and did
not distinguish deadline expiry from malformed/incomplete framing, so the exact
terminal condition is not established. Cleanup restored CLI/idle-loader access,
preserved the original companion and default service, and released the port;
radio-stop completion remained unverified. This attempt does not negate the
earlier bounded pass or count as successful Resume.

Pinned producer source shows that Marauder can acknowledge Stop before draining
queued packet buffers. The reviewed follow-up retains a one-second acknowledgment
budget, adds a finite two-second first-ack drain budget and an absolute three-second
stop ceiling; repeated acknowledgments, heartbeats and escalation cannot extend
that ceiling. Host diagnostics retain the failed report's app uptime/counters and
specific stop-failure reason without promoting it to an accepted lifecycle state.
Producer `1a5ce78` and host composition `c7f890d` passed independent source review
and eight paired tests, including a finite approximately 16 KiB tail beyond the
old one-second deadline. The new FAP was installed with its previous image parked
and hash-verified; the original third-party companion remained unchanged. The
actual Rust engine then passed UART-free Hello/Ready/Stop in 0.915901 seconds,
with generation zero, all UART counters zero and independent CLI/idle-loader,
hash and lease cleanup checks.

The new FAP SHA-256 is
`3e0143e50f302e784919b3e6dffaac79b8acb631a2f37af939d4149c0a170d68`;
the private `c7f890d` backend SHA-256 is
`522d95560e13ab90bec41679f6eb3cb22953c65ec56efa0d4be06c038de9ada0`.
One bounded test of these artifacts retained 65 packets in 183 records and ended
failed after 4.211190 seconds. Pause was requested 2.000815 seconds after observed
Capturing. The producer observed the fresh stop acknowledgment at app uptime
3,036 ms and reached Paused at 3,486 ms with 33,053 UART bytes and evidence 7.
Six milliseconds later it received 30 more bytes and emitted failure-related
Stopping status with cause 6, evidence 3 and 33,083 UART bytes. The host retained
that exact diagnostic clock/counters without accepting the transition. The bytes
are not identified by this telemetry; malformed framing is not established.
The bridge briefly reported a completed drain, but quiet Pause was not sustained and **Resume was never
dispatched**. All reported UART/RPC/host loss counters were zero; RF loss and final
radio-stop completion remained unknown. The private process exited 1; after the
bounded 6.2-second failure-settle interval, CLI/idle-loader, hashes, port/lease
release and unchanged default-service state were verified. No automatic retry
occurred. The earlier 65-packet finished run remains a separate result on the old
artifacts, not successful Resume evidence for this replacement.

### Stop-reply accounting and Resume acceptance

The next producer, `4ab3c66`, tracks replies to all issued Stop commands, including
its bounded retry. It retains the existing one-second first-acknowledgment,
two-second drain and three-second absolute stop limits. This corrects a
reproduced premature-Pause path; it does not identify the previous run's 30 bytes.
The host distinguishes late paused input (cause 13) from missing outstanding
acknowledgments at the deadline (cause 14). Eleven paired tests passed.

The new FAP SHA-256 is
`780436ec7082c147e33bc50d0460b49873d1f7bec11647bb6aab781dda6a6431`;
its private `a239454` backend SHA-256 is
`c74737918a1c8e5e7bc70b27c6ababb382b6a5f2ad84f717f8173fe4a7a9e385`.
Installation preserved both earlier bridge images and the original third-party
companion. Actual Rust UART-free acceptance passed in 0.930901 seconds with
bound Hello, generation-zero Ready/Stop, zero UART bytes and verified cleanup.

One subsequent live run **finished after 8.733094 seconds**, retaining 373 records
and **130 WiFi packets: 65 in generation 1 and 65 in generation 2**, with one app
instance and correct source attribution. Pause was requested 2.010049 seconds
and 2.000162 seconds after the two host-observed Capturing states. These are host
control timings, not hard RF-duration limits. During the settled first Pause,
fresh producer reports were 1.207 seconds apart with unchanged UART count
(28,872 bytes), unchanged 65-packet count and a held serial lease. No packet was
stored between the durable Paused boundary and the next Preflight.

One explicit Resume established generation 2. Finish retained fresh evidence:
Paused/cause 1/evidence 7, then Stopping/cause 2/evidence 3, then
Stopped/cause 2/evidence 7. All reported UART/RPC/host loss counters were zero;
RF loss remains unknown. SQLite integrity and foreign-key checks passed. The
private service exited 0, CLI/idle-loader and artifact hashes were verified,
the serial port and lease were released, and the installed default service,
recordings and profiles were unchanged. This closes this build's bounded quiet
Pause/Resume check; later completion-capable results are scoped separately below.

### Completion-capable fault and external combined acceptance

Producer `c7e72ae` (FAP SHA-256
`82a9777c1cc96048e2ae2752cfe8a18ae995871e558349639c40c4a22cbac1e3`)
adds a distinct bounded-completion bit; older evidence 7 alone is not that proof.
The normal `306d24a` lifecycle and corrected `58239fa` watchdog results appear in
the matrix. The first watchdog attempt on `306d24a` retained 66 packets in 210
records but lost validated completion and remains a failed acceptance. An offline
regression reproduced terminal-before-pending-heartbeat-ACK cleanup invalidation;
the failed hardware run's exact wire ordering was not retained. The corrected
host drains only already-submitted replies within the original deadline and logs
secondary cleanup failures without repairing failed session health.

The successful watchdog's derived completion validates the bound backend failure
context, not separately persisted intermediate wire transitions. Its generic
failed-source `radio_stop` remains `unverified`. The run stayed failed, the private
service exited 1 as expected, and device/default-service preservation passed.

The external combined run used the same producer and host `58239fa` (backend
SHA-256 `9675ea5e6d2095ec5d2e31db095569967f3f4122ffce07c399c920e02a65d8c5`).
Fresh reports advanced 1.610 seconds during quiet Pause with unchanged counts
from all three receivers; Cat resumed with a new owned helper. All 945 Cat packets
were CRC-valid `complete_advertisement` records. The original harness incorrectly
required `complete_packet`, so its false verdict is preserved. A corrected
allowlist, negative tests and independent read-only validation checked every
remaining gate. A history-only service returned all 4,183 records in 21 pages,
matching SQLite cursors, sources, kinds and payloads; original and copied DB hashes
were unchanged. No additional capture was performed.

GPS associations numbered 925 Cat, 1,332 ALFA and 700 Flipper, with zero invalid
source/device/age relationships. They describe receiver-rig position at host
processing time, not transmitter position or synchronized RF capture time.
Known UART/RPC/host/producer drop counters were zero; RF loss remains unknown.
SQLite integrity/foreign-key checks, fresh Finish, helper/port/lease cleanup and
preservation of default recordings, profiles and helper pointer all passed.
This is external-only acceptance; native radios, missing M5 and actual GUI capture
controls were not exercised by this run.

PR #295 also corrects helper-supervisor cleanup ordering: completion is published
only after child reaping and lease release. That fix was newer than the
then-installed `33a5e93` backend; the app/service were not replaced during those
tests. It is now included in the delivered `aa472a0` preview above. The accompanying
GPS fixture interpreter changes affect tests only. Earlier CI interpreter startup stalls remain
internally unexplained; production readiness timeouts are unchanged.

## Remaining acceptance

- Reconnect and verify the M5 before claiming the complete rig is ready. Native
  permission/reception passed for the prior `33a5e93` backend; verify the newly
  installed signed identity independently.
- Exercise **Attached receiver rig (USB recovery)** through the native Load, Start Recording
  and Finish Recording controls. Actual GUI service launch and native observation
  presentation passed; the earlier native/M5 combined capture used CLI controls
  against that GUI-launched service, and the latest Flipper combined run used a
  private service. A GUI-control observer timed out without starting a run
  when focus moved away; it is not counted as GUI capture acceptance.
- Extend combined acceptance to physical unplug/replug, sleep/wake and prolonged
  load, which remain untested in this assembled build. Synthetic
  malformed framing, queue overflow, EOF, boot-boundary and pause tests already
  cover the M5 reader; they do not replace hardware fault injection.
- Extend Flipper fault acceptance beyond the passed paused-host watchdog case to
  active-capture transport loss and physical USB removal/re-enumeration. The
  accepted external concurrent session does not establish the full native/M5 rig
  or endurance. The native setup UI from #297 is installed in `aa472a0`,
  but its actual capture controls remain unaccepted. Preserve prior failure
  evidence and do not retry the crashing third-party acquisition path without a
  reviewed mitigation.
  A separate bounded BLE management discovery found no advertisement for the
  known Flipper identity; no BLE connection, authenticated read, pairing request
  or write occurred. The cached paired application name is not current firmware
  or management availability evidence.
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
