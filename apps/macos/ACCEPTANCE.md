# Native preview acceptance

This checklist is intentionally separate from compilation and model coverage.
The preview is under acceptance, not a signed-off production release.

| Requirement | Evidence | Status |
| --- | --- | --- |
| Shared Rust capture/control boundary | Swift uses the shared Rust CLI client; no Swift scan/policy/database implementation | Implemented |
| Discover/select/default setup | Native candidate controls and native Wi-Fi/BLE defaults; app-model service test | Automated model check passes; visual check pending |
| Advanced receiver/channel setup | ALFA/CatSniffer fields plus backend-validated JSON/import | Compiles; native interaction check pending |
| Durable history/evidence/export | Real-service synthetic replay, retry, page decoding and SQLite export tests | Automated checks pass |
| Client disconnect/reconnect | App-model starts a real service, disconnects, reconnects a new model | Automated model check passes |
| Uncertain start after app loss | Private fsynced intent record; replacement model resolves accepted ID with input file removed | Automated recovery check passes |
| Bounded browsing/live polling | Record window eviction and cursor tests; paged/follow controls | Core checks pass; rendered interaction pending |
| Readiness/recovery/drop distinctions | Unknown-versus-zero tests and preserved backend phases/counters | Automated checks pass |
| Window close leaves capture running | Service lifetime is independent in code/model tests | Actual window acceptance pending Accessibility access |
| Packaged backend startup/signing | Local optimized bundle, plist/codesign/embedded-help checks | Passed before latest edits; repeat on final artifact |
| ALFA + CatSniffer capture together | Local packaged-backend attempt on 2026-11-22 | Failed: ALFA MCU timeout; reset/retest required |
| Native rendered UI/accessibility | Accessibility trust probe returned false; System Events request timed out | Pending user permission |

The combined attempt retained 1,355 CatSniffer packets with zero reported host
drops. ALFA produced no observations, reported two recovery attempts and failed.
A separate bounded receive-only diagnostic returned `MCU command 12 sequence 1
timed out`; a read-only register probe succeeded. Those results do not establish
successful combined capture, RF loss rates or ALFA endurance.

Private recordings and device-specific configuration remain outside the repo.
No nearby-device identifiers or raw captures belong in the committed test
fixtures. The regression tests use synthetic replay input and isolated temporary
service databases.

Before sign-off: repeat the dual-receiver run after ALFA reset; exercise native
Start/Pause/Resume/Finish, browse observations/evidence, close/reopen during capture,
and export/reopen a snapshot. Inspect the final bundle in light/dark appearance,
keyboard focus and a narrow window. Rebuild and verify the artifact after fixes.
