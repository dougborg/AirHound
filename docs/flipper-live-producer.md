# Flipper live producer (development)

This separate `airhound-live-bridge` target implements the device side of the
[AHFB protocol](flipper-bridge-protocol.md). Revision `f457474` passed bounded
UART-free RPC load/Hello/Stop/exit checks through Python and the production Rust
transport engine on the attached Flipper. A subsequent private full-service
Start/Pause/Finish run stored 65 live Wi-Fi packet observations. The diagnostic
application's earlier 85-packet file acquisition remains separate evidence.

Launch is RPC-only. Default/standalone launch and Hello do not open UART. An
explicit bound StartBeacon first obtains the UART, sends a line-synchronized stop,
observes the exact stop text and quiet framing boundary, then sends only
`sniffbeacon -serial`. No arbitrary UART command, firmware flash, `info`, channel
assignment or power toggle is exposed. Hello reports compiled API87.1 and its
build provenance; those fields do not identify AWOK firmware.

The controller validates nonce, app-instance token and increasing request IDs.
Hello is the first device message, with no preceding AHFB Ack. App-instance is a
random launch token, not a firmware boot identity. Accepted acknowledgments mean
control admission; observed start/stop text is reported separately and is not
authenticated RF proof. Each Start has a fresh generation and stop preflight.
Pause-to-Stop requires a fresh request-scoped stop witness; escalation cannot
extend the original stopping deadline. Stop before any Start performs terminal
app cleanup with generation zero and no UART/physical-stop claim.

The main loop owns UART commands, consumes at most two queued controls and 248
UART bytes per iteration, and checks its own three-second heartbeat watchdog.
RX ISR activity timestamps/sequences and buffered input prevent declaring quiet
while received data awaits processing. Old-phase raw bytes are published before
any ready/start boundary. Every UART action uses a common tested dispatcher:
failed status enqueue prevents Start, while Stop is still attempted. Late input
after acknowledged Pause is an explicit failed session and requests another stop;
it never silently restarts capture. The host must reset framing only at the
explicit new-generation ready boundary, and fail on sequence gaps/loss.

RPC callbacks copy at most 300 bytes into an eight-message command queue. A
separate sender drains a sixteen-message output queue; neither RPC sending nor
filesystem access occurs in the UART controller. Queue overflow stops the source.
The GUI uses static strings and a Back cancellation flag, not borrowed logs.
Final ISR counters/unread bytes are checked after unregistering RX before any
successful terminal status is published.

The pinned SDK's `rpc_send` can block in `pipe_send`, delaying even the session
close callback. The UART watchdog therefore does not depend on that callback.
After terminal status the host closes its owned RPC session; the application
retains memory until the callback unregisters itself and the sender finishes.
A blocked host may delay app exit, even though UART stopping proceeds separately;
there is no forced thread kill or unconditional bounded app-join claim.

RPC subsystem free/SessionClose executes on the SDK timer daemon. After observing
session close and joining the sender, the main thread calls `furi_timer_flush()`
before freeing shared context or returning. In pinned Momentum `e1784e74`,
`rpc.c:354` schedules subsystem cleanup on that daemon and `furi/core/timer.c:79`
queues a firmware flush epilogue after it. This waits for the entire FAP callback
return, avoiding an unload race that a counter decremented before return cannot
solve. No main-thread callback-pointer unregister is used.

Build and static artifact verification from `companions/flipper`:

```sh
target/audit-env/bin/python scripts/build-diagnostic.py target/momentum-api.csv --binary airhound-live-bridge
target/audit-env/bin/python scripts/test-diagnostic-artifacts.py target/live-bridge airhound-live-bridge
```

The outputs retain DWARF, matching code/data and debuglink CRC, exact source/SDK
hashes and an explicit `hardware_tested: false`; acceptance evidence remains
separate from immutable build metadata. Independent source/artifact review and the
UART-free and single-source passive device checks below passed. Remaining gates
include physical watchdog/transport-failure tests, concurrent sources,
endurance and the normal native setup/capture workflow. The diagnostic and
original Marauder companion remain preserved.

## UART-free device evidence (2026-11-22)

The reviewed `f457474` artifact (SHA-256
`40652e454a05d28a63dec7f94d77b8bb10d9095a4c12d317abb466dbfd63d976`,
25,012 file bytes and 12,521 allocated ELF bytes) was installed create-only at
`/ext/apps/GPIO/airhound_bridge.fap`. Fresh device identity, API87.1, idle-loader,
heap and original-companion hash checks preceded installation; the uploaded hash
matched. No existing app or firmware was replaced.

A bounded Python harness using the pinned official RPC schema received exact
Hello build/capability metadata, then Ready at app uptime 206 ms. Explicit Stop
produced an accepted acknowledgment and Stopped at 212 ms, with generation zero,
zero evidence bits and zero UART/drop/error/unread counters. No Start command was
sent and UART was never acquired. This terminal state means app cleanup, not an
assertion about the independently powered AWOK radio.

