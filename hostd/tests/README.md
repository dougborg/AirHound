# Integration fixture timing

These fixtures preserve actual executable argument handling, shared-client
protocol responses, replay EOF behavior and durable SQLite results. Readiness
and completion use causal witnesses rather than a number of sleep/poll attempts.
No fixture opens hardware or an installed service.

`ServiceChild` drains bounded stderr and waits for the operational debug message
`AirHound control socket listening`, emitted only after private socket binding,
listen and permission setup succeed. The fixture enables ordinary debug logging;
there is no test-only environment switch. This message establishes a listening
control socket, not receiver readiness. Socket directory entry creation alone
can precede listen, and macOS cannot register a vnode watch on a socket pathname.
A service exit, oversized diagnostic or missing marker fails with the exact owned
child killed/reaped. JSON stdout remains unchanged.

Replay completion watches the database, WAL and their parent directories before
reading terminal durable state. macOS uses kqueue vnode events; Linux uses
inotify writes/creation/moves, excluding reads. Every wake refreshes inode
identity and registrations before rereading the predicate, including parent
notification before removal of an old inode. It does not wait for actor-active
state using storage events: that state can change after the final durable write.
Marauder's in-process shared-client calls reuse the reviewed paused-clock guard;
actual CLI binaries retain their production request deadlines and response
semantics. Deterministic deadline boundaries are tested separately in the shared
client and adapters, not by slowing an executable fixture.

Child completion uses kqueue process-exit events or Linux pidfd, followed by
`wait`/reap. Unsupported pidfd kernels fail explicitly instead of falling back
to sleep polling. Output is drained while finite replay children run. Each wait
has a separate 120s wall-clock outer hang guard; its expiration is a fixture
failure, never a normal protocol timeout result or performance assertion. An
already-expired injected guard tests the cleanup path without waiting in real
time. Temporary names use PID/atomic sequence rather than wall-clock timestamps.

`fixture_events` covers parent-before-replacement ordering, subsequent in-place
writes, registration/recheck races, output larger than pipe capacity, early
service exit/oversized logs, and timeout cleanup/reaping. All existing CLI,
source-attribution, history and saved-payload assertions remain. The API crate
is pure serialization; Flipper bridge conformance already uses explicit tick
inputs. Hostd runtime unit tests and other repository timing work remain separate
under [#329](https://github.com/dougborg/AirHound/issues/329).
