# Test timing regression policy

The guard rejects unreviewed correctness dependencies on wall time.
[#329](https://github.com/dougborg/AirHound/issues/329) tracks the audit and its
remaining production followups. This is a lexical review aid, not a call-graph
proof or a ban on production timers.

## What is scanned

Tracked test, benchmark and TestSupport files; inline Rust test/cfg-test items
(including visibility modifiers and nested modules); automatically resolved
external cfg-test modules; and the exact helper paths in
[test-timing-policy.json](test-timing-policy.json). Registered helpers are scanned,
not exempted. Mixed production/test helpers still require per-site review.
Embedded shell/Python/JavaScript fixture strings are scanned too. Comments are
ignored, ordinary escaped identifiers and line continuations are normalized with
source-offset mapping, and global Rust timing aliases are propagated into tests.
Unresolved external module paths fail closed.

Known clock, delay, deadline and Client connection/handshake APIs produce
findings. A raw Duration constant alone does not: it may be protocol data or an
explicit virtual-time input. Unknown wrappers, computed imports/reflection,
macros and arbitrary dynamically constructed scripts still require human review.
New test support outside the recognized paths must be registered explicitly.
Review imported/executed helpers, not just the immediate assertion. No zero-count
or green-CI claim establishes that all indirect timing dependencies were found.

## Reviewing a finding

The report gives its path, scope, rule, excerpt and context hash. Prefer causal
readiness, injected clocks or scheduler-controlled virtual time. Preserve real
timeout/cancellation/ownership assertions, including child cleanup. An outer OS
hang watchdog may bound a broken fixture, but must not decide whether the tested
behavior is correct or advance its correctness clock.

An accepted disposition names the exact fingerprint and matching path/scope/rule/
context, a specific rationale, tracking issue and review reference. Categories
are virtual-time, causal-event, outer-hang-guard, fixed-input,
benchmark-or-physical, production-scope and detector-input. `unresolved` is an
explicit unfinished item, not a safe exception. The guard's own adversarial
snippets are detector-input data and are never executed as fixture programs.
Changed scope or alias bindings invalidate prior identities; duplicate calls
cannot reuse one classification. Remove stale classifications after review.
There are no path-wide safe exceptions or automatic baseline-update command.

## Frozen migration inventory

The initial policy records source commit/tree, the generator SHA-256, and sorted
exact fingerprints. Its inventory SHA-256 is over UTF-8 compact JSON of that
sorted list (`json.dumps(keys, separators=(',', ':'))`). The parser verifies the
hash and unique identities. It was generated from the tracked files at the pinned
source with the explicit helper registry; the new detector and its test file are
separately classified. To reproduce, use that source tree, the generator version
matching the pinned hash, the helper registry and empty dispositions, then hash
the sorted finding keys. Later source changes produce new findings; absent old
keys are reported as stale, never silently relabeled as fixed or accepted.

This inventory includes legitimate virtual clocks, physical measurements and
production helper timing alongside remaining defects. Every baseline entry is
**unresolved** until its exact context is independently classified or corrected.
The optional non-strict mode reports that debt while rejecting new/changed sites.
CI now uses `--require-resolved`; an audit must also review the scanner's
limitations and helper coverage. Do not regenerate the baseline to accept a
failure or describe its entries as reviewed exceptions.

## Reviewed composition checkpoint

The reviewed source composition classifies all 440 detected sites with exact
context identities: controlled clocks, causal events, fixed inputs, fixture hang
guards, measurements, and identified production scopes. Superseded dispositions
were removed; the original 766-entry migration inventory remains unchanged.
Disappearing fingerprints do not establish correctness by themselves.

This policy depends on the source migrations in #332, #338 and #340, the firmware
idle-wait change in #341, and the guard in #339. It must travel with those source
changes, not be applied as exceptions to their older implementations. Real-service
integration tests retain qualified startup/protocol budgets. Operational polling,
the receiver-helper late-EOF deadline boundary, and the Sniffle startup settle
remain production followups; their classification does not establish that these
delays are optimal or required.

The adversarial suite exercises aliases, external modules and both path-attribute
orders, embedded scripts, clock variants, context changes, malformed policy,
stale entries, and the normal-versus-closure exit status. Run it with
`python3 -B -m unittest discover -s scripts/tests -p test_test_timing_guard.py -v`.