Owned RPC SessionClose restored the CLI; the loader returned idle, the original
companion hash was unchanged, and USB ownership was released. Free heap was
117,336 bytes before load and 125,272 after exit. Loaded-app heap was not sampled.
The initial sandbox-denied USB-open invocation is retained separately and made
no device change. This proves the tested device protocol/lifetime path, not the
production host source or live concurrent capture.

A separate bounded wrapper around the production Rust transport engine at
`dbda889` subsequently passed the same no-UART lifecycle in 0.935 seconds. A
test-only outbound guard rejected any Start command before USB transmission;
actual controls were app launch, Hello, Stop and SessionClose. The clean backend
build and exact companion version/revision were verified, all checked UART/loss
counters remained zero, and post-run CLI/idle-loader/original and bridge hashes
passed. The default service and its capture state were unchanged. This confirms
the production transport engine without claiming RF reception from that test.

## Single-source service capture evidence (2026-11-22)

The unchanged `f457474`/`40652e` companion and private production backend
`b2db65a` (SHA-256
`38f8b3b61c253b0b38193b6127157b43341ec4b3d7c6b7754bf03de551b16ad2`)
passed one explicit Start → Pause → Finish session. SQLite and the shared client
returned 187 records, including **65 live Wi-Fi packet observations** attributed
to the configured Flipper receiver, bound app instance and capture generation 1.
The run finished, SQLite `quick_check` passed, and packet provenance retained
`replay: false`, the device boot clock with unknown wrap/reset, and unknown RSSI,
receive channel, GPS and AWOK firmware identity. No receiver location was attached.

Pause was requested 2.003 seconds after the host observed Capturing. The producer's
Capturing-to-Paused report interval was 3.016 seconds; neither interval measures
exact RF duration or establishes a hard two-second device limit. The existing
three-second heartbeat watchdog remains distinct from the requested pause target.
Durable reports show Paused with stop-text evidence, a new Finish-specific
Stopping boundary with that evidence cleared, then Stopped with fresh observed
stop text and completed drain. Reported UART/RPC/output drops, UART errors and
unread bytes were zero; RF loss remains unknown.

The private service exited 0. Fresh post-run checks confirmed CLI restoration,
idle loader, unchanged bridge/original companion hashes, reacquired serial lease
and released USB ownership. The user's default service and capture state were
unchanged. This accepts one single-source service lifecycle, not Resume,
watchdog fault injection, concurrent capture, endurance or native UI controls.


## First Resume-stage failure and bounded drain follow-up

A subsequent same-build attempt failed during its first Pause, **before Resume**.
Its private run retained 63 packets / 176 records and remained failed. Stop text
was observed 625 ms after the stopping transition, but UART bytes continued
through the original shared one-second acknowledgment/drain deadline. Known loss
counters were zero; that does not establish a completed drain or RF stop. The
original terminal cause merely retained `user_pause`, so malformed framing versus
incomplete/active drain cannot be resolved from that evidence. The default
service was unchanged; post-run CLI, idle loader, hashes and USB release passed.
This does not invalidate the earlier bounded success or accept Resume.

Pinned upstream Marauder source `d9197cf` calls `StartScan(OFF)` before printing
stop text (`CommandLine.cpp:379–397`), then the main loop calls `buffer_obj.save()`
(`esp32_marauder.ino:557`). `Buffer.cpp:223–267` can serialize two pending buffers
after that text. Source configuration selects two 3 KiB buffers normally, two
2 KiB buffers under its reduced-memory profile, or two 8 KiB buffers with PSRAM
(`configs.h:3373–3384`). Six KiB needs about 0.53 seconds at 115200 baud/8N1,
exceeding the 0.375 seconds remaining after the observed acknowledgment; the
16 KiB upper variant needs about 1.42 seconds. These source bounds support a
queued-tail explanation, but neither the installed binary's precise buffer
configuration nor the failed run's exact cause is proven.

The follow-up retains a one-second acknowledgment budget and gives its first
fresh acknowledgment a separate two-second drain budget, with an absolute
three-second stop ceiling. Repeated acknowledgments, heartbeats and escalation
cannot extend that ceiling. Quiet, complete framing and loss checks remain
required. Specific terminal causes distinguish late/missing text, malformed
framing, incomplete framing and drain expiry. Synthetic delayed-ack tests cover
valid 6 KiB and 16 KiB tails, tick wrap, missing/late acknowledgments, malformed or
endless tails and escalation. Independent source review and all 52 companion
tests passed. The new clean `1a5ce78` ARM artifact passed API87.1 import, DWARF,
code/data and debuglink audits, including four artifact-corruption checks.
Earlier hardware results remain tied to their original `40652e` artifact.

The reviewed replacement installed new FAP SHA-256
`3e0143e50f302e784919b3e6dffaac79b8acb631a2f37af939d4149c0a170d68`
only after verifying the existing image and staging the new one. The prior bridge
was retained in a separate on-device backup and local artifact archive; the
third-party companion remained unchanged. Installation did not launch an app or
open UART.

A separate production Rust transport check with clean host `c7f890d` then passed
Hello/Ready/explicit Stop/SessionClose in 0.916 seconds. The actual producer
reported `1a5ce78`, generation zero, zero UART bytes and no stop-text evidence;
this proves app cleanup without claiming an AWOK radio stop. Postchecks found
the loader idle, expected image hashes, released lease/USB ownership and unchanged
default service. Passive Resume and fault-path acceptance were still pending at
that stage.

