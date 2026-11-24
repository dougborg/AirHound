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

Remaining Swift work: event-based private-service readiness in core integration
fixtures; process teardown helpers that poll the real clock; cleanup tests that
infer non-exit from a small elapsed interval; SwiftUI render/snapshot setup using
fixed run-loop delays. Optional backend integration tests must be exercised with
private synthetic services after their helpers change; an environment-based skip
is not validation of those paths.

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
