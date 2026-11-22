# Flipper live producer (development)

This separate `airhound-live-bridge` target implements the device side of the
[AHFB protocol](flipper-bridge-protocol.md). It has not been installed or accepted
on hardware. The diagnostic application's successful 85-packet file acquisition
is evidence for its UART/parser foundation, not live RPC acceptance.

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
hashes and an explicit `hardware_tested: false`. Required next gates are independent
runtime review, paired host/device fault tests, actual UART-free RPC load/exit and
memory checks, then separately authorized passive capture and watchdog/stop tests.
The installed diagnostic and original Marauder companion remain unchanged.
