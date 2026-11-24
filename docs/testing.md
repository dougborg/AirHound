# Testing, analysis and performance

## Deterministic correctness tests

[Issue #329](https://github.com/dougborg/AirHound/issues/329) tracks this policy
across the repository, not just one receiver or language. Correctness must not
depend on machine speed, scheduling luck, wall-clock timestamps, or a short real
sleep. Use fixed fixture timestamps, injected clocks and explicit virtual-time
advancement for freshness, timeout, retry and grace-period behavior. Assert the
boundary before, at and after expiration, including cancellation and cleanup.

Coordinate asynchronous work with the actual readiness or completion event:
channel messages, continuations, publisher notifications, process pipe handshakes,
or filesystem events. Subscribe before rechecking the condition so an event cannot
be lost. Do not replace sleeps with arbitrary yield counts, busy loops, or larger
production deadlines. Virtual time must not outrun an actual subprocess before its
readiness barrier; test-owned children still need real ownership and reaping checks.

A generous outer test/job hang limit is an infrastructure safeguard. It may stop a
broken test, but elapsed time is not a correctness assertion. Keep bounded cleanup
of real owned children and resource ownership checks. Runtime timers still use the
production clock; tests inject scheduling only where they exercise clock behavior.
For example, the subprocess collector's public initializer retains monotonic time,
while its deadline test advances a controlled clock after the child reports READY.

Performance benchmarks and physical timing measurements are separate activities.
They should report their method and measurements, never become correctness tests
with fragile elapsed-time pass thresholds. Hardware acceptance is not a substitute
for deterministic unit and synthetic integration coverage.

The migration inventory and remaining work are in
[test-timing-audit.md](test-timing-audit.md). A lexical audit is only a review aid:
absence of a spelling such as `sleep` does not prove deterministic behavior.


[Project home](../README.md) · [Documentation index](README.md) · [Developer setup](../CONTRIBUTING.md) · [Scripts](../scripts/README.md)

Run commands from the repository root. Hardware-free checks use stable Rust unless a pinned coverage or fuzz toolchain is specified.

Desktop pipeline benchmarks run on Linux and macOS in the Desktop performance
workflow. Use `just bench-hostd` locally, or `just profile-hostd` for a macOS live
radio profile with pause/resume and shutdown measurements. See the
[desktop guide](../hostd/README.md#tests-and-performance) for methodology,
permissions, and the default macOS BLE address lookup and public-API-only build.

CI runs the portable core tests, desktop tests on Linux and macOS, strict Clippy
(including core test code), formatting, schema checks, dependency audits for all
four lockfiles, workflow/shell linting, and all four firmware builds. Dependabot
covers the core, both runtimes, fuzz harness, Actions and development container.
Firmware compilation is not hardware execution. Wokwi is explicitly skipped when
its token is unavailable; live board, radio and memory checks remain in
[#141](https://github.com/dougborg/AirHound/issues/141).

Coverage uses `cargo-llvm-cov 0.9.1` and `nightly-2026-11-18`, with test bodies
excluded via `coverage(off)`. Run `bash .github/scripts/coverage.sh core` or
`bash .github/scripts/coverage.sh hostd` with that toolchain selected and
`llvm-tools-preview` installed. CI uploads HTML and JSON reports and publishes
line/function/region totals in its job summary. Coverage directories are separate
from ordinary builds; do not mix compiler versions in them.

The core production-line floor is 89%; both desktop platforms enforce 81%,
and the independent receiver enforces 81% on each platform, recorded in [`.github/coverage-thresholds.json`](../.github/coverage-thresholds.json).
Linux and macOS run independently. Desktop tests exercise session startup,
scanner selection, event filtering and serialization, command framing, output
failures, cancellation and draining, plus adapter conversion and offline capture.
CLI subprocess tests verify argument handling and file protection before adapters
are initialized. Test bodies are excluded; OS adapter code remains in the totals.
Permission prompts, live radio scans and location delivery still require hardware
validation. Branch coverage is not measured. New behavior should add tests rather
than lower these floors.

Parser fuzzing uses `cargo-fuzz 0.13.2` with the same nightly toolchain and address
sanitizer. The `commands` target checks bounded line framing and recovery as well
as JSON parsing. The `frames` target exercises BLE, WiFi and Remote ID parsing,
seeded with valid advertisements and the public NAN capture already used by our
regression tests. To run locally:

```bash
cargo +nightly-2026-11-18 fuzz run commands -- -max_total_time=60 -max_len=4096
cargo +nightly-2026-11-18 fuzz run frames -- -max_total_time=60 -max_len=4096
```

CI runs each target for 30 seconds per PR/push, and for 10 minutes weekly.
Crashes and timeouts fail the job and their reproducer artifacts are uploaded.
Only named seed inputs are checked in; promote findings into deterministic
regression tests. A clean fuzz run does not prove absence of parser bugs.

Desktop integration tests exercise chunked command input through state changes
and status output, oversized-line rejection, output errors, and shutdown while
scanner tasks are idle or blocked on a full queue. They use in-memory streams and
scanner tasks, so they do not establish driver or physical-radio behavior.

## Timing regression guard

Run `python3 scripts/check-test-timing.py --report /tmp/test-timing-report.json --require-resolved`.
CI rejects new or changed unclassified clock/wait sites, unresolved migration
findings, and scanner/configuration errors. It publishes the report even when
the check fails. A green check does **not** prove that every indirect timing
dependency has been found.

The original migration inventory remains immutable. See the
[timing policy guide](test-timing-guard.md) for scope, review requirements and
limitations. Production-delay investigation is separate from this test guard;
protocol dwell, retry/backoff and periodic work must not be removed merely to
satisfy a lexical check.
