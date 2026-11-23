# Native preview acceptance

This checklist is intentionally separate from compilation and model coverage.
The preview is under acceptance, not a signed-off production release.

## Current scope

The runnable counter preview has GUI source `946d3c0` and the unchanged signed
`07c7142` backend. The current backend passed native Wi-Fi/BLE reception after
renewed permission approval. The reconnected M5's bounded identity, Wi-Fi/BLE and
host lifecycle success belongs to firmware `0.1.1`. Its first `07c7142` / `0.2.0`
application passed boot checks but later failed BLE readiness. The reviewed
[#318](https://github.com/dougborg/AirHound/pull/318) correction, source `3baaacc`,
is now installed and passed a 240-second both-media continuity observation.
After the observer closed, the production-reader lifecycle attempt received no
device input and failed before Pause/Resume. An explicit line-state diagnostic
produced a new boot and both media but did not establish physical button recovery;
the user's unresponsive-button report remains unresolved. The reviewed `14d6085`
host transport policy passed focused tests but awaits hardware validation in
[#175](https://github.com/dougborg/AirHound/issues/175). The earlier BLE-only failure
in [#317](https://github.com/dougborg/AirHound/issues/317) remains distinct.
None of these results proves that the current GUI's Start, repeat recording,
live/history presentation or all-receiver controls work. Those actual-window checks remain open for the
everyday-recording goal.

Later backend-controlled sessions accepted concurrent CatSniffer, ALFA and
Flipper reception with managed GPS; an earlier assembled session also included
M5. Preserve their exact source/firmware and failure scopes in the
[attached-hardware ledger](../../docs/attached-hardware-validation.md), which
owns the current support matrix. Physical disconnect recovery and endurance
remain separate acceptance work. Official Rayhunter `0.13.0` on the Orbic and its
separate raw-feed fork prototype do not constitute an AirHound live cellular
source. The fork's helper preflight passed, but full raw-feed runtime acceptance
remains pending after two failed staged attempts and restoration of the official
daemon; experimental work in [#293](https://github.com/dougborg/AirHound/issues/293)
is not a prerequisite for the currently supported rig. The historical ALFA
failure below is retained, not the latest overall ALFA status.

## Earlier native interaction evidence

The manual checks below belong to the predecessor preview on 2026-11-22 unless
otherwise stated. They are useful regression evidence, not acceptance of every
later GUI/backend signing identity.

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
| Packaged backend startup/signing | Local optimized bundles, plist/codesign/embedded-help checks | Earlier bundle/archive checks passed; the current GUI-only delivery also verified its outer signature and preserved the nested signed backend byte-for-byte. Actual current GUI controls remain a separate gate |
| ALFA + CatSniffer capture together | Initial packaged-backend attempt on 2026-11-22 | Historically failed with ALFA MCU timeout; subsequent individual and combined success is recorded in the current hardware ledger, without an endurance claim |
| Service connection status/popover | User confirmed clicking the bottom status opens the popover on 2026-11-22 | Passed manual interaction check |
| Native rendered UI/accessibility | Compact predecessor row rendered; current bottom status is available across both tabs | Capture/observations interactions verified with native accessibility and dark-mode screenshots; light appearance and full VoiceOver review not claimed |

That initial combined attempt retained 1,355 CatSniffer packets with zero reported host
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

The earlier unnamed-observation and selection work was reviewed and merged.
Dark-mode rendering was inspected on that preview; light appearance and a
complete VoiceOver audit remain future UI acceptance work. A passing build or
model test does not replace actual current-window testing.

The original ALFA failure and recovery history remain tracked in #155 and the
hardware ledger. Do not repeat the old combined-failure verdict as the current
support status or treat the later short successes as physical recovery/endurance
acceptance.

The merged [#316](https://github.com/dougborg/AirHound/pull/316)
`RepeatRecordingTests` regression uses the real Rust service and a
synthetic helper with no hardware access. One unchanged plan and SessionModel
successfully Start/Finish twice, including a second Start queued during polling,
distinct request/run IDs, first-run Pause/Resume, nonempty live observations and
reopening both saved histories without cross-run rows. The local check passed
against the preserved signed `07c7142` backend. This verifies the model/service
path; it does not close the actual-window second-recording acceptance item.

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
