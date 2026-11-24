# Terminal client

A passive macOS/Linux terminal client for an existing AirHound Rust service.
It uses `airhound-client` and `airhound-api` directly. Receiver discovery, session
validation and policy remain in the backend; this component has no scanner,
planner, USB, BLE or WiFi dependency. It never launches a backend or starts capture.

See the [shared API contract](../../docs/shared-backend-api.md) for service startup
and socket ownership. This client requires an explicit existing Unix socket:

```sh
cargo +stable run --manifest-path clients/tui/Cargo.toml --locked -- \
  --socket /absolute/path/to/private/service.sock \
  --session /absolute/path/to/session.json
```

`--session` is optional. The TUI reads a bounded regular JSON file only when
validation is requested; it sends the document to `session.validate` unchanged.
Relative paths inside that JSON resolve against the **backend's working directory**,
not the session file or terminal client's directory. Successful validation does
not reserve devices or prove firmware, permissions or RF readiness.

## Controls

| Key | Action |
| --- | --- |
| `r`, F5 | Reconnect and refresh status/passive inventory |
| Up/Down, `k`/`j` | Select a receiver |
| Tab | Cycle receiver details, validation result and service status |
| Page Up/Down | Scroll details |
| `v` | Validate the `--session` file through the backend |
| `q`, Escape, Ctrl+C | Detach and exit |

The client requests `hello`, `status`, `devices.list` and `session.validate` only.
It presents backend candidate states and diagnostics without upgrading them to
working-radio claims. Failed refreshes keep the prior inventory labeled stale;
selection follows stable device ID across reordered refreshes. Validation results
are separate from inventory and never imply a live session.

One request operation runs at a time. The shared client enforces a twenty-second
request deadline and 1 MiB framing bound; complete refresh/validation operations
have a thirty-second bound. Session files are limited to the shared API's 64 KiB
limit and opened nonblocking before validating the handle's regular-file type.
Terminal control characters are removed from plain backend labels/errors; raw
JSON escapes them. Closing the client cancels outstanding requests and detaches;
it does not shut down the independently owned service.

Ratatui restores the terminal through its panic hook. This app also restores on
normal exit, returned errors, SIGINT, SIGTERM and SIGHUP. SIGKILL or host failure
cannot perform orderly restoration. It requires interactive stdin/stdout; `--help`
works without a terminal and emits no terminal escape sequences.

## Development and limits

This is an independent Cargo build root and lockfile. Headless tests use Ratatui's
`TestBackend` and a fake Unix-socket service; they never open a user's terminal UI
or radio. The macOS/Linux workflow checks formatting, tests and strict Clippy.
Backend fixtures use [controlled test time](../TESTING.md); production request
and operation deadlines remain unchanged.

```sh
cargo +stable test --manifest-path clients/tui/Cargo.toml --locked
cargo +stable clippy --manifest-path clients/tui/Cargo.toml --locked --all-targets -- -D warnings
```

The first slice is inventory and validation only: no live observations, session
start/stop, events subscription, history or export viewer. Windows transport is
unsupported. Headless tests pass on macOS and Linux; interactive terminal/
assistive-technology acceptance remains separate. Ratatui was selected for its
maintained Rust implementation, terminal restoration support and headless backend;
see its [initialization contract](https://docs.rs/ratatui/0.30.2/ratatui/fn.init.html)
and [testing backend](https://docs.rs/ratatui/0.30.2/ratatui/backend/struct.TestBackend.html).
