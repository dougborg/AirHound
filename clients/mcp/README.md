# AirHound MCP adapter

A stdio MCP server for a separately running AirHound backend on macOS or Linux.
It uses the same [Rust client](../../hostd/crates/airhound-client) as other desktop
presentations. Matching, policy, device discovery and session planning stay in the
backend. This experimental slice supports passive queries, not capture control.

| Tool | Backend operation | What it establishes |
| --- | --- | --- |
| `backend_status` | `status` | Backend-reported state; a passive service is not an active scan |
| `list_devices` | `devices.list` (`all: false`) | Passive candidate inventory, including its reported uncertainty |
| `validate_session` | `session.validate` | Configuration and local resource checks; readiness remains unverified |

All three tools have read-only annotations. There are no capture start/stop,
receiver opening, shell, firmware, or filesystem-writing tools. Session JSON is
forwarded as an object: the backend owns its schema and validation. Relative
paths in that JSON resolve against the **backend's working directory**. Results
preserve backend fields, including unknown fields and readiness qualifications.

## Build and connect

From the repository root:

```sh
cargo +stable build --manifest-path clients/mcp/Cargo.toml --locked --release
```

Start the passive backend separately with an explicit private socket path:

```sh
airhound-hostd serve --socket "$HOME/.airhound/control.sock"
```

The socket's immediate parent must be owned by your user with mode `0700`; the
backend creates it if absent, provided its own parent exists. The backend checks
same-user peers and creates the socket with mode `0600`. An existing socket is
not silently replaced. See the [shared backend contract](../../docs/shared-backend-api.md).

Configure your MCP client to launch the compiled adapter with absolute paths:

```json
{
  "mcpServers": {
    "airhound": {
      "command": "/absolute/path/AirHound/clients/mcp/target/release/airhound-mcp",
      "args": ["--socket", "/absolute/private-directory/control.sock"]
    }
  }
}
```

The configuration wrapper varies between MCP clients. The adapter does not
install itself, start the backend, or discover a socket automatically. Standard
output is reserved for MCP; startup diagnostics use standard error. Running it
alone waits for a client to initialize the MCP session.

Example `validate_session` arguments:

```json
{
  "config": {
    "version": 1,
    "receivers": [
      {"id": "saved", "source": {"kind": "wifi_pcap", "path": "/captures/test.pcap"}}
    ]
  }
}
```

The accepted source kinds depend on the connected backend version. Validation
does not replay the capture or prove that a receiver works.

## Bounds and failures

The official [Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
handles MCP framing and lifecycle; this package pins released `rmcp` 3.11.0.
Incoming MCP lines are limited to 128 KiB and session JSON to the shared API's
64 KiB limit. The shared client rejects backend frames larger than 1 MiB before
allocation. Results include structured JSON plus the SDK's equivalent text form;
no truncation silently removes devices or evidence. Backend errors retain their
code and retryability, with messages limited to 2,048 characters.

Each tool opens a fresh shared-client connection and performs its handshake.
One backend request runs at a time; concurrent calls return a
`busy` error instead of queueing; retry after the current request finishes. Each client stage has a 20-second deadline and
the entire tool operation a 25-second deadline. Backend failure is an error,
never an empty inventory or an invented stopped/ready state. Cancelled calls drop
their connection; later calls reconnect.

## Development

```sh
cargo +stable test --manifest-path clients/mcp/Cargo.toml --locked
cargo +stable clippy --manifest-path clients/mcp/Cargo.toml --locked --all-targets -- -D warnings
cargo +stable fmt --manifest-path clients/mcp/Cargo.toml --check
```

The separate Cargo root and lockfile keep MCP dependencies out of firmware and
hostd. CI runs on Linux and macOS. SDK-driven integration tests launch only this
stdio adapter and a fake local backend: initialization, tool metadata, forwarding,
unknown fields, backend errors and input bounds are synthetic checks. They do not
establish acceptance in a particular MCP application or any live radio support.

Follow-up capture controls and event subscriptions depend on the shared backend
contract in [#238](https://github.com/dougborg/AirHound/issues/238); this adapter is
tracked by [#240](https://github.com/dougborg/AirHound/issues/240).
