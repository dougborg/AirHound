# Contributing to AirHound

AirHound is a community-driven project. The most impactful contribution you can make is adding new device signatures — every new MAC prefix, SSID pattern, or BLE identifier helps the entire detection community.

## Ways to Contribute

Roughly ordered by accessibility:

1. **Report a device signature** — Open an [issue](https://github.com/dougborg/AirHound/issues) with whatever data you have: MAC address, SSID, BLE advertisement name, manufacturer ID. Partial data is fine.
2. **Add signatures to the database** — Edit `src/defaults.rs` and open a PR. See [Adding Device Signatures](#adding-device-signatures) below.
3. **Add board support** — New ESP32 board? Add a feature flag in `Cargo.toml` and pin assignments in `src/board.rs`.
4. **Protocol improvements** — Changes to the NDJSON message format in `src/protocol.rs`. See [#9](https://github.com/dougborg/AirHound/issues/9).
5. **Bug fixes and code improvements** — Always welcome.

## Development Setup

### Docker (recommended)

No local toolchain needed. Requires Docker and [`just`](https://github.com/casey/just).

```bash
cargo install just

just docker-build            # Build both targets
just docker-check            # Type-check only
just docker-test             # Run unit tests
just docker-clean            # Clean (required after dependency changes)
```

### Native

Requires the ESP Rust toolchain via [`espup`](https://github.com/esp-rs/espup).

```bash
cargo install espup --locked && espup install
. ~/export-esp.sh

just build-xiao
just build-m5stickc
just test                    # Run unit tests (requires nightly)
```

### Formatting

```bash
cargo fmt --check            # Check (requires nightly)
cargo fmt                    # Fix
```

## Commit Conventions

This project uses [Conventional Commits](https://www.conventionalcommits.org/). Run `just setup-hooks` to install git hooks that enforce the format.

Examples:

```
feat: add Verkada MAC OUI prefixes
fix: handle truncated BLE advertisement packets
feat(m5stickc): add battery voltage display
docs: update filter data counts in README
chore: bump esp-hal to latest git main
```

Common prefixes: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`.

## Pull Request Process

- **One logical change per PR.** A signature batch is one change; a signature batch plus a refactor is two PRs.
- **CI must pass.** The PR pipeline runs formatting checks and tests on every push.
- **Describe what and why.** The PR description should explain what changed and why, not just what files were touched.

## Adding Device Signatures

The signature database lives in [`src/defaults.rs`](src/defaults.rs). There are several types of signatures you can add:

### MAC OUI Prefixes

Add entries to `MAC_PREFIXES`. Group by vendor and add a comment header for new vendors:

```rust
// === Verkada ===
([0xAA, 0xBB, 0xCC], "Verkada"),
([0xDD, 0xEE, 0xFF], "Verkada"),
```

Find OUI assignments at [Wireshark OUI Lookup](https://www.wireshark.org/tools/oui-lookup.html) or the [IEEE OUI database](https://standards-oui.ieee.org/).

### SSID Patterns

For SSIDs with a fixed prefix and variable suffix, add a `SsidPattern` to `SSID_PATTERNS`:

```rust
SsidPattern {
    prefix: "Verkada-",
    suffix_len: 6,
    suffix_kind: SuffixKind::HexChars,
    description: "Verkada camera WiFi",
},
```

For exact SSID matches, add to `SSID_EXACT`. For case-insensitive substring matches, add to `SSID_KEYWORDS`.

### BLE Identifiers

- **Device names** — Add to `BLE_NAME_PATTERNS` (case-insensitive substring match)
- **Service UUIDs** — Add 16-bit short UUIDs to `BLE_SERVICE_UUIDS_16` or `BLE_STANDARD_UUIDS_16`
- **Manufacturer IDs** — Add company IDs to `BLE_MANUFACTURER_IDS` (find these in BLE advertisement data or the [Bluetooth SIG company list](https://www.bluetooth.com/specifications/assigned-numbers/))

### Guidelines

- **Cite your source.** Add a code comment or mention in the PR where the signature came from (Wireshark capture, another project's database, FCC filing, etc.).
- **Group by vendor.** Keep entries organized under vendor comment headers.
- **Test your changes.** Run `just docker-test` (or `just test` natively) — the unit tests verify that filter matching works correctly.
- **Update counts.** If you add MAC prefixes, update the count in `README.md` under Filter Data.
