# Production delays: reviewed inventory and remaining work

The initial inventory was based on `a9dd4d1` (2026-11-24); the table also records
the subsequent helper deadline correction in #343. It follows the
production-delay work in [#329](https://github.com/dougborg/AirHound/issues/329),
separately from deterministic test-clock migration. It is a targeted inventory,
not proof that every transitive wait has been found or that every hardware delay
is necessary. No timing constant is changed by this document.

## Resolved startup and idle waits

- [#338](https://github.com/dougborg/AirHound/pull/338) replaced the desktop's
  socket-existence polling with authenticated Hello/Status readiness and one
  absolute startup budget. The remaining typed transient-connect retry is
  backoff; elapsed time is never proof that the service is ready.
- [#341](https://github.com/dougborg/AirHound/pull/341) replaced the ESP-IDF BLE
  output loop's empty 100 ms timeout with channel receive, and the embedded
  scanner's no-work 60 s wakeup with a pending future retaining its scan guard.
  Channel closure and guard cancellation remain meaningful lifecycle events.
  These source changes do not establish new hardware acceptance.

## Twelve reviewed operational sites

| Site | Current purpose | Follow-up and constraints |
| --- | --- | --- |
| [Swift process collector](../apps/macos/Sources/AirHoundCore/DiscoveryProcess.swift), `collect` | Polls output, process state, cancellation and deadline every 10 ms. `ProcessScheduling.wait` defaults to `Thread.sleep`; test injection did **not** remove production polling. | Highest-value next event-driven change: multiplex pipe readability, owned exit, cancellation and deadline. Preserve bounded drain, the 250 ms TERM-to-KILL grace, reaping and descendant-held-pipe handling; EOF alone is insufficient. |
| [BLE child supervisor](../hostd/src/ble_source/child.rs) | Checks child exit and stop requests every 20 ms. | Wait on process-exit and stop events; retain reap-before-lease-release and ownership retention when reap state is unknown. |
| [Managed gpsd](../hostd/src/gpsd/managed.rs), readiness loop | Up to 30 actual child/listener ownership checks, paced by 100 ms waits. Probe work adds to elapsed time. | Preserve exact endpoint ownership and finite failure bounds. Spawn completion alone does not prove listener readiness. |
| [Sniffle helper](../scripts/sniffle-source.py), startup | Waits 700 ms after serial construction, then sends a marker and queries firmware version. The delay has no in-source protocol rationale; the version response supplies actual identity evidence. | Verify the pinned reset/boot protocol before replacing settling with bounded acknowledgment. The signal handler sets `stopped`, but startup marker/version/setup do not check it before the capture loop; test cancellation during startup explicitly. Do not remove settling by assumption. |
| [macOS profiler](../scripts/profile-macos.py), PID discovery | Polls process inventory every 50 ms after LaunchServices submission. | Prefer a launch PID receipt or process event; preserve exact process ownership checks. |
| [macOS profiler](../scripts/profile-macos.py), measurement loop | Wakes every 5 ms for scheduled commands, one-second resource samples and log progress. | Use IO/process events plus the next scheduled deadline. Physical measurements still use real time and remain separate from correctness gates. |
| [macOS profiler](../scripts/profile-macos.py), shutdown | Checks process exit every 10 ms within a ten-second shutdown bound. | Use an owned exit event; retain identity verification and explicit forced-cleanup failure reporting. |
| [Receiver soak runner](../scripts/receiver-soak.py), selector loop | Services IO and protocol/control deadlines with a 50 ms selector timeout. | Use the nearest scheduled deadline and process/cancellation events. Preserve ACK matching, EOF handling and bounded cleanup. This interval is not proof of readiness. |
| [Native test runner](../apps/macos/run-tests.py), `cleanup_owned` | Allows a two-second TERM grace on failure before KILL/reap. | May finish early on verified owned-process exits; preserve descendant identities, fallback and reaping. Tests inject grace decisions; the operational cleanup bound remains necessary. |
| [Helper capability inspection](../scripts/stage-local-receiver-helpers.py), `helper_capabilities` | Enforces the exclusive three-second query deadline through EOF, child exit and decoded-result acceptance. The budget begins after process creation. | [#343](https://github.com/dougborg/AirHound/pull/343) corrected the former minimum 10 ms exit-wait extension after EOF. Cleanup retains its separate bound; this does not remove interpreter-dependent subprocess polling. |
| [Desktop refresh](../apps/macos/Sources/AirHoundApp/SessionModel.swift) | Refreshes status/history every two seconds. | This is scheduling policy, not startup readiness. Event-based refresh requires a backend notification contract; cancellation must remain prompt. |
| [Firmware monitor](../scripts/monitor-firmware.py), optional reset | Holds RTS for 100 ms only when reset is explicitly requested. | Preserve physical reset sequencing until the hardware requirement is reviewed. It is not a general readiness wait. |

Timed Python subprocess waits can also poll internally, depending on the
interpreter; searching for direct `sleep` calls cannot establish their absence.
The inventory therefore includes deadline-bearing subprocess paths as well as
explicit delays.

## Other retained timing classes

The broader review also identifies finite retry/backoff (serial, BLE, GPS,
Rayhunter, USB and SQLite contention), periodic durability/health work, RF channel
dwell, GPIO sampling/debounce, buzzer duration and display/reset sequencing.
These require their own policy or hardware rationale, not mechanical deletion.
Flipper bridge/diagnostic polling and ALFA paused-command polling remain event
multiplexing candidates. Flipper callback teardown must wait for actual callback
return; replacing that wait with a timeout must not free a live context.

Continue these follow-ups in #329 with causal readiness/exit tests, injected clocks
for deadline boundaries and separate outer hang guards. A clean
[test timing guard](test-timing-guard.md) only addresses its reviewed correctness
scope; it does not certify that production polling or unexplained settling has
been eliminated.
