# Native macOS app

AirHound's SwiftUI interface is an early, source-built component for macOS 14 or
later. It currently browses recorded observations and inventories candidate
devices through the Rust backend. It does **not** start or control a capture.
Linux GUI work is deferred; `hostd` remains the shared macOS/Linux backend.

## Build and run

Install Xcode command-line tools with Swift 5.9 or later. No third-party Swift
packages are required.

```sh
cd apps/macos
swift test --enable-code-coverage
sh bundle.sh
open dist/AirHound.app
```

The bundle is locally ad-hoc signed, not notarized or a release distribution.
Its identifier is `tools.airhound.desktop`, distinct from the existing hostd
permission bundle. Packaging does not copy a backend or radio firmware. The
`.app` includes its example recording and can open recordings without hostd.

To build the backend, run `just build-hostd` at the repository root. In the app,
open **Devices**, choose `hostd/target/release/airhound-hostd`, then click
**Refresh**. A backend at `AirHound.app/Contents/Helpers/airhound-hostd` or a sibling
`airhound-hostd` executable is located automatically if present. A discovered
executable is not run automatically. Choose a trusted backend: the operating
system executes that file directly, without a shell.

## Devices

The app invokes only `airhound-hostd devices list --json`, using the existing
[passive discovery contract](../../docs/device-discovery.md). It displays device
identities, endpoints, support/readiness distinctions, requirements and inventory
diagnostics. Presence is not readiness, and a partial inventory is not proof that
a missing device is absent. The app does not probe serial interfaces, claim USB
devices, request radio permissions, or start scanning. Refresh replaces the prior
snapshot; Cancel stops the owned discovery command.

Inventory execution has a 15-second deadline, 4 MiB stdout cap and 64 KiB stderr
cap. Cancellation, output overflow or timeout requests termination, followed by
SIGKILL after 250 ms if needed; the direct child is reaped before completion.
Both pipes use checked nonblocking descriptors, including after child exit, so
an inherited writer cannot make a pipe read hang. Close/Quit cancels and awaits
the owned command. This ownership adapter does not implement scan-stop semantics
or claim RF cleanup; it only runs the passive inventory command. An arbitrary
replacement executable is not sandboxed, and descendants are not managed as a
scan session.

## Recordings

**Open Recording** (Command-O) reads hostd NDJSON. **Open Example Recording**
loads three explicitly illustrative observations; it is never a live session.
The table supports sorting, receiver/protocol filtering, matching-only filtering
and text search. Select a row for complete identity, channel/PHY, detection
evidence and original JSON. Option-Command-I toggles the inspector.

Native WiFi, native BLE, BLE packet observations (including CatSniffer/Sniffle)
and external WiFi observations (including ALFA) are represented. Runtime/health,
legacy match and unknown record types are counted but not treated as current
live state or duplicate observations. Missing signal and unvalidated ALFA RSSI
stay unknown; estimates remain available in raw JSON. BLE platform identifiers
are not converted to MAC addresses. Band and frequency are shown only when
explicitly supplied, never inferred from an ambiguous channel number.

The frontend displays the backend's detection metadata; it does not execute
signatures, allocate channels or decide alert policy. The inspector preserves
the complete detection object, including rule/signature references, policy,
provenance and overflow fields. A match is a lead, not confirmed device identity.

Recording import is atomic and bounded to 8 MiB, 256 KiB per line and 10,000
observations. Malformed/truncated JSON is rejected rather than silently skipped;
the previous recording remains open on failure. Imports are not tailing/replay,
and no export or persistent capture archive is written by this slice.
Selected recordings are opened nonblocking and verified as regular files using
the opened descriptor, so FIFOs and devices cannot block import. File reading
and decoding run off the UI thread; opening another recording is disabled until
that import completes.

## Structure and next slice

- `AirHoundCore`: Foundation presentation decoders and bounded subprocess adapter.
- `AirHoundApp`: SwiftUI/AppKit application, native table/sidebar/inspector and dialogs.
- `Tests`: synthetic contract, bounds, timeout, cancellation and failure tests.

The Rust backend remains the owner of session validation, allocation, detection,
alert policy and capture/control semantics. The next slice should expose those
operations through a shared control boundary usable by this app, a TUI and MCP;
do not duplicate them in Swift. Start/Stop and session configuration are absent
until that boundary and shutdown acknowledgement are implemented. No background
capture mode is enabled.

CI builds/tests/packages without radios or permissions. Local compiler, unit
tests and bundle verification are separate from visual, keyboard, VoiceOver and
hardware acceptance; those remain manual checks. Current comparison windows are
not replaced or launched by the build.
