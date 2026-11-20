# Shared core and no_std firmware source

[Project home](../README.md) · [Documentation index](../docs/README.md) · [Firmware setup](../docs/firmware.md)

This directory contains two entry points in the root Cargo package:
[`lib.rs`](lib.rs) is the portable `no_std` library; [`main.rs`](main.rs) is the
Embassy/esp-hal firmware. The desktop and ESP-IDF packages depend on the library
with default features disabled. The `xiao` and `m5stickc` features enable board
firmware dependencies; select exactly one for a firmware build.

## Work on the library

Run from the repository root with stable Rust, without an ESP toolchain or radio:

```sh
cargo +stable test --locked --lib --no-default-features
cargo +stable clippy --locked --lib --tests --no-default-features -- -D warnings
cargo +stable fmt --check
```

The repository's toolchain override selects `esp`; `+stable` is intentional for
host work. Firmware build/flash instructions live in the [firmware guide](../docs/firmware.md).

## Module map

| Area | Source | Responsibility |
| --- | --- | --- |
| Observations | [scanner.rs](scanner.rs), [ble_data.rs](ble_data.rs), [odid.rs](odid.rs) | WiFi/BLE parsing, bounded typed BLE data, Remote ID |
| Detection | [filter.rs](filter.rs), [rules.rs](rules.rs), [evidence.rs](evidence.rs) | Primitive matches, expressions, rule-local proofs and separate alert policy |
| Defaults | [defaults.rs](defaults.rs), [generated_signatures.rs](generated_signatures.rs) | Helpers and generated tables from the [canonical pack](../signatures/README.md) |
| Tracking | [observations.rs](observations.rs), [mac_index.rs](mac_index.rs), [watchlist.rs](watchlist.rs), [proximity.rs](proximity.rs) | Bounded address counts, lookups, watchlists and proximity primitives |
| Wire formats | [protocol.rs](protocol.rs), [comm.rs](comm.rs) | Serialization, command parsing and framing |
| Board/runtime | [board.rs](board.rs), [main.rs](main.rs), [memory.rs](memory.rs), [display.rs](display.rs), [buttons.rs](buttons.rs), [buzzer.rs](buzzer.rs) | Pin configuration, queues/tasks, allocation, display and controls |

Library availability does not imply every runtime exposes a feature as a command.
Check the [command support matrix](../docs/protocol.md#command-results-and-runtime-support).

## Detection semantics

A **signature** is an atomic matching predicate. A **rule** combines signature
results for one observation; it is not temporal correlation. **Evidence** records
which signatures support that rule. **Alert policy** determines whether to notify,
independently of observed counts and user watchlist membership.

Radio input passes through bounded queues to parsing/matching and serialized
output. Firmware streams ordinary observations as well as matches. Display
counts and buzzer alerts have separate meanings. Missing, truncated or cached
fields must not be treated as complete raw advertisements or evidence of absence.
See [evidence](../docs/detection-evidence.md), [BLE data](../docs/ble-observation-contract.md)
and [streaming](../docs/firmware-streaming.md).

The compiler's current positive `anyOf`/`allOf` profile is intentionally narrower
than the legacy interchange schema. Edit the [canonical JSON](../signatures/builtin.v2.json),
not the generated Rust file. [Authoring rules](../docs/signature-packs.md) describe
revision identities, budgets and unsupported predicates.

## Runtime boundaries

No_std firmware runs cooperative tasks with static queues and bounded storage.
Radio callbacks must remain non-blocking. Changes to shared event types or task
buffers need both board builds and the [firmware memory check](../scripts/README.md).
Linked stack headroom is a layout guard, not a measured runtime high-water mark.

[ESP-IDF](../firmware-std/README.md) uses a separate runtime and heap-backed queue;
[hostd](../hostd/README.md) uses host OS adapters and async tasks. Neither removes
the need to preserve receiver capability, address type, truncation and freshness.
