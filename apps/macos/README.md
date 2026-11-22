# Native macOS app

AirHound's SwiftUI preview controls the shared Rust capture service and browses
its durable recordings. The Rust backend owns discovery, scan-plan validation,
receiver management, detection, alert policy and SQLite storage. Swift presents
those results through native controls. Linux GUI work remains deferred.

## Build and run

Use Xcode command-line tools with Swift 5.9 or later and stable Rust. From the
repository root:

```sh
cargo +stable build --manifest-path hostd/Cargo.toml --release --locked --bin airhound-hostd
AIRHOUND_TEST_BACKEND="$PWD/hostd/target/release/airhound-hostd" \
  swift test --package-path apps/macos --enable-code-coverage
sh apps/macos/bundle.sh "$PWD/hostd/target/release/airhound-hostd"
open apps/macos/dist/AirHound.app
```

The bundle is ad-hoc signed for local testing, not Developer ID signed or
notarized. Its identifier is `tools.airhound.desktop`. The optional argument to
`bundle.sh` embeds that exact backend executable inside **AirHound Service.app**
(`tools.airhound.service`), with its own signed privacy usage descriptions.
The GUI starts this application through LaunchServices and waits for the shared
service socket; short-lived client commands use its executable directly.
Omitting the argument builds an app that
can browse NDJSON and lets you select a trusted backend manually. External radio
helpers, their runtimes and firmware are not bundled. The native UI targets
macOS 14; an embedded backend may require a newer version depending on its SDK
and deployment target. See the packaged backend's platform requirements.

For native radio testing, install the generated AirHound.app at a stable location
such as `~/Applications/AirHound.app`, then open that copy. Temporary worktree
paths can be excluded from application lookup by LaunchServices even when the
process launches. Keep the same installed copy while testing permissions; do not
overwrite a bundle whose service is still running. The standalone command-line
scanner retains its existing `com.airhound.hostd` identity.

Local ad-hoc signatures use a binary hash as their designated requirement.
Rebuilding the app or backend can invalidate a previous privacy grant even when
the identifier and install path stay the same. A stable developer signing
identity is needed for reliable permission continuity across builds; this
preview does not weaken signature requirements or reset privacy settings.

## Connect and capture

1. Open **Capture**, then click the service status at the bottom to open connection
   settings. Choose a trusted backend if one is not embedded.
2. Click **Start Local Service**, or **Connect** if a service already exists.
   Starting a service does not start scanning. An existing service keeps its
   current executable and permissions until it is explicitly stopped; installing
   a newer GUI does not replace the capture owner.
3. Select receivers. System Wi-Fi and Bluetooth form the initial suggested plan;
   selection does not start scanning or prove permission/readiness.
4. Expand a receiver row to inspect identification evidence, requirements and
   **Receiver settings**. ALFA needs its
   scanner executable, firmware and exact USB selector. CatSniffer needs compatible
   Sniffle firmware, its serial port, Python runtime and helper/module paths.
5. **Validate Plan**, then **Start Recording**. Advanced settings also accept a
   complete JSON plan or a plan file, including replay sources.
6. Use **Pause**, **Resume**, and **Finish Recording** to control the service.

Receiver/channel fields serialize configuration; the Rust backend validates it.
A successful validation is not a radio probe. The recording state describes the
requested lifecycle; per-receiver readiness can lag or fail independently.
Reported recovery/drop counters are displayed separately. Missing counters are
**Not reported**, never zero. Radio loss is generally unknown. Metrics are read
from retained source events, with a visible catch-up indicator when behind.

**Closing the window or quitting the app leaves the service and capture running.**
Reopen and Connect to regain control. Finish Recording explicitly stops the run
and drains its retained data. Pause may stop ingestion without turning off the
radio firmware. A failed finish remains a failed recording and is not labeled a
successful capture. A pending start ID/config is flushed to a private file beside the service socket before dispatch;
**Resolve Pending Start** retries that same request so a lost response cannot
silently create another recording. Do not change the plan while resolving it.

The default service socket and database live in
`~/Library/Application Support/AirHound/`. The directory must be private (0700),
owned by you and not a symlink. The service validates local socket ownership;
the app never removes an existing socket to force startup. Advanced connection
settings can attach to another local socket. No network listener is enabled.
The preview does not install a login agent or automatically restart a crashed
service. After a service restart, unfinished recordings become interrupted and
are not automatically resumed.

## Observations and history

Open a recording from Capture to inspect it in **Observations**. Follow new
records for a live run or use **Load Next Page** to browse forward and **From Beginning** to return to the start. The view keeps
up to 1,000 records and 4 MiB of serialized payloads; older records leave the
presentation window, with an explicit omitted count. This does not remove them
from SQLite. Refresh the history list for updated run counts and use **Older
Recordings** for further history pages.

Sort/filter observations by receiver, protocol, text or match. The inspector
retains the backend's signature/evidence object and original JSON. Native Wi-Fi,
BLE advertisements, CatSniffer BLE packets and ALFA observations are supported.
Unknown events and lifecycle records are not presented as duplicate observations.
Missing signal and unvalidated ALFA RSSI stay unknown. BLE platform identifiers
are not converted to MAC addresses. A match is evidence, not confirmed identity.

**Export Archive** writes a consistent standalone SQLite snapshot of all stored
runs to a new filename. It does not overwrite files. An active run in an export
reflects its state at snapshot time. Backend size/time limits apply; this is not
a CSV export or a retention operation.

Command-O opens bounded NDJSON files (8 MiB / 10,000 observations), and the
example is explicitly illustrative. Opening a file stops following a service
recording in the view; it does not stop capture.

## Architecture and verification

- `AirHoundCore`: bounded process adapter, shared-client presentation models,
  evidence/history decoding, bounded record window and file import.
- `AirHoundApp`: native views and presentation state. Every service operation uses
  `airhound-hostd client`, which uses the shared Rust API/client and Unix socket.
- `Tests`: model/bounds tests and optional real-service replay/lifecycle tests.
  Set `AIRHOUND_TEST_BACKEND` to enable the latter; CI builds and supplies it.

Each client subprocess has a 25-second deadline and bounded output. Cancelling
that subprocess does not cancel an already accepted service command. Reconnect
and query state after uncertain outcomes. The service process has no pipes tied
to the app and is not terminated on app disconnect.

Hardware and visual acceptance are separate from compiler/unit tests. Packaged
CatSniffer capture, GUI pause/resume/finish, actual window close/reopen, retained
observation browsing, synthetic replay/failure display and SQLite archive export
have been exercised locally. ALFA MCU recovery and combined ALFA/CatSniffer
acceptance remain separate hardware follow-ups. Dark-mode rendering was checked;
full VoiceOver and light-appearance review are not claimed. See ACCEPTANCE.md for
the evidence and limits.

When no recording is open, switching to Observations selects the service’s active
run or most recently completed run. Active runs enable Follow Live; an explicitly
opened recording or example is preserved. Missing helper paths are setup errors,
not evidence that the service has disconnected.
