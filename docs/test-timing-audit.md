# Test timing migration inventory

Tracking: [#329](https://github.com/dougborg/AirHound/issues/329). This is an ongoing
classified audit, not a statement that the entire suite is already deterministic.
Production deadlines and bounded process cleanup remain in force.

## Current source checkpoint — 2026-11-26

At `17a2791`, the strict timing guard reports 454 classified sites and no
unclassified or unresolved findings, scan errors, or stale dispositions. The
17 guard/helper-registration tests pass. The original migration inventory is
unchanged; see [the guard policy](test-timing-guard.md) for its scope and limits.

The authenticated Hello/Status startup path is implemented in the shared Rust
client and used by the Swift app. All seven focused Rust readiness tests passed
in this checkpoint, including controlled deadline boundaries, cancellation and
actual private-socket cleanup. The socket test required permission to bind its
temporary Unix socket; its initial sandbox denial is not a protocol failure.
This check did not exercise the installed GUI, rerun the complete platform suites,
or establish repository-wide indirect-helper completeness.

The sections below preserve the migration findings and earlier checkpoints.
Their descriptions of the guard and startup handshake as pending are historical;
the current source status above supersedes those descriptions, without closing
the remaining integration and behavioral acceptance requirements.

## Swift, Python and frontend scope

| Paths | Finding / disposition |
| --- | --- |
| `apps/macos/Tests/AirHoundCoreTests/CoreTests.swift` | Replaced short real command timeout and cancellation sleep with explicit child READY plus injected monotonic deadline/grace boundaries; actual signals and process reaping remain asserted. Removed elapsed-speed assertion from the regular-file rejection test; FIFO/directory refusal remains required. |
| `apps/macos/Tests/AirHoundAppTests/{Session,ServiceLauncher,GPSVersion,SetupPresentation,FlipperPresentation,RepeatRecording}Tests.swift` | Model readiness now follows actual publishers; socket and race-fixture readiness use filesystem events. File-load race uses an actor continuation. Remaining teardown/rendering waits are listed below. |
| `apps/macos/Tests/BuildMetadataTests/test_test_runner.py` | Actual process timeout injection occurs only after FIFO readiness; descendant cleanup requires a reaped acknowledgment. No startup-speed assumption or real grace sleep in these correctness cases. |
| `apps/macos/Tests/BuildMetadataTests/test_build_metadata.py` | Build metadata fixtures require source/content identity; included in audit scope. |
| `scripts/tests/test_profile_macos.py` | Already injects `monotonic` and `sleep` with a controlled fixture clock. Preserve that boundary; do not restore real elapsed assertions. |
| `scripts/tests/test_receiver_soak.py` | Synthetic children retain actual protocol pipes and process cleanup; fixture time advances only between witnessed operations. Missing ACK and forced-kill cases explicitly visit before/at/after deadline boundaries. Selector/process waits are outer hang guards, not measured correctness intervals. |
| `scripts/tests/test_helper_capabilities.py` | Stalled children report FIFO readiness before injected query deadlines. Before/at/after cases retain real kill/reap assertions; no interpreter startup-speed assumption. |
| `scripts/tests/test_sniffle_ownership.py` | Synthetic hardware waits on signal wakeup pipes instead of sleeps. Denied-owner UART silence is checked after that child has terminated, not after a quiet-period delay. Real-device settling behavior is unchanged. |
| Other `scripts/tests/*.py`, `signatures/tests/test_compiler.py`, `companions/flipper/scripts/test-diagnostic-artifacts.py` | Include both test bodies and helper/embedded child programs in behavior review. A clean clock-pattern scan alone is not clearance. |
| `website/tests/*.mjs`, `website/tests/browser/*.ts`, `website/playwright.config.ts` | Current tests use actual DOM readiness/condition assertions and browser operations, without fixed sleeps or elapsed correctness thresholds. Playwright's outer operation/test safeguards are not virtual-time assertions. Keep browser setup isolated from a developer's server. |
| `.github/scripts`, workflow-invoked fixtures | Include in repository-wide review; job deadlines, fuzz budgets and benchmark durations must be distinguished from correctness gates. |

The remaining Swift fixture batch uses shared test-only file/process event helpers.
Direct-process service fixtures observe the bounded stderr diagnostic emitted only
after successful listen; socket-path creation alone is explicitly rejected by a
causal negative test. Replay completion observes durable
DB/WAL writes and queries finished history rather than polling the actor clock.
WAL replacement re-arms the watcher under either file/directory delivery order.
Owned-process cleanup uses termination notifications; its escalation tests inject
grace expiration after the child has installed its signal policy, then verify
actual kill/reaping. Optional render fixtures await SwiftUI appearance and flush
layout/display, instead of guessing readiness from run-loop delays. Session race
fixtures release actual subprocesses through FIFOs. Process-owned session tests
refuse bundled backends before launch, avoiding an unowned LaunchServices route.

These optional paths were exercised with private passive/replay services and
offscreen synthetic views; their environment-based skips alone are not validation.
Production collector pacing and GUI refresh timers remain production behavior,
not correctness-time assertions. The repository-wide indirect-helper audit and
the regression guard below remain unfinished; a clean lexical search does not
establish completion of #329.

The GUI's own standalone/LaunchServices startup path is a separate production
readiness change: it must use an authenticated backend handshake within one total
deadline. These fixture witnesses do not mask or claim to fix that startup path.

Rust host/core/firmware, companions and drivers are coordinated separately under
#329. Their findings belong in the same closure checklist. No language or helper
script is exempt merely because a top-level test does not directly read time.

## Practical regression guard

Use a scoped static report to flag newly introduced clock/sleep patterns in known
test files and test-owned helpers. Include embedded child scripts, Rust test modules,
Swift test targets and CI fixtures in the inventory. Report path, owning test scope,
pattern and review disposition; do not blindly reject production deadlines or
mock-clock advancement by matching text anywhere in a source file.

A reviewed migration baseline may identify unresolved existing findings, but must
not label them safe or hide additions. CI should fail newly introduced unclassified
findings and require a reviewer to classify them as virtual time, causal event wait,
outer hang safeguard, benchmark/physical measurement, or a defect to fix. Matching
patterns must never be automatically allowlisted just to make the report green.
Known timing APIs are a guard against regressions, not proof that indirect helpers
or scheduling assumptions are absent.

Closure requires source review of each classified case, tests of virtual boundaries
and ownership cleanup, appropriate Linux/macOS suites, and independent review. Do
not lower timeout assertions, relax production behavior, or omit failure cases to
complete the migration.

## Positive command fixtures and benchmark readiness

Synthetic Core/GPS command fixtures inject a frozen subprocess collector clock
through `BackendClient`; `SessionModel` accepts a client factory while its
production default remains unchanged. These tests still execute real private
commands and verify arguments, outputs, evidence rejection and owned cleanup.
Run them through `apps/macos/run-tests.py`, whose outer watchdog inventories and
cleans owned descendants if a frozen-clock fixture hangs. The positive native
runner test injects actual child completion and asserts the original timeout
argument, keeping a separate outer child-exit guard. Deadline/cancellation tests
retain explicitly advanced clocks and their before/at/after assertions.

`TableBenchmark` waits for actual SwiftUI `onAppear` and layout before resolving
its scroll view. Its measured workload still includes the explicitly labeled
1 ms run-loop service per scroll step; measurements are informational, with no
elapsed-time correctness threshold. The outer render watchdog only fails a
stuck benchmark.

Optional actual-service/LaunchServices integration tests retain production
startup and request budgets. They exercise the deployed protocol path and are
kept as integration evidence, not deterministic unit proof. Production collector
polling and refresh cadence remain separately tracked by the operational delay
audit. These qualifications do not close the repository-wide #329 migration.
