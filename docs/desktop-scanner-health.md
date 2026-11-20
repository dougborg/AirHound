# Desktop scanner health

The NDJSON stream includes additive `scanner_health` records on state changes
(at most five seconds after a quiet adapter changes), and with `GetStatus`.
The existing status response and command acknowledgment remain available.

```json
{"type":"scanner_health","schema_version":1,"scan_requested":true,"scanners":{"ble":{"state":"running","reason":"scan_started"},"wifi":{"state":"blocked","reason":"location_permission_denied_or_disabled"}}}
```

`scan_requested` describes the user's requested state. Each scanner separately
reports `disabled`, `initializing`, `running`, `paused`, `blocked`, `unavailable`
or `stopped`. Running means the adapter started scanning or completed a scan;
it does not prove that observations were received. Reasons are diagnostic codes,
not device identifiers. Consumers should tolerate additional reasons.

macOS WiFi reports denied/restricted Location Services or disabled location
services as blocked, and clears cached coordinates after revocation. A scan with
only redacted BSSIDs also reports blocked with a distinct diagnostic reason.
The scanner retries and returns to running when access recovers. Pause/resume
commands do not clear permission failures. BLE initialization has a ten-second
budget and observes shutdown; timeout reports unavailability and asks the user
to check Bluetooth permission/power, rather than asserting a known denial.

The macOS profiler now requires observations from each selected radio for a
successful normal run. `--allow-empty` permits a diagnostic run in a quiet or
permission-blocked environment but leaves `reception_verified` false. Scanner
shutdown that requires task cancellation fails either mode. Metrics retain the
latest health snapshot. Simulated tests cover the state and acceptance policy;
manual denied/revoked/granted permission transitions remain tracked in #159.