### Outstanding stop replies after the drain update

The first new-artifact Resume-stage attempt again failed **before Resume**. It
retained 65 packets / 183 records. The producer reported Paused at 3486 ms with
33,053 UART bytes and complete stop evidence, then received 30 additional bytes
at 3492 ms and failed closed. The bytes themselves were not retained; their
contents and the exact device-side source remain unknown. The shared lease,
loader/hash checks, USB release and default-service preservation passed after
bounded cleanup; this attempt does not establish a completed radio stop.

The deterministic controller defect is that its first stop command at 2300 ms
was retried at 2800 ms, yet completion required only one acknowledgment. The
first reply arrived at 3036 ms. Pinned upstream `CommandLine.cpp:73–89` consumes
one newline per main-loop iteration; buffered writes occur later in the loop,
and the screenless path then delays 50 ms (`esp32_marauder.ino:586–589`). Thus a
queued second Stop can produce a later reply after the first drain/quiet window.
This is a source-supported explanation, not identification of the missing bytes.

The next correction requires observed exact replies to account for every issued
Stop before completing Pause/Stop, including retries, preflight cancellation,
and outstanding commands carried through Pause-to-Finish escalation. Replies
already observed before a new command cannot satisfy it. The existing first-reply,
drain and absolute deadlines remain unchanged, and reply counts cannot extend
them. A missing outstanding reply becomes cause14; unexpected input after a
completed Pause still fails closed as cause13, distinct from proven malformed
framing (cause6). No late data is silently accepted as resumed capture. Tests
reproduce the observed timing with a delayed split second reply, missing replies,
clock wrap, escalation and historical duplicate replies. A new independent
source/artifact review and matched-host tests subsequently passed. The new
artifact and bounded Resume evidence are recorded below; the missing bytes from
the prior failed attempt remain unidentified.


## Explicit Resume service acceptance

The independently reviewed `4ab3c66` producer artifact, SHA-256
`780436ec7082c147e33bc50d0460b49873d1f7bec11647bb6aab781dda6a6431`,
was staged and hash-verified before replacing only the AirHound bridge. The prior
`1a5ce78` image was parked separately, preserving the older accepted bridge and
original third-party companion. A separate production Rust no-UART check with
clean host `a239454` passed in 0.931 seconds: generation-zero Ready/Stop, zero UART
bytes, no Start, clean SessionClose, idle loader and unchanged default service.

One subsequent private full-service Start/Pause/Resume/Pause/Finish run passed
with the matched host binary SHA-256
`c74737918a1c8e5e7bc70b27c6ababb382b6a5f2ad84f717f8173fe4a7a9e385`.
SQLite retained 373 records and 130 live Wi-Fi packet observations, split 65/65
between generations one and two in the same producer app instance. Independent
read-only checks confirmed source attribution, both generations, a finished run
and `quick_check=ok`. The shared client exposed the observations.

During the settled first Pause, fresh producer reports advanced 1.207 seconds
while UART bytes remained 28,872; no packet observations were stored between that
Pause and the second Preflight. The same service retained the serial descriptor
and shared lease. Resume explicitly established generation two and new reception.
Finish used fresh evidence: gen-two Paused/cause1/bits7, then a new
Stopping/cause2/bits3 boundary, then Stopped/cause2/bits7. Reported UART drops,
UART errors, RPC/output drops and unread bytes were zero. The private service
exited normally; postchecks confirmed idle loader, expected hashes, released
lease/USB ownership and unchanged user service/capture state.

Pause requests were issued 2.010 and 2.000 seconds after observing each Capturing
state. These are host request intervals, not exact RF-duration limits. Literal
firmware text remains unauthenticated and unknown RF loss remains unknown.
Packet RSSI, receive channel and GPS/receiver location remain absent; timestamps
retain device-boot microseconds with wrap/reset uncertainty. This accepts one
bounded single-source Resume lifecycle. Physical watchdog/transport-loss,
concurrent receivers, endurance and the normal native workflow remain separate
acceptance gates. Both earlier failed Resume-stage attempts remain failures.


## Explicit stop completion revision (not yet hardware accepted)

The `c7e72ae` producer adds a required Hello capability and a distinct bounded
stop-completion evidence bit. The previous autonomous `Failed/cause3/bits7` report
could describe either a completed stop or a deadline reached with outstanding
replies, noise or incomplete framing. The new bit is set only for all-reply,
complete-framing, quiet completion within the existing deadlines; final known
transport loss or unread bytes revoke it. Failed session health never recovers.
Matching hosts must retain revocation and reject old capabilities before Start.

Independent review and 60 native companion tests passed, including clean versus
incomplete autonomous outcomes, clock wrap, fresh Finish and final loss. The same
outcome regression fails against the prior source. A separate API87.1 artifact
was built with source/debug provenance; this revision has no device acceptance
yet. Installed `4ab3c66` and the accepted normal-lifecycle evidence above remain
unchanged. Physical fault and concurrency acceptance remain pending.
