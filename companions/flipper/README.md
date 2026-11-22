# Flipper companion prototype

For the working stock-firmware setup, see [Flipper and AWOK Marauder](../../docs/flipper-marauder.md). The third-party Marauder companion is distinct from this AirHound prototype.

[Project home](../../README.md) · [Firmware stream](../../docs/firmware-streaming.md) · [Companion plan #34](https://github.com/dougborg/AirHound/issues/34)

The original `airhound-flipper` target is a **receive-parser and synthetic FAP
feasibility milestone**. It accesses no GPIO, UART, radio, SD file or device
firmware; it shows one synthetic AP observation and labels itself disconnected.

The separate `airhound-uart-diagnostic` target below is experimental. Its default
launch also leaves UART untouched. Only explicit `capture` mode controls the
attached Marauder using fixed passive commands. It has not been installed or
hardware accepted. Neither target replaces the existing third-party companion.

## Parser contract

`Receiver` is `no_std`, allocation-free and independent of the matching engine.
The root core is a **test-only** dependency for producing actual wire records;
core extraction (#33) is not required. There is no internal event queue: consume
returned events synchronously, and account separately for future transport queue
overflow. Counts are saturating per-session **sightings**, not unique devices:

- AP observations: WiFi beacon/probe-response records.
- Other WiFi observations: probes/data/other frames; BLE observations separately.
- Signature observations: a received observation has match reasons or a rule.
  This is a clue count, not verified identity, threat or an audible alert.
- Evidence records: retained separately with their pack/ruleset, basis and IDs.
  They are **unassociated**: ID/scope/timestamp is not unique and adjacency does
  not prove linkage. Evidence never increments observation/signature counts.
- Sensor `stream_stats` preserves scan/output/BLE-output/serialization counters
  separately. Parser rejections/oversized lines are local counters and do not
  establish RF losses or complete delivery. Stats remain wrapping boot counters;
  this milestone does not infer boot identity or calculate loss deltas.

`begin_session()` clears fragments, counts and old stats; the caller owns source
identity and must invoke it on reconnect. `disconnect()` clears partial framing,
counts discarded partial lines and exposes disconnected state. Future UI must add
receive-age/stale timing; this library deliberately has no hardware clock.

The JSON capacity is **512 bytes**, derived from current firmware's 512-byte
serialization buffer; the newline also occupies that producer buffer. Tests use
shared schema fixtures and real production serialization with maximum field and
evidence counts. The schema is broader than this embedded subset: larger future
records are explicitly rejected. A separate 128-byte prefix allowance handles
existing serial log framing; BLE/plain NDJSON also works. Prefixes are discarded
up to the first `{`, not authenticated or interpreted as source identity.

Overlong lines report `Oversized` once and discard through newline before
resuming. Malformed/unsupported bounded fields report `MalformedOrUnsupported`;
neither is treated as an ordinary no-match observation. JSON syntax and depth
(maximum 12) are checked before parsing, including ignored future fields.
Unknown message types return `Unknown` after syntax validation; unknown fields
are ignored. Unicode/escaped text is decoded into fixed scratch storage.
No absence-based evidence or alert execution is implemented.

## Host validation

From this directory, with stable Rust installed:

```sh
cargo test --locked
cargo clippy --locked --lib --tests -- -D warnings
cargo fmt --all -- --check
```

Tests cover synthetic shared fixtures, all-observation counts, separate evidence,
chunk boundaries, log prefixes/CRLF, escaped names, malformed/deep JSON, unsupported
capacity, overflow recovery, reconnect boundaries and actual producer maxima.
There are no ambient device captures in the test corpus.

## API 87.1 feasibility build

The connected Flipper was identified as Momentum `mntm-012` / API **87.1**.
The dependencies are pinned to Rust SDK **0.16.0** and resolved in this component's
own lockfile. Newer 0.17 SDK/API88 builds are not a drop-in replacement. The SDK
and bindings are MIT licensed; dependency sources/notices remain their authors'.
No Momentum firmware implementation is copied into this project.

```sh
rustup target add thumbv7em-none-eabihf --toolchain stable
cargo build --locked --release --features fap --bin airhound-flipper --target thumbv7em-none-eabihf
python3 -m venv target/audit-env
target/audit-env/bin/pip install -r requirements-dev.txt
curl --fail --location https://raw.githubusercontent.com/Next-Flip/Momentum-Firmware/e1784e7418d8b074e971983ceb6fef0f37e52ae4/targets/f7/api_symbols.csv -o target/momentum-api.csv
cp target/thumbv7em-none-eabihf/release/airhound-flipper target/airhound-flipper-demo.fap
target/audit-env/bin/python scripts/check-fap.py target/airhound-flipper-demo.fap target/momentum-api.csv
```

The audit verifies the exact SDK CSV checksum, ARM relocatable ELF, target 7,
API87.1 manifest, entry symbol and every imported symbol against that SDK. It
reports file/allocated-section sizes and configured stack; those numbers are
**not measured runtime RAM/stack headroom**. The app requests an 8 KiB stack;
the dialog/runtime uses SDK allocation even though the receiver itself does not.
CI uploads the synthetic demo and audit as development artifacts, not release
assets. Compilation/import checks do not establish loader relocation support,
ABI behavior, load/exit safety or live UART throughput.

Before installation: review the artifact and SDK match, then run explicit
load/exit and memory acceptance on the intended Flipper. Keep live UART/control disabled until exact AWOK routing/power, resource ownership,
clean framing, source/boot identity, backpressure and command acknowledgments are
established.
Nothing in these build commands installs a FAP or changes Flipper firmware.

References: [Rust SDK 0.16.0](https://github.com/flipperzero-rs/flipperzero-rs/tree/v0.16.0),
[SDK exports](https://github.com/Next-Flip/Momentum-Firmware/blob/e1784e7418d8b074e971983ceb6fef0f37e52ae4/targets/f7/api_symbols.csv),
[FAP format](https://developer.flipper.net/flipperzero/doxygen/apps_on_sd_card.html).


## Bounded UART diagnostic

This separate API87.1 application investigates the failed binary capture path
recorded in the [hardware ledger](../../docs/attached-hardware-validation.md#flipper-and-awok-follow-up).
It does not claim to fix the installed companion's unknown NULL-pointer fault.
It has not been installed or exercised on hardware. The original third-party FAP
and both device firmwares must remain preserved during diagnostic acceptance.

Default launch, `selftest`, and unknown arguments never acquire UART. The only
hardware mode is the exact argument `capture`; it is intentionally not started by
browsing the app menu. There is no arbitrary-command API. Its commands are fixed:
initial `stopscan`, then `sniffbeacon -serial`, then `stopscan` (one retry if the
stop text acknowledgment is missing). It never sends `info`, because Marauder
1.21.0 leaves a state that prevents the subsequent sniff command.

The implementation uses a 48 KiB static raw buffer, a 4 KiB RX stream, an 8 KiB
application stack, and fixed parser/report buffers. Allocations and create-new
output files precede UART acquisition. Free-heap checks require 16 KiB before
setup and 8 KiB remaining immediately before UART acquisition; these are minimum
reserves, not proof against fragmentation, concurrent allocations or stack
exhaustion. FAP load failure happens before UART use. Static ELF allocation,
actual UART-free load/exit and measured headroom remain separate acceptance gates.

One main loop owns UART commands. ISR work only transfers a byte into the bounded
stream and increments drop/error counters. The display reads static strings and
Back sets an atomic cancellation flag; it never borrows a mutable log. While UART
is active, the main loop performs no filesystem writes, UI updates or unbounded
queue drains. It polls deadlines before each nonblocking receive of at most 512
bytes. Capture requests stop after two seconds, or immediately on cancellation,
malformed framing, overflow or UART loss. Stop acknowledgment/drain gets at most
one further second. Acknowledgment must be an exact complete console line in the
correct command phase, outside framed PCAP records; marker-like payload bytes do
not count. Console text can itself include untrusted SSIDs, so these are
**unauthenticated observed text acknowledgments**, not proof of physical RF state.

The SDK transmit routine waits for hardware TX-ready without a timeout. Fixed
10/22-byte commands normally take less than two milliseconds at 115200 baud, but
a hung peripheral, scheduler failure, power loss or Flipper reset can defeat any
local software deadline. Reset or transport failure leaves AWOK stop **unknown**;
a separately verified stop-only recovery is still required. No host/RPC session
is needed to trigger the normal local stop deadline.

RX is stopped before releasing its callback context. Final counters are read
only after shutdown, including late ISR losses after the last state-machine poll.
Any accepted bytes still queued at shutdown are counted as `unread_ring_bytes`
and force an incomplete result, even when overflow/error counters are zero.
Only then does the app persist the raw serial bytes and metadata under
`/ext/apps_data/airhound_diag/run-<boot-tick>.*`. All files use create-new semantics;
boot-tick collisions fail instead of overwriting. The closed `.begin.json`
marker survives independently of the final report; an empty/partial `.serial` or
`.json` remains failure evidence. Short writes retry, zero writes fail, and close
is attempted even after write failure. The final report cannot certify its own
close: failure is also returned by the application. Records and raw bytes must
still pass the host's strict Marauder importer and private durable replay before
capture acceptance. RSSI, channel, GPS and synchronized wall time are not invented.
The report always retains `capture_accepted: false`.

Build with source and matching debug symbols retained (from this directory):

```sh
rustup component add llvm-tools-preview --toolchain stable
# Prepare target/audit-env and pinned target/momentum-api.csv as above.
target/audit-env/bin/python scripts/build-diagnostic.py target/momentum-api.csv
```

`target/diagnostic/` contains the FAP, its matching DWARF debug ELF and `audit.json`.
A diagnostic-only copy of the MIT SDK linker script removes its explicit DWARF
discard; the ordinary demo layout remains unchanged. The build rejects missing
DWARF, mismatched allocated code/data sections, and a bad debuglink CRC.
The audit records source hashes, revision/dirty state, toolchain/flags, SDK pin,
artifact hashes, imports and static footprint. CI builds these as clearly untested
development artifacts. This script neither downloads firmware nor installs or
launches anything. Host tests cover split markers, marker/acknowledgment strings
inside packets, complete-line matching, malformed/truncated streams, tick wrap,
missing acknowledgments, cancellation, overflow, late UART losses, short/failed
writes and failed closes. They do not emulate the Flipper scheduler, SD or RF.

Before a live attempt: independently review the exact artifact; verify identity,
API and available heap; perform UART-free load/exit; exercise synthetic replay;
then separately review one bounded passive acquisition and its stop fallback.
Keep raw capture/identifier contents private. Do not retry the crashing companion
or flash firmware as part of this diagnostic. The user-assisted CDC1 bridge
alternative is described in the [hardware guide](../../docs/flipper-marauder.md#current-binary-capture-limitation).

This diagnostic is an acquisition experiment, not a completed Flipper integration.
Even after a complete file passes private import, a normal supported workflow
still needs a backend-controlled companion/live source or an accepted bridge
route, with device discovery, setup and lifecycle controls. It does not close
[the attached-hardware milestone #269](https://github.com/dougborg/AirHound/issues/269)
on the strength of an offline parser test.
