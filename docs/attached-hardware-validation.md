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
| Flipper Zero | STM32 `0483:5740`, USB CDC; Momentum `mntm-012`, commit `e1784e74`, API 87.1 | After the user reconnected USB, fresh `device_info` and idle-loader checks responded; subsequent companion metadata reads matched the same hardware identity | Earlier nonresponsive attempts remain failures. Later screen snapshots reported an out-of-memory crash after the JavaScript attempt and a distinct NULL-pointer crash after a native companion attempt; exact native fault site remains unknown. Stop recovery and transport release were verified |
| AWOK Dual Touch v3 | No directly attached CP2102 endpoint; Flipper GPIO USART route | Fresh GPIO `info` reported official Marauder 1.21.0, Dev Board Pro target; settings reported SavePCAP enabled | Binary-capture attempts produced empty files. A native console displayed beacon reception before a crash, but no complete binary capture survived. Final stop-only recovery received two firmware-text stop acknowledgments. Genuine capture/import and live host bridge remain gaps |
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

## Installed recovery update

[#292](https://github.com/dougborg/AirHound/pull/292) and
[#294](https://github.com/dougborg/AirHound/pull/294) are merged. The installed
application now combines GUI source `41f704a` (including the reviewed capture
controls) with backend source `33a5e93`. These are the actual component builds,
not a claim that an earlier artifact was rebuilt from the merge commit. The final
merged tree differs from this composition only in a test-only GPS fixture fix.
The installed signed backend SHA-256 is
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
was approved automatically. This is pending user authorization, not native-radio
acceptance for the new binary. Unlock the Mac and approve **AirHound** Bluetooth
and **AirHound Service** Location before repeating the bounded check with the
same installed service.

At the reviewed #294 head, all 19 required checks and the relevant native,
packaging, performance, website and CodeQL workflows passed. Desktop line
coverage was 84.54% on macOS and 84.38% on Linux; function coverage was 81.98% and
81.95%, respectively. Test bodies were excluded and no physical hardware was
exercised by coverage. Linux region coverage was 79.98%, so these results do not
mean every coverage metric exceeds 80%.

Physical CatSniffer reconnect remains tracked in
[#290](https://github.com/dougborg/AirHound/issues/290). The full hardware milestone
[#269](https://github.com/dougborg/AirHound/issues/269), native GUI Start/Finish
acceptance, reattached M5 verification and native permission retry remain open.

## Flipper and AWOK follow-up

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

The next isolated implementation is the [AirHound UART diagnostic](../companions/flipper/README.md#bounded-uart-diagnostic):
fixed memory, one UART command owner, device-local stop timing, immutable UI text,
and storage only after RX shutdown. It is **not capture accepted**, is not a fix for the unknown native fault, and
does not replace or flash either firmware.
Its default launch is UART-free. The unique `de00984` build passed actual synthetic
load/selftest/exit with 53,048 bytes free heap while loaded and 127,752 after exit;
its original companion hash stayed unchanged. One capture-mode attempt then
aborted in preflight: `start_sent: false`, no scan command issued, closed empty raw
file, zero reported UART drops/errors/unread bytes. Because that build did not
retain preflight bytes, silence versus line mismatch remains unknown. A subsequent
stop-only recovery returned two stop acknowledgments in 85 hash-verified bytes,
ending in `stopscan` with idle loader and released transport. No RF retry occurred.
The follow-up retains bounded preflight bytes and adds a mode that cannot emit
Start, so transport diagnosis precedes another capture attempt.

## Remaining acceptance

- Approve the updated installed application's pending Bluetooth and Location
  prompts, then repeat native reception without rebuilding or resetting privacy
  settings. Reconnect and verify the M5 before claiming the complete rig is ready.
- Exercise **Attached receiver rig (USB recovery)** through the native Load, Start Recording
  and Finish Recording controls. Actual GUI service launch and native observation
  presentation passed; the combined capture above used CLI controls against that
  GUI-launched service. A GUI-control observer timed out without starting a run
  when focus moved away; it is not counted as GUI capture acceptance.
- Extend combined acceptance to physical unplug/replug, sleep/wake and prolonged
  load, which remain untested in this assembled build. Synthetic
  malformed framing, queue overflow, EOF, boot-boundary and pause tests already
  cover the M5 reader; they do not replace hardware fault injection.
- Validate the isolated diagnostic's new stop-only preflight mode, then its
  bounded passive acquisition and a genuine complete capture/import. The earlier
  revision passed UART-free load/exit and measured headroom only.
  Fresh identity/control and displayed beacon text do not replace a complete
  binary capture. Preserve the acknowledged-stop state; do not retry the
  crashing third-party acquisition path without a reviewed mitigation.
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
