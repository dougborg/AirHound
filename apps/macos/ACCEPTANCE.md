# Native preview acceptance

This checklist is intentionally separate from compilation and model coverage.
The preview is under acceptance, not a signed-off production release.

| Requirement | Evidence | Status |
| --- | --- | --- |
| Shared Rust capture/control boundary | Swift uses the shared Rust CLI client; no Swift scan/policy/database implementation | Implemented |
| Discover/select/default setup | Native candidate controls and native Wi-Fi/BLE defaults; app-model service test | Automated model check passes; user confirmed connection and expandable receiver details on 2026-11-22 |
| Advanced receiver/channel setup | ALFA/CatSniffer fields plus backend-validated JSON/import | Passed GUI JSON editing, validation and start; native helper fields compile and remain backend validated |
| Packaged CatSniffer GUI start | User imported the prepared plan and showed Bluetooth running; backend subsequently retained a finished run with 3,629 records | Passed; second live run also exercised GUI pause/resume/finish with backend acknowledgments |
| Durable history/evidence/export | Real-service synthetic replay, retry, page decoding and SQLite export tests | Automated checks pass; GUI-exported SQLite archive integrity was ok with all 3,629 records from the first run |
| Client disconnect/reconnect | App-model starts a real service, disconnects, reconnects a new model | Automated model check passes |
| Uncertain start after app loss | Private fsynced intent record; replacement model resolves accepted ID with input file removed | Automated recovery check passes |
| Bounded browsing/live polling | Record window eviction and cursor tests; paged/follow controls | Passed rendered table/inspector and next-page check (500 to 1,000 retained records); live following advanced after reconnect |
| Readiness/recovery/drop distinctions | Unknown-versus-zero tests and preserved backend phases/counters | Automated checks pass |
| Window close leaves capture running | Service lifetime is independent in code/model tests | Passed on 2026-11-22: Cmd-W terminated GUI; service PID and active run survived, retained records grew, reopened GUI reconnected to that run |
| Packaged backend startup/signing | Local optimized bundle, plist/codesign/embedded-help checks | Bundle and extracted archive signatures, executable hashes and embedded help verified; repeat after final name fallback build |
| ALFA + CatSniffer capture together | Local packaged-backend attempt on 2026-11-22 | Failed: ALFA MCU timeout; reset/retest required |
| Service connection status/popover | User confirmed clicking the bottom status opens the popover on 2026-11-22 | Passed manual interaction check |
| Native rendered UI/accessibility | Compact predecessor row rendered; current bottom status is available across both tabs | Capture/observations interactions verified with native accessibility and dark-mode screenshots; light appearance and full VoiceOver review not claimed |

The combined attempt retained 1,355 CatSniffer packets with zero reported host
drops. ALFA produced no observations, reported two recovery attempts and failed.
A separate bounded receive-only diagnostic returned `MCU command 12 sequence 1
timed out`; a read-only register probe succeeded. Those results do not establish
successful combined capture, RF loss rates or ALFA endurance.

Private recordings and device-specific configuration remain outside the repo.
No nearby-device identifiers or raw captures belong in the committed test
fixtures. The regression tests use synthetic replay input and isolated temporary
service databases.

Packaged GUI replay passed: run `18ef5e3d01a2dfa7383379de11af4e4b` retained all
three synthetic APs and opened in Observations. Invalid replay run
`c0b3e3e570f20967d3e5f10a1247b05b` finished as failed, and the GUI showed
`receiver synthetic-replay: early eof`. A valid plan was restored afterward.

Before merge: complete final code review, final-head CI, and bundle verification
with the unnamed-observation display fix. Dark-mode rendering was inspected;
light appearance and a complete VoiceOver audit remain future UI acceptance work.

ALFA recovery and successful ALFA + CatSniffer capture remain separate open
hardware follow-ups (#155), not prerequisites for this native preview.

The observation-selection regression now covers automatic selection of both live
and completed runs, plus preservation of a manually stopped Follow Live state.
The incomplete-setup regression names missing receiver paths in shared Rust
validation; a configuration error no longer changes the connection indicator.

Live lifecycle evidence: run `608b017a33c5d2ef79ac1f51dc258a81` remained
recording after the real app window closed. The GUI process exited while service
PID 85652 survived; 3,937 records were retained before reopening. Reconnected GUI
Pause produced `paused` with `sniffle_ingestion_paused`; Resume produced `recording`
and `sniffle_passive`; the Finish confirmation produced `finished`. This checks
control acknowledgments and service ownership, not RF completeness.
