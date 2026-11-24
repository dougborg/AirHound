# Shared-client and presentation test timing

The shared Rust client, TUI backend fixtures and in-process MCP SDK integration
use paused Tokio time. Request arrival and response release use channels rather
than sleeps or scheduler-yield counts. The test-only `ManualTime` guard in
`hostd/crates/airhound-client/src/test_clock.rs` holds one blocking task on a
channel: pinned Tokio 1.54.1 explicitly inhibits automatic clock advancement
while that task runs. This keeps real Unix socket and file readiness from racing
virtual deadlines. The guard releases and joins the task on normal completion;
its destructor also releases the task on panic or cancellation. A separate 120s
wall-clock receive watchdog aborts the isolated test process if the fixture hangs.
It never releases virtual time to turn a stalled fixture into a normal timeout
result. Ordinary `cargo test` otherwise has no per-test hang bound. This guard is
not part of production builds.

Deadline tests explicitly advance to just before and then exactly to the bound.
They retain timeout/error identity, poisoned-connection refusal, peer EOF and
fresh-connection-after-cancellation assertions. MCP tests distinguish its 20s
client-stage deadline from its 25s overall deadline, including time spent in the
handshake. TUI passive refresh and unchanged JSON forwarding run without virtual
time advancement; FIFO rejection checks the actual nonblocking regular-file
error, not completion within a wall-clock second.

The real MCP executable smoke retains stdio initialization and the passive tool
catalog. The argument parser test checks the explicit socket path, and SDK
forwarding tests bind that selected path to a synthetic backend. Backend request
correctness tests run in-process so the test controls their Tokio clock; no test
clock environment variable changes the deployed executable. The executable smoke
retains a 30s outer hang guard for a stuck OS process/transport. That guard is not
a deadline correctness or performance assertion. Other test runner/CI timeouts
also remain outer hang guards. No fixture opens radios or an installed service.

This audit covers `hostd/crates/airhound-client`, `clients/tui` and `clients/mcp`.
Hostd receivers/process supervision, native Swift presentation, firmware/drivers,
and physical acceptance are separate parts of the repository timing work in
[#329](https://github.com/dougborg/AirHound/issues/329).
