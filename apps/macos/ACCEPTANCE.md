# Native preview acceptance

This checklist is intentionally separate from compilation and model coverage.
The preview is under acceptance, not a signed-off production release.

| Requirement | Evidence | Status |
| --- | --- | --- |
| Shared Rust capture/control boundary | Swift uses the shared Rust CLI client; no Swift scan/policy/database implementation | Implemented |
| Discover/select/default setup | Native candidate controls and native Wi-Fi/BLE defaults; app-model service test | Automated model check passes; user confirmed connection and expandable receiver details on 2026-11-22 |
| Advanced receiver/channel setup | ALFA/CatSniffer fields plus backend-validated JSON/import | Compiles; native interaction check pending |
| Packaged CatSniffer GUI start | User imported the prepared plan and showed Bluetooth running; backend subsequently retained a finished run with 3,629 records | Start/reception and persistence confirmed; pause/resume and close/reopen still pending |
| Durable history/evidence/export | Real-service synthetic replay, retry, page decoding and SQLite export tests | Automated checks pass |
| Client disconnect/reconnect | App-model starts a real service, disconnects, reconnects a new model | Automated model check passes |
| Uncertain start after app loss | Private fsynced intent record; replacement model resolves accepted ID with input file removed | Automated recovery check passes |
| Bounded browsing/live polling | Record window eviction and cursor tests; paged/follow controls | Core checks pass; rendered interaction pending |
| Readiness/recovery/drop distinctions | Unknown-versus-zero tests and preserved backend phases/counters | Automated checks pass |
| Window close leaves capture running | Service lifetime is independent in code/model tests | Actual window acceptance pending; automation access is enabled |
| Packaged backend startup/signing | Local optimized bundle, plist/codesign/embedded-help checks | Passed before latest edits; repeat on final artifact |
| ALFA + CatSniffer capture together | Local packaged-backend attempt on 2026-11-22 | Failed: ALFA MCU timeout; reset/retest required |
| Service connection status/popover | User confirmed clicking the bottom status opens the popover on 2026-11-22 | Passed manual interaction check |
| Native rendered UI/accessibility | Compact predecessor row rendered; current bottom status is available across both tabs | Full capture interaction/appearance check pending; automation intermittently loses window access |

The combined attempt retained 1,355 CatSniffer packets with zero reported host
drops. ALFA produced no observations, reported two recovery attempts and failed.
A separate bounded receive-only diagnostic returned `MCU command 12 sequence 1
timed out`; a read-only register probe succeeded. Those results do not establish
successful combined capture, RF loss rates or ALFA endurance.

Private recordings and device-specific configuration remain outside the repo.
No nearby-device identifiers or raw captures belong in the committed test
fixtures. The regression tests use synthetic replay input and isolated temporary
service databases.

Before preview sign-off: exercise the packaged app with CatSniffer and synthetic
replay, including failure/reconnect cases. Exercise native
Start/Pause/Resume/Finish, browse observations/evidence, close/reopen during capture,
and export/reopen a snapshot. Inspect the final bundle in light/dark appearance,
keyboard focus and a narrow window. Rebuild and verify the artifact after fixes.

ALFA recovery and successful ALFA + CatSniffer capture remain separate open
hardware follow-ups (#155), not prerequisites for this native preview.

The observation-selection regression now covers automatic selection of both live
and completed runs, plus preservation of a manually stopped Follow Live state.
The incomplete-setup regression names missing receiver paths in shared Rust
validation; a configuration error no longer changes the connection indicator.
