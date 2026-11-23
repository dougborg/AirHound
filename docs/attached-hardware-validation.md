# Attached hardware validation — 2026-11-23

This is bounded acceptance evidence for the local macOS setup, not a release-wide
hardware certification. Supported sources still need permissions and correct
firmware. USB product names alone establish neither firmware nor working reception.
The remaining integration work is tracked in [#269](https://github.com/dougborg/AirHound/issues/269).

## Current acceptance status

After renewed user permission approval, the installed `07c7142` backend received
321 WiFi and 1,273 BLE observations in a finished 20.135618-second native check.
All prior recordings and saved setups were preserved. This verifies reception on
the current signed backend, not native GUI capture controls; the earlier failed
attempt remains retained.

The completion-capable Flipper producer `c7e72ae` passed a private normal
Pause/Resume/Finish run with host `306d24a` (129 packets), then a paused-host
watchdog test with corrected host `58239fa`. The latter correctly left the
recording failed while retaining backend-validated stop completion. A subsequent
CatSniffer/ALFA/Flipper session with managed GPS finished with 3,106 observations
and 2,957 fresh receiver-location associations. Its original harness rejected a
valid Cat packet classification; independent saved-evidence revalidation passed
without recapturing. A later active-host suspension with host `07c7142` retained
147 packets and backend-validated producer stop completion, but strict fault
acceptance failed because the host reported eight output drops. Physical USB
loss/reconnect remains untested, and producer completion does not prove physical
RF silence. Earlier acquisition, drain and watchdog failures remain below.
The Flipper UI and live-source builds are delivered in the installed `07c7142`
preview described below. Its newly verified native reception and the separately
delivered GUI update do not establish actual GUI control acceptance.

The M5 now runs the verified `96b19ae` / `0.2.0` application from
[#321](https://github.com/dougborg/AirHound/pull/321). Its ten-minute observation
received 118,135 BLE and 2,569 WiFi reports with one boot and no reported panic
or assertion. Both final windows contained both media and repeated advertisers.
The strict acceptance still failed: health reports were missing, output and log
drops were reported, and internal-memory samples were not constant. The user
confirmed working buttons and a quiet buzzer during capture, then both buttons
still working after the reader closed without reset or unplug. This bounded
physical-control evidence does not establish current host Pause/Resume/Finish,
physical recovery or complete-rig acceptance. The reviewed `3fbc1a0` follow-up
addresses health-output bursts; target CI and hardware acceptance remain pending,
and it is not installed. Earlier BLE-only and zero-input failures remain below,
tracked with the current runtime work in
[#317](https://github.com/dougborg/AirHound/issues/317),
[#320](https://github.com/dougborg/AirHound/issues/320) and
[#175](https://github.com/dougborg/AirHound/issues/175).
Physical CatSniffer unplug/replug, physical USB transport loss, actual GUI capture
controls and longer-duration recovery checks remain open. Bounded host burst handling after
the eight output drops is tracked in
[#312](https://github.com/dougborg/AirHound/issues/312), separately from the
validated watchdog stop completion and original strict acceptance failure.
The complete current rig, including native radios, the updated M5 and Flipper,
has not passed concurrent acceptance. The merged
[#316](https://github.com/dougborg/AirHound/pull/316) regression verifies consecutive
recordings and live/saved history through the model and a private real backend;
it does not establish actual GUI button acceptance.

The Orbic currently runs official Rayhunter `0.13.0`. A later bounded attempt
of the separate [raw-feed fork](https://github.com/dougborg/rayhunter/pull/1)
launched the prototype with raw streaming disabled and finalized a valid
25,306-byte gzip recording. The harness failed: a background process retained
its ADB output pipe, and BusyBox `wget` rejected a legitimate HTTP 202 response.
The enabled raw-feed phase never ran. Official-service restoration and protected
recording/configuration preservation were verified; cleanup removed only the
exact obsolete copied executable and preserved the other 43 staged files.
The corrected harness has offline review coverage but its revised runtime has
not executed. Neither this disabled-phase result nor the earlier helper preflight
establishes raw-feed or AirHound cellular-source acceptance. Experimental work
remains in [#293](https://github.com/dougborg/AirHound/issues/293), outside current
rig acceptance; the earlier unsuccessful staging attempts remain historical.

The matrices below retain earlier successes and failures with their build and
test scope; they do not imply that every tested device is currently attached or
enabled.

On 2026-11-23 UTC, clean candidate `fc3a556` passed one explicit u-blox
`IdentifyDevice` request through the production Rust backend in a private service.
The receiver reported firmware `SPG 3.01`, software `ROM CORE 3.01 (107888)`,
hardware `00080000` and protocol `18.00`, with a timestamp inside the actual
request. No `MOD` field was reported, so model remained unknown; compatibility
also remained unknown and reception remained `not_tested`. No capture started; the private service
exited cleanly, the serial port and shared lease were released, and the installed
service, saved data and setups were preserved. This establishes the macOS backend
identification path for [#309](https://github.com/dougborg/AirHound/issues/309),
not GPS reception or native button acceptance. The installed `07c7142` preview
now includes this explicit identification action; its native button remains
unaccepted on the installed build.

## Current preview delivery

The reviewed desktop preview is built from clean source `07c7142`, whose full
source tree matches merged main `e36c5b8`. It includes the macOS-only explicit
u-blox identification action and queried-version snapshot UI from
[#310](https://github.com/dougborg/AirHound/pull/310). Identification does not
establish a GPS fix, reception or receiver compatibility; an unreported module
model remains unknown.

This delivery retains the host integration from
[#299](https://github.com/dougborg/AirHound/pull/299), compatible with the separately
installed companion from [#298](https://github.com/dougborg/AirHound/pull/298),
and the native integration tests from
[#306](https://github.com/dougborg/AirHound/pull/306). The prior `aa472a0` rollout
and its narrower acceptance evidence remain historical results.

The preview rollout preserved existing recordings, saved setups and settings.
Reviewed checks covered signed application startup, saved-data compatibility and
restoration preparation. These checks establish delivery and preservation, not
new hardware reception or a live downgrade. The rollout did not flash device
firmware or start capture.

A subsequent bounded native-only check on installed `07c7142` failed after
20.129466 seconds with no observations. WiFi reported
`addresses_redacted_check_location_permission`, pointing to a Location-access
check; Bluetooth adapter initialization timed out, with the underlying cause still
unknown. The failed recording was retained, existing recordings and saved setups
were preserved, and the same service returned to idle. This failed result remains
unchanged; it is not permission or reception acceptance.

On 2026-11-23 UTC, after the user approved more permissions, one separately
reviewed retry on the same signed `07c7142` backend finished in 20.135618 seconds:
321 WiFi and 1,273 BLE observations, 1,599 total records, and zero invalid source
associations. Independent checks verified database integrity and exact preservation
of every prior row across all six tables and all saved profile-store files. The
same service returned to idle; the earlier failed recording was retained.

A separate runnable GUI built from `946d3c0` was delivered with the exact existing
signed `07c7142` backend preserved byte-for-byte. It includes the reported Flipper
loss-counter presentation fix. The backend stayed running, and the older GUI was
left running to retain any RAM-only draft. The user subsequently confirmed that
GUI windows were visible; an empty
accessibility-window inventory was not proof that no window existed. Launch and
visible-window confirmation do not establish capture-control acceptance.

Actual GUI Identify/Load/Validate/Start/Pause/Resume/Finish controls and observations
presentation remain unaccepted. Earlier `33a5e93` native reception and private
Flipper/combined results below retain their original artifact scope.

### M5 revalidation before the firmware update

On 2026-11-23 UTC, the reconnected M5 was tested through a private `07c7142`
backend at 460800 baud. The first attempt retained 67 WiFi observations but no
BLE observations or device-status report in 12.225539 seconds; its acceptance
failed before Pause/Resume. The 12-second identity gate was shorter than the
firmware's documented 30-second reporting interval, so absent identity in that
window was inconclusive. The zero BLE count remains actual and unexplained.
The original failed acceptance and its cleanly finished recording were retained.

A reviewed correction allowed 35 seconds for the unchanged identity/both-media
gate and a 70-second independent private-process deadline. The single corrected
attempt finished in 18.683479 seconds with 86 WiFi and 24 BLE observations in
130 records. Actual device status reported AirHound `0.1.1` on `m5stickc_plus2`.
Host Pause stayed quiet for 2.0357155 seconds; both media advanced after Resume,
with the same connection and sampled boot generation, then Finish released the
port and shared lease. Twelve observations were deliberately discarded while
host ingestion was paused; the autonomous device radios were not commanded off.

Reported host drops, malformed/oversized records and reconnects were zero. One
producer sample reported nine cumulative scan drops since boot, with unknown
timing; output drops, BLE output drops and serialization errors were zero.
These counters do not establish zero RF loss. Independent checks verified source,
clock and identity provenance, database integrity, clean private-service exit and
unchanged default-service data and saved setups. This is individual M5 acceptance,
not the complete current rig, physical recovery, endurance or GUI acceptance.

### M5 firmware update and continuity gap — 2026-11-23 UTC

A complete 8 MiB flash backup was read and checked against a same-session device
digest, with a second verified copy and an application-sector rollback image
preserved. The old application, bootloader and partition bytes matched the
historical `7739926` artifact, closing the old build-provenance gap. The first
backup attempt had failed before reading or writing flash because the procedure
omitted the tool's required flash-attach step; that failure was retained and the
corrected procedure was independently reviewed before the successful read.

The selected `0.2.0` application came from official CI source `07c7142`, with
firmware inputs equivalent to the reviewed current source. Its 943,728-byte
application image was written only at the existing application offset. Full-flash
device digest verification matched the reconstructed expected image, including
the erased-sector tail, unchanged bootloader, partition table, NVS and PHY data.
The linked stack reservation was 7,292 bytes, above the 7,168-byte build gate;
this is not a runtime stack high-water measurement. Rollback artifacts and a
reviewed restore procedure are prepared; an actual rollback was not exercised.

One 180.170-second boot observation reported `0.2.0` / `m5stickc_plus2`, one boot,
1,395 WiFi and 90 BLE observations, successful 2 MiB PSRAM initialization, and five
periodic samples with stable internal free heap (3,672 bytes) and external free
memory (2,068,472 bytes). All four sampled producer drop/error counters were zero;
no panic or error was observed. These are bounded samples, not an endurance or
zero-RF-loss claim. This earlier boot receipt did not establish physical
button/display behavior or audible buzzer output.

The subsequent private recording on the same boot retained 234 WiFi observations
and no BLE observations in 251 records over 35.262374 seconds. Actual status at
uptime 301 seconds confirmed the expected version and board, with zero reported
producer and host loss/error counters. The strict both-media gate failed; Pause
and Resume were not attempted. Receipt-owned Finish, private-service exit,
database integrity, port/lease release and default-data preservation passed. The
failed acceptance remains unchanged.

The retained boot stream's last BLE report was at device time 109.260 seconds,
while WiFi continued through 179.606 seconds. The pinned legacy BLE library enables
controller duplicate filtering, and firmware retains that scan session indefinitely;
the same configuration was present in the old image. This is a concrete source
finding consistent with the observed pattern, not proof that the controller
stalled or that ambient RF was quiet. The correction and continued-report/lifecycle
validation are tracked in [#317](https://github.com/dougborg/AirHound/issues/317).
The reviewed fix in [#318](https://github.com/dougborg/AirHound/pull/318) explicitly
disables controller duplicate filtering after scan startup and requires command
acknowledgment. Its first target build failed the unchanged stack-reservation
gate. Follow-up `3baaacc` reuses async storage across startup/configuration. Its
M5 and XIAO target builds passed; the M5 linked stack reservation is 7,316 bytes,
above the unchanged 7,168-byte threshold. This is not a runtime stack high-water
measurement. The audited application was subsequently installed and verified;
the bootloader, partition table, NVS, PHY and all other flash bytes were preserved.
The preceding application and rollback material remain available; no rollback was
needed for this installation.

The corrected image passed a 240.097-second serial observation: 37,244 BLE and
1,430 WiFi reports, with 56 advertisers present in both early and late intervals.
Both final 30-second intervals contained both media. Seven dedicated memory
samples reported 3,448 bytes internal and 2,068,472 bytes external free; separate
status samples reported 3,348–3,448 internal free bytes. All seven stream-statistics
samples had zero scan/output/BLE-output drops and serialization errors. These
sampled counters do not establish RF losslessness. Button events were logged
while the observer was running, and the user temporarily reported working buttons;
that does not establish reliable operation after the observer closes.

After close, the user reported unresponsive buttons again. The subsequent
production-reader attempt failed after 35.220412 seconds with no device input:
no status, uptime, WiFi or BLE. Its 13 durable records were host-side records only.
Pause/Resume were not attempted. Owned Finish, private-service exit, port release
and preservation checks passed. This is a distinct failure from the earlier
BLE-only result, not evidence of quiet RF.

A separate 35.072-second diagnostic deasserted DTR/RTS before opening, without a
deliberate reset pulse or UART writes. It produced a fresh boot, 6,098 BLE and 159
WiFi reports, and status uptime 31 seconds. Its one stream-statistics sample had
four scan drops and zero output/BLE-output drops or serialization errors. Owned-fd
snapshots showed deasserted DTR/RTS and enabled `HUPCL` after open and before close.
The user did not confirm button recovery and reported continued unresponsiveness.
These results motivate a control-line investigation; they do not prove the
physical reset-line state or a fixed firmware/UI fault.

The scoped host candidate `14d6085` clears and verifies Unix close-time hangup,
restores the configured baud through the platform serial API, and atomically
deasserts/verifies DTR/RTS before ingestion. Fourteen focused tests and independent
review passed, including macOS custom-baud handling, unsupported-control refusal,
and close-before-lease cleanup. Its first hardware opening test then failed:
35.099607 seconds, 13 host-side records and no device input, status, uptime, WiFi
or BLE. Pause/Resume and the second stage were not attempted; no reset or retry
followed. Owned Finish, private exit, port/lease release, database integrity and
default-data preservation passed. The recording was finished cleanly, but the
acceptance verdict remains failed. This policy is not a proven reception or
button fix and has not replaced the installed backend. Passive discovery and
other receiver transports are unchanged. Opening may still reset the board;
Pause/Resume must not toggle lines. At that stage, lifecycle, physical buttons and
full-rig acceptance remained pending under
[#175](https://github.com/dougborg/AirHound/issues/175) and
[#269](https://github.com/dougborg/AirHound/issues/269).

### Current M5 UART correction — 2026-11-23 UTC

A subsequent controlled-reset diagnostic on `3baaacc` received both media, then
captured an explicit controller assertion and CPU exception while the reader
remained open. Completion of that diagnostic was not runtime acceptance; the
original raw panic and failed lifecycle evidence remain retained under
[#320](https://github.com/dougborg/AirHound/issues/320).

The reviewed `96b19ae` UART-output correction was then installed from official
M5 target CI. The 950,032-byte application (SHA-256 prefix `d7e383a0`) was written
only at the existing application offset. Full-device digest verification matched
the expected image and preserved all bytes outside the 950,272-byte erase span.
Two verified copies of the previous complete image and its exact application
sectors remain available for rollback. The linked stack reservation is 7,276
bytes against the unchanged 7,168-byte gate; runtime stack high-water is unknown.

| Current-image observation | Actual result |
| --- | --- |
| Duration / boot / faults | 600.038997 seconds; one boot; no printed panic or assertion |
| Reception | 118,135 BLE and 2,569 WiFi reports; both media in 540–570 and 570–600 second windows; 42 advertisers seen both early and late |
| Periodic evidence | 14 Status and nine StreamStats reports instead of at least 19 each; 19 memory and 19 serial-output samples |
| Latest sampled counters | 30 output drops, including four diagnostic-log drops; 26 scan drops; BLE-output drops, serialization, formatting and UART-write errors zero |
| Memory | Dedicated internal samples 3,396–3,448 bytes; Status minimum 3,368; external samples constant at 2,068,472 bytes |
| Physical controls | User confirmed both buttons and quiet buzzer during capture, then both buttons still working after close without reset/unplug |

The original acceptance verdict remains **failed** for incomplete periodic
reports, reported output/log loss and nonconstant internal-memory samples. The
52-byte fluctuation is not by itself evidence of a leak. Diagnostic-log drops
are included in output drops, not an additional loss count; cumulative samples
do not establish unsampled final totals or RF loss. Reader cleanup and default
data preservation passed. No current-image host lifecycle or full-rig run followed.
The independently reviewed `3fbc1a0` follow-up in
[#321](https://github.com/dougborg/AirHound/pull/321) yields between the remaining
health-report enqueues; its target CI and hardware acceptance are pending. The
installed image remains `96b19ae`, not that unflashed follow-up.

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
| Native Bluetooth | CoreBluetooth, OS-managed | Current signed `07c7142` retry: 1,273 observations alongside WiFi, clean finish; earlier `33a5e93` result retained below | OS determines adapter/PHY; USB Bluetooth dongles do not become separate CoreBluetooth receivers |
| Native WiFi | CoreWLAN, OS interface | Current signed `07c7142` retry: 321 observations alongside BLE after renewed permission approval, clean finish | Earlier redacted-address attempts remain failures. Permission is specific to the running app/backend identity; reception is not proof of GUI capture-control acceptance |
| CatSniffer 3.2 | RP2040 `2e8a:00c0`, USB CDC; reported Sniffle 1.11.0/API 0 | 30-second run: 2,563 packet observations; clean finish, zero reported host drops/reconnects | Channel 37/1M test; no new Coded/extended acceptance claim. A later explicit identity probe and reuse of active-owner metadata also passed |
| M5StickC Plus2 | WCH `1a86:55d4`, UART at 460800 baud; verified installed `96b19ae` / AirHound `0.2.0`, application `d7e383a0` | Ten-minute observation: 118,135 BLE + 2,569 WiFi, one boot/no printed panic, repeated late advertisers; user buttons worked during capture and after close, buzzer quiet during capture | Strict acceptance failed: incomplete health reports, 30 output drops including four log drops, 26 scan drops, nonconstant internal memory. Reviewed `3fbc1a0` follow-up unflashed; current host lifecycle/full rig/physical recovery pending. Preserve earlier failures under [#320](https://github.com/dougborg/AirHound/issues/320), [#317](https://github.com/dougborg/AirHound/issues/317) and [#175](https://github.com/dougborg/AirHound/issues/175); RF loss unknown |
| ALFA AWUS036ACHM | MT7610U `0e8d:7610`; explicit USB topology selector; ASIC `0x76100002` | First capture hit MCU command 12 timeout. After physical reconnect: 10 seconds on channel 1, 740 beacons, 1,600 frames/1,607 transfers, 7 malformed, cleanup verified. Later 20-second metadata run retained 1,259 observations, all attributed to its configured source; clean finish and zero reported host/producer drops or reconnects | Actual helper build reported; supplied image retained separately. Five running statuses correctly said running image unverified because the MCU was reused; stopped status cleared firmware. Short success is not an endurance claim; signal calibration remains limited |
| u-blox GNSS | `1546:01a8`, USB CDC; explicit MON-VER query reported firmware SPG 3.01, software ROM CORE 3.01 (107888), hardware 00080000 and protocol 18.00 | Candidate `fc3a556` passed one production Rust identification request with preservation and cleanup verified. Earlier 60-second CatSniffer/GPSD session: all 4,520 BLE packets carried durable GPS associations; fix age 16–1,020ms, host age 0–1,004ms | Module model unknown: no MOD field. Query does not establish compatibility or reception. gpsd 3.27.5/protocol 16.1 is host software, separate from receiver firmware. Identification is macOS-only and included in installed `07c7142`; native button acceptance remains open |
| Flipper Zero | STM32 `0483:5740`, USB CDC; Momentum `mntm-012`, commit `e1784e74`, API 87.1 | Completion-capable producer passed UART-free startup, normal Pause/Resume, one paused-host watchdog case, and concurrent Cat/ALFA/GPS reception | FAP build identity is separate from Flipper firmware and AWOK identity. Active-host suspension retained producer stop completion but failed strict acceptance with eight host output drops; physical USB loss/reconnect, endurance and actual GUI controls remain open. Earlier failures remain scoped to their artifacts |
| AWOK Dual Touch v3 | No directly attached CP2102 endpoint; Flipper GPIO USART route | Independent GPIO `info` reported Marauder 1.21.0/Dev Board Pro; diagnostic import retained 85 packets. Latest external combined run retained 728 live packets across two Flipper capture generations | Prior 63- and 65-packet attempts failed before Resume. The bridge Hello does not establish downstream firmware, assigned channel, RSSI or RF loss; literal stop acknowledgments do not prove physical RF silence |
| Orbic / Rayhunter | Official Rayhunter `0.13.0` active; exact retail model remains unconfirmed | Latest prototype attempt launched only the raw-disabled phase and finalized a valid 25,306-byte gzip; harness failed on ADB pipe ownership and valid HTTP 202 handling. Official service and protected data restored; exact obsolete-executable cleanup preserved the remaining staged files | Corrected harness runtime unexecuted; enabled raw-feed and AirHound live-cellular acceptance still pending in [#293](https://github.com/dougborg/AirHound/issues/293). Raw diagnostic records are not I/Q or all cellular RF traffic |
| Nordic BLE sniffer | `1915:522a`, USB CDC, nRF Sniffer product descriptor; historical explicit REQ_VERSION on 2026-11-21 reported firmware 4.1.1 / UART protocol 3 | A bounded query and subsequent read-only window observed an already-streaming advertising feed; prior scan configuration and queued reports were unknown ([#188](https://github.com/dougborg/AirHound/issues/188)) | Current discovery is descriptor-only; current firmware remains unverified. No live AirHound source integration; historical reads do not establish capture rate, RF loss, extended/Coded support or a verified board model |
| CSR-compatible Bluetooth dongle | `0a12:0001`, CSR8510A10 product descriptor | Recognized prototype/research target only | No verified macOS live USB-HCI path; product name does not establish genuine silicon or capabilities |

In the earlier `0.1.1` tests, M5 internal free heap was about 3.6 KiB; its startup diagnostic confirmed
2 MiB PSRAM with about 2 MiB free external memory. Neither value describes total free
memory by itself. The serial test's reported uptime progressed 61→91→121 seconds
through pause/resume, with one connection and one boot generation. Earlier Python
monitor opens reset the device despite DTR/RTS being held false; do not promise
that opening the port is nondisruptive. In the earlier 66-second M5 run, the
512 stored records included 407 WiFi and 71 BLE observations. Its eight-second
host Pause discarded 47 observations with none recorded during Pause; reported
host errors/drops and producer counters were zero in that historical run. Those
values do not replace the newer cumulative producer-drop report above.

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
| Current installed `07c7142`, native WiFi + BLE after renewed permission approval, 20.135618 seconds | 321 WiFi + 1,273 BLE observations; 1,599 total records; finished | Zero invalid source associations; every prior database row and saved profile-store file preserved. Earlier failed attempt retained; same signed service stayed running. No GUI control or combined external-rig claim |
| Private Flipper `c7e72ae` / host `306d24a`, normal lifecycle, 8.984887 seconds | 129 packets: 65 in generation 1, 64 in generation 2; 377 records; finished | Quiet Pause for 1.207 seconds, explicit Resume, fresh Finish evidence 15 → 3 → 15; cleanup and default-service preservation passed |
| Same Flipper / host `58239fa`, paused-host watchdog | 67 packets, 212 records; recording correctly **failed**; fault acceptance passed | Exact private host suspended for 6.559710 seconds after quiet Pause. No later packets; final cause 3, evidence 15 and backend-derived completion true, zero known loss/unread input. This covers paused-host heartbeat expiry only |
| Same Flipper producer `c7e72ae` / host `07c7142`, active-host suspension | 147 durable packets, 303 records over 9.660146 seconds; recording **failed** and strict fault acceptance **failed** | Exact private host was suspended with SIGSTOP for 6.523048875 seconds while capture was active. Final cause 3, evidence 15 and backend-derived `stop_complete: true`; reported producer UART/RPC losses and unread input were zero, but eight host output drops failed the loss gate. Cleanup and default data preservation passed. Physical RF silence remains unverified; this was not physical USB loss/reconnect |
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
tests. It remains included in the delivered `07c7142` preview above. The accompanying
GPS fixture interpreter changes affect tests only. Earlier CI interpreter startup stalls remain
internally unexplained; production readiness timeouts are unchanged.

## Remaining acceptance

- Verify the complete compatible rig with current native and external receivers,
  including the updated M5 and Flipper. The signed `07c7142` native retry passed;
  the M5 lifecycle success belongs to prior firmware `0.1.1`. Installed `3baaacc`
  passed continued both-media reporting, then failed production lifecycle with no
  input after observer close; physical buttons remain unresolved. Validate the
  host transport and close/reopen behavior after the reviewed `14d6085` policy
  also failed its first hardware opening test in
  [#175](https://github.com/dougborg/AirHound/issues/175) before claiming current
  full-rig acceptance. Preserve the separate earlier BLE-only failure in
  [#317](https://github.com/dougborg/AirHound/issues/317) and all later failures.
- Exercise **Attached receiver rig (USB recovery)** through the native Load, Start Recording
  and Finish Recording controls. Actual GUI service launch and native observation
  presentation passed; the earlier native/M5 combined capture used CLI controls
  against that GUI-launched service, and the latest Flipper combined run used a
  private service. A GUI-control observer timed out without starting a run
  when focus moved away; it is not counted as GUI capture acceptance.
- Verify a second recording in the same actual GUI window, then reopen both
  saved histories and inspect live/saved observations. The merged #316 automated
  regression covers this model/service path; actual button acceptance remains open.
- Extend combined acceptance to physical unplug/replug, sleep/wake and prolonged
  load, which remain untested in this assembled build. Synthetic
  malformed framing, queue overflow, EOF, boot-boundary and pause tests already
  cover the M5 reader; they do not replace hardware fault injection.
- Track bounded host burst handling after the eight output drops in
  [#312](https://github.com/dougborg/AirHound/issues/312); retain the original
  strict acceptance failure separately from validated watchdog stop completion.
  Producer completion does not establish physical RF silence. Physical USB
  removal/re-enumeration remains untested. The accepted external concurrent
  session does not establish the full native/M5 rig or endurance. The native setup UI from #297 is included in installed `07c7142`,
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
