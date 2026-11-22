# Flipper companion prototype

For the working stock-firmware setup, see [Flipper and AWOK Marauder](../../docs/flipper-marauder.md). The third-party Marauder companion is distinct from this AirHound prototype.

[Project home](../../README.md) · [Firmware stream](../../docs/firmware-streaming.md) · [Companion plan #34](https://github.com/dougborg/AirHound/issues/34)

This is a **receive-parser and synthetic FAP feasibility milestone**, not a working
UART companion. No GPIO, serial port, radio, SD file, sensor command or device
firmware is accessed. The FAP shows results from one explicitly synthetic AP
observation and labels itself disconnected. It has not been installed or tested
on a Flipper. Existing Marauder apps and AWOK recovery are separate.

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
load/exit and memory acceptance on the intended Flipper. Keep UART/control
unimplemented until exact AWOK routing/power, resource ownership, clean framing,
source/boot identity, backpressure and command acknowledgments are established.
Nothing in these build commands installs a FAP or changes Flipper firmware.

References: [Rust SDK 0.16.0](https://github.com/flipperzero-rs/flipperzero-rs/tree/v0.16.0),
[SDK exports](https://github.com/Next-Flip/Momentum-Firmware/blob/e1784e7418d8b074e971983ceb6fef0f37e52ae4/targets/f7/api_symbols.csv),
[FAP format](https://developer.flipper.net/flipperzero/doxygen/apps_on_sd_card.html).
