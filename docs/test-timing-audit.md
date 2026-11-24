# Test timing migration inventory

Tracking: [#329](https://github.com/dougborg/AirHound/issues/329). This is an ongoing
classified audit, not a statement that the entire suite is already deterministic.
Production deadlines and bounded process cleanup remain in force.

## Swift, Python and frontend scope

| Paths | Finding / disposition |
| --- | --- |
| `apps/macos/Tests/AirHoundCoreTests/CoreTests.swift` | Replaced short real command timeout and cancellation sleep with explicit child READY plus injected monotonic deadline/grace boundaries; actual signals and process reaping remain asserted. Removed elapsed-speed assertion from the regular-file rejection test; FIFO/directory refusal remains required. |
| `apps/macos/Tests/AirHoundAppTests/{Session,ServiceLauncher,GPSVersion,SetupPresentation,FlipperPresentation,RepeatRecording}Tests.swift` | Model readiness now follows actual publishers; socket and race-fixture readiness use filesystem events. File-load race uses an actor continuation. Remaining teardown/rendering waits are listed below. |
| `apps/macos/Tests/BuildMetadataTests/test_test_runner.py` | Actual process timeout injection occurs only after FIFO readiness; descendant cleanup requires a reaped acknowledgment. No startup-speed assumption or real grace sleep in these correctness cases. |
| `apps/macos/Tests/BuildMetadataTests/test_build_metadata.py` | Build metadata fixtures require source/content identity; included in audit scope. |
| `scripts/tests/test_profile_macos.py` | Already injects `monotonic` and `sleep` with a controlled fixture clock. Preserve that boundary; do not restore real elapsed assertions. |
| `scripts/tests/test_receiver_soak.py` | Pending: real short run/ACK/recovery deadlines in child fixtures must become controlled-time/state-machine tests plus independently synchronized real-child cleanup coverage. |
| `scripts/tests/test_helper_capabilities.py` | Pending: stalled query currently depends on the child writing a PID before the real query deadline. Replace with explicit readiness and injected timeout. |
| `scripts/tests/test_sniffle_ownership.py` | Pending: ready events already protect lease contention, but fixture sleeps and negative quiet-period checks need event/terminal-state-based replacements. |
| Other `scripts/tests/*.py`, `signatures/tests/test_compiler.py`, `companions/flipper/scripts/test-diagnostic-artifacts.py` | Include both test bodies and helper/embedded child programs in behavior review. A clean clock-pattern scan alone is not clearance. |
| `website/tests/*.mjs`, `website/tests/browser/*.ts`, `website/playwright.config.ts` | Current tests use actual DOM readiness/condition assertions and browser operations, without fixed sleeps or elapsed correctness thresholds. Playwright's outer operation/test safeguards are not virtual-time assertions. Keep browser setup isolated from a developer's server. |
| `.github/scripts`, workflow-invoked fixtures | Include in repository-wide review; job deadlines, fuzz budgets and benchmark durations must be distinguished from correctness gates. |

The remaining Swift fixture batch uses shared test-only file/process event helpers.
Core service startup observes socket creation; replay completion observes durable
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
