# Shared backend control API

The local Unix-socket service supports passive inventory/validation, durable
recording history and opt-in service-owned capture through shared Rust API and
client crates. The [TUI](../clients/tui/README.md) and
[MCP adapter](../clients/mcp/README.md) currently use the passive query subset.
The CLI also exposes history and capture controls. Event subscriptions, remote
phone access (#246), automotive displays (#247), and native UI attachment remain
subsequent work. Sections marked **planned** describe those future capabilities.

## Run the passive service

Build from the repository root with `just build-hostd`. Then, in separate terminals:

```sh
# Use an absolute socket path. Its immediate parent is created mode 0700 if absent.
# The parent's parent must already exist; an existing immediate parent must be
# owned by this user, nonsymlink and exactly mode 0700.
hostd/target/release/airhound-hostd serve --socket /tmp/airhound-control/control.sock

hostd/target/release/airhound-hostd client --socket /tmp/airhound-control/control.sock status
hostd/target/release/airhound-hostd client --socket /tmp/airhound-control/control.sock devices
hostd/target/release/airhound-hostd client --socket /tmp/airhound-control/control.sock validate --session-config session.json
```

All client results are JSON. A backend validation error is a typed JSON error
and nonzero CLI exit; connection/framing errors also exit nonzero. The status
result advertises `mode: passive_control` and `capture_control: false`; it does
not claim any separate running scanner is stopped or managed by this service.

The client sends config JSON, not a server pathname. Paths inside that config
resolve relative to the **backend's working directory**, just like existing
hostd session configuration. Prefer absolute source/helper paths. Syntax/size
checks at the client protect transport; only the backend resolves/validates the
plan, using the same function as `session validate`. Successful validation does
not reserve devices, open receivers or establish runtime permission/readiness.

The service supports 16 simultaneous local clients. Frames are capped at 1 MiB,
configuration at 64 KiB, an incoming frame (including idle wait) at 30 seconds,
a backend operation at 15 seconds and response writes at 5 seconds. The CLI uses
a 20-second request deadline. The shared client poisons/closes a connection after
cancellation, timeout or protocol mismatch, preventing late response confusion;
callers reconnect before further requests. Capture start requires a durable
application-level request ID, separate from the per-connection envelope ID. Retry
with the same ID and configuration to retrieve the original run. The client does
not automatically retry mutations.

Ctrl-C/SIGTERM shuts down passive requests and removes only the socket inode
created by that service. An existing path, including a stale socket, is never
unlinked automatically. After a crash, establish that no service owns the socket
before removing it manually. Disconnecting one client does not stop the service.
Without `--capture`, the service cannot start radios. Capture-enabled shutdown
stops owned receivers and drains recording before exit. Installation/autostart,
TCP listeners and network authentication remain unimplemented.

### Current wire contract

Each frame is a 4-byte big-endian length followed by JSON. Example request:

```json
{"version":1,"request_id":1,"command":{"method":"hello","params":{}}}
```

Send hello first; request IDs must be nonzero and strictly increasing per
connection. Replies contain `version`, matching `request_id` and exactly one of
`result` or `error`. Errors have typed `code`, `message` and `retryable` fields.
A malformed frame without a usable request ID may receive error ID zero or be
closed. Unsupported methods and capture requests on a passive service return
`unsupported_method`, never an implicit fallback. Unknown fields are rejected.

The shared client exposes `Client::connect(path, deadline)`, `hello`, and async
`status()`, `devices(all)`, `validate(config)`, `recordings(before, limit)`,
`recording_records(run_id, after, limit)` and `capture(action)` calls. Framing is generic over
asynchronous IO; only Unix-stream connection is implemented. Future remote
transports must add authentication and authorization before exposing this API;
a phone/web/CarPlay/Android Auto view must remain a client of backend state, not
another scanner/policy engine.

The initial command/error/hello envelopes are typed, while status, inventory and
validation results deliberately retain `serde_json::Value`: discovery and preview
already emit versioned backend payloads, which this slice forwards unchanged.
Frontends may render those fields but must not infer readiness, validate plans or
reapply policy. Shared typed result DTOs are the next contract refinement, so
TUI/MCP do not grow independent semantic interpretations. Their schema evolution
must remain compatible with the existing passive CLI payloads.


## Durable capture and history

Use `serve --capture --database PATH --socket ABSOLUTE_PATH` to enable explicit
capture requests. Attaching alone starts nothing. One active run can contain
multiple configured receivers. Without `--capture`, `--database` exposes only
read-only history and never recovers or changes an active run.

The `capture` method accepts a typed `action`:

| Action | Parameters | Result |
| --- | --- | --- |
| `start` | `request_id`, `config` | Durable run ID; acceptance does not imply receiver readiness |
| `status` | None | Active run/requested state and receiver health, or last completed result |
| `pause` / `resume` | `run_id` | Requested state, acknowledged after its database transaction |
| `finish` | `run_id` | Terminal result after receiver cleanup and recording commit |

For example, the start command inside a framed request is:

```json
{"method":"capture","params":{"action":{"action":"start","request_id":"trip-2026-11-22","config":{"version":1,"receivers":[{"id":"ble","source":{"kind":"native_ble"}}]}}}}
```

The nested action discriminator is part of the typed `CaptureAction` contract.
The envelope still supplies its own increasing numeric `request_id` and version.
Start IDs are persisted with the original configuration. Reusing one with changed
configuration fails; reuse after a lost reply or service restart returns the
original run rather than starting radios again. A capture-enabled service recovers
abandoned runs as interrupted on startup; it does not resume them automatically.

Requests are serialized by a bounded service-owned actor. Cancelling a client
request does not cancel an accepted action. After timeout, query state/history:
cleanup may still be running. Repeated pause/resume of the same requested state
is harmless, and repeating finish for the most recently completed run returns
its result. Finish for older/non-owned runs is rejected; durable history remains
available. A failed capture returns terminal `state: failed`; inspect the result
instead of interpreting a successful RPC exchange as successful RF capture.

Pause means a requested scan pause; individual receiver health/acknowledgements
supply readiness evidence. Stop drains accepted output, but does not establish
zero RF loss. Queue overflow fails recording explicitly. Normalized observations,
evidence, source tags and available loss counters are retained without inferring
missing identities or timestamps.

`recordings.list` uses `before`/`limit` and returns `runs`, `next_before`,
`has_more`. `recordings.records` uses `run_id`/`after`/`limit` and returns
`records`, `next_after`, `has_more`. Continue only while `has_more` is true.
Run limits are 1–100; record limits are 1–500, with independent byte bounds.
History readers do not own receivers or recover runs. The service capture path
currently writes SQLite, not a parallel CSV or live stdout stream; the standalone
capture CLI can retain SQLite alongside its JSON/CSV output. Explicit gpsd
configuration through the service and live event subscriptions remain future work.

See [hostd commands](../hostd/README.md#durable-recording) and the
[acceptance record](storage-acceptance.md) for tested scope and remaining gates.

## Capture ownership and remaining frontend work

One Rust backend instance owns radio transports, resource reservations, session
plans, channel allocation, detection/evidence, alert policy, location association,
exports and cleanup. Frontends submit intent and present the returned state.
There must not be a Swift, terminal or MCP implementation of those rules.

The persistent service can have several attached clients at once. A CLI command
can inspect or control the same capture shown by SwiftUI or a TUI. Closing an
attached client detaches it; it does not implicitly stop someone else's session.
Stopping capture is an explicit backend operation. Shutting down the service is
a separate explicit operation that first stops owned capture and flushes exports.

A GUI may offer a temporary, exclusively owned backend mode later, but the
ownership relationship must be explicit in both handshake and UI. That mode's
close/Quit action requests stop/shutdown and awaits backend cleanup; it cannot
reuse the first slice's passive `DiscoveryProcess` collector for radio lifetime.
Persistent service installation/autostart is not part of the first API slice.

## Component boundaries

Start inside the existing hostd build root, avoiding a disruptive repository-wide
workspace migration or dependency changes to the no_std firmware package:

| Component | Path / status | Owns |
| --- | --- | --- |
| Typed protocol crate | `hostd/crates/airhound-api` | Versioned request/response envelopes, error codes and hello; result DTOs/events are planned. No transport, radios or policy implementation. |
| Shared client crate | `hostd/crates/airhound-client` | Unix-socket framing, handshake, request correlation, deadlines and typed errors; subscriptions/gap detection are planned. Depends on api; no frontend rendering or hardware. |
| Backend service modules | `hostd/src/service.rs`, `service_cli.rs`, `capture_service.rs` and `storage/` | Connections, permissions and dispatch; canonical preview/discovery, capture actor and durable history. Live event subscriptions are planned. |
| CLI adapter | Existing hostd CLI first, optional `airhound` binary later | Arguments, typed client calls, human/JSON rendering and exit codes. |
| TUI adapter | `clients/tui` (independent Cargo root) | Terminal state and views consuming the same client. No process scraping. |
| MCP adapter | `clients/mcp` (independent Cargo root) | MCP tool/resource schemas, explicit operation calls, summaries and subscriptions through the same client. No duplicated radio/session logic. |
| SwiftUI adapter | `apps/macos` | Thin framed-protocol adapter and presentation; generated/fixture-checked wire contract rather than a parallel planner. |

Do not extract all of hostd into new crates in one change. Move shared wire DTOs
only when a second caller needs them. Keep canonical `SessionConfig::resolve`,
platform checks and resource conflict checks on the backend side. The implemented
`session validate` command and API `session.validate` must call the same function.
Whether session DTOs initially live in the existing hostd library or the API
crate should be decided by dependency direction, not by reimplementing them.

The GPL MT7610U helper remains a subprocess behind the Rust backend; neither API,
client nor UI links its implementation. External-scanner identity and lifecycle
checks remain in the existing Rust supervisor.

## Transport and local access (implemented constraints)

Use a Unix-domain socket for the initial macOS/Linux transport. An explicit
socket path is required by service and clients; there is no implicit default.
Never use a world-writable socket without private parent-directory protection. Directory mode 0700 and socket mode 0600 are required. Check peer UID
where supported; reject a mismatched user. Do not add a TCP listener in this slice.

Bind fails closed when the selected endpoint is occupied. Do not unlink a path
just because connect failed: prove a stale socket belongs to this user/service,
or require explicit recovery. Service PID files alone do not prove identity.
Permissions prevent other users connecting; they are not a sandbox for processes
already running as this user. MCP uses this same access boundary rather than an
unrelated privileged radio-control path.

Use length-prefixed UTF-8 JSON frames (4-byte big-endian byte length) with a
fixed v1 cap of 1 MiB, advertised in hello. This avoids ambiguous
partial lines and allows bounded decoder tests. Existing NDJSON capture files
stay NDJSON; the service envelope is not a replacement for exported records.
Protocol major version 1 is checked during the required hello handshake. Unknown request methods
return a typed error, not an implicit fallback to a different operation.

## Planned capture requests and responses

Requests use the versioned envelope above, with `command.method` and `command.params`. Responses carry the same
ID and either `result` or an error `{code, message, details, retryable}`. Never
parse prose to decide whether retry or cleanup succeeded. Wire error codes cover
invalid request/config, incompatible protocol, unsupported platform, conflict,
permission denied, unavailable device, stale revision, timeout and internal error.

First methods, implemented incrementally:

1. `hello`: protocol/capabilities, backend instance UUID and build version.
2. `devices.list`: the current passive inventory contract and diagnostics.
3. `session.validate`: resolved plan plus explicit warnings/requirements; no
   transport initialization. Successful validation is not runtime permission or
   RF acceptance and does not reserve a device.
4. `session.status` and `events.subscribe`: authoritative snapshot and events.
5. `session.start`: backend revalidates and atomically reserves resources before
   starting receivers. No client may assume an earlier validation is a lease.
6. `session.pause`, `session.resume`, `session.stop`: existing Rust control
   semantics, explicit acknowledgements and final cleanup state.
7. `service.shutdown`: explicit administrative operation, separate from detach.

One active capture session per service is sufficient initially; it can already
contain multiple receivers. Additional independent sessions should follow only
after cross-session resource reservations are tested. Multiple clients do not
require multiple radio sessions.

Mutations include the expected session identity/revision. A stale client gets a
conflict and the current revision instead of overwriting newer state. Return a
state transition/operation ID for asynchronous starts/stops; command acceptance
does not mean radios are ready or stopped. `session.stop` is idempotent and can
preempt ordinary command work; telemetry queues cannot prevent stop handling.

On a lost connection after sending a mutation, the client must query the backend
before retrying. The implemented Start receipt is durable across backend restarts and keyed by
application request ID plus exact configuration. Additional mutation revisions,
operation IDs and subscription resynchronization remain planned; clients must
handshake and query authoritative state after reconnect.

## Planned capability-based nodes and phone gateways

A node can independently contribute location, radio observations, receiver tuning,
and control/presentation. Do not assign a single fixed role based on whether the
node is a phone, embedded detector or desktop. A phone may be GPS-only, capture-only,
a remote control, or a gateway for attached M5/Flipper/other receivers.

Advertise capabilities per source, including supported protocols/bands/channels,
fixed-channel versus OS-managed scanning, passive versus active discovery,
clock/timestamp semantics, and transport/queue loss reporting. The backend assigns
channels or other slices only when the source can accept and acknowledge them;
unavailable tuning is never silently treated as successful. Preserve the physical
receiver identity separately from the forwarding phone and network connection.

Location fixes carry source, time, accuracy and freshness. Associate a phone's
location with another receiver only through an explicit co-location relationship;
receiving a remote stream does not establish that both devices are in one place.
Reconnection/replay must retain original timestamps and report missing intervals.

The first proposed iPhone gateway transport is CoreBluetooth GATT to existing
firmware: observations/commands on the device link, optional phone location and
presentation, and authenticated forwarding to another backend later. External
receiver addresses remain payload data, independent of the iPhone's own Bluetooth
peripheral identifiers. BLE notification throughput, fragmentation, reconnects and
background execution require acceptance testing. Generic USB serial is not an
assumed iPhone capability; Android USB host adapters and supported iPad DriverKit
paths are separate platform integrations. None of these phone roles is implemented
by the initial local Unix-socket service. Track mobile work in #246.

## Planned events, replay and loss

Events carry backend instance UUID, monotonically increasing event sequence,
session ID (when applicable), type and payload. Preserve the existing
receiver/source ingress identity in observation payloads. Service event order is
not a hardware timestamp or RF delivery guarantee.

Subscription returns a snapshot with its sequence watermark and then events
after that watermark, avoiding a snapshot/subscribe race. A bounded ring allows
short reconnects with `after_sequence`. If history is gone, return an explicit
gap and require a new snapshot. The shared Rust client implements this once for
CLI, TUI and MCP; SwiftUI follows the same tested wire semantics.

Each subscriber has a bounded queue. Slow clients may lose presentation events
but must receive an explicit gap/overflow indication; one client cannot block
capture, export or another client. Keep command responses/control notifications
on a path that cannot be starved by observation traffic. Disconnect a peer that
cannot consume required control state within the configured deadline.

Backend export owns durable recording and flush acknowledgement. UI row eviction,
subscription gaps, backend ingress drops, producer drops and unobservable RF loss
are distinct quantities. Never call a capture lossless merely because client/UI
queues show zero drops. Replay/import observations retain provenance and recorded
timestamps; attached clients must not silently present them as live RF captures.

## Planned capture shutdown and acceptance

The backend publishes stopping, receiver cleanup outcomes, export completion and
finally stopped. Stopped-with-verified-cleanup requires both receiver termination
and export completion, not just a successful command write or child exit. Forced
termination leaves cleanup unverified and visible. Client detach needs no capture
cleanup and must not claim one occurred.

Implement and test the service against fake receivers first. Acceptance includes
two clients seeing the same session; mutation conflicts; disconnect/reconnect
without duplicate Start; snapshot ordering; output floods and slow clients;
bounded framing; stop under saturated telemetry; failed exports; backend restart;
socket permission/stale-endpoint behavior; and version incompatibility. Then
exercise the same operations via CLI and a thin SwiftUI adapter. Hardware tests
remain separately coordinated and never occur just because a client attaches.

## Implementation sequence

1. Backend `session validate` with CLI JSON result and shared validation entry
   point (root agent's current precursor).
2. API/client crates plus service hello, passive discovery, validation and typed
   errors; two-client fake transport tests. Existing capture CLI unchanged.
3. Service-owned capture, snapshot/subscription, reliable stop/export lifecycle
   and attachable CLI controls, tested with fake and offline sources.
4. SwiftUI session setup/control through this boundary, then the TUI. MCP can
   proceed in parallel once the shared client contract is stable.
5. Hardware acceptance, permission attribution, signed packaging and explicit
   persistent/background-service UX before enabling background capture by default.
