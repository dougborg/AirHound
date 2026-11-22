# Dependency audit

Run `cargo audit` for every independent lockfile: the repository root,
`firmware-std`, `hostd`, `fuzz`, `companions/flipper`, and `drivers/mt7610u`.
The CI audit job covers the first five; the driver workflow covers its own lock.

## heapless review (2026-11-21)

All direct dependencies now require at least **0.9.4**, and all 0.9 lock entries
resolve to 0.9.4. This includes the root firmware/core, ESP-IDF firmware, desktop,
fuzz harness and Flipper companion. The root lock preserves `bt-hci`'s existing
0.9 dependency family. The patched crate requires Rust 1.88; the firmware CI
compiler is pinned to 1.90.0.0. Firmware builds, rather than that version comparison
alone, are the compatibility gate.

RustSec reports fixes for deque invariants, pool concurrency, panic safety and
other collection soundness defects in this release. See the
[heapless advisory list](https://rustsec.org/packages/heapless.html) and
[tracking issue #222](https://github.com/dougborg/AirHound/issues/222).
AirHound directly uses bounded `String` and `Vec`; source inspection found no
application use of `Deque::make_contiguous`, heapless pools, `HistoryBuf`, or
`IndexMap`. This does not prove that dependency paths are unreachable.

### Remaining 0.8 dependency exposure

The upgrade does **not** remove heapless 0.8.0:

- All five core-related lockfiles retain `tlv-rs 0.3.0` through `ieee80211 0.5.9`.
  Its heapless serialization helper returns `Vec<u8, N>`; byte elements have no
  panicking destructor. This narrows the observed use, rather than exempting the
  entire package from advisories.
- ESP-IDF additionally retains 0.8 through `embassy-sync 0.6.2`, `embedded-svc
  0.28.1`, `esp-idf-hal 0.45.2`, `esp-idf-svc 0.51.0`, and `esp32-nimble 0.11.1`.
  These include strings, vectors, the SPI driver's deque, and a generic pubsub
  deque with a clear operation. No blanket unreachability claim is justified.
- The ALFA receiver lockfile contains no heapless dependency.

The audited 0.8.0 package still produces **nine informational unsound warnings**:
RUSTSEC-2026-0574 through RUSTSEC-2026-0582. These cover sorted/map invariants,
SPSC ordering, panic safety, deque conversion/contiguity and pool concurrency.
Their affected-function and feature ranges differ; package presence alone is
not proof that an affected operation executes.

No advisory is suppressed. `cargo audit` exits successfully for informational
unsound warnings by default, so a green audit job is **not** a claim that these
warnings are resolved. CI retains the complete warning output. Eliminating the
0.8 dependency paths and introducing a reviewed policy for failing on unsound
warnings remain under #222; do not add global advisory-ID ignores, which could
also conceal a regression to affected 0.9 versions. Existing unmaintained-crate
warnings remain separate maintenance work.
