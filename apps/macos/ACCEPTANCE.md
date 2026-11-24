# Native preview acceptance

This checklist is intentionally separate from compilation and model coverage.
The preview is under acceptance, not a signed-off production release.

## Current scope

The installed backend is `d8ad552` with the reviewed storage-full recovery
correction; the running Counter Preview GUI remains `946d3c0`. Existing recordings
and setups were preserved during the authorized recovery rollout. Two subsequent
native Wi-Fi/BLE recordings used the same unchanged plan through actual GUI
controls: user validation/first Start, accessibility-driven Pause/Resume and second
Start, and user-confirmed Finish actions. Both recordings finished; live second-run
rows were inspected and the user confirmed the saved second-run rows were visible
and responsive. Independent audit verified prior data and setups remained intact.

This is functional native-only evidence. Both runs exceeded prepared duration
targets; an automated Finish-confirmation attempt failed before the user completed
it. First saved-view visual proof and full-rig controls remain separate gaps.
The staged `8ba1e90` live-selection improvement is not installed or accepted in the
actual window; unsuccessful window-acquisition preparation did not alter the app.

The M5 now runs `5c3d9eb` / `0.2.0`. Its first ten-minute observer had both media,
complete periodic health and zero sampled loss/timeouts, but failed the sampled
memory-recovery criterion. A separate no-reset continuation recovered repeatedly
to the later memory plateau, yet retained 16 scan drops and a 23,006µs writer poll gap;
it also remains failed. Same-boot host lifecycle, complete-rig reception and
physical recovery remain pending. Earlier BLE, zero-input, controller-panic and
output-loss failures are retained in the
[attached-hardware ledger](../../docs/attached-hardware-validation.md), which owns
the support matrix. Earlier successful lifecycle evidence belongs to firmware `0.1.1`.

Backend-controlled CatSniffer/ALFA/Flipper reception with managed GPS and an earlier
assembled M5 session retain their exact source/firmware scope. They do not establish
current full-rig GUI acceptance. Earlier official Rayhunter `0.13.0` operation and bounded raw-feed experiments
remain distinct. The `7de225d` prototype is now installed and seen running after
one reboot, but its deployment readiness parser failed and separate postboot
verification remains pending. Raw transport and offline LTE decoding were
validated earlier; the AirHound cellular source has only private synthetic
acceptance so far. No contemporaneous GPS/validated UTC/upload eligibility follows
from the retained cellular sample. Experimental work in
[#293](https://github.com/dougborg/AirHound/issues/293) is not a prerequisite for
the supported rig. The historical ALFA failure below is not its latest overall status.

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
| Packaged backend startup/signing | Local optimized bundles, plist/codesign/embedded-help checks | Earlier bundle/archive checks passed; the current GUI-only delivery also verified its outer signature and preserved the nested signed backend byte-for-byte. Current native-only control evidence is scoped above; external-rig controls remain separate |
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
path; the subsequent actual-window native-only two-recording evidence is scoped
separately above and does not inherit an automated procedural pass.

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
