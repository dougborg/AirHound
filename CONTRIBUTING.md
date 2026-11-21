# Contributing to AirHound

AirHound welcomes source-backed signature research, receiver support and improvements to the shared detection pipeline. Start with the [documentation index](docs/README.md) for the component you want to change.

## Ways to Contribute

Roughly ordered by accessibility:

1. **Report a device signature** — Open an [issue](https://github.com/dougborg/AirHound/issues) with sources, device/model context and sanitized advertisement examples. Partial evidence is useful; keep private device identifiers and captures out of public issues.
2. **Add signatures to the database** — Follow the [canonical pack workflow](docs/signature-packs.md) and open a PR. See [Adding Device Signatures](#adding-device-signatures) below.
3. **Add board support** — New ESP32 board? Add a feature flag in `Cargo.toml` and pin assignments in `src/board.rs`.
4. **Protocol improvements** — Changes to the NDJSON message format in `src/protocol.rs`. See [#9](https://github.com/dougborg/AirHound/issues/9).
5. **Bug fixes and code improvements** — Always welcome.
6. **Library module contributions** — Layer 2 modules ([#28](https://github.com/dougborg/AirHound/issues/28)–[#32](https://github.com/dougborg/AirHound/issues/32)) are pure Rust with no platform dependencies, fully testable on host with `cargo test`. See the [architecture vision](https://github.com/dougborg/AirHound/issues/17) for context.
7. **Desktop scanner and integrations** — Improve the existing macOS/Linux CLI in `hostd/`, add detector-device transport ingestion, or help with planned Kismet integration ([#12](https://github.com/dougborg/AirHound/issues/12)).
8. **Cross-project signature sharing** — Help bridge AirHound's portable signature format ([#11](https://github.com/dougborg/AirHound/issues/11)) to other detection tools ([#16](https://github.com/dougborg/AirHound/issues/16)).

## Development Setup

Use the [repository map](docs/repository-layout.md) to choose the right build root,
toolchain and lockfile before making changes.

### Docker (recommended)

No local ESP toolchain needed. Requires Docker and [`just`](https://github.com/casey/just).

```bash
cargo install just

just docker-build            # Build both targets
just docker-check            # Type-check only
just docker-test             # Run unit tests
just docker-clean            # Clean stale artifacts when needed
```

### Host library development

The portable library tests and formatter work with stable Rust; no ESP toolchain
or hardware is needed. The repository defaults to the `esp` toolchain for firmware,
so select stable explicitly for host work:

```bash
cargo +stable test --locked --lib --no-default-features
cargo +stable fmt --check
cargo +stable fmt --manifest-path firmware-std/Cargo.toml --check
node --test .github/scripts/preview-comment.test.js
```

### Desktop development and performance

`hostd/` selects stable Rust independently of the firmware toolchain. Install the
platform prerequisites in the [desktop guide](hostd/README.md), then run:

```sh
just test-hostd
just clippy-hostd
just fmt-hostd
python3 -B -m unittest discover -s scripts/tests -v
just bench-hostd
```

The benchmark is synthetic and needs no radio permissions. `just profile-hostd`
is a separate live macOS check requiring a connected radio and granted permissions.
See [testing and analysis](docs/testing.md) for coverage, fuzzing,
CI checks and hardware-validation limits. Keep raw captures, nearby identifiers
and device flash backups out of commits and CI artifacts.

### Native firmware

Requires the ESP Rust toolchain via [`espup`](https://github.com/esp-rs/espup).

```bash
cargo install espup --locked && espup install
. ~/export-esp.sh

just build-xiao
just build-m5stickc
just test                    # Run library tests using stable Rust
```

### Formatting

```bash
cargo +stable fmt --check    # Check
cargo +stable fmt            # Fix
```

## Commit Conventions

This project uses [Conventional Commits](https://www.conventionalcommits.org/). Run `just setup-hooks` to install git hooks that enforce the format.

Examples:

```
feat: add Verkada MAC OUI prefixes
fix: handle truncated BLE advertisement packets
feat(m5stickc): add battery voltage display
docs: update signature counts in README
chore: bump esp-hal to latest git main
```

Common prefixes: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`.

## Pull Request Process

- **One logical change per PR.** A signature batch is one change; a signature batch plus a refactor is two PRs.
- **CI must pass.** The PR pipeline runs formatting checks and tests on every push.
- **Describe what and why.** The PR description should explain what changed and why, not just what files were touched.

## Adding Device Signatures

Start with [signature packs, research and admission](signatures/README.md).
Production defaults come from `signatures/builtin.v2.json`; do not edit
`src/generated_signatures.rs` directly. `src/defaults.rs` contains helpers and
includes the generated tables. The legacy v1 examples are not compiler input.

1. Record the primary source, model/firmware/mode, transport and known limits.
   Distinguish a vendor or protocol clue from device identity.
2. Add positive and ordinary-device negative fixtures. Include malformed,
   truncated, wrong-company/UUID and address-type cases where relevant.
3. Use the supported [compiled profile](docs/signature-packs.md#explicit-compiled-profile).
   Unsupported predicates must be rejected, not approximated by weaker rules.
4. Preserve published numeric-ID meaning. Behavior or label changes require the
   appropriate new pack/ruleset identity and revision-registry entry.
5. Regenerate and validate from the repository root with `check-jsonschema`
   installed in your Python environment:

   ```sh
   just generate-signatures
   just check-signatures
   just check-json
   just check-schemas
   just check-examples
   python3 -m unittest discover -s signatures/tests -v
   cargo +stable test --locked --lib --no-default-features
   ```

Research entries remain disabled until reviewed. A source reference or a
`device_validated` metadata flag alone does not validate a claim. The
[admission policy](signatures/README.md#admission-policy) covers provenance,
licensing, false positives and promotion. Review alert behavior independently
from signature matching; broad vendor/name/service clues should not become
confident device alerts.

## Documentation changes

Keep the root README focused on choosing a component. Put setup and usage in the
component README, and detailed contracts or measurement reports under `docs/`.
Link new guides from [docs/README.md](docs/README.md), and preserve existing paths
or provide links when moving material. Verify commands against `justfile`, CLI
help and the current source. Describe hardware support, measured validation and
planned integration separately.
