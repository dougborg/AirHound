# Testing, analysis and performance

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
